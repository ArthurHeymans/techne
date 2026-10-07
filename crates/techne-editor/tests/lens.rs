//! Slice 5 (PLAN.md, Stage 1): one editable search lens, and a structured
//! view with targets and actions.

use std::{path::Path, time::Instant};

use techne_editor::{
    present::{Input, Snapshot},
    runtime::{Runtime, lisp_dir},
};

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn type_text(rt: &mut Runtime, text: &str) {
    for c in text.chars() {
        let key = if c == ' ' { "SPC".to_string() } else { c.to_string() };
        rt.handle(Input::Key { key, at: Instant::now() });
    }
}

/// A runtime on a.txt, with b.txt open too.
fn two_files(dir: &Path) -> Runtime {
    std::fs::write(dir.join("a.txt"), "foo one\nbar\n").unwrap();
    std::fs::write(dir.join("b.txt"), "baz\nfoo two\n").unwrap();
    let mut rt = Runtime::open(&dir.join("a.txt"), &dir.join("a.journal"), "emacs").unwrap().0;
    rt.eval(&format!("(add-buffer! (file-document {:?}))", dir.join("b.txt").display().to_string())).unwrap();
    rt
}

fn text(s: &Snapshot) -> String {
    s.pane().text.to_string()
}

/// The text of a buffer by name.
fn buffer(rt: &mut Runtime, name: &str) -> String {
    rt.eval(&format!("(document-string (find (lambda (d) (equal? (buffer-name d) {name:?})) (buffer-list)))")).unwrap()
}

#[test]
fn editing_through_a_search_lens() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = two_files(dir.path());
    keys(&mut rt, "M-s o");
    type_text(&mut rt, "foo");
    keys(&mut rt, "RET");
    let s = rt.snapshot();
    assert_eq!(text(&s), "b.txt:2: foo two\na.txt:1: foo one\n", "the buffers last shown first");
    assert!(s.pane().status.contains("*lens foo*"), "{}", s.pane().status);
    assert!(s.pane().layers.iter().any(|h| (h.from, h.to, h.face.as_str()) == (0, 9, "comment")), "labels are drawn as comments");
    // Edit both excerpts: each edit lands in its document.
    keys(&mut rt, "C-s o n e RET M-b");
    type_text(&mut rt, "first ");
    keys(&mut rt, "M-< C-e");
    type_text(&mut rt, "!");
    assert_eq!(text(&rt.snapshot()), "b.txt:2: foo two!\na.txt:1: foo first one\n");
    assert_eq!(buffer(&mut rt, "a.txt"), "\"foo first one\\nbar\\n\"");
    assert_eq!(buffer(&mut rt, "b.txt"), "\"baz\\nfoo two!\\n\"");
    // Labels are not editable.
    keys(&mut rt, "C-a C-d");
    assert!(rt.snapshot().echo.contains("generated"), "{}", rt.snapshot().echo);
    // Someone else changes the line of an excerpt: an edit there is
    // refused, with why.
    rt.eval("(let ((d (find (lambda (d) (equal? (buffer-name d) \"b.txt\")) (buffer-list)))) (view-edit! (make-view d \"agent\") '((4 4 \">\")) \"new\"))").unwrap();
    let s = rt.snapshot();
    assert!(s.pane().layers.iter().any(|h| h.face == "warning"), "the stale excerpt is marked");
    keys(&mut rt, "C-e");
    type_text(&mut rt, "?");
    let s = rt.snapshot();
    assert!(s.echo.contains("changed since the lens showed it") && s.echo.contains("agent"), "{}", s.echo);
    assert_eq!(buffer(&mut rt, "b.txt"), "\"baz\\n>foo two!\\n\"");
    // Refreshed, it shows the change and can be edited again.
    keys(&mut rt, "C-c C-r C-e");
    type_text(&mut rt, "?");
    assert_eq!(text(&rt.snapshot()), "b.txt:2: >foo two!?\na.txt:1: foo first one\n");
    assert_eq!(buffer(&mut rt, "b.txt"), "\"baz\\n>foo two!?\\n\"");
    // Undo in the lens undoes in the source; C-x C-s writes the sources.
    keys(&mut rt, "C-/");
    assert_eq!(buffer(&mut rt, "b.txt"), "\"baz\\n>foo two!\\n\"");
    keys(&mut rt, "C-x C-s");
    assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "foo first one\nbar\n");
    assert_eq!(std::fs::read_to_string(dir.path().join("b.txt")).unwrap(), "baz\n>foo two!\n");
    // C-c C-o goes to the source line.
    keys(&mut rt, "C-c C-o");
    let s = rt.snapshot();
    assert_eq!(text(&s), "baz\n>foo two!\n");
    assert_eq!(s.pane().head(), 13);
}

#[test]
fn candidates_exported_as_a_lens() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = two_files(dir.path());
    keys(&mut rt, "C-c s B");
    type_text(&mut rt, "foo");
    keys(&mut rt, "C-c C-e");
    let s = rt.snapshot();
    assert!(s.minibuffer.is_none());
    assert_eq!(text(&s), "b.txt:2: foo two\na.txt:1: foo one\n");
    keys(&mut rt, "C-e");
    type_text(&mut rt, "!");
    assert_eq!(buffer(&mut rt, "b.txt"), "\"baz\\nfoo two!\\n\"");
}

#[test]
fn a_structured_view_with_targets_and_actions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "one\n// TODO: two\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "TODO three\n").unwrap();
    let mut rt = Runtime::open(&dir.path().join("a.txt"), &dir.path().join("a.journal"), "emacs").unwrap().0;
    let path = lisp_dir().join("examples/directory-todos.scm").display().to_string();
    for _ in 0..3 {
        rt.eval(&format!("(load-package 'todos {path:?})")).unwrap();
    }
    keys(&mut rt, "M-x");
    type_text(&mut rt, "directory-todos");
    keys(&mut rt, "RET");
    assert_eq!(text(&rt.snapshot()), "a.txt:2  // TODO: two\nb.txt:1  TODO three\n");
    // Read-only; RET goes to the row's target.
    type_text(&mut rt, "x");
    assert!(rt.snapshot().echo.contains("generated"));
    keys(&mut rt, "C-n RET");
    let s = rt.snapshot();
    assert_eq!((text(&s).as_str(), s.pane().head()), ("TODO three\n", 0));
    // Back, and the actions on locations, with no code of the view's.
    keys(&mut rt, "C-x b RET M-< C-;");
    let s = rt.snapshot();
    let actions: Vec<String> = s.minibuffer.as_ref().unwrap().rows.iter().map(|r| r.text(0)).collect();
    assert_eq!(actions, ["goto-location", "goto-location-other-pane", "copy-location-line"]);
    type_text(&mut rt, "other");
    keys(&mut rt, "RET");
    let s = rt.snapshot();
    assert_eq!(s.panes.len(), 2);
    assert_eq!(s.pane().text.byte_to_line(s.pane().head()), 1);
    rt.eval("(unload-package 'todos)").unwrap();
    assert_eq!(rt.eval("(memq 'directory-todos (command-names))").unwrap(), "#f");
}
