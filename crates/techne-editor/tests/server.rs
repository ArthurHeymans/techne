//! Opening files in a running editor for other programs, as emacsclient
//! does: shown in the session used last, and with `wait`, returning once
//! the file is done with. And evaluating code in it, as emacsclient
//! --eval does.

use std::{
    io::Write,
    path::Path,
    sync::{OnceLock, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use techne_editor::{
    host::{Event, Host},
    present::{Input, Output, Snapshot},
    runtime::Runtime,
    server::{self, Evaluation, OpenError, Request},
};

/// Journals of files opened from Lisp go under the state directory: one
/// for the tests, as they run side by side.
fn state() {
    static STATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    STATE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", dir.path()) };
        dir
    });
}

struct Editor {
    host: Host,
    events: mpsc::Receiver<Event>,
    _listener: server::Listener,
}

fn editor(socket: &Path, profile: &str) -> Editor {
    state();
    let (tx, events) = mpsc::channel();
    // Says when a program waiting is forgotten.
    let setup = |rt: &mut Runtime| {
        rt.eval("(define %forget editor-forget!)").unwrap();
        rt.eval("(define (editor-forget! id) (%forget id) (message! (current-session) \"Forgotten\"))").unwrap();
    };
    let host = Host::start(None, profile.into(), setup, move |e| drop(tx.send(e)));
    let listener = host.listen(socket).unwrap().expect("no other editor listens");
    Editor { host, events, _listener: listener }
}

impl Editor {
    fn keys(&mut self, keys: &str) {
        keys.split(' ').for_each(|k| self.host.send(Input::Key { key: k.into(), at: Instant::now() }));
    }

    fn snapshot(&self, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.checked_duration_since(Instant::now()).expect("a matching snapshot in time");
            match self.events.recv_timeout(left).expect("an event") {
                Event::Output(Output::Snapshot(s)) if done(&s) => return *s,
                Event::Failed(e) => panic!("{e}"),
                _ => {}
            }
        }
    }
}

fn open_waiting(socket: &Path, path: &Path) -> JoinHandle<Result<(), OpenError>> {
    let (socket, path) = (socket.to_owned(), path.to_owned());
    std::thread::spawn(move || server::open(&socket, &path, true))
}

/// Gives the thread time to return if it were going to.
fn still_waiting(t: &JoinHandle<Result<(), OpenError>>) -> bool {
    std::thread::sleep(Duration::from_millis(200));
    !t.is_finished()
}

#[test]
fn a_program_waits_until_the_file_is_done_with() {
    let dir = tempfile::tempdir().unwrap();
    let (socket, file) = (dir.path().join("socket"), dir.path().join("COMMIT_MSG"));
    std::fs::write(&file, "\n# a comment\n").unwrap();
    let mut e = editor(&socket, "emacs");
    assert!(e.host.listen(&socket).unwrap().is_none(), "one editor listens");
    let waiting = open_waiting(&socket, &file);
    let s = e.snapshot(|s| s.pane().status.starts_with(&*file.to_string_lossy()));
    assert!(s.echo.contains("C-x #"), "{}", s.echo);
    e.keys("f i x");
    assert!(still_waiting(&waiting));
    e.keys("C-x #");
    waiting.join().unwrap().unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "fix\n# a comment\n");
    // The buffer is gone; the session goes on.
    let s = e.snapshot(|s| !s.pane().status.starts_with(&*file.to_string_lossy()));
    assert!(s.pane().status.starts_with("*scratch*"), "{}", s.pane().status);
    e.host.close();
}

/// A program writing the file again before each request, as Git does
/// COMMIT_EDITMSG, gets what it wrote shown, not the text of the last.
#[test]
fn the_file_is_read_again_for_each_request() {
    let dir = tempfile::tempdir().unwrap();
    let (socket, file) = (dir.path().join("socket"), dir.path().join("COMMIT_EDITMSG"));
    let mut e = editor(&socket, "emacs");
    for msg in ["first\n", "second\n"] {
        std::fs::write(&file, msg).unwrap();
        let waiting = open_waiting(&socket, &file);
        e.snapshot(|s| s.pane().text == msg);
        e.keys("!");
        e.keys("C-x #");
        waiting.join().unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), format!("!{msg}"));
    }
    e.host.close();
}

