//! Layout of a text snapshot in a window: what the frontend owns (EDITOR.md,
//! sections 6 and 7). Shaping and wrapping happen here and only around what
//! is shown, in display segments (`techne_editor::segment`, which also
//! scrolls by anchor); the runtime gets back semantic positions. Shaped
//! segments are cached by their text and the width they wrap at, so panes
//! of different widths share the cache.
//!
//! Weight and slant shape with the text (`ShapeRun`), in a font whose
//! bold and italic keep the plain glyphs' advances — checked when the
//! layout is made (`probe_styled`), because geometry is also counted
//! without styles (scrolling, keeping the caret visible, segments past
//! the styled window) and the two must agree. In a font they move
//! (a proportional one), weight and slant are not shaped: the text is
//! drawn plain rather than drawn where it is not measured. Family and
//! height, which would change advances and line heights in a larger
//! way, are not shaped here at all.

use std::collections::HashMap;

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
    used: u64,
}

pub struct Buffers<'a>(&'a HashMap<String, Shaped>);

impl<'a> Buffers<'a> {
    pub fn get(&self, key: &str) -> Option<&'a Buffer> {
        self.0.get(key).map(|s| &s.buffer)
    }
}

/// A range shaped with a weight and slant of its own, in text positions.
#[derive(Clone, Copy, Debug)]
pub struct ShapeRun {
    pub from: usize,
    pub to: usize,
    pub weight: Weight,
    pub slant: Slant,
}

/// One stretch of a segment in one weight and slant, in bytes from its
/// start: what `shape` works in, a `ShapeRun` clipped to the segment.
type RunSpan = (usize, usize, Weight, Slant);

/// A segment placed in the frame, `top` pixels from the text area's top.
#[derive(Clone, Debug)]
pub struct Placed {
    pub seg: Segment,
    pub key: String,
    pub top: f32,
    pub lines: usize,
    /// The inlays shaped with it.
    pub inlays: Inlays,
}

fn family(name: &str) -> Family<'_> {
    match name {
        "monospace" => Family::Monospace,
        "sans-serif" => Family::SansSerif,
        "serif" => Family::Serif,
        name => Family::Name(name),
    }
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
    /// Whether weight and slant may shape: the font's bold and italic
    /// keep its plain advances (`probe_styled`).
    shape_styles: bool,
    /// The inlays of the pane being laid out (`set_inlays`), and the
    /// weight and slant of each one's style.
    inlays: Vec<Inlay>,
    inlay_shapes: Vec<(Weight, Slant)>,
}

impl Layout {
    pub fn new(font_size: f32, family: &str) -> Layout {
        let mut layout = Layout {
            fonts: FontSystem::new(),
            metrics: Metrics::new(font_size, (font_size * 1.35).round()),
            family: family.to_string(),
            width: 0.0,
            char_width: 0.0,
            cache: HashMap::new(),
            frame: 0,
            shape_styles: false,
            inlays: Vec::new(),
            inlay_shapes: Vec::new(),
        };
        let sample = layout.label(&"0".repeat(20), f32::MAX);
        layout.char_width = sample.layout_runs().map(|r| r.line_w).fold(0.0, f32::max) / 20.0;
        layout.shape_styles = layout.probe_styled();
        layout
    }

    /// Whether this font's bold and italic lay out where its plain text
    /// does: the same wrap points and glyph positions for a long
    /// wrapping sample. Only then may styled runs shape, all the
    /// geometry counted elsewhere being the plain text's.
    fn probe_styled(&mut self) -> bool {
        let text = "the quick brown fox jumps over 0123456789 lazy dogs, pack my box with five dozen liquor jugs! ".repeat(3);
        let laid_out = |layout: &mut Layout, weight: Weight, slant: Slant| {
            let mut b = Buffer::new(&mut layout.fonts, layout.metrics);
            b.set_wrap(Wrap::WordOrGlyph);
            b.set_size(Some(220.0), None);
            let attrs = Attrs::new()
                .family(family(&layout.family))
                .weight(match weight {
                    Weight::Light => FontWeight::LIGHT,
                    Weight::Normal => FontWeight::NORMAL,
                    Weight::Bold => FontWeight::BOLD,
                })
                .style(match slant {
                    Slant::Normal => FontStyle::Normal,
                    Slant::Italic => FontStyle::Italic,
                });
            b.set_text(&text, &attrs, Shaping::Advanced, None);
            b.shape_until_scroll(&mut layout.fonts, false);
            b.layout_runs().flat_map(|r| r.glyphs.iter().map(|g| (g.start, g.x)).collect::<Vec<_>>()).collect::<Vec<_>>()
        };
        let plain = laid_out(self, Weight::Normal, Slant::Normal);
        [
            (Weight::Bold, Slant::Normal),
            (Weight::Normal, Slant::Italic),
            (Weight::Bold, Slant::Italic),
            (Weight::Light, Slant::Normal),
            (Weight::Light, Slant::Italic),
        ]
        .iter()
        .all(|&(w, s)| {
            let styled = laid_out(self, w, s);
            styled.len() == plain.len() && styled.iter().zip(&plain).all(|(a, b)| a.0 == b.0 && (a.1 - b.1).abs() < 0.01)
        })
    }

