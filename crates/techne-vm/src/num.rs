//! Generic arithmetic: the slow paths behind the VM's fixnum fast paths.
//!
//! Integers are fixnums (48 bits) or heap bignums: sign-magnitude, the
//! magnitude's 64-bit limbs (least significant first) after the header and
//! the sign in a header flag. Every integer that fits a fixnum is one, so
//! values are normalised after each operation. Arithmetic stays in `i64`
//! and switches to `num-bigint` on overflow.
//!
//! Exact non-integers are ratios: a heap object holding the numerator and
//! the denominator as integer values, in lowest terms with a denominator
//! above 1, so that a ratio is never an integer and equal ratios have equal
//! parts. Inexact numbers are floats.
//!
//! Non-real numbers are complex: a heap object holding the real and the
//! imaginary part, any two real numbers, the imaginary part never an exact
//! zero (that number is real). Its parts may be exact (`1/2+3i`).

use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};

use crate::{
    builtins::type_error,
    heap::{Kind, NEGATIVE, field, header, is_kind, len_of, set_field},
    value::Value,
    vm::{Error, Vm},
};

#[derive(Clone)]
pub enum N {
    I(i64),
    /// Only integers outside the `i64` range.
    B(BigInt),
    /// Only non-integers. Boxed, as the complex parts are, to keep `N` small:
    /// it is moved around on the integer paths.
    R(Box<BigRational>),
    F(f64),
    /// Real and imaginary part, both real; the imaginary part is not an
    /// exact zero.
    C(Box<(N, N)>),
}

impl N {
    pub fn f(&self) -> f64 {
        match self {
            N::I(i) => *i as f64,
            N::B(b) => b.to_f64().unwrap_or(f64::NAN),
            N::R(r) => r.to_f64().unwrap_or(f64::NAN),
            N::F(f) => *f,
            N::C(_) => f64::NAN,
        }
    }
    /// An integer's value (truncating a ratio or float).
    fn big(&self) -> BigInt {
        match self {
            N::I(i) => BigInt::from(*i),
            N::B(b) => b.clone(),
            N::R(r) => r.to_integer(),
            N::F(f) => BigInt::from(*f as i64),
            N::C(_) => BigInt::zero(),
        }
    }
    /// An exact number's value.
    pub fn rat(&self) -> BigRational {
        match self {
            N::R(r) => (**r).clone(),
            n => BigRational::from_integer(n.big()),
        }
    }
    pub fn is_exact(&self) -> bool {
        match self {
            N::F(_) => false,
            N::C(c) => c.0.is_exact() && c.1.is_exact(),
            _ => true,
        }
    }
    pub fn is_real(&self) -> bool {
        !matches!(self, N::C(_))
    }
    fn is_exact_zero(&self) -> bool {
        matches!(self, N::I(0))
    }
    /// Real and imaginary part.
    pub fn parts(&self) -> (N, N) {
        match self {
            N::C(c) => (c.0.clone(), c.1.clone()),
            n => (n.clone(), N::I(0)),
        }
    }
}

/// The number `re + im i`: real when `im` is an exact zero.
pub fn complex(re: N, im: N) -> N {
    if im.is_exact_zero() { re } else { N::C(Box::new((re, im))) }
}

