//! Transport-agnostic connection abstraction.
//!
//! `serial.rs` and `ssh.rs` each provide a `Connection` implementation so
//! `app.rs`/`main.rs` can drive either transport through the same loop.

use std::io;

/// A live, byte-oriented session (serial port or SSH shell channel).
pub trait Connection {
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
