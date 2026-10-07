//! The window and the terminal frontend give the same semantic state for
//! the same session (PLAN.md, Stage 1, slices 3 and 4). Each frontend gets
//! the input it would get from its platform: winit keys and pixel positions
//! here, the bytes a terminal with the kitty keyboard protocol sends there.
//! Each resolves keys, clicks and scrolling with its own normalizer and
//! layout against a runtime of its own; after every step the documents and
//! every pane's selections and scroll anchor must agree, and undoing
//! afterwards must step through the same units. Then the session splits in
//! two panes, edits in one and clicks and scrolls in both.
//!
//! The window's side is `Screen`, as `App` uses it, without the window.

use std::time::Instant;

use techne_editor::{
    present::{Input, Output},
    runtime::Runtime,
};
use techne_term::{Grid, Term};
use techne_text::Document;
use winit::keyboard::{Key, ModifiersState as M, NamedKey};

use crate::{
    keys,
    layout::{self, Layout},
    screen::Screen,
};

/// Lines for text in both frontends with one pane; one more for its mode
/// line, then the echo area.
const ROWS: usize = 20;

struct Window {
    rt: Runtime,
    layout: Layout,
    screen: Screen,
}

impl Window {
    fn new(text: &str) -> Window {
        let layout = Layout::new(15.0, "monospace");
        let mut screen = Screen::default();
        screen.set_size(2000.0, (ROWS + 2) as f32 * layout.line_height());
        let rt = Runtime::with_document(Document::new(text), "emacs").unwrap();
        let mut w = Window { rt, layout, screen };
        let s = w.rt.snapshot();
        w.screen.take(&mut w.layout, s);
        w.screen.frame(&mut w.layout);
        w
    }

    fn run(&mut self, input: Input) {
        let mut inputs = vec![input];
        while !inputs.is_empty() {
            for i in inputs.drain(..) {
                self.rt.handle(i);
            }
            let s = self.rt.snapshot();
            inputs = self.screen.take(&mut self.layout, s);
        }
        self.screen.frame(&mut self.layout);
    }

    fn key(&mut self, key: &Key, mods: M) {
        let key = keys::key_name(key, mods).expect("a key");
        self.run(Input::Key { key, at: Instant::now() });
    }

    /// A click on the left half of `on` as shown on visual line `row` of a
    /// pane.
    fn click(&mut self, pane: usize, row: usize, on: &str, extend: bool) {
        let (s, shown) = (self.screen.snap.as_ref().expect("a snapshot"), &self.screen.shown[pane]);
        let text = &s.panes[pane].text;
        let seg = shown.placed[row].seg;
        let pos = seg.start + text.byte_slice(seg.start..seg.end).to_string().find(on).expect("shown");
        let r = layout::caret(&self.layout, &shown.placed, text, pos).expect("visible");
        let (x, y) = (shown.area.text_left() + r.x + r.w * 0.25, shown.area.top + r.y + r.h * 0.5);
        let input = self.screen.press(&self.layout, x, y, extend).expect("a click");
        self.screen.release();
        self.run(input);
    }

