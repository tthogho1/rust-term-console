//! Serial port abstraction (FR-1..FR-5). Wraps the `serialport` crate behind
//! the [`Connection`] trait so `app.rs` doesn't care whether it's talking to
//! a serial port or an SSH shell.

use std::io::{self, ErrorKind, Read, Write};
use std::time::Duration;

use anyhow::{Context, Result};
use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};

use crate::connection::Connection;

/// User-facing serial configuration (FR-2).
#[derive(Debug, Clone)]
pub struct SerialConfig {
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: DataBits,
    pub parity: Parity,
    pub stop_bits: StopBits,
    pub flow_control: FlowControl,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port_name: String::new(),
            baud_rate: 115_200,
            data_bits: DataBits::Eight,
            parity: Parity::None,
            stop_bits: StopBits::One,
            flow_control: FlowControl::None,
        }
    }
}

/// FR-1: enumerate available serial ports on the host.
pub fn list_ports() -> Result<Vec<String>> {
    let ports = serialport::available_ports().context("failed to enumerate serial ports")?;
    Ok(ports.into_iter().map(|p| p.port_name).collect())
}

pub struct SerialConnection {
    port: Box<dyn SerialPort>,
    config: SerialConfig,
}

impl SerialConnection {
    /// FR-3: open a connection to a selected serial port with the given
    /// parameters. A short read timeout lets the main loop poll without
    /// blocking the UI (NFR-3).
    pub fn open(config: SerialConfig) -> Result<Self> {
        let port = serialport::new(&config.port_name, config.baud_rate)
            .data_bits(config.data_bits)
            .parity(config.parity)
            .stop_bits(config.stop_bits)
            .flow_control(config.flow_control)
            .timeout(Duration::from_millis(10))
            .open()
            .with_context(|| {
                format!(
                    "failed to open serial port '{}' (check the device is present and you have \
                     permission — on Linux you may need to be in the 'dialout' group)",
                    config.port_name
                )
            })?;

        Ok(Self { port, config })
    }
}

impl Connection for SerialConnection {
    fn read_available(&mut self) -> io::Result<Vec<u8>> {
        let mut buf = [0u8; 4096];
        match self.port.read(&mut buf) {
            Ok(0) => Ok(Vec::new()),
            Ok(n) => Ok(buf[..n].to_vec()),
            // A timed-out read with zero bytes just means "nothing yet".
            Err(e) if e.kind() == ErrorKind::TimedOut => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.port.write_all(data)?;
        self.port.flush()
    }

    fn describe(&self) -> String {
        format!("{} @ {} baud", self.config.port_name, self.config.baud_rate)
    }

    fn close(&mut self) {
        // serialport has no explicit close(); dropping the handle releases
        // the OS resource. Nothing else to do here (NFR-5).
    }
}
