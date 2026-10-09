//! Baseline JIT (Cranelift).
//!
//! When a function has been called or has looped `threshold` times, the whole
//! function is compiled. Its entry points (pc 0, loop heads and the
//! instruction after each call) are replaced by `Op::EnterJit`, where the
//! interpreter jumps into native code. Native code runs straight-line
//! instructions, branches, loops, calls to Rust natives and calls to other
//! compiled functions.
//!
//! Native calls do not push interpreter frames. Every exceptional case
//! (errors, task preemption and waits, a reallocated register stack, too deep
//! a native stack, a callee that is not compiled, handler installation)
//! unwinds the native stack: each native frame records its interpreter frame
//! (`Vm::jit_unwind`) and the interpreter continues from the innermost one
//! (`JitCtx::code`/`bp` and the returned pc). So unwinding, tasks and error
//! traces stay the interpreter's business. Tail calls return `TAILCALL` to
//! the caller's trampoline, which keeps the native stack bounded.
//!
//! Compilation runs on a background thread (`Compiler`). A `Job` carries a
//! copy of everything the compiler reads, so it never touches VM memory; the
//! VM installs finished code when it next counts calls or loop iterations.
//! A job whose code dies before then is cancelled, and its result dropped.
//!
//! Machine code goes into arenas of up to `ARENA_FUNCTIONS` functions of one
//! package generation, one Cranelift module each, since a module is only
//! freed whole. When a code with native code dies, the VM releases its
//! function; an arena whose functions are all released is freed. So a
//! retired generation's machine code goes with it, and long-lived code
//! (generation 0) does not keep it. The machine code, and what queued jobs
//! are expected to take, count against the world's memory limit
//! (`Compiler::held`); nothing is queued while there is no room for it.
//!
//! Scheme registers live in machine registers (Cranelift variables) while in
//! native code. They are written back to the register stack before every exit
//! and before every slow path, and re-read after it, because a slow path may
//! run the moving GC. Fast paths (fixnum and float arithmetic, comparisons,
//! pair and vector access, globals, boxes) are inline. Everything else goes
//! through `Vm::jit_slow_op`, which executes one instruction in Rust with the
//! interpreter's semantics.

use std::{
    cell::{Cell, OnceCell},
    collections::HashMap,
    fmt,
    mem::offset_of,
};

use cranelift_codegen::{
    Context,
    ir::{
        self, AbiParam, Block, BlockArg, InstBuilder, JumpTableData, MemFlagsData, Signature, TrapCode,
        condcodes::{FloatCC, IntCC},
        types::{F64, I8, I32, I64},
    },
    settings::{self, Configurable},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::Module;

use crate::{
    code::{Code, Op, Reg},
    heap::{Kind, field, header, is_kind},
    value::Value,
    vm::Vm,
};

/// `(vm, ctx, registers at bp, bp, entry pc, native depth) -> pc | status << 32`.
pub type JitFn = unsafe extern "C" fn(*mut Vm, *mut JitCtx, *mut Value, u64, u32, u32) -> u64;

/// What native code needs from the VM, and where it leaves the frame the
/// interpreter continues with. Inputs stay valid for a whole native run:
/// anything that could change them (a Rust native growing the register stack
/// or defining globals) makes native code return.
#[repr(C)]
pub struct JitCtx {
    pub globals: *mut Value,
    pub regs_end: *mut Value,
    pub fuel: *mut u32,
    pub stack_top: *mut usize,
    /// Nursery bump pointer and limit; null when allocation must go through
    /// the VM (GC stress).
    pub heap_top: *mut *mut u64,
    pub heap_end: *mut u64,
    /// Output: the code and frame base where the interpreter continues.
    pub code: *const Code,
    pub bp: u64,
    /// Output of a tail call: the callee's entry.
    pub tail: usize,
    /// A callee's result when it handed over to the interpreter inside a
    /// call made by `jit_call_slow`.
    pub res: u64,
}

/// Native calls nest at most this deep before handing over to the interpreter.
const MAX_DEPTH: i64 = 1000;

/// Continue interpreting at `pc` (an instruction native code does not handle).
pub const EXIT: u32 = 0;
/// `Vm::jit_error` holds an error raised by the instruction before `pc`.
pub const ERROR: u32 = 1;
/// The task's time slice ran out at a loop back-edge; resume at `pc`.
pub const TICK: u32 = 2;
/// The native call before `pc` wants to suspend the task (`Vm::jit_error`
/// holds the wait).
pub const WAIT: u32 = 3;
/// Continue at `pc`; registers are already stored (the register stack may
/// have moved).
pub const RESUME: u32 = 4;
/// The function returned; its value is in the callee slot (`bp - 1`).
pub const RETURNED: u32 = 5;
/// The frame now belongs to a tail-called function (`JitCtx::code`/`tail`),
/// to be entered at pc 0.
pub const TAILCALL: u32 = 6;
/// The tail call to a Rust native before `pc` returned (its value is in the
/// call's base register), but the register stack moved: return it from the
/// frame.
pub const RET_MOVED: u32 = 7;
/// Run the instruction at `pc` in Rust (`Vm::jit_slow_op`): a case native
/// code does not handle inline (overflow, bignums, errors, a full nursery).
pub const STEP: u32 = 8;

/// Per-function JIT state.
#[derive(Default)]
pub struct JitSlot {
    /// Loop back-edges taken so far (until compiled).
    pub hot: Cell<u32>,
    pub entry: Cell<Option<JitFn>>,
    /// `entry` if the function can be entered at pc 0 (by native calls).
    pub call_entry: Cell<Option<JitFn>>,
    pub failed: Cell<bool>,
    /// The arena `entry` is in.
    pub arena: Cell<u32>,
    /// The original instructions (loop heads are overwritten by `EnterJit`).
    pub ops: OnceCell<Box<[Op]>>,
    /// The closures the compiled code's calls through globals expect (see
    /// `Known::closure`): GC roots, kept current as they move, and keeping
    /// them alive after the globals change.
    pub callees: OnceCell<Box<[Value]>>,
}

impl fmt::Debug for JitSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JitSlot {{ compiled: {} }}", self.entry.get().is_some())
    }
}

/// Functions compiled into one arena.
const ARENA_FUNCTIONS: usize = 64;

/// A Cranelift module holding up to `ARENA_FUNCTIONS` functions of one
/// generation.
struct Arena {
    id: u32,
    generation: u32,
    module: JITModule,
    functions: usize,
    /// Functions not released yet.
    live: usize,
    /// Bytes of machine code: each function's pages.
    bytes: usize,
}

/// What the compiler thread holds, for the VM to read.
#[derive(Default)]
struct Usage {
    /// Arenas not freed.
    arenas: std::sync::atomic::AtomicUsize,
    /// Their bytes of machine code.
    bytes: std::sync::atomic::AtomicUsize,
}

/// Machine code a function of `ops` instructions is expected to take, and
/// its job.
fn reserve(ops: usize) -> usize {
    (ops * (RESERVE_PER_OP + std::mem::size_of::<Op>())).next_multiple_of(PAGE) + std::mem::size_of::<Job>()
}

/// Bytes of machine code reserved per instruction (`reserve`): compiled,
/// one takes 150 to 250 on average.
const RESERVE_PER_OP: usize = 256;
/// The pages a function's code takes are counted (each is finalized on
/// pages of its own).
const PAGE: usize = 4096;

struct Jit {
    isa: cranelift_codegen::isa::OwnedTargetIsa,
    /// The last of a generation's arenas is where its functions go.
    arenas: Vec<Arena>,
    arena_ids: u32,
    ctx: Context,
    fctx: FunctionBuilderContext,
    usage: std::sync::Arc<Usage>,
}

/// What the VM asks of the compiler thread.
enum Request {
    Compile(Box<Job>),
    /// A function of this arena is dead.
    Release(u32),
}

/// A function to compile. The addresses are only embedded in the generated
/// code; the compiler thread reads nothing but this job.
pub struct Job {
    /// Identifies the job's result (`Done::id`).
    pub id: u64,
    /// Set when the code dies: the job is skipped.
    pub cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The `Code` (stored in `JitCtx::code`).
    pub code: usize,
    /// Its package generation, whose arenas it goes into.
    pub generation: u32,
    pub name: String,
    /// The original instructions, and their address in the VM (slow paths
    /// pass the address of the instruction to run).
    pub ops: Vec<Op>,
    pub ops_addr: usize,
    /// Constant pool bits and address (heap constants are loaded from there,
    /// since the GC moves them).
    pub consts: Vec<u64>,
    pub consts_addr: usize,
    pub frame_size: u16,
    pub nparams: u16,
    pub rest: bool,
    /// Entry points (sorted; pc 0 first if it is one).
    pub heads: Vec<usize>,
    /// The `apply` native, which needs the interpreter.
    pub apply: u64,
    /// Registers each `Closure` instruction's code captures, by code index.
    pub captures: HashMap<u32, Vec<Reg>>,
    /// Globals called by this code that held closures when it was queued;
    /// only calls of those closures get the inline closure-call path,
    /// specialised for their code.
    pub closure_globals: HashMap<u32, Known>,
}

/// A function a call site expects to call.
#[derive(Clone, Copy)]
pub struct Known {
    /// Where the closure the global held is (in `JitSlot::callees`): a call
    /// of that same closure needs no other check.
    pub closure: usize,
    pub code: usize,
    pub nparams: u16,
    pub rest: bool,
    pub frame_size: u16,
}

/// How a call reaches its callee.
enum Target {
    /// This function itself: a direct call.
    Own,
    /// Through the callee's native entry.
    Entry(ir::Value),
}

/// A compiled (or rejected) job.
pub struct Done {
    pub id: u64,
    pub entry: Option<JitFn>,
    /// The arena `entry` is in, to release it with (`Compiler::release`).
    pub arena: u32,
    pub heads: Vec<usize>,
    pub name: String,
    pub ops: usize,
    pub time: std::time::Duration,
}

