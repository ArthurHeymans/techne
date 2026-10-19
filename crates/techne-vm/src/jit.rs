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
//! pair and vector access, globals, boxes, a few natives: `Inline`) are
//! inline. Everything else goes through `Vm::jit_slow_op`, which executes
//! one instruction in Rust with the interpreter's semantics. Registers known
//! to hold fixnums (`fixnums`) skip the checks and float paths. Vectors'
//! lengths are kept once read (`vectors`), and in loops, the globals read
//! vectors through or called inline (`cached_globals`), read anew after
//! calls.
//!
//! In a function that loops, the fuel (calls and back-edges left before
//! preemption or a check for stops) is counted in a variable too
//! (`Gen::fuel_var`).
//!
//! Small functions called through globals run inline (`inlinable`,
//! `combine`), and the small functions they call, nested: their
//! instructions are compiled after the caller's.
//! Leaving native code before effects or nested calls lets the interpreter
//! make the outermost call instead (`redo_points`); after, it makes the
//! frames as calls would (`Gen::materialize`, `Gen::inline_exit`).

use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::HashMap,
    fmt,
    mem::offset_of,
};

use cranelift_codegen::{
    Context,
    ir::{
        self, AbiParam, Block, BlockArg, InstBuilder, JumpTableData, MemFlagsData, Signature, TrapCode,
        condcodes::{FloatCC, IntCC},
        types::{F64, I32, I64},
    },
    settings::{self, Configurable},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::Module;

#[cfg(test)]
use crate::heap::field;
use crate::{
    code::{Code, Op, Reg},
    heap::{Kind, header, is_kind},
    value::Value,
    vm::{Error, Moves, PlainFn, Vm},
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
    /// `Vm::code_ptrs`, the table of codes by id (a closure's first field),
    /// which never moves: compiled code loads it once per call and takes a
    /// closure's slot in it masked (`heap::code_slot`).
    pub code_table: *const *const Code,
    pub regs_end: *mut Value,
    pub fuel: *mut u32,
    pub stack_top: *mut usize,
    /// Where the nursery's bump pointer and allocation limit are; null
    /// when allocation must go through the VM (GC stress).
    pub heap_top: *mut *mut u64,
    pub heap_end: *const *mut u64,
    /// Output: the code and frame base where the interpreter continues.
    pub code: *const Code,
    pub bp: u64,
    /// Output of a tail call: the callee's entry.
    pub tail: usize,
    /// A callee's result when it handed over to the interpreter inside a
    /// call made by `jit_call_slow`.
    pub res: u64,
    /// `Vm::moves` when native code was entered: while it is the same, so
    /// are `globals` and the register stack.
    pub moves: Moves,
    /// The base of the cage the heap is in: native code takes the address
    /// a value names from its offset there, as `alloc::caged` does, so
    /// that no value, however it came about, names memory outside it.
    pub cage: *mut u8,
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
/// What native code assumes at entry point `pc` (`Job::observed`) does
/// not hold: the interpreter runs the frame's original instructions
/// (`JitSlot::ops`) from `pc`, entering native code again in it only once
/// it has called or looped on (to the next safepoint).
pub const INTERPRET: u32 = 9;
/// Not a status native code returns either: the key of the exit (`Gen::exits`)
/// at an inlined call that lets the interpreter make it after all (`redo`),
/// which returns `EXIT`.
const REDO: u32 = 10;
/// Leave on an edge into an entry point whose assumptions do not hold
/// (`Gen::target`): as `INTERPRET` there, but storing the registers in
/// their tagged form, as what is known there does not hold.
const GUARD: u32 = 11;
/// `TICK` on an edge into an entry point before its assumptions are
/// checked (`Gen::tick`): registers stored in their tagged form, as for
/// `GUARD`.
const GUARD_TICK: u32 = 12;

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
    /// The ids of the codes whose code or constants the compiled code uses
    /// by address (its known callees, the functions it runs inline): kept
    /// while this code is (`Vm::retained`), whatever the closures that ran
    /// them hold now, since their memory is outside the heap. Every address
    /// of a `Code` compiled code takes in (`Known::code`, `Site::code`,
    /// `Inlinee::consts_addr`) must have its id here. Ids of codes freed
    /// meanwhile (none while this code lives) may name later ones, kept
    /// for nothing until this code goes.
    pub deps: RefCell<Vec<u32>>,
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
    /// Registers each `Closure` instruction's code captures, by code index.
    pub captures: HashMap<u32, Vec<Reg>>,
    /// Globals called by this code that held closures when it was queued;
    /// only calls of those closures get the inline closure-call path,
    /// specialised for their code.
    pub closure_globals: HashMap<u32, Known>,
    /// Globals called by this code that held plain Rust natives when it was
    /// queued; calls of those natives call them directly.
    pub native_globals: HashMap<u32, KnownNative>,
    /// Calls and tail calls through globals (by pc) of small functions
    /// (`inlinable`) that native code runs inline, the global holding
    /// their closure (`closure_globals`).
    pub inlined: HashMap<u32, Inlinee>,
    /// The registers of a fixnum and of a float at the loop head, or the
    /// arguments of the call, where the interpreter asked for the code
    /// (`Observed`): native code assumes them there.
    pub observed: Option<Observed>,
}

/// What the interpreter saw at a loop head or at the start (pc 0) of a
/// call: the registers holding a fixnum, and those holding a float
/// (`Job::observed`).
pub struct Observed {
    pub head: usize,
    pub fixnums: Vec<Reg>,
    pub floats: Vec<Reg>,
}

/// A function a call site runs inline (`Job::inlined`).
pub struct Inlinee {
    /// Its `Code`, and the address of its original instructions.
    pub code: usize,
    pub ops: Vec<Op>,
    /// Its constant pool's bits and address (as `Job::consts`).
    pub consts: Vec<u64>,
    pub consts_addr: usize,
    pub frame_size: u16,
    /// Its calls (by pc) of small functions that run inline too, nested.
    pub inlined: HashMap<u32, Inlinee>,
}

impl Inlinee {
    /// Its instructions and those of the functions inlined in it.
    pub fn size(&self) -> usize {
        self.ops.len() + self.inlined.values().map(Inlinee::size).sum::<usize>()
    }
}

/// Instructions a function inlined at a call site may have at most.
pub const INLINE_MAX: usize = 20;
/// Functions inlined in each other at most, a call of the function
/// compiled inlining the first.
pub const INLINE_DEPTH: usize = 3;
/// Instructions a call inlined may bring at most, nested ones included.
pub const INLINE_TOTAL: usize = 40;

/// Whether calls of a function of `ops` (taking `nparams` arguments, and
/// rest arguments if `rest`) with `n` arguments may run inline: a small
/// function without loops, handlers, closures to make or calls but of the
/// Rust natives that `native` says globals hold (`KnownNative`), of which
/// at most one without an inline case (`Inline`): native code makes the
/// callee's frame before each such call, which costs more to compile than
/// it saves; and of the functions that run inline in it (`nested`, by
/// pc). Its instructions then run as part of the caller's native
/// code, its registers above the call's base register (where a call would
/// have put its frame). Leaving native code inside them makes the call in
/// the interpreter (`redo_points`), or its frame as the call would have
/// (`Gen::inline_exit`).
pub fn inlinable(
    ops: &[Op],
    nparams: u16,
    rest: bool,
    n: u16,
    native: impl Fn(u32) -> Option<KnownNative>,
    nested: &HashMap<u32, Inlinee>,
) -> bool {
    use Op::*;
    let forward = |pc: usize, op: &Op| !matches!(op, Jmp { .. }) && !is_branch(op) || jump_or_branch_target(op) > pc;
    let mut calls = 0;
    let ok = ops.iter().enumerate().all(|(pc, op)| {
        forward(pc, op)
            && match *op {
                CallG { .. } | TailCallG { .. } if nested.contains_key(&(pc as u32)) => true,
                CallG { g, n, .. } | TailCallG { g, n, .. } => native(g).inspect(|k| calls += k.inline(n).is_none() as usize).is_some(),
                Closure { .. }
                | PushHandler { .. }
                | PopHandler
                | PushEscape { .. }
                | Loop { .. }
                | Call { .. }
                | TailCall { .. }
                | EnterJit => false,
                _ => true,
            }
    });
    ok && calls <= 1 && !rest && nparams == n && ops.len() <= INLINE_MAX
}

fn is_branch(op: &Op) -> bool {
    use Op::*;
    matches!(
        op,
        Jf { .. }
            | Jt { .. }
            | JNLt { .. }
            | JNLe { .. }
            | JNNumEq { .. }
            | JNEq { .. }
            | JNNull { .. }
            | JNLtI { .. }
            | JNGtI { .. }
            | JNEqI { .. }
            | JNPair { .. }
    )
}

fn jump_or_branch_target(op: &Op) -> usize {
    match *op {
        Op::Jmp { t } => t as usize,
        _ => jump_target(op),
    }
}

/// `op` of a function inlined with its registers `off` above the caller's
/// and its instruction `t` at `at[t]`.
fn relocated(op: Op, off: Reg, at: &[usize]) -> Op {
    use Op::*;
    let r = |x: Reg| x + off;
    let t = |x: u32| at[x as usize] as u32;
    match op {
        LoadK { dst, k } => LoadK { dst: r(dst), k },
        LoadI { dst, i } => LoadI { dst: r(dst), i },
        Mov { dst, src } => Mov { dst: r(dst), src: r(src) },
        GetG { dst, g } => GetG { dst: r(dst), g },
        SetG { g, src } => SetG { g, src: r(src) },
        GetC { dst, i } => GetC { dst: r(dst), i },
        GetCB { dst, i } => GetCB { dst: r(dst), i },
        SetCB { i, src } => SetCB { i, src: r(src) },
        MkBox { r: x } => MkBox { r: r(x) },
        Unbox { dst, r: x } => Unbox { dst: r(dst), r: r(x) },
        SetBox { r: x, src } => SetBox { r: r(x), src: r(src) },
        Jmp { t: x } => Jmp { t: t(x) },
        Jf { c, t: x } => Jf { c: r(c), t: t(x) },
        Jt { c, t: x } => Jt { c: r(c), t: t(x) },
        JNLt { a, b, t: x } => JNLt { a: r(a), b: r(b), t: t(x) },
        JNLe { a, b, t: x } => JNLe { a: r(a), b: r(b), t: t(x) },
        JNNumEq { a, b, t: x } => JNNumEq { a: r(a), b: r(b), t: t(x) },
        JNEq { a, b, t: x } => JNEq { a: r(a), b: r(b), t: t(x) },
        JNNull { a, t: x } => JNNull { a: r(a), t: t(x) },
        JNLtI { a, i, t: x } => JNLtI { a: r(a), i, t: t(x) },
        JNGtI { a, i, t: x } => JNGtI { a: r(a), i, t: t(x) },
        JNEqI { a, i, t: x } => JNEqI { a: r(a), i, t: t(x) },
        JNPair { a, t: x } => JNPair { a: r(a), t: t(x) },
        Add { dst, a, b } => Add { dst: r(dst), a: r(a), b: r(b) },
        AddI { dst, a, i } => AddI { dst: r(dst), a: r(a), i },
        Sub { dst, a, b } => Sub { dst: r(dst), a: r(a), b: r(b) },
        Mul { dst, a, b } => Mul { dst: r(dst), a: r(a), b: r(b) },
        Quo { dst, a, b } => Quo { dst: r(dst), a: r(a), b: r(b) },
        Rem { dst, a, b } => Rem { dst: r(dst), a: r(a), b: r(b) },
        Mod { dst, a, b } => Mod { dst: r(dst), a: r(a), b: r(b) },
        Lt { dst, a, b } => Lt { dst: r(dst), a: r(a), b: r(b) },
        Le { dst, a, b } => Le { dst: r(dst), a: r(a), b: r(b) },
        NumEq { dst, a, b } => NumEq { dst: r(dst), a: r(a), b: r(b) },
        Car { dst, a } => Car { dst: r(dst), a: r(a) },
        Cdr { dst, a } => Cdr { dst: r(dst), a: r(a) },
        Cons { dst, a, b } => Cons { dst: r(dst), a: r(a), b: r(b) },
        NullP { dst, a } => NullP { dst: r(dst), a: r(a) },
        PairP { dst, a } => PairP { dst: r(dst), a: r(a) },
        Not { dst, a } => Not { dst: r(dst), a: r(a) },
        EqP { dst, a, b } => EqP { dst: r(dst), a: r(a), b: r(b) },
        VRef { dst, v, i } => VRef { dst: r(dst), v: r(v), i: r(i) },
        VSet { v, i, x } => VSet { v: r(v), i: r(i), x: r(x) },
        CallG { base, n, g } => CallG { base: r(base), n, g },
        TailCallG { base, n, g } => TailCallG { base: r(base), n, g },
        _ => unreachable!("not inlined: {op:?}"),
    }
}

/// A call site whose callee runs inline (`Job::inlined`): its instructions
/// follow the caller's in the function compiled (`combine`).
struct Site {
    /// The call's pc (in the function compiled) and base register, and
    /// its arguments.
    call: usize,
    base: Reg,
    args: u16,
    /// The call inlined whose callee makes this call, if nested.
    parent: Option<usize>,
    /// Where the caller continues after the call: its own pc.
    ret_pc: usize,
    /// A tail call: the callee takes over the caller's frame (`combine`).
    tail: bool,
    /// The callee's registers are the caller's from `off`: `base + 1`, or
    /// the caller's own for a tail call.
    off: Reg,
    /// What its returns do.
    ret: Return,
    /// Where its instructions start.
    start: usize,
    code: i64,
    frame: u16,
    /// Where the closure called is (`Known::closure`): a GC root, so
    /// current after anything that collects, and the closure running
    /// (the call's guard compared it), even if the global changed since.
    closure: i64,
    consts: i64,
    const_bits: Vec<u64>,
}

/// What a return of a function inlined does: the caller's own return, or a
/// move of the value to the base register of call `site` and a jump to
/// `after` it.
#[derive(Clone, Copy)]
enum Return {
    Ret,
    Move { dst: Reg, after: u32, site: usize },
}

impl Site {
    /// The callee's registers that the call does not set (to clear when
    /// it starts): its own, but the arguments, and for a tail call those
    /// that hold the arguments until moved into place.
    fn fresh(&self) -> impl Iterator<Item = Reg> + '_ {
        let args = self.base + 1..=self.base + self.args;
        (self.off + self.args..self.off + self.frame).filter(move |r| !(self.tail && args.contains(r)))
    }
}

/// The outermost call inlined that leads to `site`'s (a call of the
/// function compiled).
fn root(sites: &[Site], mut site: usize) -> usize {
    while let Some(p) = sites[site].parent {
        site = p;
    }
    site
}

/// The calls inlined whose frames exist when native code leaves in the
/// callee of `site`'s: that one, then down to the outermost, but those a
/// tail call replaced.
fn frames(sites: &[Site], site: usize) -> Vec<usize> {
    let mut out = vec![site];
    let mut s = site;
    while let Some(p) = sites[s].parent {
        if !sites[s].tail {
            out.push(p);
        }
        s = p;
    }
    out
}

/// An instruction of a function inlined at `Gen::sites[site]`, its `pc`
/// there (that of the instruction after a tail call of a native, for what
/// returns its value).
#[derive(Clone, Copy)]
struct Inlined {
    site: usize,
    pc: usize,
    /// A tail call (`TailCallG`) of a native, turned into a call.
    tail: bool,
}

