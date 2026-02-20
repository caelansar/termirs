use std::collections::{BTreeMap, HashMap, VecDeque};
use std::convert::TryFrom;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures::FutureExt;
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufWriter};
use tokio::sync::{OnceCell, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::{self, PrivateKeyWithHashAlg, ssh_key};
use russh::{
    Channel, ChannelMsg, Disconnect, Error as RusshError, MethodKind, Preferred, compression,
};
use russh_sftp::client::rawsession::RawSftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::net::{TcpListener, TcpStream};

use crate::config::manager::{AuthMethod, Connection, PortForward, PortForwardType};
use crate::error::{AppError, Result};
use crate::transfer::{ScpResult, ScpTransferProgress};

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

pub(crate) trait HostFile: AsyncRead + AsyncWrite + Unpin {
    async fn file_size(&self) -> Result<u64>;
}

impl HostFile for tokio::fs::File {
    async fn file_size(&self) -> Result<u64> {
        let metadata = self
            .metadata()
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to get metadata: {e}")))?;
        Ok(metadata.len())
    }
}

pub(crate) trait ProgressReporter {
    fn report_progress(&self, transferred_bytes: u64);
    fn set_total_bytes(&self, total_bytes: Option<u64>);
}

pub(crate) struct TxProgressReporter {
    progress: Option<mpsc::Sender<ScpResult>>,
    file_index: usize,
    total_bytes: std::cell::Cell<Option<u64>>,
}

impl TxProgressReporter {
    pub(crate) fn new(
        progress: Option<mpsc::Sender<ScpResult>>,
        file_index: usize,
        total_bytes: Option<u64>,
    ) -> Self {
        Self {
            progress,
            file_index,
            total_bytes: std::cell::Cell::new(total_bytes),
        }
    }
}

impl ProgressReporter for TxProgressReporter {
    fn report_progress(&self, transferred_bytes: u64) {
        if let Some(progress) = &self.progress {
            let _ = progress.try_send(ScpResult::Progress(ScpTransferProgress {
                file_index: self.file_index,
                transferred_bytes,
                total_bytes: self.total_bytes.get(),
            }));
        }
    }

    fn set_total_bytes(&self, total_bytes: Option<u64>) {
        self.total_bytes.set(total_bytes);
    }
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
    server_key: Arc<OnceCell<String>>,
    // Channel for forwarding remote port forwarding connections
    forwarded_tcpip_tx: Option<tokio::sync::mpsc::UnboundedSender<Channel<client::Msg>>>,
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
        let _ = self.server_key.set(server_key_openssh.clone());

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

    /// Called when the server opens a channel for a new remote port forwarding connection
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<client::Msg>,
        connected_address: &str,
        connected_port: u32,
        originator_address: &str,
        originator_port: u32,
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        info!(
            "Received forwarded-tcpip channel: {}:{} <- {}:{}",
            connected_address, connected_port, originator_address, originator_port
        );

        // Send the channel to the handler if we have a sender
        if let Some(ref tx) = self.forwarded_tcpip_tx {
            if let Err(e) = tx.send(channel) {
                error!("Failed to send forwarded-tcpip channel: {}", e);
                return Err(AppError::PortForwardingError(
                    "Failed to send forwarded channel".to_string(),
                ));
            }
            info!("Forwarded channel sent to handler");
        } else {
            warn!("Received forwarded-tcpip channel but no handler is registered");
        }

        Ok(())
    }
}

pub struct SshSession {
    session: Arc<tokio::sync::Mutex<Option<client::Handle<SshClient>>>>,
    r: Option<russh::ChannelReadHalf>,
    w: russh::ChannelWriteHalf<client::Msg>,
    server_key: Arc<OnceCell<String>>,
}

