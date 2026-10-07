//! Slice 5 (PLAN.md, Stage 1): the minibuffer. Commands by name, files
//! and buffers by completion, in both key profiles.

use std::{path::Path, time::Instant};

use techne_editor::{
    present::{Input, Snapshot},
    runtime::Runtime,
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

fn open(dir: &Path, name: &str, profile: &str) -> Runtime {
    Runtime::open(&dir.join(name), &dir.join(format!("{name}.journal")), profile).unwrap().0
}

fn open_with(dir: &Path, name: &str, text: &str) -> Runtime {
    std::fs::write(dir.join(name), text).unwrap();
    open(dir, name, "emacs")
}

/// The candidates shown, by their text.
fn shown(s: &Snapshot) -> Vec<String> {
    s.minibuffer.as_ref().expect("the minibuffer").rows.iter().map(|r| r.text(0)).collect()
}

fn selected(s: &Snapshot) -> String {
    let m = s.minibuffer.as_ref().expect("the minibuffer");
    m.rows[m.selected.expect("a selection")].text(0)
}

#[test]
fn a_command_by_name() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "one two").unwrap();
    let mut rt = open(dir.path(), "a.txt", "emacs");
    keys(&mut rt, "M-x");
    let s = rt.snapshot();
    let m = s.minibuffer.as_ref().unwrap();
    assert!(m.prompt.ends_with("M-x "), "{}", m.prompt);
    assert_eq!(m.rows.len(), 10, "ten candidates are shown");
    // Parts match in any order; the matched text is marked.
    type_text(&mut rt, "char forw");
    let s = rt.snapshot();
    assert_eq!(shown(&s), ["forward-char (C-f)"], "with its key, not matched");
    let row = &s.minibuffer.as_ref().unwrap().rows[0];
    assert!(row.columns[0].iter().any(|r| r.text == "forw" && r.face.as_deref() == Some("match")), "{row:?}");
    assert_eq!((row.text(0).as_str(), row.text(1).as_str()), ("forward-char (C-f)", "Move forward by characters."));
    keys(&mut rt, "RET");
    let s = rt.snapshot();
    assert!(s.minibuffer.is_none());
    assert_eq!(s.pane().head(), 1, "the command ran in the pane, not the input");
    // The input is edited with the usual keys; C-g closes.
    keys(&mut rt, "M-x");
    type_text(&mut rt, "xyz");
    keys(&mut rt, "DEL DEL DEL C-a");
    type_text(&mut rt, "end-of-buf");
    assert_eq!(rt.snapshot().minibuffer.unwrap().input, "end-of-buf");
    keys(&mut rt, "C-g");
    let s = rt.snapshot();
    assert!(s.minibuffer.is_none());
    assert_eq!((s.pane().head(), s.echo.as_str()), (1, "Quit"));
}

