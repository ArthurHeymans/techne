//! `techne`: the editor in a window on the current desktop.
//!
//! The runtime (VM, document, Lisp session) runs on its own thread; this
//! thread owns the window, layout and drawing, as a frontend over the
//! presentation protocol (EDITOR.md, section 6). Keys go to the runtime in
//! Emacs notation; clicks and scrolling are resolved here against the
//! snapshot shown and sent as positions in its revision.
//!
//! `--bench N` types N keys by itself and reports the time from each key to
//! the frame that shows its effect (with `TECHNE_BENCH_SLOW` set, also each
//! key that took 10 ms or more); `--load` keeps a Lisp task busy in the
//! runtime meanwhile. `headless.sh` runs it in a headless Wayland session.

mod keys;
mod layout;
mod render;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use techne_editor::{
    present::{CursorShape, Input, Output, Snapshot},
    runtime::Runtime,
};
use techne_vm::tasks::Progress;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::ModifiersState,
    window::{Window, WindowId},
};

use crate::{
    layout::{Layout, Placed, Rect},
    render::{CURSOR, FOREGROUND, Renderer, Rgb, SELECTION, STATUS_BG, TextPiece},
};

struct Args {
    path: PathBuf,
    profile: String,
    bench: Option<usize>,
    load: bool,
    font: String,
    size: f32,
}

const USAGE: &str = "usage: techne [--modal] [--font FAMILY] [--size PT] [--bench KEYS] [--load] FILE";

fn args() -> Result<Args, String> {
    let mut a = Args { path: PathBuf::new(), profile: "emacs".into(), bench: None, load: false, font: "monospace".into(), size: 15.0 };
    let mut it = std::env::args().skip(1);
    let mut path = None;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--modal" => a.profile = "modal".into(),
            "--load" => a.load = true,
            "--bench" => a.bench = Some(it.next().and_then(|n| n.parse().ok()).ok_or(USAGE)?),
            "--font" => a.font = it.next().ok_or(USAGE)?,
            "--size" => a.size = it.next().and_then(|n| n.parse().ok()).ok_or(USAGE)?,
            "-h" | "--help" => return Err(USAGE.into()),
            _ if path.is_none() && !arg.starts_with("--") => path = Some(PathBuf::from(arg)),
            _ => return Err(USAGE.into()),
        }
    }
    a.path = path.ok_or(USAGE)?;
    Ok(a)
}

/// Unsaved edits of a file are journaled under the state directory, named by
/// a hash of the file's absolute path.
fn journal_for(path: &Path) -> std::io::Result<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .ok_or_else(|| std::io::Error::other("no XDG_STATE_HOME or HOME"))?;
    let dir = state.join("techne/journals");
    std::fs::create_dir_all(&dir)?;
    let abs = std::path::absolute(path)?;
    let hash = techne_text::journal::hash(abs.as_os_str().as_encoded_bytes());
    let name: String = hash[..12].iter().map(|b| format!("{b:02x}")).collect();
    Ok(dir.join(format!("{name}.journal")))
}

/// Woken when the runtime has output.
struct Wake;

