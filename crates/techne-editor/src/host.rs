//! The runtime on its own thread (`Runtime::serve`), restarted when it ends
//! without the session quitting: it crashed. The new runtime opens the same
//! file with its journal, so it has the unsaved edits back; the frontend
//! keeps its window or terminal and draws the new runtime's snapshots.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread::JoinHandle,
};

use crate::{
    present::{Input, Output},
    runtime::Runtime,
};

/// What the runtime thread delivers.
#[derive(Debug)]
pub enum Event {
    Output(Output),
    /// The runtime could not open the file; `Ended` follows.
    Failed(String),
    /// The thread ended, however it ended (a panic too).
    Ended,
}

type Deliver = Arc<dyn Fn(Event) + Send + Sync>;
type Setup = Arc<dyn Fn(&mut Runtime) + Send + Sync>;

pub struct Host {
    path: PathBuf,
    journal: PathBuf,
    profile: String,
    setup: Setup,
    deliver: Deliver,
    inputs: mpsc::Sender<Input>,
    thread: Option<JoinHandle<()>>,
    /// Inputs were sent to the runtime since it started.
    sent: bool,
}

impl Host {
    /// Open `path` with its journal in a runtime on a new thread, `setup`
    /// it, and serve: `deliver` gets its outputs, then `Ended`.
    pub fn start(
        path: PathBuf,
        journal: PathBuf,
        profile: String,
        setup: impl Fn(&mut Runtime) + Send + Sync + 'static,
        deliver: impl Fn(Event) + Send + Sync + 'static,
    ) -> Host {
        let (setup, deliver): (Setup, Deliver) = (Arc::new(setup), Arc::new(deliver));
        let (inputs, thread) = spawn(&path, &journal, &profile, setup.clone(), deliver.clone());
        Host { path, journal, profile, setup, deliver, inputs, thread: Some(thread), sent: false }
    }

    pub fn send(&mut self, input: Input) {
        self.sent = true;
        let _ = self.inputs.send(input);
    }

    /// After `Ended` without the session quitting: start a new runtime for
    /// the same file. Not when the runtime ended before it had any input,
    /// as it would again.
    pub fn restart(&mut self) -> bool {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if !self.sent {
            return false;
        }
        let (inputs, thread) = spawn(&self.path, &self.journal, &self.profile, self.setup.clone(), self.deliver.clone());
        (self.inputs, self.thread, self.sent) = (inputs, Some(thread), false);
        true
    }

    /// Tell the runtime the frontend is closing and wait for it.
    pub fn close(mut self) {
        let _ = self.inputs.send(Input::Close);
        drop(self.inputs);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn spawn(path: &Path, journal: &Path, profile: &str, setup: Setup, deliver: Deliver) -> (mpsc::Sender<Input>, JoinHandle<()>) {
    /// Delivers `Ended` when dropped, also while a panic unwinds.
    struct Ending(Deliver);
    impl Drop for Ending {
        fn drop(&mut self) {
            (self.0)(Event::Ended)
        }
    }
    let (tx, rx) = mpsc::channel();
    let (path, journal, profile) = (path.to_path_buf(), journal.to_path_buf(), profile.to_string());
    // The VM is not Send: the runtime is made on its own thread.
    let thread = std::thread::Builder::new()
        .name("runtime".into())
        .spawn(move || {
            let ending = Ending(deliver);
            match Runtime::open(&path, &journal, &profile) {
                Ok((mut rt, _)) => {
                    setup(&mut rt);
                    rt.serve(rx, |o| (ending.0)(Event::Output(o)))
                }
                Err(e) => (ending.0)(Event::Failed(e)),
            }
        })
        .expect("a thread for the runtime");
    (tx, thread)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::present::Snapshot;

    use super::*;

    /// The restart path: the runtime crashes on input
    /// (`%crash-runtime`, evaluated by C-M-x), the host starts another with
    /// the unsaved edits.
    #[test]
    fn a_crashed_runtime_is_restarted_with_the_unsaved_edits() {
        let dir = tempfile::tempdir().unwrap();
        let (path, journal) = (dir.path().join("f.txt"), dir.path().join("f.journal"));
        std::fs::write(&path, "").unwrap();
        let (tx, events) = mpsc::channel();
        let mut host = Host::start(
            path,
            journal,
            "emacs".into(),
            |_| {},
            move |e| {
                let _ = tx.send(e);
            },
        );
        let snapshot = |events: &mpsc::Receiver<Event>, done: &dyn Fn(&Snapshot) -> bool| loop {
            match events.recv_timeout(Duration::from_secs(20)).expect("an event") {
                Event::Output(Output::Snapshot(s)) if done(&s) => return *s,
                Event::Ended => panic!("the runtime ended"),
                _ => {}
            }
        };
        let key = |host: &mut Host, k: &str| host.send(Input::Key { key: k.into(), at: std::time::Instant::now() });
        "(%crash-runtime)".chars().for_each(|c| key(&mut host, &c.to_string()));
        snapshot(&events, &|s| s.pane().text == "(%crash-runtime)");
        key(&mut host, "C-M-x");
        while !matches!(events.recv_timeout(Duration::from_secs(20)).expect("an event"), Event::Ended) {}
        assert!(host.restart());
        let s = snapshot(&events, &|_| true);
        assert_eq!(s.pane().text.to_string(), "(%crash-runtime)");
        assert!(s.echo.contains("Recovered"), "{}", s.echo);
        host.close();
    }
}
