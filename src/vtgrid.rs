//! A real VT100 terminal grid, rendered alongside the plain-text log (see
//! the header's "View → Real terminal" and `ui::draw_vt_grid`) rather than
//! replacing it — the plain-text log stays simple, selectable, and is what
//! `Session::log`/the log file/Notebook/Script all read; this is a second,
//! independent view of the same incoming bytes for the one thing the log
//! fundamentally can't do: full-screen interactive programs (vim, htop,
//! less) that rely on absolute cursor addressing to redraw the screen.
//!
//! This feeds the connection's *raw* bytes (before `AnsiFilter` strips
//! them) into a [`vt100::Parser`], which tracks a fixed-size grid of cells
//! (text, color, bold/underline/etc.), the cursor position, and a
//! scrollback buffer, the same way a real terminal emulator would.
//!
//! Known gaps, same as any terminal emulator built on a cell grid rather
//! than a real framebuffer: bold is rendered as a second painted copy
//! offset by half a pixel (`ui::draw_vt_grid`), not a true bold font face,
//! since egui only has the one (regular-weight) monospace font bundled by
//! default; and `ansi_256_color` uses xterm's documented default palette,
//! which a terminal with a customized palette (many users reconfigure
//! their 16 basic colors) won't exactly match.

use std::num::NonZeroU16;

/// How many scrollback lines to keep, independent of the visible grid
/// size. 2000 matches tmux's conventional default and is a comparable
/// memory budget to `MAX_LOG_BYTES` in `app.rs` (2000 lines of 80
/// characters is on the same order as 2 MB only at very long line
/// lengths; cells are small, so this is in practice far cheaper).
const SCROLLBACK_LINES: usize = 2000;

pub struct TerminalGrid {
    parser: vt100::Parser,
}

impl Default for TerminalGrid {
    fn default() -> Self {
        Self { parser: vt100::Parser::new(vt100::Parser::DEFAULT_ROWS, vt100::Parser::DEFAULT_COLS, SCROLLBACK_LINES) }
    }
}

impl TerminalGrid {
    /// Feed raw incoming bytes (escape sequences and all) from the
    /// connection. Unlike `Session::append_log`, nothing is stripped here —
    /// vt100 needs the escape sequences themselves to update the grid.
    ///
    /// If the view is currently scrolled back, it stays anchored to the
    /// same absolute lines as new output arrives (like a real terminal:
    /// scrolling back and having the ground shift under you because more
    /// output showed up would be useless) rather than snapping back to
    /// the live tail — only scrolling all the way to the bottom resumes
    /// following it.
    pub fn process(&mut self, bytes: &[u8]) {
        let offset = self.scrollback_offset();
        let len_before = self.scrollback_len();
        self.parser.process(bytes);
        if offset > 0 {
            let grew = self.scrollback_len().saturating_sub(len_before);
            self.set_scrollback_offset(offset + grew);
        }
    }

