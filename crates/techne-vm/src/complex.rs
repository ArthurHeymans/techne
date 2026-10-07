//! Complex numbers (R7RS `(scheme complex)`) and the procedures whose
//! result can be complex: `sqrt`, `exp`, `log`, the trigonometric
//! functions and `expt`. On real arguments in their real domain these give
//! real results as before; outside it (`(sqrt -4)`, `(log -1)`,
//! `(asin 2)`) complex ones, where they used to give NaN.

use num_complex::Complex64;

use crate::{
    num::{self, N},
    value::Value,
    vm::{Error, Native, NativeFn, NativeImpl, Vm},
};

type R = Result<Value, Error>;

fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

fn number(vm: &Vm, args: usize, i: usize, who: &str) -> Result<N, Error> {
    num::num(arg(vm, args, i), who)
}

fn real(vm: &Vm, args: usize, i: usize, who: &str) -> Result<N, Error> {
    num::real(arg(vm, args, i), who)
}

fn c64(n: &N) -> Complex64 {
    let (re, im) = n.parts();
    Complex64::new(re.f(), im.f())
}

fn from_c64(c: Complex64) -> N {
    num::complex(N::F(c.re), N::F(c.im))
}

/// `f` of a real number in the real domain `domain`, else `c` of it as a
/// complex number. The result is inexact.
fn real_or_complex(vm: &mut Vm, args: usize, who: &str, domain: fn(f64) -> bool, f: fn(f64) -> f64, c: fn(Complex64) -> Complex64) -> R {
    let n = number(vm, args, 0, who)?;
    let r = match n {
        N::C(_) => from_c64(c(c64(&n))),
        n if domain(n.f()) || n.f().is_nan() => N::F(f(n.f())),
        n => from_c64(c(c64(&n))),
    };
    Ok(num::from_n(vm, r))
}

fn sqrt(vm: &mut Vm, args: usize, _: usize) -> R {
    let n = number(vm, args, 0, "sqrt")?;
    let r = match &n {
        // An inexact zero imaginary part is taken as zero, whatever its
        // sign: the root of -1.0-0.0i is +i, as for -1.0.
        N::C(c) if c.1.f() == 0.0 && c.0.f() < 0.0 => num::complex(N::F(0.0), N::F((-c.0.f()).sqrt())),
        N::C(_) => from_c64(c64(&n).sqrt()),
        n if n.is_exact() => {
            let negative = n.f() < 0.0;
            let magnitude = if negative { num::n_sub(&N::I(0), n) } else { n.clone() };
            let root = num::exact_sqrt(&magnitude).unwrap_or_else(|| N::F(magnitude.f().sqrt()));
            match (negative, root.is_exact()) {
                (false, _) => root,
                (true, true) => num::complex(N::I(0), root),
                (true, false) => num::complex(N::F(0.0), root),
            }
        }
        n if n.f() < 0.0 => num::complex(N::F(0.0), N::F((-n.f()).sqrt())),
        n => N::F(n.f().sqrt()),
    };
    Ok(num::from_n(vm, r))
}

fn log(vm: &mut Vm, args: usize, n: usize) -> R {
    let ln = |x: &N| match x {
        N::C(_) => from_c64(c64(x).ln()),
        x if x.f() >= 0.0 || x.f().is_nan() => N::F(x.f().ln()),
        x => from_c64(c64(x).ln()),
    };
    let r = ln(&number(vm, args, 0, "log")?);
    let r = if n == 2 {
        let base = ln(&number(vm, args, 1, "log")?);
        if r.is_real() && base.is_real() { N::F(r.f() / base.f()) } else { from_c64(c64(&r) / c64(&base)) }
    } else {
        r
    };
    Ok(num::from_n(vm, r))
}

fn atan(vm: &mut Vm, args: usize, n: usize) -> R {
    if n == 2 {
        let (y, x) = (real(vm, args, 0, "atan")?, real(vm, args, 1, "atan")?);
        return Ok(Value::float(y.f().atan2(x.f())));
    }
    real_or_complex(vm, args, "atan", |_| true, f64::atan, Complex64::atan)
}

fn expt(vm: &mut Vm, args: usize, _: usize) -> R {
    let (b, e) = (number(vm, args, 0, "expt")?, number(vm, args, 1, "expt")?);
    if let N::I(k) = e
        && b.is_exact()
    {
        return num::expt_exact(vm, &b, k);
    }
    let zero = |n: &N| n.is_real() && n.f() == 0.0;
    let r = if zero(&b) {
        // 0 to a power: 1 for an exponent of 0, else 0.
        match (zero(&e), b.is_exact() && e.is_exact()) {
            (true, true) => N::I(1),
            (true, false) => N::F(1.0),
            (false, true) => N::I(0),
            (false, false) => N::F(0.0),
        }
    } else if b.is_real() && e.is_real() && (b.f() >= 0.0 || e.f().fract() == 0.0 || b.f().is_nan() || e.f().is_nan()) {
        N::F(b.f().powf(e.f()))
    } else {
        from_c64(c64(&b).powc(c64(&e)))
    };
    Ok(num::from_n(vm, r))
}

