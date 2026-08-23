// #![windows_subsystem = "windows"]

mod config;
mod keyring_manager;
mod tunnel;

use config::{TunnelConfig, load_configs, save_configs};
use slint::{Model, ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

slint::include_modules!();

fn s2r(s: SharedString) -> String {
    s.to_string()
}
fn r2s(s: &str) -> SharedString {
    SharedString::from(s)
}

fn config_to_data(c: &TunnelConfig, is_running: bool) -> TunnelData {
    TunnelData {
        id: r2s(&c.id),
        name: r2s(&c.name),
        local_port: r2s(&c.local_port.to_string()),
        proxy_host: r2s(&c.proxy_host),
        proxy_port: r2s(&c.proxy_port.to_string()),
        proxy_username: r2s(&c.proxy_username),
        save_proxy_password: c.save_proxy_password,
        remote_host: r2s(&c.remote_host),
        remote_port: r2s(&c.remote_port.to_string()),
        remote_username: r2s(&c.remote_username),
        save_remote_password: c.save_remote_password,
        target_host: r2s(&c.target_host),
        target_port: r2s(&c.target_port.to_string()),
        auto_connect: c.auto_connect,
        is_running,
        uptime: r2s(if is_running {
            "1h 45m"
        } else {
            "Never Connected"
        }),
        tx_rx: r2s(if is_running {
            "15.2MB / 2.1MB"
        } else {
            "Never Connected"
        }),
        signal: if is_running { 3 } else { 0 },
    }
}

fn data_to_config(d: &TunnelData) -> TunnelConfig {
    TunnelConfig {
        id: s2r(d.id.clone()),
        name: s2r(d.name.clone()),
        local_port: s2r(d.local_port.clone()).parse().unwrap_or(8080),
        proxy_host: s2r(d.proxy_host.clone()),
        proxy_port: s2r(d.proxy_port.clone()).parse().unwrap_or(22),
        proxy_username: s2r(d.proxy_username.clone()),
        save_proxy_password: d.save_proxy_password,
        remote_host: s2r(d.remote_host.clone()),
        remote_port: s2r(d.remote_port.clone()).parse().unwrap_or(22),
        remote_username: s2r(d.remote_username.clone()),
        save_remote_password: d.save_remote_password,
        target_host: s2r(d.target_host.clone()),
        target_port: s2r(d.target_port.clone()).parse().unwrap_or(80),
        auto_connect: d.auto_connect,
    }
}

struct AppState {
    configs: Vec<TunnelConfig>,
    running_tunnels: HashMap<String, Arc<AtomicBool>>,
}

fn main() {
    let app = AppWindow::new().unwrap();

    // Slint window hiding on close
    app.window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);

    let state = Rc::new(Mutex::new(AppState {
        configs: load_configs(),
        running_tunnels: HashMap::new(),
    }));

    let tunnels_model = Rc::new(VecModel::default());
    app.set_tunnels(ModelRc::from(tunnels_model.clone()));

    // Initial load
    {
        let st = state.lock().unwrap();
        for c in &st.configs {
            tunnels_model.push(config_to_data(c, false));
        }
    }

    // Callbacks
    let state_clone = state.clone();
    let tunnels_model_clone = tunnels_model.clone();
    let app_weak = app.as_weak();
    app.on_create_new(move || {
        let mut st = state_clone.lock().unwrap();
        let new_c = TunnelConfig::default();
        st.configs.push(new_c.clone());
        tunnels_model_clone.push(config_to_data(&new_c, false));
        let _ = save_configs(&st.configs);

        if let Some(app) = app_weak.upgrade() {
            let new_idx = (st.configs.len() - 1) as i32;
            app.set_selected_idx(new_idx);
            app.set_edit_data(config_to_data(&new_c, false));
        }
    });

    let state_clone = state.clone();
    let tunnels_model_clone = tunnels_model.clone();
    app.on_save_config(move |data: TunnelData| {
        let mut st = state_clone.lock().unwrap();
        let updated = data_to_config(&data);

        if !updated.save_proxy_password {
            let _ = keyring_manager::delete_password(
                &keyring_manager::get_proxy_service_name(&updated.id),
                &updated.proxy_username,
            );
        }

        if !updated.save_remote_password {
            let _ = keyring_manager::delete_password(
                &keyring_manager::get_remote_service_name(&updated.id),
                &updated.remote_username,
            );
        }

        if let Some(pos) = st.configs.iter().position(|c| c.id == updated.id) {
            st.configs[pos] = updated.clone();
            let is_running = st.running_tunnels.contains_key(&updated.id);
            tunnels_model_clone.set_row_data(pos, config_to_data(&updated, is_running));
        }
        let _ = save_configs(&st.configs);
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_select_tunnel(move |idx: i32| {
        if let Some(app) = app_weak.upgrade() {
            let st = state_clone.lock().unwrap();
            if idx >= 0 && (idx as usize) < st.configs.len() {
                app.set_selected_idx(idx);
                let c = &st.configs[idx as usize];
                let is_running = st.running_tunnels.contains_key(&c.id);
                app.set_edit_data(config_to_data(c, is_running));
            }
        }
    });

    let state_clone = state.clone();
    let tunnels_model_clone = tunnels_model.clone();
    app.on_toggle_auto_connect(move |idx: i32, auto_connect: bool| {
        let mut st = state_clone.lock().unwrap();
        if idx >= 0 && (idx as usize) < st.configs.len() {
            st.configs[idx as usize].auto_connect = auto_connect;
            let c = &st.configs[idx as usize];
            let is_running = st.running_tunnels.contains_key(&c.id);
            tunnels_model_clone.set_row_data(idx as usize, config_to_data(c, is_running));
            let _ = save_configs(&st.configs);
        }
    });

    let state_clone = state.clone();
    let tunnels_model_clone = tunnels_model.clone();
    app.on_copy_tunnel(move |idx: i32| {
        let mut st = state_clone.lock().unwrap();
        if idx >= 0 && (idx as usize) < st.configs.len() {
            let mut new_c = st.configs[idx as usize].clone();
            new_c.id = uuid::Uuid::new_v4().to_string();
            new_c.name = format!("{} (Copy)", new_c.name);

            st.configs.push(new_c.clone());
            tunnels_model_clone.push(config_to_data(&new_c, false));
            let _ = save_configs(&st.configs);
        }
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_start_tunnel(move |id: SharedString| {
        let id = s2r(id);

        let (needs_proxy, needs_remote, proxy_pass, remote_pass) = {
            let st = state_clone.lock().unwrap();
            if let Some(c) = st.configs.iter().find(|x| x.id == id) {
                let proxy_user = c.proxy_username.clone();
                let remote_user = c.remote_username.clone();

                let proxy_pass = keyring_manager::get_password(
                    &keyring_manager::get_proxy_service_name(&id),
                    &proxy_user,
                )
                .unwrap_or_default();
                let remote_pass = keyring_manager::get_password(
                    &keyring_manager::get_remote_service_name(&id),
                    &remote_user,
                )
                .unwrap_or_default();

                let needs_proxy = proxy_pass.is_empty();
                let needs_remote = remote_pass.is_empty();

                (needs_proxy, needs_remote, proxy_pass, remote_pass)
            } else {
                return;
            }
        };

        if let Some(app) = app_weak.upgrade() {
            if needs_proxy || needs_remote {
                app.set_prompt_tunnel_id(r2s(&id));
                app.set_prompt_needs_proxy(needs_proxy);
                app.set_prompt_needs_remote(needs_remote);
                app.set_show_password_prompt(true);
            } else {
                app.invoke_submit_passwords(r2s(&id), r2s(&proxy_pass), r2s(&remote_pass));
            }
        }
    });

    let state_clone = state.clone();
    let tunnels_model_clone = tunnels_model.clone();
    let app_weak = app.as_weak();
    app.on_submit_passwords(
        move |id: SharedString, p_pass: SharedString, r_pass: SharedString| {
            let id = s2r(id);
            let mut p_pass = s2r(p_pass);
            let mut r_pass = s2r(r_pass);

            let mut st = state_clone.lock().unwrap();
            if let Some(c) = st.configs.iter().find(|x| x.id == id).cloned() {
                if p_pass.is_empty() {
                    p_pass = keyring_manager::get_password(
                        &keyring_manager::get_proxy_service_name(&id),
                        &c.proxy_username,
                    )
                    .unwrap_or_default();
                } else if c.save_proxy_password {
                    let _ = keyring_manager::save_password(
                        &keyring_manager::get_proxy_service_name(&id),
                        &c.proxy_username,
                        &p_pass,
                    );
                }

                if r_pass.is_empty() {
                    r_pass = keyring_manager::get_password(
                        &keyring_manager::get_remote_service_name(&id),
                        &c.remote_username,
                    )
                    .unwrap_or_default();
                } else if c.save_remote_password {
                    let _ = keyring_manager::save_password(
                        &keyring_manager::get_remote_service_name(&id),
                        &c.remote_username,
                        &r_pass,
                    );
                }

                let is_running = Arc::new(AtomicBool::new(true));
                st.running_tunnels.insert(id.clone(), is_running.clone());

                let id_clone = id.clone();
                let app_w = app_weak.clone();
                thread::spawn(move || {
                    let result = tunnel::start_tunnel(c, p_pass, r_pass, is_running);

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_w.upgrade() {
                            if let Err(e) = result {
                                app.set_error_message(r2s(&e));
                                app.set_show_error_prompt(true);
                            }
                            app.invoke_stop_tunnel(r2s(&id_clone));
                        }
                    });
                });

                if let Some(pos) = st.configs.iter().position(|x| x.id == id) {
                    let c_ref = &st.configs[pos];
                    tunnels_model_clone.set_row_data(pos, config_to_data(c_ref, true));
                    if let Some(app) = app_weak.upgrade()
                        && app.get_selected_idx() == pos as i32
                    {
                        app.set_edit_data(config_to_data(c_ref, true));
                    }
                }
            }
        },
    );

    let state_clone = state.clone();
    let tunnels_model_clone = tunnels_model.clone();
    let app_weak = app.as_weak();
    app.on_stop_tunnel(move |id: SharedString| {
        let id = s2r(id);
        let mut st = state_clone.lock().unwrap();
        if let Some(running) = st.running_tunnels.remove(&id) {
            running.store(false, Ordering::Relaxed);
        }

        if let Some(pos) = st.configs.iter().position(|x| x.id == id) {
            let c_ref = &st.configs[pos];
            tunnels_model_clone.set_row_data(pos, config_to_data(c_ref, false));
            if let Some(app) = app_weak.upgrade()
                && app.get_selected_idx() == pos as i32
            {
                app.set_edit_data(config_to_data(c_ref, false));
            }
        }
    });

    // Auto Connect — collect IDs first so the lock is dropped before invoking,
    // because invoke_start_tunnel synchronously calls on_start_tunnel which also
    // needs to acquire state.lock() → deadlock if we hold it here.
    let auto_connect_ids: Vec<String> = {
        let st = state.lock().unwrap();
        st.configs
            .iter()
            .filter(|c| c.auto_connect)
            .map(|c| c.id.clone())
            .collect()
    }; // lock dropped here
    for id in auto_connect_ids {
        app.invoke_start_tunnel(r2s(&id));
    }

    // System Tray: Slint's native SystemTrayIcon — integrates cleanly with the event loop
    let tray = AppTray::new().unwrap();
    let app_weak_tray = app.as_weak();
    tray.on_show_window(move || {
        if let Some(app) = app_weak_tray.upgrade() {
            let _ = app.window().show();
        }
    });
    tray.on_quit(|| {
        slint::quit_event_loop().unwrap();
    });

    let app_weak2 = app.as_weak();
    app.on_show_window(move || {
        if let Some(app) = app_weak2.upgrade() {
            let _ = app.window().show();
        }
    });

    // Show the main window — required when SystemTrayIcon is present.
    app.window().show().unwrap();

    app.run().unwrap();
}