/// The compiler thread. It owns the executable memory of the code it
/// compiled and frees it when this is dropped, which only the VM's own drop
/// does: compiled code stays installed in the VM's functions until then.
pub struct Compiler {
    jobs: std::sync::mpsc::Sender<Request>,
    done: std::sync::mpsc::Receiver<Done>,
    /// Calls or loop iterations after which a function is compiled; `None`
    /// compiles nothing more.
    pub threshold: Option<u32>,
    /// Wait for each compilation (deterministic, for tests).
    pub sync: bool,
    /// Jobs submitted and not yet installed.
    pub pending: usize,
    usage: std::sync::Arc<Usage>,
}

impl Compiler {
    /// `None` if Cranelift does not support the host. What compiling
    /// allocates is charged to `account`, the world's.
    pub fn new(threshold: u32, sync: bool, account: crate::alloc::AccountRef) -> Option<Compiler> {
        let threshold = Some(threshold.max(1));
        cranelift_native::builder().ok()?;
        let (jobs, job_rx) = std::sync::mpsc::channel::<Request>();
        let (done_tx, done) = std::sync::mpsc::channel();
        let usage = std::sync::Arc::new(Usage::default());
        let shared = usage.clone();
        std::thread::Builder::new()
            .name("techne-jit".into())
            .spawn(move || {
                let Some(mut jit) = ({
                    let _charged = account.enter();
                    Jit::new(shared)
                }) else {
                    return;
                };
                for request in job_rx {
                    let _charged = account.enter();
                    let job = match request {
                        Request::Compile(job) => job,
                        Request::Release(arena) => {
                            jit.release(arena);
                            continue;
                        }
                    };
                    let started = std::time::Instant::now();
                    let cancelled = job.cancelled.load(std::sync::atomic::Ordering::Relaxed);
                    let (entry, arena) = if cancelled { (None, 0) } else { jit.compile(&job) };
                    let done =
                        Done { id: job.id, entry, arena, heads: job.heads, name: job.name, ops: job.ops.len(), time: started.elapsed() };
                    if done_tx.send(done).is_err() {
                        break;
                    }
                }
                // SAFETY: the job channel closes when the `Compiler` drops,
                // with the VM, so none of this code can run any more.
                for arena in jit.arenas.drain(..) {
                    unsafe { arena.module.free_memory() };
                }
            })
            .ok()?;
        Some(Compiler { jobs, done, threshold, sync, pending: 0, usage })
    }

    /// Whether a function of `ops` instructions is compiled within `room`
    /// bytes: compiling is optional, and skipped under pressure.
    pub fn fits(ops: usize, room: usize) -> bool {
        reserve(ops) <= room
    }

    /// Queue `job`; false if the compiler thread is gone (then nothing
    /// more is compiled).
    pub fn submit(&mut self, job: Job) -> bool {
        let sent = self.jobs.send(Request::Compile(Box::new(job))).is_ok();
        if sent {
            self.pending += 1;
        } else {
            self.threshold = None;
        }
        sent
    }

    /// A function of `arena` is dead: nothing runs or calls it any more.
    pub fn release(&mut self, arena: u32) {
        let _ = self.jobs.send(Request::Release(arena));
    }

    /// Arenas of machine code not freed (released functions are freed with
    /// their arena, asynchronously).
    pub fn live_arenas(&self) -> usize {
        self.usage.arenas.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Bytes of machine code not freed: mapped apart, where the allocator
    /// does not count it (`crate::alloc`).
    pub fn held(&self) -> usize {
        self.usage.bytes.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Finished jobs (waiting for one in sync mode).
    pub fn finished(&mut self) -> Vec<Done> {
        let mut out = Vec::new();
        let mut gone = loop {
            match self.done.try_recv() {
                Ok(done) => out.push(done),
                Err(std::sync::mpsc::TryRecvError::Empty) => break false,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break true,
            }
        };
        if self.sync && !gone && out.is_empty() && self.pending > 0 {
            match self.done.recv() {
                Ok(done) => out.push(done),
                Err(_) => gone = true,
            }
        }
        self.pending -= out.len();
        if gone {
            // The compiler thread ended, and with it the jobs it had not
            // finished: nothing more is compiled.
            (self.pending, self.threshold) = (0, None);
        }
        out
    }
}

/// Instructions with native code: `native` ones, plus calls and returns,
/// which exit to the interpreter in the cases they do not handle.
fn compiled(op: &Op) -> bool {
    native(op) || matches!(op, Op::Call { .. } | Op::CallG { .. } | Op::TailCall { .. } | Op::TailCallG { .. } | Op::Ret { .. })
}

/// Instructions native code always executes itself (possible entry points).
pub fn native(op: &Op) -> bool {
    !matches!(
        op,
        Op::PushHandler { .. }
            | Op::PushEscape { .. }
            | Op::Call { .. }
            | Op::CallG { .. }
            | Op::TailCall { .. }
            | Op::TailCallG { .. }
            | Op::Ret { .. }
            | Op::EnterJit
    )
}

unsafe extern "C" fn jit_step(vm: *mut Vm, r: *mut Value, op: *const Op) -> u32 {
    unsafe {
        match (*vm).jit_slow_op(r, *op) {
            Ok(false) => 0,
            Ok(true) => 1,
            Err(e) => {
                (*vm).jit_error = Some(e);
                2
            }
        }
    }
}

/// Call Rust native `f` with the `n` arguments after `regs[base]` and store
/// the result in `regs[base]` (and `regs[also]`). Result codes as for
/// `jit_call_slow`.
unsafe fn call_native(vm: &mut Vm, f: Value, base: usize, n: usize, also: Option<usize>) -> u64 {
    let (regs, globals) = (vm.regs.as_ptr(), vm.globals.as_ptr());
    vm.regs[base] = f;
    vm.stack_top = vm.stack_top.max(base + 1 + n);
    match vm.call_native(f.as_native(), base + 1, n) {
        Ok(v) => {
            vm.regs[base] = v;
            if let Some(i) = also {
                vm.regs[i] = v;
            }
            // Native code holds on to both; leave it if either moved.
            if vm.regs.as_ptr() == regs && vm.globals.as_ptr() == globals { 0 } else { 4 }
        }
        Err(e) => {
            let wait = e.wait.is_some();
            vm.jit_error = Some(e);
            if wait { 3 } else { 2 }
        }
    }
}

/// Run tail calls handed back by a native callee (`TAILCALL`) until it
/// returns or hands over to the interpreter.
unsafe extern "C" fn jit_after_call(vm: *mut Vm, ctx: *mut JitCtx, mut res: u64, frame: *mut Value, bp: u64, depth: u64) -> u64 {
    unsafe {
        while (res >> 32) as u32 == TAILCALL {
            let next: JitFn = std::mem::transmute((*ctx).tail);
            res = next(vm, ctx, frame, bp, 0, depth as u32);
        }
        res
    }
}

/// A non-tail call the inline path does not handle: Rust natives and
/// compiled rest-argument closures. Returns 0 (done, result in the base
/// register), 1 (let the interpreter make the call), 2 (error), 3 (the task
/// must wait), 4 (done, but the register stack or globals moved) or 5 (the
/// callee handed over to the interpreter; its result is in `ctx.res`).
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn jit_call_slow(vm: *mut Vm, ctx: *mut JitCtx, r: *mut Value, bp: u64, base: u64, n: u64, depth: u64, f: u64) -> u64 {
    unsafe {
        let vm = &mut *vm;
        let (f, bp, base, n) = (Value::from_bits(f), bp as usize, base as usize, n as usize);
        if f.is_native() && f != vm.apply_native {
            return call_native(vm, f, bp + base, n, None);
        }
        if !is_kind(f, Kind::Closure) || depth as i64 >= MAX_DEPTH {
            return 1;
        }
        let code = field(f.as_ptr(), 0).as_untraced_ptr::<Code>();
        let c = &*code;
        let callee_bp = bp + base + 1;
        let Some(entry) = c.jit.call_entry.get() else { return 1 };
        if !c.rest || n < c.nparams as usize || callee_bp + (c.frame_size as usize).max(n) + 1 > vm.regs.len() {
            return 1;
        }
        // Count the call like the interpreter; at the end of the slice let it
        // make the call (and suspend).
        let fuel = &mut *(*ctx).fuel;
        if *fuel <= 1 {
            return 1;
        }
        *fuel -= 1;
        *r.add(base) = f;
        vm.enter(code, callee_bp, n).expect("arity checked");
        let frame = r.add(base + 1);
        let res = entry(vm, ctx, frame, callee_bp as u64, 0, depth as u32 + 1);
        let res = jit_after_call(vm, ctx, res, frame, callee_bp as u64, depth + 1);
        if (res >> 32) as u32 == RETURNED {
            0
        } else {
            (*ctx).res = res;
            5
        }
    }
}

/// A tail call the inline path does not handle: Rust natives. Returns 0
/// (the value is in the frame's callee slot: return it), 1 (let the
/// interpreter make the call), 2 (error), 3 (wait) or 4 (returned, but the
/// register stack or globals moved).
unsafe extern "C" fn jit_tail_slow(vm: *mut Vm, bp: u64, base: u64, n: u64, f: u64) -> u64 {
    unsafe {
        let vm = &mut *vm;
        let (f, bp) = (Value::from_bits(f), bp as usize);
        if f.is_native() && f != vm.apply_native {
            return call_native(vm, f, bp + base as usize, n as usize, Some(bp - 1));
        }
        1
    }
}

unsafe extern "C" fn jit_pop_handler(vm: *mut Vm) {
    unsafe { (*vm).jit_pop_handler() }
}

unsafe extern "C" fn jit_unwind_push(vm: *mut Vm, code: *const Code, pc: u64, bp: u64) {
    unsafe { (*vm).jit_push_frame(code, pc as u32, bp as u32) }
}

unsafe extern "C" fn jit_barrier(vm: *mut Vm, obj: *mut u64, v: Value) {
    unsafe { (*vm).write_barrier(obj, v) }
}

const TAG_INT: i64 = 0xFFF9;
const TAG_PTR: i64 = 0xFFFA;
const PAYLOAD: i64 = (1 << 48) - 1;
const CANONICAL_NAN: i64 = 0x7FF8_0000_0000_0000;

/// A set of registers.
#[derive(Clone, PartialEq, Debug)]
struct Regs(Vec<u64>);

impl Regs {
    fn empty(n: usize) -> Regs {
        Regs(vec![0; n.div_ceil(64).max(1)])
    }
    fn full(n: usize) -> Regs {
        let mut s = Regs::empty(n);
        (0..n).for_each(|r| s.insert(r as Reg));
        s
    }
    fn insert(&mut self, r: Reg) {
        self.0[r as usize / 64] |= 1 << (r % 64);
    }
    fn remove(&mut self, r: Reg) {
        self.0[r as usize / 64] &= !(1 << (r % 64));
    }
    fn union(&mut self, o: &Regs) {
        self.0.iter_mut().zip(&o.0).for_each(|(a, b)| *a |= b);
    }
    fn and(&self, o: &Regs) -> Regs {
        Regs(self.0.iter().zip(&o.0).map(|(a, b)| a & b).collect())
    }
    fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(w, bits)| (0..64).filter(move |i| bits & (1 << i) != 0).map(move |i| w * 64 + i))
    }
}

