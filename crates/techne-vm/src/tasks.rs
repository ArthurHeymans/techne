//! Cooperative, preemptible tasks.
//!
//! Each task owns a register/frame/handler stack; running a task swaps it in.
//! A task is preempted after `TASK_SLICE` calls or backward jumps, and
//! suspends when a native it calls must wait: `sleep`, `channel-recv`,
//! `channel-send`, `select`, `task-join`, `yield`, or a Rust future
//! registered with `register_async`.
//! The waiting native's call site receives the result when the task resumes.
//!
//! The scheduler runs only the tasks ready, from a run queue, in the order
//! they became ready. What a waiting task waits for puts it there: a future's
//! waker (from any thread, through `Vm::wakes`), a channel operation that
//! may let it go (channels list the tasks waiting to receive and to send),
//! the task it joins finishing, its timer expiring (a heap by deadline).
//! A woken task looks at its wait again and lists itself anew if it must
//! go on waiting; entries of its earlier waits are stale (`Waiter::armed`)
//! and dropped as they are met. So a round costs what is ready, however
//! many tasks wait.
//!
//! Code that is not in a task (the main program, the REPL) waits by running
//! the scheduler until its wait is satisfied, so `(task-join t)` at top level
//! drives every task. A Rust native that calls back into Scheme is a
//! non-suspendable boundary, like a C call in Lua.
//!
//! A host with its own event loop drives tasks with `run_tasks_for` instead:
//! it runs them for a time budget and never blocks the thread. `next_timer`
//! and `set_wake_notifier` tell the host when to call it again.
//!
//! `cancel_task` / `task-cancel` raise the condition "task cancelled" where the
//! task is suspended (dropping the Rust future it waits on), so its
//! `dynamic-wind` and `guard` cleanup runs; a task may catch it.
//!
//! Channels are bounded: `(make-channel)` is a rendezvous (a send completes
//! when a receiver takes the value), `(make-channel n #:bytes b)` buffers up
//! to `n` messages and, if given, `b` bytes of strings (a single larger
//! message still fits an empty buffer). A full channel makes
//! senders wait; waiting senders are queued as offers, taken in order.
//! `channel-close` refuses later sends and fails waiting ones; receivers get
//! the buffered values, then the eof object.
//!
//! A task's Lisp object holds its slot in `Vm::tasks` and the slot's
//! generation while it runs, and its result once it has finished: the slot
//! is then free for the next task spawned. So the result stays as long as
//! someone holds the object (a Rust `TaskId` roots it), and finished tasks
//! take no slot. A channel's object holds its slot and generation too; a
//! channel closed and drained frees its slot, and an object of an older
//! generation is a closed, empty channel.
//!
//! `select` waits for the first of several receives, sends and timeouts and
//! commits exactly one: taking one offer of a waiting select withdraws its
//! others at once, and a select that stops waiting (it won, was cancelled or
//! interrupted) withdraws its offers. Ready operations are tried in turn
//! from a rotating start, timeouts last. A ready operation completes without
//! suspending, so outside tasks (where nothing preempts) a loop polling with
//! `(timeout 0)` starves the tasks it waits for; give it a positive timeout.
//!
//! ```
//! use techne_vm::vm::Vm;
//! let mut vm = Vm::new();
//! vm.register_async("slow-double", 1, |vm, args| {
//!     let x: i64 = args.get(vm, 0).unwrap();
//!     async move { Ok::<_, String>(x * 2) }
//! });
//! let v = vm.eval_source("(task-join (spawn (lambda () (+ 1 (slow-double 20)))))").unwrap();
//! assert_eq!(v.as_int(), Some(41));
//! ```

use std::{
    fmt::Display,
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    thread::Thread,
    time::{Duration, Instant},
};

use std::{cmp::Reverse, collections::VecDeque};

use rustc_hash::FxHashMap;

use crate::{
    api::{IntoValue, Root},
    builtins::type_error,
    heap::{Kind, field, is_kind, set_field},
    value::Value,
    vm::{Error, Exit, Native, NativeImpl, SpecialObj, Stack, Suspend, Vm},
};

/// Turns a finished Rust future's output into a Scheme value on the VM thread.
pub(crate) type Converter = Box<dyn FnOnce(&mut Vm) -> Result<Value, Error>>;
pub(crate) type BoxFuture = Pin<Box<dyn Future<Output = Result<Converter, Error>>>>;

/// A task: its slot and the slot's generation then. A task finished, its
/// slot taken by another, is told apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TaskRef {
    slot: usize,
    generation: u32,
}

/// A task waiting, in a channel's, a task's or the timers' list of those to
/// wake: one entry of an earlier wait (`armed` counts them) is stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Waiter {
    task: TaskRef,
    armed: u32,
}

/// The tasks futures woke, from any thread, for the scheduler to take.
#[derive(Default)]
pub(crate) struct Wakes(Mutex<Vec<TaskRef>>);

/// Wakes a task (and the scheduler thread, and the host) when a future it
/// waits on can make progress. A task has one for all its waits: a late
/// wake of an earlier one only makes it look again.
pub(crate) struct Flag {
    /// Set when woken, until the future is polled; waking it again
    /// meanwhile does nothing more.
    pending: AtomicBool,
    /// The task it wakes; none for code outside tasks.
    task: Option<TaskRef>,
    wakes: Arc<Wakes>,
    thread: Thread,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if self.pending.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(task) = self.task {
            self.wakes.0.lock().unwrap_or_else(|e| e.into_inner()).push(task);
        }
        self.thread.unpark();
        if let Some(notify) = &self.notify {
            notify();
        }
    }
}

/// Where `run_tasks_for` stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// No unfinished tasks.
    Finished,
    /// Every unfinished task waits (timers, futures, channels, joins).
    Blocked,
    /// The budget ran out with tasks still runnable.
    OutOfTime,
}

/// The condition a cancelled task sees.
pub const CANCELLED: &str = "task cancelled";

/// What a suspended task (or the main program) waits for.
pub(crate) enum Wait {
    Yield,
    Sleep(Instant),
    Select(Box<Select>),
    /// The task's object.
    Join(Root),
    Future {
        fut: BoxFuture,
        flag: Arc<Flag>,
    },
}

impl Wait {
    /// Whether the values it holds are of `world` (a host native may hand
    /// on an error of another world, its wait with it).
    pub(crate) fn of_world(&self, world: crate::api::WorldId) -> bool {
        match self {
            Wait::Join(r) => r.world() == world,
            Wait::Select(s) => s.ops.iter().all(|op| !matches!(op, Op::Send { value, .. } if value.world() != world)),
            Wait::Yield | Wait::Sleep(_) | Wait::Future { .. } => true,
        }
    }
}

/// A channel's slot in `Vm::channels`, and the slot's generation then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChannelId {
    slot: usize,
    generation: u32,
}

/// A bounded channel.
pub(crate) struct Channel {
    /// Counts the channels the slot has held.
    generation: u32,
    /// Accepted messages and their sizes in bytes.
    buf: VecDeque<(Root, usize)>,
    bytes: usize,
    capacity: usize,
    max_bytes: Option<usize>,
    closed: bool,
    /// Waiting senders, in order.
    offers: VecDeque<Offer>,
    /// Tasks waiting to receive, and to send, woken when it may go.
    receivers: Vec<Waiter>,
    senders: Vec<Waiter>,
}

struct Offer {
    group: u64,
    branch: usize,
    value: Root,
    size: usize,
}

/// The groups of waiting sends and selects with offers out.
#[derive(Default)]
pub(crate) struct Offers {
    next: u64,
    groups: FxHashMap<u64, Group>,
    /// Where the next select starts trying (fairness).
    turn: usize,
}

struct Group {
    /// Channels holding the group's offers (open ones: closing withdraws
    /// them), by slot.
    channels: Vec<usize>,
    /// The branch whose offer a receiver took.
    won: Option<usize>,
    /// The task waiting, woken when it won.
    task: Option<TaskRef>,
}

/// One operation of a select.
pub(crate) enum Op {
    Recv(ChannelId),
    Send { ch: ChannelId, value: Root, size: usize },
    After(Instant),
}

/// A wait for the first ready operation: `channel-recv`, `channel-send` and
/// `select`.
pub(crate) struct Select {
    ops: Vec<Op>,
    /// Its offers' group, once it waits.
    group: Option<u64>,
    /// Deliver `(index . value)` (select) or just the value.
    indexed: bool,
}

