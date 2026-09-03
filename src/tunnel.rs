use crate::config::TunnelConfig;
use async_trait::async_trait;
use russh::*;
use russh_keys::PublicKeyBase64;
use russh_keys::key;
use sha2::{Digest, Sha256};
use std::io::BufRead;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
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

impl From<russh::Error> for TunnelError {
    fn from(e: russh::Error) -> Self {
        TunnelError::Message(e.to_string())
    }
}

impl From<russh_keys::Error> for TunnelError {
    fn from(e: russh_keys::Error) -> Self {
        TunnelError::Message(e.to_string())
    }
}

impl From<std::io::Error> for TunnelError {
    fn from(e: std::io::Error) -> Self {
        TunnelError::Message(e.to_string())
    }
}

pub struct TunnelTelemetry {
    pub start_time: Instant,
    pub tx_bytes: AtomicU64,
    pub rx_bytes: AtomicU64,
}

struct ClientHandler {
    host: String,
    port: u16,
    known_hosts_path: std::path::PathBuf,
    key_error: Arc<Mutex<Option<TunnelError>>>,
}

#[async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let mut found = false;

        let pub_key_bytes = server_public_key.public_key_bytes();
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let b64_key = STANDARD.encode(&pub_key_bytes);

        if let Ok(file) = std::fs::File::open(&self.known_hosts_path) {
            let reader = std::io::BufReader::new(file);
            for line in reader.lines().map_while(Result::ok) {
                if line.contains(&b64_key) {
                    found = true;
                    break;
                }
            }
        }

        if found {
            return Ok(true);
        }

        // Compute fingerprint
        let mut hasher = Sha256::new();
        hasher.update(pub_key_bytes);
        let hash = hasher.finalize();
        let fingerprint = hash
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();

        let key_type_str = server_public_key.name(); // e.g., ssh-ed25519

        let port_str = if self.port == 22 {
            self.host.clone()
        } else {
            format!("[{}]:{}", self.host, self.port)
        };
        let base64_line = format!("{} {} {}", port_str, key_type_str, b64_key);

        let mut lock = self.key_error.lock().await;
        *lock = Some(TunnelError::UnknownHostKey(fingerprint, base64_line));

        Ok(false) // Reject to halt connection
    }
}

pub async fn start_tunnel(
    config: TunnelConfig,
    proxy_pass: Zeroizing<String>,
    is_running: Arc<AtomicBool>,
    telemetry: Arc<TunnelTelemetry>,
    connection_timeout: u64,
) -> Result<(), TunnelError> {
    let known_hosts_path = dirs::home_dir()
        .ok_or_else(|| "Cannot determine home directory".to_string())?
        .join(".ssh")
        .join("known_hosts");

    let key_error = Arc::new(Mutex::new(None));

    let handler = ClientHandler {
        host: config.proxy_host.clone(),
        port: config.proxy_port,
        known_hosts_path,
        key_error: key_error.clone(),
    };

    let russh_config = russh::client::Config::default();
    let russh_config = Arc::new(russh_config);

    let connect_future = russh::client::connect(
        russh_config,
        (config.proxy_host.as_str(), config.proxy_port),
        handler,
    );
    let mut session = tokio::time::timeout(Duration::from_secs(connection_timeout), connect_future)
        .await
        .map_err(|_| TunnelError::Message("Connection timeout".to_string()))??;

    // Check if key error was set during connect
    if let Some(err) = key_error.lock().await.take() {
        return Err(err);
    }

    // Auth
    let auth_res = if !config.rsa_key_path.is_empty() {
        let key_path = std::path::Path::new(&config.rsa_key_path);
        let passphrase = if proxy_pass.is_empty() {
            None
        } else {
            Some(proxy_pass.as_str())
        };
        let key = russh_keys::load_secret_key(key_path, passphrase)?;
        session
            .authenticate_publickey(config.proxy_username.clone(), Arc::new(key))
            .await?
    } else {
        session
            .authenticate_password(config.proxy_username.clone(), proxy_pass.as_str())
            .await?
    };

    if !auth_res {
        return Err(TunnelError::Message("Authentication failed".to_string()));
    }

    let listener = TcpListener::bind(format!("127.0.0.1:{}", config.local_port)).await?;
    let session = Arc::new(tokio::sync::Mutex::new(session));

    let mut last_keepalive = Instant::now();

    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                if !is_running.load(Ordering::Relaxed) {
                    break;
                }
                if last_keepalive.elapsed() > Duration::from_secs(5) {
                    last_keepalive = Instant::now();
                }
            },
            accept_res = listener.accept() => {
                if let Ok((user_tcp, _)) = accept_res {
                    let session_handle = session.clone();
                    let target_host = config.target_host.clone();
                    let target_port = config.target_port;
                    let telemetry_clone = telemetry.clone();

                    tokio::spawn(async move {
                        let channel_res = {
                            let sess = session_handle.lock().await;
                            sess.channel_open_direct_tcpip(target_host, target_port as u32, "localhost", 0).await
                        };
                        if let Ok(channel) = channel_res {
                            let stream = channel.into_stream();

                            let (mut user_rx, mut user_tx) = tokio::io::split(user_tcp);
                            let (mut ch_rx, mut ch_tx) = tokio::io::split(stream);

                            let tel_tx = telemetry_clone.clone();
                            let t1 = tokio::spawn(async move {
                                let mut buf = [0u8; 8192];
                                loop {
                                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                                    match user_rx.read(&mut buf).await {
                                        Ok(0) => break,
                                        Ok(n) => {
                                            let _: Result<(), _> = ch_tx.write_all(&buf[..n]).await;
                                            tel_tx.tx_bytes.fetch_add(n as u64, Ordering::Relaxed);
                                        }
                                        Err(_) => break,
                                    }
                                }
                            });

                            let tel_rx = telemetry_clone.clone();
                            let t2 = tokio::spawn(async move {
                                let mut buf = [0u8; 8192];
                                loop {
                                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                                    match ch_rx.read(&mut buf).await {
                                        Ok(0) => break,
                                        Ok(n) => {
                                            let _: Result<(), _> = user_tx.write_all(&buf[..n]).await;
                                            tel_rx.rx_bytes.fetch_add(n as u64, Ordering::Relaxed);
                                        }
                                        Err(_) => break,
                                    }
                                }
                            });

                            let _ = tokio::join!(t1, t2);
                        }
                    });
                }
            }
        }
    }

    let sess = session.lock().await;
    let _ = sess
        .disconnect(
            russh::Disconnect::ByApplication,
            "rTunnel session closed",
            "en-US",
        )
        .await;

    Ok(())
}
