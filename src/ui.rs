//! GUI rendering (FR-6, FR-7). The window is always the terminal (header +
//! log + input); the connect form (FR-2, FR-3) is a left side panel toggled
//! by the header's "Connection" button, and the notebook cells are a right
//! side panel toggled from the header's "View" menu. Cells only send
//! commands; their output lands in the shared log.

use eframe::egui::{self, Color32, ComboBox, RichText, ScrollArea, TextEdit};

use crate::app::{AuthMode, ConnMode, ConnectForm, Session};
use crate::connection::NewlineMode;

pub enum ConnectAction {
    None,
    /// The Connect button was pressed; the caller opens the connection.
    Connect,
}

pub enum HeaderAction {
    None,
    Disconnect,
    /// Start logging to the path in the header's Log menu.
    StartLog,
    StopLog,
}

/// Connect form, drawn as a left side panel. Call only while it is shown.
pub fn draw_connect(ui: &mut egui::Ui, form: &mut ConnectForm) -> ConnectAction {
    let mut action = ConnectAction::None;

    egui::Panel::left("connect_panel").default_size(320.0).show(ui, |ui| {
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            draw_connect_form(ui, form, &mut action);
        });
    });

    action
}

fn draw_connect_form(ui: &mut egui::Ui, form: &mut ConnectForm, action: &mut ConnectAction) {
    ui.heading("Connection");
    ui.label("Connect over SSH or a serial port.");
    ui.separator();

    ui.horizontal(|ui| {
        ui.radio_value(&mut form.mode, ConnMode::Ssh, "SSH");
        ui.radio_value(&mut form.mode, ConnMode::Serial, "Serial");
    });
    ui.add_space(8.0);

    match form.mode {
        ConnMode::Ssh => draw_ssh_fields(ui, form),
        ConnMode::Serial => draw_serial_fields(ui, form),
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label("Newline on Enter:");
        ComboBox::from_id_salt("newline_mode")
            .selected_text(form.newline.label())
            .show_ui(ui, |ui| {
                for mode in NewlineMode::ALL {
                    ui.selectable_value(&mut form.newline, mode, mode.label());
                }
            });
    });

    ui.add_space(8.0);
    ui.checkbox(&mut form.log_enabled, "Save session log to file");
    if form.log_enabled {
        ui.add(
            TextEdit::singleline(&mut form.log_path)
                .desired_width(f32::INFINITY)
                .hint_text("Log file path, e.g. ~/logs/session.log"),
        );
    }

    ui.separator();
    draw_profiles(ui, form);

    ui.add_space(8.0);
    if let Some(err) = &form.error {
        ui.colored_label(Color32::from_rgb(220, 60, 60), err);
    }

    if ui.button(RichText::new("Connect").strong()).clicked() {
        *action = ConnectAction::Connect;
    }
}

fn draw_ssh_fields(ui: &mut egui::Ui, form: &mut ConnectForm) {
    egui::Grid::new("ssh_grid").num_columns(2).show(ui, |ui| {
        ui.label("Host");
        ui.text_edit_singleline(&mut form.host);
        ui.end_row();

        ui.label("Port");
        ui.text_edit_singleline(&mut form.ssh_port);
        ui.end_row();

        ui.label("Username");
        ui.text_edit_singleline(&mut form.username);
        ui.end_row();

        ui.label("Auth");
        ui.horizontal(|ui| {
            ui.radio_value(&mut form.auth_mode, AuthMode::Password, "Password");
            ui.radio_value(&mut form.auth_mode, AuthMode::Key, "Key file");
            ui.radio_value(&mut form.auth_mode, AuthMode::Agent, "ssh-agent");
        });
        ui.end_row();

        match form.auth_mode {
            AuthMode::Password => {
                ui.label("Password");
                ui.add(TextEdit::singleline(&mut form.password).password(true));
                ui.end_row();
            }
            AuthMode::Key => {
                ui.label("Key path");
                ui.text_edit_singleline(&mut form.key_path);
                ui.end_row();
                ui.label("Passphrase");
                ui.add(TextEdit::singleline(&mut form.password).password(true));
                ui.end_row();
            }
            AuthMode::Agent => {}
        }
    });
}

