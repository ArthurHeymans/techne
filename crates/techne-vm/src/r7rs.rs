//! R7RS small procedures the rest of the runtime did not have: characters,
//! case folding, ranges over strings and vectors, transcendental functions,
//! integer division and roots, and clocks (PLAN.md, language step 0).
//! Strings are immutable, and string indices count characters, so taking a
//! range walks the string (runtime/TECHNE-VM.md, deviations from R7RS).

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use num_bigint::BigInt;

use crate::{
    builtins::type_error,
    heap::{Kind, field, is_kind, len_of, str_bytes},
    num::{self, N},
    value::Value,
    vm::{Error, Native, NativeFn, NativeImpl, SpecialObj, Vm},
};

type R = Result<Value, Error>;

fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

fn char_of(v: Value, who: &str) -> Result<char, Error> {
    if v.is_char() { Ok(v.as_char()) } else { Err(type_error(who, "char", v)) }
}

fn str_of(v: Value, who: &str) -> Result<String, Error> {
    if is_kind(v, Kind::String) {
        Ok(String::from_utf8_lossy(unsafe { str_bytes(v.as_ptr()) }).into_owned())
    } else {
        Err(type_error(who, "string", v))
    }
}

fn float_of(v: Value, who: &str) -> Result<f64, Error> {
    Ok(num::num(v, who)?.f())
}

/// Optional `[start [end]]` arguments at `from`, as a character or element
/// range of a sequence of `len`.
fn range(vm: &Vm, args: usize, n: usize, from: usize, len: usize, who: &str) -> Result<(usize, usize), Error> {
    let index = |i: usize, default: usize| -> Result<usize, Error> {
        if i >= n {
            return Ok(default);
        }
        let k = num::integer(arg(vm, args, i), who)?;
        usize::try_from(k).ok().filter(|&k| k <= len).ok_or_else(|| Error::new(format!("{who}: index {k} out of range")))
    };
    let (start, end) = (index(from, 0)?, index(from + 1, len)?);
    if start > end {
        return Err(Error::new(format!("{who}: start {start} is after end {end}")));
    }
    Ok((start, end))
}

/// Simple case folding of one character (R7RS char-foldcase).
fn fold(c: char) -> char {
    match c {
        'ſ' => 's',
        'ς' => 'σ',
        c => {
            let mut lower = c.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(l), None) => l,
                _ => c,
            }
        }
    }
}

/// Full case folding of a string (R7RS string-foldcase): ß becomes ss.
fn fold_string(s: &str) -> String {
    s.chars()
        .flat_map(|c| -> Vec<char> {
            match c {
                'ß' | 'ẞ' => vec!['s', 's'],
                'ſ' => vec!['s'],
                'ς' => vec!['σ'],
                c => c.to_lowercase().collect(),
            }
        })
        .collect()
}

/// The first digit of every block of ten decimal digits (Unicode general
/// category Nd).
const DIGIT_ZEROS: &[u32] = &[
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66, 0xDE6, 0xE50, 0xED0, 0xF20, 0x1040, 0x1090,
    0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90, 0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0,
    0xFF10, 0x104A0, 0x10D30, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0, 0x11650, 0x116C0, 0x11730, 0x118E0, 0x11950,
    0x11C50, 0x11D50, 0x11DA0, 0x11F50, 0x16A60, 0x16AC0, 0x16B50, 0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E4F0,
    0x1E950, 0x1FBF0,
];

pub fn digit_value(c: char) -> Option<u32> {
    let c = c as u32;
    DIGIT_ZEROS.iter().find(|&&z| z <= c && c < z + 10).map(|z| c - z)
}

/// Compare characters pairwise along all arguments.
fn char_cmp(vm: &mut Vm, args: usize, n: usize, who: &str, ci: bool, ok: fn(char, char) -> bool) -> R {
    let key = |c: char| if ci { fold(c) } else { c };
    let mut prev = key(char_of(arg(vm, args, 0), who)?);
    for i in 1..n {
        let c = key(char_of(arg(vm, args, i), who)?);
        if !ok(prev, c) {
            return Ok(Value::FALSE);
        }
        prev = c;
    }
    Ok(Value::TRUE)
}

fn two_values(vm: &mut Vm, a: Value, b: Value) -> R {
    let rtd = vm.special(SpecialObj::ValuesRtd);
    Ok(vm.make_record(rtd, &[a, b]))
}

