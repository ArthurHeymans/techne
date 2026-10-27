//! The editor in a terminal (EDITOR.md, section 8): a frontend over the
//! presentation protocol like the window, but in cells.
//!
//! `Term` is the whole frontend except the terminal itself. It decodes the
//! bytes a terminal sends into inputs (`keys`), resolves clicks and
//! scrolling against the frame it drew last, and draws snapshots into a
//! `Grid` of cells (`layout`), which `Grid::paint` turns into the bytes that
//! update a terminal: the panes stacked, each with its mode line, the
//! minibuffer below them while it is open (its input line, then its
//! candidates), and the echo area on the last row. Tests drive it with
//! bytes, headless; [`cli`] connects it to a real terminal, and to a
//! runtime through `host`.
//!
//! The terminal is asked whether it has the kitty keyboard protocol, then
//! for its device attributes; once that answer is in, the bound keys this
//! terminal cannot send are reported to the session.

pub mod cli;
pub mod keys;
pub mod layout;

use std::{fmt::Write, ops::Range, time::Instant};

use techne_editor::{
    hints,
    present::{
        Color, Completion, CursorShape, Input, KeyHint, Minibuffer, Output, Pane, Place, Recenter, Run, Slant, Snapshot,
        Style as FaceStyle, Underline, ViewRequest, Weight,
    },
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
/// without the protocol ignores that), and resets the cursor's shape
/// and colour.
pub const RESTORE: &str = "\x1b[<u\x1b[?1004l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[?7h\x1b[0 q\x1b]112\x07\x1b[?25h\x1b[?1049l";

/// Lines the mouse wheel scrolls, as in the window.
const WHEEL_LINES: i64 = 3;

/// The completion popup is at least this wide, as Arthur's corfu-min-width.
const COMPLETION_MIN_WIDTH: usize = 15;

/// Where a pane is on the screen: `height` rows from `top`, its text in all
/// but the last, which is its mode line; `width` cells from `left`, the
/// first `gutter` for line numbers, the last a divider when another pane is
/// to its right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub top: usize,
    pub height: usize,
    pub left: usize,
    pub width: usize,
    pub divider: bool,
    pub gutter: usize,
}

impl Area {
    pub fn text_rows(&self) -> usize {
        self.height.saturating_sub(1)
    }

    pub fn text_cols(&self) -> usize {
        self.width.saturating_sub(usize::from(self.divider) + self.gutter).max(1)
    }