fn draw_serial_fields(ui: &mut egui::Ui, form: &mut ConnectForm) {
    ui.horizontal(|ui| {
        ComboBox::from_id_salt("serial_port")
            .selected_text(if form.serial_port.is_empty() { "Select a port…" } else { &form.serial_port })
            .show_ui(ui, |ui| {
                for port in form.available_ports.clone() {
                    ui.selectable_value(&mut form.serial_port, port.clone(), port);
                }
            });
        if ui.button("Refresh").clicked() {
            form.refresh_ports();
        }
    });

    egui::Grid::new("serial_grid").num_columns(2).show(ui, |ui| {
        ui.label("Baud rate");
        ui.text_edit_singleline(&mut form.baud);
        ui.end_row();

        ui.label("Data bits");
        ComboBox::from_id_salt("data_bits")
            .selected_text(form.data_bits.to_string())
            .show_ui(ui, |ui| {
                for bits in [5u8, 6, 7, 8] {
                    ui.selectable_value(&mut form.data_bits, bits, bits.to_string());
                }
            });
        ui.end_row();

        ui.label("Parity");
        ComboBox::from_id_salt("parity")
            .selected_text(form.parity.clone())
            .show_ui(ui, |ui| {
                for p in ["none", "odd", "even"] {
                    ui.selectable_value(&mut form.parity, p.to_string(), p);
                }
            });
        ui.end_row();

        ui.label("Stop bits");
        ComboBox::from_id_salt("stop_bits")
            .selected_text(form.stop_bits.to_string())
            .show_ui(ui, |ui| {
                for bits in [1u8, 2] {
                    ui.selectable_value(&mut form.stop_bits, bits, bits.to_string());
                }
            });
        ui.end_row();

        ui.label("Flow control");
        ComboBox::from_id_salt("flow_control")
            .selected_text(form.flow_control.clone())
            .show_ui(ui, |ui| {
                for fc in ["none", "rtscts", "xonxoff"] {
                    ui.selectable_value(&mut form.flow_control, fc.to_string(), fc);
                }
            });
        ui.end_row();
    });
}

fn draw_profiles(ui: &mut egui::Ui, form: &mut ConnectForm) {
    ui.label(RichText::new("Profiles").strong());
    ui.horizontal(|ui| {
        let mut selected: Option<String> = None;
        ComboBox::from_id_salt("load_profile")
            .selected_text("Load saved profile…")
            .show_ui(ui, |ui| {
                for name in form.available_profiles.clone() {
                    if ui.selectable_label(false, &name).clicked() {
                        selected = Some(name);
                    }
                }
            });
        if ui.button("↻").on_hover_text("Refresh profile list").clicked() {
            form.refresh_profiles();
        }
        if let Some(name) = selected {
            form.load_profile(&name);
        }
    });
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut form.profile_name);
        if ui.button("Save as profile").clicked() {
            form.save_profile();
        }
    });
}

/// Top header bar: the "Connection" toggle, the "View" and "Log" menus and
/// connection status.
pub fn draw_header(
    ui: &mut egui::Ui,
    session: Option<&mut Session>,
    show_connect: &mut bool,
    show_notebook: &mut bool,
    log_path: &mut String,
) -> HeaderAction {
    let mut action = HeaderAction::None;

    egui::Panel::top("header").show(ui, |ui| {
        ui.horizontal(|ui| {
            if ui.selectable_label(*show_connect, "Connection").clicked() {
                *show_connect = !*show_connect;
            }
            ui.menu_button("View", |ui| {
                ui.checkbox(show_notebook, "Notebook");
            });

            let connected = session.as_ref().is_some_and(|s| s.connected);
            let logging_to = session.as_ref().and_then(|s| s.log_path()).map(|p| p.display().to_string());
            ui.menu_button("Log", |ui| {
                if !connected {
                    ui.label("Connect first to start logging.");
                    return;
                }
                match &logging_to {
                    Some(path) => {
                        ui.label(format!("Logging to {path}"));
                        if ui.button("Stop logging").clicked() {
                            action = HeaderAction::StopLog;
                            ui.close();
                        }
                    }
                    None => {
                        ui.label("Log file path");
                        let response = ui.add(
                            TextEdit::singleline(log_path)
                                .desired_width(260.0)
                                .hint_text("~/logs/session.log"),
                        );
                        let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("Start logging").clicked() || enter_pressed {
                            action = HeaderAction::StartLog;
                            ui.close();
                        }
                    }
                }
            });
            ui.separator();

            let (text, color) = if connected {
                ("CONNECTED", Color32::from_rgb(30, 150, 60))
            } else {
                ("DISCONNECTED", Color32::from_rgb(190, 40, 40))
            };
            ui.colored_label(color, RichText::new(text).strong());

            match session {
                Some(session) => {
                    ui.label(session.connection_label().to_string());
                    if let Some(path) = session.log_path() {
                        ui.label(RichText::new("● LOG").color(Color32::from_rgb(30, 110, 200)))
                            .on_hover_text(path.display().to_string());
                    }
                    ui.separator();
                    ComboBox::from_id_salt("live_newline")
                        .selected_text(session.newline_mode.label())
                        .show_ui(ui, |ui| {
                            for mode in NewlineMode::ALL {
                                ui.selectable_value(&mut session.newline_mode, mode, mode.label());
                            }
                        });
                    ui.separator();
                    ui.label(session.status.clone());
                }
                None => {
                    ui.label("Not connected");
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(connected, egui::Button::new("Disconnect")).clicked() {
                    action = HeaderAction::Disconnect;
                }
            });
        });
    });

    action
}

