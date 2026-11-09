//! Entry point: opens the native window and runs the eframe update loop.

mod ansi;
mod app;
mod config;
mod logfile;
mod notebook;
mod script;
mod timestamp;
mod transport;
mod ui;
mod vtgrid;

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;
use std::time::Duration;

use eframe::egui;
use egui_commonmark::CommonMarkCache;

use app::{ConnectForm, Session};
use logfile::LogFile;
use notebook::Cell;
use script::ScriptAction;
use transport::Connection;
use ui::{ConnectAction, HeaderAction, NotebookAction, ScriptPanelAction, TransferPanelAction};

/// A connect attempt running on a background thread (see `GuiApp::connect`),
/// polled once per frame rather than blocked on.
struct PendingConnect {
    receiver: Receiver<anyhow::Result<Box<dyn Connection>>>,
    newline: transport::NewlineMode,
    timestamps: bool,
    /// Carried along so it's only opened once, by `connect()`, even though
    /// it's attached to the `Session` after the background attempt finishes.
    log_file: Option<LogFile>,
}

struct GuiApp {
    form: ConnectForm,
    session: Option<Session>,
    /// Whether the connect side panel is open. Starts open so the first
    /// thing the user sees is how to connect.
    show_connect: bool,
    /// Set while a connect attempt is running on a background thread, so
    /// the UI thread never blocks on a slow DNS lookup or SSH handshake.
    connecting: Option<PendingConnect>,
    /// Notebook cells and whether their panel is open.
    cells: Vec<Cell>,
    show_notebook: bool,
    /// Path used by the notebook panel's Save/Load buttons.
    notebook_path: String,
    /// Set when a Save/Load in the notebook panel fails; cleared on success.
    notebook_error: Option<String>,
    /// Rendered-Markdown cache for note cells; kept across frames so the
    /// panel doesn't reparse every cell every repaint.
    md_cache: CommonMarkCache,
    /// Rhai script source and whether its panel is open.
    script_source: String,
    show_script: bool,
    /// Path used by the script panel's Save/Load buttons.
    script_path: String,
    /// What the script's last run printed/debugged, or the error it hit.
    script_output: String,
    script_error: Option<String>,
    /// Render a real terminal grid instead of the plain-text log (View →
    /// Real terminal). See `vtgrid.rs`.
    show_vt_grid: bool,
    /// Whether the file transfer panel is open. SSH-only (see
    /// `Session::ssh_session`); the panel says so when it's not available.
    show_transfer: bool,
    transfer_form: TransferForm,
    /// Set while a transfer is running on a background thread.
    transfer: Option<PendingTransfer>,
    /// Set when a transfer fails; cleared when one starts or succeeds.
    transfer_error: Option<String>,
}

/// The transfer panel's own fields — a local path, a remote path, and
/// which way the file goes between them.
struct TransferForm {
    local_path: String,
    remote_path: String,
    /// `true` sends `local_path` to `remote_path`; `false` fetches
    /// `remote_path` down to `local_path`.
    upload: bool,
}

impl Default for TransferForm {
    fn default() -> Self {
        Self { local_path: String::new(), remote_path: String::new(), upload: true }
    }
}

/// An SCP transfer running on a background thread (see `GuiApp::start_transfer`),
/// polled once per frame rather than blocked on.
struct PendingTransfer {
    receiver: Receiver<transport::scp::Event>,
    /// What's shown while it runs, e.g. "Uploading foo.txt".
    label: String,
    transferred: u64,
    total: u64,
}

impl Default for GuiApp {
    fn default() -> Self {
        Self {
            form: ConnectForm::default(),
            session: None,
            show_connect: true,
            connecting: None,
            cells: vec![Cell::command()],
            show_notebook: false,
            notebook_path: String::new(),
            notebook_error: None,
            md_cache: CommonMarkCache::default(),
            script_source: String::new(),
            show_script: false,
            script_path: String::new(),
            script_output: String::new(),
            script_error: None,
            show_vt_grid: false,
            show_transfer: false,
            transfer_form: TransferForm::default(),
            transfer: None,
            transfer_error: None,
        }
    }
}

