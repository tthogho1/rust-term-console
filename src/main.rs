//! Entry point: opens the native window and runs the eframe update loop.

mod ansi;
mod app;
mod config;
mod connection;
mod serial;
mod ssh;
mod ui;

use eframe::egui;

use app::{ConnectForm, Screen, Session};
use ui::{ConnectAction, TerminalAction};

#[derive(Default)]
struct GuiApp {
    screen: Screen,
}

impl eframe::App for GuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        match &mut self.screen {
            Screen::Connect(form) => {
                if let ConnectAction::Connected(conn, newline) = crate::ui::draw_connect(ui, form) {
                    self.screen = Screen::Terminal(Session::new(conn, newline));
                }
            }
            Screen::Terminal(session) => {
                session.poll_connection();
                if let TerminalAction::Disconnect = crate::ui::draw_terminal(ui, session) {
                    session.disconnect();
                    self.screen = Screen::Connect(ConnectForm::default());
                }
            }
        }
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
