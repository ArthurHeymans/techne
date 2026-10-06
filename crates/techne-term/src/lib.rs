//! The editor in a terminal (EDITOR.md, section 8): a frontend over the
//! presentation protocol like the window, but in cells.
//!
//! `Term` is the whole frontend except the terminal itself. It decodes the
//! bytes a terminal sends into inputs (`keys`), resolves clicks and
//! scrolling against the frame it drew last, and draws snapshots into a
//! `Grid` of cells (`layout`), which `Grid::paint` turns into the bytes that
//! update a terminal. Tests drive it with bytes, headless; `main.rs`
//! connects it to a real terminal.
//!
//! The terminal is asked whether it has the kitty keyboard protocol, then
//! for its device attributes; once that answer is in, the bound keys this
//! terminal cannot send are reported to the session.

pub mod keys;
pub mod layout;

use std::{fmt::Write, time::Instant};

use techne_editor::{
    present::{CursorShape, Input, Output, Snapshot},
    segment::Segment,
};
use techne_text::ropey::Rope;

use crate::{
    keys::{Decoder, Event, KITTY_FLAGS, Mouse, MouseKind, Protocol},
    layout::Line,
};

/// Written when starting: the alternate screen, no automatic wrapping,
/// mouse buttons and drags in SGR encoding, then the queries for the kitty
/// keyboard protocol and the device attributes.
pub const SETUP: &str = "\x1b[?1049h\x1b[?7l\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?u\x1b[c";

/// Written when leaving: undoes `SETUP` and the kitty flags (a terminal
/// without the protocol ignores that), and resets the cursor.
pub const RESTORE: &str = "\x1b[<u\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[?7h\x1b[0 q\x1b[?25h\x1b[?1049l";

/// Lines the mouse wheel scrolls, as in the window.
const WHEEL_LINES: i64 = 3;

pub struct Term {
    cols: usize,
    rows: usize,
    decoder: Decoder,
    protocol: Protocol,
    /// The terminal answered the device attributes query, so `protocol` is
    /// what it has.
    probed: bool,
    /// Bound keys not yet checked against the protocol.
    bindings: Option<Vec<String>>,
    snap: Option<Snapshot>,
    anchor: usize,
    /// The revision and lines of the frame drawn last: what a click is on.
    shown: Option<(u64, Vec<Line>)>,
    painted: Option<Grid>,
    dragging: bool,
    /// Bytes for the terminal other than the screen.
    replies: String,
}

impl Term {
    pub fn new(cols: usize, rows: usize) -> Term {
        Term {
            cols: cols.max(1),
            rows: rows.max(2),
            decoder: Decoder::default(),
            protocol: Protocol::Legacy,
            probed: false,
            bindings: None,
            snap: None,
            anchor: 0,
            shown: None,
            painted: None,
            dragging: false,
            replies: String::new(),
        }
    }

