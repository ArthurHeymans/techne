//! Slice 4 (PLAN.md, Stage 1): two views of a document, the live loop
//! (evaluate in the file's module, redefine, jump to definitions), and the
//! canonical extension examples loading, reloading and unloading cleanly.

use std::{path::Path, time::Instant};

use techne_editor::{
    present::{Highlight, Input, Output, Snapshot},
    runtime::{Runtime, lisp_dir},
};
use techne_text::Document;

fn keys(rt: &mut Runtime, keys: &str) -> Option<Output> {
    keys.split(' ').filter_map(|k| rt.handle(Input::Key { key: k.to_string(), at: Instant::now() })).last()
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

fn open(dir: &Path, name: &str, text: &str) -> Runtime {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    Runtime::open(&path, &dir.join(format!("{name}.journal")), "emacs").unwrap().0
}

fn text(s: &Snapshot, pane: usize) -> String {
    s.panes[pane].text.to_string()
}

#[test]
fn two_views_of_one_document() {
    let mut rt = Runtime::with_document(Document::new("one\ntwo\nthree\n"), "emacs").unwrap();
    keys(&mut rt, "C-n C-x 2");
    let s = rt.snapshot();
    assert_eq!(s.panes.len(), 2);
    assert_eq!(s.focus, 0);
    assert_eq!(s.panes[1].head(), 4, "the new view starts where the old one is");
    // Move the second view to the end and scroll it; edit in the first.
    keys(&mut rt, "C-x o M->");
    let s = rt.snapshot();
    let (second, revision) = (s.panes[1].view, s.panes[1].revision);
    rt.handle(Input::Scroll { view: second, revision, anchor: 8, caret: None });
    keys(&mut rt, "C-x o C-a");
    type_text(&mut rt, "zero\n");
    let s = rt.snapshot();
    assert_eq!(text(&s, 0), "one\nzero\ntwo\nthree\n");
    assert_eq!(text(&s, 1), text(&s, 0), "both views show the edit");
    assert_eq!(s.panes[0].head(), 9);
    assert_eq!(s.panes[1].head(), 19, "the other view's caret follows the edit");
    assert_eq!(s.panes[1].scroll, 13, "and so does its scroll anchor");
    // A click in the second pane focuses it.
    rt.handle(Input::Click { view: second, revision: s.panes[1].revision, pos: 0, extend: false, at: Instant::now() });
    let s = rt.snapshot();
    assert_eq!((s.focus, s.panes[1].head()), (1, 0));
    keys(&mut rt, "C-x 1");
    assert_eq!(rt.snapshot().panes.len(), 1);
}

#[test]
fn redefining_a_command_changes_its_next_invocation() {
    let dir = tempfile::tempdir().unwrap();
    let source = "(import (techne editor))\n\
                  (define-command (greet s n) \"Say hello.\" (message! s \"hello v1\"))\n\
                  (define-key! emacs-map \"C-c g\" 'greet)\n";
    let mut rt = open(dir.path(), "greet.scm", source);
    keys(&mut rt, "C-c C-k C-c g");
    assert_eq!(rt.snapshot().echo, "hello v1");
    // Edit the definition and evaluate just it: no restart.
    keys(&mut rt, "C-s v 1 RET DEL");
    type_text(&mut rt, "2");
    keys(&mut rt, "C-M-x");
    assert!(rt.snapshot().echo.contains("greet"), "{}", rt.snapshot().echo);
    keys(&mut rt, "C-c g");
    assert_eq!(rt.snapshot().echo, "hello v2");
    // The command lives in the file's module, where its definition is.
    assert!(rt.eval("(procedure-location (command 'greet))").unwrap().contains("greet.scm\" 2 1"));
}

#[test]
fn jumping_to_a_definition_and_back() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.scm"), "(provide twice)\n\n(define (twice x)\n  (* 2 x))\n").unwrap();
    let mut rt = open(dir.path(), "use.scm", "(require \"lib.scm\")\n(twice 21)\n");
    keys(&mut rt, "C-c C-k");
    assert_eq!(rt.snapshot().echo, "42");
    keys(&mut rt, "C-n C-f C-h .");
    let echo = rt.snapshot().echo;
    assert!(echo.contains("(twice x)") && echo.contains("lib.scm"), "{echo}");
    keys(&mut rt, "M-.");
    let s = rt.snapshot();
    assert!(s.pane().status.contains("lib.scm"), "{}", s.pane().status);
    let pane = s.pane();
    assert_eq!(pane.text.byte_to_line(pane.head()), 2, "on the definition's line");
    keys(&mut rt, "M-,");
    assert!(rt.snapshot().pane().status.contains("use.scm"));
}

