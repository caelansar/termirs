use std::env;
use std::path::PathBuf;
use std::sync::{Arc};

use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::{self, ssh_key, PrivateKeyWithHashAlg};
use russh::{ChannelMsg, Disconnect, MethodKind};
use tokio::sync::Mutex;

use crate::config::manager::{AuthMethod, Connection};
use crate::error::{AppError, Result};
use crate::ssh_client::ByteProcessor;

struct SshClient {}

impl client::Handler for SshClient {
    type Error = AppError;

    async fn check_server_key(
        &mut self,
        _server_public_key: &ssh_key::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        Ok(true)
    }
}

pub struct SshSession {
    session: Option<client::Handle<SshClient>>,
    channel: Arc<Mutex<russh::Channel<client::Msg>>>,
}

impl Clone for SshSession {
    fn clone(&self) -> Self {
        Self { session: None, channel: Arc::clone(&self.channel) }
    }
}

impl SshSession {
    pub async fn connect(connection: &Connection) -> Result<Self> {
        let config = client::Config {
            inactivity_timeout: Some(std::time::Duration::from_secs(5)),
            ..Default::default()
        };

        let config = Arc::new(config);
        let mut session = client::connect(config, connection.host_port(), SshClient {}).await?;

        let auth_result = session.authenticate_none(&connection.username).await?;
        let mut interactive = false;
        if let AuthResult::Failure {
            remaining_methods, ..
        } = auth_result
        {
            if remaining_methods.contains(&MethodKind::KeyboardInteractive) {
                interactive = true;
            }
        }

        match &connection.auth_method {
            AuthMethod::Password(password) => {
                if interactive {
                    let mut step1 = session
                        .authenticate_keyboard_interactive_start(&connection.username, None)
                        .await?;

                    loop {
                        match step1 {
                            KeyboardInteractiveAuthResponse::Success => {
                                println!("Authentication successful");
                                break;
                            }
                            KeyboardInteractiveAuthResponse::Failure { .. } => {
                                return Err(AppError::AuthenticationError(
                                    "Authentication failed".to_string(),
                                ));
                            }
                            KeyboardInteractiveAuthResponse::InfoRequest {
                                ref prompts, ..
                            } => {
                                if prompts.is_empty() {
                                    step1 = session
                                        .authenticate_keyboard_interactive_respond(vec![])
                                        .await?;
                                } else {
                                    step1 = session
                                        .authenticate_keyboard_interactive_respond(vec![
                                            password.clone(),
                                        ])
                                        .await?;
                                }
                            }
                        }
                    }
                } else {
                    let auth_result = session
                        .authenticate_password(&connection.username, password)
                        .await?;
                    if !auth_result.success() {
                        return Err(AppError::AuthenticationError(
                            "Authentication failed".to_string(),
                        ));
                    }
                }
            }
            AuthMethod::PublicKey { private_key_path, passphrase } => {
                let algo = session.best_supported_rsa_hash().await?.flatten();

                let key_path = if private_key_path.starts_with("~/") {
                    let home = env::var_os("HOME").ok_or_else(|| {
                        AppError::SshConnectionError("HOME environment variable is not set".to_string())
                    })?;
                    PathBuf::from(home).join(&private_key_path[2..])
                } else {
                    PathBuf::from(private_key_path)
                };

                let private_key = keys::load_secret_key(key_path, passphrase.as_deref()).map_err(|e| AppError::AuthenticationError(e.to_string()))?;
                let private_key_with_hash_alg = PrivateKeyWithHashAlg::new(Arc::new(private_key), algo);

                let auth_result = session.authenticate_publickey(&connection.username, private_key_with_hash_alg).await?;
                if !auth_result.success() {
                    return Err(AppError::AuthenticationError(
                        "Authentication failed".to_string(),
                    ));
                }
            }
        }

        let channel = session.channel_open_session().await?;
        channel
            .request_pty(true, "xterm-256color", 80, 120, 0, 0, &[])
            .await?;
        // Start an interactive shell to mirror ssh2-based client behavior
        channel.request_shell(true).await?;

        Ok(Self { session: Some(session), channel: Arc::new(Mutex::new(channel)) })
    }

    pub async fn request_size(&self, cols: u16, rows: u16) {
        let _ = self
            .channel.lock().await
            .window_change(cols as u32, rows as u32, 0, 0)
            .await;
    }

    pub async fn write_all(&self, data: &[u8]) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        // Create a writer handle without holding the lock across awaits
        let mut writer = {
            let ch = self.channel.lock().await;
            ch.make_writer()
        };

        writer
            .write_all(data)
            .await
            .map_err(|e| AppError::SshWriteError(format!("Failed to write to SSH channel: {}", e)))?;
        Ok(())
    }

    pub async fn read_loop<B: ByteProcessor>(&mut self, processor: Arc<std::sync::Mutex<B>>) {
        loop {
            // Wait for next message; we hold the lock only while awaiting one message
            let msg_opt = {
                let mut ch = self.channel.lock().await;
                ch.wait().await
            };

            let Some(msg) = msg_opt else { break };

            match msg {
                ChannelMsg::Data { data } => {
                    if let Ok(mut guard) = processor.lock() {
                        guard.process_bytes(&data);
                    }
                }
                ChannelMsg::ExtendedData { data, .. } => {
                    if let Ok(mut guard) = processor.lock() {
                        guard.process_bytes(&data);
                    }
                }
                ChannelMsg::Eof | ChannelMsg::Close => {
                    break;
                }
                _ => {
                    // Ignore other control messages
                }
            }
        }
    }

    pub async fn close(& self) -> Result<()> {
        if let Some(session) = &self.session {
            session.disconnect(Disconnect::ByApplication, "", "").await?;
        }
        Ok(())
    }

    pub async fn scp_send_file(&self, local_path: &str, remote_path: &str) -> Result<()> {
        panic!("Not implemented");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoByteProcessor {
    }
    impl ByteProcessor for EchoByteProcessor {
        fn process_bytes(&mut self, bytes: &[u8]) {
            println!("Received bytes:\n {}", String::from_utf8_lossy(bytes));
        }
    }

    #[tokio::test]
    #[ignore = "requires a running ssh server"]
    async fn test_connect_docker() {
        let conn = Connection::new(
            "127.0.0.1".to_string(),
            2222,
            "dockeruser".to_string(),
            AuthMethod::Password("dockerpass".to_string()),
        );
        let client = SshSession::connect(&conn).await.unwrap();

        let mut client_clone = client.clone();
        tokio::spawn(async move {
            client_clone.read_loop(Arc::new(std::sync::Mutex::new(EchoByteProcessor {}))).await;
        });

        client.write_all(b"pwd\n").await.unwrap();

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        client.close().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a running ssh server"]
    async fn test_connect_interactive_keyboard() {
        let conn = Connection::new(
            "192.168.1.1".to_string(),
            22,
            "root".to_string(),
            AuthMethod::Password("password".to_string()),
        );
        let client = SshSession::connect(&conn).await.unwrap();
        client.close().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a running orbstack ssh server"]
    async fn test_connect_orbstack() {
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
        let client = SshSession::connect(&conn).await.unwrap();
        client.close().await.unwrap();
    }
}
