//! *Messages*, M-: and the REPL.

use std::time::Instant;

use techne_editor::{present::Input, runtime::Runtime};
use techne_text::Document;

fn rt(text: &str) -> Runtime {
    Runtime::with_document(Document::new(text), "emacs").unwrap()
}

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

fn text(rt: &mut Runtime) -> String {
    rt.snapshot().pane().text.to_string()
}

#[test]
fn messages_are_kept() {
    let mut r = rt("");
    keys(&mut r, "C-c C-q C-c C-q C-g");
    keys(&mut r, "C-h e");
    let s = r.snapshot();
    assert!(s.pane().status.starts_with("*Messages*"), "{}", s.pane().status);
    assert_eq!(text(&mut r), "C-c C-q is undefined [2 times]\nQuit\n", "the same message counted");
    assert_eq!(s.pane().head(), s.pane().text.len_bytes(), "at its end");
    // Read-only.
    keys(&mut r, "x");
    assert_eq!(r.snapshot().echo, "view-edit!: This buffer is read-only");
}

#[test]
fn evaluating_an_expression() {
    let mut r = rt("");
    keys(&mut r, "M-:");
    assert_eq!(r.snapshot().minibuffer.unwrap().prompt, "Eval: ");
    type_text(&mut r, "(+ 1 2)");
    keys(&mut r, "RET");
    assert_eq!(r.snapshot().echo, "3");
}
