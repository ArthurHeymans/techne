//! Several frontends attached to one runtime (EDITOR.md, section 6): each
//! has a session of its own, and they share the documents and buffers.

use std::time::Instant;

use techne_editor::{
    present::{CursorShape, Input, Snapshot},
    runtime::{Client, Runtime},
};
use techne_text::Document;

const OTHER: Client = Client(1);

fn keys(rt: &mut Runtime, client: Client, keys: &str) {
    rt.select(client).unwrap();
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn type_text(rt: &mut Runtime, client: Client, text: &str) {
    rt.select(client).unwrap();
    for c in text.chars() {
        let key = if c == ' ' { "SPC".to_string() } else { c.to_string() };
        rt.handle(Input::Key { key, at: Instant::now() });
    }
}

fn snapshot(rt: &mut Runtime, client: Client) -> Snapshot {
    rt.select(client).unwrap();
    rt.snapshot()
}

/// A runtime over "hello" with a second frontend, in the modal profile,
/// attached without a file.
fn two() -> Runtime {
    let mut rt = Runtime::with_document(Document::new("hello"), "emacs").unwrap();
    rt.attach(OTHER, None, "modal").unwrap();
    rt
}

#[test]
fn frontends_share_documents_but_not_views() {
    let mut rt = two();
    // The second shows what the first does, in a view of its own.
    let (a, b) = (snapshot(&mut rt, Client::FIRST), snapshot(&mut rt, OTHER));
    assert_eq!(b.pane().text.to_string(), "hello");
    assert_ne!(a.pane().view, b.pane().view);
    keys(&mut rt, Client::FIRST, "C-e");
    type_text(&mut rt, Client::FIRST, " world");
    keys(&mut rt, Client::FIRST, "C-x 2 C-x");
    let (a, b) = (snapshot(&mut rt, Client::FIRST), snapshot(&mut rt, OTHER));
    assert_eq!(b.pane().text.to_string(), "hello world");
    assert_eq!((a.pane().head(), b.pane().head()), (11, 0));
    // Panes, keys being typed and the profile are each frontend's own.
    assert_eq!((a.panes.len(), b.panes.len()), (2, 1));
    assert!(a.echo.contains("C-x-"), "{}", a.echo);
    assert!(!b.echo.contains("C-x-"), "{}", b.echo);
    assert_eq!((a.pane().cursor, b.pane().cursor), (CursorShape::Bar, CursorShape::Block));
}

#[test]
fn a_buffer_shown_elsewhere_gets_a_view_of_its_own() {
    let mut rt = two();
    let switch = |rt: &mut Runtime, client: Client, keys_: &str, name: &str| {
        keys(rt, client, keys_);
        type_text(rt, client, name);
        keys(rt, client, "RET");
    };
    switch(&mut rt, OTHER, "SPC b b", "Messages");
    // The first leaves *scratch* and comes back: its view is the one
    // *scratch* was last shown in.
    switch(&mut rt, Client::FIRST, "C-x b", "Messages");
    switch(&mut rt, Client::FIRST, "C-x b", "scratch");
    switch(&mut rt, OTHER, "SPC b b", "scratch");
    let (a, b) = (snapshot(&mut rt, Client::FIRST), snapshot(&mut rt, OTHER));
    assert_eq!(b.pane().text.to_string(), "hello");
    assert_ne!(a.pane().view, b.pane().view);
}

#[test]
fn killing_a_buffer_takes_it_from_every_frontend() {
    let mut rt = two();
    keys(&mut rt, Client::FIRST, "C-x k");
    let b = snapshot(&mut rt, OTHER);
    assert!(b.pane().status.starts_with("*Messages*"), "{}", b.pane().status);
}

#[test]
fn the_kill_ring_is_shared() {
    let mut rt = two();
    keys(&mut rt, Client::FIRST, "M-d");
    keys(&mut rt, OTHER, "p");
    assert_eq!(snapshot(&mut rt, OTHER).pane().text.to_string(), "hello");
    assert_eq!(snapshot(&mut rt, Client::FIRST).pane().text.to_string(), "hello");
    keys(&mut rt, OTHER, "d d");
    keys(&mut rt, Client::FIRST, "C-y");
    assert_eq!(snapshot(&mut rt, Client::FIRST).pane().text.to_string(), "hello\n");
}

#[test]
fn evaluated_code_acts_on_the_session_whose_input_it_is() {
    let mut rt = two();
    keys(&mut rt, OTHER, "i");
    type_text(&mut rt, OTHER, "(sset! (current-session) 'mark 1)");
    keys(&mut rt, OTHER, "ESC SPC c e");
    rt.select(OTHER).unwrap();
    assert_eq!(rt.eval("(sget (current-session) 'mark)").unwrap(), "1");
    rt.select(Client::FIRST).unwrap();
    assert_eq!(rt.eval("(sget (current-session) 'mark)").unwrap(), "#f");
}

#[test]
fn a_frontend_detaching_leaves_the_others() {
    let mut rt = two();
    rt.select(Client::FIRST).unwrap();
    rt.eval("(sset! (current-session) 'mark 1)").unwrap();
    rt.select(OTHER).unwrap();
    rt.detach(OTHER);
    assert_eq!(rt.clients().collect::<Vec<_>>(), [Client::FIRST]);
    assert!(rt.select(OTHER).is_err());
    // The one left is current.
    assert_eq!(rt.eval("(sget (current-session) 'mark)").unwrap(), "1");
    keys(&mut rt, Client::FIRST, "x");
    assert_eq!(snapshot(&mut rt, Client::FIRST).pane().text.to_string(), "xhello");
}

/// Undoing a preview brings back the view the pane had, unless another
/// frontend shows it by then: then the pane gets a view of its own.
#[test]
fn an_aborted_preview_does_not_take_another_frontends_view() {
    let mut rt = two();
    keys(&mut rt, OTHER, "SPC b b");
    type_text(&mut rt, OTHER, "Messages");
    keys(&mut rt, OTHER, "RET");
    keys(&mut rt, Client::FIRST, "C-x b");
    type_text(&mut rt, Client::FIRST, "Messages");
    // The first previews *Messages*, leaving *scratch*'s view, which the
    // second takes.
    keys(&mut rt, OTHER, "SPC b b");
    type_text(&mut rt, OTHER, "scratch");
    keys(&mut rt, OTHER, "RET");
    keys(&mut rt, Client::FIRST, "C-g");
    let (a, b) = (snapshot(&mut rt, Client::FIRST), snapshot(&mut rt, OTHER));
    assert_eq!((a.pane().text.to_string(), b.pane().text.to_string()), ("hello".into(), "hello".into()));
    assert_ne!(a.pane().view, b.pane().view);
}

/// Undoing a preview does not bring back a buffer another frontend killed
/// meanwhile.
#[test]
fn an_aborted_preview_does_not_bring_back_a_killed_buffer() {
    let mut rt = Runtime::with_document(Document::new("hello"), "emacs").unwrap();
    rt.attach(OTHER, None, "emacs").unwrap();
    keys(&mut rt, Client::FIRST, "C-x b");
    type_text(&mut rt, Client::FIRST, "Messages");
    keys(&mut rt, OTHER, "C-x k");
    // Shown as *Messages* is: read-only.
    keys(&mut rt, Client::FIRST, "C-g x");
    let a = snapshot(&mut rt, Client::FIRST);
    assert!(a.pane().status.starts_with("*Messages*"), "{}", a.pane().status);
    assert!(!a.pane().text.to_string().contains('x'), "{}", a.pane().text);
}

/// Kills join only the frontend's own last kill.
#[test]
fn kills_join_only_their_own() {
    let mut rt = Runtime::with_document(Document::new("a b c d"), "emacs").unwrap();
    rt.attach(OTHER, None, "emacs").unwrap();
    keys(&mut rt, Client::FIRST, "M-d");
    keys(&mut rt, OTHER, "M-f M-d");
    keys(&mut rt, Client::FIRST, "M-d");
    rt.select(Client::FIRST).unwrap();
    // The first killed " b" after the second killed " c": not joined.
    assert_eq!(rt.eval("(map car (kill-ring (current-session)))").unwrap(), "(\" b\" \" c\" \"a\")");
}
