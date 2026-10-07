//! The terminal frontend headless: bytes as a terminal sends them go
//! through `Term` to a runtime, and its snapshots come back as cells.

use std::{sync::mpsc, time::Duration};

use techne_editor::{
    host::{Event, File, Host},
    present::{Highlight, Input, Output, Snapshot},
    runtime::Runtime,
};
use techne_term::{Face, Grid, Style, Term};
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
            if let Some(text) = self.rt.clipboard_out() {
                self.term.output(Output::Clipboard(text));
            }
        }
        self.grid = self.term.draw();
    }

    fn echo(&self) -> String {
        self.grid.row_text(self.grid.rows - 1)
    }

    fn styles(&self, row: usize) -> Vec<Style> {
        (0..self.grid.cols).map(|c| self.grid.cell(row, c).style).collect()
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
    assert_eq!(t.grid.row_text(3).trim_end(), "*scratch*  L1");
    let echo = t.echo();
    assert!(echo.starts_with("Keys this terminal cannot send: C-/ (undo), C-; (act-at-point), C-? (redo), C-DEL"), "{echo}");
    // C-/ arrives as C-_, which it shares a byte with; a keymap never sees
    // a C-/ that may not have been typed. C-_ is undo too, as in Emacs.
    t.send(b"x\x1f");
    assert_eq!(t.rt.snapshot().pane().text.to_string(), "abc");
    // Input with no name is reported too.
    t.send(b"\x1b[99~");
    assert!(t.echo().contains("Unrecognized input: \\x1b[99~"), "{}", t.echo());
}

#[test]
fn a_kitty_terminal_sends_every_chord() {
    let mut t = Tty::new("abc", 60, 5);
    t.send(KITTY);
    assert!(t.term.paint().starts_with("\x1b[>5u"), "the protocol is switched on");
    assert!(!t.echo().contains("cannot send"), "{}", t.echo());
    t.send(b"x\x1b[47;5u");
    assert_eq!(t.rt.snapshot().pane().text.to_string(), "abc", "C-/ undid the x");
    t.send(b"\x1b[47:63;6u");
    assert_eq!(t.rt.snapshot().pane().text.to_string(), "xabc", "C-? redid it");
}

#[test]
fn wide_characters_tabs_and_selections_in_cells() {
    let mut t = Tty::new("a\t名x\nnext", 20, 4);
    assert_eq!(t.grid.row_text(0), "a       名x         ");
    assert_eq!(t.grid.cursor, Some((0, 0)));
    // A click on the second cell of 名 puts the caret before it.
    t.send(&click(9, 0, false));
    assert_eq!(t.rt.snapshot().pane().head(), 2);
    assert_eq!(t.grid.cursor, Some((0, 8)));
    // Shift-click past the end of the next line: the selection covers 名, x,
    // the line break (one cell after the line) and "next".
    t.send(&click(15, 1, true));
    assert_eq!(t.rt.snapshot().pane().selections, [(2, 11)]);
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
    assert_eq!(s.pane().text.to_string(), "!one two three");
    assert_eq!(s.pane().head(), 5, "on the t of two in the current text");
}

#[test]
fn scrolling_by_anchor() {
    let text: String = (0..40).map(|i| format!("line {i} {}\n", "w".repeat(i % 9))).collect();
    let mut t = Tty::new(&text, 12, 7);
    t.send(KITTY);
    // The wheel scrolls three visual lines, which here are whole lines.
    t.send(b"\x1b[<65;1;1M");
    assert_eq!(t.rt.snapshot().pane().scroll, text.find("line 3").unwrap());
    assert_eq!(t.grid.row_text(0), "line 3 www  ");
    // The caret moved below the screen brings it back as the last row; lines
    // 6 to 8 are wider than the terminal and take two rows each.
    t.send(&b"\x1b[110;5u".repeat(10));
    assert_eq!(t.rt.snapshot().pane().head(), text.find("line 10").unwrap());
    assert_eq!(t.grid.cursor, Some((4, 0)));
    let rows: Vec<String> = (0..5).map(|r| t.grid.row_text(r).trim_end().to_string()).collect();
    assert_eq!(rows, ["ww", "line 8 wwwww", "www", "line 9", "line 10 w"]);
}

