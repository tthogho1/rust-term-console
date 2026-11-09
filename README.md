# rust-term-console

A Tera Term–like terminal console, in Rust, for **SSH, Telnet and serial
port sessions** — one native window, one codebase, Windows/macOS/Linux.

Originally scoped as a TUI per `spec.md`'s FR-6/4.2; rebuilt as a native
windowed GUI (`egui`/`eframe`) instead, matching FR-O5 / Phase 4 of the
roadmap ahead of schedule, so the connect flow looks like Tera Term's own
dialog rather than command-line flags. A transport-agnostic `Connection`
trait keeps the serial, SSH and Telnet backends sharing one poll loop, in
a modular layout (`app.rs` / `ui.rs` / `transport/{serial,ssh,telnet}.rs` /
`config.rs` / `main.rs`).

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

Tick **Timestamp each line** in the connect panel (or **View →
Timestamps** at any time) to prefix each new line with the local time,
e.g. `[14:03:27.512] `, in both the log view and the log file. A line is
stamped when its first byte arrives; turning it on mid-line starts at the
next line.

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

**View → Script** opens a right-hand panel holding a single [Rhai]
script. It never touches the connection directly — running it only builds
up a list of things to send, then hands that list back exactly as if
you'd typed it or run it from a Notebook cell (same local echo, history,
and logging). Inside a script:

- `send(text)` queues a line (or several, one per `\n`) to send.
- `ctrl_c()` / `ctrl_d()` / `ctrl_z()` / `tab()` / `esc()` queue a control
  key, the same ones as the input bar's **Keys** menu.
- `print(x)` / `debug(x)` show up in the panel's **Output** area instead of
  being sent anywhere — useful for working out what a script *would* send
  before wiring up the `send()` calls.

For example:

```rhai
for i in range(0, 3) {
    send("echo " + i);
}
```