/// The exact number `r`: an integer when its denominator is 1.
pub fn from_rational(r: BigRational) -> N {
    if r.denom().is_one() {
        let n = r.to_integer();
        match n.to_i64() {
            Some(i) => N::I(i),
            None => N::B(n),
        }
    } else {
        N::R(Box::new(r))
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

/// An integer value (a fixnum or a bignum).
fn int_value(v: Value) -> BigInt {
    if v.is_int() { BigInt::from(v.as_int()) } else { heap_int(v).big() }
}

pub fn num(v: Value, who: &str) -> Result<N, Error> {
    if v.is_int() {
        Ok(N::I(v.as_int()))
    } else if v.is_float() {
        Ok(N::F(v.as_float()))
    } else if is_kind(v, Kind::BigInt) {
        Ok(heap_int(v))
    } else if is_kind(v, Kind::Ratio) {
        let p = v.as_ptr();
        Ok(N::R(Box::new(unsafe { BigRational::new_raw(int_value(field(p, 0)), int_value(field(p, 1))) })))
    } else if is_kind(v, Kind::Complex) {
        let p = v.as_ptr();
        Ok(N::C(Box::new(unsafe { (num(field(p, 0), who)?, num(field(p, 1), who)?) })))
    } else {
        Err(type_error(who, "number", v))
    }
}

/// A real number argument.
pub fn real(v: Value, who: &str) -> Result<N, Error> {
    let n = num(v, who)?;
    if n.is_real() { Ok(n) } else { Err(type_error(who, "real number", v)) }
}

pub fn is_number(v: Value) -> bool {
    v.is_int() || v.is_float() || is_kind(v, Kind::BigInt) || is_kind(v, Kind::Ratio) || is_kind(v, Kind::Complex)
}

/// Whether `v` is an exact number.
pub fn is_exact(v: Value) -> bool {
    v.is_int()
        || is_kind(v, Kind::BigInt)
        || is_kind(v, Kind::Ratio)
        || (is_kind(v, Kind::Complex) && num(v, "").is_ok_and(|n| n.is_exact()))
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
        N::R(_) | N::F(_) | N::C(_) => Err(type_error(who, "integer", v)),
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
        N::R(r) => alloc_two(vm, Kind::Ratio, from_integer(r.numer().clone()), from_integer(r.denom().clone())),
        N::F(f) => Value::float(f),
        N::C(c) => {
            let (re, im) = *c;
            alloc_two(vm, Kind::Complex, re, im)
        }
    }
}

fn from_integer(b: BigInt) -> N {
    match b.to_i64() {
        Some(i) => N::I(i),
        None => N::B(b),
    }
}

/// A heap object of `kind` with the numbers `a` and `b` as fields.
fn alloc_two(vm: &mut Vm, kind: Kind, a: N, b: N) -> Value {
    let a = from_n(vm, a);
    // Allocating the second may collect: keep the first rooted.
    vm.scratch.push(a);
    let b = from_n(vm, b);
    vm.scratch.push(b);
    let p = vm.alloc(3);
    let b = vm.scratch.pop().expect("pushed");
    let a = vm.scratch.pop().expect("pushed");
    unsafe {
        *p = header(kind, 2, 0);
        set_field(p, 0, a);
        set_field(p, 1, b);
    }
    Value::ptr(p)
}

/// The exact value of a float (for `exact`): an integer or a ratio.
pub fn exact_of_float(vm: &mut Vm, f: f64, who: &str) -> Result<Value, Error> {
    match BigRational::from_float(f) {
        Some(r) => Ok(from_n(vm, from_rational(r))),
        None => Err(Error::new(format!("{who}: no exact number for {f}"))),
    }
}

struct Ops {
    int: fn(i64, i64) -> Option<i64>,
    big: fn(BigInt, BigInt) -> BigInt,
    rat: fn(BigRational, BigRational) -> BigRational,
    float: fn(f64, f64) -> f64,
}

/// `x` op `y` for real `x` and `y`.
fn real_op(x: &N, y: &N, ops: &Ops) -> N {
    match (x, y) {
        (N::F(_), _) | (_, N::F(_)) => N::F((ops.float)(x.f(), y.f())),
        (N::I(i), N::I(j)) => match (ops.int)(*i, *j) {
            Some(r) => N::I(r),
            None => N::B((ops.big)(x.big(), y.big())),
        },
        (N::R(_), _) | (_, N::R(_)) => from_rational((ops.rat)(x.rat(), y.rat())),
        _ => N::B((ops.big)(x.big(), y.big())),
    }
}

const ADD: Ops = Ops { int: i64::checked_add, big: |x, y| x + y, rat: |x, y| x + y, float: |x, y| x + y };
const SUB: Ops = Ops { int: i64::checked_sub, big: |x, y| x - y, rat: |x, y| x - y, float: |x, y| x - y };
const MUL: Ops = Ops { int: i64::checked_mul, big: |x, y| x * y, rat: |x, y| x * y, float: |x, y| x * y };

pub fn n_add(x: &N, y: &N) -> N {
    if x.is_real() && y.is_real() {
        return real_op(x, y, &ADD);
    }
    let ((a, b), (c, d)) = (x.parts(), y.parts());
    complex(real_op(&a, &c, &ADD), real_op(&b, &d, &ADD))
}
pub fn n_sub(x: &N, y: &N) -> N {
    if x.is_real() && y.is_real() {
        return real_op(x, y, &SUB);
    }
    let ((a, b), (c, d)) = (x.parts(), y.parts());
    complex(real_op(&a, &c, &SUB), real_op(&b, &d, &SUB))
}
pub fn n_mul(x: &N, y: &N) -> N {
    if x.is_real() && y.is_real() {
        return real_op(x, y, &MUL);
    }
    let ((a, b), (c, d)) = (x.parts(), y.parts());
    let m = |p: &N, q: &N| real_op(p, q, &MUL);
    complex(real_op(&m(&a, &c), &m(&b, &d), &SUB), real_op(&m(&a, &d), &m(&b, &c), &ADD))
}

/// `x / y`: exact operands give an exact result.
pub fn n_div(x: &N, y: &N) -> Result<N, Error> {
    if !(x.is_real() && y.is_real()) {
        // (a + bi) / (c + di) = ((ac + bd) + (bc - ad)i) / (c² + d²)
        let ((a, b), (c, d)) = (x.parts(), y.parts());
        let m = |p: &N, q: &N| real_op(p, q, &MUL);
        let denom = real_op(&m(&c, &c), &m(&d, &d), &ADD);
        let re = n_div(&real_op(&m(&a, &c), &m(&b, &d), &ADD), &denom)?;
        let im = n_div(&real_op(&m(&b, &c), &m(&a, &d), &SUB), &denom)?;
        return Ok(complex(re, im));
    }
    if x.is_exact() && y.is_exact() {
        if let (N::I(i), N::I(j)) = (x, y)
            && *j != 0
            && i.checked_rem(*j) == Some(0)
        {
            return Ok(N::I(i / j));
        }
        let y = y.rat();
        if y.is_zero() {
            return Err(Error::new("/: division by zero"));
        }
        return Ok(from_rational(x.rat() / y));
    }
    Ok(N::F(x.f() / y.f()))
}

/// Fixnums and floats without building `N`s: the common cases of the
/// natives (`+` with more than two arguments, `+` passed as a procedure).
#[inline(always)]
fn fast(vm: &mut Vm, a: Value, b: Value, int: fn(i64, i64) -> Option<i64>, float: fn(f64, f64) -> f64) -> Option<Value> {
    if a.is_int() && b.is_int() {
        return int(a.as_int(), b.as_int()).map(|r| vm.make_int(r));
    }
    let f = |v: Value| {
        if v.is_float() {
            Some(v.as_float())
        } else if v.is_int() {
            Some(v.as_int() as f64)
        } else {
            None
        }
    };
    if a.is_float() || b.is_float() {
        return Some(Value::float(float(f(a)?, f(b)?)));
    }
    None
}

fn arith(vm: &mut Vm, a: Value, b: Value, who: &str, op: fn(&N, &N) -> N) -> Result<Value, Error> {
    let (x, y) = (num(a, who)?, num(b, who)?);
    Ok(from_n(vm, op(&x, &y)))
}

pub fn add(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    match fast(vm, a, b, i64::checked_add, |x, y| x + y) {
        Some(r) => Ok(r),
        None => arith(vm, a, b, "+", n_add),
    }
}
pub fn sub(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    match fast(vm, a, b, i64::checked_sub, |x, y| x - y) {
        Some(r) => Ok(r),
        None => arith(vm, a, b, "-", n_sub),
    }
}
pub fn mul(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    match fast(vm, a, b, i64::checked_mul, |x, y| x * y) {
        Some(r) => Ok(r),
        None => arith(vm, a, b, "*", n_mul),
    }
}

/// `/`: exact operands give an exact result, an integer or a ratio.
pub fn div(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    let exact = |x: i64, y: i64| if y != 0 && x.checked_rem(y) == Some(0) { x.checked_div(y) } else { None };
    if let Some(r) = fast(vm, a, b, exact, |x, y| x / y) {
        return Ok(r);
    }
    let (x, y) = (num(a, "/")?, num(b, "/")?);
    let r = n_div(&x, &y)?;
    Ok(from_n(vm, r))
}

/// The inexact number nearest `n`.
pub fn inexact(n: &N) -> N {
    match n {
        N::C(c) => N::C(Box::new((N::F(c.0.f()), N::F(c.1.f())))),
        n => N::F(n.f()),
    }
}

/// The exact number equal to `n`; an error for infinities and NaN.
pub fn exact(n: &N, who: &str) -> Result<N, Error> {
    let real = |n: &N| match n {
        N::F(f) => BigRational::from_float(*f).map(from_rational).ok_or_else(|| Error::new(format!("{who}: no exact number for {f}"))),
        n => Ok(n.clone()),
    };
    match n {
        N::C(c) => Ok(complex(real(&c.0)?, real(&c.1)?)),
        n => real(n),
    }
}

fn int_div(
    vm: &mut Vm,
    a: Value,
    b: Value,
    who: &str,
    small: fn(i64, i64) -> Option<i64>,
    big: fn(&BigInt, &BigInt) -> BigInt,
) -> Result<Value, Error> {
    if a.is_int()
        && b.is_int()
        && let Some(r) = small(a.as_int(), b.as_int())
    {
        return Ok(vm.make_int(r));
    }
    let (x, y) = (num(a, who)?, num(b, who)?);
    for (n, v) in [(&x, a), (&y, b)] {
        if matches!(n, N::R(_) | N::C(_)) || matches!(n, N::F(f) if f.fract() != 0.0) {
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
    let (x, y) = (real(a, who)?, real(b, who)?);
    Ok(compare_real(&x, &y))
}

fn compare_real(x: &N, y: &N) -> std::cmp::Ordering {
    match (x, y) {
        (N::I(i), N::I(j)) => i.cmp(j),
        (N::F(_), N::F(_)) => x.f().partial_cmp(&y.f()).unwrap_or(std::cmp::Ordering::Greater),
        (N::I(_) | N::B(_), N::I(_) | N::B(_)) => x.big().cmp(&y.big()),
        // Exact against inexact compares the exact values, so that `=`
        // stays transitive beyond 2^53.
        (N::F(f), _) => float_cmp_exact(*f, &y.rat()).map_or(std::cmp::Ordering::Greater, std::cmp::Ordering::reverse),
        (_, N::F(f)) => float_cmp_exact(*f, &x.rat()).unwrap_or(std::cmp::Ordering::Greater),
        _ => x.rat().cmp(&y.rat()),
    }
}

/// How the exact number `n` compares with the float `f`; `None` for NaN.
fn float_cmp_exact(f: f64, n: &BigRational) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering::*;
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        return Some(if f > 0.0 { Less } else { Greater });
    }
    Some(n.cmp(&BigRational::from_float(f).expect("finite")))
}

pub fn lt(a: Value, b: Value) -> Result<bool, Error> {
    if a.is_int() && b.is_int() {
        return Ok(a.as_int() < b.as_int());
    }
    Ok(compare(a, b, "<")?.is_lt())
}
pub fn le(a: Value, b: Value) -> Result<bool, Error> {
    if a.is_int() && b.is_int() {
        return Ok(a.as_int() <= b.as_int());
    }
    Ok(compare(a, b, "<=")?.is_le())
}
/// `=`: complex numbers are equal when both parts are.
pub fn num_eq(a: Value, b: Value) -> Result<bool, Error> {
    if a.is_int() && b.is_int() {
        return Ok(a == b);
    }
    let (x, y) = (num(a, "=")?, num(b, "=")?);
    if x.is_real() && y.is_real() {
        return Ok(compare_real(&x, &y).is_eq());
    }
    let ((p, q), (r, s)) = (x.parts(), y.parts());
    Ok(compare_real(&p, &r).is_eq() && compare_real(&q, &s).is_eq())
}

/// `expt` with an exact base and an exact integer exponent.
pub fn expt_exact(vm: &mut Vm, base: &N, exp: i64) -> Result<Value, Error> {
    if let N::I(i) = base
        && let Ok(e) = u32::try_from(exp)
        && let Some(r) = i.checked_pow(e)
    {
        return Ok(vm.make_int(r));
    }
    let magnitude = usize::try_from(exp.unsigned_abs()).map_err(|_| Error::new("expt: exponent too large"))?;
    if let N::C(_) = base {
        // Squaring and multiplying.
        let (mut acc, mut sq, mut k) = (N::I(1), base.clone(), magnitude);
        while k > 0 {
            if k & 1 == 1 {
                acc = n_mul(&acc, &sq);
            }
            sq = n_mul(&sq, &sq);
            k >>= 1;
        }
        let r = if exp < 0 { n_div(&N::I(1), &acc).map_err(|_| Error::new("expt: division by zero"))? } else { acc };
        return Ok(from_n(vm, r));
    }
    let r = num_traits::pow(base.rat(), magnitude);
    if exp < 0 && r.is_zero() {
        return Err(Error::new("expt: division by zero"));
    }
    Ok(from_n(vm, from_rational(if exp < 0 { r.recip() } else { r })))
}

/// The exact square root of the exact number `n` ≥ 0, if it has one.
pub fn exact_sqrt(n: &N) -> Option<N> {
    let root = |b: &BigInt| {
        let r = b.sqrt();
        (&r * &r == *b).then_some(r)
    };
    let r = n.rat();
    Some(from_rational(BigRational::new_raw(root(r.numer())?, root(r.denom())?)))
}

pub fn to_string_radix(n: &N, radix: u32) -> String {
    match n {
        N::I(i) if radix == 10 => i.to_string(),
        N::I(i) => BigInt::from(*i).to_str_radix(radix),
        N::B(b) => b.to_str_radix(radix),
        N::R(r) => format!("{}/{}", r.numer().to_str_radix(radix), r.denom().to_str_radix(radix)),
        N::F(f) => crate::reader::float_repr(*f),
        N::C(c) => {
            let re = if c.0.is_exact_zero() { String::new() } else { to_string_radix(&c.0, radix) };
            let im = match &c.1 {
                N::I(1) => "+".to_owned(),
                N::I(-1) => "-".to_owned(),
                im => {
                    let s = to_string_radix(im, radix);
                    if s.starts_with(['+', '-']) { s } else { format!("+{s}") }
                }
            };
            format!("{re}{im}i")
        }
    }
}

/// `floor`, `ceiling`, `truncate` or `round` (to even) of a ratio.
pub fn round_ratio(r: &BigRational, how: Rounding) -> BigInt {
    match how {
        Rounding::Floor => r.floor().to_integer(),
        Rounding::Ceiling => r.ceil().to_integer(),
        Rounding::Truncate => r.trunc().to_integer(),
        Rounding::Round => {
            let floor = r.floor().to_integer();
            let half = BigRational::new(BigInt::one(), BigInt::from(2));
            match (r - BigRational::from_integer(floor.clone())).cmp(&half) {
                std::cmp::Ordering::Less => floor,
                std::cmp::Ordering::Greater => floor + 1,
                std::cmp::Ordering::Equal if floor.is_even() => floor,
                std::cmp::Ordering::Equal => floor + 1,
            }
        }
    }
}

#[derive(Clone, Copy)]
pub enum Rounding {
    Floor,
    Ceiling,
    Truncate,
    Round,
}

/// The simplest rational in `[lo, hi]` (`lo` ≤ `hi`): the one with the
/// smallest denominator, by continued fractions (Stern-Brocot).
pub fn simplest(lo: &BigRational, hi: &BigRational) -> BigRational {
    if lo.is_positive() {
        let fl = lo.floor();
        if fl == *lo {
            fl
        } else if fl < hi.floor() {
            fl + BigRational::one()
        } else {
            fl.clone() + simplest(&(hi - &fl).recip(), &(lo - &fl).recip()).recip()
        }
    } else if hi.is_negative() {
        -simplest(&-hi, &-lo)
    } else {
        BigRational::zero()
    }
}

/// What number syntax (R7RS 7.1.1) denotes.
#[derive(Debug, PartialEq)]
pub enum Parsed {
    Number(N),
    /// Number syntax for a value Techne does not have (complex numbers,
    /// exact non-integers), with the reason.
    Unsupported(&'static str),
    /// Not a number: an identifier, for the reader.
    No,
}

impl PartialEq for N {
    fn eq(&self, other: &N) -> bool {
        match (self, other) {
            (N::I(a), N::I(b)) => a == b,
            (N::B(a), N::B(b)) => a == b,
            (N::R(a), N::R(b)) => a == b,
            (N::C(a), N::C(b)) => a.0 == b.0 && a.1 == b.1,
            (N::F(a), N::F(b)) => a.to_bits() == b.to_bits(),
            _ => false,
        }
    }
}

impl std::fmt::Debug for N {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            N::I(i) => write!(f, "{i}"),
            N::B(b) => write!(f, "{b}"),
            N::R(r) => write!(f, "{r}"),
            N::C(c) => write!(f, "{:?}+{:?}i", c.0, c.1),
            N::F(x) => write!(f, "{x:?}"),
        }
    }
}