/// The instructions to compile for `job`: its own, then those of each
/// function inlined (each returning with a move to the call's base
/// register and a jump after the call, a tail call of a native being a
/// call), with where each of these is from. A function inlined at a tail
/// call takes over the caller's frame, as the call would: its registers
/// are the caller's own, moves put the arguments in place first, and its
/// returns are the caller's. The calls a function inlined makes of others
/// inlined (nested) are compiled the same way, their callee's instructions
/// after. An instruction that native code never runs separates the
/// function's own instructions from the others: exits after its last
/// instruction are its own. Also by instruction: the call inlined whose
/// result a move returns (`Return::Move`).
#[allow(clippy::type_complexity)]
fn combine(job: &Job) -> (Vec<Op>, Vec<Option<Inlined>>, Vec<Site>, Vec<Option<usize>>) {
    let mut ops = job.ops.clone();
    ops.push(Op::EnterJit);
    let mut origin = vec![None; ops.len()];
    let mut returns = vec![None; ops.len()];
    let mut calls: Vec<_> = job.inlined.keys().copied().collect();
    calls.sort_unstable();
    let mut todo: std::collections::VecDeque<(usize, &Inlinee, Option<usize>)> =
        calls.iter().map(|&call| (call as usize, &job.inlined[&call], None)).collect();
    let mut sites: Vec<Site> = Vec::new();
    while let Some((call, inl, parent)) = todo.pop_front() {
        let (tail, base, args, g) = match ops[call] {
            Op::CallG { base, n, g } => (false, base, n, g),
            Op::TailCallG { base, n, g } => (true, base, n, g),
            op => unreachable!("inlined call: {op:?}"),
        };
        let site = sites.len();
        let (off, ret) = match (tail, parent) {
            (true, None) => (0, Return::Ret),
            (true, Some(p)) => (sites[p].off, sites[p].ret),
            (false, _) => (base + 1, Return::Move { dst: base, after: call as u32 + 1, site }),
        };
        let ret_pc = match parent {
            Some(_) => origin[call].map_or(0, |o: Inlined| o.pc) + 1,
            None => call + 1,
        };
        let start = ops.len();
        let from = |pc: usize, tail: bool| Some(Inlined { site, pc, tail });
        if tail {
            // In increasing order, no move overwrites an argument still to
            // move (each goes down).
            ops.extend((0..args).map(|i| Op::Mov { dst: off + i, src: base + 1 + i }));
            origin.extend((0..args).map(|_| from(0, false)));
            returns.extend((0..args).map(|_| None));
        }
        let ret_len = match ret {
            Return::Ret => 1,
            Return::Move { .. } => 2,
        };
        let nested = |pc: usize| inl.inlined.contains_key(&(pc as u32));
        let size = |pc: usize, op: &Op| match op {
            Op::Ret { .. } => ret_len,
            Op::TailCallG { .. } if !nested(pc) => 1 + ret_len,
            _ => 1,
        };
        let at: Vec<usize> =
            inl.ops.iter().enumerate().scan(ops.len(), |next, (pc, op)| Some(std::mem::replace(next, *next + size(pc, op)))).collect();
        // The return of the value in register `r` (the callee's) for its
        // instruction `pc`.
        let give = |ops: &mut Vec<Op>, origin: &mut Vec<_>, returns: &mut Vec<_>, r: Reg, pc: usize| match ret {
            Return::Ret => {
                ops.push(Op::Ret { r: r + off });
                origin.push(from(pc, false));
                returns.push(None);
            }
            Return::Move { dst, after, site } => {
                ops.extend([Op::Mov { dst, src: r + off }, Op::Jmp { t: after }]);
                origin.extend([from(pc, false), from(pc, false)]);
                returns.extend([Some(site), None]);
            }
        };
        for (pc, &op) in inl.ops.iter().enumerate() {
            match op {
                Op::Ret { r } => give(&mut ops, &mut origin, &mut returns, r, pc),
                Op::TailCallG { base: b, n, g } if !nested(pc) => {
                    ops.push(Op::CallG { base: b + off, n, g });
                    origin.push(from(pc, true));
                    returns.push(None);
                    give(&mut ops, &mut origin, &mut returns, b, pc + 1);
                }
                _ => {
                    if nested(pc) {
                        todo.push_back((ops.len(), &inl.inlined[&(pc as u32)], Some(site)));
                    }
                    ops.push(relocated(op, off, &at));
                    origin.push(from(pc, false));
                    returns.push(None);
                }
            }
        }
        sites.push(Site {
            call,
            base,
            args,
            parent,
            ret_pc,
            tail,
            off,
            ret,
            start,
            code: inl.code as i64,
            frame: inl.frame_size,
            closure: job.closure_globals[&g].closure as i64,
            consts: inl.consts_addr as i64,
            const_bits: inl.consts.clone(),
        });
    }
    (ops, origin, sites, returns)
}

/// The inlined instructions (`combine`) at which leaving native code may
/// let the interpreter make the outermost call inlining them instead
/// (`REDO`): those that native code reaches from that callee's start only
/// through instructions without effects outside its frame, without calls,
/// and leaving its arguments as they were. Nested inline entry also ends
/// replay: it counts fuel and clears fresh registers, which can overlap
/// the outer call's original arguments after a tail-frame replacement.
/// Resuming from frames after entry preserves completed siblings too,
/// without having to refund every call that replay would repeat.
fn redo_points(ops: &[Op], origin: &[Option<Inlined>], sites: &[Site], succs: &[Vec<usize>]) -> Vec<bool> {
    use Op::*;
    let roots: Vec<Option<usize>> = origin.iter().map(|o| o.map(|o| root(sites, o.site))).collect();
    let effect: Vec<bool> = (0..ops.len())
        .map(|pc| {
            let Some(r) = roots[pc] else { return false };
            let s = &sites[r];
            let args = s.base + 1..=s.base + s.args;
            let call = matches!(ops[pc], CallG { .. } | TailCallG { .. });
            call || matches!(ops[pc], SetG { .. } | SetCB { .. } | SetBox { .. } | VSet { .. } | PopHandler)
                || uses_defs(&ops[pc], &HashMap::new()).1.iter().any(|d| args.contains(d))
        })
        .collect();
    // Calls inlined in inlined code jump back to their caller's
    // instructions: to a fixpoint.
    let mut tainted = vec![false; ops.len()];
    loop {
        let mut changed = false;
        for pc in (0..ops.len()).filter(|&pc| roots[pc].is_some()) {
            let out = tainted[pc] || effect[pc];
            for &next in succs[pc].iter().filter(|&&next| roots[next] == roots[pc]) {
                changed |= out && !tainted[next];
                tainted[next] |= out;
            }
        }
        if !changed {
            break;
        }
    }
    (0..ops.len()).map(|pc| roots[pc].is_some() && !tainted[pc]).collect()
}

/// A Rust native a call site expects to call.
#[derive(Clone, Copy)]
pub struct KnownNative {
    /// Its value (an immediate: the GC does not move it).
    pub value: u64,
    pub f: PlainFn,
    pub min: usize,
    pub max: Option<usize>,
    /// Its common case, inline.
    pub inline: Option<Inline>,
    /// It returns only fixnums (one of `FIXNUM_NATIVES`): a call site
    /// whose guard found it knows its value is one (`fixnums`).
    pub fixnum: bool,
}

/// The globals holding builtin natives that return a fixnum whenever
/// they return.
pub const FIXNUM_NATIVES: [&str; 6] =
    ["string-length", "vector-length", "bytevector-length", "bytevector-u8-ref", "char->integer", "length"];

impl KnownNative {
    fn takes(&self, n: u16) -> bool {
        n as usize >= self.min && self.max.is_none_or(|m| n as usize <= m)
    }

    /// Its inline case, for a call with `n` arguments.
    fn inline(&self, n: u16) -> Option<Inline> {
        self.inline.filter(|i| i.arity() == n)
    }
}

/// Builtin natives whose common case native code runs inline, calling the
/// native for anything else (errors included).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Inline {
    /// Of an ASCII string.
    StringLength,
    /// Of an ASCII string.
    StringRef,
    VectorLength,
    BytevectorU8Ref,
    CharToInteger,
    /// Of two characters.
    CharEq,
    /// A field of a record of the type given (record accessors).
    RecordRef,
}

impl Inline {
    /// Each, and the global holding its native.
    pub const ALL: [(Inline, &str); 7] = [
        (Inline::StringLength, "string-length"),
        (Inline::StringRef, "string-ref"),
        (Inline::VectorLength, "vector-length"),
        (Inline::BytevectorU8Ref, "bytevector-u8-ref"),
        (Inline::CharToInteger, "char->integer"),
        (Inline::CharEq, "char=?"),
        (Inline::RecordRef, "%record-ref"),
    ];

    fn arity(self) -> u16 {
        match self {
            Inline::StringLength | Inline::VectorLength | Inline::CharToInteger => 1,
            Inline::StringRef | Inline::BytevectorU8Ref | Inline::CharEq => 2,
            Inline::RecordRef => 3,
        }
    }
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

/// Run a JIT helper's Rust body, containing panics: unwinding out of an
/// `extern "C"` helper into generated code would abort the process, so a
/// panic poisons the world and the caller reports it through its error
/// channel (`Vm::poisoned`).
fn catch_jit<T>(vm: *mut Vm, f: impl FnOnce(&mut Vm) -> T) -> Option<T> {
    let vm = unsafe { &mut *vm };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(vm))) {
        // Poisoned through a nested execution whose error was swallowed
        // in there: report failure so the caller takes its error channel
        // instead of compiled code running on with the helper's result.
        Ok(_) if vm.poisoned().is_some() => None,
        Ok(v) => Some(v),
        Err(panic) => {
            vm.poison_panic(panic.as_ref());
            crate::vm::dispose_panic_payload(panic);
            None
        }
    }
}

/// The error status a helper returns when `catch_jit` contained a panic:
/// `jit_error` carries the poison error.
fn jit_poisoned(vm: *mut Vm) -> Error {
    let vm = unsafe { &mut *vm };
    vm.poison_error()
}

unsafe extern "C" fn jit_step(vm: *mut Vm, bp: usize, op: *const Op) -> u32 {
    unsafe {
        match catch_jit(vm, |vm| vm.jit_slow_op(bp, *op)) {
            Some(Ok(false)) => 0,
            Some(Ok(true)) => 1,
            Some(Err(e)) => {
                (*vm).jit_error = Some(e);
                2
            }
            None => {
                (*vm).jit_error = Some(jit_poisoned(vm));
                2
            }
        }
    }
}

/// Call Rust native `f` with the `n` arguments after `regs[base]` and store
/// the result in `regs[base]` (and `regs[also]`). Result codes as for
/// `jit_call_slow`.
fn call_native(vm: &mut Vm, f: Value, base: usize, n: usize, also: usize) -> u64 {
    let moves = vm.moves;
    vm.regs[base] = f;
    vm.stack_top = vm.stack_top.max(base + 1 + n);
    // Under `catch_jit`: a panic stops at the helper's boundary.
    let result = vm.call_native_running(f.as_native(), base + 1, n);
    native_returned(vm, result, base, also, moves)
}

/// The end of a native call: see `call_native`.
#[inline(always)]
fn native_returned(vm: &mut Vm, result: Result<Value, Error>, base: usize, also: usize, moves: Moves) -> u64 {
    match result {
        Ok(v) => {
            vm.regs[base] = v;
            vm.regs[also] = v;
            // Native code holds on to both; leave it if either moved.
            if vm.moves == moves { 0 } else { 4 }
        }
        Err(e) => native_failed(vm, e),
    }
}

#[cold]
#[inline(never)]
fn native_failed(vm: &mut Vm, e: Error) -> u64 {
    let wait = e.wait.is_some();
    vm.jit_error = Some(e);
    if wait { 3 } else { 2 }
}

/// What a plain native (`PlainFn`) returns, in two registers: the
/// platform's C convention returns this struct the way Cranelift returns
/// two `I64` results (`rax`/`rdx`, `x0`/`x1`; see the test `two_results`).
/// So compiled code calls plain natives themselves.
#[repr(C)]
pub struct NativeRet {
    /// The native's value, unless it failed.
    value: u64,
    /// 0, or its error (`Error::into_raw`).
    error: u64,
}

impl NativeRet {
    /// Call a plain native from compiled code, containing its panics
    /// (`Vm::catch_native`): a panic poisons the world and returns an
    /// error instead of unwinding out of the `extern "C"` entry, which
    /// would abort the process. Every `PlainFn` must go through this.
    #[inline(always)]
    pub fn from_call(vm: &mut Vm, f: impl FnOnce(&mut Vm) -> Result<Value, Error>) -> NativeRet {
        Self::from_result(vm.catch_native(f))
    }

    #[inline(always)]
    pub fn from_result(result: Result<Value, Error>) -> NativeRet {
        match result {
            Ok(v) => NativeRet { value: v.bits(), error: 0 },
            Err(e) => NativeRet { value: 0, error: e.into_raw() as u64 },
        }
    }

    #[inline(always)]
    pub(crate) fn into_result(self) -> Result<Value, Error> {
        match self.error {
            0 => Ok(Value::from_bits(self.value)),
            // SAFETY: made by `from_result`, once.
            e => Err(unsafe { Error::from_raw(e as *mut _) }),
        }
    }
}

/// What `jit_native_unusual` returns, in two registers.
#[repr(C)]
struct Unusual {
    /// A result code of `jit_call_slow`.
    status: u64,
    /// The frame's registers, where they are now.
    frame: *mut Value,
}

/// A plain native called from compiled code (`KnownNative`) at frame `bp`
/// failed (with `error`) or moved the register stack or the globals (and
/// returned `value`): the result code of `jit_call_slow` for that, 2
/// (error), 3 (wait) or 4 (store `value` in the call's `base` register),
/// and the frame's registers. `native` is the native's value.
unsafe extern "C" fn jit_native_unusual(vm: *mut Vm, value: u64, error: u64, bp: u64, base: u64, native: u64) -> Unusual {
    let status = catch_jit(vm, |vm| match (NativeRet { value, error }).into_result() {
        Ok(x) => {
            vm.regs[(bp + base) as usize] = x;
            4
        }
        Err(e) => {
            let e = vm.native_error(Value::from_bits(native).as_native(), e);
            native_failed(vm, e)
        }
    });
    let vm = unsafe { &mut *vm };
    let status = match status {
        Some(status) => status,
        None => native_failed(vm, vm.poison_error()),
    };
    Unusual { status, frame: unsafe { vm.regs.as_mut_ptr().add(bp as usize) } }
}

/// Run tail calls handed back by a native callee (`TAILCALL`) until it
/// returns or hands over to the interpreter.
unsafe extern "C" fn jit_after_call(vm: *mut Vm, ctx: *mut JitCtx, mut res: u64, bp: u64, depth: u64) -> u64 {
    unsafe {
        while (res >> 32) as u32 == TAILCALL {
            let next: JitFn = std::mem::transmute(std::ptr::with_exposed_provenance::<()>((*ctx).tail));
            // The preceding entry may have collected or reentered. Derive
            // this entry's frame from its index, not that entry's pointer.
            let frame = (*vm).regs.as_mut_ptr().add(bp as usize);
            res = next(vm, ctx, frame, bp, 0, depth as u32);
        }
        res
    }
}

/// A non-tail call the inline path does not handle: Rust natives, compiled
/// rest-argument closures, and compiled closures called where native code
/// expected another procedure (a redefinition). Returns 0 (done, result in the base
/// register), 1 (let the interpreter make the call), 2 (error), 3 (the task
/// must wait), 4 (done, but the register stack or globals moved) or 5 (the
/// callee handed over to the interpreter; its result is in `ctx.res`).
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn jit_call_slow(vm: *mut Vm, ctx: *mut JitCtx, bp: u64, base: u64, n: u64, depth: u64, f: u64) -> u64 {
    match catch_jit(vm, |_| unsafe { jit_call_slow_inner(vm, ctx, bp, base, n, depth, f) }) {
        Some(status) => status,
        None => {
            let vm_ref = unsafe { &mut *vm };
            native_failed(vm_ref, vm_ref.poison_error())
        }
    }
}

unsafe fn jit_call_slow_inner(vm: *mut Vm, ctx: *mut JitCtx, bp: u64, base: u64, n: u64, depth: u64, f: u64) -> u64 {
    unsafe {
        let vm = &mut *vm;
        let (f, bp, base, n) = (Value::from_bits(f), bp as usize, base as usize, n as usize);
        if f.is_native() && f != vm.apply_native {
            return call_native(vm, f, bp + base, n, bp + base);
        }
        if !is_kind(f, Kind::Closure) || depth as i64 >= MAX_DEPTH {
            return 1;
        }
        let code = vm.closure_code(f);
        let c = &*code;
        let callee_bp = bp + base + 1;
        let Some(entry) = c.jit.call_entry.get() else { return 1 };
        if (if c.rest { n < c.nparams as usize } else { n != c.nparams as usize })
            || callee_bp + (c.frame_size as usize).max(n) + 1 > vm.regs.len()
        {
            return 1;
        }
        // Count the call like the interpreter; at the end of the slice let it
        // make the call (and suspend).
        let fuel = &mut *(*ctx).fuel;
        if *fuel <= 1 {
            return 1;
        }
        *fuel -= 1;
        vm.regs[bp + base] = f;
        vm.enter(code, callee_bp, n).expect("arity checked");
        let frame = vm.regs.as_mut_ptr().add(callee_bp);
        let res = entry(vm, ctx, frame, callee_bp as u64, 0, depth as u32 + 1);
        let res = jit_after_call(vm, ctx, res, callee_bp as u64, depth + 1);
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
    let status = catch_jit(vm, |vm| {
        let (f, bp) = (Value::from_bits(f), bp as usize);
        if f.is_native() && f != vm.apply_native {
            return call_native(vm, f, bp + base as usize, n as usize, bp - 1);
        }
        1
    });
    match status {
        Some(status) => status,
        None => {
            let vm = unsafe { &mut *vm };
            native_failed(vm, vm.poison_error())
        }
    }
}

/// The register stack has no room for the `needed` registers of a frame
/// of `code` with the functions inlined in it (`Gen::room`): grow it, and
/// return `RESUME << 32` (enter again); if it cannot grow, let the
/// interpreter run `code` from now on and return `EXIT << 32`.
unsafe extern "C" fn jit_make_room(vm: *mut Vm, code: *mut Code, needed: u64) -> u64 {
    // On a contained panic the world is poisoned; EXIT hands back to the
    // interpreter, which refuses at its next poll (`poll_interrupt`).
    let status = match catch_jit(vm, |vm| vm.jit_make_room(code, needed as usize)) {
        Some(true) => RESUME,
        Some(false) | None => EXIT,
    };
    (status as u64) << 32
}

unsafe extern "C" fn jit_pop_handler(vm: *mut Vm) {
    let _ = catch_jit(vm, |vm| vm.jit_pop_handler());
}

unsafe extern "C" fn jit_unwind_push(vm: *mut Vm, code: *const Code, pc: u64, bp: u64) {
    let _ = catch_jit(vm, |vm| vm.jit_push_frame(code, pc as u32, bp as u32));
}

unsafe extern "C" fn jit_barrier(vm: *mut Vm, obj: *mut u64, v: Value) {
    let _ = catch_jit(vm, |vm| vm.write_barrier(obj, v));
}

const TAG_INT: i64 = 0xFFF9;
const TAG_PTR: i64 = 0xFFFA;
const TAG_CHAR: i64 = 0xFFFB;
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
    fn contains(&self, r: Reg) -> bool {
        self.0[r as usize / 64] & (1 << (r % 64)) != 0
    }
    fn union(&mut self, o: &Regs) {
        self.0.iter_mut().zip(&o.0).for_each(|(a, b)| *a |= b);
    }
    fn and(&self, o: &Regs) -> Regs {
        Regs(self.0.iter().zip(&o.0).map(|(a, b)| a & b).collect())
    }
    fn minus(&self, o: &Regs) -> Regs {
        Regs(self.0.iter().zip(&o.0).map(|(a, b)| a & !b).collect())
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

/// The control flow of the instructions compiled (`combine`).
struct Shape {
    /// Each instruction's successors: an inlined call's include the start
    /// of its callee's instructions.
    succs: Vec<Vec<usize>>,
    /// By inlined call: where its callee's instructions start, and its
    /// registers that the call's arguments do not set.
    entries: HashMap<usize, (usize, Regs)>,
    /// The instructions that are an inlined callee's.
    inlined: Vec<bool>,
    /// By inlined call: the moves of its callee's results to its base
    /// register (at the callee's returns), and the register moved.
    results: HashMap<usize, Vec<(usize, Reg)>>,
}

impl Shape {
    /// The registers set on the edge from `pc` to `s`: an inlined callee's
    /// own (`entries`).
    fn fresh(&self, pc: usize, s: usize) -> Option<&Regs> {
        self.entries.get(&pc).filter(|e| e.0 == s).map(|e| &e.1)
    }
}

/// `native` tells the calls of known Rust natives (`KnownNative`), and
/// whether they may continue inline, without writing registers back
/// (`Inline`). `fix` are the registers known to hold fixnums (`fixnums`),
/// which such calls do not write back (`deferred`).
fn analyze(
    ops: &[Op],
    n: usize,
    captures: &HashMap<u32, Vec<Reg>>,
    self_jump: bool,
    fix: &[Regs],
    native: impl Fn(usize) -> Option<bool>,
    shape: &Shape,
) -> Flow {
    let len = ops.len();
    let effects: Vec<_> = ops.iter().map(|op| uses_defs(op, captures)).collect();
    let succs = &shape.succs;
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
            let out = match (ops[pc], native(pc)) {
                // A known native's value stays in a machine register. Its
                // call writes registers back (unless inline) but for known
                // fixnums, and keeps them or re-reads them.
                (Op::CallG { base, .. }, Some(inline)) => {
                    let mut d = if inline {
                        dirty_in[pc].clone()
                    } else if shape.inlined[pc] {
                        Regs::empty(n)
                    } else {
                        deferred(&ops[pc], &dirty_in[pc], &live_in[pc], &fix[pc])
                    };
                    d.insert(base);
                    d
                }
                // Other calls write registers back and re-read them.
                (Op::Call { .. } | Op::CallG { .. }, _) => Regs::empty(n),
                _ => {
                    let mut d = dirty_in[pc].clone();
                    effects[pc].1.iter().for_each(|&r| d.insert(r));
                    d
                }
            };
            for &s in &succs[pc] {
                let before = dirty_in[s].clone();
                match shape.fresh(pc, s) {
                    // Into an inlined callee: nothing written back; its own
                    // registers are not in memory.
                    Some(fresh) => {
                        let mut d = dirty_in[pc].clone();
                        d.union(fresh);
                        dirty_in[s].union(&d);
                    }
                    None => dirty_in[s].union(&out),
                }
                changed |= dirty_in[s] != before;
            }
        }
        if !changed {
            break;
        }
    }
    Flow { live_in, live_out, dirty_in, landing }
}

