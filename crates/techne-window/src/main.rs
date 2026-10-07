//! `techne`: the editor in a window on the current desktop.
//!
//! The runtime (VM, documents, Lisp session) runs on its own thread
//! (`host`, which starts a new one with the unsaved edits when it crashes);
//! this thread owns the window, layout and drawing, as a frontend over the
//! presentation protocol (EDITOR.md, section 6). The session's panes are
//! stacked (`screen`), each with its mode line, the minibuffer and the echo
//! area below them.
//! Keys go to the runtime in Emacs notation; clicks and scrolling are
//! resolved here against the pane shown and sent as positions in its
//! revision.
//!
//! `--bench N` types N keys by itself and reports the time from each key to
//! the frame that shows its effect (with `TECHNE_BENCH_SLOW` set, also each
//! key that took 10 ms or more); `--load` keeps a Lisp task busy in the
//! runtime meanwhile. `headless.sh` runs it in a headless Wayland session.

mod keys;
mod layout;
#[cfg(test)]
mod parity;
mod render;
mod screen;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use glyphon::Buffer;
use techne_editor::{
    host::{Event, File, Host},
    present::{CursorShape, Input, Output},
    runtime::journal_for,
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::ModifiersState,
    raw_window_handle::{HasDisplayHandle, RawDisplayHandle},
    window::{Window, WindowId},
};

use crate::{
    layout::{Layout, Rect},
    render::{CURSOR, CURSOR_DIM, FOREGROUND, LINE_NUMBER, MODE_LINE, MODE_LINE_DIM, Paint, Renderer, Rgb, SELECTION, TextPiece},
    screen::Screen,
};

struct Args {
    /// Without one, the editor starts on an empty *scratch* buffer.
    path: Option<PathBuf>,
    profile: String,
    bench: Option<usize>,
    load: bool,
    font: String,
    size: f32,
}

const USAGE: &str = "usage: techne [--modal] [--font FAMILY] [--size PT] [--bench KEYS] [--load] [FILE]";

fn args() -> Result<Args, String> {
    let mut a = Args { path: None, profile: "emacs".into(), bench: None, load: false, font: "monospace".into(), size: 15.0 };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--modal" => a.profile = "modal".into(),
            "--load" => a.load = true,
            "--bench" => a.bench = Some(it.next().and_then(|n| n.parse().ok()).ok_or(USAGE)?),
            "--font" => a.font = it.next().ok_or(USAGE)?,
            "--size" => a.size = it.next().and_then(|n| n.parse().ok()).ok_or(USAGE)?,
            "-h" | "--help" => return Err(USAGE.into()),
            _ if a.path.is_none() && !arg.starts_with("--") => a.path = Some(PathBuf::from(arg)),
            _ => return Err(USAGE.into()),
        }
    }
    Ok(a)
}

/// Woken when the runtime has output.
struct Wake;

/// Latencies from input to the frame that showed it, and to the snapshot.
#[derive(Default)]
struct Latency {
    frame: Vec<Duration>,
    snapshot: Vec<Duration>,
}

impl Latency {
    fn report(&self) {
        let show = |name: &str, v: &[Duration]| {
            let mut v = v.to_vec();
            v.sort();
            if v.is_empty() {
                return;
            }
            let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
            eprintln!(
                "{name}: n={} p50={:.2}ms p99={:.2}ms max={:.2}ms",
                v.len(),
                at(0.5).as_secs_f64() * 1e3,
                at(0.99).as_secs_f64() * 1e3,
                v[v.len() - 1].as_secs_f64() * 1e3
            );
        };
        show("key to snapshot", &self.snapshot);
        show("key to frame", &self.frame);
    }
}

/// Keys the benchmark types: editing and moving, with jumps across the file.
const BENCH_KEYS: &[&str] =
    &["x", "y", "DEL", "C-n", "M-f", "C-b", "z", "C-n", "C-e", "DEL", "C-a", "C-p", "M->", "a", "DEL", "M-<", "C-n"];

struct Bench {
    keys: usize,
    sent: usize,
    next: Instant,
}

