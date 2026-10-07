//! The inspector: a structured view of a value, its parts inspected in
//! turn; and evaluating the expression before point.

use std::{path::Path, time::Instant};

use techne_editor::{present::Input, runtime::Runtime};

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn open(dir: &Path, name: &str, text: &str) -> Runtime {
    std::fs::write(dir.join(name), text).unwrap();
    Runtime::open(&dir.join(name), &dir.join(format!("{name}.journal")), "emacs").unwrap().0
}

fn text(rt: &mut Runtime) -> String {
    rt.snapshot().pane().text.to_string()
}

#[test]
fn evaluating_the_expression_before_point() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = open(dir.path(), "a.scm", "(+ 1 (* 2 3))\n");
    keys(&mut rt, "C-s 3 ) RET C-x C-e");
    assert_eq!(rt.snapshot().echo, "6");
    keys(&mut rt, "C-e C-x C-e");
    assert_eq!(rt.snapshot().echo, "7");
}

#[test]
fn inspecting_values_and_their_parts() {
    let dir = tempfile::tempdir().unwrap();
    let source = "(define-record-type point (make-point x y) point? (x point-x) (y point-y))\n\
                  (define (twice x)\n  \"Double X.\"\n  (* 2 x))\n\
                  (list (make-point 1 #(2 3)) \"s\")\n";
    let mut rt = open(dir.path(), "a.scm", source);
    keys(&mut rt, "C-c C-k C-c M-i");
    let shown = text(&mut rt);
    assert!(shown.starts_with("value  (#<point 1 #(2 3)> \"s\")\ntype   pair\n0      #<point 1 #(2 3)>\n1      \"s\"\n"), "{shown}");
    // Into the record, then its vector field; back with l.
    keys(&mut rt, "C-n C-n RET");
    assert!(text(&mut rt).contains("\nx      1\ny      #(2 3)\n"), "{}", text(&mut rt));
    keys(&mut rt, "M-> C-p RET");
    assert!(text(&mut rt).contains("\n0      2\n1      3\n"), "{}", text(&mut rt));
    keys(&mut rt, "l l");
    assert!(text(&mut rt).starts_with("value  (#<point 1 #(2 3)> \"s\")"));
    // A procedure by its name: its documentation and definition; RET on
    // that goes to the source.
    keys(&mut rt, "C-x b RET C-x C-f");
    keys(&mut rt, "RET");
    keys(&mut rt, "C-s ( t w i RET C-c c k");
    let shown = text(&mut rt);
    assert!(shown.contains("name     twice\ndoc      Double X.\ndefined  ") && shown.contains("a.scm:2"), "{shown}");
    keys(&mut rt, "M-> C-p RET");
    let s = rt.snapshot();
    assert!(s.pane().status.contains("a.scm"));
    assert_eq!(s.pane().text.byte_to_line(s.pane().head()), 1);
}

#[test]
fn inspecting_a_command() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = open(dir.path(), "a.scm", "forward-char\n");
    keys(&mut rt, "C-c c k");
    let shown = text(&mut rt);
    assert!(shown.contains("command  Move forward by characters.") && shown.contains("commands.scm"), "{shown}");
}