#[test]
fn files_and_buffers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("a.txt"), "aaa\n").unwrap();
    std::fs::write(dir.path().join("sub/b.txt"), "bbb\n").unwrap();
    let mut rt = open(dir.path(), "a.txt", "emacs");
    keys(&mut rt, "C-e");
    // Find a file a directory at a time, starting in the document's.
    keys(&mut rt, "C-x C-f");
    let s = rt.snapshot();
    assert_eq!(s.minibuffer.as_ref().unwrap().input, format!("{}/", dir.path().display()));
    assert_eq!(shown(&s), ["a.txt", "a.txt.journal", "sub/"]);
    type_text(&mut rt, "su");
    keys(&mut rt, "RET");
    let s = rt.snapshot();
    assert!(s.minibuffer.as_ref().unwrap().input.ends_with("/sub/"));
    assert_eq!(shown(&s), ["b.txt"]);
    keys(&mut rt, "RET");
    let s = rt.snapshot();
    assert!(s.minibuffer.is_none());
    assert_eq!(s.pane().text.to_string(), "bbb\n");
    // A name that does not exist yet opens an empty document.
    keys(&mut rt, "C-x C-f");
    type_text(&mut rt, "new.txt");
    keys(&mut rt, "RET");
    let s = rt.snapshot();
    assert!(s.pane().status.contains("new.txt"), "{}", s.pane().status);
    // Buffers: the last one shown first, the current one last.
    keys(&mut rt, "C-x b");
    assert_eq!(shown(&rt.snapshot()), ["b.txt", "a.txt", "new.txt"]);
    // Moving previews the buffer; C-g puts the pane back.
    keys(&mut rt, "C-n");
    assert_eq!(rt.snapshot().pane().text.to_string(), "aaa\n", "a.txt is previewed");
    keys(&mut rt, "C-g");
    assert_eq!(rt.snapshot().pane().text.to_string(), "");
    // Back to a.txt: its caret is where it was.
    keys(&mut rt, "C-x b");
    type_text(&mut rt, "a.t");
    keys(&mut rt, "RET");
    let s = rt.snapshot();
    assert_eq!((s.pane().text.to_string().as_str(), s.pane().head()), ("aaa\n", 3));
    // Opening a file twice gives the same document.
    keys(&mut rt, "C-x 2 C-x C-f");
    keys(&mut rt, "RET");
    keys(&mut rt, "x");
    let s = rt.snapshot();
    assert_eq!(s.panes[0].text.to_string(), "aaax\n");
    assert_eq!(s.panes[1].text.to_string(), s.panes[0].text.to_string());
}

#[test]
fn the_modal_profile_has_the_minibuffer_on_its_leader_key() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "one two").unwrap();
    let mut rt = open(dir.path(), "a.txt", "modal");
    keys(&mut rt, "SPC :");
    type_text(&mut rt, "end-of-line");
    let s = rt.snapshot();
    assert_eq!(selected(&s), "end-of-line");
    keys(&mut rt, "RET");
    assert_eq!(rt.snapshot().pane().head(), 7);
    // ESC leaves the minibuffer, not the normal state.
    keys(&mut rt, "SPC b b ESC");
    let s = rt.snapshot();
    assert!(s.minibuffer.is_none());
    keys(&mut rt, "0");
    assert_eq!(rt.snapshot().pane().head(), 0);
}

#[test]
fn acting_on_candidates() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "bee\n").unwrap();
    let mut rt = open(dir.path(), "a.txt", "emacs");
    // A command: the actions on commands are offered, the default first.
    keys(&mut rt, "M-x");
    type_text(&mut rt, "forward-char");
    keys(&mut rt, "C-;");
    let s = rt.snapshot();
    assert!(s.minibuffer.as_ref().unwrap().prompt.ends_with("Act on forward-char: "));
    assert_eq!(shown(&s), ["run-named-command", "describe-command", "find-command-definition"]);
    type_text(&mut rt, "desc");
    keys(&mut rt, "RET");
    let echo = rt.snapshot().echo;
    assert!(echo.starts_with("forward-char: Move forward by characters.") && echo.contains("commands.scm"), "{echo}");
    // A file, opened in a new pane.
    keys(&mut rt, "C-x C-f");
    type_text(&mut rt, "b.t");
    keys(&mut rt, "C-;");
    type_text(&mut rt, "other");
    keys(&mut rt, "RET");
    let s = rt.snapshot();
    assert_eq!(s.panes.iter().map(|p| p.text.to_string()).collect::<Vec<_>>(), ["one\ntwo\n", "bee\n"]);
    assert_eq!(s.focus, 1);
    // Nothing at point to act on in a file.
    keys(&mut rt, "C-;");
    assert_eq!(rt.snapshot().echo, "No target at point");
}

