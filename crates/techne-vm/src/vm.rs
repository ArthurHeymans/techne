//! The interpreter: modules, globals, code objects, the register stack,
//! exception handlers and dispatch.
//!
//! GC roots are the live register window `regs[..stack_top]`, globals, code
//! constants, `scratch`, `specials` and Rust `Root`s. Any operation that
//! allocates must update `stack_top` first and re-read register values after
//! allocating, because a collection moves objects.
//!
//! The VM is re-entrant: Rust (including natives) can call Scheme procedures
//! with `Vm::call`, which runs a nested dispatch above the current stack top.

use std::{
    any::Any,
    cell::Cell,
    fmt,
    io::{BufWriter, Stdout, Write},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::Thread,
};

pub(crate) mod held;
mod retained;

pub use held::Held;

use rustc_hash::FxHashMap;

use crate::{
    api::Root,
    code::{CapSrc, Code, Op},
    compiler::Compiler,
    expand::Macro,
    heap::{self, Heap, Kind, LARGE_WORDS, Roots, field, header, is_kind, set_field},
    num,
    reader::{self, NO_POS, Sexp, symbol_name},
    value::Value,
};

/// Register stack limit (128 MiB); deeper recursion raises an error.
const MAX_REGS: usize = 1 << 24;
const CANNOT_SUSPEND: &str = "cannot suspend here: a Rust native procedure is calling back into Scheme (vm.call)";
/// Module 0 holds builtins and the prelude; it is visible from every module.
pub const ROOT_MODULE: u32 = 0;
/// The module of a freed global.
const NO_MODULE: u32 = u32::MAX;
/// Module for code evaluated without a file (REPL, `eval_source`).
pub const USER_MODULE: u32 = 1;

/// Authority a world can hold. Natives that need one are registered inside
/// `Vm::requiring`; in a world not granted it they only raise an error, so
/// code there cannot reach it by any name, import or value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Capability {
    /// Reading and writing files.
    Files,
    /// Environment variables and the command line.
    Environment,
    /// Starting and controlling child processes.
    Processes,
    /// Connecting to other machines (nodes).
    Network,
    /// Loading modules from files (`require`, naming a file's module).
    Loading,
    /// Ending the host process (`exit`).
    HostControl,
}

impl Capability {
    pub fn name(self) -> &'static str {
        match self {
            Capability::Files => "files",
            Capability::Environment => "environment",
            Capability::Processes => "processes",
            Capability::Network => "network",
            Capability::Loading => "loading",
            Capability::HostControl => "host control",
        }
    }
}

/// The capabilities a world is granted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Grants(u8);

impl Grants {
    pub const NONE: Grants = Grants(0);
    pub const ALL: Grants = Grants(0x3f);

    pub const fn with(self, c: Capability) -> Grants {
        Grants(self.0 | 1 << c as u8)
    }

    pub const fn without(self, c: Capability) -> Grants {
        Grants(self.0 & !(1 << c as u8))
    }

    pub const fn has(self, c: Capability) -> bool {
        self.0 & 1 << c as u8 != 0
    }
}

/// A failure: boxed, so that `Result<Value, Error>` is two words, which
/// natives and the interpreter's helpers return in registers. Its parts are
/// `ErrorData`'s.
pub struct Error(Box<ErrorData>);

impl std::ops::Deref for Error {
    type Target = ErrorData;
    fn deref(&self) -> &ErrorData {
        &self.0
    }
}

impl std::ops::DerefMut for Error {
    fn deref_mut(&mut self) -> &mut ErrorData {
        &mut self.0
    }
}

pub struct ErrorData {
    pub msg: String,
    /// Innermost first: "name (file:line:col)".
    pub trace: Vec<String>,
    /// The raised Scheme object, if any (error objects, `raise`d values).
    pub payload: Option<Root>,
    /// Escape to the `call/cc` with this id, carrying the value.
    pub escape: Option<(i64, Root)>,
    /// Handlers at or above this index were already consulted (set when the
    /// error leaves a dispatch level, so outer levels continue below it).
    searched: Option<u32>,
    /// Set by natives that must wait (see `tasks`): suspends the running task.
    pub wait: Option<crate::tasks::Wait>,
    /// What kind of failure, for `file-error?` and `read-error?`.
    pub kind: ErrorKind,
}

/// The kinds of error R7RS tells apart.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ErrorKind {
    #[default]
    General,
    /// Opening, reading or writing a file failed.
    File,
    /// `read` met malformed text.
    Read,
    /// `exit` asked the host to end the program with this status. Handlers
    /// do not see it; it unwinds to the host.
    Exit(i32),
    /// The execution was killed (`crate::stop`). Handlers do not see it, and
    /// `dynamic-wind` after thunks do not run.
    Killed,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Error {
        Error(Box::new(ErrorData {
            msg: msg.into(),
            trace: Vec::new(),
            payload: None,
            escape: None,
            searched: None,
            wait: None,
            kind: ErrorKind::General,
        }))
    }

    /// Its parts, to move them out.
    pub fn into_inner(self) -> ErrorData {
        *self.0
    }

    pub fn with_kind(mut self, kind: ErrorKind) -> Error {
        self.kind = kind;
        self
    }

    /// The status of an `exit` request, which the host carries out.
    pub fn exit_code(&self) -> Option<i32> {
        match self.kind {
            ErrorKind::Exit(code) => Some(code),
            _ => None,
        }
    }

    /// Raised by a break (`crate::stop`).
    pub fn is_interrupt(&self) -> bool {
        self.msg == INTERRUPTED
    }

    /// The execution was killed (`crate::stop`).
    pub fn is_kill(&self) -> bool {
        self.kind == ErrorKind::Killed
    }

    /// Whether it unwinds to the host, past every handler.
    fn reaches_host(&self) -> bool {
        matches!(self.kind, ErrorKind::Exit(_) | ErrorKind::Killed)
    }

    /// A copy for another consumer (e.g. every task joining a failed task).
    pub fn duplicate(&self) -> Error {
        let mut copy = Error::new(self.msg.clone());
        (copy.trace, copy.payload, copy.kind) = (self.trace.clone(), self.payload.clone(), self.kind);
        copy
    }
}

/// How a dispatch loop stopped.
pub(crate) enum Exit {
    Done(Value),
    Suspend(Suspend),
}

/// A suspended task's execution point. `slot` receives the result of the
/// waiting native call (or, for a tail call, the frame returns it).
pub(crate) struct Suspend {
    pub code: *const Code,
    pub pc: usize,
    pub bp: usize,
    pub slot: usize,
    pub tail: bool,
    pub wait: Option<crate::tasks::Wait>,
}

/// An execution stack; tasks own one each and swap it in to run.
#[derive(Default)]
pub(crate) struct Stack {
    pub regs: Vec<Value>,
    frames: Vec<Frame>,
    handlers: Vec<Handler>,
    pub stack_top: usize,
    /// Task-local dynamic state (parameters, output port, restarts).
    pub locals: FxHashMap<i64, Root>,
}

impl Stack {
    pub(crate) fn with_regs(n: usize) -> Stack {
        Stack { regs: vec![Value::VOID; n], ..Stack::default() }
    }
}

/// Calls and backward jumps a task runs before it is preempted.
pub const TASK_SLICE: u32 = 10_000;
/// Calls or back-edges native code runs outside tasks between interrupt polls.
const POLL_SLICE: u32 = 1 << 16;

/// What `Vm::procedure_info` reports.
#[derive(Clone, Debug)]
pub struct ProcedureInfo {
    pub name: Rc<str>,
    /// Parameter names as written (`#:key`, `(x default)`, `. rest`).
    pub params: Vec<Rc<str>>,
    pub doc: Option<Rc<str>>,
    /// The source file name, and 1-based line and column (0 if unknown).
    pub file: Option<Rc<str>>,
    pub line: usize,
    pub column: usize,
    /// A built-in (Rust) procedure.
    pub native: bool,
}

/// What a name denotes (`Vm::describe_name`), as help shows it.
#[derive(Clone, Debug)]
pub struct Description {
    pub name: Rc<str>,
    /// `procedure`, `built-in procedure`, `macro`, `special form`,
    /// `variable` or `unbound`.
    pub kind: &'static str,
    /// A procedure's parameters as written; a special form's syntax, whole.
    pub params: Option<Vec<Rc<str>>>,
    /// A built-in's arguments, at least and at most.
    pub arity: Option<(usize, Option<usize>)>,
    pub doc: Option<Rc<str>>,
    /// Where it is defined: a file, and 1-based line and column (0 if
    /// unknown).
    pub file: Option<Rc<str>>,
    pub line: usize,
    pub column: usize,
}

impl Description {
    /// How it is called: `(name param ...)`, or a special form's syntax.
    pub fn signature(&self) -> Option<String> {
        let params = self.params.as_ref()?;
        Some(if self.kind == "special form" {
            params.concat()
        } else {
            let sep = if params.is_empty() { "" } else { " " };
            format!("({}{sep}{})", self.name, params.join(" "))
        })
    }
}

/// `(f x)  procedure, file:line`, then the docstring after a blank line.
impl std::fmt::Display for Description {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.signature() {
            Some(sig) => write!(f, "{sig}  {}", self.kind)?,
            None => write!(f, "{}: {}", self.name, self.kind)?,
        }
        if let (None, Some(arity)) = (&self.params, self.arity) {
            let plural = |n: usize| if n == 1 { "" } else { "s" };
            match arity {
                (a, Some(b)) if a == b => write!(f, ", {a} argument{}", plural(a))?,
                (a, Some(b)) => write!(f, ", {a}-{b} arguments")?,
                (a, None) => write!(f, ", at least {a} argument{}", plural(a))?,
            }
        }
        if let Some(file) = &self.file {
            write!(f, ", {file}")?;
            if self.line > 0 {
                write!(f, ":{}", self.line)?;
            }
        }
        if let Some(doc) = &self.doc {
            write!(f, "\n\n{doc}")?;
        }
        Ok(())
    }
}

pub use crate::stop::{ExecId, INTERRUPTED, InterruptHandle, KILLED, Stop, TASK_KILLED};

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error: {}", self.msg)?;
        for t in &self.trace {
            write!(f, "\n  in {t}")?;
        }
        Ok(())
    }
}

pub type NativeFn = fn(&mut Vm, usize, usize) -> Result<Value, Error>;
pub type BoxedNative = Rc<dyn Fn(&mut Vm, usize, usize) -> Result<Value, Error>>;

#[derive(Clone)]
pub enum NativeImpl {
    Plain(NativeFn),
    Boxed(BoxedNative),
}

#[derive(Clone)]
pub struct Native {
    pub name: Rc<str>,
    pub f: NativeImpl,
    pub min: usize,
    pub max: Option<usize>,
    /// Its parameters and documentation, from its definition in Rust (the
    /// docstring is empty for an internal native, `%name`).
    pub doc: Option<&'static NativeDoc>,
}

/// What a native's definition in Rust says about it (`techne_vm_macros`
/// makes these from the signature and doc comments of each).
#[derive(Debug)]
pub struct NativeDoc {
    /// Its parameters as written in Lisp: `x`, `[x]`, `. rest`.
    pub params: &'static [&'static str],
    pub doc: &'static str,
    /// The Rust source file and line it is defined at.
    pub file: &'static str,
    pub line: u32,
}

#[derive(Clone)]
pub enum GlobalBinding {
    Var(u32),
    Macro(Rc<Macro>),
}

pub struct Module {
    pub name: Rc<str>,
    pub path: Option<PathBuf>,
    pub(crate) imports: FxHashMap<u32, GlobalBinding>,
    /// What `provide` or a library's `export` makes visible: each binding's
    /// name inside the module and the name importers see.
    pub(crate) exports: Option<Vec<(u32, u32)>>,
    pub(crate) defined: Vec<u32>,
    loading: bool,
    /// Sees only its own definitions and imports, not the root module (R7RS
    /// libraries and environments).
    pub(crate) isolated: bool,
    /// The package generation the module belongs to (0: none).
    pub generation: u32,
    /// Its generation was retired (`Vm::retire_generation`): it is freed
    /// once nothing live uses it.
    retired: bool,
}

/// A package generation being loaded (`Vm::stage_package`).
struct Staging {
    /// The package's directory: files under it load afresh.
    dir: PathBuf,
    generation: u32,
    modules: FxHashMap<PathBuf, u32>,
}

/// Datum labels while a literal is materialised: each label's value, and
/// the placeholders that stood for a datum inside itself.
#[derive(Default)]
struct Labels {
    values: FxHashMap<u32, Value>,
    placeholders: Vec<(Value, Value)>,
    /// Header flags of the pairs, vectors, strings and bytevectors built:
    /// `IMMUTABLE` for a literal in code.
    flags: u64,
}

impl Labels {
    /// Replaces the placeholders in the (old-space) pairs and vectors of `v`.
    fn patch(&self, v: Value) -> Value {
        let real = |x: Value| self.placeholders.iter().find(|(p, _)| *p == x).map_or(x, |(_, r)| *r);
        let mut seen = rustc_hash::FxHashSet::default();
        let mut todo = vec![real(v)];
        while let Some(x) = todo.pop() {
            let fields = if is_kind(x, Kind::Pair) {
                2
            } else if is_kind(x, Kind::Vector) {
                unsafe { heap::len_of(x.as_ptr()) }
            } else {
                0
            };
            if fields == 0 || !seen.insert(x.bits()) {
                continue;
            }
            for i in 0..fields {
                let f = real(unsafe { field(x.as_ptr(), i) });
                unsafe { set_field(x.as_ptr(), i, f) };
                todo.push(f);
            }
        }
        real(v)
    }
}

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub name: Rc<str>,
    pub text: Rc<str>,
}

#[derive(Clone, Copy)]
struct Frame {
    code: *const Code,
    pc: u32,
    bp: u32,
}

/// An installed exception handler.
#[derive(Clone)]
enum Handler {
    /// `guard`: unwind to `target` in the frame that installed it.
    Guard { frames_len: usize, code: *const Code, bp: usize, target: u32, dst: u16 },
    /// `with-exception-handler`: call the procedure at the raise point.
    Proc { handler: Root },
    /// `call/cc` escape point.
    Escape { id: i64, frames_len: usize, code: *const Code, bp: usize, target: u32, dst: u16 },
    /// `dynamic-wind`: run `after` when unwinding past this point.
    Wind { after: Root },
}

/// Where unwinding resumes: a guard's handler or an escape point.
struct Landing {
    frames_len: usize,
    code: *const Code,
    bp: usize,
    target: u32,
    dst: u16,
}

/// Builtin record types and similar VM-owned objects (GC roots).
#[derive(Clone, Copy)]
pub enum SpecialObj {
    ErrorRtd = 0,
    ContinuationRtd = 1,
    ValuesRtd = 2,
    TaskRtd = 3,
    ChannelRtd = 4,
}
const SPECIALS: usize = 5;

/// `TECHNE_DUMP`: print the code compiled for each top-level form.
fn dump_code() -> bool {
    static DUMP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DUMP.get_or_init(|| std::env::var_os("TECHNE_DUMP").is_some())
}

/// `TECHNE_JIT`: unset for the default, `0` to disable, or the number of loop
/// iterations after which a function is compiled.
fn jit_threshold_from_env() -> Option<u32> {
    match std::env::var("TECHNE_JIT") {
        Ok(s) if s == "0" || s == "off" => None,
        Ok(s) => Some(s.parse().unwrap_or(1000)),
        Err(_) => Some(1000),
    }
}