#[test]
fn panes_with_their_mode_lines() {
    let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
    // Eleven rows for panes: six for the first (five of text and its mode
    // line), five for the second, then the echo area.
    let mut t = Tty::new(&text, 30, 12);
    t.send(KITTY);
    // C-x 2 splits, C-x o focuses the lower pane; then x is typed there.
    t.send(b"\x1b[120;5u2\x1b[120;5uox");
    let rows: Vec<String> = [0, 4, 6, 9].iter().map(|&r| t.grid.row_text(r).trim_end().to_string()).collect();
    assert_eq!(rows, ["xline 0", "line 4", "xline 0", "line 3"], "the edit shows in both panes");
    assert!(t.grid.row_text(5).starts_with("*scratch*  [+]  L1"));
    assert!(t.grid.row_text(10).starts_with("*scratch*  [+]  L1"));
    assert!(t.styles(5).iter().all(|&s| s == Style::InactiveStatus));
    assert!(t.styles(10).iter().all(|&s| s == Style::Status), "the focused pane's mode line");
    assert_eq!(t.grid.cursor, Some((6, 1)));
    // The upper pane's caret is drawn in its text.
    assert_eq!((0..5).flat_map(|r| t.styles(r)).filter(|&s| s == Style::Caret).count(), 1);

    // The wheel over the upper pane scrolls it alone; a click there focuses
    // it.
    t.send(b"\x1b[<65;1;2M");
    let s = t.rt.snapshot();
    assert_eq!((s.panes[0].scroll, s.panes[1].scroll), (text.find("line 3").unwrap() + 1, 0));
    t.send(&click(2, 1, false));
    let s = t.rt.snapshot();
    assert_eq!(s.focus, 0);
    assert_eq!(s.pane().head(), text.find("line 4").unwrap() + 3);
    assert_eq!(s.panes[1].head(), 1, "the other pane's caret stays");
    assert!(t.styles(5).iter().all(|&s| s == Style::Status));
}

#[test]
fn highlights_as_sgr_attributes() {
    let mut t = Tty::new("(define x \"str\") ; note\nnext\nlast\n", 40, 4);
    // Highlights as a layer gives them, in order: on the first line, one of
    // a face the terminal does not know on the second, and one on the third,
    // which is not shown.
    let mut s = t.rt.snapshot();
    let h = |from, to, face: &str| Highlight { from, to, face: face.into() };
    s.panes[0].layers = vec![h(1, 7, "keyword"), h(10, 15, "string"), h(17, 23, "comment"), h(24, 28, "sparkle"), h(29, 33, "error")];
    t.term.output(Output::Snapshot(Box::new(s)));
    t.grid = t.term.draw();
    let faces = |row| t.styles(row).iter().map(|s| if let Style::Face(f) = s { Some(*f) } else { None }).collect::<Vec<_>>();
    let row0 = faces(0);
    assert_eq!(
        row0[0..8],
        [
            None,
            Some(Face::Keyword),
            Some(Face::Keyword),
            Some(Face::Keyword),
            Some(Face::Keyword),
            Some(Face::Keyword),
            Some(Face::Keyword),
            None
        ]
    );
    assert_eq!(row0[10..16], [Some(Face::String); 5].into_iter().chain([None]).collect::<Vec<_>>());
    assert_eq!(row0[17..24], [Some(Face::Comment); 6].into_iter().chain([None]).collect::<Vec<_>>());
    assert!(faces(1).iter().all(Option::is_none), "an unknown face is plain");
    let painted = t.term.paint();
    assert!(painted.contains("(\x1b[0;1;35mdefine\x1b[0m x "), "{painted:?}");
    assert!(painted.contains("\x1b[0;32m\"str\""), "{painted:?}");
    assert!(painted.contains("\x1b[0;3;90m; note"), "{painted:?}");
    assert!(!painted.contains("\x1b[0;1;31m"), "{painted:?}");
}