    /// The column the text starts at, after the gutter.
    pub fn text_left(&self) -> usize {
        self.left + self.gutter
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
            Area { top, height: bottom - top, left, width: right - left, divider: right < cols, gutter: 0 }
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
    /// How much colour the terminal takes.
    depth: Depth,
    /// The cursor colour last told to the terminal (OSC 12), from the
    /// snapshot's `cursor` face.
    cursor_color: Option<[u8; 3]>,
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
            depth: Depth::from_env(),
            cursor_color: None,
        }
    }

    /// How much colour the terminal takes; for one that is known other
    /// than from the environment.
    pub fn set_depth(&mut self, depth: Depth) {
        self.depth = depth;
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
        let panes = self.snap.as_ref().map_or(&[][..], |s| &s.panes[..]);
        let places: Vec<Place> = panes.iter().map(|p| p.place).collect();
        let areas = areas(self.rows - 1 - self.minibuffer_rows() - self.hint_columns().first().map_or(0, Vec::len), self.cols, &places);
        // A gutter leaves the text at least a cell.
        let gutter = |a: &Area, p: &Pane| Some(p.display.gutter(p.text.len_lines())).filter(|&g| g + usize::from(a.divider) < a.width);
        areas.into_iter().zip(panes).map(|(a, p)| Area { gutter: gutter(&a, p).unwrap_or(0), ..a }).collect()
    }

    /// which-key's columns of the keys shown, as many rows as they need
    /// to fit the width, up to a quarter of the screen.
    fn hint_columns(&self) -> Vec<Vec<&KeyHint>> {
        let Some(s) = &self.snap else { return Vec::new() };
        let max = (self.rows / 4).max(3).min(self.rows.saturating_sub(2 + self.minibuffer_rows()));
        hints::columns(&s.key_hints, max, self.cols, |h| layout::width(&h.key), |h| layout::width(&h.description))
    }

    /// The rows of the open minibuffer: its input line and candidates, as
    /// many as fit above the echo area.
    fn minibuffer_rows(&self) -> usize {
        let m = self.snap.as_ref().and_then(|s| s.minibuffer.as_ref());
        m.map_or(0, |m| (1 + m.rows.len()).min(self.rows - 1))
    }

    /// The runtime keeps an input waiting (`host::BUSY_AFTER`): the echo
    /// area says so until the next snapshot.
    pub fn busy(&mut self) {
        if let Some(s) = &mut self.snap {
            s.echo = techne_editor::host::BUSY.into();
        }
    }

    /// Whether the echo area says the runtime is busy, or there is nothing
    /// to say it in.
    pub fn shows_busy(&self) -> bool {
        self.snap.as_ref().is_none_or(|s| s.echo == techne_editor::host::BUSY)
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
                    (a.top..a.top + a.text_rows()).contains(&m.row) && (a.left..a.text_left() + a.text_cols()).contains(&m.col)
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
        let pos = layout::hit(&shown.lines, m.col.saturating_sub(shown.area.text_left()), row)?;
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
        Some(Input::Scroll { view, revision: pane.revision, anchor: self.anchors[i], caret: None })
    }

    /// Take an output of the runtime. A snapshot that answers input scrolls
    /// panes whose caret it may have moved off screen: the focused pane when
    /// its caret or text changed, a new pane, every pane when there are more
    /// or fewer.
    pub fn output(&mut self, out: Output) -> Vec<Input> {
        match out {
            Output::Snapshot(s) => {
                let cursor = s.style(s.chrome.cursor);
                if let Some(rgb) = cursor.bg.or(cursor.fg).map(|c| c.rgb)
                    && self.cursor_color != Some(rgb)
                {
                    write!(self.replies, "\x1b]12;#{:02x}{:02x}{:02x}\x07", rgb[0], rgb[1], rgb[2]).expect("a string");
                    self.cursor_color = Some(rgb);
                }
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
                let requests: Vec<(usize, ViewRequest)> = s.panes.iter().enumerate().filter_map(|(i, p)| Some((i, p.request?))).collect();
                self.anchors = s.panes.iter().map(|p| p.scroll).collect();
                self.snap = Some(*s);
                let asked = |i: &usize| requests.iter().any(|(j, _)| j == i);
                let mut inputs: Vec<Input> = scroll.into_iter().filter(|i| !asked(i)).filter_map(|i| self.keep_caret_visible(i)).collect();
                inputs.extend(requests.into_iter().filter_map(|(i, r)| self.resolve(i, r)));
                inputs
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
            Output::Session(_) | Output::Quit | Output::Refused(_) => Vec::new(),
        }
    }

    /// A pane's request, resolved with the cells it has: where to scroll
    /// and where its caret goes.
    fn resolve(&mut self, i: usize, request: ViewRequest) -> Option<Input> {
        let area = *self.areas().get(i)?;
        let (rows, cols) = (area.text_rows(), area.text_cols());
        let p = &self.snap.as_ref()?.panes[i];
        let (text, head, anchor) = (&p.text, p.head(), self.anchors[i]);
        if rows == 0 {
            return None;
        }
        let lines = layout::frame(text, anchor, rows, cols);
        let (anchor, caret) = match request {
            ViewRequest::Page { screens, context } => {
                let n = if screens.abs() >= 1.0 { rows.saturating_sub(context).max(1) as f32 * screens } else { rows as f32 * screens };
                let n = n.round() as i64;
                // Nothing further: the end (or the start) is on the screen.
                let at_end = lines.len() < rows || lines.last().is_some_and(|l| l.end >= text.len_bytes() && !l.continued);
                if (n > 0 && at_end) || (n < 0 && layout::scroll_lines(text, anchor, -1, cols) == anchor) {
                    return Some(Input::Edge { view: p.view, end: n > 0 });
                }
                let new = layout::scroll_lines(text, anchor, n, cols);
                // The caret keeps its row and column on the screen.
                let (row, col) = layout::caret(&lines, head).unwrap_or((0, 0));
                let new_lines = layout::frame(text, new, rows, cols);
                (new, layout::hit(&new_lines, col, row.min(new_lines.len().saturating_sub(1))))
            }
            ViewRequest::Recenter(at) => {
                let k = match at {
                    Recenter::Middle => rows / 2,
                    Recenter::Top => 0,
                    Recenter::Bottom => rows - 1,
                };
                (layout::scroll_lines(text, head, -(k as i64), cols), None)
            }
        };
        let (view, revision) = (p.view, p.revision);
        self.anchors[i] = anchor;
        Some(Input::Scroll { view, revision, anchor, caret })
    }

    fn keep_caret_visible(&mut self, i: usize) -> Option<Input> {
        let area = *self.areas().get(i)?;
        let rows = area.text_rows();
        let p = &self.snap.as_ref()?.panes[i];
        if rows == 0 {
            return None;
        }
        self.anchors[i] = layout::keep_visible(&p.text, self.anchors[i], p.head(), rows, area.text_cols())?;
        Some(Input::Scroll { view: p.view, revision: p.revision, anchor: self.anchors[i], caret: None })
    }

    /// Lay out and draw the latest snapshot; clicks are on it from now on.
    pub fn draw(&mut self) -> Grid {
        let mut grid = Grid::new(self.cols, self.rows);
        let areas = self.areas();
        let mb_rows = self.minibuffer_rows();
        let Some(s) = &self.snap else { return grid };
        let blank = s.panes.first().map_or_else(Style::default, |p| Style::realized(&s.style(p.default)));
        grid.on(self.depth, blank);
        self.shown = s
            .panes
            .iter()
            .zip(areas)
            .zip(&self.anchors)
            .enumerate()
            .map(|(i, ((pane, area), &anchor))| {
                let lines = layout::frame(&pane.text, anchor, area.text_rows(), area.text_cols());
                grid.pane(pane, s, area, &lines, i == s.focus, i == s.focus && s.minibuffer.is_none());
                Shown { view: pane.view, revision: pane.revision, area, lines }
            })
            .collect();
        if let Some(c) = &s.completion
            && let Some(shown) = self.shown.iter().find(|p| p.view == c.view)
            && let Some((r, col)) = layout::caret(&shown.lines, c.at)
        {
            grid.completion(c, s, shown.area, r, col);
        }
        if let Some(m) = &s.minibuffer {
            grid.minibuffer(m, s, self.rows - 1 - mb_rows, mb_rows);
        }
        let columns = self.hint_columns();
        let top = self.rows - 1 - mb_rows - columns.first().map_or(0, Vec::len);
        let (key, sep, prefix) =
            (blank.patched(&s.style(s.chrome.key)), blank.patched(&s.style(s.chrome.shadow)), blank.patched(&s.style(s.chrome.keyword)));
        let mut left = 0;
        for column in &columns {
            let keys = column.iter().map(|h| layout::width(&h.key)).max().unwrap_or(0);
            let descriptions = column.iter().map(|h| layout::width(&h.description)).max().unwrap_or(0);
            for (r, h) in column.iter().enumerate() {
                let at = left + keys - layout::width(&h.key);
                let at = grid.text(top + r, at, &h.key, key);
                let after = grid.text(top + r, at, " : ", sep);
                grid.text(top + r, after, &h.description, if h.prefix { prefix } else { blank });
            }
            left += keys + 3 + descriptions + hints::GAP;
        }
        grid.label(self.rows - 1, &s.echo, blank);
        grid
    }

    /// The bytes that bring the terminal up to date: replies to it, and the
    /// changed cells since the last paint.
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

/// How much colour the terminal takes: truecolor, the 256-colour cube,
/// or the sixteen ANSI colours. From `COLORTERM` and `TERM` when the
/// frontend is made; a colour's own ANSI hint is for the last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Depth {
    #[default]
    Truecolor,
    Ansi256,
    Ansi16,
}

