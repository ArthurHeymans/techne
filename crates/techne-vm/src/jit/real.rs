//! Lower explicit scalar conversion, never implicit loss of exactness.
use super::*;

impl Gen {
    pub(super) fn float_div(&self, b: &mut FunctionBuilder, pc: usize, base: Reg, a: ir::Value, z: ir::Value, slow: Block) -> ir::Value {
        let kf = self.floats_known(b, pc, base + 1, base + 2);
        let value = if kf.0.is_some() || kf.1.is_some() {
            let xn = self.num(b, pc, base + 1);
            let yn = self.num(b, pc, base + 2);
            let fa = Self::as_f64(b, xn, kf.0, slow);
            let fb = Self::as_f64(b, yn, kf.1, slow);
            b.ins().fdiv(fa, fb)
        } else {
            let (af, zf) = (Self::is_float(b, a), Self::is_float(b, z));
            let inexact = b.ins().bor(af, zf);
            // Exact division must still produce an exact integer/ratio, and
            // exact zero division must raise rather than invent an infinity.
            Self::guard(b, inexact, slow);
            let (a, z) = self.floats(b, a, z, slow);
            b.ins().fdiv(a, z)
        };
        if std::env::var_os("TECHNE_JIT_LOG").is_some() {
            eprintln!("jit-float-div: code={:x}", self.code_ptr);
        }
        if self.flo.get(pc + 1).is_some_and(|f| f.contains(base)) {
            b.def_var(self.fvals[base as usize], value);
        }
        Self::from_f(b, value)
    }

    pub(super) fn inexact(&self, b: &mut FunctionBuilder, pc: usize, base: Reg, x: ir::Value, slow: Block) -> ir::Value {
        let want_f = self.flo.get(pc + 1).is_some_and(|f| f.contains(base));
        if self.flo[pc].contains(base + 1) {
            if want_f {
                let f = b.use_var(self.fvals[(base + 1) as usize]);
                b.def_var(self.fvals[base as usize], f);
            }
            return x;
        }
        if self.fix[pc].contains(base + 1) {
            let s = b.use_var(self.shifts[(base + 1) as usize]);
            let n = b.ins().sshr_imm_s(s, 16);
            let f = b.ins().fcvt_from_sint(F64, n);
            if want_f {
                b.def_var(self.fvals[base as usize], f);
            }
            return b.ins().bitcast(I64, MemFlagsData::new(), f);
        }
        let number = Self::is_number(b, x);
        Self::guard(b, number, slow);
        let int = Self::is_int(b, x);
        let n = Self::untag(b, x);
        let f = b.ins().fcvt_from_sint(F64, n);
        let converted = b.ins().bitcast(I64, MemFlagsData::new(), f);
        // An already-inexact argument is returned verbatim, including -0.0
        // and NaN bits. Only explicit fixnum conversion reaches the cast.
        let value = b.ins().select(int, converted, x);
        if want_f {
            let fv = b.ins().bitcast(F64, MemFlagsData::new(), value);
            b.def_var(self.fvals[base as usize], fv);
        }
        if std::env::var_os("TECHNE_JIT_LOG").is_some() {
            eprintln!("jit-inexact: code={:x}", self.code_ptr);
        }
        value
    }
}
