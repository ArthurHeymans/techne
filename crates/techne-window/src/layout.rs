//! Layout of a text snapshot in a window: what the frontend owns (EDITOR.md,
//! sections 6 and 7). Shaping and wrapping happen here and only around what
//! is shown, in display segments (`techne_editor::segment`, which also
//! scrolls by anchor); the runtime gets back semantic positions. Shaped
//! segments are cached by their text, the width they wrap at and the
//! shapes over them, so panes of different widths share the cache.
//!
//! Text is shaped in the shapes its styles ask for: weight, slant, font
//! family and size, from any face over it (a heading's, a theme's
//! variable-pitch prose). A visual line is as tall as the tallest text
//! on it. Geometry (scrolling, keeping the caret visible, clicks) is
//! counted from the same shaping that is drawn, the pane's shape runs
//! set beside its inlays (`set_shape`); text beyond the styled window
//! is counted in the pane's base shape until it is shown.

use std::{collections::HashMap, sync::Arc};

use glyphon::{Attrs, Buffer, Cursor, Family, FontSystem, Metrics, Shaping, Style as FontStyle, Weight as FontWeight, Wrap};
use techne_editor::{
    present::{Inlay, Slant, StyleId, Weight},
    segment::{self, Segment, next_segment, segment_at, snap},
};
use techne_text::{motion, ropey::Rope};

/// The text shaped for a segment: a stray carriage return would start a new
/// line for the shaper, so it is shown as a space (same byte length); the
/// inlays in it are in it, where they are shown.
fn display_text(text: &Rope, seg: Segment, inlays: &[Inlay]) -> (String, Inlays) {
    let plain = text.byte_slice(seg.start..seg.end).to_string().replace('\r', " ");
    let ends_line = seg.end == motion::line_end(text, seg.start);
    let own: Vec<&Inlay> = inlays[inlays.partition_point(|i| i.at < seg.start)..]
        .iter()
        .take_while(|i| i.at < seg.end || (ends_line && i.at == seg.end))
        .collect();
    if own.is_empty() {
        return (plain, Inlays::default());
    }
    let mut shown = String::with_capacity(plain.len() + own.iter().map(|i| i.text.len()).sum::<usize>());
    let mut at = 0;
    let mut map = Vec::new();
    for i in own {
        let rel = i.at - seg.start;
        shown.push_str(&plain[at..rel]);
        // One line: an inlay's own line breaks would be the shaper's.
        let text = i.text.replace(['\n', '\r'], " ");
        shown.push_str(&text);
        map.push((rel, text.len(), i.style));
        at = rel;
    }
    shown.push_str(&plain[at..]);
    (shown, Inlays(map))
}

/// Where a segment's inlays are in its shaped text: each (position in
/// the segment's text, length shown, style), in order. Shaped offsets
/// and the text's differ by the inlays before them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inlays(Vec<(usize, usize, StyleId)>);

impl Inlays {
    /// The shaped offset of the character at `t`: after the inlays there.
    pub fn char_at(&self, t: usize) -> usize {
        t + self.0.iter().filter(|i| i.0 <= t).map(|i| i.1).sum::<usize>()
    }

    /// The shaped offset of the caret at `t`: before the inlays there.
    pub fn caret_at(&self, t: usize) -> usize {
        t + self.0.iter().filter(|i| i.0 < t).map(|i| i.1).sum::<usize>()
    }

    /// The text position a shaped offset is on: in an inlay, where it is.
    pub fn text_at(&self, d: usize) -> usize {
        let mut shift = 0;
        for &(at, len, _) in &self.0 {
            if d < at + shift {
                break;
            }
            if d < at + shift + len {
                return at;
            }
            shift += len;
        }
        d - shift
    }

    /// The shaped ranges of the text from `a` to `b` (in the segment),
    /// the inlays between them left out (all of those at one place).
    fn text_ranges(&self, a: usize, b: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut from = self.char_at(a);
        let mut last = None;
        for &(at, _, _) in self.0.iter().filter(|i| a < i.0 && i.0 < b) {
            if last == Some(at) {
                continue;
            }
            last = Some(at);
            out.push((from, self.caret_at(at)));
            from = self.char_at(at);
        }
        out.push((from, self.caret_at(b).max(from)));
        out
    }

    /// Each inlay's shaped range and style: those at one place one after
    /// another.
    fn shown(&self) -> impl Iterator<Item = (usize, usize, StyleId)> + '_ {
        let mut at_place = (usize::MAX, 0);
        self.0.iter().map(move |&(at, len, style)| {
            if at_place.0 != at {
                at_place = (at, self.caret_at(at));
            }
            let start = at_place.1;
            at_place.1 += len;
            (start, start + len, style)
        })
    }
}

