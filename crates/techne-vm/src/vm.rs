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
};

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

const MAX_REGS: usize = 1 << 26;
const CANNOT_SUSPEND: &str = "cannot suspend here: a Rust native procedure is calling back into Scheme (vm.call)";
/// Module 0 holds builtins and the prelude; it is visible from every module.
pub const ROOT_MODULE: u32 = 0;
/// Module for code evaluated without a file (REPL, `eval_source`).
pub const USER_MODULE: u32 = 1;

pub struct Error {
    pub msg: String,
    /// Innermost first: "name (file:line:col)".
    pub trace: Vec<String>,
    /// The raised Scheme object, if any (error objects, `raise`d values).
    pub payload: Option<Root>,
    /// Escape to the `call/cc` with this id, carrying the value.
    pub escape: Option<(i64, Root)>,
    /// Handlers at or above this index were already consulted (set when the
    /// error leaves a dispatch level, so outer levels continue below it).
    searched: Option<usize>,
    /// Set by natives that must wait (see `tasks`): suspends the running task.
    pub wait: Option<crate::tasks::Wait>,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Error {
        Error { msg: msg.into(), trace: Vec::new(), payload: None, escape: None, searched: None, wait: None }
    }

    /// A copy for another consumer (e.g. every task joining a failed task).
    pub fn duplicate(&self) -> Error {
        Error { msg: self.msg.clone(), trace: self.trace.clone(), payload: self.payload.clone(), ..Error::new("") }
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
}

#[derive(Clone)]
pub enum GlobalBinding {
    Var(u32),
    Macro(Rc<Macro>),
}

pub struct Module {
    pub name: Rc<str>,
    pub path: Option<PathBuf>,
    imports: FxHashMap<u32, GlobalBinding>,
    exports: Option<Vec<u32>>,
    defined: Vec<u32>,
    loading: bool,
}

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

/// `TECHNE_JIT`: unset for the default, `0` to disable, or the number of loop
/// iterations after which a function is compiled.
fn jit_from_env() -> Option<Box<crate::jit::Jit>> {
    let threshold = match std::env::var("TECHNE_JIT") {
        Ok(s) if s == "0" || s == "off" => return None,
        Ok(s) => s.parse().unwrap_or(1000),
        Err(_) => 1000,
    };
    crate::jit::Jit::new(threshold).map(Box::new)
}

pub struct Vm {
    pub heap: Heap,
    pub regs: Vec<Value>,
    frames: Vec<Frame>,
    handlers: Vec<Handler>,
    pub globals: Vec<Value>,
    global_names: Vec<u32>,
    global_module: Vec<u32>,
    user_defined: Vec<bool>,
    bindings: FxHashMap<(u32, u32), GlobalBinding>,
    /// Field counts of record types defined at top level, by the global that
    /// holds the type (for `match` record patterns).
    pub record_types: FxHashMap<u32, usize>,
    pub modules: Vec<Module>,
    module_paths: FxHashMap<PathBuf, u32>,
    pub files: Vec<SourceFile>,
    codes: Vec<Box<Code>>,
    /// Baseline JIT (`None` when disabled with `TECHNE_JIT=0`).
    jit: Option<Box<crate::jit::Jit>>,
    /// An error raised in JIT-compiled code, handed to the interpreter.
    pub(crate) jit_error: Option<Error>,
    pub natives: Vec<Native>,
    apply_native: Value,
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
    pub(crate) channels: Vec<std::collections::VecDeque<Root>>,
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
    codes: &'a mut [Box<Code>],
    scratch: &'a mut [Value],
    specials: &'a mut [Value],
    roots: &'a mut Vec<Weak<Cell<Value>>>,
    tasks: &'a mut [crate::tasks::Task],
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
        self.globals.iter_mut().for_each(&mut *f);
        self.scratch.iter_mut().for_each(&mut *f);
        self.specials.iter_mut().for_each(&mut *f);
        for code in self.codes.iter_mut() {
            code.consts.iter_mut().for_each(&mut *f);
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
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    /// A VM with builtins and the Scheme prelude loaded.
    pub fn new() -> Vm {
        let mut vm = Vm::bare();
        if let Err(e) = vm.eval_in(ROOT_MODULE, "<prelude>", crate::PRELUDE) {
            panic!("prelude failed to load: {e}");
        }
        vm
    }

    /// A VM with builtins only.
    pub fn bare() -> Vm {
        let mut vm = Vm {
            heap: Heap::new(),
            regs: vec![Value::VOID; 1 << 16],
            frames: Vec::with_capacity(1024),
            handlers: Vec::new(),
            globals: Vec::new(),
            global_names: Vec::new(),
            global_module: Vec::new(),
            user_defined: Vec::new(),
            bindings: FxHashMap::default(),
            record_types: FxHashMap::default(),
            modules: Vec::new(),
            module_paths: FxHashMap::default(),
            files: Vec::new(),
            codes: Vec::new(),
            jit: jit_from_env(),
            jit_error: None,
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
            channels: Vec::new(),
            current_task: None,
            masks: Vec::new(),
            locals: FxHashMap::default(),
            out: BufWriter::with_capacity(1 << 16, std::io::stdout()),
        };
        vm.new_module("root", None);
        vm.new_module("user", None);
        vm.specials[SpecialObj::ErrorRtd as usize] = vm.make_rtd("error", &["message", "irritants"]);
        vm.specials[SpecialObj::ContinuationRtd as usize] = vm.make_rtd("continuation", &["id"]);
        vm.specials[SpecialObj::ValuesRtd as usize] = vm.make_rtd("values", &[]);
        vm.specials[SpecialObj::TaskRtd as usize] = vm.make_rtd("task", &["id"]);
        vm.specials[SpecialObj::ChannelRtd as usize] = vm.make_rtd("channel", &["id"]);
        crate::builtins::install(&mut vm);
        let apply = vm.global_var(ROOT_MODULE, reader::intern("apply"));
        vm.apply_native = vm.globals[apply as usize];
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

    fn new_module(&mut self, name: &str, path: Option<PathBuf>) -> u32 {
        self.modules.push(Module {
            name: name.into(),
            path,
            imports: FxHashMap::default(),
            exports: None,
            defined: Vec::new(),
            loading: false,
        });
        self.modules.len() as u32 - 1
    }

    /// The binding `sym` denotes at top level of `module`: its own definitions,
    /// then imports, then the root module.
    pub fn lookup_global(&self, module: u32, sym: u32) -> Option<GlobalBinding> {
        self.bindings
            .get(&(module, sym))
            .or_else(|| self.modules[module as usize].imports.get(&sym))
            .or_else(|| self.bindings.get(&(ROOT_MODULE, sym)))
            .cloned()
    }

    fn new_global(&mut self, module: u32, sym: u32) -> u32 {
        let g = self.globals.len() as u32;
        self.globals.push(Value::UNDEFINED);
        self.global_names.push(sym);
        self.global_module.push(module);
        self.user_defined.push(false);
        self.bindings.insert((module, sym), GlobalBinding::Var(g));
        g
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

    pub fn define_native(&mut self, native: Native) {
        let g = self.global_var(ROOT_MODULE, reader::intern(&native.name));
        self.globals[g as usize] = Value::native(self.natives.len() as u32);
        self.natives.push(native);
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
        match self.lookup_global(USER_MODULE, reader::intern(name))? {
            GlobalBinding::Var(g) => Some(self.globals[g as usize]).filter(|v| *v != Value::UNDEFINED),
            GlobalBinding::Macro(_) => None,
        }
    }

    pub fn set_global(&mut self, name: &str, v: Value) {
        let g = self.define_var(USER_MODULE, reader::intern(name));
        self.globals[g as usize] = v;
    }

    pub fn provide(&mut self, module: u32, syms: Vec<u32>) {
        self.modules[module as usize].exports.get_or_insert_with(Vec::new).extend(syms);
    }

    /// Load (once) the module at `spec`, relative to `from`'s file, and import
    /// its exports (all definitions when it has no `provide`) into `from`.
    pub fn require(&mut self, from: u32, spec: &str) -> Result<(), Error> {
        let base = self.modules[from as usize]
            .path
            .as_ref()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let path = base.join(spec);
        let path = path.canonicalize().map_err(|e| Error::new(format!("require {spec}: {e}")))?;
        let m = match self.module_paths.get(&path) {
            Some(&m) if self.modules[m as usize].loading => {
                return Err(Error::new(format!("require {spec}: circular module dependency")));
            }
            Some(&m) => m,
            None => {
                let text = std::fs::read_to_string(&path).map_err(|e| Error::new(format!("require {spec}: {e}")))?;
                let m = self.new_module(&path.to_string_lossy(), Some(path.clone()));
                self.module_paths.insert(path.clone(), m);
                self.modules[m as usize].loading = true;
                let result = self.eval_in(m, &path.to_string_lossy(), &text);
                self.modules[m as usize].loading = false;
                result?;
                m
            }
        };
        let module = &self.modules[m as usize];
        let names = module.exports.clone().unwrap_or_else(|| module.defined.clone());
        for sym in names {
            let binding = self
                .bindings
                .get(&(m, sym))
                .cloned()
                .ok_or_else(|| Error::new(format!("{spec} provides undefined {}", symbol_name(sym))))?;
            self.modules[from as usize].imports.insert(sym, binding);
        }
        Ok(())
    }

    // ----- code and constants (used by the compiler) -----

    pub fn add_code(&mut self, code: Code) -> u32 {
        self.codes.push(Box::new(code));
        self.codes.len() as u32 - 1
    }
    pub fn set_captures(&mut self, code: u32, captures: Vec<CapSrc>) {
        self.codes[code as usize].captures = captures;
    }

    /// Materialise a literal. Heap parts go to the old space; they only
    /// reference each other, so they need no remembering.
    pub fn constant(&mut self, s: &Sexp) -> Value {
        match s {
            Sexp::Int(i) => Value::fixnum(*i).unwrap_or_else(|| {
                let p = self.heap.alloc_old_unremembered(2);
                unsafe {
                    *p = header(Kind::BigInt, 1, 0);
                    *p.add(1) = *i as u64;
                }
                Value::ptr(p)
            }),
            Sexp::Float(f) => Value::float(*f),
            Sexp::Bool(b) => Value::bool(*b),
            Sexp::Char(c) => Value::char(*c),
            Sexp::Sym(id) => Value::symbol(reader::strip(*id)),
            Sexp::Keyword(id) => Value::keyword(*id),
            Sexp::Str(s) => {
                let p = self.heap.alloc_old_unremembered(heap::string_words(s.len()));
                unsafe { init_string(p, s.as_bytes()) };
                Value::ptr(p)
            }
            Sexp::List(items, tail, _) => {
                let mut acc = tail.as_ref().map_or(Value::NIL, |t| self.constant(t));
                for item in items.iter().rev() {
                    let car = self.constant(item);
                    let p = self.heap.alloc_old_unremembered(3);
                    unsafe {
                        *p = header(Kind::Pair, 2, 0);
                        set_field(p, 0, car);
                        set_field(p, 1, acc);
                    }
                    acc = Value::ptr(p);
                }
                acc
            }
            Sexp::Vector(items) => {
                let vals: Vec<Value> = items.iter().map(|i| self.constant(i)).collect();
                let p = self.heap.alloc_old_unremembered(1 + vals.len());
                unsafe {
                    *p = header(Kind::Vector, vals.len(), 0);
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

    #[cold]
    fn alloc_slow(&mut self, words: usize) -> *mut u64 {
        if words >= LARGE_WORDS {
            return self.heap.alloc_old(words);
        }
        self.collect();
        self.heap.bump(words)
    }

    pub fn collect(&mut self) {
        let Vm { heap, regs, globals, codes, scratch, specials, roots, stack_top, tasks, .. } = self;
        let mut r = VmRoots { regs: &mut regs[..*stack_top], globals, codes, scratch, specials, roots, tasks };
        heap.collect(&mut r);
        self.release_dead_foreign();
    }

    pub fn full_collect(&mut self) {
        let Vm { heap, regs, globals, codes, scratch, specials, roots, stack_top, tasks, .. } = self;
        let mut r = VmRoots { regs: &mut regs[..*stack_top], globals, codes, scratch, specials, roots, tasks };
        heap.full_collect(&mut r);
        self.release_dead_foreign();
    }

    fn release_dead_foreign(&mut self) {
        for i in std::mem::take(&mut self.heap.dead_foreign) {
            self.foreign[i] = None;
            self.foreign_free.push(i);
        }
    }

    /// Keep `v` alive and up to date across collections while the `Root` lives.
    pub fn root(&mut self, v: Value) -> Root {
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

    pub fn make_string(&mut self, bytes: &[u8]) -> Value {
        let p = self.alloc(heap::string_words(bytes.len()));
        unsafe { init_string(p, bytes) };
        Value::ptr(p)
    }

    /// A record of type `rtd` with `fields`; all inputs are rooted meanwhile.
    pub fn make_record(&mut self, rtd: Value, fields: &[Value]) -> Value {
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
        let p = self.alloc(3 * items.len());
        let mut acc = Value::NIL;
        unsafe {
            for (k, i) in (mark..self.scratch.len()).rev().enumerate() {
                let q = p.add(3 * k);
                *q = header(Kind::Pair, 2, 0);
                set_field(q, 0, self.scratch[i]);
                set_field(q, 1, acc);
                acc = Value::ptr(q);
            }
        }
        self.scratch.truncate(mark);
        acc
    }

    /// Box an integer, as a fixnum when it fits.
    pub fn make_int(&mut self, i: i64) -> Value {
        Value::fixnum(i).unwrap_or_else(|| {
            let p = self.alloc(2);
            unsafe {
                *p = header(Kind::BigInt, 1, 0);
                *p.add(1) = i as u64;
            }
            Value::ptr(p)
        })
    }

    /// An error object with `message` and `irritants`.
    pub fn make_error_object(&mut self, message: &str, irritants: &[Value]) -> Value {
        let mark = self.scratch.len();
        self.scratch.extend_from_slice(irritants);
        let msg = self.make_string(message.as_bytes());
        self.scratch.push(msg);
        let items: Vec<Value> = self.scratch[mark..self.scratch.len() - 1].to_vec();
        let list = self.make_list(&items);
        let msg = self.scratch[self.scratch.len() - 1];
        self.scratch.truncate(mark);
        let rtd = self.special(SpecialObj::ErrorRtd);
        self.make_record(rtd, &[msg, list])
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
        self.modules[USER_MODULE as usize].path = Some(canonical);
        self.eval_in(USER_MODULE, &path.to_string_lossy(), &text)
    }

    pub fn eval_in(&mut self, module: u32, name: &str, source: &str) -> Result<Value, Error> {
        self.files.push(SourceFile { name: name.into(), text: source.into() });
        let file = self.files.len() as u32 - 1;
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
            let code = Compiler::new(self, module, file).compile_toplevel(form)?;
            if std::env::var_os("TECHNE_DUMP").is_some() {
                self.dump_from(code);
            }
            last = self.run(code)?;
        }
        Ok(last)
    }

    /// Compile and run one form in the user module (`eval`).
    pub fn eval_sexp(&mut self, form: &Sexp) -> Result<Value, Error> {
        if !self.files.iter().any(|f| &*f.name == "<eval>") {
            self.files.push(SourceFile { name: "<eval>".into(), text: "".into() });
        }
        let file = self.files.iter().position(|f| &*f.name == "<eval>").unwrap() as u32;
        predeclare(self, USER_MODULE, form);
        let code = Compiler::new(self, USER_MODULE, file).compile_toplevel(form)?;
        self.run(code)
    }

    fn run(&mut self, entry: u32) -> Result<Value, Error> {
        let code: *const Code = &*self.codes[entry as usize];
        let saved_top = self.stack_top;
        let bp = self.stack_top + 1;
        let size = unsafe { (*code).frame_size } as usize;
        self.ensure_regs(bp + size);
        self.regs[bp - 1] = Value::VOID;
        self.regs[bp..bp + size].fill(Value::VOID);
        let result = unsafe { self.dispatch(code, bp) };
        self.stack_top = saved_top;
        result
    }

    /// Call a Scheme procedure from Rust. Arguments are copied onto the stack
    /// before anything can allocate.
    pub fn call(&mut self, f: Value, args: &[Value]) -> Result<Value, Error> {
        let saved_top = self.stack_top;
        let base = self.stack_top;
        self.ensure_regs(base + 2 + args.len());
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
    fn unwind_to(&mut self, keep: usize) {
        while self.handlers.len() > keep {
            if let Some(Handler::Wind { after }) = self.handlers.pop() {
                // An error in an after-thunk does not replace the one unwinding.
                let _ = self.call(after.get(), &[]);
            }
        }
    }

    /// Start a task's entry procedure on the (already swapped-in) task stack.
    pub(crate) fn start_task(&mut self, f: Value) -> Result<Exit, Error> {
        self.ensure_regs(8);
        self.regs[0] = f;
        self.stack_top = 1;
        if is_kind(f, Kind::Closure) {
            unsafe {
                let callee = field(f.as_ptr(), 0).as_int() as *const Code;
                self.ensure_regs(1 + (*callee).frame_size as usize);
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
                e.trace.push(self.location(code, pc - 1));
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
                let callee = field(f.as_ptr(), 0).as_int() as *const Code;
                let bp = base + 1;
                self.ensure_regs(bp + (*callee).frame_size as usize + n);
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
            Err(Error::new(format!("not a procedure: {}", crate::builtins::repr(f))))
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
        self.ensure_regs(base + 2 + total);
        self.regs[base + fixed..base + fixed + items.len()].copy_from_slice(&items);
        self.stack_top = self.stack_top.max(base + 1 + total);
        Ok(total)
    }

    /// The procedure to run when record `f` is called, if its type is applicable.
    #[inline]
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
        e.escape = Some((id, self.root(value)));
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
        let before = &file.text[..(pos as usize).min(file.text.len())];
        let line = before.matches('\n').count() + 1;
        let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
        format!("{} ({}:{line}:{col})", code.name, file.name)
    }

    /// Find a handler for error `e` raised in the dispatch level whose base
    /// frame is at `base_bp`, innermost first. Handler procedures run right
    /// here at the raise point, whatever level installed them (they do not
    /// unwind). A `guard` of this level is returned to unwind to; a `guard` of
    /// an outer level ends the search, and the error propagates there (through
    /// natives such as `dynamic-wind`).
    fn catch(&mut self, mut e: Error, base_bp: usize) -> Result<(Landing, Value), Error> {
        let mut idx = e.searched.unwrap_or(usize::MAX).min(self.handlers.len());
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
                        return Ok((Landing { frames_len, code, bp, target, dst }, value.get()));
                    }
                }
                Handler::Guard { frames_len, code, bp, target, dst } if e.escape.is_none() => {
                    let condition = self.condition_of(&mut e);
                    self.unwind_to(idx - 1);
                    return Ok((Landing { frames_len, code, bp, target, dst }, condition.get()));
                }
                Handler::Proc { handler } if e.escape.is_none() => {
                    // Run at the raise point; raises inside go to outer handlers.
                    let condition = self.condition_of(&mut e);
                    let result = self.call_masked(idx - 1, handler.get(), condition.get());
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
        e.searched = Some(idx);
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
        let obj = self.make_error_object(&msg, &[]);
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
    unsafe fn dispatch_loop(&mut self, code: *const Code, pc: usize, bp: usize, base_frames: usize, suspendable: bool) -> Result<Exit, Error> {
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
            self.ensure_regs(bp + (*code).frame_size as usize);
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
            // Raise: unwind to a guard in this dispatch level, or return the
            // error (with this frame's location) to the caller.
            macro_rules! fail {
                ($e:expr) => {{
                    let mut e: Error = $e;
                    if e.escape.is_none() {
                        e.trace.push(self.location(code, pc - 1));
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
                            continue;
                        }
                        Err(mut e) => {
                            if e.escape.is_none() {
                                e.trace.extend(
                                    self.frames[base_frames..].iter().rev().take(16).map(|f| self.location(f.code, f.pc as usize - 1)),
                                );
                            }
                            return Err(e);
                        }
                    }
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
                    let callee = field(f.as_ptr(), 0).as_int() as *const Code;
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
                    self.ensure_regs(new_bp + size.max(n));
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
                            fail!(Error::new(format!("not a procedure: {}", crate::builtins::repr(f))))
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
                        let target: *const Code = &**self.codes.get_unchecked(c as usize);
                        let n = (*target).captures.len();
                        sync_top!();
                        let p = self.alloc(2 + n);
                        *p = header(Kind::Closure, 1 + n, 0);
                        // Code objects are boxed and never freed, so the address is stable.
                        set_field(p, 0, Value::int_unchecked(target as i64));
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
                        if let Some(jit) = &self.jit
                            && n == jit.threshold
                        {
                            self.jit_compile(code);
                            ops = (*code).ops.as_ptr();
                        }
                        tick!();
                    }
                    Op::EnterJit => {
                        sync_top!();
                        let f = (*code).jit.entry.get().unwrap_unchecked();
                        // Outside tasks native loops never run out of fuel.
                        let mut unlimited = u32::MAX;
                        let fuel_ptr: *mut u32 = if SUSPENDABLE { &mut fuel } else { &mut unlimited };
                        let globals = self.globals.as_mut_ptr();
                        let res = f(self as *mut Vm, r, globals, fuel_ptr, (pc - 1) as u32);
                        pc = res as u32 as usize;
                        // A native called from JIT code may have grown the register stack.
                        r = self.regs.as_mut_ptr().add(bp);
                        match (res >> 32) as u32 {
                            crate::jit::EXIT | crate::jit::RESUME => {}
                            crate::jit::ERROR => {
                                let e = self.jit_error.take().expect("JIT error");
                                fail!(e)
                            }
                            crate::jit::TICK => {
                                if SUSPENDABLE {
                                    return Ok(Exit::Suspend(Suspend { code, pc, bp, slot: 0, tail: false, wait: None }));
                                }
                            }
                            _ => {
                                let mut e = self.jit_error.take().expect("JIT wait");
                                if !SUSPENDABLE {
                                    fail!(Error::new(CANNOT_SUSPEND))
                                }
                                let (Op::Call { base, .. } | Op::CallG { base, .. }) = (*code).jit.ops.get().unwrap()[pc - 1] else {
                                    unreachable!()
                                };
                                let wait = e.wait.take();
                                return Ok(Exit::Suspend(Suspend { code, pc, bp, slot: bp + base as usize, tail: false, wait }));
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
                        if !is_kind(vec, Kind::Vector) || !k.is_int() || k.as_int() as u64 >= heap::len_of(vec.as_ptr()) as u64
                        {
                            fail!(crate::builtins::index_error("vector-set!", vec, k));
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
            }
        }
    }

    #[inline(always)]
    fn ensure_regs(&mut self, needed: usize) {
        if needed > self.regs.len() {
            self.grow_regs(needed);
        }
    }

    #[cold]
    fn grow_regs(&mut self, needed: usize) {
        assert!(needed <= MAX_REGS, "stack overflow");
        let len = (self.regs.len() * 2).max(needed);
        self.regs.resize(len, Value::VOID);
    }

    /// Enable the JIT, compiling functions after `threshold` loop iterations,
    /// or disable it (`None`) for code that has not been compiled yet. The
    /// default comes from `TECHNE_JIT`.
    pub fn set_jit(&mut self, threshold: Option<u32>) {
        self.jit = threshold.and_then(crate::jit::Jit::new).map(Box::new);
    }

    /// Compile a hot function and enter native code at its loop heads.
    #[cold]
    unsafe fn jit_compile(&mut self, code: *const Code) {
        let c = unsafe { &mut *(code as *mut Code) };
        if c.jit.entry.get().is_some() || c.jit.failed.get() {
            return;
        }
        let mut heads: Vec<usize> = c
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Loop { t } => Some(*t as usize),
                _ => None,
            })
            .filter(|&t| crate::jit::native(&c.ops[t]))
            .collect();
        heads.sort_unstable();
        heads.dedup();
        let orig = c.jit.ops.get_or_init(|| c.ops.clone().into_boxed_slice());
        let compiled = if heads.is_empty() { None } else { self.jit.as_mut().and_then(|j| j.compile(c, orig, &heads)) };
        if std::env::var_os("TECHNE_JIT_LOG").is_some() {
            eprintln!("jit: {} {}", c.name, if compiled.is_some() { "compiled" } else { "not compiled" });
        }
        match compiled {
            Some(f) => {
                c.jit.entry.set(Some(f));
                for h in heads {
                    c.ops[h] = Op::EnterJit;
                }
            }
            None => c.jit.failed.set(true),
        }
    }

    /// A non-tail call from JIT code: run a Rust native here; anything else is
    /// left to the interpreter. See `jit::Gen::op` for the result codes.
    pub(crate) unsafe fn jit_call_op(&mut self, r: *mut Value, op: Op) -> u32 {
        unsafe {
            let (base, n, f) = match op {
                Op::CallG { base, n, g } => (base as usize, n as usize, self.globals[g as usize]),
                Op::Call { base, n } => (base as usize, n as usize, *r.add(base as usize)),
                _ => unreachable!(),
            };
            if !f.is_native() || f == self.apply_native {
                return 1;
            }
            *r.add(base) = f;
            let regs = self.regs.as_mut_ptr();
            let bp = r.offset_from(regs) as usize;
            self.stack_top = self.stack_top.max(bp + base + 1 + n);
            match self.call_native(f.as_native(), bp + base + 1, n) {
                Ok(v) => {
                    self.regs[bp + base] = v;
                    if self.regs.as_mut_ptr() == regs { 0 } else { 4 }
                }
                Err(e) => {
                    let status = if e.wait.is_some() { 3 } else { 2 };
                    self.jit_error = Some(e);
                    status
                }
            }
        }
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
                Op::VSet { v, i, .. } => return Err(crate::builtins::index_error("vector-set!", *reg(v), *reg(i))),
                Op::PopHandler => {
                    self.handlers.pop();
                }
                _ => unreachable!("no JIT slow path for {op:?}"),
            }
            Ok(false)
        }
    }

    /// Arity check, rest-list construction and register initialisation for a
    /// closure call whose arguments are at `bp..bp + n`.
    unsafe fn enter(&mut self, callee: *const Code, bp: usize, n: usize) -> Result<(), Error> {
        unsafe {
            let c = &*callee;
            let fixed = c.nparams as usize;
            let size = c.frame_size as usize;
            self.ensure_regs(bp + size.max(n) + 1);
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
        let Native { f, min, max, name } = self.natives[index].clone();
        if n < min || max.is_some_and(|m| n > m) {
            return Err(Error::new(format!("{name}: wrong number of arguments ({n})")));
        }
        let result = match f {
            NativeImpl::Plain(f) => f(self, args, n),
            NativeImpl::Boxed(f) => f(self, args, n),
        };
        result.map_err(|mut e| {
            if e.escape.is_none() && e.payload.is_none() && !name.starts_with('%') && !e.msg.starts_with(&*name) {
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

    /// Text for `(help name)`: signature, kind, location and docstring.
    pub fn describe_binding(&mut self, sym: u32) -> String {
        let name = symbol_name(sym);
        match self.lookup_global(USER_MODULE, sym) {
            Some(GlobalBinding::Macro(_)) => format!("{name}: syntax (macro)"),
            None if crate::compiler::is_special_form(&name) => format!("{name}: special form"),
            None => format!("{name}: unbound"),
            Some(GlobalBinding::Var(g)) => {
                let v = self.globals[g as usize];
                self.describe_value(&name, v)
            }
        }
    }

    pub fn describe_value(&self, name: &str, v: Value) -> String {
        if is_kind(v, Kind::Closure) {
            let code = unsafe { &*(field(v.as_ptr(), 0).as_int() as *const Code) };
            let file = &self.files[code.file as usize];
            let location = if code.pos == NO_POS {
                file.name.to_string()
            } else {
                let before = &file.text[..(code.pos as usize).min(file.text.len())];
                format!("{}:{}", file.name, before.matches('\n').count() + 1)
            };
            let mut s = format!("({name}{}{})  procedure, {location}", if code.params.is_empty() { "" } else { " " }, code.params.join(" "));
            if let Some(doc) = &code.doc {
                s.push_str("\n\n");
                s.push_str(doc);
            }
            s
        } else if v.is_native() {
            let n = &self.natives[v.as_native()];
            let arity = match (n.min, n.max) {
                (a, Some(b)) if a == b => format!("{a} argument{}", if a == 1 { "" } else { "s" }),
                (a, Some(b)) => format!("{a}-{b} arguments"),
                (a, None) => format!("at least {a} argument{}", if a == 1 { "" } else { "s" }),
            };
            format!("{name}: built-in procedure, {arity}")
        } else if let Some(p) = Vm::applicable_proc(v) {
            let rtd = unsafe { field(field(v.as_ptr(), 0).as_ptr(), 0) };
            format!("{name}: {} (applicable record)\n{}", crate::builtins::repr(rtd), self.describe_value(name, p))
        } else {
            format!("{name} = {}", crate::builtins::repr(v))
        }
    }

    /// The docstring of a procedure (or of an applicable record's procedure).
    pub fn documentation(&self, v: Value) -> Option<Rc<str>> {
        if is_kind(v, Kind::Closure) {
            unsafe { &*(field(v.as_ptr(), 0).as_int() as *const Code) }.doc.clone()
        } else {
            Vm::applicable_proc(v).and_then(|p| self.documentation(p))
        }
    }

    /// The name a procedure value was defined with, if it has one.
    pub fn procedure_name(&self, v: Value) -> Option<Rc<str>> {
        if is_kind(v, Kind::Closure) {
            Some(unsafe { &*(field(v.as_ptr(), 0).as_int() as *const Code) }.name.clone())
        } else if v.is_native() {
            Some(self.natives[v.as_native()].name.clone())
        } else {
            None
        }
    }

    /// Names visible from the user module (for completion).
    pub fn global_names(&self) -> Vec<Rc<str>> {
        let mut names: Vec<Rc<str>> = self
            .bindings
            .keys()
            .filter(|(m, _)| *m == ROOT_MODULE || *m == USER_MODULE)
            .map(|(_, s)| symbol_name(*s))
            .filter(|n| !n.starts_with('%') && !n.contains('\u{1f}'))
            .collect();
        names.extend(self.modules[USER_MODULE as usize].imports.keys().map(|s| symbol_name(*s)));
        names.extend(crate::compiler::SPECIAL_FORMS.iter().map(|s| Rc::from(*s)));
        names.sort();
        names.dedup();
        names
    }

    /// Print the code objects compiled for the last top-level form.
    fn dump_from(&self, last: u32) {
        let first = (0..=last).rev().take_while(|&i| i == last || &*self.codes[i as usize].name != "toplevel").last().unwrap_or(last);
        for code in &self.codes[first as usize..=last as usize] {
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
fn predeclare(vm: &mut Vm, module: u32, form: &Sexp) {
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
        *p = header(Kind::String, bytes.len(), ascii);
        let words = bytes.len().div_ceil(8);
        if words > 0 {
            *p.add(words) = 0;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.add(1) as *mut u8, bytes.len());
    }
}
