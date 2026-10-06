//! The terminal frontend's state: the snapshot shown, the scroll anchor and
//! the layout it was drawn with. Terminal events become inputs for the
//! runtime; a click is resolved against what was drawn. Independent of the
//! terminal backend, so tests draw into ratatui's test backend.

use ratatui::{
    Frame,
    crossterm::event::{Event, KeyModifiers, MouseButton, MouseEventKind},
    style::{Color, Modifier, Style},
};
use techne_editor::{
    display::{self, Placed},
    present::{CursorShape, Input, Snapshot},
};
use techne_text::motion;

use crate::{
    cells::{self, Cells},
    keys,
};

pub struct Term {
    snap: Option<Snapshot>,
    anchor: usize,
    cols: u16,
    rows: u16,
    /// What the last frame showed, for resolving clicks against.
    placed: Vec<Placed>,
}

const SELECTION: Style = Style::new().bg(Color::DarkGray);

impl Term {
    pub fn new(cols: u16, rows: u16) -> Term {
        Term { snap: None, anchor: 0, cols, rows, placed: Vec::new() }
    }

    fn cells(&self) -> Cells {
        Cells::new(self.cols as usize)
    }

    /// Rows for text, above the status line.
    fn text_rows(&self) -> usize {
        self.rows.saturating_sub(1).max(1) as usize
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snap.as_ref()
    }

    pub fn cursor_shape(&self) -> CursorShape {
        self.snap.as_ref().map_or(CursorShape::Bar, |s| s.cursor)
    }

    /// Show a new snapshot. When it answers input that moved the caret off
    /// screen, scroll to it and tell the runtime.
    pub fn show(&mut self, s: Snapshot) -> Vec<Input> {
        let moved = self.snap.as_ref().is_none_or(|old| old.head() != s.head() || old.revision != s.revision);
        let answers = !s.answers.is_empty();
        self.anchor = s.scroll;
        self.snap = Some(s);
        let mut cells = self.cells();
        let fit = self.text_rows();
        let s = self.snap.as_ref().expect("just set");
        match display::keep_visible(&mut cells, &s.text, self.anchor, s.head(), fit) {
            Some(a) if answers && moved => {
                self.anchor = a;
                vec![Input::Scroll { revision: s.revision, anchor: a }]
            }
            _ => Vec::new(),
        }
    }

    /// Inputs for a terminal event; resizing only changes the layout.
    pub fn event(&mut self, ev: &Event) -> Vec<Input> {
        let at = std::time::Instant::now();
        match ev {
            Event::Key(k) => keys::key_name(k).map(|key| Input::Key { key, at }).into_iter().collect(),
            Event::Resize(c, r) => {
                (self.cols, self.rows) = (*c, *r);
                Vec::new()
            }
            Event::Mouse(m) => {
                let Some(s) = &self.snap else { return Vec::new() };
                match m.kind {
                    MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                        let extend = matches!(m.kind, MouseEventKind::Drag(_)) || m.modifiers.contains(KeyModifiers::SHIFT);
                        self.hit(m.column, m.row).map(|pos| Input::Click { revision: s.revision, pos, extend, at }).into_iter().collect()
                    }
                    MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                        let n = if m.kind == MouseEventKind::ScrollDown { 3 } else { -3 };
                        let mut cells = self.cells();
                        self.anchor = display::scroll_lines(&mut cells, &s.text, self.anchor, n);
                        vec![Input::Scroll { revision: s.revision, anchor: self.anchor }]
                    }
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    /// The text position drawn at a cell, if it is in the text area.
    pub fn hit(&self, col: u16, row: u16) -> Option<usize> {
        let s = self.snap.as_ref()?;
        if row as usize >= self.text_rows() {
            return None;
        }
        let row = row as i64;
        let p = self.placed.iter().find(|p| row < p.row + p.rows as i64).or(self.placed.last())?;
        let rows = self.cells().rows(&s.text, p.seg);
        Some(cells::position_at(&rows, (row - p.row).max(0) as usize, col as usize, p.seg.end))
    }

    /// Where `pos` is on screen, if it is shown.
    pub fn cell_of(&self, pos: usize) -> Option<(u16, u16)> {
        let s = self.snap.as_ref()?;
        let p = self.placed.iter().rev().find(|p| p.seg.start <= pos && pos <= p.seg.end)?;
        let (r, c) = cells::locate(&self.cells().rows(&s.text, p.seg), pos);
        let y = p.row + r as i64;
        (0..self.text_rows() as i64).contains(&y).then_some((c as u16, y as u16))
    }

    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        (self.cols, self.rows) = (area.width, area.height);
        let Some(s) = &self.snap else { return };
        let cells = self.cells();
        let text_rows = self.text_rows() as i64;
        self.placed = display::frame(&mut self.cells(), &s.text, self.anchor, text_rows as usize);
        let sel: Vec<(usize, usize)> = s.selections.iter().filter(|(a, h)| a != h).map(|&(a, h)| (a.min(h), a.max(h))).collect();
        let selected = |pos: usize| sel.iter().any(|&(a, b)| a <= pos && pos < b);
        let buf = f.buffer_mut();
        for p in &self.placed {
            let rows = cells.rows(&s.text, p.seg);
            let continues = p.seg.end < motion::line_end(&s.text, p.seg.start);
            for (i, row) in rows.iter().enumerate() {
                let y = p.row + i as i64;
                if !(0..text_rows).contains(&y) {
                    continue;
                }
                let y = y as u16;
                for g in row {
                    let style = if selected(g.start) { SELECTION } else { Style::new() };
                    buf.set_stringn(g.col as u16, y, &g.show, g.width, style);
                }
                let last = i + 1 == rows.len();
                if !last || continues {
                    buf.set_stringn(cells.width as u16, y, "\\", 1, Style::new().fg(Color::DarkGray));
                } else if selected(p.seg.end) && p.seg.end < s.text.len_bytes() {
                    // A selected line break.
                    let x = row.last().map_or(0, |g| g.col + g.width) as u16;
                    buf.set_stringn(x, y, " ", 1, SELECTION);
                }
            }
        }
        let status_y = area.height.saturating_sub(1);
        let status = Style::new().add_modifier(Modifier::REVERSED);
        buf.set_stringn(0, status_y, " ".repeat(area.width as usize), area.width as usize, status);
        buf.set_stringn(0, status_y, &s.status, area.width as usize, status);
        if let Some(c) = self.cell_of(s.head()) {
            f.set_cursor_position(c);
        }
    }
}
