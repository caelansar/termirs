use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ssh2::{Channel, Session};

use crate::config::manager::Connection;
use crate::error::{AppError, Result};

#[derive(Clone)]
pub struct SshClient {
    pub channel: Arc<Mutex<Channel>>, // exposed for simple locking by UI loop
}

impl SshClient {
    pub fn connect(connection: &Connection) -> Result<Self> {
        Self::connect_raw(
            &connection.host_port(),
            &connection.username,
            &connection.password,
        )
    }

    pub fn connect_raw(host: &str, user: &str, pass: &str) -> Result<Self> {
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

        sess.userauth_password(user, pass).map_err(|e| {
            AppError::AuthenticationError(format!("Failed to authenticate with SSH: {}", e))
        })?;
        if !sess.authenticated() {
            return Err(AppError::AuthenticationError(
                "SSH authentication failed".to_string(),
            ));
        }

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
        if let Ok(mut ch) = self.channel.lock() {
            ch.write_all(data)?;
            ch.flush().ok();
        }
        Ok(())
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
}