    /// A new terminal size. The scroll anchor stays, as in the window.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        (self.cols, self.rows) = (cols.max(1), rows.max(2));
    }

    /// Rows for text, above the status line.
    fn text_rows(&self) -> usize {
        self.rows - 1
    }

    /// The inputs for bytes from the terminal.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Input> {
        self.decoder.feed(bytes).into_iter().filter_map(|e| self.event(e)).collect()
    }

    /// Whether the input ends in an incomplete sequence: `flush` after a
    /// short pause without more.
    pub fn waiting(&self) -> bool {
        self.decoder.waiting()
    }

    pub fn flush(&mut self) -> Vec<Input> {
        self.decoder.flush().into_iter().filter_map(|e| self.event(e)).collect()
    }

    fn event(&mut self, event: Event) -> Option<Input> {
        match event {
            Event::Key(key) => Some(Input::Key { key, at: Instant::now() }),
            Event::Unrecognized(input) => Some(Input::Unrecognized { input }),
            Event::KittyKeyboard => {
                self.protocol = Protocol::Kitty;
                write!(self.replies, "\x1b[>{KITTY_FLAGS}u").expect("a string");
                None
            }
            Event::DeviceAttributes => {
                self.probed = true;
                self.report()
            }
            Event::Mouse(m) => self.mouse(m),
        }
    }

    /// Once both the protocol and the bindings are known, the bound keys
    /// this terminal cannot send.
    fn report(&mut self) -> Option<Input> {
        if !self.probed {
            return None;
        }
        let protocol = self.protocol;
        let keys: Vec<String> =
            self.bindings.take()?.into_iter().filter(|seq| seq.split(' ').any(|k| !keys::sendable(k, protocol))).collect();
        (!keys.is_empty()).then_some(Input::Unsendable { keys })
    }

    fn mouse(&mut self, m: Mouse) -> Option<Input> {
        match m.kind {
            MouseKind::Press => {
                self.dragging = true;
                self.click(m, m.shift)
            }
            MouseKind::Drag if self.dragging => self.click(m, true),
            MouseKind::Release => {
                self.dragging = false;
                None
            }
            MouseKind::WheelUp => self.scroll_by(-WHEEL_LINES),
            MouseKind::WheelDown => self.scroll_by(WHEEL_LINES),
            _ => None,
        }
    }

    /// A click on the frame shown, as a position in its revision.
    fn click(&self, m: Mouse, extend: bool) -> Option<Input> {
        let (revision, lines) = self.shown.as_ref()?;
        if m.row >= self.text_rows() {
            return None;
        }
        let pos = layout::hit(lines, m.col, m.row)?;
        Some(Input::Click { revision: *revision, pos, extend, at: Instant::now() })
    }

    fn scroll_by(&mut self, lines: i64) -> Option<Input> {
        let s = self.snap.as_ref()?;
        self.anchor = layout::scroll_lines(&s.text, self.anchor, lines, self.cols);
        Some(Input::Scroll { revision: s.revision, anchor: self.anchor })
    }

    /// Take an output of the runtime; a snapshot that moved the caret off
    /// screen gives a scroll.
    pub fn output(&mut self, out: Output) -> Option<Input> {
        match out {
            Output::Snapshot(s) => {
                let moved = self.snap.as_ref().is_none_or(|old| old.head() != s.head() || old.revision != s.revision);
                self.anchor = s.scroll;
                let answers_input = !s.answers.is_empty();
                self.snap = Some(*s);
                if answers_input && moved { self.keep_caret_visible() } else { None }
            }
            Output::Bindings(keys) => {
                self.bindings = Some(keys);
                self.report()
            }
            Output::Quit => None,
        }
    }

    fn keep_caret_visible(&mut self) -> Option<Input> {
        let s = self.snap.as_ref()?;
        self.anchor = layout::keep_visible(&s.text, self.anchor, s.head(), self.text_rows(), self.cols)?;
        Some(Input::Scroll { revision: s.revision, anchor: self.anchor })
    }

    /// Lay out and draw the latest snapshot; clicks are on it from now on.
    pub fn draw(&mut self) -> Grid {
        let mut grid = Grid::new(self.cols, self.rows);
        let Some(s) = &self.snap else { return grid };
        let lines = layout::frame(&s.text, self.anchor, self.text_rows(), self.cols);
        for (r, line) in lines.iter().enumerate() {
            for g in &line.glyphs {
                grid.put(r, g.col, &g.shown, g.width, Style::Plain);
            }
        }
        for (from, to) in s.selections.iter().filter(|(a, h)| a != h).map(|&(a, h)| (a.min(h), a.max(h))) {
            for (r, line) in lines.iter().enumerate() {
                let glyphs = line.glyphs.iter().filter(|g| g.pos < to && g.pos + g.len > from).map(|g| (g.col, g.width));
                // A selected line break shows as one cell after its line.
                let newline = (!line.continued && from <= line.end && line.end < to).then_some((line.width, 1));
                for (col, width) in glyphs.chain(newline) {
                    grid.style(r, col..col + width, Style::Selected);
                }
            }
        }
        grid.cursor = layout::caret(&lines, s.head()).map(|(r, c)| (r, c.min(self.cols - 1)));
        grid.shape = s.cursor;
        let status = Rope::from_str(&s.status);
        let line = layout::wrap(&status, Segment { start: 0, end: status.len_bytes() }, self.cols).swap_remove(0);
        let last = self.rows - 1;
        grid.style(last, 0..self.cols, Style::Status);
        for g in &line.glyphs {
            grid.put(last, g.col, &g.shown, g.width, Style::Status);
        }
        self.shown = Some((s.revision, lines));
        grid
    }

    /// The bytes that bring the terminal up to date: replies to it, and the
    /// rows of the screen that changed since the last paint.
    pub fn paint(&mut self) -> String {
        let grid = self.draw();
        let out = std::mem::take(&mut self.replies) + &grid.paint(self.painted.as_ref());
        self.painted = Some(grid);
        out
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    #[default]
    Plain,
    Selected,
    Status,
}

