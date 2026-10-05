//! Generic arithmetic: the slow paths behind the VM's fixnum fast paths.
//! Integers beyond 48 bits are boxed i64; beyond i64 is not supported yet.

use crate::{
    builtins::type_error,
    heap::{Kind, is_kind},
    value::Value,
    vm::{Error, Vm},
};

#[derive(Clone, Copy)]
pub enum N {
    I(i64),
    F(f64),
}

impl N {
    fn f(self) -> f64 {
        match self {
            N::I(i) => i as f64,
            N::F(f) => f,
        }
    }
}

pub fn num(v: Value, who: &str) -> Result<N, Error> {
    if v.is_int() {
        Ok(N::I(v.as_int()))
    } else if v.is_float() {
        Ok(N::F(v.as_float()))
    } else if is_kind(v, Kind::BigInt) {
        Ok(N::I(unsafe { *v.as_ptr().add(1) } as i64))
    } else {
        Err(type_error(who, "number", v))
    }
}

pub fn is_number(v: Value) -> bool {
    v.is_int() || v.is_float() || is_kind(v, Kind::BigInt)
}

pub fn integer(v: Value, who: &str) -> Result<i64, Error> {
    match num(v, who)? {
        N::I(i) => Ok(i),
        N::F(f) if f.fract() == 0.0 => Ok(f as i64),
        N::F(_) => Err(type_error(who, "integer", v)),
    }
}

fn overflow(who: &str) -> Error {
    Error::new(format!("{who}: integer overflow (bignums are not implemented yet)"))
}

pub fn from_n(vm: &mut Vm, n: N) -> Value {
    match n {
        N::I(i) => vm.make_int(i),
        N::F(f) => Value::float(f),
    }
}

fn arith(
    vm: &mut Vm,
    a: Value,
    b: Value,
    who: &str,
    int: fn(i64, i64) -> Option<i64>,
    float: fn(f64, f64) -> f64,
) -> Result<Value, Error> {
    let r = match (num(a, who)?, num(b, who)?) {
        (N::I(x), N::I(y)) => N::I(int(x, y).ok_or_else(|| overflow(who))?),
        (x, y) => N::F(float(x.f(), y.f())),
    };
    Ok(from_n(vm, r))
}

pub fn add(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    arith(vm, a, b, "+", i64::checked_add, |x, y| x + y)
}
pub fn sub(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    arith(vm, a, b, "-", i64::checked_sub, |x, y| x - y)
}
pub fn mul(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    arith(vm, a, b, "*", i64::checked_mul, |x, y| x * y)
}

pub fn div(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    let r = match (num(a, "/")?, num(b, "/")?) {
        (N::I(_), N::I(0)) => return Err(Error::new("/: division by zero")),
        (N::I(x), N::I(y)) if x % y == 0 => N::I(x / y),
        (x, y) => N::F(x.f() / y.f()),
    };
    Ok(from_n(vm, r))
}

fn int_div(vm: &mut Vm, a: Value, b: Value, who: &str, op: fn(i64, i64) -> i64) -> Result<Value, Error> {
    let (x, y) = (integer(a, who)?, integer(b, who)?);
    if y == 0 {
        return Err(Error::new(format!("{who}: division by zero")));
    }
    let float = matches!(num(a, who)?, N::F(_)) || matches!(num(b, who)?, N::F(_));
    let r = op(x, y);
    Ok(if float { Value::float(r as f64) } else { vm.make_int(r) })
}

pub fn quotient(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    int_div(vm, a, b, "quotient", |x, y| x / y)
}
pub fn remainder(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    int_div(vm, a, b, "remainder", |x, y| x % y)
}
pub fn modulo(vm: &mut Vm, a: Value, b: Value) -> Result<Value, Error> {
    int_div(vm, a, b, "modulo", modulo_i64)
}

/// Scheme `modulo`: the result has the sign of the divisor.
#[inline(always)]
pub fn modulo_i64(x: i64, y: i64) -> i64 {
    let m = x.rem_euclid(y);
    if y < 0 && m != 0 { m + y } else { m }
}

fn compare(a: Value, b: Value, who: &str) -> Result<std::cmp::Ordering, Error> {
    Ok(match (num(a, who)?, num(b, who)?) {
        (N::I(x), N::I(y)) => x.cmp(&y),
        (x, y) => x.f().partial_cmp(&y.f()).unwrap_or(std::cmp::Ordering::Greater),
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
