// #![windows_subsystem = "windows"]

mod config;
mod keyring_manager;
mod tunnel;

use config::{
    AppConfig, TunnelConfig, load_app_config, load_configs, save_app_config, save_configs,
};
use slint::{ModelRc, SharedString, VecModel};
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
        target_host: s2r(d.target_host.clone()),
        target_port: s2r(d.target_port.clone()).parse().unwrap_or(80),
        auto_connect: d.auto_connect,
    }
}

fn app_config_to_settings(c: &AppConfig) -> AppSettings {
    AppSettings {
        connection_timeout: r2s(&c.connection_timeout.to_string()),
        minimize_to_tray: c.minimize_to_tray,
        start_on_boot: c.start_on_boot,
    }
}

fn settings_to_app_config(s: &AppSettings) -> AppConfig {
    AppConfig {
        connection_timeout: s2r(s.connection_timeout.clone()).parse().unwrap_or(10),
        minimize_to_tray: s.minimize_to_tray,
        start_on_boot: s.start_on_boot,
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

    let app_config = load_app_config();
    app.set_settings(app_config_to_settings(&app_config));

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
    app.on_update_search(move || {
        if let Some(app) = app_weak.upgrade() {
            let query = app.get_search_text().to_string().to_lowercase();
            let st = state_clone.lock().unwrap();
            let mut new_data = Vec::new();
            for c in &st.configs {
                if query.is_empty() || c.name.to_lowercase().contains(&query) {
                    let is_running = st.running_tunnels.contains_key(&c.id);
                    new_data.push(config_to_data(c, is_running));
                }
            }
            tunnels_model_clone.set_vec(new_data);
        }
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_create_new(move || {
        let mut st = state_clone.lock().unwrap();
        let new_c = TunnelConfig::default();
        st.configs.push(new_c.clone());
        let _ = save_configs(&st.configs);
        drop(st);

        if let Some(app) = app_weak.upgrade() {
            app.set_search_text("".into());
            app.invoke_update_search();
            app.set_selected_id(r2s(&new_c.id));
            app.set_edit_data(config_to_data(&new_c, false));
        }
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_save_config(move |data: TunnelData| {
        let mut st = state_clone.lock().unwrap();
        let updated = data_to_config(&data);

        if !updated.save_proxy_password {
            let _ = keyring_manager::delete_password(
                &keyring_manager::get_proxy_service_name(&updated.id),
                &updated.proxy_username,
            );
        }

        if let Some(pos) = st.configs.iter().position(|c| c.id == updated.id) {
            st.configs[pos] = updated.clone();
        }
        let _ = save_configs(&st.configs);
        drop(st);

        if let Some(app) = app_weak.upgrade() {
            app.set_selected_id("".into());
            app.invoke_update_search();
        }
    });

    app.on_save_settings(move |settings: AppSettings| {
        let updated = settings_to_app_config(&settings);
        let _ = save_app_config(&updated);
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_select_tunnel(move |id: SharedString| {
        if let Some(app) = app_weak.upgrade() {
            let st = state_clone.lock().unwrap();
            let id = s2r(id);
            if let Some(c) = st.configs.iter().find(|x| x.id == id) {
                app.set_selected_id(r2s(&id));
                let is_running = st.running_tunnels.contains_key(&c.id);
                app.set_edit_data(config_to_data(c, is_running));
            }
        }
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_toggle_auto_connect(move |id: SharedString, auto_connect: bool| {
        let mut st = state_clone.lock().unwrap();
        let id_str = s2r(id);
        if let Some(pos) = st.configs.iter().position(|x| x.id == id_str) {
            st.configs[pos].auto_connect = auto_connect;
            let _ = save_configs(&st.configs);
        }
        drop(st);
        if let Some(app) = app_weak.upgrade() {
            app.invoke_update_search();
        }
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_copy_tunnel(move |id: SharedString| {
        let mut st = state_clone.lock().unwrap();
        let id_str = s2r(id);
        if let Some(pos) = st.configs.iter().position(|x| x.id == id_str) {
            let mut new_c = st.configs[pos].clone();
            new_c.id = uuid::Uuid::new_v4().to_string();
            new_c.name = format!("{} (Copy)", new_c.name);
            st.configs.push(new_c);
            let _ = save_configs(&st.configs);
        }
        drop(st);
        if let Some(app) = app_weak.upgrade() {
            app.invoke_update_search();
        }
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_delete_tunnel(move |id: SharedString| {
        let mut st = state_clone.lock().unwrap();
        let id_str = s2r(id);
        if let Some(pos) = st.configs.iter().position(|x| x.id == id_str) {
            let c = &st.configs[pos];
            let _ = keyring_manager::delete_password(
                &keyring_manager::get_proxy_service_name(&id_str),
                &c.proxy_username,
            );
            st.configs.remove(pos);
            let _ = save_configs(&st.configs);

            if let Some(running) = st.running_tunnels.remove(&id_str) {
                running.store(false, Ordering::Relaxed);
            }
        }
        drop(st);
        if let Some(app) = app_weak.upgrade() {
            app.invoke_update_search();
        }
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_start_tunnel(move |id: SharedString| {
        let id = s2r(id);

        let (needs_proxy, proxy_pass) = {
            let st = state_clone.lock().unwrap();
            if let Some(c) = st.configs.iter().find(|x| x.id == id) {
                let proxy_user = c.proxy_username.clone();

                let proxy_pass = keyring_manager::get_password(
                    &keyring_manager::get_proxy_service_name(&id),
                    &proxy_user,
                )
                .unwrap_or_default();

                let needs_proxy = proxy_pass.is_empty();

                (needs_proxy, proxy_pass)
            } else {
                return;
            }
        };

        if let Some(app) = app_weak.upgrade() {
            if needs_proxy {
                app.set_prompt_tunnel_id(r2s(&id));
                app.set_prompt_needs_proxy(needs_proxy);
                app.set_show_password_prompt(true);
            } else {
                app.invoke_submit_passwords(r2s(&id), r2s(&proxy_pass));
            }
        }
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_submit_passwords(move |id: SharedString, p_pass: SharedString| {
        let id = s2r(id);
        let mut p_pass = s2r(p_pass);

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

            let is_running = Arc::new(AtomicBool::new(true));
            st.running_tunnels.insert(id.clone(), is_running.clone());

            let id_clone = id.clone();
            let app_w = app_weak.clone();
            thread::spawn(move || {
                let result = tunnel::start_tunnel(c, p_pass, is_running);

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
        }
        drop(st);
        if let Some(app) = app_weak.upgrade() {
            app.invoke_update_search();
        }
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_stop_tunnel(move |id: SharedString| {
        let id = s2r(id);
        let mut st = state_clone.lock().unwrap();
        if let Some(running) = st.running_tunnels.remove(&id) {
            running.store(false, Ordering::Relaxed);
        }
        drop(st);
        if let Some(app) = app_weak.upgrade() {
            app.invoke_update_search();
        }
    });

    // Auto Connect
    let auto_connect_ids: Vec<String> = {
        let st = state.lock().unwrap();
        st.configs
            .iter()
            .filter(|c| c.auto_connect)
            .map(|c| c.id.clone())
            .collect()
    };
    for id in auto_connect_ids {
        app.invoke_start_tunnel(r2s(&id));
    }

    // System Tray
    let _tray = AppTray::new().ok().inspect(|tray| {
        let app_weak_tray = app.as_weak();
        tray.on_show_window(move || {
            if let Some(app) = app_weak_tray.upgrade() {
                let _ = app.window().show();
            }
        });
        tray.on_quit(|| {
            slint::quit_event_loop().unwrap();
        });
    });

    let app_weak2 = app.as_weak();
    app.on_show_window(move || {
        if let Some(app) = app_weak2.upgrade() {
            let _ = app.window().show();
        }
    });

    app.window().show().unwrap();
    app.run().unwrap();
}