pub struct Vm {
    pub heap: Heap,
    /// What the world's memory is charged to (`crate::alloc`): entered
    /// while the VM runs, collects or allocates in the old generation.
    account: crate::alloc::Account,
    /// While the host runs an execution: the account entered.
    entered: Option<crate::alloc::Entered>,
    /// The most bytes the world is to hold (`held`): growth is admitted
    /// only if it fits (`admit`).
    memory_limit: usize,
    /// Over the limit (`check_memory`).
    pressure: Option<held::Pressure>,
    pub regs: Vec<Value>,
    frames: Vec<Frame>,
    handlers: Vec<Handler>,
    pub globals: Vec<Value>,
    /// Whether each global is a root: all but those of retired modules,
    /// which live while something live uses them (`retained`).
    global_rooted: Vec<bool>,
    free_globals: Vec<u32>,
    global_names: Vec<u32>,
    global_module: Vec<u32>,
    user_defined: Vec<bool>,
    pub(crate) bindings: FxHashMap<(u32, u32), GlobalBinding>,
    /// Field counts of record types defined at top level, by the global that
    /// holds the type (for `match` record patterns).
    pub record_types: FxHashMap<u32, usize>,
    pub modules: Vec<Module>,
    free_modules: Vec<u32>,
    /// Retired modules not freed yet, with their globals.
    retired: Vec<(u32, Vec<u32>)>,
    /// Retired modules found dead in the last collection, to free.
    dead_modules: Vec<(u32, Vec<u32>)>,
    /// Modules evaluations are running in (`pinning`).
    pinned_modules: Vec<u32>,
    /// The last package generation staged.
    generations: u32,
    module_paths: FxHashMap<PathBuf, u32>,
    /// A package generation being loaded: its files load into fresh
    /// modules, kept apart until published.
    staging: Option<Staging>,
    /// The last generation staged and its modules, until published or
    /// discarded.
    staged: Option<(u32, FxHashMap<PathBuf, u32>)>,
    /// The module of the evaluation in progress (`eval_in`), where `eval`
    /// without a module and `help` resolve names; `in-module` changes it.
    current_module: u32,
    /// What this world may reach outside the VM.
    grants: Grants,
    /// The capability natives being defined need (`requiring`).
    requiring: Option<Capability>,
    /// Source text by file index (`Code::file`). A file lives while codes
    /// from it do (`file_codes` counts them) or while it is being evaluated
    /// (`pinned_files`); then its slot is emptied and reused.
    pub files: Vec<SourceFile>,
    file_codes: Vec<u32>,
    free_files: Vec<u32>,
    pinned_files: Vec<u32>,
    /// The docstrings of variables, by global: `(define name value "doc")`.
    pub variable_docs: FxHashMap<u32, Rc<str>>,
    /// Boxed so a `Code` keeps its address when the vector grows: closures
    /// and JIT code point into it. `None`: freed, its index in `free_codes`.
    codes: Vec<Option<Box<Code>>>,
    free_codes: Vec<u32>,
    /// Codes of compilations and the top-level forms they run, kept until
    /// the form has run (`eval_form`): nothing else refers to them yet.
    compiling: Vec<u32>,
    /// Codes whose handles died in the last collection, to release.
    #[allow(clippy::vec_box)]
    dead_codes: Vec<Box<Code>>,
    /// Baseline JIT (`None` when disabled with `TECHNE_JIT=0`).
    jit: Option<Box<crate::jit::Compiler>>,
    /// Loop iterations or calls after which a function is compiled
    /// (`u32::MAX` without a JIT).
    jit_threshold: u32,
    /// Jobs submitted to the JIT and not installed yet: their id, code and
    /// cancellation flag. A code that dies cancels its job; a result whose
    /// job is gone is dropped.
    jit_jobs: Vec<(u64, *const Code, Arc<AtomicBool>)>,
    jit_job_ids: u64,
    /// Codes whose JIT code expects closures (`JitSlot::callees`): those
    /// are kept current as they move, and live while the code does.
    jit_callers: Vec<*const Code>,
    /// Codes added with constants in the nursery (closures inlining
    /// compared with, objects `eval` was given): the collection that moves
    /// them updates `Code::consts` and the handle's copy, which nothing
    /// remembers.
    young_consts: Vec<*const Code>,
    /// An error raised in JIT-compiled code, handed to the interpreter.
    pub(crate) jit_error: Option<Error>,
    /// Frames of native calls, innermost first, recorded when native code
    /// hands over to the interpreter.
    jit_unwind: Vec<Frame>,
    /// While a handler procedure runs: where the condition was raised,
    /// innermost first (see `raise_backtrace`).
    raise_trace: Vec<String>,
    /// Set while the running execution has a stop pending (`crate::stop`);
    /// polled at safepoints.
    pub(crate) interrupt: Arc<AtomicBool>,
    pub(crate) stops: Arc<crate::stop::Stops>,
    /// The top-level executions running, innermost last, with the dynamic
    /// state to restore if one is killed (no after thunk restores it).
    executions: Vec<(ExecId, FxHashMap<i64, Root>)>,
    /// The executions active, innermost last: top-level ones and the tasks
    /// running (with their index), nested as they call each other. The
    /// last runs; stops address it.
    pub(crate) active: Vec<(ExecId, Option<usize>)>,
    /// The thread the VM runs on (woken by interrupts and futures).
    pub(crate) thread: Thread,
    /// Called (from any thread) when a Rust future a task waits on is woken.
    pub(crate) wake_notifier: Option<Arc<dyn Fn() + Send + Sync>>,
    pub natives: Vec<Native>,
    pub(crate) apply_native: Value,
    /// Exclusive end of the live register window (GC root extent).
    pub stack_top: usize,
    /// Extra roots for natives that allocate repeatedly.
    pub scratch: Vec<Value>,
    pub specials: [Value; SPECIALS],
    roots: Vec<Weak<Cell<Value>>>,
    foreign: Vec<Option<(Rc<dyn Any>, &'static str)>>,
    foreign_free: Vec<usize>,
    /// Scheme names for foreign Rust types (by `type_name`), for dispatch.
    pub foreign_type_names: FxHashMap<&'static str, u32>,
    next_id: i64,
    pub(crate) tasks: Vec<crate::tasks::Task>,
    /// The tasks not finished, in the order spawned.
    pub(crate) live_tasks: Vec<usize>,
    /// Where in `live_tasks` the next round starts: one cut short by its
    /// budget goes on where it stopped.
    pub(crate) task_turn: usize,
    pub(crate) channels: Vec<crate::tasks::Channel>,
    /// Blocked senders' offers, by group (one per waiting send or select).
    pub(crate) offers: crate::tasks::Offers,
    pub(crate) current_task: Option<usize>,
    /// Handler-stack ranges hidden from raises while a handler procedure for
    /// them runs (escape points and winds there stay live).
    masks: Vec<(usize, usize)>,
    /// Dynamic state of the running stack (swapped with tasks).
    pub(crate) locals: FxHashMap<i64, Root>,
    pub out: BufWriter<Stdout>,
}

struct VmRoots<'a> {
    regs: &'a mut [Value],
    globals: &'a mut [Value],
    global_rooted: &'a [bool],
    global_module: &'a [u32],
    modules: &'a [Module],
    retired: &'a mut Vec<(u32, Vec<u32>)>,
    dead_modules: &'a mut Vec<(u32, Vec<u32>)>,
    /// Modules evaluations run in, which stay.
    pinned_modules: Vec<u32>,
    /// Retired modules found used in the marking in progress (a full
    /// collection may run two).
    used: rustc_hash::FxHashSet<u32>,
    codes: &'a mut Vec<Option<Box<Code>>>,
    free_codes: &'a mut Vec<u32>,
    #[allow(clippy::vec_box)]
    dead_codes: &'a mut Vec<Box<Code>>,
    /// Codes running or about to (frames, handlers, suspended tasks,
    /// compilations), whose handles are roots.
    running: Vec<*const Code>,
    jit_callers: &'a [*const Code],
    bindings: &'a FxHashMap<(u32, u32), GlobalBinding>,
    young_consts: &'a [*const Code],
    scratch: &'a mut [Value],
    specials: &'a mut [Value],
    roots: &'a mut Vec<Weak<Cell<Value>>>,
    tasks: &'a mut [crate::tasks::Task],
}

impl Stack {
    /// The codes its frames and handlers run in.
    fn codes(&self) -> impl Iterator<Item = *const Code> + '_ {
        self.frames.iter().map(|f| f.code).chain(self.handlers.iter().filter_map(Handler::code))
    }
}

impl Handler {
    fn code(&self) -> Option<*const Code> {
        match self {
            Handler::Guard { code, .. } | Handler::Escape { code, .. } => Some(*code),
            Handler::Proc { .. } | Handler::Wind { .. } => None,
        }
    }
}