struct Shaped {
    buffer: Buffer,
    /// Byte offsets (in the segment) where its visual lines start.
    starts: Vec<usize>,
    /// Its visual lines, top and height.
    lines: Vec<Line>,
    used: u64,
}

pub struct Buffers<'a>(&'a HashMap<String, Shaped>);

impl<'a> Buffers<'a> {
    pub fn get(&self, key: &str) -> Option<&'a Buffer> {
        self.0.get(key).map(|s| &s.buffer)
    }
}

/// What a stretch of text is shaped in: what changes its glyphs, where
/// colours do not. `family` is a font family, `fixed` for the
/// frontend's own or `variable` for a proportional one, or none for
/// the frontend's own; `height` a percentage of its size.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Shape {
    pub weight: Weight,
    pub slant: Slant,
    pub family: Option<Arc<str>>,
    pub height: u16,
}

impl Default for Shape {
    fn default() -> Shape {
        Shape { weight: Weight::Normal, slant: Slant::Normal, family: None, height: 100 }
    }
}

/// A range shaped in a shape of its own, in text positions.
#[derive(Clone, Debug)]
pub struct ShapeRun {
    pub from: usize,
    pub to: usize,
    pub shape: Shape,
}

/// One stretch of a segment in one shape, in bytes from its start: what
/// `shape` works in, a `ShapeRun` clipped to the segment.
type RunSpan = (usize, usize, Shape);

/// A visual line of a placed segment: its top, from the segment's top,
/// and its height, the tallest text on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub top: f32,
    pub height: f32,
}

/// A segment placed in the frame, `top` pixels from the text area's top.
#[derive(Clone, Debug)]
pub struct Placed {
    pub seg: Segment,
    pub key: String,
    pub top: f32,
    /// Its visual lines: at least one.
    pub lines: Vec<Line>,
    /// The inlays shaped with it.
    pub inlays: Inlays,
}

impl Placed {
    /// Its height: its lines'.
    pub fn height(&self) -> f32 {
        self.lines.last().map_or(0.0, |l| l.top + l.height)
    }
}

fn family<'a>(name: &'a str, own: &'a str) -> Family<'a> {
    match name {
        "fixed" => family(own, own),
        "monospace" => Family::Monospace,
        "variable" | "sans-serif" => Family::SansSerif,
        "serif" => Family::Serif,
        name => Family::Name(name),
    }
}

/// The metrics of text `height` percent of `size`.
fn metrics(size: f32, height: u16) -> Metrics {
    let size = size * height as f32 / 100.0;
    Metrics::new(size, (size * 1.35).round())
}

pub struct Layout {
    pub fonts: FontSystem,
    metrics: Metrics,
    family: String,
    width: f32,
    /// The advance of a character of the text's font (for which-key's
    /// columns, laid out in characters).
    char_width: f32,
    cache: HashMap<String, Shaped>,
    frame: u64,
    /// The inlays of the pane being laid out (`set_inlays`), and the
    /// weight and slant of each one's style.
    inlays: Vec<Inlay>,
    inlay_shapes: Vec<(Weight, Slant)>,
    /// The shape of the pane being laid out (`set_shape`): everywhere,
    /// and where its styles ask for more.
    base: Shape,
    runs: Vec<ShapeRun>,
}

impl Layout {
    pub fn new(font_size: f32, family: &str) -> Layout {
        let mut layout = Layout {
            fonts: FontSystem::new(),
            metrics: metrics(font_size, 100),
            family: family.to_string(),
            width: 0.0,
            char_width: 0.0,
            cache: HashMap::new(),
            frame: 0,
            inlays: Vec::new(),
            inlay_shapes: Vec::new(),
            base: Shape::default(),
            runs: Vec::new(),
        };
        let sample = layout.label(&"0".repeat(20), f32::MAX);
        layout.char_width = sample.layout_runs().map(|r| r.line_w).fold(0.0, f32::max) / 20.0;
        layout
    }

    pub fn char_width(&self) -> f32 {
        self.char_width
    }

    /// The height of a line of the frontend's text, at its size.
    pub fn line_height(&self) -> f32 {
        self.metrics.line_height
    }

    /// The width text wraps at, from now on (each pane sets its own).
    pub fn set_width(&mut self, width: f32) {
        self.width = width;
    }