/// The runtime thread: handle inputs as they come, answering each batch with
/// a snapshot; between inputs, run background Lisp tasks.
fn run_runtime(mut rt: Runtime, inputs: mpsc::Receiver<Input>, out: mpsc::Sender<Output>, wake: EventLoopProxy<Wake>) {
    let send = |o: Output| {
        let _ = out.send(o);
        let _ = wake.send_event(Wake);
    };
    send(Output::Snapshot(Box::new(rt.snapshot())));
    let mut busy = rt.run_tasks(Duration::ZERO) == Progress::OutOfTime;
    loop {
        let first = if busy {
            match inputs.try_recv() {
                Ok(i) => i,
                Err(mpsc::TryRecvError::Empty) => {
                    busy = rt.run_tasks(Duration::from_millis(2)) == Progress::OutOfTime;
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        } else {
            match inputs.recv() {
                Ok(i) => i,
                Err(_) => return,
            }
        };
        let mut quit = false;
        for input in std::iter::once(first).chain(inputs.try_iter()) {
            quit |= matches!(rt.handle(input), Some(Output::Quit));
        }
        send(Output::Snapshot(Box::new(rt.snapshot())));
        if quit {
            send(Output::Quit);
            return;
        }
        busy |= rt.run_tasks(Duration::ZERO) == Progress::OutOfTime;
    }
}

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
    inputs: mpsc::Sender<Input>,
    outputs: mpsc::Receiver<Output>,
    renderer: Option<Renderer>,
    layout: Layout,
    snap: Option<Snapshot>,
    anchor: usize,
    placed: Vec<Placed>,
    status: Option<(String, glyphon::Buffer)>,
    mods: ModifiersState,
    mouse: (f32, f32),
    dragging: bool,
    unanswered: Vec<Instant>,
    latency: Latency,
    bench: Option<Bench>,
}

impl App {
    fn scale(&self) -> f32 {
        self.renderer.as_ref().map_or(1.0, |r| r.window.scale_factor() as f32)
    }

    fn pad(&self) -> f32 {
        8.0 * self.scale()
    }

    /// The text area: (width, height) above the status line.
    fn text_area(&self) -> (f32, f32) {
        let (w, h) = self.renderer.as_ref().map_or((800.0, 600.0), |r| r.size());
        (w - 2.0 * self.pad(), (h - self.layout.line_height()).max(self.layout.line_height()))
    }

    fn send(&self, input: Input) {
        let _ = self.inputs.send(input);
    }

    fn redraw(&self) {
        if let Some(r) = &self.renderer {
            r.window.request_redraw();
        }
    }

    fn take_outputs(&mut self, event_loop: &ActiveEventLoop) {
        while let Ok(out) = self.outputs.try_recv() {
            match out {
                Output::Snapshot(s) => {
                    let now = Instant::now();
                    self.latency.snapshot.extend(s.answers.iter().map(|at| now - *at));
                    self.unanswered.extend(s.answers.iter().copied());
                    let moved = self.snap.as_ref().is_none_or(|old| old.head() != s.head() || old.revision != s.revision);
                    self.anchor = s.scroll;
                    let answers_input = !s.answers.is_empty();
                    self.snap = Some(*s);
                    if answers_input && moved {
                        self.keep_caret_visible();
                    }
                }
                Output::Quit => event_loop.exit(),
            }
        }
        self.redraw();
    }

    fn keep_caret_visible(&mut self) {
        let (width, height) = self.text_area();
        self.layout.set_width(width);
        let Some(s) = &self.snap else { return };
        if let Some(a) = self.layout.keep_visible(&s.text, self.anchor, s.head(), height) {
            self.anchor = a;
            self.send(Input::Scroll { revision: s.revision, anchor: a });
        }
    }

    fn scroll_by(&mut self, lines: i64) {
        let Some(s) = &self.snap else { return };
        self.anchor = self.layout.scroll_lines(&s.text, self.anchor, lines);
        self.send(Input::Scroll { revision: s.revision, anchor: self.anchor });
        self.redraw();
    }

    fn click(&mut self, extend: bool) {
        let Some(s) = &self.snap else { return };
        let (x, y) = (self.mouse.0 - self.pad(), self.mouse.1);
        if let Some(pos) = layout::hit(&self.layout, &self.placed, x, y) {
            self.send(Input::Click { revision: s.revision, pos, extend, at: Instant::now() });
        }
    }

    fn draw(&mut self) {
        let (width, height) = self.text_area();
        let pad = self.pad();
        let lh = self.layout.line_height();
        let Some(snap) = &self.snap else { return };
        self.layout.begin_frame();
        self.layout.set_width(width);
        self.placed = self.layout.frame(&snap.text, self.anchor, height);

        let clip = Rect { x: pad, y: 0.0, w: width, h: height };
        let shift = |r: Rect| Rect { x: r.x + pad, ..r };
        let mut rects: Vec<(Rect, Rgb)> = Vec::new();
        for &(a, h) in &snap.selections {
            if a != h {
                let sel = layout::selection(&self.layout, &self.placed, a.min(h), a.max(h));
                rects.extend(sel.into_iter().map(|r| (shift(r), SELECTION)));
            }
        }
        if let Some(c) = layout::caret(&self.layout, &self.placed, &snap.text, snap.head()) {
            let c = shift(c);
            let w = match snap.cursor {
                CursorShape::Bar => 2.0 * self.scale(),
                CursorShape::Block => c.w,
            };
            rects.push((Rect { w, ..c }, CURSOR));
        }
        let (win_w, _) = self.renderer.as_ref().map_or((0.0, 0.0), |r| r.size());
        rects.push((Rect { x: 0.0, y: height, w: win_w, h: lh }, STATUS_BG));
        let rects: Vec<_> = rects.into_iter().filter(|(r, _)| r.y + r.h > 0.0 && r.y < height + lh).collect();

        if self.status.as_ref().is_none_or(|(t, _)| *t != snap.status) {
            let mut b = glyphon::Buffer::new(&mut self.layout.fonts, glyphon::Metrics::new(lh / 1.35, lh));
            b.set_size(Some(width), Some(lh));
            b.set_text(&snap.status, &glyphon::Attrs::new().family(glyphon::Family::SansSerif), glyphon::Shaping::Advanced, None);
            b.shape_until_scroll(&mut self.layout.fonts, false);
            self.status = Some((snap.status.clone(), b));
        }
        let (fonts, buffers) = self.layout.split();
        let mut texts: Vec<TextPiece> = self
            .placed
            .iter()
            .filter_map(|p| buffers.get(&p.key).map(|b| TextPiece { buffer: b, left: pad, top: p.top, clip, color: FOREGROUND }))
            .collect();
        if let Some((_, b)) = &self.status {
            texts.push(TextPiece {
                buffer: b,
                left: pad,
                top: height,
                clip: Rect { x: 0.0, y: height, w: win_w, h: lh },
                color: FOREGROUND,
            });
        }
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
                let _ = self.inputs.send(Input::Key { key, at: now });
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(b.next));
        } else if self.latency.frame.len() >= b.keys {
            self.latency.report();
            if std::env::var_os("TECHNE_BENCH_SLOW").is_some() {
                for (i, d) in self.latency.frame.iter().enumerate().filter(|(_, d)| d.as_millis() >= 10) {
                    eprintln!("slow: key {i} {:?} {:.1}ms", BENCH_KEYS[i % BENCH_KEYS.len()], d.as_secs_f64() * 1e3);
                }
            }
            let _ = self.inputs.send(Input::Close);
            event_loop.exit();
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
        let title = format!("techne — {}", self.args.path.display());
        let attrs = Window::default_attributes().with_title(title).with_inner_size(winit::dpi::LogicalSize::new(900.0, 700.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("a window"));
        let scale = window.scale_factor() as f32;
        self.layout = Layout::new(self.args.size * scale, &self.args.font);
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
            WindowEvent::CloseRequested => {
                self.send(Input::Close);
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
                self.redraw();
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && self.bench.is_none() => {
                if let Some(key) = keys::key_name(&event.logical_key, self.mods) {
                    self.send(Input::Key { key, at: Instant::now() });
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse = (position.x as f32, position.y as f32);
                if self.dragging {
                    self.click(true);
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                self.dragging = state == ElementState::Pressed;
                if self.dragging {
                    self.click(self.mods.shift_key());
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (-y * 3.0).round() as i64,
                    MouseScrollDelta::PixelDelta(p) => (-p.y as f32 / self.layout.line_height()).round() as i64,
                };
                if lines != 0 {
                    self.scroll_by(lines);
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.bench_step(event_loop);
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
    let journal = if args.bench.is_some() {
        std::env::temp_dir().join(format!("techne-bench-{}.journal", std::process::id()))
    } else {
        journal_for(&args.path).unwrap_or_else(|e| {
            eprintln!("techne: journal: {e}");
            std::process::exit(1)
        })
    };
    let event_loop = EventLoop::<Wake>::with_user_event().build().expect("an event loop");
    let proxy = event_loop.create_proxy();
    let (in_tx, in_rx) = mpsc::channel();
    let (out_tx, out_rx) = mpsc::channel();
    let load = args.load;
    // The VM is not Send: the runtime is made on its own thread.
    let (path, profile, journal2) = (args.path.clone(), args.profile.clone(), journal.clone());
    let runtime = std::thread::spawn(move || {
        let (mut rt, _) = match Runtime::open(&path, &journal2, &profile) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("techne: {e}");
                let _ = out_tx.send(Output::Quit);
                let _ = proxy.send_event(Wake);
                return;
            }
        };
        if load {
            // Allocates and computes forever, yielding only when preempted.
            let busy =
                "(lambda () (let loop ((i 0) (acc '())) (loop (+ i 1) (if (> (length acc) 200) '() (cons (make-vector 16 i) acc)))))";
            if let Err(e) = rt.spawn(busy) {
                eprintln!("techne: --load: {e}");
            }
        }
        run_runtime(rt, in_rx, out_tx, proxy);
    });
    let bench = args.bench.map(|keys| Bench { keys, sent: 0, next: Instant::now() });
    let mut app = App {
        args,
        inputs: in_tx,
        outputs: out_rx,
        renderer: None,
        layout: Layout::new(15.0, "monospace"),
        snap: None,
        anchor: 0,
        placed: Vec::new(),
        status: None,
        mods: ModifiersState::empty(),
        mouse: (0.0, 0.0),
        dragging: false,
        unanswered: Vec::new(),
        latency: Latency::default(),
        bench,
    };
    event_loop.run_app(&mut app).expect("the event loop");
    drop(app);
    let _ = runtime.join();
    if journal.starts_with(std::env::temp_dir()) {
        let _ = std::fs::remove_file(&journal);
    }
}
