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
    while rt.run_tasks(Duration::ZERO).unwrap() == techne_editor::runtime::Progress::OutOfTime {}
    std::thread::sleep(Duration::from_millis(300));
    rt.run_tasks(Duration::from_millis(500)).unwrap();
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

/// Filtering uses the minibuffer's matcher without changing source order,
/// smart case or Unicode matching. Deleting restores candidates filtered out
/// earlier, including when a new pattern branches from the previous one.
#[test]
fn filtering_and_restoring_candidates_in_both_profiles() {
    for profile in ["emacs", "modal"] {
        let mut rt = Runtime::with_document(Document::new(""), profile).unwrap();
        rt.eval(
            r#"(define-mode test-completion-mode "A completion source for a test."
                 #:parent 'scheme-mode
                 #:complete (lambda (b p)
                   (cons 0 (map (lambda (name) (candidate name #:annotation name))
                                '("zeta-map" "Alpha-map" "alpha-reduce" "äpfel-map" "Äpfel-list")))))
               (set-buffer-mode! (current-buffer (current-session)) 'test-completion-mode)"#,
        )
        .unwrap();
        if profile == "modal" {
            keys(&mut rt, "i");
        }
        keys(&mut rt, "C-M-i");
        let all = shown(&mut rt);
        assert_eq!(all, ["zeta-map", "Alpha-map", "alpha-reduce", "äpfel-map", "Äpfel-list"]);
        type_text(&mut rt, "alpha");
        assert_eq!(shown(&mut rt), ["Alpha-map", "alpha-reduce"]);
        type_text(&mut rt, "-r");
        assert_eq!(shown(&mut rt), ["alpha-reduce"]);
        keys(&mut rt, "DEL DEL");
        assert_eq!(text(&mut rt), "alpha");
        assert_eq!(shown(&mut rt), ["Alpha-map", "alpha-reduce"]);
        type_text(&mut rt, "-m");
        assert_eq!(shown(&mut rt), ["Alpha-map"], "branching must restore candidates discarded by -r");
        keys(&mut rt, "DEL DEL");
        assert_eq!(shown(&mut rt), ["Alpha-map", "alpha-reduce"]);
        keys(&mut rt, "DEL DEL DEL DEL DEL");
        assert_eq!(shown(&mut rt), all);
        type_text(&mut rt, "A");
        assert_eq!(shown(&mut rt), ["Alpha-map"], "upper case is literal");
        keys(&mut rt, "DEL");
        type_text(&mut rt, "ä");
        assert_eq!(shown(&mut rt), ["äpfel-map", "Äpfel-list"]);
        keys(&mut rt, "DEL");
        type_text(&mut rt, "Ä");
        assert_eq!(shown(&mut rt), ["Äpfel-list"]);
        let completion = rt.snapshot().completion.unwrap();
        assert_eq!(completion.rows[0].text(1), "Äpfel-list", "annotation belongs to the matched entry");
        keys(&mut rt, "C-n");
        assert_eq!(text(&mut rt), "Äpfel-list");
        keys(&mut rt, "C-n");
        assert_eq!(text(&mut rt), "Ä", "cycling returns to the typed prompt");
        type_text(&mut rt, "z");
        assert!(shown(&mut rt).is_empty(), "no match closes the popup");
        assert_eq!(text(&mut rt), "Äz");
        keys(&mut rt, "DEL C-M-i");
        assert_eq!(shown(&mut rt), ["Äpfel-list"]);
        keys(&mut rt, "C-g");
        assert!(shown(&mut rt).is_empty());
        assert!(rt.take_errors().is_empty());
    }
}

/// Editing a selected preview keeps its text, then filters from that text
/// rather than the original prompt. It also clears the candidate selection.
#[test]
fn editing_a_selected_preview_in_both_profiles() {
    for profile in ["emacs", "modal"] {
        let dir = tempfile::tempdir().unwrap();
        let mut rt = file(dir.path(), "a.scm", "", profile);
        if profile == "modal" {
            keys(&mut rt, "i");
        }
        type_text(&mut rt, "(vector-r");
        keys(&mut rt, "C-M-i C-n");
        assert_eq!(text(&mut rt), "(vector-ref");
        assert_eq!(rt.snapshot().completion.unwrap().selected, Some(0));
        keys(&mut rt, "DEL");
        assert_eq!(text(&mut rt), "(vector-re");
        assert_eq!(shown(&mut rt), ["vector-ref"]);
        assert_eq!(rt.snapshot().completion.unwrap().selected, None);
        keys(&mut rt, "C-n");
        assert_eq!(text(&mut rt), "(vector-ref");
        type_text(&mut rt, "x");
        assert_eq!(text(&mut rt), "(vector-refx");
        assert!(shown(&mut rt).is_empty());
        assert!(rt.take_errors().is_empty());
    }
}

/// Match indices differ from entry indices after filtering. Scrolling and
/// selecting beyond the first page must retain the corresponding annotation.
#[test]
fn filtered_candidates_scroll_and_select_by_entry() {
    let mut rt = Runtime::with_document(Document::new(""), "emacs").unwrap();
    rt.eval(
        r#"(define-mode test-completion-mode "A paged completion source for a test."
             #:parent 'scheme-mode
             #:complete (lambda (b p)
               (cons 0 (map (lambda (i)
                              (candidate (string-append (if (even? i) "skip-" "keep-") (number->string i))
                                         #:annotation (number->string i)))
                            (iota 50)))))
           (set-buffer-mode! (current-buffer (current-session)) 'test-completion-mode)"#,
    )
    .unwrap();
    type_text(&mut rt, "keep-");
    keys(&mut rt, "C-M-i");
    assert_eq!(shown(&mut rt).len(), 16);
    for _ in 0..17 {
        keys(&mut rt, "C-n");
    }
    let completion = rt.snapshot().completion.unwrap();
    assert_eq!(completion.selected, Some(15));
    assert_eq!(completion.rows[0].text(0), "keep-3");
    assert_eq!(completion.rows[15].text(0), "keep-33");
    assert_eq!(completion.rows[15].text(1), "33");
    assert_eq!(text(&mut rt), "keep-33");
    keys(&mut rt, "C-g");
    assert_eq!(text(&mut rt), "keep-");
    assert!(rt.take_errors().is_empty());
}

/// M-SPC puts in a separator that keeps the popup open, as Corfu's: the
/// parts each match, in any order, and a candidate taken replaces them all.
#[test]
fn completing_with_parts_after_a_separator() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.scm", "", "emacs");
    type_text(&mut rt, "(ref");
    pause(&mut rt);
    assert!(!shown(&mut rt).is_empty());
    keys(&mut rt, "M-SPC");
    assert_eq!(text(&mut rt), "(ref ");
    assert!(!shown(&mut rt).is_empty(), "the separator keeps the popup open");
    type_text(&mut rt, "vec");
    let names = shown(&mut rt);
    assert!(names.contains(&"vector-ref".to_string()), "{names:?}");
    assert!(names.iter().all(|n| n.contains("ref") && n.contains("vec")), "every part matches, in any order: {names:?}");
    // After a separator a space goes on completing too.
    type_text(&mut rt, " tor");
    assert!(shown(&mut rt).iter().all(|n| n.contains("tor")), "{:?}", shown(&mut rt));
    keys(&mut rt, "C-n");
    keys(&mut rt, "RET");
    assert!(text(&mut rt).starts_with("(") && !text(&mut rt).contains(' '), "the candidate replaced every part: {}", text(&mut rt));
    // Without a separator a space still ends it.
    type_text(&mut rt, " (vec");
    pause(&mut rt);
    assert!(!shown(&mut rt).is_empty());
    type_text(&mut rt, " ");
    assert!(shown(&mut rt).is_empty());
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
