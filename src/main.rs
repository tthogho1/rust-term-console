//! Entry point: opens the native window and runs the eframe update loop.

mod ansi;
mod app;
mod config;
mod connection;
mod serial;
mod ssh;
mod ui;

use eframe::egui;

use app::{ConnectForm, Session};
use ui::{ConnectAction, HeaderAction};

struct GuiApp {
    form: ConnectForm,
    session: Option<Session>,
    /// Whether the connect side panel is open. Starts open so the first
    /// thing the user sees is how to connect.
    show_connect: bool,
    /// Notebook cells (command text only) and whether their panel is open.
    cells: Vec<String>,
    show_notebook: bool,
}

impl Default for GuiApp {
    fn default() -> Self {
        Self {
            form: ConnectForm::default(),
            session: None,
            show_connect: true,
            cells: vec![String::new()],
            show_notebook: false,
        }
    }
}

impl GuiApp {
    /// Open the connection described by the form. Any live session is closed
    /// first so a serial port it holds is free to reopen. On failure the
    /// error stays in the form and the panel stays open.
    fn connect(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.disconnect();
        }
        let newline = self.form.newline;
        if let Some(conn) = self.form.connect() {
            self.session = Some(Session::new(conn, newline));
            self.show_connect = false;
        }
    }
}

impl eframe::App for GuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(session) = self.session.as_mut() {
            session.poll_connection();
        }

        if let HeaderAction::Disconnect = ui::draw_header(ui, self.session.as_mut(), &mut self.show_connect, &mut self.show_notebook)
            && let Some(session) = self.session.as_mut()
        {
            session.disconnect();
        }

        if self.show_connect
            && let ConnectAction::Connect = ui::draw_connect(ui, &mut self.form)
        {
            self.connect();
        }

        if self.show_notebook {
            let can_run = self.session.as_ref().is_some_and(|s| s.connected);
            if let Some(text) = ui::draw_notebook(ui, &mut self.cells, can_run)
                && let Some(session) = self.session.as_mut()
            {
                session.send_text(&text);
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
