//! The runtime on its own thread (`Runtime::serve`), shared by the
//! frontends attached to it, each through a `Host` (EDITOR.md, section 6):
//! `Host::start` starts it with a first frontend, `Host::attach` attaches
//! another. It ends when the last one detaches.
//!
//! A runtime that ends while frontends are attached crashed: each of them
//! gets `Event::Ended`, and the first to ask (`restart`) starts a new one,
//! to which every frontend is attached again. The new runtime opens their
//! files with their journals, so it has the unsaved edits back, and
//! restores each session's state as the old one last sent it
//! (`Output::Session`): the other files it had open, with theirs, its
//! panes, carets and scroll anchors. The frontends keep their windows or
//! terminals and draw the new runtime's snapshots. A frontend attached
//! without a file shows what the frontend used last shows, and the first
//! an empty *scratch* buffer, which is not journaled.
//!
//! C-g breaks an evaluation that does not end, from the frontend's thread,
//! as the key itself waits behind it, and discards the frontend's inputs
//! queued before it (`runtime::Interrupts`); a second C-g kills one that
//! caught the break, and closing kills it. The last frontend closing leaves
//! behind only a runtime stuck in a Rust native, which no kill reaches.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub use crate::runtime::File;
use crate::{
    present::{Input, Output},
    runtime::{Client, Interrupts, Msg, Runtime},
};

/// How long closing waits for the runtime to end.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// What a frontend gets from the runtime.
#[derive(Debug)]
pub enum Event {
    Output(Output),
    /// The frontend could not attach (its file could not be opened, or
    /// the editor could not load): nothing more comes.
    Failed(String),
    /// The runtime stopped while the frontend was attached: it crashed.
    Ended,
}

type Deliver = Arc<dyn Fn(Event) + Send + Sync>;
type Setup = Arc<dyn Fn(&mut Runtime) + Send + Sync>;

/// A frontend's attachment to the runtime.
pub struct Host {
    shared: Arc<Mutex<Shared>>,
    client: Client,
    /// The runtime it was last attached to, by its generation.
    generation: u64,
}

/// The runtime, and the frontends attached to it.
pub(crate) struct Shared {
    setup: Setup,
    /// Counts the runtimes started.
    generation: u64,
    /// The running runtime's; none once it is told to stop.
    pub(crate) inputs: Option<mpsc::Sender<Msg>>,
    thread: Option<JoinHandle<()>>,
    /// The inputs sent to the running runtime, by all its frontends.
    sent: u64,
    frontends: BTreeMap<Client, Frontend>,
    next_client: u64,
}

struct Frontend {
    file: Option<File>,
    profile: String,
    deliver: Deliver,
    /// Its session's state as the runtime last sent it.
    state: Option<String>,
    /// Its inputs' in the running runtime.
    interrupts: Arc<Interrupts>,
    /// The inputs it sent to the running runtime.
    sent: u64,
}

impl Host {
    /// Start a runtime on a new thread, `setup` it, and attach a frontend:
    /// it shows `file`, opened with its journal (or an empty *scratch*
    /// buffer), its keys read by `profile`. `deliver` gets its outputs.
    pub fn start(
        file: Option<File>,
        profile: String,
        setup: impl Fn(&mut Runtime) + Send + Sync + 'static,
        deliver: impl Fn(Event) + Send + Sync + 'static,
    ) -> Host {
        let shared = Arc::new(Mutex::new(Shared {
            setup: Arc::new(setup),
            generation: 0,
            inputs: None,
            thread: None,
            sent: 0,
            frontends: BTreeMap::new(),
            next_client: 0,
        }));
        attach(&shared, file, profile, Arc::new(deliver))
    }

    /// Attach another frontend to this one's runtime, as `start` attaches
    /// the first. Without `file`, it shows what the frontend used last
    /// shows.
    pub fn attach(&self, file: Option<File>, profile: String, deliver: impl Fn(Event) + Send + Sync + 'static) -> Host {
        attach(&self.shared, file, profile, Arc::new(deliver))
    }

