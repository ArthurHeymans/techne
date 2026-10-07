//! A snapshot's panes in the window, without drawing: where each pane is,
//! its scroll anchor and its segments laid out, and what clicks, the wheel
//! and new snapshots mean for them. Each pane is in its place, rounded to
//! whole lines, with its mode line below its text, and a gap with a
//! divider between it and a pane to its right; the open minibuffer is
//! below them (its input line, then its candidates), and the echo area is
//! the window's last line. Positions here are in pixels from the text's
//! left edge and the window's top.

use std::time::Instant;

use techne_editor::present::{Input, Pane, Place, Snapshot};

use crate::layout::{self, Layout, Placed};

/// Where a pane is: its text from `top`, `text` pixels high, then its mode
/// line (when it has a line at all); from `left`, `width` pixels wide,
/// then a gap when another pane is to its right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub top: f32,
    pub text: f32,
    pub mode_line: bool,
    pub left: f32,
    pub width: f32,
    pub gap: bool,
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

    /// The echo area's top.
    pub fn echo_top(&self, layout: &Layout) -> f32 {
        (self.height - layout.line_height()).max(0.0)
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
        let (lh, bottom) = (layout.line_height(), self.minibuffer_top(layout));
        let lines = (bottom / lh).floor() as usize;
        let gap = Self::gap(layout);
        let line = |f: f32| ((f * lines as f32).round() as usize).min(lines);
        let x = |f: f32| (f * self.width).round();
        let places: Vec<Place> = self.snap.as_ref().map_or(Vec::new(), |s| s.panes.iter().map(|p| p.place).collect());
        places
            .iter()
            .map(|p| {
                let (first, last) = (line(p.y), line(p.y + p.h));
                let top = first as f32 * lh;
                // The bottom panes also get what is left of a line.
                let end = if last == lines { bottom } else { last as f32 * lh };
                let (left, right) = (x(p.x), x(p.x + p.w));
                let has_gap = right < self.width - 0.5;
                Area {
                    top,
                    text: (end - top - lh).max(0.0),
                    mode_line: last > first,
                    left,
                    width: (right - left - if has_gap { gap } else { 0.0 }).max(1.0),
                    gap: has_gap,
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
        self.anchors = s.panes.iter().map(|p| p.scroll).collect();
        self.snap = Some(s);
        let areas = self.areas(layout);
        scroll
            .into_iter()
            .filter_map(|i| {
                let p = &self.snap.as_ref()?.panes[i];
                layout.set_width(areas[i].width);
                self.anchors[i] = layout.keep_visible(&p.text, self.anchors[i], p.head(), areas[i].text)?;
                Some(Input::Scroll { view: p.view, revision: p.revision, anchor: self.anchors[i] })
            })
            .collect()
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
                layout.set_width(area.width);
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
        let (view, width) = (shown.view, shown.area.width);
        let s = self.snap.as_ref()?;
        let i = s.panes.iter().position(|p| p.view == view)?;
        let pane = &s.panes[i];
        layout.set_width(width);
        self.anchors[i] = layout.scroll_lines(&pane.text, self.anchors[i], lines);
        Some(Input::Scroll { view, revision: pane.revision, anchor: self.anchors[i] })
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
    let pos = layout::hit(layout, &shown.placed, x - shown.area.left, y)?;
    Some(Input::Click { view: shown.view, revision: shown.revision, pos, extend, at: Instant::now() })
}
