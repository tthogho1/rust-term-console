//! Per-line timestamps (FR-O3). Tracks whether the output stream is at the
//! start of a line across reads, so a line split over two reads is stamped
//! once, when its first byte arrives.

/// Format of the stamp put in front of each line.
pub const FORMAT: &str = "[%H:%M:%S%.3f] ";

pub struct LineStamper {
    at_line_start: bool,
}

impl Default for LineStamper {
    fn default() -> Self {
        Self { at_line_start: true }
    }
}

impl LineStamper {
    /// Return `data` with `stamp` inserted before the first byte of every
    /// line, or unchanged when `stamp` is `None`. Line position is tracked
    /// either way, so turning stamps on mid-line starts at the next line
    /// rather than stamping the middle of one.
    pub fn apply(&mut self, data: &[u8], stamp: Option<&str>) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        for &byte in data {
            if self.at_line_start
                && let Some(stamp) = stamp
            {
                out.extend_from_slice(stamp.as_bytes());
            }
            out.push(byte);
            self.at_line_start = byte == b'\n';
        }
        out
    }
}

/// The current local time, formatted as `FORMAT`.
pub fn now() -> String {
    chrono::Local::now().format(FORMAT).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_each_line_once() {
        let mut s = LineStamper::default();
        assert_eq!(s.apply(b"one\ntwo\n", Some("T ")), b"T one\nT two\n");
    }

    #[test]
    fn line_split_across_reads_is_stamped_once() {
        let mut s = LineStamper::default();
        assert_eq!(s.apply(b"hel", Some("A ")), b"A hel");
        assert_eq!(s.apply(b"lo\nwor", Some("B ")), b"lo\nB wor");
    }

    #[test]
    fn stamp_waits_for_the_next_lines_first_byte() {
        let mut s = LineStamper::default();
        s.apply(b"prompt$ ls\n", Some("A "));
        // Nothing pending is stamped until the next line actually starts.
        assert_eq!(s.apply(b"", Some("B ")), b"");
        assert_eq!(s.apply(b"x", Some("C ")), b"C x");
    }

    #[test]
    fn enabling_mid_line_starts_at_the_next_line() {
        let mut s = LineStamper::default();
        assert_eq!(s.apply(b"partial", None), b"partial");
        assert_eq!(s.apply(b" end\nnext", Some("T ")), b" end\nT next");
    }

    #[test]
    fn format_is_time_with_milliseconds() {
        let stamp = now();
        // "[HH:MM:SS.mmm] "
        assert_eq!(stamp.len(), 15);
        assert!(stamp.starts_with('[') && stamp.ends_with("] "));
    }
}
