//! GUI rendering (FR-6, FR-7). The window is always the terminal (header +
//! log + input); the connect form (FR-2, FR-3) is a left side panel toggled
//! by the header's "Connection" button, and the notebook cells are a right
//! side panel toggled from the header's "View" menu. Command cells send
//! their text over the connection; output lands in the shared log, not the
//! cell. Note cells are never sent — they hold Markdown documentation and
//! are rendered inline. Both are saved/loaded together as an XML file.

use eframe::egui::{self, Color32, ComboBox, RichText, ScrollArea, TextEdit};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::app::{AuthMode, ConnMode, ConnectForm, Session};
use crate::transport::{ControlKey, NewlineMode};
use crate::notebook::{Cell, CellKind};

pub enum ConnectAction {
    None,
    /// The Connect button was pressed; the caller starts connecting.
    Connect,
    /// Cancel was pressed while connecting; the caller drops the attempt
    /// (it keeps running to completion in the background, but its result
    /// is ignored).
    Cancel,
}

pub enum HeaderAction {
    None,
    Disconnect,
    /// Start logging to the path in the header's Log menu.
    StartLog,
    StopLog,
}

/// Connect form, drawn as a left side panel. Call only while it is shown.
/// `connecting` is true while a background attempt (started by a previous
/// `Connect`) hasn't resolved yet.
pub fn draw_connect(ui: &mut egui::Ui, form: &mut ConnectForm, connecting: bool) -> ConnectAction {
    let mut action = ConnectAction::None;

    egui::Panel::left("connect_panel").default_size(320.0).show(ui, |ui| {
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            draw_connect_form(ui, form, connecting, &mut action);
        });
    });

    action
}

