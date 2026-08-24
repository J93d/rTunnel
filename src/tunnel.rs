use crate::config::TunnelConfig;
use ssh2::Session;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

pub fn start_tunnel(
    config: TunnelConfig,
    proxy_pass: String,
    is_running: Arc<AtomicBool>,
) -> Result<(), String> {
    // 1. Proxy Setup
    let proxy_addr = (config.proxy_host.as_str(), config.proxy_port)
        .to_socket_addrs()
        .map_err(|e| format!("Proxy DNS resolution failed: {}", e))?
        .next()
        .ok_or_else(|| "Could not resolve proxy host".to_string())?;

    let proxy_tcp = TcpStream::connect_timeout(&proxy_addr, Duration::from_secs(10))
        .map_err(|e| format!("Proxy connect failed: {}", e))?;

    let mut proxy_sess = Session::new().map_err(|e| e.to_string())?;
    proxy_sess.set_timeout(10000); // 10 seconds
    proxy_sess.set_tcp_stream(proxy_tcp);
    proxy_sess
        .handshake()
        .map_err(|e| format!("Proxy handshake failed: {}", e))?;
    proxy_sess
        .userauth_password(&config.proxy_username, &proxy_pass)
        .map_err(|e| format!("Proxy auth failed: {}", e))?;

    // 2. Local Tunnel Port Setup
    let listener = TcpListener::bind(format!("127.0.0.1:{}", config.local_port))
        .map_err(|e| format!("Failed to bind local port: {}", e))?;
    listener.set_nonblocking(true).unwrap();
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
            thread::sleep(Duration::from_millis(2));
        }
    }

    Ok(())
}
