<h1 align="center">rTunnel (Rust)</h1>

<p align="center"><b>rTunnel is an SSH tunneling desktop application designed to link a local Windows port to a Remote Server by tunneling through an intermediary Proxy SSH server.</b></p>

<p align="center">
  <a> <img alt="GitHub Downloads (all assets, all releases)" src="https://img.shields.io/github/downloads/j93d/rTunnel/total"></a>
  <a href="https://opensource.org/license/mit"><img alt="GitHub License" src="https://img.shields.io/github/license/j93d/rTunnel"></a>
</p>



## Design Philosophy

The primary objective of rTunnel is to provide an easy-to-use GUI for managing "Jump Host" port forwarding scenarios where the user needs to authenticate with a Proxy server and forward local traffic to a target destination through a secure SSH tunnel.

### Core Features
- **Portable Configuration**: `config.json` is stored alongside the executable (`std::env::current_exe()`). This allows the application to be completely portable. If the config is missing, the app defaults to an empty state.
- **Secure Password Storage**: We utilize the `keyring` crate to store passwords natively in the **Windows Credential Manager**.
  - Passwords are saved with the prefixes `rTunnel_<id>_proxy` and `rTunnel_<id>_remote`.
- **SSH Key & Password Authentication**: Supports both password-based and private key authentication (RSA / PKCS#8) with a native file picker to browse private keys, automatically bypassing password prompts when a key is configured.
- **Host Key Verification & TOFU**: Performs strict known-hosts verification with Trust On First Use (TOFU) confirmation dialogs for unknown host keys.
- **Auth Retry on Failure**: When proxy authentication fails (e.g. expired password), a dedicated retry dialog lets the user enter a new password immediately instead of requiring a manual toggle of the save-password setting.
- **Auto-reconnect & Keep-Alive**: Periodically performs keep-alive checks and automatically reconnects if the SSH tunnel drops.
- **System Tray Integration**: Slint GUI window can be minimized/restored from the Windows system tray.

## Architecture

- **Language**: Rust (Edition 2024)
- **GUI Framework**: Slint (`ui/main.slint`)
- **SSH Backend**: `russh` 0.63 (pure-Rust async SSH implementation powered by Tokio and Ring)
- **Async Runtime**: `tokio`

### Asynchronous Tunneling Logic (`src/tunnel.rs`)

rTunnel runs a fully asynchronous event loop powered by Tokio:
1. **Proxy Connection & Handshake**: Connects asynchronously to the SSH jump host using `russh::client::connect` and validates the server key against `known_hosts`.
2. **Authentication**: Authenticates using either password or private key (`authenticate_publickey` / `authenticate_password`).
3. **Local Listener**: Binds a `tokio::net::TcpListener` on the configured local port.
4. **Direct TCPIP Forwarding**: For each incoming client connection on the local port, opens an async direct-tcpip channel (`channel_open_direct_tcpip`) to the target destination host and port through the proxy session.
5. **Bidirectional Stream Copy**: Streams data between local sockets and SSH channels concurrently with real-time throughput telemetry (TX/RX metrics).

### File Structure
- `src/main.rs`: Coordinates the Slint event loop, System Tray, auto-reconnect, and bridges state.
- `src/tunnel.rs`: Implements the asynchronous SSH connection, host key verification, and tunneling logic via `russh`.
- `src/config.rs`: Manages reading and writing `TunnelConfig` to the portable JSON file.
- `src/keyring_manager.rs`: Wrapper around the `keyring` crate for Windows Credential Manager integration.
- `ui/main.slint`: Declarative Slint UI frontend.
- `build.rs`: Compiles the `.slint` UI file.

*This project was made using viibecoding.*