    pub fn char_width(&self) -> f32 {
        self.char_width
    }

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
    /// runs that shape it (empty: plainly, the key text has had all along,
    /// so unstyled geometry and plain segments share an entry). Paint
    /// attributes are no part of it: a colour never reshapes.
    fn key(&self, text: &str, base: (Weight, Slant), runs: &[RunSpan]) -> String {
        let (base, runs) = self.effective(base, runs);
        let mut key = format!("{}\u{0}{text}", self.width.round() as u32);
        if base != (Weight::Normal, Slant::Normal) || !runs.is_empty() {
            key.push_str(&format!("\u{0}{:?}{:?}", base.0, base.1));
            for (from, to, w, s) in runs {
                key.push_str(&format!(";{from}-{to}:{w:?}{s:?}"));
            }
        }
        key
    }

    /// What shaping is asked for, as shaped: in a font whose bold or
    /// italic moves its advances, plain — drawn where it is measured,
    /// not otherwise. `key` and `shape` both take it from here, or a
    /// placed segment would name a buffer nothing shaped.
    fn effective<'r>(&self, base: (Weight, Slant), runs: &'r [RunSpan]) -> ((Weight, Slant), &'r [RunSpan]) {
        if self.shape_styles { (base, runs) } else { ((Weight::Normal, Slant::Normal), &[][..]) }
    }

    /// A buffer's geometry: where each glyph starts, line by line —
    /// what drawing and counting must agree on, blank for blank.
    fn geometry(buffer: &Buffer) -> Vec<(usize, f32)> {
        buffer.layout_runs().flat_map(|r| r.glyphs.iter().map(|g| (g.start, g.x)).collect::<Vec<_>>()).collect()
    }

    fn shape(&mut self, text: &str, base: (Weight, Slant), runs: &[RunSpan]) -> &Shaped {
        let styled = base != (Weight::Normal, Slant::Normal) || !runs.is_empty();
        let (base, runs) = self.effective(base, runs);
        let frame = self.frame;
        let key = &self.key(text, base, runs);
        if !self.cache.contains_key(key) {
            let mut buffer = self.build_buffer(text, base, runs);
            if styled && self.shape_styles {
                // The probe sampled this font's own glyphs; a fallback
                // font under some of the text may still move them.
                // Measured per segment: where the styled shaping sits
                // anywhere else than the plain one, the plain buffer is
                // drawn — geometry is never any styled run's to move.
                let plain = self.build_buffer(text, (Weight::Normal, Slant::Normal), &[]);
                let (a, b) = (Self::geometry(&buffer), Self::geometry(&plain));
                if a.len() != b.len() || a.iter().zip(&b).any(|(x, y)| x.0 != y.0 || (x.1 - y.1).abs() >= 0.01) {
                    buffer = plain;
                }
            }
            let starts = buffer.layout_runs().map(|r| r.glyphs.iter().map(|g| g.start).min().unwrap_or(0)).collect();
            self.cache.insert(key.to_string(), Shaped { buffer, starts, used: frame });
        }
        let s = self.cache.get_mut(key).expect("just inserted");
        s.used = frame;
        s
    }

    /// Shape `text` once, in stretches of one weight and slant.
    fn build_buffer(&mut self, text: &str, base: (Weight, Slant), runs: &[RunSpan]) -> Buffer {
        let mut buffer = Buffer::new(&mut self.fonts, self.metrics);
        buffer.set_wrap(Wrap::WordOrGlyph);
        buffer.set_size(Some(self.width.max(1.0)), None);
        let attrs = |w: Weight, s: Slant| {
            Attrs::new()
                .family(family(&self.family))
                .weight(match w {
                    Weight::Light => FontWeight::LIGHT,
                    Weight::Normal => FontWeight::NORMAL,
                    Weight::Bold => FontWeight::BOLD,
                })
                .style(match s {
                    Slant::Normal => FontStyle::Normal,
                    Slant::Italic => FontStyle::Italic,
                })
        };
        if runs.is_empty() && base == (Weight::Normal, Slant::Normal) {
            buffer.set_text(text, &attrs(Weight::Normal, Slant::Normal), Shaping::Advanced, None);
        } else {
            // The segment in stretches of one weight and slant, the
            // runs' boundaries on character boundaries.
            let floor = |i: usize| (0..=i.min(text.len())).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
            let ceil = |i: usize| (i.min(text.len())..=text.len()).find(|&i| text.is_char_boundary(i)).unwrap_or(text.len());
            let mut spans: Vec<(&str, Attrs)> = Vec::new();
            let mut at = 0;
            for &(from, to, w, s) in runs {
                let (from, to) = (floor(from), ceil(to));
                if at < from {
                    spans.push((&text[at..from], attrs(base.0, base.1)));
                }
                if from < to {
                    spans.push((&text[from.max(at)..to], attrs(w, s)));
                    at = to;
                }
            }
            if at < text.len() {
                spans.push((&text[at..], attrs(base.0, base.1)));
            }
            buffer.set_rich_text(spans, &attrs(base.0, base.1), Shaping::Advanced, None);
        }
        buffer.shape_until_scroll(&mut self.fonts, false);
        buffer
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
        let attrs = Attrs::new()
            .family(family(&self.family))
            .weight(match weight {
                Weight::Light => FontWeight::LIGHT,
                Weight::Normal => FontWeight::NORMAL,
                Weight::Bold => FontWeight::BOLD,
            })
            .style(match slant {
                Slant::Normal => FontStyle::Normal,
                Slant::Italic => FontStyle::Italic,
            });
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

    /// Where the segment's visual lines start, as positions in the text.
    pub fn visual_starts(&mut self, text: &Rope, seg: Segment) -> Vec<usize> {
        let inlays = std::mem::take(&mut self.inlays);
        let (key, map) = display_text(text, seg, &inlays);
        self.inlays = inlays;
        let starts = &self.shape(&key, (Weight::Normal, Slant::Normal), &[]).starts;
        let mut v: Vec<usize> = starts.iter().map(|&s| seg.start + map.text_at(s)).collect();
        if v.is_empty() {
            v.push(seg.start);
        }
        v[0] = seg.start;
        // A line that starts in an inlay starts at the text it is
        // before: several can start there, and each counts
        // (`segment::scroll_lines`).
        v
    }

    /// The visual line of `seg` that `pos` is on.
    fn visual_index(&mut self, text: &Rope, seg: Segment, pos: usize) -> usize {
        segment::line_index(&self.visual_starts(text, seg), pos)
    }

    /// The segments to show from the scroll anchor down to `height`,
    /// shaped with `base` everywhere and `runs` (in text positions)
    /// where a style asks for more.
    pub fn frame(&mut self, text: &Rope, anchor: usize, height: f32, base: (Weight, Slant), runs: &[ShapeRun]) -> Vec<Placed> {
        let lh = self.line_height();
        let anchor = snap(text, anchor.min(text.len_bytes()));
        let first = segment_at(text, anchor);
        let mut y = -(self.visual_index(text, first, anchor) as f32) * lh;
        let mut placed = Vec::new();
        let mut seg = Some(first);
        while let Some(s) = seg {
            if y >= height {
                break;
            }
            let (shown, map) = display_text(text, s, &self.inlays);
            // The text's runs, around the inlays in them; each inlay in
            // its own style's weight and slant.
            let first = self.inlays.partition_point(|i| i.at < s.start);
            let mut own: Vec<RunSpan> = runs
                .iter()
                .filter(|r| r.from < s.end && r.to > s.start)
                .flat_map(|r| {
                    map.text_ranges(r.from.max(s.start) - s.start, r.to.min(s.end) - s.start)
                        .into_iter()
                        .filter(|(a, b)| a < b)
                        .map(|(a, b)| (a, b, r.weight, r.slant))
                })
                .chain(
                    map.shown()
                        .zip(&self.inlay_shapes[first..])
                        .filter(|(_, (w, sl))| (*w, *sl) != (Weight::Normal, Slant::Normal))
                        .map(|((a, b, _), &(w, sl))| (a, b, w, sl)),
                )
                .collect();
            own.sort_by_key(|r| r.0);
            let lines = self.shape(&shown, base, &own).starts.len().max(1);
            let key = self.key(&shown, base, &own);
            placed.push(Placed { seg: s, key, top: y, lines, inlays: map });
            y += lines as f32 * lh;
            seg = next_segment(text, s);
        }
        placed
    }

    /// The scroll anchor `n` visual lines further down (up when negative).
    pub fn scroll_lines(&mut self, text: &Rope, anchor: usize, n: i64) -> usize {
        segment::scroll_lines(text, anchor, n, &mut |seg| self.visual_starts(text, seg))
    }

    /// A new scroll anchor that shows `head`, if it is off screen: at the
    /// top when it is above, at the bottom when it is below.
    pub fn keep_visible(&mut self, text: &Rope, anchor: usize, head: usize, height: f32) -> Option<usize> {
        let fit = (height / self.line_height()).floor() as usize;
        segment::keep_visible(text, anchor, head, fit, &mut |seg| self.visual_starts(text, seg))
    }
}

