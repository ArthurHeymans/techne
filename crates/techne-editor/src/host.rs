//! The runtime on its own thread (`Runtime::serve`), restarted when it ends
//! without the session quitting: it crashed. The new runtime opens the same
//! file with its journal, so it has the unsaved edits back, and restores
//! the session's state as the old one last sent it (`Output::Session`):
//! the other files it had open, with theirs, its panes, carets and scroll
//! anchors. The frontend keeps its window or terminal and draws the new
//! runtime's snapshots. Started without a file, the runtime begins on an
//! empty *scratch* buffer, which is not journaled.
//!
//! C-g interrupts an evaluation that does not end, from the frontend's
//! thread, as the key itself waits behind it, and discards the inputs
//! queued before it (`runtime::Interrupts`); closing interrupts it too, and
//! leaves behind a runtime that still does not end.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use techne_text::Document;

use crate::{
    present::{Input, Output},
    runtime::{Interrupts, Runtime},
};

/// How long closing waits for the runtime to end.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// What the runtime thread delivers.
#[derive(Debug)]
pub enum Event {
    Output(Output),
    /// The runtime could not open the file; `Ended` follows.
    Failed(String),
    /// The thread ended, however it ended (a panic too).
    Ended,
}

/// A file to open, and where its unsaved edits are journaled.
#[derive(Clone, Debug)]
pub struct File {
    pub path: PathBuf,
    pub journal: PathBuf,
}

type Deliver = Arc<dyn Fn(Event) + Send + Sync>;
type Setup = Arc<dyn Fn(&mut Runtime) + Send + Sync>;
/// The session's state as the runtime last sent it.
type State = Arc<Mutex<Option<String>>>;

pub struct Host {
    file: Option<File>,
    profile: String,
    setup: Setup,
    deliver: Deliver,
    state: State,
    /// The running runtime's.
    interrupts: Arc<Interrupts>,
    inputs: mpsc::Sender<Input>,
    thread: Option<JoinHandle<()>>,
    /// The inputs sent to the running runtime.
    sent: u64,
}

impl Host {
    /// Open `file` with its journal (or start on an empty *scratch*
    /// buffer) in a runtime on a new thread, `setup`
    /// it, and serve: `deliver` gets its outputs, then `Ended`.
    pub fn start(
        file: Option<File>,
        profile: String,
        setup: impl Fn(&mut Runtime) + Send + Sync + 'static,
        deliver: impl Fn(Event) + Send + Sync + 'static,
    ) -> Host {
        let (setup, deliver, state): (Setup, Deliver, State) = (Arc::new(setup), Arc::new(deliver), State::default());
        let interrupts = Arc::<Interrupts>::default();
        let (inputs, thread) = spawn(file.clone(), &profile, setup.clone(), deliver.clone(), state.clone(), interrupts.clone());
        Host { file, profile, setup, deliver, state, interrupts, inputs, thread: Some(thread), sent: 0 }
    }

    /// Send an input. C-g also interrupts the input the runtime is
    /// handling, and discards those queued before it.
    pub fn send(&mut self, input: Input) {
        if matches!(&input, Input::Key { key, .. } if key == "C-g") {
            self.interrupts.interrupt_before(self.sent);
        }
        self.sent += 1;
        let _ = self.inputs.send(input);
    }

    /// After `Ended` without the session quitting: start a new runtime for
    /// the same file. Not when the runtime ended before it had any input,
    /// as it would again.
    pub fn restart(&mut self) -> bool {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if self.sent == 0 {
            return false;
        }
        let interrupts = Arc::<Interrupts>::default();
        let (inputs, thread) =
            spawn(self.file.clone(), &self.profile, self.setup.clone(), self.deliver.clone(), self.state.clone(), interrupts.clone());
        (self.interrupts, self.inputs, self.thread, self.sent) = (interrupts, inputs, Some(thread), 0);
        true
    }

