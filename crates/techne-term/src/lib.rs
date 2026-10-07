//! The editor in a terminal (EDITOR.md, section 8): a frontend over the
//! presentation protocol like the window, but in cells.
//!
//! `Term` is the whole frontend except the terminal itself. It decodes the
//! bytes a terminal sends into inputs (`keys`), resolves clicks and
//! scrolling against the frame it drew last, and draws snapshots into a
//! `Grid` of cells (`layout`), which `Grid::paint` turns into the bytes that
//! update a terminal: the panes stacked, each with its mode line, and the
//! echo area on the last row. Tests drive it with bytes, headless; `main.rs`
//! connects it to a real terminal, and to a runtime through `host`.
//!
//! The terminal is asked whether it has the kitty keyboard protocol, then
//! for its device attributes; once that answer is in, the bound keys this
//! terminal cannot send are reported to the session.

pub mod keys;
pub mod layout;

use std::{fmt::Write, ops::Range, time::Instant};

use techne_editor::{
    present::{CursorShape, Input, Output, Pane, Snapshot},
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

/// Where a pane is on the screen: `height` rows from `top`, its text in all
/// but the last, which is its mode line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub top: usize,
    pub height: usize,
}

impl Area {
    pub fn text_rows(&self) -> usize {
        self.height.saturating_sub(1)
    }

    fn contains(&self, row: usize) -> bool {
        (self.top..self.top + self.height).contains(&row)
    }
}

/// `n` panes stacked in `rows` rows: equal heights, the first ones a row
/// higher when the rows do not divide evenly.
pub fn areas(rows: usize, n: usize) -> Vec<Area> {
    let heights = (0..n).map(|i| rows / n + usize::from(i < rows % n));
    heights
        .scan(0, |top, height| {
            let area = Area { top: *top, height };
            *top += height;
            Some(area)
        })
        .collect()
}

