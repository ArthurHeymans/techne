//! A snapshot's panes in the window, without drawing: where each pane is,
//! its scroll anchor and its segments laid out, and what clicks, the wheel
//! and new snapshots mean for them. Each pane is in its place, rounded to
//! whole lines, with its mode line below its text, and a gap with a
//! divider between it and a pane to its right; the open minibuffer is
//! below them (its input line, then its candidates), and the echo area is
//! the window's last line. Positions here are in pixels from the text's
//! left edge and the window's top.

use std::time::Instant;

use techne_editor::{
    hints,
    present::{Input, KeyHint, Pane, Recenter, Slant, Snapshot, ViewRequest, Weight},
};

use crate::layout::{self, Layout, Placed, Shape, ShapeRun};

/// A style's shape: what of it changes glyphs.
fn shape_of(style: &techne_editor::present::Style) -> Shape {
    Shape {
        weight: style.weight.unwrap_or(Weight::Normal),
        slant: style.slant.unwrap_or(Slant::Normal),
        family: style.family.clone(),
        height: style.height.unwrap_or(100),
    }
}

/// How a pane's text is shaped: its default style's shape everywhere,
/// and the runs its highlights ask more of. The styles come composed
/// from the runtime; only what changes glyphs is taken.
fn shape_runs(s: &Snapshot, p: &Pane) -> (Shape, Vec<ShapeRun>) {
    let default = s.style(p.default);
    let base = shape_of(&default);
    // Neighbouring runs that shape the same are one run: a change of
    // colour alone (a search match in a comment) must not enter the
    // shaping key and reshape what only needed painting.
    let mut runs: Vec<ShapeRun> = Vec::new();
    for h in &p.layers {
        let shape = shape_of(&default.patch(&s.style(h.style)));
        if shape == base {
            continue;
        }
        match runs.last_mut() {
            Some(last) if last.to == h.from && last.shape == shape => last.to = h.to,
            _ => runs.push(ShapeRun { from: h.from, to: h.to, shape }),
        }
    }
    (base, runs)
}

/// Lay out pane `p` of the snapshot `s` from now on: its width, its
/// inlays and its shapes. Every layout of a pane goes through here, so
/// counting (scrolling, the caret) and drawing shape it alike.
fn prepare(layout: &mut Layout, s: &Snapshot, p: &Pane, area: Area) {
    layout.set_width(area.text_width());
    layout.set_inlays(&p.inlays, |id| inlay_shape(Some(s), id));
    let (base, runs) = shape_runs(s, p);
    layout.set_shape(base, runs);
    // An image as tall as most of the pane at most: some text stays.
    layout.set_projections(&p.hangs, &p.blocks, (area.text * 0.8).max(layout.line_height()));
}

/// Where a pane is: its text from `top`, `text` pixels high, then its mode
/// line (when it has a line at all); from `left`, `width` pixels wide, the
/// first `gutter` for line numbers, then a gap when another pane is to its
/// right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub top: f32,
    pub text: f32,
    pub mode_line: bool,
    pub left: f32,
    pub width: f32,
    pub gap: bool,
    pub gutter: f32,
}

impl Area {
    /// Where the text starts, after the gutter.
    pub fn text_left(&self) -> f32 {
        self.left + self.gutter
    }

    /// The width the text wraps at.
    pub fn text_width(&self) -> f32 {
        (self.width - self.gutter).max(1.0)
    }
}

/// A pane as laid out for the last frame: what a click on it is on.
pub struct Shown {
    pub view: u64,
    pub revision: u64,
    pub area: Area,
    /// Segments, their tops from the pane's top.
    pub placed: Vec<Placed>,
}

#[derive(Default)]
pub struct Screen {
    pub snap: Option<Snapshot>,
    /// Each pane's scroll anchor: the snapshot's, or where this frontend
    /// scrolled it since.
    anchors: Vec<usize>,
    pub shown: Vec<Shown>,
    /// The width text wraps at and the window's height.
    width: f32,
    height: f32,
    /// The view a press was in, while the button is held.
    dragging: Option<u64>,
}

impl Screen {
    pub fn set_size(&mut self, width: f32, height: f32) {
        (self.width, self.height) = (width, height);
    }

    /// The runtime keeps an input waiting (`host::BUSY_AFTER`): the echo
    /// area says so until the next snapshot. Whether it did not already.
    pub fn busy(&mut self) -> bool {
        let busy = techne_editor::host::BUSY;
        match &mut self.snap {
            Some(s) if s.echo != busy => {
                s.echo = busy.into();
                true
            }
            _ => false,
        }
    }

    /// The echo area's top.
    pub fn echo_top(&self, layout: &Layout) -> f32 {
        (self.height - layout.line_height()).max(0.0)
    }

