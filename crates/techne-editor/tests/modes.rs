//! Buffers and their major modes: keys per mode and input state, from one
//! resolver.

use std::{path::Path, time::Instant};

use techne_editor::{present::Input, runtime::Runtime};
use techne_text::Document;

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn type_text(rt: &mut Runtime, text: &str) {
    for c in text.chars() {
        let key = match c {
            ' ' => "SPC".to_string(),
            c => c.to_string(),
        };
        rt.handle(Input::Key { key, at: Instant::now() });
    }
}

fn file(dir: &Path, name: &str, text: &str, profile: &str) -> Runtime {
    std::fs::write(dir.join(name), text).unwrap();
    Runtime::open(&dir.join(name), &dir.join("journal"), profile).unwrap().0
}

/// A file's name chooses its mode, whose keys apply there only: Geiser's
/// C-M-x in Scheme, nothing in prose; C-x C-e is global, as in Arthur's
/// Emacs.
#[test]
fn the_mode_of_a_file_and_its_keys() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "notes.txt", "(+ 1 2)", "emacs");
    assert!(rt.snapshot().pane().status.ends_with("(text)"), "{}", rt.snapshot().pane().status);
    keys(&mut rt, "C-e C-M-x");
    assert_eq!(rt.snapshot().echo, "C-M-x is undefined");
    keys(&mut rt, "C-x C-e");
    assert_eq!(rt.snapshot().echo, "3");
    // M-x scheme-mode: the same text, now Scheme.
    keys(&mut rt, "M-x");
    type_text(&mut rt, "scheme-mode");
    keys(&mut rt, "RET C-M-x");
    assert_eq!(rt.snapshot().echo, "3");
    assert!(rt.snapshot().pane().status.ends_with("(scheme)"));
    let mode = |rt: &mut Runtime, name: &str| rt.eval(&format!("(mode-for-file {name:?})")).unwrap();
    assert_eq!(mode(&mut rt, "a/b.rs"), "prog-mode");
    assert_eq!(mode(&mut rt, "init.scm"), "scheme-mode");
    assert_eq!(mode(&mut rt, "Makefile"), "fundamental-mode");
}

/// A mode extends its parent's keys; a key a child binds shadows the
/// parent's. The inspector has a structured view's keys and its own `l`.
#[test]
fn modes_inherit_keys() {
    let mut rt = Runtime::with_document(Document::new("(list 1 2)"), "emacs").unwrap();
    keys(&mut rt, "C-c C-k C-c M-i");
    let maps = rt.eval("(map mode-name (mode-chain (buffer-mode (current-buffer (current-session)))))").unwrap();
    assert_eq!(maps, "(inspector-mode rows-mode special-mode fundamental-mode)");
    // C-c C-o is rows-mode's.
    keys(&mut rt, "M-> C-c C-o");
    assert!(rt.snapshot().pane().text.to_string().starts_with("value  2"), "{}", rt.snapshot().pane().text);
    keys(&mut rt, "l");
    assert!(rt.snapshot().pane().text.to_string().starts_with("value  (1 2)"));
    // A key bound in a mode's keymap later applies to its buffers, and to
    // those of the modes extending it.
    rt.eval("(define-key! (mode-map 'special-mode) \"C-c z\" 'end-of-buffer)").unwrap();
    keys(&mut rt, "M-< C-c z");
    assert_eq!(rt.snapshot().pane().head(), rt.snapshot().pane().text.len_bytes());
}

/// In the modal profile a mode's chord keys apply in insert state, its
/// normal keys in normal state, and Vim's own keys are not shadowed by
/// chord keys: RET evaluates in itl, `l` moves in the inspector.
#[test]
fn modes_in_the_modal_profile() {
    let mut rt = Runtime::with_document(Document::new(""), "modal").unwrap();
    keys(&mut rt, "SPC :");
    type_text(&mut rt, "itl");
    keys(&mut rt, "RET A");
    type_text(&mut rt, "(list 1 2)");
    keys(&mut rt, "RET");
    assert!(rt.snapshot().pane().text.to_string().ends_with("(list 1 2)\n(1 2)\ntechne> "), "{}", rt.snapshot().pane().text);
    // The inspector: `l` moves in normal state; RET and C-c C-r are its.
    keys(&mut rt, "ESC");
    rt.eval("(run-command (current-session) 'inspect-last-result 1)").unwrap();
    let text = rt.snapshot().pane().text.to_string();
    assert!(text.starts_with("value  (1 2)"), "{text}");
    keys(&mut rt, "l");
    assert_eq!(rt.snapshot().pane().head(), 1, "l moves");
    keys(&mut rt, "G RET");
    assert!(rt.snapshot().pane().text.to_string().starts_with("value  2"), "{}", rt.snapshot().pane().text);
    keys(&mut rt, "C-c C-r");
    assert!(rt.snapshot().pane().text.to_string().starts_with("value  2"));
}