/// A program that stopped waiting is not waited for: quitting quits.
#[test]
fn a_program_gone_is_no_longer_waited_for() {
    let dir = tempfile::tempdir().unwrap();
    let (socket, file) = (dir.path().join("socket"), dir.path().join("f.txt"));
    std::fs::write(&file, "text\n").unwrap();
    let mut e = editor(&socket, "emacs");
    let mut program = std::os::unix::net::UnixStream::connect(&socket).unwrap();
    let request = rmp_serde::to_vec_named(&server::Request::Open { path: file, wait: true }).unwrap();
    program.write_all(&(request.len() as u32).to_be_bytes()).unwrap();
    program.write_all(&request).unwrap();
    e.snapshot(|s| s.pane().text == "text\n");
    drop(program);
    e.snapshot(|s| s.echo.contains("Forgotten"));
    e.keys("C-x C-c");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let left = deadline.checked_duration_since(Instant::now()).expect("quitting in time");
        if let Event::Output(Output::Quit) = e.events.recv_timeout(left).expect("an event") {
            break;
        }
    }
}

#[test]
fn quitting_in_the_modal_profile_is_done_with_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let (socket, file) = (dir.path().join("socket"), dir.path().join("f.txt"));
    std::fs::write(&file, "one\n").unwrap();
    let mut e = editor(&socket, "modal");
    // :q is done with it without saving, and does not quit.
    let waiting = open_waiting(&socket, &file);
    let s = e.snapshot(|s| s.pane().text == "one\n");
    assert!(s.echo.contains(":wq"), "{}", s.echo);
    e.keys("x : q RET");
    waiting.join().unwrap().unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\n");
    // Consume the closing frame before reopening. Otherwise the snapshot
    // of the first edit ("ne") can satisfy the next wait before it opens.
    e.snapshot(|s| s.pane().status.starts_with("*scratch*"));
    // :wq saves it first.
    let waiting = open_waiting(&socket, &file);
    e.snapshot(|s| s.pane().text == "ne\n");
    e.keys("x : w q RET");
    waiting.join().unwrap().unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "e\n");
    e.keys("i");
    e.snapshot(|s| s.pane().status.contains("INSERT"));
    e.host.close();
}

#[test]
fn opening_without_waiting_and_killing_the_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let (socket, a, b) = (dir.path().join("socket"), dir.path().join("a.txt"), dir.path().join("b.txt"));
    std::fs::write(&a, "a\n").unwrap();
    std::fs::write(&b, "b\n").unwrap();
    let mut e = editor(&socket, "emacs");
    server::open(&socket, &a, false).unwrap();
    e.snapshot(|s| s.pane().text == "a\n");
    let waiting = open_waiting(&socket, &b);
    e.snapshot(|s| s.pane().text == "b\n");
    assert!(still_waiting(&waiting));
    e.keys("C-x k");
    waiting.join().unwrap().unwrap();
    // A file that cannot be opened is refused.
    let refused = server::open(&socket, dir.path(), false);
    assert!(matches!(refused, Err(OpenError::Failed(_))), "{refused:?}");
    e.host.close();
}

#[test]
fn with_no_editor_running() {
    let dir = tempfile::tempdir().unwrap();
    let opened = server::open(&dir.path().join("socket"), &dir.path().join("f"), true);
    assert!(matches!(opened, Err(OpenError::NotRunning)), "{opened:?}");
    let evaluated = server::eval(&dir.path().join("socket"), &request("1", false, None));
    assert!(matches!(evaluated, Err(OpenError::NotRunning)), "{evaluated:?}");
}

fn request(source: &str, detached: bool, timeout_ms: Option<u64>) -> Request {
    Request::Eval { source: source.into(), module: None, detached, principal: Some("a test".into()), timeout_ms }
}