fn draw_connect_form(ui: &mut egui::Ui, form: &mut ConnectForm, connecting: bool, action: &mut ConnectAction) {
    ui.heading("Connection");
    ui.label("Connect over SSH, Telnet, or a serial port.");
    ui.separator();

    ui.horizontal(|ui| {
        ui.radio_value(&mut form.mode, ConnMode::Ssh, "SSH");
        ui.radio_value(&mut form.mode, ConnMode::Telnet, "Telnet");
        ui.radio_value(&mut form.mode, ConnMode::Serial, "Serial");
    });
    ui.add_space(8.0);

    match form.mode {
        ConnMode::Ssh => draw_ssh_fields(ui, form),
        ConnMode::Telnet => draw_telnet_fields(ui, form),
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
    ui.checkbox(&mut form.timestamps, "Timestamp each line");
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

    if connecting {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Connecting…");
            if ui.button("Cancel").clicked() {
                *action = ConnectAction::Cancel;
            }
        });
    } else if ui.button(RichText::new("Connect").strong()).clicked() {
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

fn draw_telnet_fields(ui: &mut egui::Ui, form: &mut ConnectForm) {
    ui.label(
        RichText::new("No authentication at this layer — any login prompt is plain text from the remote.")
            .weak(),
    );
    egui::Grid::new("telnet_grid").num_columns(2).show(ui, |ui| {
        ui.label("Host");
        ui.text_edit_singleline(&mut form.host);
        ui.end_row();

        ui.label("Port");
        ui.text_edit_singleline(&mut form.telnet_port);
        ui.end_row();
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
// The View menu's checkboxes have grown to tip this over clippy's default
// limit; `show_vt_grid` was the one that pushed it from 7 to 8. Grouping
// these into a settings struct would be the natural fix if another one
// gets added.
#[allow(clippy::too_many_arguments)]
pub fn draw_header(
    ui: &mut egui::Ui,
    session: Option<&mut Session>,
    show_connect: &mut bool,
    show_notebook: &mut bool,
    show_script: &mut bool,
    show_vt_grid: &mut bool,
    show_transfer: &mut bool,
    timestamps: &mut bool,
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
                ui.checkbox(show_script, "Script")
                    .on_hover_text("A Rhai script that builds up commands to send");
                ui.checkbox(show_transfer, "Transfer")
                    .on_hover_text("Send or fetch a file over SCP (SSH connections only)");
                ui.checkbox(timestamps, "Timestamps")
                    .on_hover_text("Prefix each new line with the local time, in the view and the log file");
                ui.separator();
                ui.checkbox(show_vt_grid, "Real terminal").on_hover_text(
                    "Render a real VT100 grid (color, cursor addressing, scrollback) instead \
                     of the plain-text log, so full-screen programs like vim/htop/less work. \
                     Scroll with the mouse wheel over it. See README \u{2018}Known limitations\u{2019}.",
                );
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

pub enum NotebookAction {
    None,
    /// A command cell's Run button (or Shift+Enter) was hit; the caller
    /// sends this text over the connection.
    Run(String),
    /// Write `cells` to the path field as XML.
    Save,
    /// Replace `cells` with what the path field's XML file holds.
    Load,
}

/// Notebook cells, drawn as a right side panel. Call only while it is shown.
/// `can_run` is false while there is no live connection.
pub fn draw_notebook(
    ui: &mut egui::Ui,
    cells: &mut Vec<Cell>,
    file_path: &mut String,
    error: Option<&str>,
    md_cache: &mut CommonMarkCache,
    can_run: bool,
) -> NotebookAction {
    let mut action = NotebookAction::None;

    egui::Panel::right("notebook_panel").default_size(360.0).show(ui, |ui| {
        ui.heading("Notebook");
        ui.label("Command cells send their text; note cells document and are never sent.");
        ui.separator();

        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(file_path)
                    .desired_width(ui.available_width() - 110.0)
                    .hint_text("notebook.xml"),
            );
            if ui.button("Save").clicked() {
                action = NotebookAction::Save;
            }
            if ui.button("Load").clicked() {
                action = NotebookAction::Load;
            }
        });
        if let Some(err) = error {
            ui.colored_label(Color32::from_rgb(220, 60, 60), err);
        }
        ui.separator();

        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut remove = None;
            for (i, cell) in cells.iter_mut().enumerate() {
                let id = ui.make_persistent_id(("notebook_cell", i));

                ui.horizontal(|ui| {
                    ui.radio_value(&mut cell.kind, CellKind::Command, "Command");
                    ui.radio_value(&mut cell.kind, CellKind::Note, "Note");
                });

                let focused = ui.memory(|m| m.has_focus(id));
                let shift_enter = focused
                    && cell.kind == CellKind::Command
                    && ui.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter));

                let hint = match cell.kind {
                    CellKind::Command => "Command (Shift+Enter to run)",
                    CellKind::Note => "Note (Markdown, never sent)",
                };
                ui.add(
                    TextEdit::multiline(&mut cell.text)
                        .id(id)
                        .code_editor()
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text(hint),
                );

                if cell.kind == CellKind::Note && !cell.text.trim().is_empty() {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        CommonMarkViewer::new().show(ui, md_cache, &cell.text);
                    });
                }

                ui.horizontal(|ui| {
                    if cell.kind == CellKind::Command {
                        let run_clicked = ui.add_enabled(can_run, egui::Button::new("Run")).clicked();
                        if (run_clicked || (shift_enter && can_run)) && !cell.text.trim().is_empty() {
                            action = NotebookAction::Run(cell.text.clone());
                        }
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

            ui.horizontal(|ui| {
                if ui.button("+ Command cell").clicked() {
                    cells.push(Cell::command());
                }
                if ui.button("+ Note cell").clicked() {
                    cells.push(Cell { kind: CellKind::Note, text: String::new() });
                }
            });
        });
    });

    action
}