impl Depth {
    fn from_env() -> Depth {
        let term = std::env::var("TERM").unwrap_or_default();
        match std::env::var("COLORTERM").as_deref() {
            Ok("truecolor" | "24bit") => Depth::Truecolor,
            // Beyond COLORTERM, names that mean it: a direct-colour
            // terminfo, and the terminals that are truecolor without
            // saying so in as many words.
            _ if term.contains("direct")
                || ["kitty", "alacritty", "foot", "wezterm", "ghostty", "contour"].iter().any(|n| term.contains(n)) =>
            {
                Depth::Truecolor
            }
            _ if term.contains("256color") || term.starts_with("xterm") || term.starts_with("screen") || term.starts_with("tmux") => {
                Depth::Ansi256
            }
            _ if !term.is_empty() => Depth::Ansi16,
            _ => Depth::Truecolor,
        }
    }
}

/// A snapshot's style as a cell is drawn: resolved (every attribute a
/// face left open already taken from what is under it), so a cell's
/// style is the SGR it takes. The plain style is the terminal's own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: Option<(Underline, Option<Color>)>,
    pub strike: bool,
}

impl Style {
    /// A face style itself: what it does not set is plain in it, so this
    /// is for a whole area a single face styles (a mode line), resolved
    /// over a default first when there is one.
    fn realized(style: &FaceStyle) -> Style {
        Style::default().patched(style)
    }