fn magnitude(vm: &mut Vm, args: usize, _: usize) -> R {
    let n = number(vm, args, 0, "magnitude")?;
    let (re, im) = n.parts();
    if n.is_real() {
        return Ok(num::abs(vm, re));
    }
    if n.is_exact() {
        let squares = num::n_add(&num::n_mul(&re, &re), &num::n_mul(&im, &im));
        if let Some(root) = num::exact_sqrt(&squares) {
            return Ok(num::from_n(vm, root));
        }
    }
    Ok(Value::float(re.f().hypot(im.f())))
}

fn angle(vm: &mut Vm, args: usize, _: usize) -> R {
    let n = number(vm, args, 0, "angle")?;
    let (re, im) = n.parts();
    if n.is_real() && n.is_exact() && re.f() >= 0.0 {
        return Ok(Value::int_unchecked(0));
    }
    Ok(Value::float(im.f().atan2(re.f())))
}

fn inexact(vm: &mut Vm, args: usize, who: &str) -> R {
    let v = arg(vm, args, 0);
    if v.is_float() {
        return Ok(v);
    }
    if v.is_int() {
        return Ok(Value::float(v.as_int() as f64));
    }
    let n = num::num(v, who)?;
    Ok(num::from_n(vm, num::inexact(&n)))
}

fn parts_test(vm: &Vm, args: usize, who: &str, test: fn(f64) -> bool, all: bool) -> R {
    let (re, im) = number(vm, args, 0, who)?.parts();
    let (a, b) = (test(re.f()), test(im.f()));
    Ok(Value::bool(if all { a && b } else { a || b }))
}

macro_rules! natives {
    ($vm:expr; $($name:literal $min:literal $max:tt => $f:expr;)*) => {
        $( {
            let f: NativeFn = $f;
            $vm.define_native(Native { name: $name.into(), f: NativeImpl::Plain(f), min: $min, max: natives!(@max $max) });
        } )*
    };
    (@max _) => { None };
    (@max $m:literal) => { Some($m) };
}

pub fn install(vm: &mut Vm) {
    natives! { vm;
        "complex?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::is_number(arg(vm, a, 0))));
        "real?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::num(arg(vm, a, 0), "").is_ok_and(|n| n.is_real())));
        "rational?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::num(arg(vm, a, 0), "").is_ok_and(|n| n.is_real() && n.f().is_finite())));
        "finite?" 1 1 => |vm: &mut Vm, a, _| parts_test(vm, a, "finite?", f64::is_finite, true);
        "infinite?" 1 1 => |vm: &mut Vm, a, _| parts_test(vm, a, "infinite?", f64::is_infinite, false);
        "nan?" 1 1 => |vm: &mut Vm, a, _| parts_test(vm, a, "nan?", f64::is_nan, false);
        "inexact?" 1 1 => |vm: &mut Vm, a, _| { let n = number(vm, a, 0, "inexact?")?; Ok(Value::bool(!n.is_exact())) };
        "inexact" 1 1 => |vm: &mut Vm, a, _| inexact(vm, a, "inexact");
        "exact->inexact" 1 1 => |vm: &mut Vm, a, _| inexact(vm, a, "exact->inexact");
        "make-rectangular" 2 2 => |vm: &mut Vm, a, _| {
            let (re, im) = (real(vm, a, 0, "make-rectangular")?, real(vm, a, 1, "make-rectangular")?);
            Ok(num::from_n(vm, num::complex(re, im))) };
        "make-polar" 2 2 => |vm: &mut Vm, a, _| {
            let (m, angle) = (real(vm, a, 0, "make-polar")?, real(vm, a, 1, "make-polar")?);
            Ok(num::from_n(vm, num::polar(&m, &angle))) };
        "real-part" 1 1 => |vm: &mut Vm, a, _| { let (re, _) = number(vm, a, 0, "real-part")?.parts(); Ok(num::from_n(vm, re)) };
        "imag-part" 1 1 => |vm: &mut Vm, a, _| { let (_, im) = number(vm, a, 0, "imag-part")?.parts(); Ok(num::from_n(vm, im)) };
        "magnitude" 1 1 => magnitude;
        "angle" 1 1 => angle;
        "sqrt" 1 1 => sqrt;
        "expt" 2 2 => expt;
        "exp" 1 1 => |vm: &mut Vm, a, _| real_or_complex(vm, a, "exp", |_| true, f64::exp, Complex64::exp);
        "log" 1 2 => log;
        "sin" 1 1 => |vm: &mut Vm, a, _| real_or_complex(vm, a, "sin", |_| true, f64::sin, Complex64::sin);
        "cos" 1 1 => |vm: &mut Vm, a, _| real_or_complex(vm, a, "cos", |_| true, f64::cos, Complex64::cos);
        "tan" 1 1 => |vm: &mut Vm, a, _| real_or_complex(vm, a, "tan", |_| true, f64::tan, Complex64::tan);
        "asin" 1 1 => |vm: &mut Vm, a, _| real_or_complex(vm, a, "asin", |x| (-1.0..=1.0).contains(&x), f64::asin, Complex64::asin);
        "acos" 1 1 => |vm: &mut Vm, a, _| real_or_complex(vm, a, "acos", |x| (-1.0..=1.0).contains(&x), f64::acos, Complex64::acos);
        "atan" 1 2 => atan;
    }
}