/// Which placed segment shows `pos`; at a piece boundary of a long line the
/// later piece.
pub fn placed_at(placed: &[Placed], pos: usize) -> Option<&Placed> {
    placed.iter().rev().find(|p| p.seg.start <= pos && pos <= p.seg.end)
}

/// The text position at (x, y) in the text area.
pub fn hit(layout: &Layout, placed: &[Placed], x: f32, y: f32) -> Option<usize> {
    let lh = layout.line_height();
    let p = placed.iter().find(|p| y < p.top + p.lines as f32 * lh).or(placed.last())?;
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
    let lh = layout.line_height();
    let (on, next) = if pos < p.seg.end {
        (p.inlays.char_at(rel), p.inlays.caret_at(motion::next_grapheme(text, pos).min(p.seg.end) - p.seg.start))
    } else {
        (idx, idx)
    };
    for run in buffer.layout_runs() {
        if let Some(x) = run.cursor_position(&Cursor::new(0, idx)) {
            let w = run.highlight(Cursor::new(0, on), Cursor::new(0, next)).map(|(_, w)| w).next().unwrap_or(lh * 0.5);
            return Some(Rect { x, y: p.top + run.line_top, w, h: lh });
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
    let lh = layout.line_height();
    let mut covered = Covered::default();
    for p in placed.iter().filter(|p| from <= p.seg.end && to > p.seg.start) {
        let Some(buffer) = layout.buffer(&p.key) else { continue };
        let (a, b) = (from.max(p.seg.start) - p.seg.start, to.min(p.seg.end) - p.seg.start);
        let mut last = None;
        for run in buffer.layout_runs() {
            // The text's, not the inlays among it.
            for (a, b) in p.inlays.text_ranges(a, b) {
                for (x, w) in run.highlight(Cursor::new(0, a), Cursor::new(0, b)) {
                    covered.text.push(Rect { x, y: p.top + run.line_top, w, h: lh });
                }
            }
            last = Some((run.line_w, p.top + run.line_top));
        }
        if to > p.seg.end
            && let Some((x, y)) = last
        {
            covered.breaks.push(Rect { x, y, w: lh * 0.4, h: lh });
        }
    }
    covered
}

/// Where the inlays are drawn, and with which style.
pub fn inlays(layout: &Layout, placed: &[Placed]) -> Vec<(Rect, StyleId)> {
    let lh = layout.line_height();
    let mut out = Vec::new();
    for p in placed {
        let Some(buffer) = layout.buffer(&p.key) else { continue };
        for (a, b, style) in p.inlays.shown() {
            for run in buffer.layout_runs() {
                for (x, w) in run.highlight(Cursor::new(0, a), Cursor::new(0, b)) {
                    out.push((Rect { x, y: p.top + run.line_top, w, h: lh }, style));
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
        let placed = l.frame(&t, 0, 10.0 * lh, (Weight::Normal, Slant::Normal), &[]);
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
        let plain = l.frame(&t, 0, 1000.0, (Weight::Normal, Slant::Normal), &[]);
        l.set_inlays(&[Inlay { at: 5, text: ": i32".into(), style: 7 }], |_| (Weight::Normal, Slant::Normal));
        let placed = l.frame(&t, 0, 1000.0, (Weight::Normal, Slant::Normal), &[]);
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
        let placed = l.frame(&t, 0, 1000.0, (Weight::Normal, Slant::Normal), &[]);
        assert_eq!(placed.len(), 1);
        assert!(placed[0].lines > 3, "a long line wraps");
        let starts = l.visual_starts(&t, placed[0].seg);
        // The second visual line starts at a word.
        assert_eq!(starts[1] % 5, 0);
        let c = caret(&l, &placed, &t, starts[1]).unwrap();
        let pos = hit(&l, &placed, c.x + 1.0, c.y + 1.0).unwrap();
        assert_eq!(pos, starts[1]);
        // Scrolling by one visual line moves into the wrapped line.
        assert_eq!(l.scroll_lines(&t, 0, 1), starts[1]);
    }

    #[test]
    fn weight_and_slant_shape_without_moving_the_lines() {
        // The promise geometry rests on, checked on the buffers drawn
        // from (not two plain shapings of one segment): in a font that
        // keeps its advances, the styled buffer's glyphs sit exactly
        // where the plain one's do; in one that does not, no styled
        // buffer is shaped at all.
        let t = Rope::from_str(&"word ".repeat(40));
        let mut l = Layout::new(14.0, "monospace");
        l.set_width(200.0);
        let plain = l.frame(&t, 0, 1000.0, (Weight::Normal, Slant::Normal), &[]);
        let runs = [ShapeRun { from: 5, to: 60, weight: Weight::Bold, slant: Slant::Italic }];
        let styled = l.frame(&t, 0, 1000.0, (Weight::Normal, Slant::Normal), &runs);
        if !l.shape_styles {
            assert_eq!(plain[0].key, styled[0].key, "this font's bold moves its advances: nothing styled is shaped");
            return;
        }
        assert_ne!(plain[0].key, styled[0].key, "another buffer is shaped");
        assert_eq!(plain[0].lines, styled[0].lines);
        let glyphs = |l: &Layout, key: &str| {
            l.buffer(key)
                .expect("shaped")
                .layout_runs()
                .flat_map(|r| r.glyphs.iter().map(|g| (g.start, g.x)).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        };
        let (a, b) = (glyphs(&l, &plain[0].key), glyphs(&l, &styled[0].key));
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(x, y)| x.0 == y.0 && (x.1 - y.1).abs() < 0.01), "every glyph where the plain one's is");
    }

    #[test]
    fn a_proportional_font_is_shaped_plain() {
        // Whichever sans the system has, if its bold is wider than its
        // plain text (the usual case), styled runs shape nothing: the
        // buffer drawn is the plain one geometry was counted from.
        let t = Rope::from_str(&"word ".repeat(40));
        let mut l = Layout::new(14.0, "sans-serif");
        l.set_width(200.0);
        let plain = l.frame(&t, 0, 1000.0, (Weight::Normal, Slant::Normal), &[]);
        let runs = [ShapeRun { from: 5, to: 60, weight: Weight::Bold, slant: Slant::Italic }];
        let styled = l.frame(&t, 0, 1000.0, (Weight::Normal, Slant::Normal), &runs);
        if !l.shape_styles {
            assert_eq!(plain[0].key, styled[0].key);
        }
        assert_eq!(plain[0].lines, styled[0].lines);
    }

    #[test]
    fn fallback_glyphs_do_not_move_the_lines() {
        // The probe samples ASCII; text the font has no glyphs for
        // falls back to another, whose bold may well be wider. Such a
        // segment is drawn from its plain buffer, so the styled
        // frame wraps exactly where the plain geometry counts do.
        let t = Rope::from_str(&"\u{939}\u{93f}\u{928}\u{94d}\u{926}\u{940} x ".repeat(30));
        let mut l = Layout::new(14.0, "monospace");
        l.set_width(79.0);
        let plain = l.frame(&t, 0, 5000.0, (Weight::Normal, Slant::Normal), &[]);
        let runs = [ShapeRun { from: 0, to: t.len_bytes(), weight: Weight::Bold, slant: Slant::Italic }];
        let styled = l.frame(&t, 0, 5000.0, (Weight::Normal, Slant::Normal), &runs);
        assert_eq!(plain[0].lines, styled[0].lines, "the styled frame wraps where geometry counted");
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
            let placed = l.frame(&t, anchor, 500.0, (Weight::Normal, Slant::Normal), &[]);
            let first = &placed[0];
            let starts = l.visual_starts(&t, first.seg);
            let line = starts.iter().rposition(|&s| s <= anchor).unwrap();
            assert_eq!(first.top + line as f32 * l.line_height(), 0.0, "width {width}");
        }
    }
}
