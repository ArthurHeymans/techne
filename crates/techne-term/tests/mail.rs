//! Check mail through the real terminal layout, including responsive navigation.
use std::os::unix::fs::PermissionsExt;
use techne_editor::{
    present::{CursorShape, Output},
    runtime::Runtime,
};
use techne_term::{Depth, Term};
use techne_text::Document;

fn present(rt: &mut Runtime, term: &mut Term) -> String {
    for _ in 0..10 {
        let inputs = term.output(Output::Snapshot(Box::new(rt.snapshot())));
        if inputs.is_empty() {
            let grid = term.draw();
            return (0..grid.rows).map(|r| grid.row_text(r)).collect::<Vec<_>>().join("\n");
        }
        for input in inputs {
            rt.handle(input);
        }
    }
    panic!("frontend/runtime feedback did not settle");
}

#[test]
fn reader_next_and_previous_keep_the_selected_title_visible_in_both_layouts() {
    for position in ["bottom", "side"] {
        let dir = tempfile::Builder::new().permissions(std::fs::Permissions::from_mode(0o700)).tempdir().unwrap();
        let mut rt = Runtime::with_document(Document::new("previous"), "emacs").unwrap();
        rt.eval(&format!(
            "(require {:?}) (mail-test-seed-navigation! {:?} 100)
             (set-option! 'mail-reader-position '{position})
             (run-command (current-session) 'mail 1)
             (run-command (current-session) 'mail-all 1)",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../lisp/tests/fixtures/mail/mail-navigation-fixture.scm"),
            dir.path().join("navigation.sqlite").display().to_string()
        ))
        .unwrap();
        let mut term = Term::new(160, 50);
        present(&mut rt, &mut term);
        for (name, key, steps, expected) in [("read", b"\r", 1, "000"), ("next", b"n", 40, "040"), ("previous", b"p", 25, "015")] {
            for _ in 0..steps {
                for input in term.feed(key) {
                    rt.handle(input);
                }
                let text = present(&mut rt, &mut term);
                assert!(rt.take_errors().is_empty(), "{text}");
            }
            let text = present(&mut rt, &mut term);
            let summary = if position == "bottom" {
                text.lines().take_while(|l| !l.contains("(mail-summary)")).collect::<Vec<_>>().join("\n")
            } else {
                text.lines().map(|l| l.split('│').next().unwrap()).collect::<Vec<_>>().join("\n")
            };
            assert!(summary.contains(&format!("Nav item {expected}")), "selected title is offscreen in {position}: {text}");
            let snap = rt.snapshot();
            assert_eq!(snap.focus, 1);
            if expected == "040" {
                assert!(snap.panes[0].scroll > 0, "the unfocused list must actually scroll");
            }
            println!("Mail {position} after {name}, item {expected}:\n{text}");
        }
    }
}

#[test]
fn the_dashboard_is_a_full_frame_of_rows_with_a_highlight_not_a_caret() {
    let dir = tempfile::Builder::new().permissions(std::fs::Permissions::from_mode(0o700)).tempdir().unwrap();
    let mut rt = Runtime::with_document(Document::new("previous"), "emacs").unwrap();
    rt.eval(&format!(
        "(require {:?}) (mail-test-seed-samples! {:?}) (run-command (current-session) 'mail 1)",
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../lisp/tests/fixtures/mail/mail-sample-fixture.scm"),
        dir.path().join("samples.sqlite").display().to_string()
    ))
    .unwrap();
    let mut term = Term::new(100, 40);
    term.set_depth(Depth::Truecolor);
    let text = present(&mut rt, &mut term);
    assert_eq!(rt.snapshot().panes.len(), 1);
    assert!(term.draw().cursor.is_none(), "rows are selected by a highlight: {text}");
    let rows: Vec<&str> = text.lines().collect();
    let inbox = rows.iter().find(|l| l.contains("Inbox") && l.contains(" i ")).unwrap();
    let personal = rows.iter().find(|l| l.contains("Personal")).unwrap();
    assert!(inbox.trim_end().ends_with('7') && personal.trim_end().ends_with('7'), "counts share a column: {text}");
    let column = |l: &str| l.chars().position(|c| c == '4');
    assert_eq!(column(inbox), column(personal), "unread counts share a column: {text}");
    assert!(text.contains("RET open"), "{text}");
    println!("Mail dashboard 100x40:\n{text}");
    for input in term.feed(b"i") {
        rt.handle(input);
    }
    let text = present(&mut rt, &mut term);
    assert_eq!(rt.snapshot().panes.len(), 1, "the list alone until RET opens a message: {text}");
    assert!(text.contains("Inbox · 7 messages"), "{text}");
    assert!(rt.take_errors().is_empty());
}

#[test]
fn mail_cells_are_usable_wide_and_narrow_and_support_paging() {
    let dir = tempfile::Builder::new().permissions(std::fs::Permissions::from_mode(0o700)).tempdir().unwrap();
    let mut rt = Runtime::with_document(Document::new("previous"), "emacs").unwrap();
    rt.eval(&format!(
        "(require {:?}) (mail-test-seed-samples! {:?})
         (run-command (current-session) 'mail 1)
         (run-command (current-session) 'mail-inbox 1)",
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../lisp/tests/fixtures/mail/mail-sample-fixture.scm"),
        dir.path().join("samples.sqlite").display().to_string()
    ))
    .unwrap();
    let mut wide = Term::new(140, 50);
    wide.set_depth(Depth::Truecolor);
    let text = present(&mut rt, &mut wide);
    assert_eq!(rt.snapshot().panes.len(), 1, "the list opens alone: {text}");
    // RET opens the message below it, b goes back to the list.
    for input in wide.feed(b"\rb") {
        rt.handle(input);
    }
    let text = present(&mut rt, &mut wide);
    assert_eq!(rt.snapshot().panes.len(), 2);
    assert!(text.contains("Inbox · 7 messages"));
    assert_eq!(rt.snapshot().panes[0].cursor, CursorShape::Hidden);
    assert!(wide.draw().cursor.is_none(), "the message list has a row highlight, not an editing caret");
    assert!(text.contains("Reply"), "{text}");
    assert!(text.contains("conversations we return to."), "ordinary words must not split across wide rows: {text}");
    assert!(text.contains("*mail reader*"));
    assert!(text.contains("RET read"));
    assert!(text.lines().position(|l| l.contains("Elias Hart <elias@example.org>")).unwrap() > 8, "reader belongs below the list: {text}");
    assert!(rt.snapshot().panes[0].text.to_string().contains("A place for the longer conversations"));
    println!("Mail bottom grid 140x50:\n{text}");
    rt.eval("(run-command (current-session) 'mail-toggle-reader-position 1)").unwrap();
    let text = present(&mut rt, &mut wide);
    assert!(
        text.lines().position(|l| l.contains("Elias Hart <elias@example.org>")).unwrap() < 4,
        "side reader starts beside summaries: {text}"
    );
    let dates: Vec<usize> =
        text.lines().filter_map(|l| l.split('│').next().unwrap().find("Nov ").map(|at| l[..at].chars().count())).collect();
    assert!(dates.len() == 7 && dates.iter().all(|at| *at == dates[0]), "dates share a column: {text}");
    let senders: Vec<usize> = ["Elias Hart", "Jonas Berg", "Nora Lind"]
        .iter()
        .map(|name| text.lines().find_map(|l| l.find(name).map(|at| l[..at].chars().count())).unwrap())
        .collect();
    assert!(senders.iter().all(|at| *at == senders[0]), "senders share a column, whatever precedes them: {text}");
    assert!(text.contains("╰─► "), "a reply is drawn under its parent: {text}");
    println!("Mail side grid 140x50:\n{text}");
    rt.eval("(run-command (current-session) 'mail-toggle-reader-position 1)").unwrap();
    let mut tall = Term::new(80, 50);
    let text = present(&mut rt, &mut tall);
    assert_eq!(rt.snapshot().panes.len(), 2, "bottom layout can use a tall narrow viewport");
    assert!(text.contains("Hi Arthur,"), "bottom reader is visible: {text}");
    println!("Mail bottom grid 80x50:\n{text}");

    let mut narrow = Term::new(80, 30);
    narrow.set_depth(Depth::Truecolor);
    let text = present(&mut rt, &mut narrow);
    assert_eq!(rt.snapshot().panes.len(), 1, "responsive layout must not cram two panes into 80 columns");
    assert!(text.contains("A place for"), "single-line subject is visible: {text}");
    assert!(text.contains("RET read"));
    assert!(!text.contains("<html>"));
    println!("Mail summaries 80x30:\n{text}");
    rt.eval("(run-command (current-session) 'mail-read 1)").unwrap();
    let text = present(&mut rt, &mut narrow);
    assert!(text.contains("▸ to Arthur"));
    assert!(text.contains("Hi Arthur,"));
    assert!(text.contains("the conversations we return to."), "word-aware narrow prose: {text}");
    assert!(text.lines().position(|l| l.contains("Hi Arthur,")).unwrap() < 8, "compact header: {text}");
    assert!(text.contains("*mail reader*"));
    println!("Mail reader 80x30:\n{text}");
    for input in narrow.feed(b" ") {
        rt.handle(input);
    }
    let text = present(&mut rt, &mut narrow);
    assert!(rt.snapshot().panes[0].scroll > 0, "SPC pages the reader");
    assert!(text.contains("Back to messages"), "footer is reachable: {text}");
    println!("Mail reader paged 80x30:\n{text}");
    for input in narrow.feed(b"b") {
        rt.handle(input);
    }
    assert!(present(&mut rt, &mut narrow).contains("Inbox · 7 messages"));
    assert!(rt.take_errors().is_empty());
}
