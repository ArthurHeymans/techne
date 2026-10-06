//! The window and the terminal frontend give the same semantic state for
//! the same session (PLAN.md, Stage 1, slice 3). Each frontend gets the
//! input it would get from its platform: winit keys and pixel positions
//! here, the bytes a terminal with the kitty keyboard protocol sends there.
//! Each resolves keys, clicks and scrolling with its own normalizer and
//! layout against a runtime of its own; after every step the documents,
//! selections and scroll anchors must agree, and undoing afterwards must
//! step through the same units.
//!
//! The window's glue below (snapshot arrival, keeping the caret visible,
//! clicks, the wheel) is `App`'s without the window.

use std::time::Instant;

use techne_editor::{
    present::{Input, Output, Snapshot},
    runtime::Runtime,
};
use techne_term::{Grid, Term};
use techne_text::Document;
use winit::keyboard::{Key, ModifiersState as M, NamedKey};

use crate::{
    keys,
    layout::{self, Layout, Placed},
};

/// Visible text lines in both frontends.
const ROWS: usize = 20;

struct Window {
    rt: Runtime,
    layout: Layout,
    snap: Option<Snapshot>,
    anchor: usize,
    placed: Vec<Placed>,
    height: f32,
}

impl Window {
    fn new(text: &str) -> Window {
        let mut layout = Layout::new(15.0, "monospace");
        layout.set_width(2000.0);
        let height = ROWS as f32 * layout.line_height();
        let rt = Runtime::with_document(Document::new(text), "emacs").unwrap();
        let mut w = Window { rt, layout, snap: None, anchor: 0, placed: Vec::new(), height };
        let s = w.rt.snapshot();
        w.take(s);
        w.draw();
        w
    }

    fn run(&mut self, input: Input) {
        let mut next = Some(input);
        while let Some(i) = next.take() {
            self.rt.handle(i);
            let s = self.rt.snapshot();
            next = self.take(s);
        }
        self.draw();
    }

    fn take(&mut self, s: Snapshot) -> Option<Input> {
        let moved = self.snap.as_ref().is_none_or(|old| old.head() != s.head() || old.revision != s.revision);
        self.anchor = s.scroll;
        let answers_input = !s.answers.is_empty();
        self.snap = Some(s);
        let s = self.snap.as_ref().filter(|_| answers_input && moved)?;
        self.anchor = self.layout.keep_visible(&s.text, self.anchor, s.head(), self.height)?;
        Some(Input::Scroll { revision: s.revision, anchor: self.anchor })
    }

    fn draw(&mut self) {
        let s = self.snap.as_ref().expect("a snapshot");
        self.layout.begin_frame();
        self.placed = self.layout.frame(&s.text, self.anchor, self.height);
    }

    fn key(&mut self, key: &Key, mods: M) {
        let key = keys::key_name(key, mods).expect("a key");
        self.run(Input::Key { key, at: Instant::now() });
    }

    /// A click on the left half of `on` as shown on visual line `row`.
    fn click(&mut self, row: usize, on: &str, extend: bool) {
        let s = self.snap.as_ref().expect("a snapshot");
        let seg = self.placed[row].seg;
        let pos = seg.start + s.text.byte_slice(seg.start..seg.end).to_string().find(on).expect("shown");
        let r = layout::caret(&self.layout, &self.placed, &s.text, pos).expect("visible");
        let pos = layout::hit(&self.layout, &self.placed, r.x + r.w * 0.25, r.y + r.h * 0.5).expect("a hit");
        let revision = s.revision;
        self.run(Input::Click { revision, pos, extend, at: Instant::now() });
    }

    fn wheel(&mut self, notches: i64) {
        let s = self.snap.as_ref().expect("a snapshot");
        self.anchor = self.layout.scroll_lines(&s.text, self.anchor, 3 * notches);
        let revision = s.revision;
        self.run(Input::Scroll { revision, anchor: self.anchor });
    }
}

struct Tty {
    rt: Runtime,
    term: Term,
    grid: Grid,
}

impl Tty {
    fn new(text: &str) -> Tty {
        let mut rt = Runtime::with_document(Document::new(text), "emacs").unwrap();
        let mut term = Term::new(120, ROWS + 1);
        term.output(Output::Snapshot(Box::new(rt.snapshot())));
        let mut t = Tty { grid: term.draw(), rt, term };
        // The terminal answers the kitty keyboard and device queries.
        t.send(b"\x1b[?0u\x1b[?62;22c");
        t
    }

