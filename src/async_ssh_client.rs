use std::collections::VecDeque;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::io::AsyncReadExt;
use tokio_util;

use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::{self, PrivateKeyWithHashAlg, ssh_key};
use russh::{ChannelMsg, Disconnect, MethodKind};
use russh_sftp::client::rawsession::RawSftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};

use crate::config::manager::{AuthMethod, Connection};
use crate::error::{AppError, Result};

pub(crate) trait ByteProcessor {
    fn process_bytes(&mut self, bytes: &[u8]);
}

/// Buffer pool for efficient memory reuse during SFTP transfers
struct BufferPool {
    pool: Arc<tokio::sync::Mutex<VecDeque<BytesMut>>>,
    buffer_size: usize,
    max_buffers: usize,
}

impl BufferPool {
    fn new(buffer_size: usize, max_buffers: usize) -> Self {
        Self {
            pool: Arc::new(tokio::sync::Mutex::new(VecDeque::with_capacity(
                max_buffers,
            ))),
            buffer_size,
            max_buffers,
        }
    }

    async fn get_buffer(&self) -> BytesMut {
        let mut pool = self.pool.lock().await;
        if let Some(mut buffer) = pool.pop_front() {
            // Clear and resize the buffer to the expected size
            buffer.clear();
            buffer.resize(self.buffer_size, 0);
            buffer
        } else {
            // Create a new buffer if pool is empty
            BytesMut::zeroed(self.buffer_size)
        }
    }

    async fn return_buffer(&self, buffer: BytesMut) {
        let mut pool = self.pool.lock().await;
        if pool.len() < self.max_buffers {
            pool.push_back(buffer);
        }
        // If pool is full, just drop the buffer
    }
}

struct SshClient {
    connection: Connection,
    server_key: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl client::Handler for SshClient {
    type Error = AppError;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        // Encode the server public key in OpenSSH format
        let server_key_openssh = server_public_key.to_openssh().map_err(|e| {
            AppError::SshPublicKeyValidationError(format!("Failed to encode server key: {}", e))
        })?;

        // Store the server key for later validation
        {
            let mut key_guard = self.server_key.lock().await;
            *key_guard = Some(server_key_openssh.clone());
        }

        // Check if connection already has a stored public key
        if let Some(stored_key) = &self.connection.public_key {
            // Compare stored key with server key
            if stored_key == &server_key_openssh {
                return Ok(true);
            } else {
                return Err(AppError::SshPublicKeyValidationError(format!(
                    "Server public key mismatch for {}:{}. Expected: {}, Got: {}",
                    self.connection.host, self.connection.port, stored_key, server_key_openssh
                )));
            }
        }

        // No stored key, accept it for now - we'll save it after successful connection
        Ok(true)
    }
}

