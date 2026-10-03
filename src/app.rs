//! Application state and logic (FR-3..FR-8). `ui.rs` renders this state
//! into egui widgets; this module owns the connect form, the live session
//! (log buffer, input line, connection).

use std::borrow::Cow;
use std::time::{Duration, Instant};

use crate::ansi::AnsiFilter;
use crate::config::{self, Profile};
use crate::connection::{Connection, ControlKey, NewlineMode};
use crate::logfile::LogFile;
use crate::serial::{self, SerialConfig};
use crate::ssh::{SshAuth, SshConfig, SshConnection};
use crate::timestamp::{self, LineStamper};

/// Cap on retained output so long-running sessions stay bounded (NFR-4).
const MAX_LOG_BYTES: usize = 2 * 1024 * 1024;

/// Cap on remembered command-history entries.
/// How long the window size must hold still before the remote is told, so
/// dragging a window edge sends one resize rather than one per frame.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(200);

const MAX_HISTORY: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnMode {
    Serial,
    Ssh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Password,
    Key,
    Agent,
}

/// Everything the connect dialog needs (FR-2). Lives across screens so
/// values survive a failed connection attempt.
pub struct ConnectForm {
    pub mode: ConnMode,

    // Serial fields (FR-2).
    pub available_ports: Vec<String>,
    pub serial_port: String,
    pub baud: String,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: u8,
    pub flow_control: String,

    // SSH fields (FR-O1).
    pub host: String,
    pub ssh_port: String,
    pub username: String,
    pub auth_mode: AuthMode,
    pub password: String,
    pub key_path: String,

    pub newline: NewlineMode,

    // Session logging (FR-O3).
    pub log_enabled: bool,
    pub log_path: String,
    /// Stamp each line of output with the local time (view and log file).
    pub timestamps: bool,

    // Profiles (FR-O4).
    pub available_profiles: Vec<String>,
    pub profile_name: String,

    pub error: Option<String>,
}

impl Default for ConnectForm {
    fn default() -> Self {
        Self {
            mode: ConnMode::Ssh,
            available_ports: serial::list_ports().unwrap_or_default(),
            serial_port: String::new(),
            baud: "115200".to_string(),
            data_bits: 8,
            parity: "none".to_string(),
            stop_bits: 1,
            flow_control: "none".to_string(),
            host: String::new(),
            ssh_port: "22".to_string(),
            username: String::new(),
            auth_mode: AuthMode::Password,
            password: String::new(),
            key_path: String::new(),
            newline: NewlineMode::CrLf,
            log_enabled: false,
            log_path: String::new(),
            timestamps: false,
            available_profiles: config::list_profiles().unwrap_or_default(),
            profile_name: String::new(),
            error: None,
        }
    }
}

impl ConnectForm {
    pub fn refresh_ports(&mut self) {
        self.available_ports = serial::list_ports().unwrap_or_default();
    }

    pub fn refresh_profiles(&mut self) {
        self.available_profiles = config::list_profiles().unwrap_or_default();
    }