/// Killing a buffer shown in two panes shows the next one in both, each in
/// a view of its own that keeps what it is: a lens stays a lens.
#[test]
fn killing_a_buffer_shown_twice() {
    let mut rt = Runtime::with_document(Document::new("one two\n"), "emacs").unwrap();
    keys(&mut rt, "M-s o");
    type_text(&mut rt, "two");
    keys(&mut rt, "RET C-x b");
    type_text(&mut rt, "scratch");
    keys(&mut rt, "RET C-x 2 C-x k");
    let s = rt.snapshot();
    let names: Vec<&str> = s.panes.iter().map(|p| p.status.split("  ").next().unwrap()).collect();
    assert_eq!(names, ["*lens two*", "*lens two*"]);
    assert_ne!(s.panes[0].view, s.panes[1].view);
    // Edits go through to the source.
    keys(&mut rt, "C-e");
    type_text(&mut rt, "!");
    assert_eq!(rt.document().unwrap().borrow().text().to_string(), "one two!\n");
}

/// The runtime calls the session's procedures by name, so redefining one
/// while running changes what the next snapshot shows.
#[test]
fn redefining_what_the_runtime_calls() {
    let mut rt = Runtime::with_document(Document::new(""), "emacs").unwrap();
    rt.eval("(define (pane-status s view) \"mine\")").unwrap();
    assert_eq!(rt.snapshot().pane().status, "mine");
}

/// Options resolve by cell, the more specific first; a minor mode is on
/// where its option is, so Doom's prog-mode hook is one setting, and a
/// mode a buffer turns on ranks before its major mode's keys.
#[test]
fn options_and_minor_modes() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.scm", "; TODO\n", "emacs");
    rt.eval(&format!("(load-package 'todo {:?})", techne_editor::runtime::lisp_dir().join("examples/todo-mode.scm").display().to_string()))
        .unwrap();
    rt.eval("(set-option! 'todo-mode #t #:mode 'prog-mode)").unwrap();
    let s = rt.snapshot();
    assert_eq!(s.pane().layers.len(), 1, "on in a Scheme buffer");
    assert!(s.pane().status.ends_with("(scheme todo)"), "{}", s.pane().status);
    let explain = rt.eval("(explain-option (current-buffer (current-session)) 'todo-mode)").unwrap();
    assert_eq!(explain, "((prog-mode #t root) (default #f default))");
    keys(&mut rt, "C-h e");
    assert!(rt.snapshot().pane().status.ends_with("(log)"), "not in *Messages*: {}", rt.snapshot().pane().status);
    // Turned off in one buffer, the buffer's setting wins.
    keys(&mut rt, "C-x b RET M-x");
    type_text(&mut rt, "todo-mode");
    keys(&mut rt, "RET");
    assert!(rt.snapshot().pane().layers.is_empty());
    // A mode's own setting gives way to one made for that mode.
    rt.eval("(set-option! 'read-only #f #:mode 'log-mode)").unwrap();
    keys(&mut rt, "C-h e");
    type_text(&mut rt, "x");
    assert!(rt.snapshot().pane().text.to_string().ends_with('x'));
    // Values are checked against the option's type.
    assert!(rt.eval("(set-option! 'read-only 'yes)").is_err());
}

/// describe-option shows the settings that apply, the winner first.
#[test]
fn describing_an_option() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.txt", "", "emacs");
    keys(&mut rt, "C-u M-x");
    type_text(&mut rt, "set-option");
    keys(&mut rt, "RET");
    type_text(&mut rt, "read-only");
    keys(&mut rt, "RET");
    type_text(&mut rt, "#t");
    keys(&mut rt, "RET");
    assert_eq!(rt.snapshot().echo, "read-only is #t");
    keys(&mut rt, "M-x");
    type_text(&mut rt, "describe-option");
    keys(&mut rt, "RET");
    type_text(&mut rt, "read-only");
    keys(&mut rt, "RET");
    let text = rt.snapshot().pane().text.to_string();
    assert!(text.starts_with("option   read-only\nvalue    #t\ntype     boolean\n"), "{text}");
    assert!(text.ends_with("global   #t  set by root\ndefault  #f"), "{text}");
}