/// Registers an instruction reads and writes. `captures` gives the caller
/// registers a `Closure` instruction captures.
fn uses_defs(op: &Op, captures: &HashMap<u32, Vec<Reg>>) -> (Vec<Reg>, Vec<Reg>) {
    use Op::*;
    match *op {
        LoadK { dst, .. } | LoadI { dst, .. } | GetG { dst, .. } | GetC { dst, .. } | GetCB { dst, .. } => (vec![], vec![dst]),
        Mov { dst, src } | Unbox { dst, r: src } => (vec![src], vec![dst]),
        SetG { src, .. } | SetCB { src, .. } => (vec![src], vec![]),
        MkBox { r } => (vec![r], vec![r]),
        SetBox { r, src } => (vec![r, src], vec![]),
        Closure { dst, code } => (captures.get(&code).cloned().unwrap_or_default(), vec![dst]),
        PushEscape { k, .. } => (vec![], vec![k]),
        Jf { c, .. } | Jt { c, .. } => (vec![c], vec![]),
        JNLt { a, b, .. } | JNLe { a, b, .. } | JNNumEq { a, b, .. } | JNEq { a, b, .. } => (vec![a, b], vec![]),
        JNNull { a, .. } | JNLtI { a, .. } | JNGtI { a, .. } | JNEqI { a, .. } | JNPair { a, .. } => (vec![a], vec![]),
        Add { dst, a, b }
        | Sub { dst, a, b }
        | Mul { dst, a, b }
        | Quo { dst, a, b }
        | Rem { dst, a, b }
        | Mod { dst, a, b }
        | Lt { dst, a, b }
        | Le { dst, a, b }
        | NumEq { dst, a, b }
        | Cons { dst, a, b }
        | EqP { dst, a, b } => (vec![a, b], vec![dst]),
        AddI { dst, a, .. } | Car { dst, a } | Cdr { dst, a } | NullP { dst, a } | PairP { dst, a } | Not { dst, a } => {
            (vec![a], vec![dst])
        }
        VRef { dst, v, i } => (vec![v, i], vec![dst]),
        VSet { v, i, x } => (vec![v, i, x], vec![]),
        Call { base, n } => ((base..=base + n).collect(), vec![base]),
        CallG { base, n, .. } => ((base + 1..=base + n).collect(), vec![base]),
        TailCall { base, n } => ((base..=base + n).collect(), vec![]),
        TailCallG { base, n, .. } => ((base + 1..=base + n).collect(), vec![]),
        Ret { r } => (vec![r], vec![]),
        PushHandler { .. } | PopHandler | Jmp { .. } | Loop { .. } | EnterJit => (vec![], vec![]),
    }
}

/// Where a conditional branch jumps.
pub fn jump_target(op: &Op) -> usize {
    use Op::*;
    match *op {
        Jf { t, .. }
        | Jt { t, .. }
        | JNLt { t, .. }
        | JNLe { t, .. }
        | JNNumEq { t, .. }
        | JNEq { t, .. }
        | JNNull { t, .. }
        | JNLtI { t, .. }
        | JNGtI { t, .. }
        | JNEqI { t, .. }
        | JNPair { t, .. } => t as usize,
        _ => unreachable!("not a branch: {op:?}"),
    }
}

fn successors(op: &Op, pc: usize, len: usize) -> Vec<usize> {
    use Op::*;
    match *op {
        Jmp { t } | Loop { t } => vec![t as usize],
        Jf { t, .. }
        | Jt { t, .. }
        | JNLt { t, .. }
        | JNLe { t, .. }
        | JNNumEq { t, .. }
        | JNEq { t, .. }
        | JNNull { t, .. }
        | JNLtI { t, .. }
        | JNGtI { t, .. }
        | JNEqI { t, .. }
        | JNPair { t, .. } => vec![pc + 1, t as usize],
        Ret { .. } | TailCall { .. } | TailCallG { .. } => vec![],
        _ if pc + 1 < len => vec![pc + 1],
        _ => vec![],
    }
}

/// Which registers native code must keep in memory, per instruction.
struct Flow {
    live_in: Vec<Regs>,
    live_out: Vec<Regs>,
    /// Registers whose machine copy may differ from memory.
    dirty_in: Vec<Regs>,
    /// Registers a `guard` or escape landing in this function may read. A
    /// raise can land there from any instruction, so they count as live
    /// everywhere (they are part of every `live_in`/`live_out`).
    #[allow(dead_code)]
    landing: Regs,
}

fn analyze(ops: &[Op], n: usize, captures: &HashMap<u32, Vec<Reg>>, self_jump: bool) -> Flow {
    let len = ops.len();
    let effects: Vec<_> = ops.iter().map(|op| uses_defs(op, captures)).collect();
    let succs: Vec<_> = ops.iter().enumerate().map(|(pc, op)| successors(op, pc, len)).collect();
    let mut live_in = vec![Regs::empty(n); len];
    let mut live_out = vec![Regs::empty(n); len];
    let mut landing = Regs::empty(n);
    loop {
        let mut changed = false;
        for pc in (0..len).rev() {
            let mut out = landing.clone();
            succs[pc].iter().for_each(|&s| out.union(&live_in[s]));
            let mut inn = out.clone();
            effects[pc].1.iter().for_each(|&d| inn.remove(d));
            effects[pc].0.iter().for_each(|&u| inn.insert(u));
            changed |= inn != live_in[pc];
            live_in[pc] = inn;
            live_out[pc] = out;
        }
        let mut new_landing = Regs::empty(n);
        for op in ops {
            if let Op::PushHandler { dst, t } | Op::PushEscape { dst, t, .. } = *op {
                let mut l = live_in[t as usize].clone();
                l.remove(dst);
                new_landing.union(&l);
            }
        }
        changed |= new_landing != landing;
        landing = new_landing;
        if !changed {
            break;
        }
    }
    // Entry points load their live registers, so they start clean; a self
    // tail call jumps to pc 0 with every register changed.
    let mut dirty_in = vec![Regs::empty(n); len];
    if self_jump {
        dirty_in[0] = Regs::full(n);
    }
    loop {
        let mut changed = false;
        for pc in 0..len {
            let out = match ops[pc] {
                // Calls write registers back and re-read them.
                Op::Call { .. } | Op::CallG { .. } => Regs::empty(n),
                _ => {
                    let mut d = dirty_in[pc].clone();
                    effects[pc].1.iter().for_each(|&r| d.insert(r));
                    d
                }
            };
            for &s in &succs[pc] {
                let before = dirty_in[s].clone();
                dirty_in[s].union(&out);
                changed |= dirty_in[s] != before;
            }
        }
        if !changed {
            break;
        }
    }
    Flow { live_in, live_out, dirty_in, landing }
}

impl Jit {
    /// A JIT for the host, or `None` if Cranelift does not support it.
    fn new(usage: std::sync::Arc<Usage>) -> Option<Jit> {
        let mut flags = settings::builder();
        flags.set("opt_level", "speed").ok()?;
        flags.set("use_colocated_libcalls", "false").ok()?;
        flags.set("is_pic", "false").ok()?;
        flags.set("enable_verifier", "false").ok()?;
        let isa = cranelift_native::builder().ok()?.finish(settings::Flags::new(flags)).ok()?;
        Some(Jit { isa, arenas: Vec::new(), arena_ids: 0, ctx: Context::new(), fctx: FunctionBuilderContext::new(), usage })
    }

    /// The index of the arena to compile `generation`'s code into: its
    /// last, or a new one if that is full.
    fn arena(&mut self, generation: u32) -> usize {
        match self.arenas.iter().rposition(|a| a.generation == generation) {
            Some(i) if self.arenas[i].functions < ARENA_FUNCTIONS => i,
            _ => {
                self.arena_ids += 1;
                let module = JITModule::new(JITBuilder::with_isa(self.isa.clone(), cranelift_module::default_libcall_names()));
                self.arenas.push(Arena { id: self.arena_ids, generation, module, functions: 0, live: 0, bytes: 0 });
                self.usage.arenas.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.arenas.len() - 1
            }
        }
    }

