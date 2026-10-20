//! Indentation: RET, TAB, C-M-q and the modal = operator indent Scheme as
//! Emacs's lisp-mode does, and only where the buffer's mode indents.

use std::{path::Path, time::Instant};

use techne_editor::{present::Input, runtime::Runtime};

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn type_text(rt: &mut Runtime, text: &str) {
    for c in text.chars() {
        let key = match c {
            ' ' => "SPC".to_string(),
            '\n' => "RET".to_string(),
            c => c.to_string(),
        };
        rt.handle(Input::Key { key, at: Instant::now() });
    }
}

fn open(dir: &Path, name: &str, text: &str, profile: &str) -> Runtime {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    Runtime::open(&path, &dir.join(format!("{name}.journal")), profile).unwrap().0
}

fn text(rt: &mut Runtime) -> String {
    rt.snapshot().pane().text.to_string()
}

const INDENTED: &str = "(define (f x)\n  (let ((a 1)\n        (b 2))\n    (when (> a b)\n      (display a))\n    (+ a\n       b)))\n";

/// Typed with RET, code comes out indented; TAB and C-M-q indent lines
/// made flat; a buffer whose mode does not indent gets a tab.
#[test]
fn typing_and_reindenting_scheme() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = open(dir.path(), "a.scm", "", "emacs");
    type_text(&mut rt, "(define (f x)\n(let ((a 1)\n(b 2))\n(when (> a b)\n(display a))\n(+ a\nb)))\n");
    assert_eq!(text(&mut rt), INDENTED);
    // Undone, a line break goes with the indentation it brought.
    keys(&mut rt, "C-/");
    assert_eq!(text(&mut rt), INDENTED.trim_end());

    let flat = INDENTED.lines().map(str::trim_start).collect::<Vec<_>>().join("\n") + "\n";
    let mut rt = open(dir.path(), "b.scm", &flat, "emacs");
    // TAB indents its line, point going past the blanks.
    keys(&mut rt, "C-n TAB");
    let s = rt.snapshot();
    assert!(s.pane().text.to_string().starts_with("(define (f x)\n  (let"));
    assert_eq!(s.pane().head(), "(define (f x)\n  ".len());
    keys(&mut rt, "C-M-q");
    assert_eq!(text(&mut rt), INDENTED);

    let mut rt = open(dir.path(), "c.txt", "a\n", "emacs");
    keys(&mut rt, "TAB RET");
    assert_eq!(text(&mut rt), "\t\na\n");
}

/// In the modal profile, o opens an indented line, RET in insert state
/// indents, and = indents what a motion covers.
#[test]
fn modal_open_insert_and_the_indent_operator() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = open(dir.path(), "a.scm", "(when x\n  (a))\n", "modal");
    keys(&mut rt, "o");
    type_text(&mut rt, "(b\nc)");
    keys(&mut rt, "ESC");
    assert_eq!(text(&mut rt), "(when x\n  (b\n   c)\n  (a))\n");

    let flat = INDENTED.lines().map(str::trim_start).collect::<Vec<_>>().join("\n") + "\n";
    let mut rt = open(dir.path(), "b.scm", &flat, "modal");
    keys(&mut rt, "= G");
    assert_eq!(text(&mut rt), INDENTED);
}
