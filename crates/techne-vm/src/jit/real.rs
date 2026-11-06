//! Lower explicit scalar conversion, never implicit loss of exactness.
use super::*;

impl Gen {
    pub(super) fn float_div(&self, b: &mut FunctionBuilder, a: ir::Value, z: ir::Value, slow: Block) -> ir::Value {
        let (af, zf) = (Self::is_float(b, a), Self::is_float(b, z));
        let inexact = b.ins().bor(af, zf);
        // Exact division must still produce an exact integer/ratio, and
        // exact zero division must raise rather than invent an infinity.
        Self::guard(b, inexact, slow);
        let (a, z) = self.floats(b, a, z, slow);
        let value = b.ins().fdiv(a, z);
        if std::env::var_os("TECHNE_JIT_LOG").is_some() {
            eprintln!("jit-float-div: code={:x}", self.code_ptr);
        }
        Self::from_f(b, value)
    }

    pub(super) fn inexact(&self, b: &mut FunctionBuilder, x: ir::Value, slow: Block) -> ir::Value {
        let number = Self::is_number(b, x);
        Self::guard(b, number, slow);
        let int = Self::is_int(b, x);
        let n = Self::untag(b, x);
        let f = b.ins().fcvt_from_sint(F64, n);
        let converted = b.ins().bitcast(I64, MemFlagsData::new(), f);
        // An already-inexact argument is returned verbatim, including -0.0
        // and NaN bits. Only explicit fixnum conversion reaches the cast.
        let value = b.ins().select(int, converted, x);
        if std::env::var_os("TECHNE_JIT_LOG").is_some() {
            eprintln!("jit-inexact: code={:x}", self.code_ptr);
        }
        value
    }
}
