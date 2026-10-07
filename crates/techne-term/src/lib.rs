//! The editor in a terminal (EDITOR.md, section 8): a frontend over the
//! presentation protocol like the window, but in cells.
//!
//! `Term` is the whole frontend except the terminal itself. It decodes the
//! bytes a terminal sends into inputs (`keys`), resolves clicks and
//! scrolling against the frame it drew last, and draws snapshots into a
//! `Grid` of cells (`layout`), which `Grid::paint` turns into the bytes that
//! update a terminal: the panes stacked, each with its mode line, the
//! minibuffer below them while it is open (its input line, then its
//! candidates), and the echo area on the last row. Tests drive it with bytes, headless; `main.rs`
//! connects it to a real terminal, and to a runtime through `host`.
//!
//! The terminal is asked whether it has the kitty keyboard protocol, then
//! for its device attributes; once that answer is in, the bound keys this
//! terminal cannot send are reported to the session.

pub mod keys;
pub mod layout;

use std::{fmt::Write, ops::Range, time::Instant};

use techne_editor::{
    present::{CursorShape, Input, Minibuffer, Output, Pane, Place, Run, Snapshot},
    segment::Segment,
};
use techne_text::ropey::Rope;

use crate::{
    keys::{Decoder, Event, KITTY_FLAGS, Mouse, MouseKind, Protocol},
    layout::Line,
};

/// Written when starting: the alternate screen, no automatic wrapping,
/// mouse buttons and drags in SGR encoding, focus reports, then the
/// queries for the kitty keyboard protocol and the device attributes.
pub const SETUP: &str = "\x1b[?1049h\x1b[?7l\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?1004h\x1b[?u\x1b[c";

/// Written when leaving: undoes `SETUP` and the kitty flags (a terminal
/// without the protocol ignores that), and resets the cursor.
pub const RESTORE: &str = "\x1b[<u\x1b[?1004l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[?7h\x1b[0 q\x1b[?25h\x1b[?1049l";

/// Lines the mouse wheel scrolls, as in the window.
const WHEEL_LINES: i64 = 3;

/// Where a pane is on the screen: `height` rows from `top`, its text in all
/// but the last, which is its mode line; `width` cells from `left`, the
/// last a divider when another pane is to its right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub top: usize,
    pub height: usize,
    pub left: usize,
    pub width: usize,
    pub divider: bool,
}

impl Area {
    pub fn text_rows(&self) -> usize {
        self.height.saturating_sub(1)
    }

    pub fn text_cols(&self) -> usize {
        self.width.saturating_sub(usize::from(self.divider)).max(1)
    }

    fn contains(&self, row: usize, col: usize) -> bool {
        (self.top..self.top + self.height).contains(&row) && (self.left..self.left + self.width).contains(&col)
    }
}