    /// Tell the runtime the frontend is closing and wait for it,
    /// interrupting what it evaluates. One still running after
    /// `CLOSE_WAIT` (its code ignores interrupts) is left to end with the
    /// process.
    pub fn close(mut self) {
        let _ = self.inputs.send(Input::Close);
        if let Some(t) = self.thread.take() {
            let deadline = Instant::now() + CLOSE_WAIT;
            while !t.is_finished() && Instant::now() < deadline {
                self.interrupts.interrupt_before(self.sent);
                std::thread::sleep(Duration::from_millis(10));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

fn spawn(
    file: Option<File>,
    profile: &str,
    setup: Setup,
    deliver: Deliver,
    state: State,
    interrupts: Arc<Interrupts>,
) -> (mpsc::Sender<Input>, JoinHandle<()>) {
    /// Delivers `Ended` when dropped, also while a panic unwinds.
    struct Ending(Deliver);
    impl Drop for Ending {
        fn drop(&mut self) {
            (self.0)(Event::Ended)
        }
    }
    let (tx, rx) = mpsc::channel();
    let wake = tx.clone();
    let profile = profile.to_string();
    // The VM is not Send: the runtime is made on its own thread.
    let thread = std::thread::Builder::new()
        .name("runtime".into())
        .spawn(move || {
            let ending = Ending(deliver);
            let opened = match &file {
                Some(f) => Runtime::open(&f.path, &f.journal, &profile).map(|(rt, _)| rt),
                None => Runtime::with_document(Document::new(""), &profile).map_err(|e| e.to_string()),
            };
            match opened {
                Ok(mut rt) => {
                    rt.set_interrupts(interrupts);
                    let last = state.lock().expect("the state").clone();
                    if let Some(e) = last.and_then(|s| rt.restore(&s).err()) {
                        let _ = rt.message(&format!("The session could not be restored: {e}"));
                    }
                    setup(&mut rt);
                    rt.serve(rx, wake, |o| match o {
                        Output::Session(s) => *state.lock().expect("the state") = Some(s),
                        o => (ending.0)(Event::Output(o)),
                    })
                }
                Err(e) => (ending.0)(Event::Failed(e)),
            }
        })
        .expect("a thread for the runtime");
    (tx, thread)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::present::Snapshot;

    use super::*;

    fn wait_before(events: &mpsc::Receiver<Event>, deadline: Instant, done: impl Fn(&Event) -> bool, what: &str) -> Event {
        let mut last = None;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| panic!("timed out waiting for {what}; last snapshot: {last:?}"));
            let event = events.recv_timeout(remaining).unwrap_or_else(|e| panic!("waiting for {what}: {e}; last snapshot: {last:?}"));
            if done(&event) {
                return event;
            }
            match event {
                Event::Output(Output::Snapshot(s)) => last = Some(s),
                Event::Failed(e) => panic!("waiting for {what}: {e}; last snapshot: {last:?}"),
                Event::Ended => panic!("the runtime ended while waiting for {what}; last snapshot: {last:?}"),
                _ => {}
            }
        }
    }

    fn snapshot(events: &mpsc::Receiver<Event>, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
        match wait_before(
            events,
            Instant::now() + Duration::from_secs(20),
            |e| matches!(e, Event::Output(Output::Snapshot(s)) if done(s)),
            "a matching snapshot",
        ) {
            Event::Output(Output::Snapshot(s)) => *s,
            _ => unreachable!("the predicate only accepts snapshots"),
        }
    }

    fn ended(events: &mpsc::Receiver<Event>) {
        wait_before(events, Instant::now() + Duration::from_secs(20), |e| matches!(e, Event::Ended), "the runtime to end");
    }

    #[test]
    #[should_panic(expected = "timed out waiting for an expected event")]
    fn expired_wait_does_not_keep_receiving_events() {
        let (tx, events) = mpsc::channel();
        tx.send(Event::Output(Output::Session("still active".into()))).unwrap();
        wait_before(&events, Instant::now() - Duration::from_secs(1), |_| false, "an expected event");
    }

    /// The restart path: the runtime crashes on input
    /// (`%crash-runtime`, evaluated by C-x C-e), the host starts another with
    /// the unsaved edits.
    #[test]
    fn a_crashed_runtime_is_restarted_with_the_unsaved_edits() {
        let dir = tempfile::tempdir().unwrap();
        let (path, journal) = (dir.path().join("f.txt"), dir.path().join("f.journal"));
        std::fs::write(&path, "").unwrap();
        let (tx, events) = mpsc::channel();
        let mut host = Host::start(
            Some(File { path, journal }),
            "emacs".into(),
            |_| {},
            move |e| {
                let _ = tx.send(e);
            },
        );
        let key = |host: &mut Host, k: &str| host.send(Input::Key { key: k.into(), at: std::time::Instant::now() });
        "(%crash-runtime)".chars().for_each(|c| key(&mut host, &c.to_string()));
        snapshot(&events, |s| s.pane().text == "(%crash-runtime)");
        key(&mut host, "C-x");
        key(&mut host, "C-e");
        ended(&events);
        assert!(host.restart());
        let s = snapshot(&events, |_| true);
        assert_eq!(s.pane().text.to_string(), "(%crash-runtime)");
        assert!(s.echo.contains("Recovered"), "{}", s.echo);
        host.close();
    }

    /// The session comes back too: the other file opened, its unsaved
    /// edit, both panes with their carets, the focus.
    #[test]
    fn a_restarted_runtime_brings_the_session_back() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.txt"), dir.path().join("b.txt"));
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "one\n").unwrap();
        // Journals of files opened from Lisp go under the state directory.
        unsafe { std::env::set_var("XDG_STATE_HOME", dir.path().join("state")) };
        let (tx, events) = mpsc::channel();
        let mut host = Host::start(
            Some(File { path: a, journal: dir.path().join("a.journal") }),
            "emacs".into(),
            |_| {},
            move |e| {
                let _ = tx.send(e);
            },
        );
        let keys =
            |host: &mut Host, ks: &str| ks.split(' ').for_each(|k| host.send(Input::Key { key: k.into(), at: std::time::Instant::now() }));
        let text =
            |host: &mut Host, t: &str| t.chars().for_each(|c| host.send(Input::Key { key: c.into(), at: std::time::Instant::now() }));
        keys(&mut host, "C-x C-f");
        text(&mut host, "b.txt");
        keys(&mut host, "RET C-e");
        text(&mut host, "!");
        keys(&mut host, "C-x 2 C-x o C-x b RET");
        text(&mut host, "(%crash-runtime)");
        let s = snapshot(&events, |s| s.pane().text == "(%crash-runtime)");
        assert_eq!((s.panes.len(), s.focus), (2, 1));
        keys(&mut host, "C-x C-e");
        ended(&events);
        assert!(host.restart());
        let s = snapshot(&events, |_| true);
        let texts: Vec<String> = s.panes.iter().map(|p| p.text.to_string()).collect();
        assert_eq!(texts, ["one!\n", "(%crash-runtime)"]);
        assert_eq!((s.focus, s.panes[0].head(), s.panes[1].head()), (1, 4, 16));
        host.close();
    }

