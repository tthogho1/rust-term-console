## Specification: Cross‑Platform Terminal Console (Rust)

**Working title:** `rust-term-console`
**Goal:** Build a Tera Term–like terminal console application in Rust that runs on Windows, macOS, and Linux, supporting serial connections initially, with optional SSH/telnet support later.

***

## 1. Scope and Objectives

### 1.1 Scope

The application shall provide:

- A text-based terminal console UI running inside the native terminal (TUI).
- Serial port communication (connect, send, receive, configure).
- Cross‑platform support for:
  - Windows 10/11
  - macOS (recent versions, Intel and Apple Silicon)
  - Linux (major distributions, including WSL2 for Linux environments on Windows)

Future scope (optional phases):

- SSH and telnet sessions.
- Multiple simultaneous connections (tabs or split panes).
- Session logging to file.
- GUI front-end (windowed app) using a Rust GUI toolkit.

### 1.2 Objectives

- Provide a single Rust codebase that builds and runs on all three target OSes.
- Offer core functionality comparable to Tera Term's serial terminal mode.
- Ensure a consistent user experience across platforms while respecting OS conventions (e.g., device naming, keybindings).

***

## 2. Functional Requirements

### 2.1 Connection Management

**FR‑1: Serial Port Enumeration**
The application shall enumerate available serial ports on the host system and present them to the user.

- Windows: ports identified as `COM1`, `COM2`, etc.
- macOS: ports such as `/dev/cu.usbserial-*` or `/dev/tty.usbserial-*`
- Linux: ports such as `/dev/ttyUSB*`, `/dev/ttyACM*`, `/dev/ttyS*`

**FR‑2: Serial Port Configuration**
The application shall allow the user to configure:

- Port name (selected from enumerated list or typed)
- Baud rate (e.g., 9600, 19200, 115200, etc.)
- Data bits, parity, stop bits (as supported by the underlying library)
- Flow control (RTS/CTS, XON/XOFF, or none, as supported)

**FR‑3: Connect / Disconnect**
The application shall allow the user to:

- Open a connection to a selected serial port with specified parameters.
- Close an open connection cleanly.
- Show connection status (connected/disconnected, errors).

**FR‑4: Data Transmission**
While connected, the application shall:

- Display received data from the serial port in a scrollable log area.
- Allow the user to type text and send it to the serial port.
- Optionally append a newline (`\n` or `\r\n`) when sending, configurable by the user.

**FR‑5: Error Handling**
The application shall:

- Detect and display serial communication errors (e.g., port unavailable, permission denied, device disconnected).
- Allow the user to retry or change settings after an error.

### 2.2 User Interface (TUI)

**FR‑6: Main Layout**
The TUI shall include at minimum:

- A **log area** showing received characters/lines from the serial port.
- An **input area** for typing data to send.
- A **status bar** showing:
  - Current connection state
  - Port name and baud rate
  - Basic hints (e.g., "Ctrl+C to quit", "Enter to send")

**FR‑7: Navigation and Input**
The application shall support:

- Keyboard navigation within the UI (e.g., moving focus between input and other controls if extended).
- Basic line editing in the input area (backspace, cursor movement if feasible).
- Sending text on Enter key press.

**FR‑8: Exit**
The application shall provide a clear way to exit (e.g., Ctrl+C or a menu/shortcut), ensuring:

- Serial port is closed cleanly.
- Terminal is restored to normal mode (raw mode disabled, alternate screen exited).

### 2.3 Optional Future Features (Phased)

These are out of scope for the initial version but should be considered in the architecture:

- **FR‑O1: SSH/Telnet Sessions** – Open TCP-based terminal sessions with basic authentication.
- **FR‑O2: Multiple Connections** – Support multiple concurrent connections via tabs or split panes.
- **FR‑O3: Session Logging** – Save received data to a file (with configurable path and format).
- **FR‑O4: Profiles** – Save and load connection profiles (port, baud, etc.).
- **FR‑O5: GUI Front-End** – Provide a windowed application variant with menus, dialogs, and similar UX to Tera Term.

***

## 3. Non‑Functional Requirements

### 3.1 Portability

**NFR‑1: Cross‑Platform Build**
The application shall build and run on:

- Windows 10/11 (x86_64)
- macOS (x86_64 and arm64)
- Linux (x86_64; arm64 desirable)

using a single Rust codebase with conditional compilation only where necessary.

**NFR‑2: Terminal Compatibility**
The TUI shall function correctly in common terminals:

- Windows: Windows Terminal, PowerShell console, CMD (modern Windows 10/11)
- macOS: Terminal.app, iTerm2
- Linux: gnome-terminal, kitty, alacritty, etc.

### 3.2 Performance

**NFR‑3: Responsiveness**
The UI shall remain responsive during normal serial communication:

- No noticeable freezing while receiving data at typical baud rates (up to at least 921600).
- Input latency low enough for interactive use.

**NFR‑4: Resource Usage**
The application shall be lightweight:

- Low memory footprint suitable for long-running sessions.
- Minimal CPU usage when idle.

### 3.3 Reliability and Safety

**NFR‑5: Clean Resource Management**
The application shall:

- Always close serial ports on exit or disconnect.
- Restore terminal state even after errors or abnormal exit (as far as practicable).

