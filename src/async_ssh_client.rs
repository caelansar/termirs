use std::collections::{HashMap, VecDeque};
use std::convert::TryFrom;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::{self, PrivateKeyWithHashAlg, ssh_key};
use russh::{Channel, ChannelMsg, Disconnect, MethodKind};
use russh_sftp::client::rawsession::RawSftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::net::TcpListener;

use crate::config::manager::{AuthMethod, Connection, PortForward};
use crate::error::{AppError, Result};

const STANDARD_KEY_PATHS: &[&str] = &[
    "~/.ssh/id_rsa",
    "~/.ssh/id_ecdsa",
    "~/.ssh/id_ecdsa_sk",
    "~/.ssh/id_ed25519",
    "~/.ssh/id_ed25519_sk",
];

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

pub struct SshClient {
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
            AppError::SshPublicKeyValidationError(format!("Failed to encode server key: {e}"))
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
    session: Arc<tokio::sync::Mutex<Option<client::Handle<SshClient>>>>,
    r: Arc<tokio::sync::Mutex<russh::ChannelReadHalf>>,
    w: Arc<russh::ChannelWriteHalf<client::Msg>>,
    server_key: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl Clone for SshSession {
    fn clone(&self) -> Self {
        Self {
            session: Arc::clone(&self.session),
            r: Arc::clone(&self.r),
            w: Arc::clone(&self.w),
            server_key: Arc::clone(&self.server_key),
        }
    }
}

impl SshSession {
    pub(crate) async fn new_session_with_timeout(
        connection: &Connection,
        timeout: Option<Duration>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<(
        client::Handle<SshClient>,
        Arc<tokio::sync::Mutex<Option<String>>>,
    )> {
        let config = client::Config {
            keepalive_interval: Some(std::time::Duration::from_secs(30)),
            keepalive_max: 3,
            ..Default::default()
        };

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

        Self::authenticate_session(&mut session, connection).await?;

        Ok((session, server_key))
    }

    async fn authenticate_session(
        session: &mut client::Handle<SshClient>,
        connection: &Connection,
    ) -> Result<()> {
        let username = &connection.username;
        let mut attempted = Vec::new();
        let mut auth_result = session.authenticate_none(username).await?;

        loop {
            if auth_result.success() {
                return Ok(());
            }

            let methods: Vec<MethodKind> = match &auth_result {
                AuthResult::Failure {
                    remaining_methods, ..
                } => remaining_methods.iter().copied().collect(),
                AuthResult::Success => unreachable!(),
            };
            let Some(next_method) = methods.iter().copied().find(|method| {
                !attempted.contains(method)
                    && Self::supports_method(*method, &connection.auth_method)
            }) else {
                let offered = Self::format_method_list(&methods);
                return Err(AppError::AuthenticationError(format!(
                    "Server does not offer a supported authentication method. Offered: {offered}"
                )));
            };

            attempted.push(next_method);
            auth_result = match next_method {
                MethodKind::Password => {
                    let password = Self::password_from_auth(&connection.auth_method)?;
                    session.authenticate_password(username, password).await?
                }
                MethodKind::KeyboardInteractive => {
                    let password = Self::password_from_auth(&connection.auth_method)?;
                    Self::authenticate_keyboard_interactive(session, username, password).await?;
                    AuthResult::Success
                }
                MethodKind::PublicKey => match &connection.auth_method {
                    AuthMethod::AutoLoadKey => {
                        Self::authenticate_auto_load_key(session, username).await?
                    }
                    _ => {
                        Self::authenticate_public_key(session, username, &connection.auth_method)
                            .await?
                    }
                },
                MethodKind::None | MethodKind::HostBased => unreachable!(),
            };
        }
    }

    fn supports_method(method: MethodKind, auth_method: &AuthMethod) -> bool {
        matches!(
            (method, auth_method),
            (MethodKind::Password, AuthMethod::Password(_))
                | (MethodKind::KeyboardInteractive, AuthMethod::Password(_))
                | (MethodKind::PublicKey, AuthMethod::PublicKey { .. })
                | (MethodKind::PublicKey, AuthMethod::AutoLoadKey)
        )
    }

    fn format_method_list(methods: &[MethodKind]) -> String {
        if methods.is_empty() {
            return "none".to_string();
        }
        methods
            .iter()
            .map(|method| {
                let label: &'static str = method.into();
                label
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    async fn authenticate_keyboard_interactive(
        session: &mut client::Handle<SshClient>,
        username: &str,
        password: &str,
    ) -> Result<()> {
        let mut response = session
            .authenticate_keyboard_interactive_start(username, None)
            .await?;

        loop {
            match response {
                KeyboardInteractiveAuthResponse::Success => break,
                KeyboardInteractiveAuthResponse::Failure { .. } => {
                    return Err(AppError::AuthenticationError(
                        "Keyboard-interactive authentication failed".to_string(),
                    ));
                }
                KeyboardInteractiveAuthResponse::InfoRequest { ref prompts, .. } => {
                    let responses = if prompts.is_empty() {
                        Vec::new()
                    } else {
                        vec![password.to_owned(); prompts.len()]
                    };
                    response = session
                        .authenticate_keyboard_interactive_respond(responses)
                        .await?;
                }
            }
        }

        Ok(())
    }

    async fn authenticate_public_key(
        session: &mut client::Handle<SshClient>,
        username: &str,
        auth_method: &AuthMethod,
    ) -> Result<AuthResult> {
        let AuthMethod::PublicKey {
            private_key_path,
            passphrase,
        } = auth_method
        else {
            return Err(AppError::AuthenticationError(
                "Server requested public key authentication, but the connection is not configured for it".to_string(),
            ));
        };

        let key_path = Self::resolve_private_key_path(private_key_path)?;
        let algo = session.best_supported_rsa_hash().await?.flatten();
        let private_key = keys::load_secret_key(key_path, passphrase.as_deref())
            .map_err(|e| AppError::AuthenticationError(e.to_string()))?;

        let private_key_with_hash_alg = PrivateKeyWithHashAlg::new(Arc::new(private_key), algo);
        let result = session
            .authenticate_publickey(username, private_key_with_hash_alg)
            .await?;
        Ok(result)
    }

    async fn authenticate_auto_load_key(
        session: &mut client::Handle<SshClient>,
        username: &str,
    ) -> Result<AuthResult> {
        let mut last_error = None;

        for key_path in STANDARD_KEY_PATHS {
            let expanded_path = match Self::resolve_private_key_path(key_path) {
                Ok(path) => path,
                Err(_) => continue,
            };

            // Skip if key doesn't exist
            if !expanded_path.exists() {
                continue;
            }

            // Try loading key with no passphrase (skip if encrypted)
            let private_key = match keys::load_secret_key(&expanded_path, None) {
                Ok(key) => key,
                Err(_) => {
                    last_error = Some(format!(
                        "Key at {} requires passphrase or is invalid",
                        key_path
                    ));
                    continue;
                }
            };

            let algo = match session.best_supported_rsa_hash().await {
                Ok(algo) => algo.flatten(),
                Err(_) => continue,
            };
            let private_key_with_hash_alg = PrivateKeyWithHashAlg::new(Arc::new(private_key), algo);

            match session
                .authenticate_publickey(username, private_key_with_hash_alg)
                .await
            {
                Ok(result) if result.success() => return Ok(result),
                Ok(_) | Err(_) => {
                    last_error = Some(format!("Authentication failed with key: {}", key_path));
                    continue;
                }
            }
        }

        Err(AppError::AuthenticationError(format!(
            "Auto-load key authentication failed. Tried standard key paths but none worked. {}",
            last_error.unwrap_or_default()
        )))
    }

    fn password_from_auth(auth_method: &AuthMethod) -> Result<&str> {
        if let AuthMethod::Password(password) = auth_method {
            Ok(password.as_str())
        } else {
            Err(AppError::AuthenticationError(
                "Server requested password authentication, but the connection is not configured for it".to_string(),
            ))
        }
    }

    fn resolve_private_key_path(private_key_path: &str) -> Result<PathBuf> {
        if let Some(stripped) = private_key_path.strip_prefix("~/") {
            let home = env::var_os("HOME").ok_or_else(|| {
                AppError::SshConnectionError("HOME environment variable is not set".to_string())
            })?;
            Ok(PathBuf::from(home).join(stripped))
        } else if private_key_path == "~" {
            let home = env::var_os("HOME").ok_or_else(|| {
                AppError::SshConnectionError("HOME environment variable is not set".to_string())
            })?;
            Ok(PathBuf::from(home))
        } else {
            Ok(PathBuf::from(private_key_path))
        }
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
            session: Arc::new(tokio::sync::Mutex::new(Some(session))),
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
        writer
            .write_all(data)
            .await
            .map_err(|e| AppError::SshWriteError(format!("Failed to write to SSH channel: {e}")))?;
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
        let guard = self.session.lock().await;
        if let Some(session) = guard.as_ref() {
            session
                .disconnect(Disconnect::ByApplication, "", "")
                .await
                .map_err(|e| AppError::SshConnectionError(format!("Failed to disconnect: {e}")))?;
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
        channel: Option<Channel<client::Msg>>,
        connection: &Connection,
        local_path: &str,
        remote_path: &str,
        timeout: Option<Duration>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        // let now = std::time::Instant::now();

        let channel = if let Some(channel) = channel {
            channel
        } else {
            let (session, _server_key) =
                Self::new_session_with_timeout(connection, timeout, cancel).await?;

            session.channel_open_session().await?
        };
        channel.request_subsystem(true, "sftp").await?;

        // Create RawSftpSession for better performance
        let sftp = RawSftpSession::new(channel.into_stream());

        // Initialize the SFTP session
        sftp.init()
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to initialize SFTP: {e}")))?;

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
            .map_err(|e| AppError::SftpError(format!("Failed to open remote file: {e}")))?;

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
                            AppError::SftpError(format!("Failed to read local file: {e}"))
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
                                AppError::SftpError(format!("Failed to write chunk: {e}"))
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
                                AppError::SftpError(format!("Failed to write chunk: {e}"))
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
            .map_err(|e| AppError::SftpError(format!("Failed to close remote file: {e}")))?;

        // eprintln!(
        //     "Transfer completed: {} bytes in {:?}, speed: {:.2} MB/s",
        //     bytes_written,
        //     now.elapsed(),
        //     bytes_written as f64 / now.elapsed().as_secs_f64() / 1024.0 / 1024.0
        // );

        Ok(())
    }

    pub async fn sftp_send_file(
        channel: Option<Channel<client::Msg>>,
        connection: &Connection,
        local_path: &str,
        remote_path: &str,
    ) -> Result<()> {
        Self::sftp_send_file_with_timeout(
            channel,
            connection,
            local_path,
            remote_path,
            None,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    pub async fn sftp_receive_file_with_timeout(
        channel: Option<Channel<client::Msg>>,
        connection: &Connection,
        remote_path: &str,
        local_path: &str,
        timeout: Option<Duration>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        let channel = if let Some(ch) = channel {
            ch
        } else {
            let (session, _server_key) =
                Self::new_session_with_timeout(connection, timeout, cancel).await?;
            session.channel_open_session().await?
        };
        channel.request_subsystem(true, "sftp").await?;

        // Create RawSftpSession for better performance
        let sftp = RawSftpSession::new(channel.into_stream());

        // Initialize the SFTP session
        sftp.init()
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to initialize SFTP: {e}")))?;

        // Open remote file for reading
        let remote_handle = sftp
            .open(remote_path, OpenFlags::READ, FileAttributes::empty())
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to open remote file: {e}")))?;

        // For simplicity, we'll transfer without knowing the exact file size
        let file_size = 0u64;

        // Create local file for writing
        let mut local_file = tokio::fs::File::create(expand_tilde(local_path)).await?;

        // Use optimal buffer size for SFTP protocol (128KB for better throughput)
        const CHUNK_SIZE: usize = 128 * 1024; // 128KB - good balance between memory and throughput

        let mut bytes_read = 0u64;
        let mut offset = 0u64;
        let mut last_progress_logged = 0u64;

        // Set a shorter timeout for faster operations
        sftp.set_timeout(3).await;

        // Optimized pipeline logic: concurrent read and write with ordered writes
        const MAX_CONCURRENT_READS: usize = 12; // Reasonable number of concurrent operations
        let mut read_futures = FuturesUnordered::new();
        let mut write_queue: VecDeque<(u64, Vec<u8>)> = VecDeque::new();
        let mut next_write_offset = 0u64;
        let mut eof_reached = false;
        let mut pending_reads: VecDeque<(u64, u32)> = VecDeque::new();

        // Wrap sftp in Arc to share between tasks
        let sftp = Arc::new(sftp);

        loop {
            // Check exit condition
            if eof_reached
                && read_futures.is_empty()
                && write_queue.is_empty()
                && pending_reads.is_empty()
            {
                break;
            }

            // Check if we can read more data
            let can_read = read_futures.len() < MAX_CONCURRENT_READS
                && (!eof_reached || !pending_reads.is_empty());

            // Start new read operations if we have capacity
            if can_read {
                let (current_offset, requested_len) =
                    if let Some((pending_offset, pending_len)) = pending_reads.pop_front() {
                        (pending_offset, pending_len)
                    } else {
                        let next_offset = offset;
                        offset += CHUNK_SIZE as u64;
                        (next_offset, CHUNK_SIZE as u32)
                    };

                if requested_len > 0 {
                    let handle = remote_handle.handle.clone();
                    let sftp_clone = Arc::clone(&sftp);

                    let read_future = async move {
                        let result = sftp_clone
                            .read(&handle, current_offset, requested_len)
                            .await;
                        match result {
                            Ok(data) => Ok((current_offset, requested_len, data.data, false)),
                            Err(russh_sftp::client::error::Error::Status(status))
                                if status.status_code == StatusCode::Eof =>
                            {
                                Ok((current_offset, requested_len, Vec::new(), true))
                            }
                            Err(err) => Err(AppError::RusshSftpError(err)),
                        }
                    };

                    read_futures.push(read_future);
                }
            }

            // Process completed reads
            tokio::select! {
                _ = cancel.cancelled() => {
                    return Err(AppError::SftpError("Transfer cancelled".to_string()));
                }

                read_result = read_futures.next(), if !read_futures.is_empty() => {
                    if let Some(result) = read_result {
                        let result = result?;
                        let (read_offset, requested_len, data, is_eof) = result;
                        let bytes_in_chunk = data.len();
                        let bytes_in_chunk_u32 = u32::try_from(bytes_in_chunk).map_err(|_| {
                            AppError::SftpError(
                                "Received chunk larger than u32::MAX; unsupported transfer size"
                                    .to_string(),
                            )
                        })?;

                        if bytes_in_chunk_u32 == 0 || is_eof {
                            eof_reached = true;
                        } else {
                            // Add to write queue with offset for ordering
                            write_queue.push_back((read_offset, data));
                            write_queue.make_contiguous().sort_by_key(|(offset, _)| *offset);

                            if !is_eof && bytes_in_chunk_u32 < requested_len {
                                let remaining = requested_len - bytes_in_chunk_u32;
                                let next_offset = read_offset + u64::from(bytes_in_chunk_u32);
                                pending_reads.push_back((next_offset, remaining));
                            }
                        }
                    }
                }

                // If no reads are pending and we can't start new ones, just yield
                _ = tokio::task::yield_now(), if read_futures.is_empty() && !can_read => {}
            }

            // Process writes in order
            while let Some((write_offset, _)) = write_queue.front() {
                if *write_offset == next_write_offset {
                    let (_, data) = write_queue.pop_front().unwrap();

                    // Write data to local file
                    use tokio::io::AsyncWriteExt;
                    local_file.write_all(&data).await.map_err(|e| {
                        AppError::SftpError(format!("Failed to write to local file: {e}"))
                    })?;

                    next_write_offset += data.len() as u64;
                    bytes_read += data.len() as u64;

                    // Log progress for large files (every 5MB)
                    if file_size > 5 * 1024 * 1024
                        && bytes_read - last_progress_logged >= 5 * 1024 * 1024
                    {
                        last_progress_logged = bytes_read;
                    }
                } else {
                    break; // Wait for the next expected chunk
                }
            }
        }

        // Flush and close the local file
        use tokio::io::AsyncWriteExt;
        local_file
            .flush()
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to flush local file: {e}")))?;

        // Close the remote file handle
        sftp.close(&remote_handle.handle)
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to close remote file: {e}")))?;

        Ok(())
    }

    pub async fn sftp_receive_file(
        channel: Option<Channel<client::Msg>>,
        connection: &Connection,
        remote_path: &str,
        local_path: &str,
    ) -> Result<()> {
        Self::sftp_receive_file_with_timeout(
            channel,
            connection,
            remote_path,
            local_path,
            None,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    pub async fn open_session_channel(&self) -> Result<Channel<client::Msg>> {
        let guard = self.session.lock().await;
        let session = guard.as_ref().ok_or_else(|| {
            AppError::SshConnectionError("SSH session handle unavailable".to_string())
        })?;
        session
            .channel_open_session()
            .await
            .map_err(|e| AppError::SshConnectionError(format!("Failed to open channel: {e}")))
    }

    /// Start a port forwarding task and return the handle and cancellation token
    pub async fn start_port_forwarding_task(
        local_addr: &str,
        local_port: u16,
        connection: &Connection,
        service_host: &str,
        service_port: u16,
    ) -> Result<(JoinHandle<()>, CancellationToken)> {
        let (session, _) = Self::new_session(connection).await?;
        let cancel_token = CancellationToken::new();
        let cancel_token_for_task = cancel_token.clone();

        let local_addr = local_addr.to_string();
        let service_host = service_host.to_string();

        let local_listener = match TcpListener::bind((local_addr.as_str(), local_port)).await {
            Ok(listener) => listener,
            Err(e) => {
                return Err(AppError::PortForwardingError(format!(
                    "Failed to bind to {local_addr}:{local_port}: {e}"
                )));
            }
        };

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel_token_for_task.cancelled() => {
                        break;
                    }
                    result = local_listener.accept() => {
                        match result {
                            Ok((mut local_socket, _)) => {
                                let ssh_channel = match session
                                    .channel_open_direct_tcpip(
                                        service_host.clone(),
                                        service_port as u32,
                                        local_addr.clone(),
                                        local_port as u32,
                                    )
                                    .await
                                {
                                    Ok(channel) => channel,
                                    Err(e) => {
                                        eprintln!("Failed to open SSH forwarding channel: {}", e);
                                        continue;
                                    }
                                };

                                let mut ssh_stream = ssh_channel.into_stream();

                                // Handle the connection in a separate task
                                let cancel_for_connection = cancel_token_for_task.clone();
                                tokio::spawn(async move {
                                    tokio::select! {
                                        _ = cancel_for_connection.cancelled() => {
                                            // Connection cancelled
                                        }
                                        result = tokio::io::copy_bidirectional(&mut local_socket, &mut ssh_stream) => {
                                            if let Err(e) = result {
                                                eprintln!("Copy error between local socket and SSH stream: {}", e);
                                            }
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                eprintln!("Failed to accept connection: {}", e);
                                continue;
                            }
                        }
                    }
                }
            }
        });

        Ok((handle, cancel_token))
    }
}

/// Runtime management for port forwarding sessions
pub struct PortForwardingRuntime {
    active_forwards: Arc<Mutex<HashMap<String, (JoinHandle<()>, CancellationToken)>>>,
}

impl PortForwardingRuntime {
    pub fn new() -> Self {
        Self {
            active_forwards: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Start a port forwarding session
    pub async fn start_port_forward(
        &self,
        port_forward: &PortForward,
        connection: &Connection,
    ) -> Result<()> {
        let pf_id = port_forward.id.clone();

        // Check if already running
        if self.is_running(&pf_id).await {
            return Err(AppError::ValidationError(
                "Port forward is already running".to_string(),
            ));
        }

        // Start the port forwarding task
        let (handle, cancel_token) = SshSession::start_port_forwarding_task(
            &port_forward.local_addr,
            port_forward.local_port,
            connection,
            &port_forward.service_host,
            port_forward.service_port,
        )
        .await?;

        // Store the handle and cancellation token
        let mut active_forwards = self.active_forwards.lock().await;
        active_forwards.insert(pf_id, (handle, cancel_token));

        Ok(())
    }

    /// Stop a port forwarding session
    pub async fn stop_port_forward(&self, port_forward_id: &str) -> Result<()> {
        let mut active_forwards = self.active_forwards.lock().await;

        if let Some((handle, cancel_token)) = active_forwards.remove(port_forward_id) {
            // Cancel the task
            cancel_token.cancel();

            // Wait for the task to complete (with timeout)
            tokio::select! {
                _ = handle => {
                    // Task completed normally
                }
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    // Timeout - task might still be running but we've cancelled it
                }
            }
        }

        Ok(())
    }

    /// Check if a port forward is currently running
    pub async fn is_running(&self, port_forward_id: &str) -> bool {
        let active_forwards = self.active_forwards.lock().await;
        active_forwards.contains_key(port_forward_id)
    }

    /// Stop all port forwarding sessions
    #[allow(dead_code)]
    pub async fn stop_all(&self) -> Result<()> {
        let mut active_forwards = self.active_forwards.lock().await;

        for (_, (handle, cancel_token)) in active_forwards.drain() {
            cancel_token.cancel();

            // Wait for task completion with timeout
            tokio::select! {
                _ = handle => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }

        Ok(())
    }
}

impl Default for PortForwardingRuntime {
    fn default() -> Self {
        Self::new()
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
    use std::io;
    use std::sync::Arc;

    use russh::server::{self, Auth, Msg, Server as _, Session};
    use russh::{Channel, ChannelId, CryptoVec, MethodKind, MethodSet, Pty};
    use tokio::io::AsyncWriteExt as _;
    use tokio::net::{TcpListener, TcpStream};

    const TEST_SERVER_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n\
QyNTUxOQAAACAnHPG3J8U6lMZEixxg5IsP7JhRjl6nr2elNYTDWinZRQAAAJDL3Tply906\n\
ZQAAAAtzc2gtZWQyNTUxOQAAACAnHPG3J8U6lMZEixxg5IsP7JhRjl6nr2elNYTDWinZRQ\n\
AAAECZgBqNRwO+b/Gi/IeJMkbw3GT0jje9jiCsrzFCjLpLoycc8bcnxTqUxkSLHGDkiw/s\n\
mFGOXqevZ6U1hMNaKdlFAAAACXRlc3RAdGVzdAECAwQ=\n\
-----END OPENSSH PRIVATE KEY-----";

    const TEST_CLIENT_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n\
QyNTUxOQAAACD5HoUzlZEiEcszvrgjoVwm7ZFgnM0dzXwCF4+hzSeQxAAAAJjYpDAP2KQw\n\
DwAAAAtzc2gtZWQyNTUxOQAAACD5HoUzlZEiEcszvrgjoVwm7ZFgnM0dzXwCF4+hzSeQxA\n\
AAAEC7XSKV4/1F7qMJQyaBniq4DNgwFEUjPDuxYKq9RWViKvkehTOVkSIRyzO+uCOhXCbt\n\
kWCczR3NfAIXj6HNJ5DEAAAAEHRlc3RfY2xpZW50QHRlc3QBAgMEBQ==\n\
-----END OPENSSH PRIVATE KEY-----";

    const TEST_CLIENT_PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPkehTOVkSIRyzO+uCOhXCbtkWCczR3NfAIXj6HNJ5DE test_client@test";

    struct EmbeddedSshServer {
        port: u16,
        temp_dir: Option<std::path::PathBuf>,
    }

    impl EmbeddedSshServer {
        async fn start(username: &str, password: &str) -> io::Result<Self> {
            Self::start_with_auth(username, password, None).await
        }

        async fn start_with_auth(
            username: &str,
            password: &str,
            public_key: Option<String>,
        ) -> io::Result<Self> {
            let mut config = server::Config::default();
            config.auth_rejection_time = Duration::from_millis(50);
            let private_key = russh::keys::PrivateKey::from_openssh(TEST_SERVER_KEY)
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
            config.keys.push(private_key);

            let config = Arc::new(config);
            let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
            let port = listener.local_addr()?.port();

            let credentials = TestCredentials {
                username: username.to_string(),
                password: password.to_string(),
                public_key,
            };

            let mut server = TestServer {
                creds: Arc::new(credentials),
                sftp_root: None,
            };

            // Spawn the server to run in the background
            tokio::spawn(async move { server.run_on_socket(config, &listener).await });

            // Give the server a moment to start
            tokio::time::sleep(Duration::from_millis(100)).await;

            Ok(Self {
                port,
                temp_dir: None,
            })
        }

        async fn start_with_sftp(username: &str, password: &str) -> io::Result<Self> {
            let temp_dir = std::env::temp_dir().join(format!("sftp_test_{}", uuid::Uuid::new_v4()));
            tokio::fs::create_dir_all(&temp_dir).await?;

            let mut config = server::Config::default();
            config.auth_rejection_time = Duration::from_millis(50);
            let private_key = russh::keys::PrivateKey::from_openssh(TEST_SERVER_KEY)
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
            config.keys.push(private_key);

            let config = Arc::new(config);
            let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
            let port = listener.local_addr()?.port();

            let credentials = TestCredentials {
                username: username.to_string(),
                password: password.to_string(),
                public_key: None,
            };

            let mut server = TestServer {
                creds: Arc::new(credentials),
                sftp_root: Some(temp_dir.clone()),
            };

            // Spawn the server to run in the background
            tokio::spawn(async move { server.run_on_socket(config, &listener).await });

            // Give the server a moment to start
            tokio::time::sleep(Duration::from_millis(100)).await;

            Ok(Self {
                port,
                temp_dir: Some(temp_dir),
            })
        }

        fn port(&self) -> u16 {
            self.port
        }

        fn temp_dir(&self) -> Option<&std::path::Path> {
            self.temp_dir.as_deref()
        }

        async fn shutdown(self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for EmbeddedSshServer {
        fn drop(&mut self) {
            // Clean up temp directory if it exists
            if let Some(temp_dir) = &self.temp_dir {
                let _ = std::fs::remove_dir_all(temp_dir);
            }
            // Server will be shut down when the task completes
        }
    }

    #[derive(Clone)]
    struct TestCredentials {
        username: String,
        password: String,
        public_key: Option<String>,
    }

    #[derive(Clone)]
    struct TestServer {
        creds: Arc<TestCredentials>,
        sftp_root: Option<std::path::PathBuf>,
    }

    impl server::Server for TestServer {
        type Handler = EmbeddedSshHandler;

        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
            EmbeddedSshHandler::new(self.creds.clone(), self.sftp_root.clone())
        }

        fn handle_session_error(
            &mut self,
            _error: <Self::Handler as russh::server::Handler>::Error,
        ) {
            // eprintln!("Session error: {:#?}", _error);
        }
    }

    #[derive(Clone)]
    struct EmbeddedSshHandler {
        creds: Arc<TestCredentials>,
        sftp_root: Option<std::path::PathBuf>,
        sftp_buffer: Arc<tokio::sync::Mutex<Vec<u8>>>,
        sftp_active: Arc<tokio::sync::Mutex<bool>>,
    }

    impl EmbeddedSshHandler {
        fn new(creds: Arc<TestCredentials>, sftp_root: Option<std::path::PathBuf>) -> Self {
            Self {
                creds,
                sftp_root,
                sftp_buffer: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                sftp_active: Arc::new(tokio::sync::Mutex::new(false)),
            }
        }

        fn auth_methods(&self) -> Option<MethodSet> {
            let mut methods = MethodSet::empty();
            methods.push(MethodKind::Password);
            if self.creds.public_key.is_some() {
                methods.push(MethodKind::PublicKey);
            }
            Some(methods)
        }
    }

    impl server::Handler for EmbeddedSshHandler {
        type Error = russh::Error;

        async fn auth_none(&mut self, _user: &str) -> std::result::Result<Auth, Self::Error> {
            Ok(Auth::Reject {
                proceed_with_methods: self.auth_methods(),
                partial_success: false,
            })
        }

        async fn auth_password(
            &mut self,
            user: &str,
            password: &str,
        ) -> std::result::Result<Auth, Self::Error> {
            if user == self.creds.username && password == self.creds.password {
                Ok(Auth::Accept)
            } else {
                Ok(Auth::Reject {
                    proceed_with_methods: self.auth_methods(),
                    partial_success: false,
                })
            }
        }

        async fn auth_publickey(
            &mut self,
            user: &str,
            public_key: &ssh_key::PublicKey,
        ) -> std::result::Result<Auth, Self::Error> {
            if user != self.creds.username {
                return Ok(Auth::Reject {
                    proceed_with_methods: self.auth_methods(),
                    partial_success: false,
                });
            }

            if let Some(expected_key) = &self.creds.public_key {
                // Parse the expected public key from OpenSSH format
                let expected_pk = match ssh_key::PublicKey::from_openssh(expected_key) {
                    Ok(pk) => pk,
                    Err(_) => {
                        return Ok(Auth::Reject {
                            proceed_with_methods: self.auth_methods(),
                            partial_success: false,
                        });
                    }
                };

                // Compare the key data directly
                if public_key.key_data() == expected_pk.key_data() {
                    Ok(Auth::Accept)
                } else {
                    Ok(Auth::Reject {
                        proceed_with_methods: self.auth_methods(),
                        partial_success: false,
                    })
                }
            } else {
                Ok(Auth::Reject {
                    proceed_with_methods: self.auth_methods(),
                    partial_success: false,
                })
            }
        }

        async fn channel_open_session(
            &mut self,
            _channel: Channel<Msg>,
            _session: &mut Session,
        ) -> std::result::Result<bool, Self::Error> {
            Ok(true)
        }

        async fn channel_open_direct_tcpip(
            &mut self,
            channel: Channel<Msg>,
            host_to_connect: &str,
            port_to_connect: u32,
            _originator_address: &str,
            _originator_port: u32,
            _session: &mut Session,
        ) -> std::result::Result<bool, Self::Error> {
            let port = match u16::try_from(port_to_connect) {
                Ok(port) => port,
                Err(_) => return Ok(false),
            };

            match TcpStream::connect((host_to_connect, port)).await {
                Ok(mut remote_stream) => {
                    let mut ssh_stream = channel.into_stream();

                    tokio::spawn(async move {
                        if tokio::io::copy_bidirectional(&mut ssh_stream, &mut remote_stream)
                            .await
                            .is_err()
                        {
                            // Copy errors just terminate the forwarding session.
                        }

                        let _ = ssh_stream.shutdown().await;
                        let _ = remote_stream.shutdown().await;
                    });

                    Ok(true)
                }
                Err(_) => Ok(false),
            }
        }

        async fn pty_request(
            &mut self,
            channel: ChannelId,
            _term: &str,
            _col_width: u32,
            _row_height: u32,
            _pix_width: u32,
            _pix_height: u32,
            _modes: &[(Pty, u32)],
            session: &mut Session,
        ) -> std::result::Result<(), Self::Error> {
            session.channel_success(channel)?;
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: ChannelId,
            session: &mut Session,
        ) -> std::result::Result<(), Self::Error> {
            session.channel_success(channel)?;
            session.data(channel, CryptoVec::from_slice(b"Welcome to test shell\n"))?;
            Ok(())
        }

        async fn data(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut Session,
        ) -> std::result::Result<(), Self::Error> {
            let is_sftp = *self.sftp_active.lock().await;

            if is_sftp {
                // Handle SFTP data
                if let Some(sftp_root) = &self.sftp_root {
                    let mut buffer = self.sftp_buffer.lock().await;
                    buffer.extend_from_slice(data);

                    // Process complete SFTP packets
                    loop {
                        if buffer.len() < 4 {
                            break;
                        }

                        let packet_len =
                            u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]])
                                as usize;

                        if buffer.len() < packet_len + 4 {
                            break;
                        }

                        let packet = buffer.drain(..packet_len + 4).collect::<Vec<_>>();

                        // Process SFTP packet
                        if let Some(response) =
                            Self::process_sftp_packet(&packet[4..], sftp_root).await
                        {
                            // Send response
                            let mut response_with_len = Vec::with_capacity(response.len() + 4);
                            response_with_len
                                .extend_from_slice(&(response.len() as u32).to_be_bytes());
                            response_with_len.extend_from_slice(&response);
                            session.data(channel, CryptoVec::from_slice(&response_with_len))?;
                        }
                    }
                }
            } else if data.eq_ignore_ascii_case(b"pwd\n") {
                session.data(channel, CryptoVec::from_slice(b"/cae\n"))?;
            } else {
                session.data(channel, CryptoVec::from_slice(data))?;
            }
            Ok(())
        }

        async fn subsystem_request(
            &mut self,
            channel_id: ChannelId,
            name: &str,
            session: &mut Session,
        ) -> std::result::Result<(), Self::Error> {
            if name == "sftp" {
                if self.sftp_root.is_some() {
                    *self.sftp_active.lock().await = true;
                    session.channel_success(channel_id)?;
                    Ok(())
                } else {
                    session.channel_failure(channel_id)?;
                    Ok(())
                }
            } else {
                session.channel_failure(channel_id)?;
                Ok(())
            }
        }
    }

    impl EmbeddedSshHandler {
        async fn process_sftp_packet(
            packet: &[u8],
            sftp_root: &std::path::Path,
        ) -> Option<Vec<u8>> {
            if packet.is_empty() {
                return None;
            }

            let packet_type = packet[0];

            match packet_type {
                1 => {
                    // SSH_FXP_INIT
                    // Response: SSH_FXP_VERSION
                    let mut response = Vec::new();
                    response.push(2); // SSH_FXP_VERSION
                    response.extend_from_slice(&3u32.to_be_bytes()); // version 3
                    Some(response)
                }
                3 => {
                    // SSH_FXP_OPEN
                    if let Ok((request_id, filename, flags, _attrs)) =
                        Self::parse_open_request(&packet[1..])
                    {
                        Self::handle_open(request_id, &filename, flags, sftp_root).await
                    } else {
                        None
                    }
                }
                4 => {
                    // SSH_FXP_CLOSE
                    if let Ok((request_id, _handle)) = Self::parse_close_request(&packet[1..]) {
                        Self::handle_close(request_id).await
                    } else {
                        None
                    }
                }
                5 => {
                    // SSH_FXP_READ
                    if let Ok((request_id, handle, offset, len)) =
                        Self::parse_read_request(&packet[1..])
                    {
                        Self::handle_read(request_id, &handle, offset, len, sftp_root).await
                    } else {
                        None
                    }
                }
                6 => {
                    // SSH_FXP_WRITE
                    if let Ok((request_id, handle, offset, data)) =
                        Self::parse_write_request(&packet[1..])
                    {
                        Self::handle_write(request_id, &handle, offset, data, sftp_root).await
                    } else {
                        None
                    }
                }
                _ => {
                    // Unsupported operation
                    None
                }
            }
        }

        fn parse_open_request(
            data: &[u8],
        ) -> std::result::Result<(u32, String, u32, FileAttributes), ()> {
            let mut pos = 0;
            if data.len() < 4 {
                return Err(());
            }
            let request_id = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            pos += 4;

            if data.len() < pos + 4 {
                return Err(());
            }
            let filename_len =
                u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                    as usize;
            pos += 4;

            if data.len() < pos + filename_len {
                return Err(());
            }
            let filename = String::from_utf8_lossy(&data[pos..pos + filename_len]).to_string();
            pos += filename_len;

            if data.len() < pos + 4 {
                return Err(());
            }
            let flags =
                u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);

            Ok((request_id, filename, flags, FileAttributes::empty()))
        }

        fn parse_close_request(data: &[u8]) -> std::result::Result<(u32, String), ()> {
            let mut pos = 0;
            if data.len() < 4 {
                return Err(());
            }
            let request_id = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            pos += 4;

            if data.len() < pos + 4 {
                return Err(());
            }
            let handle_len =
                u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                    as usize;
            pos += 4;

            if data.len() < pos + handle_len {
                return Err(());
            }
            let handle = String::from_utf8_lossy(&data[pos..pos + handle_len]).to_string();

            Ok((request_id, handle))
        }

        fn parse_read_request(data: &[u8]) -> std::result::Result<(u32, String, u64, u32), ()> {
            let mut pos = 0;
            if data.len() < 4 {
                return Err(());
            }
            let request_id = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            pos += 4;

            if data.len() < pos + 4 {
                return Err(());
            }
            let handle_len =
                u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                    as usize;
            pos += 4;

            if data.len() < pos + handle_len {
                return Err(());
            }
            let handle = String::from_utf8_lossy(&data[pos..pos + handle_len]).to_string();
            pos += handle_len;

            if data.len() < pos + 8 {
                return Err(());
            }
            let offset = u64::from_be_bytes([
                data[pos],
                data[pos + 1],
                data[pos + 2],
                data[pos + 3],
                data[pos + 4],
                data[pos + 5],
                data[pos + 6],
                data[pos + 7],
            ]);
            pos += 8;

            if data.len() < pos + 4 {
                return Err(());
            }
            let len = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);

            Ok((request_id, handle, offset, len))
        }

        fn parse_write_request(
            data: &[u8],
        ) -> std::result::Result<(u32, String, u64, Vec<u8>), ()> {
            let mut pos = 0;
            if data.len() < 4 {
                return Err(());
            }
            let request_id = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            pos += 4;

            if data.len() < pos + 4 {
                return Err(());
            }
            let handle_len =
                u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                    as usize;
            pos += 4;

            if data.len() < pos + handle_len {
                return Err(());
            }
            let handle = String::from_utf8_lossy(&data[pos..pos + handle_len]).to_string();
            pos += handle_len;

            if data.len() < pos + 8 {
                return Err(());
            }
            let offset = u64::from_be_bytes([
                data[pos],
                data[pos + 1],
                data[pos + 2],
                data[pos + 3],
                data[pos + 4],
                data[pos + 5],
                data[pos + 6],
                data[pos + 7],
            ]);
            pos += 8;

            if data.len() < pos + 4 {
                return Err(());
            }
            let data_len =
                u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                    as usize;
            pos += 4;

            if data.len() < pos + data_len {
                return Err(());
            }
            let file_data = data[pos..pos + data_len].to_vec();

            Ok((request_id, handle, offset, file_data))
        }

        async fn handle_open(
            request_id: u32,
            filename: &str,
            _flags: u32,
            _sftp_root: &std::path::Path,
        ) -> Option<Vec<u8>> {
            // Create a simple handle (use filename as handle for simplicity)
            let handle = filename.to_string();

            let mut response = Vec::new();
            response.push(102); // SSH_FXP_HANDLE
            response.extend_from_slice(&request_id.to_be_bytes());
            let handle_bytes = handle.as_bytes();
            response.extend_from_slice(&(handle_bytes.len() as u32).to_be_bytes());
            response.extend_from_slice(handle_bytes);

            Some(response)
        }

        async fn handle_close(request_id: u32) -> Option<Vec<u8>> {
            let mut response = Vec::new();
            response.push(101); // SSH_FXP_STATUS
            response.extend_from_slice(&request_id.to_be_bytes());
            response.extend_from_slice(&0u32.to_be_bytes()); // SSH_FX_OK
            // Empty error message
            response.extend_from_slice(&0u32.to_be_bytes());
            // Empty language tag
            response.extend_from_slice(&0u32.to_be_bytes());
            Some(response)
        }

        async fn handle_read(
            request_id: u32,
            handle: &str,
            offset: u64,
            len: u32,
            sftp_root: &std::path::Path,
        ) -> Option<Vec<u8>> {
            use tokio::io::{AsyncReadExt, AsyncSeekExt};

            let file_path = sftp_root.join(handle);

            match tokio::fs::File::open(&file_path).await {
                Ok(mut file) => {
                    if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
                        return Self::create_error_response(request_id, 2); // SSH_FX_EOF
                    }

                    let mut buffer = vec![0u8; len as usize];
                    match file.read(&mut buffer).await {
                        Ok(0) => {
                            // EOF
                            Self::create_error_response(request_id, 1) // SSH_FX_EOF
                        }
                        Ok(n) => {
                            buffer.truncate(n);
                            let mut response = Vec::new();
                            response.push(103); // SSH_FXP_DATA
                            response.extend_from_slice(&request_id.to_be_bytes());
                            response.extend_from_slice(&(buffer.len() as u32).to_be_bytes());
                            response.extend_from_slice(&buffer);
                            Some(response)
                        }
                        Err(_) => {
                            Self::create_error_response(request_id, 2) // SSH_FX_FAILURE
                        }
                    }
                }
                Err(_) => {
                    Self::create_error_response(request_id, 2) // SSH_FX_NO_SUCH_FILE
                }
            }
        }

        async fn handle_write(
            request_id: u32,
            handle: &str,
            offset: u64,
            data: Vec<u8>,
            sftp_root: &std::path::Path,
        ) -> Option<Vec<u8>> {
            use tokio::io::{AsyncSeekExt, AsyncWriteExt};

            let file_path = sftp_root.join(handle);

            // Ensure parent directory exists
            if let Some(parent) = file_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }

            match tokio::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&file_path)
                .await
            {
                Ok(mut file) => {
                    if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
                        return Self::create_error_response(request_id, 2); // SSH_FX_FAILURE
                    }

                    match file.write_all(&data).await {
                        Ok(_) => {
                            let mut response = Vec::new();
                            response.push(101); // SSH_FXP_STATUS
                            response.extend_from_slice(&request_id.to_be_bytes());
                            response.extend_from_slice(&0u32.to_be_bytes()); // SSH_FX_OK
                            // Empty error message
                            response.extend_from_slice(&0u32.to_be_bytes());
                            // Empty language tag
                            response.extend_from_slice(&0u32.to_be_bytes());
                            Some(response)
                        }
                        Err(_) => {
                            Self::create_error_response(request_id, 2) // SSH_FX_FAILURE
                        }
                    }
                }
                Err(_) => {
                    Self::create_error_response(request_id, 2) // SSH_FX_FAILURE
                }
            }
        }

        fn create_error_response(request_id: u32, error_code: u32) -> Option<Vec<u8>> {
            let mut response = Vec::new();
            response.push(101); // SSH_FXP_STATUS
            response.extend_from_slice(&request_id.to_be_bytes());
            response.extend_from_slice(&error_code.to_be_bytes());
            // Empty error message
            response.extend_from_slice(&0u32.to_be_bytes());
            // Empty language tag
            response.extend_from_slice(&0u32.to_be_bytes());
            Some(response)
        }
    }

    struct TestByteProcessor {
        expected_lines: VecDeque<Vec<u8>>,
        buffer: Vec<u8>,
    }

    impl TestByteProcessor {
        fn with_expectations(expected: &[&[u8]]) -> Self {
            Self {
                expected_lines: expected.iter().map(|line| line.to_vec()).collect(),
                buffer: Vec::new(),
            }
        }

        fn assert_expectations_met(&self) {
            assert!(
                self.expected_lines.is_empty(),
                "Missing SSH output lines: {:?}",
                self.expected_lines
                    .iter()
                    .map(|line| String::from_utf8_lossy(line).into_owned())
                    .collect::<Vec<_>>()
            );
            assert!(
                self.buffer.is_empty(),
                "Unprocessed trailing bytes: {}",
                String::from_utf8_lossy(&self.buffer)
            );
        }
    }

    impl ByteProcessor for TestByteProcessor {
        fn process_bytes(&mut self, bytes: &[u8]) {
            self.buffer.extend_from_slice(bytes);

            while let Some(pos) = self.buffer.iter().position(|&b| b == b'\n') {
                let line = self.buffer.drain(..=pos).collect::<Vec<_>>();
                let expected = self.expected_lines.pop_front().unwrap_or_else(|| {
                    panic!(
                        "Received unexpected SSH output line: {}",
                        String::from_utf8_lossy(&line)
                    )
                });

                assert_eq!(
                    line,
                    expected,
                    "SSH output mismatch. expected '{}', got '{}'",
                    String::from_utf8_lossy(&expected),
                    String::from_utf8_lossy(&line)
                );
            }
        }
    }

    #[tokio::test]
    async fn test_connect_embedded_server() {
        let server = EmbeddedSshServer::start("tester", "testerpass")
            .await
            .expect("failed to start embedded server");
        let port = server.port();
        let conn = Connection::new(
            "127.0.0.1".to_string(),
            port,
            "tester".to_string(),
            AuthMethod::Password("testerpass".to_string()),
        );
        let client = SshSession::connect(&conn).await.unwrap();

        let mut client_clone = client.clone();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel_token.clone();
        let processor = Arc::new(tokio::sync::Mutex::new(
            TestByteProcessor::with_expectations(&[b"Welcome to test shell\n", b"/cae\n"]),
        ));
        let reader_processor = processor.clone();
        let read_handle = tokio::spawn(async move {
            client_clone
                .read_loop(
                    reader_processor,
                    cancel_clone,
                    None, // No event sender for test
                )
                .await;
        });

        // make sure the read_loop is started before writing
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        client.write_all(b"pwd\n").await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        cancel_token.cancel();
        read_handle.await.unwrap();

        {
            let guard = processor.lock().await;
            guard.assert_expectations_met();
        }

        client.close().await.unwrap();

        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_connect_embedded_server_public_key() {
        use std::io::Write;

        // Create a temporary file for the private key
        let temp_dir = std::env::temp_dir();
        let key_path = temp_dir.join("test_ssh_key");
        let mut key_file = std::fs::File::create(&key_path).unwrap();
        key_file
            .write_all(TEST_CLIENT_PRIVATE_KEY.as_bytes())
            .unwrap();
        drop(key_file);

        // Set appropriate permissions (Unix-like systems only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&key_path).unwrap().permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(&key_path, perms).unwrap();
        }

        let server = EmbeddedSshServer::start_with_auth(
            "tester",
            "testerpass",
            Some(TEST_CLIENT_PUBLIC_KEY.to_string()),
        )
        .await
        .expect("failed to start embedded server");
        let port = server.port();

        let conn = Connection::new(
            "127.0.0.1".to_string(),
            port,
            "tester".to_string(),
            AuthMethod::PublicKey {
                private_key_path: key_path.to_string_lossy().to_string(),
                passphrase: None,
            },
        );
        let client = SshSession::connect(&conn).await.unwrap();

        let mut client_clone = client.clone();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel_token.clone();
        let processor = Arc::new(tokio::sync::Mutex::new(
            TestByteProcessor::with_expectations(&[b"Welcome to test shell\n", b"/cae\n"]),
        ));
        let reader_processor = processor.clone();
        let read_handle = tokio::spawn(async move {
            client_clone
                .read_loop(
                    reader_processor,
                    cancel_clone,
                    None, // No event sender for test
                )
                .await;
        });

        // make sure the read_loop is started before writing
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        client.write_all(b"pwd\n").await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        cancel_token.cancel();
        read_handle.await.unwrap();

        {
            let guard = processor.lock().await;
            guard.assert_expectations_met();
        }

        client.close().await.unwrap();

        server.shutdown().await.unwrap();

        // Clean up the temporary key file
        let _ = std::fs::remove_file(key_path);
    }

    #[tokio::test]
    async fn test_connect_embedded_server_auto_load_key() {
        use std::io::Write;

        // Create a temporary directory for SSH keys
        let temp_dir = std::env::temp_dir().join("test_ssh_auto_load");
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create a fake .ssh directory structure
        let ssh_dir = temp_dir.join(".ssh");
        std::fs::create_dir_all(&ssh_dir).unwrap();

        // Write one of the standard keys (id_ed25519) to the temp location
        let key_path = ssh_dir.join("id_ed25519");
        let mut key_file = std::fs::File::create(&key_path).unwrap();
        key_file
            .write_all(TEST_CLIENT_PRIVATE_KEY.as_bytes())
            .unwrap();
        drop(key_file);

        // Set appropriate permissions (Unix-like systems only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&key_path).unwrap().permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(&key_path, perms).unwrap();
        }

        // Temporarily override HOME to point to our temp dir
        let original_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &temp_dir);
        }

        // Start the test server with public key auth
        let server = EmbeddedSshServer::start_with_auth(
            "tester",
            "testerpass",
            Some(TEST_CLIENT_PUBLIC_KEY.to_string()),
        )
        .await
        .expect("failed to start embedded server");
        let port = server.port();

        // Create connection with AutoLoadKey
        let conn = Connection::new(
            "127.0.0.1".to_string(),
            port,
            "tester".to_string(),
            AuthMethod::AutoLoadKey,
        );

        // Test connection
        let client = SshSession::connect(&conn).await.unwrap();

        let mut client_clone = client.clone();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel_token.clone();
        let processor = Arc::new(tokio::sync::Mutex::new(
            TestByteProcessor::with_expectations(&[b"Welcome to test shell\n", b"/cae\n"]),
        ));
        let reader_processor = processor.clone();
        let read_handle = tokio::spawn(async move {
            client_clone
                .read_loop(reader_processor, cancel_clone, None)
                .await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        client.write_all(b"pwd\n").await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        cancel_token.cancel();
        read_handle.await.unwrap();

        {
            let guard = processor.lock().await;
            guard.assert_expectations_met();
        }
        client.close().await.unwrap();
        server.shutdown().await.unwrap();

        // Restore original HOME
        unsafe {
            if let Some(home) = original_home {
                std::env::set_var("HOME", home);
            } else {
                std::env::remove_var("HOME");
            }
        }

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_connect_cancel() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::task::yield_now;

        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_for_call = cancel.clone();

        let started = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(AtomicBool::new(false));
        let started_inner = started.clone();
        let completed_inner = completed.clone();

        let handle = tokio::spawn(async move {
            cancellable_timeout(
                Duration::from_secs(5),
                move || {
                    let started = started_inner.clone();
                    let completed = completed_inner.clone();
                    async move {
                        started.store(true, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        completed.store(true, Ordering::SeqCst);
                        Ok::<(), AppError>(())
                    }
                },
                &cancel_for_call,
            )
            .await
        });

        // let the cancellable future start and register its timer
        yield_now().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(
            started.load(Ordering::SeqCst),
            "connection future never started"
        );

        cancel.cancel();
        yield_now().await;

        let res = handle.await.expect("join cancellation task");
        assert!(
            matches!(res, Err(AppError::SshConnectionError(ref msg)) if msg == "cancelled"),
            "unexpected result: {res:?}"
        );
        assert!(
            !completed.load(Ordering::SeqCst),
            "connection future unexpectedly completed after cancellation"
        );
    }

    #[tokio::test]
    async fn test_connect_timeout() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::task::yield_now;

        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_for_call = cancel.clone();

        let started = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(AtomicBool::new(false));
        let started_inner = started.clone();
        let completed_inner = completed.clone();

        let handle = tokio::spawn(async move {
            cancellable_timeout(
                Duration::from_secs(1),
                move || {
                    let started = started_inner.clone();
                    let completed = completed_inner.clone();
                    async move {
                        started.store(true, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        completed.store(true, Ordering::SeqCst);
                        Ok::<(), AppError>(())
                    }
                },
                &cancel_for_call,
            )
            .await
        });

        // allow the future to register its timeout and go pending
        yield_now().await;
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(
            started.load(Ordering::SeqCst),
            "connection future never started"
        );

        yield_now().await;

        let res = handle.await.expect("join timeout task");
        assert!(
            matches!(res, Err(AppError::SshConnectionError(ref msg)) if msg == "timeout"),
            "unexpected result: {res:?}"
        );
        assert!(
            !completed.load(Ordering::SeqCst),
            "connection future unexpectedly completed before timing out"
        );
        assert!(
            !cancel.is_cancelled(),
            "timeout path should not trigger explicit cancellation"
        );
    }
    // Helper functions for SFTP tests
    async fn create_test_file(path: &std::path::Path, size: usize) -> io::Result<()> {
        use rand::RngCore;
        let mut rng = rand::thread_rng();
        let mut data = vec![0u8; size];
        rng.fill_bytes(&mut data);
        tokio::fs::write(path, data).await
    }

    async fn verify_file_content(
        path1: &std::path::Path,
        path2: &std::path::Path,
    ) -> io::Result<bool> {
        let content1 = tokio::fs::read(path1).await?;
        let content2 = tokio::fs::read(path2).await?;
        Ok(content1 == content2)
    }

    #[tokio::test]
    async fn test_sftp_send_large_file() {
        let server = EmbeddedSshServer::start_with_sftp("tester", "testerpass")
            .await
            .expect("failed to start embedded server");
        let port = server.port();
        let temp_dir = server.temp_dir().unwrap();

        let conn = Connection::new(
            "127.0.0.1".to_string(),
            port,
            "tester".to_string(),
            AuthMethod::Password("testerpass".to_string()),
        );

        // Create a local test file
        let local_file_path = std::env::temp_dir().join("test_large_file.txt");
        create_test_file(&local_file_path, 30 * 1024 * 1024)
            .await
            .unwrap();

        // Upload the file
        let remote_file_name = "uploaded_large_file.txt";
        SshSession::sftp_send_file(
            None,
            &conn,
            local_file_path.to_str().unwrap(),
            remote_file_name,
        )
        .await
        .expect("failed to send file");

        // Verify the file exists on the server
        let server_file_path = temp_dir.join(remote_file_name);
        assert!(server_file_path.exists(), "File should exist on server");

        // Verify content matches
        assert!(
            verify_file_content(&local_file_path, &server_file_path)
                .await
                .unwrap(),
            "File content should match"
        );

        // Cleanup
        let _ = tokio::fs::remove_file(&local_file_path).await;
        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_sftp_receive_large_file() {
        let server = EmbeddedSshServer::start_with_sftp("tester", "testerpass")
            .await
            .expect("failed to start embedded server");
        let port = server.port();
        let temp_dir = server.temp_dir().unwrap();

        let conn = Connection::new(
            "127.0.0.1".to_string(),
            port,
            "tester".to_string(),
            AuthMethod::Password("testerpass".to_string()),
        );

        // Create a remote test file
        let remote_file_name = "remote_large_file.txt";
        let remote_file_path = temp_dir.join(remote_file_name);
        create_test_file(&remote_file_path, 30 * 1024 * 1024)
            .await
            .unwrap();

        // Download the file
        let local_file_path = std::env::temp_dir().join("downloaded_large_file.txt");
        SshSession::sftp_receive_file(
            None,
            &conn,
            remote_file_name,
            local_file_path.to_str().unwrap(),
        )
        .await
        .expect("failed to receive file");

        // Verify the file exists locally
        assert!(local_file_path.exists(), "File should exist locally");

        // Verify content matches
        assert!(
            verify_file_content(&remote_file_path, &local_file_path)
                .await
                .unwrap(),
            "File content should match"
        );

        // Cleanup
        let _ = tokio::fs::remove_file(&local_file_path).await;
        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_sftp_send_receive_roundtrip() {
        let server = EmbeddedSshServer::start_with_sftp("tester", "testerpass")
            .await
            .expect("failed to start embedded server");
        let port = server.port();

        let conn = Connection::new(
            "127.0.0.1".to_string(),
            port,
            "tester".to_string(),
            AuthMethod::Password("testerpass".to_string()),
        );

        // Create an original test file
        let original_file_path = std::env::temp_dir().join("original_file.txt");
        create_test_file(&original_file_path, 50 * 1024)
            .await
            .unwrap(); // 50 KB

        // Upload the file
        let remote_file_name = "roundtrip_file.txt";
        SshSession::sftp_send_file(
            None,
            &conn,
            original_file_path.to_str().unwrap(),
            remote_file_name,
        )
        .await
        .expect("failed to send file");

        // Download the file back
        let downloaded_file_path = std::env::temp_dir().join("downloaded_roundtrip_file.txt");
        SshSession::sftp_receive_file(
            None,
            &conn,
            remote_file_name,
            downloaded_file_path.to_str().unwrap(),
        )
        .await
        .expect("failed to receive file");

        // Verify the downloaded file matches the original
        assert!(
            verify_file_content(&original_file_path, &downloaded_file_path)
                .await
                .unwrap(),
            "Round-trip file content should match original"
        );

        // Cleanup
        let _ = tokio::fs::remove_file(&original_file_path).await;
        let _ = tokio::fs::remove_file(&downloaded_file_path).await;
        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_port_forwarding() {
        let server = EmbeddedSshServer::start("tester", "testerpass")
            .await
            .expect("failed to start embedded server");
        let ssh_port = server.port();

        let http_listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("failed to bind HTTP listener");
        let http_port = http_listener
            .local_addr()
            .expect("failed to read HTTP listener port")
            .port();

        let http_handle = tokio::spawn(async move {
            if let Ok((mut socket, _)) = http_listener.accept().await {
                let mut buffer = [0u8; 1024];
                let _ = socket.read(&mut buffer).await;
                let response = b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\ncae";
                let _ = socket.write_all(response).await;
                let _ = socket.shutdown().await;
            }
        });

        let conn = Connection::new(
            "127.0.0.1".to_string(),
            ssh_port,
            "tester".to_string(),
            AuthMethod::Password("testerpass".to_string()),
        );

        let local_listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("failed to reserve local port");
        let local_port = local_listener
            .local_addr()
            .expect("failed to read reserved local port")
            .port();
        drop(local_listener);

        let (handle, cancel_token) = SshSession::start_port_forwarding_task(
            "127.0.0.1",
            local_port,
            &conn,
            "127.0.0.1",
            http_port,
        )
        .await
        .expect("failed to start port forwarding");

        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client_socket = TcpStream::connect(("127.0.0.1", local_port))
            .await
            .expect("failed to connect to forwarded port");
        client_socket
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("failed to write HTTP request");

        let mut response = Vec::new();
        client_socket
            .read_to_end(&mut response)
            .await
            .expect("failed to read HTTP response");

        // Clean up
        cancel_token.cancel();
        let _ = handle.await;
        let response_str = String::from_utf8(response).expect("response not valid UTF-8");
        assert!(
            response_str.contains("HTTP/1.1 200 OK"),
            "unexpected response: {response_str}"
        );
        assert!(
            response_str.ends_with("cae"),
            "unexpected body in response: {response_str}"
        );

        http_handle.await.expect("http server task failed");
        server.shutdown().await.unwrap();
    }
}
