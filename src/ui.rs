//! GUI rendering (FR-6, FR-7). Two screens: the connect dialog (FR-2, FR-3)
//! and the live terminal (log + input + status bar).

use eframe::egui::{self, Color32, ComboBox, RichText, ScrollArea, TextEdit};

use crate::app::{AuthMode, ConnMode, ConnectForm, Session};
use crate::connection::{Connection, NewlineMode};

pub enum ConnectAction {
    None,
    Connected(Box<dyn Connection>, NewlineMode),
}

pub enum TerminalAction {
    None,
    Disconnect,
}

pub fn draw_connect(ui: &mut egui::Ui, form: &mut ConnectForm) -> ConnectAction {
    let mut action = ConnectAction::None;

    egui::CentralPanel::default().show(ui, |ui| {
        ui.heading("rust-term-console");
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

        ui.separator();
        draw_profiles(ui, form);

        ui.add_space(8.0);
        if let Some(err) = &form.error {
            ui.colored_label(Color32::from_rgb(220, 60, 60), err);
        }

        if ui.button(RichText::new("Connect").strong()).clicked() {
            let newline = form.newline;
            if let Some(conn) = form.connect() {
                action = ConnectAction::Connected(conn, newline);
            }
        }
    });

    action
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

pub fn draw_terminal(ui: &mut egui::Ui, session: &mut Session) -> TerminalAction {
    let mut action = TerminalAction::None;

    egui::Panel::top("status_bar").show(ui, |ui| {
        ui.horizontal(|ui| {
            let (text, color) = if session.connected {
                ("CONNECTED", Color32::from_rgb(30, 150, 60))
            } else {
                ("DISCONNECTED", Color32::from_rgb(190, 40, 40))
            };
            ui.colored_label(color, RichText::new(text).strong());
            ui.label(session.connection_label().to_string());
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Disconnect").clicked() {
                    action = TerminalAction::Disconnect;
                }
            });
        });
    });

    egui::Panel::bottom("input_bar").show(ui, |ui| {
        ui.horizontal(|ui| {
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
            } else if !response.has_focus() && session.connected {
                response.request_focus();
            }
        });
    });

    egui::CentralPanel::default().show(ui, |ui| {
        ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(session.log_text()).monospace()).wrap().selectable(true));
        });
    });

    if session.connected {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
    }

    action
}
