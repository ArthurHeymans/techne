//! Stopping executions (PLAN.md, Stage 1, step 8). An execution is a
//! top-level call into the VM (`Vm::enter_execution`, or any evaluation
//! from the host) or a task; stops are addressed to one, by its `ExecId`,
//! and never reach another.
//!
//! A *break* raises the catchable condition "interrupted" at the
//! execution's next check (a call or loop iteration, a wait). A *kill* is
//! caught by nothing: it unwinds the execution without running Lisp code
//! (no handler, no `dynamic-wind` after thunk) and stays pending until the
//! execution has ended, so nothing it runs meanwhile goes on. A task
//! joining a killed task gets the catchable condition "task killed".
//!
//! Requests come from any thread. They wait in a table until the
//! execution runs; the VM's one attention flag is set while the running
//! execution has one, so the checks in the hot paths stay a relaxed load.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::Thread,
};

use rustc_hash::{FxHashMap, FxHashSet};

/// Identifies an execution; never reused. 0 is none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExecId(pub u64);

/// What a stop request asks of an execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stop {
    /// Raise the catchable condition "interrupted".
    Break,
    /// End it; nothing catches this, and no Lisp cleanup runs.
    Kill,
}

/// The condition a break raises.
pub const INTERRUPTED: &str = "interrupted";

/// The message of a kill, which only the host sees.
pub const KILLED: &str = "killed";

/// The condition a task joining a killed task sees.
pub const TASK_KILLED: &str = "task killed";

#[derive(Default)]
struct Table {
    live: FxHashSet<u64>,
    pending: FxHashMap<u64, Stop>,
    /// The execution running now (0: none).
    running: u64,
}

/// The stop requests of a VM's executions, shared with `InterruptHandle`s.
pub(crate) struct Stops {
    /// Set while the running execution has a stop pending.
    pub(crate) attention: Arc<AtomicBool>,
    table: Mutex<Table>,
    next: AtomicU64,
    /// The outermost top-level execution running, for `interrupt`.
    root: AtomicU64,
}

impl Stops {
    pub(crate) fn new(attention: Arc<AtomicBool>) -> Stops {
        Stops { attention, table: Mutex::default(), next: AtomicU64::new(1), root: AtomicU64::new(0) }
    }

    fn table(&self) -> std::sync::MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A new execution, which requests may reach from now on.
    pub(crate) fn begin(&self) -> ExecId {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.table().live.insert(id);
        ExecId(id)
    }

    /// The execution ended: its pending stop is dropped, and later
    /// requests for it with it.
    pub(crate) fn end(&self, id: ExecId) {
        let mut t = self.table();
        t.live.remove(&id.0);
        t.pending.remove(&id.0);
        if t.running == id.0 {
            t.running = 0;
            self.attention.store(false, Ordering::SeqCst);
        }
    }

    /// `id` runs from now on (0: none).
    pub(crate) fn run(&self, id: ExecId) {
        let mut t = self.table();
        t.running = id.0;
        self.attention.store(t.pending.contains_key(&id.0), Ordering::SeqCst);
    }

    pub(crate) fn set_root(&self, id: ExecId) {
        self.root.store(id.0, Ordering::SeqCst);
    }

    /// Ask `id` to stop; a kill outranks a break. Whether it is live.
    fn request(&self, id: ExecId, stop: Stop) -> bool {
        let mut t = self.table();
        if !t.live.contains(&id.0) {
            return false;
        }
        let entry = t.pending.entry(id.0).or_insert(stop);
        *entry = (*entry).max(stop);
        if t.running == id.0 {
            self.attention.store(true, Ordering::SeqCst);
        }
        true
    }

    /// The running execution's pending stop, taken if it is a break: a
    /// kill stays until the execution ends.
    pub(crate) fn take(&self) -> Option<Stop> {
        let mut t = self.table();
        let running = t.running;
        let stop = t.pending.get(&running).copied();
        if stop == Some(Stop::Break) {
            t.pending.remove(&running);
        }
        self.attention.store(stop == Some(Stop::Kill), Ordering::SeqCst);
        stop
    }

    /// The stop pending for `id`, left in place.
    pub(crate) fn pending(&self, id: ExecId) -> Option<Stop> {
        self.table().pending.get(&id.0).copied()
    }

    /// Take a break pending for `id`; a kill stays.
    pub(crate) fn take_break(&self, id: ExecId) -> bool {
        let mut t = self.table();
        let found = t.pending.get(&id.0) == Some(&Stop::Break);
        if found {
            t.pending.remove(&id.0);
        }
        found
    }

    /// Whether any stop waits, for the scheduler to look before it runs
    /// tasks.
    pub(crate) fn any_pending(&self) -> bool {
        !self.table().pending.is_empty()
    }
}

/// Stops a VM's executions from any thread.
#[derive(Clone)]
pub struct InterruptHandle {
    pub(crate) stops: Arc<Stops>,
    pub(crate) thread: Thread,
    pub(crate) notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl InterruptHandle {
    /// Ask the execution `id` to stop. Returns false if it has ended (or
    /// never was), in which case nothing happens.
    pub fn stop(&self, id: ExecId, stop: Stop) -> bool {
        let live = self.stops.request(id, stop);
        if live {
            self.thread.unpark();
            if let Some(notify) = &self.notify {
                notify();
            }
        }
        live
    }

    /// Break the outermost top-level execution running now (a REPL's
    /// evaluation), if any.
    pub fn interrupt(&self) {
        self.stop(ExecId(self.stops.root.load(Ordering::SeqCst)), Stop::Break);
    }

    /// The running execution has a stop pending.
    pub fn is_pending(&self) -> bool {
        self.stops.attention.load(Ordering::SeqCst)
    }

    /// The flag set while the running execution has a stop pending, for
    /// natives that run long to watch.
    pub fn flag(&self) -> &AtomicBool {
        &self.stops.attention
    }

    /// Take the running execution's pending break, returning whether a stop
    /// was pending: a native that stopped for it raises "interrupted"
    /// instead. A kill stays, and ends the execution at its next check.
    pub fn take(&self) -> bool {
        self.stops.take().is_some()
    }
}
