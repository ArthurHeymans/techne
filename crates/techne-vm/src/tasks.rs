//! Cooperative, preemptible tasks.
//!
//! Each task owns a register/frame/handler stack; running a task swaps it in.
//! A task is preempted after `TASK_SLICE` calls or backward jumps, and
//! suspends when a native it calls must wait: `sleep`, `channel-recv`,
//! `channel-send`, `select`, `task-join`, `yield`, or a Rust future
//! registered with `register_async`.
//! The waiting native's call site receives the result when the task resumes.
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
//!     let x: i64 = vm.get(args[0]).unwrap();
//!     async move { Ok::<_, String>(x * 2) }
//! });
//! let v = vm.eval_source("(task-join (spawn (lambda () (+ 1 (slow-double 20)))))").unwrap();
//! assert_eq!(v.as_int(), 41);
//! ```

use std::{
    fmt::Display,
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    thread::Thread,
    time::{Duration, Instant},
};

use std::collections::VecDeque;

use rustc_hash::FxHashMap;

use crate::{
    api::{IntoValue, Root},
    builtins::type_error,
    heap::{Kind, field, is_kind, set_field},
    value::Value,
    vm::{Error, Exit, Native, NativeImpl, SpecialObj, Stack, Suspend, Vm},
};

/// Turns a finished Rust future's output into a Scheme value on the VM thread.
pub type Converter = Box<dyn FnOnce(&mut Vm) -> Result<Value, Error>>;
pub type BoxFuture = Pin<Box<dyn Future<Output = Result<Converter, Error>>>>;

/// Wakes the scheduler thread (and the host) when a future can make progress.
pub struct Flag {
    woken: AtomicBool,
    thread: Thread,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Flag {
    fn new(vm: &Vm) -> Arc<Flag> {
        // Starts woken so the future is polled once immediately.
        Arc::new(Flag { woken: AtomicBool::new(true), thread: vm.thread.clone(), notify: vm.wake_notifier.clone() })
    }
}

impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.woken.store(true, Ordering::SeqCst);
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
pub enum Wait {
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

/// A channel's slot in `Vm::channels`, and the slot's generation then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelId {
    slot: usize,
    generation: u32,
}

/// A bounded channel.
pub struct Channel {
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
}

struct Offer {
    group: u64,
    branch: usize,
    value: Root,
    size: usize,
}

/// The groups of waiting sends and selects with offers out.
#[derive(Default)]
pub struct Offers {
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
}

/// One operation of a select.
pub enum Op {
    Recv(ChannelId),
    Send { ch: ChannelId, value: Root, size: usize },
    After(Instant),
}

/// A wait for the first ready operation: `channel-recv`, `channel-send` and
/// `select`.
pub struct Select {
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
    pub fn value(&self) -> Value {
        self.0.get()
    }
}

impl Error {
    /// Raised in a task by `cancel_task`.
    pub fn is_cancellation(&self) -> bool {
        self.msg == CANCELLED
    }

    pub fn suspend(wait: Wait) -> Error {
        let mut e = Error::new("task suspended");
        e.wait = Some(wait);
        e
    }
}

impl Vm {
    /// Create a task running the zero-argument procedure `f`.
    pub fn spawn(&mut self, f: Value) -> TaskId {
        let t = self.spawn_task(f);
        TaskId(self.root(t))
    }

    /// Create a task running `f`; returns its object.
    fn spawn_task(&mut self, f: Value) -> Value {
        let entry = self.root(f);
        let slot = self.free_tasks.pop();
        let id = slot.unwrap_or(self.tasks.len());
        let generation = slot.map_or(0, |i| self.tasks[i].generation);
        let rtd = self.special(SpecialObj::TaskRtd);
        let handle =
            self.make_record(rtd, &[Value::int_unchecked(id as i64), Value::int_unchecked(generation as i64), Value::FALSE, Value::FALSE]);
        let mut stack = Stack::with_regs(TASK_REGS);
        // New tasks inherit the spawner's parameters and output port.
        stack.locals = self.locals.clone();
        let exec = self.stops.begin();
        let task = Task { generation, handle, exec, stack, state: State::Runnable, resume: None, entry: Some(entry), delivery: None };
        match slot {
            Some(i) => self.tasks[i] = task,
            None => self.tasks.push(task),
        }
        self.live_tasks.push(id);
        handle
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
        let mut e = match self.foreign(error).and_then(|(e, _)| e.downcast_ref::<Error>()) {
            Some(e) => e.duplicate(),
            None => Error::new("task failed"),
        };
        e.payload = Some(self.root(value));
        Some(Err(e))
    }

