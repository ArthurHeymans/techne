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
    /// Only non-integers.
    R(BigRational),
    F(f64),
}

impl N {
    pub fn f(&self) -> f64 {
        match self {
            N::I(i) => *i as f64,
            N::B(b) => b.to_f64().unwrap_or(f64::NAN),
            N::R(r) => r.to_f64().unwrap_or(f64::NAN),
            N::F(f) => *f,
        }
    }
    /// An integer's value (truncating a ratio or float).
    fn big(&self) -> BigInt {
        match self {
            N::I(i) => BigInt::from(*i),
            N::B(b) => b.clone(),
            N::R(r) => r.to_integer(),
            N::F(f) => BigInt::from(*f as i64),
        }
    }
    /// An exact number's value.
    pub fn rat(&self) -> BigRational {
        match self {
            N::R(r) => r.clone(),
            n => BigRational::from_integer(n.big()),
        }
    }
    pub fn is_exact(&self) -> bool {
        !matches!(self, N::F(_))
    }
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
        N::R(r)
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
        Ok(N::R(unsafe { BigRational::new_raw(int_value(field(p, 0)), int_value(field(p, 1))) }))
    } else {
        Err(type_error(who, "number", v))
    }
}

pub fn is_number(v: Value) -> bool {
    v.is_int() || v.is_float() || is_kind(v, Kind::BigInt) || is_kind(v, Kind::Ratio)
}

/// Whether `v` is an exact number.
pub fn is_exact(v: Value) -> bool {
    v.is_int() || is_kind(v, Kind::BigInt) || is_kind(v, Kind::Ratio)
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
        N::R(_) | N::F(_) => Err(type_error(who, "integer", v)),
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
        N::R(r) => alloc_ratio(vm, &r),
        N::F(f) => Value::float(f),
    }
}

