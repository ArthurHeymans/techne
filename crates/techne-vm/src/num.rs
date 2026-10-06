//! Generic arithmetic: the slow paths behind the VM's fixnum fast paths.
//!
//! Integers are fixnums (48 bits) or heap bignums: sign-magnitude, the
//! magnitude's 64-bit limbs (least significant first) after the header and
//! the sign in a header flag. Every integer that fits a fixnum is one, so
//! values are normalised after each operation. Arithmetic stays in `i64`
//! and switches to `num-bigint` on overflow.

use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};

use crate::{
    builtins::type_error,
    heap::{Kind, NEGATIVE, header, is_kind, len_of},
    value::Value,
    vm::{Error, Vm},
};

#[derive(Clone)]
pub enum N {
    I(i64),
    /// Only integers outside the `i64` range.
    B(BigInt),
    F(f64),
}

impl N {
    pub fn f(&self) -> f64 {
        match self {
            N::I(i) => *i as f64,
            N::B(b) => b.to_f64().unwrap_or(f64::NAN),
            N::F(f) => *f,
        }
    }
    fn big(&self) -> BigInt {
        match self {
            N::I(i) => BigInt::from(*i),
            N::B(b) => b.clone(),
            N::F(f) => BigInt::from(*f as i64),
        }
    }
    pub fn is_exact(&self) -> bool {
        !matches!(self, N::F(_))
    }
}

/// The integer a heap bignum holds.
pub fn heap_int(v: Value) -> N {
    unsafe {
        let p = v.as_ptr();
        let limbs = std::slice::from_raw_parts(p.add(1), len_of(p));
        let negative = *p & NEGATIVE != 0;
        if let [m] = limbs {
            let i = if negative { 0i64.checked_sub_unsigned(*m) } else { i64::try_from(*m).ok() };
            if let Some(i) = i {
                return N::I(i);
            }
        }
        let sign = if negative { Sign::Minus } else { Sign::Plus };
        N::B(BigInt::from_slice_native(sign, limbs))
    }
}

trait FromLimbs {
    fn from_slice_native(sign: Sign, limbs: &[u64]) -> BigInt;
}
impl FromLimbs for BigInt {
    fn from_slice_native(sign: Sign, limbs: &[u64]) -> BigInt {
        let digits: Vec<u32> = limbs.iter().flat_map(|l| [*l as u32, (*l >> 32) as u32]).collect();
        BigInt::from_slice(sign, &digits)
    }
}

pub fn num(v: Value, who: &str) -> Result<N, Error> {
    if v.is_int() {
        Ok(N::I(v.as_int()))
    } else if v.is_float() {
        Ok(N::F(v.as_float()))
    } else if is_kind(v, Kind::BigInt) {
        Ok(heap_int(v))
    } else {
        Err(type_error(who, "number", v))
    }
}

pub fn is_number(v: Value) -> bool {
    v.is_int() || v.is_float() || is_kind(v, Kind::BigInt)
}

pub fn is_integer(v: Value) -> bool {
    v.is_int() || is_kind(v, Kind::BigInt) || (v.is_float() && v.as_float().fract() == 0.0)
}

/// An integer argument that fits `i64` (indices, counts, Rust arguments).
pub fn integer(v: Value, who: &str) -> Result<i64, Error> {
    match num(v, who)? {
        N::I(i) => Ok(i),
        N::F(f) if f.fract() == 0.0 && f.abs() < 9.2e18 => Ok(f as i64),
        N::B(_) => Err(Error::new(format!("{who}: integer too large"))),
        N::F(_) => Err(type_error(who, "integer", v)),
    }
}

/// A heap object for a bignum (any integer, including ones that would fit).
pub fn alloc_big(vm: &mut Vm, b: &BigInt) -> Value {
    let limbs = b.magnitude().to_u64_digits();
    let flags = if b.is_negative() { NEGATIVE } else { 0 };
    let p = vm.alloc(1 + limbs.len());
    unsafe {
        *p = header(Kind::BigInt, limbs.len(), flags);
        std::ptr::copy_nonoverlapping(limbs.as_ptr(), p.add(1), limbs.len());
    }
    Value::ptr(p)
}

/// The value of an integer: a fixnum when it fits.
pub fn make_integer(vm: &mut Vm, b: &BigInt) -> Value {
    match b.to_i64() {
        Some(i) => vm.make_int(i),
        None => alloc_big(vm, b),
    }
}

pub fn from_n(vm: &mut Vm, n: N) -> Value {
    match n {
        N::I(i) => vm.make_int(i),
        N::B(b) => make_integer(vm, &b),
        N::F(f) => Value::float(f),
    }
}

/// The integer of a float with no fraction (for `exact`).
pub fn exact_of_float(vm: &mut Vm, f: f64, who: &str) -> Result<Value, Error> {
    if !f.is_finite() || f.fract() != 0.0 {
        return Err(Error::new(format!("{who}: no exact integer for {f}")));
    }
    let b = <BigInt as num_traits::FromPrimitive>::from_f64(f).expect("finite");
    Ok(make_integer(vm, &b))
}

fn arith(
    vm: &mut Vm,
    a: Value,
    b: Value,
    who: &str,
    int: fn(i64, i64) -> Option<i64>,
    big: fn(BigInt, BigInt) -> BigInt,
    float: fn(f64, f64) -> f64,
) -> Result<Value, Error> {
    let (x, y) = (num(a, who)?, num(b, who)?);
    let r = match (&x, &y) {
        (N::F(_), _) | (_, N::F(_)) => N::F(float(x.f(), y.f())),
        (N::I(i), N::I(j)) => match int(*i, *j) {
            Some(r) => N::I(r),
            None => N::B(big(x.big(), y.big())),
        },
        _ => N::B(big(x.big(), y.big())),
    };
    Ok(from_n(vm, r))
}