    fn send(&mut self, bytes: &[u8]) {
        let mut inputs = self.term.feed(bytes);
        while !inputs.is_empty() {
            for i in inputs.drain(..) {
                self.rt.handle(i);
            }
            inputs.extend(self.term.output(Output::Snapshot(Box::new(self.rt.snapshot()))));
        }
        self.grid = self.term.draw();
    }

    /// A click on the last cell of `on` as shown on row `row`.
    fn click(&mut self, row: usize, on: &str, extend: bool) {
        let col = (0..self.grid.cols).find(|&c| self.grid.cell(row, c).text == on).expect("shown");
        let col = col + usize::from(self.grid.cell(row, col + 1).text.is_empty());
        let b = if extend { 4 } else { 0 };
        self.send(format!("\x1b[<{b};{};{}M\x1b[<{b};{};{}m", col + 1, row + 1, col + 1, row + 1).as_bytes());
    }
}

enum Step {
    /// A key as winit and as the terminal give it.
    Key(Key, M, &'static [u8]),
    Click {
        row: usize,
        on: &'static str,
        extend: bool,
    },
    Wheel(i64),
}

fn key(s: &str, mods: M, bytes: &'static [u8]) -> Step {
    Step::Key(Key::Character(s.into()), mods, bytes)
}

fn named(k: NamedKey, mods: M, bytes: &'static [u8]) -> Step {
    Step::Key(Key::Named(k), mods, bytes)
}

/// What both frontends must agree on.
fn state(rt: &mut Runtime) -> (String, Vec<(usize, usize)>, usize) {
    let s = rt.snapshot();
    (s.text.to_string(), s.selections, s.scroll)
}

#[test]
fn the_window_and_the_terminal_agree() {
    let text: String = (0..60).map(|i| format!("{i}\tfn 名前() {{ x{i} }}\n")).collect();
    let (c, a, none) = (M::CONTROL, M::ALT, M::empty());
    let c_n = || key("n", c, b"\x1b[110;5u");
    let undo = || key("/", c, b"\x1b[47;5u");
    let script: Vec<Step> = [c_n(), c_n(), key("f", a, b"\x1b[102;3u"), key("h", none, b"h"), key("\u{e9}", none, "\u{e9}".as_bytes())]
        .into_iter()
        .chain([named(NamedKey::Space, c, b"\x1b[32;5u"), key("e", c, b"\x1b[101;5u"), key("w", a, b"\x1b[119;3u")])
        .chain((0..30).map(|_| c_n()))
        .chain([
            key("y", c, b"\x1b[121;5u"),
            Step::Click { row: 5, on: "名", extend: false },
            Step::Click { row: 8, on: "x", extend: true },
            named(NamedKey::Backspace, none, b"\x7f"),
            Step::Wheel(2),
            named(NamedKey::Enter, none, b"\r"),
            key("<", a | M::SHIFT, b"\x1b[44:60;4u"),
            undo(),
        ])
        .collect();

    let mut w = Window::new(&text);
    let mut t = Tty::new(&text);
    let mut scrolled = false;
    for (i, step) in script.iter().enumerate() {
        match step {
            Step::Key(k, mods, bytes) => {
                w.key(k, *mods);
                t.send(bytes);
            }
            Step::Click { row, on, extend } => {
                w.click(*row, on, *extend);
                t.click(*row, on, *extend);
            }
            Step::Wheel(n) => {
                w.wheel(*n);
                t.send(if *n > 0 { b"\x1b[<65;1;1M" } else { b"\x1b[<64;1;1M" }.repeat(n.unsigned_abs() as usize).as_slice());
            }
        }
        let (ws, ts) = (state(&mut w.rt), state(&mut t.rt));
        assert_eq!(ws, ts, "after step {i}");
        scrolled |= ws.2 > 0;
    }
    assert!(scrolled, "the session scrolled");
    let s = w.rt.snapshot();
    assert_ne!(s.text.to_string(), text, "the session edited");

    // Undo steps through the same units in both: the deletion, the yank,
    // the typed text; then there is nothing more to undo.
    let window_undos: Vec<String> = (0..4)
        .map(|_| {
            w.key(&Key::Character("/".into()), c);
            w.rt.snapshot().text.to_string()
        })
        .collect();
    let term_undos: Vec<String> = (0..4)
        .map(|_| {
            t.send(b"\x1b[47;5u");
            t.rt.snapshot().text.to_string()
        })
        .collect();
    assert_eq!(window_undos, term_undos);
    assert_eq!(window_undos[2..], [text.clone(), text]);
    assert_ne!(window_undos[0], window_undos[1]);
}
