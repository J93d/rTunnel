use crate::config::TunnelConfig;
use ssh2::Session;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

fn run_bridge_single(mut tcp: TcpStream, mut channel: ssh2::Channel, session: &mut Session) {
    session.set_blocking(false);
    let _ = tcp.set_nonblocking(true);
    let mut tb = [0; 8192];
    let mut sb = [0; 8192];

    loop {
        let mut progress = false;

        match tcp.read(&mut tb) {
            Ok(0) => break,
            Ok(n) => {
                session.set_blocking(true);
                if channel.write_all(&tb[..n]).is_err() {
                    break;
                }
                session.set_blocking(false);
                progress = true;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }

        match channel.read(&mut sb) {
            Ok(0) => {
                if channel.eof() {
                    break;
                }
            }
            Ok(n) => {
                let _ = tcp.set_nonblocking(false);
                if tcp.write_all(&sb[..n]).is_err() {
                    break;
                }
                let _ = tcp.set_nonblocking(true);
                progress = true;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }

        if !progress {
            thread::sleep(Duration::from_millis(2));
        }
    }
}

pub fn start_tunnel(
    config: TunnelConfig,
    proxy_pass: String,
    remote_pass: String,
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

    // 2. Loopback listener for Remote Session
    let internal_listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let internal_port = internal_listener.local_addr().unwrap().port();

    let remote_host = config.remote_host.clone();
    let remote_port = config.remote_port;

    thread::spawn(move || {
        if let Ok((internal_tcp, _)) = internal_listener.accept()
            && let Ok(proxy_channel) =
                proxy_sess.channel_direct_tcpip(&remote_host, remote_port, None)
        {
            run_bridge_single(internal_tcp, proxy_channel, &mut proxy_sess);
        }
    });

    // 3. Remote Setup
    let remote_tcp =
        TcpStream::connect_timeout(&"127.0.0.1".parse().unwrap(), Duration::from_secs(10))
            .unwrap_or_else(|_| TcpStream::connect(("127.0.0.1", internal_port)).unwrap());

    let mut remote_sess = Session::new().map_err(|e| e.to_string())?;
    remote_sess.set_timeout(10000); // 10 seconds
    remote_sess.set_tcp_stream(remote_tcp);
    remote_sess
        .handshake()
        .map_err(|e| format!("Remote handshake failed: {}", e))?;
    remote_sess
        .userauth_password(&config.remote_username, &remote_pass)
        .map_err(|e| format!("Remote auth failed: {}", e))?;

    // 4. Local Tunnel Port Setup
    let listener = TcpListener::bind(format!("127.0.0.1:{}", config.local_port))
        .map_err(|e| format!("Failed to bind local port: {}", e))?;
    listener.set_nonblocking(true).unwrap();
    remote_sess.set_blocking(false);

    // 5. Multiplexing Loop
    let mut clients: Vec<(TcpStream, ssh2::Channel)> = Vec::new();
    let mut tb = [0; 8192];
    let mut sb = [0; 8192];

    while is_running.load(Ordering::Relaxed) {
        let mut progress = false;

        // Accept new connections
        match listener.accept() {
            Ok((user_tcp, _)) => {
                let _ = user_tcp.set_nonblocking(true);
                remote_sess.set_blocking(true); // temporary blocking to open channel
                match remote_sess.channel_direct_tcpip(
                    &config.target_host,
                    config.target_port,
                    None,
                ) {
                    Ok(channel) => {
                        clients.push((user_tcp, channel));
                        progress = true;
                    }
                    Err(e) => {
                        eprintln!("Failed to open target channel: {}", e);
                    }
                }
                remote_sess.set_blocking(false);
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
                    remote_sess.set_blocking(true);
                    if channel.write_all(&tb[..n]).is_err() {
                        keep = false;
                    }
                    remote_sess.set_blocking(false);
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