#[test]
fn searching_lines_with_preview() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "alpha\nbeta\ngamma\nbeta two\n").unwrap();
    let mut rt = open(dir.path(), "a.txt", "emacs");
    keys(&mut rt, "C-c s s");
    type_text(&mut rt, "beta");
    let s = rt.snapshot();
    assert_eq!(shown(&s), ["beta", "beta two"]);
    assert_eq!(s.minibuffer.as_ref().unwrap().rows[1].text(1), "4");
    assert_eq!(s.pane().head(), 6, "the first match is previewed");
    keys(&mut rt, "C-n");
    assert_eq!(rt.snapshot().pane().head(), 17);
    keys(&mut rt, "C-g");
    assert_eq!(rt.snapshot().pane().head(), 0, "C-g goes back");
    keys(&mut rt, "C-c s s");
    type_text(&mut rt, "gam");
    keys(&mut rt, "RET");
    assert_eq!(rt.snapshot().pane().head(), 11);
}

#[test]
fn a_completion_source_with_preview() {
    let dir = tempfile::tempdir().unwrap();
    let source = "(define a 1)\n\n(define (f x)\n  (define y 2)\n  x)\n(define b 3)\n";
    let mut rt = open_with(dir.path(), "defs.scm", source);
    let path = techne_editor::runtime::lisp_dir().join("examples/goto-definition.scm").display().to_string();
    for _ in 0..3 {
        rt.eval(&format!("(load-package 'defs {path:?})")).unwrap();
    }
    keys(&mut rt, "C-c d");
    let s = rt.snapshot();
    assert_eq!(shown(&s), ["(define a 1)", "(define (f x)", "(define b 3)"], "only the top-level ones");
    keys(&mut rt, "M->");
    assert_eq!(rt.snapshot().pane().head(), 48, "previewed");
    keys(&mut rt, "C-g");
    assert_eq!(rt.snapshot().pane().head(), 0);
    keys(&mut rt, "C-c d");
    type_text(&mut rt, "(f");
    keys(&mut rt, "RET");
    assert_eq!(rt.snapshot().pane().head(), 14);
    rt.eval("(unload-package 'defs)").unwrap();
    keys(&mut rt, "C-c d");
    assert!(rt.snapshot().echo.contains("C-c d is undefined"));
}

/// The keys are those of Arthur's Emacs, as emacsclient reported them: Doom
/// without evil (vertico, consult, embark; its leader is C-c), and Geiser
/// in Scheme buffers.
#[test]
fn doom_keys() {
    let mut rt = techne_editor::runtime::Runtime::with_document(techne_text::Document::new(""), "emacs").unwrap();
    let bound = |rt: &mut Runtime, map: &str, keys: &str| rt.eval(&format!("(lookup-key {map} (kbd {keys:?}))")).unwrap();
    for (keys, command) in [
        ("M-x", "execute-extended-command"),
        ("C-x C-f", "find-file"),
        ("C-c f f", "find-file"),
        ("C-x b", "switch-to-buffer"),
        ("C-x k", "kill-buffer"),
        ("C-;", "act-at-point"),
        ("C-c a", "act-at-point"),
        ("C-c s s", "search-lines"),
        ("C-c s b", "search-lines"),
        ("C-c s B", "search-all-buffers"),
        ("M-s o", "lens-search"),
        ("C-M-x", "eval-defun"),
        ("C-x C-e", "eval-last-sexp"),
        ("C-c C-k", "eval-buffer"),
        ("M-.", "find-definition"),
        ("M-,", "pop-definition"),
        ("C-c c d", "find-definition"),
        ("C-c c e", "eval-buffer-or-region"),
        ("C-c c k", "inspect-at-point"),
        ("C-c C-d C-d", "inspect-at-point"),
        ("C-x u", "#f"),
    ] {
        assert_eq!(bound(&mut rt, "emacs-map", keys), command, "{keys}");
    }
    for (keys, command) in [("C-;", "minibuffer-act"), ("C-c C-e", "minibuffer-export"), ("C-c C-;", "minibuffer-export")] {
        assert_eq!(bound(&mut rt, "minibuffer-map", keys), command, "{keys}");
    }
}