impl Channel {
    fn has_room(&self, size: usize) -> bool {
        self.buf.len() < self.capacity && (self.buf.is_empty() || self.max_bytes.is_none_or(|m| self.bytes + size <= m))
    }
}

/// Bytes a message counts against a channel's byte limit: a string's
/// length in bytes; other values count only as a message.
fn message_size(v: Value) -> usize {
    if is_kind(v, Kind::String) { unsafe { crate::heap::str_bytes(v.as_ptr()) }.len() } else { 0 }
}

impl Wait {
    /// When a timer ends this wait.
    fn deadline(&self) -> Option<Instant> {
        match self {
            Wait::Sleep(d) => Some(*d),
            Wait::Select(s) => s.ops.iter().filter_map(|op| if let Op::After(d) = op { Some(*d) } else { None }).min(),
            _ => None,
        }
    }
}

pub(crate) enum State {
    Runnable,
    Waiting(Wait),
    Done,
}

pub(crate) struct Task {
    /// Counts the tasks the slot has held.
    pub generation: u32,
    /// The task's object while it is unfinished (a GC root), else `#f`.
    pub handle: Value,
    /// The execution it is (`crate::stop`).
    pub exec: crate::stop::ExecId,
    pub stack: Stack,
    pub state: State,
    pub resume: Option<Suspend>,
    pub entry: Option<Root>,
    pub delivery: Option<Result<Root, Error>>,
    /// In the run queue.
    pub queued: bool,
    /// Counts the waits armed: lists to wake it from hold the count then.
    pub armed: u32,
    /// Tasks waiting for it to finish.
    pub joiners: Vec<Waiter>,
    /// What wakes it when a future it waits on is ready, made when it
    /// first waits on one.
    pub flag: Option<Arc<Flag>>,
}

/// Push `w` on a list of waiters, first dropping the stale entries once
/// they may be half of it.
fn register(list: &mut Vec<Waiter>, w: Waiter, tasks: &[Task]) {
    if list.len() >= 8 && list.len() == list.capacity() {
        list.retain(|w| armed(tasks, *w));
    }
    list.push(w);
}

/// Whether `w` is the wait its task is in.
fn armed(tasks: &[Task], w: Waiter) -> bool {
    let t = &tasks[w.task.slot];
    t.generation == w.task.generation && t.armed == w.armed && matches!(t.state, State::Waiting(_))
}

/// Registers a task's stack starts with.
pub(crate) const TASK_REGS: usize = 256;

/// The fields of a task's object: its slot and the slot's generation
/// while it is unfinished, `#f` and its result once it has finished: the
/// value or the condition it raised, and then the error (a foreign object,
/// without the condition).
pub(crate) const TASK_FIELDS: &[&str] = &["slot", "generation", "value", "error"];
const TASK_SLOT: usize = 1;
const TASK_GENERATION: usize = 2;
const TASK_VALUE: usize = 3;
const TASK_ERROR: usize = 4;

/// Handle to a task from Rust: its object, rooted, so its result stays
/// available while the handle lives.
#[derive(Clone, Debug)]
pub struct TaskId(Root);

impl TaskId {
    /// The task's Lisp object.
    pub fn value(&self) -> crate::api::Value {
        crate::api::Value::from_root(self.0.clone())
    }
}

impl Error {
    /// Raised in a task by `cancel_task`.
    pub fn is_cancellation(&self) -> bool {
        self.msg == CANCELLED
    }
}

impl Vm {
    /// The error a native returns to suspend the running task until `wait`
    /// is satisfied. Its box is reused (`spare_suspend`).
    pub(crate) fn suspend(&mut self, wait: Wait) -> Error {
        let mut e = self.spare_suspend.take().unwrap_or_else(|| Error::new(String::new()));
        e.wait = Some(wait);
        e
    }

    /// Create a task running the zero-argument procedure `f`.
    /// As before, admission is left to the caller; use `try_spawn` to admit
    /// its storage and return a catchable error. Panics on allocation failure.
    /// Spawn a task calling `f`. Panics if `f` is of another world, or if
    /// there is no room for the task (`try_spawn` returns those errors).
    pub fn spawn(&mut self, f: &crate::api::Value) -> TaskId {
        let f = self.import_value(f).expect("a procedure of this world");
        self.spawn_raw(f)
    }

    pub(crate) fn spawn_raw(&mut self, f: Value) -> TaskId {
        let t = self.spawn_task(f, false).expect("registers for the task");
        TaskId(self.root(t))
    }

    /// Admit and create a task, returning a catchable error if it cannot fit.
    pub fn try_spawn(&mut self, f: &crate::api::Value) -> Result<TaskId, Error> {
        let f = self.import_value(f)?;
        self.try_spawn_raw(f)
    }

    pub(crate) fn try_spawn_raw(&mut self, f: Value) -> Result<TaskId, Error> {
        let entry = self.root(f);
        self.admit(self.task_bytes())?;
        let t = self.spawn_task(entry.get(), false)?;
        Ok(TaskId(self.root(t)))
    }

    /// Create a task running `f`, a child of the running execution if
    /// `child`; returns its object.
    fn spawn_task(&mut self, f: Value, child: bool) -> Result<Value, Error> {
        // Allocate before consuming a free slot or publishing a task/execution.
        let mut stack = Stack::with_regs_in(TASK_REGS, self.regs.budget().clone())?;
        let entry = self.root(f);
        let slot = self.free_tasks.pop();
        let id = slot.unwrap_or(self.tasks.len());
        let generation = slot.map_or(0, |i| self.tasks[i].generation);
        let rtd = self.special(SpecialObj::TaskRtd);
        let handle =
            self.make_record(rtd, &[Value::int_unchecked(id as i64), Value::int_unchecked(generation as i64), Value::FALSE, Value::FALSE]);
        // New tasks inherit the spawner's parameters, output port and
        // current module.
        stack.locals = self.locals.clone();
        stack.module = self.current_module();
        let exec = self.stops.begin();
        let parent = self.running_execution();
        if child && parent != crate::stop::ExecId(0) {
            let tasks = &self.tasks;
            let children = self.children.entry(parent.0).or_default();
            // A long-lived parent spawning on: forget those finished
            // (`finish` moves a slot's generation on) before growing.
            if children.len() == children.capacity() {
                children.retain(|&(s, g)| tasks[s].generation == g);
                // Pruning must leave substantial room. Otherwise grow,
                // so one finished child cannot cause a scan per spawn.
                if children.len() > children.capacity() / 2 {
                    children.reserve(children.len());
                }
            }
            children.push((id, generation));
        }
        let task = Task {
            generation,
            handle,
            exec,
            stack,
            state: State::Runnable,
            resume: None,
            entry: Some(entry),
            delivery: None,
            queued: false,
            armed: 0,
            joiners: Vec::new(),
            flag: None,
        };
        match slot {
            Some(i) => self.tasks[i] = task,
            None => self.tasks.push(task),
        }
        self.live += 1;
        self.make_ready(id);
        Ok(handle)
    }

    /// The slot of the task whose object is `t` while it is unfinished,
    /// none once it has finished. An object of another generation than its
    /// slot's (not one the VM made) is an error.
    fn task_slot(&self, t: Value, who: &str) -> Result<Option<usize>, Error> {
        let p = t.as_ptr();
        let slot = unsafe { field(p, TASK_SLOT) };
        if !slot.is_int() {
            return Ok(None);
        }
        let generation = unsafe { field(p, TASK_GENERATION) };
        match usize::try_from(slot.as_int()).ok().and_then(|i| self.tasks.get(i).map(|task| (i, task))) {
            Some((i, task)) if generation.is_int() && generation.as_int() == task.generation as i64 => Ok(Some(i)),
            _ => Err(Error::new(format!("{who}: stale task"))),
        }
    }

    /// The result of the task whose object is `t`, once it has finished.
    fn task_outcome(&mut self, t: Value) -> Option<Result<Value, Error>> {
        let p = t.as_ptr();
        if unsafe { field(p, TASK_SLOT) }.is_int() {
            return None;
        }
        let (value, error) = unsafe { (field(p, TASK_VALUE), field(p, TASK_ERROR)) };
        if !error.is_truthy() {
            return Some(Ok(value));
        }
        let mut e = match self.foreign_raw(error).and_then(|(e, _)| e.downcast_ref::<Error>()) {
            Some(e) => e.duplicate(),
            None => Error::new("task failed"),
        };
        e.payload = Some(self.root(value));
        Some(Err(e))
    }

