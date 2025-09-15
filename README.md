# TermiRs

A modern, async SSH terminal client built with Rust and [Ratatui](https://ratatui.rs/). TermiRs provides a fast, secure, and user-friendly terminal interface for managing SSH connections with advanced features like secure file transfers and encrypted configuration storage.

## Features

### 🚀 Core Features
- **Async SSH Connections**: Built on `russh` for high-performance, non-blocking SSH operations
- **Multiple Authentication Methods**: Support for password, public key, and keyboard-interactive authentication
- **Secure Configuration**: Encrypted password storage using AES-256-GCM encryption
- **Connection Management**: Save, edit, and organize SSH connections with a clean TUI interface
- **File Transfer**: SFTP support for secure file uploads with progress tracking
- **Terminal Emulation**: Full VT100 terminal emulation with color support and scrollback

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

### Prerequisites
- Rust 1.85+ (2024 edition support)
- A terminal that supports VT100 escape sequences

### Building from Source
```bash
# Clone the repository
git clone https://github.com/your-username/termirs.git
cd termirs

# Build the project
cargo build --release

# Run the application
./target/release/termirs
```


## Usage

### First Time Setup
1. Launch TermiRs: `./termirs` or `cargo run --release`
2. Press `n` to create a new SSH connection
3. Fill in the connection details (host, username, authentication method)
4. Press `Enter` to connect, connection will be saved in connection list
5. Or select the connection and press `Enter` to connect

### Keyboard Shortcuts

#### Connection List
- `↑/↓` or `j/k`: Navigate connections
- `Enter`: Connect to selected connection
- `n`: Create new connection
- `e`: Edit selected connection
- `d`: Delete selected connection
- `s`: Start SFTP file transfer
- `/`: Search connections
- `q`: Quit application

#### Connected Terminal
- `Page Up/Down` or `Ctrl+b/Ctrl+f`: Scroll terminal history
- `Esc`: Disconnect and return to connection list

#### File Transfer (SFTP)
- `↑/↓`: Switch between local and remote path fields
- `Enter`: Start file transfer
- `Esc`: Cancel and return to connection list

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

#### Keyboard Interactive
Automatically handled when the server requires interactive authentication.

### Configuration

TermiRs stores configuration in `~/.config/termirs/config.toml`. The file includes:
- Saved SSH connections (with encrypted passwords)
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
auth_method = { password = "encrypted-password-data" }
created_at = "2023-01-01T00:00:00Z"
public_key = "ssh-rsa AAAAB3NzaC1yc2E..."
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
- **Terminal Emulation**: [vt100](https://crates.io/crates/vt100) - VT100 terminal emulator

### Async Architecture
TermiRs is built with a fully asynchronous architecture:

1. **Event Loop**: The main loop handles UI rendering and event processing
2. **SSH Operations**: All SSH operations are non-blocking and use async/await
3. **File Transfers**: SFTP transfers run in background tasks with progress updates
4. **Connection Management**: Connection establishment and teardown are async
5. **Input Handling**: Keyboard and terminal events are processed asynchronously