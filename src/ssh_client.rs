use std::env;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ssh2::{Channel, KeyboardInteractivePrompt, Prompt, Session};

use crate::config::manager::{AuthMethod, Connection};
use crate::error::{AppError, Result};

struct KbdIntPrompter {
    password: String,
}

impl KeyboardInteractivePrompt for KbdIntPrompter {
    fn prompt<'a>(
        &mut self,
        _username: &str,
        _instructions: &str,
        prompts: &[Prompt<'a>],
    ) -> Vec<String> {
        prompts
            .iter()
            .map(|p| {
                if p.echo {
                    String::new()
                } else {
                    self.password.clone()
                }
            })
            .collect()
    }
}

#[derive(Clone)]
pub struct SshClient {
    pub channel: Arc<Mutex<Channel>>, // exposed for simple locking by UI loop
}

impl SshClient {
    pub fn connect(connection: &Connection) -> Result<Self> {
        let sess = Self::make_session(connection)?;

        let mut channel = sess.channel_session().map_err(|e| {
            AppError::SshConnectionError(format!("Failed to open SSH channel: {}", e))
        })?;
        channel
            .request_pty("xterm-256color", None, Some((100, 30, 0, 0)))
            .map_err(|e| AppError::SshConnectionError(format!("Failed to request PTY: {}", e)))?;
        channel.shell().map_err(|e| {
            AppError::SshConnectionError(format!("Failed to start remote shell: {}", e))
        })?;

        // non-blocking for polling reads
        sess.set_blocking(false);

        Ok(Self {
            channel: Arc::new(Mutex::new(channel)),
        })
    }

    pub fn request_size(&self, cols: u16, rows: u16) {
        if let Ok(mut ch) = self.channel.lock() {
            let _ = ch.request_pty_size(cols as u32, rows as u32, None, None);
        }
    }

    pub fn write_all(&self, data: &[u8]) -> Result<()> {
        let mut written = 0usize;
        while written < data.len() {
            // Attempt a write with a short-lived lock to let the reader drain between retries
            let write_result = {
                if let Ok(mut ch) = self.channel.lock() {
                    ch.write(&data[written..])
                } else {
                    return Err(AppError::SshWriteError("Failed to lock SSH channel".into()));
                }
            };

            match write_result {
                Ok(0) => {
                    // Treat as WouldBlock-like; brief backoff to allow draining
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(n) => {
                    written += n;
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    return Err(AppError::SshWriteError(format!(
                        "Failed to write to SSH channel: {}",
                        e
                    )));
                }
            }
        }
        Ok(())
    }

    fn try_publickey_auth(
        sess: &Session,
        username: &str,
        private_key_path: &str,
        passphrase: Option<&str>,
    ) -> bool {
        let key_path = if private_key_path.starts_with("~/") {
            if let Some(home) = env::var_os("HOME") {
                PathBuf::from(home).join(&private_key_path[2..])
            } else {
                return false;
            }
        } else {
            PathBuf::from(private_key_path)
        };

        // 1) Try without a passphrase first (works for unencrypted keys)
        if sess
            .userauth_pubkey_file(username, None, &key_path, None)
            .is_ok()
        {
            if sess.authenticated() {
                return true;
            }
        }

        // 2) If that failed and we have a passphrase, try with it
        if let Some(pp) = passphrase {
            if sess
                .userauth_pubkey_file(username, None, &key_path, Some(pp))
                .is_ok()
            {
                if sess.authenticated() {
                    return true;
                }
            }
        }
        false
    }

    pub fn make_session(connection: &Connection) -> Result<Session> {
        let host = connection.host_port();
        let user = &connection.username;

        // Parse the host and port into a socket address
        let socket_addr = host
            .parse::<SocketAddr>()
            .map_err(|e| AppError::SshConnectionError(format!("Invalid host/port: {}", e)))?;

        let tcp = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(10))?;
        tcp.set_nodelay(true).ok();

        let mut sess = Session::new().map_err(|e| {
            AppError::SshConnectionError(format!("Failed to create SSH session: {}", e))
        })?;
        sess.set_tcp_stream(tcp);
        sess.handshake().map_err(|e| {
            AppError::SshConnectionError(format!("Failed to perform SSH handshake: {}", e))
        })?;

        let methods_str = sess.auth_methods(user).unwrap_or("");
        let methods: Vec<&str> = methods_str.split(',').filter(|s| !s.is_empty()).collect();

        let has_interactive = methods.iter().any(|m| *m == "keyboard-interactive");
        let has_password = methods.iter().any(|m| *m == "password");

        if has_interactive || has_password {
            // Extract password from auth_method if it's a password authentication
            if let AuthMethod::Password(password) = &connection.auth_method {
                if has_interactive {
                    let mut prompter = KbdIntPrompter {
                        password: password.clone(),
                    };
                    let _ = sess.userauth_keyboard_interactive(user, &mut prompter);
                }
                if !sess.authenticated() && has_password {
                    let _ = sess.userauth_password(user, password);
                }
            }
            if !sess.authenticated() {
                return Err(AppError::AuthenticationError(
                    "SSH authentication failed".to_string(),
                ));
            }
        } else {
            if methods.iter().any(|m| *m == "publickey") {
                if let AuthMethod::PublicKey {
                    private_key_path,
                    passphrase,
                } = &connection.auth_method
                {
                    if !Self::try_publickey_auth(
                        &sess,
                        user,
                        private_key_path,
                        passphrase.as_deref(),
                    ) {
                        return Err(AppError::AuthenticationError(
                            "SSH publickey authentication failed".to_string(),
                        ));
                    } else {
                        return Ok(sess);
                    }
                }
            }

            return Err(AppError::AuthenticationError(format!(
                "SSH authentication failed: no supported authentication methods found, methods: {}",
                methods_str
            )));
        }

        if !sess.authenticated() {
            return Err(AppError::AuthenticationError(
                "SSH authentication failed".to_string(),
            ));
        }

        Ok(sess)
    }

