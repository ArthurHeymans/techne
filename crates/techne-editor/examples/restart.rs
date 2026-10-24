//! Starting the editor, and starting it again after a crash, in wall
//! time (REQUIREMENTS.md, "Responsiveness budgets": runtime startup under
//! 100 ms warm; a restart to a usable session with unsaved content
//! restored, target set by a first measurement, then tracked).
//!
//! Startup: from starting a runtime on a file to the first snapshot a
//! frontend gets, the first time in the process (cold, as far as one
//! process goes: files and libraries may be cached) and then again.
//! Restart: a working session on Techne's own Lisp, every file of
//! `lisp/editor` open with an unsaved edit, the first one edited at
//! length, two panes; the runtime crashes, and is timed from starting
//! another to the first snapshot showing the session back, journals
//! replayed. `restart` prints p50, p99 and the slowest of each.

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

use techne_editor::{
    host::{Event, Host},
    present::{Input, Output, Snapshot},
    runtime::File,
};

/// Startups timed after the first.
const STARTS: usize = 20;
/// Restarts timed.
const RESTARTS: usize = 10;
/// Characters typed into the first file before the crashes.
const TYPED: usize = 2000;

fn main() {
    // The crashes are on purpose: not reported, unlike other panics.
    let reported = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if info.payload().downcast_ref::<&str>().is_none_or(|m| *m != "%crash-runtime") {
            reported(info);
        }
    }));
    let dir = tempfile::tempdir().expect("a temporary directory");
    // Journals of files opened from Lisp, and the session, go under it.
    unsafe { std::env::set_var("XDG_STATE_HOME", dir.path().join("state")) };
    let files = copy_lisp(dir.path());

    let mut starts: Vec<Duration> = (0..=STARTS).map(|i| start(&files[0], dir.path(), i)).collect();
    let first = starts.remove(0);
    println!("startup, first in the process: {}", ms(first));
    report("startup, again", &mut starts);

    let mut restarts = restarts(&files, dir.path());
    report("restart to the session", &mut restarts);
}

/// Every file of `lisp/editor`, copied into `dir` to edit.
fn copy_lisp(dir: &Path) -> Vec<PathBuf> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp/editor");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&source)
        .expect("lisp/editor")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "scm"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|from| {
            let to = dir.join(from.file_name().expect("a file name"));
            std::fs::copy(&from, &to).expect("a copy");
            to
        })
        .collect()
}

fn file(path: &Path, dir: &Path, n: usize) -> File {
    File { path: path.to_path_buf(), journal: dir.join(format!("{n}.journal")) }
}

fn host(file: File) -> (Host, mpsc::Receiver<Event>) {
    let (tx, events) = mpsc::channel();
    let host = Host::start(Some(file), "emacs".into(), |_| {}, move |e| drop(tx.send(e)));
    (host, events)
}

/// The next snapshot `done` holds for.
fn snapshot(events: &mpsc::Receiver<Event>, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
    loop {
        match events.recv_timeout(Duration::from_secs(60)).expect("an event") {
            Event::Output(Output::Snapshot(s)) if done(&s) => return *s,
            Event::Failed(e) => panic!("{e}"),
            Event::Ended => panic!("the runtime ended"),
            _ => {}
        }
    }
}

/// Wait until every key sent is answered: a frontend sending far more
/// than the runtime takes has the rest discarded (`host::BACKLOG_INPUTS`).
fn settle(host: &Host, events: &mpsc::Receiver<Event>) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while host.busy_from().is_some() {
        assert!(Instant::now() < deadline, "the runtime answers");
        match events.recv_timeout(Duration::from_millis(10)) {
            Ok(Event::Failed(e)) => panic!("{e}"),
            // A key that crashes it is not answered.
            Ok(Event::Ended) => return,
            Err(mpsc::RecvTimeoutError::Disconnected) => panic!("the runtime is gone"),
            _ => {}
        }
    }
}

fn keys(host: &mut Host, events: &mpsc::Receiver<Event>, keys: &str) {
    keys.split(' ').for_each(|k| host.send(Input::Key { key: k.into(), at: Instant::now() }));
    settle(host, events);
}

fn text(host: &mut Host, events: &mpsc::Receiver<Event>, text: &str) {
    for chunk in text.chars().collect::<Vec<_>>().chunks(100) {
        chunk.iter().for_each(|&c| host.send(Input::Key { key: if c == ' ' { "SPC".into() } else { c.into() }, at: Instant::now() }));
        settle(host, events);
    }
}

/// Start a runtime on `path`: the time to its first snapshot.
fn start(path: &Path, dir: &Path, n: usize) -> Duration {
    let begun = Instant::now();
    let (host, events) = host(file(path, dir, 1000 + n));
    snapshot(&events, |_| true);
    let took = begun.elapsed();
    host.close();
    took
}

/// Open every file with an unsaved edit, the first edited at length, in
/// two panes; then crash and restart the runtime `RESTARTS` times.
fn restarts(files: &[PathBuf], dir: &Path) -> Vec<Duration> {
    let (mut host, events) = host(file(&files[0], dir, 0));
    snapshot(&events, |_| true);
    for f in &files[1..] {
        // The prompt starts in the first file's directory, theirs.
        keys(&mut host, &events, "C-x C-f");
        text(&mut host, &events, &f.file_name().unwrap().to_string_lossy());
        keys(&mut host, &events, "RET M-> RET");
        text(&mut host, &events, ";; edited");
    }
    keys(&mut host, &events, "C-x b");
    text(&mut host, &events, &files[0].file_name().unwrap().to_string_lossy());
    keys(&mut host, &events, "RET M->");
    let typed: String = "(define (edited) 'unsaved) ".chars().cycle().take(TYPED).collect();
    text(&mut host, &events, &typed);
    // The second pane shows the second file, its end, where the crashes
    // are typed.
    keys(&mut host, &events, "C-x 2 C-x o C-x b");
    text(&mut host, &events, &files[1].file_name().unwrap().to_string_lossy());
    keys(&mut host, &events, "RET M-> RET");
    let mut times = Vec::new();
    for i in 0..RESTARTS {
        let crash = format!(";{i}");
        text(&mut host, &events, &crash);
        keys(&mut host, &events, "RET");
        text(&mut host, &events, "(%crash-runtime)");
        // Evaluated, it crashes the runtime.
        keys(&mut host, &events, "C-x C-e");
        let begun = Instant::now();
        assert!(host.restart(), "restarted");
        let s = snapshot(&events, |_| true);
        times.push(begun.elapsed());
        assert!(s.pane().text.to_string().contains(&crash), "the session came back");
        assert!(s.panes.iter().any(|p| p.text.to_string().ends_with(&typed)), "with the first file's edits");
        // The next crash goes on a line of its own.
        keys(&mut host, &events, "RET");
    }
    host.close();
    times
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

fn report(what: &str, times: &mut [Duration]) {
    times.sort();
    let at = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
    println!("{what}: p50 {}, p99 {}, slowest {} ({} runs)", ms(at(0.5)), ms(at(0.99)), ms(times[times.len() - 1]), times.len());
}