    fn wheel(&mut self, pane: usize, notches: i64) {
        let y = self.screen.shown[pane].area.top + 1.0;
        let input = self.screen.wheel(&mut self.layout, 0.0, y, 3 * notches).expect("a pane");
        self.run(input);
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
        let mut term = Term::new(120, ROWS + 2);
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

    /// A click on the last cell of `on` as shown on row `row` of a pane.
    fn click(&mut self, pane: usize, row: usize, on: &str, extend: bool) {
        let area = self.term.areas()[pane];
        let row = area.top + row;
        let col = (area.text_left()..self.grid.cols).find(|&c| self.grid.cell(row, c).text == on).expect("shown");
        let col = col + usize::from(self.grid.cell(row, col + 1).text.is_empty());
        let b = if extend { 4 } else { 0 };
        self.send(format!("\x1b[<{b};{};{}M\x1b[<{b};{};{}m", col + 1, row + 1, col + 1, row + 1).as_bytes());
    }

    fn wheel(&mut self, pane: usize, notches: i64) {
        let row = self.term.areas()[pane].top + 1;
        let b = if notches > 0 { 65 } else { 64 };
        self.send(format!("\x1b[<{b};1;{row}M").repeat(notches.unsigned_abs() as usize).as_bytes());
    }
}

enum Step {
    /// A key as winit and as the terminal give it.
    Key(Key, M, &'static [u8]),
    Click {
        pane: usize,
        row: usize,
        on: &'static str,
        extend: bool,
    },
    Wheel {
        pane: usize,
        notches: i64,
    },
}

fn key(s: &str, mods: M, bytes: &'static [u8]) -> Step {
    Step::Key(Key::Character(s.into()), mods, bytes)
}

fn named(k: NamedKey, mods: M, bytes: &'static [u8]) -> Step {
    Step::Key(Key::Named(k), mods, bytes)
}

/// The focused pane, and every pane's text, selections and scroll anchor:
/// what both frontends must agree on.
type State = (usize, Vec<(String, Vec<(usize, usize)>, usize)>);

fn state(rt: &mut Runtime) -> State {
    let s = rt.snapshot();
    (s.focus, s.panes.into_iter().map(|p| (p.text.to_string(), p.selections, p.scroll)).collect())
}

/// Run a script in both frontends; the states they agree on after each step.
fn run(w: &mut Window, t: &mut Tty, script: &[Step]) -> Vec<State> {
    script
        .iter()
        .enumerate()
        .map(|(i, step)| {
            match step {
                Step::Key(k, mods, bytes) => {
                    w.key(k, *mods);
                    t.send(bytes);
                }
                Step::Click { pane, row, on, extend } => {
                    w.click(*pane, *row, on, *extend);
                    t.click(*pane, *row, on, *extend);
                }
                Step::Wheel { pane, notches } => {
                    w.wheel(*pane, *notches);
                    t.wheel(*pane, *notches);
                }
            }
            let (ws, ts) = (state(&mut w.rt), state(&mut t.rt));
            assert_eq!(ws, ts, "after step {i}");
            ws
        })
        .collect()
}

#[test]
fn the_window_and_the_terminal_agree() {
    let text: String = (0..60).map(|i| format!("{i}\tfn 名前() {{ x{i} }}\n")).collect();
    let (c, a, none) = (M::CONTROL, M::ALT, M::empty());
    let c_n = || key("n", c, b"\x1b[110;5u");
    let c_x = || key("x", c, b"\x1b[120;5u");
    let undo = || key("/", c, b"\x1b[47;5u");
    let script: Vec<Step> = [c_n(), c_n(), key("f", a, b"\x1b[102;3u"), key("h", none, b"h"), key("\u{e9}", none, "\u{e9}".as_bytes())]
        .into_iter()
        .chain([named(NamedKey::Space, c, b"\x1b[32;5u"), key("e", c, b"\x1b[101;5u"), key("w", a, b"\x1b[119;3u")])
        .chain((0..30).map(|_| c_n()))
        .chain([
            key("y", c, b"\x1b[121;5u"),
            Step::Click { pane: 0, row: 5, on: "名", extend: false },
            Step::Click { pane: 0, row: 8, on: "x", extend: true },
            named(NamedKey::Backspace, none, b"\x7f"),
            Step::Wheel { pane: 0, notches: 2 },
            named(NamedKey::Enter, none, b"\r"),
            key("<", a | M::SHIFT, b"\x1b[44:60;4u"),
            undo(),
        ])
        .collect();

    let mut w = Window::new(&text);
    let mut t = Tty::new(&text);
    let states = run(&mut w, &mut t, &script);
    assert!(states.iter().any(|s| s.1[0].2 > 0), "the session scrolled");
    assert_ne!(states.last().unwrap().1[0].0, text, "the session edited");

    // Undo steps through the same units in both: the deletion, the yank,
    // the typed text; then there is nothing more to undo.
    let text_of = |rt: &mut Runtime| rt.snapshot().pane().text.to_string();
    let window_undos: Vec<String> = (0..4)
        .map(|_| {
            w.key(&Key::Character("/".into()), c);
            text_of(&mut w.rt)
        })
        .collect();
    let term_undos: Vec<String> = (0..4)
        .map(|_| {
            t.send(b"\x1b[47;5u");
            text_of(&mut t.rt)
        })
        .collect();
    assert_eq!(window_undos, term_undos);
    assert_eq!(window_undos[2..], [text.clone(), text.clone()]);
    assert_ne!(window_undos[0], window_undos[1]);

    // Two panes of the document: split, move down in the lower one and edit
    // there, scroll each, click in the lower one from the upper one, edit in
    // the upper one, then back to one pane.
    let script: Vec<Step> = [c_x(), key("2", none, b"2"), c_x(), key("o", none, b"o")]
        .into_iter()
        .chain((0..14).map(|_| c_n()))
        .chain([key("z", none, b"z"), Step::Wheel { pane: 0, notches: 1 }, Step::Wheel { pane: 1, notches: -2 }])
        .chain([c_x(), key("o", none, b"o"), Step::Click { pane: 1, row: 2, on: "名", extend: false }])
        .chain([key("q", none, b"q"), c_x(), key("o", none, b"o"), key("v", none, b"v")])
        .chain([c_x(), key("1", none, b"1")])
        .collect();
    let states = run(&mut w, &mut t, &script);
    let split = &states[1];
    assert_eq!((split.0, split.1.len()), (0, 2), "split, the upper pane focused");
    assert_eq!(states[3].0, 1, "C-x o focuses the lower pane");
    let (before, edited) = (&states[17], &states[18]);
    assert_ne!(before.1[1].0, edited.1[1].0, "the edit in the lower pane");
    assert_eq!(edited.1[0].0, edited.1[1].0, "shows in the upper one");
    assert_ne!(edited.1[0].1, edited.1[1].1, "each pane has its selections");
    let scrolled = &states[20];
    assert_ne!(scrolled.1[0].2, scrolled.1[1].2, "and its scroll anchor");
    assert_eq!((states[22].0, states[23].0), (0, 1), "the click focused the lower pane");
    assert_eq!(states.last().unwrap().1.len(), 1);
}
