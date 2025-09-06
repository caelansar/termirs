use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use ssh2::{Channel, Session};

pub struct SshClient {
    session: Session,
    pub channel: Arc<Mutex<Channel>>, // exposed for simple locking by UI loop
}

impl SshClient {
    pub fn connect(host: &str, user: &str, pass: &str) -> Result<Self> {
        let tcp = TcpStream::connect(host).context("connect SSH host")?;
        tcp.set_nodelay(true).ok();
        let mut sess = Session::new().context("new SSH session")?;
        sess.set_tcp_stream(tcp);
        sess.handshake().context("ssh handshake")?;
        sess.userauth_password(user, pass).context("ssh auth")?;
        if !sess.authenticated() { anyhow::bail!("SSH authentication failed"); }

        let mut channel = sess.channel_session().context("open channel")?;
        channel.request_pty("xterm-256color", None, Some((100, 30, 0, 0))).context("request pty")?;
        channel.shell().context("start remote shell")?;

        // non-blocking for polling reads
        sess.set_blocking(false);

        Ok(Self { session: sess, channel: Arc::new(Mutex::new(channel)) })
    }

    pub fn request_size(&self, cols: u16, rows: u16) {
        if let Ok(mut ch) = self.channel.lock() {
            let _ = ch.request_pty_size(cols as u32, rows as u32, None, None);
        }
    }

    pub fn write_all(&self, data: &[u8]) -> Result<()> {
        if let Ok(mut ch) = self.channel.lock() { ch.write_all(data).context("ssh write")?; ch.flush().ok(); }
        Ok(())
    }

    pub fn read_some(&self, buf: &mut [u8]) -> usize {
        let mut n = 0usize;
        if let Ok(mut ch) = self.channel.lock() {
            if let Ok(got) = ch.read(buf) { n = got; }
        }
        n
    }

    pub fn close(&self) {
        if let Ok(mut ch) = self.channel.lock() {
            let _ = ch.send_eof();
            let _ = ch.close();
        }
    }
} 