/// Run the terminal frontend over a runtime thread until a snapshot meets
/// `done`.
fn until(host: &mut Host, term: &mut Term, events: &mpsc::Receiver<Event>, done: impl Fn(&Snapshot) -> bool) -> Vec<Event> {
    let mut others = Vec::new();
    while !term.snapshot().is_some_and(&done) {
        match events.recv_timeout(Duration::from_secs(20)).expect("an event from the runtime") {
            Event::Output(o) => term.output(o).into_iter().for_each(|i| host.send(i)),
            e => others.push(e),
        }
    }
    others
}

#[test]
fn a_crashed_runtime_is_restarted_with_the_unsaved_edits() {
    let dir = tempfile::tempdir().unwrap();
    let (path, journal) = (dir.path().join("f.txt"), dir.path().join("f.journal"));
    std::fs::write(&path, "").unwrap();
    let (tx, events) = mpsc::channel();
    let mut host = Host::start(
        Some(File { path, journal }),
        "emacs".into(),
        |_| {},
        move |e| {
            let _ = tx.send(e);
        },
    );
    let mut term = Term::new(40, 6);
    let text = |s: &Snapshot| s.pane().text.to_string();
    for i in term.feed(KITTY).into_iter().chain(term.feed(b"(%crash-runtime)")) {
        host.send(i);
    }
    until(&mut host, &mut term, &events, |s| text(s) == "(%crash-runtime)");
    // C-M-x evaluates the form, which ends the runtime thread as a crash
    // would.
    for i in term.feed(b"\x1b[120;7u") {
        host.send(i);
    }
    while !matches!(events.recv_timeout(Duration::from_secs(20)).expect("the runtime to end"), Event::Ended) {}
    assert!(host.restart(), "restarted after input");
    term.restarted();
    until(&mut host, &mut term, &events, |s| s.echo.contains("Recovered"));
    assert_eq!(text(term.snapshot().unwrap()), "(%crash-runtime)");
    let screen = term.paint();
    assert!(screen.contains("(%crash-runtime)") && screen.contains("Recovered"), "all of the screen is painted again");
    // The new runtime takes input.
    for i in term.feed(b"\x1b[101;5u!") {
        host.send(i);
    }
    until(&mut host, &mut term, &events, |s| text(s) == "(%crash-runtime)!");
    host.close();
}

#[test]
fn a_runtime_that_ends_before_any_input_is_not_restarted() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, events) = mpsc::channel();
    // A directory cannot be opened as a file.
    let mut host = Host::start(
        Some(File { path: dir.path().to_path_buf(), journal: dir.path().join("f.journal") }),
        "emacs".into(),
        |_| {},
        move |e| {
            let _ = tx.send(e);
        },
    );
    assert!(matches!(events.recv().unwrap(), Event::Failed(_)));
    assert!(matches!(events.recv().unwrap(), Event::Ended));
    assert!(!host.restart());
}

#[test]
fn the_minibuffer_is_drawn_below_the_panes() {
    let mut t = Tty::new("abc", 60, 12);
    t.send(KITTY);
    t.send(b"\x1bxforward-");
    // Input line, then the candidates in two columns; the panes shrank.
    let row = |t: &Tty, r: usize| t.grid.row_text(r).trim_end().to_string();
    assert_eq!(row(&t, 8), "1/2 M-x forward-");
    assert_eq!(t.grid.cursor, Some((8, 16)));
    assert!(row(&t, 9).starts_with("forward-char (C-f)  Move forward by"), "{}", row(&t, 9));
    assert!(row(&t, 10).starts_with("forward-word (M-f)  Move to the end"), "{}", row(&t, 10));
    assert_eq!(t.styles(9)[0], Style::Selected);
    assert_eq!(t.styles(10)[..8], [Style::Face(Face::Match); 8]);
    assert_eq!(t.styles(10)[13..18], [Style::Face(Face::Key); 5]);
    assert_eq!(t.styles(10)[20], Style::Face(Face::Comment), "the documentation is in a column of its own");
    assert_eq!(row(&t, 7), "*scratch*  L1", "the mode line is above the minibuffer");
    // Moving the caret in the input moves the cursor.
    t.send(b"\x01");
    assert_eq!(t.grid.cursor, Some((8, 8)));
    t.send(b"\x0e\r");
    assert_eq!(t.rt.snapshot().pane().head(), 3, "forward-word ran");
    assert_eq!(row(&t, 10), "*scratch*  L1", "the minibuffer is gone");
}