    /// The inlays shaped in the text, from now on (each pane sets its
    /// own, with its width): geometry counted with them everywhere.
    /// `shape` gives the weight and slant of an inlay's style.
    pub fn set_inlays(&mut self, inlays: &[Inlay], shape: impl Fn(StyleId) -> (Weight, Slant)) {
        self.inlays.clear();
        self.inlays.extend_from_slice(inlays);
        self.inlay_shapes = inlays.iter().map(|i| shape(i.style)).collect();
    }

    /// The shape of the text from now on: `base` everywhere, `runs` (in
    /// text positions, in order) where its styles ask for more. Each
    /// pane sets its own, as its inlays: geometry is counted in the
    /// shapes drawn, so scrolling, the caret and clicks agree with what
    /// is shown. Text no run covers (beyond the styled window) is
    /// counted in `base`, until it is shown with its runs.
    pub fn set_shape(&mut self, base: Shape, runs: Vec<ShapeRun>) {
        self.base = base;
        self.runs = runs;
    }

    /// Start a frame: segments not used since the previous one may go, and
    /// shaped words not used for a few hundred frames.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.fonts.shape_run_cache.trim(600);
        if self.cache.len() > 2000 {
            let frame = self.frame;
            self.cache.retain(|_, s| s.used + 2 >= frame);
        }
    }

    /// The cache key of text shaped at the current width, with the shape
    /// runs that shape it (none, in the plain shape: the key the text has
    /// had all along). Paint attributes are no part of it: a colour never
    /// reshapes.
    fn key(&self, text: &str, base: &Shape, runs: &[RunSpan]) -> String {
        let mut key = format!("{}\u{0}{text}", self.width.round() as u32);
        if *base != Shape::default() || !runs.is_empty() {
            key.push_str(&format!("\u{0}{base:?}"));
            for (from, to, s) in runs {
                key.push_str(&format!(";{from}-{to}:{s:?}"));
            }
        }
        key
    }

    fn shape(&mut self, text: &str, base: &Shape, runs: &[RunSpan]) -> &Shaped {
        let frame = self.frame;
        let key = &self.key(text, base, runs);
        if !self.cache.contains_key(key) {
            let buffer = self.build_buffer(text, base, runs);
            let starts = buffer.layout_runs().map(|r| r.glyphs.iter().map(|g| g.start).min().unwrap_or(0)).collect();
            let mut lines: Vec<Line> = buffer.layout_runs().map(|r| Line { top: r.line_top, height: r.line_height }).collect();
            if lines.is_empty() {
                lines.push(Line { top: 0.0, height: metrics(self.metrics.font_size, base.height).line_height });
            }
            self.cache.insert(key.to_string(), Shaped { buffer, starts, lines, used: frame });
        }
        let s = self.cache.get_mut(key).expect("just inserted");
        s.used = frame;
        s
    }

    /// Shape `text` once, in stretches of one shape.
    fn build_buffer(&mut self, text: &str, base: &Shape, runs: &[RunSpan]) -> Buffer {
        let mut buffer = Buffer::new(&mut self.fonts, metrics(self.metrics.font_size, base.height));
        buffer.set_wrap(Wrap::WordOrGlyph);
        buffer.set_size(Some(self.width.max(1.0)), None);
        // The text in stretches of one shape, the runs' boundaries on
        // character boundaries.
        let floor = |i: usize| (0..=i.min(text.len())).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
        let ceil = |i: usize| (i.min(text.len())..=text.len()).find(|&i| text.is_char_boundary(i)).unwrap_or(text.len());
        let mut spans: Vec<(&str, &Shape)> = Vec::new();
        let mut at = 0;
        for (from, to, s) in runs {
            let (from, to) = (floor(*from), ceil(*to));
            if at < from {
                spans.push((&text[at..from], base));
            }
            if from.max(at) < to {
                spans.push((&text[from.max(at)..to], s));
                at = to;
            }
        }
        if at < text.len() || spans.is_empty() {
            spans.push((&text[at..], base));
        }
        let this = Layout::attrs_of(&self.family, self.metrics.font_size);
        buffer.set_rich_text(spans.iter().map(|(t, s)| (*t, this(s))), &this(base), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fonts, false);
        buffer
    }

    /// `attrs`, without borrowing the whole layout (its font system is
    /// borrowed to shape).
    fn attrs_of<'a>(own: &'a str, size: f32) -> impl Fn(&'a Shape) -> Attrs<'a> {
        move |shape: &'a Shape| {
            let attrs = Attrs::new()
                .family(family(shape.family.as_deref().unwrap_or(own), own))
                .weight(match shape.weight {
                    Weight::Light => FontWeight::LIGHT,
                    Weight::Normal => FontWeight::NORMAL,
                    Weight::Bold => FontWeight::BOLD,
                })
                .style(match shape.slant {
                    Slant::Normal => FontStyle::Normal,
                    Slant::Italic => FontStyle::Italic,
                });
            if shape.height == 100 { attrs } else { attrs.metrics(metrics(size, shape.height)) }
        }
    }

    /// Text shaped on one line in the text's font, as mode lines, the
    /// echo area and the minibuffer are: they line up with the text and
    /// with each other.
    pub fn label(&mut self, text: &str, width: f32) -> Buffer {
        self.label_styled(text, width, Weight::Normal, Slant::Normal)
    }

    /// A label in a weight and slant: chrome whose face asks for them
    /// (the mode line's bold). Its width is its own, so it is measured
    /// from this buffer, not a plain one.
    pub fn label_styled(&mut self, text: &str, width: f32, weight: Weight, slant: Slant) -> Buffer {
        let mut b = Buffer::new(&mut self.fonts, self.metrics);
        b.set_size(Some(width), Some(self.metrics.line_height));
        let shape = Shape { weight, slant, ..Shape::default() };
        let attrs = Layout::attrs_of(&self.family, self.metrics.font_size)(&shape);
        b.set_text(text, &attrs, Shaping::Advanced, None);
        b.shape_until_scroll(&mut self.fonts, false);
        b
    }

    pub fn buffer(&self, key: &str) -> Option<&Buffer> {
        self.cache.get(key).map(|s| &s.buffer)
    }

    /// The font system, and the shaped buffers to draw with it.
    pub fn split(&mut self) -> (&mut FontSystem, Buffers<'_>) {
        (&mut self.fonts, Buffers(&self.cache))
    }

    /// The segment `s` as shaped: its text with its inlays, where they
    /// are, and the runs over it (the text's, around the inlays, and
    /// each inlay in its own style's weight and slant).
    fn segment(&self, text: &Rope, s: Segment) -> (String, Inlays, Vec<RunSpan>) {
        let (shown, map) = display_text(text, s, &self.inlays);
        let first = self.inlays.partition_point(|i| i.at < s.start);
        let mut own: Vec<RunSpan> = self
            .runs
            .iter()
            .filter(|r| r.from < s.end && r.to > s.start)
            .flat_map(|r| {
                map.text_ranges(r.from.max(s.start) - s.start, r.to.min(s.end) - s.start)
                    .into_iter()
                    .filter(|(a, b)| a < b)
                    .map(|(a, b)| (a, b, r.shape.clone()))
            })
            .chain(
                map.shown()
                    .zip(&self.inlay_shapes[first..])
                    .filter(|(_, (w, sl))| (*w, *sl) != (self.base.weight, self.base.slant))
                    .map(|((a, b, _), &(w, sl))| (a, b, Shape { weight: w, slant: sl, ..self.base.clone() })),
            )
            .collect();
        own.sort_by_key(|r| r.0);
        (shown, map, own)
    }

    /// The segment's shaping: its key, its line starts (in the text) and
    /// its lines, and its inlays.
    fn shaped(&mut self, text: &Rope, s: Segment) -> (String, Vec<usize>, Vec<Line>, Inlays) {
        let (shown, map, runs) = self.segment(text, s);
        let base = self.base.clone();
        let key = self.key(&shown, &base, &runs);
        let shaped = self.shape(&shown, &base, &runs);
        let mut starts: Vec<usize> = shaped.starts.iter().map(|&st| s.start + map.text_at(st)).collect();
        if starts.is_empty() {
            starts.push(s.start);
        }
        // A line that starts in an inlay starts at the text it is
        // before: several can start there, and each counts
        // (`segment::scroll_lines`).
        starts[0] = s.start;
        let lines = shaped.lines.clone();
        (key, starts, lines, map)
    }

    /// Where the segment's visual lines start, as positions in the text.
    pub fn visual_starts(&mut self, text: &Rope, seg: Segment) -> Vec<usize> {
        self.shaped(text, seg).1
    }

    /// The segments to show from the scroll anchor down to `height`.
    pub fn frame(&mut self, text: &Rope, anchor: usize, height: f32) -> Vec<Placed> {
        let anchor = snap(text, anchor.min(text.len_bytes()));
        let first = segment_at(text, anchor);
        let (_, starts, lines, _) = self.shaped(text, first);
        let mut y = -lines.get(segment::line_index(&starts, anchor)).map_or(0.0, |l| l.top);
        let mut placed = Vec::new();
        let mut seg = Some(first);
        while let Some(s) = seg {
            if y >= height {
                break;
            }
            let (key, _, lines, inlays) = self.shaped(text, s);
            let p = Placed { seg: s, key, top: y, lines, inlays };
            y += p.height();
            placed.push(p);
            seg = next_segment(text, s);
        }
        placed
    }

    /// The scroll anchor `n` visual lines further down (up when negative).
    pub fn scroll_lines(&mut self, text: &Rope, anchor: usize, n: i64) -> usize {
        segment::scroll_lines(text, anchor, n, &mut |seg| self.visual_starts(text, seg))
    }

    /// A new scroll anchor that shows `head`, if it is off screen: at the
    /// top when it is above, at the bottom when it is below. Lines are
    /// counted at the frontend's height first, then the frame from that
    /// anchor is checked, taller lines (a heading's) scrolling further.
    pub fn keep_visible(&mut self, text: &Rope, anchor: usize, head: usize, height: f32) -> Option<usize> {
        let fit = (height / self.line_height()).floor() as usize;
        let counted = segment::keep_visible(text, anchor, head, fit, &mut |seg| self.visual_starts(text, seg));
        let mut at = counted.unwrap_or(anchor);
        for _ in 0..fit.max(1) {
            let placed = self.frame(text, at, height);
            // Below the frame: not placed at all, or past its bottom.
            let below = match caret(self, &placed, text, head) {
                Some(c) => c.y > 0.0 && c.y + c.h > height + 0.5,
                None => placed.last().is_some_and(|p| head > p.seg.end),
            };
            let next = if below { self.scroll_lines(text, at, 1) } else { at };
            if next == at {
                break;
            }
            at = next;
        }
        (at != anchor).then_some(at)
    }
}

