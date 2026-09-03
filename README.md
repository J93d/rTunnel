# rTunnel v0.1.5

**rTunnel** is an SSH tunneling desktop application designed to link a local Windows port to a Remote Server by tunneling through an intermediary Proxy SSH server.

*This project was made using viibecoding.*

## Design Philosophy

The primary objective of rTunnel is to provide an easy-to-use GUI for managing complex "Jump Host" port forwarding scenarios where the user needs to authenticate with a Proxy server and then authenticate again with a Remote server before forwarding a specific target port.

### Core Features
- **Portable Configuration**: `config.json` is stored alongside the executable (`std::env::current_exe()`). This allows the application to be completely portable. If the config is missing, the app defaults to an empty state.
- **Secure Password Storage**: We utilize the `keyring` crate to store passwords natively in the **Windows Credential Manager**.
  - Passwords are saved with the prefixes `rTunnel_<id>_proxy` and `rTunnel_<id>_remote`.
- **System Tray Integration**: Uses `tray-icon`. The Slint GUI intercepts the window close event to hide the application into the system tray, and clicking the tray icon restores it.

## Architecture

- **Language**: Rust
- **GUI Framework**: Slint (`ui/main.slint`)
- **SSH Backend**: `ssh2` (a libssh2 wrapper).

### The Double-SSH Tunneling Logic (`src/tunnel.rs`)

Multiplexing a single `ssh2::Session` across multiple threads for simultaneous port-forwarding channels is extremely difficult in Rust due to `libssh2`'s strict locking constraints and lack of `Sync` traits.

To ensure **100% reliability** and bypass these threading complexities, rTunnel uses a **per-connection isolated tunnel** approach. For *every* incoming client connection on the local port, the application spins up an isolated background thread that performs the following steps:

1. **Proxy Connection**: Establishes a new `TcpStream` and a new `ssh2::Session` to the Proxy Server and authenticates.
2. **Internal Loopback**: Creates a temporary `TcpListener` on `127.0.0.1:0`.
3. **Proxy Channel Bridge**: Requests a `direct_tcpip` channel on the Proxy session to the Remote Server, and bidirectionally bridges it to the internal loopback socket.
4. **Remote Connection**: Connects a second `ssh2::Session` to the local loopback socket, completing the SSH handshake with the Remote Server through the Proxy channel.
5. **Target Bridge**: Finally, requests a `direct_tcpip` channel on the Remote session to the Target Destination, and bidirectionally bridges the original user's TCP stream to this final channel.

While this approach introduces a slight latency penalty during the initial connection setup (~500ms to establish double SSH handshakes), it is entirely stateless, parallelizable, and rock-solid for long-running TCP streams (like HTTP keep-alive).

### File Structure
- `src/main.rs`: Coordinates the Slint event loop, System Tray, and bridges the state.
- `src/tunnel.rs`: Implements the blocking, multithreaded double-SSH bridging logic.
- `src/config.rs`: Manages the reading and writing of `TunnelConfig` to the portable JSON file.
- `src/keyring_manager.rs`: Wrapper around the `keyring` crate for password management.
- `ui/main.slint`: The declarative Slint frontend.
- `build.rs`: Compiles the `.slint` UI file.

## Future Scope (Next Phases)
- **SSH Key Authentication**: Currently, only password-based authentication is supported. The next phase will involve adding support for RSA/Ed25519 keys (e.g., parsing keys via `rfd` file dialogs or checking `~/.ssh/id_rsa`).
