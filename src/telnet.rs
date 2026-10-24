//! Telnet client (FR-O1): a plain TCP connection with minimal IAC option
//! negotiation, exposed through the same [`Connection`] trait the serial
//! and SSH transports use.
//!
//! Every option a server offers (`WILL`) or requests (`DO`) is refused
//! (`DONT`/`WONT`), which keeps the session in plain NVT mode rather than
//! negotiating character-at-a-time mode (`WILL ECHO` + `WILL SGA`). That
//! fits how this app already talks to SSH and serial: a typed line, sent
//! whole, with local echo — not a keystroke-by-keystroke stream. Most
//! servers (Unix `telnetd`, network gear) work fine in NVT mode; a few that
//! insist on character mode may not.

use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::connection::{Connection, connect_tcp_with_timeout};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

pub struct TelnetConfig {
    pub host: String,
    pub port: u16,
}

pub struct TelnetConnection {
    stream: TcpStream,
    target: String,
    /// Carried across reads so an IAC sequence split across two
    /// `read_available` calls is still recognized.
    parser: IacParser,
}

impl TelnetConnection {
    pub fn connect(config: TelnetConfig) -> Result<Self> {
        let addr = format!("{}:{}", config.host, config.port);
        let stream = connect_tcp_with_timeout(&addr, CONNECT_TIMEOUT)
            .with_context(|| format!("failed to reach telnet host '{addr}' within {CONNECT_TIMEOUT:?}"))?;
        stream.set_read_timeout(Some(Duration::from_millis(10))).context("failed to set read timeout")?;
        stream.set_nodelay(true).context("failed to set TCP_NODELAY")?;

        Ok(Self { stream, target: addr, parser: IacParser::default() })
    }
}

