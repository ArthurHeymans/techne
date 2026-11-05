//! A deliberately small self-recursive integer kernel. See
//! runtime/bench/FAST-CALLS.md for the admission and materialization contract.
use super::*;
use cranelift_module::FuncId;

// Zero is the hot return status; the high bit distinguishes every cold
// exit, including EXIT at pc 0. The ordinary entry removes this bit.
const BAIL: i64 = i64::MIN;
// Numeric live ranges can make a kernel's native frame larger than the
// ordinary reload-after-call frame. Bound the entire kernel chain, not
// just its virtual register span; deeper calls deoptimize precisely.
const MAX_KERNEL_DEPTH: i64 = 128;

pub(super) struct Plan {
    flow: Flow,
    pub advance: usize,
}

pub(super) fn plan(job: &Job) -> Option<Plan> {
    if std::env::var("TECHNE_JIT_FAST").as_deref() == Ok("0")
        || job.rest
        || !(1..=3).contains(&job.nparams)
        || job.ops.len() > 32
        || job.frame_size > 64
        || !job.heads.contains(&0)
        || !job.inlined.is_empty()
    {
        return None;
    }
    let ops = &job.ops;
    let n = job.frame_size as usize;
    let effects: Vec<_> = ops.iter().map(|op| uses_defs(op, &job.captures)).collect();
    if effects.iter().flat_map(|(u, d)| u.iter().chain(d)).any(|&r| r as usize >= n) {
        return None;
    }
    let mut calls = false;
    let mut advance = 1;
    for op in ops {
        match *op {
            Op::LoadI { .. }
            | Op::Mov { .. }
            | Op::AddI { .. }
            | Op::Add { .. }
            | Op::Sub { .. }
            | Op::Mul { .. }
            | Op::JNLt { .. }
            | Op::JNLe { .. }
            | Op::JNNumEq { .. }
            | Op::JNLtI { .. }
            | Op::JNGtI { .. }
            | Op::JNEqI { .. }
            | Op::Jmp { .. }
            | Op::Loop { .. }
            | Op::Ret { .. } => {}
            Op::LoadK { k, .. } if job.consts.get(k as usize).is_some_and(|&v| Value::from_bits(v).is_int()) => {}
            Op::CallG { base, n, g } | Op::TailCallG { base, n, g }
                if n == job.nparams && job.closure_globals.get(&g).is_some_and(|k| k.code == job.code && k.closure.is_some()) =>
            {
                if matches!(op, Op::CallG { .. }) {
                    calls = true;
                    advance = advance.max(base as usize + 1);
                }
            }
            _ => return None,
        }
    }
    if !calls || !matches!(ops.last(), Some(Op::Ret { .. } | Op::TailCallG { .. } | Op::Jmp { .. } | Op::Loop { .. })) {
        return None;
    }
    let succs: Vec<_> = ops.iter().enumerate().map(|(pc, op)| successors(op, pc, ops.len())).collect();
    if succs.iter().flatten().any(|&s| s >= ops.len()) {
        return None;
    }
    let shape = Shape { effects, succs, entries: HashMap::new(), inlined: vec![false; ops.len()], results: HashMap::new() };
    let mut args = Regs::empty(n);
    (0..job.nparams).for_each(|r| args.insert(r));
    let assumed = HashMap::from([(0, args)]);
    let fix = fixnums(ops, n, |_, k| Value::from_bits(job.consts[k as usize]), &job.captures, &[0], |_| true, &shape, &assumed).regs;
    let flow = analyze(ops, n, &job.captures, false, &fix, |_| None, &shape);
    // The self-return hypothesis is valid by induction over normally
    // returning calls: every other definition/return is checked here, and
    // overflow and changed callees exit rather than returning a value.
    for (pc, op) in ops.iter().enumerate() {
        if flow.live_in[pc].iter().any(|r| !fix[pc].contains(r as Reg)) {
            return None;
        }
        if let Op::CallG { base, .. } = *op {
            if flow.live_out[pc].iter().any(|r| r > base as usize) {
                return None;
            }
        }
    }
    Some(Plan { flow, advance })
}

