//! Minimal ANSI/VT100 escape-sequence stripper.
//!
//! We render the log as plain text, not a real terminal grid, so control
//! sequences for color, cursor movement, and line-clearing (which any
//! interactive shell sends by default) would otherwise show up as literal
//! garbage. This consumes and discards them instead. It is stateful so a
//! sequence split across two `read()` chunks is still recognized.
//!
//! This does not make the log a full terminal emulator: full-screen
//! interactive programs (vim, htop, less) rely on absolute cursor
//! addressing to redraw the screen and will still look wrong here. Plain
//! shell sessions come out clean.

#[derive(Debug, Default)]
enum State {
    #[default]
    Normal,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

#[derive(Debug, Default)]
pub struct AnsiFilter {
    state: State,
}

impl AnsiFilter {
    /// Feed raw bytes in, get cleaned bytes out. Safe to call repeatedly
    /// with successive chunks from the same stream.
    pub fn filter(&mut self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len());
        for &b in input {
            match self.state {
                State::Normal => match b {
                    0x1B => self.state = State::Escape,
                    b'\r' => {}
                    0x00..=0x1F if b != b'\n' && b != b'\t' => {}
                    _ => out.push(b),
                },
                State::Escape => {
                    self.state = match b {
                        b'[' => State::Csi,
                        b']' => State::Osc,
                        _ => State::Normal,
                    };
                }
                State::Csi => {
                    if (0x40..=0x7E).contains(&b) {
                        self.state = State::Normal;
                    }
                }
                State::Osc => match b {
                    0x07 => self.state = State::Normal,
                    0x1B => self.state = State::OscEscape,
                    _ => {}
                },
                State::OscEscape => {
                    self.state = if b == b'\\' { State::Normal } else { State::Osc };
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_color_codes() {
        let mut f = AnsiFilter::default();
        let cleaned = f.filter(b"\x1b[1;32muser@host\x1b[0m:\x1b[34m~\x1b[0m$ ");
        assert_eq!(cleaned, b"user@host:~$ ");
    }

    #[test]
    fn strips_osc_title_sequence() {
        let mut f = AnsiFilter::default();
        let cleaned = f.filter(b"\x1b]0;my terminal title\x07hello");
        assert_eq!(cleaned, b"hello");
    }

    #[test]
    fn drops_bare_carriage_return() {
        let mut f = AnsiFilter::default();
        let cleaned = f.filter(b"line1\r\nline2");
        assert_eq!(cleaned, b"line1\nline2");
    }

    #[test]
    fn handles_sequence_split_across_chunks() {
        let mut f = AnsiFilter::default();
        let mut out = f.filter(b"before\x1b[1");
        out.extend(f.filter(b";32mafter"));
        assert_eq!(out, b"beforeafter");
    }
}