fn request_in(module: &str, source: &str, detached: bool, timeout_ms: Option<u64>) -> Request {
    Request::Eval { source: source.into(), module: Some(module.into()), detached, principal: Some("a test".into()), timeout_ms }
}

fn eval(socket: &Path, source: &str) -> Evaluation {
    server::eval(socket, &request(source, false, None)).unwrap()
}

/// *Server log*'s text, written.
const LOG: &str = "(let ((d (buffer-document (buffer-named \"*Server log*\")))) (document-substring d 0 (document-length d)))";

/// The module of server.scm, which sees what the editor's own code does.
static SERVER: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| techne_vm::library::lisp_dir().join("editor/server.scm").display().to_string());

fn value_in(socket: &Path, module: &str, source: &str) -> String {
    let e = server::eval(socket, &request_in(module, source, false, None)).unwrap();
    e.result.unwrap_or_else(|error| panic!("{source}: {error}"))
}

fn value(socket: &Path, source: &str) -> String {
    let e = eval(socket, source);
    e.result.unwrap_or_else(|error| panic!("{source}: {error}"))
}

/// Code another program evaluates acts on the session used last, as
/// emacsclient --eval's does: what it shows is shown there. It gets the
/// value written and what the code wrote; *Server log* keeps both.
#[test]
fn a_program_evaluates_in_the_session_used_last() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let e = editor(&socket, "emacs");
    let got = eval(&socket, "(display \"out\") (message! (current-session) \"from a program\") (+ 1 2)");
    assert_eq!(got, Evaluation { result: Ok("3".into()), output: "out".into() });
    e.snapshot(|s| s.echo == "from a program");
    // No value: nothing written.
    assert_eq!(value(&socket, "(define x 1)"), "");
    assert_eq!(value(&socket, "(list x \"two\")"), "(1 \"two\")");
    // Run as an operation for whom it asked to run for.
    assert_eq!(value(&socket, "(current-principal)"), "\"a test\"");
    let log = value(&socket, LOG);
    assert!(log.contains(";; a test, in user:\\n(list x \\\"two\\\")\\n;; => (1 \\\"two\\\")"), "{log}");
    assert!(log.contains("(define x 1)\\n;; no value"), "{log}");
    e.host.close();
}

/// What the code raised comes back with where it was raised; the editor
/// goes on.
#[test]
fn an_error_says_where_it_was_raised() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let e = editor(&socket, "emacs");
    value(&socket, "(define (boom) (error \"Boom\" 42))");
    let got = eval(&socket, "(display \"before\") (boom)");
    let error = got.result.unwrap_err();
    assert!(error.starts_with("Boom: 42"), "{error}");
    assert!(error.contains("\n  in boom"), "{error}");
    assert_eq!(got.output, "before");
    // Unreadable source is an error of the same kind.
    assert!(eval(&socket, "(car").result.is_err());
    assert_eq!(value(&socket, "(+ 1 1)"), "2");
    e.host.close();
}

/// Detached, it runs in a session no frontend shows: the one used last
/// stays as it was, and stays the current one.
#[test]
fn detached_it_leaves_the_session_alone() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let e = editor(&socket, "emacs");
    value(&socket, "(define shown (current-session))");
    let attached = "(eq? (current-session) shown)";
    let detached = |source: &str| server::eval(&socket, &request(source, true, None)).unwrap().result.unwrap();
    assert_eq!(detached(attached), "#f");
    detached("(show-document! (current-session) (make-document \"hidden\")) (message! (current-session) \"hidden\")");
    // The next one finds what the last showed there.
    let text = "(let ((d (session-document (current-session)))) (document-substring d 0 (document-length d)))";
    assert_eq!(detached(text), "\"hidden\"");
    assert_eq!(value(&socket, attached), "#t");
    assert_eq!(value(&socket, text), "\"\"");
    // Killed, too.
    let killed = server::eval(&socket, &request("(let loop () (loop))", true, Some(300))).unwrap();
    assert!(killed.result.is_err_and(|e| e.contains("killed")));
    assert_eq!(value(&socket, attached), "#t");
    // A document both show is shown in a view of each: a caret moved in
    // one is not moved in the other, whichever showed it first.
    for first in [true, false] {
        value_in(&socket, &SERVER, "(define d (make-document \"abc\"))");
        let show = "(show-document! (current-session) d)";
        server::eval(&socket, &request_in(&SERVER, show, first, None)).unwrap().result.unwrap();
        server::eval(&socket, &request_in(&SERVER, show, !first, None)).unwrap().result.unwrap();
        let shared = value_in(&socket, &SERVER, "(view=? (session-view (current-session)) (session-view (detached-session)))");
        assert_eq!(shared, "#f", "shown detached first: {first}");
    }
    let s = e.snapshot(|_| true);
    assert_ne!(s.echo, "hidden");
    e.host.close();
}

