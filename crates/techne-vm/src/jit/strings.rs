//! Bounded string-native cases. No collection, pointer retention or string
//! mutation: all unsupported representations/ranges use the original native.
use super::*;
use crate::heap::{ASCII, INDIRECT};

const MAX_BYTES: i64 = 256;

fn unaligned_flags() -> MemFlagsData {
    let mut f = MemFlagsData::new();
    f.set_notrap();
    f
}

impl Gen {
    fn flat_string(&self, b: &mut FunctionBuilder, x: ir::Value, ascii: bool, slow: Block) -> (ir::Value, ir::Value) {
        let mask = INDIRECT | if ascii { ASCII } else { 0 };
        let expected = Kind::String as i64 | if ascii { ASCII as i64 } else { 0 };
        let (p, h) = self.check_bits(b, x, 0xFF | mask as i64, expected, slow);
        (p, Self::header_len(b, h))
    }

    /// Guard that accessing `bytes_end` payload bytes (after the 8-byte header)
    /// from object `x` stays strictly within the cage (`bytes_end <= MAX_BYTES + 8`).
    fn check_cage_bytes(b: &mut FunctionBuilder, x: ir::Value, bytes_end: ir::Value, slow: Block) {
        let off = b.ins().band_imm_s(x, crate::alloc::CAGE_MASK as i64);
        let end = b.ins().iadd(off, bytes_end);
        let end = b.ins().iadd_imm_s(end, 8);
        let ok = b.ins().icmp_imm_s(IntCC::UnsignedLessThanOrEqual, end, crate::alloc::CAGE_MASK as i64);
        Self::guard(b, ok, slow);
    }