    /// which-key's columns of the keys shown: as many lines as they need
    /// to fit the width, up to a quarter of the window.
    pub fn hint_columns(&self, layout: &Layout) -> Vec<Vec<&KeyHint>> {
        let Some(s) = &self.snap else { return Vec::new() };
        let lh = layout.line_height();
        let lines = (self.minibuffer_top(layout) / lh).floor() as usize;
        let max = ((self.height / lh / 4.0) as usize).max(3).min(lines.saturating_sub(1));
        let chars = (self.width / layout.char_width().max(1.0)) as usize;
        hints::columns(&s.key_hints, max, chars, |h| h.key.chars().count(), |h| h.description.chars().count())
    }

    /// which-key's top, above the minibuffer.
    pub fn hints_top(&self, layout: &Layout) -> f32 {
        self.minibuffer_top(layout) - self.hint_columns(layout).first().map_or(0, Vec::len) as f32 * layout.line_height()
    }

    /// The open minibuffer's top: it has a line for its input and one for
    /// each candidate, as many as fit above the echo area.
    pub fn minibuffer_top(&self, layout: &Layout) -> f32 {
        let lh = layout.line_height();
        let lines = self.snap.as_ref().and_then(|s| s.minibuffer.as_ref()).map_or(0, |m| 1 + m.rows.len());
        let fit = (self.echo_top(layout) / lh).floor() as usize;
        self.echo_top(layout) - lines.min(fit) as f32 * lh
    }

    /// Where the latest snapshot's panes are: they share the lines above the
    /// minibuffer and the echo area equally, the first ones a line more when
    /// they do not divide evenly; the last also has what is left of a line.
    pub fn areas(&self, layout: &Layout) -> Vec<Area> {
        let (lh, bottom) = (layout.line_height(), self.hints_top(layout));
        let lines = (bottom / lh).floor() as usize;
        let gap = Self::gap(layout);
        let line = |f: f32| ((f * lines as f32).round() as usize).min(lines);
        let x = |f: f32| (f * self.width).round();
        let panes = self.snap.as_ref().map_or(&[][..], |s| &s.panes[..]);
        panes
            .iter()
            .map(|pane| {
                let p = pane.place;
                let (first, last) = (line(p.y), line(p.y + p.h));
                let top = first as f32 * lh;
                // The bottom panes also get what is left of a line.
                let end = if last == lines { bottom } else { last as f32 * lh };
                let (left, right) = (x(p.x), x(p.x + p.w));
                let has_gap = right < self.width - 0.5;
                let width = (right - left - if has_gap { gap } else { 0.0 }).max(1.0);
                // A gutter leaves the text at least a character.
                let gutter = pane.display.gutter(pane.text.len_lines()) as f32 * layout.char_width();
                Area {
                    top,
                    text: (end - top - lh).max(0.0),
                    mode_line: last > first,
                    left,
                    width,
                    gap: has_gap,
                    gutter: if gutter + layout.char_width() < width { gutter } else { 0.0 },
                }
            })
            .collect()
    }

    /// The gap between panes side by side, with a divider in its middle.
    pub fn gap(layout: &Layout) -> f32 {
        layout.line_height()
    }

    /// Take a snapshot. One that answers input scrolls panes whose caret it
    /// may have moved off screen: the focused pane when its caret or text
    /// changed, a new pane, every pane when there are more or fewer.
    pub fn take(&mut self, layout: &mut Layout, s: Snapshot) -> Vec<Input> {
        let old = self.snap.take();
        let moved = |i: usize, p: &Pane| {
            let before = old.as_ref().and_then(|o| Some((o, o.panes.iter().find(|q| q.view == p.view)?)));
            before
                .is_none_or(|(o, q)| o.panes.len() != s.panes.len() || (i == s.focus && (q.head() != p.head() || q.revision != p.revision)))
        };
        // What moves the lines arriving late (a server's inlays; the styles
        // of text scrolled to, counted in the base shape until they came;
        // hangs and images): the caret of the focused pane is kept on the
        // screen through them.
        let reflowed = |i: usize, p: &Pane| {
            i == s.focus
                && old.as_ref().and_then(|o| Some((o, o.panes.iter().find(|q| q.view == p.view)?))).is_some_and(|(o, q)| {
                    q.inlays != p.inlays || q.hangs != p.hangs || q.blocks != p.blocks || shape_runs(o, q) != shape_runs(&s, p)
                })
        };
        let scroll: Vec<usize> =
            s.panes.iter().enumerate().filter(|&(i, p)| (!s.answers.is_empty() && moved(i, p)) || reflowed(i, p)).map(|(i, _)| i).collect();
        let requests: Vec<(usize, ViewRequest)> = s.panes.iter().enumerate().filter_map(|(i, p)| Some((i, p.request?))).collect();
        self.anchors = s.panes.iter().map(|p| p.scroll).collect();
        self.snap = Some(s);
        let areas = self.areas(layout);
        let mut inputs: Vec<Input> = requests.iter().filter_map(|&(i, r)| self.resolve(layout, i, areas[i], r)).collect();
        let kept = scroll
            .into_iter()
            .filter(|i| !requests.iter().any(|(j, _)| j == i))
            .filter_map(|i| {
                let s = self.snap.as_ref()?;
                let p = &s.panes[i];
                prepare(layout, s, p, areas[i]);
                self.anchors[i] = layout.keep_visible(&p.text, self.anchors[i], p.head(), areas[i].text)?;
                Some(Input::Scroll { view: p.view, revision: p.revision, anchor: self.anchors[i], caret: None })
            })
            .collect::<Vec<_>>();
        inputs.extend(kept);
        inputs
    }