    /// Without a file, the session starts on an empty *scratch* buffer,
    /// from which files can be opened.
    #[test]
    fn starting_without_a_file() {
        let (tx, events) = mpsc::channel();
        let host = Host::start(
            None,
            "emacs".into(),
            |_| {},
            move |e| {
                let _ = tx.send(e);
            },
        );
        let s = snapshot(&events, |_| true);
        assert_eq!(s.pane().text.to_string(), "");
        assert!(s.pane().status.starts_with("*scratch*"), "{}", s.pane().status);
        host.close();
    }

    /// C-g interrupts an evaluation that never ends, and the session goes
    /// on; closing interrupts one too, and ends the runtime.
    #[test]
    fn a_stuck_evaluation_is_interrupted() {
        let (tx, events) = mpsc::channel();
        let mut host = Host::start(
            None,
            "emacs".into(),
            // One key evaluates, so no C-g can come between its keys.
            |rt| drop(rt.eval("(define-key! emacs-map \"<f5>\" 'eval-last-sexp)").unwrap()),
            move |e| {
                let _ = tx.send(e);
            },
        );
        let key = |host: &mut Host, k: &str| host.send(Input::Key { key: k.into(), at: std::time::Instant::now() });
        let stuck = |host: &mut Host| {
            "(let loop () (loop))".chars().for_each(|c| key(host, &if c == ' ' { "SPC".into() } else { c.to_string() }));
            snapshot(&events, |s| s.pane().text.to_string().ends_with("(let loop () (loop))"));
            key(host, "<f5>");
        };
        // One C-g, sent at once: the evaluation is discarded or interrupted,
        // whether or not it has started.
        stuck(&mut host);
        key(&mut host, "C-g");
        snapshot(&events, |s| s.echo == "Quit");
        // One C-g once it runs; what was typed meanwhile is discarded.
        stuck(&mut host);
        std::thread::sleep(Duration::from_millis(200));
        key(&mut host, "y");
        key(&mut host, "C-g");
        let s = snapshot(&events, |s| s.echo == "Quit");
        assert!(s.pane().text.to_string().ends_with("(loop))"), "{}", s.pane().text);
        key(&mut host, "x");
        snapshot(&events, |s| s.pane().text.to_string().ends_with("(loop))x"));
        key(&mut host, "RET");
        stuck(&mut host);
        std::thread::sleep(Duration::from_millis(100));
        let start = Instant::now();
        host.close();
        assert!(start.elapsed() < CLOSE_WAIT, "the runtime ended by itself");
        ended(&events);
    }

    /// Output a background task writes is drawn when it comes, with no
    /// input to answer: the task wakes the runtime.
    #[test]
    fn background_work_is_drawn_as_it_finishes() {
        let (tx, events) = mpsc::channel();
        let mut host = Host::start(
            None,
            "emacs".into(),
            |_| {},
            move |e| {
                let _ = tx.send(e);
            },
        );
        let key = |host: &mut Host, k: &str| host.send(Input::Key { key: k.into(), at: std::time::Instant::now() });
        key(&mut host, "M-&");
        for c in "sleep 0.3; echo done".chars() {
            let k = if c == ' ' { "SPC".to_string() } else { c.to_string() };
            key(&mut host, &k);
        }
        key(&mut host, "RET");
        snapshot(&events, |s| s.panes.len() == 2 && s.panes[1].text == "done\n");
        host.close();
    }
}
