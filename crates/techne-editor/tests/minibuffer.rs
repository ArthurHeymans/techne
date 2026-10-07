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
    assert_eq!(shown(&s), ["forward-char"]);
    let row = &s.minibuffer.as_ref().unwrap().rows[0];
    assert!(row.columns[0].iter().any(|r| r.text == "forw" && r.face.as_deref() == Some("match")), "{row:?}");
    assert!(row.text(1).starts_with("C-f, <right>  Move forward"), "{row:?}");
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