    /// A pane's request, resolved with its layout: where to scroll and
    /// where its caret goes.
    fn resolve(&mut self, layout: &mut Layout, i: usize, area: Area, request: ViewRequest) -> Option<Input> {
        let lh = layout.line_height();
        let rows = (area.text / lh).floor() as usize;
        let p = &self.snap.as_ref()?.panes[i];
        let (text, head, anchor) = (&p.text, p.head(), self.anchors[i]);
        if rows == 0 {
            return None;
        }
        prepare(layout, self.snap.as_ref()?, p, area);
        let placed = layout.frame(text, anchor, area.text);
        // Paging and recentring in pixels, in the heights drawn: a page
        // of tall lines is fewer lines, one of images fewer still.
        let (anchor, caret) = match request {
            ViewRequest::Page { screens, context } => {
                let px = if screens.abs() >= 1.0 { (area.text - context as f32 * lh).max(lh) * screens } else { area.text * screens };
                // Nothing further: the end (or the start) is on the screen.
                let at_end = placed.last().is_some_and(|q| q.seg.end >= text.len_bytes() && q.top + q.height() <= area.text + 0.5);
                if (px > 0.0 && at_end) || (px < 0.0 && layout.scroll_lines(text, anchor, -1) == anchor) {
                    return Some(Input::Edge { view: p.view, end: px > 0.0 });
                }
                let new = layout.scroll_pixels(text, anchor, px);
                // The caret keeps its place on the screen.
                let at = layout::caret(layout, &placed, text, head).map_or((0.0, 0.0), |r| (r.x, r.y + r.h * 0.5));
                let new_placed = layout.frame(text, new, area.text);
                (new, layout::hit(layout, &new_placed, at.0, at.1.min(area.text - lh * 0.5)))
            }
            ViewRequest::Recenter(at) => {
                let h = layout::caret(layout, &placed, text, head).map_or(lh, |r| r.h);
                let top = layout.scroll_lines(text, head, 0);
                let anchor = match at {
                    Recenter::Middle => layout.scroll_pixels(text, top, -((area.text - h) / 2.0).max(0.0)),
                    Recenter::Top => top,
                    Recenter::Bottom => layout.scroll_pixels(text, top, -(area.text - h).max(0.0)),
                };
                (anchor, None)
            }
        };
        let (view, revision) = (p.view, p.revision);
        self.anchors[i] = anchor;
        Some(Input::Scroll { view, revision, anchor, caret })
    }

    /// Lay out what the panes show for a frame.
    pub fn frame(&mut self, layout: &mut Layout) {
        layout.begin_frame();
        let areas = self.areas(layout);
        let Some(s) = &self.snap else { return };
        self.shown = s
            .panes
            .iter()
            .zip(areas)
            .zip(&self.anchors)
            .map(|((p, area), &anchor)| {
                prepare(layout, s, p, area);
                Shown { view: p.view, revision: p.revision, area, placed: layout.frame(&p.text, anchor, area.text) }
            })
            .collect();
    }

    /// A press of the button at (x, y): a click in the pane there.
    pub fn press(&mut self, layout: &Layout, x: f32, y: f32, extend: bool) -> Option<Input> {
        let shown = self.shown.iter().find(|s| {
            let a = s.area;
            a.top <= y && y < a.top + a.text && a.left <= x && x < a.left + a.width
        })?;
        self.dragging = Some(shown.view);
        click(layout, shown, x, y, extend)
    }

    /// The mouse moved with the button held: the selection of the pane the
    /// press was in extends to it.
    pub fn drag(&self, layout: &Layout, x: f32, y: f32) -> Option<Input> {
        click(layout, self.shown.iter().find(|s| Some(s.view) == self.dragging)?, x, y, true)
    }

    pub fn release(&mut self) {
        self.dragging = None;
    }

