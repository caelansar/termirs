use std::collections::VecDeque;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::io::AsyncReadExt;

use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::{self, PrivateKeyWithHashAlg, ssh_key};
use russh::{Channel, ChannelMsg, Disconnect, MethodKind};
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
                MethodKind::PublicKey => {
                    Self::authenticate_public_key(session, username, &connection.auth_method)
                        .await?
                }
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
        let mut write_queue: std::collections::VecDeque<(u64, Vec<u8>)> =
            std::collections::VecDeque::new();
        let mut next_write_offset = 0u64;
        let mut eof_reached = false;

        // Wrap sftp in Arc to share between tasks
        let sftp = Arc::new(sftp);

        loop {
            // Check exit condition
            if eof_reached && read_futures.is_empty() && write_queue.is_empty() {
                break;
            }

            // Check if we can read more data
            let can_read = read_futures.len() < MAX_CONCURRENT_READS && !eof_reached;

            // Start new read operations if we have capacity
            if can_read {
                let current_offset = offset;
                let chunk_size = CHUNK_SIZE;
                let handle = remote_handle.handle.clone();
                let sftp_clone = Arc::clone(&sftp);

                let read_future = async move {
                    let result = sftp_clone
                        .read(&handle, current_offset, chunk_size as u32)
                        .await;
                    match result {
                        Ok(data) => {
                            let data_bytes = data.data;
                            let is_eof = data_bytes.is_empty();
                            (current_offset, data_bytes.len(), data_bytes, is_eof)
                        }
                        Err(_) => (current_offset, 0, Vec::new(), true), // Treat error as EOF
                    }
                };

                read_futures.push(read_future);
                offset += chunk_size as u64;
            }

            // Process completed reads
            tokio::select! {
                _ = cancel.cancelled() => {
                    return Err(AppError::SftpError("Transfer cancelled".to_string()));
                }

                read_result = read_futures.next(), if !read_futures.is_empty() => {
                    if let Some(result) = read_result {
                        let (read_offset, bytes_in_chunk, data, is_eof) = result;

                        if bytes_in_chunk == 0 || is_eof {
                            eof_reached = true;
                        }

                        if bytes_in_chunk > 0 {
                            // Add to write queue with offset for ordering
                            write_queue.push_back((read_offset, data));
                            write_queue.make_contiguous().sort_by_key(|(offset, _)| *offset);
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
    use tokio::net::TcpListener;

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
            };

            // Spawn the server to run in the background
            tokio::spawn(async move { server.run_on_socket(config, &listener).await });

            // Give the server a moment to start
            tokio::time::sleep(Duration::from_millis(100)).await;

            Ok(Self { port })
        }

        fn port(&self) -> u16 {
            self.port
        }

        async fn shutdown(self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for EmbeddedSshServer {
        fn drop(&mut self) {
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
    }

    impl server::Server for TestServer {
        type Handler = EmbeddedSshHandler;

        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
            EmbeddedSshHandler::new(self.creds.clone())
        }

        fn handle_session_error(
            &mut self,
            _error: <Self::Handler as russh::server::Handler>::Error,
        ) {
            eprintln!("Session error: {:#?}", _error);
        }
    }

    #[derive(Clone)]
    struct EmbeddedSshHandler {
        creds: Arc<TestCredentials>,
    }

    impl EmbeddedSshHandler {
        fn new(creds: Arc<TestCredentials>) -> Self {
            Self { creds }
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
            if data.eq_ignore_ascii_case(b"pwd\n") {
                session.data(channel, CryptoVec::from_slice(b"/cae\n"))?;
            } else {
                session.data(channel, CryptoVec::from_slice(data))?;
            }
            Ok(())
        }
    }

    struct EchoByteProcessor;
    impl ByteProcessor for EchoByteProcessor {
        fn process_bytes(&mut self, bytes: &[u8]) {
            println!("Received bytes:\n {}", String::from_utf8_lossy(bytes));
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
        tokio::spawn(async move {
            client_clone
                .read_loop(
                    Arc::new(tokio::sync::Mutex::new(EchoByteProcessor)),
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
        tokio::spawn(async move {
            client_clone
                .read_loop(
                    Arc::new(tokio::sync::Mutex::new(EchoByteProcessor)),
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

        client.close().await.unwrap();

        server.shutdown().await.unwrap();

        // Clean up the temporary key file
        let _ = std::fs::remove_file(key_path);
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