/// An evaluation that does not end is killed at its deadline, or stopped
/// by C-g in a frontend; either way it is evaluated again when asked.
#[test]
fn an_evaluation_that_does_not_end_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let mut e = editor(&socket, "emacs");
    let forever = "(let loop () (loop))";
    // Killed, the operation it ran in is ended, and those under it.
    value_in(&socket, &SERVER, "(define outer #f) (define inner #f)");
    let probe = "(set! outer (current-operation))
                 (call-in-operation (start-operation! 'inner (current-session))
                   (lambda () (set! inner (current-operation)) (let loop () (loop))))";
    let killed = server::eval(&socket, &request_in(&SERVER, probe, false, Some(300))).unwrap();
    assert!(killed.result.as_ref().is_err_and(|e| e.starts_with("killed: it ran past 300 ms")), "{killed:?}");
    assert_eq!(value_in(&socket, &SERVER, "(list (operation-pending? outer) (operation-pending? inner))"), "(#f #f)");
    let log = value(&socket, LOG);
    assert!(log.contains("(let loop () (loop))))\\n;; killed: it ran past 300 ms"), "{log}");
    // C-g once it runs: it says so in a file first.
    let started = dir.path().join("started");
    let s = socket.clone();
    let source = format!("(close-port (open-output-file {:?})) {forever}", started.display().to_string());
    let waiting = std::thread::spawn(move || server::eval(&s, &request(&source, false, Some(60_000))));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !started.exists() {
        assert!(Instant::now() < deadline, "the evaluation did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    e.keys("C-g");
    let stopped = waiting.join().unwrap().unwrap();
    assert!(stopped.result.as_ref().is_err_and(|e| e.contains("interrupted")), "{stopped:?}");
    assert_eq!(value(&socket, "(+ 2 2)"), "4");
    e.host.close();
}

/// Commands an evaluation runs are run for whom it runs for, not the user.
#[test]
fn what_an_evaluation_runs_is_its_principals() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let e = editor(&socket, "emacs");
    // What a command a key runs starts in: `run-command`'s.
    let nested = "(record-principal (%start-operation! 'probe (current-session) #f #f #f #f))";
    assert_eq!(value_in(&socket, &SERVER, nested), "\"a test\"");
    e.host.close();
}

/// With the option server-eval off, programs may not evaluate.
#[test]
fn evaluation_can_be_turned_off() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let e = editor(&socket, "emacs");
    value(&socket, "(set-option! 'server-eval #f)");
    let refused = server::eval(&socket, &request("1", false, None));
    assert!(matches!(&refused, Err(OpenError::Failed(e)) if e.contains("server-eval is off")), "{refused:?}");
    e.host.close();
}

/// A socket left behind by an editor that is gone is taken over; once an
/// editor stops listening, another may.
#[test]
fn the_socket_goes_to_the_next_editor() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    std::fs::write(&socket, "").unwrap();
    let a = editor(&socket, "emacs");
    let (tx, _events) = mpsc::channel();
    let b = Host::start(None, "emacs".into(), |_| {}, move |e| drop(tx.send(e)));
    assert!(b.listen(&socket).unwrap().is_none());
    drop(a._listener);
    assert!(b.listen(&socket).unwrap().is_some());
    a.host.close();
    b.close();
}