/// Pull a Ctrl+C/D/Z press for the focused input bar out of this frame's
/// events, so the text box never sees it (Ctrl+Z would otherwise be undo on
/// Windows/Linux). Windows/Linux deliver Ctrl+C as a Copy event instead of a
/// key press; it counts as an interrupt only when nothing is selected, so
/// copying selected text still works. On macOS copy is Cmd+C and Ctrl+C
/// arrives as a plain key press.
fn take_control_shortcut(ui: &mut egui::Ui, input_id: egui::Id) -> Option<ControlKey> {
    let has_selection = TextEdit::load_state(ui.ctx(), input_id)
        .and_then(|state| state.cursor.char_range())
        .is_some_and(|range| !range.is_empty());

    ui.input_mut(|input| {
        let mut found = None;
        input.events.retain(|event| {
            let key = match event {
                egui::Event::Key { key, pressed: true, modifiers, .. }
                    if modifiers.ctrl && !modifiers.shift && !modifiers.alt && !modifiers.mac_cmd =>
                {
                    match key {
                        egui::Key::C => ControlKey::CtrlC,
                        egui::Key::D => ControlKey::CtrlD,
                        egui::Key::Z => ControlKey::CtrlZ,
                        _ => return true,
                    }
                }
                egui::Event::Copy if cfg!(not(target_os = "macos")) && !has_selection => ControlKey::CtrlC,
                _ => return true,
            };
            found = Some(key);
            false
        });
        found
    })
}

/// How many monospace characters fit across and down the log area, leaving
/// room for the vertical scroll bar.
fn log_area_size_in_chars(ui: &egui::Ui) -> (u32, u32) {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let (char_width, row_height) =
        ui.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
    let scroll = &ui.style().spacing.scroll;
    let width = ui.available_width() - scroll.bar_width - scroll.bar_outer_margin - scroll.bar_inner_margin;
    let height = ui.available_height();
    let cols = (width / char_width.max(1.0)).floor().max(1.0) as u32;
    let rows = (height / row_height.max(1.0)).floor().max(1.0) as u32;
    (cols, rows)
}

pub enum ScriptPanelAction {
    None,
    /// Run button pressed; the caller runs the script and sends whatever it
    /// asked to send.
    Run,
    /// Write `source` to the path field as plain text.
    Save,
    /// Replace `source` with what the path field's file holds.
    Load,
}

/// A Rhai script, drawn as a right side panel. Call only while it is shown.
/// The script itself never touches the connection — running it only
/// *collects* what to send (see `script.rs`); `can_run` is false while
/// there is no live connection to send it over.
pub fn draw_script(
    ui: &mut egui::Ui,
    source: &mut String,
    file_path: &mut String,
    output: &str,
    error: Option<&str>,
    can_run: bool,
) -> ScriptPanelAction {
    let mut action = ScriptPanelAction::None;

    egui::Panel::right("script_panel").default_size(380.0).show(ui, |ui| {
        ui.heading("Script");
        ui.label(
            "Rhai. send(text) queues a line to send; ctrl_c()/ctrl_d()/ctrl_z()/tab()/esc() queue a \
             control key; print()/debug() show up below instead of being sent.",
        );
        ui.separator();

        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(file_path)
                    .desired_width(ui.available_width() - 110.0)
                    .hint_text("script.rhai"),
            );
            if ui.button("Save").clicked() {
                action = ScriptPanelAction::Save;
            }
            if ui.button("Load").clicked() {
                action = ScriptPanelAction::Load;
            }
        });
        if let Some(err) = error {
            ui.colored_label(Color32::from_rgb(220, 60, 60), err);
        }
        ui.separator();

        ScrollArea::vertical()
            .id_salt("script_source")
            .max_height(ui.available_height() * 0.55)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    TextEdit::multiline(source)
                        .code_editor()
                        .desired_rows(10)
                        .desired_width(f32::INFINITY)
                        .hint_text("for i in range(0, 3) {\n    send(\"echo \" + i);\n}"),
                );
            });

        ui.horizontal(|ui| {
            if ui.add_enabled(can_run, egui::Button::new("Run")).clicked() {
                action = ScriptPanelAction::Run;
            }
            if !can_run {
                ui.label(RichText::new("Connect first to run a script").weak());
            }
        });
        ui.separator();

        ui.label(RichText::new("Output").strong());
        ScrollArea::vertical().id_salt("script_output").auto_shrink([false, false]).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(output).monospace()).wrap().selectable(true));
        });
    });

    action
}

pub enum TransferPanelAction {
    None,
    /// Transfer button pressed; the caller starts it on a background
    /// thread (see `transport::scp`).
    Start,
}