    /// FR-O4: fill the form from a saved profile. Credentials are never
    /// stored, so passwords/keys are left for the user to re-enter.
    pub fn load_profile(&mut self, name: &str) {
        match config::load_profile(name) {
            Ok(Profile::Serial { port_name, baud_rate, newline }) => {
                self.mode = ConnMode::Serial;
                self.serial_port = port_name;
                self.baud = baud_rate.to_string();
                self.newline = newline.parse().unwrap_or(NewlineMode::CrLf);
            }
            Ok(Profile::Ssh { host, port, username, newline }) => {
                self.mode = ConnMode::Ssh;
                self.host = host;
                self.ssh_port = port.to_string();
                self.username = username;
                self.newline = newline.parse().unwrap_or(NewlineMode::CrLf);
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    pub fn save_profile(&mut self) {
        if self.profile_name.trim().is_empty() {
            self.error = Some("Enter a profile name before saving".to_string());
            return;
        }
        let result = match self.mode {
            ConnMode::Serial => self.baud.parse::<u32>().map_err(anyhow::Error::from).and_then(
                |baud_rate| {
                    config::save_profile(
                        &self.profile_name,
                        Profile::Serial {
                            port_name: self.serial_port.clone(),
                            baud_rate,
                            newline: self.newline.label().to_string(),
                        },
                    )
                },
            ),
            ConnMode::Ssh => self.ssh_port.parse::<u16>().map_err(anyhow::Error::from).and_then(
                |port| {
                    config::save_profile(
                        &self.profile_name,
                        Profile::Ssh {
                            host: self.host.clone(),
                            port,
                            username: self.username.clone(),
                            newline: self.newline.label().to_string(),
                        },
                    )
                },
            ),
        };
        match result {
            Ok(()) => {
                self.error = None;
                self.refresh_profiles();
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// FR-3: attempt to open the connection described by the form. Runs
    /// synchronously — a slow DNS lookup or handshake briefly blocks the
    /// UI thread (see README "Known limitations").
    pub fn connect(&mut self) -> Option<Box<dyn Connection>> {
        let result = match self.mode {
            ConnMode::Serial => self.connect_serial(),
            ConnMode::Ssh => self.connect_ssh(),
        };
        match result {
            Ok(conn) => {
                self.error = None;
                Some(conn)
            }
            Err(e) => {
                self.error = Some(e.to_string());
                None
            }
        }
    }

    fn connect_serial(&self) -> anyhow::Result<Box<dyn Connection>> {
        use serialport::{DataBits, FlowControl, Parity, StopBits};

        let baud_rate: u32 = self.baud.parse().map_err(|_| anyhow::anyhow!("invalid baud rate '{}'", self.baud))?;
        let data_bits = match self.data_bits {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        };
        let parity = match self.parity.as_str() {
            "odd" => Parity::Odd,
            "even" => Parity::Even,
            _ => Parity::None,
        };
        let stop_bits = if self.stop_bits == 2 { StopBits::Two } else { StopBits::One };
        let flow_control = match self.flow_control.as_str() {
            "rtscts" => FlowControl::Hardware,
            "xonxoff" => FlowControl::Software,
            _ => FlowControl::None,
        };

        let config = SerialConfig {
            port_name: self.serial_port.clone(),
            baud_rate,
            data_bits,
            parity,
            stop_bits,
            flow_control,
        };
        Ok(Box::new(crate::serial::SerialConnection::open(config)?))
    }

    fn connect_ssh(&self) -> anyhow::Result<Box<dyn Connection>> {
        let port: u16 = self.ssh_port.parse().map_err(|_| anyhow::anyhow!("invalid port '{}'", self.ssh_port))?;
        let auth = match self.auth_mode {
            AuthMode::Password => SshAuth::Password(self.password.clone()),
            AuthMode::Key => SshAuth::PrivateKey {
                path: self.key_path.clone().into(),
                passphrase: if self.password.is_empty() { None } else { Some(self.password.clone()) },
            },
            AuthMode::Agent => SshAuth::Agent,
        };
        let config = SshConfig {
            host: self.host.clone(),
            port,
            username: self.username.clone(),
            auth,
            term: "xterm-256color".to_string(),
            cols: 120,
            rows: 32,
        };
        Ok(Box::new(SshConnection::connect(config)?))
    }
}

/// A live connection plus its output log and pending input (FR-4, FR-6).
pub struct Session {
    connection: Option<Box<dyn Connection>>,
    connection_label: String,
    log: Vec<u8>,
    /// FR-O3: when set, everything appended to `log` is also written here.
    log_file: Option<LogFile>,
    ansi_filter: AnsiFilter,
    stamper: LineStamper,
    /// Stamp each new line (output and local echo) with the local time.
    pub timestamps: bool,
    pub input: String,
    /// Lines previously sent, oldest first (from the input bar and from
    /// notebook cells), for Up/Down recall in the input bar.
    history: Vec<String>,
    /// Position in `history` while recalling with Up/Down; `None` means the
    /// input bar holds a fresh line, not a recalled one.
    history_index: Option<usize>,
    /// What was in the input bar before the first Up press, restored when
    /// Down is pressed past the most recent history entry.
    history_draft: String,
    pub newline_mode: NewlineMode,
    pub status: String,
    pub connected: bool,
    /// Terminal size (cols, rows) last sent to the remote, if any.
    term_size: Option<(u32, u32)>,
    /// Latest size seen in the window and when it was first seen.
    pending_size: Option<((u32, u32), Instant)>,
}

impl Session {
    pub fn new(connection: Box<dyn Connection>, newline_mode: NewlineMode) -> Self {
        let connection_label = connection.describe();
        Self {
            connection: Some(connection),
            connection_label,
            log: Vec::new(),
            log_file: None,
            ansi_filter: AnsiFilter::default(),
            stamper: LineStamper::default(),
            timestamps: false,
            input: String::new(),
            history: Vec::new(),
            history_index: None,
            history_draft: String::new(),
            newline_mode,
            status: "Connected".to_string(),
            connected: true,
            term_size: None,
            pending_size: None,
        }
    }

    pub fn connection_label(&self) -> &str {
        &self.connection_label
    }

    pub fn log_path(&self) -> Option<&std::path::Path> {
        self.log_file.as_ref().map(LogFile::path)
    }

    /// FR-O3: start mirroring the log to `file`, tagged with the connection
    /// so appended sessions can be told apart.
    pub fn set_log_file(&mut self, file: LogFile) {
        self.log_file = Some(file);
        let header = format!("# {}\n", self.connection_label);
        self.write_log_file(header.as_bytes());
    }

    /// Start logging in the middle of a session. Only output from now on is
    /// written; what is already on screen is not back-filled.
    pub fn start_log(&mut self, file: LogFile) {
        self.set_log_file(file);
        self.status = "Logging started".to_string();
    }

    pub fn stop_log(&mut self) {
        if self.log_file.take().is_some() {
            self.status = "Logging stopped".to_string();
        }
    }

    /// A write error stops logging (and says so in the status bar) but never
    /// takes the connection down.
    fn write_log_file(&mut self, data: &[u8]) {
        if let Some(file) = self.log_file.as_mut()
            && let Err(e) = file.write(data)
        {
            self.status = format!("Log write failed, logging stopped: {e}");
            self.log_file = None;
        }
    }

    pub fn log_text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.log)
    }

    fn append_log(&mut self, data: &[u8]) {
        let stamp = self.timestamps.then(timestamp::now);
        let data = self.stamper.apply(data, stamp.as_deref());
        self.write_log_file(&data);
        self.log.extend_from_slice(&data);
        if self.log.len() > MAX_LOG_BYTES {
            let excess = self.log.len() - MAX_LOG_BYTES;
            self.log.drain(0..excess);
        }
    }

    /// FR-4: pull whatever the transport has for us. FR-5/NFR-6: a read
    /// error (device unplugged, remote closed the channel) disconnects
    /// cleanly and surfaces a message instead of crashing.
    pub fn poll_connection(&mut self) {
        let Some(conn) = self.connection.as_mut() else {
            return;
        };
        match conn.read_available() {
            Ok(data) if !data.is_empty() => {
                let cleaned = self.ansi_filter.filter(&data);
                self.append_log(&cleaned);
            }
            Ok(_) => {}
            Err(e) => {
                self.status = format!("Disconnected: {e}");
                self.connected = false;
                if let Some(mut conn) = self.connection.take() {
                    conn.close();
                }
            }
        }
    }

    /// Note the log area's current size in characters. It is sent to the
    /// remote by `flush_resize` once it has been stable for a moment.
    pub fn request_resize(&mut self, cols: u32, rows: u32, now: Instant) {
        let size = (cols.max(1), rows.max(1));
        match self.pending_size {
            Some((pending, _)) if pending == size => {}
            _ if self.pending_size.is_none() && self.term_size == Some(size) => {}
            _ => self.pending_size = Some((size, now)),
        }
    }

    /// Send a pending size once it has held for `RESIZE_DEBOUNCE`. A failed
    /// resize is reported but doesn't drop the connection — a real link
    /// failure is caught by the next read.
    pub fn flush_resize(&mut self, now: Instant) {
        let Some((size, since)) = self.pending_size else {
            return;
        };
        if now.duration_since(since) < RESIZE_DEBOUNCE {
            return;
        }
        self.pending_size = None;
        if self.term_size == Some(size) {
            return;
        }
        let Some(conn) = self.connection.as_mut() else {
            return;
        };
        match conn.resize(size.0, size.1) {
            Ok(()) => self.term_size = Some(size),
            Err(e) => self.status = format!("Resize failed: {e}"),
        }
    }

    /// FR-4/FR-7: send the current input line, appending the configured
    /// newline, then clear the input box.
    pub fn send_current_input(&mut self) {
        if !self.connected || self.input.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.input);
        self.send_text(&text);
    }

    /// Send `text` one line at a time, each followed by the configured
    /// newline. Used by the input bar and by notebook cells, so both show up
    /// in the log the same way.
    pub fn send_text(&mut self, text: &str) {
        if !self.connected {
            return;
        }
        for line in text.lines() {
            self.record_history(line);

            let mut payload = line.as_bytes().to_vec();
            payload.extend_from_slice(self.newline_mode.as_bytes());

            // Local echo so the sent line is visible even if the remote
            // doesn't echo it back.
            self.append_log(line.as_bytes());
            self.append_log(self.newline_mode.as_bytes());

            if let Some(conn) = self.connection.as_mut()
                && let Err(e) = conn.write_all(&payload)
            {
                self.status = format!("Send failed: {e}");
                self.connected = false;
                if let Some(mut conn) = self.connection.take() {
                    conn.close();
                }
                return;
            }
        }
    }

    /// Send `key` as its raw byte, with no newline. Tab first sends whatever
    /// is typed in the input bar (and clears it) so the remote shell can
    /// complete it; the rest of the line is then typed and sent as usual.
    /// The other keys leave the input bar alone and are not echoed locally —
    /// the remote shows its own `^C` etc. if it wants to.
    pub fn send_control(&mut self, key: ControlKey) {
        if !self.connected {
            return;
        }
        let mut payload = Vec::new();
        if key == ControlKey::Tab {
            let partial = std::mem::take(&mut self.input);
            self.history_index = None;
            self.history_draft.clear();
            self.append_log(partial.as_bytes());
            payload.extend_from_slice(partial.as_bytes());
        }
        payload.push(key.byte());

        if let Some(conn) = self.connection.as_mut()
            && let Err(e) = conn.write_all(&payload)
        {
            self.status = format!("Send failed: {e}");
            self.connected = false;
            if let Some(mut conn) = self.connection.take() {
                conn.close();
            }
        }
    }

    /// Remember `line` for Up/Down recall, skipping blanks and immediate
    /// repeats (like typical shell history). Also ends any in-progress
    /// recall, since the input bar just sent something new.
    fn record_history(&mut self, line: &str) {
        self.history_index = None;
        self.history_draft.clear();
        if line.is_empty() || self.history.last().is_some_and(|last| last == line) {
            return;
        }
        self.history.push(line.to_string());
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }
    }

    /// Recall an older history entry into `input` (Up). The first call
    /// stashes whatever was already typed so Down can restore it later.
    pub fn history_prev(&mut self) {
        let new_index = match self.history_index {
            None => {
                if self.history.is_empty() {
                    return;
                }
                self.history_draft = std::mem::take(&mut self.input);
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_index = Some(new_index);
        self.input = self.history[new_index].clone();
    }

    /// Recall a newer history entry into `input` (Down), or restore the
    /// pre-recall draft once past the most recent entry.
    pub fn history_next(&mut self) {
        let Some(i) = self.history_index else {
            return;
        };
        if i + 1 < self.history.len() {
            self.history_index = Some(i + 1);
            self.input = self.history[i + 1].clone();
        } else {
            self.history_index = None;
            self.input = std::mem::take(&mut self.history_draft);
        }
    }

    /// FR-8: close the connection cleanly.
    pub fn disconnect(&mut self) {
        if let Some(mut conn) = self.connection.take() {
            conn.close();
        }
        self.connected = false;
        self.status = "Disconnected".to_string();
        self.log_file = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockConnection {
        incoming: Vec<u8>,
    }

    impl Connection for MockConnection {
        fn read_available(&mut self) -> std::io::Result<Vec<u8>> {
            Ok(std::mem::take(&mut self.incoming))
        }
        fn write_all(&mut self, _data: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        fn describe(&self) -> String {
            "mock".to_string()
        }
        fn close(&mut self) {}
    }

    #[test]
    fn log_file_mirrors_output_and_sent_lines() {
        let dir = std::env::temp_dir().join(format!("rtc-session-{}", std::process::id()));
        let path = dir.join("session.log");
        let _ = std::fs::remove_dir_all(&dir);

        let conn = MockConnection { incoming: b"\x1b[32mhello\x1b[0m\r\n".to_vec() };
        let mut session = Session::new(Box::new(conn), NewlineMode::Lf);
        session.set_log_file(LogFile::open(path.to_str().unwrap()).unwrap());
        session.poll_connection();
        session.send_text("ls");
        session.disconnect();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mock\nhello\nls\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_can_start_and_stop_mid_session() {
        let dir = std::env::temp_dir().join(format!("rtc-midlog-{}", std::process::id()));
        let path = dir.join("session.log");
        let _ = std::fs::remove_dir_all(&dir);

        let conn = MockConnection { incoming: Vec::new() };
        let mut session = Session::new(Box::new(conn), NewlineMode::Lf);
        session.send_text("before");
        session.start_log(LogFile::open(path.to_str().unwrap()).unwrap());
        assert!(session.log_path().is_some());
        session.send_text("during");
        session.stop_log();
        assert!(session.log_path().is_none());
        session.send_text("after");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mock\nduring\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn mock_session() -> Session {
        Session::new(Box::new(MockConnection { incoming: Vec::new() }), NewlineMode::Lf)
    }

    #[test]
    fn history_up_and_down_recall_sent_lines_in_order() {
        let mut session = mock_session();
        session.send_text("first");
        session.send_text("second");

        session.history_prev();
        assert_eq!(session.input, "second");
        session.history_prev();
        assert_eq!(session.input, "first");
        // Oldest entry: further Up stays put rather than wrapping.
        session.history_prev();
        assert_eq!(session.input, "first");

        session.history_next();
        assert_eq!(session.input, "second");
    }

    #[test]
    fn history_down_past_newest_restores_the_unsent_draft() {
        let mut session = mock_session();
        session.send_text("ls");
        session.input = "unsent draft".to_string();

        session.history_prev();
        assert_eq!(session.input, "ls");
        session.history_next();
        assert_eq!(session.input, "unsent draft");
    }

    #[test]
    fn history_skips_blank_lines_and_immediate_repeats() {
        let mut session = mock_session();
        session.send_text("ls");
        session.send_text("ls");
        session.send_text("");

        session.history_prev();
        assert_eq!(session.input, "ls");
        // Only one "ls" was recorded, so a second Up has nowhere further to go.
        session.history_prev();
        assert_eq!(session.input, "ls");
    }

    #[test]
    fn sending_a_new_line_ends_an_in_progress_recall() {
        let mut session = mock_session();
        session.send_text("first");
        session.history_prev();
        assert_eq!(session.input, "first");

        session.input = "second".to_string();
        session.send_current_input();

        // No stale draft or recall position left over from before the send.
        session.history_prev();
        assert_eq!(session.input, "second");
    }

    /// Records everything written so tests can check the exact bytes sent.
    struct RecordingConnection {
        sent: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    }

    impl Connection for RecordingConnection {
        fn read_available(&mut self) -> std::io::Result<Vec<u8>> {
            Ok(Vec::new())
        }
        fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
            self.sent.borrow_mut().extend_from_slice(data);
            Ok(())
        }
        fn describe(&self) -> String {
            "recording".to_string()
        }
        fn close(&mut self) {}
    }

    fn recording_session() -> (Session, std::rc::Rc<std::cell::RefCell<Vec<u8>>>) {
        let sent = std::rc::Rc::default();
        let conn = RecordingConnection { sent: std::rc::Rc::clone(&sent) };
        (Session::new(Box::new(conn), NewlineMode::CrLf), sent)
    }

    #[test]
    fn control_keys_send_their_raw_byte_without_a_newline() {
        let (mut session, sent) = recording_session();
        session.input = "half typed".to_string();

        session.send_control(ControlKey::CtrlC);
        session.send_control(ControlKey::CtrlD);
        session.send_control(ControlKey::CtrlZ);
        session.send_control(ControlKey::Esc);

        assert_eq!(*sent.borrow(), b"\x03\x04\x1a\x1b");
        // The input bar's text was not part of any of those sends.
        assert_eq!(session.input, "half typed");
        assert!(session.log_text().is_empty());
    }

    #[test]
    fn tab_sends_the_typed_prefix_then_a_tab_and_clears_the_input() {
        let (mut session, sent) = recording_session();
        session.input = "cd /us".to_string();

        session.send_control(ControlKey::Tab);

        assert_eq!(*sent.borrow(), b"cd /us\t");
        assert_eq!(session.input, "");
        assert_eq!(session.log_text(), "cd /us");
        // A partial line is not history.
        session.history_prev();
        assert_eq!(session.input, "");
    }

    #[test]
    fn control_keys_are_not_sent_once_disconnected() {
        let (mut session, sent) = recording_session();
        session.disconnect();
        session.send_control(ControlKey::CtrlC);
        assert!(sent.borrow().is_empty());
    }

    #[test]
    fn timestamps_prefix_output_and_echoed_lines() {
        let conn = MockConnection { incoming: b"remote line\n".to_vec() };
        let mut session = Session::new(Box::new(conn), NewlineMode::Lf);
        session.timestamps = true;
        session.poll_connection();
        session.send_text("ls");

        let log = session.log_text();
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with('[') && lines[0].ends_with("] remote line"));
        assert!(lines[1].starts_with('[') && lines[1].ends_with("] ls"));
    }

    type SentSizes = std::rc::Rc<std::cell::RefCell<Vec<(u32, u32)>>>;

    /// Records each resize the session sends.
    struct ResizeConnection {
        sizes: SentSizes,
    }

    impl Connection for ResizeConnection {
        fn read_available(&mut self) -> std::io::Result<Vec<u8>> {
            Ok(Vec::new())
        }
        fn write_all(&mut self, _data: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        fn resize(&mut self, cols: u32, rows: u32) -> std::io::Result<()> {
            self.sizes.borrow_mut().push((cols, rows));
            Ok(())
        }
        fn describe(&self) -> String {
            "resize".to_string()
        }
        fn close(&mut self) {}
    }

    fn resize_session() -> (Session, SentSizes) {
        let sizes = std::rc::Rc::default();
        let conn = ResizeConnection { sizes: std::rc::Rc::clone(&sizes) };
        (Session::new(Box::new(conn), NewlineMode::Lf), sizes)
    }

    #[test]
    fn resize_is_sent_once_the_size_holds_still() {
        let (mut session, sizes) = resize_session();
        let t0 = Instant::now();

        session.request_resize(100, 30, t0);
        session.flush_resize(t0 + Duration::from_millis(50));
        assert!(sizes.borrow().is_empty());

        session.flush_resize(t0 + RESIZE_DEBOUNCE);
        assert_eq!(*sizes.borrow(), [(100, 30)]);
    }

    #[test]
    fn dragging_sends_only_the_final_size() {
        let (mut session, sizes) = resize_session();
        let t0 = Instant::now();
        let ms = Duration::from_millis;

        // Each frame of a drag reports a new size, restarting the wait.
        for (i, cols) in [90, 95, 100, 105].into_iter().enumerate() {
            let now = t0 + ms(30 * i as u64);
            session.request_resize(cols, 30, now);
            session.flush_resize(now);
        }
        assert!(sizes.borrow().is_empty());

        session.flush_resize(t0 + ms(90) + RESIZE_DEBOUNCE);
        assert_eq!(*sizes.borrow(), [(105, 30)]);
    }

    #[test]
    fn unchanged_size_is_not_resent() {
        let (mut session, sizes) = resize_session();
        let t0 = Instant::now();
        session.request_resize(80, 24, t0);
        session.flush_resize(t0 + RESIZE_DEBOUNCE);

        // Same size reported every frame afterwards: nothing more is sent.
        let later = t0 + RESIZE_DEBOUNCE * 2;
        session.request_resize(80, 24, later);
        session.flush_resize(later + RESIZE_DEBOUNCE);
        assert_eq!(*sizes.borrow(), [(80, 24)]);
    }

    #[test]
    fn size_back_to_the_sent_one_mid_drag_is_not_resent() {
        let (mut session, sizes) = resize_session();
        let t0 = Instant::now();
        session.request_resize(80, 24, t0);
        session.flush_resize(t0 + RESIZE_DEBOUNCE);

        let t1 = t0 + RESIZE_DEBOUNCE * 2;
        session.request_resize(90, 24, t1);
        session.request_resize(80, 24, t1 + Duration::from_millis(10));
        session.flush_resize(t1 + RESIZE_DEBOUNCE * 2);
        assert_eq!(*sizes.borrow(), [(80, 24)]);
    }
}