/// Which placed segment shows `pos`; at a piece boundary of a long line the
/// later piece.
pub fn placed_at(placed: &[Placed], pos: usize) -> Option<&Placed> {
    placed.iter().rev().find(|p| p.seg.start <= pos && pos <= p.seg.end)
}

/// The text position at (x, y) in the text area.
pub fn hit(layout: &Layout, placed: &[Placed], x: f32, y: f32) -> Option<usize> {
    let p = placed.iter().find(|p| y < p.top + p.height()).or(placed.last())?;
    let buffer = layout.buffer(&p.key)?;
    let cursor = buffer.hit(x, y - p.top)?;
    Some(p.seg.start + p.inlays.text_at(cursor.index).min(p.seg.end - p.seg.start))
}

/// A rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// The part of this rectangle in `other`, if any.
    pub fn intersect(self, other: Rect) -> Option<Rect> {
        let (x, y) = (self.x.max(other.x), self.y.max(other.y));
        let (right, bottom) = ((self.x + self.w).min(other.x + other.w), (self.y + self.h).min(other.y + other.h));
        (right > x && bottom > y).then_some(Rect { x, y, w: right - x, h: bottom - y })
    }

    /// This rectangle without `hole`: up to four rectangles (the bands
    /// above and below it, and beside it between them).
    pub fn minus(self, hole: Rect) -> Vec<Rect> {
        let Some(h) = self.intersect(hole) else { return vec![self] };
        let (right, bottom) = (self.x + self.w, self.y + self.h);
        [
            Rect { h: h.y - self.y, ..self },
            Rect { y: h.y + h.h, h: bottom - (h.y + h.h), ..self },
            Rect { y: h.y, w: h.x - self.x, h: h.h, ..self },
            Rect { x: h.x + h.w, y: h.y, w: right - (h.x + h.w), h: h.h },
        ]
        .into_iter()
        .filter(|r| r.w > 0.0 && r.h > 0.0)
        .collect()
    }
}

