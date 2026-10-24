//! **Prototype** (branch `prototype/vt100-terminal`): a real VT100 terminal
//! grid, evaluating how much of the UI would need to change to render
//! full-screen interactive programs (vim, htop, less) correctly — which the
//! plain-text log in `ansi.rs`/`ui::draw_terminal` cannot do, since those
//! rely on absolute cursor addressing a scrolling text label can't express
//! (see README "Known limitations").
//!
//! This feeds the connection's *raw* bytes (before `AnsiFilter` strips
//! them) into a [`vt100::Parser`], which tracks a fixed-size grid of cells
//! (text, color, bold/underline/etc.) and the cursor position, the same
//! way a real terminal emulator would. It is gated behind the header's
//! "View → Real terminal (experimental)" so it can be compared side by
//! side with the existing plain-text log rather than replacing it outright
//! — see `ui::draw_vt_grid` for the rendering half.
//!
//! What this prototype does *not* attempt (left for a real implementation
//! if this direction is adopted): scrollback browsing (vt100 tracks it,
//! but nothing here exposes a scrollbar for it — only the current screen
//! is drawn), bold rendered as an actual bold font (egui needs a second
//! monospace font registered for that), and the exact xterm 256-color
//! palette (`ansi_256_color` below is an approximation).

use std::num::NonZeroU16;

#[derive(Default)]
pub struct TerminalGrid {
    parser: vt100::Parser,
}

impl TerminalGrid {
    /// Feed raw incoming bytes (escape sequences and all) from the
    /// connection. Unlike `Session::append_log`, nothing is stripped here —
    /// vt100 needs the escape sequences themselves to update the grid.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Resize the grid to match the log area, in characters. Cheap and
    /// local (unlike `Connection::resize`, which tells the remote), so this
    /// is called every frame rather than debounced.
    pub fn resize(&mut self, cols: u32, rows: u32) {
        let one = NonZeroU16::new(1).expect("1 is non-zero");
        let cols = NonZeroU16::new(cols.clamp(1, u16::MAX.into()) as u16).unwrap_or(one);
        let rows = NonZeroU16::new(rows.clamp(1, u16::MAX.into()) as u16).unwrap_or(one);
        self.parser.set_size(rows, cols);
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
