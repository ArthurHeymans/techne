//! Incremental search as Emacs's isearch: its own keymap, looked up first
//! while it goes on, whose keys add text from the buffer and the kill ring,
//! go through the strings searched for before and edit the string in the
//! minibuffer; other keys end the search and do what they do.

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

fn text(rt: &mut Runtime) -> String {
    rt.snapshot().pane().text.to_string()
}

/// Ending a search by a click drops the prefix it was waiting on.
#[test]
fn a_click_ends_the_search() {
    let mut r = rt("foo bar");
    keys(&mut r, "C-s b a r M-s");
    let pane = r.snapshot().pane().clone();
    r.handle(Input::Click { view: pane.view, revision: pane.revision, pos: 0, extend: false, at: Instant::now() });
    keys(&mut r, "x");
    assert_eq!(text(&mut r), "xfoo bar");
}