impl SshSession {
    pub(crate) async fn new_session_with_timeout(
        connection: &Connection,
        timeout: Option<Duration>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<(client::Handle<SshClient>, Arc<OnceCell<String>>)> {
        Self::new_session_with_timeout_and_forwarding(connection, timeout, cancel, None).await
    }

    /// Create a new SSH session with optional remote port forwarding channel
    pub(crate) async fn new_session_with_timeout_and_forwarding(
        connection: &Connection,
        timeout: Option<Duration>,
        cancel: &tokio_util::sync::CancellationToken,
        forwarded_tcpip_tx: Option<tokio::sync::mpsc::UnboundedSender<Channel<client::Msg>>>,
    ) -> Result<(client::Handle<SshClient>, Arc<OnceCell<String>>)> {
        info!(
            "Initiating SSH connection to {}@{}",
            connection.username,
            connection.host_port()
        );

        // Configure preferred algorithms, especially compression
        // We prefer zlib@openssh.com over zlib because:
        // - zlib starts compression IMMEDIATELY after key exchange (before auth)
        // - zlib@openssh.com starts compression AFTER authentication
        // russh only initializes decompression after auth success, so using "zlib"
        // with servers that compress immediately (like tmate) will fail.
        let mut preferred = Preferred::default();
        preferred.compression = std::borrow::Cow::Borrowed(&[
            compression::NONE,
            compression::ZLIB_LEGACY, // zlib@openssh.com - compression after auth
            compression::ZLIB,        // zlib - compression immediately (fallback)
        ]);

        let config = client::Config {
            keepalive_interval: Some(std::time::Duration::from_secs(30)),
            keepalive_max: 3,
            preferred,
            ..Default::default()
        };

        let config = Arc::new(config);
        let server_key = Arc::new(OnceCell::new());
        let ssh_client = SshClient {
            connection: connection.clone(),
            server_key: server_key.clone(),
            forwarded_tcpip_tx,
        };

        debug!("Establishing TCP connection to {}", connection.host_port());
        let mut session = client::connect(config, connection.host_port(), ssh_client)
            .or_cancel(cancel)
            .or_timeout(timeout.unwrap_or(Duration::from_secs(10)))
            .await
            .flatten()
            .flatten()?;

        info!("TCP connection established to {}", connection.host_port());

        Self::authenticate_session(&mut session, connection).await?;

        Ok((session, server_key))
    }

    pub(crate) async fn setup_sftp_session(
        channel: Option<Channel<client::Msg>>,
        connection: &Connection,
        timeout: Option<Duration>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<RawSftpSession> {
        let channel = match channel {
            Some(channel) => channel,
            None => {
                let (session, _server_key) =
                    Self::new_session_with_timeout(connection, timeout, cancel).await?;

                session.channel_open_session().await?
            }
        };
        channel.request_subsystem(true, "sftp").await?;

        // Create RawSftpSession for better performance
        let sftp = RawSftpSession::new(channel.into_stream());

        // Initialize the SFTP session
        sftp.init()
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to initialize SFTP: {e}")))?;

        Ok(sftp)
    }

    async fn authenticate_session(
        session: &mut client::Handle<SshClient>,
        connection: &Connection,
    ) -> Result<()> {
        let username = &connection.username;
        debug!("Starting authentication for user '{}'", username);
        let mut attempted = Vec::new();
        let mut auth_result = session.authenticate_none(username).await?;

        loop {
            if auth_result.success() {
                info!("Authentication successful for user '{}'", username);
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
                error!(
                    "Authentication failed: no supported method available. Offered: {}",
                    offered
                );
                return Err(AppError::AuthenticationError(format!(
                    "Server does not offer a supported authentication method. Offered: {offered}"
                )));
            };

            attempted.push(next_method);
            debug!("Attempting authentication with method: {:?}", next_method);
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
                MethodKind::None => {
                    // None authentication was already attempted at the start
                    // If we're here, it means the server accepts None auth
                    session.authenticate_none(username).await?
                }
                MethodKind::HostBased => unreachable!(),
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
                | (MethodKind::None, AuthMethod::None)
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

        debug!(
            "Attempting public key authentication with key: {}",
            private_key_path
        );
        let key_path = Self::resolve_private_key_path(private_key_path)?;
        let algo = session.best_supported_rsa_hash().await?.flatten();
        let private_key = keys::load_secret_key(&key_path, passphrase.as_deref()).map_err(|e| {
            warn!("Failed to load private key from {:?}: {}", key_path, e);
            AppError::AuthenticationError(e.to_string())
        })?;

        debug!("Private key loaded successfully from {:?}", key_path);
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
        debug!("Attempting auto-load key authentication from standard paths");
        let mut last_error = None;

        for key_path in STANDARD_KEY_PATHS {
            let expanded_path = match Self::resolve_private_key_path(key_path) {
                Ok(path) => path,
                Err(_) => continue,
            };

            // Skip if key doesn't exist
            if !expanded_path.exists() {
                debug!("Key path does not exist: {}", key_path);
                continue;
            }

            debug!("Trying key: {}", key_path);
            // Try loading key with no passphrase (skip if encrypted)
            let private_key = match keys::load_secret_key(&expanded_path, None) {
                Ok(key) => key,
                Err(_) => {
                    last_error = Some(format!(
                        "Key at {key_path} requires passphrase or is invalid"
                    ));
                    debug!("Key at {} requires passphrase or is invalid", key_path);
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
                Ok(result) if result.success() => {
                    info!(
                        "Successfully authenticated with auto-loaded key: {}",
                        key_path
                    );
                    return Ok(result);
                }
                Ok(_) | Err(_) => {
                    last_error = Some(format!("Authentication failed with key: {key_path}"));
                    debug!("Authentication failed with key: {}", key_path);
                    continue;
                }
            }
        }

        error!("Auto-load key authentication failed for all standard paths");
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
            let home = dirs::home_dir().ok_or_else(|| {
                AppError::SshConnectionError("Could not determine home directory".to_string())
            })?;
            Ok(home.join(stripped))
        } else if private_key_path == "~" {
            let home = dirs::home_dir().ok_or_else(|| {
                AppError::SshConnectionError("Could not determine home directory".to_string())
            })?;
            Ok(home)
        } else {
            Ok(PathBuf::from(private_key_path))
        }
    }

    async fn new_session(
        connection: &Connection,
    ) -> Result<(client::Handle<SshClient>, Arc<OnceCell<String>>)> {
        Self::new_session_with_timeout(
            connection,
            None,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    /// Initiate an SSH connection asynchronously
    /// Returns a cancel token and a receiver for the connection result
    /// `cols` and `rows` specify the initial PTY size
    pub(crate) fn initiate_connection(
        conn: Connection,
        cols: u16,
        rows: u16,
    ) -> (
        tokio_util::sync::CancellationToken,
        mpsc::Receiver<Result<SshSession>>,
    ) {
        let (tx, rx) = mpsc::channel(1);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel_token.clone();

        tokio::spawn(async move {
            let result = Self::connect_with_cancel(&conn, cols, rows, None, &cancel_clone).await;
            // Only send result if not cancelled
            if !cancel_clone.is_cancelled() {
                let _ = tx.send(result).await;
            }
        });

        (cancel_token, rx)
    }

    async fn connect_with_cancel(
        connection: &Connection,
        cols: u16,
        rows: u16,
        timeout: Option<Duration>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Self> {
        let timeout = timeout.unwrap_or(Duration::from_secs(10));

        let f = async {
            let (session, server_key) =
                Self::new_session_with_timeout(connection, Some(timeout), cancel).await?;

            debug!("Opening SSH session channel");
            let channel = session.channel_open_session().await?;
            info!("Requesting PTY with size {} cols x {} rows", cols, rows);

            let _ = channel.set_env(false, "LC_CTYPE", "C.UTF-8").await;

            channel
                .request_pty(true, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
                .await?;

            debug!("Requesting shell");
            channel.request_shell(true).await?;

            let (r, w) = channel.split();

            info!(
                "SSH session established successfully to {}",
                connection.host_port()
            );

            Ok::<Self, AppError>(Self {
                session: Arc::new(tokio::sync::Mutex::new(Some(session))),
                r: Some(r),
                w,
                server_key,
            })
        };

        f.or_cancel(cancel)
            .or_timeout(timeout)
            .await
            .flatten()
            .flatten()
    }

    pub async fn connect(connection: &Connection, cols: u16, rows: u16) -> Result<Self> {
        Self::connect_with_cancel(
            connection,
            cols,
            rows,
            None,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    pub async fn request_size(&self, cols: u16, rows: u16) {
        let _ = self.w.window_change(cols as u32, rows as u32, 0, 0).await;
    }

    pub async fn write_all(&self, data: &[u8]) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let mut writer = self.w.make_writer();

        match writer.write_all(data).await {
            Ok(_) => Ok(()),
            Err(e) => match e.kind() {
                std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::UnexpectedEof => {
                    Err(AppError::ChannelClosedError(e.to_string()))
                }
                _ => Err(AppError::SshWriteError(format!(
                    "Failed to write to SSH channel: {e}"
                ))),
            },
        }
    }

    /// Take the reader from this session. Returns `None` if already taken.
    /// The reader should be passed to `read_loop` in a separate task.
    pub fn take_reader(&mut self) -> Option<russh::ChannelReadHalf> {
        self.r.take()
    }

    /// Read loop that processes incoming SSH channel messages.
    /// Takes ownership of the reader and uses stream-based iteration with cancellation support.
    pub(crate) async fn read_loop<B: ByteProcessor>(
        reader: russh::ChannelReadHalf,
        processor: Arc<tokio::sync::Mutex<B>>,
        cancel: tokio_util::sync::CancellationToken,
        event_tx: Option<tokio::sync::mpsc::Sender<crate::AppEvent>>,
    ) {
        use futures::stream::{self, StreamExt};

        // Create a stream from the reader using unfold
        let msg_stream =
            stream::unfold(
                reader,
                |mut r| async move { r.wait().await.map(|msg| (msg, r)) },
            );

        // Take messages until cancellation
        let mut msg_stream = std::pin::pin!(msg_stream.take_until(cancel.cancelled()));

        while let Some(msg) = msg_stream.next().await {
            match msg {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    let mut guard = processor.lock().await;
                    guard.process_bytes(&data);
                    drop(guard); // Release lock before sending event

                    // Notify event loop that terminal has updates (event-driven refresh)
                    if let Some(tx) = &event_tx {
                        let _ = tx.send(crate::AppEvent::TerminalUpdate).await;
                    }
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
        debug!("Closing SSH session");
        let guard = self.session.lock().await;
        if let Some(session) = guard.as_ref() {
            // If the session is already closed, return early
            if session.is_closed() {
                debug!("SSH session already closed");
                return Ok(());
            }
            session
                .disconnect(Disconnect::ByApplication, "", "")
                .await
                .map_err(|e| {
                    error!("Failed to disconnect SSH session: {}", e);
                    AppError::SshConnectionError(format!("Failed to disconnect: {e}"))
                })?;
            info!("SSH session closed successfully");
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn close_channel(&self) -> Result<()> {
        self.w.close().await?;
        Ok(())
    }

    /// Get the server public key that was received during connection
    pub fn get_server_key(&self) -> Option<&str> {
        self.server_key.get().map(|s| s.as_str())
    }

    pub(crate) async fn sftp_send_file_with_timeout(
        channel: Option<Channel<client::Msg>>,
        connection: &Connection,
        mut from: impl HostFile,
        to: &str,
        timeout: Option<Duration>,
        cancel: &tokio_util::sync::CancellationToken,
        progress_reporter: impl ProgressReporter,
    ) -> Result<()> {
        // Setup SFTP session
        let sftp = Self::setup_sftp_session(channel, connection, timeout, cancel).await?;
        progress_reporter.report_progress(0);

        // Open remote file for writing
        let remote_handle = sftp
            .open(
                to,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
                FileAttributes::empty(),
            )
            .await
            .map_err(|e| {
                AppError::SftpError(format!("Failed to open remote file '{}': {}", to, e))
            })?;

        // Configure transfer parameters
        const CHUNK_SIZE: usize = 128 * 1024; // 128KB - optimal for SFTP throughput
        const MAX_CONCURRENT_WRITES: usize = 12; // Balance between parallelism and resource usage

        let mut bytes_written = 0u64;
        let mut read_offset = 0u64;
        let mut write_futures = FuturesUnordered::new();
        let mut eof_reached = false;

        sftp.set_timeout(30).await;

        // Wrap sftp in Arc for safe sharing across concurrent tasks
        let sftp = Arc::new(sftp);
        let buffer_pool = Arc::new(BufferPool::new(CHUNK_SIZE, MAX_CONCURRENT_WRITES * 2));

        // Main transfer loop: pipeline reads and writes concurrently
        // This achieves maximum throughput by keeping both the network and disk busy
        loop {
            // Exit when all data is read and all writes are complete
            if eof_reached && write_futures.is_empty() {
                break;
            }

            // Determine if we have capacity for more reads
            let can_initiate_read = write_futures.len() < MAX_CONCURRENT_WRITES && !eof_reached;

            tokio::select! {
                biased; // Process cancellation first

                _ = cancel.cancelled() => {
                    return Err(AppError::SftpError("File transfer cancelled by user".to_string()));
                }

                // Read next chunk from local file when we have capacity
                read_result = async {
                    let mut buffer = buffer_pool.get_buffer().await;
                    let result = from.read(&mut buffer).await;
                    (buffer, result)
                }, if can_initiate_read => {
                    let (mut buffer, read_result) = read_result;
                    let bytes_read = read_result.map_err(|e| {
                        AppError::SftpError(format!("Failed to read from local file: {}", e))
                    })?;

                    if bytes_read == 0 {
                        // End of file reached
                        buffer_pool.return_buffer(buffer).await;
                        eof_reached = true;
                    } else {
                        // Spawn async write operation for this chunk
                        let data: Bytes = buffer.split_to(bytes_read).freeze();
                        let current_offset = read_offset;
                        read_offset += bytes_read as u64;

                        let handle = remote_handle.handle.clone();
                        let chunk_size = bytes_read as u64;
                        let sftp_clone = Arc::clone(&sftp);

                        // Create write future - will execute concurrently
                        let write_future = async move {
                            let result = sftp_clone.write(&handle, current_offset, data.to_vec()).await;
                            (current_offset, chunk_size, result)
                        };

                        write_futures.push(write_future);
                        buffer_pool.return_buffer(buffer).await;
                    }
                }

                // Process completed writes
                write_result = write_futures.next(), if !write_futures.is_empty() => {
                    if let Some((write_offset, chunk_size, write_res)) = write_result {
                        write_res.map_err(|e| {
                            AppError::SftpError(format!(
                                "Failed to write chunk at offset {}: {}",
                                write_offset, e
                            ))
                        })?;

                        bytes_written += chunk_size;
                        progress_reporter.report_progress(bytes_written);
                    }
                }
            }
        }

        // Cleanup: close remote file handle
        sftp.close(&remote_handle.handle).await.map_err(|e| {
            AppError::SftpError(format!("Failed to close remote file '{}': {}", to, e))
        })?;

        progress_reporter.report_progress(bytes_written);

        Ok(())
    }

    pub async fn sftp_send_file(
        channel: Option<Channel<client::Msg>>,
        connection: &Connection,
        local_path: &str,
        remote_path: &str,
        file_index: usize,
        progress: Option<mpsc::Sender<ScpResult>>,
    ) -> Result<()> {
        info!("Starting SFTP upload: {} -> {}", local_path, remote_path);
        let local_file = tokio::fs::File::open(expand_tilde(local_path))
            .await
            .map_err(|e| {
                error!("Failed to open local file '{}': {}", local_path, e);
                e
            })?;

        // Open local file and get its size
        let file_size = local_file.file_size().await;

        let progress_reporter = TxProgressReporter::new(progress, file_index, file_size.ok());

        let result = Self::sftp_send_file_with_timeout(
            channel,
            connection,
            local_file,
            remote_path,
            None,
            &tokio_util::sync::CancellationToken::new(),
            progress_reporter,
        )
        .await;

        match &result {
            Ok(_) => info!(
                "SFTP upload completed successfully: {} -> {}",
                local_path, remote_path
            ),
            Err(e) => error!("SFTP upload failed for {}: {}", local_path, e),
        }

        result
    }

    pub(crate) async fn sftp_receive_file_with_timeout(
        channel: Option<Channel<client::Msg>>,
        connection: &Connection,
        from: &str,
        to: impl HostFile,
        timeout: Option<Duration>,
        cancel: &tokio_util::sync::CancellationToken,
        progress_reporter: impl ProgressReporter,
    ) -> Result<()> {
        // Setup SFTP session
        let sftp = Self::setup_sftp_session(channel, connection, timeout, cancel).await?;

        // Query remote file size for progress reporting
        let file_size = sftp.stat(from).await.ok().map(|attrs| attrs.attrs.len());
        progress_reporter.set_total_bytes(file_size);
        progress_reporter.report_progress(0);

        // Open remote file for reading
        let remote_handle = sftp
            .open(from, OpenFlags::READ, FileAttributes::empty())
            .await
            .map_err(|e| {
                AppError::SftpError(format!("Failed to open remote file '{}': {}", from, e))
            })?;

        // Configure transfer parameters
        const FILE_BUFFER_SIZE: usize = 512 * 1024; // Large buffer to reduce write syscalls
        const CHUNK_SIZE: usize = 128 * 1024; // 128KB - optimal for SFTP throughput
        const MAX_CONCURRENT_READS: usize = 12; // Balance between parallelism and resource usage

        // Setup local file writer with buffering
        let mut local_file = BufWriter::with_capacity(FILE_BUFFER_SIZE, to);

        // Transfer state management
        let mut bytes_transferred = 0u64;
        let mut next_read_offset = 0u64;
        let mut next_write_offset = 0u64;
        let mut eof_reached = false;

        // Concurrent read pipeline state
        let mut read_futures = FuturesUnordered::new();
        let mut write_queue: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
        let mut pending_reads: VecDeque<(u64, u32)> = VecDeque::new();

        sftp.set_timeout(30).await;

        // Wrap sftp in Arc for safe sharing across concurrent tasks
        let sftp = Arc::new(sftp);

        // Main transfer loop: pipeline reads and writes concurrently
        // Reads are concurrent but writes are ordered to maintain file integrity
        loop {
            // Exit when all operations are complete
            if eof_reached
                && read_futures.is_empty()
                && write_queue.is_empty()
                && pending_reads.is_empty()
            {
                break;
            }

            // Fill the read pipeline up to max concurrent limit
            while read_futures.len() < MAX_CONCURRENT_READS
                && (!eof_reached || !pending_reads.is_empty())
            {
                // Determine the next read operation
                let (read_offset, read_length) =
                    if let Some((pending_offset, pending_len)) = pending_reads.pop_front() {
                        // Retry partial read
                        (pending_offset, pending_len)
                    } else {
                        // Start new read
                        let offset = next_read_offset;
                        next_read_offset += CHUNK_SIZE as u64;
                        (offset, CHUNK_SIZE as u32)
                    };

                if read_length == 0 {
                    continue;
                }

                // Spawn async read operation
                let handle = remote_handle.handle.clone();
                let sftp_clone = Arc::clone(&sftp);

                let read_future = async move {
                    let result = sftp_clone.read(&handle, read_offset, read_length).await;
                    match result {
                        Ok(data) => Ok((read_offset, read_length, data.data, false)),
                        Err(russh_sftp::client::error::Error::Status(status))
                            if status.status_code == StatusCode::Eof =>
                        {
                            // EOF is not an error, it's expected
                            Ok((read_offset, read_length, Vec::new(), true))
                        }
                        Err(err) => Err(AppError::RusshSftpError(err)),
                    }
                };

                read_futures.push(read_future);
            }

            // Wait for and process completed reads
            tokio::select! {
                biased; // Process cancellation first

                _ = cancel.cancelled() => {
                    return Err(AppError::SftpError("File transfer cancelled by user".to_string()));
                }

                read_result = read_futures.next(), if !read_futures.is_empty() => {
                    if let Some(result) = read_result {
                        // Process the first completed read
                        Self::process_read_result(
                            result,
                            &mut eof_reached,
                            &mut write_queue,
                            &mut pending_reads,
                        )?;

                        // Optimization: batch process additional completed reads without blocking
                        // This reduces loop overhead when multiple reads complete simultaneously
                        while let Some(Some(res)) = read_futures.next().now_or_never() {
                            Self::process_read_result(
                                res,
                                &mut eof_reached,
                                &mut write_queue,
                                &mut pending_reads,
                            )?;
                        }
                    }
                }

                // Yield if no reads are pending to avoid busy waiting
                _ = tokio::task::yield_now(), if read_futures.is_empty() => {}
            }

            // Write completed chunks to local file in order
            // This loop ensures data is written sequentially even though reads are concurrent
            while let Some(data) = write_queue.remove(&next_write_offset) {
                local_file.write_all(&data).await.map_err(|e| {
                    AppError::SftpError(format!(
                        "Failed to write to local file at offset {}: {}",
                        next_write_offset, e
                    ))
                })?;

                next_write_offset += data.len() as u64;
                bytes_transferred += data.len() as u64;

                progress_reporter.report_progress(bytes_transferred);
            }
        }

        // Cleanup: flush and close files
        local_file
            .flush()
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to flush local file: {}", e)))?;

        sftp.close(&remote_handle.handle).await.map_err(|e| {
            AppError::SftpError(format!("Failed to close remote file '{}': {}", from, e))
        })?;

        Ok(())
    }

    /// Helper function to process a single read result
    /// Updates the transfer state based on the read outcome
    fn process_read_result(
        result: std::result::Result<(u64, u32, Vec<u8>, bool), AppError>,
        eof_reached: &mut bool,
        write_queue: &mut BTreeMap<u64, Vec<u8>>,
        pending_reads: &mut VecDeque<(u64, u32)>,
    ) -> Result<()> {
        let (read_offset, requested_len, data, is_eof) = result?;
        let bytes_received = data.len();

        // Validate chunk size
        let bytes_received_u32 = u32::try_from(bytes_received).map_err(|_| {
            AppError::SftpError(format!(
                "Received chunk size ({}) exceeds u32::MAX",
                bytes_received
            ))
        })?;

        if bytes_received_u32 == 0 || is_eof {
            // End of file reached
            *eof_reached = true;
        } else {
            // Queue data for ordered writing
            write_queue.insert(read_offset, data);

            // Handle partial reads: if we got less than requested, queue the remainder
            if !is_eof && bytes_received_u32 < requested_len {
                let remaining = requested_len - bytes_received_u32;
                let next_offset = read_offset + u64::from(bytes_received_u32);
                pending_reads.push_back((next_offset, remaining));
            }
        }

        Ok(())
    }

    pub async fn sftp_receive_file(
        channel: Option<Channel<client::Msg>>,
        connection: &Connection,
        remote_path: &str,
        local_path: &str,
        file_index: usize,
        progress: Option<mpsc::Sender<ScpResult>>,
    ) -> Result<()> {
        info!("Starting SFTP download: {} -> {}", remote_path, local_path);
        let local_file = tokio::fs::File::create(expand_tilde(local_path))
            .await
            .map_err(|e| {
                error!("Failed to create local file '{}': {}", local_path, e);
                e
            })?;

        let progress_reporter = TxProgressReporter::new(progress, file_index, None);
        let result = Self::sftp_receive_file_with_timeout(
            channel,
            connection,
            remote_path,
            local_file,
            None,
            &tokio_util::sync::CancellationToken::new(),
            progress_reporter,
        )
        .await;

        match &result {
            Ok(_) => info!(
                "SFTP download completed successfully: {} -> {}",
                remote_path, local_path
            ),
            Err(e) => error!("SFTP download failed for {}: {}", remote_path, e),
        }

        result
    }

    /// Read the first `len` bytes from a remote file via SFTP.
    ///
    /// Opens a new SSH/SFTP session, reads up to `len` bytes from offset 0,
    /// and returns the data. Useful for binary detection without downloading
    /// the entire file.
    pub async fn sftp_read_head(
        connection: &Connection,
        remote_path: &str,
        len: usize,
    ) -> Result<Vec<u8>> {
        let cancel = tokio_util::sync::CancellationToken::new();
        let sftp = Self::setup_sftp_session(None, connection, None, &cancel).await?;

        let handle = sftp
            .open(remote_path, OpenFlags::READ, FileAttributes::empty())
            .await
            .map_err(|e| {
                AppError::SftpError(format!(
                    "Failed to open remote file '{}': {}",
                    remote_path, e
                ))
            })?;

        let data = sftp
            .read(&handle.handle, 0, len as u32)
            .await
            .map(|resp| resp.data)
            .or_else(|e| match e {
                russh_sftp::client::error::Error::Status(status)
                    if status.status_code == StatusCode::Eof =>
                {
                    Ok(Vec::new())
                }
                other => Err(AppError::RusshSftpError(other)),
            })?;

        Ok(data)
    }

    /// Start a port forwarding task and return the handle and cancellation token
    pub async fn start_port_forwarding_task(
        local_addr: &str,
        local_port: u16,
        connection: &Connection,
        service_host: &str,
        service_port: u16,
    ) -> Result<(JoinHandle<()>, CancellationToken)> {
        info!(
            "Setting up port forwarding: {}:{} -> {}:{}",
            local_addr, local_port, service_host, service_port
        );
        let (session, _) = Self::new_session(connection).await?;
        let cancel_token = CancellationToken::new();
        let cancel_token_for_task = cancel_token.clone();

        let local_addr = local_addr.to_string();
        let service_host = service_host.to_string();
        let connection = connection.clone();

        let local_listener = match TcpListener::bind((local_addr.as_str(), local_port)).await {
            Ok(listener) => {
                info!(
                    "Port forwarding listener bound to {}:{}",
                    local_addr, local_port
                );
                listener
            }
            Err(e) => {
                error!(
                    "Failed to bind port forwarding listener to {}:{}: {}",
                    local_addr, local_port, e
                );
                return Err(AppError::PortForwardingError(format!(
                    "Failed to bind to {local_addr}:{local_port}: {e}"
                )));
            }
        };

        let handle = tokio::spawn(async move {
            let mut session = session;
            loop {
                tokio::select! {
                    _ = cancel_token_for_task.cancelled() => {
                        break;
                    }
                    result = local_listener.accept() => {
                        match result {
                            Ok((mut local_socket, _)) => {
                                let mut attempts = 0;
                                let ssh_channel = loop {
                                    match session
                                        .channel_open_direct_tcpip(
                                            service_host.clone(),
                                            service_port as u32,
                                            local_addr.clone(),
                                            local_port as u32,
                                        )
                                        .await
                                    {
                                        Ok(channel) => {
                                            debug!("Port forwarding channel opened successfully");
                                            break Some(channel);
                                        }
                                        Err(e) => {
                                            warn!("Failed to open SSH forwarding channel: {}", e);

                                            let should_recreate = Self::is_port_forwarding_session_error(&session, &e);

                                            if should_recreate && attempts < 3 {
                                                debug!("Attempting to recreate SSH session for port forwarding (attempt {})", attempts + 1);
                                                match Self::new_session(&connection).await {
                                                    Ok((new_session, _)) => {
                                                        info!("SSH session recreated successfully for port forwarding");
                                                        session = new_session;
                                                        attempts += 1;
                                                        continue;
                                                    }
                                                    Err(new_err) => {
                                                        error!("Failed to recreate SSH session for port forwarding: {}", new_err);
                                                    }
                                                }
                                            }

                                            break None;
                                        }
                                    }
                                };

                                let Some(ssh_channel) = ssh_channel else {
                                    // Unable to establish forwarding channel; close the local socket and continue.
                                    let _ = local_socket.shutdown().await;
                                    continue;
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
                                                eprintln!("Copy error between local socket and SSH stream: {e}");
                                            }
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                eprintln!("Failed to accept connection: {e}");
                                continue;
                            }
                        }
                    }
                }
            }
        });

        Ok((handle, cancel_token))
    }

    /// Start a remote port forwarding task (ssh -R)
    /// Remote server listens on remote_bind_addr:remote_port and forwards to local service_host:service_port
    pub async fn start_remote_port_forwarding_task(
        remote_bind_addr: Option<&str>,
        remote_port: u16,
        connection: &Connection,
        service_host: &str,
        service_port: u16,
    ) -> Result<(JoinHandle<()>, CancellationToken)> {
        let remote_bind = remote_bind_addr.unwrap_or("127.0.0.1");
        info!(
            "Setting up remote port forwarding: {}:{} <- {}:{}",
            remote_bind, remote_port, service_host, service_port
        );

        // Create channel for receiving forwarded connections
        let (forwarded_tx, forwarded_rx) = tokio::sync::mpsc::unbounded_channel();

        let cancel_token = CancellationToken::new();
        let cancel_token_for_session = cancel_token.clone();

        // Create a new session with forwarding channel
        let (mut session, _) = Self::new_session_with_timeout_and_forwarding(
            connection,
            None,
            &cancel_token_for_session,
            Some(forwarded_tx),
        )
        .await?;

        let remote_bind = remote_bind.to_string();

        // Request remote port forwarding
        match session
            .tcpip_forward(&remote_bind, remote_port as u32)
            .await
        {
            Ok(bound_port) => {
                info!(
                    "Remote port forwarding established on {}:{} (actual port: {})",
                    remote_bind, remote_port, bound_port
                );
            }
            Err(e) => {
                error!("Failed to establish remote port forwarding: {}", e);
                return Err(AppError::PortForwardingError(format!(
                    "Failed to request remote port forwarding: {e}"
                )));
            }
        }

        // Clone these for the async task
        let service_host_clone = service_host.to_string();
        let service_port_clone = service_port;
        let remote_bind_clone = remote_bind.clone();
        let cancel_token_for_task = cancel_token.clone();

        let handle = tokio::spawn(async move {
            // Create a handler that will process forwarded connections
            let mut forwarding_handler = RemoteForwardingHandler {
                service_host: service_host_clone,
                service_port: service_port_clone,
                session,
                remote_bind: remote_bind_clone,
                remote_port,
                cancel_token: cancel_token_for_task,
                forwarded_rx,
            };

            if let Err(e) = forwarding_handler.run().await {
                error!("Remote forwarding handler error: {}", e);
            }
        });

        Ok((handle, cancel_token))
    }

    /// Start a dynamic SOCKS5 port forwarding task (ssh -D)
    /// Creates a SOCKS5 proxy on local_addr:local_port that tunnels through SSH
    pub async fn start_dynamic_port_forwarding_task(
        local_addr: &str,
        local_port: u16,
        connection: &Connection,
    ) -> Result<(JoinHandle<()>, CancellationToken)> {
        info!(
            "Setting up dynamic SOCKS5 proxy on {}:{}",
            local_addr, local_port
        );

        let cancel_token = CancellationToken::new();
        let cancel_token_for_task = cancel_token.clone();

        let local_addr = local_addr.to_string();
        let connection = connection.clone();

        let local_listener = match TcpListener::bind((local_addr.as_str(), local_port)).await {
            Ok(listener) => {
                info!("SOCKS5 proxy listening on {}:{}", local_addr, local_port);
                listener
            }
            Err(e) => {
                error!(
                    "Failed to bind SOCKS5 listener to {}:{}: {}",
                    local_addr, local_port, e
                );
                return Err(AppError::PortForwardingError(format!(
                    "Failed to bind to {local_addr}:{local_port}: {e}"
                )));
            }
        };

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel_token_for_task.cancelled() => {
                        info!("Dynamic SOCKS5 proxy task cancelled");
                        break;
                    }
                    result = local_listener.accept() => {
                        match result {
                            Ok((local_socket, addr)) => {
                                debug!("SOCKS5 connection from {}", addr);

                                let cancel_for_connection = cancel_token_for_task.clone();
                                let connection_clone = connection.clone();

                                // Handle SOCKS5 connection in a separate task
                                // Each connection gets its own SSH session
                                tokio::spawn(async move {
                                    if let Err(e) = Self::handle_socks5_connection(
                                        local_socket,
                                        cancel_for_connection,
                                        connection_clone,
                                    ).await {
                                        debug!("SOCKS5 connection error: {}", e);
                                    }
                                });
                            }
                            Err(e) => {
                                error!("Failed to accept SOCKS5 connection: {}", e);
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

/// Handler for remote port forwarding connections
struct RemoteForwardingHandler {
    service_host: String,
    service_port: u16,
    session: client::Handle<SshClient>,
    remote_bind: String,
    remote_port: u16,
    cancel_token: CancellationToken,
    forwarded_rx: tokio::sync::mpsc::UnboundedReceiver<Channel<client::Msg>>,
}

impl RemoteForwardingHandler {
    /// Run the forwarding handler, processing incoming connections
    async fn run(&mut self) -> Result<()> {
        info!(
            "Remote forwarding handler running, waiting for connections to forward to {}:{}",
            self.service_host, self.service_port
        );

        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("Remote port forwarding task cancelled");
                    // Cancel the remote forwarding
                    let _ = self.session.cancel_tcpip_forward(&self.remote_bind, self.remote_port as u32).await;
                    break;
                }
                Some(channel) = self.forwarded_rx.recv() => {
                    info!("Received forwarded connection, spawning handler");

                    // Clone what we need for the connection handler
                    let service_host = self.service_host.clone();
                    let service_port = self.service_port;

                    // Handle each forwarded connection in a separate task
                    tokio::spawn(async move {
                        if let Err(e) = SshSession::handle_remote_forwarded_connection(
                            channel,
                            &service_host,
                            service_port,
                        )
                        .await
                        {
                            error!("Error handling remote forwarded connection: {}", e);
                        }
                    });
                }
                _ = tokio::time::sleep(Duration::from_secs(30)) => {
                    // Periodic check if session is still alive
                    if self.session.is_closed() {
                        warn!("SSH session closed, remote forwarding terminated");
                        return Err(AppError::PortForwardingError(
                            "SSH session closed".to_string(),
                        ));
                    }
                    debug!("Remote forwarding session still alive");
                }
            }
        }

        Ok(())
    }
}

impl SshSession {
    /// Handle a forwarded connection for remote port forwarding
    /// This receives connections from the SSH server and forwards them to the local service
    async fn handle_remote_forwarded_connection(
        channel: Channel<client::Msg>,
        service_host: &str,
        service_port: u16,
    ) -> Result<()> {
        info!(
            "Handling remote forwarded connection to {}:{}",
            service_host, service_port
        );

        // Connect to the local service
        let mut local_stream = match TcpStream::connect((service_host, service_port)).await {
            Ok(stream) => {
                info!(
                    "Connected to local service at {}:{}",
                    service_host, service_port
                );
                stream
            }
            Err(e) => {
                error!(
                    "Failed to connect to local service {}:{}: {}",
                    service_host, service_port, e
                );
                return Err(AppError::PortForwardingError(format!(
                    "Failed to connect to local service {service_host}:{service_port}: {e}"
                )));
            }
        };

        // Convert channel to stream for bidirectional copying
        let mut ssh_stream = channel.into_stream();

        // Proxy data bidirectionally between the SSH channel and local service
        match tokio::io::copy_bidirectional(&mut ssh_stream, &mut local_stream).await {
            Ok((to_service, to_channel)) => {
                info!(
                    "Remote forward complete: {} bytes to service, {} bytes to client",
                    to_service, to_channel
                );
            }
            Err(e) => {
                warn!("Error during remote forwarding: {}", e);
            }
        }

        info!("Remote forwarded connection closed");
        Ok(())
    }

    /// Handle a single SOCKS5 connection
    async fn handle_socks5_connection(
        mut local_socket: TcpStream,
        cancel_token: CancellationToken,
        connection: Connection,
    ) -> Result<()> {
        // Create a session for this SOCKS5 connection
        let (mut session, _) = Self::new_session(&connection).await?;
        // SOCKS5 handshake: receive greeting
        let mut buf = [0u8; 2];
        tokio::select! {
            _ = cancel_token.cancelled() => {
                return Ok(());
            }
            result = local_socket.read_exact(&mut buf) => {
                result.map_err(|e| AppError::PortForwardingError(format!("SOCKS5 greeting read error: {e}")))?;
            }
        }

        let version = buf[0];
        let nmethods = buf[1];

        if version != 5 {
            return Err(AppError::PortForwardingError(format!(
                "Unsupported SOCKS version: {version}"
            )));
        }

        // Read methods
        let mut methods = vec![0u8; nmethods as usize];
        tokio::select! {
            _ = cancel_token.cancelled() => {
                return Ok(());
            }
            result = local_socket.read_exact(&mut methods) => {
                result.map_err(|e| AppError::PortForwardingError(format!("SOCKS5 methods read error: {e}")))?;
            }
        }

        // Send method selection (0x00 = no authentication)
        tokio::select! {
            _ = cancel_token.cancelled() => {
                return Ok(());
            }
            result = local_socket.write_all(&[5, 0]) => {
                result.map_err(|e| AppError::PortForwardingError(format!("SOCKS5 method response error: {e}")))?;
            }
        }

        // Read request
        let mut buf = [0u8; 4];
        tokio::select! {
            _ = cancel_token.cancelled() => {
                return Ok(());
            }
            result = local_socket.read_exact(&mut buf) => {
                result.map_err(|e| AppError::PortForwardingError(format!("SOCKS5 request read error: {e}")))?;
            }
        }

        let version = buf[0];
        let cmd = buf[1];
        let _reserved = buf[2];
        let atyp = buf[3];

        if version != 5 {
            return Err(AppError::PortForwardingError(format!(
                "Unsupported SOCKS version in request: {version}"
            )));
        }

        if cmd != 1 {
            // Only support CONNECT (1), not BIND (2) or UDP ASSOCIATE (3)
            let _ = local_socket
                .write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0])
                .await; // Command not supported
            return Err(AppError::PortForwardingError(format!(
                "Unsupported SOCKS command: {cmd}"
            )));
        }

        // Parse destination address
        let dest_addr = match atyp {
            1 => {
                // IPv4
                let mut ipv4 = [0u8; 4];
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        return Ok(());
                    }
                    result = local_socket.read_exact(&mut ipv4) => {
                        result.map_err(|e| AppError::PortForwardingError(format!("SOCKS5 IPv4 read error: {e}")))?;
                    }
                }
                format!("{}.{}.{}.{}", ipv4[0], ipv4[1], ipv4[2], ipv4[3])
            }
            3 => {
                // Domain name
                let mut len_buf = [0u8; 1];
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        return Ok(());
                    }
                    result = local_socket.read_exact(&mut len_buf) => {
                        result.map_err(|e| AppError::PortForwardingError(format!("SOCKS5 domain length read error: {e}")))?;
                    }
                }
                let len = len_buf[0] as usize;
                let mut domain = vec![0u8; len];
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        return Ok(());
                    }
                    result = local_socket.read_exact(&mut domain) => {
                        result.map_err(|e| AppError::PortForwardingError(format!("SOCKS5 domain read error: {e}")))?;
                    }
                }
                String::from_utf8(domain).map_err(|e| {
                    AppError::PortForwardingError(format!("Invalid domain name: {e}"))
                })?
            }
            4 => {
                // IPv6
                let mut ipv6 = [0u8; 16];
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        return Ok(());
                    }
                    result = local_socket.read_exact(&mut ipv6) => {
                        result.map_err(|e| AppError::PortForwardingError(format!("SOCKS5 IPv6 read error: {e}")))?;
                    }
                }
                format!(
                    "{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}",
                    ipv6[0],
                    ipv6[1],
                    ipv6[2],
                    ipv6[3],
                    ipv6[4],
                    ipv6[5],
                    ipv6[6],
                    ipv6[7],
                    ipv6[8],
                    ipv6[9],
                    ipv6[10],
                    ipv6[11],
                    ipv6[12],
                    ipv6[13],
                    ipv6[14],
                    ipv6[15]
                )
            }
            _ => {
                let _ = local_socket
                    .write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0])
                    .await; // Address type not supported
                return Err(AppError::PortForwardingError(format!(
                    "Unsupported address type: {atyp}"
                )));
            }
        };

        // Read port
        let mut port_buf = [0u8; 2];
        tokio::select! {
            _ = cancel_token.cancelled() => {
                return Ok(());
            }
            result = local_socket.read_exact(&mut port_buf) => {
                result.map_err(|e| AppError::PortForwardingError(format!("SOCKS5 port read error: {e}")))?;
            }
        }
        let dest_port = u16::from_be_bytes(port_buf);

        debug!("SOCKS5 CONNECT to {}:{}", dest_addr, dest_port);

        // Open SSH channel to destination
        let mut attempts = 0;
        let ssh_channel = loop {
            match session
                .channel_open_direct_tcpip(
                    dest_addr.clone(),
                    dest_port as u32,
                    "127.0.0.1".to_string(), // Originator address for SOCKS5
                    0,                       // local port doesn't matter for SOCKS5
                )
                .await
            {
                Ok(channel) => {
                    debug!(
                        "SSH channel opened for SOCKS5 connection to {}:{}",
                        dest_addr, dest_port
                    );
                    break Some(channel);
                }
                Err(e) => {
                    warn!("Failed to open SSH channel for SOCKS5: {}", e);

                    if Self::is_port_forwarding_session_error(&session, &e) && attempts < 3 {
                        debug!(
                            "Attempting to recreate SSH session (attempt {})",
                            attempts + 1
                        );
                        match Self::new_session(&connection).await {
                            Ok((new_session, _)) => {
                                info!("SSH session recreated successfully");
                                session = new_session;
                                attempts += 1;
                                continue;
                            }
                            Err(e) => {
                                error!("Failed to recreate SSH session: {}", e);
                            }
                        }
                    }

                    break None;
                }
            }
        };

        let Some(ssh_channel) = ssh_channel else {
            // Send SOCKS5 error response
            let _ = local_socket
                .write_all(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0])
                .await; // General failure
            return Err(AppError::PortForwardingError(
                "Failed to establish SSH channel for SOCKS5".to_string(),
            ));
        };

        // Send SOCKS5 success response
        tokio::select! {
            _ = cancel_token.cancelled() => {
                return Ok(());
            }
            result = local_socket.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]) => {
                result.map_err(|e| AppError::PortForwardingError(format!("SOCKS5 success response error: {e}")))?;
            }
        }

