use keyring::Entry;

pub fn get_proxy_service_name(id: &str) -> String {
    format!("rTunnel_{}_proxy", id)
}

pub fn save_password(service: &str, username: &str, password: &str) -> Result<(), String> {
    let entry = Entry::new(service, username).map_err(|e| e.to_string())?;
    entry.set_password(password).map_err(|e| e.to_string())
}

pub fn get_password(service: &str, username: &str) -> Result<String, String> {
    let entry = Entry::new(service, username).map_err(|e| e.to_string())?;
    entry.get_password().map_err(|e| e.to_string())
}

pub fn delete_password(service: &str, username: &str) -> Result<(), String> {
    let entry = Entry::new(service, username).map_err(|e| e.to_string())?;
    let _ = entry.delete_credential(); // Ignore errors if it didn't exist
    Ok(())
}
