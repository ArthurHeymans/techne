//! Opening files in a running editor for other programs, as emacsclient
//! does: shown in the session used last, and with `wait`, returning once
//! the file is done with.

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
    server::{self, OpenError},
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