    /// Open files in this runtime for other programs (`server`) that ask
    /// on `socket`, unless an editor listens there already (None then).
    /// It is listened on until the `Listener` is dropped.
    pub fn listen(&self, socket: &std::path::Path) -> std::io::Result<Option<crate::server::Listener>> {
        crate::server::listen(self.shared.clone(), socket)
    }

    /// Send an input. C-g also interrupts the frontend's input the runtime
    /// is handling, and discards those queued before it.
    pub fn send(&mut self, input: Input) {
        let mut s = self.shared.lock().expect("the host");
        let s = &mut *s;
        let Some(f) = s.frontends.get_mut(&self.client) else { return };
        if matches!(&input, Input::Key { key, .. } if key == "C-g") {
            f.interrupts.interrupt_before(f.sent);
        }
        f.sent += 1;
        s.sent += 1;
        if let Some(tx) = &s.inputs {
            let _ = tx.send(Msg::Input(self.client, input));
        }
    }

    /// After `Ended`: start a new runtime with every frontend attached
    /// again, unless another frontend did already. Not when the runtime
    /// ended before it had any input, as it would again.
    pub fn restart(&mut self) -> bool {
        let shared = self.shared.clone();
        // The thread is joined unlocked: it locks as it ends.
        let thread = {
            let mut s = shared.lock().expect("the host");
            if s.generation != self.generation {
                return self.restarted(&s);
            }
            s.thread.take()
        };
        if let Some(t) = thread {
            let _ = t.join();
        }
        let mut s = shared.lock().expect("the host");
        if s.generation != self.generation {
            return self.restarted(&s);
        }
        if s.sent == 0 {
            return false;
        }
        s.inputs = None;
        spawn(&shared, &mut s);
        self.generation = s.generation;
        true
    }

    /// Another frontend restarted the runtime: whether this one is
    /// attached to the new one.
    fn restarted(&mut self, s: &Shared) -> bool {
        self.generation = s.generation;
        s.frontends.contains_key(&self.client)
    }