/// The registers a call of a known native (`op`) writes back only if
/// native code is left after it, given those `dirty` and `live` before it
/// and the fixnums `known` there: the fixnums, which the GC does not move,
/// but for its arguments, which the native reads.
fn deferred(op: &Op, dirty: &Regs, live: &Regs, known: &Regs) -> Regs {
    let mut regs = dirty.and(live).and(known);
    uses_defs(op, &HashMap::new()).0.iter().for_each(|&r| regs.remove(r));
    regs
}

/// Registers known to hold fixnums when native code runs each instruction:
/// constants and fixnum results, as long as nothing else is stored in
/// them. Native code continues after arithmetic only with a fixnum result
/// when both operands are fixnums (it steps out on overflow), and after a
/// division only with a fixnum, and after a call of a native that returns
/// only fixnums (`KnownNative::fixnum`, told by `fixnum_call`) only with
/// a fixnum: if the guard finds another procedure in the global, native
/// code checks what it returned. Registers known at pc 0 are none (the
/// arguments), and calls change none but their base register.
///
/// What is known is what holds on every path native code takes from pc 0.
/// An entry point on such paths (a loop head, the instruction after a
/// call) ignores the interpreter's way in: its loader checks the registers
/// known there and leaves the frame to the interpreter (`INTERPRET`) if
/// one is not a fixnum (the interpreter computed a bignum, say). Native
/// code the interpreter alone leads to (past a `guard`'s
/// handler, say) knows nothing at its entry points: what it would know
/// there holds on no path at all, and would be a guess that fails for
/// good when wrong.
#[allow(clippy::too_many_arguments)]
fn fixnums(
    ops: &[Op],
    n: usize,
    konst: impl Fn(usize, u32) -> Value,
    captures: &HashMap<u32, Vec<Reg>>,
    heads: &[usize],
    fixnum_call: impl Fn(usize) -> bool,
    shape: &Shape,
    assumed: &HashMap<usize, Regs>,
) -> Kinds {
    let step = |pc: usize, known: &[Regs]| {
        let has = |r: Reg| known[pc].contains(r);
        match ops[pc] {
            Op::LoadI { dst, .. } => Some((dst, true)),
            Op::LoadK { dst, k } => Some((dst, konst(pc, k).is_int())),
            Op::Mov { dst, src } => Some((dst, has(src))),
            Op::Add { dst, a, b } | Op::Sub { dst, a, b } | Op::Mul { dst, a, b } => Some((dst, has(a) && has(b))),
            Op::AddI { dst, a, .. } => Some((dst, has(a))),
            Op::Quo { dst, .. } | Op::Rem { dst, .. } | Op::Mod { dst, .. } => Some((dst, true)),
            // An inlined call returns a fixnum if each of its callee's
            // returns does (`Gen::call_op` checks the call's result).
            Op::CallG { base, .. } => {
                let results = shape.results.get(&pc);
                Some((base, fixnum_call(pc) || results.is_some_and(|m| m.iter().all(|&(at, r)| known[at].contains(r)))))
            }
            _ => None,
        }
    };
    known_regs(ops, n, captures, heads, shape, assumed, step, |_| None)
}

/// Registers known to hold floats when native code runs each instruction,
/// as `fixnums` knows fixnums: float constants, and the results of
/// arithmetic with an operand known a float. Native code continues after
/// such arithmetic only with a float: when the other operand is a float or
/// a fixnum (converted), else it steps out (a bignum, a complex number...).
fn flonums(
    ops: &[Op],
    n: usize,
    konst: impl Fn(usize, u32) -> Value,
    captures: &HashMap<u32, Vec<Reg>>,
    heads: &[usize],
    shape: &Shape,
    assumed: &HashMap<usize, Regs>,
) -> Kinds {
    let step = |pc: usize, known: &[Regs]| {
        let has = |r: Reg| known[pc].contains(r);
        match ops[pc] {
            Op::LoadK { dst, k } => Some((dst, konst(pc, k).is_float())),
            Op::Mov { dst, src } => Some((dst, has(src))),
            Op::Add { dst, a, b } | Op::Sub { dst, a, b } | Op::Mul { dst, a, b } => Some((dst, has(a) || has(b))),
            Op::AddI { dst, a, .. } => Some((dst, has(a))),
            _ => None,
        }
    };
    known_regs(ops, n, captures, heads, shape, assumed, step, |_| None)
}

/// The globals native code keeps in machine registers (`Gen::cached`), if
/// it never sets them: those it reads in a loop (an instruction that can
/// follow itself, along `succs`), a few, to read vectors through, with
/// their vectors' lengths (`true`), or to make calls inlined (at
/// `inlined`), whose check that they still hold the closure inlined is
/// kept too (`Gen::same`). Others it calls would cost a read after every
/// call to save one before.
fn cached_globals(ops: &[Op], succs: &[Vec<usize>], inlined: &[usize]) -> Vec<(u32, bool)> {
    let set: Vec<u32> = ops.iter().filter_map(|op| if let Op::SetG { g, .. } = *op { Some(g) } else { None }).collect();
    let read_through = |r: Reg| ops.iter().any(|op| matches!(*op, Op::VRef { v, .. } | Op::VSet { v, .. } if v == r));
    let in_loop = |pc: usize| {
        let mut seen = vec![false; ops.len()];
        let mut todo = succs[pc].clone();
        while let Some(s) = todo.pop() {
            if s == pc {
                return true;
            }
            if !std::mem::replace(&mut seen[s], true) {
                todo.extend(&succs[s]);
            }
        }
        false
    };
    let mut cached: Vec<(u32, bool)> = ops
        .iter()
        .enumerate()
        .filter_map(|(pc, op)| match *op {
            Op::GetG { dst, g } if read_through(dst) => Some((pc, (g, true))),
            Op::CallG { g, .. } | Op::TailCallG { g, .. } if inlined.contains(&pc) => Some((pc, (g, false))),
            _ => None,
        })
        .filter(|&(_, (g, _))| !set.contains(&g))
        .filter(|&(pc, _)| in_loop(pc))
        .map(|(_, c)| c)
        .collect();
    // Vectors first, each global once.
    cached.sort_unstable_by_key(|&(g, lens)| (!lens, g));
    cached.dedup_by_key(|c| c.0);
    cached.truncate(16);
    cached
}

/// By instruction, the globals kept (`cached`, a bit each) that native
/// code may read from there before it reads those kept anew
/// (`Gen::reload_globals`): where it does, it reads only these. Calls
/// read them anew on their way to the next instruction, but for natives
/// running inline (`inline`).
fn anticipated(ops: &[Op], cached: &[u32], succs: &[Vec<usize>], inline: impl Fn(usize) -> bool) -> Vec<u16> {
    let bit = |g: u32| cached.iter().position(|&c| c == g).map_or(0, |i| 1 << i);
    let uses: Vec<u16> = ops
        .iter()
        .map(|op| match *op {
            Op::GetG { g, .. } | Op::CallG { g, .. } | Op::TailCallG { g, .. } => bit(g),
            _ => 0,
        })
        .collect();
    let reloads = |pc: usize, s: usize| {
        s == pc + 1
            && match ops[pc] {
                Op::Closure { .. } | Op::PopHandler | Op::Call { .. } => true,
                Op::CallG { .. } => !inline(pc),
                _ => false,
            }
    };
    let mut ant = uses.clone();
    loop {
        let mut changed = false;
        for pc in (0..ops.len()).rev() {
            let a = succs[pc].iter().filter(|&&s| !reloads(pc, s)).fold(uses[pc], |a, &s| a | ant[s]);
            changed |= a != ant[pc];
            ant[pc] = a;
        }
        if !changed {
            return ant;
        }
    }
}

/// Registers whose vector lengths native code keeps (`Gen::lens`): those
/// it read a vector through, and copies, as long as nothing else is
/// stored in them, and those holding a global it keeps (`cached`, see
/// `Gen::cached`). A vector's length and mutability stay as long as it
/// does (collections move it, not change it), so calls change none but
/// what they set. Entry points' loaders work them out anew.
fn vectors(ops: &[Op], n: usize, captures: &HashMap<u32, Vec<Reg>>, heads: &[usize], shape: &Shape, cached: &[u32]) -> Vec<Regs> {
    let step = |pc: usize, known: &[Regs]| match ops[pc] {
        Op::GetG { dst, g } => Some((dst, cached.contains(&g))),
        Op::Mov { dst, src } => Some((dst, known[pc].contains(src))),
        Op::VSet { v, .. } => Some((v, true)),
        _ => None,
    };
    // Native code goes on after a `VRef` only with a vector in `v`.
    let also = |pc: usize| match ops[pc] {
        Op::VRef { dst, v, .. } => (dst != v).then_some(v),
        _ => None,
    };
    known_regs(ops, n, captures, heads, shape, &HashMap::new(), step, also).regs
}

/// The registers whose kind some instruction uses: operands of arithmetic
/// and comparisons, and vector indices, and what is moved into them.
fn kind_used(ops: &[Op], n: usize) -> Regs {
    let mut used = Regs::empty(n);
    for op in ops {
        match *op {
            Op::Add { a, b, .. }
            | Op::Sub { a, b, .. }
            | Op::Mul { a, b, .. }
            | Op::Quo { a, b, .. }
            | Op::Rem { a, b, .. }
            | Op::Mod { a, b, .. }
            | Op::Lt { a, b, .. }
            | Op::Le { a, b, .. }
            | Op::NumEq { a, b, .. }
            | Op::JNLt { a, b, .. }
            | Op::JNLe { a, b, .. }
            | Op::JNNumEq { a, b, .. } => [a, b].into_iter().for_each(|r| used.insert(r)),
            Op::AddI { a, .. } | Op::JNLtI { a, .. } | Op::JNGtI { a, .. } | Op::JNEqI { a, .. } => used.insert(a),
            Op::VRef { i, .. } | Op::VSet { i, .. } => used.insert(i),
            _ => {}
        }
    }
    loop {
        let before = used.clone();
        for op in ops {
            if let Op::Mov { dst, src } = *op
                && used.contains(dst)
            {
                used.insert(src);
            }
        }
        if used == before {
            return used;
        }
    }
}

/// What `fixnums`, `flonums` and `vectors` share: the registers of a kind
/// on every path native code takes, given what each instruction does to
/// the kind of the register it sets (`step`: `Some((dst, of the kind))`,
/// or `None` for an instruction that sets its definitions to anything),
/// and a register it finds of the kind (`also`).
///
/// At an entry point with an assumption (`assumed`: registers of the kind
/// the interpreter saw there when it asked for the code), those are known
/// too: native code checks them on its way in where they are not known
/// (`Kinds::missing`, by edge), and the entry point's loader with the rest.
fn known_regs(
    ops: &[Op],
    n: usize,
    captures: &HashMap<u32, Vec<Reg>>,
    heads: &[usize],
    shape: &Shape,
    assumed: &HashMap<usize, Regs>,
    step: impl Fn(usize, &[Regs]) -> Option<(Reg, bool)>,
    also: impl Fn(usize) -> Option<Reg>,
) -> Kinds {
    let len = ops.len();
    // Native code runs from pc 0 and the entry points, along the edges of
    // the instructions it executes.
    let edges: Vec<Vec<usize>> =
        ops.iter().enumerate().map(|(pc, op)| if compiled(op) { shape.succs[pc].clone() } else { vec![] }).collect();
    let reach = |from: &[usize]| {
        let mut reached = vec![false; len];
        let mut todo = from.to_vec();
        while let Some(pc) = todo.pop() {
            if !std::mem::replace(&mut reached[pc], true) {
                todo.extend(&edges[pc]);
            }
        }
        reached
    };
    let from_start = reach(&[0]);
    let reached = reach(&heads.iter().copied().chain([0]).collect::<Vec<_>>());
    // Known everywhere to start with (the meet over the paths then lowers
    // it), but where nothing is: pc 0, and entry points native code does
    // not reach from there.
    let unknown = |pc: usize| pc == 0 || !reached[pc] || (heads.contains(&pc) && !from_start[pc]);
    let mut known: Vec<Regs> = (0..len)
        .map(|pc| match (unknown(pc), assumed.get(&pc)) {
            (true, Some(a)) => a.clone(),
            (true, None) => Regs::empty(n),
            (false, _) => Regs::full(n),
        })
        .collect();
    // What is known after the instruction at `pc`, on its edge to `s`.
    let out = |known: &[Regs], pc: usize, s: usize| {
        let mut k = known[pc].clone();
        match step(pc, known) {
            Some((dst, true)) => k.insert(dst),
            Some((dst, false)) => k.remove(dst),
            None => uses_defs(&ops[pc], captures).1.iter().for_each(|&d| k.remove(d)),
        }
        if let Some(r) = also(pc) {
            k.insert(r);
        }
        match shape.fresh(pc, s) {
            Some(fresh) => k.minus(fresh),
            None => k,
        }
    };
    loop {
        let mut changed = false;
        for pc in (0..len).filter(|&pc| reached[pc]) {
            for &s in &edges[pc] {
                let mut meet = known[s].and(&out(&known, pc, s));
                if let Some(a) = assumed.get(&s) {
                    meet.union(a);
                }
                changed |= meet != known[s];
                known[s] = meet;
            }
        }
        if !changed {
            break;
        }
    }
    let mut missing = HashMap::new();
    for pc in (0..len).filter(|&pc| reached[pc]) {
        for &s in &edges[pc] {
            if let Some(a) = assumed.get(&s) {
                let m = a.minus(&out(&known, pc, s));
                if m.iter().next().is_some() {
                    missing.insert((pc, s), m);
                }
            }
        }
    }
    Kinds { regs: known, missing }
}

