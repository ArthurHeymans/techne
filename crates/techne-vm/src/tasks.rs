//! Cooperative, preemptible tasks.
//!
//! Each task owns a register/frame/handler stack; running a task swaps it in.
//! A task is preempted after `TASK_SLICE` calls or backward jumps, and
//! suspends when a native it calls must wait: `sleep`, `channel-recv`,
//! `task-join`, `yield`, or a Rust future registered with `register_async`.
//! The waiting native's call site receives the result when the task resumes.
//!
//! Code that is not in a task (the main program, the REPL) waits by running
//! the scheduler until its wait is satisfied, so `(task-join t)` at top level
//! drives every task. A native that calls back into Scheme (e.g.
//! `dynamic-wind`) is a non-suspendable boundary, like a C call in Lua.
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

use crate::{
    api::{IntoValue, Root},
    builtins::type_error,
    heap::{Kind, field, is_kind},
    value::Value,
    vm::{Error, Exit, Native, NativeFn, NativeImpl, SpecialObj, Stack, Suspend, Vm},
};

/// Turns a finished Rust future's output into a Scheme value on the VM thread.
pub type Converter = Box<dyn FnOnce(&mut Vm) -> Result<Value, Error>>;
pub type BoxFuture = Pin<Box<dyn Future<Output = Result<Converter, Error>>>>;

/// Wakes the scheduler thread when a future can make progress.
pub struct Flag {
    woken: AtomicBool,
    thread: Thread,
}

impl Flag {
    fn new() -> Arc<Flag> {
        // Starts woken so the future is polled once immediately.
        Arc::new(Flag { woken: AtomicBool::new(true), thread: std::thread::current() })
    }
}

impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.woken.store(true, Ordering::SeqCst);
        self.thread.unpark();
    }
}

/// What a suspended task (or the main program) waits for.
pub enum Wait {
    Yield,
    Sleep(Instant),
    Channel(usize),
    Join(usize),
    Future { fut: BoxFuture, flag: Arc<Flag> },
}

pub(crate) enum State {
    Runnable,
    Waiting(Wait),
    Done,
}

pub(crate) struct Task {
    pub stack: Stack,
    pub state: State,
    pub resume: Option<Suspend>,
    pub entry: Option<Root>,
    pub delivery: Option<Result<Root, Error>>,
    pub result: Option<Result<Root, Error>>,
}

/// Handle to a task spawned from Rust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskId(pub usize);

impl Error {
    pub fn suspend(wait: Wait) -> Error {
        let mut e = Error::new("task suspended");
        e.wait = Some(wait);
        e
    }
}

impl Vm {
    /// Create a task running the zero-argument procedure `f`.
    pub fn spawn(&mut self, f: Value) -> TaskId {
        let entry = self.root(f);
        let mut stack = Stack::with_regs(256);
        // New tasks inherit the spawner's parameters and output port.
        stack.locals = self.locals.clone();
        self.tasks.push(Task {
            stack,
            state: State::Runnable,
            resume: None,
            entry: Some(entry),
            delivery: None,
            result: None,
        });
        TaskId(self.tasks.len() - 1)
    }

    /// The task's result once it has finished.
    pub fn task_result(&self, id: TaskId) -> Option<Result<Value, Error>> {
        self.tasks[id.0].result.as_ref().map(|r| match r {
            Ok(v) => Ok(v.get()),
            Err(e) => Err(e.duplicate()),
        })
    }

    /// Run until every task has finished (or all remaining ones are blocked
    /// forever, which is reported as a deadlock).
    pub fn run_tasks(&mut self) -> Result<(), Error> {
        while self.tasks.iter().any(|t| !matches!(t.state, State::Done)) {
            if !self.run_round() {
                self.park(None)?;
            }
        }
        Ok(())
    }

    /// Wait from a native: suspend the current task, or (outside tasks) run
    /// the scheduler until the wait is satisfied.
    pub fn wait_on(&mut self, wait: Wait) -> Result<Value, Error> {
        if self.current_task.is_some() {
            return Err(Error::suspend(wait));
        }
        let mut wait = wait;
        loop {
            if let Some(result) = self.satisfy(&mut wait) {
                return result;
            }
            if !self.run_round() {
                if let Some(result) = self.satisfy(&mut wait) {
                    return result;
                }
                self.park(Some(&wait))?;
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
            vm.wait_on(Wait::Future { fut, flag: Flag::new() })
        });
        self.define_native(Native { name: name.into(), f: NativeImpl::Boxed(native), min: arity, max: Some(arity) });
    }

