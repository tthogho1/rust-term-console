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
use ui::{ConnectAction, HeaderAction, NotebookAction, ScriptPanelAction};

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
}

impl eframe::App for GuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_connect();
        if self.connecting.is_some() {
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

        ui::draw_terminal(ui, self.session.as_mut());
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