impl Style {
    fn sgr(self) -> &'static str {
        match self {
            Style::Plain => "\x1b[0m",
            Style::Selected => "\x1b[0;97;44m",
            Style::Status => "\x1b[0;7m",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// What the cell shows; empty when the glyph of a cell to its left
    /// covers it (the second half of a wide character, the rest of a tab).
    pub text: String,
    pub style: Style,
}

/// A screen of cells, the cursor on one of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Cell>,
    pub cursor: Option<(usize, usize)>,
    pub shape: CursorShape,
}

impl Grid {
    fn new(cols: usize, rows: usize) -> Grid {
        let blank = Cell { text: " ".into(), style: Style::Plain };
        Grid { cols, rows, cells: vec![blank; cols * rows], cursor: None, shape: CursorShape::Bar }
    }

    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.cells[row * self.cols + col]
    }

    fn row(&self, row: usize) -> &[Cell] {
        &self.cells[row * self.cols..(row + 1) * self.cols]
    }

    /// The text of a row as the terminal shows it.
    pub fn row_text(&self, row: usize) -> String {
        self.row(row).iter().map(|c| c.text.as_str()).collect()
    }

    /// Show `text`, `width` cells wide, from a cell on; clipped when it does
    /// not fit.
    fn put(&mut self, row: usize, col: usize, text: &str, width: usize, style: Style) {
        if col + width > self.cols {
            return;
        }
        let at = row * self.cols + col;
        self.cells[at] = Cell { text: text.into(), style };
        for c in &mut self.cells[at + 1..at + width] {
            *c = Cell { text: String::new(), style };
        }
    }

    fn style(&mut self, row: usize, cols: std::ops::Range<usize>, style: Style) {
        let cols = cols.start.min(self.cols)..cols.end.min(self.cols);
        for c in &mut self.cells[row * self.cols + cols.start..row * self.cols + cols.end] {
            c.style = style;
        }
    }

    /// The bytes that draw this grid over `old`: only the rows that differ,
    /// in one synchronized update.
    pub fn paint(&self, old: Option<&Grid>) -> String {
        let same_size = old.filter(|o| (o.cols, o.rows) == (self.cols, self.rows));
        let mut out = String::from("\x1b[?2026h\x1b[?25l");
        for r in (0..self.rows).filter(|&r| same_size.is_none_or(|o| o.row(r) != self.row(r))) {
            write!(out, "\x1b[{};1H", r + 1).expect("a string");
            let mut style = None;
            for c in self.row(r).iter().filter(|c| !c.text.is_empty()) {
                if style != Some(c.style) {
                    out.push_str(c.style.sgr());
                    style = Some(c.style);
                }
                out.push_str(&c.text);
            }
            out.push_str("\x1b[0m");
        }
        if let Some((r, c)) = self.cursor {
            let shape = match self.shape {
                CursorShape::Bar => 6,
                CursorShape::Block => 2,
            };
            write!(out, "\x1b[{};{}H\x1b[{shape} q\x1b[?25h", r + 1, c + 1).expect("a string");
        }
        out.push_str("\x1b[?2026l");
        out
    }
}
