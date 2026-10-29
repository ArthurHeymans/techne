//! The modal profile's search (/) and command (:) lines, read in the
//! minibuffer with Vim's command-line keys over its own.

use std::time::Instant;

use techne_editor::{present::Input, runtime::Runtime};
use techne_text::Document;

fn rt(text: &str) -> Runtime {
    Runtime::with_document(Document::new(text), "modal").unwrap()
}

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn input(rt: &mut Runtime) -> Option<String> {
    rt.snapshot().minibuffer.map(|m| m.input.to_string())
}

fn head(rt: &mut Runtime) -> usize {
    rt.snapshot().pane().head()
}

fn highlights(rt: &mut Runtime) -> Vec<(usize, usize)> {
    rt.snapshot().pane().layers.iter().map(|h| (h.from, h.to)).collect()
}

#[test]
fn searching_highlights_as_you_type() {
    let mut r = rt("one two one two");
    keys(&mut r, "/ t w");
    assert_eq!(input(&mut r).as_deref(), Some("tw"));
    assert_eq!(highlights(&mut r), [(4, 6), (12, 14)]);
    assert_eq!(head(&mut r), 0, "the cursor waits for RET");
    keys(&mut r, "o RET");
    assert_eq!(input(&mut r), None);
    assert_eq!(head(&mut r), 4);
    assert!(highlights(&mut r).is_empty());
    keys(&mut r, "n");
    assert_eq!(head(&mut r), 12);
}

#[test]
fn the_command_line_keys() {
    let mut r = rt("one two");
    keys(&mut r, "y w / C-r \"");
    assert_eq!(input(&mut r).as_deref(), Some("one "), "C-r \" puts the register");
    keys(&mut r, "a b SPC c d C-w");
    assert_eq!(input(&mut r).as_deref(), Some("one ab "));
    keys(&mut r, "C-r \"");
    assert_eq!(input(&mut r).as_deref(), Some("one ab one "), "C-w leaves the register alone");
    // DEL with nothing typed leaves the line, as ESC does.
    keys(&mut r, "C-a C-k DEL");
    assert_eq!(input(&mut r), None);
    keys(&mut r, "/ x ESC");
    assert_eq!(input(&mut r), None);
    assert_eq!(head(&mut r), 0);
}

#[test]
fn searches_are_remembered() {
    let mut r = rt("alpha beta gamma");
    keys(&mut r, "/ g a m m a RET 0 / M-p");
    assert_eq!(input(&mut r).as_deref(), Some("gamma"));
    keys(&mut r, "RET");
    assert_eq!(head(&mut r), 11);
}

#[test]
fn commands_are_completed() {
    let mut r = rt("text");
    keys(&mut r, ": s p l i t - w i n d o w - b");
    let rows: Vec<String> = r.snapshot().minibuffer.unwrap().rows.iter().map(|row| row.text(0)).collect();
    assert_eq!(rows, ["split-window-below"]);
    keys(&mut r, "RET");
    assert_eq!(r.eval("(length (session-panes (current-session)))").unwrap(), "2");
    keys(&mut r, ": n o - s u c h RET");
    assert_eq!(r.snapshot().echo, "Not an editor command: no-such");
    keys(&mut r, ": RET");
    assert_eq!(r.snapshot().echo, "", "nothing typed runs nothing");
}

#[test]
fn an_empty_search_repeats_the_last() {
    let mut r = rt("one two one");
    keys(&mut r, "/ RET");
    assert_eq!(r.snapshot().echo, "no previous search");
    keys(&mut r, "/ o n e RET 0 / RET");
    assert_eq!(head(&mut r), 8);
}

#[test]
fn the_command_line_runs_what_was_meant() {
    let mut r = rt("text");
    // Spaces are nothing typed: not the first command, w.
    keys(&mut r, ": SPC RET");
    assert_eq!(r.snapshot().echo, "", "nothing ran");
    // What ran is remembered, not what was typed.
    keys(&mut r, ": s p l i t - w i n d o w C-n RET");
    assert_eq!(r.eval("(car (minibuffer-history 'modal-ex))").unwrap(), "\"split-window-right\"");
    keys(&mut r, ": n o - s u c h RET");
    assert_eq!(r.eval("(minibuffer-history 'modal-ex)").unwrap(), "(\"split-window-right\")", "no command is not remembered");
    // A visual selection ends.
    keys(&mut r, "v l : C-g");
    assert_eq!(r.eval("(sget (current-session) 'mode)").unwrap(), "normal");
}