/// A real number as written: an exact ratio, or an infinity or NaN.
enum Real {
    Ratio(BigInt, BigInt),
    Special(f64),
}

/// Parses number syntax: `#x`/`#b`/`#o`/`#d` and `#e`/`#i` prefixes,
/// integers, `n/d`, decimals with an exponent (`e`, and R5RS's `s`, `f`,
/// `d`, `l`), `+inf.0`, `-inf.0` and `+nan.0`, case-insensitively.
/// Integers and `n/d` are exact, decimals inexact, unless a prefix says
/// otherwise.
pub fn parse(s: &str, default_radix: u32) -> Parsed {
    let (mut radix, mut exact, mut rest) = (None, None, s);
    while let Some(p) = rest.strip_prefix('#') {
        let c = p.chars().next().map(|c| c.to_ascii_lowercase());
        match c {
            Some(c @ ('x' | 'b' | 'o' | 'd')) if radix.is_none() => {
                radix = Some(match c {
                    'x' => 16,
                    'b' => 2,
                    'o' => 8,
                    _ => 10,
                })
            }
            Some(c @ ('e' | 'i')) if exact.is_none() => exact = Some(c == 'e'),
            _ => return Parsed::No,
        }
        rest = &p[1..];
    }
    let prefixed = radix.is_some() || exact.is_some();
    let radix = radix.unwrap_or(default_radix);
    let parsed = match real_number(rest, radix, exact) {
        Some(r) => r,
        None => match complex_number(rest, radix, exact) {
            Some(c) => c,
            None if prefixed => Err("bad number syntax"),
            None => return Parsed::No,
        },
    };
    match parsed {
        Ok(n) => Parsed::Number(n),
        Err(why) => Parsed::Unsupported(why),
    }
}

