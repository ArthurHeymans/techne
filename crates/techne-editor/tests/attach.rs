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

#[test]
fn a_version_two_frontend_is_refused_before_exchanging_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let (_local, _listener) = editor(&socket);
    let why = server::request(&socket, &Request::Attach { profile: "emacs".into(), file: None, version: 2 }).unwrap_err();
    assert!(why.contains("attachment protocol, the frontend 2: run the same techne"), "{why}");
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
    let Link::Remote(remote) = &mut other.link else { unreachable!() };
    remote.send_in_generation(Input::Text { text: "stale".into(), at: Instant::now() }, 0);
    remote.send_in_generation(Input::Key { key: "b".into(), at: Instant::now() }, 1);
    let new = other.snapshot(|s| s.pane().text.to_string().ends_with('b') && !s.answers.is_empty());
    assert!(!new.pane().text.to_string().contains("stale"));
    assert_eq!(other.link.busy_from(), None);
    let Link::Local(h) = local.link else { unreachable!() };
    h.close();
}

/// Keys sent while the runtime is busy past its backlog are refused; the
/// frontend does not wait for them once those before are answered.
#[test]
fn keys_refused_are_not_waited_for() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let (local, _listener) = editor(&socket);
    let mut other = remote(&socket, None);
    other.snapshot(|_| true);
    // Busy until killed at its deadline; it says it started in a file.
    let started = dir.path().join("started");
    let busy = format!("(close-port (open-output-file {:?})) (let loop () (loop))", started.display().to_string());
    let s = socket.clone();
    let evaluation = std::thread::spawn(move || {
        let request = Request::Eval { source: busy, module: None, detached: true, principal: None, timeout_ms: Some(2000) };
        server::eval(&s, &request)
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    while !started.exists() {
        assert!(Instant::now() < deadline, "the evaluation did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    for _ in 0..techne_editor::host::BACKLOG_INPUTS + 50 {
        other.link.send(Input::Key { key: "x".into(), at: Instant::now() });
    }
    evaluation.join().unwrap().unwrap();
    other.event(|e| match e {
        Event::Output(Output::Snapshot(_)) => other.link.busy_from().is_none().then_some(()),
        Event::Failed(e) => panic!("{e}"),
        _ => None,
    });
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

/// A daemon: a runtime no frontend started, which runs on as frontends
/// attach and detach, is started again when it crashes with none
/// attached, with the unsaved edits of files from their journals, and
/// detaches those attached when stopped, keeping what they keep when they
/// close (minibuffer histories).
#[test]
fn a_daemon_runs_with_no_frontend() {
    state();
    let dir = tempfile::tempdir().unwrap();
    let (socket, file, histories) = (dir.path().join("socket"), dir.path().join("f.txt"), dir.path().join("histories"));
    std::fs::write(&file, "file\n").unwrap();
    let kept = histories.clone();
    let daemon = techne_editor::host::Daemon::start(move |rt| rt.keep_histories(Some(&kept)).unwrap());
    let listener = daemon.listen(&socket).unwrap().expect("no other editor listens");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let theirs = stop.clone();
    let daemon = std::thread::spawn(move || {
        let ran = daemon.run(|| theirs.load(std::sync::atomic::Ordering::Relaxed));
        daemon.stop();
        ran
    });
    assert_eq!(attached(&socket), "0");
    let mut first = remote(&socket, Some(&file));
    first.snapshot(|s| s.pane().text == "file\n");
    first.keys("k e p t SPC");
    first.snapshot(|s| s.pane().text == "kept file\n");
    let Link::Remote(r) = first.link else { unreachable!() };
    r.close();
    let deadline = Instant::now() + Duration::from_secs(20);
    while attached(&socket) != "0" {
        assert!(Instant::now() < deadline, "the frontend was not detached");
        std::thread::sleep(Duration::from_millis(20));
    }
    // The next one finds what the last left.
    let second = remote(&socket, None);
    second.snapshot(|s| s.pane().text == "kept file\n");
    let Link::Remote(r) = second.link else { unreachable!() };
    r.close();
    while attached(&socket) != "0" {
        assert!(Instant::now() < deadline, "the frontend was not detached");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Crashed with none attached, it is started again, with the edits.
    let _ = eval(&socket, "(%crash-runtime)", true);
    let deadline = Instant::now() + Duration::from_secs(60);
    while eval(&socket, "(+ 1 1)", true).ok().as_deref() != Some("2") {
        assert!(Instant::now() < deadline, "the runtime was not started again");
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut third = remote(&socket, Some(&file));
    third.snapshot(|s| s.pane().text == "kept file\n");
    third.keys("M-: ( + SPC 1 SPC 2 ) RET");
    third.snapshot(|s| s.echo == "3");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    daemon.join().unwrap().unwrap();
    let why = third.event(|e| match e {
        Event::Failed(why) => Some(why),
        _ => None,
    });
    assert_eq!(why, "the editor was stopped");
    // Keep the listener alive to force the same ordering as a request
    // already accepted by the CLI's listener when it begins shutdown.
    let late = remote(&socket, None);
    let why = late.event(|e| match e {
        Event::Failed(why) => Some(why),
        Event::Output(Output::Snapshot(_)) => panic!("an attach restarted a stopped daemon"),
        _ => None,
    });
    assert_eq!(why, "the editor was stopped");
    let kept = std::fs::read_to_string(&histories).unwrap_or_default();
    assert!(kept.contains("(+ 1 2)"), "{kept:?}");
    drop(listener);
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