pub(super) struct Compiled {
    pub id: FuncId,
    pub bytes: usize,
    pub insts: usize,
    pub advance: usize,
}

pub(super) fn compile(
    module: &mut JITModule,
    ctx: &mut Context,
    fctx: &mut FunctionBuilderContext,
    job: &Job,
    plan: &Plan,
) -> Option<Compiled> {
    module.clear_context(ctx);
    let sig = &mut ctx.func.signature;
    // ctx, virtual frame index, shifted integer parameters, remaining depth.
    sig.params.extend((0..2 + job.nparams).map(|_| AbiParam::new(I64)));
    sig.params.push(AbiParam::new(I32));
    sig.returns.extend([AbiParam::new(I64), AbiParam::new(I64)]);
    let id = module.declare_anonymous_function(sig).ok()?;
    let own = module.declare_func_in_func(id, &mut ctx.func);
    let frontend = module.isa().frontend_config();
    let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
    let cc = b.func.signature.call_conv;
    let mut helper_sig = |params| {
        let mut s = Signature::new(cc);
        s.params.extend((0..params).map(|_| AbiParam::new(I64)));
        s.returns.push(AbiParam::new(I64));
        b.import_signature(s)
    };
    let prepare = helper_sig(3);
    let unwind = helper_sig(4);
    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    let p = b.block_params(entry).to_vec();
    let vars: Vec<_> = (0..job.frame_size).map(|_| b.declare_var(I64)).collect();
    let zero = b.ins().iconst(I64, 0);
    for (i, &v) in vars.iter().enumerate() {
        b.def_var(v, p.get(2 + i).copied().filter(|_| i < job.nparams as usize).unwrap_or(zero));
    }
    let blocks: Vec<_> = job.ops.iter().map(|_| b.create_block()).collect();
    let mut g =
        Kernel { ctx: p[0], bp: p[1], left: *p.last().unwrap(), vars, blocks, exits: HashMap::new(), own, prepare, unwind, job, plan };
    b.ins().jump(g.blocks[0], &[]);
    for (pc, op) in job.ops.iter().enumerate() {
        b.switch_to_block(g.blocks[pc]);
        g.op(&mut b, pc, *op);
    }
    let exits: Vec<_> = g.exits.iter().map(|(&key, &block)| (key, block)).collect();
    for ((pc, status), block) in exits {
        b.switch_to_block(block);
        // Only the innermost exit initializes the active span. Callers
        // materialize below it on their way out; they must not clear it.
        let frame = b.ins().iconst(I64, job.frame_size as i64);
        let r = g.helper(&mut b, g.prepare, jit_fast_prepare as *const (), &[g.ctx, g.bp, frame]);
        g.pointer_or_return(&mut b, r, status, pc);
        for i in plan.flow.live_in[pc].iter() {
            let s = b.use_var(g.vars[i]);
            let v = Gen::tag_shifted(&mut b, s);
            b.ins().store(flags(), v, r, (i * 8) as i32);
        }
        let code = b.ins().iconst(I64, job.code as i64);
        b.ins().store(flags(), code, g.ctx, offset_of!(JitCtx, code) as i32);
        b.ins().store(flags(), g.bp, g.ctx, offset_of!(JitCtx, bp) as i32);
        let result = b.ins().iconst(I64, BAIL | ((status as i64) << 32) | pc as i64);
        b.ins().return_(&[zero, result]);
    }
    b.seal_all_blocks();
    b.finalize(frontend);
    let insts = ctx.func.layout.blocks().map(|bl| ctx.func.layout.block_insts(bl).count()).sum();
    let asm = std::env::var("TECHNE_JIT_ASM").is_ok_and(|v| v == job.name);
    ctx.set_disasm(asm);
    module.define_function(id, ctx).ok()?;
    if let Some(text) = ctx.compiled_code().and_then(|c| c.vcode.as_ref()).filter(|_| asm) {
        eprintln!("fast-asm: {}\n{text}", job.name);
    }
    let bytes = ctx.compiled_code()?.code_info().total_size as usize;
    if std::env::var_os("TECHNE_JIT_LOG").is_some() {
        eprintln!("fast-jit: {} {} ops -> {insts} IR insts, {bytes} code bytes", job.name, job.ops.len());
    }
    Some(Compiled { id, bytes: bytes.next_multiple_of(PAGE), insts, advance: plan.advance })
}

