//! The terminal frontend headless: bytes as a terminal sends them go
//! through `Term` to a runtime, and its snapshots come back as cells.

use techne_editor::{
    present::{Input, Output},
    runtime::Runtime,
};
use techne_term::{Grid, Term};
use techne_text::Document;

/// A terminal frontend and its runtime in one thread.
struct Tty {
    rt: Runtime,
    term: Term,
    grid: Grid,
}

/// The terminal's answers to the startup queries.
const KITTY: &[u8] = b"\x1b[?0u\x1b[?62;22c";
const LEGACY: &[u8] = b"\x1b[?62;22c";

impl Tty {
    fn new(text: &str, cols: usize, rows: usize) -> Tty {
        let mut rt = Runtime::with_document(Document::new(text), "emacs").unwrap();
        let mut term = Term::new(cols, rows);
        let bindings = rt.bindings();
        term.output(Output::Snapshot(Box::new(rt.snapshot())));
        let mut tty = Tty { grid: term.draw(), rt, term };
        let report = tty.term.output(Output::Bindings(bindings));
        tty.run(report.into_iter().collect());
        tty
    }

    /// What the terminal sends, then the screen updated.
    fn send(&mut self, bytes: &[u8]) {
        let inputs = self.term.feed(bytes);
        self.run(inputs);
    }

    fn run(&mut self, mut inputs: Vec<Input>) {
        while !inputs.is_empty() {
            for i in inputs.drain(..) {
                self.rt.handle(i);
            }
            inputs.extend(self.term.output(Output::Snapshot(Box::new(self.rt.snapshot()))));
        }
        self.grid = self.term.draw();
    }

    fn status(&self) -> String {
        self.grid.row_text(self.grid.rows - 1)
    }
}

/// A left click (press and release) on a zero-based cell.
fn click(col: usize, row: usize, shift: bool) -> Vec<u8> {
    let b = if shift { 4 } else { 0 };
    format!("\x1b[<{b};{};{}M\x1b[<{b};{};{}m", col + 1, row + 1, col + 1, row + 1).into_bytes()
}

#[test]
fn chords_a_legacy_terminal_cannot_send_are_reported() {
    let mut t = Tty::new("abc", 100, 5);
    t.send(LEGACY);
    assert_eq!(t.status().trim_end(), "*scratch*  L1  Keys this terminal cannot send: C-/ (undo), C-? (redo)");
    // C-/ arrives as C-_, which it shares a byte with; a keymap never sees
    // a C-/ that may not have been typed.
    t.send(b"x\x1f");
    assert_eq!(t.rt.snapshot().text.to_string(), "xabc");
    assert!(t.status().contains("C-_ is undefined"), "{}", t.status());
    // Input with no name is reported too.
    t.send(b"\x1b[99~");
    assert!(t.status().contains("Unrecognized input: \\x1b[99~"), "{}", t.status());
}

#[test]
fn a_kitty_terminal_sends_every_chord() {
    let mut t = Tty::new("abc", 60, 5);
    t.send(KITTY);
    assert!(t.term.paint().starts_with("\x1b[>5u"), "the protocol is switched on");
    assert!(!t.status().contains("cannot send"), "{}", t.status());
    t.send(b"x\x1b[47;5u");
    assert_eq!(t.rt.snapshot().text.to_string(), "abc", "C-/ undid the x");
    t.send(b"\x1b[47:63;6u");
    assert_eq!(t.rt.snapshot().text.to_string(), "xabc", "C-? redid it");
}

#[test]
fn wide_characters_tabs_and_selections_in_cells() {
    let mut t = Tty::new("a\t名x\nnext", 20, 4);
    assert_eq!(t.grid.row_text(0), "a       名x         ");
    assert_eq!(t.grid.cursor, Some((0, 0)));
    // A click on the second cell of 名 puts the caret before it.
    t.send(&click(9, 0, false));
    assert_eq!(t.rt.snapshot().head(), 2);
    assert_eq!(t.grid.cursor, Some((0, 8)));
    // Shift-click past the end of the next line: the selection covers 名, x,
    // the line break (one cell after the line) and "next".
    t.send(&click(15, 1, true));
    assert_eq!(t.rt.snapshot().selections, [(2, 11)]);
    let selected = |row| (0..20).filter(|&c| t.grid.cell(row, c).style == techne_term::Style::Selected).collect::<Vec<_>>();
    assert_eq!(selected(0), [8, 9, 10, 11]);
    assert_eq!(selected(1), [0, 1, 2, 3]);
    assert_eq!(t.grid.cursor, Some((1, 4)));
}

#[test]
fn a_click_is_resolved_against_the_frame_it_was_made_on() {
    let mut t = Tty::new("one two three", 30, 3);
    // Text is inserted at the start, but the screen has not caught up: the
    // click on "two" as shown is a click on the old revision.
    let frame = t.grid.clone();
    t.rt.handle(Input::Key { key: "!".into(), at: std::time::Instant::now() });
    t.term.output(Output::Snapshot(Box::new(t.rt.snapshot())));
    assert_eq!(frame.row_text(0).find("two"), Some(4));
    let inputs = t.term.feed(&click(4, 0, false));
    t.run(inputs);
    let s = t.rt.snapshot();
    assert_eq!(s.text.to_string(), "!one two three");
    assert_eq!(s.head(), 5, "on the t of two in the current text");
}

#[test]
fn scrolling_by_anchor() {
    let text: String = (0..40).map(|i| format!("line {i} {}\n", "w".repeat(i % 9))).collect();
    let mut t = Tty::new(&text, 12, 6);
    t.send(KITTY);
    // The wheel scrolls three visual lines, which here are whole lines.
    t.send(b"\x1b[<65;1;1M");
    assert_eq!(t.rt.snapshot().scroll, text.find("line 3").unwrap());
    assert_eq!(t.grid.row_text(0), "line 3 www  ");
    // The caret moved below the screen brings it back as the last row; lines
    // 6 to 8 are wider than the terminal and take two rows each.
    t.send(&b"\x1b[110;5u".repeat(10));
    assert_eq!(t.rt.snapshot().head(), text.find("line 10").unwrap());
    assert_eq!(t.grid.cursor, Some((4, 0)));
    let rows: Vec<String> = (0..5).map(|r| t.grid.row_text(r).trim_end().to_string()).collect();
    assert_eq!(rows, ["ww", "line 8 wwwww", "www", "line 9", "line 10 w"]);
}