impl Roots for VmRoots<'_> {
    fn visit(&mut self, f: &mut dyn FnMut(&mut Value)) {
        // Stacks of tasks that are not running (or the main stack, parked in
        // the running task's slot).
        for t in self.tasks.iter_mut() {
            let top = t.stack.stack_top.min(t.stack.regs.len());
            t.stack.regs[..top].iter_mut().for_each(&mut *f);
        }
        self.regs.iter_mut().for_each(&mut *f);
        self.globals.iter_mut().zip(self.global_rooted).filter(|(_, r)| **r).for_each(|(v, _)| f(v));
        self.scratch.iter_mut().for_each(&mut *f);
        self.specials.iter_mut().for_each(&mut *f);
        // Handles are old objects, which do not move.
        for &code in &self.running {
            f(&mut unsafe { (*code).handle.get() });
        }
        self.roots.retain(|w| match w.upgrade() {
            Some(cell) => {
                let mut v = cell.get();
                f(&mut v);
                cell.set(v);
                true
            }
            None => false,
        });
    }

    fn visit_unrooted(&mut self, f: &mut dyn FnMut(&mut Value)) {
        for (_, globals) in self.retired.iter() {
            globals.iter().for_each(|&g| f(&mut self.globals[g as usize]));
        }
        for &code in self.jit_callers {
            // Live codes; nothing else refers to the slots meanwhile.
            if let Some(callees) = unsafe { (*code.cast_mut()).jit.callees.get_mut() } {
                callees.iter_mut().for_each(&mut *f);
            }
        }
        for &code in self.young_consts {
            let code = unsafe { &mut *code.cast_mut() };
            let h = code.handle.get().as_ptr();
            for (i, v) in code.consts.iter_mut().enumerate() {
                f(v);
                unsafe { set_field(h, i, *v) };
            }
        }
    }

    /// The closures a marked code's JIT code expects live. A retired module
    /// is used by a marked code, by an evaluation running in it, or through
    /// the imports (of variables, or of macros it defines) of a module that
    /// is not retired or is used; its globals' values live then.
    fn retained(&mut self, marked: &dyn Fn(Value) -> bool, shade: &mut dyn FnMut(Value)) {
        for &code in self.jit_callers {
            let code = unsafe { &*code };
            if marked(code.handle.get()) {
                code.jit.callees.get().into_iter().flatten().for_each(|&v| shade(v));
            }
        }
        if self.retired.is_empty() {
            return;
        }
        let global_module = self.global_module;
        // A macro's expansions refer to the bindings of its module, and of
        // the modules of the aliases in its rules.
        let mut defined: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
        for ((m, _), b) in self.bindings {
            if let GlobalBinding::Macro(mac) = b
                && !mac.modules.is_empty()
            {
                defined.entry(*m).or_default().extend(mac.modules.iter().copied());
            }
        }
        let uses = |i: u32| -> Vec<u32> {
            let imported = self.modules[i as usize].imports.values().flat_map(|b| match b {
                GlobalBinding::Var(g) => vec![global_module[*g as usize]],
                GlobalBinding::Macro(mac) => std::iter::once(mac.module).chain(mac.modules.iter().copied()).collect(),
            });
            imported.chain(defined.get(&i).into_iter().flatten().copied()).collect()
        };
        let mut found: Vec<u32> = self.pinned_modules.clone();
        found.extend((0..self.modules.len() as u32).filter(|&m| !self.modules[m as usize].retired).flat_map(&uses));
        found.extend(
            self.codes.iter().flatten().filter(|c| !c.uses.is_empty() && marked(c.handle.get())).flat_map(|c| c.uses.iter().copied()),
        );
        while !found.is_empty() {
            let new: Vec<u32> =
                found.drain(..).filter(|&m| self.modules.get(m as usize).is_some_and(|m| m.retired) && self.used.insert(m)).collect();
            for &m in &new {
                if let Some((_, globals)) = self.retired.iter().find(|(r, _)| *r == m) {
                    globals.iter().for_each(|&g| shade(self.globals[g as usize]));
                }
                found.extend(uses(m));
            }
        }
    }

    fn reclaim(&mut self, marked: &dyn Fn(Value) -> bool) {
        for (m, globals) in std::mem::take(self.retired) {
            if self.used.contains(&m) {
                self.retired.push((m, globals));
            } else {
                // Their values are about to be freed.
                globals.iter().for_each(|&g| self.globals[g as usize] = Value::UNDEFINED);
                self.dead_modules.push((m, globals));
            }
        }
        for (i, slot) in self.codes.iter_mut().enumerate() {
            if slot.as_ref().is_some_and(|c| !marked(c.handle.get())) {
                self.dead_codes.extend(slot.take());
                self.free_codes.push(i as u32);
            }
        }
        // The next marking finds them anew.
        self.used.clear();
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    /// A VM with builtins and the Scheme prelude loaded, granted every
    /// capability: a trusted world.
    pub fn new() -> Vm {
        Vm::with_grants(Grants::ALL)
    }

    /// A VM with builtins and the prelude, holding only `grants`.
    pub fn with_grants(grants: Grants) -> Vm {
        let mut vm = Vm::bare_with(grants);
        if let Err(e) = vm.eval_in(ROOT_MODULE, crate::PRELUDE_PATH, crate::PRELUDE) {
            panic!("prelude failed to load: {e}");
        }
        // What the prelude made lives for good: promote it now, so programs
        // start with an empty nursery, and count only their own collections.
        vm.collect();
        vm.heap.stats = Default::default();
        vm
    }

    /// A VM with builtins only, granted every capability.
    pub fn bare() -> Vm {
        Vm::bare_with(Grants::ALL)
    }

    fn bare_with(grants: Grants) -> Vm {
        // What the VM is made of is its world's from the start.
        let account = crate::alloc::Account::new().expect("an account for the world");
        let _charged = account.enter();
        let interrupt = Arc::new(AtomicBool::new(false));
        let mut vm = Vm {
            heap: Heap::new(),
            account,
            entered: None,
            memory_limit: 0,
            pressure: None,
            regs: vec![Value::VOID; 1 << 16],
            frames: Vec::with_capacity(1024),
            handlers: Vec::new(),
            globals: Vec::new(),
            global_rooted: Vec::new(),
            free_globals: Vec::new(),
            global_names: Vec::new(),
            global_module: Vec::new(),
            user_defined: Vec::new(),
            bindings: FxHashMap::default(),
            record_types: FxHashMap::default(),
            modules: Vec::new(),
            free_modules: Vec::new(),
            retired: Vec::new(),
            dead_modules: Vec::new(),
            pinned_modules: Vec::new(),
            generations: 0,
            module_paths: FxHashMap::default(),
            staging: None,
            staged: None,
            current_module: USER_MODULE,
            grants,
            requiring: None,
            files: Vec::new(),
            file_codes: Vec::new(),
            free_files: Vec::new(),
            pinned_files: Vec::new(),
            variable_docs: FxHashMap::default(),
            codes: Vec::new(),
            free_codes: Vec::new(),
            compiling: Vec::new(),
            dead_codes: Vec::new(),
            jit_jobs: Vec::new(),
            jit_job_ids: 0,
            jit_callers: Vec::new(),
            young_consts: Vec::new(),
            jit: None,
            jit_threshold: u32::MAX,
            jit_error: None,
            jit_unwind: Vec::new(),
            raise_trace: Vec::new(),
            interrupt: interrupt.clone(),
            stops: Arc::new(crate::stop::Stops::new(interrupt, Arc::default())),
            executions: Vec::new(),
            active: Vec::new(),
            thread: std::thread::current(),
            wake_notifier: None,
            natives: Vec::new(),
            apply_native: Value::VOID,
            stack_top: 0,
            scratch: Vec::new(),
            specials: [Value::VOID; SPECIALS],
            roots: Vec::new(),
            foreign: Vec::new(),
            foreign_free: Vec::new(),
            foreign_type_names: FxHashMap::default(),
            next_id: 0,
            tasks: Vec::new(),
            live_tasks: Vec::new(),
            task_turn: 0,
            channels: Vec::new(),
            offers: Default::default(),
            current_task: None,
            masks: Vec::new(),
            locals: FxHashMap::default(),
            out: BufWriter::with_capacity(1 << 16, std::io::stdout()),
        };
        vm.heap.charge_to(vm.account.reference());
        vm.memory_limit = held::default_limit();
        vm.account.set_limits(vm.memory_limit, vm.memory_limit.saturating_mul(3), vm.memory_waker());
        vm.new_module("root", None);
        vm.new_module("user", None);
        vm.specials[SpecialObj::ErrorRtd as usize] = vm.make_rtd("error", &["message", "irritants", "kind"]);
        vm.specials[SpecialObj::ContinuationRtd as usize] = vm.make_rtd("continuation", &["id"]);
        vm.specials[SpecialObj::ValuesRtd as usize] = vm.make_rtd("values", &[]);
        vm.specials[SpecialObj::TaskRtd as usize] = vm.make_rtd("task", &["id"]);
        vm.specials[SpecialObj::ChannelRtd as usize] = vm.make_rtd("channel", &["id"]);
        crate::builtins::install(&mut vm);
        let apply = vm.global_var(ROOT_MODULE, reader::intern("apply"));
        vm.apply_native = vm.globals[apply as usize];
        vm.set_jit(jit_threshold_from_env());
        vm
    }

    pub fn special(&self, s: SpecialObj) -> Value {
        self.specials[s as usize]
    }

    pub fn fresh_id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    fn make_rtd(&mut self, name: &str, fields: &[&str]) -> Value {
        let names = Sexp::list_of(fields.iter().map(|f| Sexp::Sym(reader::intern(f))).collect());
        let names = self.constant(&names);
        let id = self.fresh_id();
        let p = self.heap.alloc_old_unremembered(5);
        unsafe {
            *p = header(Kind::Rtd, 4, 0);
            set_field(p, 0, Value::symbol(reader::intern(name)));
            set_field(p, 1, names);
            set_field(p, 2, Value::int_unchecked(id));
            set_field(p, 3, Value::FALSE);
        }
        Value::ptr(p)
    }

    // ----- modules and globals -----

    pub(crate) fn new_module(&mut self, name: &str, path: Option<PathBuf>) -> u32 {
        let module = Module {
            name: name.into(),
            path,
            imports: FxHashMap::default(),
            exports: None,
            defined: Vec::new(),
            loading: false,
            isolated: false,
            generation: self.staging.as_ref().map_or(0, |s| s.generation),
            retired: false,
        };
        match self.free_modules.pop() {
            Some(m) => {
                self.modules[m as usize] = module;
                m
            }
            None => {
                self.modules.push(module);
                self.modules.len() as u32 - 1
            }
        }
    }

    /// The binding `sym` denotes at top level of `module`: its own definitions,
    /// then imports, then the root module (unless the module is isolated).
    pub fn lookup_global(&self, module: u32, sym: u32) -> Option<GlobalBinding> {
        let m = &self.modules[module as usize];
        self.bindings
            .get(&(module, sym))
            .or_else(|| m.imports.get(&sym))
            .or_else(|| if m.isolated { None } else { self.bindings.get(&(ROOT_MODULE, sym)) })
            .cloned()
    }

    fn new_global(&mut self, module: u32, sym: u32) -> u32 {
        let g = match self.free_globals.pop() {
            Some(g) => {
                let i = g as usize;
                (self.globals[i], self.global_names[i], self.global_module[i]) = (Value::UNDEFINED, sym, module);
                (self.user_defined[i], self.global_rooted[i]) = (false, true);
                g
            }
            None => {
                self.globals.push(Value::UNDEFINED);
                self.global_names.push(sym);
                self.global_module.push(module);
                self.user_defined.push(false);
                self.global_rooted.push(true);
                self.globals.len() as u32 - 1
            }
        };
        self.bindings.insert((module, sym), GlobalBinding::Var(g));
        g
    }

    /// The value of a root-module global defined by the runtime.
    pub fn global_value(&mut self, name: &str) -> Result<Value, Error> {
        let g = self.global_var(ROOT_MODULE, reader::intern(name));
        let v = self.globals[g as usize];
        if v == Value::UNDEFINED { Err(Error::new(format!("{name} is not defined"))) } else { Ok(v) }
    }

    /// Variable for a free reference; creates a forward reference in `module`.
    pub fn global_var(&mut self, module: u32, sym: u32) -> u32 {
        match self.lookup_global(module, sym) {
            Some(GlobalBinding::Var(g)) => g,
            _ => self.new_global(module, sym),
        }
    }

    /// Variable for a definition in `module`, shadowing imports and root.
    pub fn define_var(&mut self, module: u32, sym: u32) -> u32 {
        let g = match self.bindings.get(&(module, sym)) {
            Some(GlobalBinding::Var(g)) => *g,
            _ => {
                self.modules[module as usize].defined.push(sym);
                self.new_global(module, sym)
            }
        };
        if module != ROOT_MODULE || self.globals[g as usize] == Value::UNDEFINED || !self.globals[g as usize].is_native() {
            self.user_defined[g as usize] = true;
        }
        g
    }

    pub fn define_macro(&mut self, module: u32, sym: u32, m: Rc<Macro>) {
        if !self.bindings.contains_key(&(module, sym)) {
            self.modules[module as usize].defined.push(sym);
        }
        self.bindings.insert((module, sym), GlobalBinding::Macro(m));
    }

    /// The capabilities this world holds.
    pub fn grants(&self) -> Grants {
        self.grants
    }

    /// Define natives (with `f`) that need `cap`: in a world without it, each
    /// becomes a procedure that raises "NAME: not granted".
    pub fn requiring<T>(&mut self, cap: Capability, f: impl FnOnce(&mut Vm) -> T) -> T {
        let outer = self.requiring.replace(cap);
        let result = f(self);
        self.requiring = outer;
        result
    }

    pub fn define_native(&mut self, mut native: Native) {
        if let Some(cap) = self.requiring.filter(|c| !self.grants.has(*c)) {
            let message = format!("{}: not granted in this world (needs {})", native.name, cap.name());
            native.f = NativeImpl::Boxed(Rc::new(move |_: &mut Vm, _, _| Err(Error::new(message.clone()))));
            (native.min, native.max) = (0, None);
        }
        let g = self.global_var(ROOT_MODULE, reader::intern(&native.name));
        self.globals[g as usize] = Value::native(self.natives.len() as u32);
        self.natives.push(native);
    }

    /// Document the native `name`, defined already (`document!`), taking
    /// from `min` to `max` arguments as its signature says.
    pub fn document_native(&mut self, name: &str, min: usize, max: Option<usize>, doc: &'static NativeDoc) {
        let v = self.get_global_in(ROOT_MODULE, name).filter(|v| v.is_native());
        let n = &mut self.natives[v.unwrap_or_else(|| panic!("document!: no native {name}")).as_native()];
        // A native this world is not granted takes any arguments.
        let granted = (n.min, n.max) != (0, None) || (min, max) == (0, None);
        assert!(
            !granted || (n.min, n.max) == (min, max),
            "{name}: the signature takes {min}-{max:?} arguments, the native {}-{:?}",
            n.min,
            n.max
        );
        n.doc = Some(doc);
    }

    /// True if calls to global `g` may compile to an inline instruction.
    pub fn inlinable(&self, g: u32) -> bool {
        !self.user_defined[g as usize] && self.global_module[g as usize] == ROOT_MODULE
    }

    pub fn global_name(&self, g: u32) -> Rc<str> {
        symbol_name(self.global_names[g as usize])
    }

    /// Value of a global defined in the user module or root, by name.
    pub fn get_global(&self, name: &str) -> Option<Value> {
        self.get_global_in(USER_MODULE, name)
    }

    /// Value of a global visible from `module`, by name.
    pub fn get_global_in(&self, module: u32, name: &str) -> Option<Value> {
        match self.lookup_global(module, reader::intern(name))? {
            GlobalBinding::Var(g) => Some(self.globals[g as usize]).filter(|v| *v != Value::UNDEFINED),
            GlobalBinding::Macro(_) => None,
        }
    }

    pub fn set_global(&mut self, name: &str, v: Value) {
        let g = self.define_var(USER_MODULE, reader::intern(name));
        self.globals[g as usize] = v;
    }

    pub fn provide(&mut self, module: u32, syms: Vec<u32>) {
        self.modules[module as usize].exports.get_or_insert_with(Vec::new).extend(syms.into_iter().map(|s| (s, s)));
    }

    /// A module's name: `root`, `user`, or the canonical path of its file.
    pub fn module_name(&self, module: u32) -> Rc<str> {
        self.modules[module as usize].name.clone()
    }

    /// What `module` provides or exports, as importers see the names;
    /// `None` if it does not say (it has no `provide`).
    pub fn module_exports(&self, module: u32) -> Option<Vec<Rc<str>>> {
        let exports = self.modules[module as usize].exports.as_ref()?;
        Some(exports.iter().map(|(_, seen)| symbol_name(*seen)).collect())
    }

    /// The names of the modules loaded, each once, in the order loaded.
    pub fn loaded_module_names(&self) -> Vec<Rc<str>> {
        let mut seen = rustc_hash::FxHashSet::default();
        self.modules.iter().map(|m| m.name.clone()).filter(|n| !n.is_empty() && seen.insert(n.clone())).collect()
    }

    /// The names `module` itself defines, in no order.
    pub fn module_definitions(&self, module: u32) -> Vec<Rc<str>> {
        self.bindings.keys().filter(|(m, _)| *m == module).map(|(_, s)| symbol_name(*s)).collect()
    }

    /// The module of the evaluation in progress.
    pub fn current_module(&self) -> u32 {
        self.current_module
    }

    /// Make `module` current for the rest of the evaluation in progress, and
    /// for the following ones of the REPL or session that runs it.
    pub fn set_current_module(&mut self, module: u32) {
        self.current_module = module;
    }

    /// Like `find_module`, but only among modules already loaded.
    pub fn loaded_module(&self, name: &str) -> Option<u32> {
        // A file's current module (a package's published generation), else
        // the newest module of that name.
        let file = Path::new(name).canonicalize().ok().and_then(|p| self.module_paths.get(&p).copied());
        file.or_else(|| self.modules.iter().rposition(|m| &*m.name == name && !m.retired).map(|m| m as u32))
    }

    /// The module called `name`, or the module of the file at path `name`
    /// (relative to the working directory), loading the file if needed.
    pub fn find_module(&mut self, name: &str) -> Result<u32, Error> {
        if let Some(m) = self.loaded_module(name) {
            return Ok(m);
        }
        self.check_loading()?;
        let path = Path::new(name).canonicalize().map_err(|_| Error::new(format!("no module {name}")))?;
        self.load_module(&path).map_err(|e| Error::new(format!("module {name}: {}", e.msg)))
    }

    pub(crate) fn check_loading(&self) -> Result<(), Error> {
        match self.grants.has(Capability::Loading) {
            true => Ok(()),
            false => Err(Error::new("loading modules is not granted in this world (needs loading)")),
        }
    }

    /// The module of the file at canonical `path`, loaded once.
    pub(crate) fn load_module(&mut self, path: &Path) -> Result<u32, Error> {
        // A package's own files load afresh for each generation.
        if let Some(staging) = &self.staging
            && path.starts_with(&staging.dir)
        {
            if let Some(&m) = staging.modules.get(path) {
                return if self.modules[m as usize].loading { Err(Error::new("circular module dependency")) } else { Ok(m) };
            }
            let text = std::fs::read_to_string(path).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
            let m = self.new_module(&path.to_string_lossy(), Some(path.to_path_buf()));
            self.staging.as_mut().expect("staging").modules.insert(path.to_path_buf(), m);
            self.modules[m as usize].loading = true;
            let result = self.eval_in(m, &path.to_string_lossy(), &text);
            self.modules[m as usize].loading = false;
            return result.map(|_| m);
        }
        match self.module_paths.get(path) {
            Some(&m) if self.modules[m as usize].loading => Err(Error::new("circular module dependency")),
            Some(&m) => Ok(m),
            None => {
                let text = std::fs::read_to_string(path).map_err(|e| Error::new(e.to_string()))?;
                let m = self.new_module(&path.to_string_lossy(), Some(path.to_path_buf()));
                self.module_paths.insert(path.to_path_buf(), m);
                self.modules[m as usize].loading = true;
                let result = self.eval_in(m, &path.to_string_lossy(), &text);
                self.modules[m as usize].loading = false;
                result.map(|_| m)
            }
        }
    }

    /// Load a new generation of the package whose main file is `path`: it
    /// and the files it requires from its directory load into fresh
    /// modules, which `publish_staged` makes the ones `require` and module
    /// names find, and `discard_staged` retires. Returns the main module and
    /// the generation. Nothing else changes unless the package's code does
    /// it; if loading fails, the generation is retired.
    pub fn stage_package(&mut self, path: &Path) -> Result<(u32, u32), Error> {
        if self.staging.is_some() || self.staged.is_some() {
            return Err(Error::new("a package is already being loaded"));
        }
        self.check_loading()?;
        let path = path.canonicalize().map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        let dir = path.parent().map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        self.generations += 1;
        let generation = self.generations;
        self.staging = Some(Staging { dir, generation, modules: FxHashMap::default() });
        let result = self.load_module(&path);
        let staging = self.staging.take().expect("staging");
        match result {
            Ok(m) => {
                self.staged = Some((generation, staging.modules));
                Ok((m, generation))
            }
            Err(e) => {
                self.retire_generation(generation);
                Err(e)
            }
        }
    }

    /// Make the staged generation's modules the current ones.
    pub fn publish_staged(&mut self) {
        if let Some((_, modules)) = self.staged.take() {
            self.module_paths.extend(modules);
        }
    }

    pub fn discard_staged(&mut self) {
        if let Some((generation, _)) = self.staged.take() {
            self.retire_generation(generation);
        }
    }

    /// Retire a package generation, replaced or unloaded: `require` and
    /// module names no longer find its modules, and its globals stop being
    /// roots. Its modules, globals and code are freed once nothing live
    /// uses them; closures and tasks of the generation keep working until
    /// then.
    pub fn retire_generation(&mut self, generation: u32) {
        let modules: Vec<u32> = (0..self.modules.len() as u32)
            .filter(|&m| generation != 0 && self.modules[m as usize].generation == generation && !self.modules[m as usize].retired)
            .collect();
        for &m in &modules {
            self.modules[m as usize].retired = true;
            let globals: Vec<u32> = (0..self.globals.len() as u32).filter(|&g| self.global_module[g as usize] == m).collect();
            for &g in &globals {
                self.global_rooted[g as usize] = false;
            }
            self.retired.push((m, globals));
        }
        self.module_paths.retain(|_, m| !modules.contains(m));
    }

    /// Arenas of JIT machine code not freed yet.
    pub fn live_jit_arenas(&self) -> usize {
        self.jit.as_ref().map_or(0, |j| j.live_arenas())
    }

    /// Modules alive now (freed ones are reused).
    pub fn live_modules(&self) -> usize {
        self.modules.len() - self.free_modules.len()
    }

    /// Globals alive now (freed ones are reused).
    pub fn live_globals(&self) -> usize {
        self.globals.len() - self.free_globals.len()
    }

    /// Free the modules of retired generations the last collection found
    /// unused (their globals' values are already cleared).
    fn release_dead_modules(&mut self) {
        for (m, globals) in std::mem::take(&mut self.dead_modules) {
            for g in globals {
                self.record_types.remove(&g);
                self.variable_docs.remove(&g);
                // Of no module: a module reusing `m`'s slot does not own it.
                (self.global_names[g as usize], self.global_module[g as usize]) = (0, NO_MODULE);
                self.free_globals.push(g);
            }
            self.bindings.retain(|&(module, _), _| module != m);
            let module = &mut self.modules[m as usize];
            (module.imports, module.exports, module.defined) = (FxHashMap::default(), None, Vec::new());
            (module.name, module.path) = ("".into(), None);
            self.free_modules.push(m);
        }
    }

    /// Load (once) the module at `spec`, relative to `from`'s file, and import
    /// its exports (all definitions when it has no `provide`) into `from`.
    pub fn require(&mut self, from: u32, spec: &str) -> Result<(), Error> {
        self.as_execution(|vm| vm.require_running(from, spec))
    }

    fn require_running(&mut self, from: u32, spec: &str) -> Result<(), Error> {
        self.check_loading().map_err(|e| Error::new(format!("require {spec}: {}", e.msg)))?;
        let base =
            self.modules[from as usize].path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."));
        let path = base.join(spec);
        let path = path.canonicalize().map_err(|e| Error::new(format!("require {spec}: {e}")))?;
        let m = self.load_module(&path).map_err(|e| match e.msg.as_str() {
            "circular module dependency" => Error::new(format!("require {spec}: circular module dependency")),
            _ => e,
        })?;
        for (name, binding) in self.exported(m, spec)? {
            self.modules[from as usize].imports.insert(name, binding);
        }
        Ok(())
    }

    /// The bindings module `m` exports (all its definitions when it has no
    /// `provide`), by the names importers see.
    pub(crate) fn exported(&self, m: u32, what: &str) -> Result<Vec<(u32, GlobalBinding)>, Error> {
        let module = &self.modules[m as usize];
        let names = module.exports.clone().unwrap_or_else(|| module.defined.iter().map(|&s| (s, s)).collect());
        names
            .into_iter()
            .map(|(inside, outside)| {
                // Its own definitions, or what it imported (re-exports).
                let binding = self.lookup_global(m, inside);
                binding.map(|b| (outside, b)).ok_or_else(|| Error::new(format!("{what} provides undefined {}", symbol_name(inside))))
            })
            .collect()
    }

    // ----- code and constants (used by the compiler) -----

    /// Add a code, with its handle: its constants and the handles of the
    /// codes it makes closures of.
    pub fn add_code(&mut self, mut code: Code) -> u32 {
        // Only a package generation's modules are retired.
        let mut uses: Vec<u32> = code
            .ops
            .iter()
            .filter_map(Op::global)
            .map(|g| self.global_module[g as usize])
            .filter(|&m| self.modules.get(m as usize).is_some_and(|m| m.generation != 0))
            .collect();
        uses.sort_unstable();
        uses.dedup();
        code.uses = uses.into();
        let nested = code.ops.iter().filter(|op| matches!(op, Op::Closure { .. })).count();
        let n = code.consts.len() + nested;
        let h = self.heap.alloc_old_unremembered(1 + n);
        unsafe {
            *h = header(Kind::Code, n, 0);
            let handles = code.ops.iter().filter_map(|op| match *op {
                Op::Closure { code, .. } => Some(self.code(code).handle.get()),
                _ => None,
            });
            for (i, v) in code.consts.iter().copied().chain(handles).enumerate() {
                set_field(h, i, v);
            }
        }
        code.handle.set(Value::ptr(h));
        let young = code.consts.iter().any(|v| v.is_ptr() && self.heap.in_nursery(v.as_ptr()));
        let i = match self.free_codes.pop() {
            Some(i) => {
                self.codes[i as usize] = Some(Box::new(code));
                i
            }
            None => {
                self.codes.push(Some(Box::new(code)));
                self.codes.len() as u32 - 1
            }
        };
        self.compiling.push(i);
        if young {
            self.young_consts.push(self.code(i));
        }
        let file = self.code(i).file as usize;
        self.file_codes[file] += 1;
        i
    }

    /// The code at index `i`, which is live.
    pub(crate) fn code(&self, i: u32) -> &Code {
        self.codes[i as usize].as_deref().expect("live code")
    }

    pub fn set_captures(&mut self, code: u32, captures: Vec<CapSrc>) {
        self.codes[code as usize].as_mut().expect("live code").captures = captures;
    }

    /// Codes alive now.
    pub fn live_codes(&self) -> usize {
        self.codes.len() - self.free_codes.len()
    }

    /// Materialise a literal of code: its pairs, vectors, strings and
    /// bytevectors cannot be changed. Heap parts go to the old space; they
    /// only reference each other, so they need no remembering.
    pub fn constant(&mut self, s: &Sexp) -> Value {
        self.build_datum(s, heap::IMMUTABLE)
    }

    /// Materialise a datum that can be changed (what `read` returns).
    pub fn datum(&mut self, s: &Sexp) -> Value {
        self.build_datum(s, 0)
    }

    fn build_datum(&mut self, s: &Sexp, flags: u64) -> Value {
        let mut labels = Labels { flags, ..Labels::default() };
        let v = self.constant_in(s, &mut labels);
        if labels.placeholders.is_empty() { v } else { labels.patch(v) }
    }

    fn constant_in(&mut self, s: &Sexp, labels: &mut Labels) -> Value {
        match s {
            Sexp::List(..) | Sexp::Vector(_) | Sexp::Labeled(..) | Sexp::Complex(..) | Sexp::Ratio(_) => {
                crate::nested(|| self.constant_of(s, labels))
            }
            _ => self.constant_of(s, labels),
        }
    }

    fn constant_of(&mut self, s: &Sexp, labels: &mut Labels) -> Value {
        match s {
            Sexp::Labeled(n, d) => {
                // References from inside the datum get a placeholder,
                // replaced once the datum exists.
                let placeholder = self.heap.alloc_old_unremembered(3);
                unsafe {
                    *placeholder = header(Kind::Pair, 2, 0);
                    set_field(placeholder, 0, Value::UNSET);
                    set_field(placeholder, 1, Value::UNSET);
                }
                let placeholder = Value::ptr(placeholder);
                labels.values.insert(*n, placeholder);
                let v = self.constant_in(d, labels);
                labels.values.insert(*n, v);
                labels.placeholders.push((placeholder, v));
                v
            }
            Sexp::LabelRef(n) => labels.values.get(n).copied().unwrap_or(Value::UNSET),
            Sexp::Int(i) => Value::fixnum(*i).unwrap_or_else(|| self.constant_in(&Sexp::BigInt(Rc::new((*i).into())), labels)),
            Sexp::BigInt(b) => {
                use num_traits::Signed;
                let limbs = b.magnitude().to_u64_digits();
                let p = self.heap.alloc_old_unremembered(1 + limbs.len());
                unsafe {
                    *p = header(Kind::BigInt, limbs.len(), if b.is_negative() { heap::NEGATIVE } else { 0 });
                    std::ptr::copy_nonoverlapping(limbs.as_ptr(), p.add(1), limbs.len());
                }
                Value::ptr(p)
            }
            Sexp::Ratio(r) => {
                let part = |b: &num_bigint::BigInt| match num_traits::ToPrimitive::to_i64(b) {
                    Some(i) => Sexp::Int(i),
                    None => Sexp::BigInt(Rc::new(b.clone())),
                };
                let n = self.constant_in(&part(r.numer()), labels);
                let d = self.constant_in(&part(r.denom()), labels);
                let p = self.heap.alloc_old_unremembered(3);
                unsafe {
                    *p = header(Kind::Ratio, 2, 0);
                    set_field(p, 0, n);
                    set_field(p, 1, d);
                }
                Value::ptr(p)
            }
            Sexp::Complex(re, im) => {
                let re = self.constant_in(re, labels);
                let im = self.constant_in(im, labels);
                let p = self.heap.alloc_old_unremembered(3);
                unsafe {
                    *p = header(Kind::Complex, 2, 0);
                    set_field(p, 0, re);
                    set_field(p, 1, im);
                }
                Value::ptr(p)
            }
            Sexp::Float(f) => Value::float(*f),
            Sexp::Bool(b) => Value::bool(*b),
            Sexp::Char(c) => Value::char(*c),
            Sexp::Sym(id) => Value::symbol(reader::strip(*id)),
            Sexp::Keyword(id) => Value::keyword(*id),
            Sexp::Str(s) => {
                let p = self.heap.alloc_old_unremembered(heap::string_words(s.len()));
                unsafe {
                    init_string(p, s.as_bytes());
                    *p |= labels.flags;
                }
                Value::ptr(p)
            }
            Sexp::Bytes(b) => {
                let p = self.heap.alloc_old_unremembered(heap::string_words(b.len()));
                unsafe {
                    init_bytes(p, b);
                    *p |= labels.flags;
                }
                Value::ptr(p)
            }
            Sexp::List(items, tail, _) => {
                // In reading order, so that a label is defined before it is used.
                let cars: Vec<Value> = items.iter().map(|i| self.constant_in(i, labels)).collect();
                let mut acc = tail.as_ref().map_or(Value::NIL, |t| self.constant_in(t, labels));
                for car in cars.into_iter().rev() {
                    let p = self.heap.alloc_old_unremembered(3);
                    unsafe {
                        *p = header(Kind::Pair, 2, labels.flags);
                        set_field(p, 0, car);
                        set_field(p, 1, acc);
                    }
                    acc = Value::ptr(p);
                }
                acc
            }
            Sexp::Vector(items) => {
                let vals: Vec<Value> = items.iter().map(|i| self.constant_in(i, labels)).collect();
                let p = self.heap.alloc_old_unremembered(1 + vals.len());
                unsafe {
                    *p = header(Kind::Vector, vals.len(), labels.flags);
                    for (i, v) in vals.into_iter().enumerate() {
                        set_field(p, i, v);
                    }
                }
                Value::ptr(p)
            }
        }
    }

    // ----- allocation and GC -----

    #[inline(always)]
    pub fn alloc(&mut self, words: usize) -> *mut u64 {
        if self.heap.has_room(words) {
            return self.heap.bump(words);
        }
        self.alloc_slow(words)
    }

    /// `words` in the nursery (collecting first if needed); for blocks that
    /// are split into several objects. `words` must fit the nursery.
    pub fn reserve_nursery(&mut self, words: usize) -> *mut u64 {
        debug_assert!(words <= self.heap.nursery_capacity());
        if !self.heap.has_room(words) {
            self.collect();
        }
        self.heap.bump(words)
    }

    /// Large objects, and any that would not fit an empty nursery (it can
    /// be smaller than `LARGE_WORDS`: `TECHNE_NURSERY_KB`), go to the old
    /// generation.
    #[cold]
    fn alloc_slow(&mut self, words: usize) -> *mut u64 {
        let _charged = self.account.enter();
        if words >= LARGE_WORDS || words > self.heap.nursery_capacity() {
            self.check_memory();
            return self.heap.alloc_old(words);
        }
        self.collect();
        self.heap.bump(words)
    }

    pub fn collect(&mut self) {
        self.collect_with(Heap::collect);
        self.check_memory();
    }

    pub fn full_collect(&mut self) {
        self.collect_with(Heap::full_collect);
        // Back under, it is under pressure no more.
        if self.held().total() <= self.memory_limit {
            self.pressure = None;
        }
    }

    fn collect_with(&mut self, collect: fn(&mut Heap, &mut dyn Roots)) {
        let _charged = self.account.enter();
        let running = self.running_codes();
        let jit_callers = std::mem::take(&mut self.jit_callers);
        let young_consts = std::mem::take(&mut self.young_consts);
        let pinned_modules = self.pinned_modules.iter().copied().chain([self.current_module]).collect();
        let Vm {
            heap,
            regs,
            globals,
            global_rooted,
            global_module,
            modules,
            retired,
            dead_modules,
            codes,
            free_codes,
            dead_codes,
            bindings,
            scratch,
            specials,
            roots,
            stack_top,
            tasks,
            ..
        } = self;
        let mut r = VmRoots {
            regs: &mut regs[..*stack_top],
            globals,
            global_rooted,
            global_module,
            modules,
            retired,
            dead_modules,
            pinned_modules,
            used: Default::default(),
            codes,
            free_codes,
            dead_codes,
            running,
            jit_callers: &jit_callers,
            bindings,
            young_consts: &young_consts,
            scratch,
            specials,
            roots,
            tasks,
        };
        collect(heap, &mut r);
        self.jit_callers = jit_callers;
        // A collection empties the nursery; codes it found dead are dropped
        // with them below.
        self.young_consts = young_consts;
        self.young_consts.retain(|&c| unsafe { (*c).consts.iter().any(|v| v.is_ptr() && heap.in_nursery(v.as_ptr())) });
        self.release_dead_foreign();
        self.release_dead_codes();
        self.release_dead_modules();
        // Only now: the codes dropped above were looked at until then.
        self.heap.release_empty();
    }

    /// The codes running or about to: frames and handlers of every stack,
    /// suspended tasks, native frames handed over, compilations.
    fn running_codes(&self) -> Vec<*const Code> {
        let main = self.frames.iter().map(|f| f.code).chain(self.handlers.iter().filter_map(Handler::code));
        let tasks = self.tasks.iter().flat_map(|t| t.stack.codes().chain(t.resume.as_ref().map(|s| s.code)));
        let compiling = self.compiling.iter().map(|&i| self.code(i) as *const Code);
        main.chain(tasks).chain(self.jit_unwind.iter().map(|f| f.code)).chain(compiling).filter(|c| !c.is_null()).collect()
    }

    fn release_dead_codes(&mut self) {
        for code in std::mem::take(&mut self.dead_codes) {
            let addr: *const Code = &*code;
            if code.jit.entry.get().is_some()
                && let Some(jit) = self.jit.as_mut()
            {
                jit.release(code.jit.arena.get());
            }
            self.jit_callers.retain(|&c| c != addr);
            self.young_consts.retain(|&c| c != addr);
            self.jit_jobs.retain(|(_, c, cancelled)| {
                if *c == addr {
                    cancelled.store(true, Ordering::Relaxed);
                }
                *c != addr
            });
            self.file_codes[code.file as usize] -= 1;
            self.release_file(code.file);
        }
    }

    fn release_dead_foreign(&mut self) {
        for i in std::mem::take(&mut self.heap.dead_foreign) {
            self.foreign[i] = None;
            self.foreign_free.push(i);
        }
    }

    /// Keep `v` alive and up to date across collections while the `Root` lives.
    pub fn root(&mut self, v: Value) -> Root {
        // The host may root between executions, or another VM on this
        // thread may be running.
        let _charged = self.charging();
        let cell = Rc::new(Cell::new(v));
        self.roots.push(Rc::downgrade(&cell));
        Root::from_cell(cell)
    }

    /// Wrap a Rust value as a Scheme object. It is dropped when the object dies.
    pub fn make_foreign(&mut self, value: Rc<dyn Any>, type_name: &'static str) -> Value {
        let index = match self.foreign_free.pop() {
            Some(i) => {
                self.foreign[i] = Some((value, type_name));
                i
            }
            None => {
                self.foreign.push(Some((value, type_name)));
                self.foreign.len() - 1
            }
        };
        let p = self.alloc(2);
        unsafe {
            *p = header(Kind::Foreign, 1, 0);
            *p.add(1) = index as u64;
        }
        self.heap.register_foreign(p);
        Value::ptr(p)
    }

    pub fn foreign(&self, v: Value) -> Option<(&Rc<dyn Any>, &'static str)> {
        if !is_kind(v, Kind::Foreign) {
            return None;
        }
        let i = unsafe { *v.as_ptr().add(1) } as usize;
        self.foreign[i].as_ref().map(|(rc, name)| (rc, *name))
    }

    pub fn live_foreign_count(&self) -> usize {
        self.foreign.iter().filter(|f| f.is_some()).count()
    }

    pub fn alloc_pair(&mut self, car: Value, cdr: Value) -> Value {
        // Root both operands across a possible collection.
        self.scratch.push(car);
        self.scratch.push(cdr);
        let p = self.alloc(3);
        let cdr = self.scratch.pop().unwrap();
        let car = self.scratch.pop().unwrap();
        unsafe {
            *p = header(Kind::Pair, 2, 0);
            set_field(p, 0, car);
            set_field(p, 1, cdr);
        }
        Value::ptr(p)
    }

    /// Allocate a string. Bytevectors, not strings, hold arbitrary bytes.
    pub fn make_string(&mut self, text: &str) -> Value {
        let bytes = text.as_bytes();
        let p = self.alloc(heap::string_words(bytes.len()));
        unsafe { init_string(p, bytes) };
        Value::ptr(p)
    }

    pub fn make_bytevector(&mut self, bytes: &[u8]) -> Value {
        let p = self.alloc(heap::string_words(bytes.len()));
        unsafe { init_bytes(p, bytes) };
        Value::ptr(p)
    }

    /// A record of type `rtd` with `fields`; all inputs are rooted meanwhile.
    pub(crate) fn make_record(&mut self, rtd: Value, fields: &[Value]) -> Value {
        let mark = self.scratch.len();
        self.scratch.push(rtd);
        self.scratch.extend_from_slice(fields);
        let p = self.alloc(2 + fields.len());
        unsafe {
            *p = header(Kind::Record, 1 + fields.len(), 0);
            for (i, v) in self.scratch.drain(mark..).enumerate() {
                set_field(p, i, v);
            }
        }
        Value::ptr(p)
    }

    /// A vector of `items`, which are rooted meanwhile.
    pub fn make_vector(&mut self, items: &[Value]) -> Value {
        let mark = self.scratch.len();
        self.scratch.extend_from_slice(items);
        let words = 1 + items.len();
        let p = if words >= LARGE_WORDS { self.heap.alloc_old(words) } else { self.alloc(words) };
        unsafe {
            *p = header(Kind::Vector, items.len(), 0);
            for (i, v) in self.scratch.drain(mark..).enumerate() {
                set_field(p, i, v);
            }
        }
        Value::ptr(p)
    }

    /// A list of `items`, which are rooted meanwhile.
    pub fn make_list(&mut self, items: &[Value]) -> Value {
        let mark = self.scratch.len();
        self.scratch.extend_from_slice(items);
        let mut bulk = crate::builtins::Bulk::reserve(self, 3 * items.len());
        let mut acc = Value::NIL;
        for i in (mark..self.scratch.len()).rev() {
            acc = bulk.pair(self, self.scratch[i], acc);
        }
        self.scratch.truncate(mark);
        acc
    }

    /// Box an integer, as a fixnum when it fits.
    pub fn make_int(&mut self, i: i64) -> Value {
        Value::fixnum(i).unwrap_or_else(|| {
            let p = self.alloc(2);
            unsafe {
                *p = header(Kind::BigInt, 1, if i < 0 { heap::NEGATIVE } else { 0 });
                *p.add(1) = i.unsigned_abs();
            }
            Value::ptr(p)
        })
    }

    /// An error object with `message` and `irritants`.
    pub fn make_error_object(&mut self, message: &str, irritants: &[Value]) -> Value {
        self.make_error_object_of(message, irritants, ErrorKind::General)
    }

    /// An error object; its `kind` field is `#f`, `file` or `read`.
    pub fn make_error_object_of(&mut self, message: &str, irritants: &[Value], kind: ErrorKind) -> Value {
        let mark = self.scratch.len();
        self.scratch.extend_from_slice(irritants);
        let msg = self.make_string(message);
        self.scratch.push(msg);
        let items: Vec<Value> = self.scratch[mark..self.scratch.len() - 1].to_vec();
        let list = self.make_list(&items);
        let msg = self.scratch[self.scratch.len() - 1];
        self.scratch.truncate(mark);
        let rtd = self.special(SpecialObj::ErrorRtd);
        let kind = match kind {
            ErrorKind::General | ErrorKind::Exit(_) | ErrorKind::Killed => Value::FALSE,
            ErrorKind::File => Value::symbol(reader::intern("file")),
            ErrorKind::Read => Value::symbol(reader::intern("read")),
        };
        self.make_record(rtd, &[msg, list, kind])
    }

    #[inline(always)]
    pub fn write_barrier(&mut self, obj: *mut u64, v: Value) {
        self.heap.barrier(obj, v);
    }

    // ----- running code -----

    /// Evaluate source text in the user module.
    pub fn eval_source(&mut self, source: &str) -> Result<Value, Error> {
        self.eval_in(USER_MODULE, "<input>", source)
    }

    /// Run a file as the main module (requires resolve relative to it).
    pub fn eval_file(&mut self, path: &Path) -> Result<Value, Error> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.modules[USER_MODULE as usize].path = Some(canonical.clone());
        self.module_paths.insert(canonical, USER_MODULE);
        self.eval_in(USER_MODULE, &path.to_string_lossy(), &text)
    }

    /// Evaluate source text in `*module` for a REPL or session, then set
    /// `*module` to the module current at the end (`in-module` switches it).
    pub fn eval_interactive(&mut self, module: &mut u32, name: &str, source: &str) -> Result<Value, Error> {
        let saved = std::mem::replace(&mut self.current_module, *module);
        let result = self.eval_forms(*module, name, source);
        *module = std::mem::replace(&mut self.current_module, saved);
        result
    }

    /// Evaluate source text in `module`, the file `name`.
    pub fn eval_in(&mut self, module: u32, name: &str, source: &str) -> Result<Value, Error> {
        let saved = std::mem::replace(&mut self.current_module, module);
        let result = self.eval_forms(module, name, source);
        self.current_module = saved;
        result
    }

    fn eval_forms(&mut self, module: u32, name: &str, source: &str) -> Result<Value, Error> {
        self.as_execution(|vm| vm.pinning(|vm| vm.eval_forms_pinned(module, name, source)))
    }

    fn eval_forms_pinned(&mut self, module: u32, name: &str, source: &str) -> Result<Value, Error> {
        let file = self.add_file(name, source);
        let forms = reader::read_located(source).map_err(|e| match e.pos {
            Some(pos) => {
                let (line, col) = reader::line_col(source, pos);
                Error::new(format!("{name}:{line}:{col}: {}", e.message))
            }
            None => Error::new(format!("{name}: {}", e.message)),
        })?;
        // Definitions anywhere in the file shadow imports/root for the whole file.
        for f in &forms {
            predeclare(self, module, f);
        }
        let mut last = Value::VOID;
        for form in &forms {
            let compiling = self.compiling.len();
            let result = Compiler::new(self, module, file).compile_toplevel(form).and_then(|code| {
                if dump_code() {
                    self.dump_from(code);
                }
                self.run(code)
            });
            self.compiling.truncate(compiling);
            last = result?;
        }
        Ok(last)
    }

    /// Compile and run one form in the current module (`eval`).
    pub fn eval_sexp(&mut self, form: &Sexp) -> Result<Value, Error> {
        self.eval_sexp_in(self.current_module, form)
    }

    /// Register source text (for locations in errors); its file index. It
    /// is kept until the evaluation adding it ends (`pinning`), then as long
    /// as codes compiled from it live.
    pub(crate) fn add_file(&mut self, name: &str, text: &str) -> u32 {
        let source = SourceFile { name: name.into(), text: text.into() };
        let file = match self.free_files.pop() {
            Some(i) => {
                self.files[i as usize] = source;
                i
            }
            None => {
                self.files.push(source);
                self.file_codes.push(0);
                self.files.len() as u32 - 1
            }
        };
        self.pinned_files.push(file);
        file
    }

    /// Run `f`, keeping the codes it compiles and the files it adds until
    /// it returns.
    fn pinning<T>(&mut self, f: impl FnOnce(&mut Vm) -> T) -> T {
        let (codes, files, modules) = (self.compiling.len(), self.pinned_files.len(), self.pinned_modules.len());
        self.pinned_modules.push(self.current_module);
        let result = f(self);
        self.compiling.truncate(codes);
        self.pinned_modules.truncate(modules);
        for file in self.pinned_files.split_off(files) {
            self.release_file(file);
        }
        result
    }

    /// Free `file` if no code and no evaluation in progress refers to it.
    fn release_file(&mut self, file: u32) {
        if self.file_codes[file as usize] == 0 && !self.pinned_files.contains(&file) {
            self.files[file as usize] = SourceFile { name: "".into(), text: "".into() };
            self.free_files.push(file);
        }
    }

    /// Source files alive now.
    pub fn live_files(&self) -> usize {
        self.files.len() - self.free_files.len()
    }

    /// Compile and run one top-level form of source file `file` in `module`.
    pub(crate) fn eval_form(&mut self, module: u32, file: u32, form: &Sexp) -> Result<Value, Error> {
        predeclare(self, module, form);
        let saved = std::mem::replace(&mut self.current_module, module);
        let result = self.pinning(|vm| Compiler::new(vm, module, file).compile_toplevel(form).and_then(|code| vm.run(code)));
        self.current_module = saved;
        result
    }

    /// Compile and run one form in `module`.
    pub fn eval_sexp_in(&mut self, module: u32, form: &Sexp) -> Result<Value, Error> {
        self.as_execution(|vm| vm.eval_sexp_in_running(module, form))
    }

    fn eval_sexp_in_running(&mut self, module: u32, form: &Sexp) -> Result<Value, Error> {
        self.pinning(|vm| {
            let file = vm.add_file("<eval>", "");
            vm.eval_form(module, file, form)
        })
    }

    fn run(&mut self, entry: u32) -> Result<Value, Error> {
        let code: *const Code = self.code(entry);
        let saved_top = self.stack_top;
        let bp = self.stack_top + 1;
        let size = unsafe { (*code).frame_size } as usize;
        self.ensure_regs(bp + size)?;
        self.regs[bp - 1] = Value::VOID;
        self.regs[bp..bp + size].fill(Value::VOID);
        let result = unsafe { self.dispatch(code, bp) };
        self.stack_top = saved_top;
        result
    }

    /// Call a Scheme procedure from Rust. Arguments are copied onto the stack
    /// before anything can allocate.
    pub fn call(&mut self, f: Value, args: &[Value]) -> Result<Value, Error> {
        // Entering an execution allocates nothing the collector moves.
        self.as_execution(|vm| vm.call_running(f, args))
    }

    fn call_running(&mut self, f: Value, args: &[Value]) -> Result<Value, Error> {
        let saved_top = self.stack_top;
        let base = self.stack_top;
        self.ensure_regs(base + 2 + args.len())?;
        self.regs[base] = f;
        self.regs[base + 1..base + 1 + args.len()].copy_from_slice(args);
        self.stack_top = base + 1 + args.len();
        let result = self.call_at(base, args.len());
        self.stack_top = saved_top;
        match result {
            Err(e) if e.wait.is_some() => Err(Error::new(CANNOT_SUSPEND)),
            other => other,
        }
    }

    /// Swap the running stack with `other` (task switch).
    pub(crate) fn swap_stack(&mut self, other: &mut Stack) {
        std::mem::swap(&mut self.regs, &mut other.regs);
        std::mem::swap(&mut self.frames, &mut other.frames);
        std::mem::swap(&mut self.handlers, &mut other.handlers);
        std::mem::swap(&mut self.stack_top, &mut other.stack_top);
        std::mem::swap(&mut self.locals, &mut other.locals);
    }

    pub(crate) fn push_wind(&mut self, after: Root) {
        self.handlers.push(Handler::Wind { after });
    }

    pub(crate) fn push_proc_handler(&mut self, handler: Root) {
        self.handlers.push(Handler::Proc { handler });
    }

    /// Pop the innermost handler entry (end of a dynamic-wind body or a
    /// with-exception-handler thunk).
    pub(crate) fn pop_handler(&mut self) {
        self.handlers.pop();
    }

    /// Pop handler entries down to `keep`, running `dynamic-wind` after-thunks
    /// innermost first (each with the handlers outside it installed).
    pub(crate) fn unwind_to(&mut self, keep: usize) {
        while self.handlers.len() > keep {
            if let Some(Handler::Wind { after }) = self.handlers.pop()
                && !self.killing()
            {
                // An error in an after-thunk does not replace the one unwinding.
                let _ = self.call(after.get(), &[]);
            }
        }
    }

    /// Start a task's entry procedure on the (already swapped-in) task stack.
    pub(crate) fn start_task(&mut self, f: Value) -> Result<Exit, Error> {
        self.ensure_regs(8)?;
        self.regs[0] = f;
        self.stack_top = 1;
        if is_kind(f, Kind::Closure) {
            unsafe {
                let callee = field(f.as_ptr(), 0).as_untraced_ptr::<Code>();
                self.ensure_regs(1 + (*callee).frame_size as usize)?;
                self.enter(callee, 1, 0)?;
                self.dispatch_loop(callee, 0, 1, 0, true)
            }
        } else {
            self.call_at(0, 0).map(Exit::Done)
        }
    }

    /// Continue a suspended task, delivering the awaited result.
    pub(crate) fn resume_task(&mut self, s: Suspend, delivery: Option<Result<Value, Error>>) -> Result<Exit, Error> {
        let (mut code, mut pc, mut bp) = (s.code, s.pc, s.bp);
        match delivery {
            None => {}
            Some(Ok(v)) if s.tail => {
                // The waiting native was in tail position: return its value.
                self.regs[bp - 1] = v;
                match self.frames.pop() {
                    None => return Ok(Exit::Done(v)),
                    Some(fr) => {
                        code = fr.code;
                        pc = fr.pc as usize;
                        bp = fr.bp as usize;
                    }
                }
            }
            Some(Ok(v)) => self.regs[s.slot] = v,
            Some(Err(mut e)) => {
                e.trace.push(self.location(code, pc.saturating_sub(1)));
                match self.catch(e, 1) {
                    Ok((Landing { frames_len, code: c, bp: b, target, dst }, condition)) => {
                        self.frames.truncate(frames_len);
                        code = c;
                        pc = target as usize;
                        bp = b;
                        self.regs[bp + dst as usize] = condition;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        unsafe { self.dispatch_loop(code, pc, bp, 0, true) }
    }

    /// Call the procedure at `regs[base]` with `n` arguments after it.
    fn call_at(&mut self, base: usize, n: usize) -> Result<Value, Error> {
        let f = self.regs[base];
        if is_kind(f, Kind::Closure) {
            unsafe {
                let callee = field(f.as_ptr(), 0).as_untraced_ptr::<Code>();
                let bp = base + 1;
                self.ensure_regs(bp + (*callee).frame_size as usize + n)?;
                self.enter(callee, bp, n)?;
                self.dispatch(callee, bp)
            }
        } else if f == self.apply_native {
            let n = self.spread_apply(base, n)?;
            self.call_at(base, n)
        } else if let Some(p) = Vm::applicable_proc(f) {
            self.regs[base] = p;
            self.call_at(base, n)
        } else if f.is_native() {
            self.call_native(f.as_native(), base + 1, n)
        } else if let Some(e) = self.continuation_escape(f, base, n) {
            Err(e)
        } else {
            Err(Error::new(format!("not a procedure: {}", crate::builtins::brief(f))))
        }
    }

    /// `(apply f a ... list)` at `regs[base]`: rewrite in place to `(f a ... items...)`.
    fn spread_apply(&mut self, base: usize, n: usize) -> Result<usize, Error> {
        if n == 0 {
            return Err(Error::new("apply: expected a procedure"));
        }
        let list = self.regs[base + n];
        let items: Vec<Value> = crate::builtins::list_values(list).ok_or_else(|| Error::new("apply: last argument must be a list"))?;
        self.regs.copy_within(base + 1..base + n, base);
        let fixed = n - 1;
        let total = fixed - 1 + items.len();
        self.ensure_regs(base + 2 + total)?;
        self.regs[base + fixed..base + fixed + items.len()].copy_from_slice(&items);
        self.stack_top = self.stack_top.max(base + 1 + total);
        Ok(total)
    }

    /// The procedure to run when record `f` is called, if its type is applicable.
    #[inline]
    /// Whether `v` can be called: natives, closures, continuations and
    /// applicable records.
    pub fn is_procedure(&self, v: Value) -> bool {
        v.is_native()
            || is_kind(v, Kind::Closure)
            || Vm::applicable_proc(v).is_some()
            || (is_kind(v, Kind::Record) && unsafe { field(v.as_ptr(), 0) } == self.special(SpecialObj::ContinuationRtd))
    }

    pub fn applicable_proc(f: Value) -> Option<Value> {
        if !is_kind(f, Kind::Record) {
            return None;
        }
        unsafe {
            let rtd = field(f.as_ptr(), 0);
            let index = field(rtd.as_ptr(), 3);
            index.is_int().then(|| field(f.as_ptr(), 1 + index.as_int() as usize))
        }
    }

    fn continuation_escape(&mut self, f: Value, base: usize, n: usize) -> Option<Error> {
        let rtd = self.special(SpecialObj::ContinuationRtd);
        if !(is_kind(f, Kind::Record) && unsafe { field(f.as_ptr(), 0) } == rtd) {
            return None;
        }
        let id = unsafe { field(f.as_ptr(), 1) }.as_int();
        let value = if n == 1 {
            self.regs[base + 1]
        } else {
            let args = self.regs[base + 1..base + 1 + n].to_vec();
            let vrtd = self.special(SpecialObj::ValuesRtd);
            self.make_record(vrtd, &args)
        };
        let mut e = Error::new("continuation invoked outside its dynamic extent");
        // Outside its extent nothing would catch the escape: an ordinary error.
        if self.handlers.iter().any(|h| matches!(h, Handler::Escape { id: i, .. } if *i == id)) {
            e.escape = Some((id, self.root(value)));
        }
        Some(e)
    }

    /// Source location of instruction `pc` in `code`.
    fn location(&self, code: *const Code, pc: usize) -> String {
        let code = unsafe { &*code };
        let pos = code.spans.get(pc).copied().unwrap_or(NO_POS);
        let file = &self.files[code.file as usize];
        if pos == NO_POS {
            return format!("{} ({})", code.name, file.name);
        }
        let (line, col) = reader::line_col(&file.text, pos);
        format!("{} ({}:{line}:{col})", code.name, file.name)
    }

    /// Find a handler for error `e` raised in the dispatch level whose base
    /// frame is at `base_bp`, innermost first. Handler procedures run right
    /// here at the raise point, whatever level installed them (they do not
    /// unwind). A `guard` of this level is returned to unwind to; a `guard` of
    /// an outer level ends the search, and the error propagates there (through
    /// natives such as `dynamic-wind`).
    fn catch(&mut self, mut e: Error, base_bp: usize) -> Result<(Landing, Value), Error> {
        // Killed meanwhile (in an after thunk, say): no handler sees it.
        if !e.reaches_host() && self.killing() {
            e = Error::new(KILLED).with_kind(ErrorKind::Killed);
        }
        let mut idx = e.searched.map_or(usize::MAX, |i| i as usize).min(self.handlers.len());
        while idx > 0 {
            if e.escape.is_none() && self.masked(idx - 1) {
                idx -= 1;
                continue;
            }
            let h = self.handlers[idx - 1].clone();
            match h {
                Handler::Wind { .. } => {}
                // Nested dispatch levels start above their caller's registers, so
                // a landing below this level's base frame belongs to an outer level.
                Handler::Escape { bp, .. } | Handler::Guard { bp, .. } if bp < base_bp => break,
                Handler::Escape { id, frames_len, code, bp, target, dst } => {
                    if let Some((eid, value)) = &e.escape
                        && *eid == id
                    {
                        let value = value.clone();
                        self.unwind_to(idx - 1);
                        if self.killing() {
                            return Err(Error::new(KILLED).with_kind(ErrorKind::Killed));
                        }
                        return Ok((Landing { frames_len, code, bp, target, dst }, value.get()));
                    }
                }
                Handler::Guard { frames_len, code, bp, target, dst } if e.escape.is_none() && !e.reaches_host() => {
                    let condition = self.condition_of(&mut e);
                    self.unwind_to(idx - 1);
                    // Killed in an after thunk unwinding to here.
                    if self.killing() {
                        return Err(Error::new(KILLED).with_kind(ErrorKind::Killed));
                    }
                    return Ok((Landing { frames_len, code, bp, target, dst }, condition.get()));
                }
                Handler::Proc { handler } if e.escape.is_none() && !e.reaches_host() => {
                    // Run at the raise point; raises inside go to outer handlers.
                    let condition = self.condition_of(&mut e);
                    let trace: Vec<String> = e
                        .trace
                        .iter()
                        .cloned()
                        .chain(self.frames.iter().rev().take(32).map(|f| self.location(f.code, f.pc as usize - 1)))
                        .collect();
                    let outer = std::mem::replace(&mut self.raise_trace, trace);
                    let result = self.call_masked(idx - 1, handler.get(), condition.get());
                    self.raise_trace = outer;
                    e = match result {
                        // A handler returning from a non-continuable raise is itself an error.
                        Ok(_) => Error::new(format!("exception handler returned from non-continuable raise: {}", e.msg)),
                        // Re-raising the same condition keeps the original trace.
                        Err(inner) if inner.escape.is_none() && inner.payload.as_ref().map(|p| p.get()) == Some(condition.get()) => e,
                        Err(inner) => inner,
                    };
                    if e.escape.is_some() {
                        // The handler escaped (e.g. invoked a restart): its target
                        // may be anywhere on the stack, including above the handler.
                        idx = self.handlers.len();
                        continue;
                    }
                }
                Handler::Guard { .. } | Handler::Proc { .. } => {}
            }
            idx -= 1;
        }
        e.searched = Some(idx as u32);
        Err(e)
    }

    fn masked(&self, index: usize) -> bool {
        self.masks.iter().any(|&(lo, hi)| index >= lo && index < hi)
    }

    /// Call handler `h` for `condition` with handler entries from `lo` up
    /// hidden from raises.
    fn call_masked(&mut self, lo: usize, h: Value, condition: Value) -> Result<Value, Error> {
        self.masks.push((lo, self.handlers.len()));
        let depth = self.masks.len();
        let result = self.call(h, &[condition]);
        self.masks.truncate(depth - 1);
        result
    }

    /// The raised object of `e`, making an error object for VM/Rust errors.
    fn condition_of(&mut self, e: &mut Error) -> Root {
        if let Some(p) = &e.payload {
            return p.clone();
        }
        let msg = e.msg.clone();
        let obj = self.make_error_object_of(&msg, &[], e.kind);
        let root = self.root(obj);
        e.payload = Some(root.clone());
        root
    }

    /// The interpreter loop. `code`/`pc`/`bp` and the register base pointer
    /// are kept in locals and written back only around calls.
    unsafe fn dispatch(&mut self, code: *const Code, bp: usize) -> Result<Value, Error> {
        let base_frames = self.frames.len();
        let base_handlers = self.handlers.len();
        let result = match unsafe { self.dispatch_loop(code, 0, bp, base_frames, false) } {
            Ok(Exit::Done(v)) => Ok(v),
            Ok(Exit::Suspend(_)) => unreachable!("non-suspendable dispatch suspended"),
            Err(e) => Err(e),
        };
        if result.is_err() {
            self.unwind_to(base_handlers);
            self.frames.truncate(base_frames);
        }
        result
    }

    /// `base_bp` is the frame base of this dispatch level's first frame (a
    /// resumed task continues deeper than that).
    unsafe fn dispatch_loop(
        &mut self,
        code: *const Code,
        pc: usize,
        bp: usize,
        base_frames: usize,
        suspendable: bool,
    ) -> Result<Exit, Error> {
        // Separate instantiations: only task code pays for preemption checks.
        unsafe {
            if suspendable {
                self.dispatch_loop_impl::<true>(code, pc, bp, base_frames, 1)
            } else {
                self.dispatch_loop_impl::<false>(code, pc, bp, base_frames, bp)
            }
        }
    }

    // Macros refresh `r` defensively after calls; some paths overwrite it again.
    #[allow(unused_assignments)]
    unsafe fn dispatch_loop_impl<const SUSPENDABLE: bool>(
        &mut self,
        mut code: *const Code,
        mut pc: usize,
        mut bp: usize,
        base_frames: usize,
        base_bp: usize,
    ) -> Result<Exit, Error> {
        let suspendable = SUSPENDABLE;
        unsafe {
            let mut fuel: u32 = TASK_SLICE;
            let mut ops: *const Op = (*code).ops.as_ptr();
            self.ensure_regs(bp + (*code).frame_size as usize)?;
            let mut r: *mut Value = self.regs.as_mut_ptr().add(bp);

            macro_rules! reg {
                ($i:expr) => {
                    *r.add($i as usize)
                };
            }
            macro_rules! sync_top {
                () => {
                    self.stack_top = self.stack_top.max(bp + (*code).frame_size as usize)
                };
            }
            // Raise: leave the instruction loop for the handling below it,
            // which unwinds to a guard in this dispatch level or returns the
            // error to the caller. (Out of line: the loop stays small.)
            macro_rules! fail {
                ($e:expr) => {{
                    let e: Error = $e;
                    break e;
                }};
            }
            macro_rules! tryv {
                ($e:expr) => {
                    match $e {
                        Ok(v) => v,
                        Err(e) => fail!(e),
                    }
                };
            }
            // Return `v` from the current frame.
            macro_rules! ret {
                ($v:expr) => {{
                    let v: Value = $v;
                    *r.sub(1) = v;
                    if self.frames.len() == base_frames {
                        return Ok(Exit::Done(v));
                    }
                    let fr = self.frames.pop().unwrap();
                    code = fr.code;
                    ops = (*code).ops.as_ptr();
                    pc = fr.pc as usize;
                    bp = fr.bp as usize;
                    r = self.regs.as_mut_ptr().add(bp);
                }};
            }
            // Count a call or backward jump; preempt a task when its slice is used.
            macro_rules! tick {
                () => {
                    if SUSPENDABLE {
                        fuel -= 1;
                        if fuel == 0 {
                            sync_top!();
                            return Ok(Exit::Suspend(Suspend { code, pc, bp, slot: 0, tail: false, wait: None }));
                        }
                    }
                };
            }
            // Fixnum fast path, then float fast path, then the generic slow path.
            macro_rules! arith {
                ($dst:expr, $x:expr, $y:expr, $iop:ident, $fop:tt, $slow:path) => {{
                    let (x, y) = ($x, $y);
                    reg!($dst) = if Value::both_int(x, y)
                        && let Some(v) = x.as_int().$iop(y.as_int()).and_then(Value::fixnum)
                    {
                        v
                    } else if x.is_float() && y.is_float() {
                        Value::float(x.as_float() $fop y.as_float())
                    } else {
                        sync_top!();
                        tryv!($slow(self, x, y))
                    };
                }};
            }
            macro_rules! cmp {
                ($x:expr, $y:expr, $op:tt, $slow:path) => {{
                    let (x, y) = ($x, $y);
                    if Value::both_int(x, y) {
                        x.as_int() $op y.as_int()
                    } else if x.is_float() && y.is_float() {
                        x.as_float() $op y.as_float()
                    } else {
                        tryv!($slow(x, y))
                    }
                }};
            }
            macro_rules! call_closure {
                ($f:expr, $base:expr, $n:expr, $tail:expr) => {{
                    let f: Value = $f;
                    let (base, n) = ($base as usize, $n as usize);
                    let callee = field(f.as_ptr(), 0).as_untraced_ptr::<Code>();
                    let hot = &(*callee).jit.hot;
                    let calls = hot.get().wrapping_add(1);
                    hot.set(calls);
                    if calls & 255 == 0 || calls == self.jit_threshold {
                        if let Err(e) = self.safepoint(callee, calls) {
                            fail!(e);
                        }
                    }
                    // The callee finds its closure at `bp - 1`.
                    *r.add(base) = f;
                    let new_bp = if $tail {
                        // Slide callee and arguments down over the current frame.
                        std::ptr::copy(r.add(base), r.sub(1), n + 1);
                        bp
                    } else {
                        self.frames.push(Frame { code, pc: pc as u32, bp: bp as u32 });
                        bp + base + 1
                    };
                    let size = (*callee).frame_size as usize;
                    if let Err(e) = self.ensure_regs(new_bp + size.max(n)) {
                        if !$tail {
                            self.frames.pop();
                        }
                        fail!(e);
                    }
                    if !(*callee).rest && n == (*callee).nparams as usize {
                        // Zero bits encode the float 0.0: a safe non-pointer for the GC.
                        std::ptr::write_bytes(self.regs.as_mut_ptr().add(new_bp + n), 0, size.saturating_sub(n));
                    } else {
                        if !$tail {
                            self.frames.pop();
                        }
                        r = self.regs.as_mut_ptr().add(bp);
                        let e = self.enter(callee, new_bp, n);
                        if let Err(e) = e {
                            fail!(e);
                        }
                        if !$tail {
                            self.frames.push(Frame { code, pc: pc as u32, bp: bp as u32 });
                        }
                    }
                    code = callee;
                    ops = (*code).ops.as_ptr();
                    pc = 0;
                    bp = new_bp;
                    r = self.regs.as_mut_ptr().add(bp);
                    tick!();
                }};
            }
            // Call a non-closure: natives (with `apply` spread in place) and
            // escape continuations.
            macro_rules! call_other {
                ($f:expr, $base:expr, $n:expr, $tail:expr) => {{
                    let (mut f, base, mut n): (Value, usize, usize) = ($f, $base as usize, $n as usize);
                    reg!(base) = f;
                    sync_top!();
                    self.stack_top = self.stack_top.max(bp + base + 1 + n);
                    if f == self.apply_native {
                        n = tryv!(self.spread_apply(bp + base, n));
                        r = self.regs.as_mut_ptr().add(bp);
                        f = reg!(base);
                    }
                    if let Some(p) = Vm::applicable_proc(f) {
                        f = p;
                        reg!(base) = f;
                    }
                    if is_kind(f, Kind::Closure) {
                        call_closure!(f, base, n, $tail);
                    } else {
                        let v = if f.is_native() {
                            match self.call_native(f.as_native(), bp + base + 1, n) {
                                Ok(v) => v,
                                Err(mut e) if e.wait.is_some() => {
                                    if suspendable {
                                        sync_top!();
                                        let wait = e.wait.take();
                                        return Ok(Exit::Suspend(Suspend { code, pc, bp, slot: bp + base, tail: $tail, wait }));
                                    }
                                    fail!(Error::new(CANNOT_SUSPEND))
                                }
                                Err(e) => fail!(e),
                            }
                        } else if let Some(e) = self.continuation_escape(f, bp + base, n) {
                            fail!(e)
                        } else {
                            fail!(Error::new(format!("not a procedure: {}", crate::builtins::brief(f))))
                        };
                        r = self.regs.as_mut_ptr().add(bp);
                        if $tail {
                            ret!(v);
                        } else {
                            reg!(base) = v;
                        }
                    }
                }};
            }

            loop {
                let mut e: Error = loop {
                    let op = *ops.add(pc);
                    pc += 1;
                    match op {
                        Op::LoadK { dst, k } => {
                            let consts: &[Value] = &(*code).consts;
                            reg!(dst) = *consts.get_unchecked(k as usize);
                        }
                        Op::LoadI { dst, i } => reg!(dst) = Value::int_unchecked(i as i64),
                        Op::Mov { dst, src } => reg!(dst) = reg!(src),
                        Op::GetG { dst, g } => {
                            let v = *self.globals.get_unchecked(g as usize);
                            if v == Value::UNDEFINED {
                                fail!(Error::new(format!("unbound variable: {}", self.global_name(g))));
                            }
                            reg!(dst) = v;
                        }
                        Op::SetG { g, src } => *self.globals.get_unchecked_mut(g as usize) = reg!(src),
                        Op::GetC { dst, i } => reg!(dst) = field((*r.sub(1)).as_ptr(), 1 + i as usize),
                        Op::GetCB { dst, i } => {
                            let b = field((*r.sub(1)).as_ptr(), 1 + i as usize);
                            reg!(dst) = field(b.as_ptr(), 0);
                        }
                        Op::SetCB { i, src } => {
                            let b = field((*r.sub(1)).as_ptr(), 1 + i as usize).as_ptr();
                            let v = reg!(src);
                            set_field(b, 0, v);
                            self.write_barrier(b, v);
                        }
                        Op::MkBox { r: x } => {
                            sync_top!();
                            let p = self.alloc(2);
                            *p = header(Kind::Box, 1, 0);
                            set_field(p, 0, reg!(x));
                            reg!(x) = Value::ptr(p);
                        }
                        Op::Unbox { dst, r: x } => reg!(dst) = field(reg!(x).as_ptr(), 0),
                        Op::SetBox { r: x, src } => {
                            let b = reg!(x).as_ptr();
                            let v = reg!(src);
                            set_field(b, 0, v);
                            self.write_barrier(b, v);
                        }
                        Op::Closure { dst, code: c } => {
                            let target: *const Code = self.codes.get_unchecked(c as usize).as_deref().unwrap_unchecked();
                            let n = (*target).captures.len();
                            sync_top!();
                            let p = self.alloc(2 + n);
                            *p = header(Kind::Closure, 1 + n, 0);
                            // Codes are boxed, so the address is stable; the closure keeps
                            // its code alive (`Code::handle`).
                            set_field(p, 0, Value::untraced_ptr(target));
                            let current = *r.sub(1);
                            for (i, src) in (*target).captures.iter().enumerate() {
                                let v = match *src {
                                    CapSrc::Reg(x) => reg!(x),
                                    CapSrc::Cap(j) => field(current.as_ptr(), 1 + j as usize),
                                };
                                set_field(p, 1 + i, v);
                            }
                            reg!(dst) = Value::ptr(p);
                        }
                        Op::PushHandler { dst, t } => {
                            self.handlers.push(Handler::Guard { frames_len: self.frames.len(), code, bp, target: t, dst });
                        }
                        Op::PopHandler => {
                            self.handlers.pop();
                        }
                        Op::PushEscape { k, dst, t } => {
                            sync_top!();
                            let id = self.fresh_id();
                            let rtd = self.special(SpecialObj::ContinuationRtd);
                            reg!(k) = self.make_record(rtd, &[Value::int_unchecked(id)]);
                            self.handlers.push(Handler::Escape { id, frames_len: self.frames.len(), code, bp, target: t, dst });
                        }

                        Op::Jmp { t } => pc = t as usize,
                        Op::Loop { t } => {
                            pc = t as usize;
                            let hot = &(*code).jit.hot;
                            let n = hot.get().wrapping_add(1);
                            hot.set(n);
                            if n & 255 == 0 || n == self.jit_threshold {
                                if let Err(e) = self.safepoint(code, n) {
                                    fail!(e);
                                }
                                ops = (*code).ops.as_ptr();
                            }
                            tick!();
                        }
                        Op::EnterJit => {
                            sync_top!();
                            let f = (*code).jit.entry.get().unwrap_unchecked();
                            // Outside tasks, native code returns every POLL_SLICE
                            // calls or back-edges so interrupts are delivered.
                            let mut slice = POLL_SLICE;
                            let fuel_ptr: *mut u32 = if SUSPENDABLE { &mut fuel } else { &mut slice };
                            let vm = self as *mut Vm;
                            let (heap_top, heap_end) = self.heap.bump_pointers();
                            let mut ctx = crate::jit::JitCtx {
                                globals: self.globals.as_mut_ptr(),
                                regs_end: self.regs.as_mut_ptr().add(self.regs.len()),
                                fuel: fuel_ptr,
                                stack_top: std::ptr::addr_of_mut!((*vm).stack_top),
                                heap_top,
                                heap_end,
                                code,
                                bp: bp as u64,
                                tail: 0,
                                res: 0,
                            };
                            let mut res = f(vm, &mut ctx, r, bp as u64, (pc - 1) as u32, 0);
                            // Trampoline for tail calls between compiled functions.
                            while (res >> 32) as u32 == crate::jit::TAILCALL {
                                let next: crate::jit::JitFn = std::mem::transmute(ctx.tail);
                                res = next(vm, &mut ctx, r, bp as u64, 0, 0);
                            }
                            let status = (res >> 32) as u32;
                            pc = res as u32 as usize;
                            if status == crate::jit::RETURNED {
                                r = self.regs.as_mut_ptr().add(bp);
                                ret!(*r.sub(1));
                                continue;
                            }
                            // Continue in the frame native code stopped in, below
                            // the frames of the native calls that led there.
                            self.frames.extend(self.jit_unwind.drain(..).rev());
                            code = ctx.code;
                            ops = (*code).ops.as_ptr();
                            bp = ctx.bp as usize;
                            // A native called from JIT code may have grown the register stack.
                            r = self.regs.as_mut_ptr().add(bp);
                            match status {
                                crate::jit::EXIT | crate::jit::RESUME => {}
                                crate::jit::ERROR => {
                                    let e = self.jit_error.take().expect("JIT error");
                                    fail!(e)
                                }
                                crate::jit::STEP => {
                                    // A case native code leaves to Rust: run the
                                    // original instruction, then continue after it.
                                    let op = (*code).jit.ops.get().unwrap()[pc];
                                    match self.jit_slow_op(r, op) {
                                        Ok(false) => pc += 1,
                                        Ok(true) => pc = crate::jit::jump_target(&op),
                                        Err(e) => {
                                            pc += 1;
                                            fail!(e)
                                        }
                                    }
                                }
                                crate::jit::RET_MOVED => {
                                    let (Op::TailCall { base, .. } | Op::TailCallG { base, .. }) = (*code).jit.ops.get().unwrap()[pc - 1]
                                    else {
                                        unreachable!()
                                    };
                                    ret!(*r.add(base as usize));
                                }
                                crate::jit::TICK => {
                                    // Polled first: a task's compiled loop would
                                    // otherwise be suspended and resumed for good.
                                    if let Err(e) = self.poll_interrupt() {
                                        // The slice is used up: what handles the
                                        // stop starts another (else counting on
                                        // from zero would not preempt the task).
                                        fuel = TASK_SLICE;
                                        fail!(e);
                                    }
                                    if SUSPENDABLE {
                                        return Ok(Exit::Suspend(Suspend { code, pc, bp, slot: 0, tail: false, wait: None }));
                                    }
                                }
                                _ => {
                                    let mut e = self.jit_error.take().expect("JIT wait");
                                    if !SUSPENDABLE {
                                        fail!(Error::new(CANNOT_SUSPEND))
                                    }
                                    let call = (*code).jit.ops.get().unwrap()[pc - 1];
                                    let (Op::Call { base, .. }
                                    | Op::CallG { base, .. }
                                    | Op::TailCall { base, .. }
                                    | Op::TailCallG { base, .. }) = call
                                    else {
                                        unreachable!()
                                    };
                                    let tail = matches!(call, Op::TailCall { .. } | Op::TailCallG { .. });
                                    let wait = e.wait.take();
                                    return Ok(Exit::Suspend(Suspend { code, pc, bp, slot: bp + base as usize, tail, wait }));
                                }
                            }
                        }
                        Op::Jf { c, t } => {
                            if reg!(c).is_false() {
                                pc = t as usize;
                            }
                        }
                        Op::Jt { c, t } => {
                            if reg!(c).is_truthy() {
                                pc = t as usize;
                            }
                        }
                        Op::JNLt { a, b, t } => {
                            if !cmp!(reg!(a), reg!(b), <, num::lt) {
                                pc = t as usize;
                            }
                        }
                        Op::JNLe { a, b, t } => {
                            if !cmp!(reg!(a), reg!(b), <=, num::le) {
                                pc = t as usize;
                            }
                        }
                        Op::JNNumEq { a, b, t } => {
                            if !cmp!(reg!(a), reg!(b), ==, num::num_eq) {
                                pc = t as usize;
                            }
                        }
                        Op::JNLtI { a, i, t } => {
                            if !cmp!(reg!(a), Value::int_unchecked(i as i64), <, num::lt) {
                                pc = t as usize;
                            }
                        }
                        Op::JNGtI { a, i, t } => {
                            if !cmp!(Value::int_unchecked(i as i64), reg!(a), <, num::lt) {
                                pc = t as usize;
                            }
                        }
                        Op::JNEqI { a, i, t } => {
                            if !cmp!(reg!(a), Value::int_unchecked(i as i64), ==, num::num_eq) {
                                pc = t as usize;
                            }
                        }
                        Op::JNEq { a, b, t } => {
                            if reg!(a) != reg!(b) {
                                pc = t as usize;
                            }
                        }
                        Op::JNNull { a, t } => {
                            if reg!(a) != Value::NIL {
                                pc = t as usize;
                            }
                        }
                        Op::JNPair { a, t } => {
                            if !is_kind(reg!(a), Kind::Pair) {
                                pc = t as usize;
                            }
                        }

                        Op::Add { dst, a, b } => arith!(dst, reg!(a), reg!(b), checked_add, +, num::add),
                        Op::AddI { dst, a, i } => {
                            let x = reg!(a);
                            reg!(dst) = if x.is_int()
                                && let Some(v) = Value::fixnum(x.as_int() + i as i64)
                            {
                                v
                            } else {
                                sync_top!();
                                tryv!(num::add(self, x, Value::int_unchecked(i as i64)))
                            };
                        }
                        Op::Sub { dst, a, b } => arith!(dst, reg!(a), reg!(b), checked_sub, -, num::sub),
                        Op::Mul { dst, a, b } => arith!(dst, reg!(a), reg!(b), checked_mul, *, num::mul),
                        Op::Quo { dst, a, b } => {
                            sync_top!();
                            reg!(dst) = tryv!(num::quotient(self, reg!(a), reg!(b)));
                        }
                        Op::Rem { dst, a, b } => {
                            sync_top!();
                            reg!(dst) = tryv!(num::remainder(self, reg!(a), reg!(b)));
                        }
                        Op::Mod { dst, a, b } => {
                            let (x, y) = (reg!(a), reg!(b));
                            reg!(dst) = if Value::both_int(x, y) && y.as_int() != 0 {
                                Value::int_unchecked(num::modulo_i64(x.as_int(), y.as_int()))
                            } else {
                                sync_top!();
                                tryv!(num::modulo(self, x, y))
                            };
                        }
                        Op::Lt { dst, a, b } => reg!(dst) = Value::bool(cmp!(reg!(a), reg!(b), <, num::lt)),
                        Op::Le { dst, a, b } => reg!(dst) = Value::bool(cmp!(reg!(a), reg!(b), <=, num::le)),
                        Op::NumEq { dst, a, b } => reg!(dst) = Value::bool(cmp!(reg!(a), reg!(b), ==, num::num_eq)),
                        Op::Car { dst, a } => {
                            let x = reg!(a);
                            if !is_kind(x, Kind::Pair) {
                                fail!(crate::builtins::type_error("car", "pair", x));
                            }
                            reg!(dst) = field(x.as_ptr(), 0);
                        }
                        Op::Cdr { dst, a } => {
                            let x = reg!(a);
                            if !is_kind(x, Kind::Pair) {
                                fail!(crate::builtins::type_error("cdr", "pair", x));
                            }
                            reg!(dst) = field(x.as_ptr(), 1);
                        }
                        Op::Cons { dst, a, b } => {
                            sync_top!();
                            let p = self.alloc(3);
                            *p = header(Kind::Pair, 2, 0);
                            set_field(p, 0, reg!(a));
                            set_field(p, 1, reg!(b));
                            reg!(dst) = Value::ptr(p);
                        }
                        Op::NullP { dst, a } => reg!(dst) = Value::bool(reg!(a) == Value::NIL),
                        Op::PairP { dst, a } => reg!(dst) = Value::bool(is_kind(reg!(a), Kind::Pair)),
                        Op::Not { dst, a } => reg!(dst) = Value::bool(reg!(a).is_false()),
                        Op::EqP { dst, a, b } => reg!(dst) = Value::bool(reg!(a) == reg!(b)),
                        Op::VRef { dst, v, i } => {
                            let (x, k) = (reg!(v), reg!(i));
                            if !is_kind(x, Kind::Vector) || !k.is_int() || k.as_int() as u64 >= heap::len_of(x.as_ptr()) as u64 {
                                fail!(crate::builtins::index_error("vector-ref", x, k));
                            }
                            reg!(dst) = field(x.as_ptr(), k.as_int() as usize);
                        }
                        Op::VSet { v, i, x } => {
                            let (vec, k, val) = (reg!(v), reg!(i), reg!(x));
                            if !heap::is_changeable(vec, Kind::Vector)
                                || !k.is_int()
                                || k.as_int() as u64 >= heap::len_of(vec.as_ptr()) as u64
                            {
                                fail!(crate::builtins::vector_set_error(vec, k));
                            }
                            set_field(vec.as_ptr(), k.as_int() as usize, val);
                            self.write_barrier(vec.as_ptr(), val);
                        }

                        Op::CallG { base, n, g } | Op::TailCallG { base, n, g } => {
                            let f = *self.globals.get_unchecked(g as usize);
                            let tail = matches!(op, Op::TailCallG { .. });
                            if is_kind(f, Kind::Closure) {
                                call_closure!(f, base, n, tail);
                            } else if f == Value::UNDEFINED {
                                fail!(Error::new(format!("unbound variable: {}", self.global_name(g))));
                            } else {
                                call_other!(f, base, n, tail);
                            }
                        }
                        Op::Call { base, n } | Op::TailCall { base, n } => {
                            let f = reg!(base);
                            let tail = matches!(op, Op::TailCall { .. });
                            if is_kind(f, Kind::Closure) {
                                call_closure!(f, base, n, tail);
                            } else {
                                call_other!(f, base, n, tail);
                            }
                        }
                        Op::Ret { r: x } => ret!(reg!(x)),
                    }
                };
                // A raise from the instruction before `pc`.
                if e.escape.is_none() {
                    e.trace.push(self.location(code, pc.saturating_sub(1)));
                }
                sync_top!();
                match self.catch(e, base_bp) {
                    Ok((Landing { frames_len, code: c, bp: b, target, dst }, condition)) => {
                        self.frames.truncate(frames_len);
                        code = c;
                        ops = (*code).ops.as_ptr();
                        pc = target as usize;
                        bp = b;
                        r = self.regs.as_mut_ptr().add(bp);
                        reg!(dst) = condition;
                    }
                    Err(mut e) => {
                        if e.escape.is_none() {
                            e.trace
                                .extend(self.frames[base_frames..].iter().rev().take(16).map(|f| self.location(f.code, f.pc as usize - 1)));
                        }
                        return Err(e);
                    }
                }
            }
        }
    }

    #[inline(always)]
    fn ensure_regs(&mut self, needed: usize) -> Result<(), Error> {
        if needed > self.regs.len() {
            return self.grow_regs(needed);
        }
        Ok(())
    }

    #[cold]
    fn grow_regs(&mut self, needed: usize) -> Result<(), Error> {
        if needed > MAX_REGS {
            return Err(Error::new("stack overflow: recursion too deep"));
        }
        let len = (self.regs.len() * 2).min(MAX_REGS).max(needed);
        self.admit_without_collecting((len - self.regs.len()) * std::mem::size_of::<Value>())?;
        self.regs.resize(len, Value::VOID);
        Ok(())
    }

    /// Bound minor collection pauses by allocating at most `bytes` between
    /// them (see `Heap::set_nursery_window`; about 0.8 ms per MiB on the
    /// reference host). `TECHNE_NURSERY_KB` sets the nursery's capacity.
    pub fn set_nursery_window(&mut self, bytes: usize) {
        self.heap.set_nursery_window(bytes / 8);
    }

    /// Enable the JIT, compiling functions after `threshold` loop iterations,
    /// or disable it (`None`) for code that has not been compiled yet. The
    /// default comes from `TECHNE_JIT`.
    /// Compilation happens on a background thread; with a threshold of 1 or
    /// `TECHNE_JIT_SYNC` set, each function is compiled before it continues.
    /// The compiler thread, once started, lives as long as the VM: it owns
    /// the memory of the code it compiled (see `jit::Compiler`).
    pub fn set_jit(&mut self, threshold: Option<u32>) {
        let sync = threshold == Some(1) || std::env::var_os("TECHNE_JIT_SYNC").is_some();
        if let Some(t) = threshold
            && self.jit.is_none()
        {
            self.jit = crate::jit::Compiler::new(t, sync, self.account.reference()).map(Box::new);
        }
        if let Some(jit) = &mut self.jit {
            jit.threshold = threshold.map(|t| t.max(1));
            jit.sync = sync;
        }
        self.jit_threshold = self.jit.as_ref().and_then(|j| j.threshold).unwrap_or(u32::MAX);
    }

    pub(crate) fn jit_pop_handler(&mut self) {
        self.handlers.pop();
    }

    pub(crate) fn jit_push_frame(&mut self, code: *const Code, pc: u32, bp: u32) {
        self.jit_unwind.push(Frame { code, pc, bp });
    }

    /// Queue a hot function for compilation (or compile it now in sync mode).
    #[cold]
    unsafe fn jit_compile(&mut self, code: *const Code) {
        let c = unsafe { &mut *(code as *mut Code) };
        if c.jit.entry.get().is_some() || c.jit.failed.get() || self.jit.as_ref().is_none_or(|j| j.threshold.is_none()) {
            return;
        }
        // Compiling is optional: not while there is no room for it.
        if !crate::jit::Compiler::fits(c.ops.len(), self.room()) {
            return;
        }
        // Submitted once: a function that cannot be compiled is not retried.
        c.jit.failed.set(true);
        // Entry points: the start, loop heads and returns from calls.
        let mut heads: Vec<usize> = std::iter::once(0)
            .chain(c.ops.iter().enumerate().filter_map(|(pc, op)| match op {
                Op::Loop { t } => Some(*t as usize),
                Op::Call { .. } | Op::CallG { .. } => Some(pc + 1),
                _ => None,
            }))
            .filter(|&t| t < c.ops.len() && crate::jit::native(&c.ops[t]))
            .collect();
        heads.sort_unstable();
        heads.dedup();
        if heads.is_empty() {
            return;
        }
        let orig = c.jit.ops.get_or_init(|| c.ops.clone().into_boxed_slice());
        let captures = orig
            .iter()
            .filter_map(|op| match *op {
                Op::Closure { code, .. } => Some((
                    code,
                    self.code(code)
                        .captures
                        .iter()
                        .filter_map(|s| match *s {
                            CapSrc::Reg(r) => Some(r),
                            CapSrc::Cap(_) => None,
                        })
                        .collect(),
                )),
                _ => None,
            })
            .collect();
        // Globals called that hold closures now: calls of the same closure
        // get the inline path, specialised for its code.
        let mut known: Vec<u32> = orig
            .iter()
            .filter_map(|op| match *op {
                Op::CallG { g, .. } | Op::TailCallG { g, .. } if is_kind(self.globals[g as usize], Kind::Closure) => Some(g),
                _ => None,
            })
            .collect();
        known.sort_unstable();
        known.dedup();
        if c.jit.callees.get().is_none() && !known.is_empty() {
            self.jit_callers.push(c);
        }
        let callees = c.jit.callees.get_or_init(|| known.iter().map(|&g| self.globals[g as usize]).collect());
        self.jit_job_ids += 1;
        let (id, cancelled) = (self.jit_job_ids, Arc::new(AtomicBool::new(false)));
        let job = crate::jit::Job {
            id,
            cancelled: cancelled.clone(),
            code: code as usize,
            generation: c.generation,
            name: c.name.to_string(),
            ops: orig.to_vec(),
            ops_addr: orig.as_ptr() as usize,
            consts: c.consts.iter().map(|v| v.bits()).collect(),
            consts_addr: c.consts.as_ptr() as usize,
            frame_size: c.frame_size,
            nparams: c.nparams,
            rest: c.rest,
            heads,
            apply: self.apply_native.bits(),
            captures,
            closure_globals: known
                .iter()
                .zip(callees.iter())
                .map(|(&g, closure)| {
                    let callee = unsafe { &*field(closure.as_ptr(), 0).as_untraced_ptr::<Code>() };
                    let known = crate::jit::Known {
                        closure: closure as *const Value as usize,
                        code: callee as *const Code as usize,
                        nparams: callee.nparams,
                        rest: callee.rest,
                        frame_size: callee.frame_size,
                    };
                    (g, known)
                })
                .collect(),
        };
        let jit = self.jit.as_mut().unwrap();
        if jit.submit(job) {
            self.jit_jobs.push((id, code, cancelled));
        }
        if jit.sync {
            self.jit_install();
        }
    }

    /// Install functions the compiler thread has finished.
    #[cold]
    fn jit_install(&mut self) {
        let Some(jit) = self.jit.as_mut() else { return };
        for done in jit.finished() {
            // A job whose code died meanwhile is gone.
            let Some(at) = self.jit_jobs.iter().position(|j| j.0 == done.id) else {
                if done.entry.is_some() {
                    jit.release(done.arena);
                }
                continue;
            };
            let (_, code, _) = self.jit_jobs.swap_remove(at);
            if std::env::var_os("TECHNE_JIT_LOG").is_some() {
                let what = if done.entry.is_some() { "compiled" } else { "not compiled" };
                eprintln!("jit: {} {what} ({} instructions, {:?})", done.name, done.ops, done.time);
            }
            let Some(f) = done.entry else { continue };
            // The code lives (its job does), and `EnterJit` behaves exactly
            // like the instruction it replaces, so this is safe at any point.
            let c = unsafe { &mut *(code as *mut Code) };
            c.jit.entry.set(Some(f));
            c.jit.arena.set(done.arena);
            if done.heads[0] == 0 {
                c.jit.call_entry.set(Some(f));
            }
            for h in done.heads {
                c.ops[h] = Op::EnterJit;
            }
        }
    }

    /// Every 256 calls or loop iterations of a function: deliver a pending
    /// interrupt, compile the function once it is hot, and install finished
    /// code.
    #[cold]
    unsafe fn safepoint(&mut self, code: *const Code, count: u32) -> Result<(), Error> {
        self.poll_interrupt()?;
        if count >= self.jit_threshold && !unsafe { (*code).jit.failed.get() } {
            unsafe { self.jit_compile(code) };
        }
        if self.jit.as_ref().is_some_and(|j| j.pending > 0) {
            self.jit_install();
        }
        Ok(())
    }

    /// A handle that stops this VM's executions from any thread.
    pub fn interrupt_handle(&self) -> InterruptHandle {
        InterruptHandle { stops: self.stops.clone(), thread: self.thread.clone(), notify: self.wake_notifier.clone() }
    }

    /// Drop a break pending for the running execution (e.g. one meant for
    /// what it just finished).
    pub fn clear_interrupt(&self) {
        self.stops.take_break(self.running_execution());
        self.stops.run(self.running_execution());
    }

    /// Raise "interrupted" if the running execution has a break pending, or
    /// end it if it has a kill pending (which stays).
    pub(crate) fn poll_interrupt(&mut self) -> Result<(), Error> {
        if self.interrupt.load(Ordering::Relaxed) {
            let stop = self.stops.take().or_else(|| {
                self.memory_looked();
                self.stops.take()
            });
            match stop {
                Some(Stop::Break) => return Err(Error::new(INTERRUPTED)),
                Some(Stop::OutOfMemory) => return Err(self.out_of_memory()),
                Some(Stop::Kill) => return Err(Error::new(KILLED).with_kind(ErrorKind::Killed)),
                None => {}
            }
        }
        Ok(())
    }

    /// The error a native that stopped for the attention flag raises.
    pub(crate) fn take_interrupt(&mut self) -> Error {
        self.poll_interrupt().err().unwrap_or_else(|| Error::new(INTERRUPTED))
    }

    /// The execution running: the innermost active one (0: none).
    pub fn running_execution(&self) -> ExecId {
        self.active.last().map_or(ExecId(0), |e| e.0)
    }

    /// Whether task `id` is active: running, or below what runs.
    pub(crate) fn task_active(&self, id: usize) -> bool {
        self.active.iter().any(|e| e.1 == Some(id))
    }

    /// Whether the running execution is being killed: its unwinding runs
    /// no Lisp code.
    pub(crate) fn killing(&self) -> bool {
        self.interrupt.load(Ordering::Relaxed) && self.stops.pending(self.running_execution()) == Some(Stop::Kill)
    }

    /// `enter_account` unless this thread charges the world already.
    fn charging(&self) -> Option<crate::alloc::Entered> {
        let account = self.account.reference();
        (!account.entered()).then(|| account.enter())
    }

    /// Charge this thread's allocations to the world until the guard
    /// returned is dropped: what a host does for the world (the VM enters
    /// it itself while it runs).
    pub fn enter_account(&self) -> crate::alloc::Entered {
        self.account.enter()
    }

    /// A new top-level execution, which stops can reach by its id from now
    /// on; run it between `enter_execution` and `leave_execution`.
    pub fn new_execution(&self) -> ExecId {
        self.stops.begin()
    }

    /// Forget the execution `id` (`new_execution`), which will not run.
    pub fn discard_execution(&self, id: ExecId) {
        self.stops.end(id);
    }

    /// Run what follows as the execution `id` (`new_execution`), until
    /// `leave_execution`.
    pub fn enter_execution(&mut self, id: ExecId) {
        if self.executions.is_empty() {
            self.stops.set_root(id);
            self.entered = Some(self.account.enter());
        }
        self.executions.push((id, self.locals.clone()));
        self.active.push((id, None));
        self.stops.run(id);
    }

    /// End the execution `id`, entered last. The dynamic state it changed
    /// is restored if it was killed, as its after thunks did not run.
    pub fn leave_execution(&mut self, id: ExecId) {
        // What it grew is its own, still (a host's execution keeps its
        // result: the refusal only counts, for the next one).
        self.look_at_growth();
        let Some((entered, locals)) = self.executions.pop() else { return };
        debug_assert_eq!(entered, id, "executions left in the order entered");
        debug_assert_eq!(self.active.last().map(|e| e.0), Some(id), "executions left in the order entered");
        self.active.pop();
        if self.stops.pending(id) == Some(Stop::Kill) {
            self.locals = locals;
        }
        self.stops.end(id);
        if self.executions.is_empty() {
            self.stops.set_root(ExecId(0));
            self.entered = None;
        }
        self.stops.run(self.running_execution());
    }

    /// Run `f` as an execution of its own if the host calls it, else as
    /// part of the running one.
    fn as_execution<T>(&mut self, f: impl FnOnce(&mut Vm) -> Result<T, Error>) -> Result<T, Error> {
        if !self.active.is_empty() {
            // Called back from another VM's native, perhaps.
            let _charged = self.charging();
            return f(self);
        }
        let id = self.new_execution();
        self.enter_execution(id);
        // A panic (a native's, caught by the host) leaves the execution and
        // the account too; the VM's state is otherwise undefined after one
        // (a task's stack may stay swapped in): drop it.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        // Ending well, refused for what it grew all the same.
        let result = match result {
            Ok(Ok(v)) => self.growth_stop(id).map_or(Ok(Ok(v)), |e| Ok(Err(e))),
            other => other,
        };
        self.leave_execution(id);
        result.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }

    /// Execute one instruction for JIT-compiled code (its slow paths). For a
    /// conditional branch the result says whether to jump.
    pub(crate) unsafe fn jit_slow_op(&mut self, r: *mut Value, op: Op) -> Result<bool, Error> {
        unsafe {
            let reg = |i: u16| r.add(i as usize);
            let int = |i: i16| Value::int_unchecked(i as i64);
            match op {
                Op::GetG { dst, g } => {
                    let v = self.globals[g as usize];
                    if v == Value::UNDEFINED {
                        return Err(Error::new(format!("unbound variable: {}", self.global_name(g))));
                    }
                    *reg(dst) = v;
                }
                Op::MkBox { r: x } => {
                    let p = self.alloc(2);
                    *p = header(Kind::Box, 1, 0);
                    set_field(p, 0, *reg(x));
                    *reg(x) = Value::ptr(p);
                }
                Op::Cons { dst, a, b } => {
                    let p = self.alloc(3);
                    *p = header(Kind::Pair, 2, 0);
                    set_field(p, 0, *reg(a));
                    set_field(p, 1, *reg(b));
                    *reg(dst) = Value::ptr(p);
                }
                Op::Add { dst, a, b } => *reg(dst) = num::add(self, *reg(a), *reg(b))?,
                Op::AddI { dst, a, i } => *reg(dst) = num::add(self, *reg(a), int(i))?,
                Op::Sub { dst, a, b } => *reg(dst) = num::sub(self, *reg(a), *reg(b))?,
                Op::Mul { dst, a, b } => *reg(dst) = num::mul(self, *reg(a), *reg(b))?,
                Op::Quo { dst, a, b } => *reg(dst) = num::quotient(self, *reg(a), *reg(b))?,
                Op::Rem { dst, a, b } => *reg(dst) = num::remainder(self, *reg(a), *reg(b))?,
                Op::Mod { dst, a, b } => *reg(dst) = num::modulo(self, *reg(a), *reg(b))?,
                Op::Lt { dst, a, b } => *reg(dst) = Value::bool(num::lt(*reg(a), *reg(b))?),
                Op::Le { dst, a, b } => *reg(dst) = Value::bool(num::le(*reg(a), *reg(b))?),
                Op::NumEq { dst, a, b } => *reg(dst) = Value::bool(num::num_eq(*reg(a), *reg(b))?),
                Op::JNLt { a, b, .. } => return Ok(!num::lt(*reg(a), *reg(b))?),
                Op::JNLe { a, b, .. } => return Ok(!num::le(*reg(a), *reg(b))?),
                Op::JNNumEq { a, b, .. } => return Ok(!num::num_eq(*reg(a), *reg(b))?),
                Op::JNLtI { a, i, .. } => return Ok(!num::lt(*reg(a), int(i))?),
                Op::JNGtI { a, i, .. } => return Ok(!num::lt(int(i), *reg(a))?),
                Op::JNEqI { a, i, .. } => return Ok(!num::num_eq(*reg(a), int(i))?),
                Op::Car { a, .. } | Op::Cdr { a, .. } => {
                    let name = if matches!(op, Op::Car { .. }) { "car" } else { "cdr" };
                    return Err(crate::builtins::type_error(name, "pair", *reg(a)));
                }
                Op::VRef { v, i, .. } => return Err(crate::builtins::index_error("vector-ref", *reg(v), *reg(i))),
                Op::VSet { v, i, .. } => return Err(crate::builtins::vector_set_error(*reg(v), *reg(i))),
                Op::PopHandler => {
                    self.handlers.pop();
                }
                Op::Closure { dst, code: c } => {
                    let target: *const Code = self.code(c);
                    let n = (*target).captures.len();
                    let p = self.alloc(2 + n);
                    *p = header(Kind::Closure, 1 + n, 0);
                    set_field(p, 0, Value::untraced_ptr(target));
                    let current = *r.sub(1);
                    for (i, src) in (*target).captures.iter().enumerate() {
                        let v = match *src {
                            CapSrc::Reg(x) => *reg(x),
                            CapSrc::Cap(j) => field(current.as_ptr(), 1 + j as usize),
                        };
                        set_field(p, 1 + i, v);
                    }
                    *reg(dst) = Value::ptr(p);
                }
                _ => unreachable!("no JIT slow path for {op:?}"),
            }
            Ok(false)
        }
    }

    /// Arity check, rest-list construction and register initialisation for a
    /// closure call whose arguments are at `bp..bp + n`.
    pub(crate) unsafe fn enter(&mut self, callee: *const Code, bp: usize, n: usize) -> Result<(), Error> {
        unsafe {
            let c = &*callee;
            let fixed = c.nparams as usize;
            let size = c.frame_size as usize;
            self.ensure_regs(bp + size.max(n) + 1)?;
            if c.rest {
                if n < fixed {
                    return Err(Error::new(format!("{}: expected at least {fixed} arguments, got {n}", c.name)));
                }
                // Initialise the frame before allocating: the GC scans it.
                if size > n {
                    self.regs[bp + n..bp + size].fill(Value::VOID);
                }
                self.stack_top = self.stack_top.max(bp + n.max(size));
                let rest = self.regs[bp + fixed..bp + n].to_vec();
                let list = self.make_list(&rest);
                self.regs[bp + fixed] = list;
                if n > fixed + 1 {
                    self.regs[bp + fixed + 1..bp + n].fill(Value::VOID);
                }
            } else {
                if n != fixed {
                    return Err(Error::new(format!("{}: expected {fixed} arguments, got {n}", c.name)));
                }
                self.regs[bp + n..bp + size.max(n)].fill(Value::VOID);
            }
            Ok(())
        }
    }

    pub fn call_native(&mut self, index: usize, args: usize, n: usize) -> Result<Value, Error> {
        let native = &self.natives[index];
        if n < native.min || native.max.is_some_and(|m| n > m) {
            return Err(Error::new(format!("{}: wrong number of arguments ({n})", native.name)));
        }
        let result = match &native.f {
            NativeImpl::Plain(f) => {
                let f = *f;
                f(self, args, n)
            }
            NativeImpl::Boxed(f) => {
                // Natives are never removed, so the closure outlives the call
                // even if `natives` grows meanwhile.
                let f: *const dyn Fn(&mut Vm, usize, usize) -> Result<Value, Error> = Rc::as_ptr(f);
                unsafe { (*f)(self, args, n) }
            }
        };
        result.map_err(|mut e| {
            let name = &self.natives[index].name;
            // Name the native in errors it raises itself, not in errors from
            // Scheme code it ran (`eval`, callbacks), which carry a trace.
            if e.escape.is_none()
                && e.trace.is_empty()
                && e.payload.is_none()
                && !e.is_interrupt()
                && !e.is_cancellation()
                && !e.reaches_host()
                && !name.starts_with('%')
                && !e.msg.starts_with(&**name)
            {
                e.msg = format!("{name}: {}", e.msg);
            }
            e
        })
    }

    /// Install a handler procedure below all code run from now on (e.g. a
    /// REPL debugger). It sees every condition no `guard` catches.
    pub fn push_handler(&mut self, handler: Value) {
        let handler = self.root(handler);
        self.handlers.push(Handler::Proc { handler });
    }

    /// Install a `with-exception-handler` procedure for the duration of `thunk`.
    pub fn with_handler(&mut self, handler: Value, thunk: Value) -> Result<Value, Error> {
        let handler = self.root(handler);
        self.handlers.push(Handler::Proc { handler });
        let depth = self.handlers.len();
        let result = self.call(thunk, &[]);
        self.unwind_to(depth - 1);
        result
    }

    /// `raise-continuable`: call the innermost handler procedure and return
    /// its value; guards unwind as for `raise`.
    pub fn raise_continuable(&mut self, v: Value) -> Result<Value, Error> {
        // The innermost visible handler: a procedure handles it here and its
        // value is returned; a guard (or none) unwinds as for `raise`.
        let mut idx = self.handlers.len();
        while idx > 0 {
            if !self.masked(idx - 1) {
                match self.handlers[idx - 1].clone() {
                    Handler::Proc { handler } => return self.call_masked(idx - 1, handler.get(), v),
                    Handler::Guard { .. } => break,
                    Handler::Escape { .. } | Handler::Wind { .. } => {}
                }
            }
            idx -= 1;
        }
        Err(self.raise_error(v))
    }

    /// The error used to `raise` the object `v`.
    pub fn raise_error(&mut self, v: Value) -> Error {
        let msg = crate::builtins::condition_message(self, v);
        let mut e = Error::new(msg);
        e.payload = Some(self.root(v));
        e
    }

    /// What `sym` denotes in `module`, as help shows it.
    pub fn describe_name(&self, module: u32, sym: u32) -> Description {
        let name = symbol_name(sym);
        let unbound =
            Description { name: name.clone(), kind: "unbound", params: None, arity: None, doc: None, file: None, line: 0, column: 0 };
        match self.lookup_global(module, sym) {
            Some(GlobalBinding::Macro(m)) => {
                let (line, column) = if m.pos == NO_POS { (0, 0) } else { reader::line_col(&m.file.text, m.pos) };
                Description { kind: "macro", doc: m.doc.clone(), file: Some(m.file.name.clone()), line, column, ..unbound }
            }
            None => match crate::compiler::special_form_doc(&name) {
                Some((syntax, doc)) => {
                    Description { kind: "special form", params: Some(vec![syntax.into()]), doc: Some(doc.into()), ..unbound }
                }
                None => unbound,
            },
            Some(GlobalBinding::Var(g)) => {
                let v = self.globals[g as usize];
                // A variable's docstring documents what it holds when that
                // has none: an alias, a parameter.
                let own = self.variable_docs.get(&g).cloned();
                match self.procedure_info(v) {
                    Some(info) => Description {
                        kind: if info.native { "built-in procedure" } else { "procedure" },
                        arity: self.native_arity(v),
                        params: (info.doc.is_some() || !info.native).then_some(info.params),
                        doc: own.or(info.doc),
                        file: info.file,
                        line: info.line,
                        column: info.column,
                        ..unbound
                    },
                    None if v == Value::UNDEFINED => unbound,
                    None => Description { kind: "variable", doc: own, ..unbound },
                }
            }
        }
    }

    /// Text for `(help name)`: signature, kind, location and docstring.
    pub fn describe_binding(&mut self, module: u32, sym: u32) -> String {
        let d = self.describe_name(module, sym);
        match d.kind {
            "unbound" => format!("{}: unbound", d.name),
            "variable" => {
                let v = self.get_global_in(module, &d.name).unwrap_or(Value::UNDEFINED);
                let doc = d.doc.map(|doc| format!("\n\n{doc}")).unwrap_or_default();
                format!("{} = {}{doc}", d.name, crate::builtins::repr(v))
            }
            _ => d.to_string(),
        }
    }

    pub fn describe_value(&self, name: &str, v: Value) -> String {
        match self.procedure_info(v) {
            Some(info) => {
                let kind = if info.native { "built-in procedure" } else { "procedure" };
                let d = Description {
                    name: name.into(),
                    kind,
                    arity: self.native_arity(v),
                    params: (info.doc.is_some() || !info.native).then_some(info.params),
                    doc: info.doc,
                    file: info.file,
                    line: info.line,
                    column: info.column,
                };
                match Vm::applicable_proc(v) {
                    Some(_) => {
                        let rtd = unsafe { field(field(v.as_ptr(), 0).as_ptr(), 0) };
                        format!("{name}: {} (applicable record)\n{d}", crate::builtins::repr(rtd))
                    }
                    None => d.to_string(),
                }
            }
            None => format!("{name} = {}", crate::builtins::repr(v)),
        }
    }

    /// In a handler procedure (`with-exception-handler`, `handler-bind`):
    /// the frames where the condition was raised, innermost first, as
    /// "name (file:line:col)". Empty elsewhere.
    pub fn raise_backtrace(&self) -> &[String] {
        &self.raise_trace
    }

    /// Signature, docstring and definition site of a procedure (or of an
    /// applicable record's procedure).
    pub fn procedure_info(&self, v: Value) -> Option<ProcedureInfo> {
        if v.is_native() {
            let n = &self.natives[v.as_native()];
            let doc = n.doc;
            return Some(ProcedureInfo {
                name: n.name.clone(),
                params: doc.map_or_else(Vec::new, |d| d.params.iter().map(|p| Rc::from(*p)).collect()),
                doc: doc.filter(|d| !d.doc.is_empty()).map(|d| d.doc.into()),
                file: doc.map(|d| d.file.into()),
                line: doc.map_or(0, |d| d.line as usize),
                column: doc.map_or(0, |_| 1),
                native: true,
            });
        }
        if !is_kind(v, Kind::Closure) {
            return Vm::applicable_proc(v).and_then(|p| self.procedure_info(p));
        }
        let code = unsafe { &*field(v.as_ptr(), 0).as_untraced_ptr::<Code>() };
        let file = &self.files[code.file as usize];
        let (line, column) = if code.pos == NO_POS { (0, 0) } else { reader::line_col(&file.text, code.pos) };
        Some(ProcedureInfo {
            name: code.name.clone(),
            params: code.params.clone(),
            doc: code.doc.clone(),
            file: Some(file.name.clone()),
            line,
            column,
            native: false,
        })
    }

    /// How many arguments a native takes, at least and at most.
    fn native_arity(&self, v: Value) -> Option<(usize, Option<usize>)> {
        v.is_native().then(|| {
            let n = &self.natives[v.as_native()];
            (n.min, n.max)
        })
    }

    /// The docstring of a procedure (or of an applicable record's procedure).
    pub fn documentation(&self, v: Value) -> Option<Rc<str>> {
        if v.is_native() {
            self.natives[v.as_native()].doc.filter(|d| !d.doc.is_empty()).map(|d| d.doc.into())
        } else if is_kind(v, Kind::Closure) {
            unsafe { &*field(v.as_ptr(), 0).as_untraced_ptr::<Code>() }.doc.clone()
        } else {
            Vm::applicable_proc(v).and_then(|p| self.documentation(p))
        }
    }

    /// The name a procedure value was defined with, if it has one.
    pub fn procedure_name(&self, v: Value) -> Option<Rc<str>> {
        if is_kind(v, Kind::Closure) {
            Some(unsafe { &*field(v.as_ptr(), 0).as_untraced_ptr::<Code>() }.name.clone())
        } else if v.is_native() {
            Some(self.natives[v.as_native()].name.clone())
        } else {
            None
        }
    }

    /// Names visible from `module` (for completion); not forward references
    /// that were never defined.
    /// Every name the root module defines, internal ones (`%...`) included.
    pub fn root_names(&self) -> Vec<Rc<str>> {
        self.bindings.keys().filter(|(m, _)| *m == ROOT_MODULE).map(|(_, s)| symbol_name(*s)).collect()
    }

    pub fn global_names(&self, module: u32) -> Vec<Rc<str>> {
        let mut names: Vec<Rc<str>> = self
            .bindings
            .iter()
            .filter(|((m, _), b)| {
                (*m == ROOT_MODULE || *m == module) && !matches!(b, GlobalBinding::Var(g) if self.globals[*g as usize] == Value::UNDEFINED)
            })
            .map(|((_, s), _)| symbol_name(*s))
            .chain(self.modules[module as usize].imports.keys().map(|s| symbol_name(*s)))
            .chain(crate::compiler::SPECIAL_FORMS.iter().map(|s| Rc::from(*s)))
            .filter(|n| !n.starts_with('%') && !n.contains('\u{1f}'))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Print the code objects compiled for the last top-level form.
    fn dump_from(&self, last: u32) {
        let first = (0..=last).rev().take_while(|&i| i == last || &*self.code(i).name != "toplevel").last().unwrap_or(last);
        for code in self.codes[first as usize..=last as usize].iter().flatten() {
            eprintln!("== {} (params {}, frame {})", code.name, code.nparams, code.frame_size);
            for (i, op) in code.ops.iter().enumerate() {
                eprintln!("  {i:3} {op:?}");
            }
        }
    }

    pub fn flush(&mut self) {
        let _ = self.out.flush();
    }
}

/// Declare the top-level definitions of a form so references anywhere in the
/// file resolve to them rather than to imports or builtins.
pub(crate) fn predeclare(vm: &mut Vm, module: u32, form: &Sexp) {
    let Some(items) = form.list() else { return };
    match items.first() {
        Some(h) if h.is_sym("define") => match items.get(1) {
            Some(Sexp::Sym(name)) => {
                vm.define_var(module, *name);
            }
            Some(Sexp::List(sig, _, _)) => {
                if let Some(Sexp::Sym(name)) = sig.first() {
                    vm.define_var(module, *name);
                }
            }
            _ => {}
        },
        Some(h) if h.is_sym("begin") => items[1..].iter().for_each(|f| predeclare(vm, module, f)),
        _ => {}
    }
}

pub unsafe fn init_string(p: *mut u64, bytes: &[u8]) {
    unsafe {
        let ascii = if bytes.is_ascii() { heap::ASCII } else { 0 };
        init_raw(p, Kind::String, bytes, ascii);
    }
}

pub unsafe fn init_bytes(p: *mut u64, bytes: &[u8]) {
    unsafe { init_raw(p, Kind::Bytevector, bytes, 0) }
}

unsafe fn init_raw(p: *mut u64, kind: Kind, bytes: &[u8], flags: u64) {
    unsafe {
        *p = header(kind, bytes.len(), flags);
        let words = bytes.len().div_ceil(8);
        if words > 0 {
            *p.add(words) = 0;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.add(1) as *mut u8, bytes.len());
    }
}