pub fn add(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    arith(vm, a, b, "+", i64::checked_add, |x, y| x + y, |x, y| x + y)
}
pub fn sub(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    arith(vm, a, b, "-", i64::checked_sub, |x, y| x - y, |x, y| x - y)
}
pub fn mul(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    arith(vm, a, b, "*", i64::checked_mul, |x, y| x * y, |x, y| x * y)
}

pub fn div(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    let (x, y) = (num(a, "/")?, num(b, "/")?);
    if x.is_exact() && y.is_exact() {
        let (x, y) = (x.big(), y.big());
        if y.is_zero() {
            return Err(Error::new("/: division by zero"));
        }
        let (q, r) = x.div_rem(&y);
        if r.is_zero() {
            return Ok(make_integer(vm, &q));
        }
        // No rationals: an inexact result.
        return Ok(Value::float(x.to_f64().unwrap_or(f64::NAN) / y.to_f64().unwrap_or(f64::NAN)));
    }
    Ok(Value::float(x.f() / y.f()))
}

fn int_div(
    vm: &mut Vm,
    a: Value,
    b: Value,
    who: &str,
    small: fn(i64, i64) -> Option<i64>,
    big: fn(&BigInt, &BigInt) -> BigInt,
) -> Result<Value, Error> {
    let (x, y) = (num(a, who)?, num(b, who)?);
    for (n, v) in [(&x, a), (&y, b)] {
        if let N::F(f) = n
            && f.fract() != 0.0
        {
            return Err(type_error(who, "integer", v));
        }
    }
    let float = !x.is_exact() || !y.is_exact();
    let (x, y) = (if float { N::I(integer(a, who)?) } else { x }, if float { N::I(integer(b, who)?) } else { y });
    let zero = matches!(y, N::I(0));
    if zero {
        return Err(Error::new(format!("{who}: division by zero")));
    }
    let r = match (&x, &y) {
        (N::I(i), N::I(j)) => match small(*i, *j) {
            Some(r) => N::I(r),
            None => N::B(big(&x.big(), &y.big())),
        },
        _ => N::B(big(&x.big(), &y.big())),
    };
    Ok(if float { Value::float(r.f()) } else { from_n(vm, r) })
}

pub fn quotient(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    int_div(vm, a, b, "quotient", i64::checked_div, |x, y| x / y)
}
pub fn remainder(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    int_div(vm, a, b, "remainder", i64::checked_rem, |x, y| x % y)
}
pub fn modulo(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    int_div(vm, a, b, "modulo", |x, y| x.checked_rem(y).map(|_| modulo_i64(x, y)), |x, y| x.mod_floor(y))
}

/// Scheme `modulo`: the result has the sign of the divisor.
#[inline(always)]
pub fn modulo_i64(x: i64, y: i64) -> i64 {
    let m = x.rem_euclid(y);
    if y < 0 && m != 0 { m + y } else { m }
}

fn compare(a: Value, b: Value, who: &str) -> Result<std::cmp::Ordering, Error> {
    let (x, y) = (num(a, who)?, num(b, who)?);
    Ok(match (&x, &y) {
        (N::I(i), N::I(j)) => i.cmp(j),
        _ if x.is_exact() && y.is_exact() => x.big().cmp(&y.big()),
        _ => x.f().partial_cmp(&y.f()).unwrap_or(std::cmp::Ordering::Greater),
    })
}

pub fn lt(a: Value, b: Value) -> Result<bool, Error> {
    Ok(compare(a, b, "<")?.is_lt())
}
pub fn le(a: Value, b: Value) -> Result<bool, Error> {
    Ok(compare(a, b, "<=")?.is_le())
}
pub fn num_eq(a: Value, b: Value) -> Result<bool, Error> {
    Ok(compare(a, b, "=")?.is_eq())
}

/// `expt` with an exact integer base and non-negative exponent.
pub fn expt_int(vm: &mut Vm, base: &N, exp: u32) -> Value {
    if let N::I(i) = base
        && let Some(r) = i.checked_pow(exp)
    {
        return vm.make_int(r);
    }
    make_integer(vm, &num_traits::pow(base.big(), exp as usize))
}

/// Exact square root if `n` (an exact integer ≥ 0) is a perfect square.
pub fn exact_sqrt(n: &N) -> Option<BigInt> {
    let b = n.big();
    let r = b.sqrt();
    (&r * &r == b).then_some(r)
}

pub fn to_string_radix(n: &N, radix: u32) -> String {
    match n {
        N::I(i) if radix == 10 => i.to_string(),
        N::I(i) => BigInt::from(*i).to_str_radix(radix),
        N::B(b) => b.to_str_radix(radix),
        N::F(f) => f.to_string(),
    }
}

pub fn parse_integer(s: &str) -> Option<BigInt> {
    let digits = s.strip_prefix('+').unwrap_or(s);
    if digits.is_empty() || !digits.trim_start_matches('-').bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

pub fn big_parity_even(n: &N) -> bool {
    match n {
        N::I(i) => i % 2 == 0,
        N::B(b) => b.is_even(),
        N::F(f) => f % 2.0 == 0.0,
    }
}

pub fn abs(vm: &mut Vm, n: N) -> Value {
    match n {
        N::I(i) => match i.checked_abs() {
            Some(a) => vm.make_int(a),
            None => make_integer(vm, &BigInt::from(i).abs()),
        },
        N::B(b) => make_integer(vm, &b.abs()),
        N::F(f) => Value::float(f.abs()),
    }
}