    /// Resize the grid to match the log area, in characters. Cheap and
    /// local (unlike `Connection::resize`, which tells the remote), so this
    /// is called every frame rather than debounced.
    pub fn resize(&mut self, cols: u32, rows: u32) {
        let one = NonZeroU16::new(1).expect("1 is non-zero");
        let cols = NonZeroU16::new(cols.clamp(1, u16::MAX.into()) as u16).unwrap_or(one);
        let rows = NonZeroU16::new(rows.clamp(1, u16::MAX.into()) as u16).unwrap_or(one);
        self.parser.set_size(rows, cols);
        // A reflow can change how many lines the same content takes up;
        // keep the offset in bounds rather than pointing past the end.
        let offset = self.scrollback_offset();
        self.set_scrollback_offset(offset);
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// How many lines of history are available above the current screen.
    pub fn scrollback_len(&self) -> usize {
        self.parser.screen().scrollback_len()
    }

    /// How far back the view is currently scrolled; `0` is the live tail.
    pub fn scrollback_offset(&self) -> usize {
        self.parser.screen().scrollback()
    }

    /// Scroll by `delta` lines (positive = back into history, negative =
    /// toward the live tail), clamped to `[0, scrollback_len()]`.
    pub fn scroll_by(&mut self, delta: i64) {
        let current = self.scrollback_offset() as i64;
        self.set_scrollback_offset((current + delta).max(0) as usize);
    }

    pub fn jump_to_live_tail(&mut self) {
        self.set_scrollback_offset(0);
    }

    fn set_scrollback_offset(&mut self, offset: usize) {
        // `Screen::set_scrollback` already clamps to the actual history
        // size, so an out-of-range `offset` (e.g. after a resize shrinks
        // it) is handled for us.
        self.parser.screen_mut().set_scrollback(offset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill_lines(grid: &mut TerminalGrid, count: usize) {
        for i in 0..count {
            grid.process(format!("line {i}\r\n").as_bytes());
        }
    }

    #[test]
    fn plain_text_lands_in_the_top_left_cells() {
        let mut grid = TerminalGrid::default();
        grid.process(b"hi");
        assert_eq!(grid.screen().cell(0, 0).unwrap().contents(), "h");
        assert_eq!(grid.screen().cell(0, 1).unwrap().contents(), "i");
    }

    #[test]
    fn cursor_addressing_places_text_anywhere_on_the_grid() {
        // This is exactly what the plain-text log can't do: jump the
        // cursor and overwrite a specific cell, as `vim`/`htop` rely on.
        let mut grid = TerminalGrid::default();
        grid.process(b"\x1b[5;10Hx");
        assert_eq!(grid.screen().cell(4, 9).unwrap().contents(), "x");
    }

    #[test]
    fn sgr_color_codes_set_cell_colors() {
        let mut grid = TerminalGrid::default();
        grid.process(b"\x1b[31mred");
        assert_eq!(grid.screen().cell(0, 0).unwrap().fgcolor(), vt100::Color::Idx(1));
    }

    #[test]
    fn resize_changes_the_reported_grid_size() {
        let mut grid = TerminalGrid::default();
        grid.resize(100, 40);
        let (rows, cols) = grid.screen().size();
        assert_eq!((rows.get(), cols.get()), (40, 100));
    }

    #[test]
    fn scrolling_back_reveals_history_above_the_current_screen() {
        let mut grid = TerminalGrid::default();
        // Default grid is 24 rows; push well past that into scrollback.
        fill_lines(&mut grid, 30);
        assert!(grid.scrollback_len() > 0);

        grid.scroll_by(grid.scrollback_len() as i64);
        assert_eq!(grid.scrollback_offset(), grid.scrollback_len());
        // The oldest line ("line 0") should now be at the top.
        assert_eq!(grid.screen().cell(0, 0).unwrap().contents(), "l");

        grid.jump_to_live_tail();
        assert_eq!(grid.scrollback_offset(), 0);
    }

    #[test]
    fn scroll_by_is_clamped_to_the_available_history() {
        let mut grid = TerminalGrid::default();
        fill_lines(&mut grid, 30);

        grid.scroll_by(1_000_000);
        assert_eq!(grid.scrollback_offset(), grid.scrollback_len());

        grid.scroll_by(-1_000_000);
        assert_eq!(grid.scrollback_offset(), 0);
    }

    #[test]
    fn staying_scrolled_back_does_not_shift_under_new_output() {
        let mut grid = TerminalGrid::default();
        fill_lines(&mut grid, 30);
        grid.scroll_by(grid.scrollback_len() as i64);
        let line_at_top_before = grid.screen().cell(0, 0).unwrap().contents().to_string();

        // More output arrives while scrolled back...
        fill_lines(&mut grid, 5);

        // ...but the view should still show the same absolute line, not
        // have been dragged along by it.
        assert_eq!(grid.screen().cell(0, 0).unwrap().contents(), line_at_top_before);
    }

    #[test]
    fn being_at_the_live_tail_keeps_following_new_output() {
        let mut grid = TerminalGrid::default();
        fill_lines(&mut grid, 30);
        assert_eq!(grid.scrollback_offset(), 0);

        fill_lines(&mut grid, 5);

        // Still following the tail, not accidentally pinned to history.
        assert_eq!(grid.scrollback_offset(), 0);
    }
}