    /// A function of `arena` is dead; free the arena if it was the last.
    fn release(&mut self, arena: u32) {
        let Some(i) = self.arenas.iter().position(|a| a.id == arena) else { return };
        self.arenas[i].live -= 1;
        if self.arenas[i].live == 0 {
            let a = self.arenas.remove(i);
            // Safety: none of its functions runs or is called any more.
            unsafe { a.module.free_memory() };
            self.usage.arenas.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            self.usage.bytes.fetch_sub(a.bytes, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// The function and the arena it is in.
    fn compile(&mut self, job: &Job) -> (Option<JitFn>, u32) {
        let i = self.arena(job.generation);
        let f = self.compile_in(i, job);
        let arena = &mut self.arenas[i];
        let id = arena.id;
        if let Some((_, bytes)) = f {
            (arena.functions, arena.live, arena.bytes) = (arena.functions + 1, arena.live + 1, arena.bytes + bytes);
            self.usage.bytes.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
        } else if arena.live == 0 {
            // Nothing will release it.
            arena.live = 1;
            self.release(id);
        }
        (f.map(|(f, _)| f), id)
    }

    /// The function and the bytes of its pages.
    fn compile_in(&mut self, arena: usize, job: &Job) -> Option<(JitFn, usize)> {
        let module = &mut self.arenas[arena].module;
        let (orig, heads) = (&job.ops[..], &job.heads[..]);
        let n = job.frame_size as usize;
        let effects: Vec<_> = orig.iter().map(|op| uses_defs(op, &job.captures)).collect();
        if effects.iter().flat_map(|(u, d)| u.iter().chain(d)).any(|&r| r as usize >= n) {
            return None;
        }
        module.clear_context(&mut self.ctx);
        let sig = &mut self.ctx.func.signature;
        sig.params.extend([
            AbiParam::new(I64),
            AbiParam::new(I64),
            AbiParam::new(I64),
            AbiParam::new(I64),
            AbiParam::new(I32),
            AbiParam::new(I32),
        ]);
        sig.returns.push(AbiParam::new(I64));
        let jit_sig = sig.clone();
        let call_conv = sig.call_conv;
        let frontend = module.isa().frontend_config();
        let id = module.declare_anonymous_function(&self.ctx.func.signature).ok()?;
        let own = module.declare_func_in_func(id, &mut self.ctx.func);
        let mut b = FunctionBuilder::new(&mut self.ctx.func, &mut self.fctx);
        // Helper signatures: `params` i64 arguments, optionally an i64 result.
        let mut helper = |params: usize, ret: bool| {
            let mut s = Signature::new(call_conv);
            s.params.extend(vec![AbiParam::new(I64); params]);
            if ret {
                s.returns.push(AbiParam::new(I64));
            }
            b.import_signature(s)
        };
        let sigs = Sigs {
            step: helper(3, true),
            barrier: helper(3, false),
            unwind: helper(4, false),
            pop: helper(1, false),
            after: helper(6, true),
            call_slow: helper(8, true),
            tail_slow: helper(5, true),
            jit: b.import_signature(jit_sig),
        };

        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let p = b.block_params(entry).to_vec();
        let vars: Vec<Variable> = (0..n).map(|_| b.declare_var(I64)).collect();
        let load_ctx = |b: &mut FunctionBuilder, off: usize| b.ins().load(I64, flags(), p[1], off as i32);
        let globals = load_ctx(&mut b, offset_of!(JitCtx, globals));
        let fuel = load_ctx(&mut b, offset_of!(JitCtx, fuel));
        let regs_end = load_ctx(&mut b, offset_of!(JitCtx, regs_end));
        let depth = b.ins().uextend(I64, p[5]);
        let self_jump = heads.contains(&0) && orig.iter().any(|op| matches!(op, Op::TailCall { .. } | Op::TailCallG { .. }));
        let mut g = Gen {
            vm: p[0],
            ctx: p[1],
            r: p[2],
            bp: p[3],
            depth,
            globals,
            fuel,
            regs_end,
            code_ptr: job.code as i64,
            entry0: heads.contains(&0),
            flow: analyze(orig, n, &job.captures, self_jump),
            vars,
            blocks: orig.iter().map(|op| compiled(op).then(|| b.create_block())).collect(),
            exits: HashMap::new(),
            sigs,
            closure_globals: job.closure_globals.clone(),
            own,
            consts: job.consts_addr as i64,
            const_bits: job.consts.clone(),
        };
        // Dead registers start as 0; each entry point loads its live ones.
        let zero = b.ins().iconst(I64, 0);
        g.vars.iter().for_each(|v| b.def_var(*v, zero));
        let trap = b.create_block();
        let max = heads.iter().max().copied().unwrap_or(0);
        let loaders: HashMap<usize, Block> = heads.iter().map(|&h| (h, b.create_block())).collect();
        let table: Vec<_> = (0..=max).map(|pc| b.func.dfg.block_call(loaders.get(&pc).copied().unwrap_or(trap), &[])).collect();
        let default = b.func.dfg.block_call(trap, &[]);
        let jt = b.create_jump_table(JumpTableData::new(default, &table));
        // Calls enter at pc 0: straight to its loader, without the table.
        if let Some(&start) = loaders.get(&0) {
            let dispatch = b.create_block();
            b.ins().brif(p[4], dispatch, &[], start, &[]);
            b.switch_to_block(dispatch);
        }
        b.ins().br_table(p[4], jt);
        b.switch_to_block(trap);
        b.ins().trap(TrapCode::unwrap_user(1));
        for (&h, &block) in &loaders {
            b.switch_to_block(block);
            if h == 0 {
                // Prologue: callers only place the arguments. Clear the rest
                // of the frame (stale values above the GC's root window may
                // dangle) and include it in that window.
                let first = job.nparams as usize + job.rest as usize;
                (first..n).for_each(|i| {
                    b.ins().store(flags(), zero, g.r, (i * 8) as i32);
                });
                let top = b.ins().iadd_imm_s(g.bp, n as i64);
                g.raise_stack_top(&mut b, top);
            }
            g.reload(&mut b, &g.flow.live_in[h].clone());
            b.ins().jump(g.blocks[h].unwrap(), &[]);
        }

        for (pc, op) in orig.iter().enumerate() {
            if let Some(block) = g.blocks[pc] {
                b.switch_to_block(block);
                g.op(&mut b, pc, op, (job.ops_addr + pc * std::mem::size_of::<Op>()) as i64);
            }
        }
        let exits: Vec<_> = g.exits.iter().map(|(k, v)| (*k, *v)).collect();
        for ((pc, status), block) in exits {
            b.switch_to_block(block);
            if matches!(status, EXIT | TICK | STEP) {
                g.spill_at(&mut b, pc);
            }
            let code = b.ins().iconst(I64, g.code_ptr);
            b.ins().store(flags(), code, g.ctx, offset_of!(JitCtx, code) as i32);
            b.ins().store(flags(), g.bp, g.ctx, offset_of!(JitCtx, bp) as i32);
            let v = b.ins().iconst(I64, ((status as i64) << 32) | pc as i64);
            b.ins().return_(&[v]);
        }
        b.seal_all_blocks();
        b.finalize(frontend);
        if std::env::var_os("TECHNE_JIT_IR").is_some() {
            let f = &self.ctx.func;
            let insts: usize = f.layout.blocks().map(|bl| f.layout.block_insts(bl).count()).sum();
            eprintln!("ir: {} {} ops -> {} blocks, {insts} insts", job.name, orig.len(), f.layout.blocks().count());
            if std::env::var("TECHNE_JIT_IR").is_ok_and(|v| v == job.name) {
                eprintln!("{}", f.display());
            }
        }

        module.define_function(id, &mut self.ctx).ok()?;
        let bytes = self.ctx.compiled_code().map_or(0, |c| c.code_info().total_size as usize).next_multiple_of(PAGE);
        module.clear_context(&mut self.ctx);
        module.finalize_definitions().ok()?;
        let f = module.get_finalized_function(id);
        Some((unsafe { std::mem::transmute::<*const u8, JitFn>(f) }, bytes))
    }
}

struct Sigs {
    step: ir::SigRef,
    barrier: ir::SigRef,
    unwind: ir::SigRef,
    pop: ir::SigRef,
    after: ir::SigRef,
    call_slow: ir::SigRef,
    tail_slow: ir::SigRef,
    /// Compiled functions.
    jit: ir::SigRef,
}

struct Gen {
    vm: ir::Value,
    ctx: ir::Value,
    r: ir::Value,
    bp: ir::Value,
    depth: ir::Value,
    globals: ir::Value,
    fuel: ir::Value,
    regs_end: ir::Value,
    code_ptr: i64,
    flow: Flow,
    /// pc 0 is an entry point (its block starts the function).
    entry0: bool,
    vars: Vec<Variable>,
    /// Native instruction blocks by pc.
    blocks: Vec<Option<Block>>,
    /// Exit blocks by (pc, status).
    exits: HashMap<(usize, u32), Block>,
    sigs: Sigs,
    closure_globals: HashMap<u32, Known>,
    /// This function, for direct recursive calls.
    own: ir::FuncRef,
    consts: i64,
    const_bits: Vec<u64>,
}

fn flags() -> MemFlagsData {
    MemFlagsData::trusted()
}

impl Gen {
    fn spill(&self, b: &mut FunctionBuilder, regs: &Regs) {
        for i in regs.iter() {
            let x = b.use_var(self.vars[i]);
            b.ins().store(flags(), x, self.r, (i * 8) as i32);
        }
    }

    /// Before leaving native code or running code that reads registers or
    /// may collect, at instruction `pc`: store the changed registers that are
    /// still needed (live sets include what `guard` landings read).
    fn spill_at(&self, b: &mut FunctionBuilder, pc: usize) {
        self.spill(b, &self.flow.dirty_in[pc].and(&self.flow.live_in[pc]));
    }

    fn reload(&self, b: &mut FunctionBuilder, regs: &Regs) {
        for i in regs.iter() {
            let x = b.ins().load(I64, flags(), self.r, (i * 8) as i32);
            b.def_var(self.vars[i], x);
        }
    }

    /// After instruction `pc` ran outside native code: re-read the registers
    /// needed afterwards (the GC may have moved what they point to).
    fn reload_after(&self, b: &mut FunctionBuilder, pc: usize) {
        self.reload(b, &self.flow.live_out[pc]);
    }

    fn exit(&mut self, b: &mut FunctionBuilder, pc: usize, status: u32) -> Block {
        *self.exits.entry((pc, status)).or_insert_with(|| b.create_block())
    }

    /// Leave native code to run instruction `pc` in Rust (its rare cases).
    fn step(&mut self, b: &mut FunctionBuilder, pc: usize) -> Block {
        self.exit(b, pc, STEP)
    }

    /// The block that executes instruction `pc`.
    fn target(&mut self, b: &mut FunctionBuilder, pc: usize) -> Block {
        match self.blocks.get(pc).copied().flatten() {
            Some(block) => block,
            None => self.exit(b, pc, EXIT),
        }
    }

    fn get(&self, b: &mut FunctionBuilder, r: Reg) -> ir::Value {
        b.use_var(self.vars[r as usize])
    }

    fn set(&self, b: &mut FunctionBuilder, r: Reg, v: ir::Value) {
        b.def_var(self.vars[r as usize], v);
    }

    fn jump(&mut self, b: &mut FunctionBuilder, pc: usize) {
        let t = self.target(b, pc);
        b.ins().jump(t, &[]);
    }

    /// Branch to instruction `yes` if `c`, else to `no`.
    fn branch(&mut self, b: &mut FunctionBuilder, c: ir::Value, yes: usize, no: usize) {
        let (y, n) = (self.target(b, yes), self.target(b, no));
        b.ins().brif(c, y, &[], n, &[]);
    }

    /// Continue in a new block if `c`, else go to `no`.
    fn guard(b: &mut FunctionBuilder, c: ir::Value, no: Block) {
        let yes = b.create_block();
        b.ins().brif(c, yes, &[], no, &[]);
        b.switch_to_block(yes);
    }

    fn call(&self, b: &mut FunctionBuilder, sig: ir::SigRef, f: *const (), args: &[ir::Value]) -> Option<ir::Value> {
        let f = b.ins().iconst(I64, f as i64);
        let inst = b.ins().call_indirect(sig, f, args);
        b.inst_results(inst).first().copied()
    }

    /// Count a call or back-edge; when the task's slice is used up, exit
    /// with `TICK` to resume at `resume`.
    fn tick(&mut self, b: &mut FunctionBuilder, resume: usize) {
        let f = b.ins().load(I32, flags(), self.fuel, 0);
        let f = b.ins().iadd_imm_s(f, -1);
        b.ins().store(flags(), f, self.fuel, 0);
        let left = b.ins().icmp_imm_s(IntCC::NotEqual, f, 0);
        let tick = self.exit(b, resume, TICK);
        Self::guard(b, left, tick);
    }

    /// For a call of `f`, expected to be the closure `k`, with `n` arguments
    /// and a frame at `frame`: continue if it is and the frame fits, else go
    /// to `slow` (also when the global now holds another procedure). `None`
    /// (after jumping to `slow`) if `k` does not take `n` arguments.
    fn known_callee(&mut self, b: &mut FunctionBuilder, f: ir::Value, k: Known, n: u16, frame: ir::Value, slow: Block) -> Option<Target> {
        if k.rest || k.nparams != n {
            b.ins().jump(slow, &[]);
            return None;
        }
        let addr = b.ins().iconst(I64, k.closure as i64);
        let expected = b.ins().load(I64, flags(), addr, 0);
        let same = b.ins().icmp(IntCC::Equal, f, expected);
        Self::guard(b, same, slow);
        let end = b.ins().iadd_imm_s(frame, k.frame_size as i64 * 8);
        let fits = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, end, self.regs_end);
        Self::guard(b, fits, slow);
        if k.code as i64 == self.code_ptr && self.entry0 {
            return Some(Target::Own);
        }
        let slot = offset_of!(Code, jit) + offset_of!(JitSlot, call_entry);
        let addr = b.ins().iconst(I64, (k.code + slot) as i64);
        let entry = b.ins().load(I64, flags(), addr, 0);
        let compiled = b.ins().icmp_imm_s(IntCC::NotEqual, entry, 0);
        Self::guard(b, compiled, slow);
        Some(Target::Entry(entry))
    }

    /// For a call of `f` with `n` arguments whose frame starts at `frame`:
    /// continue if `f` is a compiled closure taking exactly `n` arguments
    /// whose frame fits the register stack, else go to `no`. Returns the
    /// callee's code and native entry.
    fn callee(&mut self, b: &mut FunctionBuilder, f: ir::Value, n: u16, frame: ir::Value, no: Block) -> (ir::Value, ir::Value) {
        let p = Self::check_kind(b, f, Kind::Closure, no);
        let w = b.ins().load(I64, flags(), p, 8);
        let code = Self::ptr(b, w);
        let slot = offset_of!(Code, jit) + offset_of!(JitSlot, call_entry);
        let entry = b.ins().load(I64, flags(), code, slot as i32);
        let np = b.ins().uload16(I64, flags(), code, offset_of!(Code, nparams) as i32);
        let rest = b.ins().uload8(I64, flags(), code, offset_of!(Code, rest) as i32);
        let size = b.ins().uload16(I64, flags(), code, offset_of!(Code, frame_size) as i32);
        let has_entry = b.ins().icmp_imm_s(IntCC::NotEqual, entry, 0);
        let exact = b.ins().icmp_imm_s(IntCC::Equal, np, n as i64);
        let no_rest = b.ins().icmp_imm_s(IntCC::Equal, rest, 0);
        let bytes = b.ins().ishl_imm_s(size, 3);
        let end = b.ins().iadd(frame, bytes);
        let fits = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, end, self.regs_end);
        let ok = b.ins().band(has_entry, exact);
        let ok = b.ins().band(ok, no_rest);
        let ok = b.ins().band(ok, fits);
        Self::guard(b, ok, no);
        (code, entry)
    }

    /// `*stack_top = max(*stack_top, top)`.
    fn raise_stack_top(&self, b: &mut FunctionBuilder, top: ir::Value) {
        let p = b.ins().load(I64, flags(), self.ctx, offset_of!(JitCtx, stack_top) as i32);
        let old = b.ins().load(I64, flags(), p, 0);
        let new = b.ins().umax(old, top);
        b.ins().store(flags(), new, p, 0);
    }

    /// Inline nursery allocation of `words` words with `header`; goes to
    /// `slow` when the nursery is full or allocation must go through the VM.
    fn alloc(&self, b: &mut FunctionBuilder, words: i64, header: u64, slow: Block) -> ir::Value {
        let top_ptr = b.ins().load(I64, flags(), self.ctx, offset_of!(JitCtx, heap_top) as i32);
        let inline = b.ins().icmp_imm_s(IntCC::NotEqual, top_ptr, 0);
        Self::guard(b, inline, slow);
        let top = b.ins().load(I64, flags(), top_ptr, 0);
        let end = b.ins().load(I64, flags(), self.ctx, offset_of!(JitCtx, heap_end) as i32);
        let new = b.ins().iadd_imm_s(top, words * 8);
        let room = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, new, end);
        Self::guard(b, room, slow);
        b.ins().store(flags(), new, top_ptr, 0);
        let h = b.ins().iconst(I64, header as i64);
        b.ins().store(flags(), h, top, 0);
        top
    }

