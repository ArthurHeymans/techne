//! A snapshot's panes in the window, without drawing: where each pane is,
//! its scroll anchor and its segments laid out, and what clicks, the wheel
//! and new snapshots mean for them. Panes are stacked in whole lines, each
//! with its mode line below its text; the echo area is the window's last
//! line. Positions here are in pixels from the text's left edge and the
//! window's top.

use std::time::Instant;

use techne_editor::present::{Input, Pane, Snapshot};

use crate::layout::{self, Layout, Placed};

/// Where a pane is: its text from `top`, `text` pixels high, then its mode
/// line (when it has a line at all).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub top: f32,
    pub text: f32,
    pub mode_line: bool,
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

    /// Where the latest snapshot's panes are: they share the lines above the
    /// echo area equally, the first ones a line more when they do not divide
    /// evenly; the last also has what is left of a line.
    pub fn areas(&self, layout: &Layout) -> Vec<Area> {
        let n = self.snap.as_ref().map_or(0, |s| s.panes.len());
        let (lh, bottom) = (layout.line_height(), self.echo_top(layout));
        let lines = (bottom / lh).floor() as usize;
        (0..n)
            .scan(0, |line, i| {
                let height = lines / n + usize::from(i < lines % n);
                let top = *line as f32 * lh;
                *line += height;
                let end = if i + 1 == n { bottom } else { *line as f32 * lh };
                Some(Area { top, text: (end - top - lh).max(0.0), mode_line: height > 0 })
            })
            .collect()
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
        layout.set_width(self.width);
        let areas = self.areas(layout);
        scroll
            .into_iter()
            .filter_map(|i| {
                let p = &self.snap.as_ref()?.panes[i];
                self.anchors[i] = layout.keep_visible(&p.text, self.anchors[i], p.head(), areas[i].text)?;
                Some(Input::Scroll { view: p.view, revision: p.revision, anchor: self.anchors[i] })
            })
            .collect()
    }

    /// Lay out what the panes show for a frame.
    pub fn frame(&mut self, layout: &mut Layout) {
        layout.begin_frame();
        layout.set_width(self.width);
        let areas = self.areas(layout);
        let Some(s) = &self.snap else { return };
        self.shown = s
            .panes
            .iter()
            .zip(areas)
            .zip(&self.anchors)
            .map(|((p, area), &anchor)| Shown {
                view: p.view,
                revision: p.revision,
                area,
                placed: layout.frame(&p.text, anchor, area.text),
            })
            .collect();
    }

    /// A press of the button at (x, y): a click in the pane there.
    pub fn press(&mut self, layout: &Layout, x: f32, y: f32, extend: bool) -> Option<Input> {
        let shown = self.shown.iter().find(|s| s.area.top <= y && y < s.area.top + s.area.text)?;
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
    pub fn wheel(&mut self, layout: &mut Layout, y: f32, lines: i64) -> Option<Input> {
        let lh = layout.line_height();
        let view = self.shown.iter().find(|s| s.area.top <= y && y < s.area.top + s.area.text + lh)?.view;
        let s = self.snap.as_ref()?;
        let i = s.panes.iter().position(|p| p.view == view)?;
        let pane = &s.panes[i];
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
    let pos = layout::hit(layout, &shown.placed, x, y)?;
    Some(Input::Click { view: shown.view, revision: shown.revision, pos, extend, at: Instant::now() })
}
