//! Frontends in other processes, attached over the editor's socket: each
//! with a session of its own, sent what one in the runtime's process is.

use std::{
    path::Path,
    sync::{OnceLock, mpsc},
    time::{Duration, Instant},
};

use techne_editor::{
    attach::{Link, Remote},
    host::{Event, Host},
    present::{Input, Output, Snapshot},
    server::{self, OpenError, Request},
};

/// Journals of files opened go under the state directory: one for the
/// tests, as they run side by side.
fn state() {
    static STATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    STATE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", dir.path()) };
        dir
    });
}

/// A frontend, in the runtime's process or not, and what it gets.
struct Frontend {
    link: Link,
    events: mpsc::Receiver<Event>,
}

impl Frontend {
    fn keys(&mut self, keys: &str) {
        keys.split(' ').for_each(|k| self.link.send(Input::Key { key: k.into(), at: Instant::now() }));
    }

    /// The first snapshot that is `done`.
    fn snapshot(&self, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
        self.event(|e| match e {
            Event::Output(Output::Snapshot(s)) if done(&s) => Some(*s),
            Event::Failed(e) => panic!("{e}"),
            _ => None,
        })
    }

    fn event<T>(&self, mut take: impl FnMut(Event) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.checked_duration_since(Instant::now()).expect("the event in time");
            if let Some(t) = take(self.events.recv_timeout(left).expect("an event")) {
                return t;
            }
        }
    }
}

/// An editor listening on `socket`, and the frontend it started with.
fn editor(socket: &Path) -> (Frontend, server::Listener) {
    state();
    let (tx, events) = mpsc::channel();
    let host = Host::start(None, "emacs".into(), |_| {}, move |e| drop(tx.send(e)));
    let listener = host.listen(socket).unwrap().expect("no other editor listens");
    (Frontend { link: Link::Local(host), events }, listener)
}

fn remote(socket: &Path, file: Option<&Path>) -> Frontend {
    let (tx, events) = mpsc::channel();
    let remote = Remote::attach(socket, "emacs", file, move |e| drop(tx.send(e))).unwrap();
    Frontend { link: Link::Remote(remote), events }
}

/// How many frontends the editor has attached.
fn attached(socket: &Path) -> String {
    eval(socket, "(length (attached-sessions))", true).unwrap()
}

/// Evaluate `source` in the editor, in the module of session.scm: in the
/// session used last, or `detached`.
fn eval(socket: &Path, source: &str, detached: bool) -> Result<String, String> {
    let module = Some(techne_vm::library::lisp_dir().join("editor/session.scm").display().to_string());
    let request = Request::Eval { source: source.into(), module, detached, principal: None, timeout_ms: None };
    server::eval(socket, &request).map_err(|e| format!("{e:?}"))?.result
}

/// Typing in either is shown in both, each in a view of its own: the text
/// sent as what changed, characters of several bytes whole.
#[test]
fn a_frontend_attached_from_another_process_types_and_sees() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let (mut local, _listener) = editor(&socket);
    local.snapshot(|_| true);
    let mut other = remote(&socket, None);
    other.snapshot(|s| s.pane().status.starts_with("*scratch*"));
    assert_eq!(attached(&socket), "2");
    other.keys("h é l");
    let s = other.snapshot(|s| s.pane().text == "hél");
    // Its keys answered, as their snapshot says.
    assert!(!s.answers.is_empty());
    local.snapshot(|s| s.pane().text == "hél");
    local.keys("C-a 🙂 C-e ! RET");
    other.snapshot(|s| s.pane().text == "🙂hél!\n");
    // Each has a caret of its own.
    other.keys("C-a");
    let theirs = other.snapshot(|s| s.pane().head() == 0);
    let ours = local.snapshot(|s| s.pane().text == "🙂hél!\n");
    assert_ne!(theirs.pane().view, ours.pane().view);
    assert_eq!(ours.pane().head(), "🙂hél!\n".len());
    // A text of many chunks, edited in its middle: the other frontend
    // (the one used last) puts it there.
    let big = "a line of text, é and all\n".repeat(20_000);
    let edit = format!("(let ((v (session-view (current-session)))) (view-edit! v (list (list 0 0 {big:?})) \"new\"))");
    eval(&socket, &edit, false).unwrap();
    let expected = format!("{big}🙂hél!\n");
    other.snapshot(|s| s.pane().text == expected);
    local.snapshot(|s| s.pane().text == expected);
    let middle = big.len() / 2 - big.len() / 2 % 27;
    eval(&socket, &format!("(view-edit! (session-view (current-session)) (list (list {middle} {} \"🙃\")) \"new\")", middle + 1), false)
        .unwrap();
    let expected = format!("{}🙃{}🙂hél!\n", &big[..middle], &big[middle + 1..]);
    other.snapshot(|s| s.pane().text == expected);
    local.snapshot(|s| s.pane().text == expected);
    // Gone, it is detached.
    let Link::Remote(r) = other.link else { unreachable!() };
    r.close();
    let deadline = Instant::now() + Duration::from_secs(20);
    while attached(&socket) != "1" {
        assert!(Instant::now() < deadline, "the frontend was not detached");
        std::thread::sleep(Duration::from_millis(20));
    }
    let Link::Local(h) = local.link else { unreachable!() };
    h.close();
}

