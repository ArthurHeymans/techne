//! Layout of a text snapshot in a window: what the frontend owns (EDITOR.md,
//! sections 6 and 7). Shaping and wrapping happen here and only around what
//! is shown; the runtime gets back semantic positions. Segments and
//! scrolling by visual lines are shared with the other frontends
//! (`techne_editor::display`); shaped segments are cached by their text.

use std::collections::HashMap;

use glyphon::{Attrs, Buffer, Cursor, Family, FontSystem, Metrics, Shaping, Wrap};
use techne_editor::display::{self, Segment, Wrapping};
use techne_text::{motion, ropey::Rope};

/// The text shaped for a segment: a stray carriage return would start a new
/// line for the shaper, so it is shown as a space (same byte length).
fn display_text(text: &Rope, seg: Segment) -> String {
    text.byte_slice(seg.start..seg.end).to_string().replace('\r', " ")
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

/// A segment placed in the frame, `top` pixels from the text area's top.
#[derive(Clone, Debug)]
pub struct Placed {
    pub seg: Segment,
    pub key: String,
    pub top: f32,
    pub lines: usize,
}

pub struct Layout {
    pub fonts: FontSystem,
    metrics: Metrics,
    family: String,
    width: f32,
    cache: HashMap<String, Shaped>,
    frame: u64,
}

impl Layout {
    pub fn new(font_size: f32, family: &str) -> Layout {
        Layout {
            fonts: FontSystem::new(),
            metrics: Metrics::new(font_size, (font_size * 1.35).round()),
            family: family.to_string(),
            width: 0.0,
            cache: HashMap::new(),
            frame: 0,
        }
    }

    pub fn line_height(&self) -> f32 {
        self.metrics.line_height
    }

    /// The width text wraps at; shaped segments are dropped when it changes.
    pub fn set_width(&mut self, width: f32) {
        if (width - self.width).abs() > 0.5 {
            self.width = width;
            self.cache.clear();
        }
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

    fn shape(&mut self, key: &str) -> &Shaped {
        let frame = self.frame;
        if !self.cache.contains_key(key) {
            let mut buffer = Buffer::new(&mut self.fonts, self.metrics);
            buffer.set_wrap(Wrap::WordOrGlyph);
            buffer.set_size(Some(self.width.max(1.0)), None);
            let family = match self.family.as_str() {
                "monospace" => Family::Monospace,
                "sans-serif" => Family::SansSerif,
                "serif" => Family::Serif,
                name => Family::Name(name),
            };
            buffer.set_text(key, &Attrs::new().family(family), Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.fonts, false);
            let starts = buffer.layout_runs().map(|r| r.glyphs.iter().map(|g| g.start).min().unwrap_or(0)).collect();
            self.cache.insert(key.to_string(), Shaped { buffer, starts, used: frame });
        }
        let s = self.cache.get_mut(key).expect("just inserted");
        s.used = frame;
        s
    }

    pub fn buffer(&self, key: &str) -> Option<&Buffer> {
        self.cache.get(key).map(|s| &s.buffer)
    }

    /// The font system, and the shaped buffers to draw with it.
    pub fn split(&mut self) -> (&mut FontSystem, Buffers<'_>) {
        (&mut self.fonts, Buffers(&self.cache))
    }

    /// The segments to show from the scroll anchor down to `height`.
    pub fn frame(&mut self, text: &Rope, anchor: usize, height: f32) -> Vec<Placed> {
        let lh = self.line_height();
        let fit = (height / lh).ceil() as usize;
        display::frame(self, text, anchor, fit)
            .into_iter()
            .map(|p| Placed { seg: p.seg, key: display_text(text, p.seg), top: p.row as f32 * lh, lines: p.rows })
            .collect()
    }

    /// Visual lines that fit in `height` entirely.
    pub fn fit(&self, height: f32) -> usize {
        (height / self.line_height()).floor() as usize
    }
}

impl Wrapping for Layout {
    fn visual_starts(&mut self, text: &Rope, seg: Segment) -> Vec<usize> {
        let key = display_text(text, seg);
        let starts = &self.shape(&key).starts;
        let mut v: Vec<usize> = starts.iter().map(|s| seg.start + s).collect();
        if v.is_empty() {
            v.push(seg.start);
        }
        v[0] = seg.start;
        v
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
    Some(p.seg.start + cursor.index.min(p.seg.end - p.seg.start))
}

/// A rectangle in text-area pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The caret at `pos`: its left edge, top, and the width of the character
/// it is on (for a block cursor).
pub fn caret(layout: &Layout, placed: &[Placed], text: &Rope, pos: usize) -> Option<Rect> {
    let p = placed_at(placed, pos)?;
    let buffer = layout.buffer(&p.key)?;
    let idx = pos - p.seg.start;
    let lh = layout.line_height();
    let next = if pos < p.seg.end { motion::next_grapheme(text, pos).min(p.seg.end) - p.seg.start } else { idx };
    for run in buffer.layout_runs() {
        if let Some(x) = run.cursor_position(&Cursor::new(0, idx)) {
            let w = run.highlight(Cursor::new(0, idx), Cursor::new(0, next)).map(|(_, w)| w).next().unwrap_or(lh * 0.5);
            return Some(Rect { x, y: p.top + run.line_top, w, h: lh });
        }
    }
    Some(Rect { x: 0.0, y: p.top, w: lh * 0.5, h: lh })
}

/// The rectangles covering the range [from, to); a selected line break shows
/// as a narrow box after its line.
pub fn selection(layout: &Layout, placed: &[Placed], from: usize, to: usize) -> Vec<Rect> {
    let lh = layout.line_height();
    let mut rects = Vec::new();
    for p in placed.iter().filter(|p| from <= p.seg.end && to > p.seg.start) {
        let Some(buffer) = layout.buffer(&p.key) else { continue };
        let (a, b) = (from.max(p.seg.start) - p.seg.start, to.min(p.seg.end) - p.seg.start);
        let mut last = None;
        for run in buffer.layout_runs() {
            for (x, w) in run.highlight(Cursor::new(0, a), Cursor::new(0, b)) {
                rects.push(Rect { x, y: p.top + run.line_top, w, h: lh });
            }
            last = Some((run.line_w, p.top + run.line_top));
        }
        if to > p.seg.end
            && let Some((x, y)) = last
        {
            rects.push(Rect { x, y, w: lh * 0.4, h: lh });
        }
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_scroll_and_keep_the_caret_visible() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let t = Rope::from_str(&text);
        let mut l = Layout::new(14.0, "monospace");
        l.set_width(800.0);
        let lh = l.line_height();
        let placed = l.frame(&t, 0, 10.0 * lh);
        assert_eq!(placed.len(), 10);
        assert_eq!(display::scroll_lines(&mut l, &t, 0, 3), t.line_to_byte(3));
        // The caret on line 50: scrolled so that it is the last visible line.
        let head = t.line_to_byte(50) + 2;
        let fit = l.fit(10.0 * lh);
        assert_eq!(display::keep_visible(&mut l, &t, 0, head, fit), Some(t.line_to_byte(41)));
    }

    #[test]
    fn wrapping_hit_testing_and_carets() {
        let t = Rope::from_str(&"word ".repeat(40));
        let mut l = Layout::new(14.0, "monospace");
        l.set_width(200.0);
        let placed = l.frame(&t, 0, 1000.0);
        assert_eq!(placed.len(), 1);
        assert!(placed[0].lines > 3, "a long line wraps");
        let starts = l.visual_starts(&t, placed[0].seg);
        // The second visual line starts at a word.
        assert_eq!(starts[1] % 5, 0);
        let c = caret(&l, &placed, &t, starts[1]).unwrap();
        let pos = hit(&l, &placed, c.x + 1.0, c.y + 1.0).unwrap();
        assert_eq!(pos, starts[1]);
        // Scrolling by one visual line moves into the wrapped line.
        assert_eq!(display::scroll_lines(&mut l, &t, 0, 1), starts[1]);
    }

    /// The scripted session every frontend runs (techne_editor::scenario),
    /// with clicks at the glyphs this layout shows for their targets.
    #[test]
    fn the_scripted_session_ends_where_the_runtime_alone_does() {
        use std::time::Instant;
        use techne_editor::{
            present::Input,
            runtime::Runtime,
            scenario::{self, Step},
        };
        let mut rt = Runtime::with_document(techne_text::Document::new(scenario::TEXT), "emacs").unwrap();
        let mut l = Layout::new(14.0, "monospace");
        l.set_width(900.0);
        let key = |rt: &mut Runtime, k: &str| rt.handle(Input::Key { key: k.into(), at: Instant::now() });
        for step in scenario::STEPS {
            match *step {
                Step::Key(k) => _ = key(&mut rt, k),
                Step::Text(t) => t.chars().for_each(|c| _ = key(&mut rt, &c.to_string())),
                Step::Click { on, offset, extend } => {
                    let s = rt.snapshot();
                    let target = scenario::click_target(&s.text.to_string(), on, offset);
                    let placed = l.frame(&s.text, s.scroll, 600.0);
                    let c = caret(&l, &placed, &s.text, target).expect("the target is on screen");
                    let pos = hit(&l, &placed, c.x + 1.0, c.y + c.h / 2.0).unwrap();
                    rt.handle(Input::Click { revision: s.revision, pos, extend, at: Instant::now() });
                }
                // Columns, as about ten pixels each.
                Step::Resize { width, .. } => l.set_width(width as f32 * 10.0),
            }
        }
        assert_eq!(scenario::state(&mut rt), scenario::expected("emacs"));
    }

    #[test]
    fn resizing_keeps_the_anchor_on_the_first_line() {
        let text: String = (0..50).map(|i| format!("{i} {}\n", "word ".repeat(i % 30))).collect();
        let t = Rope::from_str(&text);
        let mut l = Layout::new(14.0, "monospace");
        l.set_width(900.0);
        let anchor = display::scroll_lines(&mut l, &t, 0, 40);
        for width in [300.0, 1200.0, 150.0] {
            l.set_width(width);
            let placed = l.frame(&t, anchor, 500.0);
            let first = &placed[0];
            let starts = l.visual_starts(&t, first.seg);
            let line = starts.iter().rposition(|&s| s <= anchor).unwrap();
            assert_eq!(first.top + line as f32 * l.line_height(), 0.0, "width {width}");
        }
    }
}