/// A pane as drawn last: what a click on it is on.
struct Shown {
    view: u64,
    revision: u64,
    area: Area,
    lines: Vec<Line>,
}

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
    /// Each pane's scroll anchor: the snapshot's, or where this frontend
    /// scrolled it since.
    anchors: Vec<usize>,
    shown: Vec<Shown>,
    painted: Option<Grid>,
    /// The view a press was in, while the button is held.
    dragging: Option<u64>,
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
            anchors: Vec::new(),
            shown: Vec::new(),
            painted: None,
            dragging: None,
            replies: String::new(),
        }
    }

    /// A new terminal size. The scroll anchors stay, as in the window.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        (self.cols, self.rows) = (cols.max(1), rows.max(2));
    }

    /// The latest snapshot.
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snap.as_ref()
    }

    /// Where the latest snapshot's panes are drawn, above the echo area.
    pub fn areas(&self) -> Vec<Area> {
        areas(self.rows - 1, self.snap.as_ref().map_or(0, |s| s.panes.len()))
    }

    /// A new runtime serves this frontend (the old one ended): the screen is
    /// painted afresh, over whatever was written to it meanwhile.
    pub fn restarted(&mut self) {
        self.painted = None;
        self.dragging = None;
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
                let shown = self.shown.iter().find(|s| (s.area.top..s.area.top + s.area.text_rows()).contains(&m.row))?;
                self.dragging = Some(shown.view);
                self.click(shown, m, m.shift)
            }
            MouseKind::Drag => self.click(self.shown.iter().find(|s| Some(s.view) == self.dragging)?, m, true),
            MouseKind::Release => {
                self.dragging = None;
                None
            }
            MouseKind::WheelUp => self.wheel(m.row, -WHEEL_LINES),
            MouseKind::WheelDown => self.wheel(m.row, WHEEL_LINES),
            _ => None,
        }
    }

    /// A click on a pane as shown, as a position in its revision. Dragged
    /// out of the pane, it is on the pane's nearest row.
    fn click(&self, shown: &Shown, m: Mouse, extend: bool) -> Option<Input> {
        let last = shown.area.text_rows().checked_sub(1)?;
        let row = m.row.saturating_sub(shown.area.top).min(last);
        let pos = layout::hit(&shown.lines, m.col, row)?;
        Some(Input::Click { view: shown.view, revision: shown.revision, pos, extend, at: Instant::now() })
    }

    /// The wheel over a row: scrolls the pane there.
    fn wheel(&mut self, row: usize, lines: i64) -> Option<Input> {
        let view = self.shown.iter().find(|s| s.area.contains(row))?.view;
        let s = self.snap.as_ref()?;
        let i = s.panes.iter().position(|p| p.view == view)?;
        let pane = &s.panes[i];
        self.anchors[i] = layout::scroll_lines(&pane.text, self.anchors[i], lines, self.cols);
        Some(Input::Scroll { view, revision: pane.revision, anchor: self.anchors[i] })
    }

    /// Take an output of the runtime. A snapshot that answers input scrolls
    /// panes whose caret it may have moved off screen: the focused pane when
    /// its caret or text changed, a new pane, every pane when there are more
    /// or fewer.
    pub fn output(&mut self, out: Output) -> Vec<Input> {
        match out {
            Output::Snapshot(s) => {
                let old = self.snap.take();
                let moved = |i: usize, p: &Pane| {
                    let before = old.as_ref().and_then(|o| Some((o, o.panes.iter().find(|q| q.view == p.view)?)));
                    before.is_none_or(|(o, q)| {
                        o.panes.len() != s.panes.len() || (i == s.focus && (q.head() != p.head() || q.revision != p.revision))
                    })
                };
                let scroll: Vec<usize> = if s.answers.is_empty() {
                    Vec::new()
                } else {
                    s.panes.iter().enumerate().filter(|&(i, p)| moved(i, p)).map(|(i, _)| i).collect()
                };
                self.anchors = s.panes.iter().map(|p| p.scroll).collect();
                self.snap = Some(*s);
                scroll.into_iter().filter_map(|i| self.keep_caret_visible(i)).collect()
            }
            Output::Bindings(keys) => {
                self.bindings = Some(keys);
                self.report().into_iter().collect()
            }
            Output::Quit => Vec::new(),
        }
    }

    fn keep_caret_visible(&mut self, i: usize) -> Option<Input> {
        let rows = self.areas()[i].text_rows();
        let p = &self.snap.as_ref()?.panes[i];
        if rows == 0 {
            return None;
        }
        self.anchors[i] = layout::keep_visible(&p.text, self.anchors[i], p.head(), rows, self.cols)?;
        Some(Input::Scroll { view: p.view, revision: p.revision, anchor: self.anchors[i] })
    }

    /// Lay out and draw the latest snapshot; clicks are on it from now on.
    pub fn draw(&mut self) -> Grid {
        let mut grid = Grid::new(self.cols, self.rows);
        let areas = self.areas();
        let Some(s) = &self.snap else { return grid };
        self.shown = s
            .panes
            .iter()
            .zip(areas)
            .zip(&self.anchors)
            .enumerate()
            .map(|(i, ((pane, area), &anchor))| {
                let lines = layout::frame(&pane.text, anchor, area.text_rows(), self.cols);
                grid.pane(pane, area, &lines, i == s.focus);
                Shown { view: pane.view, revision: pane.revision, area, lines }
            })
            .collect();
        grid.label(self.rows - 1, &s.echo, Style::Plain);
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

/// How a highlight's face is drawn, for the faces this frontend knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Highlight,
    Warning,
    Error,
    Comment,
    Keyword,
    String,
}