    pub(super) fn substring(&self, b: &mut FunctionBuilder, pc: usize, base: Reg, args: &[ir::Value], slow: Block) -> ir::Value {
        let x = args[0];
        let (xp, slen) = self.flat_string(b, x, true, slow);
        let a = if self.fix[pc].contains(base + 2) {
            let s = b.use_var(self.shifts[(base + 2) as usize]);
            b.ins().sshr_imm_s(s, 16)
        } else {
            let v = args[1];
            let ai = Self::is_int(b, v);
            Self::guard(b, ai, slow);
            Self::untag(b, v)
        };
        let z = if self.fix[pc].contains(base + 3) {
            let s = b.use_var(self.shifts[(base + 3) as usize]);
            b.ins().sshr_imm_s(s, 16)
        } else {
            let v = args[2];
            let zi = Self::is_int(b, v);
            Self::guard(b, zi, slow);
            Self::untag(b, v)
        };
        // Unsigned checks reject negative indices as well as reversed ranges.
        let ordered = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, a, z);
        let bounded = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, z, slen);
        let valid = b.ins().band(ordered, bounded);
        Self::guard(b, valid, slow);
        let len = b.ins().isub(z, a);
        let small = b.ins().icmp_imm_s(IntCC::UnsignedLessThanOrEqual, len, MAX_BYTES);
        Self::guard(b, small, slow);
        // Validate source range once against the cage instead of wrapping per byte.
        Self::check_cage_bytes(b, x, z, slow);
        // Header plus ceil(len/8) payload words.
        let bytes = b.ins().iadd_imm_s(len, 15);
        let bytes = b.ins().band_imm_s(bytes, -8);
        let top = self.alloc_bytes(b, bytes, slow);
        // Zero the last payload word (the header for an empty string), so
        // padding is deterministic, just as init_ascii_with requires.
        let last_off = b.ins().iadd_imm_s(bytes, -8);
        let last = b.ins().iadd(top, last_off);
        let zero = b.ins().iconst(I64, 0);
        b.ins().store(flags(), zero, last, 0);
        let h = b.ins().ishl_imm_s(len, 16);
        let h = b.ins().bor_imm_s(h, Kind::String as i64 | ASCII as i64);
        b.ins().store(flags(), h, top, 0);

        let src_base = b.ins().iadd(xp, a);
        let src_base = b.ins().iadd_imm_s(src_base, 8);
        let dst_base = b.ins().iadd_imm_s(top, 8);
        let word_bytes = b.ins().band_imm_s(len, -8);
        let at = b.declare_var(I64);
        b.def_var(at, zero);
        // Copy 8 bytes at a time while at least 8 bytes remain.
        let wtest = b.create_block();
        let wbody = b.create_block();
        let btest = b.create_block();
        let bbody = b.create_block();
        let done = b.create_block();
        b.ins().jump(wtest, &[]);
        b.switch_to_block(wtest);
        let i = b.use_var(at);
        let more_w = b.ins().icmp(IntCC::UnsignedLessThan, i, word_bytes);
        b.ins().brif(more_w, wbody, &[], btest, &[]);
        b.switch_to_block(wbody);
        let sp = b.ins().iadd(src_base, i);
        let dp = b.ins().iadd(dst_base, i);
        let w = b.ins().load(I64, unaligned_flags(), sp, 0);
        b.ins().store(flags(), w, dp, 0);
        let next_w = b.ins().iadd_imm_s(i, 8);
        b.def_var(at, next_w);
        b.ins().jump(wtest, &[]);
        // Copy remaining 0..7 bytes.
        b.switch_to_block(btest);
        let i = b.use_var(at);
        let more_b = b.ins().icmp(IntCC::UnsignedLessThan, i, len);
        b.ins().brif(more_b, bbody, &[], done, &[]);
        b.switch_to_block(bbody);
        let sp = b.ins().iadd(src_base, i);
        let dp = b.ins().iadd(dst_base, i);
        let byte = b.ins().uload8(I64, unaligned_flags(), sp, 0);
        b.ins().istore8(unaligned_flags(), byte, dp, 0);
        let next_b = b.ins().iadd_imm_s(i, 1);
        b.def_var(at, next_b);
        b.ins().jump(btest, &[]);
        b.switch_to_block(done);
        if std::env::var_os("TECHNE_JIT_LOG").is_some() {
            eprintln!("jit-substring: code={:x}", self.code_ptr);
        }
        Self::tag_ptr(b, top)
    }

    pub(super) fn string_eq(&self, b: &mut FunctionBuilder, args: &[ir::Value], slow: Block) -> ir::Value {
        let (a, z) = (args[0], args[1]);
        let (ap, al) = self.flat_string(b, a, false, slow);
        let (zp, zl) = self.flat_string(b, z, false, slow);
        let yes = b.create_block();
        let no = b.create_block();
        let check_len = b.create_block();
        let start = b.create_block();
        let test = b.create_block();
        let body = b.create_block();
        let advance = b.create_block();
        let done = b.create_block();
        b.append_block_param(done, I64);
        // Pointer identity fast path (both operands already validated as strings).
        let same_ptr = b.ins().icmp(IntCC::Equal, a, z);
        b.ins().brif(same_ptr, yes, &[], check_len, &[]);
        b.switch_to_block(check_len);
        let same_len = b.ins().icmp(IntCC::Equal, al, zl);
        b.ins().brif(same_len, start, &[], no, &[]);
        b.switch_to_block(start);
        let small = b.ins().icmp_imm_s(IntCC::UnsignedLessThanOrEqual, al, MAX_BYTES);
        Self::guard(b, small, slow);
        // Strings zero-pad their final payload word, so compare 8 bytes per step.
        let word_bytes = b.ins().iadd_imm_s(al, 7);
        let word_bytes = b.ins().band_imm_s(word_bytes, -8);
        Self::check_cage_bytes(b, a, word_bytes, slow);
        Self::check_cage_bytes(b, z, word_bytes, slow);
        let one_word = b.ins().icmp_imm_s(IntCC::Equal, word_bytes, 8);
        let fast_one = b.create_block();
        let loop_init = b.create_block();
        b.ins().brif(one_word, fast_one, &[], loop_init, &[]);
        b.switch_to_block(fast_one);
        let aw = b.ins().load(I64, flags(), ap, 8);
        let zw = b.ins().load(I64, flags(), zp, 8);
        let eq = b.ins().icmp(IntCC::Equal, aw, zw);
        b.ins().brif(eq, yes, &[], no, &[]);
        b.switch_to_block(loop_init);
        let at = b.declare_var(I64);
        let zero = b.ins().iconst(I64, 0);
        b.def_var(at, zero);
        b.ins().jump(test, &[]);
        b.switch_to_block(test);
        let i = b.use_var(at);
        let more = b.ins().icmp(IntCC::UnsignedLessThan, i, word_bytes);
        b.ins().brif(more, body, &[], yes, &[]);
        b.switch_to_block(body);
        let ap_i = b.ins().iadd(ap, i);
        let zp_i = b.ins().iadd(zp, i);
        let aw = b.ins().load(I64, flags(), ap_i, 8);
        let zw = b.ins().load(I64, flags(), zp_i, 8);
        let eq = b.ins().icmp(IntCC::Equal, aw, zw);
        b.ins().brif(eq, advance, &[], no, &[]);
        b.switch_to_block(advance);
        let next = b.ins().iadd_imm_s(i, 8);
        b.def_var(at, next);
        b.ins().jump(test, &[]);
        b.switch_to_block(yes);
        let value = Self::imm(b, Value::TRUE);
        b.ins().jump(done, &[BlockArg::Value(value)]);
        b.switch_to_block(no);
        let value = Self::imm(b, Value::FALSE);
        b.ins().jump(done, &[BlockArg::Value(value)]);
        b.switch_to_block(done);
        if std::env::var_os("TECHNE_JIT_LOG").is_some() {
            eprintln!("jit-string-eq: code={:x}", self.code_ptr);
        }
        b.block_params(done)[0]
    }
}