struct Kernel<'a> {
    ctx: ir::Value,
    bp: ir::Value,
    left: ir::Value,
    vars: Vec<Variable>,
    blocks: Vec<Block>,
    exits: HashMap<(usize, u32), Block>,
    own: ir::FuncRef,
    prepare: ir::SigRef,
    unwind: ir::SigRef,
    job: &'a Job,
    plan: &'a Plan,
}

impl Kernel<'_> {
    fn get(&self, b: &mut FunctionBuilder, r: Reg) -> ir::Value {
        b.use_var(self.vars[r as usize])
    }
    fn set(&self, b: &mut FunctionBuilder, r: Reg, v: ir::Value) {
        b.def_var(self.vars[r as usize], v);
    }
    fn exit(&mut self, b: &mut FunctionBuilder, pc: usize, status: u32) -> Block {
        *self.exits.entry((pc, status)).or_insert_with(|| {
            let bl = b.create_block();
            b.set_cold_block(bl);
            bl
        })
    }
    fn helper(&self, b: &mut FunctionBuilder, sig: ir::SigRef, f: *const (), args: &[ir::Value]) -> ir::Value {
        let f = b.ins().iconst(I64, f as i64);
        let call = b.ins().call_indirect(sig, f, args);
        b.inst_results(call)[0]
    }
    fn pointer_or_return(&self, b: &mut FunctionBuilder, r: ir::Value, status: u32, pc: usize) {
        let good = b.create_block();
        let bad = b.create_block();
        b.set_cold_block(bad);
        b.ins().brif(r, good, &[], bad, &[]);
        b.switch_to_block(bad);
        // A contained core panic poisons the VM; the dispatcher checks it
        // before consuming status or touching the reconstructed frames.
        let zero = b.ins().iconst(I64, 0);
        let status = b.ins().iconst(I64, BAIL | ((status as i64) << 32) | pc as i64);
        b.ins().return_(&[zero, status]);
        b.switch_to_block(good);
    }
    fn tick(&mut self, b: &mut FunctionBuilder, pc: usize) {
        let ptr = b.ins().load(I64, flags(), self.ctx, offset_of!(JitCtx, fuel) as i32);
        let old = b.ins().load(I32, flags(), ptr, 0);
        let left = b.ins().iadd_imm_s(old, -1);
        b.ins().store(flags(), left, ptr, 0);
        let stop = self.exit(b, pc, TICK);
        Gen::guard(b, left, stop);
    }
    fn op(&mut self, b: &mut FunctionBuilder, pc: usize, op: Op) {
        let next = pc + 1;
        match op {
            Op::LoadI { dst, i } => {
                let v = b.ins().iconst(I64, (i as i64) << 16);
                self.set(b, dst, v);
            }
            Op::LoadK { dst, k } => {
                let v = b.ins().iconst(I64, Value::from_bits(self.job.consts[k as usize]).as_int() << 16);
                self.set(b, dst, v);
            }
            Op::Mov { dst, src } => {
                let v = self.get(b, src);
                self.set(b, dst, v);
            }
            Op::AddI { dst, a, i } => {
                let x = self.get(b, a);
                let y = b.ins().iconst(I64, (i as i64) << 16);
                let (v, ov) = b.ins().sadd_overflow(x, y);
                let good = b.ins().icmp_imm_s(IntCC::Equal, ov, 0);
                let fail = self.exit(b, pc, STEP);
                Gen::guard(b, good, fail);
                self.set(b, dst, v);
            }
            Op::Add { dst, a, b: r } | Op::Sub { dst, a, b: r } | Op::Mul { dst, a, b: r } => {
                let x = self.get(b, a);
                let y = self.get(b, r);
                let (v, ov) = match op {
                    Op::Add { .. } => b.ins().sadd_overflow(x, y),
                    Op::Sub { .. } => b.ins().ssub_overflow(x, y),
                    _ => {
                        let y = b.ins().sshr_imm_s(y, 16);
                        b.ins().smul_overflow(x, y)
                    }
                };
                let good = b.ins().icmp_imm_s(IntCC::Equal, ov, 0);
                let fail = self.exit(b, pc, STEP);
                Gen::guard(b, good, fail);
                self.set(b, dst, v);
            }
            Op::Jmp { t } | Op::Loop { t } => {
                if matches!(op, Op::Loop { .. }) {
                    self.tick(b, t as usize);
                }
                b.ins().jump(self.blocks[t as usize], &[]);
                return;
            }
            Op::JNLt { a, b: r, t } | Op::JNLe { a, b: r, t } | Op::JNNumEq { a, b: r, t } => {
                let x = self.get(b, a);
                let y = self.get(b, r);
                let c = b.ins().icmp(Gen::cmp_ops(&op).0, x, y);
                b.ins().brif(c, self.blocks[next], &[], self.blocks[t as usize], &[]);
                return;
            }
            Op::JNLtI { a, i, t } | Op::JNGtI { a, i, t } | Op::JNEqI { a, i, t } => {
                let x = self.get(b, a);
                let y = b.ins().iconst(I64, (i as i64) << 16);
                let c = if matches!(op, Op::JNGtI { .. }) {
                    b.ins().icmp(IntCC::SignedLessThan, y, x)
                } else {
                    b.ins().icmp(Gen::cmp_ops(&op).0, x, y)
                };
                b.ins().brif(c, self.blocks[next], &[], self.blocks[t as usize], &[]);
                return;
            }
            Op::Ret { r } => {
                let v = self.get(b, r);
                let ok = b.ins().iconst(I64, 0);
                b.ins().return_(&[v, ok]);
                return;
            }
            Op::CallG { base, n, g } | Op::TailCallG { base, n, g } => {
                let tail = matches!(op, Op::TailCallG { .. });
                let k = self.job.closure_globals[&g];
                let globals = b.ins().load(I64, flags(), self.ctx, offset_of!(JitCtx, globals) as i32);
                let f = b.ins().load(I64, flags(), globals, (g * 8) as i32);
                let addr = b.ins().iconst(I64, k.closure.unwrap() as i64);
                let expected = b.ins().load(I64, flags(), addr, 0);
                let same = b.ins().icmp(IntCC::Equal, f, expected);
                let fail = self.exit(b, pc, EXIT);
                Gen::guard(b, same, fail);
                if !tail {
                    Gen::guard(b, self.left, fail);
                }
                self.tick(b, pc);
                let args: Vec<_> = (0..n).map(|i| self.get(b, base + 1 + i)).collect();
                if tail {
                    let zero = b.ins().iconst(I64, 0);
                    for (i, &var) in self.vars.iter().enumerate() {
                        b.def_var(var, args.get(i).copied().unwrap_or(zero));
                    }
                    b.ins().jump(self.blocks[0], &[]);
                    return;
                }
                let bp = b.ins().iadd_imm_s(self.bp, base as i64 + 1);
                let left = b.ins().iadd_imm_s(self.left, -1);
                let call_args: Vec<_> = [self.ctx, bp].into_iter().chain(args).chain([left]).collect();
                let call = b.ins().call(self.own, &call_args);
                let (value, status) = (b.inst_results(call)[0], b.inst_results(call)[1]);
                let returned = b.ins().icmp_imm_s(IntCC::Equal, status, 0);
                let done = b.create_block();
                let bail = b.create_block();
                b.set_cold_block(bail);
                b.ins().brif(returned, done, &[], bail, &[]);
                b.switch_to_block(bail);
                let code = b.ins().iconst(I64, self.job.code as i64);
                let ret = b.ins().iconst(I64, next as i64);
                let r = self.helper(b, self.unwind, jit_fast_unwind as *const (), &[self.ctx, self.bp, code, ret]);
                self.pointer_or_return(b, r, EXIT, pc);
                for i in 0..base as usize {
                    let v = if self.plan.flow.live_out[pc].contains(i as Reg) {
                        let s = self.get(b, i as Reg);
                        Gen::tag_shifted(b, s)
                    } else {
                        b.ins().iconst(I64, 0)
                    };
                    b.ins().store(flags(), v, r, (i * 8) as i32);
                }
                let addr = b.ins().iconst(I64, k.closure.unwrap() as i64);
                let f = b.ins().load(I64, flags(), addr, 0);
                b.ins().store(flags(), f, r, (base as i32) * 8);
                let zero = b.ins().iconst(I64, 0);
                b.ins().return_(&[zero, status]);
                b.switch_to_block(done);
                self.set(b, base, value);
            }
            _ => unreachable!("not admitted: {op:?}"),
        }
        b.ins().jump(self.blocks[next], &[]);
    }
}