    /// This style with every attribute `over` sets taken from it.
    fn patched(self, over: &FaceStyle) -> Style {
        let (bold, dim) = match over.weight {
            Some(Weight::Bold) => (true, false),
            Some(Weight::Light) => (false, true),
            Some(Weight::Normal) => (false, false),
            None => (self.bold, self.dim),
        };
        Style {
            fg: over.fg.or(self.fg),
            bg: over.bg.or(self.bg),
            bold,
            dim,
            italic: match over.slant {
                Some(Slant::Italic) => true,
                Some(Slant::Normal) => false,
                None => self.italic,
            },
            underline: match over.underline {
                Some((Underline::None, _)) => None,
                Some(u) => Some(u),
                None => self.underline,
            },
            strike: over.strike.unwrap_or(self.strike),
        }
    }

    /// The SGR sequence selecting this style on a terminal of `depth`,
    /// from a reset.
    fn sgr(self, depth: Depth) -> String {
        let mut out = String::from("\x1b[0");
        let mut push = |s: &str| {
            out.push(';');
            out.push_str(s);
        };
        if self.bold {
            push("1");
        }
        if self.dim {
            push("2");
        }
        if self.italic {
            push("3");
        }
        if let Some((kind, color)) = self.underline {
            // At sixteen colours, only the plain underline: colon
            // sub-parameters are the first thing such a terminal drops.
            if kind != Underline::None {
                push(match (depth, kind) {
                    (Depth::Ansi16, _) | (_, Underline::Line) => "4",
                    (_, Underline::Double) => "4:2",
                    (_, Underline::Curl) => "4:3",
                    (_, Underline::Dotted) => "4:4",
                    (_, Underline::Dashed) => "4:5",
                    (_, Underline::None) => "4",
                });
            }
            if let Some(color) = color
                && kind != Underline::None
                && let Some(s) = color_param(depth, color, 58)
            {
                push(&s);
            }
        }
        if self.strike {
            push("9");
        }
        for (color, base) in [(self.fg, 38), (self.bg, 48)] {
            if let Some(s) = color.and_then(|c| color_param(depth, c, base)) {
                push(&s);
            }
        }
        out.push('m');
        out
    }
}

/// A colour as SGR parameters under `base` (38 foreground, 48
/// background, 58 underline), at a terminal's depth; none for an
/// underline colour at sixteen colours, which have nowhere to put it.
/// The underline colour takes the colon form: a terminal that does not
/// know 58 drops one parameter, where the semicolon form's numbers
/// would be read as attributes of their own (2 is dim, 5 blink, 0 a
/// reset undoing what the sequence already set).
fn color_param(depth: Depth, color: Color, base: u8) -> Option<String> {
    let [r, g, b] = color.rgb;
    if base == 58 {
        return match depth {
            Depth::Truecolor => Some(format!("58:2::{r}:{g}:{b}")),
            Depth::Ansi256 => Some(format!("58:5:{}", cube(color.rgb))),
            Depth::Ansi16 => None,
        };
    }
    Some(match depth {
        Depth::Truecolor => format!("{base};2;{r};{g};{b}"),
        Depth::Ansi256 => format!("{base};5;{}", cube(color.rgb)),
        Depth::Ansi16 => {
            let n = color.ansi.unwrap_or_else(|| nearest16(color.rgb));
            let start = match (base, n < 8) {
                (38, true) => 30,
                (38, false) => 90,
                (_, true) => 40,
                (_, false) => 100,
            };
            format!("{}", start + n % 8)
        }
    })
}

