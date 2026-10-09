//! Deadlines for the runtime's calls into Lisp between inputs (a part of a
//! snapshot, the session's state, a message): a thread kills the call's
//! execution (`techne_vm::stop`) when its deadline passes, so a
//! procedure redefined to loop cannot stall the editor.

use std::{
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
    time::Instant,
};

use techne_vm::vm::{ExecId, InterruptHandle, Stop};

#[derive(Default)]
struct State {
    /// The call watched, and when it is to end.
    call: Option<(ExecId, Instant)>,
    stop: bool,
}

/// Kills the execution armed when its deadline passes.
pub(crate) struct Watch {
    shared: Arc<(Mutex<State>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    pub(crate) fn new(handle: InterruptHandle) -> Watch {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let theirs = shared.clone();
        let thread = std::thread::Builder::new()
            .name("lisp deadlines".into())
            .spawn(move || {
                let (lock, wake) = &*theirs;
                let mut state = lock.lock().expect("the deadline");
                while !state.stop {
                    state = match state.call {
                        None => wake.wait(state).expect("the deadline"),
                        Some((id, at)) => match at.checked_duration_since(Instant::now()) {
                            Some(left) => wake.wait_timeout(state, left).expect("the deadline").0,
                            None => {
                                handle.stop(id, Stop::Kill);
                                state.call = None;
                                state
                            }
                        },
                    };
                }
            })
            .ok();
        Watch { shared, thread }
    }

    /// Kill `id` at `at`, unless it has ended by then (a stop for an
    /// execution that ended does nothing), or another is armed. The thread
    /// is woken only if it waits for none or for a later deadline: one
    /// waiting for an earlier deadline wakes then and finds this one.
    pub(crate) fn arm(&self, id: ExecId, at: Instant) {
        let (lock, wake) = &*self.shared;
        let before = lock.lock().expect("the deadline").call.replace((id, at));
        if before.is_none_or(|(_, earlier)| at < earlier) {
            wake.notify_one();
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let (lock, wake) = &*self.shared;
        lock.lock().expect("the deadline").stop = true;
        wake.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
