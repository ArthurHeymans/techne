//! Completion in the buffer, as Corfu: in itl and Scheme buffers, from the
//! names the module sees.

use std::{path::Path, time::Duration, time::Instant};

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
            '\n' => "RET".to_string(),
            c => c.to_string(),
        };
        rt.handle(Input::Key { key, at: Instant::now() });
    }
}

fn text(rt: &mut Runtime) -> String {
    rt.snapshot().pane().text.to_string()
}

/// The candidates the popup shows, or none when it is closed.
fn shown(rt: &mut Runtime) -> Vec<String> {
    rt.snapshot().completion.map_or(Vec::new(), |c| c.rows.iter().map(|r| r.text(0)).collect())
}

/// Let the pause after typing pass, as corfu-auto waits.
fn pause(rt: &mut Runtime) {
    // The tasks the keys started wait from now.
    while rt.run_tasks(Duration::ZERO) == techne_editor::runtime::Progress::OutOfTime {}
    std::thread::sleep(Duration::from_millis(300));
    rt.run_tasks(Duration::from_millis(500));
}

fn file(dir: &Path, name: &str, text: &str, profile: &str) -> Runtime {
    std::fs::write(dir.join(name), text).unwrap();
    Runtime::open(&dir.join(name), &dir.join("journal"), profile).unwrap().0
}

/// TAB in itl completes as ielm's does: the candidates matching what was
/// typed, the shortest first; TAB again selects one, which goes in the
/// text; RET takes it; RET with none selected evaluates.
#[test]
fn completing_in_itl() {
    let mut rt = Runtime::with_document(Document::new(""), "emacs").unwrap();
    keys(&mut rt, "M-x");
    type_text(&mut rt, "itl");
    keys(&mut rt, "RET");
    type_text(&mut rt, "(string-ap");
    keys(&mut rt, "TAB");
    let s = rt.snapshot();
    let c = s.completion.as_ref().expect("the popup");
    assert_eq!(c.rows[0].text(0), "string-append");
    assert_eq!(c.rows[0].text(1), "procedure");
    assert_eq!(c.selected, None, "nothing is selected at first");
    assert!(text(&mut rt).ends_with("techne> (string-ap"), "{}", text(&mut rt));
    keys(&mut rt, "TAB");
    assert!(text(&mut rt).ends_with("(string-append"), "the selected one is in the text");
    assert_eq!(rt.snapshot().completion.unwrap().selected, Some(0));
    keys(&mut rt, "RET");
    assert!(rt.snapshot().completion.is_none());
    type_text(&mut rt, " \"a\" \"b\")");
    keys(&mut rt, "RET");
    assert!(text(&mut rt).ends_with("(string-append \"a\" \"b\")\n\"ab\"\ntechne> "), "{}", text(&mut rt));
    // RET with nothing selected does what it does without the popup.
    type_text(&mut rt, "(car '(1 2))");
    keys(&mut rt, "C-b C-b C-b C-b C-b C-b C-b C-b C-b C-b TAB");
    assert!(!shown(&mut rt).is_empty());
    keys(&mut rt, "RET");
    assert!(text(&mut rt).ends_with("(car '(1 2))\n1\ntechne> "), "{} / {}", text(&mut rt), rt.snapshot().echo);
}

/// In a Scheme buffer the popup opens by itself after two characters and
/// a pause; typing on narrows it (as Orderless matches), C-g puts back
/// what was typed, and a character no name has closes it.
#[test]
fn completing_while_typing() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.scm", "", "emacs");
    type_text(&mut rt, "(v");
    pause(&mut rt);
    assert!(shown(&mut rt).is_empty(), "not after one character");
    type_text(&mut rt, "ector-r");
    assert!(shown(&mut rt).is_empty(), "not before the pause");
    pause(&mut rt);
    let names = shown(&mut rt);
    assert_eq!(names.first().map(String::as_str), Some("vector-ref"), "{names:?}");
    type_text(&mut rt, "e");
    assert_eq!(shown(&mut rt), ["vector-ref"]);
    keys(&mut rt, "C-n");
    assert_eq!(text(&mut rt), "(vector-ref");
    // Cycling passes the prompt: what was typed.
    keys(&mut rt, "C-n");
    assert_eq!(text(&mut rt), "(vector-re");
    keys(&mut rt, "C-n");
    keys(&mut rt, "C-g");
    assert_eq!(text(&mut rt), "(vector-re");
    assert!(shown(&mut rt).is_empty());
    // Typing again opens it again; a space ends the identifier and closes it.
    type_text(&mut rt, "f");
    pause(&mut rt);
    assert!(!shown(&mut rt).is_empty());
    type_text(&mut rt, " ");
    assert!(shown(&mut rt).is_empty());
    // Names the file defines, once evaluated.
    type_text(&mut rt, "(vector 1) 0)\n(define (greet-everyone) 1)\n(greet-e");
    keys(&mut rt, "C-c C-k C-M-i");
    assert_eq!(shown(&mut rt), ["greet-everyone"]);
}

/// Another actor edits while the popup is open: an edit before the
/// identifier is followed; one inside it is kept, and the popup closes.
#[test]
fn completing_around_another_actors_edits() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.scm", "", "emacs");
    let other = |at: usize, text: &str| {
        format!("(view-edit! (make-view (session-document (current-session)) \"agent\") '(({at} {at} {text:?})) \"new\")")
    };
    type_text(&mut rt, "(vector-r");
    keys(&mut rt, "C-M-i");
    assert!(!shown(&mut rt).is_empty());
    rt.eval(&other(0, "! ")).unwrap();
    keys(&mut rt, "C-n");
    assert_eq!(text(&mut rt), "! (vector-ref");
    rt.eval(&other(4, "X")).unwrap();
    keys(&mut rt, "C-n");
    assert_eq!(text(&mut rt), "! (vXector-ref");
    assert!(shown(&mut rt).is_empty());
    assert!(rt.snapshot().echo.contains("changed"), "{}", rt.snapshot().echo);
}

/// Only where the mode completes: prose has nothing to complete.
#[test]
fn no_completion_in_prose() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "notes.txt", "", "emacs");
    type_text(&mut rt, "string");
    pause(&mut rt);
    assert!(shown(&mut rt).is_empty());
}

/// In the modal profile the popup is insert state's: C-n selects, ESC
/// leaves insert state and closes it.
#[test]
fn completing_in_insert_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.scm", "", "modal");
    keys(&mut rt, "i");
    type_text(&mut rt, "(string-up");
    pause(&mut rt);
    assert_eq!(shown(&mut rt).first().map(String::as_str), Some("string-upcase"));
    keys(&mut rt, "C-n");
    assert_eq!(text(&mut rt), "(string-upcase");
    keys(&mut rt, "ESC");
    assert!(shown(&mut rt).is_empty());
    assert_eq!(text(&mut rt), "(string-upcase");
}