/// Try the numeric kernel only at pc 0, after ordinary frame publication.
/// Capacity/depth are budgets, not a new register-stack allocation policy.
pub(super) fn enter(module: &mut JITModule, b: &mut FunctionBuilder, g: &Gen, kernel: &Compiled, job: &Job) {
    let regular = b.create_block();
    let fast = b.create_block();
    let mut bad = b.ins().iconst(I64, 0);
    let mut args = Vec::new();
    for i in 0..job.nparams {
        let x = b.ins().load(I64, flags(), g.r, (i as i32) * 8);
        let t = b.ins().bxor_imm_s(x, TAG_INT << 48);
        bad = b.ins().bor(bad, t);
        args.push(Gen::shifted(b, x));
    }
    let mut bad = b.ins().ushr_imm_s(bad, 48);
    // Tail jumps do not write a new closure slot. Require one actual
    // closure throughout the kernel, not merely one code identity: even
    // an unused environment/closure can be observed through weak keys.
    let actual = b.ins().load(I64, flags(), g.r, -8);
    for address in job.closure_globals.values().filter(|k| k.code == job.code).filter_map(|k| k.closure) {
        let address = b.ins().iconst(I64, address as i64);
        let expected = b.ins().load(I64, flags(), address, 0);
        let different = b.ins().bxor(actual, expected);
        bad = b.ins().bor(bad, different);
    }
    let full = b.ins().icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, g.depth, MAX_DEPTH);
    let full = b.ins().uextend(I64, full);
    bad = b.ins().bor(bad, full);
    b.ins().brif(bad, regular, &[], fast, &[]);
    b.switch_to_block(fast);
    let bytes = b.ins().isub(g.regs_end, g.r);
    let words = b.ins().ushr_imm_s(bytes, 3);
    let spare = b.ins().iadd_imm_s(words, -(job.frame_size as i64));
    let advance = b.ins().iconst(I64, kernel.advance as i64);
    let room = b.ins().udiv(spare, advance);
    // Count the companion's initial activation as well as its children.
    let max = b.ins().iconst(I64, MAX_DEPTH - 1);
    let depth = b.ins().isub(max, g.depth);
    let cap = b.ins().iconst(I64, MAX_KERNEL_DEPTH - 1);
    let depth = b.ins().umin(depth, cap);
    let left = b.ins().umin(room, depth);
    let left = b.ins().ireduce(I32, left);
    b.ins().store(flags(), g.bp, g.ctx, offset_of!(JitCtx, fast_root) as i32);
    let own = module.declare_func_in_func(kernel.id, b.func);
    let args: Vec<_> = [g.ctx, g.bp].into_iter().chain(args).chain([left]).collect();
    let call = b.ins().call(own, &args);
    let (value, status) = (b.inst_results(call)[0], b.inst_results(call)[1]);
    let returned = b.ins().icmp_imm_s(IntCC::Equal, status, 0);
    let done = b.create_block();
    let bail = b.create_block();
    b.ins().brif(returned, done, &[], bail, &[]);
    b.switch_to_block(bail);
    let status = b.ins().band_imm_s(status, i64::MAX);
    b.ins().return_(&[status]);
    b.switch_to_block(done);
    let value = Gen::tag_shifted(b, value);
    b.ins().store(flags(), value, g.r, -8);
    let returned = b.ins().iconst(I64, (RETURNED as i64) << 32);
    b.ins().return_(&[returned]);
    b.switch_to_block(regular);
}