**NFR‑6: Error Resilience**
The application shall handle:

- Unexpected device disconnection.
- Invalid configuration parameters.
- Permission issues (e.g., Linux user not in `dialout` group) with clear error messages.

### 3.4 Maintainability

**NFR‑7: Modular Architecture**
The codebase shall be organized into logical modules:

- `serial.rs` – serial port abstraction
- `ui.rs` – TUI rendering and input handling
- `app.rs` – application state and logic
- `config.rs` – configuration and profiles (for future use)
- `main.rs` – entry point and platform-specific initialization

**NFR‑8: Documentation**
The project shall include:

- A README with build and usage instructions for all platforms.
- Inline code comments for non-trivial logic.
- Basic developer notes on architecture and extension points.

***

## 4. Technical Architecture

### 4.1 Language and Tooling

- **Language:** Rust (edition 2021 or later)
- **Build Tool:** Cargo
- **Async Runtime (optional):** Tokio (if using `tokio-serial` or async networking later)

### 4.2 Key Crates

- **Serial Communication:**
  - `serialport` (blocking) or `tokio-serial` (async) – cross‑platform serial I/O and enumeration.
- **TUI:**
  - `ratatui` – high-level TUI widgets and layout.
  - `crossterm` – terminal backend (raw mode, events, colors).
- **Configuration (future):**
  - `serde` + `toml` (or `serde_json`) for profile storage.

### 4.3 High-Level Component Diagram

- **`main.rs`**
  - Initializes terminal (enable raw mode, alternate screen).
  - Creates `Terminal<CrosstermBackend>`.
  - Instantiates `App` state.
  - Runs main loop:
    - Polls events (keyboard, resize).
    - Reads from serial port (blocking or async).
    - Updates `App` state.
    - Renders UI via `ratatui`.

- **`app.rs`**
  - Holds:
    - Current connection state (connected/disconnected).
    - Selected port and configuration.
    - Log buffer (ring buffer or Vec with max size).
    - Input buffer.
  - Provides methods:
    - `connect(port, config)`
    - `disconnect()`
    - `send_line(&str)`
    - `append_log(&str)`

- **`serial.rs`**
  - Wraps `serialport::SerialPort` or `tokio-serial`:
    - `list_ports() -> Vec<String>`
    - `open_port(name, config) -> Result<PortHandle>`
    - `read_nonblocking(&mut buf) -> Result<usize>`
    - `write_all(&[u8]) -> Result<()>`
  - Encapsulates OS-specific path conventions behind a uniform API.

- **`ui.rs`**
  - Defines TUI layout:
    - Log area (scrollable list/paragraph).
    - Input line.
    - Status bar.
  - Renders `App` state into `Frame`.
  - Handles high-level UI logic (e.g., scroll position).

- **`config.rs` (future)**
  - Loads/saves connection profiles.
  - Stores default port, baud rate, etc.

***

## 5. Platform-Specific Details

### 5.1 Windows

- Serial ports: `COM1`, `COM3`, etc.
- Recommended terminal: Windows Terminal or recent PowerShell.
- No special group membership required for serial access (driver-dependent).

### 5.2 macOS

- Serial ports: `/dev/cu.usbserial-*` or `/dev/tty.usbserial-*`
- Requires appropriate USB‑serial drivers (FTDI, CP210x, CH340, etc.).
- Application runs from standard terminals (Terminal.app, iTerm2).

### 5.3 Linux

- Serial ports: `/dev/ttyUSB*`, `/dev/ttyACM*`, `/dev/ttyS*`
- Users typically need to be in the `dialout` group:
  ```bash
  sudo usermod -aG dialout $USER
  ```
- Works in native Linux terminals and WSL2 (for Linux-style paths when USB devices are passed through).

***

## 6. Build and Distribution

### 6.1 Build Commands

From project root:

```bash
# Debug build
cargo build

# Release build
cargo build --release
```

Output:

- Windows: `target\release\rust-term-console.exe`
- macOS: `target/release/rust-term-console`
- Linux: `target/release/rust-term-console`

### 6.2 Cross-Compilation (Optional)

- Use `cross` or platform-specific toolchains to build for other targets from a single host.
- Provide prebuilt binaries for each platform in releases.

***

## 7. Acceptance Criteria (Initial Version)

The initial version shall be considered acceptable when:

1. It builds successfully on Windows, macOS, and Linux from the same codebase.
2. It can:
   - List available serial ports on each platform.
   - Connect to a serial port with configurable baud rate.
   - Display received data in a scrollable TUI log area.
   - Send user-typed text to the serial port on Enter.
   - Disconnect and exit cleanly, restoring the terminal.
3. It handles common error cases (missing port, permission denied, device unplugged) with understandable messages.
4. Basic documentation (README) explains:
   - How to build on each platform.
   - How to run and use the application.
   - Any platform-specific setup (e.g., `dialout` group on Linux).

***

## 8. Future Roadmap (Optional Phases)

**Phase 2:**
- Add session logging to file.
- Add connection profiles (save/load).

**Phase 3:**
- Add SSH and/or telnet support.
- Support multiple concurrent connections (tabs or splits).

**Phase 4:**
- Implement a GUI variant (windowed app) using `iced`, `tauri`, or similar, reusing the core serial/connection logic.