    fn tag_ptr(b: &mut FunctionBuilder, p: ir::Value) -> ir::Value {
        b.ins().bor_imm_s(p, TAG_PTR << 48)
    }

    fn imm(b: &mut FunctionBuilder, v: Value) -> ir::Value {
        b.ins().iconst(I64, v.bits() as i64)
    }

    fn is_int(b: &mut FunctionBuilder, x: ir::Value) -> ir::Value {
        let tag = b.ins().ushr_imm_s(x, 48);
        b.ins().icmp_imm_s(IntCC::Equal, tag, TAG_INT)
    }

    fn is_float(b: &mut FunctionBuilder, x: ir::Value) -> ir::Value {
        b.ins().icmp_imm_s(IntCC::UnsignedLessThan, x, TAG_INT << 48)
    }

    fn is_ptr(b: &mut FunctionBuilder, x: ir::Value) -> ir::Value {
        let tag = b.ins().ushr_imm_s(x, 48);
        b.ins().icmp_imm_s(IntCC::Equal, tag, TAG_PTR)
    }

    /// Sign-extend the 48-bit payload.
    fn untag(b: &mut FunctionBuilder, x: ir::Value) -> ir::Value {
        let s = b.ins().ishl_imm_s(x, 16);
        b.ins().sshr_imm_s(s, 16)
    }

    fn tag_int(b: &mut FunctionBuilder, i: ir::Value) -> ir::Value {
        let p = b.ins().band_imm_s(i, PAYLOAD);
        b.ins().bor_imm_s(p, TAG_INT << 48)
    }

    /// A fixnum's payload in the high 48 bits: its value times 2^16. Sums,
    /// differences and products (by an untagged factor) of these overflow
    /// exactly when the fixnum result would not fit.
    fn shifted(b: &mut FunctionBuilder, x: ir::Value) -> ir::Value {
        b.ins().ishl_imm_s(x, 16)
    }

    fn tag_shifted(b: &mut FunctionBuilder, s: ir::Value) -> ir::Value {
        let p = b.ins().ushr_imm_s(s, 16);
        b.ins().bor_imm_s(p, TAG_INT << 48)
    }

    fn fits48(b: &mut FunctionBuilder, i: ir::Value) -> ir::Value {
        let s = Self::untag(b, i);
        b.ins().icmp(IntCC::Equal, s, i)
    }

    fn ptr(b: &mut FunctionBuilder, x: ir::Value) -> ir::Value {
        b.ins().band_imm_s(x, PAYLOAD)
    }