impl GuiApp {
    /// Start connecting as described by the form, on a background thread
    /// (see `app::ConnectTarget::open`) so a slow DNS lookup or SSH
    /// handshake doesn't block the UI thread. `self.connecting` is polled
    /// once per frame in `ui()`; a bad path/port/baud is still reported
    /// immediately, since `build_target` validates before anything is
    /// spawned. Any live session is closed first so a serial port it holds
    /// is free to reopen. Ignored if already connecting — cancel first.
    fn connect(&mut self) {
        if self.connecting.is_some() {
            return;
        }

        // Open the log file before spawning the attempt so a bad path
        // fails cleanly instead of starting a background connect for
        // nothing.
        let log_file = if self.form.log_enabled {
            match LogFile::open(&self.form.log_path) {
                Ok(file) => Some(file),
                Err(e) => {
                    self.form.error = Some(format!("{e:#}"));
                    return;
                }
            }
        } else {
            None
        };

        let target = match self.form.build_target() {
            Ok(target) => target,
            Err(e) => {
                self.form.error = Some(e.to_string());
                return;
            }
        };

        if let Some(session) = self.session.as_mut() {
            session.disconnect();
        }
        self.form.error = None;

        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(target.open());
        });
        self.connecting = Some(PendingConnect {
            receiver: rx,
            newline: self.form.newline,
            timestamps: self.form.timestamps,
            log_file,
        });
    }

    /// Check on an in-flight connect attempt without blocking. Call once
    /// per frame; does nothing if there isn't one.
    fn poll_connect(&mut self) {
        let Some(pending) = self.connecting.as_mut() else {
            return;
        };
        match pending.receiver.try_recv() {
            Ok(Ok(conn)) => {
                let pending = self.connecting.take().expect("just matched Some above");
                let mut session = Session::new(conn, pending.newline);
                session.timestamps = pending.timestamps;
                if let Some(file) = pending.log_file {
                    session.set_log_file(file);
                }
                self.session = Some(session);
                self.show_connect = false;
            }
            Ok(Err(e)) => {
                self.form.error = Some(format!("{e:#}"));
                self.connecting = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                // The sending thread panicked without sending; don't leave
                // the panel stuck showing a spinner forever.
                self.form.error = Some("Connection attempt ended unexpectedly".to_string());
                self.connecting = None;
            }
        }
    }
}

impl GuiApp {
    /// Start logging the live session to the path in the form. A bad path is
    /// reported in the header status and leaves the session untouched.
    fn start_log(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match LogFile::open(&self.form.log_path) {
            Ok(file) => session.start_log(file),
            Err(e) => session.status = format!("Log start failed: {e:#}"),
        }
    }

    /// Run the script panel's source and send whatever it asked to send, in
    /// order. The script itself never touched the connection — it only
    /// built this list (see `script.rs`) — so sending it now goes through
    /// the normal `Session` path (local echo, history, logging) exactly as
    /// if typed by hand.
    fn run_script(&mut self) {
        let (actions, output, error) = script::run(&self.script_source);
        self.script_output = output;
        self.script_error = error;

        let Some(session) = self.session.as_mut() else {
            return;
        };
        for action in actions {
            match action {
                ScriptAction::Send(text) => session.send_text(&text),
                ScriptAction::Control(key) => session.send_control(key),
            }
        }
    }

    /// Start an SCP transfer as described by the form, on a background
    /// thread (see `transport::scp::run`) so a large file doesn't block
    /// the UI — same reasoning as `connect()`. Ignored if one is already
    /// running, or if the live connection isn't SSH.
    fn start_transfer(&mut self) {
        if self.transfer.is_some() {
            return;
        }
        let Some(session) = self.session.as_ref() else {
            self.transfer_error = Some("Not connected".to_string());
            return;
        };
        let Some(ssh_session) = session.ssh_session() else {
            self.transfer_error = Some("File transfer needs an SSH connection".to_string());
            return;
        };
        if self.transfer_form.local_path.trim().is_empty() || self.transfer_form.remote_path.trim().is_empty() {
            self.transfer_error = Some("Enter both a local and a remote path".to_string());
            return;
        }

        let local = std::path::PathBuf::from(self.transfer_form.local_path.trim());
        let remote = std::path::PathBuf::from(self.transfer_form.remote_path.trim());
        let (label, direction) = if self.transfer_form.upload {
            (format!("Uploading {}", local.display()), transport::scp::Direction::Upload { local, remote })
        } else {
            (format!("Downloading {}", remote.display()), transport::scp::Direction::Download { remote, local })
        };

        self.transfer_error = None;
        let (tx, rx) = channel();
        thread::spawn(move || {
            transport::scp::run(&ssh_session, &direction, |event| {
                let _ = tx.send(event);
            });
        });
        self.transfer = Some(PendingTransfer { receiver: rx, label, transferred: 0, total: 0 });
    }

