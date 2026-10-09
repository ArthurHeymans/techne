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
    present::{Input, KeyHint, Pane, Recenter, Snapshot, ViewRequest},
};

use crate::layout::{self, Layout, Placed};

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
        let scroll: Vec<usize> = if s.answers.is_empty() {
            Vec::new()
        } else {
            s.panes.iter().enumerate().filter(|&(i, p)| moved(i, p)).map(|(i, _)| i).collect()
        };
        let requests: Vec<(usize, ViewRequest)> = s.panes.iter().enumerate().filter_map(|(i, p)| Some((i, p.request?))).collect();
        self.anchors = s.panes.iter().map(|p| p.scroll).collect();
        self.snap = Some(s);
        let areas = self.areas(layout);
        let mut inputs: Vec<Input> = requests.iter().filter_map(|&(i, r)| self.resolve(layout, i, areas[i], r)).collect();
        let kept = scroll
            .into_iter()
            .filter(|i| !requests.iter().any(|(j, _)| j == i))
            .filter_map(|i| {
                let p = &self.snap.as_ref()?.panes[i];
                layout.set_width(areas[i].text_width());
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
        layout.set_width(area.text_width());
        let placed = layout.frame(text, anchor, area.text);
        let (anchor, caret) = match request {
            ViewRequest::Page { screens, context } => {
                let n = if screens.abs() >= 1.0 { rows.saturating_sub(context).max(1) as f32 * screens } else { rows as f32 * screens };
                let n = n.round() as i64;
                // Nothing further: the end (or the start) is on the screen.
                let at_end = placed.last().is_some_and(|q| q.seg.end >= text.len_bytes() && q.top + q.lines as f32 * lh <= area.text + 0.5);
                if (n > 0 && at_end) || (n < 0 && layout.scroll_lines(text, anchor, -1) == anchor) {
                    return Some(Input::Edge { view: p.view, end: n > 0 });
                }
                let new = layout.scroll_lines(text, anchor, n);
                // The caret keeps its place on the screen.
                let at = layout::caret(layout, &placed, text, head).map_or((0.0, 0.0), |r| (r.x, r.y + r.h * 0.5));
                let new_placed = layout.frame(text, new, area.text);
                (new, layout::hit(layout, &new_placed, at.0, at.1.min(area.text - lh * 0.5)))
            }
            ViewRequest::Recenter(at) => {
                let k = match at {
                    Recenter::Middle => rows / 2,
                    Recenter::Top => 0,
                    Recenter::Bottom => rows - 1,
                };
                (layout.scroll_lines(text, head, -(k as i64)), None)
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
                layout.set_width(area.text_width());
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
        let (view, width) = (shown.view, shown.area.text_width());
        let s = self.snap.as_ref()?;
        let i = s.panes.iter().position(|p| p.view == view)?;
        let pane = &s.panes[i];
        layout.set_width(width);
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
