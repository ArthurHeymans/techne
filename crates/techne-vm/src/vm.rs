//! The interpreter: modules, globals, code objects, the register stack,
//! exception handlers and dispatch.
//!
//! GC roots are the live register window `regs[..stack_top]`, globals, code
//! constants, `scratch`, `specials` and Rust `Root`s. Any operation that
//! allocates, calls included, must update `stack_top` first to cover its
//! frame, and re-read register values after allocating, because a
//! collection moves objects. Every frame below the running one is in the
//! window: a register of a live frame left out of a collection would hold
//! a stale pointer when the window grows over it again. A call the
//! interpreter makes keeps the window as it was then (`Frame::top`), and
//! returning to it restores that: what the callee's frames used above is
//! dead, and every frame is initialised before the window grows over it,
//! so a collection after a deep recursion does not scan the frames it
//! left. Compiled code calling compiled code does not restore it, which
//! would cost every call: the window then keeps what a run of compiled
//! code used, at most `jit::MAX_DEPTH` frames, until it returns to the
//! interpreter. Register pointers must also
//! be rederived: root visitation can invalidate their borrow permissions even
//! when the register buffer itself stays at the same address.
//!
//! The VM is re-entrant: Rust (including natives) can call Scheme procedures
//! with `Vm::call`, which runs a nested dispatch above the current stack top.

use std::{
    any::Any,
    cell::RefCell,
    cmp::Reverse,
    collections::{BinaryHeap, VecDeque},
    fmt,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::Thread,
    time::{Duration, Instant},
};

pub(crate) mod held;
mod host;
pub(crate) mod meter;
mod no_gc;
pub use no_gc::{HeapAccess, HeapValue};
mod slots;
mod template;
pub use slots::Moves;
pub(crate) use slots::Slots;
pub use template::Snapshot;
mod retained;
pub(crate) mod scopes;
#[cfg(test)]
mod tests;

pub use held::Held;

use rustc_hash::FxHashMap;

use crate::{
    api::Root,
    code::{CapSrc, Code, Op},
    compiler::Compiler,
    expand::Macro,
    heap::{self, CODE_SLOTS, Heap, Kind, LARGE_WORDS, Roots, code_slot, field, header, is_kind, len_of, set_field},
    num,
    reader::{self, NO_POS, Sexp, symbol_name},
    value::Value,
};

/// Register stack limit (128 MiB); deeper recursion raises an error.
const MAX_REGS: usize = 1 << 24;
/// Registers a world starts with (32 KiB); the stack doubles as it deepens.
const INITIAL_REGS: usize = 1 << 12;
const CANNOT_SUSPEND: &str = "cannot suspend here: a Rust native procedure is calling back into Scheme (vm.call)";
/// Module 0 holds builtins and the prelude; it is visible from every module.
pub const ROOT_MODULE: u32 = 0;
/// The module of a freed global.
const NO_MODULE: u32 = u32::MAX;
/// Module for code evaluated without a file (REPL, `eval_source`).
pub const USER_MODULE: u32 = 1;

pub use crate::ports::Output;

/// Authority a world can hold. Natives that need one are registered inside
/// `Vm::requiring`; in a world not granted it they only raise an error, so
/// code there cannot reach it by any name, import or value. Loading, the
/// clock and entropy are checked when used instead (`Vm::need`), so that
/// a host can change them on a world it made (`Vm::set_grant`): it loads
/// a deployment, deterministically, then takes loading back and grants
/// the clock and entropy to the instances started from its snapshot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Capability {
    /// Reading and writing files.
    Files,
    /// Environment variables and the command line.
    Environment,
    /// Starting and controlling child processes.
    Processes,
    /// Networking: sockets (`techne-net`), connecting to other machines
    /// (nodes).
    Network,
    /// Loading modules from files (`require`, naming a file's module).
    Loading,
    /// Ending the host process (`exit`).
    HostControl,
    /// The process's standard input: without it, a world's input port
    /// is empty.
    Console,
    /// Reading the time: `current-second`, `current-jiffy`.
    Clock,
    /// Random bytes from the operating system.
    Entropy,
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
            Capability::Console => "console",
            Capability::Clock => "clock",
            Capability::Entropy => "entropy",
        }
    }

    /// Whether it is checked when used, not when natives are registered,
    /// so that it can be granted to a running world or taken back.
    pub fn checked_when_called(self) -> bool {
        matches!(self, Capability::Loading | Capability::Clock | Capability::Entropy)
    }
}

/// The capabilities a world is granted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Grants(u16);

impl Grants {
    pub const NONE: Grants = Grants(0);
    pub const ALL: Grants = Grants(0x1ff);

    pub const fn with(self, c: Capability) -> Grants {
        Grants(self.0 | 1 << c as u16)
    }

    pub const fn without(self, c: Capability) -> Grants {
        Grants(self.0 & !(1 << c as u16))
    }

    pub const fn has(self, c: Capability) -> bool {
        self.0 & 1 << c as u16 != 0
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
    pub(crate) payload: Option<Root>,
    /// Escape to the `call/cc` with this id, carrying the value.
    pub(crate) escape: Option<(i64, Root)>,
    /// Handlers at or above this index of this world's were already
    /// consulted (set when the error leaves a dispatch level, so outer
    /// levels continue below it).
    searched: Option<(crate::api::WorldId, u32)>,
    /// Set by natives that must wait (see `tasks`): suspends the running task.
    pub(crate) wait: Option<crate::tasks::Wait>,
    /// A native hands its call to the VM rather than reentering Scheme.
    tail_call: Option<NativeTailCall>,
    /// What kind of failure, for `file-error?` and `read-error?`.
    pub kind: ErrorKind,
}

enum NativeCall {
    Closure(Value, usize),
    Returned(Value),
}

struct NativeTailCall {
    procedure: Root,
    args: Vec<Root>,
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
    /// A program broke the language's rules: an argument of the wrong
    /// type, a bad index, a wrong number of arguments, an unbound variable,
    /// a call of what is not a procedure, or a recursion past the stack. Unlike what `error` raises,
    /// this is a mistake in the program, not a failure it reports
    /// (`violation?`).
    Violation,
    /// `exit` asked the host to end the program with this status. Handlers
    /// do not see it; it unwinds to the host.
    Exit(i32),
    /// The execution was killed (`crate::stop`). Handlers do not see it, and
    /// `dynamic-wind` after thunks do not run.
    Killed,
    /// A native panicked and the world was retired (`Vm::poisoned`).
    /// Handlers do not see it, and `dynamic-wind` after thunks do not run:
    /// the VM's state after a panic is not safe to run guest code on.
    Poisoned,
}

impl Error {
    /// The error as a pointer (for `jit::NativeRet`), never null.
    pub(crate) fn into_raw(self) -> *mut ErrorData {
        Box::into_raw(self.0)
    }

    /// # Safety
    /// `p` comes from `into_raw`, and is taken back once.
    pub(crate) unsafe fn from_raw(p: *mut ErrorData) -> Error {
        Error(unsafe { Box::from_raw(p) })
    }

    pub fn new(msg: impl Into<String>) -> Error {
        Error(Box::new(ErrorData {
            msg: msg.into(),
            trace: Vec::new(),
            payload: None,
            escape: None,
            searched: None,
            wait: None,
            tail_call: None,
            kind: ErrorKind::General,
        }))
    }

    /// A violation (`ErrorKind::Violation`): a mistake in the program.
    pub fn violation(msg: impl Into<String>) -> Error {
        Error::new(msg).with_kind(ErrorKind::Violation)
    }

    /// Its parts, to move them out.
    pub fn into_inner(self) -> ErrorData {
        *self.0
    }

    /// The raised Scheme object, if any (error objects, `raise`d values).
    pub fn payload(&self) -> Option<crate::api::Value> {
        self.payload.clone().map(crate::api::Value::from_root)
    }

    /// The error a host native returned, as world `world` takes it: what
    /// it carries of another world (an error it got from another VM and
    /// handed on) is dropped, the message kept.
    pub(crate) fn adopt(mut self, world: crate::api::WorldId) -> Error {
        let foreign = |r: &Root| r.world() != world;
        if self.payload.as_ref().is_some_and(foreign) {
            self.0.payload = None;
        }
        if self.escape.as_ref().is_some_and(|(_, r)| foreign(r)) {
            self.0.escape = None;
        }
        if self.tail_call.as_ref().is_some_and(|call| foreign(&call.procedure) || call.args.iter().any(foreign)) {
            self.0.tail_call = None;
            self.0.msg = "a native tail call of another world".into();
        }
        if self.wait.as_ref().is_some_and(|w| !w.of_world(world)) {
            self.0.wait = None;
            self.0.msg = format!("a wait of another world: {}", self.msg);
        }
        self
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

    /// A native panicked and the world was retired (`Vm::poisoned`).
    pub fn is_poison(&self) -> bool {
        self.kind == ErrorKind::Poisoned
    }

    /// Whether it unwinds to the host, past every handler.
    fn reaches_host(&self) -> bool {
        matches!(self.kind, ErrorKind::Exit(_) | ErrorKind::Killed | ErrorKind::Poisoned)
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
pub(crate) struct Stack {
    pub(crate) regs: Slots,
    frames: Vec<Frame>,
    handlers: Vec<Handler>,
    pub(crate) stack_top: usize,
    /// Task-local dynamic state (parameters, output port, restarts).
    pub locals: FxHashMap<i64, Root>,
    /// The current module of the task (`Vm::current_module`): `in-module`
    /// in one task does not change another's.
    pub(crate) module: u32,
    /// The modules `enter_module` saved, to restore: GC roots.
    pub(crate) saved_modules: Vec<u32>,
}

impl Stack {
    pub(crate) fn new_in(budget: crate::alloc::Budget) -> Stack {
        Stack {
            regs: Slots::new_in(budget),
            frames: Vec::new(),
            handlers: Vec::new(),
            stack_top: 0,
            locals: FxHashMap::default(),
            module: USER_MODULE,
            saved_modules: Vec::new(),
        }
    }

    pub(crate) fn with_regs_in(n: usize, budget: crate::alloc::Budget) -> Result<Stack, Error> {
        let mut stack = Stack::new_in(budget);
        stack
            .regs
            .try_resize(n, Value::VOID, &mut Moves::default())
            .map_err(|_| Error::new("out of memory: cannot allocate task registers"))?;
        Ok(stack)
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

pub(crate) type NativeFn = fn(&mut Vm, usize, usize) -> Result<Value, Error>;
/// A native as compiled code calls it: a `NativeFn` returning its result
/// in two registers (`jit::NativeRet`), which `natives!` defines for each
/// (a panic in it aborts, as in any Rust function compiled code calls).
pub(crate) type PlainFn = extern "C" fn(&mut Vm, usize, usize) -> crate::jit::NativeRet;
pub(crate) type BoxedNative = Rc<dyn Fn(&mut Vm, usize, usize) -> Result<Value, Error>>;

#[derive(Clone)]
pub(crate) enum NativeImpl {
    /// The native, and the same as compiled code calls it.
    Plain(NativeFn, PlainFn),
    Boxed(BoxedNative),
}

#[derive(Clone)]
pub(crate) struct Native {
    pub name: Rc<str>,
    pub f: NativeImpl,
    pub min: usize,
    pub max: Option<usize>,
    /// Its parameters and documentation, from its definition in Rust (the
    /// docstring is empty for an internal native, `%name`).
    pub doc: Option<&'static NativeDoc>,
}

impl Native {
    /// Whether it takes `n` arguments.
    pub fn takes(&self, n: usize) -> bool {
        n >= self.min && self.max.is_none_or(|m| n <= m)
    }
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
pub(crate) enum GlobalBinding {
    Var(u32),
    Macro(Rc<Macro>),
}

#[derive(Clone)]
pub(crate) struct Module {
    pub name: Rc<str>,
    pub path: Option<PathBuf>,
    pub(crate) imports: FxHashMap<u32, GlobalBinding>,
    /// What `provide` or a library's `export` makes visible: each binding's
    /// name inside the module and the name importers see.
    pub(crate) exports: Option<Vec<(u32, u32)>>,
    pub(crate) defined: Vec<u32>,
    loading: bool,
    /// The execution loading it from Scheme (`next_required`), which may
    /// end killed, past the cleanup that would end the loading: then it is
    /// over. None while it loads from Rust.
    loader: Option<ExecId>,
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
    /// Its main module, once made (`begin_package`).
    main: Option<u32>,
    /// The execution loading it from Scheme (`begin_package`): if it
    /// ended killed, the staging is abandoned when another begins.
    owner: Option<ExecId>,
}

/// What loading the module of a file asks (`Vm::begin_module_load`).
pub(crate) enum ModuleLoad {
    /// It is loaded.
    Loaded(u32),
    /// It is made, marked loading, and these are its file's text: to
    /// evaluate in it, then `end_module_load`.
    Load(u32, String),
}

/// Datum labels while a literal is materialised: each label's value, and
/// the placeholders that stood for a datum inside itself.
#[derive(Default)]
struct Labels<'m> {
    values: FxHashMap<u32, Value>,
    placeholders: Vec<(Value, Value)>,
    /// Header flags of the pairs, vectors, strings and bytevectors built:
    /// `IMMUTABLE` for a literal in code.
    flags: u64,
    /// Meters a literal of code, whose strings may be one many times.
    meter: Option<&'m meter::Meter>,
}

impl Labels<'_> {
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
pub(crate) struct SourceFile {
    pub name: Rc<str>,
    pub text: Rc<str>,
    /// Where each line after the first starts, once a position was asked
    /// for (`line_col`): help asks for thousands.
    lines: Rc<std::cell::OnceCell<Box<[u32]>>>,
}

impl SourceFile {
    pub(crate) fn new(name: &str, text: &str) -> SourceFile {
        SourceFile { name: name.into(), text: text.into(), lines: Rc::default() }
    }

    /// The line and column, from 1, of byte position `pos`, as
    /// `reader::line_col` gives them, found in an index of the lines.
    pub(crate) fn line_col(&self, pos: u32) -> (usize, usize) {
        let starts =
            self.lines.get_or_init(|| self.text.bytes().enumerate().filter(|&(_, b)| b == b'\n').map(|(i, _)| i as u32 + 1).collect());
        let pos = self.text.floor_char_boundary(pos as usize);
        let line = starts.partition_point(|&s| s as usize <= pos);
        let start = if line == 0 { 0 } else { starts[line - 1] as usize };
        (line + 1, pos - start + 1)
    }
}

#[derive(Clone, Copy)]
struct Frame {
    code: *const Code,
    pc: u32,
    bp: u32,
    /// `stack_top` when the frame made its call, covering it and every
    /// frame below: returning to it restores that window.
    top: u32,
}

impl Frame {
    /// The window `stack_top` returns to: this frame's, unless what runs
    /// is within it already.
    fn window(&self, stack_top: usize) -> usize {
        stack_top.min(self.top as usize)
    }
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
    /// A handler procedure is running: entries from `lo` up to this one
    /// are hidden from what it raises (escape points and winds there stay
    /// live). Pushed when it is called, popped when it returns, and with
    /// the rest when unwinding past it.
    Mask { lo: usize, raised: Option<Rc<Raised>> },
}

/// Source text being evaluated a form at a time (`Vm::read_forms`): the
/// forms not compiled yet. A foreign object of the world reading them.
pub(crate) struct Forms {
    module: u32,
    file: u32,
    forms: RefCell<VecDeque<Sexp>>,
    /// Let go of once the forms are done (`Vm::finish_forms`), or else
    /// when the collector finds them unreachable.
    hold: RefCell<Option<Hold>>,
}

/// Keeps a source file registered and a module alive (`Vm::held_files`,
/// `Vm::held_modules`) while held; dropped, with its `Forms` when the
/// collector finds them unreachable, it lets them go
/// (`Vm::release_unheld_files`).
struct Hold {
    file: u32,
    module: u32,
    unheld: Rc<RefCell<Vec<(u32, u32)>>>,
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.unheld.borrow_mut().push((self.file, self.module));
    }
}

/// The raise a handler procedure was called for.
struct Raised {
    /// The condition.
    payload: Root,
    /// Its error's trace when the handler was called: what the error keeps
    /// if the handler raises the same condition again.
    origin: Vec<String>,
    /// Where it was raised, innermost first (`Vm::raise_backtrace`).
    trace: Vec<String>,
    /// How many frames the raising stack had: those of the handler are
    /// above them.
    frames_len: usize,
}

/// What `catch` found for an error.
enum Caught {
    /// A `guard` or escape point to unwind to, and the value it receives.
    Land(Landing, Value),
    /// A handler procedure to call at the raise point: the raising frame
    /// calls `%handle-raise` (`Vm::call_handler`).
    Handle(HandlerCall),
}

struct HandlerCall {
    handler: Root,
    /// The handler's index in the handler stack.
    index: usize,
    raised: Rc<Raised>,
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
    ScopeRtd = 5,
    /// The prelude's `%handle-raise`, which calls a handler procedure at
    /// the raise point (`Vm::call_handler`); void until the prelude runs.
    HandleRaise = 6,
}
const SPECIALS: usize = 7;

/// Whether compiled code runs small functions inline (`TECHNE_JIT_INLINE=0`
/// turns it off).
fn jit_inlines() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TECHNE_JIT_INLINE").is_none_or(|v| v != "0"))
}

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
    pub(crate) heap: Heap,
    /// What the world's memory is charged to (`crate::alloc`): entered
    /// while the VM runs, collects or allocates in the old generation.
    account: crate::alloc::Account,
    /// While the host runs an execution: the account entered.
    entered: Option<crate::alloc::Entered>,
    /// The world's cage and symbols entered alongside `entered`: values decode in it.
    entered_cage: Option<crate::alloc::CageEntered>,
    /// The most bytes the world is to hold (`held`): growth is admitted
    /// only if it fits (`admit`).
    memory_limit: usize,
    /// Over the limit (`check_memory`).
    pressure: Option<held::Pressure>,
    /// The register stack and the globals: their buffers move only where
    /// `moves` counts it (`Slots`). Outside the crate, `regs()`,
    /// `globals()` and their `_mut` versions lend them as slices.
    pub(crate) regs: Slots,
    frames: Vec<Frame>,
    handlers: Vec<Handler>,
    pub(crate) globals: Slots,
    /// How often `regs` and `globals` moved (`Slots`).
    pub(crate) moves: Moves,
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
    pub(crate) record_types: FxHashMap<u32, usize>,
    pub(crate) modules: Vec<Module>,
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
    /// The execution that staged it from Scheme (`end_package`).
    staged_by: Option<ExecId>,
    /// The module of the evaluation in progress (`eval_in`), where `eval`
    /// without a module and `help` resolve names; `in-module` changes it.
    current_module: u32,
    /// What this world may reach outside the VM.
    grants: Grants,
    /// The directory the files this world reads to load code are in, if
    /// it is confined to one (`confine_reading`).
    reading_root: Option<PathBuf>,
    /// The clock's resolution in microseconds: what the clock natives
    /// return is a multiple of it.
    clock_resolution: u64,
    /// The capability natives being defined need (`requiring`).
    requiring: Option<Capability>,
    /// Source text by file index (`Code::file`). A file lives while codes
    /// from it do (`file_codes` counts them) or while it is being evaluated
    /// (`pinned_files`); then its slot is emptied and reused.
    pub(crate) files: Vec<SourceFile>,
    file_codes: Vec<u32>,
    free_files: Vec<u32>,
    pinned_files: Vec<u32>,
    /// Files source text is being evaluated from a form at a time
    /// (`Forms`), how many times each: kept while that goes on.
    held_files: FxHashMap<u32, u32>,
    /// The modules it is evaluated in, likewise: GC roots.
    held_modules: FxHashMap<u32, u32>,
    /// The running stack's modules `enter_module` saved (swapped with
    /// tasks): GC roots.
    saved_modules: Vec<u32>,
    /// Files and modules `Forms` let go of since: `release_unheld_files`
    /// frees them.
    unheld_files: Rc<RefCell<Vec<(u32, u32)>>>,
    /// The docstrings of variables, by global: `(define name value "doc")`.
    pub variable_docs: FxHashMap<u32, Rc<str>>,
    /// Boxed so a `Code` keeps its address when the vector grows: frames
    /// and JIT code point into it. `None`: freed, its index in `free_codes`.
    /// A closure's first field holds its code's index here (its code id),
    /// not the address: heap objects carry no Rust pointers, so a future
    /// heap image or cage needs no pointer fixing for closures, and the
    /// collector never has to trust an address read out of the heap.
    codes: Vec<Option<Box<Code>>>,
    /// The address of each code by id, in step with `codes` (null where
    /// freed, and past `codes.len()`): id resolution for the interpreter
    /// and compiled code (`JitCtx::code_table`). `CODE_SLOTS` long, made
    /// once and never resized, so its address never changes, and a
    /// closure's id, masked (`heap::code_slot`), names a slot whatever the
    /// closure holds. Mapped untouched, it holds only the pages its codes
    /// use, outside the world's account. Taken mutably when the code is
    /// installed, a code's address is every access to it after that
    /// (`code`, `code_mut`): one through the box, which owns it uniquely,
    /// would end the permissions of the pointers frames and JIT code hold.
    code_ptrs: Vec<*const Code, crate::alloc::Pages>,
    /// Live codes past which the next allocation runs a full collection,
    /// which alone frees codes (`add_code`): their memory is outside the
    /// heap, and a world evaluating forms that allocate little would hold
    /// a code for each until the table is full. Twice those a completed
    /// cycle left, as the old generation's threshold.
    code_trigger: usize,
    /// `code_trigger` was passed: `collect` completes a cycle.
    codes_due: bool,
    free_codes: Vec<u32>,
    /// Codes of compilations and the top-level forms they run, kept until
    /// the form has run (`eval_form`): nothing else refers to them yet.
    compiling: Vec<u32>,
    /// Codes whose handles died in the last collection, to release: their
    /// ids, and the codes.
    dead_codes: Vec<(u32, Box<Code>)>,
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
    /// Set when the world is retired: a panic was contained (a native's,
    /// the VM's own, a host call's), or its heap was found corrupt. Every
    /// later execution, and every host call, is refused with this reason
    /// (`Vm::poisoned`); the VM may only be dropped afterwards. Settable
    /// through `&self`, as inspections retire their world too.
    poisoned: std::cell::OnceCell<Rc<str>>,
    /// Frames of native calls, innermost first, recorded when native code
    /// hands over to the interpreter.
    jit_unwind: Vec<Frame>,
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
    /// Indexed by native values. An entry's function and arity never change
    /// once defined (only its documentation is added): redefining makes a
    /// new native. The JIT relies on it (`KnownNative`).
    pub(crate) natives: Vec<Native>,
    pub(crate) apply_native: Value,
    /// Natives whose common case the JIT runs inline.
    jit_inline: Vec<(Value, crate::jit::Inline)>,
    /// Natives that return only fixnums (`jit::FIXNUM_NATIVES`).
    jit_fixnum: Vec<Value>,
    /// Exclusive end of the live register window (GC root extent).
    pub(crate) stack_top: usize,
    /// Extra roots for natives that allocate repeatedly.
    pub(crate) scratch: Vec<Value>,
    pub(crate) specials: [Value; SPECIALS],
    /// Host roots; shared, so a conversion looking at a VM borrowed
    /// (`Arg::value`) roots what it keeps at once.
    pub(crate) roots: std::cell::RefCell<Vec<Weak<crate::api::RootCell>>>,
    /// This world's identity: what the values Rust holds of it carry.
    world: crate::api::WorldId,
    foreign: Vec<Option<(Rc<dyn Any>, &'static str)>>,
    foreign_free: Vec<usize>,
    /// Scheme names for foreign Rust types (by `type_name`), for dispatch.
    pub(crate) foreign_type_names: FxHashMap<&'static str, u32>,
    next_id: i64,
    pub(crate) tasks: Vec<crate::tasks::Task>,
    /// Slots of `tasks` free for new tasks: those of finished ones.
    pub(crate) free_tasks: Vec<usize>,
    /// How many tasks are not finished.
    pub(crate) live: usize,
    /// The lifetimes tasks belong to (`crate::tasks::Lifetime`), by slot.
    pub(crate) lifetimes: Vec<crate::tasks::Lifetime>,
    pub(crate) free_lifetimes: Vec<u32>,
    /// The tasks running, innermost last, and since when (thread CPU time)
    /// their slice counts: an inner one's time is not the outer's.
    pub(crate) cpu_marks: Vec<(usize, Duration)>,
    /// The tasks ready to run (or to look at what they wait for), in the
    /// order they became ready.
    pub(crate) run_queue: VecDeque<crate::tasks::TaskRef>,
    /// The tasks futures woke, from any thread; `woken` takes them.
    pub(crate) wakes: Arc<crate::tasks::Wakes>,
    pub(crate) woken: Vec<crate::tasks::TaskRef>,
    /// Deadlines of waiting tasks, earliest first; stale ones too.
    pub(crate) timers: BinaryHeap<Reverse<(Instant, crate::tasks::Waiter)>>,
    /// Its slot of the watchdog (`crate::watchdog`), once a slice with a
    /// CPU bound ran.
    pub(crate) watchdog: Option<Arc<crate::watchdog::Slot>>,
    /// The box of the last error a task suspended with, to reuse.
    pub(crate) spare_suspend: Option<Error>,
    pub(crate) channels: Vec<crate::tasks::Channel>,
    /// The tasks spawned `#:child` of each unfinished execution, by its
    /// id: slot and generation.
    pub(crate) children: FxHashMap<u64, Vec<(usize, u32)>>,
    /// Executions ended killed whose children are being killed.
    pub(crate) killed: VecDeque<ExecId>,
    /// Scopes, by slot (`scopes`); free slots are `None`, listed in
    /// `free_scopes`.
    pub(crate) scopes: Vec<Option<scopes::Scope>>,
    pub(crate) free_scopes: Vec<usize>,
    /// Slots of `channels` free for new channels: those of closed and
    /// drained ones.
    pub(crate) free_channels: Vec<usize>,
    /// Blocked senders' offers, by group (one per waiting send or select).
    pub(crate) offers: crate::tasks::Offers,
    pub(crate) current_task: Option<usize>,
    /// The fuel of the compiled code running now (`JitCtx::fuel`), if any:
    /// a stop the VM makes for the running execution ends its slice
    /// (`Vm::hurry`), as compiled code polls only when its fuel runs out.
    pub(crate) jit_fuel: *mut u32,
    /// Dynamic state of the running stack (swapped with tasks).
    pub(crate) locals: FxHashMap<i64, Root>,
    /// Where standard output and error go (`Vm::capture_output`).
    pub(crate) out: crate::ports::Standard,
    /// The world's symbols, entered with it (`alloc::cage::enter`): a
    /// copy of its template's, or its own from the start. Declared last
    /// so every field that might print a symbol while dropping (roots,
    /// foreign values, codes) drops before it.
    pub(crate) symbols: Rc<std::cell::RefCell<crate::reader::Symbols>>,
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
    /// The codes' addresses, through which they are read (`Vm::code_ptrs`).
    code_ptrs: &'a [*const Code],
    free_codes: &'a mut Vec<u32>,
    dead_codes: &'a mut Vec<(u32, Box<Code>)>,
    /// Codes running or about to (frames, handlers, suspended tasks,
    /// compilations), whose handles are roots.
    running: Vec<*const Code>,
    jit_callers: &'a [*const Code],
    bindings: &'a FxHashMap<(u32, u32), GlobalBinding>,
    young_consts: &'a [*const Code],
    scratch: &'a mut [Value],
    specials: &'a mut [Value],
    roots: &'a mut Vec<Weak<crate::api::RootCell>>,
    tasks: &'a mut [crate::tasks::Task],
    scopes: &'a mut [Option<scopes::Scope>],
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
            Handler::Proc { .. } | Handler::Wind { .. } | Handler::Mask { .. } => None,
        }
    }
}

/// A root slot, named for reports (`Roots::visit_named`).
#[derive(Clone, Copy)]
enum RootSlot {
    TaskHandle(usize),
    TaskReg(usize, usize),
    Scope(usize),
    Reg(usize),
    Global(usize),
    Scratch(usize),
    Special(usize),
    CodeHandle,
    HostRoot,
}

impl std::fmt::Display for RootSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            RootSlot::TaskHandle(t) => write!(f, "the object of task {t}"),
            RootSlot::TaskReg(t, i) => write!(f, "register {i} of task {t}'s stack"),
            RootSlot::Scope(s) => write!(f, "a value of scope {s}"),
            RootSlot::Reg(i) => write!(f, "register {i} of the running stack"),
            RootSlot::Global(g) => write!(f, "global {g}"),
            RootSlot::Scratch(i) => write!(f, "scratch slot {i}"),
            RootSlot::Special(i) => write!(f, "special object {i}"),
            RootSlot::CodeHandle => write!(f, "the handle of a running code"),
            RootSlot::HostRoot => write!(f, "a host root"),
        }
    }
}