    /// The execution the task is, which `InterruptHandle::stop` reaches;
    /// none once it has finished.
    pub fn task_execution(&self, id: &TaskId) -> Option<crate::stop::ExecId> {
        let t = self.import_root(&id.0).ok()?;
        self.task_slot(t, "task-execution").ok().flatten().map(|i| self.tasks[i].exec)
    }

    /// The task's result once it has finished.
    pub fn task_result(&mut self, id: &TaskId) -> Option<Result<crate::api::Value, Error>> {
        let t = match self.import_root(&id.0) {
            Ok(t) => t,
            Err(e) => return Some(Err(e)),
        };
        Some(self.task_outcome(t)?.map(|v| self.export(v)))
    }

    /// Run until every task has finished (or all remaining ones are blocked
    /// forever, which is reported as a deadlock).
    ///
    /// The scheduler runs host code outside any native call (future
    /// polls, converters, destructors of satisfied waits): it is
    /// contained whole, so a panic in any of it poisons the world and
    /// is reported here instead of unwinding to the host.
    pub fn run_tasks(&mut self) -> Result<(), Error> {
        match self.catch_panic(|vm| vm.run_tasks_inner()) {
            Ok(result) => result,
            Err(poison) => Err(poison),
        }
    }

    fn run_tasks_inner(&mut self) -> Result<(), Error> {
        let _charged = self.enter_account();
        while self.live > 0 {
            if self.poisoned().is_some() {
                return Err(self.poison_error());
            }
            // The caller's stops: tasks may keep it busy for good.
            self.poll_interrupt()?;
            if !self.run_round(None) && self.live > 0 {
                self.park(None)?;
            }
        }
        if self.poisoned().is_some() {
            return Err(self.poison_error());
        }
        Ok(())
    }

    /// Run tasks for about `budget` without blocking the thread: rounds,
    /// each giving every task ready one time slice, until the budget is
    /// spent (checked between tasks). For hosts with their own event loop;
    /// see `next_timer` and `set_wake_notifier`. Contained like
    /// `run_tasks`; a poisoning panic ends it `Finished` (check
    /// `Vm::poisoned`).
    pub fn run_tasks_for(&mut self, budget: Duration) -> Progress {
        match self.catch_panic(|vm| vm.run_tasks_for_inner(budget)) {
            Ok(progress) => progress,
            Err(_) => Progress::Finished,
        }
    }

    fn run_tasks_for_inner(&mut self, budget: Duration) -> Progress {
        let _charged = self.enter_account();
        let deadline = Instant::now() + budget;
        loop {
            // A poisoned world runs nothing more; hosts must check
            // `Vm::poisoned` when this returns `Finished` with tasks
            // they expected to outlive the round.
            if self.live == 0 || self.poisoned().is_some() {
                return Progress::Finished;
            }
            if !self.run_round(Some(deadline)) && self.run_queue.is_empty() {
                return if self.live == 0 { Progress::Finished } else { Progress::Blocked };
            }
            // Poison before the deadline: `OutOfTime` must not mask it.
            if self.poisoned().is_some() {
                return Progress::Finished;
            }
            if Instant::now() >= deadline {
                return Progress::OutOfTime;
            }
        }
    }

    /// The earliest time a sleeping task wants to run (or a little
    /// earlier: one that stopped waiting may not be forgotten yet).
    pub fn next_timer(&self) -> Option<Instant> {
        self.timers.peek().map(|Reverse((d, _))| *d)
    }

    /// Call `notify` (from any thread) when a Rust future a task waits on
    /// becomes ready, so a host event loop can call `run_tasks_for` again.
    /// Applies to futures started afterwards and to interrupt handles
    /// created afterwards.
    pub fn set_wake_notifier(&mut self, notify: impl Fn() + Send + Sync + 'static) {
        self.wake_notifier = Some(Arc::new(notify));
    }

    /// Kill a task (`crate::stop`): it ends without running again, no Lisp
    /// cleanup of its runs, and a task joining it sees "task killed". A
    /// finished task is unaffected. Killing the running task returns the
    /// error that ends it.
    pub fn kill_task(&mut self, id: &TaskId) -> Result<(), Error> {
        let t = self.import_root(&id.0)?;
        match self.task_slot(t, "task-kill")? {
            Some(i) => self.kill_slot(i),
            None => Ok(()),
        }
    }

    fn kill_slot(&mut self, id: usize) -> Result<(), Error> {
        let exec = self.tasks[id].exec;
        // From this thread: nothing to wake.
        self.stops.request(exec, crate::stop::Stop::Kill);
        if self.running_execution() == exec {
            return Err(self.take_interrupt());
        }
        // One below what runs ends when it goes on.
        if !self.task_active(id) {
            self.end_killed(id);
        }
        Ok(())
    }

    /// Finish the killed task `id`, which is not running: its wait is
    /// abandoned and its stack dropped.
    fn end_killed(&mut self, id: usize) {
        let task = &mut self.tasks[id];
        task.entry = None;
        task.resume = None;
        task.delivery = None;
        if let State::Waiting(mut wait) = std::mem::replace(&mut task.state, State::Runnable) {
            self.abandon(&mut wait);
            self.drop_wait(wait);
        }
        self.finish(id, Err(Error::new(crate::stop::TASK_KILLED)));
    }

    /// Cancel a task: it sees the condition "task cancelled" where it is
    /// suspended, and its wait (a Rust future, too) is dropped. A task that
    /// has not started finishes at once; a finished task is unaffected.
    /// Cancelling the running task returns the condition to raise.
    pub fn cancel_task(&mut self, id: &TaskId) -> Result<(), Error> {
        let t = self.import_root(&id.0)?;
        match self.task_slot(t, "task-cancel")? {
            Some(i) => self.cancel_slot(i),
            None => Ok(()),
        }
    }

    fn cancel_slot(&mut self, id: usize) -> Result<(), Error> {
        if self.current_task == Some(id) {
            return Err(Error::new(CANCELLED));
        }
        let task = &mut self.tasks[id];
        if task.entry.take().is_some() {
            self.finish(id, Err(Error::new(CANCELLED)));
        } else {
            let state = std::mem::replace(&mut task.state, State::Runnable);
            task.delivery = Some(Err(Error::new(CANCELLED)));
            if let State::Waiting(mut wait) = state {
                self.abandon(&mut wait);
                self.drop_wait(wait);
            }
            self.make_ready(id);
        }
        Ok(())
    }

    /// Wait from a native: suspend the current task, or (outside tasks) run
    /// the scheduler until the wait is satisfied.
    pub(crate) fn wait_on(&mut self, wait: Wait) -> Result<Value, Error> {
        let mut wait = wait;
        // Every return drops the wait (a satisfied future, say) through
        // containment: `done` takes it out (a yield in its place) and
        // drops it first.
        macro_rules! done {
            ($result:expr) => {{
                let result = $result;
                self.drop_wait(std::mem::replace(&mut wait, Wait::Yield));
                // Poisoned by the future's own destructor: that replaces
                // even a satisfied result.
                if self.poisoned().is_some() {
                    return Err(self.poison_error());
                }
                return result;
            }};
        }
        if let Some(id) = self.current_task {
            // Done at once if it can be (a send into a buffer with room); a
            // select that must wait has its offers out before it suspends.
            if !matches!(wait, Wait::Yield)
                && let Some(result) = self.satisfy(&mut wait, Some(self.task_ref(id)))
            {
                done!(result);
            }
            return Err(self.suspend(wait));
        }
        loop {
            // Outside tasks: the top-level execution's stops arrive here.
            if let Err(e) = self.poll_interrupt() {
                self.abandon(&mut wait);
                done!(Err(e));
            }
            if let Some(result) = self.satisfy(&mut wait, None) {
                done!(result);
            }
            if !self.run_round(None) {
                if let Some(result) = self.satisfy(&mut wait, None) {
                    done!(result);
                }
                if let Err(e) = self.park(Some(&wait)) {
                    self.abandon(&mut wait);
                    done!(Err(e));
                }
            }
        }
    }