/// The caret at `pos`: its left edge, top, and the width of the character
/// it is on (for a block cursor).
pub fn caret(layout: &Layout, placed: &[Placed], text: &Rope, pos: usize) -> Option<Rect> {
    let p = placed_at(placed, pos)?;
    let buffer = layout.buffer(&p.key)?;
    // Before the inlays at the caret; as wide as the character after them.
    let rel = pos - p.seg.start;
    let idx = p.inlays.caret_at(rel);
    let lh = p.lines.first().map_or(layout.line_height(), |l| l.height);
    let (on, next) = if pos < p.seg.end {
        (p.inlays.char_at(rel), p.inlays.caret_at(motion::next_grapheme(text, pos).min(p.seg.end) - p.seg.start))
    } else {
        (idx, idx)
    };
    for run in buffer.layout_runs() {
        if let Some(x) = run.cursor_position(&Cursor::new(0, idx)) {
            let w = run.highlight(Cursor::new(0, on), Cursor::new(0, next)).map(|(_, w)| w).next().unwrap_or(lh * 0.5);
            return Some(Rect { x, y: p.top + run.line_top, w, h: run.line_height });
        }
    }
    Some(Rect { x: 0.0, y: p.top, w: lh * 0.5, h: lh })
}

/// What covers a range of text on screen: the rectangles over its text,
/// and a narrow box after each line whose break the range holds.
#[derive(Debug, Default)]
pub struct Covered {
    pub text: Vec<Rect>,
    pub breaks: Vec<Rect>,
}