/// The nearest of the 256-colour cube and grey ramp.
fn cube([r, g, b]: [u8; 3]) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    // The nearest level: the steps are not even, the first midpoint is
    // 47, not half a step of 40.
    let level = |c: u8| LEVELS.iter().enumerate().min_by_key(|&(_, &l)| c.abs_diff(l)).map_or(0, |(i, _)| i as u8);
    let (lr, lg, lb) = (level(r), level(g), level(b));
    let as_cube = 16 + 36 * lr + 6 * lg + lb;
    let grey = ((r as u16 + g as u16 + b as u16) / 3).clamp(8, 248);
    let as_grey = 232 + ((grey - 8 + 5) / 10).min(23) as u8;
    let dist = |a: [u8; 3]| (0..3).map(|i| (a[i] as i32 - [r, g, b][i] as i32).pow(2)).sum::<i32>();
    let from_cube = [LEVELS[lr as usize], LEVELS[lg as usize], LEVELS[lb as usize]];
    let from_grey = [8 + (as_grey - 232) * 10; 3];
    if dist(from_grey) < dist(from_cube) { as_grey } else { as_cube }
}

/// The nearest of the sixteen ANSI colours, as terminals usually have
/// them.
fn nearest16(rgb: [u8; 3]) -> u8 {
    const ANSI: [[u8; 3]; 16] = [
        [0, 0, 0],
        [170, 0, 0],
        [0, 170, 0],
        [170, 85, 0],
        [0, 0, 170],
        [170, 0, 170],
        [0, 170, 170],
        [170, 170, 170],
        [85, 85, 85],
        [255, 85, 85],
        [85, 255, 85],
        [255, 255, 85],
        [85, 85, 255],
        [255, 85, 255],
        [85, 255, 255],
        [255, 255, 255],
    ];
    ANSI.iter()
        .enumerate()
        .min_by_key(|(_, a)| (0..3).map(|i| (a[i] as i32 - rgb[i] as i32).pow(2)).sum::<i32>())
        .map_or(7, |(i, _)| i as u8)
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
    depth: Depth,
    /// The style of a blank cell: the snapshot's default face, whose
    /// background the screen takes; plain, the terminal's own, until a
    /// snapshot is drawn.
    blank: Style,
}

impl Grid {
    fn new(cols: usize, rows: usize) -> Grid {
        let blank = Cell { text: " ".into(), style: Style::default() };
        Grid {
            cols,
            rows,
            cells: vec![blank; cols * rows],
            cursor: None,
            shape: CursorShape::Bar,
            depth: Depth::default(),
            blank: Style::default(),
        }
    }