/// Registers of a kind known at each instruction (`known_regs`), and on
/// the edges into entry points with assumptions, those native code checks.
struct Kinds {
    regs: Vec<Regs>,
    missing: HashMap<(usize, usize), Regs>,
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
        let heads = &job.heads[..];
        let frame = job.frame_size as usize;
        let effects: Vec<_> = job.ops.iter().map(|op| uses_defs(op, &job.captures)).collect();
        if effects.iter().flat_map(|(u, d)| u.iter().chain(d)).any(|&r| r as usize >= frame) {
            return None;
        }
        // The function's instructions, then those of the functions inlined,
        // whose registers are above.
        let (ops, origin, sites, returns) = combine(job);
        let orig = &ops[..];
        let n = sites.iter().map(|s| (s.off + s.frame) as usize).fold(frame, usize::max);
        let own_len = job.ops.len();
        let site_at: HashMap<usize, usize> = sites.iter().enumerate().map(|(i, s)| (s.call, i)).collect();
        let shape = Shape {
            succs: ops
                .iter()
                .enumerate()
                .map(|(pc, op)| {
                    let mut succs = match pc.cmp(&own_len) {
                        std::cmp::Ordering::Less => successors(op, pc, own_len),
                        std::cmp::Ordering::Equal => vec![],
                        std::cmp::Ordering::Greater => successors(op, pc, ops.len()),
                    };
                    succs.extend(site_at.get(&pc).map(|&i| sites[i].start));
                    succs
                })
                .collect(),
            entries: sites
                .iter()
                .map(|s| {
                    let mut fresh = Regs::empty(n);
                    s.fresh().for_each(|r| fresh.insert(r));
                    (s.call, (s.start, fresh))
                })
                .collect(),
            inlined: origin.iter().map(Option::is_some).collect(),
            results: sites
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.tail)
                .map(|(i, s)| {
                    let moves = (s.start..ops.len()).filter(|&pc| returns[pc] == Some(i));
                    let moves = moves.map(|pc| match ops[pc] {
                        Op::Mov { src, .. } => (pc, src),
                        op => unreachable!("not a return: {op:?}"),
                    });
                    (s.call, moves.collect())
                })
                .collect(),
        };
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
            after: helper(5, true),
            call_slow: helper(7, true),
            tail_slow: helper(5, true),
            room: helper(3, true),
            unusual: {
                // `jit_native_unusual`: its `Unusual` in two registers.
                let mut s = Signature::new(call_conv);
                s.params.extend(vec![AbiParam::new(I64); 6]);
                s.returns.extend([AbiParam::new(I64), AbiParam::new(I64)]);
                b.import_signature(s)
            },
            native: {
                // A `PlainFn`: its `NativeRet` in two registers.
                let mut s = Signature::new(call_conv);
                s.params.extend(vec![AbiParam::new(I64); 3]);
                s.returns.extend([AbiParam::new(I64), AbiParam::new(I64)]);
                b.import_signature(s)
            },
            jit: b.import_signature(jit_sig),
        };

        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let p = b.block_params(entry).to_vec();
        let vars: Vec<Variable> = (0..n).map(|_| b.declare_var(I64)).collect();
        let shifts: Vec<Variable> = (0..n).map(|_| b.declare_var(I64)).collect();
        let fvals: Vec<Variable> = (0..n).map(|_| b.declare_var(F64)).collect();
        let load_ctx = |b: &mut FunctionBuilder, off: usize| b.ins().load(I64, flags(), p[1], off as i32);
        let globals = load_ctx(&mut b, offset_of!(JitCtx, globals));
        let fuel = load_ctx(&mut b, offset_of!(JitCtx, fuel));
        let fuel_var = b.declare_var(I32);
        let self_jump = heads.contains(&0) && orig.iter().any(|op| matches!(op, Op::TailCall { .. } | Op::TailCallG { .. }));
        let loops = self_jump || orig.iter().any(|op| matches!(op, Op::Loop { .. }));
        let fuel_left = if loops { b.ins().load(I32, flags(), fuel, 0) } else { b.ins().iconst(I32, 0) };
        b.def_var(fuel_var, fuel_left);
        let regs_end = load_ctx(&mut b, offset_of!(JitCtx, regs_end));
        // In the entry block, before the jump to where this call starts:
        // available in every block.
        let cage = load_ctx(&mut b, offset_of!(JitCtx, cage));
        let code_table = load_ctx(&mut b, offset_of!(JitCtx, code_table));
        let depth = b.ins().uextend(I64, p[5]);
        let konst = |pc: usize, k: u32| {
            let pool = origin[pc].map_or(&job.consts, |o| &sites[o.site].const_bits);
            Value::from_bits(pool[k as usize])
        };
        // Assumed where the interpreter saw them (`Job::observed`), those
        // whose kind native code uses (`kind_used`): assuming others would
        // gain nothing, and leave frames to the interpreter when they
        // change (an accumulator growing into a bignum).
        let used = kind_used(orig, n);
        let assumed = |regs: &dyn Fn(&Observed) -> &Vec<Reg>| -> HashMap<usize, Regs> {
            job.observed
                .iter()
                .filter(|o| heads.contains(&o.head))
                .map(|o| {
                    let mut a = Regs::empty(n);
                    regs(o).iter().filter(|&&r| (r as usize) < n && used.contains(r)).for_each(|&r| a.insert(r));
                    (o.head, a)
                })
                .collect()
        };
        let flo = flonums(orig, n, konst, &job.captures, heads, &shape, &assumed(&|o| &o.floats));
        let fix = fixnums(
            orig,
            n,
            konst,
            &job.captures,
            heads,
            |pc| match orig[pc] {
                Op::CallG { g, .. } => job.native_globals.get(&g).is_some_and(|k| k.fixnum),
                _ => false,
            },
            &shape,
            &assumed(&|o| &o.fixnums),
        );
        // The edges native code checks assumptions on.
        let mut guards: HashMap<(usize, usize), (Regs, Regs)> = HashMap::new();
        for (&edge, m) in &fix.missing {
            guards.entry(edge).or_insert_with(|| (Regs::empty(n), Regs::empty(n))).0.union(m);
        }
        for (&edge, m) in &flo.missing {
            guards.entry(edge).or_insert_with(|| (Regs::empty(n), Regs::empty(n))).1.union(m);
        }
        let (fix, flo) = (fix.regs, flo.regs);
        let cached = cached_globals(orig, &shape.succs, &sites.iter().map(|s| s.call).collect::<Vec<_>>());
        let vec_globals: Vec<u32> = cached.iter().filter(|c| c.1).map(|c| c.0).collect();
        let vecs = vectors(orig, n, &job.captures, heads, &shape, &vec_globals);
        let inline = |pc: usize| match orig[pc] {
            Op::CallG { g, n, .. } => job.native_globals.get(&g).is_some_and(|k| k.inline(n).is_some()),
            _ => false,
        };
        let ant = anticipated(orig, &cached.iter().map(|c| c.0).collect::<Vec<_>>(), &shape.succs, inline);
        let mut g = Gen {
            vm: p[0],
            ctx: p[1],
            r: p[2],
            bp: p[3],
            depth,
            globals,
            fuel,
            fuel_var,
            loops,
            regs_end,
            cage,
            code_table,
            code_ptr: job.code as i64,
            entry0: heads.contains(&0),
            flow: analyze(
                orig,
                n,
                &job.captures,
                self_jump,
                &fix,
                |pc| match orig[pc] {
                    Op::CallG { g, n, .. } => job.native_globals.get(&g).map(|k| k.inline(n).is_some()),
                    _ => None,
                },
                &shape,
            ),
            fix,
            flo,
            vecs,
            ant,
            lens: (0..n).map(|_| (b.declare_var(I64), b.declare_var(I64))).collect(),
            cached: cached
                .iter()
                .map(|&(g, lens)| Cached { g, value: b.declare_var(I64), lens: lens.then(|| (b.declare_var(I64), b.declare_var(I64))) })
                .collect(),
            same: sites
                .iter()
                .enumerate()
                .filter(|(_, s)| matches!(orig[s.call], Op::CallG { g, .. } | Op::TailCallG { g, .. } if cached.iter().any(|c| c.0 == g)))
                .map(|(i, _)| (i, b.declare_var(I64)))
                .collect(),
            guards,
            guard_blocks: HashMap::new(),
            at: 0,
            vars,
            shifts,
            fvals,
            ops: orig.to_vec(),
            blocks: orig.iter().map(|op| compiled(op).then(|| b.create_block())).collect(),
            exits: HashMap::new(),
            sigs,
            closure_globals: job.closure_globals.clone(),
            native_globals: job.native_globals.clone(),
            own,
            consts: job.consts_addr as i64,
            const_bits: job.consts.clone(),
            inline_exits: HashMap::new(),
            redo: redo_points(orig, &origin, &sites, &shape.succs),
            origin,
            sites,
            site_at,
        };
        // Dead registers start as 0; each entry point loads its live ones.
        let zero = b.ins().iconst(I64, 0);
        g.vars.iter().chain(&g.shifts).for_each(|v| b.def_var(*v, zero));
        // (A length of 0 lets no access through.)
        g.lens.iter().chain(g.cached.iter().filter_map(|c| c.lens.as_ref())).for_each(|&(l, w)| {
            b.def_var(l, zero);
            b.def_var(w, zero);
        });
        g.cached.iter().map(|c| c.value).chain(g.same.iter().map(|s| s.1)).for_each(|v| b.def_var(v, zero));
        let fzero = b.ins().f64const(0.0);
        g.fvals.iter().for_each(|v| b.def_var(*v, fzero));
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
                (first..frame).for_each(|i| {
                    b.ins().store(flags(), zero, g.r, (i * 8) as i32);
                });
                let top = b.ins().iadd_imm_s(g.bp, frame as i64);
                g.raise_stack_top(&mut b, top);
            }
            if n > frame {
                g.room(&mut b, h, n);
            }
            g.reload(&mut b, &g.flow.live_in[h].clone());
            g.reload_globals(&mut b, h);
            for r in g.vecs[h].and(&g.flow.live_in[h]).iter() {
                let x = g.get(&mut b, r as Reg);
                let lens = g.lens_of(&mut b, x);
                g.set_lens(&mut b, r as Reg, lens);
            }
            // The interpreter may have stored something else where native
            // code knows fixnums or floats.
            let expected = g.fix[h].and(&g.flow.live_in[h]);
            let floats = g.flo[h].and(&g.flow.live_in[h]);
            if expected.iter().next().is_some() || floats.iter().next().is_some() {
                let bad = g.check_kinds(&mut b, &expected, &floats);
                let interpret = g.exit(&mut b, h, INTERPRET);
                b.ins().brif(bad, interpret, &[], g.blocks[h].unwrap(), &[]);
            } else {
                b.ins().jump(g.blocks[h].unwrap(), &[]);
            }
        }

        // Instructions native code reaches from more than one other.
        let mut preds = vec![0usize; orig.len()];
        let edges = orig.iter().enumerate().filter(|(_, op)| compiled(op)).flat_map(|(pc, _)| &shape.succs[pc]);
        edges.for_each(|&s| preds[s] += 1);
        for (pc, op) in orig.iter().enumerate() {
            if let Some(block) = g.blocks[pc] {
                b.switch_to_block(block);
                // Known fixnums are kept shifted too (`set_fixnum`), as most
                // uses want them. Where paths meet, the tagged form is made
                // from that one (where used), rather than carried along: a
                // loop carries only the shifted counter.
                let meet = preds[pc] > 1 || loaders.contains_key(&pc);
                for r in g.fix[pc].and(&g.flow.live_in[pc]).iter().filter(|_| meet) {
                    let s = b.use_var(g.shifts[r]);
                    let v = Gen::tag_shifted(&mut b, s);
                    b.def_var(g.vars[r], v);
                }
                // Known floats likewise, from their `f64`s (`set_float`).
                for r in g.flo[pc].and(&g.flow.live_in[pc]).iter().filter(|_| meet) {
                    let f = b.use_var(g.fvals[r]);
                    let v = Gen::from_f(&mut b, f);
                    b.def_var(g.vars[r], v);
                }
                // (Inlined instructions need no address: `Closure` is never one.)
                g.at = pc;
                g.op(&mut b, pc, op, (job.ops_addr + pc * std::mem::size_of::<Op>()) as i64);
            }
        }
        // Edges into entry points check what is assumed there and not
        // known on the way (`Gen::target`); if it does not hold, the
        // interpreter goes on from the entry point.
        let guard_blocks: Vec<_> = g.guard_blocks.iter().map(|(&edge, &block)| (edge, block)).collect();
        for ((from, to), block) in guard_blocks {
            b.switch_to_block(block);
            let (fx, fl) = &g.guards[&(from, to)];
            let live = g.flow.live_in[to].clone();
            let bad = g.check_kinds(&mut b, &fx.and(&live), &fl.and(&live));
            let fail = g.exit(&mut b, to, GUARD);
            b.ins().brif(bad, fail, &[], g.blocks[to].unwrap(), &[]);
        }
        let exits: Vec<_> = g.exits.iter().map(|(k, v)| (*k, *v)).collect();
        for ((pc, status), block) in exits {
            b.switch_to_block(block);
            if let Some(o) = g.origin[pc] {
                g.inline_exit(&mut b, pc, status, o);
                continue;
            }
            if matches!(status, EXIT | TICK | STEP | REDO) {
                g.spill_at(&mut b, pc);
            }
            if matches!(status, GUARD | GUARD_TICK) {
                for i in g.flow.dirty_in[pc].and(&g.flow.live_in[pc]).iter() {
                    let x = b.use_var(g.vars[i]);
                    b.ins().store(flags(), x, g.r, (i * 8) as i32);
                }
            }
            // Registers are as the interpreter left them.
            let status = match status {
                GUARD => INTERPRET,
                GUARD_TICK => TICK,
                REDO => {
                    // Native code counted the call: the interpreter counts it.
                    let f = b.ins().load(I32, flags(), g.fuel, 0);
                    let f = b.ins().iadd_imm_s(f, 1);
                    b.ins().store(flags(), f, g.fuel, 0);
                    EXIT
                }
                _ => status,
            };
            let code = b.ins().iconst(I64, g.code_ptr);
            b.ins().store(flags(), code, g.ctx, offset_of!(JitCtx, code) as i32);
            b.ins().store(flags(), g.bp, g.ctx, offset_of!(JitCtx, bp) as i32);
            let v = b.ins().iconst(I64, ((status as i64) << 32) | pc as i64);
            b.ins().return_(&[v]);
        }
        g.finish_inline_exits(&mut b);
        b.seal_all_blocks();
        b.finalize(frontend);
        if std::env::var_os("TECHNE_JIT_IR").is_some() {
            let f = &self.ctx.func;
            let insts: usize = f.layout.blocks().map(|bl| f.layout.block_insts(bl).count()).sum();
            eprintln!(
                "ir: {} {} ops ({} inlined at {} calls, {} tail) -> {} blocks, {insts} insts",
                job.name,
                own_len,
                orig.len() - own_len - 1,
                g.sites.len(),
                g.sites.iter().filter(|s| s.tail).count(),
                f.layout.blocks().count()
            );
            if std::env::var("TECHNE_JIT_IR").is_ok_and(|v| v == job.name) {
                eprintln!("{}", f.display());
            }
        }

        let asm = std::env::var("TECHNE_JIT_ASM").is_ok_and(|v| v == job.name);
        self.ctx.set_disasm(asm);
        module.define_function(id, &mut self.ctx).ok()?;
        if let Some(text) = self.ctx.compiled_code().and_then(|c| c.vcode.as_ref()).filter(|_| asm) {
            eprintln!("asm: {}\n{text}", job.name);
        }
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
    unusual: ir::SigRef,
    room: ir::SigRef,
    native: ir::SigRef,
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
    /// Where the fuel is counted: a counter the native code of one run of
    /// the interpreter's (`JitCtx::fuel`) shares.
    fuel: ir::Value,
    /// The fuel while this function runs, if it `loops`: counting writes it
    /// through to `fuel`, so that nothing has to write it back (callees,
    /// the interpreter), and calls of compiled code read it anew. Then
    /// counting does not wait for memory.
    fuel_var: Variable,
    /// The function loops (a back-edge or a tail call to itself). Else it
    /// counts in memory: reading the fuel when called and after each call
    /// would cost more.
    loops: bool,
    regs_end: ir::Value,
    /// `JitCtx::cage`: values name addresses in it (`Gen::ptr`).
    cage: ir::Value,
    /// `JitCtx::code_table`, which never moves (`Gen::callee`).
    code_table: ir::Value,
    code_ptr: i64,
    flow: Flow,
    /// Registers known to hold fixnums, by pc (`fixnums`).
    fix: Vec<Regs>,
    /// Registers known to hold floats, by pc (`flonums`).
    flo: Vec<Regs>,
    /// Registers whose `lens` are kept, by pc (`vectors`).
    vecs: Vec<Regs>,
    /// The globals kept native code may read, by pc (`anticipated`).
    ant: Vec<u16>,
    /// By register: the length of the vector it holds if it is one, and
    /// if it can be changed, else 0 (`Gen::lens_of`), where `vecs` has it.
    lens: Vec<(Variable, Variable)>,
    /// The globals native code keeps (`cached_globals`): read at entry
    /// points and after anything that may have changed them or moved what
    /// they hold, where native code may read them before it reads them
    /// anew (`reload_globals`).
    cached: Vec<Cached>,
    /// By call inlined through a global kept: whether the global holds the
    /// closure inlined (non-zero if so; see `enter_inlined`), kept with it.
    same: Vec<(usize, Variable)>,
    /// The fixnums and floats native code checks on an edge (from, to)
    /// into an entry point that assumes them (`known_regs`).
    guards: HashMap<(usize, usize), (Regs, Regs)>,
    /// Blocks checking them, filled after the instructions.
    guard_blocks: HashMap<(usize, usize), Block>,
    /// The instruction being compiled, whose edges `target` makes.
    at: usize,
    /// pc 0 is an entry point (its block starts the function).
    entry0: bool,
    vars: Vec<Variable>,
    /// The registers known to hold fixnums (`fix`) shifted (`Gen::shifted`),
    /// kept from where they become known (`set_fixnum`).
    shifts: Vec<Variable>,
    /// The registers known to hold floats (`flo`) as `f64`s, kept from
    /// where they become known (`set_float`).
    fvals: Vec<Variable>,
    ops: Vec<Op>,
    /// Native instruction blocks by pc.
    blocks: Vec<Option<Block>>,
    /// Exit blocks by (pc, status).
    exits: HashMap<(usize, u32), Block>,
    sigs: Sigs,
    closure_globals: HashMap<u32, Known>,
    native_globals: HashMap<u32, KnownNative>,
    /// This function, for direct recursive calls.
    own: ir::FuncRef,
    consts: i64,
    const_bits: Vec<u64>,
    /// Where each instruction is from, if inlined (`combine`).
    origin: Vec<Option<Inlined>>,
    /// The calls inlined, and their index by pc.
    sites: Vec<Site>,
    site_at: HashMap<usize, usize>,
    /// By call inlined and whether its frame is made: the end of exits
    /// from its callee (`inline_exit`).
    inline_exits: HashMap<(usize, bool), Block>,
    /// The inlined instructions where leaving native code makes the call
    /// in the interpreter instead (`redo_points`).
    redo: Vec<bool>,
}