/// SCP file transfer, drawn as a right side panel. Call only while it is
/// shown. `can_transfer` is false unless the live connection is SSH (SCP
/// has no equivalent over serial or Telnet) — see `Session::ssh_session`.
/// `progress` is `Some((label, transferred, total))` while a transfer
/// from a previous `Start` is running; the path/direction fields and
/// Transfer button are replaced by a progress bar until it finishes.
pub fn draw_transfer(
    ui: &mut egui::Ui,
    local_path: &mut String,
    remote_path: &mut String,
    upload: &mut bool,
    progress: Option<(&str, u64, u64)>,
    error: Option<&str>,
    can_transfer: bool,
) -> TransferPanelAction {
    let mut action = TransferPanelAction::None;

    egui::Panel::right("transfer_panel").default_size(340.0).show(ui, |ui| {
        ui.heading("Transfer");
        ui.label(
            "Send or fetch a single file over SCP, reusing the current SSH connection \
             rather than opening a new one — the same thing Tera Term's SSH SCP does.",
        );
        ui.separator();

        let transferring = progress.is_some();
        ui.add_enabled_ui(!transferring, |ui| {
            ui.horizontal(|ui| {
                ui.radio_value(upload, true, "Upload");
                ui.radio_value(upload, false, "Download");
            });
            ui.add_space(4.0);

            egui::Grid::new("transfer_grid").num_columns(2).show(ui, |ui| {
                ui.label("Local path");
                ui.text_edit_singleline(local_path);
                ui.end_row();

                ui.label("Remote path");
                ui.text_edit_singleline(remote_path);
                ui.end_row();
            });
        });

        ui.add_space(8.0);
        if let Some(err) = error {
            ui.colored_label(Color32::from_rgb(220, 60, 60), err);
        }

        if let Some((label, transferred, total)) = progress {
            ui.label(label);
            let fraction = if total > 0 { transferred as f32 / total as f32 } else { 0.0 };
            ui.add(egui::ProgressBar::new(fraction).text(format!("{transferred} / {total} bytes")));
        } else {
            ui.horizontal(|ui| {
                if ui.add_enabled(can_transfer, egui::Button::new("Transfer")).clicked() {
                    action = TransferPanelAction::Start;
                }
                if !can_transfer {
                    ui.label(RichText::new("Needs an SSH connection").weak());
                }
            });
        }
    });

    action
}

/// Input bar and scrolling log. `session` is `None` until the first connect.
/// "View → Real terminal" (see `vtgrid.rs`) paints the vt100 screen's
/// fixed grid of cells directly — color, cursor position, scrollback, and
/// all — instead of the plain-text log. This is the piece that a plain
/// `egui::Label` fundamentally can't do, because full-screen programs
/// (vim, htop, less) rely on jumping the cursor to an absolute position
/// and overwriting what's there, not on text just flowing downward.
fn draw_vt_grid(ui: &mut egui::Ui, session: &mut Session) {
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let (char_width, row_height) =
        ui.fonts_mut(|f| (f.glyph_width(&font_id, 'M'), f.row_height(&font_id)));
    let default_fg = ui.style().visuals.text_color();

    let (rows, cols) = session.vt_screen().size();
    let desired_size = egui::vec2(char_width * f32::from(cols.get()), row_height * f32::from(rows.get()));
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

    // The mouse wheel scrolls vt100's own scrollback offset directly,
    // rather than wrapping this in an `egui::ScrollArea`: vt100 already
    // tracks history and reflows the fixed-size grid to show any offset
    // into it (see `vtgrid.rs::TerminalGrid::process`), so moving that
    // offset is the whole job.
    if response.hovered() {
        let rows_scrolled = ui.input(|i| i.smooth_scroll_delta.y) / row_height;
        if rows_scrolled.abs() >= 0.5 {
            session.vt_scroll_by(-rows_scrolled.round() as i64);
        }
    }

    let screen = session.vt_screen();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.style().visuals.extreme_bg_color);

    for row in 0..rows.get() {
        let y = rect.top() + f32::from(row) * row_height;
        paint_vt_row(&painter, screen, row, cols.get(), rect.left(), y, char_width, row_height, &font_id, default_fg);
    }

    // The cursor only reflects where the remote is actually typing, which
    // is meaningless (and would be actively misleading) while looking at
    // scrollback history rather than the live tail.
    let scrolled_back = screen.scrollback() > 0;
    if !scrolled_back && !screen.hide_cursor() {
        paint_vt_cursor(&painter, screen, rect.left(), rect.top(), char_width, row_height);
    }

    if scrolled_back {
        let label = format!(
            "\u{23f8} scrolled back {}/{} lines \u{2014} click to jump to the live tail",
            screen.scrollback(),
            screen.scrollback_len()
        );
        let indicator_rect =
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(rect.width(), row_height.max(18.0)));
        if ui.put(indicator_rect, egui::Button::new(RichText::new(label).small())).clicked() {
            session.vt_jump_to_live_tail();
        }
    }
}

