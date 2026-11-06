//! Bounded string-native cases. No collection, pointer retention or string
//! mutation: all unsupported representations/ranges use the original native.
use super::*;
use crate::heap::{ASCII, INDIRECT};

const MAX_BYTES: i64 = 64;

pub(super) fn enabled(name: &str) -> bool {
    std::env::var(name).as_deref() != Ok("0")
}

impl Gen {
    fn flat_string_len(&self, b: &mut FunctionBuilder, x: ir::Value, ascii: bool, slow: Block) -> ir::Value {
        let mask = INDIRECT | if ascii { ASCII } else { 0 };
        let expected = Kind::String as i64 | if ascii { ASCII as i64 } else { 0 };
        let (_, h) = self.check_bits(b, x, 0xFF | mask as i64, expected, slow);
        Self::header_len(b, h)
    }

    pub(super) fn substring(&self, b: &mut FunctionBuilder, args: &[ir::Value], slow: Block) -> ir::Value {
        let x = args[0];
        let len = self.flat_string_len(b, x, true, slow);
        let (a, z) = (args[1], args[2]);
        let (ai, zi) = (Self::is_int(b, a), Self::is_int(b, z));
        let ints = b.ins().band(ai, zi);
        Self::guard(b, ints, slow);
        let (a, z) = (Self::untag(b, a), Self::untag(b, z));
        // Unsigned checks reject negative indices as well as reversed ranges.
        let ordered = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, a, z);
        let bounded = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, z, len);
        let valid = b.ins().band(ordered, bounded);
        Self::guard(b, valid, slow);
        let len = b.ins().isub(z, a);
        let small = b.ins().icmp_imm_s(IntCC::UnsignedLessThanOrEqual, len, MAX_BYTES);
        Self::guard(b, small, slow);
        // Header plus ceil(len/8) payload words, at most nine words. The
        // nursery is already admitted; pressure/stress disables inline alloc.
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
        let at = b.declare_var(I64);
        b.def_var(at, zero);
        let test = b.create_block();
        let body = b.create_block();
        let done = b.create_block();
        b.ins().jump(test, &[]);
        b.switch_to_block(test);
        let i = b.use_var(at);
        let more = b.ins().icmp(IntCC::UnsignedLessThan, i, len);
        b.ins().brif(more, body, &[], done, &[]);
        b.switch_to_block(body);
        let off = b.ins().iadd(a, i);
        let off = b.ins().iadd_imm_s(off, 8);
        let src = self.wrap(b, x, off);
        let byte = b.ins().uload8(I64, flags(), src, 0);
        let dst = b.ins().iadd(top, i);
        b.ins().istore8(flags(), byte, dst, 8);
        let next = b.ins().iadd_imm_s(i, 1);
        b.def_var(at, next);
        b.ins().jump(test, &[]);
        b.switch_to_block(done);
        if std::env::var_os("TECHNE_JIT_LOG").is_some() {
            eprintln!("jit-substring: code={:x}", self.code_ptr);
        }
        Self::tag_ptr(b, top)
    }

    pub(super) fn string_eq(&self, b: &mut FunctionBuilder, args: &[ir::Value], slow: Block) -> ir::Value {
        let (a, z) = (args[0], args[1]);
        let al = self.flat_string_len(b, a, false, slow);
        let zl = self.flat_string_len(b, z, false, slow);
        let yes = b.create_block();
        let no = b.create_block();
        let start = b.create_block();
        let test = b.create_block();
        let body = b.create_block();
        let advance = b.create_block();
        let done = b.create_block();
        b.append_block_param(done, I64);
        let same_len = b.ins().icmp(IntCC::Equal, al, zl);
        b.ins().brif(same_len, start, &[], no, &[]);
        b.switch_to_block(start);
        let small = b.ins().icmp_imm_s(IntCC::UnsignedLessThanOrEqual, al, MAX_BYTES);
        Self::guard(b, small, slow);
        let at = b.declare_var(I64);
        let zero = b.ins().iconst(I64, 0);
        b.def_var(at, zero);
        b.ins().jump(test, &[]);
        b.switch_to_block(test);
        let i = b.use_var(at);
        let more = b.ins().icmp(IntCC::UnsignedLessThan, i, al);
        b.ins().brif(more, body, &[], yes, &[]);
        b.switch_to_block(body);
        let off = b.ins().iadd_imm_s(i, 8);
        let ap = self.wrap(b, a, off);
        let zp = self.wrap(b, z, off);
        let ac = b.ins().uload8(I64, flags(), ap, 0);
        let zc = b.ins().uload8(I64, flags(), zp, 0);
        let eq = b.ins().icmp(IntCC::Equal, ac, zc);
        b.ins().brif(eq, advance, &[], no, &[]);
        b.switch_to_block(advance);
        let next = b.ins().iadd_imm_s(i, 1);
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