    /// Expose an async Rust function. `f` converts the arguments and returns a
    /// future; the calling task waits for it without blocking other tasks.
    /// What the future keeps of the arguments is converted (`Args::get`)
    /// or a `Value` (`Args::value`).
    pub fn register_async<F, Fut, T, E>(&mut self, name: &str, arity: usize, f: F)
    where
        F: for<'a> Fn(&mut Vm, crate::api::Args<'a>) -> Fut + 'static,
        Fut: Future<Output = Result<T, E>> + 'static,
        T: IntoValue + 'static,
        E: Display + 'static,
    {
        let native = Rc::new(move |vm: &mut Vm, args: usize, n: usize| {
            let args = crate::api::Args::new(vm, args, n);
            let fut = f(vm, args);
            let fut: BoxFuture = Box::pin(async move {
                match fut.await {
                    Ok(t) => Ok(Box::new(move |vm: &mut Vm| vm.convert_raw(t)) as Converter),
                    Err(e) => Err(Error::new(e.to_string())),
                }
            });
            let flag = vm.wait_flag();
            vm.wait_on(Wait::Future { fut, flag })
        });
        self.define_native(Native { name: name.into(), f: NativeImpl::Boxed(native), min: arity, max: Some(arity), doc: None });
    }

    /// What wakes the running task (or code outside tasks) when a future
    /// is ready, set to poll the future first.
    fn wait_flag(&mut self) -> Arc<Flag> {
        let task = self.current_task.map(|id| self.task_ref(id));
        if let Some(t) = task
            && let Some(flag) = &self.tasks[t.slot].flag
            && match (&flag.notify, &self.wake_notifier) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (a, b) => a.is_none() && b.is_none(),
            }
        {
            flag.pending.store(true, Ordering::SeqCst);
            return flag.clone();
        }
        let flag = Arc::new(Flag {
            pending: AtomicBool::new(true),
            task,
            wakes: self.wakes.clone(),
            thread: self.thread.clone(),
            notify: self.wake_notifier.clone(),
        });
        if let Some(t) = task {
            self.tasks[t.slot].flag = Some(flag.clone());
        }
        flag
    }

    fn task_ref(&self, slot: usize) -> TaskRef {
        TaskRef { slot, generation: self.tasks[slot].generation }
    }

    /// The result if `wait` is satisfied now. `owner` is the task waiting,
    /// none outside tasks.
    fn satisfy(&mut self, wait: &mut Wait, owner: Option<TaskRef>) -> Option<Result<Value, Error>> {
        match wait {
            Wait::Yield => Some(Ok(Value::VOID)),
            Wait::Sleep(deadline) => (Instant::now() >= *deadline).then_some(Ok(Value::VOID)),
            Wait::Select(s) => self.satisfy_select(s, owner),
            Wait::Join(t) => self.task_outcome(t.get()),
            Wait::Future { fut, flag } => {
                if !flag.pending.swap(false, Ordering::SeqCst) {
                    return None;
                }
                let waker = Waker::from(flag.clone());
                // Polling runs host code outside any native call (and
                // outside `as_execution`), and the converter it yields
                // runs host `IntoValue` code: contain both, so a panic
                // poisons the world and is delivered as its poison
                // error instead of unwinding out of the scheduler with
                // the waiting task left half-moved (`State::Waiting`
                // was already replaced by the caller).
                match self.catch_panic(|_| fut.as_mut().poll(&mut Context::from_waker(&waker))) {
                    Err(poison) => Some(Err(poison)),
                    Ok(Poll::Pending) => None,
                    Ok(Poll::Ready(Err(e))) => Some(Err(e)),
                    Ok(Poll::Ready(Ok(convert))) => Some(self.catch_native(convert)),
                }
            }
        }
    }

    /// The result if one of `s`'s operations can complete now; otherwise
    /// its sends are offered (once) and it keeps waiting.
    fn satisfy_select(&mut self, s: &mut Select, owner: Option<TaskRef>) -> Option<Result<Value, Error>> {
        // A receiver took one of our offers: that branch won.
        if let Some(g) = s.group
            && let Some(branch) = self.offers.groups.get(&g).and_then(|grp| grp.won)
        {
            self.offers.groups.remove(&g);
            s.group = None;
            return Some(Ok(self.select_result(s, branch, Value::VOID)));
        }
        let n = s.ops.len();
        let start = if n > 1 {
            self.offers.turn = self.offers.turn.wrapping_add(1);
            self.offers.turn % n
        } else {
            0
        };
        // Timeouts last.
        let order = (0..2 * n).map(|k| (k / n == 1, (start + k) % n)).filter(|&(late, i)| late == matches!(s.ops[i], Op::After(_)));
        for (_, i) in order {
            let outcome = match &s.ops[i] {
                Op::Recv(ch) => self.take(*ch, s.group).map(Ok),
                Op::Send { ch, value, size } => match self.channel(*ch) {
                    Some(c) if !c.closed => {
                        if c.offers.iter().all(|o| Some(o.group) == s.group) && c.has_room(*size) {
                            c.bytes += size;
                            c.buf.push_back((value.clone(), *size));
                            self.wake_all(ch.slot, false);
                            Some(Ok(Value::VOID))
                        } else {
                            None
                        }
                    }
                    _ => Some(Err(Error::new("channel-send: channel closed"))),
                },
                Op::After(deadline) => (Instant::now() >= *deadline).then_some(Ok(Value::VOID)),
            };
            if let Some(outcome) = outcome {
                self.withdraw(s);
                return Some(outcome.map(|v| self.select_result(s, i, v)));
            }
        }
        if s.group.is_none() && s.ops.iter().any(|op| matches!(op, Op::Send { .. })) {
            let g = self.offers.next;
            self.offers.next += 1;
            // Within what the select was admitted for.
            let mut channels = Vec::with_capacity(s.ops.len());
            for (branch, op) in s.ops.iter().enumerate() {
                if let Op::Send { ch, value, size } = op {
                    self.channels[ch.slot].offers.push_back(Offer { group: g, branch, value: value.clone(), size: *size });
                    channels.push(ch.slot);
                    self.wake_all(ch.slot, false);
                }
            }
            self.offers.groups.insert(g, Group { channels, won: None, task: owner });
            s.group = Some(g);
        }
        None
    }

    fn select_result(&mut self, s: &Select, branch: usize, v: Value) -> Value {
        if s.indexed { self.alloc_pair_raw(Value::int_unchecked(branch as i64), v) } else { v }
    }

    /// The channel `ch` unless it has been closed and drained.
    fn channel(&mut self, ch: ChannelId) -> Option<&mut Channel> {
        self.channels.get_mut(ch.slot).filter(|c| c.generation == ch.generation)
    }

    /// Free the slot of a closed and drained channel.
    fn free_channel(&mut self, slot: usize) {
        let c = &mut self.channels[slot];
        c.generation = c.generation.wrapping_add(1);
        (c.buf, c.offers) = (VecDeque::new(), VecDeque::new());
        (c.receivers, c.senders) = (Vec::new(), Vec::new());
        self.free_channels.push(slot);
    }

    /// Receive from `ch` if a value is there: the buffer first, refilled
    /// from waiting offers, then an offer directly (not one of `own`'s);
    /// the eof object once it is closed and empty.
    fn take(&mut self, id: ChannelId, own: Option<u64>) -> Option<Value> {
        let Some(c) = self.channel(id) else { return Some(Value::EOF) };
        let ch = id.slot;
        if let Some((v, size)) = c.buf.pop_front() {
            if c.closed && c.buf.is_empty() {
                // Closed channels have no offers.
                let v = v.get();
                self.free_channel(ch);
                return Some(v);
            }
            c.bytes -= size;
            // Room for a sender.
            self.wake_all(ch, true);
            while let Some(i) = self.channels[ch].offers.iter().position(|o| Some(o.group) != own) {
                if !self.channels[ch].has_room(self.channels[ch].offers[i].size) {
                    break;
                }
                let (value, size) = self.accept(ch, i);
                let c = &mut self.channels[ch];
                c.bytes += size;
                c.buf.push_back((value, size));
            }
            return Some(v.get());
        }
        if c.closed {
            return Some(Value::EOF);
        }
        let i = c.offers.iter().position(|o| Some(o.group) != own)?;
        Some(self.accept(ch, i).0.get())
    }

    /// Take offer `i` of `ch`: its group wins with that branch, and its
    /// other offers are withdrawn.
    fn accept(&mut self, ch: usize, i: usize) -> (Root, usize) {
        let offer = self.channels[ch].offers.remove(i).expect("offer");
        if let Some(group) = self.offers.groups.get_mut(&offer.group) {
            group.won = Some(offer.branch);
            let task = group.task;
            for c in std::mem::take(&mut group.channels) {
                self.channels[c].offers.retain(|o| o.group != offer.group);
                self.wake_all(c, true);
            }
            if let Some(t) = task {
                self.wake_task(t);
            }
        }
        (offer.value, offer.size)
    }

    /// Withdraw a select's offers (it stopped waiting).
    fn withdraw(&mut self, s: &mut Select) {
        if let Some(g) = s.group.take()
            && let Some(group) = self.offers.groups.remove(&g)
        {
            for c in group.channels {
                self.channels[c].offers.retain(|o| o.group != g);
                // Senders behind its offers may go.
                self.wake_all(c, true);
            }
        }
    }

    /// Clean up a wait that is abandoned (cancelled or interrupted).
    fn abandon(&mut self, wait: &mut Wait) {
        if let Wait::Select(s) = wait {
            self.withdraw(s);
        }
    }

    /// Drop a wait, containing a panicking destructor: a registered
    /// future's `Drop` is host code exactly as its `poll` is, and waits
    /// end in locals all over the scheduler (satisfied in `run_round`,
    /// abandoned by kill/cancel, left over at `wait_on` returns). Every
    /// such end goes through here; the scheduler's entries are also
    /// contained whole (`run_tasks`), for what a local missed.
    pub(crate) fn drop_wait(&mut self, wait: Wait) {
        if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(wait))) {
            self.poison_panic(panic.as_ref());
            crate::vm::dispose_panic_payload(panic);
        }
    }

    /// Put task `id` on the run queue, unless it is there.
    fn make_ready(&mut self, id: usize) {
        let t = &mut self.tasks[id];
        if t.queued {
            return;
        }
        t.queued = true;
        let generation = t.generation;
        // Entries of tasks that finished queued (cancelled, killed) go once
        // they may be half: each task has at most one entry of its own.
        if self.run_queue.len() >= 2 * self.live + 64 {
            let tasks = &self.tasks;
            self.run_queue.retain(|t| tasks[t.slot].generation == t.generation);
            self.run_queue.shrink_to(2 * self.run_queue.len() + 64);
        }
        self.run_queue.push_back(TaskRef { slot: id, generation });
    }

    /// Make `t` ready if it still waits.
    fn wake_task(&mut self, t: TaskRef) {
        let task = &self.tasks[t.slot];
        if task.generation == t.generation && matches!(task.state, State::Waiting(_)) {
            self.make_ready(t.slot);
        }
    }

    /// Make `w` ready if it still waits in the wait it was listed for.
    fn wake(&mut self, w: Waiter) {
        if armed(&self.tasks, w) {
            self.make_ready(w.task.slot);
        }
    }

    /// Make the tasks waiting to receive from channel `ch` ready, or those
    /// waiting to send: something changed that may let them go. Those
    /// that cannot go yet list themselves again.
    fn wake_all(&mut self, ch: usize, senders: bool) {
        let c = &mut self.channels[ch];
        let list = if senders { &mut c.senders } else { &mut c.receivers };
        if list.is_empty() {
            return;
        }
        let mut list = std::mem::take(list);
        for &w in &list {
            self.wake(w);
        }
        list.clear();
        let c = &mut self.channels[ch];
        *(if senders { &mut c.senders } else { &mut c.receivers }) = list;
    }

    /// Task `id` has started waiting: list it where what it waits for
    /// happens (a channel, a task, the timers), or make it ready if that
    /// has happened.
    fn arm(&mut self, id: usize) {
        let task = &mut self.tasks[id];
        task.armed = task.armed.wrapping_add(1);
        let w = Waiter { task: TaskRef { slot: id, generation: task.generation }, armed: task.armed };
        // Out meanwhile, to look at the other tasks.
        let state = std::mem::replace(&mut task.state, State::Runnable);
        let State::Waiting(wait) = &state else {
            self.tasks[id].state = state;
            return;
        };
        let mut ready = false;
        let mut join = None;
        match wait {
            Wait::Yield => ready = true,
            Wait::Sleep(_) | Wait::Select(_) => {
                if let Wait::Select(s) = wait {
                    for op in &s.ops {
                        let (Op::Recv(ch) | Op::Send { ch, .. }) = op else { continue };
                        match self.channels.get_mut(ch.slot).filter(|c| c.generation == ch.generation) {
                            Some(c) => register(if matches!(op, Op::Recv(_)) { &mut c.receivers } else { &mut c.senders }, w, &self.tasks),
                            // Closed and drained.
                            None => ready = true,
                        }
                    }
                }
                if let Some(d) = wait.deadline() {
                    // The stale entries go once they may be half.
                    if self.timers.len() >= 2 * self.live + 64 {
                        let tasks = &self.tasks;
                        self.timers.retain(|Reverse((_, w))| armed(tasks, *w));
                    }
                    self.timers.push(Reverse((d, w)));
                }
            }
            Wait::Join(t) => join = Some(t.get()),
            Wait::Future { flag, .. } => ready = flag.pending.load(Ordering::SeqCst),
        }
        if let Some(t) = join {
            match self.task_slot(t, "task-join") {
                Ok(Some(target)) => {
                    let mut list = std::mem::take(&mut self.tasks[target].joiners);
                    register(&mut list, w, &self.tasks);
                    self.tasks[target].joiners = list;
                }
                _ => ready = true,
            }
        }
        self.tasks[id].state = state;
        if ready {
            self.make_ready(id);
        }
    }

    /// Make the tasks futures woke ready.
    fn take_wakes(&mut self) {
        let mut woken = std::mem::take(&mut self.woken);
        std::mem::swap(&mut *self.wakes.0.lock().unwrap_or_else(|e| e.into_inner()), &mut woken);
        for &t in &woken {
            self.wake_task(t);
        }
        woken.clear();
        self.woken = woken;
    }

    /// Make the tasks whose timers expired ready, and drop stale timers
    /// from the top.
    fn expire_timers(&mut self) {
        if self.timers.is_empty() {
            return;
        }
        let now = Instant::now();
        while let Some(&Reverse((d, w))) = self.timers.peek() {
            if d > now && armed(&self.tasks, w) {
                break;
            }
            self.timers.pop();
            self.wake(w);
        }
    }

    /// Deliver the stops pending for tasks that are not running: a killed
    /// task ends without running, one with a break or out of memory sees
    /// "interrupted" or "out of memory" where it waits.
    fn deliver_stops(&mut self) {
        for id in 0..self.tasks.len() {
            if matches!(self.tasks[id].state, State::Done) || self.task_active(id) {
                continue;
            }
            match self.stops.pending(self.tasks[id].exec) {
                Some(crate::stop::Stop::Kill) => self.end_killed(id),
                Some(stop) if stop.catchable() && self.tasks[id].entry.is_none() => {
                    self.stops.take_catchable(self.tasks[id].exec);
                    if let State::Waiting(mut wait) = std::mem::replace(&mut self.tasks[id].state, State::Runnable) {
                        self.abandon(&mut wait);
                    }
                    let e = if stop == crate::stop::Stop::Break { Error::new(crate::stop::INTERRUPTED) } else { self.out_of_memory() };
                    self.tasks[id].delivery = Some(Err(e));
                    self.make_ready(id);
                }
                _ => {}
            }
        }
    }

    /// Run the tasks ready for one slice each, in the order they became
    /// ready, or until `deadline`; those ready meanwhile run in the next
    /// round. Returns whether any task ran. A task with a stop pending is
    /// killed without running, or sees "interrupted" where it waits.
    fn run_round(&mut self, deadline: Option<Instant>) -> bool {
        self.take_wakes();
        self.expire_timers();
        if self.stops.any_pending() {
            self.deliver_stops();
        }
        let mut ran = false;
        for _ in 0..self.run_queue.len() {
            if ran && deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
            let Some(t) = self.run_queue.pop_front() else { break };
            let id = t.slot;
            if self.tasks[id].generation != t.generation {
                continue;
            }
            self.tasks[id].queued = false;
            // One active is not waiting: a wake for it is seen when it
            // waits again (`arm`).
            if matches!(self.tasks[id].state, State::Done) || self.task_active(id) {
                continue;
            }
            if let State::Waiting(_) = self.tasks[id].state {
                let State::Waiting(mut wait) = std::mem::replace(&mut self.tasks[id].state, State::Runnable) else { unreachable!() };
                match self.satisfy(&mut wait, Some(t)) {
                    Some(result) => {
                        self.tasks[id].delivery = Some(result.map(|v| self.root(v)));
                        // The satisfied future ends here, contained.
                        self.drop_wait(wait);
                    }
                    None => {
                        self.tasks[id].state = State::Waiting(wait);
                        self.arm(id);
                        continue;
                    }
                }
            }
            // Satisfying or dropping the wait may have poisoned the
            // world: the task does not resume, not even for a slice.
            if self.poisoned().is_some() {
                break;
            }
            self.run_task(id);
            ran = true;
            // A task's native panicked and poisoned the world: no other
            // task runs on it, not even in this round.
            if self.poisoned().is_some() {
                break;
            }
        }
        ran
    }

    fn run_task(&mut self, id: usize) {
        let outer_task = self.current_task.replace(id);
        self.active.push((self.tasks[id].exec, Some(id)));
        self.stops.run(self.tasks[id].exec);
        self.swap_task_stack(id);
        let result = match self.tasks[id].entry.take() {
            Some(entry) => self.start_task(entry.get()),
            None => {
                let resume = self.tasks[id].resume.take().expect("suspended task has a resume point");
                let delivery = self.tasks[id].delivery.take().map(|d| d.map(|r| r.get()));
                self.resume_task(resume, delivery)
            }
        };
        // Before switching away: what it grew is the task's, refused to it
        // when it ends well too.
        let result = match result {
            Ok(Exit::Done(v)) => self.growth_stop(self.tasks[id].exec).map_or(Ok(Exit::Done(v)), Err),
            other => other,
        };
        let killed = self.killing();
        if result.is_err() {
            // The task dies: run its `dynamic-wind` cleanups while its stack
            // is still in place (none when it is killed).
            self.unwind_to(0);
        }
        // What it or its cleanups grew is the task's: killed for it too.
        self.look_at_growth();
        let killed = killed || self.killing();
        self.swap_task_stack(id);
        self.current_task = outer_task;
        self.active.pop();
        self.stops.run(self.running_execution());
        match result {
            Ok(Exit::Done(v)) => {
                let v = self.root(v);
                self.finish(id, Ok(v));
            }
            Ok(Exit::Suspend(mut s)) => {
                let wait = s.wait.take();
                self.tasks[id].resume = Some(s);
                match wait {
                    None => self.make_ready(id),
                    Some(Wait::Yield) => {
                        let void = self.root(Value::VOID);
                        self.tasks[id].delivery = Some(Ok(void));
                        self.make_ready(id);
                    }
                    Some(w) => {
                        self.tasks[id].state = State::Waiting(w);
                        self.arm(id);
                    }
                }
            }
            Err(mut e) => {
                if e.wait.is_some() {
                    e = Error::new("cannot suspend in a task's native entry procedure");
                }
                if e.is_kill() || killed {
                    // Joiners carry on: the kill is not theirs.
                    e = Error::new(crate::stop::TASK_KILLED);
                }
                self.finish(id, Err(e));
            }
        }
    }

    fn finish(&mut self, id: usize, mut result: Result<Root, Error>) {
        // Joiners re-raise the task's condition as is.
        if let Err(e) = &mut result
            && e.payload.is_none()
        {
            let (msg, kind) = (e.msg.clone(), e.kind);
            // Of its kind, so that `file-error?` and the like hold for a
            // joiner too.
            let obj = self.make_error_object_of_raw(&msg, &[], kind);
            e.payload = Some(self.root(obj));
        }
        // The result goes into the task's object, and the slot is free.
        let (value, error) = match result {
            Ok(v) => (v, Value::FALSE),
            Err(mut e) => {
                let payload = e.payload.take().expect("set above");
                (e.escape, e.wait) = (None, None);
                (payload, self.make_foreign_raw(Rc::new(e), "task-error"))
            }
        };
        let task = &mut self.tasks[id];
        let h = task.handle.as_ptr();
        unsafe {
            set_field(h, TASK_SLOT, Value::FALSE);
            set_field(h, TASK_VALUE, value.get());
            set_field(h, TASK_ERROR, error);
        }
        task.handle = Value::FALSE;
        task.generation = task.generation.wrapping_add(1);
        task.state = State::Done;
        task.stack = Stack::new_in(task.stack.regs.budget().clone());
        (task.entry, task.resume, task.delivery, task.flag) = (None, None, None, None);
        let joiners = std::mem::take(&mut task.joiners);
        let exec = task.exec;
        self.write_barrier(h, value.get());
        self.write_barrier(h, error);
        let killed = self.stops.end(exec) == Some(crate::stop::Stop::Kill);
        self.live -= 1;
        self.free_tasks.push(id);
        if self.live == 0 {
            // What is left in the run queue is stale.
            self.run_queue.clear();
            self.run_queue.shrink_to(64);
        }
        for w in joiners {
            self.wake(w);
        }
        self.execution_ended(exec, killed);
    }

    /// The execution `exec` ended: if killed, its unfinished children are
    /// killed (`spawn #:child`), and theirs as they end, from a list of
    /// those killed rather than by recursion, which a deep line of
    /// children would overflow.
    pub(crate) fn execution_ended(&mut self, exec: crate::stop::ExecId, killed: bool) {
        if !killed {
            self.children.remove(&exec.0);
            return;
        }
        self.killed.push_back(exec);
        if self.killed.len() > 1 {
            // An outer call goes through the list.
            return;
        }
        while let Some(&parent) = self.killed.front() {
            for (slot, generation) in self.children.remove(&parent.0).unwrap_or_default() {
                let t = &self.tasks[slot];
                if t.generation == generation && !matches!(t.state, State::Done) {
                    // One running ends at its next check, one below what
                    // runs when it goes on.
                    let _ = self.kill_slot(slot);
                }
            }
            self.killed.pop_front();
        }
    }

    /// Block the thread until a timer expires or a future is woken, unless
    /// a task is ready. Errors if nothing could ever wake up (every task
    /// waits on channels or joins).
    fn park(&mut self, extra: Option<&Wait>) -> Result<(), Error> {
        self.poll_interrupt()?;
        // First: a timer expiring since the round makes its task ready.
        self.expire_timers();
        if !self.run_queue.is_empty() || !self.wakes.0.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
            return Ok(());
        }
        if let Some(Wait::Future { flag, .. }) = extra
            && flag.pending.load(Ordering::SeqCst)
        {
            return Ok(());
        }
        let deadline = extra.and_then(Wait::deadline).into_iter().chain(self.next_timer()).min();
        let futures =
            matches!(extra, Some(Wait::Future { .. })) || self.tasks.iter().any(|t| matches!(t.state, State::Waiting(Wait::Future { .. })));
        match deadline {
            Some(d) => std::thread::park_timeout(d.saturating_duration_since(Instant::now())),
            None if futures => std::thread::park(),
            None => return Err(Error::new("deadlock: every task is waiting on a channel or another task")),
        }
        Ok(())
    }
}

fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

fn is_record(vm: &Vm, v: Value, rtd: SpecialObj) -> bool {
    is_kind(v, Kind::Record) && unsafe { field(v.as_ptr(), 0) } == vm.special(rtd)
}

/// The task object argument `i`.
fn task_arg(vm: &Vm, args: usize, i: usize, who: &str) -> Result<Value, Error> {
    let v = arg(vm, args, i);
    if is_record(vm, v, SpecialObj::TaskRtd) { Ok(v) } else { Err(type_error(who, "task", v)) }
}

/// The slot of task argument `i` while the task is unfinished.
fn task_slot_arg(vm: &Vm, args: usize, i: usize, who: &str) -> Result<Option<usize>, Error> {
    let t = task_arg(vm, args, i, who)?;
    vm.task_slot(t, who)
}

fn spawn(vm: &mut Vm, args: usize, n: usize) -> Result<Value, Error> {
    vm.admit(vm.task_bytes())?;
    let child = n > 1 && arg(vm, args, 1).is_truthy();
    vm.spawn_task(arg(vm, args, 0), child)
}

fn join(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let t = task_arg(vm, args, 0, "task-join")?;
    vm.task_slot(t, "task-join")?;
    let t = vm.root(t);
    vm.wait_on(Wait::Join(t))
}

