//! The runtime over the presentation protocol: keys through Lisp, clicks and
//! scrolling resolved against older snapshots, quitting and saving.

use std::time::Instant;

use techne_editor::{
    present::{CursorShape, Input, Output},
    runtime::Runtime,
};
use techne_text::Document;

fn runtime(text: &str, profile: &str) -> Runtime {
    Runtime::with_document(Document::new(text), profile).unwrap()
}

fn keys(rt: &mut Runtime, keys: &str) -> Option<Output> {
    keys.split(' ').filter_map(|k| rt.handle(Input::Key { key: k.to_string(), at: Instant::now() })).last()
}

fn click(rt: &mut Runtime, revision: u64, pos: usize) {
    assert!(rt.handle(Input::Click { revision, pos, extend: false, at: Instant::now() }).is_none());
}

#[test]
fn keys_go_through_the_profile() {
    let mut rt = runtime("hello", "emacs");
    keys(&mut rt, "C-e ! C-x");
    let s = rt.snapshot();
    assert_eq!(s.text.to_string(), "hello!");
    assert_eq!(s.head(), 6);
    assert_eq!(s.cursor, CursorShape::Bar);
    assert_eq!(s.answers.len(), 3);
    assert!(s.status.contains("C-x-"), "pending prefix shown: {}", s.status);
    assert!(rt.snapshot().answers.is_empty());
}

#[test]
fn a_click_on_an_older_snapshot_is_re_resolved() {
    let mut rt = runtime("world", "emacs");
    let old = rt.snapshot();
    // Text inserted before the clicked position since the snapshot.
    keys(&mut rt, "h e l l o SPC C-e");
    click(&mut rt, old.revision, 2);
    let s = rt.snapshot();
    assert_eq!(s.text.to_string(), "hello world");
    assert_eq!(s.head(), 8, "the click lands on the same character");
}

#[test]
fn a_click_on_deleted_text_is_refused() {
    let mut rt = runtime("one two", "emacs");
    let old = rt.snapshot();
    keys(&mut rt, "M-f M-d");
    let before = rt.snapshot();
    click(&mut rt, old.revision, 5);
    let s = rt.snapshot();
    assert_eq!(s.head(), before.head(), "the caret did not move");
    assert!(s.status.contains("changed"), "{}", s.status);
    // A position past the end of that revision's text is refused too.
    click(&mut rt, old.revision, 99);
    assert_eq!(rt.snapshot().head(), before.head());
}

#[test]
fn the_scroll_anchor_follows_edits() {
    let mut rt = runtime("a\nb\nc\nd\n", "emacs");
    let s = rt.snapshot();
    rt.handle(Input::Scroll { revision: s.revision, anchor: 4 });
    assert_eq!(rt.snapshot().scroll, 4);
    keys(&mut rt, "x RET");
    assert_eq!(rt.snapshot().scroll, 6, "text inserted above the screen does not move it");
}

#[test]
fn modal_clicks_and_cursor() {
    let mut rt = runtime("abc\n", "modal");
    let s = rt.snapshot();
    assert_eq!(s.cursor, CursorShape::Block);
    click(&mut rt, s.revision, 3);
    assert_eq!(rt.snapshot().head(), 2, "the cursor stays on a character");
    keys(&mut rt, "i");
    assert_eq!(rt.snapshot().cursor, CursorShape::Bar);
}

#[test]
fn saving_and_quitting() {
    let dir = tempfile::tempdir().unwrap();
    let (path, journal) = (dir.path().join("f.txt"), dir.path().join("f.journal"));
    std::fs::write(&path, "x").unwrap();
    let (mut rt, _) = Runtime::open(&path, &journal, "modal").unwrap();
    assert!(matches!(keys(&mut rt, "A y ESC : w q RET"), Some(Output::Quit)));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "xy");

    let (mut rt, _) = Runtime::open(&path, &journal, "emacs").unwrap();
    keys(&mut rt, "z");
    assert!(rt.snapshot().status.contains("[+]"));
    assert!(matches!(keys(&mut rt, "C-x C-c"), Some(Output::Quit)));
    drop(rt);
    // Quitting without saving keeps the edit in the journal.
    let (mut rt, _) = Runtime::open(&path, &journal, "emacs").unwrap();
    let s = rt.snapshot();
    assert_eq!(s.text.to_string(), "zxy");
    assert!(s.status.contains("Recovered 1 unsaved edits"), "{}", s.status);
}

#[test]
fn bound_keys_list_whole_sequences() {
    let keys = runtime("", "emacs").bound_keys().unwrap();
    for k in ["C-f", "C-x C-s", "C-x u", "M-<", "C-?"] {
        assert!(keys.iter().any(|b| b == k), "{k} in {keys:?}");
    }
    assert!(runtime("", "modal").bound_keys().unwrap().is_empty());
}

#[test]
fn the_scenario_runs_through_the_runtime() {
    let s = techne_editor::scenario::expected("emacs");
    assert!(s.text.ends_with("\nend"), "{:?}", s.text);
    assert!(s.text.contains("日x本語"), "{:?}", s.text);
    assert!(!s.text.contains("again and again"), "the region was killed: {:?}", s.text);
}