impl VmRoots<'_> {
    /// Every root slot, with its name (compiled away by `visit`).
    #[inline(always)]
    fn each(&mut self, mut f: impl FnMut(RootSlot, &mut Value)) {
        // Tasks' objects, and the stacks of tasks that are not running (or
        // the main stack, parked in the running task's slot).
        for (i, t) in self.tasks.iter_mut().enumerate() {
            f(RootSlot::TaskHandle(i), &mut t.handle);
            let top = t.stack.stack_top.min(t.stack.regs.len());
            t.stack.regs[..top].iter_mut().enumerate().for_each(|(r, v)| f(RootSlot::TaskReg(i, r), v));
        }
        for (i, s) in self.scopes.iter_mut().enumerate() {
            if let Some(s) = s {
                s.visit(&mut |v| f(RootSlot::Scope(i), v));
            }
        }
        self.regs.iter_mut().enumerate().for_each(|(i, v)| f(RootSlot::Reg(i), v));
        (self.globals.iter_mut().zip(self.global_rooted).enumerate())
            .filter(|(_, (_, r))| **r)
            .for_each(|(i, (v, _))| f(RootSlot::Global(i), v));
        self.scratch.iter_mut().enumerate().for_each(|(i, v)| f(RootSlot::Scratch(i), v));
        self.specials.iter_mut().enumerate().for_each(|(i, v)| f(RootSlot::Special(i), v));
        // Handles are old objects, which do not move.
        for &code in &self.running {
            f(RootSlot::CodeHandle, &mut unsafe { (*code).handle.get() });
        }
        self.roots.retain(|w| match w.upgrade() {
            Some(cell) => {
                let mut v = cell.get();
                f(RootSlot::HostRoot, &mut v);
                cell.set(v);
                true
            }
            None => false,
        });
    }
}

impl Roots for VmRoots<'_> {
    fn visit(&mut self, f: &mut dyn FnMut(&mut Value)) {
        self.each(|_, v| f(v));
    }

    fn visit_named(&mut self, f: &mut dyn FnMut(&dyn std::fmt::Display, &mut Value)) {
        self.each(|slot, v| f(&slot, v));
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
                let was = *v;
                f(v);
                // Only what moved: a code reclaimed by the collection in
                // progress may still be listed, its handle freed (the
                // verifier visits these after sweeping).
                if *v != was {
                    unsafe { set_field(h, i, *v) };
                }
            }
        }
    }

    /// The closures a marked code's JIT code expects live, and the codes
    /// it uses by address (`JitSlot::deps`). A retired module
    /// is used by a marked code, by an evaluation running in it, or through
    /// the imports (of variables, or of macros it defines) of a module that
    /// is not retired or is used; its globals' values live then.
    fn retained(&mut self, marked: &dyn Fn(Value) -> bool, shade: &mut dyn FnMut(Value)) {
        for &code in self.jit_callers {
            let code = unsafe { &*code };
            if marked(code.handle.get()) {
                code.jit.callees.get().into_iter().flatten().for_each(|&v| shade(v));
                for &id in code.jit.deps.borrow().iter() {
                    // SAFETY: a live code's address.
                    if let Some(&dep) = self.code_ptrs.get(id as usize)
                        && !dep.is_null()
                    {
                        shade(unsafe { (*dep).handle.get() });
                    }
                }
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
            (self.code_ptrs.iter().filter(|p| !p.is_null()))
                // SAFETY: live codes' addresses.
                .map(|&p| unsafe { &*p })
                .filter(|c| !c.uses.is_empty() && marked(c.handle.get()))
                .flat_map(|c| c.uses.iter().copied()),
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
            let p = self.code_ptrs[i];
            // SAFETY: a live code's address.
            if !p.is_null()
                && !marked(unsafe { (*p).handle.get() })
                && let Some(code) = slot.take()
            {
                self.dead_codes.push((i as u32, code));
                self.free_codes.push(i as u32);
            }
        }
        // The next marking finds them anew.
        self.used.clear();
    }
}

/// A closure names code `id`, which is no live code's: see `Vm::closure_code`.
/// It ends the world, as a panic does (`Vm::as_execution` retires it):
/// its heap is corrupt, but each world has a cage of its own
/// (`alloc::cage`), so the others' are not.
#[cold]
#[inline(never)]
fn corrupt_closure(vm: &Vm, id: usize) -> ! {
    let why = format!("a closure names code {id}, which is none: the heap is corrupt");
    vm.retire(&why);
    panic!("{why}")
}