/// A cell's paintable attributes with inverse video already resolved into
/// actual (fg, bg) — cells with identical style in a row are merged into
/// one `painter.text`/`rect_filled` call pair rather than one per
/// character, since shell output is usually long runs of the same
/// default styling.
#[derive(PartialEq, Clone, Copy)]
struct VtCellStyle {
    fg: egui::Color32,
    bg: egui::Color32,
    bold: bool,
    underline: bool,
}

#[allow(clippy::too_many_arguments)]
fn paint_vt_row(
    painter: &egui::Painter,
    screen: &vt100::Screen,
    row: u16,
    cols: u16,
    left: f32,
    y: f32,
    char_width: f32,
    row_height: f32,
    font_id: &egui::FontId,
    default_fg: egui::Color32,
) {
    let mut run_start = 0u16;
    let mut run_style: Option<VtCellStyle> = None;
    let mut run_text = String::new();

    let mut col = 0u16;
    while col < cols {
        let Some(cell) = screen.cell(row, col) else { break };
        // The second half of a double-width (e.g. CJK) character; the
        // glyph was already painted as part of the first half's cell, in
        // a font where it should already render at double width.
        if cell.is_wide_continuation() {
            col += 1;
            continue;
        }

        let (mut fg, mut bg) = (
            vt100_color_to_egui(cell.fgcolor(), default_fg),
            vt100_color_to_egui(cell.bgcolor(), egui::Color32::TRANSPARENT),
        );
        if cell.inverse() {
            std::mem::swap(&mut fg, &mut bg);
        }
        let style = VtCellStyle { fg, bg, bold: cell.bold(), underline: cell.underline() };
        let text = if cell.contents().is_empty() { " " } else { cell.contents() };

        match run_style {
            Some(s) if s == style => run_text.push_str(text),
            Some(s) => {
                paint_vt_run(painter, left, y, run_start, col, char_width, row_height, font_id, s, &run_text);
                run_style = Some(style);
                run_text.clear();
                run_text.push_str(text);
                run_start = col;
            }
            None => {
                run_style = Some(style);
                run_text.push_str(text);
                run_start = col;
            }
        }
        col += 1;
    }
    if let Some(s) = run_style {
        paint_vt_run(painter, left, y, run_start, cols, char_width, row_height, font_id, s, &run_text);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_vt_run(
    painter: &egui::Painter,
    left: f32,
    y: f32,
    start_col: u16,
    end_col: u16,
    char_width: f32,
    row_height: f32,
    font_id: &egui::FontId,
    style: VtCellStyle,
    text: &str,
) {
    if text.is_empty() {
        return;
    }
    let x = left + f32::from(start_col) * char_width;
    let width = f32::from(end_col - start_col) * char_width;
    let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, row_height));

    if style.bg != egui::Color32::TRANSPARENT {
        painter.rect_filled(rect, 0.0, style.bg);
    }
    if !text.trim().is_empty() {
        painter.text(rect.left_top(), egui::Align2::LEFT_TOP, text, font_id.clone(), style.fg);
        if style.bold {
            // egui only bundles one (regular-weight) monospace font by
            // default, so there's no real bold face to switch to;
            // approximate it with a second copy offset by a fraction of
            // a pixel rather than e.g. brightening the color, which
            // wouldn't make sense for an arbitrary RGB color.
            painter.text(rect.left_top() + egui::vec2(0.4, 0.0), egui::Align2::LEFT_TOP, text, font_id.clone(), style.fg);
        }
    }
    if style.underline {
        painter.line_segment([rect.left_bottom(), rect.right_bottom()], egui::Stroke::new(1.0, style.fg));
    }
}