/// A real number's syntax, with the exactness a prefix asks for.
fn real_number(s: &str, radix: u32, exact: Option<bool>) -> Option<Result<N, &'static str>> {
    let (real, decimal) = real_syntax(s, radix)?;
    Some(Ok(match (real, exact) {
        (Real::Special(_), Some(true)) => return Some(Err("no exact infinity or NaN")),
        (Real::Special(f), _) => N::F(f),
        (Real::Ratio(n, d), Some(false)) => N::F(ratio_to_f64(n, d, decimal.then_some(s))),
        (Real::Ratio(n, d), None) if decimal => N::F(ratio_to_f64(n, d, Some(s))),
        (Real::Ratio(n, d), _) => from_rational(BigRational::new(n, d)),
    }))
}

/// Rectangular (`a+bi`, `+i`, `-2.5i`) or polar (`m@a`) complex syntax.
fn complex_number(s: &str, radix: u32, exact: Option<bool>) -> Option<Result<N, &'static str>> {
    let part = |t: &str| real_number(t, radix, exact);
    if let Some((m, a)) = s.split_once('@') {
        let (m, a) = (part(m)?, part(a)?);
        return Some(m.and_then(|m| a.map(|a| polar(&m, &a))));
    }
    let body = s.strip_suffix(['i', 'I'])?;
    // The imaginary part: a sign alone (±1), or a signed real.
    let imaginary = |t: &str| match t {
        "+" => Some(Ok(N::I(1))),
        "-" => Some(Ok(N::I(-1))),
        t if t.starts_with(['+', '-']) => part(t),
        _ => None,
    };
    let (re, im) = match imaginary(body) {
        Some(im) => (Ok(N::I(0)), im),
        None => body
            .char_indices()
            .filter(|&(i, c)| i > 0 && (c == '+' || c == '-'))
            .find_map(|(i, _)| Some((part(&body[..i])?, imaginary(&body[i..])?)))?,
    };
    let im = im.map(|im| if exact == Some(false) { N::F(im.f()) } else { im });
    let re = re.map(|re| if exact == Some(false) { N::F(re.f()) } else { re });
    Some(re.and_then(|re| im.map(|im| complex(re, im))))
}