    /// The wheel at height `y`: scrolls the pane there by `lines`.
    pub fn wheel(&mut self, layout: &mut Layout, x: f32, y: f32, lines: i64) -> Option<Input> {
        let lh = layout.line_height();
        let shown = self.shown.iter().find(|s| {
            let a = s.area;
            a.top <= y && y < a.top + a.text + lh && a.left <= x && x < a.left + a.width + Self::gap(layout)
        })?;
        let (view, area) = (shown.view, shown.area);
        let s = self.snap.as_ref()?;
        let i = s.panes.iter().position(|p| p.view == view)?;
        let pane = &s.panes[i];
        prepare(layout, s, pane, area);
        self.anchors[i] = layout.scroll_lines(&pane.text, self.anchors[i], lines);
        Some(Input::Scroll { view, revision: pane.revision, anchor: self.anchors[i], caret: None })
    }
}

/// A click on a pane as shown, as a position in its revision; out of the
/// pane, on its nearest line.
fn click(layout: &Layout, shown: &Shown, x: f32, y: f32, extend: bool) -> Option<Input> {
    let last = shown.area.text - layout.line_height() * 0.5;
    if last < 0.0 {
        return None;
    }
    let y = (y - shown.area.top).clamp(0.0, last);
    let pos = layout::hit(layout, &shown.placed, (x - shown.area.text_left()).max(0.0), y)?;
    Some(Input::Click { view: shown.view, revision: shown.revision, pos, extend, at: Instant::now() })
}

/// The weight and slant of an inlay's style in the snapshot `snap`.
fn inlay_shape(snap: Option<&Snapshot>, id: techne_editor::present::StyleId) -> (Weight, Slant) {
    let style = snap.map(|s| s.style(id)).unwrap_or_default();
    (style.weight.unwrap_or(Weight::Normal), style.slant.unwrap_or(Slant::Normal))
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use techne_editor::{present::Input, runtime::Runtime};
    use techne_text::Document;

    use super::*;

    /// A screen of `rows` plain lines over a runtime on `text`, its text
    /// twice the frontend's size where a minor mode says so.
    fn screen(text: &str, rows: usize) -> (Runtime, Layout, Screen) {
        let layout = Layout::new(15.0, "monospace");
        let mut screen = Screen::default();
        screen.set_size(1000.0, (rows + 2) as f32 * layout.line_height());
        let mut rt = Runtime::with_document(Document::new(text), "emacs").unwrap();
        rt.eval(
            "(define-face big \"Twice the size.\" #:height 2.0)
             (define-minor-mode big-mode \"Large text.\" #:layer (lambda (d from to) (list (list from to 'big))))
             (run-command (current-session) 'big-mode 1)",
        )
        .unwrap();
        (rt, layout, screen)
    }

    fn run(rt: &mut Runtime, layout: &mut Layout, screen: &mut Screen, input: Input) {
        let mut inputs = vec![input];
        while !inputs.is_empty() {
            for i in inputs.drain(..) {
                rt.handle(i);
            }
            let s = rt.snapshot();
            inputs = screen.take(layout, s);
        }
        screen.frame(layout);
    }

    fn key(k: &str) -> Input {
        Input::Key { key: k.into(), at: Instant::now() }
    }

    /// Whether the focused pane's caret is shown whole.
    fn caret_shown(layout: &Layout, screen: &Screen) -> bool {
        let s = screen.snap.as_ref().unwrap();
        let (pane, shown) = (&s.panes[s.focus], &screen.shown[s.focus]);
        layout::caret(layout, &shown.placed, &pane.text, pane.head()).is_some_and(|c| c.y >= -0.5 && c.y + c.h <= shown.area.text + 0.5)
    }

    #[test]
    fn the_caret_stays_shown_when_the_styles_of_text_scrolled_to_arrive() {
        // Far down, the text is counted in the base shape until its
        // styles come with the next snapshot: then it is kept shown.
        let text: String = (0..15_000).map(|i| format!("line {i}\n")).collect();
        let (mut rt, mut layout, mut screen) = screen(&text, 20);
        let s = rt.snapshot();
        screen.take(&mut layout, s);
        run(&mut rt, &mut layout, &mut screen, key("M->"));
        assert!(caret_shown(&layout, &screen));
    }

    #[test]
    fn a_page_down_skips_nothing_of_tall_text() {
        let text: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let (mut rt, mut layout, mut screen) = screen(&text, 20);
        let s = rt.snapshot();
        screen.take(&mut layout, s);
        screen.frame(&mut layout);
        let shown = screen.shown[0].placed.iter().filter(|p| p.top + p.height() <= screen.shown[0].area.text + 0.5).count();
        run(&mut rt, &mut layout, &mut screen, key("C-v"));
        let s = screen.snap.as_ref().unwrap();
        let top = s.panes[0].text.byte_to_line(s.panes[0].scroll);
        assert!(top > 0 && top <= shown, "{top} lines on, {shown} shown before");
        assert!(caret_shown(&layout, &screen));
    }
}