/// Notebook cells, drawn as a right side panel. Call only while it is shown.
///
/// Returns the text of a cell whose Run button (or Shift+Enter) was hit; the
/// caller sends it. `can_run` is false while there is no live connection.
pub fn draw_notebook(ui: &mut egui::Ui, cells: &mut Vec<String>, can_run: bool) -> Option<String> {
    let mut to_run = None;

    egui::Panel::right("notebook_panel").default_size(340.0).show(ui, |ui| {
        ui.heading("Notebook");
        ui.label("Run a cell to send it; output appears in the log.");
        ui.separator();

        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut remove = None;
            for (i, cell) in cells.iter_mut().enumerate() {
                let id = ui.make_persistent_id(("notebook_cell", i));
                let focused = ui.memory(|m| m.has_focus(id));
                let shift_enter =
                    focused && ui.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter));

                ui.add(
                    TextEdit::multiline(cell)
                        .id(id)
                        .code_editor()
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("Command (Shift+Enter to run)"),
                );

                ui.horizontal(|ui| {
                    let run_clicked = ui.add_enabled(can_run, egui::Button::new("Run")).clicked();
                    if (run_clicked || (shift_enter && can_run)) && !cell.trim().is_empty() {
                        to_run = Some(cell.clone());
                    }
                    if ui.button("Delete").clicked() {
                        remove = Some(i);
                    }
                });
                ui.separator();
            }
            if let Some(i) = remove {
                cells.remove(i);
            }

            if ui.button("+ Add cell").clicked() {
                cells.push(String::new());
            }
        });
    });

    to_run
}

/// Input bar and scrolling log. `session` is `None` until the first connect.
pub fn draw_terminal(ui: &mut egui::Ui, mut session: Option<&mut Session>) {
    egui::Panel::bottom("input_bar").show(ui, |ui| {
        ui.horizontal(|ui| match session.as_deref_mut() {
            Some(session) => {
                let response = ui.add_enabled(
                    session.connected,
                    TextEdit::singleline(&mut session.input)
                        .desired_width(ui.available_width() - 70.0)
                        .hint_text("Type and press Enter to send"),
                );
                let send_clicked = ui.add_enabled(session.connected, egui::Button::new("Send")).clicked();

                let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if session.connected && (enter_pressed || send_clicked) {
                    session.send_current_input();
                    response.request_focus();
                } else if session.connected && ui.memory(|m| m.focused().is_none()) {
                    response.request_focus();
                }
            }
            None => {
                ui.add_enabled(
                    false,
                    TextEdit::singleline(&mut String::new())
                        .desired_width(ui.available_width() - 70.0)
                        .hint_text("Not connected"),
                );
                ui.add_enabled(false, egui::Button::new("Send"));
            }
        });
    });

    egui::CentralPanel::default().show(ui, |ui| {
        ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            if let Some(session) = session.as_deref() {
                ui.add(egui::Label::new(RichText::new(session.log_text()).monospace()).wrap().selectable(true));
            }
        });
    });

    if session.is_some_and(|s| s.connected) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
    }
}