fn paint_vt_cursor(
    painter: &egui::Painter,
    screen: &vt100::Screen,
    left: f32,
    top: f32,
    char_width: f32,
    row_height: f32,
) {
    let (cur_row, cur_col) = screen.cursor_position();
    let x = left + f32::from(cur_col) * char_width;
    let y = top + f32::from(cur_row) * row_height;
    let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(char_width, row_height));
    painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.5, egui::Color32::YELLOW), egui::StrokeKind::Outside);
}

fn vt100_color_to_egui(color: vt100::Color, default: egui::Color32) -> egui::Color32 {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => ansi_256_color(i),
        vt100::Color::Rgb(r, g, b) => egui::Color32::from_rgb(r, g, b),
    }
}

/// The standard xterm 256-color palette: entries 0-15 are xterm's documented
/// default RGB values for the basic/bright ANSI colors (a terminal with a
/// customized palette, which many users set up, won't exactly match —
/// there's no way to know that from the SGR color-index codes alone);
/// 16-231 are the 6×6×6 color cube and 232-255 the grayscale ramp, both
/// exact per the xterm spec.
fn ansi_256_color(i: u8) -> egui::Color32 {
    const BASIC: [egui::Color32; 16] = [
        egui::Color32::from_rgb(0x00, 0x00, 0x00),
        egui::Color32::from_rgb(0xCD, 0x00, 0x00),
        egui::Color32::from_rgb(0x00, 0xCD, 0x00),
        egui::Color32::from_rgb(0xCD, 0xCD, 0x00),
        egui::Color32::from_rgb(0x00, 0x00, 0xEE),
        egui::Color32::from_rgb(0xCD, 0x00, 0xCD),
        egui::Color32::from_rgb(0x00, 0xCD, 0xCD),
        egui::Color32::from_rgb(0xE5, 0xE5, 0xE5),
        egui::Color32::from_rgb(0x7F, 0x7F, 0x7F),
        egui::Color32::from_rgb(0xFF, 0x00, 0x00),
        egui::Color32::from_rgb(0x00, 0xFF, 0x00),
        egui::Color32::from_rgb(0xFF, 0xFF, 0x00),
        egui::Color32::from_rgb(0x5C, 0x5C, 0xFF),
        egui::Color32::from_rgb(0xFF, 0x00, 0xFF),
        egui::Color32::from_rgb(0x00, 0xFF, 0xFF),
        egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
    ];
    match i {
        0..=15 => BASIC[i as usize],
        232..=255 => {
            let level = 8 + (i - 232) * 10;
            egui::Color32::from_gray(level)
        }
        _ => {
            let i = i - 16;
            let scale = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            egui::Color32::from_rgb(scale(i / 36), scale((i / 6) % 6), scale(i % 6))
        }
    }
}