impl Face {
    pub fn named(name: &str) -> Option<Face> {
        Some(match name {
            "highlight" => Face::Highlight,
            "warning" => Face::Warning,
            "error" => Face::Error,
            "comment" => Face::Comment,
            "keyword" => Face::Keyword,
            "string" => Face::String,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    #[default]
    Plain,
    Selected,
    /// The focused pane's mode line.
    Status,
    /// The mode line of another pane.
    InactiveStatus,
    /// The caret of a pane that is not focused (the terminal's cursor is
    /// the focused one's).
    Caret,
    Face(Face),
}

impl Style {
    fn sgr(self) -> &'static str {
        match self {
            Style::Plain => "\x1b[0m",
            Style::Selected => "\x1b[0;97;44m",
            Style::Status => "\x1b[0;1;7m",
            Style::InactiveStatus => "\x1b[0;37;100m",
            Style::Caret => "\x1b[0;7m",
            Style::Face(Face::Highlight) => "\x1b[0;30;43m",
            Style::Face(Face::Warning) => "\x1b[0;1;33m",
            Style::Face(Face::Error) => "\x1b[0;1;31m",
            Style::Face(Face::Comment) => "\x1b[0;3;90m",
            Style::Face(Face::Keyword) => "\x1b[0;1;35m",
            Style::Face(Face::String) => "\x1b[0;32m",
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

/// The cells of `lines` (row, columns) that show the text from `from` to
/// `to`; with `newline`, also a cell after a line whose line break is in it.
fn cells(lines: &[Line], from: usize, to: usize, newline: bool) -> impl Iterator<Item = (usize, Range<usize>)> {
    let first = lines.partition_point(|l| l.end < from);
    lines.iter().enumerate().skip(first).take_while(move |(_, l)| l.start < to).flat_map(move |(r, line)| {
        let g = &line.glyphs;
        let glyphs = &g[g.partition_point(|g| g.pos + g.len <= from)..g.partition_point(|g| g.pos < to)];
        let newline = (newline && !line.continued && from <= line.end && line.end < to).then_some(line.width..line.width + 1);
        glyphs.iter().map(|g| g.col..g.col + g.width).chain(newline).map(move |cols| (r, cols))
    })
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

    /// Draw a pane's visual lines in its area: highlights, then selections
    /// over them, the caret, and the mode line.
    fn pane(&mut self, pane: &Pane, area: Area, lines: &[Line], focused: bool) {
        let at = |r: usize| area.top + r;
        for (r, line) in lines.iter().enumerate() {
            for g in &line.glyphs {
                self.put(at(r), g.col, &g.shown, g.width, Style::Plain);
            }
        }
        let shown = lines.first().zip(lines.last()).map_or(0..0, |(a, b)| a.start..b.end + 1);
        let faces = pane.layers.iter().filter(|h| h.from < shown.end && h.to > shown.start);
        for (h, face) in faces.filter_map(|h| Some((h, Face::named(&h.face)?))) {
            for (r, cols) in cells(lines, h.from, h.to, false) {
                self.style(at(r), cols, Style::Face(face));
            }
        }
        for (from, to) in pane.selections.iter().filter(|(a, h)| a != h).map(|&(a, h)| (a.min(h), a.max(h))) {
            for (r, cols) in cells(lines, from, to, true) {
                self.style(at(r), cols, Style::Selected);
            }
        }
        if let Some((r, c)) = layout::caret(lines, pane.head()).map(|(r, c)| (at(r), c.min(self.cols - 1))) {
            if focused {
                self.cursor = Some((r, c));
                self.shape = pane.cursor;
            } else {
                self.style(r, c..c + 1, Style::Caret);
            }
        }
        if area.height > 0 {
            let row = area.top + area.text_rows();
            let style = if focused { Style::Status } else { Style::InactiveStatus };
            self.style(row, 0..self.cols, style);
            self.label(row, &pane.status, style);
        }
    }

    /// Show the first line of `text` on a row, as far as it fits.
    fn label(&mut self, row: usize, text: &str, style: Style) {
        let text = Rope::from_str(text);
        let line = layout::wrap(&text, Segment { start: 0, end: text.len_bytes() }, self.cols).swap_remove(0);
        for g in &line.glyphs {
            self.put(row, g.col, &g.shown, g.width, style);
        }
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

    fn style(&mut self, row: usize, cols: Range<usize>, style: Style) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panes_share_the_rows() {
        let tops = |rows, n| areas(rows, n).iter().map(|a| (a.top, a.height)).collect::<Vec<_>>();
        assert_eq!(tops(21, 1), [(0, 21)]);
        assert_eq!(tops(21, 2), [(0, 11), (11, 10)]);
        assert_eq!(tops(2, 3), [(0, 1), (1, 1), (2, 0)]);
    }
}