**Run** (disabled until connected) executes the whole script, then sends
everything it queued, in order; any error it hit is shown under the path
field, and partial progress up to that point is still sent. **Save**/**Load**
work the same way as the Notebook's, as plain text (`.rhai` by convention).
A runaway script (e.g. an infinite loop) is stopped automatically after a
large but bounded number of operations rather than freezing the window
forever — see "Known limitations" below.

[Rhai]: https://rhai.rs/

**View → Transfer** opens a right-hand panel for sending or fetching a
single file over SCP — Tera Term's "SSH SCP..." menu item, equivalently.
It's SSH-only (there's no SCP over serial or Telnet); the **Transfer**
button is disabled, with a note why, unless the live connection is SSH.
Pick **Upload** or **Download**, fill in the local and remote paths, and
**Transfer**: it reuses the current SSH session as a second channel
(`transport::scp`) rather than opening a new connection, runs on a
background thread so a large file doesn't freeze the window, and shows a
progress bar while it's going. Only one file at a time — no recursive
directory copy (like Tera Term's own basic SCP feature, not `scp -r`).

## Using the app

**Connect screen** (FR-2, FR-3, FR-O4):

- Choose **SSH**, **Telnet**, or **Serial** with the radio buttons at the
  top.
- SSH: enter host/port/username, then pick Password, Key file, or ssh-agent
  authentication.
- Telnet: just host/port (23 by default) — there's no authentication at
  this layer; a login prompt, if any, is plain text sent by the remote,
  same as a real telnet client. The session negotiates nothing and refuses
  every option the server offers (`WILL`) or requests (`DO`), staying in
  plain NVT mode — see "Known limitations" for what that means in
  practice.
- Serial: pick a port from the dropdown (**Refresh** re-scans — Windows
  `COM1`/`COM2`/...; macOS `/dev/cu.usbserial-*`/`/dev/tty.usbserial-*`;
  Linux `/dev/ttyUSB*`/`/dev/ttyACM*`/`/dev/ttyS*`, may need
  `sudo usermod -aG dialout $USER`), then baud/data bits/parity/stop
  bits/flow control.
- Pick the newline mode appended when you press Enter: CRLF / LF / raw.
- **Profiles**: save the non-secret fields (host/port/username, host/port,
  or port/baud — plus newline) under a name and reload them later from the
  dropdown. Passwords/passphrases are never stored — you'll always be
  prompted. Stored at `~/.config/rust-term-console/profiles.toml` (or the
  OS-appropriate config dir).
- **Connect** starts opening the session on a background thread — the
  window stays responsive (you can still edit the form, open other
  panels, etc.) while a spinner and **Cancel** show in its place. A
  failed attempt shows the error inline and leaves the form filled in so
  you can fix it and retry (FR-5); **Cancel** just stops waiting on it —
  see "Known limitations".

**Terminal screen** (FR-4, FR-6, FR-7, FR-8):

- Status bar: connection state, target, a live newline-mode selector, and
  status/error messages.
- Log area: scrollable, auto-follows new output, monospaced, selectable
  plain text with ANSI escape sequences stripped (see `ansi.rs` below).
- **View → Real terminal** swaps the log area for a real VT100 grid
  (`vtgrid.rs`): color, cursor-addressed redraws, and scrollback, so
  full-screen programs (`vim`, `htop`, `less`, a shell with a fancy
  prompt) render the way they would in a real terminal, not just as a
  stream of lines. Scroll it with the mouse wheel; a bar at the top while
  scrolled back shows how far and offers a one-click jump back to the
  live tail (new output keeps arriving underneath without yanking your
  view back to it, like a real terminal). Toggle back to the plain-text
  log any time — this doesn't replace it, since the log area stays what
  the log file, Notebook, and Script actually read from.
- Input bar: type and press Enter (or click Send) to transmit a line; local
  echo shows what you sent even if the remote doesn't echo it back.
  **↑/↓** recall previously sent lines (from the input bar or a notebook
  cell), most recent first; ↓ past the most recent restores whatever you
  had typed before you started recalling.
- **🔒** (next to Send) arms hidden input for the next line only, for a
  password or other secret typed at a remote prompt: the input bar masks
  what you type, and once sent, that line is neither echoed (a `[hidden
  input]` placeholder takes its place in the log and log file) nor
  recorded in history — so pressing ↑ afterward can't bring it back. It
  turns itself back off after that one send.
- **Control keys**: with the input bar focused, **Ctrl+C** (interrupt),
  **Ctrl+D** (end of input), **Ctrl+Z** (suspend) and **Esc** are sent to
  the remote as their raw byte, no newline — so you can stop a running
  `ping`/`tail -f` without disconnecting. **Tab** sends what you've typed
  so far plus a Tab for remote completion, then clears the input bar;
  keep typing the rest of the line and press Enter. The **^C** button and
  **Keys** menu next to Send do the same by mouse. On Windows/Linux,
  Ctrl+C copies instead when text in the input bar is selected (on macOS
  copy is Cmd+C, so Ctrl+C is always an interrupt).
- **Disconnect** closes the connection cleanly and returns to the connect
  screen; closing the window does the same via the OS.

## Architecture notes

- `transport/mod.rs` defines the `Connection` trait (`read_available`,
  `write_all`, `describe`, `close`) that all three transports implement,
  plus `NewlineMode`, `ControlKey` (the raw bytes behind Ctrl+C/D/Z, Tab,
  Esc) and `connect_tcp_with_timeout` (shared by `transport::ssh` and
  `transport::telnet`: it resolves the host and tries
  `TcpStream::connect_timeout` on each address, since plain
  `TcpStream::connect` has no timeout at all — against a host that drops
  packets rather than refusing the connection, that would otherwise mean
  waiting out the OS's own retransmission timeout instead, commonly a
  minute or more).
- Remote terminal size follows the window: each frame the log area's size
  is measured in monospace characters, and once it has held still for
  200 ms `Session` sends it via `Connection::resize` (SSH
  `request_pty_size`; a no-op on serial and Telnet). So `ls`, `top`, long
  prompts etc. wrap at the window's width rather than a fixed 120 columns
  over SSH.
- `transport/serial.rs` wraps `serialport` with a short read timeout so
  polling never blocks the UI thread.
- `transport/ssh.rs` wraps `ssh2`: TCP connect (10s timeout) → handshake →
  password/key/agent auth → PTY + shell channel, then switches the session
  to non-blocking mode for the same poll-driven reads. The handshake/auth
  steps aren't bounded by that timeout — see "Known limitations".
- `transport/telnet.rs` is a plain TCP connection (10s connect timeout,
  same as SSH) plus a small stateful parser for Telnet's `IAC` command
  sequences
  (mirrors `AnsiFilter`'s approach to ANSI escapes: a sequence split across
  two reads is still recognized). It refuses every option the server
  offers (`WILL`) or requests (`DO`) — see "Known limitations" for what
  that means in practice — and discards subnegotiation payloads for
  options it never agreed to in the first place.
- `transport/scp.rs` runs an SCP transfer over `ssh2::Session::scp_send`/
  `scp_recv`. It needs a `Session` handle, which `Connection` exposes via
  `clone_ssh_session` (`None` by default; `SshConnection` returns
  `Some(self.session.clone())` — `ssh2::Session` is `Clone`, an `Arc`
  internally, so this reuses the live connection's already-authenticated
  session rather than opening a second one). This is the one place the
  otherwise transport-agnostic `Connection` trait knows about a specific
  transport's types, a deliberate small exception rather than a general
  capability-downcasting mechanism for one feature. Reads/writes retry on
  `WouldBlock` in small (32 KiB) chunks, the same pattern `SshConnection`
  itself already uses for its interactive channel — both channels share
  one underlying session, so holding it for a whole big file at once
  would stall the interactive terminal too.
- `app.rs` holds `ConnectForm` (the dialog's fields and profile
  load/save/connect logic) and `Session` (log buffer capped at 2 MB, input
  line, sent-line history capped at 500 entries, connection status) — both
  transport-agnostic. `Session::send_hidden` is the one-shot counterpart
  to `send_text` for the input bar's **🔒** toggle: it sends the real
  bytes but logs a `[hidden input]` placeholder instead of the line
  itself and skips `record_history`, so a password typed there never
  ends up in plain text anywhere this app controls — the log, the log
  file, or history.
- `timestamp.rs` holds `LineStamper`, which inserts the time stamp before
  each line's first byte and tracks line position across reads; `Session`
  runs everything it logs through it.
- `notebook.rs` defines `Cell`/`CellKind` (Command vs Note) and their
  XML `save`/`load`, independent of the UI.
- `script.rs` runs a [Rhai] script through a sandboxed `rhai::Engine`
  (operation count capped so a runaway loop can't hang forever) with
  `send`/`ctrl_c`/etc. registered as native functions that push onto a
  shared `Rc<RefCell<Vec<ScriptAction>>>` rather than touching `Session`
  directly — the script only *decides* what to send; `main.rs` sends it
  afterwards through the normal path, so a script can't do anything typing
  by hand couldn't.
- `ui.rs` is pure `egui` rendering: `draw_header`, `draw_connect` (a left
  side panel), `draw_notebook` and `draw_script` (right side panels;
  the former uses `egui_commonmark` to render Note cells) and
  `draw_terminal` take the current state and an `egui::Ui`, and report
  back what the user did (`HeaderAction` / `ConnectAction` /
  `NotebookAction` / `ScriptPanelAction`) rather than mutating app state
  themselves.
- `main.rs` implements `eframe::App`: it owns the form, the optional
  `Session` and the panel's open/closed flag, and acts on those actions
  (opening the connection, disconnecting).
- Connecting runs on a background thread: `ConnectForm::build_target`
  validates the form and builds an owned `ConnectTarget` synchronously (so
  a bad baud rate/port is reported immediately, with the existing session
  left untouched), then `main::GuiApp::connect` hands that to
  `thread::spawn` and polls an `mpsc` channel once per frame
  (`poll_connect`) rather than blocking on it — `Connection: Send` exists
  specifically so a `Box<dyn Connection>` can cross that channel. A slow
  DNS lookup or SSH handshake no longer freezes the window.
- Once connected, the terminal requests a repaint every ~30 ms so
  incoming data is picked up even without mouse/keyboard activity — needed
  because native GUI toolkits otherwise only redraw on input events.
- `ansi.rs` strips ANSI/VT100 escape sequences (color, cursor movement,
  OSC title-setting, bare `\r`) from incoming bytes before they hit the
  log, statefully so a sequence split across two reads is still caught.
  Without this, any shell with a themed/colored prompt (the default on
  most systems) would show raw escape-code garbage instead of clean text.
- `vtgrid.rs` wraps a [`vt100`] parser fed the same incoming bytes
  *before* `ansi_filter` strips them — vt100 needs the escape sequences
  themselves to track a real cell grid (text, color, bold/underline,
  cursor position, 2000 lines of scrollback) the way a terminal emulator
  would, rather than discarding them. `Session` feeds both the plain
  `log` and this grid from the same `poll_connection`, so toggling "View
  → Real terminal" doesn't change what's logged or re-fetch anything.
  `ui::draw_vt_grid` paints it with `egui::Painter` directly (not
  `egui::Label`): it run-length-encodes each row into same-style spans
  before painting, since shell output is usually long runs of identical
  styling and painting cell-by-cell would be one `Painter` call each.
  Mouse-wheel scrolling moves vt100's own scrollback offset (`set_scrollback`)
  rather than wrapping this in an `egui::ScrollArea`, since vt100 already
  reflows the fixed-size grid to show any offset into its history.

[`vt100`]: https://docs.rs/vt100

## Known limitations / roadmap

Matches the spec's phased roadmap — not yet implemented:

- Session log formats other than plain text (FR-O3 is plain text only).
- Notebook cells run one at a time; no "Run all" and no per-cell output.
- Scripts are send-only: `send()`/`ctrl_c()` etc. queue bytes to go out, but
  a script can't read back the connection's output or the log, so it can't
  branch on a prompt or a command's result. Scripts also run synchronously
  on the UI thread (unlike connecting, which now runs in the background),
  so a slow (if bounded) script still briefly freezes the window.
- Multiple concurrent connections / tabs (FR-O2).
- Transfer is one file at a time: no recursive directory copy, no
  queueing a batch of transfers, and no drag-and-drop — fill in the
  panel's two path fields and click Transfer. Disconnecting (or the
  remote closing the connection) while a transfer is running will fail
  it with an error rather than resuming later.
- Telnet always negotiates plain NVT mode, refusing every option (FR-O1 is
  done, but see `transport/telnet.rs`): no character-at-a-time input (the
  input bar sends whole lines either way, so this matches how SSH/serial
  already work here), and no window-size negotiation (`NAWS`), so
  remote-side wrapping to the window's width — which SSH gets via
  `request_pty_size` — doesn't happen over Telnet. There's also no way for
  the remote to tell the app a password prompt is coming (that needs
  `WILL ECHO`, which we refuse): the **🔒** toggle hides the next line you
  type, but nothing arms it for you, so it's on you to remember to click
  it before a password rather than after.
- The plain-text log (the default view) can't render full-screen
  interactive programs (vim, htop, less, etc.) correctly, since it's
  just lines of text, not a real cursor-addressed grid — switch to
  **View → Real terminal** for those. That view has its own gaps:
  bold is a second copy of the text painted a fraction of a pixel over,
  not an actual bold font face (egui only bundles one, regular-weight,
  monospace font by default); italic isn't rendered at all, for the same
  reason; the 16 basic colors are xterm's documented defaults, which
  won't match if the remote has a customized palette (common in e.g.
  Solarized-themed shells) — there's no way to detect that from the SGR
  color-index codes alone; and double-width (e.g. CJK) characters are
  handled only approximately.
- **Cancel** on a connect attempt only stops *waiting* on it in the UI;
  the background thread isn't killed. The initial TCP connect is bounded
  (`connect_tcp_with_timeout`, used by both SSH and Telnet), but the SSH
  handshake/auth and the serial port open that can follow are not, so a
  cancelled attempt can still run a while longer in the background
  (harmlessly — its result is just discarded).