    /// A fixnum or float as an `f64` (callers check it is one of the two).
    fn to_f(b: &mut FunctionBuilder, x: ir::Value) -> ir::Value {
        let as_float = b.ins().bitcast(F64, MemFlagsData::new(), x);
        let i = Self::untag(b, x);
        let from_int = b.ins().fcvt_from_sint(F64, i);
        let int = Self::is_int(b, x);
        b.ins().select(int, from_int, as_float)
    }

    fn from_f(b: &mut FunctionBuilder, f: ir::Value) -> ir::Value {
        let bits = b.ins().bitcast(I64, MemFlagsData::new(), f);
        let nan = b.ins().fcmp(FloatCC::Unordered, f, f);
        let canonical = b.ins().iconst(I64, CANONICAL_NAN);
        b.ins().select(nan, canonical, bits)
    }

    /// Two operands that are not both fixnums, as `f64`s: both floats
    /// directly, mixed fixnum/float by conversion; anything else goes to
    /// `slow`. Continues in a new block.
    fn floats(&self, b: &mut FunctionBuilder, x: ir::Value, y: ir::Value, slow: Block) -> (ir::Value, ir::Value) {
        let done = b.create_block();
        b.append_block_param(done, F64);
        b.append_block_param(done, F64);
        let mixed = b.create_block();
        let (xf, yf) = (Self::is_float(b, x), Self::is_float(b, y));
        let both = b.ins().band(xf, yf);
        let direct = b.create_block();
        b.ins().brif(both, direct, &[], mixed, &[]);
        b.switch_to_block(direct);
        let fa = b.ins().bitcast(F64, MemFlagsData::new(), x);
        let fb = b.ins().bitcast(F64, MemFlagsData::new(), y);
        b.ins().jump(done, &[BlockArg::Value(fa), BlockArg::Value(fb)]);
        b.switch_to_block(mixed);
        let (xn, yn) = (Self::is_number(b, x), Self::is_number(b, y));
        let numbers = b.ins().band(xn, yn);
        Self::guard(b, numbers, slow);
        let (fa, fb) = (Self::to_f(b, x), Self::to_f(b, y));
        b.ins().jump(done, &[BlockArg::Value(fa), BlockArg::Value(fb)]);
        b.switch_to_block(done);
        let p = b.block_params(done);
        (p[0], p[1])
    }

    fn is_number(b: &mut FunctionBuilder, x: ir::Value) -> ir::Value {
        let int = Self::is_int(b, x);
        let float = Self::is_float(b, x);
        b.ins().bor(int, float)
    }

    fn bool(b: &mut FunctionBuilder, c: ir::Value) -> ir::Value {
        let (t, f) = (Self::imm(b, Value::TRUE), Self::imm(b, Value::FALSE));
        b.ins().select(c, t, f)
    }

    /// Continue if `x` is a heap object of `kind`, else go to `no`. Returns
    /// the object's address.
    fn check_kind(b: &mut FunctionBuilder, x: ir::Value, kind: Kind, no: Block) -> ir::Value {
        Self::check_header(b, x, kind, 0xFF, no)
    }

    /// `check_kind` for an object to change: literals go to `no` too, at
    /// no extra cost (the same test, with the immutable flag in its mask).
    fn check_changeable(b: &mut FunctionBuilder, x: ir::Value, kind: Kind, no: Block) -> ir::Value {
        Self::check_header(b, x, kind, 0xFF | crate::heap::IMMUTABLE as i64, no)
    }

    fn check_header(b: &mut FunctionBuilder, x: ir::Value, kind: Kind, mask: i64, no: Block) -> ir::Value {
        let ptr = Self::is_ptr(b, x);
        Self::guard(b, ptr, no);
        let p = Self::ptr(b, x);
        let h = b.ins().load(I64, flags(), p, 0);
        let k = b.ins().band_imm_s(h, mask);
        let ok = b.ins().icmp_imm_s(IntCC::Equal, k, kind as i64);
        Self::guard(b, ok, no);
        p
    }

    fn barrier(&self, b: &mut FunctionBuilder, obj: ir::Value, v: ir::Value) {
        let call = b.create_block();
        let done = b.create_block();
        let ptr = Self::is_ptr(b, v);
        b.ins().brif(ptr, call, &[], done, &[]);
        b.switch_to_block(call);
        self.call(b, self.sigs.barrier, jit_barrier as *const (), &[self.vm, obj, v]);
        b.ins().jump(done, &[]);
        b.switch_to_block(done);
    }

    /// Arithmetic: fixnums (overflow steps out), else numbers as floats.
    fn arith(&mut self, b: &mut FunctionBuilder, pc: usize, dst: Reg, x: ir::Value, y: ir::Value, kind: char) {
        let slow = self.step(b, pc);
        let int = b.create_block();
        let float = b.create_block();
        let (xi, yi) = (Self::is_int(b, x), Self::is_int(b, y));
        let both = b.ins().band(xi, yi);
        b.ins().brif(both, int, &[], float, &[]);

        b.switch_to_block(int);
        let a = Self::shifted(b, x);
        let (s, overflow) = match kind {
            '+' => {
                let c = Self::shifted(b, y);
                b.ins().sadd_overflow(a, c)
            }
            '-' => {
                let c = Self::shifted(b, y);
                b.ins().ssub_overflow(a, c)
            }
            _ => {
                let c = Self::untag(b, y);
                b.ins().smul_overflow(a, c)
            }
        };
        let ok = b.create_block();
        b.ins().brif(overflow, slow, &[], ok, &[]);
        b.switch_to_block(ok);
        let v = Self::tag_shifted(b, s);
        self.set(b, dst, v);
        self.jump(b, pc + 1);

        b.switch_to_block(float);
        let (fa, fb) = self.floats(b, x, y, slow);
        let f = match kind {
            '+' => b.ins().fadd(fa, fb),
            '-' => b.ins().fsub(fa, fb),
            _ => b.ins().fmul(fa, fb),
        };
        let v = Self::from_f(b, f);
        self.set(b, dst, v);
        self.jump(b, pc + 1);
    }

    /// A numeric comparison; continues in a block whose parameter is the
    /// result (an `I8`). Non-numbers step out.
    fn compare(&mut self, b: &mut FunctionBuilder, pc: usize, x: ir::Value, y: ir::Value, cc: (IntCC, FloatCC)) {
        let slow = self.step(b, pc);
        let result = b.create_block();
        b.append_block_param(result, I8);
        let int = b.create_block();
        let float = b.create_block();
        let (xi, yi) = (Self::is_int(b, x), Self::is_int(b, y));
        let both = b.ins().band(xi, yi);
        b.ins().brif(both, int, &[], float, &[]);
        b.switch_to_block(int);
        let (a, c) = (b.ins().ishl_imm_s(x, 16), b.ins().ishl_imm_s(y, 16));
        let r = b.ins().icmp(cc.0, a, c);
        b.ins().jump(result, &[BlockArg::Value(r)]);
        b.switch_to_block(float);
        let (fa, fb) = self.floats(b, x, y, slow);
        let r = b.ins().fcmp(cc.1, fa, fb);
        b.ins().jump(result, &[BlockArg::Value(r)]);
        b.switch_to_block(result);
    }

    fn cmp_ops(op: &Op) -> (IntCC, FloatCC) {
        match op {
            Op::Le { .. } | Op::JNLe { .. } => (IntCC::SignedLessThanOrEqual, FloatCC::LessThanOrEqual),
            Op::NumEq { .. } | Op::JNNumEq { .. } | Op::JNEqI { .. } => (IntCC::Equal, FloatCC::Equal),
            _ => (IntCC::SignedLessThan, FloatCC::LessThan),
        }
    }

    /// Store `f` and continue with the result of the callee or interpreter
    /// call made at `pc` in `done`; otherwise leave native code.
    fn call_op(&mut self, b: &mut FunctionBuilder, pc: usize, op: &Op) {
        let next = pc + 1;
        let (base, n, global) = match *op {
            Op::CallG { base, n, g } => (base, n, Some(g)),
            Op::Call { base, n } => (base, n, None),
            _ => unreachable!(),
        };
        let f = match global {
            Some(g) => b.ins().load(I64, flags(), self.globals, (g * 8) as i32),
            None => self.get(b, base),
        };
        let slow = b.create_block();
        let done = b.create_block();
        let bail = b.create_block();
        b.append_block_param(bail, I64);
        // Calls through globals that hold no closure skip the inline path.
        let known = global.and_then(|g| self.closure_globals.get(&g).copied());
        if global.is_none() || known.is_some() {
            self.inline_call(b, pc, base, n, f, known, slow, done, bail);
        } else {
            b.ins().jump(slow, &[]);
        }

        // Rust natives, rest arguments: see `jit_call_slow`.
        b.switch_to_block(slow);
        self.spill_at(b, pc);
        let (base_v, n_v) = (b.ins().iconst(I64, base as i64), b.ins().iconst(I64, n as i64));
        let k = self
            .call(b, self.sigs.call_slow, jit_call_slow as *const (), &[self.vm, self.ctx, self.r, self.bp, base_v, n_v, self.depth, f])
            .unwrap();
        let handed_over = b.create_block();
        let calls =
            [done, self.exit(b, pc, EXIT), self.exit(b, next, ERROR), self.exit(b, next, WAIT), self.exit(b, next, RESUME), handed_over]
                .map(|blk| b.func.dfg.block_call(blk, &[]));
        let k = b.ins().ireduce(I32, k);
        let jt = b.create_jump_table(JumpTableData::new(calls[5], &calls[..5]));
        b.ins().br_table(k, jt);
        b.switch_to_block(handed_over);
        let res = b.ins().load(I64, flags(), self.ctx, offset_of!(JitCtx, res) as i32);
        b.ins().jump(bail, &[BlockArg::Value(res)]);

        // The callee handed over to the interpreter: record this frame.
        b.switch_to_block(bail);
        let res = b.block_params(bail)[0];
        let code = b.ins().iconst(I64, self.code_ptr);
        let ret_pc = b.ins().iconst(I64, next as i64);
        self.call(b, self.sigs.unwind, jit_unwind_push as *const (), &[self.vm, code, ret_pc, self.bp]);
        b.ins().return_(&[res]);

        b.switch_to_block(done);
        self.reload_after(b, pc);
        self.jump(b, next);
    }