/// A table of codes (`Vm::code_ptrs`): every slot null.
fn code_table() -> Vec<*const Code, crate::alloc::Pages> {
    let layout = std::alloc::Layout::array::<*const Code>(CODE_SLOTS).expect("code table layout");
    let p = std::alloc::Allocator::allocate_zeroed(&crate::alloc::Pages, layout).unwrap_or_else(|_| std::alloc::handle_alloc_error(layout));
    // SAFETY: zeroed storage of this layout, from this allocator: a null
    // pointer in every slot.
    unsafe { Vec::from_raw_parts_in(p.cast::<*const Code>().as_ptr(), CODE_SLOTS, CODE_SLOTS, crate::alloc::Pages) }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Vm {
    fn drop(&mut self) {
        // Its values decode in its cage while it goes.
        let _cage = crate::alloc::enter_cage(self.heap.cage(), &self.symbols);
        // What host roots hold is freed with the heap: they read as
        // `#<void>` from here, not as freed memory.
        for root in self.roots.get_mut().drain(..).filter_map(|r| r.upgrade()) {
            root.kill();
        }
        // Waits still held by tasks end contained, like every other wait
        // ending (`Vm::drop_wait`): a registered future's panicking
        // destructor must not unwind out of teardown. The world is
        // going away; poisoning it now is only bookkeeping.
        let waits: Vec<crate::tasks::Wait> = self
            .tasks
            .iter_mut()
            .filter_map(|task| match std::mem::replace(&mut task.state, crate::tasks::State::Runnable) {
                crate::tasks::State::Waiting(wait) => Some(wait),
                _ => None,
            })
            .collect();
        for wait in waits {
            self.drop_wait(wait);
        }
        // The host's values the world held, and its natives' closures,
        // each dropped on its own, contained as above.
        for value in std::mem::take(&mut self.foreign) {
            self.contain_drop(value);
        }
        for native in std::mem::take(&mut self.natives) {
            self.contain_drop(native);
        }
    }
}

/// What `Vm::charging` holds: the account entered (if it was not
/// already) and the world's cage and symbols, both until dropped.
pub(crate) struct Charging {
    _account: Option<crate::alloc::Entered>,
    _cage: crate::alloc::CageEntered,
}

/// What `Vm::enter_account` returns: the world entered on this thread
/// (allocations charged to it, symbols interned as it) until dropped.
pub struct EnteredWorld {
    _account: crate::alloc::Entered,
    _cage: crate::alloc::CageEntered,
}

/// Drop a caught panic payload without letting a panicking destructor
/// unwind further: payloads are arbitrary host values (`panic_any`),
/// and a second panic here would escape the containment the payload was
/// caught by — across an `extern "C"` entry, aborting the process. A
/// payload whose drop panics is leaked instead; the world is poisoned
/// either way.
pub(crate) fn dispose_panic_payload(payload: Box<dyn std::any::Any + Send>) {
    if let Err(again) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload))) {
        std::mem::forget(again);
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
        if template::enabled() {
            return template::start(grants);
        }
        Vm::boot(grants)
    }

    /// A world built from scratch: builtins, then the prelude.
    fn boot(grants: Grants) -> Vm {
        let mut vm = Vm::bare_with(grants);
        if let Err(e) = vm.eval_in_raw(ROOT_MODULE, crate::PRELUDE_PATH, crate::PRELUDE) {
            panic!("prelude failed to load: {e}");
        }
        // What the prelude made lives for good: promote it now, so programs
        // start with an empty nursery, and count only their own collections.
        vm.collect_raw();
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
        let mut vm = Vm::empty(grants, account);
        // Builtins and modules intern as this world, like its executions
        // do, and are made in its cage.
        let _cage = crate::alloc::enter_cage(vm.heap.cage(), &vm.symbols);
        vm.new_module("root", None);
        vm.new_module("user", None);
        vm.specials[SpecialObj::ErrorRtd as usize] = vm.make_rtd("error", &["message", "irritants", "kind"]);
        vm.specials[SpecialObj::ContinuationRtd as usize] = vm.make_rtd("continuation", &["id"]);
        vm.specials[SpecialObj::ValuesRtd as usize] = vm.make_rtd("values", &[]);
        vm.specials[SpecialObj::TaskRtd as usize] = vm.make_rtd("task", crate::tasks::TASK_FIELDS);
        vm.specials[SpecialObj::ChannelRtd as usize] = vm.make_rtd("channel", &["slot", "generation"]);
        vm.specials[SpecialObj::ScopeRtd as usize] = vm.make_rtd("scope", scopes::SCOPE_FIELDS);
        crate::builtins::install(&mut vm);
        let apply = vm.global_var(ROOT_MODULE, reader::intern("apply"));
        vm.apply_native = vm.globals[apply as usize];
        vm.jit_inline = crate::jit::Inline::ALL
            .iter()
            .map(|&(inline, name)| (vm.get_global_in_raw(ROOT_MODULE, name).filter(|v| v.is_native()).expect(name), inline))
            .collect();
        vm.jit_fixnum = crate::jit::FIXNUM_NATIVES
            .iter()
            .map(|&name| vm.get_global_in_raw(ROOT_MODULE, name).filter(|v| v.is_native()).expect(name))
            .collect();
        vm.set_jit(jit_threshold_from_env());
        vm
    }

    /// A world with nothing in it yet: no modules, builtins or code; its
    /// runtime state (heap, registers, scheduler, symbols) fresh. `account` must be entered.
    fn empty(grants: Grants, account: crate::alloc::Account) -> Vm {
        let interrupt = Arc::new(AtomicBool::new(false));
        let budget = crate::alloc::Budget::for_account(account.reference());
        let mut vm = Vm {
            heap: Heap::new_in(budget.clone()),
            account,
            entered: None,
            entered_cage: None,
            memory_limit: 0,
            pressure: None,
            regs: Slots::filled_in(Value::VOID, INITIAL_REGS, budget.clone()).expect("initial registers for the world"),
            frames: Vec::with_capacity(1024),
            handlers: Vec::new(),
            globals: Slots::new_in(budget),
            moves: Moves::default(),
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
            staged_by: None,
            current_module: USER_MODULE,
            grants,
            reading_root: None,
            clock_resolution: 1,
            requiring: None,
            files: Vec::new(),
            file_codes: Vec::new(),
            free_files: Vec::new(),
            pinned_files: Vec::new(),
            held_files: FxHashMap::default(),
            held_modules: FxHashMap::default(),
            saved_modules: Vec::new(),
            unheld_files: Rc::default(),
            variable_docs: FxHashMap::default(),
            codes: Vec::new(),
            code_ptrs: code_table(),
            code_trigger: CODE_SLOTS / 16,
            codes_due: false,
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
            poisoned: Default::default(),
            jit_unwind: Vec::new(),
            interrupt: interrupt.clone(),
            stops: Arc::new(crate::stop::Stops::new(interrupt, Arc::default())),
            executions: Vec::new(),
            active: Vec::new(),
            thread: std::thread::current(),
            wake_notifier: None,
            natives: Vec::new(),
            apply_native: Value::VOID,
            jit_inline: Vec::new(),
            jit_fixnum: Vec::new(),
            stack_top: 0,
            scratch: Vec::new(),
            specials: [Value::VOID; SPECIALS],
            roots: Default::default(),
            world: crate::api::new_world_id(),
            foreign: Vec::new(),
            foreign_free: Vec::new(),
            foreign_type_names: FxHashMap::default(),
            next_id: 0,
            tasks: Vec::new(),
            free_tasks: Vec::new(),
            live: 0,
            lifetimes: Vec::new(),
            free_lifetimes: Vec::new(),
            cpu_marks: Vec::new(),
            run_queue: VecDeque::new(),
            wakes: Arc::default(),
            woken: Vec::new(),
            timers: BinaryHeap::new(),
            watchdog: None,
            spare_suspend: None,
            channels: Vec::new(),
            children: FxHashMap::default(),
            killed: VecDeque::new(),
            scopes: Vec::new(),
            free_scopes: Vec::new(),
            free_channels: Vec::new(),
            offers: Default::default(),
            current_task: None,
            jit_fuel: std::ptr::null_mut(),
            locals: FxHashMap::default(),
            out: crate::ports::Standard::process(),
            symbols: Default::default(),
        };
        vm.memory_limit = held::default_limit();
        vm.account.set_limits(vm.memory_limit, vm.memory_limit.saturating_mul(3), vm.memory_waker());
        vm
    }

    pub(crate) fn special(&self, s: SpecialObj) -> Value {
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
            loader: None,
            isolated: false,
            generation: self.staging_here().map_or(0, |s| s.generation),
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
    pub(crate) fn lookup_global(&self, module: u32, sym: u32) -> Option<GlobalBinding> {
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
                self.globals.push(Value::UNDEFINED, &mut self.moves);
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
    pub(crate) fn global_value_raw(&mut self, name: &str) -> Result<Value, Error> {
        let _world = self.cage_entry();
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

    pub(crate) fn define_macro(&mut self, module: u32, sym: u32, m: Rc<Macro>) {
        if !self.bindings.contains_key(&(module, sym)) {
            self.modules[module as usize].defined.push(sym);
        }
        self.bindings.insert((module, sym), GlobalBinding::Macro(m));
    }

    /// The capabilities this world holds.
    pub fn grants(&self) -> Grants {
        self.grants
    }

    /// Grant or revoke `cap`, one checked when used (loading, the clock,
    /// entropy): the others are fixed when the world is made. A world
    /// started from this one's snapshot starts with its grants. Taking
    /// loading back loads no more modules from files; requiring one
    /// already loaded still imports it.
    pub fn set_grant(&mut self, cap: Capability, granted: bool) {
        assert!(cap.checked_when_called(), "{} is fixed when a world is made", cap.name());
        self.grants = if granted { self.grants.with(cap) } else { self.grants.without(cap) };
    }

    /// Make the clock natives return multiples of `resolution` (at least a
    /// microsecond): a host serving code it does not trust coarsens what
    /// timing that code can measure.
    pub fn set_clock_resolution(&mut self, resolution: std::time::Duration) {
        self.clock_resolution = (resolution.as_micros() as u64).max(1);
    }

    /// `micros` rounded down to the clock's resolution.
    pub(crate) fn coarsen(&self, micros: u64) -> u64 {
        micros - micros % self.clock_resolution
    }

    /// The instant `wait` from now, for a timer of the native `who`: on the
    /// clock's resolution, rounded up, so that what fires when tells no
    /// more than the clock would. Timers need the clock, but for no wait.
    pub(crate) fn timer_deadline(&self, wait: std::time::Duration, who: &str) -> Result<Option<std::time::Instant>, Error> {
        if wait.is_zero() {
            return Ok(Some(std::time::Instant::now()));
        }
        self.need(Capability::Clock, who)?;
        let epoch = crate::stdlib::jiffy_epoch();
        let target = epoch.elapsed().checked_add(wait).and_then(|t| u64::try_from(t.as_micros()).ok());
        let target = target.and_then(|us| us.checked_next_multiple_of(self.clock_resolution));
        Ok(target.and_then(|us| epoch.checked_add(std::time::Duration::from_micros(us))))
    }

    /// Refuse, for the native `who`, unless this world holds `cap`, one
    /// checked when called.
    pub fn need(&self, cap: Capability, who: &str) -> Result<(), Error> {
        if self.grants.has(cap) { Ok(()) } else { Err(Error::new(format!("{who}: not granted in this world (needs {})", cap.name()))) }
    }

    /// Define natives (with `f`) that need `cap`: in a world without it, each
    /// becomes a procedure that raises "NAME: not granted". Natives needing
    /// a capability checked when called call `Vm::need` instead.
    pub fn requiring<T>(&mut self, cap: Capability, f: impl FnOnce(&mut Vm) -> T) -> T {
        assert!(!cap.checked_when_called(), "{} is checked when called: use Vm::need", cap.name());
        let outer = self.requiring.replace(cap);
        let result = f(self);
        self.requiring = outer;
        result
    }

    pub(crate) fn define_native(&mut self, mut native: Native) {
        let _world = self.cage_entry();
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
        let v = self.get_global_in_raw(ROOT_MODULE, name).filter(|v| v.is_native());
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
        // A name of this world's, whichever world the host is in.
        let _world = self.cage_entry();
        symbol_name(self.global_names[g as usize])
    }

    /// Value of a global defined in the user module or root, by name.
    pub(crate) fn get_global_raw(&self, name: &str) -> Option<Value> {
        self.get_global_in_raw(USER_MODULE, name)
    }

    /// Value of a global visible from `module`, by name.
    pub(crate) fn get_global_in_raw(&self, module: u32, name: &str) -> Option<Value> {
        let _world = self.cage_entry();
        match self.lookup_global(module, reader::intern(name))? {
            GlobalBinding::Var(g) => Some(self.globals[g as usize]).filter(|v| *v != Value::UNDEFINED),
            GlobalBinding::Macro(_) => None,
        }
    }

    pub(crate) fn set_global_raw(&mut self, name: &str, v: Value) {
        let _world = self.cage_entry();
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
        self.host_ref(|_| exports.iter().map(|(_, seen)| symbol_name(*seen)).collect()).ok()
    }

    /// The names of the modules loaded, each once, in the order loaded.
    pub fn loaded_module_names(&self) -> Vec<Rc<str>> {
        let mut seen = rustc_hash::FxHashSet::default();
        self.modules.iter().map(|m| m.name.clone()).filter(|n| !n.is_empty() && seen.insert(n.clone())).collect()
    }

    /// The names `module` itself defines, in no order.
    pub fn module_definitions(&self, module: u32) -> Vec<Rc<str>> {
        self.host_ref(|vm| {
            let defined = vm.modules.get(module as usize).map_or(&[][..], |m| &m.defined);
            let mut syms: Vec<u32> = defined.iter().copied().filter(|&s| vm.bindings.contains_key(&(module, s))).collect();
            syms.sort_unstable();
            syms.dedup();
            syms.into_iter().map(symbol_name).collect()
        })
        .unwrap_or_default()
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
        // A name already canonical, as `loaded-modules` gives them, is found
        // without asking the file system.
        let file = self
            .module_paths
            .get(Path::new(name))
            .copied()
            .or_else(|| Path::new(name).canonicalize().ok().and_then(|p| self.module_paths.get(&p).copied()));
        file.or_else(|| self.modules.iter().rposition(|m| &*m.name == name && !m.retired).map(|m| m as u32))
    }

    /// The module called `name`, or the module of the file at path `name`
    /// (relative to the working directory), loading the file if needed.
    pub fn find_module(&mut self, name: &str) -> Result<u32, Error> {
        self.host_try(|vm| vm.find_module_raw(name))
    }

    pub(crate) fn find_module_raw(&mut self, name: &str) -> Result<u32, Error> {
        if let Some(m) = self.loaded_module(name) {
            return Ok(m);
        }
        self.check_loading()?;
        let path = Path::new(name).canonicalize().map_err(|_| Error::new(format!("no module {name}")))?;
        self.load_module(&path).map_err(|e| Error::new(format!("module {name}: {}", e.msg)))
    }

    /// Read no file outside `dir` to load code or text in this world (nor
    /// in worlds started from its snapshot): modules, libraries,
    /// included files, packages, `file->string`, input file ports, but
    /// for Techne's own Lisp (`library::lisp_dir`), the libraries it ships
    /// such as `(techne http)`. A host loading a deployment confines it to
    /// the deployment's files. A path outside, through a symbolic link
    /// too, reads as one that does not exist.
    /// The path is checked, then opened: `dir` must not change meanwhile.
    /// The host's own `eval_file` is not confined.
    pub fn confine_reading(&mut self, dir: &Path) -> Result<(), Error> {
        let dir = dir.canonicalize().map_err(|e| Error::new(format!("{}: {e}", dir.display())))?;
        self.reading_root = Some(dir);
        Ok(())
    }

    /// The directory reading is confined to here, if it is
    /// (`confine_reading`).
    pub fn reading_root(&self) -> Option<&Path> {
        self.reading_root.as_deref()
    }

    /// Whether `path` may be read here (`confine_reading`).
    pub fn may_read(&self, path: &Path) -> bool {
        let Some(root) = &self.reading_root else { return true };
        let own = crate::library::lisp_dir().canonicalize();
        path.canonicalize().is_ok_and(|p| p.starts_with(root) || own.is_ok_and(|own| p.starts_with(own)))
    }

    pub(crate) fn check_loading(&self) -> Result<(), Error> {
        match self.grants.has(Capability::Loading) {
            true => Ok(()),
            false => Err(Error::new("loading modules is not granted in this world (needs loading)")),
        }
    }

    /// The module of the file at canonical `path`, loaded once.
    pub(crate) fn load_module(&mut self, path: &Path) -> Result<u32, Error> {
        match self.begin_module_load(path, None)? {
            ModuleLoad::Loaded(m) => Ok(m),
            ModuleLoad::Load(m, text) => {
                let result = self.eval_in_raw(m, &path.to_string_lossy(), &text);
                self.end_module_load(m);
                result.map(|_| m)
            }
        }
    }

    /// The module of the file at canonical `path` if it is loaded, or else
    /// a new one for it, registered (so that requiring it again finds it,
    /// and a circular dependency is told) and marked loading, with the
    /// file's text: evaluate that in it, then `end_module_load`. A
    /// package's own files load afresh for each generation.
    pub(crate) fn begin_module_load(&mut self, path: &Path, loader: Option<ExecId>) -> Result<ModuleLoad, Error> {
        self.begin_module_load_from(path, loader, None)
    }

    /// `begin_module_load`, the module made from `text` rather than its
    /// file's when it is not loaded yet.
    fn begin_module_load_from(&mut self, path: &Path, loader: Option<ExecId>, text: Option<String>) -> Result<ModuleLoad, Error> {
        let staged = self.staging_here().filter(|s| path.starts_with(&s.dir)).map(|s| s.modules.get(path).copied());
        let known = match staged {
            Some(m) => m,
            None => self.module_paths.get(path).copied(),
        };
        let here = self.running_execution();
        match known {
            // Loading still: from here (a cycle), or from a task that waits.
            Some(m) if self.modules[m as usize].loading && self.modules[m as usize].loader.is_none_or(|e| self.stops.is_live(e)) => {
                Err(Error::new(match self.modules[m as usize].loader {
                    Some(e) if e != here => "module being loaded by another task",
                    _ => "circular module dependency",
                }))
            }
            Some(m) => {
                // Its loader was killed: as a load that failed, it stays
                // as far as it got.
                self.end_module_load(m);
                Ok(ModuleLoad::Loaded(m))
            }
            None => {
                // The text, and the copy kept as the file's source.
                let text = match text {
                    Some(text) => text,
                    None => self.read_file_admitted(&path.to_string_lossy(), 2)?,
                };
                let m = self.new_module(&path.to_string_lossy(), Some(path.to_path_buf()));
                match staged {
                    Some(_) => self.staging.as_mut().expect("staging").modules.insert(path.to_path_buf(), m),
                    None => self.module_paths.insert(path.to_path_buf(), m),
                };
                self.modules[m as usize].loading = true;
                self.modules[m as usize].loader = loader;
                Ok(ModuleLoad::Load(m, text))
            }
        }
    }

    /// The module `begin_module_load` made has been evaluated (or failed).
    pub(crate) fn end_module_load(&mut self, m: u32) {
        self.modules[m as usize].loading = false;
        self.modules[m as usize].loader = None;
    }

    /// Where `from` finds the file `spec` it requires: relative to its own
    /// file, canonical.
    fn required_path(&self, from: u32, spec: &str) -> std::io::Result<PathBuf> {
        let base =
            self.modules[from as usize].path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."));
        base.join(spec).canonicalize()
    }

    /// The forms of a module the next of `forms` requires and that is not
    /// loaded yet, made and marked loading as `begin_module_load` does:
    /// evaluated (from Scheme, so that it can wait), and then
    /// `end_module_load`, before that form is compiled, which then finds it
    /// loaded. A top-level `(require "file" ...)` only: one inside another
    /// form loads as it is compiled.
    pub(crate) fn next_required(&mut self, forms: &Forms) -> Result<Option<Forms>, Error> {
        let specs: Vec<Sexp> = match forms.forms.borrow().front().and_then(Sexp::list) {
            // The special form, unless the module binds the name otherwise.
            Some([Sexp::Sym(head), specs @ ..])
                if symbol_name(*head).as_ref() == "require" && self.lookup_global(forms.module, *head).is_none() =>
            {
                specs.to_vec()
            }
            _ => return Ok(None),
        };
        if self.check_loading().is_err() {
            return Ok(None);
        }
        // In order, as the compiler would load them: from one it cannot
        // load (not a string, not found), it reports that, and loads none
        // after it.
        for spec in &specs {
            let Sexp::Str(spec) = spec else { return Ok(None) };
            let Ok(path) = self.required_path(forms.module, spec) else { return Ok(None) };
            let loader = Some(self.running_execution());
            match self.begin_module_load(&path, loader)? {
                ModuleLoad::Load(m, text) => {
                    let forms = self.read_forms(m, &path.to_string_lossy(), &text);
                    if forms.is_err() {
                        self.end_module_load(m);
                    }
                    return forms.map(Some);
                }
                // One the compiler would fail to import from, too.
                ModuleLoad::Loaded(m) if self.exported(m, spec).is_err() => return Ok(None),
                ModuleLoad::Loaded(_) => {}
            }
        }
        Ok(None)
    }

    /// The forms of the module of the file at `path`, read from `text`
    /// rather than the file (an editor's buffer, unsaved edits and all):
    /// made and marked loading as `begin_module_load` does, to evaluate in
    /// it (`%load-forms`). None if the module is loaded already.
    pub(crate) fn module_forms_from(&mut self, path: &Path, text: String) -> Result<Option<Forms>, Error> {
        self.check_loading()?;
        let path = path.canonicalize().map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        let loader = Some(self.running_execution());
        match self.begin_module_load_from(&path, loader, Some(text))? {
            ModuleLoad::Loaded(_) => Ok(None),
            ModuleLoad::Load(m, text) => {
                let forms = self.read_forms(m, &path.to_string_lossy(), &text);
                if forms.is_err() {
                    self.end_module_load(m);
                }
                forms.map(Some)
            }
        }
    }

    /// Let go of the file and module of `forms`, done with: not when the
    /// collector finds them unreachable, which may be long after (a package
    /// generation unloaded meanwhile would be kept until then).
    /// Drop what is left of `forms` to evaluate, as when they end.
    pub(crate) fn drop_forms(&mut self, forms: &Forms) {
        drop(std::mem::take(&mut *forms.forms.borrow_mut()));
        self.finish_forms(forms);
    }

    pub(crate) fn finish_forms(&mut self, forms: &Forms) {
        drop(forms.hold.borrow_mut().take());
        self.release_unheld_files();
    }

    /// The module `forms` are evaluated in.
    pub(crate) fn forms_module(forms: &Forms) -> u32 {
        forms.module
    }

    /// Load a new generation of the package whose main file is `path`: it
    /// and the files it requires from its directory load into fresh
    /// modules, which `publish_staged` makes the ones `require` and module
    /// names find, and `discard_staged` retires. Returns the main module and
    /// the generation. Nothing else changes unless the package's code does
    /// it; if loading fails, the generation is retired.
    pub fn stage_package(&mut self, path: &Path) -> Result<(u32, u32), Error> {
        self.abandon_dead_package();
        if self.staging.is_some() || self.staged.is_some() {
            return Err(Error::new("a package is already being loaded"));
        }
        self.check_loading()?;
        let path = path.canonicalize().map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        let dir = path.parent().map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        self.generations += 1;
        let generation = self.generations;
        self.staging = Some(Staging { dir, generation, modules: FxHashMap::default(), main: None, owner: None });
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

    /// The package being staged, if it is for what runs here: staged from
    /// Rust, or by this execution. Another task's `require` meanwhile finds
    /// the published modules, and makes modules of no generation.
    fn staging_here(&self) -> Option<&Staging> {
        self.staging.as_ref().filter(|s| s.owner.is_none_or(|o| o == self.running_execution()))
    }

    /// Give up staging, and a generation staged, that an execution killed
    /// meanwhile left behind.
    fn abandon_dead_package(&mut self) {
        if let Some(owner) = self.staging.as_ref().and_then(|s| s.owner)
            && !self.stops.is_live(owner)
        {
            self.end_package(false);
        }
        if self.staged.is_some() && self.staged_by.is_some_and(|e| !self.stops.is_live(e)) {
            self.discard_staged();
        }
    }

    /// Begin loading a new generation of the package whose main file is
    /// `path`, as `stage_package` does, but leaving its main module's forms
    /// for Scheme to evaluate (`load-package`, so that they can wait), then
    /// `end_package`. Returns them and the generation.
    pub(crate) fn begin_package(&mut self, path: &Path) -> Result<(Forms, u32), Error> {
        // One loading still, unless what loaded it was killed meanwhile.
        self.abandon_dead_package();
        if self.staging.is_some() || self.staged.is_some() {
            return Err(Error::new("a package is already being loaded"));
        }
        self.check_loading()?;
        let path = path.canonicalize().map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        let dir = path.parent().map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        self.generations += 1;
        let generation = self.generations;
        let owner = Some(self.running_execution());
        self.staging = Some(Staging { dir, generation, modules: FxHashMap::default(), main: None, owner });
        let forms = match self.begin_module_load(&path, owner) {
            Ok(ModuleLoad::Load(m, text)) => {
                self.staging.as_mut().expect("staging").main = Some(m);
                let forms = self.read_forms(m, &path.to_string_lossy(), &text);
                if forms.is_err() {
                    self.end_module_load(m);
                }
                forms
            }
            Ok(ModuleLoad::Loaded(_)) => Err(Error::new("a package's main module is loaded already")),
            Err(e) => Err(e),
        };
        match forms {
            Ok(forms) => Ok((forms, generation)),
            Err(e) => {
                self.end_package(false);
                Err(e)
            }
        }
    }

    /// End loading the package `begin_package` began: staged if `ok`, to
    /// publish (`publish_staged`), retired otherwise. Returns its main
    /// module and generation if staged.
    pub(crate) fn end_package(&mut self, ok: bool) -> Option<(u32, u32)> {
        let staging = self.staging.take()?;
        match (ok, staging.main) {
            (true, Some(main)) => {
                self.staged = Some((staging.generation, staging.modules));
                self.staged_by = staging.owner;
                Some((main, staging.generation))
            }
            _ => {
                self.retire_generation(staging.generation);
                None
            }
        }
    }

    /// Make the staged generation's modules the current ones.
    pub fn publish_staged(&mut self) {
        self.staged_by = None;
        if let Some((_, modules)) = self.staged.take() {
            self.module_paths.extend(modules);
        }
    }

    pub fn discard_staged(&mut self) {
        self.staged_by = None;
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
        // A module loaded already is imported without loading: where
        // loading is not granted, any other fails alike, there or not.
        let path = self.required_path(from, spec);
        if !path.as_ref().is_ok_and(|p| self.module_paths.contains_key(p)) {
            self.check_loading().map_err(|e| Error::new(format!("require {spec}: {}", e.msg)))?;
        }
        let path = path.map_err(|e| Error::new(format!("require {spec}: {e}")))?;
        let m = self.load_module(&path).map_err(|e| match e.msg.as_str() {
            "circular module dependency" => Error::new(format!("require {spec}: circular module dependency")),
            _ => e,
        })?;
        let exported = self.exported(m, spec)?;
        self.add_imports(from, exported);
        Ok(())
    }

    /// Import `bindings` into `module`. A name the module used before,
    /// left unbound, is the import's now: its forward reference, which
    /// would shadow the import, is dropped. Code compiled before still
    /// reads the forward reference: evaluated again, it sees the import.
    pub(crate) fn add_imports(&mut self, module: u32, bindings: impl IntoIterator<Item = (u32, GlobalBinding)>) {
        for (name, binding) in bindings {
            if let Some(&GlobalBinding::Var(g)) = self.bindings.get(&(module, name))
                && !self.user_defined[g as usize]
                && self.globals[g as usize] == Value::UNDEFINED
            {
                self.bindings.remove(&(module, name));
            }
            self.modules[module as usize].imports.insert(name, binding);
        }
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
    pub(crate) fn add_code(&mut self, mut code: Code) -> Result<u32, Error> {
        if self.free_codes.is_empty() && self.codes.len() >= CODE_SLOTS {
            return Err(Error::new(format!("out of memory: more than {CODE_SLOTS} codes live at once")));
        }
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
        // The id tables, in step: resolution here and in compiled code,
        // and the handle the collector shades for a closure of this code.
        // Mutable: the collector updates young constants through it.
        let ptr: *mut Code = self.codes[i as usize].as_deref_mut().expect("live code");
        // SAFETY: just installed.
        let handle = unsafe { (*ptr).handle.get() };
        self.code_ptrs[i as usize] = ptr;
        if i as usize == self.heap.code_handles.len() {
            self.heap.code_handles.push(handle);
        } else {
            self.heap.code_handles[i as usize] = handle;
        }
        self.compiling.push(i);
        if young {
            self.young_consts.push(ptr);
        }
        let file = self.code(i).file as usize;
        self.file_codes[file] += 1;
        if self.codes.len() - self.free_codes.len() > self.code_trigger {
            self.codes_due = true;
            self.heap.request_collection();
        }
        Ok(i)
    }

    /// The code at index `i`, which is live.
    pub(crate) fn code(&self, i: u32) -> &Code {
        let p = self.code_ptrs[i as usize];
        assert!(!p.is_null(), "live code");
        // SAFETY: a live code's address (`code_ptrs`).
        unsafe { &*p }
    }

    /// `code`, to change.
    pub(crate) fn code_mut(&mut self, i: u32) -> &mut Code {
        let p = self.code_ptrs[i as usize].cast_mut();
        assert!(!p.is_null(), "live code");
        // SAFETY: a live code's address, taken mutably (`code_ptrs`); the
        // VM borrowed mutably, nothing else uses it meanwhile.
        unsafe { &mut *p }
    }

    /// Whether code `i` is live.
    pub(crate) fn code_live(&self, i: u32) -> bool {
        self.code_ptrs.get(i as usize).is_some_and(|p| !p.is_null())
    }

    /// The live codes.
    fn live_code_refs(&self) -> impl Iterator<Item = &Code> {
        // SAFETY: live codes' addresses (`code_ptrs`).
        self.code_ptrs[..self.codes.len()].iter().filter(|p| !p.is_null()).map(|&p| unsafe { &*p })
    }

    /// The code of closure `f`: its first field is the code's id.
    /// The caller has checked `f` is a closure; a live closure's code
    /// is live (the collector shades its handle, `Heap::shade_code`).
    /// An id that names no live code, which only a forged or stale closure
    /// holds, ends the world: the table is outside the heap, and the heap
    /// is corrupt (`corrupt_closure`).
    #[inline]
    pub(crate) fn closure_code(&self, f: Value) -> *const Code {
        let slot = code_slot(unsafe { field(f.as_ptr(), 0) });
        // SAFETY: a slot is below `CODE_SLOTS`, the table's length.
        let code = unsafe { *self.code_ptrs.get_unchecked(slot) };
        if code.is_null() {
            corrupt_closure(self, slot);
        }
        code
    }

    pub(crate) fn set_captures(&mut self, code: u32, captures: Vec<CapSrc>) {
        self.code_mut(code).captures = captures.into();
    }

    /// Codes alive now.
    pub fn live_codes(&self) -> usize {
        self.codes.len() - self.free_codes.len()
    }

    /// Materialise a literal of code: its pairs, vectors, strings and
    /// bytevectors cannot be changed. Heap parts go to the old space; they
    /// only reference each other, so they need no remembering.
    pub(crate) fn constant(&mut self, s: &Sexp) -> Value {
        self.build_datum(s, heap::IMMUTABLE, None)
    }

    /// `constant`, metered: once `meter` halts, the rest is void.
    pub(crate) fn constant_metered(&mut self, s: &Sexp, meter: &meter::Meter) -> Value {
        self.build_datum(s, heap::IMMUTABLE, Some(meter))
    }

    /// Materialise a datum that can be changed (what `read` returns).
    pub(crate) fn datum(&mut self, s: &Sexp) -> Value {
        self.build_datum(s, 0, None)
    }

    fn build_datum(&mut self, s: &Sexp, flags: u64, meter: Option<&meter::Meter>) -> Value {
        let mut labels = Labels { flags, meter, ..Labels::default() };
        let v = self.constant_in(s, &mut labels);
        if labels.placeholders.is_empty() { v } else { labels.patch(v) }
    }

    fn constant_in(&mut self, s: &Sexp, labels: &mut Labels) -> Value {
        if let Some(m) = labels.meter {
            if m.step().is_err() {
                return Value::VOID;
            }
            // A long string, bytevector or number, which may be one many times:
            // look after each (steps look often enough for short ones).
            let long = match s {
                Sexp::Str(b) => b.len() >= meter::LONG,
                Sexp::Bytes(b) => b.len() >= meter::LONG,
                Sexp::BigInt(b) => b.bits() / 8 >= meter::LONG as u64,
                Sexp::Ratio(r) => (r.numer().bits() + r.denom().bits()) / 8 >= meter::LONG as u64,
                _ => false,
            };
            if long {
                let v = self.constant_of(s, labels);
                let _ = m.look();
                return v;
            }
        }
        match s {
            Sexp::List(..) | Sexp::Vector(_) | Sexp::Dict(_) | Sexp::Set(_) | Sexp::Labeled(..) | Sexp::Complex(..) | Sexp::Ratio(_) => {
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
            Sexp::Dict(items) | Sexp::Set(items) => {
                let vals: Vec<Value> = items.iter().map(|i| self.constant_in(i, labels)).collect();
                crate::dict::constant(self, if matches!(s, Sexp::Dict(_)) { Kind::Dict } else { Kind::Set }, &vals)
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

    /// Access heap values without collection, Scheme reentry or stack growth.
    /// Reserved nursery allocation is fallible; slow work must happen after
    /// the callback returns. Heap-backed values and references cannot leave;
    /// copy their data when it must outlive the scope.
    ///
    /// Values the host holds (`api::Value`) come in with `HeapAccess::import`.
    ///
    /// ```
    /// use techne_vm::{api::Value, vm::Vm};
    /// let mut vm = Vm::bare();
    /// let n = vm.with_heap(|heap| heap.import(&Value::from(42)).unwrap().as_int()).unwrap();
    /// assert_eq!(n, Some(42));
    /// vm.with_heap(|mut heap| {
    ///     let one = heap.import(&Value::from(1)).unwrap();
    ///     if let Some(pair) = heap.try_cons(one, one) {
    ///         let (a, b) = pair.as_pair().unwrap();
    ///         assert_eq!((a.as_int(), b.as_int()), (Some(1), Some(1)));
    ///     }
    ///     if let Some(text) = heap.try_string("borrowed") {
    ///         let owned = heap.as_str(text).unwrap().to_owned();
    ///         let _ = heap.try_string("another allocation");
    ///         assert_eq!(owned, "borrowed");
    ///     }
    /// })
    /// .unwrap();
    /// ```
    ///
    /// Heap values cannot escape:
    /// ```compile_fail
    /// use techne_vm::{api::Value, vm::Vm};
    /// let mut vm = Vm::bare();
    /// let escaped = vm.with_heap(|heap| heap.import(&Value::NIL).unwrap());
    /// let _ = escaped.as_pair();
    /// ```
    ///
    /// Neither can a heap slice:
    /// ```compile_fail,E0515
    /// use techne_vm::{api::Value, vm::Vm};
    /// let mut vm = Vm::bare();
    /// let escaped = vm.with_heap(|mut heap| {
    ///     let text = heap.try_string("borrowed")?;
    ///     heap.as_str(text)
    /// });
    /// println!("{escaped:?}");
    /// ```
    ///
    /// Borrowed bytes also exclude mutable access to the context:
    /// ```compile_fail,E0502
    /// use techne_vm::vm::Vm;
    /// let mut vm = Vm::bare();
    /// vm.with_heap(|mut heap| {
    ///     if let Some(text) = heap.try_string("borrowed") {
    ///         let bytes = heap.as_str(text).unwrap();
    ///         let _ = heap.try_string("another allocation");
    ///         assert_eq!(bytes, "borrowed");
    ///     }
    /// });
    /// ```
    ///
    /// A boundary cannot run while access is live:
    /// ```compile_fail,E0499
    /// use techne_vm::{api::Value, vm::Vm};
    /// let mut vm = Vm::bare();
    /// vm.with_heap(|heap| {
    ///     let value = heap.import(&Value::NIL).unwrap();
    ///     vm.eval_source("(+ 1 2)").unwrap();
    ///     let _ = value.as_pair();
    /// });
    /// ```
    ///
    /// Independent scopes cannot exchange their branded values:
    /// ```compile_fail,E0521
    /// use techne_vm::{api::Value, vm::Vm};
    /// let (mut a, mut b) = (Vm::bare(), Vm::bare());
    /// a.with_heap(|heap_a| {
    ///     let value = heap_a.import(&Value::NIL).unwrap();
    ///     b.with_heap(|mut heap_b| { let _ = heap_b.try_cons(value, value); });
    /// });
    /// ```
    /// A retired world gives no access: its error.
    pub fn with_heap<R>(&mut self, f: impl for<'gc> FnOnce(HeapAccess<'gc>) -> R) -> Result<R, Error> {
        self.host(|vm| f(HeapAccess::new(vm)))
    }

    /// `with_heap` inside the VM, its world entered.
    #[inline(always)]
    pub(crate) fn heap_access<R>(&mut self, f: impl for<'gc> FnOnce(HeapAccess<'gc>) -> R) -> R {
        f(HeapAccess::new(self))
    }

    #[inline(always)]
    pub(crate) fn alloc(&mut self, words: usize) -> *mut u64 {
        if self.heap.has_room(words) {
            return self.heap.bump(words);
        }
        self.alloc_slow(words)
    }

    /// `words` in the nursery (collecting first if needed); for blocks that
    /// are split into several objects. `words` must fit the nursery.
    pub(crate) fn reserve_nursery(&mut self, words: usize) -> *mut u64 {
        debug_assert!(words <= self.heap.nursery_capacity());
        if !self.heap.has_room(words) {
            self.collect_raw();
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
        self.collect_raw();
        self.heap.bump(words)
    }

    /// Collect: the young generation, or a whole cycle when one is due.
    pub fn collect(&mut self) {
        let _ = self.host(Vm::collect_raw);
    }

    /// Collect a whole cycle.
    pub fn full_collect(&mut self) {
        let _ = self.host(Vm::full_collect_raw);
    }

    pub(crate) fn collect_raw(&mut self) {
        // Past the codes' trigger, a whole cycle, which frees them.
        if std::mem::take(&mut self.codes_due) {
            self.collect_with(Heap::full_collect);
        } else {
            self.collect_with(Heap::collect);
        }
        self.check_memory();
    }

    pub(crate) fn full_collect_raw(&mut self) {
        self.collect_with(Heap::full_collect);
        // Back under, it is under pressure no more.
        if self.held().total() <= self.limit_now() {
            self.pressure = None;
        }
    }

    fn collect_with(&mut self, collect: fn(&mut Heap, &mut dyn Roots)) {
        let _charged = self.account.enter();
        let _cage = crate::alloc::enter_cage(self.heap.cage(), &self.symbols);
        let cycles = self.heap.stats.full;
        // What forms collected before let go of is no root now: their
        // modules (a package generation's, say) may go in this collection.
        self.release_unheld_files();
        let running = self.running_codes();
        let jit_callers = std::mem::take(&mut self.jit_callers);
        let young_consts = std::mem::take(&mut self.young_consts);
        // Those evaluations run in: the running one's, each task's current
        // module (a suspended one's, or the outer stack's while a task
        // runs), and those source text is evaluated in a form at a time.
        let pinned_modules = self
            .pinned_modules
            .iter()
            .copied()
            .chain([self.current_module])
            .chain(self.saved_modules.iter().copied())
            .chain(
                self.tasks
                    .iter()
                    .filter(|t| !matches!(t.state, crate::tasks::State::Done))
                    .flat_map(|t| std::iter::once(t.stack.module).chain(t.stack.saved_modules.iter().copied())),
            )
            .chain(self.held_modules.keys().copied())
            .collect();
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
            code_ptrs,
            free_codes,
            dead_codes,
            bindings,
            scratch,
            specials,
            roots,
            stack_top,
            tasks,
            scopes,
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
            code_ptrs: &code_ptrs[..codes.len()],
            codes,
            free_codes,
            dead_codes,
            running,
            jit_callers: &jit_callers,
            bindings,
            young_consts: &young_consts,
            scratch,
            specials,
            roots: roots.get_mut(),
            tasks,
            scopes,
        };
        collect(heap, &mut r);
        self.jit_callers = jit_callers;
        // A collection empties the nursery; codes it found dead are dropped
        // with them below.
        self.young_consts = young_consts;
        self.young_consts.retain(|&c| unsafe { (*c).consts.iter().any(|v| v.is_ptr() && heap.in_nursery(v.as_ptr())) });
        self.release_dead_foreign();
        self.release_dead_codes();
        if self.heap.stats.full != cycles {
            let live = self.codes.len() - self.free_codes.len();
            self.code_trigger = (2 * live).clamp(CODE_SLOTS / 16, CODE_SLOTS / 2);
        }
        self.release_dead_modules();
        // Only now: the codes dropped above were looked at until then.
        self.heap.release_empty();
        // The nursery, empty, grows if the world has room for it.
        let growth = self.heap.nursery_growth();
        if growth > 0 && growth <= self.room() {
            self.heap.grow_nursery();
        }
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
        for (id, code) in std::mem::take(&mut self.dead_codes) {
            // The id resolves nowhere from here: any missed reference
            // finds a null address and no handle, not a freed `Code`
            // (the slot itself may already be reused only after this,
            // since `install_code` cannot run during a collection).
            self.code_ptrs[id as usize] = std::ptr::null();
            self.heap.code_handles[id as usize] = Value::VOID;
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
            let value = self.foreign[i].take();
            self.foreign_free.push(i);
            self.contain_drop(value);
        }
    }

    /// Drop `value`, a host's (a foreign value, a native's closure): a
    /// panic in its destructor retires the world rather than unwinding
    /// out of a collection or of the world's teardown.
    pub(crate) fn contain_drop<T>(&self, value: T) {
        if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(value))) {
            self.poison_panic(panic.as_ref());
            dispose_panic_payload(panic);
        }
    }

    /// Keep `v` alive and up to date across collections while the `Root` lives.
    pub(crate) fn root(&mut self, v: Value) -> Root {
        // The host may root between executions, or another VM on this
        // thread may be running.
        let _charged = self.charging();
        let root = Root::new(v, self.world);
        self.roots.get_mut().push(Rc::downgrade(root.cell()));
        root
    }

    /// This world's identity.
    pub(crate) fn world_id(&self) -> crate::api::WorldId {
        self.world
    }

    /// Wrap a Rust value as a Scheme object. It is dropped when the object dies.
    pub(crate) fn make_foreign_raw(&mut self, value: Rc<dyn Any>, type_name: &'static str) -> Value {
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

    pub(crate) fn foreign_raw(&self, v: Value) -> Option<(&Rc<dyn Any>, &'static str)> {
        if !is_kind(v, Kind::Foreign) {
            return None;
        }
        let i = unsafe { *crate::heap::word(v.as_ptr(), 1) } as usize;
        self.foreign.get(i)?.as_ref().map(|(rc, name)| (rc, *name))
    }

    pub fn live_foreign_count(&self) -> usize {
        self.foreign.iter().filter(|f| f.is_some()).count()
    }

    pub(crate) fn alloc_pair_raw(&mut self, car: Value, cdr: Value) -> Value {
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
    pub(crate) fn make_string_raw(&mut self, text: &str) -> Value {
        let bytes = text.as_bytes();
        let p = self.alloc(heap::string_words(bytes.len()));
        unsafe { init_string(p, bytes) };
        Value::ptr(p)
    }

    pub(crate) fn make_bytevector_raw(&mut self, bytes: &[u8]) -> Value {
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
    pub(crate) fn make_vector_raw(&mut self, items: &[Value]) -> Value {
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
    pub(crate) fn make_list_raw(&mut self, items: &[Value]) -> Value {
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
    pub(crate) fn make_int_raw(&mut self, i: i64) -> Value {
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
    pub(crate) fn make_error_object_raw(&mut self, message: &str, irritants: &[Value]) -> Value {
        self.make_error_object_of_raw(message, irritants, ErrorKind::General)
    }

    /// An error object; its `kind` field is `#f`, `file`, `read` or
    /// `violation`.
    pub(crate) fn make_error_object_of_raw(&mut self, message: &str, irritants: &[Value], kind: ErrorKind) -> Value {
        let mark = self.scratch.len();
        self.scratch.extend_from_slice(irritants);
        let msg = self.make_string_raw(message);
        self.scratch.push(msg);
        let items: Vec<Value> = self.scratch[mark..self.scratch.len() - 1].to_vec();
        let list = self.make_list_raw(&items);
        let msg = self.scratch[self.scratch.len() - 1];
        self.scratch.truncate(mark);
        let rtd = self.special(SpecialObj::ErrorRtd);
        let kind = match kind {
            ErrorKind::General | ErrorKind::Exit(_) | ErrorKind::Killed | ErrorKind::Poisoned => Value::FALSE,
            ErrorKind::File => Value::symbol(reader::intern("file")),
            ErrorKind::Read => Value::symbol(reader::intern("read")),
            ErrorKind::Violation => Value::symbol(reader::intern("violation")),
        };
        self.make_record(rtd, &[msg, list, kind])
    }

    #[inline(always)]
    pub(crate) fn write_barrier(&mut self, obj: *mut u64, v: Value) {
        self.heap.barrier(obj, v);
    }

    // ----- running code -----

    /// Evaluate source text in the user module.
    pub(crate) fn eval_source_raw(&mut self, source: &str) -> Result<Value, Error> {
        self.eval_in_raw(USER_MODULE, "<input>", source)
    }

    /// Run a file as the main module (requires resolve relative to it).
    pub(crate) fn eval_file_raw(&mut self, path: &Path) -> Result<Value, Error> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.modules[USER_MODULE as usize].path = Some(canonical.clone());
        self.module_paths.insert(canonical, USER_MODULE);
        self.eval_in_raw(USER_MODULE, &path.to_string_lossy(), &text)
    }

    /// Evaluate source text in `*module` for a REPL or session, then set
    /// `*module` to the module current at the end (`in-module` switches it).
    pub(crate) fn eval_interactive_raw(&mut self, module: &mut u32, name: &str, source: &str) -> Result<Value, Error> {
        let saved = std::mem::replace(&mut self.current_module, *module);
        let result = self.eval_forms(*module, name, source);
        *module = std::mem::replace(&mut self.current_module, saved);
        result
    }

    /// Evaluate source text in `module`, the file `name`.
    pub(crate) fn eval_in_raw(&mut self, module: u32, name: &str, source: &str) -> Result<Value, Error> {
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
        let meter = meter::Meter::new(self);
        let forms = reader::read_metered(source, false, &meter).map_err(|e| match e.pos {
            Some(pos) => {
                let (line, col) = reader::line_col(source, pos);
                Error::new(format!("{name}:{line}:{col}: {}", e.message))
            }
            None => Error::new(format!("{name}: {}", e.message)),
        });
        let forms = forms.map_err(|e| meter.error(self).unwrap_or(e))?;
        // Definitions anywhere in the file shadow imports/root for the whole file.
        for f in &forms {
            predeclare(self, module, f);
        }
        let mut last = Value::VOID;
        for form in &forms {
            // Codes go only in a full collection, which forms that allocate
            // nothing never come to (`code_trigger`): between forms, which
            // may collect as they run, nothing is held but what is rooted.
            if self.codes_due {
                self.collect_raw();
            }
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
    pub(crate) fn eval_sexp_raw(&mut self, form: &Sexp) -> Result<Value, Error> {
        self.eval_sexp_in_raw(self.current_module, form)
    }

    /// Register source text (for locations in errors); its file index. It
    /// is kept until the evaluation adding it ends (`pinning`), then as long
    /// as codes compiled from it live.
    pub(crate) fn add_file(&mut self, name: &str, text: &str) -> u32 {
        self.release_unheld_files();
        let source = SourceFile::new(name, text);
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
        if self.file_codes[file as usize] == 0 && !self.pinned_files.contains(&file) && !self.held_files.contains_key(&file) {
            self.files[file as usize] = SourceFile::new("", "");
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
    pub(crate) fn eval_sexp_in_raw(&mut self, module: u32, form: &Sexp) -> Result<Value, Error> {
        self.as_execution(|vm| vm.eval_sexp_in_running(module, form))
    }

    fn eval_sexp_in_running(&mut self, module: u32, form: &Sexp) -> Result<Value, Error> {
        self.pinning(|vm| {
            let file = vm.add_file("<eval>", "");
            vm.eval_form(module, file, form)
        })
    }

    /// Read source text to evaluate in `module` a form at a time: from
    /// Scheme, which calls each form's code (`next_form`) itself, so that
    /// what the forms do can wait in a task, as no native calling back
    /// into Scheme can. Definitions anywhere in the text shadow imports
    /// and root names for all of it, as with `eval_in`.
    pub(crate) fn read_forms(&mut self, module: u32, name: &str, source: &str) -> Result<Forms, Error> {
        // Held until the forms are dropped, not only while this runs.
        let file = self.pinning(|vm| {
            let file = vm.add_file(name, source);
            *vm.held_files.entry(file).or_default() += 1;
            file
        });
        *self.held_modules.entry(module).or_default() += 1;
        let hold = Hold { file, module, unheld: self.unheld_files.clone() };
        let forms = Forms { module, file, forms: RefCell::new(VecDeque::new()), hold: RefCell::new(Some(hold)) };
        let meter = meter::Meter::new(self);
        let read = reader::read_metered(source, false, &meter).map_err(|e| match e.pos {
            Some(pos) => {
                let (line, col) = reader::line_col(source, pos);
                Error::new(format!("{name}:{line}:{col}: {}", e.message))
            }
            None => Error::new(format!("{name}: {}", e.message)),
        });
        let read = read.map_err(|e| meter.error(self).unwrap_or(e))?;
        for f in &read {
            predeclare(self, module, f);
        }
        *forms.forms.borrow_mut() = read.into();
        Ok(forms)
    }

    /// Make `module` current, saving the one current before for
    /// `leave_module`: the very module, not its name, which may name
    /// another generation by then (`eval`, `eval-source`).
    pub(crate) fn enter_module(&mut self, module: u32) {
        self.saved_modules.push(self.current_module);
        self.current_module = module;
    }

    /// Make the module `enter_module` saved last current again.
    pub(crate) fn leave_module(&mut self) {
        if let Some(m) = self.saved_modules.pop() {
            self.current_module = m;
        }
    }

    /// Clear the registers from `from`, the end of the arguments of a
    /// native running, up to `stack_top`. A native is called from the top
    /// frame, its arguments above every register live there, so these are
    /// dead; but `stack_top` only grows in a dispatch level, and the
    /// collector would keep what they last held. Before a form is
    /// evaluated (`%next-form`, `%compile-expression`), that is what the
    /// forms evaluated before made.
    pub(crate) fn clear_dead_registers(&mut self, from: usize) {
        let top = self.stack_top.min(self.regs.len());
        if from < top {
            self.regs[from..top].fill(Value::VOID);
        }
    }

    /// Compile the next of `forms` into a procedure of no arguments that
    /// runs it, or `None` after the last. Compiled only now, after the
    /// forms before it ran: they may define the macros it uses.
    pub(crate) fn next_form(&mut self, forms: &Forms) -> Result<Option<Value>, Error> {
        let Some(form) = forms.forms.borrow_mut().pop_front() else {
            self.finish_forms(forms);
            return Ok(None);
        };
        self.compile_thunk(forms.module, forms.file, &form).map(Some)
    }

    /// `form`, compiled in `module`, as a procedure of no arguments.
    pub(crate) fn compile_thunk(&mut self, module: u32, file: u32, form: &Sexp) -> Result<Value, Error> {
        self.as_execution(|vm| {
            vm.pinning(|vm| {
                let saved = std::mem::replace(&mut vm.current_module, module);
                let code = Compiler::new(vm, module, file).compile_toplevel(form);
                vm.current_module = saved;
                let code = code?;
                // A closure of a top-level code, which captures nothing; it
                // keeps its code, and the code its file.
                let p = vm.alloc(2);
                unsafe {
                    *p = header(Kind::Closure, 1, 0);
                    set_field(p, 0, Value::int_unchecked(code as i64));
                }
                Ok(Value::ptr(p))
            })
        })
    }

    /// `form`, to evaluate in `module` (`eval`), as a procedure of no
    /// arguments.
    pub(crate) fn compile_expression(&mut self, module: u32, form: &Sexp) -> Result<Value, Error> {
        predeclare(self, module, form);
        // The file is pinned until its code, which keeps it, is made.
        self.pinning(|vm| {
            let file = vm.add_file("<eval>", "");
            vm.compile_thunk(module, file, form)
        })
    }

    /// Free the files `Forms` dropped since this last ran, if nothing else
    /// keeps them, and let their modules go.
    fn release_unheld_files(&mut self) {
        fn let_go(held: &mut FxHashMap<u32, u32>, k: u32) {
            if let Some(n) = held.get_mut(&k) {
                *n -= 1;
                if *n == 0 {
                    held.remove(&k);
                }
            }
        }
        let unheld = std::mem::take(&mut *self.unheld_files.borrow_mut());
        for (file, module) in unheld {
            let_go(&mut self.held_files, file);
            let_go(&mut self.held_modules, module);
            self.release_file(file);
        }
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
    pub(crate) fn call_raw(&mut self, f: Value, args: &[Value]) -> Result<Value, Error> {
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
        let result = self.call_at(base, args.len(), false).map(|exit| match exit {
            Exit::Done(v) => v,
            Exit::Suspend(_) => unreachable!("host dispatch suspended"),
        });
        self.stack_top = saved_top;
        match result {
            Err(e) if e.wait.is_some() => Err(Error::new(CANNOT_SUSPEND)),
            other => other,
        }
    }

    /// Swap the running stack with a task's; the outer stack stays visible to GC.
    pub(crate) fn swap_task_stack(&mut self, id: usize) {
        let other = &mut self.tasks[id].stack;
        std::mem::swap(&mut self.regs, &mut other.regs);
        std::mem::swap(&mut self.frames, &mut other.frames);
        std::mem::swap(&mut self.handlers, &mut other.handlers);
        std::mem::swap(&mut self.stack_top, &mut other.stack_top);
        std::mem::swap(&mut self.locals, &mut other.locals);
        std::mem::swap(&mut self.current_module, &mut other.module);
        std::mem::swap(&mut self.saved_modules, &mut other.saved_modules);
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
                && self.poisoned().is_none()
            {
                // An error in an after-thunk does not replace the one unwinding.
                let _ = self.call_raw(after.get(), &[]);
            }
        }
    }

    /// Start a task's entry procedure on the (already swapped-in) task stack.
    pub(crate) fn start_task(&mut self, f: Value, args: &[Root]) -> Result<Exit, Error> {
        // An applicable record is resolved as any call's is (`call_at`), to
        // the same depth: a closure it names runs where the task can
        // suspend and be preempted.
        let n = args.len();
        self.ensure_regs(8 + n)?;
        self.regs[0] = f;
        for (i, a) in args.iter().enumerate() {
            self.regs[1 + i] = a.get();
        }
        self.stack_top = 1 + n;
        self.call_at(0, n, true)
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
                        self.stack_top = fr.window(self.stack_top);
                        code = fr.code;
                        pc = fr.pc as usize;
                        bp = fr.bp as usize;
                    }
                }
            }
            Some(Ok(v)) => self.regs[s.slot] = v,
            Some(Err(mut e)) => {
                e.trace.push(self.location(code, pc.saturating_sub(1)));
                (code, pc, bp) = unsafe { self.handle_error(e, code, pc, bp, 1)? };
            }
        }
        unsafe { self.dispatch_loop(code, pc, bp, 0, true) }
    }

    /// A native's result can hand its call to another procedure. The VM
    /// replaces the native call only after Rust returns; no nested dispatch
    /// or native stack frame survives while the procedure runs or waits.
    pub(crate) fn native_tail_call(&mut self, procedure: Value, args: &[Value]) -> Error {
        let call = NativeTailCall { procedure: self.root(procedure), args: args.iter().map(|&v| self.root(v)).collect() };
        let mut result = Error::new("");
        result.tail_call = Some(call);
        result
    }

    /// Publish rooted operands only after any collecting register growth.
    fn install_native_tail_call(&mut self, base: usize, call: NativeTailCall) -> Result<usize, Error> {
        let n = call.args.len();
        self.ensure_regs(base + n + 2)?;
        self.regs[base] = call.procedure.get();
        for (i, arg) in call.args.iter().enumerate() {
            self.regs[base + 1 + i] = arg.get();
        }
        self.stack_top = self.stack_top.max(base + n + 1);
        Ok(n)
    }

    /// Resolve apply, applicable records and native tail delegation without
    /// nesting Rust calls. A closure is entered by the caller's dispatcher.
    #[inline(always)]
    fn resolve_call(&mut self, base: usize, mut n: usize) -> Result<NativeCall, Error> {
        let mut records = 0;
        loop {
            let f = self.regs[base];
            if is_kind(f, Kind::Closure) {
                return Ok(NativeCall::Closure(f, n));
            } else if f == self.apply_native {
                n = self.spread_apply(base, n)?;
                records = 0;
            } else if let Some(p) = Vm::applicable_proc(f) {
                // Match is_procedure_raw's eight-link bound. A cyclic
                // record must raise, not spin outside the dispatcher.
                if records == 8 {
                    return Err(Error::violation(format!("not a procedure: {}", crate::builtins::brief(f))));
                }
                records += 1;
                self.regs[base] = p;
            } else if f.is_native() {
                match self.call_native_running(f.as_native(), base + 1, n) {
                    Err(mut e) if e.tail_call.is_some() => {
                        n = self.install_native_tail_call(base, e.tail_call.take().unwrap())?;
                        records = 0;
                    }
                    result => return result.map(NativeCall::Returned),
                }
            } else if let Some(e) = self.continuation_escape(f, base, n) {
                return Err(e);
            } else {
                return Err(Error::violation(format!("not a procedure: {}", crate::builtins::brief(f))));
            }
        }
    }

    /// Call the procedure at `regs[base]` with `n` arguments after it.
    fn call_at(&mut self, base: usize, n: usize, suspendable: bool) -> Result<Exit, Error> {
        match self.resolve_call(base, n)? {
            NativeCall::Returned(value) => Ok(Exit::Done(value)),
            NativeCall::Closure(f, n) => unsafe {
                let callee = self.closure_code(f);
                let bp = base + 1;
                self.ensure_regs(bp + (*callee).frame_size as usize + n)?;
                self.enter(callee, bp, n)?;
                if suspendable { self.dispatch_loop(callee, 0, bp, 0, true) } else { self.dispatch(callee, bp).map(Exit::Done) }
            },
        }
    }

    /// `(apply f a ... list)` at `regs[base]`: rewrite in place to `(f a ... items...)`.
    fn spread_apply(&mut self, base: usize, n: usize) -> Result<usize, Error> {
        if n < 2 {
            return Err(Error::violation(format!("apply: expected a procedure and a list, got {} arguments", n)));
        }
        let list = self.regs[base + n];
        let items: Vec<Value> =
            crate::builtins::list_values(list).ok_or_else(|| Error::violation("apply: last argument must be a list"))?;
        let fixed = n - 1;
        let total = fixed - 1 + items.len();
        self.ensure_regs(base + 2 + total)?;
        self.regs.copy_within(base + 1..base + n, base);
        self.regs[base + fixed..base + fixed + items.len()].copy_from_slice(&items);
        self.stack_top = self.stack_top.max(base + 1 + total);
        Ok(total)
    }

    /// The procedure to run when record `f` is called, if its type is applicable.
    #[inline]
    /// Whether `v` can be called: natives, closures, continuations and
    /// applicable records.
    pub(crate) fn is_procedure_raw(&self, v: Value) -> bool {
        v.is_native()
            || is_kind(v, Kind::Closure)
            || Vm::applicable_proc(v).is_some()
            || (is_kind(v, Kind::Record) && unsafe { field(v.as_ptr(), 0) } == self.special(SpecialObj::ContinuationRtd))
    }

    pub(crate) fn applicable_proc(f: Value) -> Option<Value> {
        if !is_kind(f, Kind::Record) {
            return None;
        }
        // The type's field 3 is the index of the field holding the
        // procedure, if any: both checked, the record may be stale.
        unsafe {
            let p = f.as_ptr();
            let rtd = field(p, 0);
            if !is_kind(rtd, Kind::Rtd) || len_of(rtd.as_ptr()) <= 3 {
                return None;
            }
            let index = field(rtd.as_ptr(), 3);
            let i = usize::try_from(index.is_int().then(|| index.as_int())?).ok()?;
            (1 + i < len_of(p)).then(|| field(p, 1 + i))
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
        let (line, col) = file.line_col(pos);
        format!("{} ({}:{line}:{col})", code.name, file.name)
    }

    /// Find a handler for error `e` raised in the dispatch level whose base
    /// frame is at `base_bp`, innermost first. Handler procedures run right
    /// here at the raise point, whatever level installed them (they do not
    /// unwind). A `guard` of this level is returned to unwind to; a `guard` of
    /// an outer level ends the search, and the error propagates there (through
    /// natives such as `dynamic-wind`).
    fn catch(&mut self, mut e: Error, base_bp: usize) -> Result<Caught, Error> {
        // Killed meanwhile (in an after thunk, say): no handler sees it.
        if !e.reaches_host() && self.killing() {
            e = Error::new(KILLED).with_kind(ErrorKind::Killed);
        }
        // A handler raising the condition it was called for again: the
        // error keeps the trace of where it was first raised, and the
        // frames below that raise are those it was raised in (not the
        // handlers' above them).
        let mut raised_in = None;
        if e.escape.is_none()
            && let (Some(p), Some(raised)) = (&e.payload, self.innermost_raise())
            && p.get() == raised.payload.get()
        {
            e.trace = raised.origin.clone();
            raised_in = Some(raised.frames_len);
        }
        let searched = e.searched.filter(|&(w, _)| w == self.world).map(|(_, i)| i as usize);
        let mut idx = searched.unwrap_or(usize::MAX).min(self.handlers.len());
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
                        // Poisoned (a native panicked) in an after-thunk
                        // unwinding to here: no guest landing at all.
                        if self.poisoned().is_some() {
                            return Err(self.poison_error());
                        }
                        if self.killing() {
                            return Err(Error::new(KILLED).with_kind(ErrorKind::Killed));
                        }
                        return Ok(Caught::Land(Landing { frames_len, code, bp, target, dst }, value.get()));
                    }
                }
                Handler::Guard { frames_len, code, bp, target, dst } if e.escape.is_none() && !e.reaches_host() => {
                    let condition = self.condition_of(&mut e);
                    self.unwind_to(idx - 1);
                    // Poisoned in an after thunk unwinding to here: the
                    // poison replaces the error being handled.
                    if self.poisoned().is_some() {
                        return Err(self.poison_error());
                    }
                    // Killed in an after thunk unwinding to here.
                    if self.killing() {
                        return Err(Error::new(KILLED).with_kind(ErrorKind::Killed));
                    }
                    return Ok(Caught::Land(Landing { frames_len, code, bp, target, dst }, condition.get()));
                }
                Handler::Proc { handler } if e.escape.is_none() && !e.reaches_host() => {
                    // Run at the raise point; raises inside go to outer handlers.
                    let condition = self.condition_of(&mut e);
                    let frames_len = raised_in.unwrap_or(self.frames.len()).min(self.frames.len());
                    let trace: Vec<String> = e
                        .trace
                        .iter()
                        .cloned()
                        .chain(self.frames[..frames_len].iter().rev().take(32).map(|f| self.location(f.code, f.pc as usize - 1)))
                        .collect();
                    let raised = Rc::new(Raised { payload: condition.clone(), origin: e.trace.clone(), trace, frames_len });
                    // The raising frame calls it, so in a task it can wait.
                    if is_kind(self.special(SpecialObj::HandleRaise), Kind::Closure) {
                        return Ok(Caught::Handle(HandlerCall { handler, index: idx - 1, raised }));
                    }
                    // Without the prelude: called from here, it cannot wait.
                    let result = self.call_masked(idx - 1, handler.get(), condition.get(), Some(raised));
                    e = match result {
                        // A handler returning from a non-continuable raise is itself an error.
                        Ok(_) => Error::new(format!("exception handler returned from non-continuable raise: {}", e.msg)),
                        // Re-raising the same condition keeps the original trace.
                        Err(inner) if inner.escape.is_none() && inner.payload.as_ref().map(|p| p.get()) == Some(condition.get()) => e,
                        Err(inner) => inner,
                    };
                    // Poisoned while the handler ran: no outer handler
                    // or guard sees anything more.
                    if self.poisoned().is_some() {
                        return Err(self.poison_error());
                    }
                    if e.escape.is_some() {
                        // The handler escaped (e.g. invoked a restart): its target
                        // may be anywhere on the stack, including above the handler.
                        idx = self.handlers.len();
                        continue;
                    }
                }
                Handler::Guard { .. } | Handler::Proc { .. } | Handler::Mask { .. } => {}
            }
            idx -= 1;
        }
        e.searched = Some((self.world, idx as u32));
        Err(e)
    }

    /// Unwind to a landing for `e`, raised in the frame `code` at `pc`
    /// (after the raising instruction) with base `bp`, or call a handler
    /// procedure there: where the dispatch level continues (code, pc,
    /// base), or the error leaving it.
    unsafe fn handle_error(
        &mut self,
        e: Error,
        code: *const Code,
        pc: usize,
        bp: usize,
        base_bp: usize,
    ) -> Result<(*const Code, usize, usize), Error> {
        let mut caught = self.catch(e, base_bp);
        loop {
            match caught? {
                Caught::Land(Landing { frames_len, code, bp, target, dst }, condition) => {
                    // The frames escaped from are dead: back to the window of
                    // the call the landing frame made.
                    if let Some(fr) = self.frames.get(frames_len) {
                        self.stack_top = fr.window(self.stack_top);
                    }
                    self.frames.truncate(frames_len);
                    self.regs[bp + dst as usize] = condition;
                    return Ok((code, target as usize, bp));
                }
                Caught::Handle(call) => {
                    let index = call.index;
                    match unsafe { self.call_handler(code, pc, bp, call) } {
                        Ok((callee, new_bp)) => return Ok((callee, 0, new_bp)),
                        // No room to call it (the stack is full): what
                        // stopped it goes to the handlers outside it.
                        Err(mut e) => {
                            e.searched = Some((self.world, index as u32));
                            caught = self.catch(e, base_bp);
                        }
                    }
                }
            }
        }
    }

    /// Make the frame `code` at `pc` with base `bp`, where a condition was
    /// raised, call `(%handle-raise handler condition index)`: an ordinary
    /// call on the running stack, so a handler in a task can wait, and a
    /// handler that escapes or raises unwinds as any frame does. The
    /// handler entries from the handler's own up are hidden meanwhile (a
    /// `Handler::Mask`, which `%handle-raise` pops when the handler
    /// returns). Returns the code and base of the frame entered.
    unsafe fn call_handler(&mut self, code: *const Code, pc: usize, bp: usize, call: HandlerCall) -> Result<(*const Code, usize), Error> {
        let f = self.special(SpecialObj::HandleRaise);
        unsafe {
            let callee = self.closure_code(f);
            // Right above the raising frame: what was above it (the
            // registers of a call that failed) is dead. Not above
            // `stack_top`, which only grows: raising and escaping in a
            // loop would climb the stack.
            let base = bp + (*code).frame_size as usize;
            let new_bp = base + 1;
            let top = self.stack_top.max(base) as u32;
            self.ensure_regs(new_bp + ((*callee).frame_size as usize).max(3) + 1)?;
            self.regs[base] = f;
            self.regs[new_bp] = call.handler.get();
            self.regs[new_bp + 1] = call.raised.payload.get();
            self.regs[new_bp + 2] = Value::int_unchecked(call.index as i64);
            self.stack_top = self.stack_top.max(new_bp + 3);
            self.enter(callee, new_bp, 3)?;
            self.stack_top = self.stack_top.max(new_bp + (*callee).frame_size as usize);
            self.handlers.push(Handler::Mask { lo: call.index, raised: Some(call.raised) });
            self.frames.push(Frame { code, pc: pc as u32, bp: bp as u32, top });
            Ok((callee, new_bp))
        }
    }

    /// Whether handler entry `index` is hidden: a handler procedure for it,
    /// or for one below it, is running.
    fn masked(&self, index: usize) -> bool {
        self.handlers[index + 1..].iter().any(|h| matches!(h, Handler::Mask { lo, .. } if *lo <= index))
    }

    /// Call handler `h` for `condition` from Rust, with handler entries
    /// from `lo` up hidden. It cannot wait: `call_handler` is how the
    /// interpreter calls handlers.
    fn call_masked(&mut self, lo: usize, h: Value, condition: Value, raised: Option<Rc<Raised>>) -> Result<Value, Error> {
        let depth = self.handlers.len();
        self.handlers.push(Handler::Mask { lo, raised });
        let result = self.call_raw(h, &[condition]);
        self.handlers.truncate(depth);
        result
    }

    /// The index of the handler procedure `raise-continuable` calls: the
    /// innermost visible one, unless a `guard` is inside it.
    pub(crate) fn continuable_handler(&self) -> Option<usize> {
        (0..self.handlers.len()).rev().filter(|&i| !self.masked(i)).find_map(|i| match &self.handlers[i] {
            Handler::Proc { .. } => Some(Some(i)),
            Handler::Guard { .. } => Some(None),
            _ => None,
        })?
    }

    /// The handler procedure at `index` of the handler stack.
    pub(crate) fn handler_at(&self, index: usize) -> Option<Value> {
        match self.handlers.get(index) {
            Some(Handler::Proc { handler }) => Some(handler.get()),
            _ => None,
        }
    }

    /// Hide handler entries from `lo`, a handler procedure's, up while it
    /// runs (`raise-continuable`); `pop_handler` ends it.
    pub(crate) fn push_mask(&mut self, lo: usize) -> Result<(), Error> {
        if self.handler_at(lo).is_none() {
            return Err(Error::new("%push-mask: no handler procedure there"));
        }
        self.handlers.push(Handler::Mask { lo, raised: None });
        Ok(())
    }

    /// Make `f` the procedure the raising frame calls for a handler
    /// (`call_handler`): the prelude's `%handle-raise`, once. It must not
    /// return: the raising frame cannot continue. So it is set once, by
    /// the prelude, and nothing can replace it.
    pub(crate) fn set_raise_handler(&mut self, f: Value) -> Result<(), Error> {
        if is_kind(self.special(SpecialObj::HandleRaise), Kind::Closure) {
            return Err(Error::new("%set-raise-handler!: already set"));
        }
        let fits = is_kind(f, Kind::Closure)
            && unsafe {
                let c = &*self.closure_code(f);
                !c.rest && c.nparams == 3
            };
        if !fits {
            return Err(Error::new("%set-raise-handler!: expected a procedure of 3 arguments"));
        }
        self.specials[SpecialObj::HandleRaise as usize] = f;
        Ok(())
    }

    /// The error when the handler at `index` returned from a raise of
    /// `condition` that cannot continue: for the handlers outside it.
    pub(crate) fn handler_returned(&mut self, condition: Value, index: usize) -> Error {
        let msg = crate::builtins::condition_message(self, condition);
        let mut e = Error::new(format!("exception handler returned from non-continuable raise: {msg}"));
        e.searched = Some((self.world, index.min(self.handlers.len()) as u32));
        e
    }

    /// The raised object of `e`, making an error object for VM/Rust errors.
    fn condition_of(&mut self, e: &mut Error) -> Root {
        if let Some(p) = &e.payload {
            return p.clone();
        }
        let msg = e.msg.clone();
        let obj = self.make_error_object_of_raw(&msg, &[], e.kind);
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
            // Poisoned in an after-thunk on the way out: that replaces
            // the error leaving this level.
            if self.poisoned().is_some() {
                return Err(self.poison_error());
            }
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

    // Collecting operations refresh `r`, even if its address did not move:
    // the GC's mutable root access can invalidate its borrow permissions.
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
            // No register pointer or destination place survives this call.
            // Operands needed after collection are reread from their slots.
            macro_rules! collecting {
                ($e:expr) => {{
                    sync_top!();
                    let result = $e;
                    r = self.regs.as_mut_ptr().add(bp);
                    result
                }};
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
                    self.stack_top = fr.window(self.stack_top);
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
            // Separate stores keep the refreshed register base off the fast paths.
            macro_rules! arith {
                ($dst:expr, $x:expr, $y:expr, $iop:ident, $fop:tt, $slow:path) => {{
                    let (x, y) = ($x, $y);
                    if Value::both_int(x, y)
                        && let Some(v) = x.as_int().$iop(y.as_int()).and_then(Value::fixnum)
                    {
                        reg!($dst) = v;
                    } else if x.is_float() && y.is_float() {
                        reg!($dst) = Value::float(x.as_float() $fop y.as_float());
                    } else {
                        let v = tryv!(collecting!($slow(self, x, y)));
                        reg!($dst) = v;
                    }
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
                    // The callee may collect: this frame's registers are
                    // roots meanwhile, the dead ones above its call too.
                    // Left out, they would go stale, and the window would
                    // take them in again when it grows over them.
                    sync_top!();
                    let f: Value = $f;
                    let (base, n) = ($base as usize, $n as usize);
                    let callee = self.closure_code(f);
                    let hot = &(*callee).jit.hot;
                    let calls = hot.get().wrapping_add(1);
                    hot.set(calls);
                    // Root the closure before a safepoint, then use the
                    // updated slot when sliding a tail call's arguments.
                    *r.add(base) = f;
                    if calls & 255 == 0 || calls == self.jit_threshold {
                        // What the arguments hold, for the JIT to assume.
                        if let Err(e) = collecting!(self.safepoint(callee, calls, Some((0, bp + base + 1)))) {
                            fail!(e);
                        }
                    }
                    let new_bp = if $tail { bp } else { bp + base + 1 };
                    let size = (*callee).frame_size as usize;
                    let exact = !(*callee).rest && n == (*callee).nparams as usize;
                    // Admit all growth before a tail slide can overwrite a
                    // handler's live registers, including enter's extra slot.
                    if let Err(e) = self.ensure_regs(new_bp + size.max(n) + usize::from(!exact)) {
                        fail!(e);
                    }
                    r = self.regs.as_mut_ptr().add(bp);
                    // The callee finds its closure at `bp - 1`.
                    if $tail {
                        std::ptr::copy(r.add(base), r.sub(1), n + 1);
                    } else {
                        self.frames.push(Frame { code, pc: pc as u32, bp: bp as u32, top: self.stack_top as u32 });
                    }
                    if exact {
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
                            self.frames.push(Frame { code, pc: pc as u32, bp: bp as u32, top: self.stack_top as u32 });
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
                    let (f, base, n): (Value, usize, usize) = ($f, $base as usize, $n as usize);
                    reg!(base) = f;
                    sync_top!();
                    self.stack_top = self.stack_top.max(bp + base + 1 + n);
                    let result = self.resolve_call(bp + base, n);
                    r = self.regs.as_mut_ptr().add(bp);
                    match result {
                        Ok(NativeCall::Closure(f, n)) => call_closure!(f, base, n, $tail),
                        Ok(NativeCall::Returned(v)) => {
                            if $tail {
                                ret!(v);
                            } else {
                                reg!(base) = v;
                            }
                        }
                        Err(mut e) if e.wait.is_some() => {
                            if suspendable {
                                sync_top!();
                                let wait = e.wait.take();
                                self.spare_suspend = Some(e);
                                return Ok(Exit::Suspend(Suspend { code, pc, bp, slot: bp + base, tail: $tail, wait }));
                            }
                            fail!(Error::new(CANNOT_SUSPEND))
                        }
                        Err(e) => fail!(e),
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
                                fail!(Error::violation(format!("unbound variable: {}", self.global_name(g))));
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
                            let p = collecting!(self.alloc(2));
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
                            let target: *const Code = *self.code_ptrs.get_unchecked(c as usize);
                            let n = {
                                let code: &Code = &*target;
                                code.captures.len()
                            };
                            let p = collecting!(self.alloc(2 + n));
                            let target: *const Code = *self.code_ptrs.get_unchecked(c as usize);
                            *p = header(Kind::Closure, 1 + n, 0);
                            // The code's id, not its address; the collector
                            // keeps the code for it (`Heap::shade_code`).
                            set_field(p, 0, Value::int_unchecked(c as i64));
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
                            let v = collecting!(self.make_record(rtd, &[Value::int_unchecked(id)]));
                            reg!(k) = v;
                            self.handlers.push(Handler::Escape { id, frames_len: self.frames.len(), code, bp, target: t, dst });
                        }

                        Op::Jmp { t } => pc = t as usize,
                        Op::Loop { t } => {
                            pc = t as usize;
                            let hot = &(*code).jit.hot;
                            let n = hot.get().wrapping_add(1);
                            hot.set(n);
                            if n & 255 == 0 || n == self.jit_threshold {
                                // What the loop head holds now, for the JIT to assume.
                                if let Err(e) = collecting!(self.safepoint(code, n, Some((pc, bp)))) {
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
                                code_table: self.code_ptrs.as_ptr(),
                                regs_end: self.regs.as_mut_ptr().add(self.regs.len()),
                                fuel: fuel_ptr,
                                stack_top: std::ptr::addr_of_mut!((*vm).stack_top),
                                heap_top,
                                heap_end,
                                code,
                                bp: bp as u64,
                                tail: 0,
                                res: 0,
                                moves: self.moves,
                                cage: self.heap.cage_base(),
                            };
                            let frame = self.regs.as_mut_ptr().add(bp);
                            let res = {
                                let _fuel = JitFuel::set(std::ptr::addr_of_mut!((*vm).jit_fuel), fuel_ptr);
                                let mut res = f(vm, &mut ctx, frame, bp as u64, (pc - 1) as u32, 0);
                                // Trampoline for tail calls between compiled functions.
                                while (res >> 32) as u32 == crate::jit::TAILCALL {
                                    let next: crate::jit::JitFn = std::mem::transmute(std::ptr::with_exposed_provenance::<()>(ctx.tail));
                                    let frame = self.regs.as_mut_ptr().add(bp);
                                    res = next(vm, &mut ctx, frame, bp as u64, 0, 0);
                                }
                                res
                            };
                            if self.poisoned().is_some() {
                                fail!(self.poison_error());
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
                            self.take_jit_frames();
                            code = ctx.code;
                            ops = (*code).ops.as_ptr();
                            bp = ctx.bp as usize;
                            // A native called from JIT code may have grown the register stack.
                            r = self.regs.as_mut_ptr().add(bp);
                            match status {
                                crate::jit::EXIT | crate::jit::RESUME => {}
                                crate::jit::INTERPRET => {
                                    if std::env::var_os("TECHNE_JIT_LOG").is_some() {
                                        eprintln!("jit: {} interprets a frame from pc {pc}", (*code).name);
                                    }
                                    ops = (*code).jit.ops.get().unwrap().as_ptr();
                                }
                                crate::jit::ERROR => {
                                    let mut e = self.jit_error.take().expect("JIT error");
                                    if let Some(call) = e.tail_call.take() {
                                        let op = (*code).jit.ops.get().unwrap()[pc - 1];
                                        let (Op::Call { base, .. }
                                        | Op::CallG { base, .. }
                                        | Op::TailCall { base, .. }
                                        | Op::TailCallG { base, .. }) = op
                                        else {
                                            unreachable!()
                                        };
                                        let tail = matches!(op, Op::TailCall { .. } | Op::TailCallG { .. });
                                        let n = tryv!(collecting!(self.install_native_tail_call(bp + base as usize, call)));
                                        let f = reg!(base);
                                        call_other!(f, base, n, tail);
                                    } else {
                                        fail!(e)
                                    }
                                }
                                crate::jit::STEP => {
                                    // A case native code leaves to Rust: run the
                                    // original instruction, then continue after it.
                                    let op = (*code).jit.ops.get().unwrap()[pc];
                                    match collecting!(self.jit_slow_op(bp, op)) {
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
                                    if let Err(e) = collecting!(self.poll_interrupt()) {
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
                                    self.spare_suspend = Some(e);
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
                            let v = if x.is_int()
                                && let Some(v) = Value::fixnum(x.as_int() + i as i64)
                            {
                                v
                            } else {
                                tryv!(collecting!(num::add(self, x, Value::int_unchecked(i as i64))))
                            };
                            reg!(dst) = v;
                        }
                        Op::Sub { dst, a, b } => arith!(dst, reg!(a), reg!(b), checked_sub, -, num::sub),
                        Op::Mul { dst, a, b } => arith!(dst, reg!(a), reg!(b), checked_mul, *, num::mul),
                        Op::Quo { dst, a, b } => {
                            let v = tryv!(collecting!(num::quotient(self, reg!(a), reg!(b))));
                            reg!(dst) = v;
                        }
                        Op::Rem { dst, a, b } => {
                            let v = tryv!(collecting!(num::remainder(self, reg!(a), reg!(b))));
                            reg!(dst) = v;
                        }
                        Op::Mod { dst, a, b } => {
                            let (x, y) = (reg!(a), reg!(b));
                            let v = if Value::both_int(x, y) && y.as_int() != 0 {
                                Value::int_unchecked(num::modulo_i64(x.as_int(), y.as_int()))
                            } else {
                                tryv!(collecting!(num::modulo(self, x, y)))
                            };
                            reg!(dst) = v;
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
                            let p = collecting!(self.alloc(3));
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
                                fail!(Error::violation(format!("unbound variable: {}", self.global_name(g))));
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
                match self.handle_error(e, code, pc, bp, base_bp) {
                    Ok((c, p, b)) => {
                        code = c;
                        ops = (*code).ops.as_ptr();
                        pc = p;
                        bp = b;
                        r = self.regs.as_mut_ptr().add(bp);
                    }
                    Err(mut e) => {
                        if e.escape.is_none() {
                            // Raised again by the handler called for it: the
                            // frames below its first raise, not the handler's.
                            let end = match (&e.payload, self.innermost_raise()) {
                                (Some(p), Some(raised)) if p.get() == raised.payload.get() => raised.frames_len,
                                _ => self.frames.len(),
                            }
                            .min(self.frames.len());
                            let frames = &self.frames[base_frames.min(end)..end];
                            e.trace.extend(frames.iter().rev().take(16).map(|f| self.location(f.code, f.pc as usize - 1)));
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
            return Err(Error::violation("stack overflow: recursion too deep"));
        }
        let len = (self.regs.len() * 2).min(MAX_REGS).max(needed);
        self.admit_without_collecting((len - self.regs.len()) * std::mem::size_of::<Value>())?;
        self.regs.try_resize(len, Value::VOID, &mut self.moves).map_err(|_| Error::new("out of memory: cannot grow registers"))
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

    /// Grow the register stack to `needed` registers for compiled `code`
    /// (`jit::Gen::room`). If it cannot grow (too deep a recursion, no
    /// memory), the interpreter runs `code` from now on, as without
    /// native code: then it raises an error only if it runs out of room
    /// itself.
    #[cold]
    pub(crate) fn jit_make_room(&mut self, code: *mut Code, needed: usize) -> bool {
        if self.ensure_regs(needed).is_ok() {
            return true;
        }
        // Its original instructions at every entry point.
        let c = unsafe { &mut *code };
        c.ops.make_mut().copy_from_slice(c.jit.ops.get().expect("compiled"));
        c.jit.call_entry.set(None);
        if std::env::var_os("TECHNE_JIT_LOG").is_some() {
            eprintln!("jit: {} interpreted from now on (no room for its frame)", c.name);
        }
        false
    }

    /// A frame of compiled code that handed over to the interpreter, its
    /// window set as the interpreter takes it (`take_jit_frames`).
    pub(crate) fn jit_push_frame(&mut self, code: *const Code, pc: u32, bp: u32) {
        self.jit_unwind.push(Frame { code, pc, bp, top: 0 });
    }

    /// Make the frames of compiled code that handed over to the
    /// interpreter its own, outermost first, each with the window of its
    /// call: the window of the frame below, and its own registers.
    fn take_jit_frames(&mut self) {
        let mut below = self.frames.last().map_or(0, |f| f.top as usize);
        let taken: Vec<Frame> = self.jit_unwind.drain(..).rev().collect();
        self.frames.extend(taken.into_iter().map(|f| {
            below = below.max(f.bp as usize + unsafe { (*f.code).frame_size } as usize);
            Frame { top: below as u32, ..f }
        }));
    }

    /// Queue a hot function for compilation (or compile it now in sync mode).
    #[cold]
    unsafe fn jit_compile(&mut self, code: *const Code, seen: Option<(usize, usize)>) {
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
        let orig = c.jit.ops.get_or_init(|| c.ops.iter().copied().collect());
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
        // Globals called that hold plain Rust natives now: calls of the same
        // native call it directly.
        let known_native = |g: u32| {
            let v = self.globals[g as usize];
            let native = self.natives.get(v.as_native()).filter(|_| v.is_native() && v != self.apply_native)?;
            let NativeImpl::Plain(_, f) = native.f else { return None };
            let inline = self.jit_inline.iter().find(|(x, _)| *x == v).map(|&(_, i)| i);
            let fixnum = self.jit_fixnum.contains(&v);
            Some(crate::jit::KnownNative { value: v.bits(), f, min: native.min, max: native.max, inline, fixnum })
        };
        // Calls of small functions globals hold run inline (and the calls
        // of small functions in those, nested). Native code leaving inside
        // one makes its frame, which the interpreter then runs from its
        // original instructions. Tail calls too, the callee taking over the
        // frame, unless a `guard` landing in this function could read its
        // registers.
        let handlers = orig.iter().any(|op| matches!(op, Op::PushHandler { .. } | Op::PushEscape { .. }));
        let mut inlined = std::collections::HashMap::new();
        for (pc, op) in orig.iter().enumerate().filter(|_| jit_inlines()) {
            let (Op::CallG { n, g, .. } | Op::TailCallG { n, g, .. }) = *op else { continue };
            if matches!(op, Op::TailCallG { .. }) && handlers {
                continue;
            }
            if let Some(inlinee) = unsafe { self.jit_inlinee(g, n, 1, &mut vec![code], &known_native) } {
                inlined.insert(pc as u32, inlinee);
            }
        }
        // Globals called that hold closures now: calls of the same closure
        // get the inline path, specialised for its code. Those called by
        // functions inlined guard the calls inlined.
        fn nested_globals(i: &crate::jit::Inlinee, out: &mut Vec<u32>) {
            for (&pc, n) in &i.inlined {
                if let Op::CallG { g, .. } | Op::TailCallG { g, .. } = i.ops[pc as usize] {
                    out.push(g);
                }
                nested_globals(n, out);
            }
        }
        let mut known: Vec<u32> = orig
            .iter()
            .filter_map(|op| match *op {
                Op::CallG { g, .. } | Op::TailCallG { g, .. } if is_kind(self.globals[g as usize], Kind::Closure) => Some(g),
                _ => None,
            })
            .collect();
        inlined.values().for_each(|i| nested_globals(i, &mut known));
        known.sort_unstable();
        known.dedup();
        if c.jit.callees.get().is_none() && !known.is_empty() {
            self.jit_callers.push(c);
        }
        let callees = c.jit.callees.get_or_init(|| known.iter().map(|&g| self.globals[g as usize]).collect());
        // The codes the job takes by address: those of the closures it
        // expects, and of those the globals hold now, which it may run
        // inline.
        {
            let code_id = |v: Value| code_slot(unsafe { field(v.as_ptr(), 0) }) as u32;
            let mut deps = c.jit.deps.borrow_mut();
            deps.extend(callees.iter().map(|&v| code_id(v)));
            deps.extend(known.iter().map(|&g| self.globals[g as usize]).filter(|&v| is_kind(v, Kind::Closure)).map(code_id));
            deps.sort_unstable();
            deps.dedup();
        }
        // Not if the machine code would not fit any more.
        let inlined_ops: usize = inlined.values().map(crate::jit::Inlinee::size).sum();
        if !crate::jit::Compiler::fits(orig.len() + inlined_ops, self.room()) {
            inlined.clear();
        }
        fn all_ops<'a>(i: &'a crate::jit::Inlinee, out: &mut Vec<&'a Op>) {
            out.extend(&i.ops);
            i.inlined.values().for_each(|n| all_ops(n, out));
        }
        let mut inlined_ops = Vec::new();
        inlined.values().for_each(|i| all_ops(i, &mut inlined_ops));
        let native_globals = orig
            .iter()
            .chain(inlined_ops)
            .filter_map(|op| match *op {
                Op::CallG { g, .. } | Op::TailCallG { g, .. } => Some((g, known_native(g)?)),
                _ => None,
            })
            .collect();
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
            observed: seen.filter(|(head, _)| heads.contains(head)).map(|(head, bp)| {
                // At the start, the arguments only.
                let len = if head == 0 { c.nparams } else { c.frame_size } as usize;
                let frame = &self.regs[bp..(bp + len).min(self.regs.len())];
                let of =
                    |p: fn(Value) -> bool| frame.iter().enumerate().filter(|(_, v)| p(**v)).map(|(r, _)| r as crate::code::Reg).collect();
                crate::jit::Observed { head, fixnums: of(Value::is_int), floats: of(Value::is_float) }
            }),
            heads,
            captures,
            native_globals,
            inlined,
            closure_globals: known
                .iter()
                .zip(callees.iter())
                .map(|(&g, closure)| {
                    let callee = unsafe { &*self.closure_code(*closure) };
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

    /// What a call of the closure global `g` holds with `n` arguments runs
    /// inline, `depth` calls deep in compiled code (1: in the function
    /// compiled), if it is small enough (`jit::inlinable`), with the calls
    /// it makes of small functions, nested, up to `jit::INLINE_DEPTH` deep
    /// and `jit::INLINE_TOTAL` instructions; never a function of `outer`
    /// (those it would run in).
    unsafe fn jit_inlinee(
        &self,
        g: u32,
        n: u16,
        depth: usize,
        outer: &mut Vec<*const Code>,
        known_native: &dyn Fn(u32) -> Option<crate::jit::KnownNative>,
    ) -> Option<crate::jit::Inlinee> {
        let v = self.globals[g as usize];
        if !is_kind(v, Kind::Closure) {
            return None;
        }
        let callee = self.closure_code(v);
        if outer.contains(&callee) {
            return None;
        }
        let callee = unsafe { &*callee };
        let ops = callee.jit.ops.get_or_init(|| callee.ops.iter().copied().collect());
        if ops.len() > crate::jit::INLINE_MAX || callee.rest || callee.nparams != n {
            return None;
        }
        let mut nested = std::collections::HashMap::new();
        if depth < crate::jit::INLINE_DEPTH {
            outer.push(callee);
            for (pc, op) in ops.iter().enumerate() {
                if let Op::CallG { n, g, .. } | Op::TailCallG { n, g, .. } = *op
                    && let Some(i) = unsafe { self.jit_inlinee(g, n, depth + 1, outer, known_native) }
                {
                    nested.insert(pc as u32, i);
                }
            }
            outer.pop();
        }
        if !crate::jit::inlinable(ops, callee.nparams, callee.rest, n, known_native, &nested) {
            return None;
        }
        let inlinee = crate::jit::Inlinee {
            code: callee as *const Code as usize,
            ops: ops.to_vec(),
            consts: callee.consts.iter().map(|v| v.bits()).collect(),
            consts_addr: callee.consts.as_ptr() as usize,
            frame_size: callee.frame_size,
            inlined: nested,
        };
        (inlinee.size() <= crate::jit::INLINE_TOTAL).then_some(inlinee)
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
            // A world's own instructions from here (shared with its
            // template until now; see `Code::ops`).
            let ops = c.ops.make_mut();
            for h in done.heads {
                ops[h] = Op::EnterJit;
            }
        }
    }

    /// Every 256 calls or loop iterations of a function: deliver a pending
    /// interrupt, compile the function once it is hot, and install finished
    /// code. `seen` is a loop's head and the frame's base, or at a call, pc 0
    /// and where the callee's frame starts: the kinds of the registers
    /// there (of the arguments at a call) are what the code assumes
    /// (`Job::observed`).
    #[cold]
    unsafe fn safepoint(&mut self, code: *const Code, count: u32, seen: Option<(usize, usize)>) -> Result<(), Error> {
        self.poll_interrupt()?;
        if count >= self.jit_threshold && !unsafe { (*code).jit.failed.get() } {
            unsafe { self.jit_compile(code, seen) };
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
        if self.poisoned().is_some() {
            return Err(self.poison_error());
        }
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

    /// The world's cage and symbols entered, values decoding in it and
    /// names interned in it, until dropped.
    pub(crate) fn cage_entry(&self) -> crate::alloc::CageEntered {
        crate::alloc::enter_cage(self.heap.cage(), &self.symbols)
    }

    /// `enter_account` unless this thread charges the world already.
    pub(crate) fn charging(&self) -> Charging {
        let account = self.account.reference();
        Charging {
            _account: (!account.entered()).then(|| account.enter()),
            _cage: crate::alloc::enter_cage(self.heap.cage(), &self.symbols),
        }
    }

    /// Charge this thread's allocations to the world (and intern as it)
    /// until the guard returned is dropped: what a host does for the
    /// world (the VM enters it itself while it runs).
    pub fn enter_account(&self) -> EnteredWorld {
        EnteredWorld { _account: self.account.enter(), _cage: crate::alloc::enter_cage(self.heap.cage(), &self.symbols) }
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
            self.entered_cage = Some(crate::alloc::enter_cage(self.heap.cage(), &self.symbols));
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
        let killed = self.stops.end(id) == Some(Stop::Kill);
        if killed {
            self.locals = locals;
        }
        self.execution_ended(id, killed);
        if self.executions.is_empty() {
            self.stops.set_root(ExecId(0));
            self.entered_cage = None;
            self.entered = None;
        }
        self.stops.run(self.running_execution());
    }

    /// Run `f` as an execution of its own if the host calls it, else as
    /// part of the running one.
    fn as_execution<T>(&mut self, f: impl FnOnce(&mut Vm) -> Result<T, Error>) -> Result<T, Error> {
        if self.poisoned().is_some() {
            return Err(self.poison_error());
        }
        if !self.active.is_empty() {
            // Called back from another VM's native, perhaps.
            let _charged = self.charging();
            let result = f(self);
            // Poisoned meanwhile (and swallowed in there): the result
            // of a poisoned world is the poison error.
            if self.poisoned().is_some() {
                return Err(self.poison_error());
            }
            return result;
        }
        let id = self.new_execution();
        self.enter_execution(id);
        // A panic in the execution — a native's, called by the
        // interpreter (`call_native_running`), or the VM's own — unwinds
        // to here and retires the world: what it left half done (a task's
        // stack swapped in, frames pushed) is never run again, only
        // dropped. Compiled code's natives and helpers contain theirs at
        // their `extern "C"` boundary, which a panic cannot cross.
        let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))) {
            Ok(result) => result,
            Err(panic) => Err(self.contain_panic(panic)),
        };
        // Ending well, refused for what it grew all the same — and a
        // completion on a meanwhile-poisoned world is no completion.
        let result = match result {
            Ok(_) if self.poisoned().is_some() => Err(self.poison_error()),
            Ok(v) => self.growth_stop(id).map_or(Ok(v), Err),
            other => other,
        };
        self.leave_execution(id);
        result
    }

    /// Why this world was retired by a contained panic, if it was. A
    /// poisoned VM refuses every later execution; it may only be dropped.
    pub fn poisoned(&self) -> Option<&str> {
        self.poisoned.get().map(|why| &**why)
    }

    /// Retire the world for `why`, unless it is already (the first reason
    /// stays).
    #[cold]
    pub(crate) fn retire(&self, why: &str) {
        let _ = self.poisoned.set(why.into());
    }

    /// The error a poisoned world refuses executions with.
    #[cold]
    #[inline(never)]
    pub(crate) fn poison_error(&self) -> Error {
        Error::new(format!("world retired: {}", self.poisoned().unwrap_or("native panicked"))).with_kind(ErrorKind::Poisoned)
    }

    /// Record a contained panic (the first one wins).
    pub(crate) fn poison_panic(&self, panic: &(dyn std::any::Any + Send)) {
        if self.poisoned().is_none() {
            let msg: &str = if let Some(s) = panic.downcast_ref::<&str>() {
                s
            } else if let Some(s) = panic.downcast_ref::<String>() {
                s
            } else {
                "unknown panic"
            };
            self.retire(&format!("native panicked: {msg}"));
        }
    }

    /// Run host code on the VM, containing its panics: a panic poisons
    /// the world and becomes an uncatchable `ErrorKind::Poisoned` error
    /// instead of unwinding through the interpreter, JIT helper frames
    /// or the `extern "C"` native boundary (where it would abort the
    /// process). Poison dominates: if the code poisoned the VM through
    /// a nested execution and swallowed that error, its own result —
    /// success or ordinary error alike — is replaced by the poison
    /// error, so no guest code runs on a poisoned VM.
    #[inline(always)]
    pub(crate) fn catch_panic<T>(&mut self, f: impl FnOnce(&mut Vm) -> T) -> Result<T, Error> {
        if self.poisoned().is_some() {
            return Err(self.poison_error());
        }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))) {
            Ok(v) if self.poisoned().is_none() => Ok(v),
            Ok(_) => Err(self.poison_error()),
            Err(panic) => Err(self.contain_panic(panic)),
        }
    }

    /// The host's way into its world, which every public operation that
    /// touches the heap takes: `f` runs with the world entered
    /// (`charging`), and a panic in it, the VM's, a native's or a host
    /// callback's, retires the world instead of unwinding into the host,
    /// and is the poison error. A retired world runs nothing more.
    pub(crate) fn host<T>(&mut self, f: impl FnOnce(&mut Vm) -> T) -> Result<T, Error> {
        if self.poisoned().is_some() {
            return Err(self.poison_error());
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _entered = self.charging();
            f(self)
        }));
        match result {
            Ok(_) if self.poisoned().is_some() => Err(self.poison_error()),
            Ok(v) => Ok(v),
            Err(panic) => Err(self.contain_panic(panic)),
        }
    }

    /// `host`, for `f` that may fail itself.
    pub(crate) fn host_try<T>(&mut self, f: impl FnOnce(&mut Vm) -> Result<T, Error>) -> Result<T, Error> {
        self.host(f).and_then(|result| result)
    }

    /// `host` for operations that only look, through `&self`.
    pub(crate) fn host_ref<T>(&self, f: impl FnOnce(&Vm) -> T) -> Result<T, Error> {
        if self.poisoned().is_some() {
            return Err(self.poison_error());
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _entered = self.charging();
            f(self)
        }));
        match result {
            Ok(_) if self.poisoned().is_some() => Err(self.poison_error()),
            Ok(v) => Ok(v),
            Err(panic) => Err(self.contain_panic(panic)),
        }
    }

    /// Call a native, containing its panics (`catch_panic`). Every call of
    /// a native goes through here: the checks around the call are kept to
    /// two loads, the rest out of line.
    #[inline(always)]
    pub(crate) fn catch_native(&mut self, f: impl FnOnce(&mut Vm) -> Result<Value, Error>) -> Result<Value, Error> {
        self.catch_panic(f).and_then(|result| result)
    }

    /// Poison the world for a caught panic, and the error to report.
    #[cold]
    #[inline(never)]
    fn contain_panic(&self, panic: Box<dyn std::any::Any + Send>) -> Error {
        self.poison_panic(panic.as_ref());
        dispose_panic_payload(panic);
        self.poison_error()
    }

    /// Execute one instruction for JIT-compiled code (its slow paths). For a
    /// conditional branch the result says whether to jump.
    /// `bp` identifies a rooted frame; no cached register pointer crosses
    /// allocation. Numeric inputs are consumed before any collection, while
    /// pair fields and captures are reread afterwards.
    pub(crate) unsafe fn jit_slow_op(&mut self, bp: usize, op: Op) -> Result<bool, Error> {
        unsafe {
            macro_rules! reg {
                ($i:expr) => {
                    self.regs[bp + $i as usize]
                };
            }
            macro_rules! binary {
                ($dst:expr, $a:expr, $b:expr, $f:path) => {{
                    let (a, b) = (reg!($a), reg!($b));
                    let v = $f(self, a, b)?;
                    reg!($dst) = v;
                }};
            }
            let int = |i: i16| Value::int_unchecked(i as i64);
            match op {
                Op::GetG { dst, g } => {
                    let v = self.globals[g as usize];
                    if v == Value::UNDEFINED {
                        return Err(Error::violation(format!("unbound variable: {}", self.global_name(g))));
                    }
                    reg!(dst) = v;
                }
                Op::MkBox { r: x } => {
                    let p = self.alloc(2);
                    self.heap_access(|mut heap| {
                        let value = heap.register(bp + x as usize);
                        let boxed = heap.initialize_box(p, value);
                        heap.store(bp + x as usize, boxed);
                    });
                }
                Op::Cons { dst, a, b } => {
                    let p = self.alloc(3);
                    self.heap_access(|mut heap| {
                        let (a, b) = (heap.register(bp + a as usize), heap.register(bp + b as usize));
                        let pair = heap.initialize_pair(p, a, b);
                        heap.store(bp + dst as usize, pair);
                    });
                }
                Op::Add { dst, a, b } => binary!(dst, a, b, num::add),
                Op::AddI { dst, a, i } => {
                    let a = reg!(a);
                    let v = num::add(self, a, int(i))?;
                    reg!(dst) = v;
                }
                Op::Sub { dst, a, b } => binary!(dst, a, b, num::sub),
                Op::Mul { dst, a, b } => binary!(dst, a, b, num::mul),
                Op::Quo { dst, a, b } => binary!(dst, a, b, num::quotient),
                Op::Rem { dst, a, b } => binary!(dst, a, b, num::remainder),
                Op::Mod { dst, a, b } => binary!(dst, a, b, num::modulo),
                Op::Lt { dst, a, b } => reg!(dst) = Value::bool(num::lt(reg!(a), reg!(b))?),
                Op::Le { dst, a, b } => reg!(dst) = Value::bool(num::le(reg!(a), reg!(b))?),
                Op::NumEq { dst, a, b } => reg!(dst) = Value::bool(num::num_eq(reg!(a), reg!(b))?),
                Op::JNLt { a, b, .. } => return Ok(!num::lt(reg!(a), reg!(b))?),
                Op::JNLe { a, b, .. } => return Ok(!num::le(reg!(a), reg!(b))?),
                Op::JNNumEq { a, b, .. } => return Ok(!num::num_eq(reg!(a), reg!(b))?),
                Op::JNLtI { a, i, .. } => return Ok(!num::lt(reg!(a), int(i))?),
                Op::JNGtI { a, i, .. } => return Ok(!num::lt(int(i), reg!(a))?),
                Op::JNEqI { a, i, .. } => return Ok(!num::num_eq(reg!(a), int(i))?),
                Op::Car { a, .. } | Op::Cdr { a, .. } => {
                    let name = if matches!(op, Op::Car { .. }) { "car" } else { "cdr" };
                    return Err(crate::builtins::type_error(name, "pair", reg!(a)));
                }
                Op::VRef { v, i, .. } => return Err(crate::builtins::index_error("vector-ref", reg!(v), reg!(i))),
                Op::VSet { v, i, .. } => return Err(crate::builtins::vector_set_error(reg!(v), reg!(i))),
                Op::PopHandler => {
                    self.handlers.pop();
                }
                Op::Closure { dst, code: c } => {
                    let target: *const Code = self.code(c);
                    let n = {
                        let code: &Code = &*target;
                        code.captures.len()
                    };
                    let p = self.alloc(2 + n);
                    let target: *const Code = self.code(c);
                    *p = header(Kind::Closure, 1 + n, 0);
                    set_field(p, 0, Value::int_unchecked(c as i64));
                    let current = self.regs[bp - 1];
                    for (i, src) in (*target).captures.iter().enumerate() {
                        let v = match *src {
                            CapSrc::Reg(x) => reg!(x),
                            CapSrc::Cap(j) => field(current.as_ptr(), 1 + j as usize),
                        };
                        set_field(p, 1 + i, v);
                    }
                    reg!(dst) = Value::ptr(p);
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
                    return Err(Error::violation(format!("{}: expected at least {fixed} arguments, got {n}", c.name)));
                }
                // Initialise the frame before allocating: the GC scans it.
                if size > n {
                    self.regs[bp + n..bp + size].fill(Value::VOID);
                }
                self.stack_top = self.stack_top.max(bp + n.max(size));
                let rest = self.regs[bp + fixed..bp + n].to_vec();
                let list = self.make_list_raw(&rest);
                self.regs[bp + fixed] = list;
                if n > fixed + 1 {
                    self.regs[bp + fixed + 1..bp + n].fill(Value::VOID);
                }
            } else {
                if n != fixed {
                    return Err(Error::violation(format!("{}: expected {fixed} arguments, got {n}", c.name)));
                }
                self.regs[bp + n..bp + size.max(n)].fill(Value::VOID);
            }
            Ok(())
        }
    }

    /// Call native `index` with `n` arguments from `regs[args]`, from
    /// outside an execution: its panics are contained here.
    pub(crate) fn call_native(&mut self, index: usize, args: usize, n: usize) -> Result<Value, Error> {
        self.catch_native(|vm| vm.call_native_running(index, args, n))
    }

    /// Call native `index` from code running in an execution (the
    /// interpreter, compiled code's helpers). A panic unwinds to the
    /// execution's boundary (`as_execution`, `run_tasks`, `catch_jit`),
    /// which retires the world: catching it at every call would cost every
    /// call. What is checked here is a retirement the native swallowed
    /// (the error of a nested execution).
    #[inline(always)]
    pub(crate) fn call_native_running(&mut self, index: usize, args: usize, n: usize) -> Result<Value, Error> {
        let native = &self.natives[index];
        if !native.takes(n) {
            return Err(self.native_arity_error(index, n));
        }
        let result = match &native.f {
            NativeImpl::Plain(f, _) => {
                let f = *f;
                f(self, args, n)
            }
            NativeImpl::Boxed(f) => {
                // Natives are never removed, so the closure outlives the
                // call even if `natives` grows meanwhile.
                let f: *const dyn Fn(&mut Vm, usize, usize) -> Result<Value, Error> = Rc::as_ptr(f);
                unsafe { (*f)(self, args, n) }
            }
        };
        if self.poisoned().is_some() {
            return Err(self.poison_error());
        }
        result.map_err(|e| self.native_error(index, e))
    }

    #[cold]
    #[inline(never)]
    fn native_arity_error(&self, index: usize, n: usize) -> Error {
        Error::violation(format!("{}: wrong number of arguments ({n})", self.natives[index].name))
    }

    /// An error native `index` returned, named after it if it raised it
    /// itself, not in errors from Scheme code it ran (`eval`, callbacks),
    /// which carry a trace.
    #[cold]
    #[inline(never)]
    pub(crate) fn native_error(&self, index: usize, mut e: Error) -> Error {
        let name = &self.natives[index].name;
        if e.tail_call.is_none()
            && e.escape.is_none()
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
    }

    /// Install a handler procedure below all code run from now on (e.g. a
    /// REPL debugger). It sees every condition no `guard` catches.
    pub(crate) fn push_handler_raw(&mut self, handler: Value) {
        let handler = self.root(handler);
        self.handlers.push(Handler::Proc { handler });
    }

    /// Install a `with-exception-handler` procedure for the duration of `thunk`.
    pub(crate) fn with_handler_raw(&mut self, handler: Value, thunk: Value) -> Result<Value, Error> {
        let handler = self.root(handler);
        self.handlers.push(Handler::Proc { handler });
        let depth = self.handlers.len();
        let result = self.call_raw(thunk, &[]);
        self.unwind_to(depth - 1);
        if self.poisoned().is_some() {
            return Err(self.poison_error());
        }
        result
    }

    /// `raise-continuable`: call the innermost handler procedure and return
    /// its value; guards unwind as for `raise`.
    pub(crate) fn raise_continuable_raw(&mut self, v: Value) -> Result<Value, Error> {
        // The innermost visible handler: a procedure handles it here and its
        // value is returned; a guard (or none) unwinds as for `raise`.
        match self.continuable_handler() {
            Some(i) => {
                let handler = self.handler_at(i).expect("a handler procedure");
                self.call_masked(i, handler, v, None)
            }
            None => Err(self.raise_error_raw(v)),
        }
    }

    /// The error used to `raise` the object `v`.
    pub(crate) fn raise_error_raw(&mut self, v: Value) -> Error {
        let msg = crate::builtins::condition_message(self, v);
        // Of the error object's kind: raised again, a violation stays one,
        // also where its object does not go (another world).
        let kind = match crate::stdlib::error_kind(self, v) {
            Some("file") => ErrorKind::File,
            Some("read") => ErrorKind::Read,
            Some("violation") => ErrorKind::Violation,
            _ => ErrorKind::General,
        };
        let mut e = Error::new(msg).with_kind(kind);
        e.payload = Some(self.root(v));
        e
    }

    /// What `sym` denotes in `module`, as help shows it.
    pub fn describe_name(&self, module: u32, name: &str) -> Description {
        self.host_ref(|vm| vm.describe_name_raw(module, reader::intern(name))).unwrap_or_else(|_| Description {
            name: name.into(),
            kind: "unbound",
            params: None,
            arity: None,
            doc: None,
            file: None,
            line: 0,
            column: 0,
        })
    }

    /// What `sym` denotes in `module`, as `describe_name_raw` says it in
    /// its `kind`, without making the rest: its documentation, location.
    pub(crate) fn binding_kind_raw(&self, module: u32, sym: u32) -> &'static str {
        match self.lookup_global(module, sym) {
            Some(GlobalBinding::Macro(_)) => "macro",
            None if crate::compiler::special_form_doc(&symbol_name(sym)).is_some() => "special form",
            None => "unbound",
            Some(GlobalBinding::Var(g)) => {
                let v = self.globals[g as usize];
                let v = if is_kind(v, Kind::Closure) || v.is_native() { Some(v) } else { Vm::applicable_proc(v) };
                match v {
                    Some(p) if p.is_native() => "built-in procedure",
                    Some(p) if is_kind(p, Kind::Closure) => "procedure",
                    _ if self.globals[g as usize] == Value::UNDEFINED => "unbound",
                    _ => "variable",
                }
            }
        }
    }

    /// The module `sym` denotes something in, seen from `here`, as
    /// `defined_names_raw` gives it.
    pub(crate) fn binding_module_raw(&self, here: u32, sym: u32) -> Option<u32> {
        if self.binding_kind_raw(here, sym) != "unbound" {
            return Some(here);
        }
        self.loaded_module_names()
            .iter()
            .filter_map(|name| self.loaded_module(name))
            .find(|&m| self.bindings.contains_key(&(m, sym)) && self.binding_kind_raw(m, sym) != "unbound")
    }

    /// Every name a loaded module defines or `here` sees, with the module
    /// it denotes something in: `here` if it sees it, else the first module
    /// loaded that defines it; by name, without `%` names.
    pub(crate) fn defined_names_raw(&self, here: u32) -> Vec<(u32, u32)> {
        let mut found: FxHashMap<u32, u32> = FxHashMap::default();
        for m in self.loaded_module_names().iter().filter_map(|name| self.loaded_module(name)) {
            for &s in &self.modules[m as usize].defined {
                if self.bindings.contains_key(&(m, s)) {
                    found.entry(s).or_insert(m);
                }
            }
        }
        for name in self.global_names_raw(here) {
            let s = reader::intern(&name);
            if self.binding_kind_raw(here, s) != "unbound" {
                found.insert(s, here);
            }
        }
        let mut names: Vec<(Rc<str>, u32, u32)> = found
            .into_iter()
            .map(|(s, m)| (symbol_name(s), s, m))
            .filter(|(name, s, m)| !name.starts_with('%') && !name.contains('\u{1f}') && self.binding_kind_raw(*m, *s) != "unbound")
            .collect();
        names.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        names.into_iter().map(|(_, s, m)| (s, m)).collect()
    }

    pub(crate) fn describe_name_raw(&self, module: u32, sym: u32) -> Description {
        let name = symbol_name(sym);
        let unbound =
            Description { name: name.clone(), kind: "unbound", params: None, arity: None, doc: None, file: None, line: 0, column: 0 };
        match self.lookup_global(module, sym) {
            Some(GlobalBinding::Macro(m)) => {
                let (line, column) = if m.pos == NO_POS { (0, 0) } else { m.file.line_col(m.pos) };
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
                match self.procedure_info_raw(v) {
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
    pub fn describe_binding(&mut self, module: u32, name: &str) -> String {
        self.host(|vm| vm.describe_binding_raw(module, reader::intern(name))).unwrap_or_else(|e| e.msg.clone())
    }

    pub(crate) fn describe_binding_raw(&mut self, module: u32, sym: u32) -> String {
        let d = self.describe_name_raw(module, sym);
        match d.kind {
            "unbound" => format!("{}: unbound", d.name),
            "variable" => {
                let v = self.get_global_in_raw(module, &d.name).unwrap_or(Value::UNDEFINED);
                let doc = d.doc.map(|doc| format!("\n\n{doc}")).unwrap_or_default();
                format!("{} = {}{doc}", d.name, crate::builtins::repr(v))
            }
            _ => d.to_string(),
        }
    }

    pub(crate) fn describe_value_raw(&self, name: &str, v: Value) -> String {
        match self.procedure_info_raw(v) {
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
        self.innermost_raise().map_or(&[], |r| &r.trace)
    }

    /// The raise the innermost running handler procedure was called for
    /// (not counting those `raise-continuable` calls).
    fn innermost_raise(&self) -> Option<&Raised> {
        self.handlers.iter().rev().find_map(|h| match h {
            Handler::Mask { raised: Some(raised), .. } => Some(&**raised),
            _ => None,
        })
    }

    /// Signature, docstring and definition site of a procedure (or of an
    /// applicable record's procedure).
    pub(crate) fn procedure_info_raw(&self, v: Value) -> Option<ProcedureInfo> {
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
            return Vm::applicable_proc(v).and_then(|p| self.procedure_info_raw(p));
        }
        let code = unsafe { &*self.closure_code(v) };
        let file = &self.files[code.file as usize];
        let (line, column) = if code.pos == NO_POS { (0, 0) } else { file.line_col(code.pos) };
        Some(ProcedureInfo {
            name: code.name.clone(),
            params: code.params.to_vec(),
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
    pub(crate) fn documentation_raw(&self, v: Value) -> Option<Rc<str>> {
        if v.is_native() {
            self.natives[v.as_native()].doc.filter(|d| !d.doc.is_empty()).map(|d| d.doc.into())
        } else if is_kind(v, Kind::Closure) {
            unsafe { &*self.closure_code(v) }.doc.clone()
        } else {
            Vm::applicable_proc(v).and_then(|p| self.documentation_raw(p))
        }
    }

    /// The name a procedure value was defined with, if it has one.
    pub(crate) fn procedure_name_raw(&self, v: Value) -> Option<Rc<str>> {
        if is_kind(v, Kind::Closure) {
            Some(unsafe { &*self.closure_code(v) }.name.clone())
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
        self.host_ref(|vm| vm.bindings.keys().filter(|(m, _)| *m == ROOT_MODULE).map(|(_, s)| symbol_name(*s)).collect())
            .unwrap_or_default()
    }

    pub fn global_names(&self, module: u32) -> Vec<Rc<str>> {
        self.host_ref(|vm| vm.global_names_raw(module)).unwrap_or_default()
    }

    pub(crate) fn global_names_raw(&self, module: u32) -> Vec<Rc<str>> {
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
        for code in (first..=last).filter(|&i| !self.code_ptrs[i as usize].is_null()).map(|i| self.code(i)) {
            eprintln!("== {} (params {}, frame {})", code.name, code.nparams, code.frame_size);
            for (i, op) in code.ops.iter().enumerate() {
                eprintln!("  {i:3} {op:?}");
            }
        }
    }

    pub fn flush(&mut self) {
        self.out.flush();
    }

    /// Send this world's standard output and error, from now on, to a
    /// capture of at most `limit` bytes, instead of the process's: for a
    /// host running code whose output is not the process's to show (a
    /// request, a run). What is written past the limit is dropped and
    /// the capture marked truncated. `take_output` reads it.
    pub fn capture_output(&mut self, limit: usize) {
        self.out.capture(limit);
    }

    /// What was written to standard output and error since the capture
    /// began or was last taken (`capture_output`), and empty it; none if
    /// they are not captured.
    pub fn take_output(&mut self) -> Option<Output> {
        self.out.take()
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

pub(crate) unsafe fn init_string(p: *mut u64, bytes: &[u8]) {
    unsafe { init_string_with(p, bytes.len(), |out| out.copy_from_slice(bytes)) }
}

/// Makes `p`, allocated for them, a string of `len` bytes that `fill`
/// writes, which must be UTF-8: no copy of a text gathered first.
pub(crate) unsafe fn init_string_with(p: *mut u64, len: usize, fill: impl FnOnce(&mut [u8])) {
    unsafe {
        let out = string_bytes(p, len);
        fill(out);
        *p = header(Kind::String, len, if out.is_ascii() { heap::ASCII } else { 0 });
    }
}

/// `init_string_with` for text known to be ASCII, unchecked.
pub(crate) unsafe fn init_ascii_with(p: *mut u64, len: usize, fill: impl FnOnce(&mut [u8])) {
    unsafe {
        let out = string_bytes(p, len);
        fill(out);
        debug_assert!(out.is_ascii());
        *p = header(Kind::String, len, heap::ASCII);
    }
}

/// The `len` bytes of the string being made at `p`, padding zeroed.
unsafe fn string_bytes<'a>(p: *mut u64, len: usize) -> &'a mut [u8] {
    unsafe {
        let words = len.div_ceil(8);
        if words > 0 {
            *p.add(words) = 0;
        }
        std::slice::from_raw_parts_mut(p.add(1) as *mut u8, len)
    }
}

pub(crate) unsafe fn init_bytes(p: *mut u64, bytes: &[u8]) {
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

/// `Vm::jit_fuel` while compiled code runs, put back as it was however
/// that ends (an unwinding panic too): it never names a dead slice.
struct JitFuel {
    slot: *mut *mut u32,
    outer: *mut u32,
}

impl JitFuel {
    /// # Safety
    /// `slot` is valid while the guard lives, and `fuel` while it is set.
    unsafe fn set(slot: *mut *mut u32, fuel: *mut u32) -> JitFuel {
        // SAFETY: forwarded from the caller.
        let outer = unsafe { std::mem::replace(&mut *slot, fuel) };
        JitFuel { slot, outer }
    }
}

impl Drop for JitFuel {
    fn drop(&mut self) {
        // SAFETY: `set`'s caller keeps the slot valid while the guard lives.
        unsafe { *self.slot = self.outer };
    }
}