fn sleep(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let at = deadline(vm, arg(vm, args, 0), "sleep")?;
    vm.wait_on(Wait::Sleep(at))
}

/// The instant `ms` milliseconds from now (none for a negative or NaN `ms`);
/// an error for one too far away to represent, such as `+inf.0`.
fn deadline(vm: &mut Vm, ms: Value, who: &str) -> Result<Instant, Error> {
    let millis: f64 = vm.get_raw(ms)?;
    Duration::try_from_secs_f64(millis.max(0.0) / 1000.0)
        .ok()
        .and_then(|d| Instant::now().checked_add(d))
        .ok_or_else(|| Error::new(format!("{who}: duration out of range: {}", crate::builtins::repr(ms))))
}

/// `(%make-channel capacity max-bytes-or-#f)`.
fn make_channel(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let capacity: i64 = vm.get_raw(arg(vm, args, 0))?;
    let max_bytes = arg(vm, args, 1);
    let max_bytes = if max_bytes.is_truthy() { Some(vm.get_raw::<i64>(max_bytes)?) } else { None };
    if capacity < 0 || max_bytes.is_some_and(|b| b < 0) {
        return Err(Error::new("make-channel: negative capacity"));
    }
    vm.admit(vm.channel_bytes())?;
    let slot = vm.free_channels.pop();
    let generation = slot.map_or(0, |i| vm.channels[i].generation);
    let channel = Channel {
        generation,
        buf: VecDeque::new(),
        bytes: 0,
        capacity: capacity as usize,
        max_bytes: max_bytes.map(|b| b as usize),
        closed: false,
        offers: VecDeque::new(),
        receivers: Vec::new(),
        senders: Vec::new(),
    };
    let id = match slot {
        Some(i) => {
            vm.channels[i] = channel;
            i
        }
        None => {
            vm.channels.push(channel);
            vm.channels.len() - 1
        }
    };
    let rtd = vm.special(SpecialObj::ChannelRtd);
    Ok(vm.make_record(rtd, &[Value::int_unchecked(id as i64), Value::int_unchecked(generation as i64)]))
}