    /// The inline path of a call to a compiled closure with matching arity;
    /// anything else goes to `slow`.
    #[allow(clippy::too_many_arguments)]
    fn inline_call(
        &mut self,
        b: &mut FunctionBuilder,
        pc: usize,
        base: Reg,
        n: u16,
        f: ir::Value,
        known: Option<Known>,
        slow: Block,
        done: Block,
        bail: Block,
    ) {
        let frame = b.ins().iadd_imm_s(self.r, (base as i64 + 1) * 8);
        let shallow = b.ins().icmp_imm_s(IntCC::SignedLessThan, self.depth, MAX_DEPTH);
        Self::guard(b, shallow, slow);
        let target = match known {
            Some(k) => match self.known_callee(b, f, k, n, frame, slow) {
                Some(t) => t,
                None => return,
            },
            None => Target::Entry(self.callee(b, f, n, frame, slow).1),
        };
        self.tick(b, pc);
        self.spill_at(b, pc);
        if known.is_some() {
            b.ins().store(flags(), f, self.r, (base as i32) * 8);
        }
        let callee_bp = b.ins().iadd_imm_s(self.bp, base as i64 + 1);
        let depth = b.ins().iadd_imm_s(self.depth, 1);
        let depth32 = b.ins().ireduce(I32, depth);
        let zero = b.ins().iconst(I32, 0);
        let args = [self.vm, self.ctx, frame, callee_bp, zero, depth32];
        let inst = match target {
            Target::Own => b.ins().call(self.own, &args),
            Target::Entry(entry) => b.ins().call_indirect(self.sigs.jit, entry, &args),
        };
        let res = b.inst_results(inst)[0];
        let status = b.ins().ushr_imm_s(res, 32);
        let returned = b.ins().icmp_imm_s(IntCC::Equal, status, RETURNED as i64);
        let after = b.create_block();
        b.ins().brif(returned, done, &[], after, &[]);
        // A tail call in the callee, or a hand-over to the interpreter.
        b.switch_to_block(after);
        let res = self.call(b, self.sigs.after, jit_after_call as *const (), &[self.vm, self.ctx, res, frame, callee_bp, depth]).unwrap();
        let status = b.ins().ushr_imm_s(res, 32);
        let returned = b.ins().icmp_imm_s(IntCC::Equal, status, RETURNED as i64);
        b.ins().brif(returned, done, &[], bail, &[BlockArg::Value(res)]);
    }

    fn tail_call_op(&mut self, b: &mut FunctionBuilder, pc: usize, op: &Op) {
        let (base, n, global) = match *op {
            Op::TailCallG { base, n, g } => (base, n, Some(g)),
            Op::TailCall { base, n } => (base, n, None),
            _ => unreachable!(),
        };
        let f = match global {
            Some(g) => b.ins().load(I64, flags(), self.globals, (g * 8) as i32),
            None => self.get(b, base),
        };
        let slow = b.create_block();
        let known = global.and_then(|g| self.closure_globals.get(&g).copied());
        if global.is_none() || known.is_some() {
            self.inline_tail_call(b, pc, base, n, f, known, slow);
        } else {
            b.ins().jump(slow, &[]);
        }
        self.tail_slow(b, pc, base, n, f, slow);
    }

    /// The inline path of a tail call to a compiled closure with matching
    /// arity; anything else goes to `slow`.
    fn inline_tail_call(&mut self, b: &mut FunctionBuilder, pc: usize, base: Reg, n: u16, f: ir::Value, known: Option<Known>, slow: Block) {
        let (code, target) = match known {
            Some(k) => match self.known_callee(b, f, k, n, self.r, slow) {
                Some(t) => (b.ins().iconst(I64, k.code as i64), t),
                None => return,
            },
            None => {
                let (code, entry) = self.callee(b, f, n, self.r, slow);
                (code, Target::Entry(entry))
            }
        };
        self.tick(b, pc);
        // A tail call to this same code is a jump to its start (the closure
        // may differ: same code, other captured values).
        if let Some(start) = self.blocks[0].filter(|_| self.entry0) {
            let jump = b.create_block();
            let other = b.create_block();
            match target {
                Target::Own => {
                    b.ins().jump(jump, &[]);
                }
                Target::Entry(_) => {
                    let same = b.ins().icmp_imm_s(IntCC::Equal, code, self.code_ptr);
                    b.ins().brif(same, jump, &[], other, &[]);
                }
            }
            b.switch_to_block(jump);
            b.ins().store(flags(), f, self.r, -8);
            let args: Vec<_> = (0..n).map(|i| self.get(b, base + 1 + i)).collect();
            let zero = b.ins().iconst(I64, 0);
            for (i, v) in self.vars.clone().into_iter().enumerate() {
                b.def_var(v, args.get(i).copied().unwrap_or(zero));
            }
            b.ins().jump(start, &[]);
            b.switch_to_block(other);
        }
        // The callee takes over this frame: closure, then arguments; its
        // prologue clears the rest.
        b.ins().store(flags(), f, self.r, -8);
        for i in 0..n {
            let a = self.get(b, base + 1 + i);
            b.ins().store(flags(), a, self.r, (i as i32) * 8);
        }
        let entry = match target {
            Target::Entry(entry) => entry,
            // Unreachable (handled by the jump above); keeps the IR well-formed.
            Target::Own => b.ins().iconst(I64, 0),
        };
        b.ins().store(flags(), code, self.ctx, offset_of!(JitCtx, code) as i32);
        b.ins().store(flags(), self.bp, self.ctx, offset_of!(JitCtx, bp) as i32);
        b.ins().store(flags(), entry, self.ctx, offset_of!(JitCtx, tail) as i32);
        let res = b.ins().iconst(I64, (TAILCALL as i64) << 32);
        b.ins().return_(&[res]);
    }

    /// Tail calls of Rust natives: see `jit_tail_slow`.
    fn tail_slow(&mut self, b: &mut FunctionBuilder, pc: usize, base: Reg, n: u16, f: ir::Value, slow: Block) {
        let next = pc + 1;
        b.switch_to_block(slow);
        self.spill_at(b, pc);
        let (base_v, n_v) = (b.ins().iconst(I64, base as i64), b.ins().iconst(I64, n as i64));
        let k = self.call(b, self.sigs.tail_slow, jit_tail_slow as *const (), &[self.vm, self.bp, base_v, n_v, f]).unwrap();
        let returned = b.create_block();
        let calls = [returned, self.exit(b, pc, EXIT), self.exit(b, next, ERROR), self.exit(b, next, WAIT), self.exit(b, next, RET_MOVED)]
            .map(|blk| b.func.dfg.block_call(blk, &[]));
        let k = b.ins().ireduce(I32, k);
        let jt = b.create_jump_table(JumpTableData::new(calls[4], &calls[..4]));
        b.ins().br_table(k, jt);
        b.switch_to_block(returned);
        let res = b.ins().iconst(I64, (RETURNED as i64) << 32);
        b.ins().return_(&[res]);
    }