fn example(name: &str) -> String {
    lisp_dir().join("examples").join(name).display().to_string()
}

#[test]
fn a_command_on_the_region() {
    let mut rt = Runtime::with_document(Document::new("hello world"), "emacs").unwrap();
    let path = example("upcase-region.scm");
    for _ in 0..3 {
        rt.eval(&format!("(load-package 'shout {path:?})")).unwrap();
    }
    keys(&mut rt, "C-SPC M-f C-c u");
    assert_eq!(rt.snapshot().pane().text.to_string(), "HELLO world");
    rt.eval("(unload-package 'shout)").unwrap();
    assert_eq!(rt.eval("(memq 'shout-region (command-names))").unwrap(), "#f");
    keys(&mut rt, "C-a C-SPC C-e C-c u");
    assert!(rt.snapshot().echo.contains("C-c u is undefined"), "{}", rt.snapshot().echo);
}

#[test]
fn a_minor_mode_with_a_keymap_and_a_layer() {
    let mut rt = Runtime::with_document(Document::new("a TODO\nb\nc TODO\n"), "emacs").unwrap();
    let path = example("todo-mode.scm");
    for _ in 0..100 {
        rt.eval(&format!("(load-package 'todo {path:?})")).unwrap();
    }
    assert!(rt.snapshot().pane().layers.is_empty(), "the mode is off");
    rt.eval("(run-command (current-session) 'todo-mode 1)").unwrap();
    let s = rt.snapshot();
    let warning = |from, to| Highlight { from, to, face: "warning".into() };
    assert_eq!(s.pane().layers, vec![warning(2, 6), warning(11, 15)]);
    assert!(s.pane().status.contains("(todo-mode)"), "{}", s.pane().status);
    keys(&mut rt, "C-c t");
    assert_eq!(rt.snapshot().pane().head(), 2);
    keys(&mut rt, "C-f C-c t");
    assert_eq!(rt.snapshot().pane().head(), 11);
    // Unloading takes the mode, its command, keys and layer away.
    rt.eval("(unload-package 'todo)").unwrap();
    let s = rt.snapshot();
    assert!(s.pane().layers.is_empty());
    assert!(!s.pane().status.contains("todo"));
    assert_eq!(rt.eval("(list (find-mode 'todo-mode) (memq 'next-todo (command-names)))").unwrap(), "(#f #f)");
    keys(&mut rt, "M-< C-c t");
    assert_eq!(rt.snapshot().pane().head(), 0);
    assert_eq!(rt.eval("(scope-children %root-scope)").unwrap(), "()");
}

/// C-x 2 and C-x 3 split the focused pane in two, as Emacs splits a
/// window; C-x 0 gives its space back to its neighbour, C-x 1 keeps one.
#[test]
fn panes_split_as_emacs_windows() {
    let mut rt = Runtime::with_document(Document::new("text"), "emacs").unwrap();
    let places = |rt: &mut Runtime| {
        let s = rt.snapshot();
        let p = s.panes.iter().map(|p| (p.place.x, p.place.y, p.place.w, p.place.h)).collect::<Vec<_>>();
        (p, s.focus)
    };
    keys(&mut rt, "C-x 3");
    assert_eq!(places(&mut rt), (vec![(0.0, 0.0, 0.5, 1.0), (0.5, 0.0, 0.5, 1.0)], 0));
    // Only the focused pane splits; the focus stays in its upper half.
    keys(&mut rt, "C-x 2");
    assert_eq!(places(&mut rt), (vec![(0.0, 0.0, 0.5, 0.5), (0.0, 0.5, 0.5, 0.5), (0.5, 0.0, 0.5, 1.0)], 0));
    keys(&mut rt, "C-x o C-x o");
    assert_eq!(places(&mut rt).1, 2);
    // The right one goes: the left column gets its space.
    keys(&mut rt, "C-x 0");
    assert_eq!(places(&mut rt), (vec![(0.0, 0.0, 1.0, 0.5), (0.0, 0.5, 1.0, 0.5)], 1));
    keys(&mut rt, "C-x o C-x 0");
    assert_eq!(places(&mut rt), (vec![(0.0, 0.0, 1.0, 1.0)], 0));
    keys(&mut rt, "C-x 0");
    assert_eq!(rt.snapshot().echo, "Attempt to delete the sole window");
    keys(&mut rt, "C-x 3 C-x 3 C-x 1");
    assert_eq!(places(&mut rt), (vec![(0.0, 0.0, 1.0, 1.0)], 0));
}
