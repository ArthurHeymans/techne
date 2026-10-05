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
    heap::{Kind, header},
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

/// Per-function JIT state.
#[derive(Default)]
pub struct JitSlot {
    /// Loop back-edges taken so far (until compiled).
    pub hot: Cell<u32>,
    pub entry: Cell<Option<JitFn>>,
    /// `entry` if the function can be entered at pc 0 (by native calls).
    pub call_entry: Cell<Option<JitFn>>,
    pub failed: Cell<bool>,
    /// The original instructions (loop heads are overwritten by `EnterJit`).
    pub ops: OnceCell<Box<[Op]>>,
}

impl fmt::Debug for JitSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JitSlot {{ compiled: {} }}", self.entry.get().is_some())
    }
}

pub struct Jit {
    module: JITModule,
    ctx: Context,
    fctx: FunctionBuilderContext,
    pub threshold: u32,
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

fn regs_of(op: &Op) -> Vec<Reg> {
    use Op::*;
    match *op {
        LoadK { dst, .. } | LoadI { dst, .. } | GetG { dst, .. } | GetC { dst, .. } | GetCB { dst, .. } => vec![dst],
        Mov { dst, src } | Unbox { dst, r: src } => vec![dst, src],
        SetG { src, .. } | SetCB { src, .. } => vec![src],
        MkBox { r } => vec![r],
        SetBox { r, src } => vec![r, src],
        Jf { c, .. } | Jt { c, .. } => vec![c],
        JNLt { a, b, .. } | JNLe { a, b, .. } | JNNumEq { a, b, .. } | JNEq { a, b, .. } => vec![a, b],
        JNNull { a, .. } | JNLtI { a, .. } | JNGtI { a, .. } | JNEqI { a, .. } | JNPair { a, .. } => vec![a],
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
        | EqP { dst, a, b } => vec![dst, a, b],
        AddI { dst, a, .. } | Car { dst, a } | Cdr { dst, a } | NullP { dst, a } | PairP { dst, a } | Not { dst, a } => {
            vec![dst, a]
        }
        VRef { dst, v, i } => vec![dst, v, i],
        VSet { v, i, x } => vec![v, i, x],
        Call { base, n } | CallG { base, n, .. } | TailCall { base, n } | TailCallG { base, n, .. } => (base..=base + n).collect(),
        Closure { dst, .. } => vec![dst],
        Ret { r } => vec![r],
        _ => vec![],
    }
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

/// Results of `jit_native` that are not values: `SENTINEL` + 0 (error), 1
/// (the task must wait), 2 (the register stack or globals moved; the result
/// is stored in the callee slot). Special-constant bits no value uses.
const SENTINEL: u64 = (0xFFFD << 48) | 0x100;

/// Call Rust native `index` with arguments at `regs[args..args + n]`.
unsafe extern "C" fn jit_native(vm: *mut Vm, index: u64, args: u64, n: u64) -> u64 {
    unsafe {
        let vm = &mut *vm;
        let (regs, globals) = (vm.regs.as_ptr(), vm.globals.as_ptr());
        match vm.call_native(index as usize, args as usize, n as usize) {
            Ok(v) if vm.regs.as_ptr() == regs && vm.globals.as_ptr() == globals => v.bits(),
            Ok(v) => {
                vm.regs[args as usize - 1] = v;
                SENTINEL + 2
            }
            Err(e) => {
                let wait = e.wait.is_some();
                vm.jit_error = Some(e);
                SENTINEL + wait as u64
            }
        }
    }
}

unsafe extern "C" fn jit_unwind_push(vm: *mut Vm, code: *const Code, pc: u64, bp: u64) {
    unsafe { (*vm).jit_push_frame(code, pc as u32, bp as u32) }
}

unsafe extern "C" fn jit_barrier(vm: *mut Vm, obj: *mut u64, v: Value) {
    unsafe { (*vm).write_barrier(obj, v) }
}

const TAG_INT: i64 = 0xFFF9;
const TAG_PTR: i64 = 0xFFFA;
const TAG_NATIVE: i64 = 0xFFFE;
const PAYLOAD: i64 = (1 << 48) - 1;
const CANONICAL_NAN: i64 = 0x7FF8_0000_0000_0000;

impl Jit {
    /// A JIT for the host, or `None` if Cranelift does not support it.
    pub fn new(threshold: u32) -> Option<Jit> {
        let mut flags = settings::builder();
        flags.set("opt_level", "speed").ok()?;
        flags.set("use_colocated_libcalls", "false").ok()?;
        flags.set("is_pic", "false").ok()?;
        flags.set("enable_verifier", "false").ok()?;
        let isa = cranelift_native::builder().ok()?.finish(settings::Flags::new(flags)).ok()?;
        let module = JITModule::new(JITBuilder::with_isa(isa, cranelift_module::default_libcall_names()));
        Some(Jit { ctx: module.make_context(), module, fctx: FunctionBuilderContext::new(), threshold })
    }

    /// Compile `code` with entry points at `heads`; `orig` are its original
    /// instructions (their addresses are passed to the slow path).
    /// `apply` is the `apply` native, which needs the interpreter.
    pub fn compile(&mut self, code: &Code, orig: &[Op], heads: &[usize], apply: Value) -> Option<JitFn> {
        let code_ptr = code as *const Code as i64;
        let n = code.frame_size as usize;
        if orig.iter().filter(|op| compiled(op)).flat_map(regs_of).any(|r| r as usize >= n) {
            return None;
        }
        self.module.clear_context(&mut self.ctx);
        let sig = &mut self.ctx.func.signature;
        sig.params.extend([AbiParam::new(I64), AbiParam::new(I64), AbiParam::new(I64), AbiParam::new(I64), AbiParam::new(I32), AbiParam::new(I32)]);
        sig.returns.push(AbiParam::new(I64));
        let jit_sig = sig.clone();
        let call_conv = sig.call_conv;
        let frontend = self.module.isa().frontend_config();
        let mut b = FunctionBuilder::new(&mut self.ctx.func, &mut self.fctx);

        let mut step_sig = Signature::new(call_conv);
        step_sig.params.extend([AbiParam::new(I64); 3]);
        step_sig.returns.push(AbiParam::new(I32));
        let step_sig = b.import_signature(step_sig);
        let mut barrier_sig = Signature::new(call_conv);
        barrier_sig.params.extend([AbiParam::new(I64); 3]);
        let barrier_sig = b.import_signature(barrier_sig);
        let mut unwind_sig = Signature::new(call_conv);
        unwind_sig.params.extend([AbiParam::new(I64); 4]);
        let unwind_sig = b.import_signature(unwind_sig);
        let jit_sig = b.import_signature(jit_sig);
        let mut native_sig = Signature::new(call_conv);
        native_sig.params.extend([AbiParam::new(I64); 4]);
        native_sig.returns.push(AbiParam::new(I64));
        let native_sig = b.import_signature(native_sig);

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
        let mut g = Gen {
            vm: p[0],
            ctx: p[1],
            r: p[2],
            bp: p[3],
            depth,
            globals,
            fuel,
            regs_end,
            code_ptr,
            entry0: heads.contains(&0),
            vars,
            blocks: orig.iter().map(|op| compiled(op).then(|| b.create_block())).collect(),
            exits: HashMap::new(),
            step_sig,
            barrier_sig,
            unwind_sig,
            jit_sig,
            native_sig,
            apply: apply.bits(),
            consts: code.consts.as_ptr() as i64,
        };
        g.reload(&mut b);
        let trap = b.create_block();
        let max = heads.iter().max().copied().unwrap_or(0);
        let table: Vec<_> = (0..=max)
            .map(|pc| {
                let target = if heads.contains(&pc) { g.blocks[pc].unwrap() } else { trap };
                b.func.dfg.block_call(target, &[])
            })
            .collect();
        let default = b.func.dfg.block_call(trap, &[]);
        let jt = b.create_jump_table(JumpTableData::new(default, &table));
        b.ins().br_table(p[4], jt);
        b.switch_to_block(trap);
        b.ins().trap(TrapCode::unwrap_user(1));

        for (pc, op) in orig.iter().enumerate() {
            if let Some(block) = g.blocks[pc] {
                b.switch_to_block(block);
                g.op(&mut b, pc, op, &orig[pc] as *const Op as i64);
            }
        }
        let exits: Vec<_> = g.exits.iter().map(|(k, v)| (*k, *v)).collect();
        for ((pc, status), block) in exits {
            b.switch_to_block(block);
            if matches!(status, EXIT | TICK) {
                g.spill(&mut b);
            }
            let code = g.code_const(&mut b);
            b.ins().store(flags(), code, g.ctx, offset_of!(JitCtx, code) as i32);
            b.ins().store(flags(), g.bp, g.ctx, offset_of!(JitCtx, bp) as i32);
            let v = b.ins().iconst(I64, ((status as i64) << 32) | pc as i64);
            b.ins().return_(&[v]);
        }
        b.seal_all_blocks();
        b.finalize(frontend);

        let id = self.module.declare_anonymous_function(&self.ctx.func.signature).ok()?;
        self.module.define_function(id, &mut self.ctx).ok()?;
        self.module.clear_context(&mut self.ctx);
        self.module.finalize_definitions().ok()?;
        let f = self.module.get_finalized_function(id);
        Some(unsafe { std::mem::transmute::<*const u8, JitFn>(f) })
    }
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
    /// pc 0 is an entry point (its block starts the function).
    entry0: bool,
    vars: Vec<Variable>,
    /// Native instruction blocks by pc.
    blocks: Vec<Option<Block>>,
    /// Exit blocks by (pc, status).
    exits: HashMap<(usize, u32), Block>,
    step_sig: ir::SigRef,
    barrier_sig: ir::SigRef,
    unwind_sig: ir::SigRef,
    jit_sig: ir::SigRef,
    native_sig: ir::SigRef,
    apply: u64,
    consts: i64,
}

fn flags() -> MemFlagsData {
    MemFlagsData::trusted()
}

impl Gen {
    fn spill(&self, b: &mut FunctionBuilder) {
        for (i, v) in self.vars.iter().enumerate() {
            let x = b.use_var(*v);
            b.ins().store(flags(), x, self.r, (i * 8) as i32);
        }
    }

    fn reload(&self, b: &mut FunctionBuilder) {
        for (i, v) in self.vars.iter().enumerate() {
            let x = b.ins().load(I64, flags(), self.r, (i * 8) as i32);
            b.def_var(*v, x);
        }
    }

    fn exit(&mut self, b: &mut FunctionBuilder, pc: usize, status: u32) -> Block {
        *self.exits.entry((pc, status)).or_insert_with(|| b.create_block())
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

    /// The slow path for instruction `pc`: run it in Rust, then continue at
    /// `pc + 1`, or at `t` if it is a branch that is taken.
    fn slow_block(&mut self, b: &mut FunctionBuilder, pc: usize, op_addr: i64, t: Option<usize>) -> Block {
        let current = b.current_block().unwrap();
        let slow = b.create_block();
        b.switch_to_block(slow);
        self.spill(b);
        let f = b.ins().iconst(I64, jit_step as *const () as i64);
        let op = b.ins().iconst(I64, op_addr);
        let call = b.ins().call_indirect(self.step_sig, f, &[self.vm, self.r, op]);
        let status = b.inst_results(call)[0];
        self.reload(b);
        let err = b.ins().icmp_imm_s(IntCC::Equal, status, 2);
        let fail = self.exit(b, pc + 1, ERROR);
        let ok = b.create_block();
        b.ins().brif(err, fail, &[], ok, &[]);
        b.switch_to_block(ok);
        match t {
            Some(t) => {
                let taken = b.ins().icmp_imm_s(IntCC::Equal, status, 1);
                self.branch(b, taken, t, pc + 1);
            }
            None => self.jump(b, pc + 1),
        }
        b.switch_to_block(current);
        slow
    }

    fn code_const(&self, b: &mut FunctionBuilder) -> ir::Value {
        b.ins().iconst(I64, self.code_ptr)
    }

    /// Count a call or back-edge; when the task's slice is used up, exit
    /// with `TICK` to resume at `resume`.
    fn tick(&mut self, b: &mut FunctionBuilder, resume: usize) {
        let f = b.ins().load(I32, flags(), self.fuel, 0);
        let f = b.ins().iadd_imm_s(f, -1);
        b.ins().store(flags(), f, self.fuel, 0);
        let out = b.ins().icmp_imm_s(IntCC::Equal, f, 0);
        let tick = self.exit(b, resume, TICK);
        let go = b.create_block();
        b.ins().brif(out, tick, &[], go, &[]);
        b.switch_to_block(go);
    }

    /// Store zeros (the float 0.0, a safe non-pointer) in `[from, to)`.
    fn zero_fill(b: &mut FunctionBuilder, from: ir::Value, to: ir::Value) {
        let head = b.create_block();
        let body = b.create_block();
        let done = b.create_block();
        b.append_block_param(head, I64);
        b.ins().jump(head, &[BlockArg::Value(from)]);
        b.switch_to_block(head);
        let p = b.block_params(head)[0];
        let more = b.ins().icmp(IntCC::UnsignedLessThan, p, to);
        b.ins().brif(more, body, &[], done, &[]);
        b.switch_to_block(body);
        let zero = b.ins().iconst(I64, 0);
        b.ins().store(flags(), zero, p, 0);
        let next = b.ins().iadd_imm_s(p, 8);
        b.ins().jump(head, &[BlockArg::Value(next)]);
        b.switch_to_block(done);
    }

    /// For a call of `f` with `n` arguments: the callee's code and native
    /// entry, continuing in the current block if `f` is a compiled closure
    /// taking exactly `n` arguments whose frame fits below `frame_end(size)`;
    /// otherwise branch to `no`.
    fn callee(&mut self, b: &mut FunctionBuilder, f: ir::Value, n: u16, frame: ir::Value, no: Block) -> (ir::Value, ir::Value, ir::Value) {
        let p = Self::check_kind(b, f, Kind::Closure, no);
        let w = b.ins().load(I64, flags(), p, 8);
        let code = Self::untag(b, w);
        let slot = offset_of!(Code, jit) + offset_of!(JitSlot, call_entry);
        let entry = b.ins().load(I64, flags(), code, slot as i32);
        let np = b.ins().uload16(I64, flags(), code, offset_of!(Code, nparams) as i32);
        let rest = b.ins().uload8(I64, flags(), code, offset_of!(Code, rest) as i32);
        let size = b.ins().uload16(I64, flags(), code, offset_of!(Code, frame_size) as i32);
        let has_entry = b.ins().icmp_imm_s(IntCC::NotEqual, entry, 0);
        let arity = b.ins().icmp_imm_s(IntCC::Equal, np, n as i64);
        let no_rest = b.ins().icmp_imm_s(IntCC::Equal, rest, 0);
        let bytes = b.ins().ishl_imm_s(size, 3);
        let end = b.ins().iadd(frame, bytes);
        let fits = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, end, self.regs_end);
        let ok = b.ins().band(has_entry, arity);
        let ok = b.ins().band(ok, no_rest);
        let ok = b.ins().band(ok, fits);
        let go = b.create_block();
        b.ins().brif(ok, go, &[], no, &[]);
        b.switch_to_block(go);
        (code, entry, size)
    }

    /// `*stack_top = max(*stack_top, top)`.
    fn raise_stack_top(&self, b: &mut FunctionBuilder, top: ir::Value) {
        let p = b.ins().load(I64, flags(), self.ctx, offset_of!(JitCtx, stack_top) as i32);
        let old = b.ins().load(I64, flags(), p, 0);
        let new = b.ins().umax(old, top);
        b.ins().store(flags(), new, p, 0);
    }

    /// Inline nursery allocation of `words` words with `header`; branches to
    /// `slow` when the nursery is full or allocation must go through the VM.
    fn alloc(&self, b: &mut FunctionBuilder, words: i64, header: u64, slow: Block) -> ir::Value {
        let top_ptr = b.ins().load(I64, flags(), self.ctx, offset_of!(JitCtx, heap_top) as i32);
        let inline = b.ins().icmp_imm_s(IntCC::NotEqual, top_ptr, 0);
        let check = b.create_block();
        b.ins().brif(inline, check, &[], slow, &[]);
        b.switch_to_block(check);
        let top = b.ins().load(I64, flags(), top_ptr, 0);
        let end = b.ins().load(I64, flags(), self.ctx, offset_of!(JitCtx, heap_end) as i32);
        let new = b.ins().iadd_imm_s(top, words * 8);
        let room = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, new, end);
        let go = b.create_block();
        b.ins().brif(room, go, &[], slow, &[]);
        b.switch_to_block(go);
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

    fn fits48(b: &mut FunctionBuilder, i: ir::Value) -> ir::Value {
        let s = Self::untag(b, i);
        b.ins().icmp(IntCC::Equal, s, i)
    }

    fn ptr(b: &mut FunctionBuilder, x: ir::Value) -> ir::Value {
        b.ins().band_imm_s(x, PAYLOAD)
    }

    fn to_f(b: &mut FunctionBuilder, x: ir::Value) -> ir::Value {
        b.ins().bitcast(F64, MemFlagsData::new(), x)
    }

    fn from_f(b: &mut FunctionBuilder, f: ir::Value) -> ir::Value {
        let bits = b.ins().bitcast(I64, MemFlagsData::new(), f);
        let nan = b.ins().fcmp(FloatCC::Unordered, f, f);
        let canonical = b.ins().iconst(I64, CANONICAL_NAN);
        b.ins().select(nan, canonical, bits)
    }

    fn bool(b: &mut FunctionBuilder, c: ir::Value) -> ir::Value {
        let (t, f) = (Self::imm(b, Value::TRUE), Self::imm(b, Value::FALSE));
        b.ins().select(c, t, f)
    }

    /// Branch on whether `x` is a heap object of `kind`; `yes` receives the
    /// object address.
    fn check_kind(b: &mut FunctionBuilder, x: ir::Value, kind: Kind, no: Block) -> ir::Value {
        let yes = b.create_block();
        let check = b.create_block();
        let ptr = Self::is_ptr(b, x);
        b.ins().brif(ptr, check, &[], no, &[]);
        b.switch_to_block(check);
        let p = Self::ptr(b, x);
        let h = b.ins().load(I64, flags(), p, 0);
        let k = b.ins().band_imm_s(h, 0xFF);
        let ok = b.ins().icmp_imm_s(IntCC::Equal, k, kind as i64);
        b.ins().brif(ok, yes, &[], no, &[]);
        b.switch_to_block(yes);
        p
    }

    fn barrier(&self, b: &mut FunctionBuilder, obj: ir::Value, v: ir::Value) {
        let call = b.create_block();
        let done = b.create_block();
        let ptr = Self::is_ptr(b, v);
        b.ins().brif(ptr, call, &[], done, &[]);
        b.switch_to_block(call);
        let f = b.ins().iconst(I64, jit_barrier as *const () as i64);
        b.ins().call_indirect(self.barrier_sig, f, &[self.vm, obj, v]);
        b.ins().jump(done, &[]);
        b.switch_to_block(done);
    }

    /// Fixnum/float arithmetic with a slow path.
    fn arith(&mut self, b: &mut FunctionBuilder, pc: usize, op_addr: i64, dst: Reg, x: ir::Value, y: ir::Value, kind: char) {
        let slow = self.slow_block(b, pc, op_addr, None);
        let int = b.create_block();
        let fcheck = b.create_block();
        let flt = b.create_block();
        let (xi, yi) = (Self::is_int(b, x), Self::is_int(b, y));
        let both = b.ins().band(xi, yi);
        b.ins().brif(both, int, &[], fcheck, &[]);

        b.switch_to_block(int);
        let (a, c) = (Self::untag(b, x), Self::untag(b, y));
        let (s, ok) = match kind {
            '+' => {
                let s = b.ins().iadd(a, c);
                (s, Self::fits48(b, s))
            }
            '-' => {
                let s = b.ins().isub(a, c);
                (s, Self::fits48(b, s))
            }
            _ => {
                let lo = b.ins().imul(a, c);
                let hi = b.ins().smulhi(a, c);
                let sign = b.ins().sshr_imm_s(lo, 63);
                let no_overflow = b.ins().icmp(IntCC::Equal, hi, sign);
                let fits = Self::fits48(b, lo);
                (lo, b.ins().band(no_overflow, fits))
            }
        };
        let done = b.create_block();
        b.ins().brif(ok, done, &[], slow, &[]);
        b.switch_to_block(done);
        let v = Self::tag_int(b, s);
        self.set(b, dst, v);
        self.jump(b, pc + 1);

        b.switch_to_block(fcheck);
        let (xf, yf) = (Self::is_float(b, x), Self::is_float(b, y));
        let both = b.ins().band(xf, yf);
        b.ins().brif(both, flt, &[], slow, &[]);
        b.switch_to_block(flt);
        let (fa, fb) = (Self::to_f(b, x), Self::to_f(b, y));
        let f = match kind {
            '+' => b.ins().fadd(fa, fb),
            '-' => b.ins().fsub(fa, fb),
            _ => b.ins().fmul(fa, fb),
        };
        let v = Self::from_f(b, f);
        self.set(b, dst, v);
        self.jump(b, pc + 1);
    }

    /// A numeric comparison: continues in the returned block with the
    /// result (an `I8`) as its parameter. Mixed or non-numeric operands take
    /// the slow path, which continues at `pc + 1` or, for branches, `t`.
    fn compare(
        &mut self,
        b: &mut FunctionBuilder,
        pc: usize,
        op_addr: i64,
        x: ir::Value,
        y: ir::Value,
        cc: (IntCC, FloatCC),
        t: Option<usize>,
    ) -> Block {
        let slow = self.slow_block(b, pc, op_addr, t);
        let result = b.create_block();
        b.append_block_param(result, I8);
        let int = b.create_block();
        let fcheck = b.create_block();
        let flt = b.create_block();
        let (xi, yi) = (Self::is_int(b, x), Self::is_int(b, y));
        let both = b.ins().band(xi, yi);
        b.ins().brif(both, int, &[], fcheck, &[]);
        b.switch_to_block(int);
        let (a, c) = (b.ins().ishl_imm_s(x, 16), b.ins().ishl_imm_s(y, 16));
        let r = b.ins().icmp(cc.0, a, c);
        b.ins().jump(result, &[BlockArg::Value(r)]);
        b.switch_to_block(fcheck);
        let (xf, yf) = (Self::is_float(b, x), Self::is_float(b, y));
        let both = b.ins().band(xf, yf);
        b.ins().brif(both, flt, &[], slow, &[]);
        b.switch_to_block(flt);
        let (fa, fb) = (Self::to_f(b, x), Self::to_f(b, y));
        let r = b.ins().fcmp(cc.1, fa, fb);
        b.ins().jump(result, &[BlockArg::Value(r)]);
        b.switch_to_block(result);
        result
    }

    fn cmp_ops(op: &Op) -> Option<(IntCC, FloatCC)> {
        Some(match op {
            Op::Lt { .. } | Op::JNLt { .. } | Op::JNLtI { .. } | Op::JNGtI { .. } => (IntCC::SignedLessThan, FloatCC::LessThan),
            Op::Le { .. } | Op::JNLe { .. } => (IntCC::SignedLessThanOrEqual, FloatCC::LessThanOrEqual),
            Op::NumEq { .. } | Op::JNNumEq { .. } | Op::JNEqI { .. } => (IntCC::Equal, FloatCC::Equal),
            _ => return None,
        })
    }

    fn op(&mut self, b: &mut FunctionBuilder, pc: usize, op: &Op, op_addr: i64) {
        let next = pc + 1;
        match *op {
            Op::LoadK { dst, k } => {
                let c = unsafe { *(self.consts as *const Value).add(k as usize) };
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
                let slow = self.slow_block(b, pc, op_addr, None);
                let v = b.ins().load(I64, flags(), self.globals, (g * 8) as i32);
                let unbound = b.ins().icmp_imm_s(IntCC::Equal, v, Value::UNDEFINED.bits() as i64);
                let ok = b.create_block();
                b.ins().brif(unbound, slow, &[], ok, &[]);
                b.switch_to_block(ok);
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
                let slow = self.slow_block(b, pc, op_addr, None);
                let p = self.alloc(b, 3, header(Kind::Pair, 2, 0), slow);
                let (x, y) = (self.get(b, a), self.get(b, y));
                b.ins().store(flags(), x, p, 8);
                b.ins().store(flags(), y, p, 16);
                let v = Self::tag_ptr(b, p);
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::MkBox { r } => {
                let slow = self.slow_block(b, pc, op_addr, None);
                let p = self.alloc(b, 2, header(Kind::Box, 1, 0), slow);
                let x = self.get(b, r);
                b.ins().store(flags(), x, p, 8);
                let v = Self::tag_ptr(b, p);
                self.set(b, r, v);
                self.jump(b, next);
            }
            Op::Ret { r } => {
                let v = self.get(b, r);
                b.ins().store(flags(), v, self.r, -8);
                let res = b.ins().iconst(I64, (RETURNED as i64) << 32);
                b.ins().return_(&[res]);
            }
            Op::TailCall { base, n } | Op::TailCallG { base, n, .. } => {
                let f = match *op {
                    Op::TailCallG { g, .. } => b.ins().load(I64, flags(), self.globals, (g * 8) as i32),
                    _ => self.get(b, base),
                };
                let interp = self.exit(b, pc, EXIT);
                let (code, entry, size) = self.callee(b, f, n, self.r, interp);
                self.tick(b, pc);
                // A tail call to this same code is a jump to its start (the
                // closure may differ: same code, other captured values).
                let same = b.ins().icmp_imm_s(IntCC::Equal, code, self.code_ptr);
                let other_fn = b.create_block();
                if let Some(start) = self.blocks[0].filter(|_| self.entry0) {
                    let jump = b.create_block();
                    b.ins().brif(same, jump, &[], other_fn, &[]);
                    b.switch_to_block(jump);
                    b.ins().store(flags(), f, self.r, -8);
                    let args: Vec<_> = (0..n).map(|i| self.get(b, base + 1 + i)).collect();
                    let zero = b.ins().iconst(I64, 0);
                    for (i, v) in self.vars.clone().into_iter().enumerate() {
                        b.def_var(v, args.get(i).copied().unwrap_or(zero));
                    }
                    b.ins().jump(start, &[]);
                } else {
                    b.ins().jump(other_fn, &[]);
                }
                b.switch_to_block(other_fn);
                // The callee takes over this frame: closure, then arguments.
                b.ins().store(flags(), f, self.r, -8);
                for i in 0..n {
                    let a = self.get(b, base + 1 + i);
                    b.ins().store(flags(), a, self.r, (i as i32) * 8);
                }
                let from = b.ins().iadd_imm_s(self.r, n as i64 * 8);
                let bytes = b.ins().ishl_imm_s(size, 3);
                let to = b.ins().iadd(self.r, bytes);
                Self::zero_fill(b, from, to);
                let top = b.ins().iadd(self.bp, size);
                self.raise_stack_top(b, top);
                b.ins().store(flags(), code, self.ctx, offset_of!(JitCtx, code) as i32);
                b.ins().store(flags(), self.bp, self.ctx, offset_of!(JitCtx, bp) as i32);
                b.ins().store(flags(), entry, self.ctx, offset_of!(JitCtx, tail) as i32);
                let res = b.ins().iconst(I64, (TAILCALL as i64) << 32);
                b.ins().return_(&[res]);
            }
            Op::Quo { dst, a, b: y } | Op::Rem { dst, a, b: y } | Op::Mod { dst, a, b: y } => {
                let slow = self.slow_block(b, pc, op_addr, None);
                let (x, y) = (self.get(b, a), self.get(b, y));
                let (xi, yi) = (Self::is_int(b, x), Self::is_int(b, y));
                let both = b.ins().band(xi, yi);
                let nonzero = b.ins().icmp_imm_s(IntCC::NotEqual, y, Value::int_unchecked(0).bits() as i64);
                let ok = b.ins().band(both, nonzero);
                let go = b.create_block();
                b.ins().brif(ok, go, &[], slow, &[]);
                b.switch_to_block(go);
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
                let done = b.create_block();
                b.ins().brif(fits, done, &[], slow, &[]);
                b.switch_to_block(done);
                let v = Self::tag_int(b, v);
                self.set(b, dst, v);
                self.jump(b, next);
            }
            Op::Closure { .. } | Op::PopHandler => {
                let slow = self.slow_block(b, pc, op_addr, None);
                b.ins().jump(slow, &[]);
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
                let result = self.compare(b, pc, op_addr, x, y, Self::cmp_ops(op).unwrap(), Some(t as usize));
                let c = b.block_params(result)[0];
                self.branch(b, c, next, t as usize);
            }
            Op::JNLtI { a, i, t } | Op::JNGtI { a, i, t } | Op::JNEqI { a, i, t } => {
                let x = self.get(b, a);
                let k = Self::imm(b, Value::int_unchecked(i as i64));
                let (x, y) = if matches!(op, Op::JNGtI { .. }) { (k, x) } else { (x, k) };
                let result = self.compare(b, pc, op_addr, x, y, Self::cmp_ops(op).unwrap(), Some(t as usize));
                let c = b.block_params(result)[0];
                self.branch(b, c, next, t as usize);
            }
            Op::Lt { dst, a, b: y } | Op::Le { dst, a, b: y } | Op::NumEq { dst, a, b: y } => {
                let (x, y) = (self.get(b, a), self.get(b, y));
                let result = self.compare(b, pc, op_addr, x, y, Self::cmp_ops(op).unwrap(), None);
                let c = b.block_params(result)[0];
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
                self.arith(b, pc, op_addr, dst, x, y, kind);
            }
            Op::AddI { dst, a, i } => {
                let x = self.get(b, a);
                let y = Self::imm(b, Value::int_unchecked(i as i64));
                self.arith(b, pc, op_addr, dst, x, y, '+');
            }
            Op::Car { dst, a } | Op::Cdr { dst, a } => {
                let slow = self.slow_block(b, pc, op_addr, None);
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
                let slow = self.slow_block(b, pc, op_addr, None);
                let (vec, k) = (self.get(b, v), self.get(b, i));
                let p = Self::check_kind(b, vec, Kind::Vector, slow);
                let int = Self::is_int(b, k);
                let idx = Self::untag(b, k);
                let h = b.ins().load(I64, flags(), p, 0);
                let len = b.ins().ushr_imm_s(h, 16);
                let in_range = b.ins().icmp(IntCC::UnsignedLessThan, idx, len);
                let ok = b.ins().band(int, in_range);
                let go = b.create_block();
                b.ins().brif(ok, go, &[], slow, &[]);
                b.switch_to_block(go);
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
            Op::Call { base, n } | Op::CallG { base, n, .. } => {
                let f = match *op {
                    Op::CallG { g, .. } => b.ins().load(I64, flags(), self.globals, (g * 8) as i32),
                    _ => self.get(b, base),
                };
                let other = b.create_block();
                let frame = b.ins().iadd_imm_s(self.r, (base as i64 + 1) * 8);
                let interp = self.exit(b, pc, EXIT);
                let closure = b.create_block();
                let deep = b.ins().icmp_imm_s(IntCC::SignedGreaterThanOrEqual, self.depth, MAX_DEPTH);
                b.ins().brif(deep, other, &[], closure, &[]);
                b.switch_to_block(closure);
                let (_, entry, size) = self.callee(b, f, n, frame, other);
                self.tick(b, pc);
                self.spill(b);
                b.ins().store(flags(), f, self.r, (base as i32) * 8);
                let from = b.ins().iadd_imm_s(frame, n as i64 * 8);
                let bytes = b.ins().ishl_imm_s(size, 3);
                let to = b.ins().iadd(frame, bytes);
                Self::zero_fill(b, from, to);
                let callee_bp = b.ins().iadd_imm_s(self.bp, base as i64 + 1);
                let top = b.ins().iadd(callee_bp, size);
                self.raise_stack_top(b, top);
                let depth = b.ins().iadd_imm_s(self.depth, 1);
                let depth = b.ins().ireduce(I32, depth);
                let zero = b.ins().iconst(I32, 0);
                // Trampoline: a tail call in the callee hands back its successor.
                let call = b.create_block();
                b.append_block_param(call, I64);
                b.ins().jump(call, &[BlockArg::Value(entry)]);
                b.switch_to_block(call);
                let target = b.block_params(call)[0];
                let inst = b.ins().call_indirect(self.jit_sig, target, &[self.vm, self.ctx, frame, callee_bp, zero, depth]);
                let res = b.inst_results(inst)[0];
                let status = b.ins().ushr_imm_s(res, 32);
                let tail = b.create_block();
                let not_tail = b.create_block();
                let is_tail = b.ins().icmp_imm_s(IntCC::Equal, status, TAILCALL as i64);
                b.ins().brif(is_tail, tail, &[], not_tail, &[]);
                b.switch_to_block(tail);
                let next_entry = b.ins().load(I64, flags(), self.ctx, offset_of!(JitCtx, tail) as i32);
                b.ins().jump(call, &[BlockArg::Value(next_entry)]);
                b.switch_to_block(not_tail);
                let returned = b.ins().icmp_imm_s(IntCC::Equal, status, RETURNED as i64);
                let done = b.create_block();
                let bail = b.create_block();
                b.ins().brif(returned, done, &[], bail, &[]);
                b.switch_to_block(done);
                self.reload(b);
                self.jump(b, next);
                // The callee handed over to the interpreter: record this frame.
                b.switch_to_block(bail);
                let fptr = b.ins().iconst(I64, jit_unwind_push as *const () as i64);
                let code = self.code_const(b);
                let ret_pc = b.ins().iconst(I64, next as i64);
                b.ins().call_indirect(self.unwind_sig, fptr, &[self.vm, code, ret_pc, self.bp]);
                b.ins().return_(&[res]);

                // Rust natives (except `apply`) are called here; anything else
                // is left to the interpreter.
                b.switch_to_block(other);
                let tag = b.ins().ushr_imm_s(f, 48);
                let is_native = b.ins().icmp_imm_s(IntCC::Equal, tag, TAG_NATIVE);
                let not_apply = b.ins().icmp_imm_s(IntCC::NotEqual, f, self.apply as i64);
                let ok = b.ins().band(is_native, not_apply);
                let native_call = b.create_block();
                b.ins().brif(ok, native_call, &[], interp, &[]);
                b.switch_to_block(native_call);
                self.spill(b);
                b.ins().store(flags(), f, self.r, (base as i32) * 8);
                let index = b.ins().band_imm_s(f, PAYLOAD);
                let args = b.ins().iadd_imm_s(self.bp, base as i64 + 1);
                let top = b.ins().iadd_imm_s(args, n as i64);
                self.raise_stack_top(b, top);
                let count = b.ins().iconst(I64, n as i64);
                let fptr = b.ins().iconst(I64, jit_native as *const () as i64);
                let inst = b.ins().call_indirect(self.native_sig, fptr, &[self.vm, index, args, count]);
                let v = b.inst_results(inst)[0];
                let k = b.ins().iadd_imm_s(v, -(SENTINEL as i64));
                let special = b.ins().icmp_imm_s(IntCC::UnsignedLessThan, k, 3);
                let done = b.create_block();
                let odd = b.create_block();
                b.ins().brif(special, odd, &[], done, &[]);
                b.switch_to_block(done);
                self.reload(b);
                self.set(b, base, v);
                self.jump(b, next);
                b.switch_to_block(odd);
                let calls = [self.exit(b, next, ERROR), self.exit(b, next, WAIT), self.exit(b, next, RESUME)]
                    .map(|blk| b.func.dfg.block_call(blk, &[]));
                let k = b.ins().ireduce(I32, k);
                let jt = b.create_jump_table(JumpTableData::new(calls[2], &calls[..2]));
                b.ins().br_table(k, jt);
            }
            _ => unreachable!("not a native instruction: {op:?}"),
        }
    }
}