/// Kills go to the clipboard through the terminal (OSC 52); what another
/// program put there is read when the terminal gets the focus.
#[test]
fn the_system_clipboard() {
    let mut t = Tty::new("hello world", 60, 5);
    t.send(KITTY);
    t.term.paint();
    t.send(b"\x1bd");
    assert!(t.term.paint().contains("\x1b]52;c;aGVsbG8=\x07"), "hello, in base64");
    t.term.read_clipboard_with(|| Some("pasted".into()));
    t.send(b"\x1b[I\x19");
    assert_eq!(t.rt.snapshot().pane().text.to_string(), "pasted world");
}

/// Panes side by side, a divider between them.
#[test]
fn panes_side_by_side() {
    let mut t = Tty::new("left and right", 41, 5);
    t.send(KITTY);
    t.send(b"\x18"); // C-x
    t.send(b"3");
    let row = |t: &Tty, r: usize| t.grid.row_text(r).trim_end().to_string();
    assert_eq!(row(&t, 0), "left and right      │left and right");
    assert!(row(&t, 3).starts_with("*scratch*  L1        *scratch*  L1"), "{}", row(&t, 3));
    // A click in the right one focuses it.
    t.send(&click(25, 0, false));
    assert_eq!(t.rt.snapshot().focus, 1);
    assert_eq!(t.rt.snapshot().pane().head(), 4);
}

/// which-key: after a prefix and a pause, the keys that can follow it in
/// columns above the echo area.
#[test]
fn which_key_columns() {
    let mut t = Tty::new("", 120, 20);
    t.send(KITTY);
    t.send(b"\x18");
    t.rt.run_tasks(std::time::Duration::ZERO);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    t.rt.run_tasks(std::time::Duration::from_millis(10));
    t.run(Vec::new());
    t.term.output(Output::Snapshot(Box::new(t.rt.snapshot())));
    t.grid = t.term.draw();
    let screen: Vec<String> = (0..20).map(|r| t.grid.row_text(r)).collect();
    let hints = &screen[16..19];
    assert!(hints[0].starts_with("0 : delete-window         3 : split-window-right"), "{hints:#?}");
    assert!(hints.iter().any(|r| r.contains("C-f : find-file")), "{hints:#?}");
    assert_eq!(screen[19].trim_end(), "C-x-");
    // The panes made room.
    assert!(screen[15].starts_with("*scratch*"), "{screen:#?}");
}

/// PageDown and PageUp (C-v, M-v) scroll by a screen less two lines, the
/// caret keeping its place on the screen; C-l recenters. As Emacs.
#[test]
fn paging_and_recentering() {
    let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
    let mut t = Tty::new(&text, 40, 10);
    t.send(KITTY);
    let first = |t: &Tty| t.grid.row_text(0).trim_end().to_string();
    let caret = |t: &mut Tty| {
        let s = t.rt.snapshot();
        s.pane().text.byte_to_line(s.pane().head())
    };
    t.send(b"\x0e"); // C-n: the caret on the second row
    t.send(b"\x1b[6~");
    assert_eq!((first(&t).as_str(), caret(&mut t)), ("line 6", 7), "8 rows less 2 of context");
    t.send(b"\x16"); // C-v
    assert_eq!((first(&t).as_str(), caret(&mut t)), ("line 12", 13));
    t.send(b"\x1bv"); // M-v
    assert_eq!((first(&t).as_str(), caret(&mut t)), ("line 6", 7));
    t.send(b"\x1b[5~");
    t.send(b"\x1b[5~");
    assert_eq!(first(&t), "line 0");
    assert_eq!(t.echo().trim_end(), "Beginning of buffer");
    // C-l: the caret's line to the middle, then the top, then the bottom.
    t.send(b"\x1b>\x0c");
    assert_eq!(first(&t), "line 26");
    t.send(b"\x0c");
    assert_eq!(first(&t), "");
    t.send(b"\x0c");
    assert_eq!(first(&t), "line 23");
    t.send(b"\x16");
    assert_eq!(t.echo().trim_end(), "End of buffer");
}
