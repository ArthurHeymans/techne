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

/// Run a session's background tasks until `done` holds of its snapshot,
/// as `Runtime::serve` does between inputs.
fn until(rt: &mut Runtime, done: impl Fn(&techne_editor::present::Snapshot) -> bool) -> techne_editor::present::Snapshot {
    let start = Instant::now();
    loop {
        rt.run_tasks(std::time::Duration::from_millis(5));
        let s = rt.snapshot();
        if done(&s) {
            return s;
        }
        assert!(start.elapsed().as_secs() < 20, "timed out: {}", s.echo);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn shell_commands() {
    let mut r = rt("b\na\n");
    // One line of output: the echo area.
    keys(&mut r, "M-!");
    type_text(&mut r, "echo hi");
    keys(&mut r, "RET");
    until(&mut r, |s| s.echo == "hi");
    // More: a buffer shown below, the focus staying.
    keys(&mut r, "M-!");
    type_text(&mut r, "printf 'x\\ny\\n'; exit 3");
    keys(&mut r, "RET");
    let s = until(&mut r, |s| s.panes.len() == 2);
    assert_eq!(s.panes[1].text.to_string(), "x\ny\n");
    assert_eq!(s.focus, 0);
    assert_eq!(s.echo, "Shell command finished (exit 3)");
    // The region as input.
    keys(&mut r, "C-x 1 C-SPC M->");
    keys(&mut r, "M-|");
    type_text(&mut r, "sort");
    keys(&mut r, "RET");
    let s = until(&mut r, |s| s.panes.len() == 2);
    assert_eq!(s.panes[1].text.to_string(), "a\nb\n");
    // In the background, output as it comes.
    keys(&mut r, "C-x 1 M-&");
    type_text(&mut r, "echo one; sleep 0.2; echo two");
    keys(&mut r, "RET");
    until(&mut r, |s| s.panes.len() == 2 && s.panes[1].text == "one\n");
    let s = until(&mut r, |s| s.panes[1].text == "one\ntwo\n");
    assert!(s.panes[1].status.starts_with("*Async Shell Command*"));
}

#[test]
fn a_repl() {
    let mut r = rt("");
    keys(&mut r, "M-x");
    type_text(&mut r, "itl");
    keys(&mut r, "RET");
    assert_eq!(text(&mut r), ";; Interactive Techne Lisp, evaluating in user\ntechne> ");
    // An incomplete input gets a new line; a complete one is evaluated,
    // with what it printed.
    type_text(&mut r, "(begin (display \"hi\")");
    keys(&mut r, "RET");
    type_text(&mut r, "(+ 1 2))");
    keys(&mut r, "RET");
    assert_eq!(text(&mut r), ";; Interactive Techne Lisp, evaluating in user\ntechne> (begin (display \"hi\")\n(+ 1 2))\nhi\n3\ntechne> ");
    // A definition has no value to show.
    type_text(&mut r, "(define z 1)");
    keys(&mut r, "RET");
    assert!(text(&mut r).ends_with("3\ntechne> (define z 1)\ntechne> "), "{}", text(&mut r));
    // Errors, the transcript read-only, history.
    type_text(&mut r, "(car 1)");
    keys(&mut r, "RET");
    assert!(text(&mut r).contains("\nerror: car: expected pair"), "{}", text(&mut r));
    keys(&mut r, "M-< x");
    assert!(r.snapshot().echo.contains("generated"));
    keys(&mut r, "M-> M-p M-p M-p");
    assert!(text(&mut r).ends_with("techne> (begin (display \"hi\")\n(+ 1 2))"));
    keys(&mut r, "M-n M-n M-n C-a");
    type_text(&mut r, "(list 1 2)");
    keys(&mut r, "RET C-c M-i");
    assert!(text(&mut r).starts_with("value  (1 2)"), "the value is inspected");
}