/// The ratio of integers a finite float is, reduced: its numerator and
/// denominator as floats. Without exact rationals, `numerator` and
/// `denominator` of an inexact number answer from this.
fn float_ratio(f: f64) -> (f64, f64) {
    let (mut n, mut d) = (f, 1.0);
    while n.fract() != 0.0 && d < 2f64.powi(1074) {
        n *= 2.0;
        d *= 2.0;
    }
    (n, d)
}

fn start() -> Instant {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *START.get_or_init(Instant::now)
}

fn def(vm: &mut Vm, name: &str, min: usize, max: Option<usize>, f: NativeFn) {
    vm.define_native(Native { name: name.into(), f: NativeImpl::Plain(f), min, max });
}

pub fn install(vm: &mut Vm) {
    start();
    // Characters.
    def(vm, "char=?", 1, None, |vm, a, n| char_cmp(vm, a, n, "char=?", false, |x, y| x == y));
    def(vm, "char<?", 1, None, |vm, a, n| char_cmp(vm, a, n, "char<?", false, |x, y| x < y));
    def(vm, "char>?", 1, None, |vm, a, n| char_cmp(vm, a, n, "char>?", false, |x, y| x > y));
    def(vm, "char<=?", 1, None, |vm, a, n| char_cmp(vm, a, n, "char<=?", false, |x, y| x <= y));
    def(vm, "char>=?", 1, None, |vm, a, n| char_cmp(vm, a, n, "char>=?", false, |x, y| x >= y));
    def(vm, "char-ci=?", 1, None, |vm, a, n| char_cmp(vm, a, n, "char-ci=?", true, |x, y| x == y));
    def(vm, "char-ci<?", 1, None, |vm, a, n| char_cmp(vm, a, n, "char-ci<?", true, |x, y| x < y));
    def(vm, "char-ci>?", 1, None, |vm, a, n| char_cmp(vm, a, n, "char-ci>?", true, |x, y| x > y));
    def(vm, "char-ci<=?", 1, None, |vm, a, n| char_cmp(vm, a, n, "char-ci<=?", true, |x, y| x <= y));
    def(vm, "char-ci>=?", 1, None, |vm, a, n| char_cmp(vm, a, n, "char-ci>=?", true, |x, y| x >= y));
    def(vm, "char-foldcase", 1, Some(1), |vm, a, _| Ok(Value::char(fold(char_of(arg(vm, a, 0), "char-foldcase")?))));
    def(vm, "digit-value", 1, Some(1), |vm, a, _| {
        Ok(digit_value(char_of(arg(vm, a, 0), "digit-value")?).map_or(Value::FALSE, |d| Value::int_unchecked(d as i64)))
    });
    def(vm, "char-numeric?", 1, Some(1), |vm, a, _| Ok(Value::bool(digit_value(char_of(arg(vm, a, 0), "char-numeric?")?).is_some())));
    def(vm, "string-foldcase", 1, Some(1), |vm, a, _| {
        let s = fold_string(&str_of(arg(vm, a, 0), "string-foldcase")?);
        Ok(vm.make_string(s.as_bytes()))
    });

    // Ranges over strings and vectors (start and end count characters or
    // elements).
    def(vm, "string-copy", 1, Some(3), |vm, a, n| {
        let chars: Vec<char> = str_of(arg(vm, a, 0), "string-copy")?.chars().collect();
        let (s, e) = range(vm, a, n, 1, chars.len(), "string-copy")?;
        let out: String = chars[s..e].iter().collect();
        Ok(vm.make_string(out.as_bytes()))
    });
    def(vm, "string->list", 1, Some(3), |vm, a, n| {
        let chars: Vec<char> = str_of(arg(vm, a, 0), "string->list")?.chars().collect();
        let (s, e) = range(vm, a, n, 1, chars.len(), "string->list")?;
        let items: Vec<Value> = chars[s..e].iter().map(|c| Value::char(*c)).collect();
        Ok(vm.make_list(&items))
    });
    def(vm, "string->vector", 1, Some(3), |vm, a, n| {
        let chars: Vec<char> = str_of(arg(vm, a, 0), "string->vector")?.chars().collect();
        let (s, e) = range(vm, a, n, 1, chars.len(), "string->vector")?;
        let items: Vec<Value> = chars[s..e].iter().map(|c| Value::char(*c)).collect();
        Ok(vm.make_vector(&items))
    });
    def(vm, "vector->string", 1, Some(3), |vm, a, n| {
        let v = arg(vm, a, 0);
        if !is_kind(v, Kind::Vector) {
            return Err(type_error("vector->string", "vector", v));
        }
        let len = unsafe { len_of(v.as_ptr()) };
        let (s, e) = range(vm, a, n, 1, len, "vector->string")?;
        let out = (s..e).map(|i| char_of(unsafe { field(v.as_ptr(), i) }, "vector->string")).collect::<Result<String, _>>()?;
        Ok(vm.make_string(out.as_bytes()))
    });
    def(vm, "vector->list", 1, Some(3), |vm, a, n| {
        let v = arg(vm, a, 0);
        if !is_kind(v, Kind::Vector) {
            return Err(type_error("vector->list", "vector", v));
        }
        let len = unsafe { len_of(v.as_ptr()) };
        let (s, e) = range(vm, a, n, 1, len, "vector->list")?;
        let items: Vec<Value> = (s..e).map(|i| unsafe { field(v.as_ptr(), i) }).collect();
        Ok(vm.make_list(&items))
    });

    def(vm, "vector-fill!", 2, Some(4), |vm, a, n| {
        let v = arg(vm, a, 0);
        if !is_kind(v, Kind::Vector) {
            return Err(type_error("vector-fill!", "vector", v));
        }
        let (s, e) = range(vm, a, n, 2, unsafe { len_of(v.as_ptr()) }, "vector-fill!")?;
        let x = arg(vm, a, 1);
        for i in s..e {
            unsafe { crate::heap::set_field(v.as_ptr(), i, x) };
        }
        vm.write_barrier(v.as_ptr(), x);
        Ok(Value::VOID)
    });
    // (vector-copy! to at from [start [end]]), correct when they overlap.
    def(vm, "vector-copy!", 3, Some(5), |vm, a, n| {
        let (to, from) = (arg(vm, a, 0), arg(vm, a, 2));
        for v in [to, from] {
            if !is_kind(v, Kind::Vector) {
                return Err(type_error("vector-copy!", "vector", v));
            }
        }
        let at = num::integer(arg(vm, a, 1), "vector-copy!")?;
        let (s, e) = range(vm, a, n, 3, unsafe { len_of(from.as_ptr()) }, "vector-copy!")?;
        let to_len = unsafe { len_of(to.as_ptr()) };
        let at = usize::try_from(at).ok().filter(|&at| at + (e - s) <= to_len).ok_or_else(|| Error::new("vector-copy!: does not fit"))?;
        let items: Vec<Value> = (s..e).map(|i| unsafe { field(from.as_ptr(), i) }).collect();
        for (k, x) in items.into_iter().enumerate() {
            unsafe { crate::heap::set_field(to.as_ptr(), at + k, x) };
            vm.write_barrier(to.as_ptr(), x);
        }
        Ok(Value::VOID)
    });

    // Numbers.
    for (name, f) in [
        ("sin", f64::sin as fn(f64) -> f64),
        ("cos", f64::cos),
        ("tan", f64::tan),
        ("asin", f64::asin),
        ("acos", f64::acos),
        ("exp", f64::exp),
    ] {
        let f = std::rc::Rc::new(move |vm: &mut Vm, a: usize, _: usize| Ok(Value::float(f(float_of(arg(vm, a, 0), "math")?))));
        vm.define_native(Native { name: name.into(), f: NativeImpl::Boxed(f), min: 1, max: Some(1) });
    }
    def(vm, "atan", 1, Some(2), |vm, a, n| {
        let y = float_of(arg(vm, a, 0), "atan")?;
        Ok(Value::float(if n == 2 { y.atan2(float_of(arg(vm, a, 1), "atan")?) } else { y.atan() }))
    });
    def(vm, "log", 1, Some(2), |vm, a, n| {
        let z = float_of(arg(vm, a, 0), "log")?;
        Ok(Value::float(if n == 2 { z.ln() / float_of(arg(vm, a, 1), "log")?.ln() } else { z.ln() }))
    });
    def(vm, "exact-integer-sqrt", 1, Some(1), |vm, a, _| {
        let v = arg(vm, a, 0);
        let k = match num::num(v, "exact-integer-sqrt")? {
            N::I(i) if i >= 0 => BigInt::from(i),
            N::B(b) if b.sign() != num_bigint::Sign::Minus => b,
            _ => return Err(type_error("exact-integer-sqrt", "exact non-negative integer", v)),
        };
        let s = k.sqrt();
        let r = &k - &s * &s;
        let s = num::make_integer(vm, &s);
        let s = vm.root(s);
        let r = num::make_integer(vm, &r);
        two_values(vm, s.get(), r)
    });
    def(vm, "numerator", 1, Some(1), |vm, a, _| match num::num(arg(vm, a, 0), "numerator")? {
        N::F(f) if f.is_finite() => Ok(Value::float(float_ratio(f).0)),
        N::F(f) => Ok(Value::float(f)),
        _ => Ok(arg(vm, a, 0)),
    });
    def(vm, "denominator", 1, Some(1), |vm, a, _| match num::num(arg(vm, a, 0), "denominator")? {
        N::F(f) if f.is_finite() => Ok(Value::float(float_ratio(f).1)),
        N::F(_) => Ok(Value::float(1.0)),
        _ => Ok(Value::int_unchecked(1)),
    });
    def(vm, "real?", 1, Some(1), |vm, a, _| Ok(Value::bool(num::is_number(arg(vm, a, 0)))));
    def(vm, "complex?", 1, Some(1), |vm, a, _| Ok(Value::bool(num::is_number(arg(vm, a, 0)))));
    def(vm, "rational?", 1, Some(1), |vm, a, _| {
        let v = arg(vm, a, 0);
        Ok(Value::bool(num::is_number(v) && (!v.is_float() || v.as_float().is_finite())))
    });
    def(vm, "finite?", 1, Some(1), |vm, a, _| {
        let v = arg(vm, a, 0);
        num::num(v, "finite?")?;
        Ok(Value::bool(!v.is_float() || v.as_float().is_finite()))
    });
    def(vm, "infinite?", 1, Some(1), |vm, a, _| {
        let v = arg(vm, a, 0);
        num::num(v, "infinite?")?;
        Ok(Value::bool(v.is_float() && v.as_float().is_infinite()))
    });

    // Whether a procedure accepts n arguments (case-lambda).
    def(vm, "%accepts?", 2, Some(2), |vm, a, _| {
        let (f, n) = (arg(vm, a, 0), num::integer(arg(vm, a, 1), "%accepts?")? as usize);
        let accepts = if is_kind(f, Kind::Closure) {
            let code = unsafe { &*field(f.as_ptr(), 0).as_untraced_ptr::<crate::code::Code>() };
            n == code.nparams as usize || (code.rest && n >= code.nparams as usize)
        } else if f.is_native() {
            let native = &vm.natives[f.as_native()];
            n >= native.min && native.max.is_none_or(|m| n <= m)
        } else {
            true
        };
        Ok(Value::bool(accepts))
    });

    // Time.
    def(vm, "current-second", 0, Some(0), |_, _, _| {
        Ok(Value::float(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())))
    });
    def(vm, "current-jiffy", 0, Some(0), |_, _, _| Ok(Value::int_unchecked(start().elapsed().as_micros() as i64)));
    def(vm, "jiffies-per-second", 0, Some(0), |_, _, _| Ok(Value::int_unchecked(1_000_000)));
    def(vm, "features", 0, Some(0), |vm, _, _| {
        let names: Vec<Value> = ["r7rs", "techne", "full-unicode", "ratios-no", "exact-closed", "posix", std::env::consts::OS]
            .iter()
            .filter(|n| **n != "ratios-no")
            .map(|n| Value::symbol(crate::reader::intern(n)))
            .collect();
        Ok(vm.make_list(&names))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_and_folding() {
        assert_eq!(digit_value('7'), Some(7));
        assert_eq!(digit_value('\u{0664}'), Some(4));
        assert_eq!(digit_value('\u{2155}'), None);
        assert_eq!(digit_value('a'), None);
        assert_eq!(fold('Σ'), 'σ');
        assert_eq!(fold('ß'), 'ß');
        assert_eq!(fold_string("Maß ΧΑΟΣ"), "mass χαοσ");
    }

    #[test]
    fn float_ratios() {
        assert_eq!(float_ratio(0.5), (1.0, 2.0));
        assert_eq!(float_ratio(0.75), (3.0, 4.0));
        assert_eq!(float_ratio(3.0), (3.0, 1.0));
    }
}