/// A heap ratio for `r`, a non-integer in lowest terms.
fn alloc_ratio(vm: &mut Vm, r: &BigRational) -> Value {
    let n = make_integer(vm, r.numer());
    // Allocating the denominator may collect: keep the numerator rooted.
    vm.scratch.push(n);
    let d = make_integer(vm, r.denom());
    vm.scratch.push(d);
    let p = vm.alloc(3);
    let d = vm.scratch.pop().expect("pushed");
    let n = vm.scratch.pop().expect("pushed");
    unsafe {
        *p = header(Kind::Ratio, 2, 0);
        set_field(p, 0, n);
        set_field(p, 1, d);
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
    who: &'static str,
    int: fn(i64, i64) -> Option<i64>,
    big: fn(BigInt, BigInt) -> BigInt,
    rat: fn(BigRational, BigRational) -> BigRational,
    float: fn(f64, f64) -> f64,
}

fn arith(vm: &mut Vm, a: Value, b: Value, ops: Ops) -> Result<Value, Error> {
    let (x, y) = (num(a, ops.who)?, num(b, ops.who)?);
    let r = match (&x, &y) {
        (N::F(_), _) | (_, N::F(_)) => N::F((ops.float)(x.f(), y.f())),
        (N::I(i), N::I(j)) => match (ops.int)(*i, *j) {
            Some(r) => N::I(r),
            None => N::B((ops.big)(x.big(), y.big())),
        },
        (N::R(_), _) | (_, N::R(_)) => from_rational((ops.rat)(x.rat(), y.rat())),
        _ => N::B((ops.big)(x.big(), y.big())),
    };
    Ok(from_n(vm, r))
}

pub fn add(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    arith(vm, a, b, Ops { who: "+", int: i64::checked_add, big: |x, y| x + y, rat: |x, y| x + y, float: |x, y| x + y })
}
pub fn sub(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    arith(vm, a, b, Ops { who: "-", int: i64::checked_sub, big: |x, y| x - y, rat: |x, y| x - y, float: |x, y| x - y })
}
pub fn mul(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    arith(vm, a, b, Ops { who: "*", int: i64::checked_mul, big: |x, y| x * y, rat: |x, y| x * y, float: |x, y| x * y })
}

/// `/`: exact operands give an exact result, an integer or a ratio.
pub fn div(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    let (x, y) = (num(a, "/")?, num(b, "/")?);
    if x.is_exact() && y.is_exact() {
        if let (N::I(i), N::I(j)) = (&x, &y)
            && *j != 0
            && i.checked_rem(*j) == Some(0)
        {
            return Ok(vm.make_int(i / j));
        }
        let y = y.rat();
        if y.is_zero() {
            return Err(Error::new("/: division by zero"));
        }
        return Ok(from_n(vm, from_rational(x.rat() / y)));
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
        if matches!(n, N::R(_)) || matches!(n, N::F(f) if f.fract() != 0.0) {
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
        (N::F(_), N::F(_)) => x.f().partial_cmp(&y.f()).unwrap_or(std::cmp::Ordering::Greater),
        (N::I(_) | N::B(_), N::I(_) | N::B(_)) => x.big().cmp(&y.big()),
        // Exact against inexact compares the exact values, so that `=`
        // stays transitive beyond 2^53.
        (N::F(f), _) => float_cmp_exact(*f, &y.rat()).map_or(std::cmp::Ordering::Greater, std::cmp::Ordering::reverse),
        (_, N::F(f)) => float_cmp_exact(*f, &x.rat()).unwrap_or(std::cmp::Ordering::Greater),
        _ => x.rat().cmp(&y.rat()),
    })
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
    Ok(compare(a, b, "<")?.is_lt())
}
pub fn le(a: Value, b: Value) -> Result<bool, Error> {
    Ok(compare(a, b, "<=")?.is_le())
}
pub fn num_eq(a: Value, b: Value) -> Result<bool, Error> {
    Ok(compare(a, b, "=")?.is_eq())
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
        N::F(f) => f.to_string(),
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
    let Some((real, decimal)) = real(rest, radix) else {
        return if complex(rest, radix) {
            Parsed::Unsupported("complex numbers are not supported")
        } else if prefixed {
            Parsed::Unsupported("bad number syntax")
        } else {
            Parsed::No
        };
    };
    Parsed::Number(match (real, exact) {
        (Real::Special(_), Some(true)) => return Parsed::Unsupported("no exact infinity or NaN"),
        (Real::Special(f), _) => N::F(f),
        (Real::Ratio(n, d), Some(false)) => N::F(ratio_to_f64(n, d, decimal.then_some(rest))),
        (Real::Ratio(n, d), None) if decimal => N::F(ratio_to_f64(n, d, Some(rest))),
        (Real::Ratio(n, d), _) => from_rational(BigRational::new(n, d)),
    })
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
fn real(s: &str, radix: u32) -> Option<(Real, bool)> {
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
            (&s[..i], e.parse::<i64>().ok()?)
        }
        None => (s, 0),
    };
    let (whole, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let plain = |t: &str| t.chars().all(|c| c.is_ascii_digit());
    if whole.len() + frac.len() == 0 || !plain(whole) || !plain(frac) {
        return None;
    }
    let n: BigInt = format!("{whole}{frac}").parse().ok()?;
    let scale = exponent.checked_sub(frac.len() as i64)?;
    // Bound the exact arithmetic: beyond this a decimal is only a float,
    // zero or infinite.
    if scale.abs() > 10_000 {
        let f = if n.is_zero() || scale < 0 { 0.0 } else { f64::INFINITY };
        return Some((Real::Special(f), true));
    }
    let ten = |k: i64| num_traits::pow(BigInt::from(10), k as usize);
    let ratio = if scale >= 0 { Real::Ratio(n * ten(scale), BigInt::from(1)) } else { Real::Ratio(n, ten(-scale)) };
    Some((ratio, true))
}

/// Whether `s` is rectangular (`a+bi`, `+i`) or polar (`a@b`) complex syntax.
fn complex(s: &str, radix: u32) -> bool {
    if let Some((m, a)) = s.split_once('@') {
        return real(m, radix).is_some() && real(a, radix).is_some();
    }
    let Some(body) = s.strip_suffix(['i', 'I']) else { return false };
    let imaginary = |t: &str| matches!(t, "+" | "-") || (t.starts_with(['+', '-']) && real(t, radix).is_some());
    imaginary(body)
        || body
            .char_indices()
            .filter(|&(i, c)| i > 0 && (c == '+' || c == '-'))
            .any(|(i, _)| real(&body[..i], radix).is_some() && imaginary(&body[i..]))
}

pub fn big_parity_even(n: &N) -> bool {
    match n {
        N::I(i) => i % 2 == 0,
        N::B(b) => b.is_even(),
        N::R(_) => false,
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
        N::R(r) => alloc_ratio(vm, &r.abs()),
        N::F(f) => Value::float(f.abs()),
    }
}