    fn op(&mut self, b: &mut FunctionBuilder, pc: usize, op: &Op, op_addr: i64) {
        let next = pc + 1;
        match *op {
            Op::LoadK { dst, k } => {
                let c = Value::from_bits(self.const_bits[k as usize]);
                // Heap constants move with the GC: load them from the constant pool.
                let v = if c.is_ptr() {
                    let base = b.ins().iconst(I64, self.consts);
                    b.ins().load(I64, flags(), base, (k * 8) as i32)
                } else {
                    Self::imm(b, c)
                };
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::LoadI { dst, i } => {
                let v = Self::imm(b, Value::int_unchecked(i as i64));
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::Mov { dst, src } => {
                let v = self.get(b, src);
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::GetG { dst, g } => {
                let v = b.ins().load(I64, flags(), self.globals, (g * 8) as i32);
                let bound = b.ins().icmp_imm_s(IntCC::NotEqual, v, Value::UNDEFINED.bits() as i64);
                let slow = self.step(b, pc);
                Self::guard(b, bound, slow);
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::SetG { g, src } => {
                let v = self.get(b, src);
                b.ins().store(flags(), v, self.globals, (g * 8) as i32);
                self.jump(b, next);
            }
            Op::GetC { dst, i } | Op::GetCB { dst, i } => {
                let c = b.ins().load(I64, flags(), self.r, -8);
                let p = Self::ptr(b, c);
                let mut v = b.ins().load(I64, flags(), p, (8 * (2 + i as u32)) as i32);
                if matches!(op, Op::GetCB { .. }) {
                    let bx = Self::ptr(b, v);
                    v = b.ins().load(I64, flags(), bx, 8);
                }
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::SetCB { i, src } => {
                let c = b.ins().load(I64, flags(), self.r, -8);
                let p = Self::ptr(b, c);
                let bx = b.ins().load(I64, flags(), p, (8 * (2 + i as u32)) as i32);
                let bx = Self::ptr(b, bx);
                let v = self.get(b, src);
                b.ins().store(flags(), v, bx, 8);
                self.barrier(b, bx, v);
                self.jump(b, next);
            }
            Op::Unbox { dst, r } => {
                let x = self.get(b, r);
                let p = Self::ptr(b, x);
                let v = b.ins().load(I64, flags(), p, 8);
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::SetBox { r, src } => {
                let x = self.get(b, r);
                let p = Self::ptr(b, x);
                let v = self.get(b, src);
                b.ins().store(flags(), v, p, 8);
                self.barrier(b, p, v);
                self.jump(b, next);
            }
            Op::Cons { dst, a, b: y } => {
                let slow = self.step(b, pc);
                let p = self.alloc(b, 3, header(Kind::Pair, 2, 0), slow);
                let (x, y) = (self.get(b, a), self.get(b, y));
                b.ins().store(flags(), x, p, 8);
                b.ins().store(flags(), y, p, 16);
                let v = Self::tag_ptr(b, p);
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::MkBox { r } => {
                let slow = self.step(b, pc);
                let p = self.alloc(b, 2, header(Kind::Box, 1, 0), slow);
                let x = self.get(b, r);
                b.ins().store(flags(), x, p, 8);
                let v = Self::tag_ptr(b, p);
                self.set(b, r, v);
                self.jump(b, next);
            }
            Op::Closure { .. } => {
                // Allocates and reads captured registers from memory.
                self.spill_at(b, pc);
                let op = b.ins().iconst(I64, op_addr);
                self.call(b, self.sigs.step, jit_step as *const (), &[self.vm, self.r, op]);
                self.reload_after(b, pc);
                self.jump(b, next);
            }
            Op::PopHandler => {
                self.call(b, self.sigs.pop, jit_pop_handler as *const (), &[self.vm]);
                self.jump(b, next);
            }
            Op::Ret { r } => {
                let v = self.get(b, r);
                b.ins().store(flags(), v, self.r, -8);
                let res = b.ins().iconst(I64, (RETURNED as i64) << 32);
                b.ins().return_(&[res]);
            }
            Op::Call { .. } | Op::CallG { .. } => self.call_op(b, pc, op),
            Op::TailCall { .. } | Op::TailCallG { .. } => self.tail_call_op(b, pc, op),
            Op::Quo { dst, a, b: y } | Op::Rem { dst, a, b: y } | Op::Mod { dst, a, b: y } => {
                let slow = self.step(b, pc);
                let (x, y) = (self.get(b, a), self.get(b, y));
                let (xi, yi) = (Self::is_int(b, x), Self::is_int(b, y));
                let both = b.ins().band(xi, yi);
                let nonzero = b.ins().icmp_imm_s(IntCC::NotEqual, y, Value::int_unchecked(0).bits() as i64);
                let ok = b.ins().band(both, nonzero);
                Self::guard(b, ok, slow);
                let (n, d) = (Self::untag(b, x), Self::untag(b, y));
                let v = match op {
                    Op::Quo { .. } => b.ins().sdiv(n, d),
                    Op::Rem { .. } => b.ins().srem(n, d),
                    _ => {
                        // Modulo takes the divisor's sign.
                        let r = b.ins().srem(n, d);
                        let signs = b.ins().bxor(r, d);
                        let differ = b.ins().icmp_imm_s(IntCC::SignedLessThan, signs, 0);
                        let nz = b.ins().icmp_imm_s(IntCC::NotEqual, r, 0);
                        let adjust = b.ins().band(differ, nz);
                        let fixed = b.ins().iadd(r, d);
                        b.ins().select(adjust, fixed, r)
                    }
                };
                // Only quotient can leave the fixnum range (min / -1).
                let fits = Self::fits48(b, v);
                Self::guard(b, fits, slow);
                let v = Self::tag_int(b, v);
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::Jmp { t } => self.jump(b, t as usize),
            Op::Loop { t } => {
                self.tick(b, t as usize);
                self.jump(b, t as usize);
            }
            Op::Jf { c, t } | Op::Jt { c, t } => {
                let x = self.get(b, c);
                let is_false = b.ins().icmp_imm_s(IntCC::Equal, x, Value::FALSE.bits() as i64);
                if matches!(op, Op::Jf { .. }) {
                    self.branch(b, is_false, t as usize, next);
                } else {
                    self.branch(b, is_false, next, t as usize);
                }
            }
            Op::JNEq { a, b: y, t } => {
                let (x, y) = (self.get(b, a), self.get(b, y));
                let eq = b.ins().icmp(IntCC::Equal, x, y);
                self.branch(b, eq, next, t as usize);
            }
            Op::JNNull { a, t } => {
                let x = self.get(b, a);
                let eq = b.ins().icmp_imm_s(IntCC::Equal, x, Value::NIL.bits() as i64);
                self.branch(b, eq, next, t as usize);
            }
            Op::JNPair { a, t } => {
                let x = self.get(b, a);
                let no = self.target(b, t as usize);
                Self::check_kind(b, x, Kind::Pair, no);
                self.jump(b, next);
            }
            Op::JNLt { a, b: y, t } | Op::JNLe { a, b: y, t } | Op::JNNumEq { a, b: y, t } => {
                let (x, y) = (self.get(b, a), self.get(b, y));
                self.compare(b, pc, x, y, Self::cmp_ops(op));
                let c = b.block_params(b.current_block().unwrap())[0];
                self.branch(b, c, next, t as usize);
            }
            Op::JNEqI { a, i, t } => {
                // Equal to a fixnum means the same bits; another fixnum is
                // unequal; only floats and non-numbers need comparing.
                let x = self.get(b, a);
                let k = Self::imm(b, Value::int_unchecked(i as i64));
                let same = b.ins().icmp(IntCC::Equal, x, k);
                let other = b.create_block();
                let yes = self.target(b, next);
                b.ins().brif(same, yes, &[], other, &[]);
                b.switch_to_block(other);
                let int = Self::is_int(b, x);
                let compare = b.create_block();
                let no = self.target(b, t as usize);
                b.ins().brif(int, no, &[], compare, &[]);
                b.switch_to_block(compare);
                self.compare(b, pc, x, k, Self::cmp_ops(op));
                let c = b.block_params(b.current_block().unwrap())[0];
                self.branch(b, c, next, t as usize);
            }
            Op::JNLtI { a, i, t } | Op::JNGtI { a, i, t } => {
                let x = self.get(b, a);
                let k = Self::imm(b, Value::int_unchecked(i as i64));
                let (x, y) = if matches!(op, Op::JNGtI { .. }) { (k, x) } else { (x, k) };
                self.compare(b, pc, x, y, Self::cmp_ops(op));
                let c = b.block_params(b.current_block().unwrap())[0];
                self.branch(b, c, next, t as usize);
            }
            Op::Lt { dst, a, b: y } | Op::Le { dst, a, b: y } | Op::NumEq { dst, a, b: y } => {
                let (x, y) = (self.get(b, a), self.get(b, y));
                self.compare(b, pc, x, y, Self::cmp_ops(op));
                let c = b.block_params(b.current_block().unwrap())[0];
                let v = Self::bool(b, c);
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::Add { dst, a, b: y } | Op::Sub { dst, a, b: y } | Op::Mul { dst, a, b: y } => {
                let (x, y) = (self.get(b, a), self.get(b, y));
                let kind = match op {
                    Op::Add { .. } => '+',
                    Op::Sub { .. } => '-',
                    _ => '*',
                };
                self.arith(b, pc, dst, x, y, kind);
            }
            Op::AddI { dst, a, i } => {
                let x = self.get(b, a);
                let y = Self::imm(b, Value::int_unchecked(i as i64));
                self.arith(b, pc, dst, x, y, '+');
            }
            Op::Car { dst, a } | Op::Cdr { dst, a } => {
                let slow = self.step(b, pc);
                let x = self.get(b, a);
                let p = Self::check_kind(b, x, Kind::Pair, slow);
                let offset = if matches!(op, Op::Car { .. }) { 8 } else { 16 };
                let v = b.ins().load(I64, flags(), p, offset);
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::NullP { dst, a } | Op::Not { dst, a } => {
                let x = self.get(b, a);
                let k = if matches!(op, Op::NullP { .. }) { Value::NIL } else { Value::FALSE };
                let c = b.ins().icmp_imm_s(IntCC::Equal, x, k.bits() as i64);
                let v = Self::bool(b, c);
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::EqP { dst, a, b: y } => {
                let (x, y) = (self.get(b, a), self.get(b, y));
                let c = b.ins().icmp(IntCC::Equal, x, y);
                let v = Self::bool(b, c);
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::PairP { dst, a } => {
                let x = self.get(b, a);
                let no = b.create_block();
                Self::check_kind(b, x, Kind::Pair, no);
                let t = Self::imm(b, Value::TRUE);
                self.set(b, dst, t);
                self.jump(b, next);
                b.switch_to_block(no);
                let f = Self::imm(b, Value::FALSE);
                self.set(b, dst, f);
                self.jump(b, next);
            }
            Op::VRef { v, i, .. } | Op::VSet { v, i, .. } => {
                let slow = self.step(b, pc);
                let (vec, k) = (self.get(b, v), self.get(b, i));
                let p = if matches!(op, Op::VSet { .. }) {
                    Self::check_changeable(b, vec, Kind::Vector, slow)
                } else {
                    Self::check_kind(b, vec, Kind::Vector, slow)
                };
                let int = Self::is_int(b, k);
                let idx = Self::untag(b, k);
                let h = b.ins().load(I64, flags(), p, 0);
                let len = b.ins().ushr_imm_s(h, 16);
                let in_range = b.ins().icmp(IntCC::UnsignedLessThan, idx, len);
                let ok = b.ins().band(int, in_range);
                Self::guard(b, ok, slow);
                let off = b.ins().ishl_imm_s(idx, 3);
                let addr = b.ins().iadd(p, off);
                match *op {
                    Op::VRef { dst, .. } => {
                        let x = b.ins().load(I64, flags(), addr, 8);
                        self.set(b, dst, x);
                    }
                    Op::VSet { x, .. } => {
                        let val = self.get(b, x);
                        b.ins().store(flags(), val, addr, 8);
                        self.barrier(b, p, val);
                    }
                    _ => unreachable!(),
                }
                self.jump(b, next);
            }
            _ => unreachable!("not a native instruction: {op:?}"),
        }
    }
}
