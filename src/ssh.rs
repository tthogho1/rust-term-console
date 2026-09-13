//! SSH terminal session (FR-O1). Opens a TCP connection, authenticates, and
//! requests an interactive PTY + shell, exposed through the same
//! [`Connection`] trait the serial transport uses.

use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ssh2::Session;

use crate::connection::Connection;

/// How to authenticate an SSH session (FR-O1).
pub enum SshAuth {
    Password(String),
    PrivateKey {
        path: PathBuf,
        passphrase: Option<String>,
    },
    Agent,
}

pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
    pub term: String,
    pub cols: u32,
    pub rows: u32,
}

pub struct SshConnection {
    session: Session,
    channel: ssh2::Channel,
    target: String,
}

impl SshConnection {
    pub fn connect(config: SshConfig) -> Result<Self> {
        let addr = format!("{}:{}", config.host, config.port);
        let tcp = TcpStream::connect(&addr)
            .with_context(|| format!("failed to reach SSH host '{addr}'"))?;
        tcp.set_read_timeout(Some(Duration::from_millis(10)))?;
        tcp.set_nodelay(true)?;

        let mut session = Session::new().context("failed to create SSH session")?;
        session.set_tcp_stream(tcp);
        session
            .handshake()
            .context("SSH handshake failed (host unreachable or not an SSH server)")?;

        match &config.auth {
            SshAuth::Password(password) => {
                session
                    .userauth_password(&config.username, password)
                    .context("SSH password authentication failed")?;
            }
            SshAuth::PrivateKey { path, passphrase } => {
                session
                    .userauth_pubkey_file(
                        &config.username,
                        None,
                        path,
                        passphrase.as_deref(),
                    )
                    .context("SSH public-key authentication failed")?;
            }
            SshAuth::Agent => {
                session
                    .userauth_agent(&config.username)
                    .context("SSH agent authentication failed")?;
            }
        }

        if !session.authenticated() {
            bail!("SSH authentication did not succeed");
        }

        let mut channel = session.channel_session().context("failed to open SSH channel")?;
        channel
            .request_pty(
                &config.term,
                None,
                Some((config.cols, config.rows, 0, 0)),
            )
            .context("failed to request a PTY")?;
        channel.shell().context("failed to start remote shell")?;

        // Non-blocking so `read_available` never stalls the UI loop; writes
        // retry on WouldBlock instead (see `write_all`).
        session.set_blocking(false);

        let target = format!("{}@{}:{}", config.username, config.host, config.port);
        Ok(Self { session, channel, target })
    }
}

impl Connection for SshConnection {
    fn read_available(&mut self) -> io::Result<Vec<u8>> {
        if self.channel.eof() {
            return Err(io::Error::new(ErrorKind::ConnectionAborted, "remote closed the SSH channel"));
        }

        let mut buf = [0u8; 4096];
        match self.channel.read(&mut buf) {
            Ok(0) => Ok(Vec::new()),
            Ok(n) => Ok(buf[..n].to_vec()),
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        let mut remaining = data;
        let mut spins = 0;
        while !remaining.is_empty() {
            match self.channel.write(remaining) {
                Ok(n) => remaining = &remaining[n..],
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    spins += 1;
                    if spins > 2000 {
                        return Err(io::Error::new(ErrorKind::TimedOut, "SSH write timed out"));
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(e),
            }
        }
        loop {
            match self.channel.flush() {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn describe(&self) -> String {
        self.target.clone()
    }

    fn close(&mut self) {
        self.session.set_blocking(true);
        let _ = self.channel.send_eof();
        let _ = self.channel.close();
        let _ = self.channel.wait_close();
    }
}