/// The panes' places realized in `rows` by `cols` cells: edges rounded to
/// the nearest cell, so neighbours share them.
pub fn areas(rows: usize, cols: usize, places: &[Place]) -> Vec<Area> {
    let at = |f: f32, n: usize| ((f * n as f32).round() as usize).min(n);
    places
        .iter()
        .map(|p| {
            let (top, bottom) = (at(p.y, rows), at(p.y + p.h, rows));
            let (left, right) = (at(p.x, cols), at(p.x + p.w, cols));
            Area { top, height: bottom - top, left, width: right - left, divider: right < cols }
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
    /// Reads the system clipboard, when the terminal gets the focus: what
    /// another program put there becomes the newest kill. Kills go to the
    /// clipboard through the terminal (OSC 52), which cannot be read back.
    clipboard: Option<Box<dyn FnMut() -> Option<String>>>,
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
            clipboard: None,
        }
    }

    /// How to read the system clipboard when the terminal gets the focus.
    pub fn read_clipboard_with(&mut self, read: impl FnMut() -> Option<String> + 'static) {
        self.clipboard = Some(Box::new(read));
    }

    /// A new terminal size. The scroll anchors stay, as in the window.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        (self.cols, self.rows) = (cols.max(1), rows.max(2));
    }

    /// The latest snapshot.
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snap.as_ref()
    }

    /// Where the latest snapshot's panes are drawn, above the minibuffer
    /// and the echo area.
    pub fn areas(&self) -> Vec<Area> {
        let places: Vec<Place> = self.snap.as_ref().map_or(Vec::new(), |s| s.panes.iter().map(|p| p.place).collect());
        areas(self.rows - 1 - self.minibuffer_rows(), self.cols, &places)
    }

    /// The rows of the open minibuffer: its input line and candidates, as
    /// many as fit above the echo area.
    fn minibuffer_rows(&self) -> usize {
        let m = self.snap.as_ref().and_then(|s| s.minibuffer.as_ref());
        m.map_or(0, |m| (1 + m.rows.len()).min(self.rows - 1))
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
            Event::FocusIn => self.clipboard.as_mut().and_then(|read| read()).map(|text| Input::Clipboard { text }),
            Event::FocusOut => None,
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
                let shown = self.shown.iter().find(|s| {
                    let a = s.area;
                    (a.top..a.top + a.text_rows()).contains(&m.row) && (a.left..a.left + a.text_cols()).contains(&m.col)
                })?;
                self.dragging = Some(shown.view);
                self.click(shown, m, m.shift)
            }
            MouseKind::Drag => self.click(self.shown.iter().find(|s| Some(s.view) == self.dragging)?, m, true),
            MouseKind::Release => {
                self.dragging = None;
                None
            }
            MouseKind::WheelUp => self.wheel(m.row, m.col, -WHEEL_LINES),
            MouseKind::WheelDown => self.wheel(m.row, m.col, WHEEL_LINES),
            _ => None,
        }
    }

    /// A click on a pane as shown, as a position in its revision. Dragged
    /// out of the pane, it is on the pane's nearest row.
    fn click(&self, shown: &Shown, m: Mouse, extend: bool) -> Option<Input> {
        let last = shown.area.text_rows().checked_sub(1)?;
        let row = m.row.saturating_sub(shown.area.top).min(last);
        let pos = layout::hit(&shown.lines, m.col.saturating_sub(shown.area.left), row)?;
        Some(Input::Click { view: shown.view, revision: shown.revision, pos, extend, at: Instant::now() })
    }

    /// The wheel over a cell: scrolls the pane there.
    fn wheel(&mut self, row: usize, col: usize, lines: i64) -> Option<Input> {
        let shown = self.shown.iter().find(|s| s.area.contains(row, col))?;
        let (view, cols) = (shown.view, shown.area.text_cols());
        let s = self.snap.as_ref()?;
        let i = s.panes.iter().position(|p| p.view == view)?;
        let pane = &s.panes[i];
        self.anchors[i] = layout::scroll_lines(&pane.text, self.anchors[i], lines, cols);
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
            // The terminal puts it on the clipboard (OSC 52).
            Output::Clipboard(text) => {
                write!(self.replies, "\x1b]52;c;{}\x07", base64(text.as_bytes())).expect("a string");
                Vec::new()
            }
            Output::Session(_) | Output::Quit => Vec::new(),
        }
    }

    fn keep_caret_visible(&mut self, i: usize) -> Option<Input> {
        let area = *self.areas().get(i)?;
        let rows = area.text_rows();
        let p = &self.snap.as_ref()?.panes[i];
        if rows == 0 {
            return None;
        }
        self.anchors[i] = layout::keep_visible(&p.text, self.anchors[i], p.head(), rows, area.text_cols())?;
        Some(Input::Scroll { view: p.view, revision: p.revision, anchor: self.anchors[i] })
    }

    /// Lay out and draw the latest snapshot; clicks are on it from now on.
    pub fn draw(&mut self) -> Grid {
        let mut grid = Grid::new(self.cols, self.rows);
        let areas = self.areas();
        let mb_rows = self.minibuffer_rows();
        let Some(s) = &self.snap else { return grid };
        self.shown = s
            .panes
            .iter()
            .zip(areas)
            .zip(&self.anchors)
            .enumerate()
            .map(|(i, ((pane, area), &anchor))| {
                let lines = layout::frame(&pane.text, anchor, area.text_rows(), area.text_cols());
                grid.pane(pane, area, &lines, i == s.focus, i == s.focus && s.minibuffer.is_none());
                Shown { view: pane.view, revision: pane.revision, area, lines }
            })
            .collect();
        if let Some(m) = &s.minibuffer {
            grid.minibuffer(m, self.rows - 1 - mb_rows, mb_rows);
        }
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

/// Standard base64, for OSC 52.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    bytes
        .chunks(3)
        .flat_map(|c| {
            let n = c.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
            (0..4).map(move |i| if i <= c.len() { ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' })
        })
        .collect()
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
    /// What a minibuffer candidate matched.
    Match,
    /// A key binding, shown beside a command.
    Key,
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
            "match" => Face::Match,
            "key" => Face::Key,
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
            Style::Face(Face::Match) => "\x1b[0;1;36m",
            Style::Face(Face::Key) => "\x1b[0;36m",
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
    /// over them, the caret, and the mode line. The terminal's cursor is
    /// the caret of the pane that has the keys (`cursor`).
    fn pane(&mut self, pane: &Pane, area: Area, lines: &[Line], focused: bool, cursor: bool) {
        let at = |r: usize| area.top + r;
        let x = |cols: Range<usize>| area.left + cols.start..area.left + cols.end;
        for (r, line) in lines.iter().enumerate() {
            for g in &line.glyphs {
                self.put(at(r), area.left + g.col, &g.shown, g.width, Style::Plain);
            }
        }
        if area.divider {
            for r in area.top..area.top + area.text_rows() {
                self.put(r, area.left + area.width - 1, "│", 1, Style::InactiveStatus);
            }
        }
        let shown = lines.first().zip(lines.last()).map_or(0..0, |(a, b)| a.start..b.end + 1);
        let faces = pane.layers.iter().filter(|h| h.from < shown.end && h.to > shown.start);
        for (h, face) in faces.filter_map(|h| Some((h, Face::named(&h.face)?))) {
            for (r, cols) in cells(lines, h.from, h.to, false) {
                self.style(at(r), x(cols), Style::Face(face));
            }
        }
        for (from, to) in pane.selections.iter().filter(|(a, h)| a != h).map(|&(a, h)| (a.min(h), a.max(h))) {
            for (r, cols) in cells(lines, from, to, true) {
                self.style(at(r), x(cols), Style::Selected);
            }
        }
        let last_col = area.left + area.text_cols() - 1;
        if let Some((r, c)) = layout::caret(lines, pane.head()).map(|(r, c)| (at(r), (area.left + c).min(last_col))) {
            if cursor {
                self.cursor = Some((r, c));
                self.shape = pane.cursor;
            } else {
                self.style(r, c..c + 1, Style::Caret);
            }
        }
        if area.height > 0 {
            let row = area.top + area.text_rows();
            let style = if focused { Style::Status } else { Style::InactiveStatus };
            self.style(row, area.left..area.left + area.width, style);
            self.text_in(row, area.left, area.left + area.width, &pane.status, style);
        }
    }

    /// The minibuffer in `height` rows from `top`: the prompt and input,
    /// the cursor in it, then the candidates in aligned columns, the
    /// selected one marked.
    fn minibuffer(&mut self, m: &Minibuffer, top: usize, height: usize) {
        let after = self.text(top, 0, &m.prompt, Style::Plain);
        let input = Rope::from_str(&m.input);
        let line = layout::wrap(&input, Segment { start: 0, end: input.len_bytes() }, usize::MAX).swap_remove(0);
        for g in &line.glyphs {
            self.put(top, after + g.col, &g.shown, g.width, Style::Plain);
        }
        let caret = layout::caret(std::slice::from_ref(&line), m.caret).map_or(line.width, |(_, c)| c);
        (self.cursor, self.shape) = (Some((top, (after + caret).min(self.cols - 1))), CursorShape::Bar);
        let rows = &m.rows[..m.rows.len().min(height.saturating_sub(1))];
        let width = |runs: &[Run]| runs.iter().map(|r| layout::width(&r.text)).sum::<usize>();
        let columns = rows.iter().map(|r| r.columns.len()).max().unwrap_or(0);
        let stops: Vec<usize> = (0..columns)
            .scan(0, |at, c| {
                let stop = *at;
                *at += rows.iter().filter_map(|r| r.columns.get(c)).map(|runs| width(runs)).max().unwrap_or(0) + 2;
                Some(stop)
            })
            .collect();
        for (i, row) in rows.iter().enumerate() {
            let (r, chosen) = (top + 1 + i, m.selected == Some(i));
            if chosen {
                self.style(r, 0..self.cols, Style::Selected);
            }
            for (runs, &stop) in row.columns.iter().zip(&stops) {
                runs.iter().fold(stop, |col, run| {
                    let face = run.face.as_deref().and_then(Face::named).map(Style::Face);
                    self.text(r, col, &run.text, if chosen { Style::Selected } else { face.unwrap_or(Style::Plain) })
                });
            }
        }
    }

    /// Show the first line of `text` on a row, as far as it fits.
    fn label(&mut self, row: usize, text: &str, style: Style) {
        self.text(row, 0, text, style);
    }

    /// Show the first line of `text` on a row from `left`, as far as it
    /// fits before `right`.
    fn text_in(&mut self, row: usize, left: usize, right: usize, text: &str, style: Style) {
        let text = Rope::from_str(text);
        let line = layout::wrap(&text, Segment { start: 0, end: text.len_bytes() }, usize::MAX).swap_remove(0);
        for g in line.glyphs.iter().filter(|g| left + g.col + g.width <= right) {
            self.put(row, left + g.col, &g.shown, g.width, style);
        }
    }

    /// Show the first line of `text` on a row from a cell on, as far as it
    /// fits; the cell after it.
    fn text(&mut self, row: usize, col: usize, text: &str, style: Style) -> usize {
        let text = Rope::from_str(text);
        let line = layout::wrap(&text, Segment { start: 0, end: text.len_bytes() }, usize::MAX).swap_remove(0);
        for g in &line.glyphs {
            self.put(row, col + g.col, &g.shown, g.width, style);
        }
        col + line.width
    }

    /// Show `text`, `width` cells wide, from a cell on; clipped when it does
    /// not fit.
    fn put(&mut self, row: usize, col: usize, text: &str, width: usize, style: Style) {
        if col.saturating_add(width) > self.cols {
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
    fn places_become_cells() {
        let place = |x, y, w, h| Place { x, y, w, h };
        // Two panes side by side over one below: the left one has the
        // divider, edges are shared.
        let a = areas(21, 80, &[place(0.0, 0.0, 0.5, 0.5), place(0.5, 0.0, 0.5, 0.5), place(0.0, 0.5, 1.0, 0.5)]);
        let cells = |a: &Area| (a.top, a.height, a.left, a.width, a.divider);
        assert_eq!(a.iter().map(cells).collect::<Vec<_>>(), [(0, 11, 0, 40, true), (0, 11, 40, 40, false), (11, 10, 0, 80, false)]);
        assert_eq!((a[0].text_cols(), a[1].text_cols()), (39, 40));
    }
}
