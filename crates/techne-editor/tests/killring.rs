//! The kill ring and prefix arguments as in Emacs: C-u and digits, C-y
//! with an argument, M-y (as consult-yank-pop), the system clipboard.

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

fn head(rt: &mut Runtime) -> usize {
    rt.snapshot().pane().head()
}

#[test]
fn prefix_arguments() {
    let mut r = rt("0123456789abcdefghijklmnopqrstuvwxyz");
    keys(&mut r, "C-u C-f");
    assert_eq!(head(&mut r), 4);
    keys(&mut r, "C-u C-u C-f");
    assert_eq!(head(&mut r), 20);
    keys(&mut r, "C-u 1 2 C-b");
    assert_eq!(head(&mut r), 8);
    keys(&mut r, "M-3 C-f C-- C-f");
    assert_eq!(head(&mut r), 10);
    // The echo area shows the argument being typed.
    keys(&mut r, "C-u 3");
    assert_eq!(r.snapshot().echo, "C-u 3-");
    keys(&mut r, "x");
    assert!(text(&mut r).starts_with("0123456789xxxabc"));
    // C-g drops a pending argument.
    keys(&mut r, "C-u C-g C-f");
    assert_eq!(head(&mut r), 14);
}

#[test]
fn undo_keys() {
    let mut r = rt("");
    keys(&mut r, "a b C-_");
    assert_eq!(text(&mut r), "");
    keys(&mut r, "M-_");
    assert_eq!(text(&mut r), "ab");
}

#[test]
fn yanking_with_an_argument() {
    let mut r = rt("one two three ");
    keys(&mut r, "M-d C-f M-d C-f M-d");
    assert_eq!(text(&mut r), "   ");
    // C-u 2 C-y: the second most recent kill; C-u C-y leaves point before.
    keys(&mut r, "C-u 2 C-y");
    assert_eq!((text(&mut r).as_str(), head(&mut r)), ("  two ", 5));
    keys(&mut r, "C-a C-u C-y");
    assert_eq!((text(&mut r).as_str(), head(&mut r)), ("three  two ", 0));
}

#[test]
fn yank_pop() {
    let mut r = rt("one two three ");
    keys(&mut r, "M-d C-f M-d C-f M-d C-e C-y");
    assert_eq!(text(&mut r), "   three");
    // M-y after C-y: the yanked text is replaced, previewing each kill.
    keys(&mut r, "M-y");
    assert_eq!(text(&mut r), "   three", "the newest kill first");
    keys(&mut r, "C-n");
    assert_eq!(text(&mut r), "   two");
    keys(&mut r, "C-n RET");
    assert_eq!(text(&mut r), "   one");
    // Its undo unit is the yank's.
    keys(&mut r, "C-/");
    assert_eq!(text(&mut r), "   ");
    // Without a yank before it, it inserts at point; C-g takes it back.
    keys(&mut r, "C-a M-y C-n");
    assert_eq!(text(&mut r), "two   ");
    keys(&mut r, "C-g");
    assert_eq!((text(&mut r).as_str(), head(&mut r)), ("   ", 0));
}

#[test]
fn the_kill_ring_is_bounded() {
    let mut r = rt(&"w ".repeat(130));
    for _ in 0..130 {
        keys(&mut r, "M-d C-f");
    }
    assert_eq!(r.eval("(length (kill-ring (current-session)))").unwrap(), "120");
}

#[test]
fn the_system_clipboard() {
    let mut r = rt("hello world");
    keys(&mut r, "M-d");
    assert_eq!(r.clipboard_out().as_deref(), Some("hello"), "a kill goes to the clipboard");
    assert_eq!(r.clipboard_out(), None, "once");
    // What the clipboard holds, as the frontend sees it on focus.
    r.handle(Input::Clipboard { text: "hello".into() });
    r.handle(Input::Clipboard { text: "from elsewhere".into() });
    r.handle(Input::Clipboard { text: "from elsewhere".into() });
    keys(&mut r, "C-y");
    assert_eq!(text(&mut r), "from elsewhere world");
    assert_eq!(r.eval("(map car (kill-ring (current-session)))").unwrap(), "(\"from elsewhere\" \"hello\")");
}