pub fn draw_terminal(ui: &mut egui::Ui, mut session: Option<&mut Session>, show_vt_grid: bool) {
    egui::Panel::bottom("input_bar").show(ui, |ui| {
        ui.horizontal(|ui| match session.as_deref_mut() {
            Some(session) => {
                let input_id = ui.make_persistent_id("terminal_input");
                let mut control = None;
                if session.connected && ui.memory(|m| m.has_focus(input_id)) {
                    control = take_control_shortcut(ui, input_id);
                }

                let hint = if session.hide_input {
                    "Hidden: sent but not shown or recorded (\u{2191}/\u{2193} for history)"
                } else {
                    "Type and press Enter to send (\u{2191}/\u{2193} for history)"
                };
                let response = ui.add_enabled(
                    session.connected,
                    TextEdit::singleline(&mut session.input)
                        .id(input_id)
                        .password(session.hide_input)
                        // Keep focus on Tab so it can be sent for completion.
                        .lock_focus(true)
                        .desired_width(ui.available_width() - 150.0)
                        .hint_text(hint),
                );
                let send_clicked = ui.add_enabled(session.connected, egui::Button::new("Send")).clicked();
                ui.add_enabled_ui(session.connected, |ui| {
                    ui.toggle_value(&mut session.hide_input, "\u{1f512}").on_hover_text(
                        "Hide the next line sent (e.g. a password): masked while typing, \
                         and not shown or recorded once sent",
                    );
                });
                if ui
                    .add_enabled(session.connected, egui::Button::new("^C"))
                    .on_hover_text(ControlKey::CtrlC.label())
                    .clicked()
                {
                    control = Some(ControlKey::CtrlC);
                }
                ui.add_enabled_ui(session.connected, |ui| {
                    ui.menu_button("Keys", |ui| {
                        for key in ControlKey::ALL {
                            if ui.button(key.label()).clicked() {
                                control = Some(key);
                                ui.close();
                            }
                        }
                    });
                });

                // egui drops focus on Esc before the widget runs, so (like
                // Enter) it shows up as the input bar losing focus.
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    control = Some(ControlKey::Esc);
                } else if response.has_focus()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab))
                {
                    control = Some(ControlKey::Tab);
                }

                let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if let Some(key) = control.filter(|_| session.connected) {
                    session.send_control(key);
                    response.request_focus();
                } else if session.connected && (enter_pressed || send_clicked) {
                    if session.hide_input {
                        session.send_current_input_hidden();
                    } else {
                        session.send_current_input();
                    }
                    response.request_focus();
                } else if session.connected && ui.memory(|m| m.focused().is_none()) {
                    response.request_focus();
                } else if session.connected && response.has_focus() {
                    let (up, down) = ui.input(|i| {
                        (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown))
                    });
                    if up || down {
                        if up {
                            session.history_prev();
                        } else {
                            session.history_next();
                        }
                        // Recalled text replaces the line; put the cursor at
                        // its end rather than wherever it happened to be.
                        if let Some(mut state) = TextEdit::load_state(ui.ctx(), response.id) {
                            let end = egui::text::CCursor::new(session.input.chars().count());
                            state.cursor.set_char_range(Some(egui::text_selection::CCursorRange::one(end)));
                            TextEdit::store_state(ui.ctx(), response.id, state);
                        }
                    }
                }
            }
            None => {
                ui.add_enabled(
                    false,
                    TextEdit::singleline(&mut String::new())
                        .desired_width(ui.available_width() - 150.0)
                        .hint_text("Not connected"),
                );
                ui.add_enabled(false, egui::Button::new("Send"));
                ui.add_enabled_ui(false, |ui| {
                    ui.toggle_value(&mut false, "\u{1f512}");
                });
                ui.add_enabled(false, egui::Button::new("^C"));
                ui.add_enabled(false, egui::Button::new("Keys"));
            }
        });
    });

    egui::CentralPanel::default().show(ui, |ui| {
        if let Some(session) = session.as_deref_mut()
            && session.connected
        {
            let (cols, rows) = log_area_size_in_chars(ui);
            session.request_resize(cols, rows, std::time::Instant::now());
            // Purely local, so no debounce needed (unlike telling the
            // *remote* the new size via `flush_resize`) — kept in sync
            // even while this view isn't the one being shown, so toggling
            // "Real terminal" on mid-session doesn't show a stale size
            // for one frame.
            session.vt_resize(cols, rows);
        }
        if show_vt_grid {
            // Not wrapped in an `egui::ScrollArea`: it handles its own
            // scrolling (mouse wheel over it moves vt100's scrollback
            // offset — see `draw_vt_grid`), since it's a fixed-size grid
            // rather than content that keeps growing downward.
            if let Some(session) = session.as_deref_mut() {
                draw_vt_grid(ui, session);
            }
        } else {
            ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                if let Some(session) = session.as_deref() {
                    ui.add(egui::Label::new(RichText::new(session.log_text()).monospace()).wrap().selectable(true));
                }
            });
        }
    });

    if session.is_some_and(|s| s.connected) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
    }
}
