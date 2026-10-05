//! Baseline JIT (Cranelift) for loops.
//!
//! When a function's loop back-edges have run `threshold` times, the whole
//! function is compiled and each loop head's instruction is replaced by
//! `Op::EnterJit`. The interpreter jumps into native code there and gets back
//! the bytecode `pc` at which to continue. Native code handles straight-line
//! instructions, branches, loops and calls to Rust natives. Calls to Scheme
//! procedures, returns, closure creation and handler installation exit to the
//! interpreter, so frames, unwinding and task suspension work as before.
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
    heap::Kind,
    value::Value,
    vm::Vm,
};

/// `(vm, registers at bp, globals, fuel, entry pc) -> pc | status << 32`.
pub type JitFn = unsafe extern "C" fn(*mut Vm, *mut Value, *mut Value, *mut u32, u32) -> u64;

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

/// Per-function JIT state.
#[derive(Default)]
pub struct JitSlot {
    /// Loop back-edges taken so far (until compiled).
    pub hot: Cell<u32>,
    pub entry: Cell<Option<JitFn>>,
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

/// Instructions with native code: `native` ones and non-tail calls, which run
/// Rust natives directly and exit to the interpreter for anything else.
fn compiled(op: &Op) -> bool {
    native(op) || matches!(op, Op::Call { .. } | Op::CallG { .. })
}

/// Instructions native code always executes itself (possible entry points).
pub fn native(op: &Op) -> bool {
    !matches!(
        op,
        Op::Closure { .. }
            | Op::PushHandler { .. }
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
        Call { base, n } | CallG { base, n, .. } => (base..=base + n).collect(),
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

unsafe extern "C" fn jit_call(vm: *mut Vm, r: *mut Value, op: *const Op) -> u32 {
    unsafe { (*vm).jit_call_op(r, *op) }
}

unsafe extern "C" fn jit_barrier(vm: *mut Vm, obj: *mut u64, v: Value) {
    unsafe { (*vm).write_barrier(obj, v) }
}

const TAG_INT: i64 = 0xFFF9;
const TAG_PTR: i64 = 0xFFFA;
const PAYLOAD: i64 = (1 << 48) - 1;
const CANONICAL_NAN: i64 = 0x7FF8_0000_0000_0000;

impl Jit {
    /// A JIT for the host, or `None` if Cranelift does not support it.
    pub fn new(threshold: u32) -> Option<Jit> {
        let mut flags = settings::builder();
        flags.set("opt_level", "speed").ok()?;
        flags.set("use_colocated_libcalls", "false").ok()?;
        flags.set("is_pic", "false").ok()?;
        let isa = cranelift_native::builder().ok()?.finish(settings::Flags::new(flags)).ok()?;
        let module = JITModule::new(JITBuilder::with_isa(isa, cranelift_module::default_libcall_names()));
        Some(Jit { ctx: module.make_context(), module, fctx: FunctionBuilderContext::new(), threshold })
    }

    /// Compile `code` with entry points at `heads`; `orig` are its original
    /// instructions (their addresses are passed to the slow path).
    pub fn compile(&mut self, code: &Code, orig: &[Op], heads: &[usize]) -> Option<JitFn> {
        let n = code.frame_size as usize;
        if orig.iter().filter(|op| compiled(op)).flat_map(regs_of).any(|r| r as usize >= n) {
            return None;
        }
        self.module.clear_context(&mut self.ctx);
        let sig = &mut self.ctx.func.signature;
        sig.params.extend([AbiParam::new(I64), AbiParam::new(I64), AbiParam::new(I64), AbiParam::new(I64), AbiParam::new(I32)]);
        sig.returns.push(AbiParam::new(I64));
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

        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let p = b.block_params(entry).to_vec();
        let vars: Vec<Variable> = (0..n).map(|_| b.declare_var(I64)).collect();
        let mut g = Gen {
            vm: p[0],
            r: p[1],
            globals: p[2],
            fuel: p[3],
            vars,
            blocks: orig.iter().map(|op| compiled(op).then(|| b.create_block())).collect(),
            exits: HashMap::new(),
            step_sig,
            barrier_sig,
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
    r: ir::Value,
    globals: ir::Value,
    fuel: ir::Value,
    vars: Vec<Variable>,
    /// Native instruction blocks by pc.
    blocks: Vec<Option<Block>>,
    /// Exit blocks by (pc, status).
    exits: HashMap<(usize, u32), Block>,
    step_sig: ir::SigRef,
    barrier_sig: ir::SigRef,
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
            Op::MkBox { .. } | Op::Cons { .. } | Op::Quo { .. } | Op::Rem { .. } | Op::Mod { .. } | Op::PopHandler => {
                let slow = self.slow_block(b, pc, op_addr, None);
                b.ins().jump(slow, &[]);
            }
            Op::Jmp { t } => self.jump(b, t as usize),
            Op::Loop { t } => {
                let f = b.ins().load(I32, flags(), self.fuel, 0);
                let f = b.ins().iadd_imm_s(f, -1);
                b.ins().store(flags(), f, self.fuel, 0);
                let out = b.ins().icmp_imm_s(IntCC::Equal, f, 0);
                let tick = self.exit(b, t as usize, TICK);
                let go = self.target(b, t as usize);
                b.ins().brif(out, tick, &[], go, &[]);
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
            Op::Call { .. } | Op::CallG { .. } => {
                self.spill(b);
                let f = b.ins().iconst(I64, jit_call as *const () as i64);
                let op = b.ins().iconst(I64, op_addr);
                let call = b.ins().call_indirect(self.step_sig, f, &[self.vm, self.r, op]);
                let status = b.inst_results(call)[0];
                // 0: done; 1: not a native, interpret the call; 2: error;
                // 3: suspend; 4: done, but the register stack moved.
                let done = b.create_block();
                let interp = self.exit(b, pc, EXIT);
                let fail = self.exit(b, next, ERROR);
                let wait = self.exit(b, next, WAIT);
                let moved = self.exit(b, next, RESUME);
                let calls = [done, interp, fail, wait, moved].map(|blk| b.func.dfg.block_call(blk, &[]));
                let jt = b.create_jump_table(JumpTableData::new(calls[4], &calls[..4]));
                b.ins().br_table(status, jt);
                b.switch_to_block(done);
                self.reload(b);
                self.jump(b, next);
            }
            _ => unreachable!("not a native instruction: {op:?}"),
        }
    }
}
