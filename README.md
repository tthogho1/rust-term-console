# rust-term-console

A Tera Term–like terminal console, in Rust, for **SSH sessions and serial
ports** — one native window, one codebase, Windows/macOS/Linux.

Originally scoped as a TUI per `spec.md`'s FR-6/4.2; rebuilt as a native
windowed GUI (`egui`/`eframe`) instead, matching FR-O5 / Phase 4 of the
roadmap ahead of schedule, so the connect flow looks like Tera Term's own
dialog rather than command-line flags. A transport-agnostic `Connection`
trait keeps the serial and SSH backends sharing one poll loop, in a modular
layout (`app.rs` / `ui.rs` / `serial.rs` / `ssh.rs` / `config.rs` /
`main.rs`).

## Build

```bash
cargo build --release
```

Binary lands at `target/release/rust-term-console` (`.exe` on Windows). Run
it directly — no command-line arguments needed; a window opens with the
connect panel already open. Use the **Connection** button in the header to
show or hide it. Tick **Save session log to file** in the connect panel and
give a path (`~/` is expanded, missing folders are created) to also append
the log to a file; a `● LOG` marker in the header shows it is active. Logging
can also be started or stopped while connected from the header's **Log**
menu (only output from that point on is written).

**View → Notebook** opens a right-hand panel of cells, each either:

- a **Command** cell — press **Run** (or Shift+Enter) and its text is sent
  over the current connection, the same as typing it in the input bar. The
  command and its output appear in the main log, never in the cell.
- a **Note** cell — Markdown documentation for the commands around it
  (headings, lists, bold/italic, code spans, …); it is rendered inline below
  what you type and is never sent anywhere.

Use **+ Command cell** / **+ Note cell** to add either kind, and **Delete**
to remove one. **Save**/**Load** at the top of the panel write/read the
whole cell list as an XML file (path field to the left of the buttons);
loading replaces the current cells.

## Using the app

**Connect screen** (FR-2, FR-3, FR-O4):

- Choose **SSH** or **Serial** with the radio buttons at the top.
- SSH: enter host/port/username, then pick Password, Key file, or ssh-agent
  authentication.
- Serial: pick a port from the dropdown (**Refresh** re-scans — Windows
  `COM1`/`COM2`/...; macOS `/dev/cu.usbserial-*`/`/dev/tty.usbserial-*`;
  Linux `/dev/ttyUSB*`/`/dev/ttyACM*`/`/dev/ttyS*`, may need
  `sudo usermod -aG dialout $USER`), then baud/data bits/parity/stop
  bits/flow control.
- Pick the newline mode appended when you press Enter: CRLF / LF / raw.
- **Profiles**: save the non-secret fields (host/port/username or
  port/baud + newline) under a name and reload them later from the
  dropdown. Passwords/passphrases are never stored — you'll always be
  prompted. Stored at `~/.config/rust-term-console/profiles.toml` (or the
  OS-appropriate config dir).
- **Connect** opens the session; a failed attempt shows the error inline
  and leaves the form filled in so you can fix it and retry (FR-5).

**Terminal screen** (FR-4, FR-6, FR-7, FR-8):

- Status bar: connection state, target, a live newline-mode selector, and
  status/error messages.
- Log area: scrollable, auto-follows new output, monospaced.
- Input bar: type and press Enter (or click Send) to transmit a line; local
  echo shows what you sent even if the remote doesn't echo it back.
  **↑/↓** recall previously sent lines (from the input bar or a notebook
  cell), most recent first; ↓ past the most recent restores whatever you
  had typed before you started recalling.
- **Disconnect** closes the connection cleanly and returns to the connect
  screen; closing the window does the same via the OS.

## Architecture notes

- `connection.rs` defines the `Connection` trait (`read_available`,
  `write_all`, `describe`, `close`) that both transports implement, plus
  `NewlineMode`.
- `serial.rs` wraps `serialport` with a short read timeout so polling never
  blocks the UI thread.
- `ssh.rs` wraps `ssh2`: TCP connect → handshake → password/key/agent auth →
  PTY + shell channel, then switches the session to non-blocking mode for
  the same poll-driven reads.
- `app.rs` holds `ConnectForm` (the dialog's fields and profile
  load/save/connect logic) and `Session` (log buffer capped at 2 MB, input
  line, sent-line history capped at 500 entries, connection status) — both
  transport-agnostic.
- `notebook.rs` defines `Cell`/`CellKind` (Command vs Note) and their
  XML `save`/`load`, independent of the UI.
- `ui.rs` is pure `egui` rendering: `draw_header`, `draw_connect` (a left
  side panel), `draw_notebook` (a right side panel, using `egui_commonmark`
  to render Note cells) and `draw_terminal` take the current state and an
  `egui::Ui`, and report back what the user did (`HeaderAction` /
  `ConnectAction` / `NotebookAction`) rather than mutating app state
  themselves.
- `main.rs` implements `eframe::App`: it owns the form, the optional
  `Session` and the panel's open/closed flag, and acts on those actions
  (opening the connection, disconnecting).
- Connecting is synchronous: a slow DNS lookup or SSH handshake briefly
  freezes the window. Fine for LAN/typical use; a background thread with a
  channel back to the UI is the natural next step for slow/unreliable
  networks.
- Once connected, the terminal requests a repaint every ~30 ms so
  incoming data is picked up even without mouse/keyboard activity — needed
  because native GUI toolkits otherwise only redraw on input events.
- `ansi.rs` strips ANSI/VT100 escape sequences (color, cursor movement,
  OSC title-setting, bare `\r`) from incoming bytes before they hit the
  log, statefully so a sequence split across two reads is still caught.
  Without this, any shell with a themed/colored prompt (the default on
  most systems) would show raw escape-code garbage instead of clean text.

## Known limitations / roadmap

Matches the spec's phased roadmap — not yet implemented:

- Session log formats other than plain text, and per-line timestamps
  (FR-O3 is plain text only).
- Notebook cells run one at a time; no "Run all" and no per-cell output.
- Multiple concurrent connections / tabs (FR-O2).
- Telnet transport (FR-O1 covers SSH only so far).
- Remote PTY resize when the window is resized (SSH channel size is fixed
  at connect time).
- Full-screen interactive programs over the connection (vim, htop, less,
  etc.) won't render correctly — the log is plain text, not a real
  cursor-addressed terminal grid.
- Connecting runs on the UI thread (see above) rather than in the
  background.