        // Proxy data between SOCKS5 client and SSH channel
        let mut ssh_stream = ssh_channel.into_stream();

        tokio::select! {
            _ = cancel_token.cancelled() => {
                debug!("SOCKS5 connection cancelled");
            }
            result = tokio::io::copy_bidirectional(&mut local_socket, &mut ssh_stream) => {
                if let Err(e) = result {
                    debug!("SOCKS5 copy error: {}", e);
                }
            }
        }

        Ok(())
    }

    fn is_port_forwarding_session_error(
        session: &client::Handle<SshClient>,
        err: &RusshError,
    ) -> bool {
        if session.is_closed() {
            return true;
        }

        matches!(
            err,
            RusshError::Disconnect
                | RusshError::HUP
                | RusshError::IO(_)
                | RusshError::ConnectionTimeout
                | RusshError::KeepaliveTimeout
                | RusshError::InactivityTimeout
                | RusshError::SendError
                | RusshError::RecvError
                | RusshError::Elapsed(_)
        )
    }
}

type ActiveForwardMap = HashMap<String, (JoinHandle<()>, CancellationToken)>;

/// Runtime management for port forwarding sessions
pub struct PortForwardingRuntime {
    active_forwards: ActiveForwardMap,
}

impl PortForwardingRuntime {
    pub fn new() -> Self {
        Self {
            active_forwards: HashMap::new(),
        }
    }