struct App {
    args: Args,
    host: Host,
    events: mpsc::Receiver<Event>,
    renderer: Option<Renderer>,
    layout: Layout,
    screen: Screen,
    /// Mode lines and the echo area, shaped, by their text.
    labels: HashMap<String, Buffer>,
    mods: ModifiersState,
    mouse: (f32, f32),
    /// The window is closing: the runtime ending is no crash.
    quitting: bool,
    unanswered: Vec<Instant>,
    latency: Latency,
    bench: Option<Bench>,
    /// The Wayland clipboard: kills go to it; what another program put
    /// there is sent to the runtime when the window gets the focus.
    clipboard: Option<smithay_clipboard::Clipboard>,
}

/// Text to draw: a shaped segment of a pane (by its key in the layout's
/// cache) or a label, from (left, top), clipped, in a colour.
struct Piece<'a> {
    source: Source<'a>,
    left: f32,
    top: f32,
    clip: Rect,
    color: Rgb,
}

enum Source<'a> {
    Segment(&'a str),
    Label(&'a str),
}

/// The width of a shaped label.
fn line_width(b: &Buffer) -> f32 {
    b.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
}

impl App {
    fn scale(&self) -> f32 {
        self.renderer.as_ref().map_or(1.0, |r| r.window.scale_factor() as f32)
    }

    fn pad(&self) -> f32 {
        8.0 * self.scale()
    }

    /// The window's size, and the screen's: text wraps inside the padding.
    fn size(&mut self) -> (f32, f32) {
        let (w, h) = self.renderer.as_ref().map_or((800.0, 600.0), |r| r.size());
        self.screen.set_size(w - 2.0 * self.pad(), h);
        (w, h)
    }

    fn send(&mut self, input: Input) {
        self.host.send(input);
    }

    fn redraw(&self) {
        if let Some(r) = &self.renderer {
            r.window.request_redraw();
        }
    }

    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        self.quitting = true;
        event_loop.exit();
    }

    fn take_outputs(&mut self, event_loop: &ActiveEventLoop) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Output(Output::Snapshot(s)) => {
                    let now = Instant::now();
                    self.latency.snapshot.extend(s.answers.iter().map(|at| now - *at));
                    self.unanswered.extend(s.answers.iter().copied());
                    self.size();
                    for input in self.screen.take(&mut self.layout, *s) {
                        self.send(input);
                    }
                }
                // Every key a window gets can be sent.
                Event::Output(Output::Bindings(_) | Output::Session(_)) => {}
                Event::Output(Output::Clipboard(text)) => {
                    if let Some(c) = &self.clipboard {
                        c.store(text);
                    }
                }
                Event::Output(Output::Quit) => self.quit(event_loop),
                Event::Failed(e) => {
                    eprintln!("techne: {e}");
                    self.quit(event_loop);
                }
                Event::Ended if self.quitting => {}
                Event::Ended if self.host.restart() => eprintln!("techne: the runtime stopped; a new one has the unsaved edits"),
                Event::Ended => {
                    eprintln!("techne: the runtime stopped before it had input");
                    self.quit(event_loop);
                }
            }
        }
        self.redraw();
    }

    fn draw(&mut self) {
        let (win_w, _) = self.size();
        let (pad, lh, scale) = (self.pad(), self.layout.line_height(), self.scale());
        let width = win_w - 2.0 * pad;
        self.screen.frame(&mut self.layout);
        let Some(snap) = &self.screen.snap else { return };

        let mut old = std::mem::take(&mut self.labels);
        let minibuffer = snap.minibuffer.iter().flat_map(|m| {
            let runs = m.rows.iter().flat_map(|r| r.columns.iter().flatten()).map(|r| r.text.clone());
            [m.prompt.clone(), m.input.clone(), m.input[..m.caret.min(m.input.len())].to_string()].into_iter().chain(runs)
        });
        let hints = snap.key_hints.iter().flat_map(|h| [h.key.clone(), h.description.clone()]).chain([" : ".to_string()]);
        // Each pane's gutter: line numbers on the first visual line of each
        // line, right-aligned, and the end-of-buffer marker below the text.
        let gutters: Vec<Vec<(f32, String, bool)>> = snap
            .panes
            .iter()
            .zip(&self.screen.shown)
            .map(|(pane, shown)| {
                let text = &pane.text;
                let current = text.byte_to_line(pane.head());
                let width = pane.display.gutter(text.len_lines());
                let numbers = shown.placed.iter().filter(|_| shown.area.gutter > 0.0).filter_map(|p| {
                    let n = text.byte_to_line(p.seg.start);
                    (text.line_to_byte(n) == p.seg.start).then(|| (p.top, pane.display.number(n, current, width).trim().to_string(), true))
                });
                let end = shown.placed.last().map_or(0.0, |p| p.top + p.lines as f32 * lh);
                let rows = ((shown.area.text - end).max(0.0) / lh).ceil() as usize;
                let markers = pane.display.eob_marker.iter().flat_map(|m| (0..rows).map(move |k| (end + k as f32 * lh, m.clone(), false)));
                numbers.chain(markers).collect()
            })
            .collect();
        let texts = snap
            .panes
            .iter()
            .map(|p| p.status.clone())
            .chain([snap.echo.clone()])
            .chain(minibuffer)
            .chain(hints)
            .chain(gutters.iter().flatten().map(|(_, t, _)| t.clone()));
        self.labels = texts
            .map(|t| {
                let b = old.remove(&t).unwrap_or_else(|| self.layout.label(&t, width));
                (t, b)
            })
            .collect();

        let mut rects: Vec<(Rect, Rgb)> = Vec::new();
        let mut pieces: Vec<Piece> = Vec::new();
        for (i, (pane, shown)) in snap.panes.iter().zip(&self.screen.shown).enumerate() {
            let focused = i == snap.focus;
            let area = shown.area;
            let pane_left = pad + area.left;
            let left = pad + area.text_left();
            let clip = Rect { x: left, y: area.top, w: area.text_width(), h: area.text };
            let gutter_clip = Rect { x: pane_left, y: area.top, w: area.width, h: area.text };
            for (y, text, number) in &gutters[i] {
                let w = self.labels.get(text).map_or(0.0, line_width);
                let (x, color) = if *number { (left - self.layout.char_width() - w, LINE_NUMBER) } else { (pane_left, FOREGROUND) };
                pieces.push(Piece { source: Source::Label(text), left: x, top: area.top + y, clip: gutter_clip, color });
            }
            let place = |r: Rect| Rect { x: r.x + left, y: r.y + area.top, ..r }.intersect(clip);
            let (first, last) = (shown.placed.first().map_or(0, |p| p.seg.start), shown.placed.last().map_or(0, |p| p.seg.end));
            let painted = pane.layers.iter().filter(|h| h.from <= last && h.to > first).filter_map(|h| Some((h, render::face(&h.face)?)));
            let mut fore: Vec<(Rect, Rgb)> = Vec::new();
            for (h, paint) in painted {
                let covered = layout::selection(&self.layout, &shown.placed, h.from, h.to);
                match paint {
                    Paint::Back(c) => rects.extend(covered.into_iter().filter_map(|r| Some((place(r)?, c)))),
                    Paint::Fore(c) => fore.extend(covered.into_iter().map(|r| (r, c))),
                }
            }
            for &(a, h) in pane.selections.iter().filter(|(a, h)| a != h) {
                let sel = layout::selection(&self.layout, &shown.placed, a.min(h), a.max(h));
                rects.extend(sel.into_iter().filter_map(|r| Some((place(r)?, SELECTION))));
            }
            if let Some(c) = layout::caret(&self.layout, &shown.placed, &pane.text, pane.head()) {
                let w = match pane.cursor {
                    CursorShape::Bar => 2.0 * scale,
                    CursorShape::Block => c.w,
                };
                rects.extend(place(Rect { w, ..c }).map(|r| (r, if focused && snap.minibuffer.is_none() { CURSOR } else { CURSOR_DIM })));
            }
            for p in &shown.placed {
                let lines = (0..p.lines).map(|k| p.top + k as f32 * lh);
                let on = |y: f32| fore.iter().filter(move |(r, _)| (r.y - y).abs() < 0.5);
                let piece = |clip: Rect, color| Piece { source: Source::Segment(&p.key), left, top: area.top + p.top, clip, color };
                if lines.clone().all(|y| on(y).next().is_none()) {
                    pieces.push(piece(clip, FOREGROUND));
                    continue;
                }
                // Foreground highlights: the segment's lines in spans, each
                // clipped to its colour's.
                for y in lines {
                    let colored: Vec<(f32, f32, Rgb)> = on(y).map(|(r, c)| (r.x + left, r.x + left + r.w, *c)).collect();
                    for (from, to, color) in render::spans(left, left + area.text_width(), &colored, FOREGROUND) {
                        let span = Rect { x: from, y: area.top + y, w: to - from, h: lh };
                        pieces.extend(span.intersect(clip).map(|clip| piece(clip, color)));
                    }
                }
            }
            // The mode line spans the pane and the gaps beside it; a divider
            // is in the middle of the gap on its right.
            let gap = Screen::gap(&self.layout);
            let from = if area.left > 0.0 { pane_left - gap / 2.0 } else { 0.0 };
            let to = if area.gap { pane_left + area.width + gap / 2.0 } else { win_w };
            if area.gap {
                rects.push((Rect { x: to - scale / 2.0, y: area.top, w: scale, h: area.text + lh }, MODE_LINE_DIM));
            }
            if area.mode_line {
                let line = Rect { x: from, y: area.top + area.text, w: to - from, h: lh };
                let (bg, fg) = if focused { (MODE_LINE, FOREGROUND) } else { (MODE_LINE_DIM, render::DIM) };
                rects.push((line, bg));
                pieces.push(Piece { source: Source::Label(&pane.status), left: pane_left, top: line.y, clip: line, color: fg });
            }
        }
        if let Some(m) = &snap.minibuffer {
            let top = self.screen.minibuffer_top(&self.layout);
            let w = |t: &str| self.labels.get(t).map_or(0.0, line_width);
            let line = |i: usize| Rect { x: 0.0, y: top + i as f32 * lh, w: win_w, h: lh };
            fn at(text: &str, left: f32, line: Rect, color: Rgb) -> Piece<'_> {
                Piece { source: Source::Label(text), left, top: line.y, clip: line, color }
            }
            let prompt = w(&m.prompt);
            if m.input_selected {
                rects.push((line(0), SELECTION));
            }
            pieces.push(at(&m.prompt, pad, line(0), FOREGROUND));
            pieces.push(at(&m.input, pad + prompt, line(0), FOREGROUND));
            let caret = prompt + w(&m.input[..m.caret.min(m.input.len())]);
            rects.push((Rect { x: pad + caret, y: top, w: 2.0 * scale, h: lh }, CURSOR));
            let lines = ((self.screen.echo_top(&self.layout) - top) / lh).round() as usize;
            let shown = &m.rows[..m.rows.len().min(lines.saturating_sub(1))];
            let columns = shown.iter().map(|r| r.columns.len()).max().unwrap_or(0);
            let gap = 2.0 * w(" ").max(lh / 3.0);
            let stops: Vec<f32> = (0..columns)
                .scan(0.0, |x, c| {
                    let stop = *x;
                    *x += shown
                        .iter()
                        .filter_map(|r| r.columns.get(c))
                        .map(|runs| runs.iter().map(|r| w(&r.text)).sum::<f32>())
                        .fold(0.0, f32::max)
                        + gap;
                    Some(stop)
                })
                .collect();
            for (i, row) in shown.iter().enumerate() {
                if m.selected == Some(i) {
                    rects.push((line(i + 1), SELECTION));
                }
                for (runs, &stop) in row.columns.iter().zip(&stops) {
                    let mut x = stop;
                    for run in runs {
                        let color = match run.face.as_deref().and_then(render::face) {
                            Some(Paint::Fore(c)) => c,
                            Some(Paint::Back(c)) => {
                                rects.push((Rect { x: pad + x, w: w(&run.text), ..line(i + 1) }, c));
                                FOREGROUND
                            }
                            None => FOREGROUND,
                        };
                        pieces.push(at(&run.text, pad + x, line(i + 1), color));
                        x += w(&run.text);
                    }
                }
            }
        }
        // which-key: keys right-aligned in their column, " : ", what they do.
        let (cw, top) = (self.layout.char_width(), self.screen.hints_top(&self.layout));
        let mut column_left = 0;
        for column in self.screen.hint_columns(&self.layout) {
            let keys = column.iter().map(|h| h.key.chars().count()).max().unwrap_or(0);
            let descriptions = column.iter().map(|h| h.description.chars().count()).max().unwrap_or(0);
            for (r, h) in column.iter().enumerate() {
                let y = top + r as f32 * lh;
                let key = column_left + keys - h.key.chars().count();
                let face = |name: &str| match render::face(name) {
                    Some(Paint::Fore(c)) => c,
                    _ => FOREGROUND,
                };
                let line = Rect { x: 0.0, y, w: win_w, h: lh };
                let at = |text: &'static str, chars: usize, color| Piece {
                    source: Source::Label(text),
                    left: pad + chars as f32 * cw,
                    top: y,
                    clip: line,
                    color,
                };
                pieces.push(Piece { source: Source::Label(&h.key), left: pad + key as f32 * cw, top: y, clip: line, color: face("key") });
                pieces.push(at(" : ", column_left + keys, face("comment")));
                let color = if h.prefix { face("keyword") } else { FOREGROUND };
                pieces.push(Piece {
                    source: Source::Label(&h.description),
                    left: pad + (column_left + keys + 3) as f32 * cw,
                    top: y,
                    clip: line,
                    color,
                });
            }
            column_left += keys + 3 + descriptions + techne_editor::hints::GAP;
        }
        let echo = Rect { x: 0.0, y: self.screen.echo_top(&self.layout), w: win_w, h: lh };
        pieces.push(Piece { source: Source::Label(&snap.echo), left: pad, top: echo.y, clip: echo, color: FOREGROUND });

        let (fonts, buffers) = self.layout.split();
        let texts: Vec<TextPiece> = pieces
            .iter()
            .filter_map(|p| {
                let buffer = match p.source {
                    Source::Segment(key) => buffers.get(key)?,
                    Source::Label(text) => self.labels.get(text)?,
                };
                Some(TextPiece { buffer, left: p.left, top: p.top, clip: p.clip, color: p.color })
            })
            .collect();
        let Some(renderer) = &mut self.renderer else { return };
        if renderer.draw(fonts, &rects, &texts) {
            let now = Instant::now();
            self.latency.frame.extend(self.unanswered.drain(..).map(|at| now - at));
        } else {
            renderer.window.request_redraw();
        }
    }

    fn bench_step(&mut self, event_loop: &ActiveEventLoop) {
        let Some(b) = &mut self.bench else { return };
        let now = Instant::now();
        if b.sent < b.keys {
            if now >= b.next {
                let key = BENCH_KEYS[b.sent % BENCH_KEYS.len()].to_string();
                b.sent += 1;
                b.next = now + Duration::from_millis(12);
                let next = b.next;
                self.send(Input::Key { key, at: now });
                event_loop.set_control_flow(ControlFlow::WaitUntil(next));
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(b.next));
            }
        } else if self.latency.frame.len() >= b.keys {
            self.latency.report();
            if std::env::var_os("TECHNE_BENCH_SLOW").is_some() {
                for (i, d) in self.latency.frame.iter().enumerate().filter(|(_, d)| d.as_millis() >= 10) {
                    eprintln!("slow: key {i} {:?} {:.1}ms", BENCH_KEYS[i % BENCH_KEYS.len()], d.as_secs_f64() * 1e3);
                }
            }
            self.quit(event_loop);
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(5)));
        }
    }
}

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        let title = format!("techne — {}", self.args.path.as_ref().map_or("*scratch*".into(), |p| p.display().to_string()));
        let attrs = Window::default_attributes().with_title(title).with_inner_size(winit::dpi::LogicalSize::new(900.0, 700.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("a window"));
        let scale = window.scale_factor() as f32;
        self.layout = Layout::new(self.args.size * scale, &self.args.font);
        self.clipboard = match window.display_handle().map(|h| h.as_raw()) {
            // SAFETY: the display outlives the clipboard: it is dropped in
            // `exiting`, before the event loop closes the connection.
            Ok(RawDisplayHandle::Wayland(h)) => Some(unsafe { smithay_clipboard::Clipboard::new(h.display.as_ptr()) }),
            _ => None,
        };
        self.renderer = Some(Renderer::new(window, event_loop.owned_display_handle()));
        if let Some(b) = &mut self.bench {
            b.next = Instant::now() + Duration::from_millis(300);
        }
        self.redraw();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: Wake) {
        self.take_outputs(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.quit(event_loop),
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
                self.redraw();
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::Focused(true) => {
                if let Some(text) = self.clipboard.as_ref().and_then(|c| c.load().ok()) {
                    self.send(Input::Clipboard { text });
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && self.bench.is_none() => {
                if let Some(key) = keys::key_name(&event.logical_key, self.mods) {
                    self.send(Input::Key { key, at: Instant::now() });
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse = (position.x as f32, position.y as f32);
                if let Some(i) = self.screen.drag(&self.layout, self.mouse.0 - self.pad(), self.mouse.1) {
                    self.send(i);
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                if state == ElementState::Pressed {
                    let (x, y) = (self.mouse.0 - self.pad(), self.mouse.1);
                    if let Some(i) = self.screen.press(&self.layout, x, y, self.mods.shift_key()) {
                        self.send(i);
                    }
                } else {
                    self.screen.release();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (-y * 3.0).round() as i64,
                    MouseScrollDelta::PixelDelta(p) => (-p.y as f32 / self.layout.line_height()).round() as i64,
                };
                if let Some(i) = (lines != 0)
                    .then(|| {
                        let x = self.mouse.0 - self.pad();
                        self.screen.wheel(&mut self.layout, x, self.mouse.1, lines)
                    })
                    .flatten()
                {
                    self.send(i);
                    self.redraw();
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.bench_step(event_loop);
    }

    /// The clipboard goes while the Wayland connection it uses is still
    /// open: the event loop closes it when it returns, before `App` goes.
    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.clipboard = None;
        self.renderer = None;
    }
}

fn main() {
    let args = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    // A benchmark's edits are journaled in a temporary file, so they never
    // turn up as unsaved edits of the real one.
    let file = args.path.clone().map(|path| {
        let journal = if args.bench.is_some() {
            std::env::temp_dir().join(format!("techne-bench-{}.journal", std::process::id()))
        } else {
            journal_for(&path).unwrap_or_else(|e| {
                eprintln!("techne: journal: {e}");
                std::process::exit(1)
            })
        };
        File { path, journal }
    });
    let event_loop = EventLoop::<Wake>::with_user_event().build().expect("an event loop");
    let proxy = event_loop.create_proxy();
    let (tx, events) = mpsc::channel();
    let load = args.load;
    let setup = move |rt: &mut techne_editor::runtime::Runtime| {
        if load {
            // Allocates and computes forever, yielding only when preempted.
            let busy =
                "(lambda () (let loop ((i 0) (acc '())) (loop (+ i 1) (if (> (length acc) 200) '() (cons (make-vector 16 i) acc)))))";
            if let Err(e) = rt.spawn(busy) {
                eprintln!("techne: --load: {e}");
            }
        }
    };
    let host = Host::start(file.clone(), args.profile.clone(), setup, move |e| {
        let _ = tx.send(e);
        let _ = proxy.send_event(Wake);
    });
    let bench = args.bench.map(|keys| Bench { keys, sent: 0, next: Instant::now() });
    let mut app = App {
        args,
        host,
        events,
        renderer: None,
        layout: Layout::new(15.0, "monospace"),
        screen: Screen::default(),
        labels: HashMap::new(),
        mods: ModifiersState::empty(),
        mouse: (0.0, 0.0),
        quitting: false,
        unanswered: Vec::new(),
        latency: Latency::default(),
        bench,
        clipboard: None,
    };
    event_loop.run_app(&mut app).expect("the event loop");
    let App { host, .. } = app;
    host.close();
    if let Some(f) = file.filter(|f| f.journal.starts_with(std::env::temp_dir())) {
        let _ = std::fs::remove_file(&f.journal);
    }
}