/// The complex number with magnitude `m` and angle `a`.
pub fn polar(m: &N, a: &N) -> N {
    if a.is_exact() && a.f() == 0.0 {
        return m.clone();
    }
    let (m, a) = (m.f(), a.f());
    complex(N::F(m * a.cos()), N::F(m * a.sin()))
}

/// The nearest float: from the decimal text when there is one (correctly
/// rounded by Rust's parser), else from the ratio. A negative zero is
/// written `-0.0`, whose ratio is just 0.
pub fn ratio_to_f64(n: BigInt, d: BigInt, text: Option<&str>) -> f64 {
    if let Some(t) = text {
        let t: String = t.chars().map(|c| if "sSfFdDlL".contains(c) { 'e' } else { c }).collect();
        if let Ok(f) = t.parse::<f64>() {
            return f;
        }
    }
    BigRational::new(n, d).to_f64().unwrap_or(f64::NAN)
}

/// A signed real: the value, and whether it was written as a decimal.
fn real_syntax(s: &str, radix: u32) -> Option<(Real, bool)> {
    let (negative, body) = match s.as_bytes().first() {
        Some(b'+') => (false, &s[1..]),
        Some(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    if s.len() > body.len() {
        match body.to_ascii_lowercase().as_str() {
            "inf.0" => return Some((Real::Special(if negative { f64::NEG_INFINITY } else { f64::INFINITY }), false)),
            "nan.0" => return Some((Real::Special(f64::NAN), false)),
            _ => {}
        }
    }
    let (real, decimal) = ureal(body, radix)?;
    Some((
        match real {
            Real::Ratio(n, d) => Real::Ratio(if negative { -n } else { n }, d),
            Real::Special(f) => Real::Special(if negative { -f } else { f }),
        },
        decimal,
    ))
}

/// An unsigned real, and whether it was written as a decimal.
fn ureal(s: &str, radix: u32) -> Option<(Real, bool)> {
    let digits = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_digit(radix));
    let int = |t: &str| BigInt::parse_bytes(t.as_bytes(), radix);
    if let Some((n, d)) = s.split_once('/') {
        if !digits(n) || !digits(d) {
            return None;
        }
        let (n, d) = (int(n)?, int(d)?);
        return Some((
            if d.is_zero() { Real::Special(if n.is_zero() { f64::NAN } else { f64::INFINITY }) } else { Real::Ratio(n, d) },
            false,
        ));
    }
    if digits(s) {
        return Some((Real::Ratio(int(s)?, BigInt::from(1)), false));
    }
    if radix != 10 {
        return None;
    }
    // A decimal: digits, a point, digits, an exponent.
    let (mantissa, exponent) = match s.find(|c: char| "eEsSfFdDlL".contains(c)) {
        Some(i) => {
            let e = &s[i + 1..];
            let unsigned = e.strip_prefix(['+', '-']).unwrap_or(e);
            if !digits(unsigned) {
                return None;
            }
            // Digits only: parsing fails only when the exponent overflows,
            // and such a decimal is zero or infinite anyway.
            (&s[..i], e.parse::<i64>().unwrap_or(if e.starts_with('-') { i64::MIN } else { i64::MAX }))
        }
        None => (s, 0),
    };
    let (whole, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let plain = |t: &str| t.chars().all(|c| c.is_ascii_digit());
    if whole.len() + frac.len() == 0 || !plain(whole) || !plain(frac) {
        return None;
    }
    let n: BigInt = format!("{whole}{frac}").parse().ok()?;
    let scale = exponent.saturating_sub(frac.len() as i64);
    // Bound the exact arithmetic: beyond this a decimal is only a float,
    // zero or infinite.
    if scale.unsigned_abs() > 10_000 {
        let f = if n.is_zero() || scale < 0 { 0.0 } else { f64::INFINITY };
        return Some((Real::Special(f), true));
    }
    let ten = |k: i64| num_traits::pow(BigInt::from(10), k as usize);
    let ratio = if scale >= 0 { Real::Ratio(n * ten(scale), BigInt::from(1)) } else { Real::Ratio(n, ten(-scale)) };
    Some((ratio, true))
}

pub fn big_parity_even(n: &N) -> bool {
    match n {
        N::I(i) => i % 2 == 0,
        N::B(b) => b.is_even(),
        N::R(_) | N::C(_) => false,
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
        N::R(r) => from_n(vm, N::R(Box::new(r.abs()))),
        N::F(f) => Value::float(f.abs()),
        N::C(c) => from_n(vm, N::C(c)),
    }
}
