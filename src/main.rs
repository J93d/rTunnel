#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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
use std::thread::{self, JoinHandle};
use zeroize::Zeroizing;

slint::include_modules!();

fn s2r(s: SharedString) -> String {
    s.to_string()
}
fn r2s(s: &str) -> SharedString {
    SharedString::from(s)
}

fn config_to_data(c: &TunnelConfig, running_info: Option<&tunnel::TunnelTelemetry>) -> TunnelData {
    let is_running = running_info.is_some();
    let (uptime_str, tx_rx_str, signal_val) = if let Some(tel) = running_info {
        let elapsed = tel.start_time.elapsed().as_secs();
        let hours = elapsed / 3600;
        let mins = (elapsed % 3600) / 60;
        let secs = elapsed % 60;
        let uptime = if hours > 0 {
            format!("{}h {}m {}s", hours, mins, secs)
        } else if mins > 0 {
            format!("{}m {}s", mins, secs)
        } else {
            format!("{}s", secs)
        };

        let tx = tel.tx_bytes.load(Ordering::Relaxed) as f64;
        let rx = tel.rx_bytes.load(Ordering::Relaxed) as f64;

        fn format_bytes(b: f64) -> String {
            if b >= 1_048_576.0 {
                format!("{:.1} MB", b / 1_048_576.0)
            } else if b >= 1024.0 {
                format!("{:.1} KB", b / 1024.0)
            } else {
                format!("{} B", b)
            }
        }

        let tx_rx = format!("{} / {}", format_bytes(rx), format_bytes(tx));
        let signal = if tx > 0.0 || rx > 0.0 { 4 } else { 3 };
        (r2s(&uptime), r2s(&tx_rx), signal)
    } else {
        (r2s("Never Connected"), r2s("Never Connected"), 0)
    };

    TunnelData {
        id: r2s(&c.id),
        name: r2s(&c.name),
        local_port: r2s(&c.local_port.to_string()),
        proxy_host: r2s(&c.proxy_host),
        proxy_port: r2s(&c.proxy_port.to_string()),
        proxy_username: r2s(&c.proxy_username),
        save_proxy_password: c.save_proxy_password,
        rsa_key_path: r2s(&c.rsa_key_path),
        target_host: r2s(&c.target_host),
        target_port: r2s(&c.target_port.to_string()),
        auto_connect: c.auto_connect,
        is_running,
        uptime: uptime_str,
        tx_rx: tx_rx_str,
        signal: signal_val,
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
        rsa_key_path: s2r(d.rsa_key_path.clone()),
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

type TunnelState = (
    Arc<AtomicBool>,
    Arc<tunnel::TunnelTelemetry>,
    Option<JoinHandle<()>>,
);

struct AppState {
    configs: Vec<TunnelConfig>,
    running_tunnels: HashMap<String, TunnelState>,
}

fn main() {
    let app = AppWindow::new().unwrap_or_else(|e| {
        eprintln!("Fatal: Failed to create application window: {}", e);
        std::process::exit(1);
    });

    let app_config = load_app_config();
    update_auto_start(app_config.start_on_boot);

    let app_weak_close = app.as_weak();
    // Slint window hiding on close
    app.window().on_close_requested(move || {
        if let Some(app) = app_weak_close.upgrade()
            && app.get_settings().minimize_to_tray
        {
            return slint::CloseRequestResponse::HideWindow;
        }
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::KeepWindowShown
    });

    let state = Arc::new(Mutex::new(AppState {
        configs: load_configs(),
        running_tunnels: HashMap::new(),
    }));

    app.set_settings(app_config_to_settings(&app_config));

    let tunnels_model = Rc::new(VecModel::default());
    app.set_tunnels(ModelRc::from(tunnels_model.clone()));

    // Initial load
    {
        let st = state.lock().unwrap_or_else(|p| p.into_inner());
        for c in &st.configs {
            tunnels_model.push(config_to_data(c, None));
        }
        let active = st.running_tunnels.len() as i32;
        let total = st.configs.len() as i32;
        app.set_active_tunnels_count(active);
        app.set_stopped_tunnels_count(total - active);
    }

    // Callbacks
    let state_clone = state.clone();
    let tunnels_model_clone = tunnels_model.clone();
    let app_weak = app.as_weak();
    app.on_update_search(move || {
        if let Some(app) = app_weak.upgrade() {
            let query = app.get_search_text().to_string().to_lowercase();
            let st = state_clone.lock().unwrap_or_else(|p| p.into_inner());
            let mut new_data = Vec::new();
            for c in &st.configs {
                if query.is_empty() || c.name.to_lowercase().contains(&query) {
                    let info = st
                        .running_tunnels
                        .get(&c.id)
                        .map(|(_, tel, _)| tel.as_ref());
                    new_data.push(config_to_data(c, info));
                }
            }
            let active = st.running_tunnels.len() as i32;
            let total = st.configs.len() as i32;
            app.set_active_tunnels_count(active);
            app.set_stopped_tunnels_count(total - active);
            tunnels_model_clone.set_vec(new_data);
        }
    });

    let app_weak_tray = app.as_weak();
    app.on_minimize_to_tray_clicked(move || {
        if let Some(app) = app_weak_tray.upgrade() {
            let _ = app.window().hide();
        }
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_create_new(move || {
        let mut st = state_clone.lock().unwrap_or_else(|p| p.into_inner());
        let new_c = TunnelConfig::default();
        st.configs.push(new_c.clone());
        let _ = save_configs(&st.configs);
        drop(st);

        if let Some(app) = app_weak.upgrade() {
            app.set_search_text("".into());
            app.invoke_update_search();
            app.set_selected_id(r2s(&new_c.id));
            app.set_edit_data(config_to_data(&new_c, None));
        }
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_save_config(move |data: TunnelData| {
        let mut st = state_clone.lock().unwrap_or_else(|p| p.into_inner());
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
        update_auto_start(updated.start_on_boot);
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_select_tunnel(move |id: SharedString| {
        if let Some(app) = app_weak.upgrade() {
            let st = state_clone.lock().unwrap_or_else(|p| p.into_inner());
            let id = s2r(id);
            if let Some(c) = st.configs.iter().find(|x| x.id == id) {
                app.set_selected_id(r2s(&id));
                let info = st
                    .running_tunnels
                    .get(&c.id)
                    .map(|(_, tel, _)| tel.as_ref());
                app.set_edit_data(config_to_data(c, info));
            }
        }
    });

    let state_clone = state.clone();
    let app_weak = app.as_weak();
    app.on_toggle_auto_connect(move |id: SharedString, auto_connect: bool| {
        let mut st = state_clone.lock().unwrap_or_else(|p| p.into_inner());
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
        let mut st = state_clone.lock().unwrap_or_else(|p| p.into_inner());
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
        let mut st = state_clone.lock().unwrap_or_else(|p| p.into_inner());
        let id_str = s2r(id);
        if let Some(pos) = st.configs.iter().position(|x| x.id == id_str) {
            let c = &st.configs[pos];
            let _ = keyring_manager::delete_password(
                &keyring_manager::get_proxy_service_name(&id_str),
                &c.proxy_username,
            );
            st.configs.remove(pos);
            let _ = save_configs(&st.configs);

            if let Some((running, _, handle)) = st.running_tunnels.remove(&id_str) {
                running.store(false, Ordering::Relaxed);
                if let Some(h) = handle {
                    let _ = h.join();
                }
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
            let st = state_clone.lock().unwrap_or_else(|p| p.into_inner());
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

        let mut st = state_clone.lock().unwrap_or_else(|p| p.into_inner());
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
            let telemetry = Arc::new(tunnel::TunnelTelemetry {
                start_time: std::time::Instant::now(),
                tx_bytes: std::sync::atomic::AtomicU64::new(0),
                rx_bytes: std::sync::atomic::AtomicU64::new(0),
            });
            let timeout = load_app_config().connection_timeout;

            let is_running_clone = is_running.clone();
            let telemetry_clone = telemetry.clone();
            let p_pass_zero = Zeroizing::new(p_pass);

            let id_clone = id.clone();
            let app_w = app_weak.clone();
            let handle = thread::spawn(move || {
                let result = tunnel::start_tunnel(
                    c,
                    p_pass_zero,
                    is_running_clone,
                    telemetry_clone,
                    timeout,
                );

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = app_w.upgrade() {
                        match result {
                            Err(tunnel::TunnelError::UnknownHostKey(fingerprint, b64_line)) => {
                                app.set_prompt_host_key_fingerprint(r2s(&fingerprint));
                                app.set_prompt_host_key_line(r2s(&b64_line));
                                app.set_prompt_host_key_tunnel_id(r2s(&id_clone));
                                app.set_show_host_key_prompt(true);
                            }
                            Err(tunnel::TunnelError::Message(e)) => {
                                app.set_error_message(r2s(&e));
                                app.set_show_error_prompt(true);
                            }
                            Ok(_) => {}
                        }
                        app.invoke_stop_tunnel(r2s(&id_clone));
                    }
                });
            });

            st.running_tunnels
                .insert(id.clone(), (is_running, telemetry, Some(handle)));
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
        let mut st = state_clone.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((running, _, handle)) = st.running_tunnels.remove(&id) {
            running.store(false, Ordering::Relaxed);
            if let Some(h) = handle {
                let _ = h.join();
            }
        }
        drop(st);
        if let Some(app) = app_weak.upgrade() {
            app.invoke_update_search();
        }
    });

    let app_weak_tofu = app.as_weak();
    app.on_accept_host_key(move |id: SharedString, b64_line: SharedString| {
        let b64 = s2r(b64_line);
        let id_str = s2r(id);

        if let Some(home) = dirs::home_dir() {
            let ssh_dir = home.join(".ssh");
            let _ = std::fs::create_dir_all(&ssh_dir);
            let kh_path = ssh_dir.join("known_hosts");
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(kh_path)
            {
                let _ = writeln!(file, "{}", b64);
            }
        }

        if let Some(app) = app_weak_tofu.upgrade() {
            app.set_show_host_key_prompt(false);
            app.invoke_start_tunnel(r2s(&id_str));
        }
    });

    let app_weak_tofu_rej = app.as_weak();
    app.on_reject_host_key(move || {
        if let Some(app) = app_weak_tofu_rej.upgrade() {
            app.set_show_host_key_prompt(false);
        }
    });

    // Telemetry timer update
    let app_weak_timer = app.as_weak();
    let state_clone_timer = state.clone();
    let tunnels_model_timer = tunnels_model.clone();
    let _telemetry_timer = slint::Timer::default();
    _telemetry_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(1000),
        move || {
            if let Some(app) = app_weak_timer.upgrade() {
                let st = state_clone_timer.lock().unwrap_or_else(|p| p.into_inner());
                let query = app.get_search_text().to_string().to_lowercase();
                let mut new_data = Vec::new();
                let mut need_update = false;
                for c in &st.configs {
                    if query.is_empty() || c.name.to_lowercase().contains(&query) {
                        let info = st
                            .running_tunnels
                            .get(&c.id)
                            .map(|(_, tel, _)| tel.as_ref());
                        if info.is_some() {
                            need_update = true;
                        }
                        new_data.push(config_to_data(c, info));
                    }
                }
                let active = st.running_tunnels.len() as i32;
                let total = st.configs.len() as i32;
                app.set_active_tunnels_count(active);
                app.set_stopped_tunnels_count(total - active);

                if need_update {
                    tunnels_model_timer.set_vec(new_data);
                }
            }
        },
    );

    // Auto Connect
    let auto_connect_ids: Vec<String> = {
        let st = state.lock().unwrap_or_else(|p| p.into_inner());
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

    app.window().show().unwrap_or_else(|e| {
        eprintln!("Fatal: Failed to show window: {}", e);
        std::process::exit(1);
    });
    if let Err(e) = app.run() {
        eprintln!("Fatal: Event loop error: {}", e);
        std::process::exit(1);
    }
}

fn update_auto_start(start_on_boot: bool) {
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_str) = exe_path.to_str()
    {
        let app_name = "rTunnel";
        let auto = auto_launcher::AutoLaunch::new(
            app_name,
            exe_str,
            auto_launcher::WindowsEnableMode::CurrentUser,
            &[] as &[&str],
        );
        if start_on_boot {
            let _ = auto.enable();
        } else {
            let _ = auto.disable();
        }
    }
}
