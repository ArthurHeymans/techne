//! Exact scalar integers at first-class `+` sites. Keep sign-magnitude
//! object layout/canonical fixnums identical to Vm::make_int_raw. No GC in
//! this path: unsupported values, overflow and allocation use the native.
use super::*;

pub(super) fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("TECHNE_JIT_WIDE_ADD").as_deref() != Ok("0"))
}

impl Gen {
    fn integer_operand(&self, b: &mut FunctionBuilder, x: ir::Value, slow: Block) -> ir::Value {
        let int = Self::is_int(b, x);
        let fix = b.create_block();
        let wide = b.create_block();
        let done = b.create_block();
        b.append_block_param(done, I64);
        b.ins().brif(int, fix, &[], wide, &[]);
        b.switch_to_block(fix);
        let n = Self::untag(b, x);
        b.ins().jump(done, &[BlockArg::Value(n)]);
        b.switch_to_block(wide);
        let (_, h) = self.check_bits(b, x, 0xFF, Kind::BigInt as i64, slow);
        let len = Self::header_len(b, h);
        let one = b.ins().icmp_imm_s(IntCC::Equal, len, 1);
        Self::guard(b, one, slow);
        // Mask this field address as well as the header address: heap
        // values must never escape their owning cage, even if malformed.
        let off = b.ins().iconst(I64, 8);
        let p = self.wrap(b, x, off);
        let magnitude = b.ins().load(I64, flags(), p, 0);
        let sign = b.ins().band_imm_s(h, crate::heap::NEGATIVE as i64);
        let negative = b.ins().icmp_imm_s(IntCC::NotEqual, sign, 0);
        let min = b.ins().iconst(I64, i64::MIN);
        let max = b.ins().iconst(I64, i64::MAX);
        let limit = b.ins().select(negative, min, max);
        let fits = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, magnitude, limit);
        Self::guard(b, fits, slow);
        let negated = b.ins().ineg(magnitude);
        let n = b.ins().select(negative, negated, magnitude);
        b.ins().jump(done, &[BlockArg::Value(n)]);
        b.switch_to_block(done);
        b.block_params(done)[0]
    }

    pub(super) fn integer_add(&self, b: &mut FunctionBuilder, a: ir::Value, c: ir::Value, slow: Block) -> ir::Value {
        if std::env::var_os("TECHNE_JIT_LOG").is_some() {
            eprintln!("jit-wide-add: code={:x}", self.code_ptr);
        }
        let raw = (a, c);
        let floating = b.create_block();
        let a = self.integer_operand(b, a, floating);
        let c = self.integer_operand(b, c, floating);
        let (n, overflow) = b.ins().sadd_overflow(a, c);
        let ok = b.ins().icmp_imm_s(IntCC::Equal, overflow, 0);
        Self::guard(b, ok, slow);
        let small = Self::fits48(b, n);
        let fix = b.create_block();
        let wide = b.create_block();
        let done = b.create_block();
        b.append_block_param(done, I64);
        b.ins().brif(small, fix, &[], wide, &[]);
        b.switch_to_block(fix);
        let shifted = Self::shifted(b, n);
        let value = Self::tag_shifted(b, shifted);
        b.ins().jump(done, &[BlockArg::Value(value)]);
        b.switch_to_block(wide);
        let p = self.alloc_words(b, 2, slow);
        let negative = b.ins().icmp_imm_s(IntCC::SignedLessThan, n, 0);
        let zero = b.ins().iconst(I64, 0);
        let sign = b.ins().iconst(I64, crate::heap::NEGATIVE as i64);
        let sign = b.ins().select(negative, sign, zero);
        let h = b.ins().bor_imm_s(sign, header(Kind::BigInt, 1, 0) as i64);
        let negated = b.ins().ineg(n);
        let magnitude = b.ins().select(negative, negated, n);
        b.ins().store(flags(), h, p, 0);
        b.ins().store(flags(), magnitude, p, 8);
        let value = Self::tag_ptr(b, p);
        b.ins().jump(done, &[BlockArg::Value(value)]);
        b.switch_to_block(floating);
        // Reuse the existing fixnum/float arithmetic contract rather than
        // add guards and then send ordinary floating sums through Rust.
        // Exact overflow/allocation never enters this branch: arbitrary
        // exact integers and all other numeric kinds still use the native.
        let (a, c) = self.floats(b, raw.0, raw.1, slow);
        // The first-class variadic builtin folds from exact zero, unlike
        // the binary bytecode: (+ -0.0 -0.0) through that procedure returns
        // +0.0. Preserve that initial addition, not just a + c.
        let zero = b.ins().f64const(0.0);
        let a = b.ins().fadd(zero, a);
        let sum = b.ins().fadd(a, c);
        let value = Self::from_f(b, sum);
        b.ins().jump(done, &[BlockArg::Value(value)]);
        b.switch_to_block(done);
        b.block_params(done)[0]
    }
}