    /// Draw at `depth`, on `blank` (the snapshot's default style).
    fn on(&mut self, depth: Depth, blank: Style) {
        self.depth = depth;
        self.blank = blank;
        for c in &mut self.cells {
            c.style = blank;
        }
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

    /// Draw a pane's visual lines in its area: the gutter, highlights,
    /// then selections over them, the caret, and the mode line. The
    /// terminal's cursor is the caret of the pane that has the keys
    /// (`cursor`).
    fn pane(&mut self, pane: &Pane, snap: &Snapshot, area: Area, lines: &[Line], focused: bool, cursor: bool) {
        let at = |r: usize| area.top + r;
        let left = area.text_left();
        let x = |cols: Range<usize>| left + cols.start..left + cols.end;
        let default = Style::realized(&snap.style(pane.default));
        for r in area.top..area.top + area.text_rows() {
            self.style(r, area.left..area.left + area.width, default);
        }
        for (r, line) in lines.iter().enumerate() {
            for g in &line.glyphs {
                self.put(at(r), left + g.col, &g.shown, g.width, default);
            }
        }
        self.gutter(pane, snap, area, lines);
        if area.divider {
            let border = default.patched(&snap.style(snap.chrome.border));
            for r in area.top..area.top + area.text_rows() {
                self.put(r, area.left + area.width - 1, "│", 1, border);
            }
        }
        let shown = lines.first().zip(lines.last()).map_or(0..0, |(a, b)| a.start..b.end + 1);
        for h in pane.layers.iter().filter(|h| h.from < shown.end && h.to > shown.start) {
            let style = snap.style(h.style);
            let extend = style.extend == Some(true) && style.bg.is_some();
            for (r, cols) in cells(lines, h.from, h.to, extend) {
                let cols = if extend && cols.end > lines[r].width { cols.start..area.text_cols() } else { cols };
                self.patch(at(r), x(cols), &style);
            }
        }
        let region = snap.style(snap.chrome.region);
        for (from, to) in pane.selections.iter().filter(|(a, h)| a != h).map(|&(a, h)| (a.min(h), a.max(h))) {
            for (r, cols) in cells(lines, from, to, true) {
                self.patch(at(r), x(cols), &region);
            }
        }
        let last_col = left + area.text_cols() - 1;
        if let Some((r, c)) = layout::caret(lines, pane.head()).map(|(r, c)| (at(r), (left + c).min(last_col))) {
            if cursor {
                self.cursor = Some((r, c));
                self.shape = pane.cursor;
            } else {
                self.patch(r, c..c + 1, &snap.style(snap.chrome.cursor_inactive));
            }
        }
        if area.height > 0 {
            let row = area.top + area.text_rows();
            let line = if focused { snap.chrome.mode_line } else { snap.chrome.mode_line_inactive };
            let style = Style::realized(&snap.style(pane.default).patch(&snap.style(line)));
            self.style(row, area.left..area.left + area.width, style);
            self.text_in(row, area.left, area.left + area.width, &pane.status, style);
        }
    }

    /// A pane's line numbers, on the first visual line of each of its
    /// lines, and the end-of-buffer marker on the rows past its text.
    fn gutter(&mut self, pane: &Pane, snap: &Snapshot, area: Area, lines: &[Line]) {
        let text = &pane.text;
        let current = text.byte_to_line(pane.head());
        let default = Style::realized(&snap.style(pane.default));
        if area.gutter > 0 {
            let (number, this) =
                (default.patched(&snap.style(snap.chrome.line_number)), default.patched(&snap.style(snap.chrome.line_number_current)));
            for (r, line) in lines.iter().enumerate() {
                let n = text.byte_to_line(line.start);
                if text.line_to_byte(n) == line.start {
                    self.text(
                        area.top + r,
                        area.left,
                        &pane.display.number(n, current, area.gutter),
                        if n == current { this } else { number },
                    );
                }
            }
        }
        if let Some(marker) = &pane.display.eob_marker {
            let shadow = default.patched(&snap.style(snap.chrome.shadow));
            for r in lines.len()..area.text_rows() {
                self.text_in(area.top + r, area.left, area.text_left() + area.text_cols(), marker, shadow);
            }
        }
    }

    /// In-buffer completion's popup over a pane, its candidates in aligned
    /// columns a cell in from its edges: below visual line `row` of the
    /// pane's text, from column `col` (where the completed text starts),
    /// or above the line when there is more room there; moved left to fit.
    fn completion(&mut self, c: &Completion, snap: &Snapshot, area: Area, row: usize, col: usize) {
        let width = |runs: &[Run]| runs.iter().map(|r| layout::width(&r.text)).sum::<usize>();
        let columns = c.rows.iter().map(|r| r.columns.len()).max().unwrap_or(0);
        let stops: Vec<usize> = (0..columns)
            .scan(1, |at, k| {
                let stop = *at;
                *at += c.rows.iter().filter_map(|r| r.columns.get(k)).map(|runs| width(runs)).max().unwrap_or(0) + 2;
                Some(stop)
            })
            .collect();
        let last = c.rows.iter().filter_map(|r| Some(stops[r.columns.len().checked_sub(1)?] + width(r.columns.last()?))).max().unwrap_or(0);
        let w = (last + 1).max(COMPLETION_MIN_WIDTH).min(self.cols);
        let left = (area.text_left() + col).min(self.cols - w);
        let (below, above) = (area.text_rows().saturating_sub(row + 1), row);
        let n = c.rows.len().min(below.max(above));
        let top = if n <= below || below >= above { area.top + row + 1 } else { area.top + row - n };
        let popup = Style::realized(&snap.style(snap.chrome.completion));
        let chosen = Style::realized(&snap.style(snap.chrome.completion).patch(&snap.style(snap.chrome.completion_selected)));
        for (i, r) in c.rows.iter().take(n).enumerate() {
            let style = if c.selected == Some(i) { chosen } else { popup };
            self.style(top + i, left..left + w, style);
            for (runs, &stop) in r.columns.iter().zip(&stops) {
                let text: String = runs.iter().map(|r| r.text.as_str()).collect();
                self.text_in(top + i, left + stop, left + w - 1, &text, style);
            }
        }
    }

    /// The minibuffer in `height` rows from `top`: the prompt and input,
    /// the cursor in it, then the candidates in aligned columns, the
    /// selected one marked.
    fn minibuffer(&mut self, m: &Minibuffer, snap: &Snapshot, top: usize, height: usize) {
        let blank = self.blank;
        let (prompt, selected) = (blank.patched(&snap.style(snap.chrome.prompt)), blank.patched(&snap.style(snap.chrome.selected)));
        if m.input_selected {
            self.style(top, 0..self.cols, selected);
        }
        let style = if m.input_selected { selected } else { blank };
        let after = self.text(top, 0, &m.prompt, if m.input_selected { selected } else { prompt });
        let input = Rope::from_str(&m.input);
        let line = layout::wrap(&input, Segment { start: 0, end: input.len_bytes() }, usize::MAX).swap_remove(0);
        for g in &line.glyphs {
            self.put(top, after + g.col, &g.shown, g.width, style);
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
                self.style(r, 0..self.cols, selected);
            }
            for (runs, &stop) in row.columns.iter().zip(&stops) {
                runs.iter().fold(stop, |col, run| {
                    let base = run.style.map_or(blank, |id| blank.patched(&snap.style(id)));
                    self.text(r, col, &run.text, if chosen { base.patched(&snap.style(snap.chrome.selected)) } else { base })
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

    /// Patch a face style over the styles of a row's cells.
    fn patch(&mut self, row: usize, cols: Range<usize>, over: &FaceStyle) {
        let cols = cols.start.min(self.cols)..cols.end.min(self.cols);
        for c in &mut self.cells[row * self.cols + cols.start..row * self.cols + cols.end] {
            c.style = c.style.patched(over);
        }
    }

    /// The bytes that draw this grid over `old`, in one synchronized
    /// update. Unchanged ends of rows are left alone; plain blank tails
    /// are erased instead of printing spaces. In particular, a key need
    /// not redraw hundreds of cells on a framebuffer console.
    pub fn paint(&self, old: Option<&Grid>) -> String {
        let same_size = old.filter(|o| (o.cols, o.rows) == (self.cols, self.rows));
        if same_size == Some(self) {
            return String::new();
        }
        let mut out = String::from("\x1b[?2026h\x1b[?25l");
        for r in 0..self.rows {
            let row = self.row(r);
            let (mut first, mut end) = match same_size {
                Some(o) => {
                    let before = o.row(r);
                    let Some(first) = row.iter().zip(before).position(|(a, b)| a != b) else { continue };
                    let end = row.iter().zip(before).rposition(|(a, b)| a != b).expect("a changed cell") + 1;
                    (first, end)
                }
                None => (0, self.cols),
            };
            // A changed continuation cell belongs to the glyph on its
            // left. Never emit half of a wide character or a tab.
            while first > 0 && row[first].text.is_empty() {
                first -= 1;
            }
            while end < self.cols && row[end].text.is_empty() {
                end += 1;
            }
            let tail = row.iter().rposition(|c| c.style != self.blank || c.text != " ").map_or(0, |i| i + 1);
            let printed_end = end.min(tail).max(first);
            write!(out, "\x1b[{};{}H", r + 1, first + 1).expect("a string");
            let mut style = None;
            for c in row[first..printed_end].iter().filter(|c| !c.text.is_empty()) {
                if style != Some(c.style) {
                    out.push_str(&c.style.sgr(self.depth));
                    style = Some(c.style);
                }
                out.push_str(&c.text);
            }
            // The erase that may follow fills with the current style's
            // background (BCE), so end the row in the blank style: the
            // default face's background reaches the row's end, not the
            // terminal's own. For a plain blank this is the reset.
            out.push_str(&self.blank.sgr(self.depth));
            if end > tail {
                out.push_str("\x1b[K");
            }
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

    /// A style with a background, and its ANSI colours.
    fn backed() -> Style {
        Style {
            fg: Some(Color { rgb: [255, 255, 255], ansi: Some(15) }),
            bg: Some(Color { rgb: [0, 0, 170], ansi: Some(4) }),
            ..Style::default()
        }
    }

    #[test]
    fn styles_become_sgr_at_each_depth() {
        let mut style = backed();
        style.bold = true;
        style.italic = true;
        style.underline = Some((Underline::Curl, Some(Color::new(255, 0, 0))));
        assert_eq!(style.sgr(Depth::Truecolor), "\x1b[0;1;3;4:3;58:2::255:0:0;38;2;255;255;255;48;2;0;0;170m");
        assert_eq!(style.sgr(Depth::Ansi256), "\x1b[0;1;3;4:3;58:5:196;38;5;231;48;5;19m");
        assert_eq!(style.sgr(Depth::Ansi16), "\x1b[0;1;3;4;97;44m", "the ANSI hints, a plain underline, no underline colour");
        assert_eq!(cube([255, 0, 0]), 196);
        assert_eq!(cube([21, 255, 0]), 46, "21 is nearer 0 than 95");
        assert_eq!(cube([137, 137, 137]), 245, "the grey ramp, rounded");
        assert_eq!(cube([128, 128, 128]), 244, "the grey ramp");
        assert_eq!(Style::default().sgr(Depth::Ansi256), "\x1b[0m");
    }

    #[test]
    fn painting_only_changed_cells_and_erasing_blank_tails() {
        let mut old = Grid::new(240, 67);
        old.text(0, 0, "hello", Style::default());
        let mut new = old.clone();
        new.text(0, 1, "a", Style::default());
        assert_eq!(new.paint(Some(&old)), "\x1b[?2026h\x1b[?25l\x1b[1;2H\x1b[0ma\x1b[0m\x1b[?2026l");
        assert!(new.paint(Some(&new)).is_empty());

        let blank = Grid::new(240, 67);
        assert!(blank.paint(None).len() < 1500, "empty screens use erase, not 16,080 spaces");
        // On a face's background, the erased tail takes that background
        // (BCE), all the way on a wholly blank screen of it.
        let mut themed = Grid::new(10, 2);
        themed.on(Depth::Ansi16, backed());
        themed.text(0, 0, "hi", backed());
        let painted = themed.paint(None);
        assert!(painted.contains("\x1b[0;97;44mhi\x1b[0;97;44m\x1b[K"), "{painted:?}");
        assert!(painted.contains("\x1b[2;1H\x1b[0;97;44m\x1b[K"), "a blank row: {painted:?}");
        assert_eq!(blank.paint(Some(&old)), "\x1b[?2026h\x1b[?25l\x1b[1;1H\x1b[0m\x1b[K\x1b[?2026l");
        // Blank cells with a background cannot be replaced by a plain erase.
        let mut selected = blank.clone();
        selected.depth = Depth::Ansi16;
        selected.style(0, 0..3, backed());
        assert!(selected.paint(Some(&blank)).contains("\x1b[0;97;44m   \x1b[0m"));
    }

    #[test]
    fn painting_changes_that_touch_wide_glyphs_and_tabs() {
        let mut old = Grid::new(10, 1);
        old.put(0, 0, "名", 2, Style::default());
        let mut new = Grid::new(10, 1);
        new.text(0, 0, "x", Style::default());
        assert!(new.paint(Some(&old)).contains("\x1b[1;1H\x1b[0mx\x1b[0m\x1b[K"));
        // Only the continuation's style changed: redraw the entire glyph.
        new = old.clone();
        new.style(0, 1..2, backed());
        assert!(new.paint(Some(&old)).contains("\x1b[1;1H\x1b[0m名"));
        let mut tab = Grid::new(10, 1);
        tab.put(0, 0, "        ", 8, Style::default());
        assert!(tab.paint(Some(&old)).contains("\x1b[1;1H\x1b[0m        "));
    }

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