/// Two panes of one view: what changed in it is sent once, and taken once.
#[test]
fn two_panes_of_one_view() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let (local, _listener) = editor(&socket);
    let mut other = remote(&socket, None);
    other.snapshot(|_| true);
    eval(&socket, "(let ((v (session-view (current-session)))) (set-session-panes! (current-session) (list v v) 0))", false).unwrap();
    other.snapshot(|s| s.panes.len() == 2);
    other.keys("z q");
    let s = other.snapshot(|s| s.panes.iter().all(|p| p.text == "zq"));
    assert_eq!(s.panes[0].view, s.panes[1].view);
    let Link::Local(h) = local.link else { unreachable!() };
    h.close();
}

/// When the runtime crashes, one attached from another process is
/// attached to the one started instead, told so, and nothing it sent the
/// old one is waited for: the runtime is not busy for it.
#[test]
fn attached_across_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let (mut local, _listener) = editor(&socket);
    local.snapshot(|_| true);
    let mut other = remote(&socket, None);
    other.snapshot(|_| true);
    other.keys("a");
    other.snapshot(|s| s.pane().text == "a");
    // The local frontend restarts it, as frontends do.
    let _ = eval(&socket, "(%crash-runtime)", false);
    local.event(|e| matches!(e, Event::Ended).then_some(()));
    assert!(local.link.restart());
    other.event(|e| matches!(e, Event::Ended).then_some(()));
    assert!(other.link.restart());
    other.keys("b");
    other.snapshot(|s| s.pane().text.to_string().ends_with('b') && !s.answers.is_empty());
    assert_eq!(other.link.busy_from(), None);
    let Link::Local(h) = local.link else { unreachable!() };
    h.close();
}

/// One attached with a file shows it; quitting its session detaches it,
/// and the others go on.
#[test]
fn attached_with_a_file_and_quitting() {
    let dir = tempfile::tempdir().unwrap();
    let (socket, file) = (dir.path().join("socket"), dir.path().join("f.txt"));
    std::fs::write(&file, "in the file\n").unwrap();
    let (local, _listener) = editor(&socket);
    let mut other = remote(&socket, Some(&file));
    other.snapshot(|s| s.pane().text == "in the file\n");
    other.keys("C-x C-c");
    other.event(|e| matches!(e, Event::Output(Output::Quit)).then_some(()));
    assert_eq!(attached(&socket), "1");
    let Link::Local(h) = local.link else { unreachable!() };
    h.close();
}

/// The editor's own frontend closing leaves the runtime to those attached
/// from other processes; it ends after the last.
#[test]
fn the_runtime_outlives_the_frontend_it_started_with() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let (local, listener) = editor(&socket);
    let mut other = remote(&socket, None);
    other.snapshot(|_| true);
    let Link::Local(h) = local.link else { unreachable!() };
    let closing = std::thread::spawn(move || h.close_outliving());
    other.keys("s t i l l");
    other.snapshot(|s| s.pane().text == "still");
    assert!(!closing.is_finished());
    other.keys("C-x C-c");
    other.event(|e| matches!(e, Event::Output(Output::Quit)).then_some(()));
    let Link::Remote(r) = other.link else { unreachable!() };
    r.close();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !closing.is_finished() {
        assert!(Instant::now() < deadline, "the runtime did not end");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(listener);
    // Gone: no editor to attach to.
    let (tx, _events) = mpsc::channel();
    assert!(matches!(Remote::attach(&socket, "emacs", None, move |e| drop(tx.send(e))), Err(OpenError::NotRunning)));
}

/// A frontend whose editor goes (its process ended, and the connection
/// with it) is told so.
#[test]
fn a_frontend_is_told_when_the_editor_goes() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    // It takes the request, then goes.
    let editor = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        std::io::Read::read_exact(&mut stream, &mut [0; 4]).unwrap();
    });
    let other = remote(&socket, None);
    editor.join().unwrap();
    let why = other.event(|e| match e {
        Event::Failed(why) => Some(why),
        _ => None,
    });
    assert_eq!(why, "the editor went away");
}
