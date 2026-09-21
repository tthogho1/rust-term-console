//! Application state and logic (FR-3..FR-8). `ui.rs` renders this state
//! into egui widgets; this module owns the connect form, the live session
//! (log buffer, input line, connection).

use std::borrow::Cow;

use crate::ansi::AnsiFilter;
use crate::config::{self, Profile};
use crate::connection::{Connection, NewlineMode};
use crate::logfile::LogFile;
use crate::serial::{self, SerialConfig};
use crate::ssh::{SshAuth, SshConfig, SshConnection};

/// Cap on retained output so long-running sessions stay bounded (NFR-4).
const MAX_LOG_BYTES: usize = 2 * 1024 * 1024;

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
    pub input: String,
    pub newline_mode: NewlineMode,
    pub status: String,
    pub connected: bool,
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
            input: String::new(),
            newline_mode,
            status: "Connected".to_string(),
            connected: true,
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
        self.write_log_file(data);
        self.log.extend_from_slice(data);
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
}