/// A global native code keeps (`Gen::cached`), and its `lens` if it reads
/// vectors through it.
#[derive(Clone, Copy)]
struct Cached {
    g: u32,
    value: Variable,
    lens: Option<(Variable, Variable)>,
}

/// A register's value, and its shifted form (`Gen::shifted`) if it is
/// known to hold a fixnum.
type Num = (ir::Value, Option<ir::Value>);
/// Two operands' `f64`s, those known floats (`Gen::fval`).
type Known2 = (Option<ir::Value>, Option<ir::Value>);

fn flags() -> MemFlagsData {
    MemFlagsData::trusted()
}

impl Gen {
    /// Re-read the fuel after a call that may have counted.
    fn load_fuel(&self, b: &mut FunctionBuilder) {
        if !self.loops {
            return;
        }
        let f = b.ins().load(I32, flags(), self.fuel, 0);
        b.def_var(self.fuel_var, f);
    }

    /// Before leaving native code or running code that reads registers or
    /// may collect, at instruction `pc`: store the changed registers that are
    /// still needed (live sets include what `guard` landings read). Known
    /// fixnums are tagged here, so that exits shared by several paths do
    /// not make each tag them.
    fn spill_at(&self, b: &mut FunctionBuilder, pc: usize) {
        self.spill(b, pc, &self.flow.dirty_in[pc].and(&self.flow.live_in[pc]));
    }

    /// Store registers `regs` at instruction `pc` (as `spill_at`).
    fn spill(&self, b: &mut FunctionBuilder, pc: usize, regs: &Regs) {
        self.spill_to(b, pc, regs, self.r);
    }

    /// `spill` to the frame's registers at `r`.
    fn spill_to(&self, b: &mut FunctionBuilder, pc: usize, regs: &Regs, r: ir::Value) {
        for i in regs.iter() {
            let x = if self.fix[pc].contains(i as Reg) {
                let s = b.use_var(self.shifts[i]);
                Self::tag_shifted(b, s)
            } else {
                b.use_var(self.vars[i])
            };
            b.ins().store(flags(), x, r, (i * 8) as i32);
        }
    }

    /// The registers a call of a known native at `pc` stores only when
    /// native code is left after it (`deferred`).
    fn deferred_spills(&self, pc: usize) -> Regs {
        deferred(&self.ops[pc], &self.flow.dirty_in[pc], &self.flow.live_in[pc], &self.fix[pc])
    }

    fn reload(&self, b: &mut FunctionBuilder, regs: &Regs) {
        for i in regs.iter() {
            let x = b.ins().load(I64, flags(), self.r, (i * 8) as i32);
            b.def_var(self.vars[i], x);
        }
    }

    /// After instruction `pc` ran outside native code, which stored the
    /// registers before (`spill_at`): re-read those needed afterwards (the
    /// GC may have moved what they point to). The fixnums known too, their
    /// shifted forms made anew: cheaper than keeping both forms in machine
    /// registers across the call.
    fn reload_after(&self, b: &mut FunctionBuilder, pc: usize) {
        let live = &self.flow.live_out[pc];
        self.reload(b, live);
        let mut known = self.fix[pc].and(live);
        uses_defs(&self.ops[pc], &HashMap::new()).1.iter().for_each(|&d| known.remove(d));
        for r in known.iter() {
            let x = b.use_var(self.vars[r]);
            let s = Self::shifted(b, x);
            b.def_var(self.shifts[r], s);
        }
        self.reload_globals(b, pc + 1);
    }

    /// After a call of a known native at `pc` (`known_native`): as
    /// `reload_after`, but for `except` and the fixnums known, which may
    /// not be in memory (`deferred`), and nothing changed.
    fn reload_except(&self, b: &mut FunctionBuilder, pc: usize, except: Option<Reg>) {
        let mut live = self.flow.live_out[pc].clone();
        if let Some(r) = except {
            live.remove(r);
        }
        let mut known = self.fix[pc].clone();
        uses_defs(&self.ops[pc], &HashMap::new()).1.iter().for_each(|&d| known.remove(d));
        known.iter().for_each(|r| live.remove(r as Reg));
        self.reload(b, &live);
        self.reload_globals(b, pc + 1);
    }

    /// Read the globals native code keeps (`cached`) anew, those it may
    /// read from instruction `pc` on (`ant`).
    fn reload_globals(&self, b: &mut FunctionBuilder, pc: usize) {
        let ant = self.ant.get(pc).copied().unwrap_or(0);
        let wanted = |g: u32| self.cached.iter().position(|c| c.g == g).is_some_and(|i| ant & (1 << i) != 0);
        for c in self.cached.iter().filter(|c| wanted(c.g)) {
            let x = b.ins().load(I64, flags(), self.globals, (c.g * 8) as i32);
            b.def_var(c.value, x);
            if let Some(lens) = c.lens {
                let (l, w) = self.lens_of(b, x);
                b.def_var(lens.0, l);
                b.def_var(lens.1, w);
            }
        }
        for &(site, same) in &self.same {
            let (Op::CallG { g, .. } | Op::TailCallG { g, .. }) = self.ops[self.sites[site].call] else { unreachable!() };
            if !wanted(g) {
                continue;
            }
            let f = self.global(b, g);
            let expected = self.site_closure(b, site);
            let eq = b.ins().icmp(IntCC::Equal, f, expected);
            let eq = b.ins().uextend(I64, eq);
            b.def_var(same, eq);
        }
    }

    /// The value of global `g` (unbound or not), kept or read.
    fn global(&self, b: &mut FunctionBuilder, g: u32) -> ir::Value {
        match self.cached.iter().find(|c| c.g == g) {
            Some(c) => b.use_var(c.value),
            None => b.ins().load(I64, flags(), self.globals, (g * 8) as i32),
        }
    }

    /// The length of `x` if it is a vector, and if it is one that can be
    /// changed; else 0.
    fn lens_of(&self, b: &mut FunctionBuilder, x: ir::Value) -> (ir::Value, ir::Value) {
        let (object, done) = (b.create_block(), b.create_block());
        b.append_block_param(done, I64);
        b.append_block_param(done, I64);
        let zero = b.ins().iconst(I64, 0);
        let ptr = Self::is_ptr(b, x);
        b.ins().brif(ptr, object, &[], done, &[BlockArg::Value(zero), BlockArg::Value(zero)]);
        b.switch_to_block(object);
        let p = self.ptr(b, x);
        let h = b.ins().load(I64, flags(), p, 0);
        let (l, w) = Self::header_lens(b, h);
        b.ins().jump(done, &[BlockArg::Value(l), BlockArg::Value(w)]);
        b.switch_to_block(done);
        (b.block_params(done)[0], b.block_params(done)[1])
    }

    /// The length in header `h`, bounded as `heap::header_len` bounds it:
    /// what follows an object stays in its cage or its guard.
    fn header_len(b: &mut FunctionBuilder, h: ir::Value) -> ir::Value {
        let len = b.ins().ushr_imm_s(h, 16);
        b.ins().band_imm_s(len, crate::alloc::CAGE_MASK as i64)
    }

    /// `lens_of` an object with header `h`.
    fn header_lens(b: &mut FunctionBuilder, h: ir::Value) -> (ir::Value, ir::Value) {
        let zero = b.ins().iconst(I64, 0);
        let len = Self::header_len(b, h);
        let kind = b.ins().band_imm_s(h, 0xFF);
        let vector = b.ins().icmp_imm_s(IntCC::Equal, kind, Kind::Vector as i64);
        let l = b.ins().select(vector, len, zero);
        let bits = b.ins().band_imm_s(h, 0xFF | crate::heap::IMMUTABLE as i64);
        let changeable = b.ins().icmp_imm_s(IntCC::Equal, bits, Kind::Vector as i64);
        let w = b.ins().select(changeable, len, zero);
        (l, w)
    }

    fn set_lens(&self, b: &mut FunctionBuilder, r: Reg, (l, w): (ir::Value, ir::Value)) {
        b.def_var(self.lens[r as usize].0, l);
        b.def_var(self.lens[r as usize].1, w);
    }

    fn use_lens(&self, b: &mut FunctionBuilder, (l, w): (Variable, Variable)) -> (ir::Value, ir::Value) {
        (b.use_var(l), b.use_var(w))
    }

    /// The closure the call inlined at `site` called, read anew: what
    /// native code held of it is stale after a collection.
    fn site_closure(&self, b: &mut FunctionBuilder, site: usize) -> ir::Value {
        let addr = b.ins().iconst(I64, self.sites[site].closure);
        b.ins().load(I64, flags(), addr, 0)
    }

    /// The closure running instruction `pc`: the frame's, or that of the
    /// call inlining it.
    fn closure(&self, b: &mut FunctionBuilder, pc: usize) -> ir::Value {
        match self.origin[pc] {
            Some(o) => self.site_closure(b, o.site),
            None => b.ins().load(I64, flags(), self.r, -8),
        }
    }

    /// The frames native code leaving in the callee of the call inlined at
    /// `site` makes (`frames`), the registers they span, and whether the
    /// compiled function's own frame is below (no tail call replaced it).
    fn span(&self, site: usize) -> (Vec<usize>, std::ops::Range<usize>, bool) {
        let frames = frames(&self.sites, site);
        let lo = frames.iter().map(|&f| self.sites[f].off as usize).min().unwrap();
        let hi = frames.iter().map(|&f| (self.sites[f].off + self.sites[f].frame) as usize).max().unwrap();
        (frames, lo..hi, !self.sites[root(&self.sites, site)].tail)
    }

    /// Store the frame of the call that inlined instruction `pc`, at its
    /// call `site`, as the call would have made it, and those of the calls
    /// inlined that led to it, with the caller's registers needed: the
    /// callees' live registers (others cleared, as their prologues would),
    /// their closures in their callee slots, the GC's root window covering
    /// them. Before a native it calls; leaving native code there stores it
    /// as well, once for all exits (`inline_exit`).
    fn materialize(&self, b: &mut FunctionBuilder, pc: usize, site: usize) {
        self.materialize_regs(b, pc, site);
        self.materialize_frame(b, site);
    }

    /// The registers `materialize` stores.
    fn materialize_regs(&self, b: &mut FunctionBuilder, pc: usize, site: usize) {
        let (_, span, below) = self.span(site);
        // After a tail call, nothing of the caller's is needed.
        if below {
            let mut caller = self.flow.dirty_in[pc].and(&self.flow.live_in[pc]);
            span.clone().for_each(|r| caller.remove(r as Reg));
            self.spill(b, pc, &caller);
        }
        let zero = b.ins().iconst(I64, 0);
        for i in span {
            let x = if !self.flow.live_in[pc].contains(i as Reg) {
                zero
            } else if self.fix[pc].contains(i as Reg) {
                let s = b.use_var(self.shifts[i]);
                Self::tag_shifted(b, s)
            } else {
                b.use_var(self.vars[i])
            };
            b.ins().store(flags(), x, self.r, (i * 8) as i32);
        }
    }

    /// The rest of `materialize`: the closures, each below its callee's
    /// registers (in the caller's slot for a tail call), the GC's root
    /// window.
    fn materialize_frame(&self, b: &mut FunctionBuilder, site: usize) {
        let (frames, span, _) = self.span(site);
        for &f in frames.iter().rev() {
            let c = self.site_closure(b, f);
            b.ins().store(flags(), c, self.r, (self.sites[f].off as i32 - 1) * 8);
        }
        let top = b.ins().iadd_imm_s(self.bp, span.end as i64);
        self.raise_stack_top(b, top);
    }

    /// Leave native code with `status` at inlined instruction `pc` (from
    /// `o`), where the interpreter cannot make the call instead (`REDO`):
    /// in the frame of the call inlining it, made unless before the native
    /// call this follows (`materialize`), with its callers' frames below,
    /// each returning after its call (`jit_unwind_push`). One block per call
    /// inlined does that (`inline_exits`); each exit only sets what of its
    /// registers that block stores.
    fn inline_exit(&mut self, b: &mut FunctionBuilder, pc: usize, status: u32, o: Inlined) {
        let made = !matches!(status, EXIT | TICK | STEP);
        if !made {
            // The callees' registers as `materialize_regs` stores them, for
            // the block storing them for all exits (`finish_inline_exits`).
            let zero = b.ins().iconst(I64, 0);
            for i in self.span(o.site).1 {
                if !self.flow.live_in[pc].contains(i as Reg) {
                    b.def_var(self.vars[i], zero);
                } else if self.fix[pc].contains(i as Reg) {
                    let s = b.use_var(self.shifts[i]);
                    let x = Self::tag_shifted(b, s);
                    b.def_var(self.vars[i], x);
                }
            }
        }
        let v = b.ins().iconst(I64, ((status as i64) << 32) | o.pc as i64);
        let rest = *self.inline_exits.entry((o.site, made)).or_insert_with(|| {
            let block = b.create_block();
            b.append_block_param(block, I64);
            block
        });
        b.ins().jump(rest, &[BlockArg::Value(v)]);
    }

    /// The blocks `inline_exit` jumps to.
    fn finish_inline_exits(&self, b: &mut FunctionBuilder) {
        for (&(site, made), &block) in &self.inline_exits {
            b.switch_to_block(block);
            let v = b.block_params(block)[0];
            if !made {
                // Each exit's registers (`inline_exit`), and the caller's
                // registers needed after the outermost call, which the
                // callees do not change (stored again if a native one
                // called stored them).
                let (_, span, below) = self.span(site);
                if below {
                    let s = &self.sites[root(&self.sites, site)];
                    let mut caller = self.flow.dirty_in[s.call].and(&self.flow.live_out[s.call]);
                    span.clone().for_each(|r| caller.remove(r as Reg));
                    caller.remove(s.base);
                    self.spill(b, s.call, &caller);
                }
                for i in span {
                    let x = b.use_var(self.vars[i]);
                    b.ins().store(flags(), x, self.r, (i * 8) as i32);
                }
                self.materialize_frame(b, site);
            }
            // The frames below, innermost first, each continuing after its
            // call; a tail call left none of its caller's.
            let mut s = site;
            loop {
                let x = &self.sites[s];
                if !x.tail {
                    let (code, off) = x.parent.map_or((self.code_ptr, 0), |p| (self.sites[p].code, self.sites[p].off));
                    let (code, ret) = (b.ins().iconst(I64, code), b.ins().iconst(I64, x.ret_pc as i64));
                    let bp = b.ins().iadd_imm_s(self.bp, off as i64);
                    self.call(b, self.sigs.unwind, jit_unwind_push as *const (), &[self.vm, code, ret, bp]);
                }
                match x.parent {
                    Some(p) => s = p,
                    None => break,
                }
            }
            let s = &self.sites[site];
            let code = b.ins().iconst(I64, s.code);
            b.ins().store(flags(), code, self.ctx, offset_of!(JitCtx, code) as i32);
            let bp = b.ins().iadd_imm_s(self.bp, s.off as i64);
            b.ins().store(flags(), bp, self.ctx, offset_of!(JitCtx, bp) as i32);
            b.ins().return_(&[v]);
        }
    }