impl Covered {
    /// Every rectangle, the text's and the breaks'.
    pub fn all(&self) -> impl Iterator<Item = &Rect> {
        self.text.iter().chain(&self.breaks)
    }
}

/// What covers the range [from, to).
pub fn selection(layout: &Layout, placed: &[Placed], from: usize, to: usize) -> Covered {
    let mut covered = Covered::default();
    for p in placed.iter().filter(|p| from <= p.seg.end && to > p.seg.start) {
        let Some(buffer) = layout.buffer(&p.key) else { continue };
        let (a, b) = (from.max(p.seg.start) - p.seg.start, to.min(p.seg.end) - p.seg.start);
        let mut last = None;
        for run in buffer.layout_runs() {
            // The text's, not the inlays among it.
            for (a, b) in p.inlays.text_ranges(a, b) {
                for (x, w) in run.highlight(Cursor::new(0, a), Cursor::new(0, b)) {
                    covered.text.push(Rect { x, y: p.top + run.line_top, w, h: run.line_height });
                }
            }
            last = Some((run.line_w, p.top + run.line_top, run.line_height));
        }
        if to > p.seg.end
            && let Some((x, y, h)) = last
        {
            covered.breaks.push(Rect { x, y, w: layout.line_height() * 0.4, h });
        }
    }
    covered
}