    /// The execution the task is, which `InterruptHandle::stop` reaches;
    /// none once it has finished.
    pub fn task_execution(&self, id: &TaskId) -> Option<crate::stop::ExecId> {
        self.task_slot(id.value(), "task-execution").ok().flatten().map(|i| self.tasks[i].exec)
    }

    /// The task's result once it has finished.
    pub fn task_result(&mut self, id: &TaskId) -> Option<Result<Value, Error>> {
        self.task_outcome(id.value())
    }

    /// Run until every task has finished (or all remaining ones are blocked
    /// forever, which is reported as a deadlock).
    pub fn run_tasks(&mut self) -> Result<(), Error> {
        let _charged = self.enter_account();
        while !self.live_tasks.is_empty() {
            // The caller's stops: tasks may keep it busy for good.
            self.poll_interrupt()?;
            if !self.run_round(None) && !self.live_tasks.is_empty() {
                self.park(None)?;
            }
        }
        Ok(())
    }

    /// Run tasks for about `budget` without blocking the thread: rounds,
    /// each giving every runnable task one time slice, until the budget is
    /// spent (checked between tasks). For hosts with their own event loop;
    /// see `next_timer` and `set_wake_notifier`.
    pub fn run_tasks_for(&mut self, budget: Duration) -> Progress {
        let _charged = self.enter_account();
        let deadline = Instant::now() + budget;
        loop {
            if self.live_tasks.is_empty() {
                return Progress::Finished;
            }
            if !self.run_round(Some(deadline)) {
                return if self.live_tasks.is_empty() { Progress::Finished } else { Progress::Blocked };
            }
            if Instant::now() >= deadline {
                return Progress::OutOfTime;
            }
        }
    }

    /// The earliest time a sleeping task wants to run.
    pub fn next_timer(&self) -> Option<Instant> {
        self.live_tasks
            .iter()
            .filter_map(|&t| match &self.tasks[t].state {
                State::Waiting(w) => w.deadline(),
                _ => None,
            })
            .min()
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
        match self.task_slot(id.value(), "task-kill")? {
            Some(i) => self.kill_slot(i),
            None => Ok(()),
        }
    }

    fn kill_slot(&mut self, id: usize) -> Result<(), Error> {
        let exec = self.tasks[id].exec;
        self.interrupt_handle().stop(exec, crate::stop::Stop::Kill);
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
        }
        self.finish(id, Err(Error::new(crate::stop::TASK_KILLED)));
    }