pub struct SshSession {
    session: Option<client::Handle<SshClient>>,
    r: Arc<tokio::sync::Mutex<russh::ChannelReadHalf>>,
    w: Arc<russh::ChannelWriteHalf<client::Msg>>,
    server_key: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl Clone for SshSession {
    fn clone(&self) -> Self {
        Self {
            session: None,
            r: Arc::clone(&self.r),
            w: Arc::clone(&self.w),
            server_key: Arc::clone(&self.server_key),
        }
    }
}

impl SshSession {
    async fn new_session_with_timeout(
        connection: &Connection,
        timeout: Option<Duration>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<(
        client::Handle<SshClient>,
        Arc<tokio::sync::Mutex<Option<String>>>,
    )> {
        let mut config = client::Config::default();
        config.keepalive_interval = Some(std::time::Duration::from_secs(30));
        config.keepalive_max = 3;

        let config = Arc::new(config);
        let server_key = Arc::new(tokio::sync::Mutex::new(None));
        let ssh_client = SshClient {
            connection: connection.clone(),
            server_key: server_key.clone(),
        };

        let mut session = {
            let f = async || {
                let session = client::connect(config, connection.host_port(), ssh_client).await?;
                Ok::<_, AppError>(session)
            };
            cancellable_timeout(timeout.unwrap_or(Duration::from_secs(10)), f, cancel).await?
        };

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
                            "Password authentication failed".to_string(),
                        ));
                    }
                }
            }
            AuthMethod::PublicKey {
                private_key_path,
                passphrase,
            } => {
                let algo = session.best_supported_rsa_hash().await?.flatten();
                let key_path = if private_key_path.starts_with("~/") {
                    let home = env::var_os("HOME").ok_or_else(|| {
                        AppError::SshConnectionError(
                            "HOME environment variable is not set".to_string(),
                        )
                    })?;
                    PathBuf::from(home).join(&private_key_path[2..])
                } else {
                    PathBuf::from(private_key_path)
                };
                let private_key = keys::load_secret_key(key_path, passphrase.as_deref())
                    .map_err(|e| AppError::AuthenticationError(e.to_string()))?;
                let private_key_with_hash_alg =
                    PrivateKeyWithHashAlg::new(Arc::new(private_key), algo);
                let auth_result = session
                    .authenticate_publickey(&connection.username, private_key_with_hash_alg)
                    .await?;
                if !auth_result.success() {
                    return Err(AppError::AuthenticationError(
                        "Public key authentication failed".to_string(),
                    ));
                }
            }
        }

        Ok((session, server_key))
    }

    async fn new_session(
        connection: &Connection,
    ) -> Result<(
        client::Handle<SshClient>,
        Arc<tokio::sync::Mutex<Option<String>>>,
    )> {
        Self::new_session_with_timeout(
            connection,
            None,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    pub async fn connect(connection: &Connection) -> Result<Self> {
        let (session, server_key) = Self::new_session(connection).await?;

        let channel = session.channel_open_session().await?;
        channel
            .request_pty(true, "xterm-256color", 80, 120, 0, 0, &[])
            .await?;
        channel.request_shell(true).await?;

        // Build a writer from the channel upfront to avoid later locking the channel to create it
        // let writer: Box<dyn tokio::io::AsyncWrite + Send + Unpin> = Box::new(channel.make_writer());

        let (r, w) = channel.split();

        Ok(Self {
            session: Some(session),
            r: Arc::new(tokio::sync::Mutex::new(r)),
            w: Arc::new(w),
            server_key,
        })
    }

    pub async fn request_size(&self, cols: u16, rows: u16) {
        let _ = self.w.window_change(cols as u32, rows as u32, 0, 0).await;
    }

    pub async fn write_all(&self, data: &[u8]) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let mut writer = self.w.make_writer();
        writer.write_all(data).await.map_err(|e| {
            AppError::SshWriteError(format!("Failed to write to SSH channel: {}", e))
        })?;
        Ok(())
    }

    pub async fn read_loop<B: ByteProcessor>(
        &mut self,
        processor: Arc<tokio::sync::Mutex<B>>,
        cancel: tokio_util::sync::CancellationToken,
        event_tx: Option<tokio::sync::mpsc::Sender<crate::AppEvent>>,
    ) {
        loop {
            let msg_opt = {
                let mut ch = self.r.lock().await;
                // Add cancellation and timeout support
                tokio::select! {
                    _ = cancel.cancelled() => {
                        // Task was cancelled, exit cleanly
                        break;
                    }
                    result = tokio::time::timeout(Duration::from_millis(100), ch.wait()) => {
                        match result {
                            Ok(msg) => msg,
                            Err(_) => continue, // Timeout, continue loop with small delay
                        }
                    }
                }
            };
            let Some(msg) = msg_opt else { break };
            match msg {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    let mut guard = processor.lock().await;
                    guard.process_bytes(&data);
                }
                ChannelMsg::Eof | ChannelMsg::Close | ChannelMsg::ExitStatus { .. } => {
                    // Notify the main loop that the connection has been disconnected
                    if let Some(tx) = &event_tx {
                        let _ = tx.send(crate::AppEvent::Disconnect).await;
                    }
                    break;
                }
                _ => {}
            }
        }
    }

    pub async fn close(&self) -> Result<()> {
        if let Some(session) = &self.session {
            session
                .disconnect(Disconnect::ByApplication, "", "")
                .await?;
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn close_channel(&self) -> Result<()> {
        self.w.close().await?;
        Ok(())
    }

    /// Get the server public key that was received during connection
    pub async fn get_server_key(&self) -> Option<String> {
        let key_guard = self.server_key.lock().await;
        key_guard.clone()
    }

    pub async fn sftp_send_file_with_timeout(
        connection: &Connection,
        local_path: &str,
        remote_path: &str,
        timeout: Option<Duration>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        // let now = std::time::Instant::now();

        let (session, _server_key) =
            Self::new_session_with_timeout(connection, timeout, cancel).await?;

        let channel = session.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;

        // Create RawSftpSession for better performance
        let sftp = RawSftpSession::new(channel.into_stream());

        // Initialize the SFTP session
        sftp.init()
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to initialize SFTP: {}", e)))?;

        // Open local file and get its size
        let mut local_file = tokio::fs::File::open(expand_tilde(local_path)).await?;
        let file_size = local_file.metadata().await?.len();

        // Open remote file using RawSftpSession
        let remote_handle = sftp
            .open(
                remote_path,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
                FileAttributes::empty(),
            )
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to open remote file: {}", e)))?;

        // Use optimal buffer size for SFTP protocol (128KB for better throughput)
        const CHUNK_SIZE: usize = 128 * 1024; // 128KB - good balance between memory and throughput
        const MAX_CONCURRENT_WRITES: usize = 12; // Reasonable number of concurrent operations

        let mut bytes_written = 0u64;
        let mut offset = 0u64;
        let mut last_progress_logged = 0u64;
        let mut write_futures = FuturesUnordered::new();
        let mut eof_reached = false;

        // Set a shorter timeout for faster operations
        sftp.set_timeout(3).await;

        // Wrap sftp in Arc to share between tasks
        let sftp = Arc::new(sftp);

        // Create buffer pool for efficient memory reuse
        let buffer_pool = Arc::new(BufferPool::new(CHUNK_SIZE, MAX_CONCURRENT_WRITES * 2));

        // Optimized pipeline logic: true concurrent read and write
        loop {
            // Check exit condition
            if eof_reached && write_futures.is_empty() {
                break;
            }

            // Check if we can read more data
            let can_read = write_futures.len() < MAX_CONCURRENT_WRITES && !eof_reached;

            if can_read {
                // Try reading if we have capacity
                tokio::select! {
                    _ = cancel.cancelled() => {
                        return Err(AppError::SftpError("Transfer cancelled".to_string()));
                    }

                    // Try to read next chunk
                    read_result = async {
                        let mut buffer = buffer_pool.get_buffer().await;
                        let result = local_file.read(&mut buffer).await;
                        (buffer, result)
                    } => {
                        let (mut buffer, read_result) = read_result;
                        let bytes_read = read_result.map_err(|e| {
                            AppError::SftpError(format!("Failed to read local file: {}", e))
                        })?;

                        if bytes_read == 0 {
                            // EOF reached
                            buffer_pool.return_buffer(buffer).await;
                            eof_reached = true;
                        } else {
                            // Prepare data for concurrent write
                            let data: Bytes = buffer.split_to(bytes_read).freeze();
                            let current_offset = offset;
                            offset += bytes_read as u64;

                            // Create concurrent write future
                            let handle = remote_handle.handle.clone();
                            let chunk_size = bytes_read as u64;
                            let sftp_clone = Arc::clone(&sftp);

                            let write_future = async move {
                                let result = sftp_clone.write(&handle, current_offset, data.to_vec()).await;
                                (chunk_size, result)
                            };

                            write_futures.push(write_future);

                            // Return buffer to pool
                            buffer_pool.return_buffer(buffer).await;
                        }
                    }

                    // Also process any completed writes while reading
                    write_result = write_futures.next(), if !write_futures.is_empty() => {
                        if let Some(result) = write_result {
                            let (chunk_size, write_res) = result;
                            write_res.map_err(|e| {
                                AppError::SftpError(format!("Failed to write chunk: {}", e))
                            })?;

                            bytes_written += chunk_size;

                            // Log progress for large files (every 5MB)
                            if file_size > 5 * 1024 * 1024 && bytes_written - last_progress_logged >= 5 * 1024 * 1024 {
                                // eprintln!("Progress: {:.1}% ({} / {} bytes), pipeline: {}",
                                //     (bytes_written as f64 / file_size as f64) * 100.0,
                                //     bytes_written,
                                //     file_size,
                                //     write_futures.len()
                                // );
                                last_progress_logged = bytes_written;
                            }
                        }
                    }
                }
            } else {
                // Pipeline is full, only process writes
                tokio::select! {
                    _ = cancel.cancelled() => {
                        return Err(AppError::SftpError("Transfer cancelled".to_string()));
                    }
                    write_result = write_futures.next() => {
                        if let Some(result) = write_result {
                            let (chunk_size, write_res) = result;
                            write_res.map_err(|e| {
                                AppError::SftpError(format!("Failed to write chunk: {}", e))
                            })?;

                            bytes_written += chunk_size;

                            // Log progress for large files (every 5MB)
                            if file_size > 5 * 1024 * 1024 && bytes_written - last_progress_logged >= 5 * 1024 * 1024 {
                                // eprintln!("Progress: {:.1}% ({} / {} bytes), pipeline: {}",
                                //     (bytes_written as f64 / file_size as f64) * 100.0,
                                //     bytes_written,
                                //     file_size,
                                //     write_futures.len()
                                // );
                                last_progress_logged = bytes_written;
                            }
                        }
                    }
                }
            }
        }

        // Close the remote file handle
        sftp.close(&remote_handle.handle)
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to close remote file: {}", e)))?;

        // eprintln!(
        //     "Transfer completed: {} bytes in {:?}, speed: {:.2} MB/s",
        //     bytes_written,
        //     now.elapsed(),
        //     bytes_written as f64 / now.elapsed().as_secs_f64() / 1024.0 / 1024.0
        // );

        Ok(())
    }

    pub async fn sftp_send_file(
        connection: &Connection,
        local_path: &str,
        remote_path: &str,
    ) -> Result<()> {
        Self::sftp_send_file_with_timeout(
            connection,
            local_path,
            remote_path,
            None,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
    }
}

async fn cancellable_timeout<F, T>(
    dur: Duration,
    f: F,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<T>
where
    F: AsyncFnOnce() -> Result<T>,
{
    tokio::select! {
        _ = cancel.cancelled() => Err(AppError::SshConnectionError("cancelled".to_string())),
        res = tokio::time::timeout(dur, f()) => {
            match res {
                Ok(inner) => inner,
                Err(_) => Err(AppError::SshConnectionError("timeout".to_string())),
            }
        }
    }
}

pub fn expand_tilde(input: &str) -> PathBuf {
    if let Some(stripped) = input.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            return PathBuf::from(home).join(stripped);
        }
    } else if input == "~" {
        if let Ok(home) = env::var("HOME") {
            return PathBuf::from(home);
        }
    }

    PathBuf::from(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoByteProcessor {}
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
        let cancel_token = tokio_util::sync::CancellationToken::new();
        tokio::spawn(async move {
            client_clone
                .read_loop(
                    Arc::new(tokio::sync::Mutex::new(EchoByteProcessor {})),
                    cancel_token,
                    None, // No event sender for test
                )
                .await;
        });

        // make sure the read_loop is started before writing
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        client.write_all(b"pwd\n").await.unwrap();

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

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

    #[tokio::test]
    async fn test_connect_cancel() {
        let conn = Connection::new(
            "127.0.0.2".to_string(),
            2222,
            "dockeruser".to_string(),
            AuthMethod::Password("dockerpass".to_string()),
        );

        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            cancel.cancel();
        });

        let res = SshSession::new_session_with_timeout(
            &conn,
            Some(Duration::from_secs(5)),
            &cancel_clone,
        )
        .await;

        if let Err(AppError::SshConnectionError(e)) = res {
            assert_eq!(e, "cancelled");
        } else {
            unreachable!();
        }
    }

    #[tokio::test]
    async fn test_connect_timeout() {
        let conn = Connection::new(
            "127.0.0.2".to_string(),
            2222,
            "dockeruser".to_string(),
            AuthMethod::Password("dockerpass".to_string()),
        );

        let cancel = tokio_util::sync::CancellationToken::new();

        let res =
            SshSession::new_session_with_timeout(&conn, Some(Duration::from_secs(1)), &cancel)
                .await;

        if let Err(AppError::SshConnectionError(e)) = res {
            assert_eq!(e, "timeout");
        } else {
            unreachable!();
        }
    }
}
