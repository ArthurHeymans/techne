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
    fix: Vec<Regs>,
    pairs: Vec<Regs>,
    fix_params: Regs,
    ret_fix: bool,
    uses_heap: bool,
    allocs: bool,
    pass_cage: bool,
    pass_vm: bool,
    pub advance: usize,
}

pub(super) fn plan(job: &Job) -> Option<Plan> {
    if std::env::var("TECHNE_JIT_FAST").as_deref() == Ok("0")
        || job.rest
        || !(1..=3).contains(&job.nparams)
        || job.ops.len() > 40
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
    let mut uses_heap = false;
    let mut allocs = false;
    let mut advance = 1;
    let mut num_used = Regs::empty(n);
    let mut non_num_used = Regs::empty(n);
    for op in ops {
        match *op {
            Op::LoadI { .. } | Op::Mov { .. } | Op::Jmp { .. } | Op::Loop { .. } | Op::Ret { .. } => {}
            Op::AddI { a, .. } | Op::JNLtI { a, .. } | Op::JNGtI { a, .. } | Op::JNEqI { a, .. } => {
                num_used.insert(a);
            }
            Op::Add { a, b, .. }
            | Op::Sub { a, b, .. }
            | Op::Mul { a, b, .. }
            | Op::JNLt { a, b, .. }
            | Op::JNLe { a, b, .. }
            | Op::JNNumEq { a, b, .. } => {
                num_used.insert(a);
                num_used.insert(b);
            }
            Op::Car { a, .. } | Op::Cdr { a, .. } | Op::PairP { a, .. } | Op::JNPair { a, .. } => {
                uses_heap = true;
                non_num_used.insert(a);
            }
            Op::NullP { a, .. } | Op::JNNull { a, .. } => {
                non_num_used.insert(a);
            }
            Op::Not { .. } | Op::Jf { .. } | Op::Jt { .. } => {}
            Op::Cons { .. } => {
                uses_heap = true;
                allocs = true;
            }
            Op::LoadK { k, .. } if job.consts.get(k as usize).is_some_and(|&v| !Value::from_bits(v).is_ptr()) => {}
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
    loop {
        let before = (num_used.clone(), non_num_used.clone());
        for op in ops {
            if let Op::Mov { dst, src } = *op {
                if num_used.contains(dst) {
                    num_used.insert(src);
                }
                if non_num_used.contains(dst) {
                    non_num_used.insert(src);
                }
            }
        }
        if (num_used.clone(), non_num_used.clone()) == before {
            break;
        }
    }
    let mut fix_params = Regs::empty(n);
    for r in 0..job.nparams {
        if num_used.contains(r) && !non_num_used.contains(r) {
            fix_params.insert(r);
        }
    }
    let shape = Shape { effects, succs, entries: HashMap::default(), inlined: vec![false; ops.len()], results: HashMap::default() };
    let assumed: HashMap<usize, Regs> = [(0usize, fix_params.clone())].into_iter().collect();
    let konst = |_: usize, k: u32| Value::from_bits(job.consts[k as usize]);
    let fix_try = fixnums(ops, n, konst, &job.captures, &[0], |pc| matches!(ops[pc], Op::CallG { .. }), &[], &shape, &assumed).regs;
    let ret_fix = ops.iter().enumerate().all(|(pc, op)| match *op {
        Op::Ret { r } => fix_try[pc].contains(r),
        _ => true,
    });
    let fix = if ret_fix { fix_try } else { fixnums(ops, n, konst, &job.captures, &[0], |_| false, &[], &shape, &assumed).regs };
    let flow = analyze(ops, n, &job.captures, false, &fix, |_| None, &shape);
    // Reject if any control-flow edge changes a live register's stored
    // representation (shifted fixnum vs tagged Value) without redefining it.
    for (pc, op) in ops.iter().enumerate() {
        let (_, defs) = uses_defs(op, &HashMap::default());
        for &s in &shape.succs[pc] {
            for r in flow.live_in[s].iter() {
                let r = r as Reg;
                if s == pc + 1 && defs.contains(&r) {
                    continue;
                }
                if fix[pc].contains(r) != fix[s].contains(r) {
                    return None;
                }
            }
        }
    }
    let pairs = if uses_heap { pairs(ops, n, &job.captures, &[0], &shape) } else { vec![Regs::empty(n); ops.len()] };
    // Every numeric op and every fixnum self-call parameter must have known
    // fixnum operands; overflow and type-check failures exit cleanly.
    for (pc, op) in ops.iter().enumerate() {
        match *op {
            Op::AddI { a, .. } | Op::JNLtI { a, .. } | Op::JNGtI { a, .. } | Op::JNEqI { a, .. } => {
                if !fix[pc].contains(a) {
                    return None;
                }
            }
            Op::Add { a, b, .. }
            | Op::Sub { a, b, .. }
            | Op::Mul { a, b, .. }
            | Op::JNLt { a, b, .. }
            | Op::JNLe { a, b, .. }
            | Op::JNNumEq { a, b, .. } => {
                if !fix[pc].contains(a) || !fix[pc].contains(b) {
                    return None;
                }
            }
            Op::CallG { base, n: argc, .. } | Op::TailCallG { base, n: argc, .. } => {
                for i in 0..argc {
                    if fix_params.contains(i) && !fix[pc].contains(base + 1 + i) {
                        return None;
                    }
                }
                if matches!(op, Op::CallG { .. }) && flow.live_out[pc].iter().any(|r| r > base as usize) {
                    return None;
                }
            }
            _ => {}
        }
    }
    let pass_cage = uses_heap && job.nparams <= 2;
    let pass_vm = allocs && pass_cage && job.nparams <= 1;
    Some(Plan { flow, fix, pairs, fix_params, ret_fix, uses_heap, allocs, pass_cage, pass_vm, advance })
}

pub(super) struct Compiled {
    pub id: FuncId,
    pub bytes: usize,
    pub insts: usize,
    pub advance: usize,
    pub fix_params: Regs,
    pub ret_fix: bool,
    pub pass_cage: bool,
    pub pass_vm: bool,
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
    // ctx, virtual frame index, parameters, [cage], [vm], packed (left << 32 | fuel).
    let extra = usize::from(plan.pass_cage) + usize::from(plan.pass_vm);
    sig.params.extend((0..3 + job.nparams as usize + extra).map(|_| AbiParam::new(I64)));
    // (shifted return value, remaining fuel >= 0 OR negative BAIL status).
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
    let packed = *p.last().unwrap();
    let left = b.ins().ushr_imm_s(packed, 32);
    let fuel = b.ins().ireduce(I32, packed);
    let fuel = b.ins().uextend(I64, fuel);
    let fuel_var = b.declare_var(I64);
    b.def_var(fuel_var, fuel);
    let mut extra_idx = 2 + job.nparams as usize;
    let cage = if plan.pass_cage {
        let v = p[extra_idx];
        extra_idx += 1;
        Some(v)
    } else if plan.uses_heap {
        Some(b.ins().load(I64, flags(), p[0], offset_of!(JitCtx, cage) as i32))
    } else {
        None
    };
    let vm = if plan.pass_vm {
        Some(p[extra_idx])
    } else if plan.allocs {
        Some(b.ins().load(I64, flags(), p[0], offset_of!(JitCtx, vm) as i32))
    } else {
        None
    };
    let blocks: Vec<_> = job.ops.iter().map(|_| b.create_block()).collect();
    let mut g =
        Kernel { ctx: p[0], bp: p[1], left, fuel_var, cage, vm, vars, blocks, exits: HashMap::default(), own, prepare, unwind, job, plan };
    b.ins().jump(g.blocks[0], &[]);
    for (pc, op) in job.ops.iter().enumerate() {
        b.switch_to_block(g.blocks[pc]);
        g.op(&mut b, pc, *op);
    }
    let exits: Vec<_> = g.exits.iter().map(|(&key, &block)| (key, block)).collect();
    for ((pc, status), block) in exits {
        b.switch_to_block(block);
        // Publish remaining fuel before leaving the kernel.
        let fuel_ptr = b.ins().load(I64, flags(), g.ctx, offset_of!(JitCtx, fuel) as i32);
        let rem = b.use_var(g.fuel_var);
        let rem32 = b.ins().ireduce(I32, rem);
        b.ins().store(flags(), rem32, fuel_ptr, 0);
        // Only the innermost exit initializes the active span. Callers
        // materialize below it on their way out; they must not clear it.
        let frame = b.ins().iconst(I64, job.frame_size as i64);
        let r = g.helper(&mut b, g.prepare, jit_fast_prepare as *const (), &[g.ctx, g.bp, frame]);
        g.pointer_or_return(&mut b, r, status, pc);
        for i in plan.flow.live_in[pc].iter() {
            let s = b.use_var(g.vars[i]);
            let v = if plan.fix[pc].contains(i as Reg) { Gen::tag_shifted(&mut b, s) } else { s };
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
    Some(Compiled {
        id,
        bytes: bytes.next_multiple_of(PAGE),
        insts,
        advance: plan.advance,
        fix_params: plan.fix_params.clone(),
        ret_fix: plan.ret_fix,
        pass_cage: plan.pass_cage,
        pass_vm: plan.pass_vm,
    })
}

struct Kernel<'a> {
    ctx: ir::Value,
    bp: ir::Value,
    left: ir::Value,
    fuel_var: Variable,
    cage: Option<ir::Value>,
    vm: Option<ir::Value>,
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
    fn val(&self, b: &mut FunctionBuilder, pc: usize, r: Reg) -> ir::Value {
        let v = self.get(b, r);
        if self.plan.fix[pc].contains(r) { Gen::tag_shifted(b, v) } else { v }
    }
    fn set(&self, b: &mut FunctionBuilder, r: Reg, v: ir::Value) {
        b.def_var(self.vars[r as usize], v);
    }
    fn set_fix(&self, b: &mut FunctionBuilder, pc: usize, dst: Reg, shifted: ir::Value) {
        let v = if self.plan.fix.get(pc + 1).is_some_and(|f| f.contains(dst)) { shifted } else { Gen::tag_shifted(b, shifted) };
        self.set(b, dst, v);
    }
    fn ptr(&self, b: &mut FunctionBuilder, x: ir::Value) -> ir::Value {
        let off = b.ins().band_imm_s(x, crate::alloc::CAGE_MASK as i64);
        b.ins().iadd(self.cage.unwrap(), off)
    }
    fn check_pair(&self, b: &mut FunctionBuilder, x: ir::Value, slow: Block) -> ir::Value {
        let ptr = Gen::is_ptr(b, x);
        Gen::guard(b, ptr, slow);
        let p = self.ptr(b, x);
        let h = b.ins().load(I64, flags(), p, 0);
        let kind = b.ins().band_imm_s(h, 0xFF);
        let pair = b.ins().icmp_imm_s(IntCC::Equal, kind, Kind::Pair as i64);
        Gen::guard(b, pair, slow);
        p
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
        let old = b.use_var(self.fuel_var);
        let rem = b.ins().iadd_imm_s(old, -1);
        b.def_var(self.fuel_var, rem);
        let stop = self.exit(b, pc, TICK);
        Gen::guard(b, rem, stop);
    }
    fn op(&mut self, b: &mut FunctionBuilder, pc: usize, op: Op) {
        let next = pc + 1;
        match op {
            Op::LoadI { dst, i } => {
                let v = b.ins().iconst(I64, (i as i64) << 16);
                self.set_fix(b, pc, dst, v);
            }
            Op::LoadK { dst, k } => {
                let c = Value::from_bits(self.job.consts[k as usize]);
                if c.is_int() {
                    let v = b.ins().iconst(I64, c.as_int() << 16);
                    self.set_fix(b, pc, dst, v);
                } else {
                    let v = b.ins().iconst(I64, c.bits() as i64);
                    self.set(b, dst, v);
                }
            }
            Op::Mov { dst, src } => {
                if self.plan.fix[pc].contains(src) {
                    let v = self.get(b, src);
                    self.set_fix(b, pc, dst, v);
                } else {
                    let v = self.get(b, src);
                    self.set(b, dst, v);
                }
            }
            Op::AddI { dst, a, i } => {
                let x = self.get(b, a);
                let y = b.ins().iconst(I64, (i as i64) << 16);
                let (v, ov) = b.ins().sadd_overflow(x, y);
                let fail = self.exit(b, pc, STEP);
                let ok = b.create_block();
                b.ins().brif(ov, fail, &[], ok, &[]);
                b.switch_to_block(ok);
                self.set_fix(b, pc, dst, v);
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
                let fail = self.exit(b, pc, STEP);
                let ok = b.create_block();
                b.ins().brif(ov, fail, &[], ok, &[]);
                b.switch_to_block(ok);
                self.set_fix(b, pc, dst, v);
            }
            Op::Car { dst, a } | Op::Cdr { dst, a } => {
                let x = self.val(b, pc, a);
                let p = if self.plan.pairs[pc].contains(a) {
                    self.ptr(b, x)
                } else {
                    let slow = self.exit(b, pc, STEP);
                    self.check_pair(b, x, slow)
                };
                let off = if matches!(op, Op::Car { .. }) { 8 } else { 16 };
                let v = b.ins().load(I64, flags(), p, off);
                self.set(b, dst, v);
            }
            Op::Cons { dst, a, b: y } => {
                let slow = self.exit(b, pc, STEP);
                let vm = self.vm.unwrap();
                let top_off = (offset_of!(Vm, heap) + offset_of!(Heap, top)) as i32;
                let lim_off = (offset_of!(Vm, heap) + offset_of!(Heap, jit_limit)) as i32;
                let top = b.ins().load(I64, flags(), vm, top_off);
                let lim = b.ins().load(I64, flags(), vm, lim_off);
                let next_top = b.ins().iadd_imm_s(top, 24);
                let fits = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, next_top, lim);
                let ok = b.create_block();
                b.ins().brif(fits, ok, &[], slow, &[]);
                b.switch_to_block(ok);
                b.ins().store(flags(), next_top, vm, top_off);
                let h = b.ins().iconst(I64, header(Kind::Pair, 2, 0) as i64);
                let va = self.val(b, pc, a);
                let vb = self.val(b, pc, y);
                b.ins().store(flags(), h, top, 0);
                b.ins().store(flags(), va, top, 8);
                b.ins().store(flags(), vb, top, 16);
                let off = b.ins().isub(top, self.cage.unwrap());
                let ptr = b.ins().bor_imm_s(off, TAG_PTR << 48);
                self.set(b, dst, ptr);
            }
            Op::Not { dst, a } => {
                let is_false = if self.plan.fix[pc].contains(a) {
                    b.ins().iconst(I8, 0)
                } else {
                    let x = self.get(b, a);
                    b.ins().icmp_imm_s(IntCC::Equal, x, Value::FALSE.bits() as i64)
                };
                let t = b.ins().iconst(I64, Value::TRUE.bits() as i64);
                let f = b.ins().iconst(I64, Value::FALSE.bits() as i64);
                let v = b.ins().select(is_false, t, f);
                self.set(b, dst, v);
            }
            Op::NullP { dst, a } => {
                let is_nil = if self.plan.fix[pc].contains(a) {
                    b.ins().iconst(I8, 0)
                } else {
                    let x = self.get(b, a);
                    b.ins().icmp_imm_s(IntCC::Equal, x, Value::NIL.bits() as i64)
                };
                let t = b.ins().iconst(I64, Value::TRUE.bits() as i64);
                let f = b.ins().iconst(I64, Value::FALSE.bits() as i64);
                let v = b.ins().select(is_nil, t, f);
                self.set(b, dst, v);
            }
            Op::PairP { dst, a } => {
                let t = b.ins().iconst(I64, Value::TRUE.bits() as i64);
                let f = b.ins().iconst(I64, Value::FALSE.bits() as i64);
                if self.plan.fix[pc].contains(a) {
                    self.set(b, dst, f);
                } else {
                    let x = self.get(b, a);
                    let ptr = Gen::is_ptr(b, x);
                    let chk = b.create_block();
                    let done = b.create_block();
                    b.append_block_param(done, I64);
                    b.ins().brif(ptr, chk, &[], done, &[BlockArg::Value(f)]);
                    b.switch_to_block(chk);
                    let p = self.ptr(b, x);
                    let h = b.ins().load(I64, flags(), p, 0);
                    let kind = b.ins().band_imm_s(h, 0xFF);
                    let pair = b.ins().icmp_imm_s(IntCC::Equal, kind, Kind::Pair as i64);
                    let v = b.ins().select(pair, t, f);
                    b.ins().jump(done, &[BlockArg::Value(v)]);
                    b.switch_to_block(done);
                    let res = b.block_params(done)[0];
                    self.set(b, dst, res);
                }
            }
            Op::Jmp { t } | Op::Loop { t } => {
                if matches!(op, Op::Loop { .. }) {
                    self.tick(b, t as usize);
                }
                b.ins().jump(self.blocks[t as usize], &[]);
                return;
            }
            Op::Jf { c, t } | Op::Jt { c, t } => {
                if self.plan.fix[pc].contains(c) {
                    let dst = if matches!(op, Op::Jt { .. }) { t as usize } else { next };
                    b.ins().jump(self.blocks[dst], &[]);
                    return;
                }
                let x = self.get(b, c);
                let is_false = b.ins().icmp_imm_s(IntCC::Equal, x, Value::FALSE.bits() as i64);
                if matches!(op, Op::Jf { .. }) {
                    b.ins().brif(is_false, self.blocks[t as usize], &[], self.blocks[next], &[]);
                } else {
                    b.ins().brif(is_false, self.blocks[next], &[], self.blocks[t as usize], &[]);
                }
                return;
            }
            Op::JNNull { a, t } => {
                if self.plan.fix[pc].contains(a) {
                    b.ins().jump(self.blocks[t as usize], &[]);
                    return;
                }
                let x = self.get(b, a);
                let is_nil = b.ins().icmp_imm_s(IntCC::Equal, x, Value::NIL.bits() as i64);
                b.ins().brif(is_nil, self.blocks[next], &[], self.blocks[t as usize], &[]);
                return;
            }
            Op::JNPair { a, t } => {
                if self.plan.fix[pc].contains(a) {
                    b.ins().jump(self.blocks[t as usize], &[]);
                    return;
                }
                let x = self.get(b, a);
                let ptr = Gen::is_ptr(b, x);
                let chk = b.create_block();
                b.ins().brif(ptr, chk, &[], self.blocks[t as usize], &[]);
                b.switch_to_block(chk);
                let p = self.ptr(b, x);
                let h = b.ins().load(I64, flags(), p, 0);
                let kind = b.ins().band_imm_s(h, 0xFF);
                let pair = b.ins().icmp_imm_s(IntCC::Equal, kind, Kind::Pair as i64);
                b.ins().brif(pair, self.blocks[next], &[], self.blocks[t as usize], &[]);
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
                let v = if self.plan.ret_fix { self.get(b, r) } else { self.val(b, pc, r) };
                let fuel = b.use_var(self.fuel_var);
                b.ins().return_(&[v, fuel]);
                return;
            }
            Op::CallG { base, n, g } | Op::TailCallG { base, n, g } => {
                let tail = matches!(op, Op::TailCallG { .. });
                let k = self.job.closure_globals[&g];
                // Global identity was verified once on entry (`enter`); the
                // kernel performs no mutations or foreign calls, so it cannot
                // change until the kernel exits.
                if !tail {
                    let fail = self.exit(b, pc, EXIT);
                    Gen::guard(b, self.left, fail);
                }
                self.tick(b, pc);
                let args: Vec<_> = (0..n)
                    .map(|i| if self.plan.fix_params.contains(i) { self.get(b, base + 1 + i) } else { self.val(b, pc, base + 1 + i) })
                    .collect();
                if tail {
                    let zero = b.ins().iconst(I64, 0);
                    for (i, &var) in self.vars.iter().enumerate() {
                        b.def_var(var, args.get(i).copied().unwrap_or(zero));
                    }
                    b.ins().jump(self.blocks[0], &[]);
                    return;
                }
                let bp = b.ins().iadd_imm_s(self.bp, base as i64 + 1);
                let next_left = b.ins().iadd_imm_s(self.left, -1);
                let hi = b.ins().ishl_imm_s(next_left, 32);
                let fuel = b.use_var(self.fuel_var);
                let packed = b.ins().bor(hi, fuel);
                let mut call_args: Vec<_> = [self.ctx, bp].into_iter().chain(args).collect();
                if self.plan.pass_cage {
                    call_args.push(self.cage.unwrap());
                }
                if self.plan.pass_vm {
                    call_args.push(self.vm.unwrap());
                }
                call_args.push(packed);
                let call = b.ins().call(self.own, &call_args);
                let (value, status) = (b.inst_results(call)[0], b.inst_results(call)[1]);
                let bailed = b.ins().icmp_imm_s(IntCC::SignedLessThan, status, 0);
                let done = b.create_block();
                let bail = b.create_block();
                b.set_cold_block(bail);
                b.ins().brif(bailed, bail, &[], done, &[]);
                b.switch_to_block(bail);
                let code = b.ins().iconst(I64, self.job.code as i64);
                let ret = b.ins().iconst(I64, next as i64);
                let r = self.helper(b, self.unwind, jit_fast_unwind as *const (), &[self.ctx, self.bp, code, ret]);
                self.pointer_or_return(b, r, EXIT, pc);
                for i in 0..base as usize {
                    let v = if self.plan.flow.live_out[pc].contains(i as Reg) { self.val(b, pc, i as Reg) } else { b.ins().iconst(I64, 0) };
                    b.ins().store(flags(), v, r, (i * 8) as i32);
                }
                let addr = b.ins().iconst(I64, k.closure.unwrap() as i64);
                let f = b.ins().load(I64, flags(), addr, 0);
                b.ins().store(flags(), f, r, (base as i32) * 8);
                let zero = b.ins().iconst(I64, 0);
                b.ins().return_(&[zero, status]);
                b.switch_to_block(done);
                b.def_var(self.fuel_var, status);
                if self.plan.ret_fix {
                    self.set_fix(b, pc, base, value);
                } else {
                    self.set(b, base, value);
                }
            }
            _ => unreachable!("not admitted: {op:?}"),
        }
        b.ins().jump(self.blocks[next], &[]);
    }
}

/// Try the fast kernel only at pc 0, after ordinary frame publication.
/// Capacity/depth are budgets, not a new register-stack allocation policy.
pub(super) fn enter(module: &mut JITModule, b: &mut FunctionBuilder, g: &Gen, kernel: &Compiled, job: &Job) {
    let regular = b.create_block();
    let fast = b.create_block();
    let mut bad = b.ins().iconst(I64, 0);
    let mut args = Vec::new();
    for i in 0..job.nparams {
        let x = b.ins().load(I64, flags(), g.r, (i as i32) * 8);
        if kernel.fix_params.contains(i) {
            let t = b.ins().bxor_imm_s(x, TAG_INT << 48);
            bad = b.ins().bor(bad, t);
            args.push(Gen::shifted(b, x));
        } else {
            args.push(x);
        }
    }
    let mut bad = b.ins().ushr_imm_s(bad, 48);
    // Tail jumps do not write a new closure slot. Require one actual
    // closure throughout the kernel, not merely one code identity: even
    // an unused environment/closure can be observed through weak keys.
    // Also verify every self-recursive global binding once here; inside
    // the pure kernel no global can be mutated.
    let actual = b.ins().load(I64, flags(), g.r, -8);
    let globals = b.ins().load(I64, flags(), g.ctx, offset_of!(JitCtx, globals) as i32);
    for (&glob, k) in &job.closure_globals {
        if k.code == job.code
            && let Some(address) = k.closure
        {
            let address = b.ins().iconst(I64, address as i64);
            let expected = b.ins().load(I64, flags(), address, 0);
            let different = b.ins().bxor(actual, expected);
            bad = b.ins().bor(bad, different);
            let current = b.ins().load(I64, flags(), globals, (glob * 8) as i32);
            let changed = b.ins().bxor(current, expected);
            bad = b.ins().bor(bad, changed);
        }
    }
    let full = b.ins().icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, g.depth, MAX_DEPTH);
    let full = b.ins().uextend(I64, full);
    bad = b.ins().bor(bad, full);
    let fuel32 = b.ins().load(I32, flags(), g.fuel, 0);
    let empty_fuel = b.ins().icmp_imm_s(IntCC::Equal, fuel32, 0);
    let empty_fuel = b.ins().uextend(I64, empty_fuel);
    bad = b.ins().bor(bad, empty_fuel);
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
    let hi = b.ins().ishl_imm_s(left, 32);
    let fuel64 = b.ins().uextend(I64, fuel32);
    let packed = b.ins().bor(hi, fuel64);
    b.ins().store(flags(), g.bp, g.ctx, offset_of!(JitCtx, fast_root) as i32);
    let own = module.declare_func_in_func(kernel.id, b.func);
    let mut call_args: Vec<_> = [g.ctx, g.bp].into_iter().chain(args).collect();
    if kernel.pass_cage {
        call_args.push(g.cage);
    }
    if kernel.pass_vm {
        call_args.push(g.vm);
    }
    call_args.push(packed);
    let call = b.ins().call(own, &call_args);
    let (value, status) = (b.inst_results(call)[0], b.inst_results(call)[1]);
    let bailed = b.ins().icmp_imm_s(IntCC::SignedLessThan, status, 0);
    let done = b.create_block();
    let bail = b.create_block();
    b.set_cold_block(bail);
    b.ins().brif(bailed, bail, &[], done, &[]);
    b.switch_to_block(bail);
    let out = b.ins().band_imm_s(status, i64::MAX);
    b.ins().return_(&[out]);
    b.switch_to_block(done);
    let rem32 = b.ins().ireduce(I32, status);
    b.ins().store(flags(), rem32, g.fuel, 0);
    let value = if kernel.ret_fix { Gen::tag_shifted(b, value) } else { value };
    b.ins().store(flags(), value, g.r, -8);
    let returned = b.ins().iconst(I64, (RETURNED as i64) << 32);
    b.ins().return_(&[returned]);
    b.switch_to_block(regular);
}