    /// Cancel a task: it sees the condition "task cancelled" where it is
    /// suspended, and its wait (a Rust future, too) is dropped. A task that
    /// has not started finishes at once; a finished task is unaffected.
    /// Cancelling the running task returns the condition to raise.
    pub fn cancel_task(&mut self, id: &TaskId) -> Result<(), Error> {
        match self.task_slot(id.value(), "task-cancel")? {
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
            }
        }
        Ok(())
    }

    /// Wait from a native: suspend the current task, or (outside tasks) run
    /// the scheduler until the wait is satisfied.
    pub fn wait_on(&mut self, wait: Wait) -> Result<Value, Error> {
        let mut wait = wait;
        if self.current_task.is_some() {
            // Done at once if it can be (a send into a buffer with room); a
            // select that must wait has its offers out before it suspends.
            if !matches!(wait, Wait::Yield)
                && let Some(result) = self.satisfy(&mut wait)
            {
                return result;
            }
            return Err(Error::suspend(wait));
        }
        loop {
            // Outside tasks: the top-level execution's stops arrive here.
            if let Err(e) = self.poll_interrupt() {
                self.abandon(&mut wait);
                return Err(e);
            }
            if let Some(result) = self.satisfy(&mut wait) {
                return result;
            }
            if !self.run_round(None) {
                if let Some(result) = self.satisfy(&mut wait) {
                    return result;
                }
                if let Err(e) = self.park(Some(&wait)) {
                    self.abandon(&mut wait);
                    return Err(e);
                }
            }
        }
    }

    /// Expose an async Rust function. `f` converts the arguments and returns a
    /// future; the calling task waits for it without blocking other tasks.
    pub fn register_async<F, Fut, T, E>(&mut self, name: &str, arity: usize, f: F)
    where
        F: Fn(&mut Vm, &[Value]) -> Fut + 'static,
        Fut: Future<Output = Result<T, E>> + 'static,
        T: IntoValue + 'static,
        E: Display + 'static,
    {
        let native = Rc::new(move |vm: &mut Vm, args: usize, n: usize| {
            let values = vm.regs[args..args + n].to_vec();
            let fut = f(vm, &values);
            let fut: BoxFuture = Box::pin(async move {
                match fut.await {
                    Ok(t) => Ok(Box::new(move |vm: &mut Vm| t.into_value(vm)) as Converter),
                    Err(e) => Err(Error::new(e.to_string())),
                }
            });
            let flag = Flag::new(vm);
            vm.wait_on(Wait::Future { fut, flag })
        });
        self.define_native(Native { name: name.into(), f: NativeImpl::Boxed(native), min: arity, max: Some(arity), doc: None });
    }

    /// The result if `wait` is satisfied now.
    fn satisfy(&mut self, wait: &mut Wait) -> Option<Result<Value, Error>> {
        match wait {
            Wait::Yield => Some(Ok(Value::VOID)),
            Wait::Sleep(deadline) => (Instant::now() >= *deadline).then_some(Ok(Value::VOID)),
            Wait::Select(s) => self.satisfy_select(s),
            Wait::Join(t) => self.task_outcome(t.get()),
            Wait::Future { fut, flag } => {
                if !flag.woken.swap(false, Ordering::SeqCst) {
                    return None;
                }
                let waker = Waker::from(flag.clone());
                match fut.as_mut().poll(&mut Context::from_waker(&waker)) {
                    Poll::Ready(Ok(convert)) => Some(convert(self)),
                    Poll::Ready(Err(e)) => Some(Err(e)),
                    Poll::Pending => None,
                }
            }
        }
    }

    /// The result if one of `s`'s operations can complete now; otherwise
    /// its sends are offered (once) and it keeps waiting.
    fn satisfy_select(&mut self, s: &mut Select) -> Option<Result<Value, Error>> {
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
        let order = (0..n).map(|i| (start + i) % n);
        let ready = order.clone().filter(|&i| !matches!(s.ops[i], Op::After(_))).chain(order.filter(|&i| matches!(s.ops[i], Op::After(_))));
        for i in ready.collect::<Vec<_>>() {
            let outcome = match &s.ops[i] {
                Op::Recv(ch) => self.take(*ch, s.group).map(Ok),
                Op::Send { ch, value, size } => match self.channel(*ch) {
                    Some(c) if !c.closed => {
                        if c.offers.iter().all(|o| Some(o.group) == s.group) && c.has_room(*size) {
                            c.bytes += size;
                            c.buf.push_back((value.clone(), *size));
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
                }
            }
            self.offers.groups.insert(g, Group { channels, won: None });
            s.group = Some(g);
        }
        None
    }

    fn select_result(&mut self, s: &Select, branch: usize, v: Value) -> Value {
        if s.indexed { self.alloc_pair(Value::int_unchecked(branch as i64), v) } else { v }
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
            for c in std::mem::take(&mut group.channels) {
                self.channels[c].offers.retain(|o| o.group != offer.group);
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
            }
        }
    }

    /// Clean up a wait that is abandoned (cancelled or interrupted).
    fn abandon(&mut self, wait: &mut Wait) {
        if let Wait::Select(s) = wait {
            self.withdraw(s);
        }
    }

    /// Wake satisfied tasks and run every runnable task for one slice, or
    /// until `deadline`. Returns whether any task ran. A task with a stop
    /// pending is killed without running, or sees "interrupted" where it
    /// waits.
    fn run_round(&mut self, deadline: Option<Instant>) -> bool {
        let mut ran = false;
        let stops = self.stops.any_pending();
        let order = self.live_tasks.clone();
        let start = self.task_turn % order.len().max(1);
        self.task_turn = 0;
        for (k, id) in order.iter().copied().enumerate().cycle().skip(start).take(order.len()) {
            if self.task_active(id) || matches!(self.tasks[id].state, State::Done) {
                continue;
            }
            if ran && deadline.is_some_and(|d| Instant::now() >= d) {
                self.task_turn = k;
                break;
            }
            if stops {
                match self.stops.pending(self.tasks[id].exec) {
                    Some(crate::stop::Stop::Kill) => {
                        self.end_killed(id);
                        continue;
                    }
                    Some(stop) if stop.catchable() && self.tasks[id].entry.is_none() => {
                        self.stops.take_catchable(self.tasks[id].exec);
                        if let State::Waiting(mut wait) = std::mem::replace(&mut self.tasks[id].state, State::Runnable) {
                            self.abandon(&mut wait);
                        }
                        let e = if stop == crate::stop::Stop::Break { Error::new(crate::stop::INTERRUPTED) } else { self.out_of_memory() };
                        self.tasks[id].delivery = Some(Err(e));
                    }
                    _ => {}
                }
            }
            if let State::Waiting(_) = self.tasks[id].state {
                let State::Waiting(mut wait) = std::mem::replace(&mut self.tasks[id].state, State::Runnable) else { unreachable!() };
                match self.satisfy(&mut wait) {
                    Some(result) => self.tasks[id].delivery = Some(result.map(|v| self.root(v))),
                    None => self.tasks[id].state = State::Waiting(wait),
                }
            }
            if matches!(self.tasks[id].state, State::Runnable) {
                self.run_task(id);
                ran = true;
            }
        }
        ran
    }

    fn run_task(&mut self, id: usize) {
        let outer_task = self.current_task.replace(id);
        self.active.push((self.tasks[id].exec, Some(id)));
        self.stops.run(self.tasks[id].exec);
        let mut stack = std::mem::take(&mut self.tasks[id].stack);
        self.swap_stack(&mut stack);
        // The stack we swapped out (main or another level) stays visible to the GC.
        self.tasks[id].stack = stack;
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
        let mut stack = std::mem::take(&mut self.tasks[id].stack);
        self.swap_stack(&mut stack);
        self.tasks[id].stack = stack;
        self.current_task = outer_task;
        self.active.pop();
        self.stops.run(self.running_execution());
        match result {
            Ok(Exit::Done(v)) => {
                let v = self.root(v);
                self.finish(id, Ok(v));
            }
            Ok(Exit::Suspend(mut s)) => {
                self.tasks[id].state = match s.wait.take() {
                    None => State::Runnable,
                    Some(Wait::Yield) => {
                        let void = self.root(Value::VOID);
                        self.tasks[id].delivery = Some(Ok(void));
                        State::Runnable
                    }
                    Some(w) => State::Waiting(w),
                };
                self.tasks[id].resume = Some(s);
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
            let msg = e.msg.clone();
            let obj = self.make_error_object(&msg, &[]);
            e.payload = Some(self.root(obj));
        }
        // The result goes into the task's object, and the slot is free.
        let (value, error) = match result {
            Ok(v) => (v, Value::FALSE),
            Err(mut e) => {
                let payload = e.payload.take().expect("set above");
                (e.escape, e.wait) = (None, None);
                (payload, self.make_foreign(Rc::new(e), "task-error"))
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
        task.stack = Stack::default();
        (task.entry, task.resume, task.delivery) = (None, None, None);
        let exec = task.exec;
        self.write_barrier(h, value.get());
        self.write_barrier(h, error);
        self.stops.end(exec);
        self.live_tasks.retain(|&t| t != id);
        self.free_tasks.push(id);
    }

    /// Block the thread until a timer expires or a future is woken. Errors if
    /// nothing could ever wake up (every task waits on channels or joins).
    fn park(&mut self, extra: Option<&Wait>) -> Result<(), Error> {
        self.poll_interrupt()?;
        let waits = self.live_tasks.iter().filter_map(|&t| match &self.tasks[t].state {
            State::Waiting(w) => Some(w),
            _ => None,
        });
        let waits: Vec<&Wait> = waits.chain(extra).collect();
        let deadline = waits.iter().filter_map(|w| w.deadline()).min();
        let futures = waits.iter().any(|w| matches!(w, Wait::Future { .. }));
        let woken = waits.iter().any(|w| matches!(w, Wait::Future { flag, .. } if flag.woken.load(Ordering::SeqCst)));
        if woken {
            return Ok(());
        }
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

fn spawn(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    vm.admit(vm.task_bytes())?;
    Ok(vm.spawn_task(arg(vm, args, 0)))
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
    let millis: f64 = vm.get(ms)?;
    Duration::try_from_secs_f64(millis.max(0.0) / 1000.0)
        .ok()
        .and_then(|d| Instant::now().checked_add(d))
        .ok_or_else(|| Error::new(format!("{who}: duration out of range: {}", crate::builtins::repr(ms))))
}

/// `(%make-channel capacity max-bytes-or-#f)`.
fn make_channel(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let capacity: i64 = vm.get(arg(vm, args, 0))?;
    let max_bytes = arg(vm, args, 1);
    let max_bytes = if max_bytes.is_truthy() { Some(vm.get::<i64>(max_bytes)?) } else { None };
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
        /// The task runs when the current one waits or is preempted.
        "(spawn thunk)" => spawn;
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
        "(%live-task-count)" => |vm: &mut Vm, _, _| Ok(Value::int_unchecked(vm.live_tasks.len() as i64));
        /// Run the tasks until none can go on.
        "(run-tasks)" => |vm: &mut Vm, _, _| { vm.run_tasks()?; Ok(Value::VOID) };
    }
}

#[cfg(test)]
mod tests {
    use crate::vm::Vm;

    /// Finished tasks and closed, drained channels give their slots back.
    #[test]
    fn slots_are_reused() {
        let mut vm = Vm::new();
        let v = vm
            .eval_source(
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
}
