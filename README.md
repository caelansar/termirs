# TermiRs

A modern, async SSH terminal client built with Rust and [Ratatui](https://ratatui.rs/). TermiRs provides a fast, secure, and user-friendly terminal interface for managing SSH connections with advanced features like secure file transfers and encrypted configuration storage.

![demo](./assets/demo.gif)

## Features

### 🚀 Core Features
- **Async SSH Connections**: Built on `russh` for high-performance, non-blocking SSH operations
- **Multiple Authentication Methods**: Support for password, public key, and keyboard-interactive authentication
- **SSH Config Import**: Import connection details directly from your `~/.ssh/config` file with `Ctrl+L`
- **Secure Configuration**: Encrypted password storage using AES-256-GCM encryption
- **Connection Management**: Save, edit, and organize SSH connections with a clean TUI interface
- **Port Forwarding**: Create and manage local and remote port forwards with automatic status tracking
- **File Transfer**: SFTP support for secure file uploads with progress tracking
- **File Explorer**: Dual-pane SFTP browser with copy/paste transfers
- **External Editor**: Edit local and remote files in your preferred editor (e.g. vim, nano) directly from the file explorer
- **Cross Platforms**: Support macOS, Linux and Windows

### 🔒 Security Features
- **Password Encryption**: All passwords are encrypted at rest using AES-256-GCM with system-specific keys
- **Host Key Verification**: Server public key validation and storage for connection security
- **No Logging of Secrets**: Passwords and keys are never logged or written to disk unencrypted

### 💻 User Experience
- **Interactive UI**: Modern terminal user interface with keyboard navigation
- **Connection Search**: Fast search and filtering of saved connections
- **Progress Indicators**: Visual feedback for file transfers and connection operations
- **Error Handling**: Comprehensive error messages with helpful context
- **Responsive Design**: Non-blocking operations keep the UI responsive at all times

## Installation

### From GitHub release
Download the latest binary from the [releases page](https://github.com/caelansar/termirs/releases)

### Building from Source

#### Prerequisites
- Rust 1.85+ (2024 edition support)
- A terminal that supports VT100 escape sequences

```bash
# Clone the repository
git clone https://github.com/caelansar/termirs.git
cd termirs

# Build the project
cargo build --release --locked

# Run the application
./target/release/termirs
```


## Usage

### First Time Setup
1. Launch TermiRs: `./termirs` or `cargo run --release`
2. Press `n` to create a new SSH connection
3. Fill in the connection details (host, username, authentication method)
   - Or press `Ctrl+L` to import from your `~/.ssh/config` file
4. Press `Enter` to connect, connection will be saved in connection list
5. Or select the connection and press `Enter` to connect

### Keyboard Shortcuts

#### Connection List

| Key            | Action                          |
| -------------- | ------------------------------- |
| `↑/↓` or `j/k` | Navigate connections            |
| `Enter`        | Connect to selected connection  |
| `n`            | Create new connection           |
| `e`            | Edit selected connection        |
| `d`            | Delete selected connection      |
| `i`            | Open file explorer              |
| `p`            | Open port forwarding management |
| `/`            | Search connections              |
| `q`            | Quit application                |

#### New Connection Form

| Key             | Action                                                              |
| --------------- | ------------------------------------------------------------------- |
| `Tab/Shift+Tab` | Navigate between fields                                             |
| `Ctrl+L`        | Load connection details from `~/.ssh/config` (enter hostname first) |
| `Enter`         | Save and connect                                                    |
| `Esc`           | Cancel and return to connection list                                |

#### Connected Terminal

| Key                               | Action                                          |
| --------------------------------- | ----------------------------------------------- |
| `Page Up/Down` or `Ctrl+b/Ctrl+f` | Scroll terminal history                         |
| `Ctrl+S`                          | Search terminal history                         |
| `Esc`                             | Disconnect and return to connection list        |
| `n`                               | Navigate next matched item (in search mode)     |
| `p`                               | Navigate previous matched item (in search mode) |

#### File Explorer (SFTP)

| Key                | Action                                                      |
| ------------------ | ----------------------------------------------------------- |
| `Tab`              | Swap active pane between local and remote directories       |
| `j/k` or `↓/↑`     | Move selection within the active pane                       |
| `Enter` or `l`     | Enter directories                                           |
| `Backspace` or `h` | Go up one directory level                                   |
| `H`                | Toggle whether hidden files should be shown                 |
| `c`                | Copy highlighted file or folder into the transfer clipboard |
| `v`                | Paste into the destination pane to start an async transfer  |
| `e`                | Edit selected file in external editor                       |
| `r`                | Refresh the current pane listing                            |
| `Esc`              | Cancel file explorer and return to connection list          |

#### Port Forwarding Management

| Key            | Action                                   |
| -------------- | ---------------------------------------- |
| `↑/↓` or `j/k` | Navigate port forwarding rules           |
| `n`            | Create new port forwarding rule          |
| `e`            | Edit selected port forwarding rule       |
| `d`            | Delete selected port forwarding rule     |
| `Enter`        | Start/stop selected port forwarding rule |
| `/`            | Search port forwarding rules             |
| `Esc`          | Return to connection list                |

### Authentication Methods

#### Password Authentication
```
Host: example.com
Port: 22
Username: user
Password: [encrypted and stored securely]
```

#### Public Key Authentication
```
Host: example.com
Port: 22
Username: user
Private Key: ~/.ssh/id_rsa
```

#### SSH Config Import
TermiRs can import connection details from your existing `~/.ssh/config` file:

1. Press `n` to create a new connection
2. Enter the hostname or host alias from your SSH config
3. Press `Ctrl+L` to automatically load:
   - Hostname/IP address
   - Port number
   - Username
   - Identity file path

The SSH config parser supports `Include` directives, allowing you to organize your SSH configurations across multiple files.

#### Keyboard Interactive
Automatically handled when the server requires interactive authentication.

### Port Forwarding
#### Local Port Forwarding
Forward a remote service (e.g., PostgreSQL on a server) to your local machine:
- Forward Type: Local
- Local Address: 127.0.0.1
- Local Port: 8080 (on your computer)
- Service Host: remote.host (e.g., database VM)
- Service Port: 5432 (PostgreSQL default)

When started, connections to `localhost:8080` on your computer are securely forwarded through SSH to `remote.host:5432` on the remote server.


#### Remote Port Forwarding
Expose a local web server running on port 3000 to the remote server's port 8080:
- Forward Type: Remote
- Local Port: 8080 (port on remote server)
- Remote Bind Address: 0.0.0.0 (listen on all interfaces) or 127.0.0.1 (localhost only)
- Service Host: localhost
- Service Port: 3000

When started, anyone connecting to `remote_server:8080` will be forwarded to your local `localhost:3000`.

#### Dynamic SOCKS5 Forwarding
Create a SOCKS5 proxy on port 1080:
- Forward Type: Dynamic
- Local Address: 127.0.0.1
- Local Port: 1080

Configure your browser or applications to use `127.0.0.1:1080` as SOCKS5 proxy. All traffic will be tunneled through the SSH connection.


### External Editor
TermiRs lets you edit files directly from the file explorer using your preferred editor. Press `e` on any file to open it.

- **Editor Selection**: Uses the `VISUAL` or `EDITOR` environment variable (falls back to a system default)
- **Local Files**: Opened directly in the editor
- **Remote Files**: Downloaded via SFTP to a temporary file, opened in the editor, and uploaded back automatically if modified

### Configuration

TermiRs stores configuration in `~/.config/termirs/config.toml`. The file includes:
- Saved SSH connections (with encrypted passwords)
- Port forwarding rules and their associated connections
- Server public keys for host verification
- Application settings

Example configuration structure:
```toml
[settings]
default_port = 22
connection_timeout = 20

[[connections]]
id = "uuid-string"
display_name = "My Server"
host = "example.com"
port = 22
username = "user"
created_at = "2023-01-01T00:00:00Z"
public_key = "ssh-rsa AAAAB3NzaC1yc2E..."

[connections.auth_method]
password = "encrypted-password-data"

[[port_forwards]]
id = "port-forward-uuid"
connection_id = "uuid-string"
forward_type = "Local"
display_name = "Local Web Server"
local_addr = "127.0.0.1"
local_port = 8080
service_host = "localhost"
service_port = 3000
created_at = "2023-01-01T00:00:00Z"
```

## Architecture

### Technology Stack
- **SSH Client**: [russh](https://crates.io/crates/russh) - Modern async SSH implementation
- **SFTP**: [russh-sftp](https://crates.io/crates/russh-sftp) - SFTP protocol support
- **TUI Framework**: [Ratatui](https://ratatui.rs/) - Terminal user interface library
- **Async Runtime**: [Tokio](https://tokio.rs/) - Asynchronous runtime for Rust
- **Terminal Backend**: [Crossterm](https://crates.io/crates/crossterm) - Cross-platform terminal manipulation
- **Encryption**: [Ring](https://crates.io/crates/ring) - Cryptographic operations
- **Configuration**: [TOML](https://crates.io/crates/toml) - Configuration file format

### Async Architecture
TermiRs is built with a fully asynchronous architecture:

1. **Event Loop**: The main loop handles UI rendering and event processing
2. **SSH Operations**: All SSH operations are non-blocking and use async/await
3. **Port Forwarding**: Port forwards run as independent async tasks with automatic lifecycle management
4. **File Transfers**: SFTP transfers run in background tasks with progress updates
5. **Connection Management**: Connection establishment and teardown are async
6. **Input Handling**: Keyboard and terminal events are processed asynchronously