    /// Check on an in-flight transfer without blocking. Call once per
    /// frame; does nothing if there isn't one. Drains every event queued
    /// this frame (not just the first) so a burst of `Progress` events
    /// from a fast local copy doesn't trickle in over several frames.
    fn poll_transfer(&mut self) {
        let Some(pending) = self.transfer.as_mut() else {
            return;
        };
        loop {
            match pending.receiver.try_recv() {
                Ok(transport::scp::Event::Progress { transferred, total }) => {
                    pending.transferred = transferred;
                    pending.total = total;
                }
                Ok(transport::scp::Event::Done) => {
                    self.transfer = None;
                    self.transfer_error = None;
                    return;
                }
                Ok(transport::scp::Event::Failed(message)) => {
                    self.transfer = None;
                    self.transfer_error = Some(message);
                    return;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    // The sending thread panicked without sending; don't
                    // leave the panel stuck showing progress forever.
                    self.transfer = None;
                    self.transfer_error = Some("Transfer ended unexpectedly".to_string());
                    return;
                }
            }
        }
    }
}

impl eframe::App for GuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_connect();
        self.poll_transfer();
        if self.connecting.is_some() || self.transfer.is_some() {
            // Keep polling even without mouse/keyboard input, same reason
            // draw_terminal does this while connected (see below).
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }

        if let Some(session) = self.session.as_mut() {
            session.timestamps = self.form.timestamps;
            session.poll_connection();
            session.flush_resize(std::time::Instant::now());
        }

        let header_action = ui::draw_header(
            ui,
            self.session.as_mut(),
            &mut self.show_connect,
            &mut self.show_notebook,
            &mut self.show_script,
            &mut self.show_vt_grid,
            &mut self.show_transfer,
            &mut self.form.timestamps,
            &mut self.form.log_path,
        );
        match header_action {
            HeaderAction::None => {}
            HeaderAction::Disconnect => {
                if let Some(session) = self.session.as_mut() {
                    session.disconnect();
                }
            }
            HeaderAction::StartLog => self.start_log(),
            HeaderAction::StopLog => {
                if let Some(session) = self.session.as_mut() {
                    session.stop_log();
                }
            }
        }

        if self.show_connect {
            match ui::draw_connect(ui, &mut self.form, self.connecting.is_some()) {
                ConnectAction::None => {}
                ConnectAction::Connect => self.connect(),
                ConnectAction::Cancel => self.connecting = None,
            }
        }

        if self.show_notebook {
            let can_run = self.session.as_ref().is_some_and(|s| s.connected);
            let action = ui::draw_notebook(
                ui,
                &mut self.cells,
                &mut self.notebook_path,
                self.notebook_error.as_deref(),
                &mut self.md_cache,
                can_run,
            );
            match action {
                NotebookAction::None => {}
                NotebookAction::Run(text) => {
                    if let Some(session) = self.session.as_mut() {
                        session.send_text(&text);
                    }
                }
                NotebookAction::Save => match notebook::save(&self.notebook_path, &self.cells) {
                    Ok(()) => self.notebook_error = None,
                    Err(e) => self.notebook_error = Some(format!("{e:#}")),
                },
                NotebookAction::Load => match notebook::load(&self.notebook_path) {
                    Ok(cells) => {
                        self.cells = cells;
                        self.notebook_error = None;
                    }
                    Err(e) => self.notebook_error = Some(format!("{e:#}")),
                },
            }
        }

        if self.show_script {
            let can_run = self.session.as_ref().is_some_and(|s| s.connected);
            let action = ui::draw_script(
                ui,
                &mut self.script_source,
                &mut self.script_path,
                &self.script_output,
                self.script_error.as_deref(),
                can_run,
            );
            match action {
                ScriptPanelAction::None => {}
                ScriptPanelAction::Run => self.run_script(),
                ScriptPanelAction::Save => match script::save(&self.script_path, &self.script_source) {
                    Ok(()) => self.script_error = None,
                    Err(e) => self.script_error = Some(format!("{e:#}")),
                },
                ScriptPanelAction::Load => match script::load(&self.script_path) {
                    Ok(source) => {
                        self.script_source = source;
                        self.script_error = None;
                    }
                    Err(e) => self.script_error = Some(format!("{e:#}")),
                },
            }
        }

        if self.show_transfer {
            let can_transfer = self.session.as_ref().is_some_and(|s| s.ssh_session().is_some());
            let progress = self
                .transfer
                .as_ref()
                .map(|t| (t.label.as_str(), t.transferred, t.total));
            let action = ui::draw_transfer(
                ui,
                &mut self.transfer_form.local_path,
                &mut self.transfer_form.remote_path,
                &mut self.transfer_form.upload,
                progress,
                self.transfer_error.as_deref(),
                can_transfer,
            );
            if let TransferPanelAction::Start = action {
                self.start_transfer();
            }
        }

        ui::draw_terminal(ui, self.session.as_mut(), self.show_vt_grid);
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([920.0, 620.0])
            .with_title("rust-term-console"),
        ..Default::default()
    };
    eframe::run_native(
        "rust-term-console",
        options,
        Box::new(|_cc| Ok(Box::new(GuiApp::default()))),
    )
}