impl Connection for TelnetConnection {
    fn read_available(&mut self) -> io::Result<Vec<u8>> {
        let mut buf = [0u8; 4096];
        match self.stream.read(&mut buf) {
            Ok(0) => Err(io::Error::new(ErrorKind::ConnectionAborted, "remote closed the telnet connection")),
            Ok(n) => {
                let (data, reply) = self.parser.filter(&buf[..n]);
                if !reply.is_empty() {
                    self.stream.write_all(&reply)?;
                }
                Ok(data)
            }
            // A read timeout maps to `WouldBlock` on Unix and `TimedOut` on
            // Windows; either just means "nothing arrived yet".
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        // A literal 0xFF in the data has to be escaped as IAC IAC, or the
        // server would read it as the start of a command.
        self.stream.write_all(&escape_iac(data))
    }

    fn describe(&self) -> String {
        self.target.clone()
    }

    fn close(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Doubles every `IAC` (0xFF) byte in `data`; a no-op allocation-free pass
/// for the overwhelmingly common case of plain text with none.
fn escape_iac(data: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if !data.contains(&IAC) {
        return std::borrow::Cow::Borrowed(data);
    }
    let mut out = Vec::with_capacity(data.len() + 4);
    for &b in data {
        out.push(b);
        if b == IAC {
            out.push(IAC);
        }
    }
    std::borrow::Cow::Owned(out)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum ParserState {
    #[default]
    Data,
    /// Just saw an `IAC` byte.
    Iac,
    /// Saw `IAC` then one of `WILL`/`WONT`/`DO`/`DONT`; waiting for the
    /// option byte that completes the three-byte negotiation command.
    Negotiating(u8),
    /// Inside `IAC SB <option> ... IAC SE`; its payload is discarded since
    /// we never agree to any option it could be negotiating details for.
    Subnegotiation,
    /// Inside a subnegotiation, just saw `IAC`; the next byte decides
    /// whether this ends it (`SE`) or is itself an escaped data byte.
    SubnegotiationIac,
}

/// Strips Telnet `IAC` command sequences out of a byte stream, replying to
/// negotiation requests by refusing them. Stateful so a sequence split
/// across two `filter` calls is still recognized (mirrors `AnsiFilter`).
#[derive(Debug, Default)]
struct IacParser {
    state: ParserState,
}

impl IacParser {
    /// Returns the plain data (for the log) and any negotiation replies to
    /// write back to the server.
    fn filter(&mut self, input: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut data = Vec::with_capacity(input.len());
        let mut reply = Vec::new();
        for &b in input {
            match self.state {
                ParserState::Data => {
                    if b == IAC {
                        self.state = ParserState::Iac;
                    } else {
                        data.push(b);
                    }
                }
                ParserState::Iac => {
                    self.state = match b {
                        IAC => {
                            // Escaped literal 0xFF in the data.
                            data.push(IAC);
                            ParserState::Data
                        }
                        WILL | WONT | DO | DONT => ParserState::Negotiating(b),
                        SB => ParserState::Subnegotiation,
                        // NOP, data mark, break, etc: no option byte follows.
                        _ => ParserState::Data,
                    };
                }
                ParserState::Negotiating(command) => {
                    let option = b;
                    // We never offer/request anything ourselves, so the
                    // only replies needed are refusing what the server
                    // offers or requests; a WONT/DONT needs no reply.
                    match command {
                        WILL => reply.extend_from_slice(&[IAC, DONT, option]),
                        DO => reply.extend_from_slice(&[IAC, WONT, option]),
                        _ => {}
                    }
                    self.state = ParserState::Data;
                }
                ParserState::Subnegotiation => {
                    if b == IAC {
                        self.state = ParserState::SubnegotiationIac;
                    }
                }
                ParserState::SubnegotiationIac => {
                    self.state = if b == SE { ParserState::Data } else { ParserState::Subnegotiation };
                }
            }
        }
        (data, reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passes_through_untouched() {
        let mut p = IacParser::default();
        let (data, reply) = p.filter(b"hello world\r\n");
        assert_eq!(data, b"hello world\r\n");
        assert!(reply.is_empty());
    }

    #[test]
    fn will_and_do_are_both_refused() {
        let mut p = IacParser::default();
        // Server: "I WILL do echo (option 1)" and "you DO suppress go-ahead (option 3)".
        let (data, reply) = p.filter(&[IAC, WILL, 1, IAC, DO, 3]);
        assert!(data.is_empty());
        assert_eq!(reply, [IAC, DONT, 1, IAC, WONT, 3]);
    }

    #[test]
    fn wont_and_dont_need_no_reply() {
        let mut p = IacParser::default();
        let (data, reply) = p.filter(&[IAC, WONT, 1, IAC, DONT, 3]);
        assert!(data.is_empty());
        assert!(reply.is_empty());
    }

    #[test]
    fn negotiation_sequence_split_across_reads_is_still_recognized() {
        let mut p = IacParser::default();
        let (data1, reply1) = p.filter(&[b'h', b'i', IAC, WILL]);
        let (data2, reply2) = p.filter(&[1, b'!']);
        assert_eq!(data1, b"hi");
        assert!(reply1.is_empty());
        assert_eq!(data2, b"!");
        assert_eq!(reply2, [IAC, DONT, 1]);
    }

    #[test]
    fn escaped_iac_byte_in_data_is_kept_as_a_single_0xff() {
        let mut p = IacParser::default();
        let (data, reply) = p.filter(&[b'a', IAC, IAC, b'b']);
        assert_eq!(data, [b'a', IAC, b'b']);
        assert!(reply.is_empty());
    }

    #[test]
    fn subnegotiation_payload_is_discarded() {
        let mut p = IacParser::default();
        // IAC SB <opt> <...payload, including a lone 'SE'-looking byte that
        // is NOT preceded by IAC and so doesn't end it...> IAC SE, then data.
        let mut input = vec![IAC, SB, 24, 0, b'x', b'y'];
        input.extend_from_slice(&[IAC, SE]);
        input.extend_from_slice(b"after");
        let (data, reply) = p.filter(&input);
        assert_eq!(data, b"after");
        assert!(reply.is_empty());
    }

    #[test]
    fn outgoing_iac_bytes_are_doubled() {
        assert_eq!(escape_iac(b"plain").as_ref(), b"plain");
        assert_eq!(escape_iac(&[b'a', IAC, b'b']).as_ref(), [b'a', IAC, IAC, b'b']);
    }

    /// End-to-end over a real loopback socket: a fake server offers an
    /// option and sends plain text; `TelnetConnection` should refuse the
    /// option (actually writing the refusal back) and hand only the plain
    /// text to the caller.
    #[test]
    fn full_round_trip_over_a_real_socket() {
        use std::net::TcpListener;
        use std::time::Instant;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            sock.write_all(&[IAC, WILL, 1]).unwrap(); // offers option 1 (echo)
            sock.write_all(b"hello\r\n").unwrap();

            // Read back whatever the client replied with.
            sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut buf = [0u8; 16];
            let n = sock.read(&mut buf).unwrap();
            buf[..n].to_vec()
        });

        let mut conn =
            TelnetConnection::connect(TelnetConfig { host: "127.0.0.1".to_string(), port: addr.port() }).unwrap();

        // `read_available`'s 10ms read timeout means a single call can
        // legitimately return empty; poll until the expected bytes arrive.
        let mut collected = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while collected.len() < b"hello\r\n".len() && Instant::now() < deadline {
            collected.extend(conn.read_available().unwrap());
        }
        assert_eq!(collected, b"hello\r\n");

        let reply = server.join().unwrap();
        assert_eq!(reply, [IAC, DONT, 1]);
    }
}