    /// The result if `wait` is satisfied now.
    fn satisfy(&mut self, wait: &mut Wait) -> Option<Result<Value, Error>> {
        match wait {
            Wait::Yield => Some(Ok(Value::VOID)),
            Wait::Sleep(deadline) => (Instant::now() >= *deadline).then_some(Ok(Value::VOID)),
            Wait::Channel(c) => self.channels[*c].pop_front().map(|r| Ok(r.get())),
            Wait::Join(t) => match &self.tasks[*t].result {
                Some(Ok(v)) => Some(Ok(v.get())),
                Some(Err(e)) => Some(Err(e.duplicate())),
                None => None,
            },
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

    /// Wake satisfied tasks and run every runnable task for one slice.
    /// Returns whether any task ran.
    fn run_round(&mut self) -> bool {
        let mut ran = false;
        for id in 0..self.tasks.len() {
            if Some(id) == self.current_task {
                continue;
            }
            if let State::Waiting(_) = self.tasks[id].state {
                let State::Waiting(mut wait) = std::mem::replace(&mut self.tasks[id].state, State::Runnable) else {
                    unreachable!()
                };
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
        let mut stack = std::mem::take(&mut self.tasks[id].stack);
        self.swap_stack(&mut stack);
        self.tasks[id].stack = stack;
        self.current_task = outer_task;
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
        let task = &mut self.tasks[id];
        task.result = Some(result);
        task.state = State::Done;
        task.stack = Stack::default();
    }

    /// Block the thread until a timer expires or a future is woken. Errors if
    /// nothing could ever wake up (every task waits on channels or joins).
    fn park(&mut self, extra: Option<&Wait>) -> Result<(), Error> {
        let waits = self.tasks.iter().filter_map(|t| match &t.state {
            State::Waiting(w) => Some(w),
            _ => None,
        });
        let waits: Vec<&Wait> = waits.chain(extra).collect();
        let deadline = waits.iter().filter_map(|w| if let Wait::Sleep(d) = w { Some(*d) } else { None }).min();
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

fn record_id(vm: &Vm, v: Value, rtd: SpecialObj, who: &str, what: &str) -> Result<usize, Error> {
    if is_kind(v, Kind::Record) && unsafe { field(v.as_ptr(), 0) } == vm.special(rtd) {
        Ok(unsafe { field(v.as_ptr(), 1) }.as_int() as usize)
    } else {
        Err(type_error(who, what, v))
    }
}

fn spawn(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let TaskId(id) = vm.spawn(arg(vm, args, 0));
    let rtd = vm.special(SpecialObj::TaskRtd);
    Ok(vm.make_record(rtd, &[Value::int_unchecked(id as i64)]))
}

fn join(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let id = record_id(vm, arg(vm, args, 0), SpecialObj::TaskRtd, "task-join", "task")?;
    vm.wait_on(Wait::Join(id))
}

fn sleep(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let ms: f64 = vm.get(arg(vm, args, 0))?;
    vm.wait_on(Wait::Sleep(Instant::now() + Duration::from_secs_f64(ms.max(0.0) / 1000.0)))
}

fn make_channel(vm: &mut Vm, _: usize, _: usize) -> Result<Value, Error> {
    vm.channels.push(Default::default());
    let id = vm.channels.len() - 1;
    let rtd = vm.special(SpecialObj::ChannelRtd);
    Ok(vm.make_record(rtd, &[Value::int_unchecked(id as i64)]))
}

fn channel_send(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let id = record_id(vm, arg(vm, args, 0), SpecialObj::ChannelRtd, "channel-send", "channel")?;
    let v = vm.root(arg(vm, args, 1));
    vm.channels[id].push_back(v);
    Ok(Value::VOID)
}

fn channel_recv(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let id = record_id(vm, arg(vm, args, 0), SpecialObj::ChannelRtd, "channel-recv", "channel")?;
    vm.wait_on(Wait::Channel(id))
}

fn task_done(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let id = record_id(vm, arg(vm, args, 0), SpecialObj::TaskRtd, "task-done?", "task")?;
    Ok(Value::bool(vm.tasks[id].result.is_some()))
}

fn current_task(vm: &mut Vm, _: usize, _: usize) -> Result<Value, Error> {
    match vm.current_task {
        Some(id) => {
            let rtd = vm.special(SpecialObj::TaskRtd);
            Ok(vm.make_record(rtd, &[Value::int_unchecked(id as i64)]))
        }
        None => Ok(Value::FALSE),
    }
}

macro_rules! natives {
    ($vm:expr; $($name:literal $min:literal $max:tt => $f:expr;)*) => {
        $( {
            let f: NativeFn = $f;
            $vm.define_native(Native { name: $name.into(), f: NativeImpl::Plain(f), min: $min, max: natives!(@max $max) });
        } )*
    };
    (@max $m:literal) => { Some($m) };
}

pub fn install(vm: &mut Vm) {
    natives! { vm;
        "spawn" 1 1 => spawn;
        "task-join" 1 1 => join;
        "task-done?" 1 1 => task_done;
        "current-task" 0 0 => current_task;
        "yield" 0 0 => |vm: &mut Vm, _, _| vm.wait_on(Wait::Yield);
        "sleep" 1 1 => sleep;
        "make-channel" 0 0 => make_channel;
        "channel-send" 2 2 => channel_send;
        "channel-recv" 1 1 => channel_recv;
        "run-tasks" 0 0 => |vm: &mut Vm, _, _| { vm.run_tasks()?; Ok(Value::VOID) };
    }
}