    /// At entry point `h`, before anything is loaded: continue if the
    /// frame's `n` registers, those of the functions inlined included, fit
    /// the register stack, so that calls run inline need not check it. Else
    /// let `jit_make_room` grow it and enter again (`RESUME`), or, if it
    /// cannot, interpret the function from now on (`EXIT`).
    fn room(&mut self, b: &mut FunctionBuilder, h: usize, n: usize) {
        let top = b.ins().iadd_imm_s(self.r, n as i64 * 8);
        let fits = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, top, self.regs_end);
        let (grow, ok) = (b.create_block(), b.create_block());
        b.ins().brif(fits, ok, &[], grow, &[]);
        b.set_cold_block(grow);
        b.switch_to_block(grow);
        let code = b.ins().iconst(I64, self.code_ptr);
        let needed = b.ins().iadd_imm_s(self.bp, n as i64);
        let status = self.call(b, self.sigs.room, jit_make_room as *const (), &[self.vm, code, needed]).unwrap();
        b.ins().store(flags(), code, self.ctx, offset_of!(JitCtx, code) as i32);
        b.ins().store(flags(), self.bp, self.ctx, offset_of!(JitCtx, bp) as i32);
        let res = b.ins().bor_imm_s(status, h as i64);
        b.ins().return_(&[res]);
        b.switch_to_block(ok);
    }

    /// The block leaving native code with `status` at `pc`. An inlined
    /// instruction's exit to the interpreter or to Rust may be its call's
    /// (`REDO`): the exits of a call inlined then share one block, with no
    /// frame to make.
    fn exit(&mut self, b: &mut FunctionBuilder, pc: usize, status: u32) -> Block {
        let key = match self.origin[pc] {
            Some(o) if self.redo[pc] && matches!(status, EXIT | STEP) => (self.sites[root(&self.sites, o.site)].call, REDO),
            _ => (pc, status),
        };
        *self.exits.entry(key).or_insert_with(|| {
            let block = b.create_block();
            b.set_cold_block(block);
            block
        })
    }

    /// Leave native code to run instruction `pc` in Rust (its rare cases).
    fn step(&mut self, b: &mut FunctionBuilder, pc: usize) -> Block {
        self.exit(b, pc, STEP)
    }

    /// The block that executes instruction `pc`.
    fn target(&mut self, b: &mut FunctionBuilder, pc: usize) -> Block {
        let Some(block) = self.blocks.get(pc).copied().flatten() else { return self.exit(b, pc, EXIT) };
        if self.guarded(pc) {
            return *self.guard_blocks.entry((self.at, pc)).or_insert_with(|| b.create_block());
        }
        block
    }

    /// Whether the edge from the instruction being compiled to `pc` checks
    /// assumptions there (`guards`): until it has, what is known at `pc`
    /// does not hold.
    fn guarded(&self, pc: usize) -> bool {
        self.guards.get(&(self.at, pc)).is_some_and(|(fx, fl)| {
            let live = &self.flow.live_in[pc];
            fx.and(live).iter().chain(fl.and(live).iter()).next().is_some()
        })
    }

    /// Whether any of registers `fixnums` is not a fixnum or of `floats`
    /// not a float (an `I64`, non-zero if so), defining their shifted and
    /// `f64` forms.
    fn check_kinds(&self, b: &mut FunctionBuilder, fixnums: &Regs, floats: &Regs) -> ir::Value {
        let mut bad = b.ins().iconst(I64, 0);
        for r in fixnums.iter() {
            let x = self.get(b, r as Reg);
            let t = b.ins().bxor_imm_s(x, TAG_INT << 48);
            bad = b.ins().bor(bad, t);
            let s = Self::shifted(b, x);
            b.def_var(self.shifts[r], s);
        }
        let mut bad = b.ins().ushr_imm_s(bad, 48);
        for r in floats.iter() {
            let x = self.get(b, r as Reg);
            // Not a float: at or past the tags (`Gen::is_float`).
            let not = b.ins().icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, x, TAG_INT << 48);
            let not = b.ins().uextend(I64, not);
            bad = b.ins().bor(bad, not);
            let f = b.ins().bitcast(F64, MemFlagsData::new(), x);
            b.def_var(self.fvals[r], f);
        }
        bad
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
        let f = if self.loops { b.use_var(self.fuel_var) } else { b.ins().load(I32, flags(), self.fuel, 0) };
        let f = b.ins().iadd_imm_s(f, -1);
        b.def_var(self.fuel_var, f);
        b.ins().store(flags(), f, self.fuel, 0);
        let tick = self.exit(b, resume, if self.guarded(resume) { GUARD_TICK } else { TICK });
        Self::guard(b, f, tick);
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
        let p = self.check_kind(b, f, Kind::Closure, no);
        // The closure's first field is its code's id: its slot in the VM's
        // table of codes, masked (`heap::code_slot`), which a closure in
        // the heap cannot lead out of, holds its code. A slot of no live
        // code (the closure forged, or stale) goes to `no`, where the
        // interpreter refuses it too (`Vm::closure_code`).
        let w = b.ins().load(I64, flags(), p, 8);
        let id = b.ins().band_imm_s(w, crate::heap::CODE_MASK as i64);
        let off = b.ins().ishl_imm_s(id, 3);
        let slot = b.ins().iadd(self.code_table, off);
        let code = b.ins().load(I64, flags(), slot, 0);
        let live = b.ins().icmp_imm_s(IntCC::NotEqual, code, 0);
        Self::guard(b, live, no);
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
        let end = b.ins().load(I64, flags(), end, 0);
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

    /// The address a heap object's value names: its offset in the cage,
    /// from the cage's base, as `alloc::caged` decodes it (without its
    /// word alignment, which confinement does not need). A value of an
    /// object is its address already; any other (forged, stale or of
    /// another cage) still names memory of this cage. A 4 GiB cage takes
    /// a 32-bit move and an add, as V8's pointer cage does.
    fn ptr(&self, b: &mut FunctionBuilder, x: ir::Value) -> ir::Value {
        let offset = if crate::alloc::CAGE_MASK == u32::MAX as usize {
            let low = b.ins().ireduce(I32, x);
            b.ins().uextend(I64, low)
        } else {
            b.ins().band_imm_s(x, crate::alloc::CAGE_MASK as i64)
        };
        b.ins().iadd(self.cage, offset)
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
    fn check_kind(&self, b: &mut FunctionBuilder, x: ir::Value, kind: Kind, no: Block) -> ir::Value {
        self.check_header(b, x, kind, 0xFF, no)
    }

    fn check_header(&self, b: &mut FunctionBuilder, x: ir::Value, kind: Kind, mask: i64, no: Block) -> ir::Value {
        self.check_bits(b, x, mask, kind as i64, no).0
    }

    /// Continue if `x` is a heap object whose header's bits in `mask` are
    /// `bits`, else go to `no`. Returns its address and header.
    fn check_bits(&self, b: &mut FunctionBuilder, x: ir::Value, mask: i64, bits: i64, no: Block) -> (ir::Value, ir::Value) {
        let ptr = Self::is_ptr(b, x);
        Self::guard(b, ptr, no);
        let p = self.ptr(b, x);
        let h = b.ins().load(I64, flags(), p, 0);
        let k = b.ins().band_imm_s(h, mask);
        let ok = b.ins().icmp_imm_s(IntCC::Equal, k, bits);
        Self::guard(b, ok, no);
        (p, h)
    }

    fn barrier(&self, b: &mut FunctionBuilder, obj: ir::Value, v: ir::Value) {
        let call = b.create_block();
        let done = b.create_block();
        let ptr = Self::is_ptr(b, v);
        b.ins().brif(ptr, call, &[], done, &[]);
        b.set_cold_block(call);
        b.switch_to_block(call);
        self.call(b, self.sigs.barrier, jit_barrier as *const (), &[self.vm, obj, v]);
        b.ins().jump(done, &[]);
        b.switch_to_block(done);
    }

    /// Register `r` at `pc`, with its shifted form if a known fixnum.
    fn num(&self, b: &mut FunctionBuilder, pc: usize, r: Reg) -> Num {
        let s = self.fix[pc].contains(r).then(|| b.use_var(self.shifts[r as usize]));
        (self.get(b, r), s)
    }

    /// Fixnum `i`.
    fn fixnum(b: &mut FunctionBuilder, i: i64) -> Num {
        (Self::imm(b, Value::int_unchecked(i)), Some(b.ins().iconst(I64, i << 16)))
    }

    /// Register `r` at `pc` as an `f64`, if a known float.
    fn fval(&self, b: &mut FunctionBuilder, pc: usize, r: Reg) -> Option<ir::Value> {
        self.flo[pc].contains(r).then(|| b.use_var(self.fvals[r as usize]))
    }

    /// Set register `r` to the float `f`.
    fn set_float(&self, b: &mut FunctionBuilder, r: Reg, f: ir::Value) {
        b.def_var(self.fvals[r as usize], f);
        let v = Self::from_f(b, f);
        self.set(b, r, v);
    }

    /// Set register `r` to the fixnum whose shifted form is `s`.
    fn set_fixnum(&self, b: &mut FunctionBuilder, r: Reg, s: ir::Value) {
        b.def_var(self.shifts[r as usize], s);
        let v = Self::tag_shifted(b, s);
        self.set(b, r, v);
    }

    /// The shifted form of `x`.
    fn shift(b: &mut FunctionBuilder, x: Num) -> ir::Value {
        x.1.unwrap_or_else(|| Self::shifted(b, x.0))
    }

    /// Whether `x` and `y` are both fixnums; `None` if known.
    fn both_int(b: &mut FunctionBuilder, x: Num, y: Num) -> Option<ir::Value> {
        match (x.1.is_some(), y.1.is_some()) {
            (true, true) => None,
            (true, false) => Some(Self::is_int(b, y.0)),
            (false, true) => Some(Self::is_int(b, x.0)),
            (false, false) => {
                let (a, c) = (b.ins().bxor_imm_s(x.0, TAG_INT << 48), b.ins().bxor_imm_s(y.0, TAG_INT << 48));
                let tags = b.ins().bor(a, c);
                Some(b.ins().icmp_imm_s(IntCC::UnsignedLessThan, tags, 1 << 48))
            }
        }
    }

    /// Registers `a` and `c` at `pc` as `f64`s, those known floats.
    fn floats_known(&self, b: &mut FunctionBuilder, pc: usize, a: Reg, c: Reg) -> Known2 {
        (self.fval(b, pc, a), self.fval(b, pc, c))
    }

    /// `x` as an `f64`: `known` if a known float's, else a float's bits or
    /// a fixnum converted; anything else goes to `slow`.
    fn as_f64(b: &mut FunctionBuilder, x: Num, known: Option<ir::Value>, slow: Block) -> ir::Value {
        if let Some(f) = known {
            return f;
        }
        if let Some(s) = x.1 {
            let i = b.ins().sshr_imm_s(s, 16);
            return b.ins().fcvt_from_sint(F64, i);
        }
        let done = b.create_block();
        b.append_block_param(done, F64);
        let (float, other, int) = (b.create_block(), b.create_block(), b.create_block());
        let is_float = Self::is_float(b, x.0);
        b.ins().brif(is_float, float, &[], other, &[]);
        b.switch_to_block(float);
        let f = b.ins().bitcast(F64, MemFlagsData::new(), x.0);
        b.ins().jump(done, &[BlockArg::Value(f)]);
        b.switch_to_block(other);
        let is_int = Self::is_int(b, x.0);
        b.ins().brif(is_int, int, &[], slow, &[]);
        b.switch_to_block(int);
        let f = Self::to_f(b, x.0);
        b.ins().jump(done, &[BlockArg::Value(f)]);
        b.switch_to_block(done);
        b.block_params(done)[0]
    }

    /// Arithmetic: fixnums (overflow steps out), else numbers as floats.
    /// With an operand known a float (`kf`), floats only (`flonums`).
    #[allow(clippy::too_many_arguments)]
    fn arith(&mut self, b: &mut FunctionBuilder, pc: usize, dst: Reg, xn: Num, yn: Num, kf: Known2, kind: char) {
        let slow = self.step(b, pc);
        if kf.0.is_some() || kf.1.is_some() {
            let (fa, fb) = (Self::as_f64(b, xn, kf.0, slow), Self::as_f64(b, yn, kf.1, slow));
            let f = match kind {
                '+' => b.ins().fadd(fa, fb),
                '-' => b.ins().fsub(fa, fb),
                _ => b.ins().fmul(fa, fb),
            };
            self.set_float(b, dst, f);
            self.jump(b, pc + 1);
            return;
        }
        let (x, y) = (xn.0, yn.0);
        let float = Self::both_int(b, xn, yn).map(|both| {
            let (int, float) = (b.create_block(), b.create_block());
            b.ins().brif(both, int, &[], float, &[]);
            b.switch_to_block(int);
            float
        });
        let a = Self::shift(b, xn);
        let (s, overflow) = match kind {
            '+' => {
                let c = Self::shift(b, yn);
                b.ins().sadd_overflow(a, c)
            }
            '-' => {
                let c = Self::shift(b, yn);
                b.ins().ssub_overflow(a, c)
            }
            _ => {
                let c = match yn.1 {
                    Some(s) => b.ins().sshr_imm_s(s, 16),
                    None => Self::untag(b, y),
                };
                b.ins().smul_overflow(a, c)
            }
        };
        let ok = b.create_block();
        b.ins().brif(overflow, slow, &[], ok, &[]);
        b.switch_to_block(ok);
        self.set_fixnum(b, dst, s);
        self.jump(b, pc + 1);

        let Some(float) = float else { return };
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

    /// A numeric comparison: goes on with `then` and its result (an `I8`),
    /// once for fixnums and once for floats; with an operand known a float
    /// (`kf`), for floats only. Non-numbers step out.
    fn compare(
        &mut self,
        b: &mut FunctionBuilder,
        pc: usize,
        xn: Num,
        yn: Num,
        kf: Known2,
        cc: (IntCC, FloatCC),
        then: impl Fn(&mut Self, &mut FunctionBuilder, ir::Value),
    ) {
        if kf.0.is_some() || kf.1.is_some() {
            let slow = self.step(b, pc);
            let (fa, fb) = (Self::as_f64(b, xn, kf.0, slow), Self::as_f64(b, yn, kf.1, slow));
            let r = b.ins().fcmp(cc.1, fa, fb);
            then(self, b, r);
            return;
        }
        let (x, y) = (xn.0, yn.0);
        let float = Self::both_int(b, xn, yn).map(|both| {
            let (int, float) = (b.create_block(), b.create_block());
            b.ins().brif(both, int, &[], float, &[]);
            b.switch_to_block(int);
            float
        });
        // Fixnums compare as their payloads shifted to the top (with the
        // sign); equal ones also have equal bits.
        let r = match (xn.1, yn.1) {
            (None, None) if cc.0 == IntCC::Equal => b.ins().icmp(cc.0, x, y),
            _ => {
                let (a, c) = (Self::shift(b, xn), Self::shift(b, yn));
                b.ins().icmp(cc.0, a, c)
            }
        };
        then(self, b, r);
        let Some(float) = float else { return };
        b.switch_to_block(float);
        let slow = self.step(b, pc);
        let (fa, fb) = self.floats(b, x, y, slow);
        let r = b.ins().fcmp(cc.1, fa, fb);
        then(self, b, r);
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
        if self.origin[pc].is_some() && self.site_at.contains_key(&pc) {
            return self.nested_call(b, pc, op);
        }
        if let Some(o) = self.origin[pc] {
            return self.inlined_call(b, pc, op, o);
        }
        let next = pc + 1;
        let (base, n, global) = match *op {
            Op::CallG { base, n, g } => (base, n, Some(g)),
            Op::Call { base, n } => (base, n, None),
            _ => unreachable!(),
        };
        let f = match global {
            Some(g) => self.global(b, g),
            None => self.get(b, base),
        };
        let slow = b.create_block();
        let done = b.create_block();
        let bail = b.create_block();
        b.append_block_param(bail, I64);
        let returned = b.create_block();
        b.append_block_param(returned, I64);
        // Calls through globals that hold no closure skip the inline path.
        let known = global.and_then(|g| self.closure_globals.get(&g).copied());
        let native = global.and_then(|g| self.native_globals.get(&g).copied());
        let fixnum = native.is_some_and(|k| k.fixnum);
        // What native code knows a fixnum after the call, whatever path
        // the call took.
        let checked = self.fix.get(next).is_some_and(|f| f.contains(base));
        if let Some(k) = native {
            self.known_native(b, pc, base, n, f, k, slow, returned, false, |g, b, v| {
                if fixnum {
                    let s = Self::shifted(b, v);
                    g.set_fixnum(b, base, s);
                } else {
                    g.set(b, base, v);
                }
                g.jump(b, next);
            });
        } else if let Some(&site) = self.site_at.get(&pc) {
            // The callee's instructions inline, if the global still holds
            // its closure (its frame fits: see `Gen::room`); else the call
            // through `jit_call_slow`, which costs little to compile.
            self.enter_inlined(b, pc, f, site, slow);
        } else if global.is_none() || known.is_some() {
            self.inline_call(b, pc, base, n, f, known, slow, done, bail);
        } else {
            b.ins().jump(slow, &[]);
        }

        // Rust natives, rest arguments: see `jit_call_slow`.
        b.switch_to_block(slow);
        self.spill_at(b, pc);
        let (base_v, n_v) = (b.ins().iconst(I64, base as i64), b.ins().iconst(I64, n as i64));
        let k = self
            .call(b, self.sigs.call_slow, jit_call_slow as *const (), &[self.vm, self.ctx, self.bp, base_v, n_v, self.depth, f])
            .unwrap();
        self.load_fuel(b);
        b.ins().jump(returned, &[BlockArg::Value(k)]);
        b.switch_to_block(returned);
        let k = b.block_params(returned)[0];
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
        if checked {
            // Another procedure than the native (a redefinition), or a call
            // of a function inlined elsewhere, returned what native code
            // knows to be a fixnum: unless it is, let the interpreter
            // continue (its result is in the base register).
            let x = self.get(b, base);
            let int = Self::is_int(b, x);
            let other = self.exit(b, next, RESUME);
            Self::guard(b, int, other);
            let s = Self::shifted(b, x);
            b.def_var(self.shifts[base as usize], s);
        }
        self.jump(b, next);
    }

    /// Run the callee of call `pc` inline (`sites[site]`) if `f` is the
    /// closure it expects, else go to `slow`.
    fn enter_inlined(&mut self, b: &mut FunctionBuilder, pc: usize, f: ir::Value, site: usize, slow: Block) {
        let same = match self.same.iter().find(|s| s.0 == site) {
            Some(&(_, same)) => b.use_var(same),
            None => {
                let expected = self.site_closure(b, site);
                b.ins().icmp(IntCC::Equal, f, expected)
            }
        };
        Self::guard(b, same, slow);
        // Counted as a call.
        self.tick(b, pc);
        let zero = b.ins().iconst(I64, 0);
        self.sites[site].fresh().for_each(|r| b.def_var(self.vars[r as usize], zero));
        self.jump(b, self.sites[site].start);
    }

    /// A call or tail call inlined in an inlined function, nested: if the
    /// global holds another procedure, native code is left for the
    /// interpreter to make the call (`exit`).
    fn nested_call(&mut self, b: &mut FunctionBuilder, pc: usize, op: &Op) {
        let (Op::CallG { g, .. } | Op::TailCallG { g, .. }) = *op else { unreachable!("nested call: {op:?}") };
        let f = self.global(b, g);
        let other = self.exit(b, pc, EXIT);
        self.enter_inlined(b, pc, f, self.site_at[&pc], other);
    }

    /// A call of a native in an inlined function (`o`), a tail call
    /// (`Inlined::tail`) going on with the instruction returning its
    /// value: as `known_native`, but the callee's frame is made before the
    /// call (`materialize`), and native code is left there for anything
    /// but the native's common case.
    fn inlined_call(&mut self, b: &mut FunctionBuilder, pc: usize, op: &Op, o: Inlined) {
        let next = pc + 1;
        let Op::CallG { base, n, g } = *op else { unreachable!("inlined call: {op:?}") };
        let Some(k) = self.native_globals.get(&g).copied() else {
            return self.jump_exit(b, pc, EXIT);
        };
        let f = self.global(b, g);
        let slow = self.exit(b, pc, EXIT);
        let returned = b.create_block();
        b.append_block_param(returned, I64);
        self.known_native(b, pc, base, n, f, k, slow, returned, false, |g, b, v| {
            if k.fixnum {
                let s = Self::shifted(b, v);
                g.set_fixnum(b, base, s);
            } else {
                g.set(b, base, v);
            }
            g.jump(b, next);
        });
        b.switch_to_block(returned);
        let status = b.block_params(returned)[0];
        let moved = if o.tail { RET_MOVED } else { RESUME };
        let calls =
            [self.exit(b, next, ERROR), self.exit(b, next, WAIT), self.exit(b, next, moved)].map(|blk| b.func.dfg.block_call(blk, &[]));
        let status = b.ins().ireduce(I32, status);
        let index = b.ins().iadd_imm_s(status, -2);
        let jt = b.create_jump_table(JumpTableData::new(calls[2], &calls[..2]));
        b.ins().br_table(index, jt);
    }

    fn jump_exit(&mut self, b: &mut FunctionBuilder, pc: usize, status: u32) {
        let e = self.exit(b, pc, status);
        b.ins().jump(e, &[]);
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
        // Through a global, the closure is not in the base register yet.
        if matches!(self.ops[pc], Op::CallG { .. }) {
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
        self.load_fuel(b);
        let status = b.ins().ushr_imm_s(res, 32);
        let returned = b.ins().icmp_imm_s(IntCC::Equal, status, RETURNED as i64);
        let after = b.create_block();
        b.ins().brif(returned, done, &[], after, &[]);
        // A tail call in the callee, or a hand-over to the interpreter.
        b.switch_to_block(after);
        let res = self.call(b, self.sigs.after, jit_after_call as *const (), &[self.vm, self.ctx, res, callee_bp, depth]).unwrap();
        self.load_fuel(b);
        let status = b.ins().ushr_imm_s(res, 32);
        let returned = b.ins().icmp_imm_s(IntCC::Equal, status, RETURNED as i64);
        b.ins().brif(returned, done, &[], bail, &[BlockArg::Value(res)]);
    }

    fn tail_call_op(&mut self, b: &mut FunctionBuilder, pc: usize, op: &Op) {
        if self.origin[pc].is_some() {
            return self.nested_call(b, pc, op);
        }
        let (base, n, global) = match *op {
            Op::TailCallG { base, n, g } => (base, n, Some(g)),
            Op::TailCall { base, n } => (base, n, None),
            _ => unreachable!(),
        };
        let f = match global {
            Some(g) => self.global(b, g),
            None => self.get(b, base),
        };
        let slow = b.create_block();
        let returned = b.create_block();
        b.append_block_param(returned, I64);
        let known = global.and_then(|g| self.closure_globals.get(&g).copied());
        let native = global.and_then(|g| self.native_globals.get(&g).copied());
        if let Some(k) = native {
            // Its value is returned from the callee slot.
            self.known_native(b, pc, base, n, f, k, slow, returned, true, |g, b, v| {
                b.ins().store(flags(), v, g.r, -8);
                let res = b.ins().iconst(I64, (RETURNED as i64) << 32);
                b.ins().return_(&[res]);
            });
        } else if let Some(&site) = self.site_at.get(&pc) {
            // The callee's instructions inline, in this frame.
            self.enter_inlined(b, pc, f, site, slow);
        } else if global.is_none() || known.is_some() {
            self.inline_tail_call(b, pc, base, n, f, known, slow);
        } else {
            b.ins().jump(slow, &[]);
        }
        self.tail_slow(b, pc, base, n, f, slow, returned);
    }

    /// A call of `f` with `n` arguments, expected to be the native `k`: if
    /// it is, run its inline case if it has one and go on with `done` and
    /// its value, else call it (`PlainFn`) and go on with
    /// `done` and its value (after re-reading the other registers live after
    /// the call, unless a `tail` call) if it returns, else go to `returned`
    /// with the result code of `jit_call_slow` that it returns.
    /// Otherwise go to `slow`.
    #[allow(clippy::too_many_arguments)]
    fn known_native(
        &mut self,
        b: &mut FunctionBuilder,
        pc: usize,
        base: Reg,
        n: u16,
        f: ir::Value,
        k: KnownNative,
        slow: Block,
        returned: Block,
        tail: bool,
        done: impl Fn(&mut Self, &mut FunctionBuilder, ir::Value),
    ) {
        // A wrong number of arguments is the slow path's error to raise.
        if !k.takes(n) {
            b.ins().jump(slow, &[]);
            return;
        }
        let same = b.ins().icmp_imm_s(IntCC::Equal, f, k.value as i64);
        Self::guard(b, same, slow);
        if let Some(i) = k.inline(n) {
            let call = b.create_block();
            let args: Vec<_> = (1..=n).map(|a| self.get(b, base + a)).collect();
            let v = self.inline_native(b, i, &args, call);
            done(self, b, v);
            b.switch_to_block(call);
        }
        // Known fixnums need not be in memory while the native runs (the GC
        // does not move them, and the native does not read them): they are
        // stored only if native code is left after the call.
        let deferred = match self.origin[pc] {
            // In an inlined function: its frame, made.
            Some(o) => {
                self.materialize(b, pc, o.site);
                Regs::empty(self.vars.len())
            }
            None => {
                let deferred = self.deferred_spills(pc);
                self.spill(b, pc, &self.flow.dirty_in[pc].and(&self.flow.live_in[pc]).minus(&deferred));
                deferred
            }
        };
        let args = b.ins().iadd_imm_s(self.bp, base as i64 + 1);
        let n = b.ins().iconst(I64, n as i64);
        let f = b.ins().iconst(I64, k.f as usize as i64);
        let inst = b.ins().call_indirect(self.sigs.native, f, &[self.vm, args, n]);
        let (v, error) = (b.inst_results(inst)[0], b.inst_results(inst)[1]);
        // A stop the native made (out of memory, say) ends the slice.
        self.load_fuel(b);
        // The common case: one test (no error, and the register stack and
        // globals did not move), and the value in a register. (The GC may
        // have moved what the other registers point to: re-read them.)
        let now = b.ins().load(I64, flags(), self.vm, offset_of!(Vm, moves) as i32);
        let then = b.ins().load(I64, flags(), self.ctx, offset_of!(JitCtx, moves) as i32);
        let moved = b.ins().bxor(now, then);
        let unusual = b.ins().bor(moved, error);
        let next = b.create_block();
        let other = b.create_block();
        b.ins().brif(unusual, other, &[], next, &[]);
        b.switch_to_block(other);
        let base_v = b.ins().iconst(I64, base as i64);
        let native = b.ins().iconst(I64, k.value as i64);
        let helper = b.ins().iconst(I64, jit_native_unusual as *const () as i64);
        let inst = b.ins().call_indirect(self.sigs.unusual, helper, &[self.vm, v, error, self.bp, base_v, native]);
        let (status, frame) = (b.inst_results(inst)[0], b.inst_results(inst)[1]);
        // Where the frame is now: the register stack may have moved.
        self.spill_to(b, pc, &deferred, frame);
        b.ins().jump(returned, &[BlockArg::Value(status)]);
        b.switch_to_block(next);
        if !tail {
            self.reload_except(b, pc, Some(base));
        }
        done(self, b, v);
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
            // What the start assumes of the arguments (`Job::observed`),
            // checked as on edges into other entry points (`Gen::target`).
            let live = &self.flow.live_in[0];
            let (fx, fl) = (self.fix[0].and(live), self.flo[0].and(live));
            if fx.iter().chain(fl.iter()).next().is_some() {
                let bad = self.check_kinds(b, &fx, &fl);
                let fail = self.exit(b, 0, GUARD);
                b.ins().brif(bad, fail, &[], start, &[]);
            } else {
                b.ins().jump(start, &[]);
            }
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

    /// The common case of native `i` with `args`: its value, in a new block;
    /// anything else goes to `call`.
    fn inline_native(&self, b: &mut FunctionBuilder, i: Inline, args: &[ir::Value], call: Block) -> ir::Value {
        use crate::heap::{ASCII, INDIRECT};
        let x = args[0];
        // The object, of `kind` with `flags` among `checked`, and its length.
        let object = |b: &mut FunctionBuilder, kind: Kind, flags_: u64, checked: u64| {
            let (p, h) = self.check_bits(b, x, 0xFF | checked as i64, kind as i64 | flags_ as i64, call);
            (p, Self::header_len(b, h))
        };
        // The byte at fixnum index `args[1]` of the bytes at `p`.
        let byte = |b: &mut FunctionBuilder, p: ir::Value, len: ir::Value| {
            let k = args[1];
            let int = Self::is_int(b, k);
            let idx = Self::untag(b, k);
            let in_range = b.ins().icmp(IntCC::UnsignedLessThan, idx, len);
            let ok = b.ins().band(int, in_range);
            Self::guard(b, ok, call);
            let addr = b.ins().iadd(p, idx);
            b.ins().uload8(I64, flags(), addr, 8)
        };
        let tag = |b: &mut FunctionBuilder, v: ir::Value, tag: i64| b.ins().bor_imm_s(v, tag << 48);
        let is_char = |b: &mut FunctionBuilder, v: ir::Value| {
            let t = b.ins().ushr_imm_s(v, 48);
            b.ins().icmp_imm_s(IntCC::Equal, t, TAG_CHAR)
        };
        match i {
            Inline::StringLength => {
                let (_, len) = object(b, Kind::String, ASCII, ASCII | INDIRECT);
                tag(b, len, TAG_INT)
            }
            Inline::VectorLength => {
                let (_, len) = object(b, Kind::Vector, 0, 0);
                tag(b, len, TAG_INT)
            }
            Inline::StringRef => {
                let (p, len) = object(b, Kind::String, ASCII, ASCII | INDIRECT);
                let c = byte(b, p, len);
                tag(b, c, TAG_CHAR)
            }
            Inline::BytevectorU8Ref => {
                let (p, len) = object(b, Kind::Bytevector, 0, INDIRECT);
                let c = byte(b, p, len);
                tag(b, c, TAG_INT)
            }
            Inline::CharToInteger => {
                let ok = is_char(b, x);
                Self::guard(b, ok, call);
                let c = b.ins().band_imm_s(x, PAYLOAD);
                tag(b, c, TAG_INT)
            }
            Inline::RecordRef => {
                // Its type is its first field, the others follow.
                let (p, h) = self.check_bits(b, x, 0xFF, Kind::Record as i64, call);
                let t = b.ins().load(I64, flags(), p, 8);
                let same = b.ins().icmp(IntCC::Equal, t, args[1]);
                Self::guard(b, same, call);
                let k = args[2];
                let int = Self::is_int(b, k);
                let idx = Self::untag(b, k);
                // A record has its type: one field at least, else (a header
                // that is no record's) none, not 2^64 - 1.
                let len = Self::header_len(b, h);
                let one = b.ins().iconst(I64, 1);
                let len = b.ins().umax(len, one);
                let fields = b.ins().iadd_imm_s(len, -1);
                let in_range = b.ins().icmp(IntCC::UnsignedLessThan, idx, fields);
                let ok = b.ins().band(int, in_range);
                Self::guard(b, ok, call);
                let off = b.ins().ishl_imm_s(idx, 3);
                let addr = b.ins().iadd(p, off);
                b.ins().load(I64, flags(), addr, 16)
            }
            Inline::CharEq => {
                let (a, c) = (is_char(b, x), is_char(b, args[1]));
                let ok = b.ins().band(a, c);
                Self::guard(b, ok, call);
                let eq = b.ins().icmp(IntCC::Equal, x, args[1]);
                Self::bool(b, eq)
            }
        }
    }

    /// Tail calls of Rust natives: see `jit_tail_slow`.
    /// The result code goes to `done` (a block taking it), as for
    /// `known_native`.
    #[allow(clippy::too_many_arguments)]
    fn tail_slow(&mut self, b: &mut FunctionBuilder, pc: usize, base: Reg, n: u16, f: ir::Value, slow: Block, done: Block) {
        let next = pc + 1;
        b.switch_to_block(slow);
        self.spill_at(b, pc);
        let (base_v, n_v) = (b.ins().iconst(I64, base as i64), b.ins().iconst(I64, n as i64));
        let k = self.call(b, self.sigs.tail_slow, jit_tail_slow as *const (), &[self.vm, self.bp, base_v, n_v, f]).unwrap();
        b.ins().jump(done, &[BlockArg::Value(k)]);
        b.switch_to_block(done);
        let k = b.block_params(done)[0];
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
                let (consts, bits) = match self.origin[pc] {
                    Some(o) => (self.sites[o.site].consts, &self.sites[o.site].const_bits),
                    None => (self.consts, &self.const_bits),
                };
                let c = Value::from_bits(bits[k as usize]);
                // Heap constants move with the GC: load them from the constant pool.
                if c.is_int() {
                    let s = b.ins().iconst(I64, c.as_int() << 16);
                    self.set_fixnum(b, dst, s);
                } else if c.is_float() {
                    let f = b.ins().f64const(c.as_float());
                    self.set_float(b, dst, f);
                } else {
                    let v = if c.is_ptr() {
                        let base = b.ins().iconst(I64, consts);
                        b.ins().load(I64, flags(), base, (k * 8) as i32)
                    } else {
                        Self::imm(b, c)
                    };
                    self.set(b, dst, v);
                }
                self.jump(b, next);
            }
            Op::LoadI { dst, i } => {
                let s = b.ins().iconst(I64, (i as i64) << 16);
                self.set_fixnum(b, dst, s);
                self.jump(b, next);
            }
            Op::Mov { dst, src } => {
                match (self.num(b, pc, src), self.fval(b, pc, src)) {
                    ((_, Some(s)), _) => self.set_fixnum(b, dst, s),
                    (_, Some(f)) => self.set_float(b, dst, f),
                    ((v, None), None) => self.set(b, dst, v),
                }
                if self.vecs[pc].contains(src) {
                    let lens = self.use_lens(b, self.lens[src as usize]);
                    self.set_lens(b, dst, lens);
                }
                self.jump(b, next);
            }
            Op::GetG { dst, g } => {
                let v = self.global(b, g);
                let bound = b.ins().icmp_imm_s(IntCC::NotEqual, v, Value::UNDEFINED.bits() as i64);
                let slow = self.step(b, pc);
                Self::guard(b, bound, slow);
                self.set(b, dst, v);
                if let Some(lens) = self.cached.iter().find(|c| c.g == g).and_then(|c| c.lens) {
                    let lens = self.use_lens(b, lens);
                    self.set_lens(b, dst, lens);
                }
                self.jump(b, next);
            }
            Op::SetG { g, src } => {
                let v = self.get(b, src);
                b.ins().store(flags(), v, self.globals, (g * 8) as i32);
                self.jump(b, next);
            }
            Op::GetC { dst, i } | Op::GetCB { dst, i } => {
                let c = self.closure(b, pc);
                let p = self.ptr(b, c);
                let mut v = b.ins().load(I64, flags(), p, (8 * (2 + i as u32)) as i32);
                if matches!(op, Op::GetCB { .. }) {
                    let bx = self.ptr(b, v);
                    v = b.ins().load(I64, flags(), bx, 8);
                }
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::SetCB { i, src } => {
                let c = self.closure(b, pc);
                let p = self.ptr(b, c);
                let bx = b.ins().load(I64, flags(), p, (8 * (2 + i as u32)) as i32);
                let bx = self.ptr(b, bx);
                let v = self.get(b, src);
                b.ins().store(flags(), v, bx, 8);
                self.barrier(b, bx, v);
                self.jump(b, next);
            }
            Op::Unbox { dst, r } => {
                let x = self.get(b, r);
                let p = self.ptr(b, x);
                let v = b.ins().load(I64, flags(), p, 8);
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::SetBox { r, src } => {
                let x = self.get(b, r);
                let p = self.ptr(b, x);
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
                self.call(b, self.sigs.step, jit_step as *const (), &[self.vm, self.bp, op]);
                self.reload_after(b, pc);
                self.jump(b, next);
            }
            Op::PopHandler => {
                self.call(b, self.sigs.pop, jit_pop_handler as *const (), &[self.vm]);
                self.reload_globals(b, next);
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
                let (xn, yn) = (self.num(b, pc, a), self.num(b, pc, y));
                let nonzero = b.ins().icmp_imm_s(IntCC::NotEqual, yn.0, Value::int_unchecked(0).bits() as i64);
                let ok = match Self::both_int(b, xn, yn) {
                    Some(both) => b.ins().band(both, nonzero),
                    None => nonzero,
                };
                Self::guard(b, ok, slow);
                let untag = |b: &mut FunctionBuilder, x: Num| match x.1 {
                    Some(s) => b.ins().sshr_imm_s(s, 16),
                    None => Self::untag(b, x.0),
                };
                let (n, d) = (untag(b, xn), untag(b, yn));
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
                let v = Self::shifted(b, v);
                self.set_fixnum(b, dst, v);
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
                self.check_kind(b, x, Kind::Pair, no);
                self.jump(b, next);
            }
            Op::JNLt { a, b: y, t } | Op::JNLe { a, b: y, t } | Op::JNNumEq { a, b: y, t } => {
                let kf = self.floats_known(b, pc, a, y);
                let (x, y) = (self.num(b, pc, a), self.num(b, pc, y));
                self.compare(b, pc, x, y, kf, Self::cmp_ops(op), |g, b, c| g.branch(b, c, next, t as usize));
            }
            Op::JNEqI { a, i, t } => {
                // Equal to a fixnum means the same bits; another fixnum is
                // unequal; only floats and non-numbers need comparing.
                let (x, known) = self.num(b, pc, a);
                let k = Self::imm(b, Value::int_unchecked(i as i64));
                let same = b.ins().icmp(IntCC::Equal, x, k);
                if known.is_some() {
                    self.branch(b, same, next, t as usize);
                    return;
                }
                let other = b.create_block();
                let yes = self.target(b, next);
                b.ins().brif(same, yes, &[], other, &[]);
                b.switch_to_block(other);
                let int = Self::is_int(b, x);
                let compare = b.create_block();
                let no = self.target(b, t as usize);
                b.ins().brif(int, no, &[], compare, &[]);
                b.switch_to_block(compare);
                let k = Self::fixnum(b, i as i64);
                let kf = (self.fval(b, pc, a), None);
                self.compare(b, pc, (x, None), k, kf, Self::cmp_ops(op), |g, b, c| g.branch(b, c, next, t as usize));
            }
            Op::JNLtI { a, i, t } | Op::JNGtI { a, i, t } => {
                let x = self.num(b, pc, a);
                let k = Self::fixnum(b, i as i64);
                let fa = self.fval(b, pc, a);
                let ((x, y), kf) = if matches!(op, Op::JNGtI { .. }) { ((k, x), (None, fa)) } else { ((x, k), (fa, None)) };
                self.compare(b, pc, x, y, kf, Self::cmp_ops(op), |g, b, c| g.branch(b, c, next, t as usize));
            }
            Op::Lt { dst, a, b: y } | Op::Le { dst, a, b: y } | Op::NumEq { dst, a, b: y } => {
                let kf = self.floats_known(b, pc, a, y);
                let (x, y) = (self.num(b, pc, a), self.num(b, pc, y));
                self.compare(b, pc, x, y, kf, Self::cmp_ops(op), |g, b, c| {
                    let v = Self::bool(b, c);
                    g.set(b, dst, v);
                    g.jump(b, next);
                });
            }
            Op::Add { dst, a, b: y } | Op::Sub { dst, a, b: y } | Op::Mul { dst, a, b: y } => {
                let kf = self.floats_known(b, pc, a, y);
                let (x, y) = (self.num(b, pc, a), self.num(b, pc, y));
                let kind = match op {
                    Op::Add { .. } => '+',
                    Op::Sub { .. } => '-',
                    _ => '*',
                };
                self.arith(b, pc, dst, x, y, kf, kind);
            }
            Op::AddI { dst, a, i } => {
                let kf = (self.fval(b, pc, a), None);
                let x = self.num(b, pc, a);
                let y = Self::fixnum(b, i as i64);
                self.arith(b, pc, dst, x, y, kf, '+');
            }
            Op::Car { dst, a } | Op::Cdr { dst, a } => {
                let slow = self.step(b, pc);
                let x = self.get(b, a);
                let p = self.check_kind(b, x, Kind::Pair, slow);
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
                self.check_kind(b, x, Kind::Pair, no);
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
                let set = matches!(op, Op::VSet { .. });
                // Its length where kept (`vecs`), else from its header,
                // kept from now on.
                let (p, len) = if self.vecs[pc].contains(v) {
                    let (l, w) = self.use_lens(b, self.lens[v as usize]);
                    (self.ptr(b, vec), if set { w } else { l })
                } else {
                    // To change it, literals go to `slow` too, at no extra
                    // cost (the same test, with the immutable flag).
                    let mask = if set { 0xFF | crate::heap::IMMUTABLE as i64 } else { 0xFF };
                    let (p, h) = self.check_bits(b, vec, mask, Kind::Vector as i64, slow);
                    let lens = Self::header_lens(b, h);
                    if !matches!(*op, Op::VRef { dst, .. } if dst == v) {
                        self.set_lens(b, v, lens);
                    }
                    (p, Self::header_len(b, h))
                };
                // A known fixnum index from its shifted form, not made
                // tagged and untagged again.
                let idx = match self.fix[pc].contains(i) {
                    true => {
                        let s = b.use_var(self.shifts[i as usize]);
                        b.ins().sshr_imm_s(s, 16)
                    }
                    false => Self::untag(b, k),
                };
                let in_range = b.ins().icmp(IntCC::UnsignedLessThan, idx, len);
                let ok = if self.fix[pc].contains(i) {
                    in_range
                } else {
                    let int = Self::is_int(b, k);
                    b.ins().band(int, in_range)
                };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_of_another_world_reaches_nothing_of_it() {
        // World A's pair, its bits given to world B as they are: B decodes
        // them in its own cage, compiled or not, and finds there what B
        // holds at that offset (or, in debug builds, refuses a value of
        // another cage, `alloc::cage::at`), never A's pair.
        const SECRET: i64 = 123_456_789;
        let mut a = Vm::new();
        let secret = {
            let _world = a.enter_account();
            let v = a.eval_source_raw(&format!("(define secret (list {SECRET})) secret")).unwrap();
            v.bits()
        };
        for jit in [Some(1), None] {
            let mut b = Vm::new();
            let _world = b.enter_account();
            b.set_jit(jit);
            let first = b.eval_source_raw("(define (first p) (if (pair? p) (car p) 'no)) first").unwrap();
            let first = b.root(first);
            b.eval_source_raw("(let loop ((i 0)) (when (< i 3) (first (list 7)) (loop (+ i 1))))").unwrap();
            let got = b.call_raw(first.get(), &[Value::from_bits(secret)]);
            assert!(!matches!(got, Ok(v) if v.is_int() && v.as_int() == SECRET), "{jit:?}: world B read world A's pair");
        }
        let _world = a.enter_account();
        assert_eq!(crate::builtins::repr(a.eval_source_raw("secret").unwrap()), format!("({SECRET})"));
    }

    #[test]
    fn a_closure_naming_no_code_ends_its_world() {
        // The table of codes is outside the heap: a closure's code id that
        // names an empty slot is refused, called from compiled code and
        // from the interpreter. Its world ends (its heap is corrupt); the
        // others, each in a cage of its own, go on.
        for jit in [Some(1), None] {
            let mut vm = Vm::new();
            let _world = vm.enter_account();
            vm.set_jit(jit);
            let call = vm.eval_source_raw("(define (call f) (f)) call").unwrap();
            let call = vm.root(call);
            vm.eval_source_raw("(let loop ((i 0)) (when (< i 3) (call (lambda () 1)) (loop (+ i 1))))").unwrap();
            let forged = vm.eval_source_raw("(lambda () 2)").unwrap();
            let empty = crate::heap::CODE_MASK as i64;
            unsafe { crate::heap::set_field(forged.as_ptr(), 0, Value::int_unchecked(empty)) };
            let exported = vm.export(forged);
            let e = vm.call_raw(call.get(), &[forged]).unwrap_err();
            assert!(e.msg.contains(&format!("names code {empty}, which is none")), "{jit:?}: {e}");
            assert!(vm.eval_source_raw("1").is_err(), "{jit:?}: the world is retired");
            // Found by an inspection, outside any execution, the world
            // goes all the same, at the host's boundary.
            let mut other = Vm::new();
            let forged = {
                let _world = other.enter_account();
                let v = other.eval_source_raw("(lambda () 2)").unwrap();
                unsafe { crate::heap::set_field(v.as_ptr(), 0, Value::int_unchecked(empty)) };
                other.export(v)
            };
            assert!(other.procedure_info(&forged).is_none(), "{jit:?}: the inspection finds nothing");
            assert!(other.poisoned().is_some_and(|why| why.contains("corrupt")), "{jit:?}: for tasks and snapshots too");
            assert!(other.eval_source("1").is_err(), "{jit:?}: the world found corrupt is retired");
            drop(exported);
            drop(_world);
            let mut other = Vm::new();
            assert_eq!(other.eval_source("(+ 1 2)").unwrap().as_int(), Some(3), "{jit:?}: another world goes on");
        }
    }

    #[test]
    fn compiled_code_keeps_the_codes_it_calls() {
        let mut vm = Vm::new();
        let _world = vm.enter_account();
        vm.set_jit(Some(1));
        vm.eval_source_raw("(define (callee) 1) (define (caller) (+ 1 (callee)))").unwrap();
        vm.eval_source_raw("(let loop ((i 0)) (when (< i 3) (caller) (loop (+ i 1))))").unwrap();
        let id = |v: Value| unsafe { crate::heap::field(v.as_ptr(), 0) }.as_int() as u32;
        let callee = vm.eval_source_raw("callee").unwrap();
        let callee = vm.root(callee);
        let caller = vm.eval_source_raw("caller").unwrap();
        let (callee_id, caller_id) = (id(callee.get()), id(caller));
        assert!(vm.code(caller_id).jit.deps.borrow().contains(&callee_id), "compiled with its callee known");
        // The closure compiled code expects comes to name another code (a
        // write that corrupts it), and the global to hold something else:
        // the code compiled code calls by address stays all the same.
        let other = vm.eval_source_raw("(lambda () 2)").unwrap();
        unsafe { crate::heap::set_field(callee.get().as_ptr(), 0, crate::heap::field(other.as_ptr(), 0)) };
        vm.eval_source_raw("(set! callee 0)").unwrap();
        vm.full_collect_raw();
        vm.full_collect_raw();
        assert!(vm.code_live(callee_id), "the code compiled code calls is kept");
    }

    #[test]
    fn compiled_code_takes_addresses_from_the_cage() {
        let mut vm = Vm::new();
        let _world = vm.enter_account();
        vm.set_jit(Some(1));
        let first = vm.eval_source_raw("(define (first p) (if (pair? p) (car p) 'no)) first").unwrap();
        let first = vm.root(first);
        // Called from Lisp, which counts calls towards compiling.
        vm.eval_source_raw("(let loop ((i 0)) (when (< i 3) (first (list 7)) (loop (+ i 1))))").unwrap();
        let pair = vm.alloc_pair_raw(Value::fixnum(7).unwrap(), Value::NIL);
        let pair = vm.root(pair);
        assert_eq!(vm.call_raw(first.get(), &[pair.get()]).unwrap().as_int(), 7);
        let id = unsafe { crate::heap::field(first.get().as_ptr(), 0) }.as_int();
        assert!(vm.code(id as u32).jit.entry.get().is_some(), "compiled");
        // The pair's offset with other bits above it: no address of the
        // cage, but the cage's base gives its offset the pair's address, as
        // the interpreter's decoding does.
        let forged = Value::from_bits(pair.get().bits() ^ (1 << 40));
        assert_eq!(vm.call_raw(first.get(), &[forged]).unwrap().as_int(), 7);
    }

    #[test]
    fn replay_stops_at_nested_entry() {
        // Deliberately overlap original arguments with implicit fresh-slot
        // clearing: no explicit definition at the call reveals the write.
        for root_tail in [false, true] {
            for nested_tail in [false, true] {
                for n in [0, 1] {
                    let outer = Site {
                        call: 0,
                        base: 3,
                        args: 3,
                        parent: None,
                        ret_pc: 1,
                        tail: root_tail,
                        off: if root_tail { 0 } else { 4 },
                        ret: Return::Ret,
                        start: 1,
                        code: 0,
                        frame: 8,
                        closure: 0,
                        consts: 0,
                        const_bits: vec![],
                    };
                    let nested = Site {
                        call: 2,
                        base: outer.off,
                        args: n,
                        parent: Some(0),
                        ret_pc: 3,
                        tail: nested_tail,
                        off: outer.off + u16::from(!nested_tail),
                        start: 3,
                        const_bits: vec![],
                        ..outer
                    };
                    assert!(nested.fresh().any(|r| (4..=6).contains(&r)));
                    let call =
                        if nested_tail { Op::TailCallG { base: nested.base, n, g: 0 } } else { Op::CallG { base: nested.base, n, g: 0 } };
                    let ops = [
                        Op::CallG { base: 3, n: 3, g: 0 },
                        Op::LoadI { dst: 8, i: 0 },
                        call,
                        Op::LoadI { dst: 8, i: 1 },
                        Op::GetG { dst: 8, g: 1 },
                    ];
                    let from = |site| Some(Inlined { site, pc: 0, tail: false });
                    let origin = [None, from(0), from(0), from(1), from(0)];
                    // The nested callee returns to its caller; replay must
                    // stay disabled there even though the sibling completed.
                    let succs = vec![vec![1], vec![2], vec![3], vec![4], vec![]];
                    assert_eq!(redo_points(&ops, &origin, &[outer, nested], &succs), [false, true, true, false, false]);
                }
            }
        }
    }

    #[test]
    fn tail_entries_get_fresh_frames_after_collection() {
        // Rust stand-ins test the helper's pointer-refresh contract under
        // Miri, not the behavior of generated machine code itself.
        unsafe extern "C" fn first(vm: *mut Vm, ctx: *mut JitCtx, frame: *mut Value, bp: u64, pc: u32, depth: u32) -> u64 {
            unsafe {
                assert_eq!((bp, pc, depth), (1, 0, 0));
                assert_eq!(field((*frame).as_ptr(), 0).as_int(), 71);
                let vm = &mut *vm;
                vm.collect_raw();
                vm.full_collect_raw();
                (*ctx).tail = second as *const () as usize;
                (TAILCALL as u64) << 32
            }
        }
        unsafe extern "C" fn second(_: *mut Vm, _: *mut JitCtx, frame: *mut Value, bp: u64, pc: u32, depth: u32) -> u64 {
            unsafe {
                assert_eq!((bp, pc, depth), (1, 0, 0));
                assert_eq!(field((*frame).as_ptr(), 0).as_int(), 71);
                *frame.sub(1) = Value::fixnum(72).unwrap();
                (RETURNED as u64) << 32
            }
        }
        let mut vm = Vm::bare();
        let _world = vm.enter_account();
        let pair = vm.alloc_pair_raw(Value::fixnum(71).unwrap(), Value::NIL);
        vm.regs[1] = pair;
        vm.stack_top = 2;
        let mut ctx = JitCtx {
            globals: std::ptr::null_mut(),
            code_table: std::ptr::null(),
            regs_end: std::ptr::null_mut(),
            fuel: std::ptr::null_mut(),
            stack_top: std::ptr::null_mut(),
            heap_top: std::ptr::null_mut(),
            heap_end: std::ptr::null(),
            code: std::ptr::null(),
            bp: 1,
            tail: first as *const () as usize,
            res: 0,
            moves: vm.moves,
            cage: vm.heap.cage_base(),
        };
        let result = unsafe { jit_after_call(&mut vm, &mut ctx, (TAILCALL as u64) << 32, 1, 0) };
        assert_eq!(result, (RETURNED as u64) << 32);
        assert_eq!(vm.regs[0].as_int(), 72);
    }

    /// Cranelift's two `I64` results of a call in the host's C convention
    /// are where Rust returns `NativeRet` (`PlainFn`).
    #[test]
    fn two_results() {
        extern "C" fn two(a: u64) -> NativeRet {
            NativeRet { value: a + 7, error: a + 9 }
        }
        let isa = cranelift_native::builder().unwrap().finish(settings::Flags::new(settings::builder())).unwrap();
        let mut module = JITModule::new(JITBuilder::with_isa(isa, cranelift_module::default_libcall_names()));
        let mut ctx = module.make_context();
        let call_conv = ctx.func.signature.call_conv;
        ctx.func.signature.params.push(AbiParam::new(I64));
        ctx.func.signature.returns.push(AbiParam::new(I64));
        let id = module.declare_anonymous_function(&ctx.func.signature).unwrap();
        let mut fctx = FunctionBuilderContext::new();
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fctx);
        let mut sig = Signature::new(call_conv);
        sig.params.push(AbiParam::new(I64));
        sig.returns.extend([AbiParam::new(I64), AbiParam::new(I64)]);
        let sig = b.import_signature(sig);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let a = b.block_params(entry)[0];
        let f = b.ins().iconst(I64, two as *const () as i64);
        let inst = b.ins().call_indirect(sig, f, &[a]);
        let (value, status) = (b.inst_results(inst)[0], b.inst_results(inst)[1]);
        let hi = b.ins().imul_imm_s(value, 1000);
        let r = b.ins().iadd(hi, status);
        b.ins().return_(&[r]);
        b.seal_all_blocks();
        b.finalize(module.isa().frontend_config());
        module.define_function(id, &mut ctx).unwrap();
        module.finalize_definitions().unwrap();
        let f: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(module.get_finalized_function(id)) };
        assert_eq!(f(1), 8 * 1000 + 10);
        unsafe { module.free_memory() };
    }
}
