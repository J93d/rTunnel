use crate::config::TunnelConfig;
use ssh2::Session;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[derive(Debug)]
pub enum TunnelError {
    Message(String),
    UnknownHostKey(String, String), // fingerprint, base64_line
}

impl From<String> for TunnelError {
    fn from(s: String) -> Self {
        TunnelError::Message(s)
    }
}

pub struct TunnelTelemetry {
    pub start_time: Instant,
    pub tx_bytes: AtomicU64,
    pub rx_bytes: AtomicU64,
}

pub fn start_tunnel(
    config: TunnelConfig,
    proxy_pass: Zeroizing<String>,
    is_running: Arc<AtomicBool>,
    telemetry: Arc<TunnelTelemetry>,
    connection_timeout: u64,
) -> Result<(), TunnelError> {
    // 1. Proxy Setup
    let proxy_addr = (config.proxy_host.as_str(), config.proxy_port)
        .to_socket_addrs()
        .map_err(|e| format!("Proxy DNS resolution failed: {}", e))?
        .next()
        .ok_or_else(|| "Could not resolve proxy host".to_string())?;

    let proxy_tcp =
        TcpStream::connect_timeout(&proxy_addr, Duration::from_secs(connection_timeout))
            .map_err(|e| format!("Proxy connect failed: {}", e))?;

    let mut proxy_sess = Session::new().map_err(|e| e.to_string())?;
    proxy_sess.set_timeout(connection_timeout as u32 * 1000);
    proxy_sess.set_tcp_stream(proxy_tcp);
    proxy_sess
        .handshake()
        .map_err(|e| format!("Proxy handshake failed: {}", e))?;

    let mut known_hosts = proxy_sess
        .known_hosts()
        .map_err(|e| format!("Failed to init known_hosts: {}", e))?;

    let known_hosts_path = dirs::home_dir()
        .ok_or_else(|| "Cannot determine home directory".to_string())?
        .join(".ssh")
        .join("known_hosts");

    if known_hosts_path.exists() {
        known_hosts
            .read_file(&known_hosts_path, ssh2::KnownHostFileKind::OpenSSH)
            .map_err(|e| format!("Failed to read known_hosts: {}", e))?;
    }

    let (host_key, key_type) = proxy_sess
        .host_key()
        .ok_or_else(|| "Server did not provide a host key".to_string())?;

    match known_hosts.check_port(&config.proxy_host, config.proxy_port, host_key) {
        ssh2::CheckResult::Match => { /* Host key verified */ }
        ssh2::CheckResult::NotFound | ssh2::CheckResult::Mismatch => {
            let fingerprint = proxy_sess
                .host_key_hash(ssh2::HashType::Sha256)
                .map(|h| h.iter().map(|b| format!("{:02x}", b)).collect::<String>())
                .unwrap_or_else(|| "unknown".to_string());

            let key_type_str = match key_type {
                ssh2::HostKeyType::Rsa => "ssh-rsa",
                ssh2::HostKeyType::Ed25519 => "ssh-ed25519",
                ssh2::HostKeyType::Ecdsa256 => "ecdsa-sha2-nistp256",
                ssh2::HostKeyType::Ecdsa384 => "ecdsa-sha2-nistp384",
                ssh2::HostKeyType::Ecdsa521 => "ecdsa-sha2-nistp521",
                ssh2::HostKeyType::Dss => "ssh-dss",
                _ => return Err(TunnelError::Message("Unsupported host key type".into())),
            };

            use base64::{Engine as _, engine::general_purpose::STANDARD};
            let b64_key = STANDARD.encode(host_key);

            let port_str = if config.proxy_port == 22 {
                config.proxy_host.clone()
            } else {
                format!("[{}]:{}", config.proxy_host, config.proxy_port)
            };
            let base64_line = format!("{} {} {}", port_str, key_type_str, b64_key);

            return Err(TunnelError::UnknownHostKey(fingerprint, base64_line));
        }
        ssh2::CheckResult::Failure => {
            return Err(TunnelError::Message(
                "Host key verification failed".to_string(),
            ));
        }
    }

    if !config.rsa_key_path.is_empty() {
        let key_path = std::path::Path::new(&config.rsa_key_path);
        proxy_sess
            .userauth_pubkey_file(&config.proxy_username, None, key_path, Some(&proxy_pass))
            .map_err(|e| format!("Key auth failed: {}", e))?;
    } else {
        proxy_sess
            .userauth_password(&config.proxy_username, &proxy_pass)
            .map_err(|e| format!("Proxy auth failed: {}", e))?;
    }

    // 2. Local Tunnel Port Setup
    let listener = TcpListener::bind(format!("127.0.0.1:{}", config.local_port))
        .map_err(|e| format!("Failed to bind local port: {}", e))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Failed to set listener non-blocking: {}", e))?;
    proxy_sess.set_blocking(false);

    // 3. Multiplexing Loop
    let mut clients: Vec<(TcpStream, ssh2::Channel)> = Vec::new();
    let mut tb = [0; 8192];
    let mut sb = [0; 8192];

    while is_running.load(Ordering::Relaxed) {
        let mut progress = false;

        // Accept new connections
        match listener.accept() {
            Ok((user_tcp, _)) => {
                let _ = user_tcp.set_nonblocking(true);
                proxy_sess.set_blocking(true); // temporary blocking to open channel
                match proxy_sess.channel_direct_tcpip(&config.target_host, config.target_port, None)
                {
                    Ok(channel) => {
                        clients.push((user_tcp, channel));
                        progress = true;
                    }
                    Err(e) => {
                        eprintln!("Failed to open target channel: {}", e);
                    }
                }
                proxy_sess.set_blocking(false);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }

        // Process existing clients
        let mut i = 0;
        while i < clients.len() {
            let mut keep = true;
            let (tcp, channel) = &mut clients[i];

            // TCP to SSH
            match tcp.read(&mut tb) {
                Ok(0) => keep = false,
                Ok(n) => {
                    proxy_sess.set_blocking(true);
                    if channel.write_all(&tb[..n]).is_err() {
                        keep = false;
                    } else {
                        telemetry.tx_bytes.fetch_add(n as u64, Ordering::Relaxed);
                    }
                    proxy_sess.set_blocking(false);
                    progress = true;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => keep = false,
            }

            // SSH to TCP
            if keep {
                match channel.read(&mut sb) {
                    Ok(0) => {
                        if channel.eof() {
                            keep = false;
                        }
                    }
                    Ok(n) => {
                        let _ = tcp.set_nonblocking(false);
                        if tcp.write_all(&sb[..n]).is_err() {
                            keep = false;
                        } else {
                            telemetry.rx_bytes.fetch_add(n as u64, Ordering::Relaxed);
                        }
                        let _ = tcp.set_nonblocking(true);
                        progress = true;
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => keep = false,
                }
            }

            if keep {
                i += 1;
            } else {
                let mut c = clients.remove(i).1;
                let _ = c.send_eof();
                let _ = c.wait_eof();
                let _ = c.close();
                let _ = c.wait_close();
            }
        }

        if !progress {
            thread::sleep(Duration::from_millis(10));
        }
    }

    for (_, mut channel) in clients {
        let _ = channel.send_eof();
        let _ = channel.wait_eof();
        let _ = channel.close();
        let _ = channel.wait_close();
    }

    let _ = proxy_sess.disconnect(None, "rTunnel session closed", None);

    Ok(())
}