    #[allow(dead_code)]
    pub fn read_some(&self, buf: &mut [u8]) -> usize {
        let mut n = 0usize;
        if let Ok(mut ch) = self.channel.lock() {
            if let Ok(got) = ch.read(buf) {
                n = got;
            }
        }
        n
    }

    pub fn close(&self) {
        if let Ok(mut ch) = self.channel.lock() {
            let _ = ch.send_eof();
            let _ = ch.close();
            let _ = ch.wait_close();
        }
    }

    /// Perform a blocking SCP file upload using a fresh authenticated session
    pub fn scp_send_file(
        connection: &Connection,
        local_path: &str,
        remote_path: &str,
    ) -> Result<()> {
        use std::io::copy;
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let sess = Self::make_session(connection)?;

        // Prepare local file and metadata
        let mut file = File::open(local_path)?;
        let meta = file.metadata()?;
        let size = meta.len();
        let perms_i32: i32 = {
            #[cfg(unix)]
            {
                (meta.permissions().mode() as i32) & 0o777
            }
            #[cfg(not(unix))]
            {
                0o644
            }
        };

        // Start SCP send
        let remote = Path::new(remote_path);
        let mut ch = sess
            .scp_send(remote, perms_i32, size, None)
            .map_err(|e| AppError::SshConnectionError(format!("SCP send failed: {}", e)))?;

        copy(&mut file, &mut ch)?;

        // close the channel after sending
        let _ = ch.send_eof();
        let _ = ch.wait_eof();
        let _ = ch.close();
        let _ = ch.wait_close();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::config::manager::AuthMethod;

    use super::*;

    #[test]
    #[ignore = "requires a running ssh server"]
    fn test_connect_docker() {
        let conn = Connection::new(
            "127.0.0.1".to_string(),
            2222,
            "dockeruser".to_string(),
            AuthMethod::Password("dockerpass".to_string()),
        );
        let client = SshClient::connect(&conn).unwrap();
        client.close();
    }

    #[test]
    #[ignore = "requires a running orbstack ssh server"]
    fn test_connect_orbstack() {
        // https://docs.orbstack.dev/machines/ssh#connection-details
        let conn = Connection::new(
            "127.0.0.1".to_string(),
            32222,
            "default".to_string(),
            AuthMethod::PublicKey {
                private_key_path: "~/.orbstack/ssh/id_ed25519".to_string(),
                passphrase: None,
            },
        );
        let client = SshClient::connect(&conn).unwrap();
        client.close();
    }
}