fn channel_arg(vm: &Vm, v: Value, who: &str) -> Result<ChannelId, Error> {
    if is_record(vm, v, SpecialObj::ChannelRtd) {
        let (slot, generation) = unsafe { (field(v.as_ptr(), 1), field(v.as_ptr(), 2)) };
        if slot.is_int() && generation.is_int() && (slot.as_int() as u64) < vm.channels.len() as u64 {
            return Ok(ChannelId { slot: slot.as_int() as usize, generation: generation.as_int() as u32 });
        }
    }
    Err(type_error(who, "channel", v))
}

fn channel_send(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let ch = channel_arg(vm, arg(vm, args, 0), "channel-send")?;
    let v = arg(vm, args, 1);
    let size = message_size(v);
    let value = vm.root(v);
    vm.wait_on(Wait::Select(Box::new(Select { ops: vec![Op::Send { ch, value, size }], group: None, indexed: false })))
}

fn channel_recv(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let ch = channel_arg(vm, arg(vm, args, 0), "channel-recv")?;
    vm.wait_on(Wait::Select(Box::new(Select { ops: vec![Op::Recv(ch)], group: None, indexed: false })))
}

/// `(%select ops)`: each op is `(recv ch . _)`, `(send ch value . _)` or
/// `(timeout ms . _)`; returns `(index . value)` for the one that happened.
/// What a select takes per operation: the operation, and its channel in
/// its offers' group.
const SELECT_OP_BYTES: usize = std::mem::size_of::<Op>() + std::mem::size_of::<usize>();

fn select(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let n = crate::builtins::list_values(arg(vm, args, 0)).map_or(0, |specs| specs.len());
    vm.admit_items(n, SELECT_OP_BYTES, 0)?;
    let list = arg(vm, args, 0);
    let specs = crate::builtins::list_values(list).ok_or_else(|| type_error("select", "list", list))?;
    let mut ops = Vec::with_capacity(specs.len());
    for spec in specs {
        let parts = crate::builtins::list_values(spec).filter(|p| p.len() >= 2).ok_or_else(|| type_error("select", "operation", spec))?;
        let kind = if parts[0].is_symbol() { crate::reader::symbol_name(parts[0].as_symbol()) } else { "".into() };
        ops.push(match (&*kind, parts.len()) {
            ("recv", _) => Op::Recv(channel_arg(vm, parts[1], "select")?),
            ("send", 3..) => Op::Send { ch: channel_arg(vm, parts[1], "select")?, size: message_size(parts[2]), value: vm.root(parts[2]) },
            ("timeout", _) => Op::After(deadline(vm, parts[1], "select")?),
            _ => return Err(type_error("select", "operation", spec)),
        });
    }
    if ops.is_empty() {
        return Err(Error::new("select: no operations"));
    }
    vm.wait_on(Wait::Select(Box::new(Select { ops, group: None, indexed: true })))
}

fn channel_close(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let id = channel_arg(vm, arg(vm, args, 0), "channel-close")?;
    let Some(c) = vm.channel(id) else { return Ok(Value::VOID) };
    c.closed = true;
    // Waiting senders fail: their next check sees the channel closed.
    let groups: Vec<u64> = c.offers.drain(..).map(|o| o.group).collect();
    let drained = c.buf.is_empty();
    for g in groups {
        if let Some(group) = vm.offers.groups.get_mut(&g) {
            group.channels.retain(|c| *c != id.slot);
        }
    }
    vm.wake_all(id.slot, false);
    vm.wake_all(id.slot, true);
    if drained {
        vm.free_channel(id.slot);
    }
    Ok(Value::VOID)
}

fn task_done(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    Ok(Value::bool(task_slot_arg(vm, args, 0, "task-done?")?.is_none()))
}

fn current_task(vm: &mut Vm, _: usize, _: usize) -> Result<Value, Error> {
    Ok(vm.current_task.map_or(Value::FALSE, |id| vm.tasks[id].handle))
}

pub fn install(vm: &mut Vm) {
    crate::natives! { vm;
        /// Start a task running THUNK, a procedure of no arguments; return it.
        /// The task runs when the current one waits or is preempted. With
        /// CHILD true, killing what runs now kills it too.
        "(spawn thunk [child])" => spawn;
        /// Wait for TASK to finish and return its value.
        /// If it raised, raise the same condition here.
        "(task-join task)" => join;
        /// Return #t if TASK has finished, whether with a value or an error.
        "(task-done? task)" => task_done;
        /// Cancel TASK, unless it has finished.
        /// It raises "task cancelled" where it waits, so its cleanups run.
        "(task-cancel task)" => |vm: &mut Vm, args, _| {
            if let Some(id) = task_slot_arg(vm, args, 0, "task-cancel")? {
                vm.cancel_slot(id)?;
            }
            Ok(Value::VOID)
        };
        /// Kill TASK, unless it has finished.
        /// It ends without running again and runs no cleanup; a task
        /// joining it raises "task killed".
        "(task-kill task)" => |vm: &mut Vm, args, _| {
            if let Some(id) = task_slot_arg(vm, args, 0, "task-kill")? {
                vm.kill_slot(id)?;
            }
            Ok(Value::VOID)
        };
        /// Return the task running this code, or #f outside tasks.
        "(current-task)" => current_task;
        /// Let the other tasks that are ready run before this one goes on.
        "(yield)" => |vm: &mut Vm, _, _| vm.wait_on(Wait::Yield);
        /// Suspend this task for MS milliseconds; the others run meanwhile.
        "(sleep ms)" => sleep;
        "(%make-channel capacity max-bytes)" => make_channel;
        /// Send VALUE on CHANNEL, waiting while it is full.
        /// Raise an error if the channel is closed.
        "(channel-send channel value)" => channel_send;
        /// Receive the next value from CHANNEL, waiting for one.
        /// Once it is closed and empty, return the eof object.
        "(channel-recv channel)" => channel_recv;
        /// Close CHANNEL.
        /// Later and waiting sends fail; receivers get what was sent
        /// before, then the eof object.
        "(channel-close channel)" => channel_close;
        /// Return #t if CHANNEL has been closed.
        "(channel-closed? channel)" => |vm: &mut Vm, args, _| { let ch = channel_arg(vm, arg(vm, args, 0), "channel-closed?")?; Ok(Value::bool(vm.channel(ch).is_none_or(|c| c.closed))) };
        /// Return how many values CHANNEL holds, sent but not yet received.
        "(channel-length channel)" => |vm: &mut Vm, args, _| { let ch = channel_arg(vm, arg(vm, args, 0), "channel-length")?; Ok(Value::int_unchecked(vm.channel(ch).map_or(0, |c| c.buf.len()) as i64)) };
        "(%select ops)" => select;
        "(%live-task-count)" => |vm: &mut Vm, _, _| Ok(Value::int_unchecked(vm.live as i64));
        /// Run the tasks until none can go on.
        "(run-tasks)" => |vm: &mut Vm, _, _| { vm.run_tasks()?; Ok(Value::VOID) };
    }
}

