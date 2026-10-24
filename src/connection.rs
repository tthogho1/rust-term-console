//! Transport-agnostic connection abstraction.
//!
//! `serial.rs`, `ssh.rs` and `telnet.rs` each provide a `Connection`
//! implementation so `app.rs`/`main.rs` can drive any transport through the
//! same loop.

use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// A live, byte-oriented session (serial port or SSH shell channel).
///
/// `Send` so a `Box<dyn Connection>` can be built on a background thread
/// (see `app::ConnectTarget::open`) and handed back to the UI thread once
/// the connect/handshake is done, instead of blocking it.
pub trait Connection: Send {
    /// Return whatever bytes are available right now without blocking for
    /// long. An empty `Vec` means "nothing to read yet", not EOF.
    fn read_available(&mut self) -> io::Result<Vec<u8>>;

    /// Write bytes to the remote end, blocking until fully sent.
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;

    /// Human-readable summary for the status bar, e.g. "COM3 @ 115200" or
    /// "user@host:22".
    fn describe(&self) -> String;

    /// Tell the remote the terminal is now `cols` x `rows` characters, so
    /// full-width output (`ls`, `top`, prompts) wraps where the window does.
    /// Transports without a terminal size (serial) ignore it.
    fn resize(&mut self, _cols: u32, _rows: u32) -> io::Result<()> {
        Ok(())
    }

    /// Best-effort clean shutdown. Errors are logged, not propagated.
    fn close(&mut self);
}

/// Line ending to append when the user presses Enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewlineMode {
    None,
    Lf,
    CrLf,
}

impl NewlineMode {
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            NewlineMode::None => b"",
            NewlineMode::Lf => b"\n",
            NewlineMode::CrLf => b"\r\n",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NewlineMode::None => "raw",
            NewlineMode::Lf => "LF",
            NewlineMode::CrLf => "CRLF",
        }
    }

    pub const ALL: [NewlineMode; 3] = [NewlineMode::CrLf, NewlineMode::Lf, NewlineMode::None];
}

/// A key sent as its raw control byte rather than as a typed line: lets the
/// user interrupt a running command, end input, suspend, complete or escape
/// on the remote side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKey {
    CtrlC,
    CtrlD,
    CtrlZ,
    Tab,
    Esc,
}

impl ControlKey {
    pub fn byte(self) -> u8 {
        match self {
            ControlKey::CtrlC => 0x03,
            ControlKey::CtrlD => 0x04,
            ControlKey::CtrlZ => 0x1a,
            ControlKey::Tab => b'\t',
            ControlKey::Esc => 0x1b,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ControlKey::CtrlC => "Ctrl+C (interrupt)",
            ControlKey::CtrlD => "Ctrl+D (end of input)",
            ControlKey::CtrlZ => "Ctrl+Z (suspend)",
            ControlKey::Tab => "Tab (complete)",
            ControlKey::Esc => "Esc",
        }
    }

    pub const ALL: [ControlKey; 5] =
        [ControlKey::CtrlC, ControlKey::CtrlD, ControlKey::CtrlZ, ControlKey::Tab, ControlKey::Esc];
}

impl std::str::FromStr for NewlineMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "none" | "raw" => Ok(NewlineMode::None),
            "lf" => Ok(NewlineMode::Lf),
            "crlf" => Ok(NewlineMode::CrLf),
            other => anyhow::bail!("unknown newline mode '{other}' (expected none|lf|crlf)"),
        }
    }
}

/// Resolve `addr` ("host:port") and connect to the first address that
/// accepts within `timeout`. Shared by the SSH and Telnet transports,
/// which both open a plain TCP connection before doing anything else.
///
/// `TcpStream::connect` alone has no timeout: against a host that drops
/// packets rather than refusing the connection, that means waiting out the
/// OS's own retransmission timeout instead (commonly a minute or more).
/// `TcpStream::connect_timeout` bounds that, but only takes a single
/// resolved `SocketAddr`, not a hostname — so `connect`'s "try each
/// resolved address in turn" behavior has to be done by hand here.
pub fn connect_tcp_with_timeout(addr: &str, timeout: Duration) -> io::Result<TcpStream> {
    let mut last_err = None;
    for socket_addr in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&socket_addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "no addresses to connect to")))
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
        let result = connect_tcp_with_timeout("127.0.0.1:1", Duration::from_secs(5));
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
        let result = connect_tcp_with_timeout("10.255.255.1:9", Duration::from_millis(300));
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_secs(5), "connect_tcp_with_timeout should bound how long it waits");
    }

    #[test]
    fn invalid_host_is_an_error_not_a_panic() {
        assert!(connect_tcp_with_timeout("this is not a valid host:name", Duration::from_secs(1)).is_err());
    }
}
