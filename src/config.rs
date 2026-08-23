use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TunnelConfig {
    pub id: String,
    pub name: String,
    pub local_port: u16,

    // Proxy config
    pub proxy_host: String,
    pub proxy_port: u16,
    pub proxy_username: String,
    pub save_proxy_password: bool,

    // Remote config
    pub remote_host: String,
    pub remote_port: u16,
    pub remote_username: String,
    pub save_remote_password: bool,

    // Target config (relative to Remote server)
    pub target_host: String,
    pub target_port: u16,

    pub auto_connect: bool,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: "New Tunnel".to_string(),
            local_port: 8080,
            proxy_host: "".to_string(),
            proxy_port: 22,
            proxy_username: "".to_string(),
            save_proxy_password: true,
            remote_host: "".to_string(),
            remote_port: 22,
            remote_username: "".to_string(),
            save_remote_password: true,
            target_host: "127.0.0.1".to_string(),
            target_port: 80,
            auto_connect: false,
        }
    }
}

pub fn get_config_path() -> PathBuf {
    let mut path = env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    path.pop(); // Remove the executable name
    path.push("config.json");
    path
}

pub fn load_configs() -> Vec<TunnelConfig> {
    let path = get_config_path();
    if path.exists()
        && let Ok(content) = fs::read_to_string(&path)
        && let Ok(configs) = serde_json::from_str(&content)
    {
        return configs;
    }
    Vec::new() // Gracefully return empty list if not found or unparseable
}

pub fn save_configs(configs: &[TunnelConfig]) -> Result<(), String> {
    let path = get_config_path();
    let content = serde_json::to_string_pretty(configs)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;
    fs::write(path, content).map_err(|e| format!("Failed to write config: {}", e))
}
