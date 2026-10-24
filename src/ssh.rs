//! SSH terminal session (FR-O1). Opens a TCP connection, authenticates, and
//! requests an interactive PTY + shell, exposed through the same
//! [`Connection`] trait the serial transport uses.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ssh2::Session;

use crate::connection::Connection;

/// How long to wait for the initial TCP connect before giving up.
///
/// `TcpStream::connect` alone has no timeout — against an unreachable or
/// firewalled host (one that drops packets rather than refusing the
/// connection) it can hang for far longer than a user will wait, since
/// that's bounded by the OS's own retransmission timeout, commonly a
/// minute or more. We resolve the address ourselves instead and bound each
/// attempt explicitly. This does not cover the handshake/auth steps that
/// follow — see README "Known limitations".
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// `TcpStream::connect` resolves `addr` and tries each address in turn with
/// no timeout; this does the same but with `TcpStream::connect_timeout`
/// (which takes a single resolved `SocketAddr`, not a hostname) bounding
/// each attempt.
fn connect_with_timeout(addr: &str, timeout: Duration) -> io::Result<TcpStream> {
    let mut last_err = None;
    for socket_addr in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&socket_addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::new(ErrorKind::AddrNotAvailable, "no addresses to connect to")))
}

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
        let tcp = connect_with_timeout(&addr, CONNECT_TIMEOUT)
            .with_context(|| format!("failed to reach SSH host '{addr}' within {CONNECT_TIMEOUT:?}"))?;
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

    fn resize(&mut self, cols: u32, rows: u32) -> io::Result<()> {
        // Non-blocking session: retry on WouldBlock, as `write_all` does.
        let mut spins = 0;
        loop {
            match self.channel.request_pty_size(cols, rows, None, None).map_err(io::Error::from) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    spins += 1;
                    if spins > 2000 {
                        return Err(io::Error::new(ErrorKind::TimedOut, "SSH resize timed out"));
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn refused_connection_fails_fast_not_after_the_full_timeout() {
        // Nothing listens on port 1; the OS refuses it almost immediately,
        // well before a generous 5s timeout would ever matter.
        let start = Instant::now();
        let result = connect_with_timeout("127.0.0.1:1", Duration::from_secs(5));
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_secs(1), "a refused connection should not wait out the timeout");
    }

    #[test]
    fn unreachable_host_does_not_hang_past_the_timeout() {
        // 10.255.255.1 is routed nowhere here, so packets are dropped
        // rather than refused: without an explicit timeout this is exactly
        // the case that would otherwise hang on the OS's own (much longer,
        // often a minute or more) retransmission timeout.
        let start = Instant::now();
        let result = connect_with_timeout("10.255.255.1:9", Duration::from_millis(300));
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_secs(5), "connect_with_timeout should bound how long it waits");
    }

    #[test]
    fn invalid_host_is_an_error_not_a_panic() {
        assert!(connect_with_timeout("this is not a valid host:name", Duration::from_secs(1)).is_err());
    }
}