/// Where the inlays are drawn, and with which style.
pub fn inlays(layout: &Layout, placed: &[Placed]) -> Vec<(Rect, StyleId)> {
    let mut out = Vec::new();
    for p in placed {
        let Some(buffer) = layout.buffer(&p.key) else { continue };
        for (a, b, style) in p.inlays.shown() {
            for run in buffer.layout_runs() {
                for (x, w) in run.highlight(Cursor::new(0, a), Cursor::new(0, b)) {
                    out.push((Rect { x, y: p.top + run.line_top, w, h: run.line_height }, style));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rectangle_with_a_hole() {
        let r = Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 };
        let parts = r.minus(Rect { x: 2.0, y: 3.0, w: 4.0, h: 20.0 });
        assert_eq!(
            parts,
            [Rect { x: 0.0, y: 0.0, w: 10.0, h: 3.0 }, Rect { x: 0.0, y: 3.0, w: 2.0, h: 7.0 }, Rect { x: 6.0, y: 3.0, w: 4.0, h: 7.0 }]
        );
        assert_eq!(r.minus(Rect { x: 20.0, ..r }), [r]);
    }

    #[test]
    fn frames_scroll_and_keep_the_caret_visible() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let t = Rope::from_str(&text);
        let mut l = Layout::new(14.0, "monospace");
        l.set_width(800.0);
        let lh = l.line_height();
        let placed = l.frame(&t, 0, 10.0 * lh);
        assert_eq!(placed.len(), 10);
        assert_eq!(l.scroll_lines(&t, 0, 3), t.line_to_byte(3));
        assert_eq!(l.scroll_lines(&t, t.line_to_byte(3), -5), 0);
        // The caret on line 50, far below: its line is centred.
        let head = t.line_to_byte(50) + 2;
        assert_eq!(l.keep_visible(&t, 0, head, 10.0 * lh), Some(t.line_to_byte(45)));
        assert_eq!(l.keep_visible(&t, t.line_to_byte(41), head, 10.0 * lh), None);
        // Near, within ten lines: scrolled just enough, to the last line or
        // the first.
        assert_eq!(l.keep_visible(&t, t.line_to_byte(35), head, 10.0 * lh), Some(t.line_to_byte(41)));
        assert_eq!(l.keep_visible(&t, t.line_to_byte(60), head, 10.0 * lh), Some(t.line_to_byte(50)));
        assert_eq!(l.keep_visible(&t, t.line_to_byte(80), head, 10.0 * lh), Some(t.line_to_byte(45)));
    }

    #[test]
    fn inlays_shape_in_the_line_and_map_back() {
        let t = Rope::from_str("let x = 1;\n");
        let mut l = Layout::new(14.0, "monospace");
        l.set_width(800.0);
        let plain = l.frame(&t, 0, 1000.0);
        l.set_inlays(&[Inlay { at: 5, text: ": i32".into(), style: 7 }], |_| (Weight::Normal, Slant::Normal));
        let placed = l.frame(&t, 0, 1000.0);
        assert_ne!(placed[0].key, plain[0].key, "shaped with the inlay");
        let drawn = inlays(&l, &placed);
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].1, 7, "in its style");
        let inlay = drawn[0].0;
        // The caret at its place is before it, where it was without it;
        // one on, past it.
        let at = caret(&l, &placed, &t, 5).unwrap();
        let after = caret(&l, &placed, &t, 6).unwrap();
        l.set_inlays(&[], |_| (Weight::Normal, Slant::Normal));
        let plain_at = caret(&l, &plain, &t, 5).unwrap();
        let plain_after = caret(&l, &plain, &t, 6).unwrap();
        l.set_inlays(&[Inlay { at: 5, text: ": i32".into(), style: 7 }], |_| (Weight::Normal, Slant::Normal));
        assert!((at.x - plain_at.x).abs() < 0.5 && (inlay.x - at.x).abs() < 0.5, "{at:?} {plain_at:?} {inlay:?}");
        assert!((after.x - plain_after.x - inlay.w).abs() < 0.5, "{after:?} {plain_after:?} {inlay:?}");
        // A click on it is at its place.
        assert_eq!(hit(&l, &placed, inlay.x + inlay.w / 2.0, at.y + 1.0), Some(5));
        // A range over it covers the text, not the inlay.
        let width = |c: Covered| c.text.iter().map(|r| r.w).sum::<f32>();
        let with = width(selection(&l, &placed, 4, 7));
        let without = width(selection(&l, &plain, 4, 7));
        assert!((with - without).abs() < 0.5, "{with} {without}");
    }

    #[test]
    fn inlays_at_one_place_follow_one_another() {
        let map = Inlays(vec![(5, 2, 1), (5, 3, 2)]);
        assert_eq!(map.shown().collect::<Vec<_>>(), [(5, 7, 1), (7, 10, 2)]);
        assert_eq!(map.text_ranges(0, 10), [(0, 5), (10, 15)]);
        assert_eq!(map.caret_at(5), 5);
        assert_eq!(map.char_at(5), 10);
        assert_eq!(map.text_at(8), 5);
    }

    #[test]
    fn wrapping_hit_testing_and_carets() {
        let t = Rope::from_str(&"word ".repeat(40));
        let mut l = Layout::new(14.0, "monospace");
        l.set_width(200.0);
        let placed = l.frame(&t, 0, 1000.0);
        assert_eq!(placed.len(), 1);
        assert!(placed[0].lines.len() > 3, "a long line wraps");
        let starts = l.visual_starts(&t, placed[0].seg);
        // The second visual line starts at a word.
        assert_eq!(starts[1] % 5, 0);
        let c = caret(&l, &placed, &t, starts[1]).unwrap();
        let pos = hit(&l, &placed, c.x + 1.0, c.y + 1.0).unwrap();
        assert_eq!(pos, starts[1]);
        // Scrolling by one visual line moves into the wrapped line.
        assert_eq!(l.scroll_lines(&t, 0, 1), starts[1]);
    }

    fn bold(from: usize, to: usize) -> ShapeRun {
        ShapeRun { from, to, shape: Shape { weight: Weight::Bold, slant: Slant::Italic, ..Shape::default() } }
    }

    #[test]
    fn geometry_is_counted_in_the_shapes_drawn() {
        // In a proportional font bold is wider than plain text: the
        // styled buffer wraps differently, and what is counted (visual
        // lines, carets, clicks) is that buffer's, the one drawn.
        let t = Rope::from_str(&"word ".repeat(40));
        let mut l = Layout::new(14.0, "sans-serif");
        l.set_width(200.0);
        let plain = l.frame(&t, 0, 1000.0);
        l.set_shape(Shape::default(), vec![bold(0, t.len_bytes())]);
        let styled = l.frame(&t, 0, 1000.0);
        assert_ne!(plain[0].key, styled[0].key, "another buffer is shaped");
        let drawn = l.buffer(&styled[0].key).expect("shaped").layout_runs().count();
        assert_eq!(styled[0].lines.len(), drawn);
        assert_eq!(l.visual_starts(&t, styled[0].seg).len(), drawn);
        let starts = l.visual_starts(&t, styled[0].seg);
        let c = caret(&l, &styled, &t, starts[1]).unwrap();
        assert_eq!(hit(&l, &styled, c.x + 1.0, c.y + 1.0), Some(starts[1]));
    }

    #[test]
    fn a_larger_size_makes_its_line_taller() {
        // A heading at twice the size: its line is as tall as it, the
        // lines below start under it, and carets and clicks follow.
        let t = Rope::from_str("Title\nbody text\nmore\n");
        let mut l = Layout::new(14.0, "monospace");
        l.set_width(800.0);
        let lh = l.line_height();
        l.set_shape(Shape::default(), vec![ShapeRun { from: 0, to: 5, shape: Shape { height: 200, ..Shape::default() } }]);
        let placed = l.frame(&t, 0, 1000.0);
        let title = placed[0].height();
        assert!(title > 1.8 * lh, "{title} {lh}");
        assert_eq!(placed[1].top, title);
        assert!((placed[1].height() - lh).abs() < 0.5);
        let c = caret(&l, &placed, &t, 6).unwrap();
        assert!((c.y - title).abs() < 0.5 && (c.h - lh).abs() < 0.5, "{c:?}");
        assert_eq!(hit(&l, &placed, 1.0, title + 1.0), Some(6));
        assert!(caret(&l, &placed, &t, 1).unwrap().h > 1.8 * lh, "the heading's caret is its height");
        // Scrolled to the second line, the heading is above the frame.
        let placed = l.frame(&t, 6, 1000.0);
        assert_eq!(placed[0].seg.start, 6);
        assert_eq!(placed[0].top, 0.0);
    }

    #[test]
    fn taller_lines_keep_the_caret_visible() {
        // Ten lines fit at the plain height; at twice it, five do: the
        // caret on the eighth is scrolled to, not taken as visible.
        let text: String = (0..20).map(|i| format!("line {i}\n")).collect();
        let t = Rope::from_str(&text);
        let mut l = Layout::new(14.0, "monospace");
        l.set_width(800.0);
        let lh = l.line_height();
        l.set_shape(Shape { height: 200, ..Shape::default() }, Vec::new());
        let head = t.line_to_byte(8);
        let anchor = l.keep_visible(&t, 0, head, 10.0 * lh).expect("scrolled");
        let placed = l.frame(&t, anchor, 10.0 * lh);
        let c = caret(&l, &placed, &t, head).unwrap();
        assert!(c.y >= 0.0 && c.y + c.h <= 10.0 * lh + 0.5, "{c:?}");
    }

    #[test]
    fn fallback_glyphs_are_counted_as_drawn() {
        // Text the font has no glyphs for falls back to another, whose
        // bold may well be wider: still, the lines counted are the
        // lines drawn.
        let t = Rope::from_str(&"\u{939}\u{93f}\u{928}\u{94d}\u{926}\u{940} x ".repeat(30));
        let mut l = Layout::new(14.0, "monospace");
        l.set_width(79.0);
        l.set_shape(Shape::default(), vec![bold(0, t.len_bytes())]);
        let styled = l.frame(&t, 0, 5000.0);
        let buffer = l.buffer(&styled[0].key).expect("shaped");
        assert_eq!(buffer.layout_runs().count(), l.visual_starts(&t, styled[0].seg).len());
    }

    #[test]
    fn resizing_keeps_the_anchor_on_the_first_line() {
        let text: String = (0..50).map(|i| format!("{i} {}\n", "word ".repeat(i % 30))).collect();
        let t = Rope::from_str(&text);
        let mut l = Layout::new(14.0, "monospace");
        l.set_width(900.0);
        let anchor = l.scroll_lines(&t, 0, 40);
        for width in [300.0, 1200.0, 150.0] {
            l.set_width(width);
            let placed = l.frame(&t, anchor, 500.0);
            let first = &placed[0];
            let starts = l.visual_starts(&t, first.seg);
            let line = starts.iter().rposition(|&s| s <= anchor).unwrap();
            assert_eq!(first.top + first.lines[line].top, 0.0, "width {width}");
        }
    }
}
