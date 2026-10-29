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

fn head(rt: &mut Runtime) -> usize {
    rt.snapshot().pane().head()
}

fn echo(rt: &mut Runtime) -> String {
    rt.snapshot().echo
}

#[test]
fn adding_text_after_the_match() {
    let mut r = rt("foo bar baz\nfoo bar qux\n");
    keys(&mut r, "C-s f C-w");
    assert_eq!(echo(&mut r), "I-search: foo");
    keys(&mut r, "C-w");
    assert_eq!(echo(&mut r), "I-search: foo bar");
    keys(&mut r, "M-s C-e");
    assert_eq!(echo(&mut r), "I-search: foo bar baz");
    assert_eq!(head(&mut r), 11);
    assert_eq!(text(&mut r), "foo bar baz\nfoo bar qux\n", "the text is unchanged");
}

#[test]
fn yanking_and_popping_kills() {
    let mut r = rt("one two three onetwo");
    keys(&mut r, "M-d C-f M-d");
    // The kill ring is ("two" "one"): C-y adds "two", M-y replaces it.
    keys(&mut r, "C-a C-s C-y");
    assert_eq!(echo(&mut r), "I-search: two");
    keys(&mut r, "M-y");
    assert_eq!(echo(&mut r), "I-search: one");
    keys(&mut r, "M-y");
    assert_eq!(echo(&mut r), "I-search: two", "around the ring");
    // Not after a yank, M-y adds the last kill.
    keys(&mut r, "DEL DEL DEL e M-y");
    assert_eq!(echo(&mut r), "I-search: etwo");
}

#[test]
fn the_strings_searched_for_before() {
    let mut r = rt("alpha beta gamma");
    keys(&mut r, "C-s b e t a RET C-a C-s g a m m a RET C-a C-s");
    keys(&mut r, "M-p");
    assert_eq!(echo(&mut r), "I-search: gamma");
    keys(&mut r, "M-p");
    assert_eq!(echo(&mut r), "I-search: beta");
    assert_eq!(head(&mut r), 10);
    keys(&mut r, "M-p");
    assert_eq!(echo(&mut r), "I-search: beta  Beginning of history; no preceding item");
    keys(&mut r, "M-n M-n");
    assert_eq!(echo(&mut r), "I-search: ");
    // An empty search again searches for the last string.
    keys(&mut r, "C-s");
    assert_eq!(echo(&mut r), "I-search: gamma");
}

#[test]
fn editing_the_string_in_the_minibuffer() {
    let mut r = rt("abc abx abe");
    keys(&mut r, "C-s a b c M-e");
    assert_eq!(r.snapshot().minibuffer.expect("the minibuffer").input, "abc");
    keys(&mut r, "DEL e RET");
    assert!(r.snapshot().minibuffer.is_none());
    assert_eq!(echo(&mut r), "I-search: abe");
    assert_eq!(head(&mut r), 11);
    // C-g there goes back to the search as it was.
    keys(&mut r, "M-e C-a C-k C-g");
    assert_eq!(echo(&mut r), "I-search: abe  Quit");
    keys(&mut r, "C-g");
    assert_eq!(head(&mut r), 0, "C-g ends the search where it started");
}

#[test]
fn other_keys_end_the_search_and_run() {
    let mut r = rt("one two");
    keys(&mut r, "C-s t w C-f");
    assert_eq!((echo(&mut r).as_str(), head(&mut r)), ("", 7));
    // A prefix the search does not bind runs as a whole after it.
    keys(&mut r, "C-a C-s o C-x 2");
    assert_eq!(echo(&mut r), "");
    assert_eq!(r.eval("(length (session-panes (current-session)))").unwrap(), "2");
}

#[test]
fn the_keys_are_the_search_s_while_it_goes_on() {
    let mut r = rt("text");
    let explain = "(map car (explain-key (current-session) (list \"C-w\") 'chord))";
    assert_eq!(r.eval(explain).unwrap(), "(emacs)");
    keys(&mut r, "C-s");
    assert_eq!(r.eval(explain).unwrap(), "(isearch emacs)");
    keys(&mut r, "RET");
    assert_eq!(r.eval(explain).unwrap(), "(emacs)");
}

/// C-g goes back to the selection the search started from, the primary
/// range included, and searching starts from that range's head.
#[test]
fn searching_from_the_primary_range() {
    let mut r = rt("one one one");
    r.eval("(view-set-ranges! (session-view (current-session)) '((0 0) (4 4)) 1)").unwrap();
    keys(&mut r, "C-s o n e");
    assert_eq!(head(&mut r), 7);
    keys(&mut r, "C-g");
    assert_eq!(
        r.eval("(let ((v (session-view (current-session)))) (list (view-ranges v) (view-primary v)))").unwrap(),
        "(((0 0) (4 4)) 1)"
    );
}

/// Text added grows the match where it is, as Emacs's: going forward from
/// its start, going back from its end; C-w and M-s C-e add from its end
/// in either direction.
#[test]
fn growing_the_match_where_it_is() {
    let mut r = rt("foo bar foo bar");
    keys(&mut r, "C-s f o o C-s C-w");
    assert_eq!((echo(&mut r).as_str(), head(&mut r)), ("I-search: foo bar", 15));
    keys(&mut r, "RET M-> C-r f o o C-w");
    assert_eq!((echo(&mut r).as_str(), head(&mut r)), ("I-search: foo bar", 8));
    keys(&mut r, "C-r");
    assert_eq!((echo(&mut r).as_str(), head(&mut r)), ("I-search: foo bar", 0));
    keys(&mut r, "M-s C-e");
    assert_eq!((echo(&mut r).as_str(), head(&mut r)), ("I-search: foo bar foo bar", 0));
}

#[test]
fn adding_the_next_line_at_the_end_of_one() {
    let mut r = rt("foo\nbar\n");
    keys(&mut r, "C-s f o o M-s C-e");
    assert_eq!(r.eval("(cadr (sget (current-session) 'isearch))").unwrap(), "\"foo\\nbar\"");
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

/// The string edited with M-e is remembered when the search ends, not
/// when it is edited.
#[test]
fn edited_strings_are_remembered_when_the_search_ends() {
    let mut r = rt("foo bar");
    keys(&mut r, "C-s f o o RET C-s b a r M-e RET C-g");
    assert_eq!(r.eval("(minibuffer-history 'isearch)").unwrap(), "(\"foo\")");
}
