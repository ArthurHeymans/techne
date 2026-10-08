//! Help: what a name is, what a key does and why, in both profiles, with
//! keys in docstrings shown as the user's.

use std::{path::Path, time::Instant};

use techne_editor::{present::Input, runtime::Runtime};

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn type_text(rt: &mut Runtime, text: &str) {
    for c in text.chars() {
        rt.handle(Input::Key { key: if c == ' ' { "SPC".into() } else { c.to_string() }, at: Instant::now() });
    }
}

fn file(dir: &Path, name: &str, profile: &str) -> Runtime {
    std::fs::write(dir.join(name), "").unwrap();
    let mut rt = Runtime::open(&dir.join(name), &dir.join("journal"), profile).unwrap().0;
    with_help(&mut rt);
    rt
}

/// Let `eval` see help's and checkdoc's procedures: they are the
/// application's, not the library's.
fn with_help(rt: &mut Runtime) {
    for file in ["help.scm", "checkdoc.scm"] {
        rt.eval(&format!("(require {:?})", techne_editor::runtime::lisp_dir().join(file).display().to_string())).unwrap();
    }
}

/// The focused pane's text, the space between columns squashed to one.
fn text(rt: &mut Runtime) -> String {
    let text = rt.snapshot().pane().text.to_string();
    text.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ") + "\n").collect()
}

/// C-h f on a command: its signature, definition, documentation and key;
/// RET on a name in the documentation describes it; l goes back.
#[test]
fn describing_a_function() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.txt", "emacs");
    keys(&mut rt, "C-h f");
    type_text(&mut rt, "save-buffer");
    keys(&mut rt, "RET");
    let help = text(&mut rt);
    assert!(help.starts_with("procedure (save-buffer s n)\ndefined "), "{help}");
    assert!(help.contains("lisp/editor/main.scm:"), "{help}");
    assert!(help.contains("doc Write the document to its file.\ncommand save-buffer\nkeys C-x C-s\n"), "{help}");
    // A built-in: its signature and documentation come from Rust.
    keys(&mut rt, "C-h f");
    type_text(&mut rt, "string-split");
    keys(&mut rt, "RET");
    let help = text(&mut rt);
    assert!(help.starts_with("built-in procedure (string-split string [separator])\ndefined "), "{help}");
    assert!(help.contains("stdlib.rs:"), "{help}");
    keys(&mut rt, "l");
    assert!(text(&mut rt).starts_with("procedure (save-buffer s n)"));
}

/// C-h k says what a key runs and which keymap binds it, and what that
/// shadows.
#[test]
fn describing_a_key() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.scm"), "").unwrap();
    let mut rt = Runtime::open(&dir.path().join("a.scm"), &dir.path().join("journal"), "emacs").unwrap().0;
    with_help(&mut rt);
    keys(&mut rt, "C-h k C-x C-s");
    let help = text(&mut rt);
    assert!(help.starts_with("key C-x C-s\nruns save-buffer\nbound in emacs: save-buffer\n"), "{help}");
    // The fewest keys first, and keys the profile reads itself.
    let doc = rt.eval("(substitute-command-keys (current-session) \"\\\\[find-file], \\\\[keyboard-quit]\")").unwrap();
    assert_eq!(doc, "\"C-x C-f, C-g\"");
    // Keys a mode binds over the profile's.
    keys(&mut rt, "C-x b");
    type_text(&mut rt, "a.scm");
    keys(&mut rt, "RET C-h k C-M-x");
    let help = text(&mut rt);
    assert!(help.starts_with("key C-M-x\nruns eval-defun\nbound in scheme-mode: eval-defun\n"), "{help}");
}

/// The same help in the modal profile, under SPC h, with its own keys in
/// the documentation.
#[test]
fn help_in_the_modal_profile() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.txt", "modal");
    keys(&mut rt, "SPC h k SPC f f");
    assert!(text(&mut rt).starts_with("key SPC f f\nruns find-file\nbound in modal: find-file\n"), "{}", text(&mut rt));
    let keys_of = |rt: &mut Runtime, doc: &str| rt.eval(&format!("(substitute-command-keys (current-session) {doc:?})")).unwrap();
    assert_eq!(
        keys_of(&mut rt, "Type \\[find-file] or \\[execute-extended-command]; `\\[find-file]`."),
        "\"Type SPC . or SPC :; `\\\\[find-file]`.\""
    );
    assert_eq!(keys_of(&mut rt, "\\[checkdoc]"), "\"SPC : checkdoc\"");
}

/// Apropos finds names of every kind; describe-mode shows the buffer's
/// modes and their keys.
#[test]
fn apropos_and_modes() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.txt", "emacs");
    keys(&mut rt, "C-h a");
    type_text(&mut rt, "line numbers");
    keys(&mut rt, "M-RET");
    let help = text(&mut rt);
    assert!(help.starts_with("line-numbers option Line numbers beside the text"), "{help}");
    keys(&mut rt, "C-x b");
    type_text(&mut rt, "a.txt");
    keys(&mut rt, "RET C-h m");
    let help = text(&mut rt);
    assert!(help.starts_with("major mode text-mode\nparent fundamental-mode\ndoc Prose"), "{help}");
}

/// The manual, keys in it shown as the profile binds them; every key and
/// name it refers to exists.
#[test]
fn the_manual() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.txt", "modal");
    keys(&mut rt, "SPC h r");
    let manual = text(&mut rt);
    assert!(manual.starts_with("#+title: The Techne Manual\n"), "{manual}");
    assert!(manual.contains("SPC . is the key that opens a file."), "{manual}");
    assert!(manual.contains("keys are written ~\\\\[command]~"), "{manual}");
    let problems = rt
        .eval("(map (lambda (p) (string-append (symbol->string (problem-name p)) \": \" (problem-text p))) (manual-problems (call-with-input-file (manual-file) read-string-all)))")
        .unwrap();
    assert_eq!(problems, "()");
}