    /// Start a port forwarding session
    pub async fn start_port_forward(
        &mut self,
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

        // Start the appropriate port forwarding task based on type
        let (handle, cancel_token) = match port_forward.forward_type {
            PortForwardType::Local => {
                SshSession::start_port_forwarding_task(
                    &port_forward.local_addr,
                    port_forward.local_port,
                    connection,
                    &port_forward.service_host,
                    port_forward.service_port,
                )
                .await?
            }
            PortForwardType::Remote => {
                SshSession::start_remote_port_forwarding_task(
                    port_forward.remote_bind_addr.as_deref(),
                    port_forward.local_port,
                    connection,
                    &port_forward.service_host,
                    port_forward.service_port,
                )
                .await?
            }
            PortForwardType::Dynamic => {
                SshSession::start_dynamic_port_forwarding_task(
                    &port_forward.local_addr,
                    port_forward.local_port,
                    connection,
                )
                .await?
            }
        };

        // Store the handle and cancellation token
        self.active_forwards.insert(pf_id, (handle, cancel_token));

        Ok(())
    }

    /// Stop a port forwarding session
    pub async fn stop_port_forward(&mut self, port_forward_id: &str) -> Result<()> {
        if let Some((handle, cancel_token)) = self.active_forwards.remove(port_forward_id) {
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
    pub async fn is_running(&mut self, port_forward_id: &str) -> bool {
        self.active_forwards.contains_key(port_forward_id)
    }

    /// Stop all port forwarding sessions
    #[allow(dead_code)]
    pub async fn stop_all(&mut self) -> Result<()> {
        for (_, (handle, cancel_token)) in self.active_forwards.drain() {
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

/// Extension trait for futures that can be cancelled
pub trait OrCancelExt: Sized {
    type Output;

    async fn or_cancel(self, token: &CancellationToken) -> Result<Self::Output>;
}

pub trait OrTimeoutExt: Sized {
    type Output;

    async fn or_timeout(self, duration: Duration) -> Result<Self::Output>;
}

/// Extension trait for futures that can be timed out
impl<F> OrTimeoutExt for F
where
    F: Future + Send,
    F::Output: Send,
{
    type Output = F::Output;

    async fn or_timeout(self, duration: Duration) -> Result<Self::Output> {
        tokio::time::timeout(duration, self)
            .await
            .map_err(|_| AppError::SshConnectionError("timeout".to_string()))
    }
}

impl<F> OrCancelExt for F
where
    F: Future + Send,
    F::Output: Send,
{
    type Output = F::Output;

    async fn or_cancel(self, token: &CancellationToken) -> Result<Self::Output> {
        tokio::select! {
            _ = token.cancelled() => Err(AppError::SshConnectionError("cancelled".to_string())),
            res = self => Ok(res),
        }
    }
}

pub fn expand_tilde(input: &str) -> PathBuf {
    if let Some(stripped) = input.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    } else if input == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }

    PathBuf::from(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use russh::server::{self, Auth, Msg, Server as _, Session};
    use russh::{Channel, ChannelId, CryptoVec, Disconnect, MethodKind, MethodSet, Pty};
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
        forward_disconnect_count: Option<Arc<AtomicUsize>>,
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
                disconnect_on_first_forward: None,
                forward_disconnect_count: None,
            };

            // Spawn the server to run in the background
            tokio::spawn(async move { server.run_on_socket(config, &listener).await });

            // Give the server a moment to start
            tokio::time::sleep(Duration::from_millis(100)).await;

            Ok(Self {
                port,
                temp_dir: None,
                forward_disconnect_count: None,
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
                disconnect_on_first_forward: None,
                forward_disconnect_count: None,
            };

            // Spawn the server to run in the background
            tokio::spawn(async move { server.run_on_socket(config, &listener).await });

            // Give the server a moment to start
            tokio::time::sleep(Duration::from_millis(100)).await;

            Ok(Self {
                port,
                temp_dir: Some(temp_dir),
                forward_disconnect_count: None,
            })
        }

        async fn start_with_forward_disconnect(username: &str, password: &str) -> io::Result<Self> {
            // This will cause the server to disconnect the first time a forward is established
            // which is useful for testing the recovery of the port forwarding session.
            // The test will then verify that the port forwarding session is recovered and the forward is established again.
            let disconnect_flag = Arc::new(AtomicBool::new(true));
            let disconnect_count = Arc::new(AtomicUsize::new(0));

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
                sftp_root: None,
                disconnect_on_first_forward: Some(disconnect_flag.clone()),
                forward_disconnect_count: Some(disconnect_count.clone()),
            };

            tokio::spawn(async move { server.run_on_socket(config, &listener).await });
            tokio::time::sleep(Duration::from_millis(100)).await;

            Ok(Self {
                port,
                temp_dir: None,
                forward_disconnect_count: Some(disconnect_count),
            })
        }

        fn port(&self) -> u16 {
            self.port
        }

        fn temp_dir(&self) -> Option<&std::path::Path> {
            self.temp_dir.as_deref()
        }

        fn forward_disconnect_count(&self) -> usize {
            self.forward_disconnect_count
                .as_ref()
                .map(|c| c.load(Ordering::SeqCst))
                .unwrap_or(0)
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
        disconnect_on_first_forward: Option<Arc<AtomicBool>>,
        forward_disconnect_count: Option<Arc<AtomicUsize>>,
    }

    impl server::Server for TestServer {
        type Handler = EmbeddedSshHandler;

        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
            EmbeddedSshHandler::new(
                self.creds.clone(),
                self.sftp_root.clone(),
                self.disconnect_on_first_forward.clone(),
                self.forward_disconnect_count.clone(),
            )
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
        disconnect_on_first_forward: Option<Arc<AtomicBool>>,
        forward_disconnect_count: Option<Arc<AtomicUsize>>,
    }

    impl EmbeddedSshHandler {
        fn new(
            creds: Arc<TestCredentials>,
            sftp_root: Option<std::path::PathBuf>,
            disconnect_on_first_forward: Option<Arc<AtomicBool>>,
            forward_disconnect_count: Option<Arc<AtomicUsize>>,
        ) -> Self {
            Self {
                creds,
                sftp_root,
                sftp_buffer: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                sftp_active: Arc::new(tokio::sync::Mutex::new(false)),
                disconnect_on_first_forward,
                forward_disconnect_count,
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
            session: &mut Session,
        ) -> std::result::Result<bool, Self::Error> {
            let port = match u16::try_from(port_to_connect) {
                Ok(port) => port,
                Err(_) => return Ok(false),
            };

            if let Some(flag) = &self.disconnect_on_first_forward {
                if flag.swap(false, Ordering::SeqCst) {
                    if let Some(count) = &self.forward_disconnect_count {
                        count.fetch_add(1, Ordering::SeqCst);
                    }
                    let _ = session.disconnect(
                        Disconnect::ByApplication,
                        "forced disconnect for testing",
                        "",
                    );
                    return Err(russh::Error::Disconnect);
                }
            }

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
                17 => {
                    // SSH_FXP_STAT
                    if let Ok((request_id, path)) = Self::parse_stat_request(&packet[1..]) {
                        Self::handle_stat(request_id, &path, sftp_root).await
                    } else {
                        None
                    }
                }
                _ => {
                    // Unsupported operation
                    unimplemented!("Unsupported SFTP packet type: {packet_type}");
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

        fn parse_stat_request(data: &[u8]) -> std::result::Result<(u32, String), ()> {
            if data.len() < 4 {
                return Err(());
            }
            let request_id = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            let mut pos = 4;

            if data.len() < pos + 4 {
                return Err(());
            }
            let path_len =
                u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                    as usize;
            pos += 4;

            if data.len() < pos + path_len {
                return Err(());
            }
            let path = String::from_utf8_lossy(&data[pos..pos + path_len]).to_string();

            Ok((request_id, path))
        }

        async fn handle_stat(
            request_id: u32,
            path: &str,
            sftp_root: &std::path::Path,
        ) -> Option<Vec<u8>> {
            let file_path = sftp_root.join(path);

            match tokio::fs::metadata(&file_path).await {
                Ok(metadata) => {
                    let size = metadata.len();
                    println!("Remote file size: {:?}", size);
                    let mut response = Vec::new();
                    response.push(105); // SSH_FXP_ATTRS
                    response.extend_from_slice(&request_id.to_be_bytes());
                    // flags: SSH_FILEXFER_ATTR_SIZE (0x00000001)
                    response.extend_from_slice(&1u32.to_be_bytes());
                    // size (u64)
                    response.extend_from_slice(&size.to_be_bytes());
                    Some(response)
                }
                Err(_) => Self::create_error_response(request_id, 2), // SSH_FX_NO_SUCH_FILE
            }
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
            AuthMethod::Password("testerpass".to_string().into()),
        );
        let mut client = SshSession::connect(&conn, 80, 24).await.unwrap();

        let reader = client.take_reader().expect("reader already taken");
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel_token.clone();
        let processor = Arc::new(tokio::sync::Mutex::new(
            TestByteProcessor::with_expectations(&[b"Welcome to test shell\n", b"/cae\n"]),
        ));
        let reader_processor = processor.clone();
        let read_handle = tokio::spawn(async move {
            SshSession::read_loop(
                reader,
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
        let mut client = SshSession::connect(&conn, 80, 24).await.unwrap();

        let reader = client.take_reader().expect("reader already taken");
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel_token.clone();
        let processor = Arc::new(tokio::sync::Mutex::new(
            TestByteProcessor::with_expectations(&[b"Welcome to test shell\n", b"/cae\n"]),
        ));
        let reader_processor = processor.clone();
        let read_handle = tokio::spawn(async move {
            SshSession::read_loop(
                reader,
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
        let mut client = SshSession::connect(&conn, 80, 24).await.unwrap();

        let reader = client.take_reader().expect("reader already taken");
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel_token.clone();
        let processor = Arc::new(tokio::sync::Mutex::new(
            TestByteProcessor::with_expectations(&[b"Welcome to test shell\n", b"/cae\n"]),
        ));
        let reader_processor = processor.clone();
        let read_handle = tokio::spawn(async move {
            SshSession::read_loop(reader, reader_processor, cancel_clone, None).await;
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
            let f = async {
                let started = started_inner.clone();
                let completed = completed_inner.clone();
                started.store(true, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(10)).await;
                completed.store(true, Ordering::SeqCst);
                Ok::<(), AppError>(())
            };
            f.or_cancel(&cancel_for_call)
                .or_timeout(Duration::from_secs(5))
                .await
                .flatten()
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
            let f = async {
                let started = started_inner.clone();
                let completed = completed_inner.clone();
                started.store(true, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(10)).await;
                completed.store(true, Ordering::SeqCst);
                Ok::<(), AppError>(())
            };
            f.or_cancel(&cancel_for_call)
                .or_timeout(Duration::from_secs(1))
                .await
                .flatten()
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
            AuthMethod::Password("testerpass".to_string().into()),
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
            0,
            None,
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
            AuthMethod::Password("testerpass".to_string().into()),
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
            0,
            None,
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
            AuthMethod::Password("testerpass".to_string().into()),
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
            0,
            None,
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
            0,
            None,
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
            AuthMethod::Password("testerpass".to_string().into()),
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

    #[tokio::test]
    async fn test_port_forwarding_recovers_after_session_drop() {
        let server = EmbeddedSshServer::start_with_forward_disconnect("tester", "testerpass")
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
            AuthMethod::Password("testerpass".to_string().into()),
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
        let disconnects = server.forward_disconnect_count();
        assert_eq!(disconnects, 1, "expected exactly one forced disconnect");
        server.shutdown().await.unwrap();
    }
}