#[cfg(test)]
mod tests {
    use crate::vm::Vm;

    #[test]
    fn refused_stack_does_not_consume_a_task_slot() {
        let mut vm = Vm::new();
        vm.set_jit(None);
        let f = vm.eval_source_raw("(lambda () #t)").unwrap();
        let f = vm.root(f);
        let task = vm.try_spawn_raw(f.get()).unwrap();
        vm.run_tasks().unwrap();
        assert!(vm.task_result(&task).unwrap().is_ok());
        let budget = vm.regs.budget().clone();
        let before = (vm.tasks.len(), vm.free_tasks.clone(), vm.live, vm.run_queue.len(), budget.held());
        budget.set_limit(budget.held());
        assert!(vm.try_spawn_raw(f.get()).unwrap_err().msg.starts_with("out of memory"));
        assert_eq!((vm.tasks.len(), vm.free_tasks.clone(), vm.live, vm.run_queue.len(), budget.held()), before);
        budget.set_limit(usize::MAX);
        vm.try_spawn_raw(f.get()).unwrap();
        assert_eq!(vm.tasks.len(), before.0);
        vm.run_tasks().unwrap();
    }

    #[test]
    fn task_registers_are_released_on_completion_and_cancellation() {
        let mut vm = Vm::new();
        vm.set_jit(None);
        let budget = vm.regs.budget().clone();
        for cancel in [false, true] {
            let source = if cancel { "(lambda () (sleep 3600) #t)" } else { "(lambda () (yield) #t)" };
            let f = vm.eval_source_raw(source).unwrap();
            let f = vm.root(f);
            let held = budget.held();
            let task = vm.try_spawn_raw(f.get()).unwrap();
            assert_eq!(budget.held(), held + super::TASK_REGS * size_of::<super::Value>());
            if cancel {
                assert_eq!(vm.run_tasks_for(super::Duration::from_secs(1)), super::Progress::Blocked);
                vm.cancel_task(&task).unwrap();
            }
            vm.run_tasks().unwrap();
            assert_eq!(budget.held(), held);
            assert_eq!(vm.live, 0);
            assert!(vm.task_result(&task).is_some());
        }
    }

    #[test]
    fn scheme_can_catch_a_task_stack_allocation_failure() {
        let mut vm = Vm::new();
        vm.set_jit(None);
        vm.register_fn_vm("no-stack-room", |vm: &mut Vm| {
            vm.regs.budget().set_limit(vm.regs.budget().held());
        });
        let value = vm.eval_source_raw("(guard (e (else #t)) (no-stack-room) (spawn (lambda () #f)) #f)").unwrap();
        assert_eq!(value, super::Value::TRUE);
        assert_eq!(vm.live, 0);
        vm.regs.budget().set_limit(usize::MAX);
    }

    /// Finished tasks and closed, drained channels give their slots back.
    #[test]
    fn slots_are_reused() {
        let mut vm = Vm::new();
        let v = vm
            .eval_source_raw(
                "(let loop ((i 0) (acc 0))
                   (if (= i 100000)
                       acc
                       (let ((c (make-channel 1)) (t (spawn (lambda () i))))
                         (channel-send c (task-join t))
                         (channel-close c)
                         (loop (+ i 1) (+ acc (channel-recv c))))))",
            )
            .unwrap();
        assert_eq!(v.as_int(), 4_999_950_000);
        assert!(vm.tasks.len() <= 2 && vm.channels.len() <= 2, "{} task slots, {} channel slots", vm.tasks.len(), vm.channels.len());
    }

    /// Near a capacity boundary, short-lived children must not make each
    /// spawn scan every live sibling. Count scanned entries, not time.
    #[test]
    fn child_pruning_is_amortized() {
        for capacity in [64, 128, 256] {
            let mut vm = Vm::new();
            let f = vm.eval_source_raw("(lambda () #t)").unwrap();
            let f = vm.root(f);
            let parent = vm.new_execution();
            vm.enter_execution(parent);
            vm.children.insert(parent.0, Vec::with_capacity(capacity));
            for _ in 0..capacity - 1 {
                vm.spawn_task(f.get(), true).unwrap();
            }
            let mut scanned = 0;
            let churn = 1024;
            for _ in 0..churn {
                let children = &vm.children[&parent.0];
                if children.len() == children.capacity() {
                    scanned += children.len();
                }
                vm.spawn_task(f.get(), true).unwrap();
                let value = vm.root(super::Value::VOID);
                vm.finish(vm.tasks.len() - 1, Ok(value));
            }
            assert!(scanned <= 4 * (capacity + churn), "capacity {capacity}: scanned {scanned} entries");
            vm.leave_execution(parent);
        }
    }

    /// A select satisfied over and over by one channel leaves neither its
    /// timeouts nor its entries on the quiet channel behind.
    #[test]
    fn stale_waits_are_dropped() {
        let mut vm = Vm::new();
        let v = vm
            .eval_source_raw(
                "(define busy (make-channel)) (define quiet (make-channel))
                 (spawn (lambda () (do ((i 0 (+ i 1))) ((= i 20000)) (channel-send busy i))))
                 (task-join (spawn (lambda ()
                   (let loop ((n 0))
                     (if (= n 20000) n
                         (select (recv busy (v) (loop (+ n 1))) (recv quiet (v) 'quiet) (timeout 100000 'late)))))))",
            )
            .unwrap();
        assert_eq!(v.as_int(), 20000);
        let quiet = vm.channels.iter().map(|c| c.receivers.len()).max().unwrap();
        assert!(vm.timers.len() < 100 && quiet < 100, "{} timers, {quiet} receivers", vm.timers.len());
    }

    /// A timer expiring after the round, before the scheduler parks, makes
    /// its task ready: parking returns at once, though another task waits on
    /// a future and no timer is left.
    #[test]
    fn a_timer_expiring_before_parking_is_seen() {
        let mut vm = Vm::new();
        vm.register_async("never", 0, |_, _| std::future::pending::<Result<i64, String>>());
        vm.eval_source_raw("(spawn (lambda () (never))) (spawn (lambda () (sleep 20)))").unwrap();
        vm.run_round(None);
        assert!(vm.run_queue.is_empty() && vm.timers.len() == 1);
        std::thread::sleep(std::time::Duration::from_millis(30));
        // A missed timer would park for good: wake the thread after a while.
        let thread = vm.thread.clone();
        let rescue = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            thread.unpark();
        });
        let start = std::time::Instant::now();
        vm.park(None).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_millis(250), "parked {:?}", start.elapsed());
        assert_eq!(vm.run_queue.len(), 1);
        rescue.join().unwrap();
    }

    /// Tasks cancelled before they run leave nothing in the run queue.
    #[test]
    fn cancelled_tasks_leave_no_queue_behind() {
        let mut vm = Vm::new();
        vm.eval_source_raw("(define (churn n) (when (> n 0) (task-cancel (spawn (lambda () n))) (churn (- n 1))))").unwrap();
        vm.eval_source_raw("(define keep (spawn (lambda () (sleep 100000))))").unwrap();
        for _ in 0..10 {
            vm.eval_source_raw("(churn 10000)").unwrap();
            vm.run_tasks_for(std::time::Duration::from_millis(1));
            assert!(vm.run_queue.len() < 100 && vm.run_queue.capacity() < 1000, "{} queued", vm.run_queue.len());
        }
        vm.eval_source_raw("(task-cancel keep) (guard (e (#t #f)) (task-join keep)) (churn 10000)").unwrap();
        assert!(vm.run_queue.is_empty() && vm.run_queue.capacity() < 1000, "{} queued", vm.run_queue.len());
    }
}