    /// Detach, killing what the runtime evaluates for this frontend. The
    /// last frontend waits for the runtime to end; one still running after
    /// `CLOSE_WAIT` (stuck in a Rust native) is left to end with the
    /// process.
    pub fn close(self) {
        let thread = {
            let mut s = self.shared.lock().expect("the host");
            let f = s.frontends.remove(&self.client);
            if let (Some(_), Some(tx)) = (&f, &s.inputs) {
                let _ = tx.send(Msg::Input(self.client, Input::Close));
            }
            if let Some(f) = f {
                f.interrupts.kill_before(f.sent);
            }
            if !s.frontends.is_empty() {
                return;
            }
            if let Some(tx) = s.inputs.take() {
                let _ = tx.send(Msg::Stop);
            }
            s.thread.take()
        };
        if let Some(t) = thread {
            let deadline = Instant::now() + CLOSE_WAIT;
            while !t.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

/// Attach a frontend to the runtime of `shared`, started if none runs.
fn attach(shared: &Arc<Mutex<Shared>>, file: Option<File>, profile: String, deliver: Deliver) -> Host {
    let mut s = shared.lock().expect("the host");
    let client = Client(s.next_client);
    s.next_client += 1;
    let interrupts = Arc::<Interrupts>::default();
    let frontend = Frontend { file: file.clone(), profile: profile.clone(), deliver, state: None, interrupts: interrupts.clone(), sent: 0 };
    s.frontends.insert(client, frontend);
    match &s.inputs {
        Some(tx) => {
            let _ = tx.send(Msg::Attach { client, file, profile, state: None, interrupts });
        }
        None => spawn(shared, &mut s),
    }
    Host { shared: shared.clone(), client, generation: s.generation }
}

/// Start a new runtime, with every frontend in `s` attached to it.
fn spawn(shared: &Arc<Mutex<Shared>>, s: &mut Shared) {
    /// Tells the frontends attached that the runtime ended, unless it was
    /// told to: when dropped, also while a panic unwinds.
    struct Ending(Arc<Mutex<Shared>>, u64);
    impl Drop for Ending {
        fn drop(&mut self) {
            let delivers: Vec<Deliver> = {
                let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
                if s.generation != self.1 || s.inputs.take().is_none() {
                    return;
                }
                s.frontends.values().map(|f| f.deliver.clone()).collect()
            };
            delivers.iter().for_each(|d| d(Event::Ended));
        }
    }
    s.generation += 1;
    s.sent = 0;
    let (tx, rx) = mpsc::channel();
    for (&client, f) in &mut s.frontends {
        f.interrupts = Arc::default();
        f.sent = 0;
        let attach = Msg::Attach {
            client,
            file: f.file.clone(),
            profile: f.profile.clone(),
            state: f.state.clone(),
            interrupts: f.interrupts.clone(),
        };
        let _ = tx.send(attach);
    }
    let (wake, setup, generation, shared) = (tx.clone(), s.setup.clone(), s.generation, shared.clone());
    // The VM is not Send: the runtime is made on its own thread.
    let thread = std::thread::Builder::new()
        .name("runtime".into())
        .spawn(move || {
            let ending = Ending(shared.clone(), generation);
            match Runtime::new() {
                Ok(mut rt) => {
                    setup(&mut rt);
                    rt.serve(rx, wake, |client, o| route(&shared, generation, client, o));
                }
                Err(e) => {
                    // Unless a newer runtime has taken over meanwhile.
                    let delivers: Vec<Deliver> = {
                        let mut s = shared.lock().expect("the host");
                        if s.generation != generation {
                            return;
                        }
                        s.inputs = None;
                        std::mem::take(&mut s.frontends).into_values().map(|f| f.deliver).collect()
                    };
                    delivers.iter().for_each(|d| d(Event::Failed(e.to_string())));
                }
            }
            drop(ending);
        })
        .expect("a thread for the runtime");
    s.inputs = Some(tx);
    s.thread = Some(thread);
}

/// Deliver an output of the runtime of `generation` to its frontend. One
/// whose session quit, or that could not attach, is detached; with the
/// last one gone, the runtime is told to stop.
fn route(shared: &Mutex<Shared>, generation: u64, client: Client, output: Output) {
    let deliver = {
        let mut s = shared.lock().expect("the host");
        if s.generation != generation {
            return;
        }
        let event = match output {
            Output::Session(state) => {
                if let Some(f) = s.frontends.get_mut(&client) {
                    f.state = Some(state);
                }
                return;
            }
            Output::Refused(e) => Event::Failed(e),
            o => Event::Output(o),
        };
        let detached = matches!(event, Event::Failed(_) | Event::Output(Output::Quit));
        let deliver =
            if detached { s.frontends.remove(&client).map(|f| f.deliver) } else { s.frontends.get(&client).map(|f| f.deliver.clone()) };
        if detached
            && s.frontends.is_empty()
            && let Some(tx) = s.inputs.take()
        {
            let _ = tx.send(Msg::Stop);
        }
        deliver.map(|d| (d, event))
    };
    if let Some((d, event)) = deliver {
        d(event);
    }
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
        // It ended as it was told to: no frontend is told it crashed.
        assert!(!events.try_iter().any(|e| matches!(e, Event::Ended)));
    }

    /// The evaluation of `src` started with <f5>, in `host`, once its text
    /// is in the pane.
    fn evaluate(host: &mut Host, events: &mpsc::Receiver<Event>, src: &str) {
        let key = |host: &mut Host, k: &str| host.send(Input::Key { key: k.into(), at: Instant::now() });
        src.chars().for_each(|c| key(host, &if c == ' ' { "SPC".into() } else { c.to_string() }));
        snapshot(events, |s| s.pane().text.to_string().ends_with(src));
        key(host, "<f5>");
    }

    /// With <f5> evaluating the expression before point.
    fn evaluating_host() -> (Host, mpsc::Receiver<Event>) {
        let (tx, events) = mpsc::channel();
        let host = Host::start(
            None,
            "emacs".into(),
            |rt| drop(rt.eval("(define-key! emacs-map \"<f5>\" 'eval-last-sexp)").unwrap()),
            move |e| {
                let _ = tx.send(e);
            },
        );
        (host, events)
    }

    const STUBBORN: &str = "(let loop () (guard (e (#t #f)) (let spin () (spin))) (loop))";

    /// Evaluate `src` once it has written a file, and wait for that: then
    /// it runs.
    fn evaluate_running(host: &mut Host, events: &mpsc::Receiver<Event>, src: &str) {
        let marker = std::env::temp_dir().join(format!("techne-running-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_file(&marker);
        let path = marker.display().to_string();
        evaluate(host, events, &format!("(begin (call-with-output-file {path:?} (lambda (p) (write 1 p))) {src})"));
        let deadline = Instant::now() + Duration::from_secs(20);
        while !marker.exists() {
            assert!(Instant::now() < deadline, "{src} did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = std::fs::remove_file(&marker);
    }

    /// An evaluation that catches the break goes on after one C-g, and is
    /// killed by the next; closing kills one too.
    #[test]
    fn a_second_c_g_kills_an_evaluation_catching_the_first() {
        let (mut host, events) = evaluating_host();
        let key = |host: &mut Host, k: &str| host.send(Input::Key { key: k.into(), at: Instant::now() });
        evaluate_running(&mut host, &events, STUBBORN);
        // Snapshots of the typing, sent before it ran.
        events.try_iter().for_each(drop);
        key(&mut host, "C-g");
        std::thread::sleep(Duration::from_millis(200));
        assert!(!events.try_iter().any(|e| matches!(e, Event::Output(Output::Snapshot(_)))), "still running");
        key(&mut host, "C-g");
        key(&mut host, "x");
        snapshot(&events, |s| s.pane().text.to_string().ends_with("(loop)))x"));
        // The C-g keys quit too, after: the kill is in *Messages*.
        key(&mut host, "C-h");
        key(&mut host, "e");
        snapshot(&events, |s| s.pane().text.to_string().contains("Quit (killed)"));
        key(&mut host, "C-x");
        key(&mut host, "b");
        key(&mut host, "RET");
        evaluate_running(&mut host, &events, STUBBORN);
        let start = Instant::now();
        host.close();
        assert!(start.elapsed() < CLOSE_WAIT, "the runtime ended by itself");
    }

    /// C-g breaks the command it is for, not a background task running
    /// while it waits, which catches every break it gets.
    #[test]
    fn a_background_task_does_not_take_a_commands_c_g() {
        let (mut host, events) = evaluating_host();
        let key = |host: &mut Host, k: &str| host.send(Input::Key { key: k.into(), at: Instant::now() });
        evaluate(
            &mut host,
            &events,
            "(define t (spawn (lambda () (let l () (guard (e (#t (set! taken #t))) (let s () (sleep 1) (s))) (l)))))",
        );
        key(&mut host, "RET");
        evaluate(&mut host, &events, "(define taken #f)");
        key(&mut host, "RET");
        evaluate_running(&mut host, &events, "(let s () (sleep 1) (s))");
        key(&mut host, "C-g");
        snapshot(&events, |s| s.echo == "Quit");
        key(&mut host, "RET");
        evaluate(&mut host, &events, "taken");
        snapshot(&events, |s| s.echo.contains("#f"));
        host.close();
    }

    /// Closing a frontend whose command caught the break kills it, so the
    /// other frontends go on.
    #[test]
    fn closing_kills_for_the_others() {
        let (a, a_events) = evaluating_host();
        snapshot(&a_events, |_| true);
        let (b_events, deliver) = channel();
        let mut b = a.attach(None, "emacs".into(), deliver);
        snapshot(&b_events, |_| true);
        evaluate_running(&mut b, &b_events, STUBBORN);
        b.close();
        let mut a = a;
        let key = |host: &mut Host, k: &str| host.send(Input::Key { key: k.into(), at: Instant::now() });
        key(&mut a, "C-e");
        key(&mut a, "x");
        snapshot(&a_events, |s| s.pane().text.to_string().ends_with("(loop)))x"));
        a.close();
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

    fn channel() -> (mpsc::Receiver<Event>, impl Fn(Event) + Send + Sync + 'static) {
        let (tx, events) = mpsc::channel();
        (events, move |e| {
            let _ = tx.send(e);
        })
    }

    fn keys(host: &mut Host, keys: &str) {
        keys.split(' ').for_each(|k| host.send(Input::Key { key: k.into(), at: Instant::now() }));
    }

    fn text(host: &mut Host, text: &str) {
        text.chars().for_each(|c| host.send(Input::Key { key: c.into(), at: Instant::now() }));
    }

    /// Two frontends on one runtime: what one types, the other shows; one
    /// quitting leaves the other, and the last one quitting ends the
    /// runtime.
    #[test]
    fn frontends_attach_to_one_runtime() {
        let (a_events, deliver) = channel();
        let mut a = Host::start(None, "emacs".into(), |_| {}, deliver);
        snapshot(&a_events, |_| true);
        let (b_events, deliver) = channel();
        let mut b = a.attach(None, "modal".into(), deliver);
        snapshot(&b_events, |_| true);
        text(&mut a, "hi");
        snapshot(&b_events, |s| s.pane().text == "hi");
        keys(&mut b, ": q RET");
        wait_before(&b_events, Instant::now() + Duration::from_secs(20), |e| matches!(e, Event::Output(Output::Quit)), "b to quit");
        b.close();
        text(&mut a, "!");
        snapshot(&a_events, |s| s.pane().text == "hi!");
        keys(&mut a, "C-x C-c");
        wait_before(&a_events, Instant::now() + Duration::from_secs(20), |e| matches!(e, Event::Output(Output::Quit)), "a to quit");
        let thread = a.shared.lock().unwrap().thread.take().unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !thread.is_finished() {
            assert!(Instant::now() < deadline, "the runtime did not end");
            std::thread::sleep(Duration::from_millis(10));
        }
        a.close();
    }

    /// A runtime crashing takes every frontend's session down: each is
    /// brought back in the new runtime, whichever frontend restarts it.
    #[test]
    fn a_restart_brings_every_frontend_back() {
        let dir = tempfile::tempdir().unwrap();
        let (path, journal) = (dir.path().join("f.txt"), dir.path().join("f.journal"));
        std::fs::write(&path, "").unwrap();
        let (a_events, deliver) = channel();
        let mut a = Host::start(Some(File { path, journal }), "emacs".into(), |_| {}, deliver);
        let (b_events, deliver) = channel();
        let mut b = a.attach(None, "emacs".into(), deliver);
        keys(&mut b, "C-x 2");
        text(&mut a, "(%crash-runtime)");
        snapshot(&b_events, |s| s.panes.len() == 2 && s.pane().text == "(%crash-runtime)");
        keys(&mut a, "C-x C-e");
        ended(&a_events);
        ended(&b_events);
        assert!(b.restart());
        assert!(a.restart(), "restarted already");
        let s = snapshot(&a_events, |_| true);
        assert_eq!((s.panes.len(), s.pane().text.to_string()), (1, "(%crash-runtime)".into()));
        snapshot(&b_events, |s| s.panes.len() == 2);
        a.close();
        b.close();
    }

    /// A frontend sending Close is detached, as one whose session quits.
    #[test]
    fn closing_by_input_detaches() {
        let (a_events, deliver) = channel();
        let a = Host::start(None, "emacs".into(), |_| {}, deliver);
        snapshot(&a_events, |_| true);
        let (b_events, deliver) = channel();
        let mut b = a.attach(None, "emacs".into(), deliver);
        snapshot(&b_events, |_| true);
        b.send(Input::Close);
        wait_before(&b_events, Instant::now() + Duration::from_secs(20), |e| matches!(e, Event::Output(Output::Quit)), "b to quit");
        assert_eq!(a.shared.lock().unwrap().frontends.len(), 1);
        a.close();
    }
}
