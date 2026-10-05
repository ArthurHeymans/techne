//! Native procedures.
//!
//! Natives receive their arguments as a window of the (rooted) register stack.
//! Allocation can move objects, so natives read arguments again after
//! allocating. Natives that build several objects reserve one block with
//! `Bulk` and carve the objects out of it, so only one collection can happen.

use std::hash::{Hash, Hasher};

use rustc_hash::FxHasher;

use crate::{
    heap::{self, Kind, LARGE_WORDS, field, header, is_kind, kind_of, len_of, set_field, str_bytes},
    num::{self, N},
    reader::{intern, symbol_name},
    value::{Special, Value},
    vm::{Error, Native, NativeFn, NativeImpl, SpecialObj, Vm, init_string},
};

type R = Result<Value, Error>;

pub fn type_error(who: &str, expected: &str, got: Value) -> Error {
    Error::new(format!("{who}: expected {expected}, got {}", repr(got)))
}

pub fn index_error(who: &str, v: Value, k: Value) -> Error {
    Error::new(format!("{who}: bad index {} for {}", repr(k), repr(v)))
}

#[inline(always)]
fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

/// Contiguous allocation carved into several objects.
pub struct Bulk {
    p: *mut u64,
    old: bool,
}

impl Bulk {
    pub fn new(vm: &mut Vm, words: usize) -> Bulk {
        if words >= LARGE_WORDS {
            let p = vm.heap.alloc_old_unremembered(words);
            Bulk { p, old: true }
        } else {
            Bulk { p: vm.alloc(words), old: false }
        }
    }
    pub fn take(&mut self, vm: &mut Vm, words: usize) -> *mut u64 {
        let p = self.p;
        self.p = unsafe { p.add(words) };
        if self.old {
            vm.heap.remember(p);
        }
        p
    }
    fn pair(&mut self, vm: &mut Vm, car: Value, cdr: Value) -> Value {
        let p = self.take(vm, 3);
        unsafe {
            *p = header(Kind::Pair, 2, 0);
            set_field(p, 0, car);
            set_field(p, 1, cdr);
        }
        Value::ptr(p)
    }
}

fn list_len(mut l: Value, who: &str) -> Result<usize, Error> {
    let mut n = 0;
    while is_kind(l, Kind::Pair) {
        n += 1;
        l = unsafe { field(l.as_ptr(), 1) };
    }
    if l != Value::NIL {
        return Err(type_error(who, "proper list", l));
    }
    Ok(n)
}

/// Elements of a proper list, or `None`.
pub fn list_values(l: Value) -> Option<Vec<Value>> {
    let mut out = Vec::new();
    let mut l = l;
    while is_kind(l, Kind::Pair) {
        out.push(unsafe { field(l.as_ptr(), 0) });
        l = unsafe { field(l.as_ptr(), 1) };
    }
    (l == Value::NIL).then_some(out)
}

/// Human-readable description of a raised object.
pub fn condition_message(vm: &Vm, v: Value) -> String {
    if let Some((msg, irritants)) = error_object_parts(vm, v) {
        let mut s = String::from_utf8_lossy(unsafe { str_bytes(msg.as_ptr()) }).into_owned();
        for i in list_items(irritants) {
            s.push(' ');
            print(&mut s, i, true);
        }
        s
    } else {
        format!("uncaught exception: {}", repr(v))
    }
}

/// Message and irritants of an error object.
pub fn error_object_parts(vm: &Vm, v: Value) -> Option<(Value, Value)> {
    let is_error = is_kind(v, Kind::Record) && unsafe { field(v.as_ptr(), 0) } == vm.special(SpecialObj::ErrorRtd);
    is_error.then(|| unsafe { (field(v.as_ptr(), 1), field(v.as_ptr(), 2)) })
}

fn list_items(mut l: Value) -> impl Iterator<Item = Value> {
    std::iter::from_fn(move || {
        is_kind(l, Kind::Pair).then(|| unsafe {
            let car = field(l.as_ptr(), 0);
            l = field(l.as_ptr(), 1);
            car
        })
    })
}

fn string_arg<'a>(v: Value, who: &str) -> Result<&'a [u8], Error> {
    if is_kind(v, Kind::String) { Ok(unsafe { str_bytes(v.as_ptr()) }) } else { Err(type_error(who, "string", v)) }
}

fn str_arg<'a>(v: Value, who: &str) -> Result<&'a str, Error> {
    Ok(unsafe { std::str::from_utf8_unchecked(string_arg(v, who)?) })
}

fn int_arg(v: Value, who: &str) -> Result<i64, Error> {
    if v.is_int() { Ok(v.as_int()) } else { num::integer(v, who) }
}

fn index_arg(v: Value, who: &str) -> Result<usize, Error> {
    let i = int_arg(v, who)?;
    usize::try_from(i).map_err(|_| Error::new(format!("{who}: negative index {i}")))
}

// ----- printing -----

pub fn repr(v: Value) -> String {
    let mut s = String::new();
    print(&mut s, v, true);
    s
}

pub fn print(out: &mut String, v: Value, write: bool) {
    use std::fmt::Write as _;
    if v.is_int() {
        let _ = write!(out, "{}", v.as_int());
    } else if v.is_float() {
        let f = v.as_float();
        if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e16 {
            let _ = write!(out, "{f:.1}");
        } else if f.is_nan() {
            out.push_str("+nan.0");
        } else if f.is_infinite() {
            out.push_str(if f > 0.0 { "+inf.0" } else { "-inf.0" });
        } else {
            let _ = write!(out, "{f}");
        }
    } else if v.is_char() {
        if write {
            let _ = write!(out, "#\\{}", v.as_char());
        } else {
            out.push(v.as_char());
        }
    } else if v.is_symbol() {
        out.push_str(&symbol_name(v.as_symbol()));
    } else if v.is_native() {
        out.push_str("#<procedure>");
    } else if v.is_keyword() {
        out.push_str("#:");
        out.push_str(&symbol_name(v.as_keyword()));
    } else if let Some(s) = v.as_special() {
        out.push_str(match s {
            Special::Nil => "()",
            Special::False => "#f",
            Special::True => "#t",
            Special::Void => "#<void>",
            Special::Eof => "#<eof>",
            Special::Undefined => "#<undefined>",
            Special::Empty => "#<empty>",
            Special::Unset => "#<unset>",
        });
    } else {
        let p = v.as_ptr();
        match unsafe { kind_of(p) } {
            k if k == Kind::Pair as u8 => {
                out.push('(');
                let mut l = v;
                let mut first = true;
                while is_kind(l, Kind::Pair) {
                    if !first {
                        out.push(' ');
                    }
                    first = false;
                    print(out, unsafe { field(l.as_ptr(), 0) }, write);
                    l = unsafe { field(l.as_ptr(), 1) };
                }
                if l != Value::NIL {
                    out.push_str(" . ");
                    print(out, l, write);
                }
                out.push(')');
            }
            k if k == Kind::Vector as u8 => {
                out.push_str("#(");
                for i in 0..unsafe { len_of(p) } {
                    if i > 0 {
                        out.push(' ');
                    }
                    print(out, unsafe { field(p, i) }, write);
                }
                out.push(')');
            }
            k if k == Kind::String as u8 => {
                let s = unsafe { std::str::from_utf8_unchecked(str_bytes(p)) };
                if write {
                    let _ = write!(out, "{s:?}");
                } else {
                    out.push_str(s);
                }
            }
            k if k == Kind::BigInt as u8 => {
                let _ = write!(out, "{}", unsafe { *p.add(1) } as i64);
            }
            k if k == Kind::Closure as u8 => out.push_str("#<procedure>"),
            k if k == Kind::Box as u8 => {
                out.push_str("#&");
                print(out, unsafe { field(p, 0) }, write);
            }
            k if k == Kind::Table as u8 => out.push_str("#<hash-table>"),
            k if k == Kind::Record as u8 => unsafe {
                let rtd = field(p, 0);
                out.push_str("#<");
                out.push_str(&symbol_name(field(rtd.as_ptr(), 0).as_symbol()));
                for i in 1..len_of(p) {
                    out.push(' ');
                    print(out, field(p, i), true);
                }
                out.push('>');
            },
            k if k == Kind::Rtd as u8 => {
                let _ = write!(out, "#<record-type {}>", symbol_name(unsafe { field(p, 0) }.as_symbol()));
            }
            k if k == Kind::Foreign as u8 => out.push_str("#<foreign>"),
            _ => out.push_str("#<unknown>"),
        }
    }
}

/// `display`/`write`/`displayln` with an optional port after the value.
fn output(vm: &mut Vm, args: usize, n: usize, write: bool, newline: bool) -> R {
    let port = (n > 1).then(|| arg(vm, args, 1));
    if n == 0 {
        crate::stdlib::write_out(vm, None, if newline { "\n" } else { "" })?;
    } else {
        crate::stdlib::display_to(vm, arg(vm, args, 0), port, write, newline)?;
    }
    Ok(Value::VOID)
}

// ----- equality and hashing -----

pub fn eqv(a: Value, b: Value) -> bool {
    a == b || (is_kind(a, Kind::BigInt) && is_kind(b, Kind::BigInt) && unsafe { *a.as_ptr().add(1) == *b.as_ptr().add(1) })
}

pub fn equal(a: Value, b: Value) -> bool {
    if eqv(a, b) {
        return true;
    }
    if !a.is_ptr() || !b.is_ptr() {
        return false;
    }
    let (p, q) = (a.as_ptr(), b.as_ptr());
    unsafe {
        let k = kind_of(p);
        if k != kind_of(q) {
            return false;
        }
        match k {
            k if k == Kind::Pair as u8 => equal(field(p, 0), field(q, 0)) && equal(field(p, 1), field(q, 1)),
            k if k == Kind::Vector as u8 => {
                len_of(p) == len_of(q) && (0..len_of(p)).all(|i| equal(field(p, i), field(q, i)))
            }
            k if k == Kind::String as u8 => str_bytes(p) == str_bytes(q),
            k if k == Kind::Box as u8 => equal(field(p, 0), field(q, 0)),
            _ => false,
        }
    }
}

fn hash_value(v: Value) -> Result<u64, Error> {
    let mut h = FxHasher::default();
    if is_kind(v, Kind::String) {
        unsafe { str_bytes(v.as_ptr()) }.hash(&mut h);
    } else if is_kind(v, Kind::BigInt) {
        unsafe { *v.as_ptr().add(1) }.hash(&mut h);
    } else if v.is_ptr() {
        return Err(type_error("hash table", "hashable key (number, string, symbol, char)", v));
    } else {
        v.bits().hash(&mut h);
    }
    Ok(h.finish())
}

/// Slot index of `key`, or of the slot where it would be inserted (the first
/// deleted slot passed, else the empty slot that ends the probe). Deleted keys
/// are `UNDEFINED` tombstones.
unsafe fn probe(slots: *mut u64, key: Value, hash: u64) -> usize {
    unsafe {
        let cap = len_of(slots) / 2;
        let mut i = (hash as usize) & (cap - 1);
        let mut tomb = None;
        loop {
            let k = field(slots, 2 * i);
            if k == Value::EMPTY {
                return tomb.unwrap_or(i);
            }
            if k == Value::UNDEFINED {
                tomb.get_or_insert(i);
            } else if k == key || (k.is_ptr() && equal(k, key)) {
                return i;
            }
            i = (i + 1) & (cap - 1);
        }
    }
}

fn is_free(k: Value) -> bool {
    k == Value::EMPTY || k == Value::UNDEFINED
}

fn hash_delete(vm: &mut Vm, args: usize, _: usize) -> R {
    let t = table_arg(arg(vm, args, 0), "hash-table-delete!")?;
    let key = arg(vm, args, 1);
    unsafe {
        let slots = field(t, 1).as_ptr();
        let i = probe(slots, key, hash_value(key)?);
        if !is_free(field(slots, 2 * i)) {
            set_field(slots, 2 * i, Value::UNDEFINED);
            set_field(slots, 2 * i + 1, Value::VOID);
            set_field(t, 0, Value::int_unchecked(field(t, 0).as_int() - 1));
        }
    }
    Ok(Value::VOID)
}

fn table_arg(v: Value, who: &str) -> Result<*mut u64, Error> {
    if is_kind(v, Kind::Table) { Ok(v.as_ptr()) } else { Err(type_error(who, "hash table", v)) }
}

fn make_hash_table(vm: &mut Vm, _: usize, _: usize) -> R {
    let cap = 8;
    let mut b = Bulk::new(vm, 4 + 1 + 2 * cap);
    let t = b.take(vm, 4);
    let slots = b.take(vm, 1 + 2 * cap);
    unsafe {
        *slots = header(Kind::Vector, 2 * cap, 0);
        for i in 0..2 * cap {
            set_field(slots, i, Value::EMPTY);
        }
        // Fields: live count, slot vector, used slots (live + deleted).
        *t = header(Kind::Table, 3, 0);
        set_field(t, 0, Value::int_unchecked(0));
        set_field(t, 1, Value::ptr(slots));
        set_field(t, 2, Value::int_unchecked(0));
    }
    Ok(Value::ptr(t))
}

fn hash_ref(vm: &mut Vm, args: usize, n: usize) -> R {
    let t = table_arg(arg(vm, args, 0), "hash-table-ref")?;
    let key = arg(vm, args, 1);
    let slots = unsafe { field(t, 1).as_ptr() };
    let i = unsafe { probe(slots, key, hash_value(key)?) };
    let k = unsafe { field(slots, 2 * i) };
    if !is_free(k) {
        return Ok(unsafe { field(slots, 2 * i + 1) });
    }
    if n > 2 { Ok(arg(vm, args, 2)) } else { Err(Error::new(format!("hash-table-ref: key not found: {}", repr(key)))) }
}

fn hash_contains(vm: &mut Vm, args: usize, _: usize) -> R {
    let t = table_arg(arg(vm, args, 0), "hash-table-contains?")?;
    let key = arg(vm, args, 1);
    let slots = unsafe { field(t, 1).as_ptr() };
    let i = unsafe { probe(slots, key, hash_value(key)?) };
    Ok(Value::bool(!is_free(unsafe { field(slots, 2 * i) })))
}

fn hash_set(vm: &mut Vm, args: usize, _: usize) -> R {
    let t = table_arg(arg(vm, args, 0), "hash-table-set!")?;
    let hash = hash_value(arg(vm, args, 1))?;
    unsafe {
        let used = field(t, 2).as_int() as usize;
        let cap = len_of(field(t, 1).as_ptr()) / 2;
        if (used + 1) * 4 > cap * 3 {
            // Grow: allocate first, then re-read everything.
            // Double the number of key/value slot pairs (or rehash in place
            // size when most used slots are deleted).
            let count = field(t, 0).as_int() as usize;
            let new_len = if count * 2 < cap { 2 * cap } else { 4 * cap };
            let new = Bulk::new(vm, 1 + new_len).take(vm, 1 + new_len);
            *new = header(Kind::Vector, new_len, 0);
            for i in 0..new_len {
                set_field(new, i, Value::EMPTY);
            }
            let t = arg(vm, args, 0).as_ptr();
            let old = field(t, 1).as_ptr();
            for i in 0..cap {
                let k = field(old, 2 * i);
                if !is_free(k) {
                    let j = probe(new, k, hash_value(k)?);
                    set_field(new, 2 * j, k);
                    set_field(new, 2 * j + 1, field(old, 2 * i + 1));
                }
            }
            set_field(t, 1, Value::ptr(new));
            set_field(t, 2, field(t, 0));
            vm.write_barrier(t, Value::ptr(new));
        }
        let t = arg(vm, args, 0).as_ptr();
        let (key, value) = (arg(vm, args, 1), arg(vm, args, 2));
        let slots = field(t, 1).as_ptr();
        let i = probe(slots, key, hash);
        let k = field(slots, 2 * i);
        if is_free(k) {
            set_field(t, 0, Value::int_unchecked(field(t, 0).as_int() + 1));
            if k == Value::EMPTY {
                set_field(t, 2, Value::int_unchecked(field(t, 2).as_int() + 1));
            }
            set_field(slots, 2 * i, key);
            vm.write_barrier(slots, key);
        }
        set_field(slots, 2 * i + 1, value);
        vm.write_barrier(slots, value);
    }
    Ok(Value::VOID)
}

fn hash_count(vm: &mut Vm, args: usize, _: usize) -> R {
    let t = table_arg(arg(vm, args, 0), "hash-table-count")?;
    Ok(unsafe { field(t, 0) })
}

// ----- numbers -----

fn fold_num(vm: &mut Vm, args: usize, n: usize, init: Value, op: fn(&mut Vm, Value, Value) -> R) -> R {
    let mut acc = init;
    for i in 0..n {
        let x = arg(vm, args, i);
        acc = op(vm, acc, x)?;
    }
    Ok(acc)
}

fn chain(vm: &mut Vm, args: usize, n: usize, cmp: fn(Value, Value) -> Result<bool, Error>) -> R {
    for i in 1..n {
        if !cmp(arg(vm, args, i - 1), arg(vm, args, i))? {
            return Ok(Value::FALSE);
        }
    }
    Ok(Value::TRUE)
}

fn float_fn(vm: &mut Vm, args: usize, who: &str, f: fn(f64) -> f64) -> R {
    let v = arg(vm, args, 0);
    match num::num(v, who)? {
        N::I(_) => Ok(v),
        N::F(x) => Ok(Value::float(f(x))),
    }
}

fn expt(vm: &mut Vm, args: usize, _: usize) -> R {
    let (a, b) = (num::num(arg(vm, args, 0), "expt")?, num::num(arg(vm, args, 1), "expt")?);
    match (a, b) {
        (N::I(x), N::I(y)) if y >= 0 => {
            let r = u32::try_from(y).ok().and_then(|y| x.checked_pow(y));
            r.map(|r| vm.make_int(r)).ok_or_else(|| Error::new("expt: integer overflow"))
        }
        (x, y) => {
            let f = |n: N| match n {
                N::I(i) => i as f64,
                N::F(f) => f,
            };
            Ok(Value::float(f(x).powf(f(y))))
        }
    }
}

fn sqrt(vm: &mut Vm, args: usize, _: usize) -> R {
    match num::num(arg(vm, args, 0), "sqrt")? {
        N::I(i) if i >= 0 => {
            let r = (i as f64).sqrt() as i64;
            Ok(if r * r == i { vm.make_int(r) } else { Value::float((i as f64).sqrt()) })
        }
        N::I(i) => Ok(Value::float((i as f64).sqrt())),
        N::F(f) => Ok(Value::float(f.sqrt())),
    }
}

fn number_to_string(vm: &mut Vm, args: usize, n: usize) -> R {
    let v = arg(vm, args, 0);
    let radix = if n > 1 { int_arg(arg(vm, args, 1), "number->string")? } else { 10 };
    let s = match (num::num(v, "number->string")?, radix) {
        (N::I(i), 10) => i.to_string(),
        (N::I(i), 16) => format!("{i:x}"),
        (N::I(i), 2) => format!("{i:b}"),
        (N::I(i), 8) => format!("{i:o}"),
        (N::I(_), r) => return Err(Error::new(format!("number->string: unsupported radix {r}"))),
        (N::F(_), _) => repr(v),
    };
    Ok(vm.make_string(s.as_bytes()))
}

fn string_to_number(vm: &mut Vm, args: usize, _: usize) -> R {
    let s = str_arg(arg(vm, args, 0), "string->number")?;
    if let Ok(i) = s.parse::<i64>() {
        return Ok(vm.make_int(i));
    }
    Ok(s.parse::<f64>().map(Value::float).unwrap_or(Value::FALSE))
}

// ----- lists -----

fn car(vm: &mut Vm, args: usize, _: usize) -> R {
    let p = arg(vm, args, 0);
    if !is_kind(p, Kind::Pair) {
        return Err(type_error("car", "pair", p));
    }
    Ok(unsafe { field(p.as_ptr(), 0) })
}

fn cdr(vm: &mut Vm, args: usize, _: usize) -> R {
    let p = arg(vm, args, 0);
    if !is_kind(p, Kind::Pair) {
        return Err(type_error("cdr", "pair", p));
    }
    Ok(unsafe { field(p.as_ptr(), 1) })
}

fn set_pair(vm: &mut Vm, args: usize, i: usize, who: &str) -> R {
    let (p, v) = (arg(vm, args, 0), arg(vm, args, 1));
    if !is_kind(p, Kind::Pair) {
        return Err(type_error(who, "pair", p));
    }
    unsafe { set_field(p.as_ptr(), i, v) };
    vm.write_barrier(p.as_ptr(), v);
    Ok(Value::VOID)
}

fn list(vm: &mut Vm, args: usize, n: usize) -> R {
    let mut b = Bulk::new(vm, 3 * n);
    let mut acc = Value::NIL;
    for i in (0..n).rev() {
        let car = arg(vm, args, i);
        acc = b.pair(vm, car, acc);
    }
    Ok(acc)
}

fn length(vm: &mut Vm, args: usize, _: usize) -> R {
    Ok(Value::int_unchecked(list_len(arg(vm, args, 0), "length")? as i64))
}

fn reverse(vm: &mut Vm, args: usize, _: usize) -> R {
    let len = list_len(arg(vm, args, 0), "reverse")?;
    let mut b = Bulk::new(vm, 3 * len);
    let mut acc = Value::NIL;
    let mut l = arg(vm, args, 0);
    while is_kind(l, Kind::Pair) {
        let (car, next) = unsafe { (field(l.as_ptr(), 0), field(l.as_ptr(), 1)) };
        acc = b.pair(vm, car, acc);
        l = next;
    }
    Ok(acc)
}

fn append(vm: &mut Vm, args: usize, n: usize) -> R {
    if n == 0 {
        return Ok(Value::NIL);
    }
    let total = (0..n - 1).map(|i| list_len(arg(vm, args, i), "append")).sum::<Result<usize, _>>()?;
    let mut b = Bulk::new(vm, 3 * total);
    let mut result = arg(vm, args, n - 1);
    let items: Vec<Value> = (0..n - 1).flat_map(|i| list_items(arg(vm, args, i))).collect();
    for car in items.into_iter().rev() {
        result = b.pair(vm, car, result);
    }
    Ok(result)
}

fn list_tail(vm: &mut Vm, args: usize, _: usize) -> R {
    let mut l = arg(vm, args, 0);
    for _ in 0..index_arg(arg(vm, args, 1), "list-tail")? {
        if !is_kind(l, Kind::Pair) {
            return Err(Error::new("list-tail: list too short"));
        }
        l = unsafe { field(l.as_ptr(), 1) };
    }
    Ok(l)
}

fn list_ref(vm: &mut Vm, args: usize, n: usize) -> R {
    let tail = list_tail(vm, args, n)?;
    if !is_kind(tail, Kind::Pair) {
        return Err(Error::new("list-ref: index out of range"));
    }
    Ok(unsafe { field(tail.as_ptr(), 0) })
}

fn mem_generic(vm: &mut Vm, args: usize, eq: fn(Value, Value) -> bool) -> R {
    let x = arg(vm, args, 0);
    let mut l = arg(vm, args, 1);
    while is_kind(l, Kind::Pair) {
        if eq(x, unsafe { field(l.as_ptr(), 0) }) {
            return Ok(l);
        }
        l = unsafe { field(l.as_ptr(), 1) };
    }
    Ok(Value::FALSE)
}

fn ass_generic(vm: &mut Vm, args: usize, eq: fn(Value, Value) -> bool) -> R {
    let x = arg(vm, args, 0);
    for entry in list_items(arg(vm, args, 1)) {
        if is_kind(entry, Kind::Pair) && eq(x, unsafe { field(entry.as_ptr(), 0) }) {
            return Ok(entry);
        }
    }
    Ok(Value::FALSE)
}

// ----- vectors -----

fn make_vector(vm: &mut Vm, args: usize, n: usize) -> R {
    let len = index_arg(arg(vm, args, 0), "make-vector")?;
    let p = Bulk::new(vm, 1 + len).take(vm, 1 + len);
    let fill = if n > 1 { arg(vm, args, 1) } else { Value::int_unchecked(0) };
    unsafe {
        *p = header(Kind::Vector, len, 0);
        for i in 0..len {
            set_field(p, i, fill);
        }
    }
    Ok(Value::ptr(p))
}

fn vector(vm: &mut Vm, args: usize, n: usize) -> R {
    let p = Bulk::new(vm, 1 + n).take(vm, 1 + n);
    unsafe {
        *p = header(Kind::Vector, n, 0);
        for i in 0..n {
            set_field(p, i, arg(vm, args, i));
        }
    }
    Ok(Value::ptr(p))
}

fn vector_arg(v: Value, who: &str) -> Result<*mut u64, Error> {
    if is_kind(v, Kind::Vector) { Ok(v.as_ptr()) } else { Err(type_error(who, "vector", v)) }
}

fn vector_length(vm: &mut Vm, args: usize, _: usize) -> R {
    Ok(Value::int_unchecked(unsafe { len_of(vector_arg(arg(vm, args, 0), "vector-length")?) } as i64))
}

fn vector_ref(vm: &mut Vm, args: usize, _: usize) -> R {
    let (v, k) = (arg(vm, args, 0), arg(vm, args, 1));
    let p = vector_arg(v, "vector-ref")?;
    let i = index_arg(k, "vector-ref")?;
    if i >= unsafe { len_of(p) } {
        return Err(index_error("vector-ref", v, k));
    }
    Ok(unsafe { field(p, i) })
}

fn vector_set(vm: &mut Vm, args: usize, _: usize) -> R {
    let (v, k, x) = (arg(vm, args, 0), arg(vm, args, 1), arg(vm, args, 2));
    let p = vector_arg(v, "vector-set!")?;
    let i = index_arg(k, "vector-set!")?;
    if i >= unsafe { len_of(p) } {
        return Err(index_error("vector-set!", v, k));
    }
    unsafe { set_field(p, i, x) };
    vm.write_barrier(p, x);
    Ok(Value::VOID)
}

fn vector_to_list(vm: &mut Vm, args: usize, _: usize) -> R {
    let len = unsafe { len_of(vector_arg(arg(vm, args, 0), "vector->list")?) };
    let mut b = Bulk::new(vm, 3 * len);
    let p = arg(vm, args, 0).as_ptr();
    let mut acc = Value::NIL;
    for i in (0..len).rev() {
        acc = b.pair(vm, unsafe { field(p, i) }, acc);
    }
    Ok(acc)
}

fn list_to_vector(vm: &mut Vm, args: usize, _: usize) -> R {
    let len = list_len(arg(vm, args, 0), "list->vector")?;
    let p = Bulk::new(vm, 1 + len).take(vm, 1 + len);
    unsafe {
        *p = header(Kind::Vector, len, 0);
        for (i, x) in list_items(arg(vm, args, 0)).enumerate() {
            set_field(p, i, x);
        }
    }
    Ok(Value::ptr(p))
}

// ----- strings and characters -----

fn char_arg(v: Value, who: &str) -> Result<char, Error> {
    if v.is_char() { Ok(v.as_char()) } else { Err(type_error(who, "char", v)) }
}

fn string_length(vm: &mut Vm, args: usize, _: usize) -> R {
    let v = arg(vm, args, 0);
    let bytes = string_arg(v, "string-length")?;
    let len = if unsafe { heap::str_is_ascii(v.as_ptr()) } {
        bytes.len()
    } else {
        unsafe { std::str::from_utf8_unchecked(bytes) }.chars().count()
    };
    Ok(Value::int_unchecked(len as i64))
}

fn string_ref(vm: &mut Vm, args: usize, _: usize) -> R {
    let (v, k) = (arg(vm, args, 0), arg(vm, args, 1));
    let bytes = string_arg(v, "string-ref")?;
    let i = index_arg(k, "string-ref")?;
    let c = if unsafe { heap::str_is_ascii(v.as_ptr()) } {
        bytes.get(i).map(|b| *b as char)
    } else {
        unsafe { std::str::from_utf8_unchecked(bytes) }.chars().nth(i)
    };
    c.map(Value::char).ok_or_else(|| index_error("string-ref", v, k))
}

/// Byte range of characters `start..end` of a string value.
fn char_range(v: Value, start: usize, end: usize, who: &str) -> Result<(usize, usize), Error> {
    let bytes = string_arg(v, who)?;
    if unsafe { heap::str_is_ascii(v.as_ptr()) } {
        if start > end || end > bytes.len() {
            return Err(Error::new(format!("{who}: range {start}..{end} out of bounds")));
        }
        return Ok((start, end));
    }
    let s = unsafe { std::str::from_utf8_unchecked(bytes) };
    let offsets: Vec<usize> = s.char_indices().map(|(i, _)| i).chain(std::iter::once(s.len())).collect();
    match (offsets.get(start), offsets.get(end)) {
        (Some(&a), Some(&b)) if start <= end => Ok((a, b)),
        _ => Err(Error::new(format!("{who}: range {start}..{end} out of bounds"))),
    }
}

fn substring(vm: &mut Vm, args: usize, n: usize) -> R {
    let v = arg(vm, args, 0);
    let start = index_arg(arg(vm, args, 1), "substring")?;
    let end = if n > 2 {
        index_arg(arg(vm, args, 2), "substring")?
    } else {
        string_length(vm, args, 1)?.as_int() as usize
    };
    let (a, b) = char_range(v, start, end, "substring")?;
    let p = vm.alloc(heap::string_words(b - a));
    unsafe {
        let src = str_bytes(arg(vm, args, 0).as_ptr());
        init_string(p, &src[a..b]);
    }
    Ok(Value::ptr(p))
}

fn string_append(vm: &mut Vm, args: usize, n: usize) -> R {
    let total = (0..n).map(|i| string_arg(arg(vm, args, i), "string-append").map(|s| s.len())).sum::<Result<usize, _>>()?;
    let p = Bulk::new(vm, heap::string_words(total)).take(vm, heap::string_words(total));
    let mut bytes = Vec::with_capacity(total);
    for i in 0..n {
        bytes.extend_from_slice(unsafe { str_bytes(arg(vm, args, i).as_ptr()) });
    }
    unsafe { init_string(p, &bytes) };
    Ok(Value::ptr(p))
}

fn string_cmp(vm: &mut Vm, args: usize, n: usize, ok: fn(std::cmp::Ordering) -> bool, who: &str) -> R {
    for i in 1..n {
        let (a, b) = (string_arg(arg(vm, args, i - 1), who)?, string_arg(arg(vm, args, i), who)?);
        if !ok(a.cmp(b)) {
            return Ok(Value::FALSE);
        }
    }
    Ok(Value::TRUE)
}

fn string_to_list(vm: &mut Vm, args: usize, _: usize) -> R {
    let chars: Vec<char> = str_arg(arg(vm, args, 0), "string->list")?.chars().collect();
    let mut b = Bulk::new(vm, 3 * chars.len());
    Ok(chars.into_iter().rev().fold(Value::NIL, |acc, c| b.pair(vm, Value::char(c), acc)))
}

fn list_to_string(vm: &mut Vm, args: usize, _: usize) -> R {
    let s = list_items(arg(vm, args, 0)).map(|c| char_arg(c, "list->string")).collect::<Result<String, _>>()?;
    Ok(vm.make_string(s.as_bytes()))
}

fn make_string(vm: &mut Vm, args: usize, n: usize) -> R {
    let len = index_arg(arg(vm, args, 0), "make-string")?;
    let c = if n > 1 { char_arg(arg(vm, args, 1), "make-string")? } else { ' ' };
    Ok(vm.make_string(c.to_string().repeat(len).as_bytes()))
}

fn string_copy(vm: &mut Vm, args: usize, _: usize) -> R {
    let s = string_arg(arg(vm, args, 0), "string-copy")?.to_vec();
    Ok(vm.make_string(&s))
}

fn string_to_symbol(vm: &mut Vm, args: usize, _: usize) -> R {
    Ok(Value::symbol(intern(str_arg(arg(vm, args, 0), "string->symbol")?)))
}

fn symbol_to_string(vm: &mut Vm, args: usize, _: usize) -> R {
    let v = arg(vm, args, 0);
    if !v.is_symbol() {
        return Err(type_error("symbol->string", "symbol", v));
    }
    let name = symbol_name(v.as_symbol());
    Ok(vm.make_string(name.as_bytes()))
}

fn string_prefix(vm: &mut Vm, args: usize, _: usize) -> R {
    let (p, s) = (string_arg(arg(vm, args, 0), "string-prefix?")?, string_arg(arg(vm, args, 1), "string-prefix?")?);
    Ok(Value::bool(s.starts_with(p)))
}

fn char_pred(vm: &mut Vm, args: usize, who: &str, f: fn(char) -> bool) -> R {
    Ok(Value::bool(f(char_arg(arg(vm, args, 0), who)?)))
}

// ----- boxes, I/O, misc -----

fn make_box(vm: &mut Vm, args: usize, _: usize) -> R {
    let p = vm.alloc(2);
    unsafe {
        *p = header(Kind::Box, 1, 0);
        set_field(p, 0, arg(vm, args, 0));
    }
    Ok(Value::ptr(p))
}

fn box_arg(v: Value, who: &str) -> Result<*mut u64, Error> {
    if is_kind(v, Kind::Box) { Ok(v.as_ptr()) } else { Err(type_error(who, "box", v)) }
}

fn read_lines(vm: &mut Vm, args: usize, _: usize) -> R {
    let path = str_arg(arg(vm, args, 0), "read-lines")?.to_owned();
    let text = std::fs::read_to_string(&path).map_err(|e| Error::new(format!("{path}: {e}")))?;
    let lines: Vec<&str> = text.lines().collect();
    let words: usize = lines.iter().map(|l| 3 + heap::string_words(l.len())).sum();
    let mut b = Bulk::new(vm, words);
    let mut acc = Value::NIL;
    for line in lines.iter().rev() {
        let s = b.take(vm, heap::string_words(line.len()));
        unsafe { init_string(s, line.as_bytes()) };
        acc = b.pair(vm, Value::ptr(s), acc);
    }
    Ok(acc)
}

fn error(vm: &mut Vm, args: usize, n: usize) -> R {
    let mut msg = String::new();
    print(&mut msg, arg(vm, args, 0), false);
    let irritants = vm.regs[args + 1..args + n].to_vec();
    let obj = vm.make_error_object(&msg, &irritants);
    Err(vm.raise_error(obj))
}

fn gc_stats(vm: &mut Vm, _: usize, _: usize) -> R {
    let s = format!("{:?}\n", vm.heap.stats);
    eprint!("{s}");
    Ok(Value::VOID)
}

fn collect_garbage(vm: &mut Vm, _: usize, _: usize) -> R {
    vm.collect();
    Ok(Value::VOID)
}

fn type_pred(vm: &mut Vm, args: usize, k: Kind) -> R {
    Ok(Value::bool(is_kind(arg(vm, args, 0), k)))
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
        "+" 0 _ => |vm: &mut Vm, a, n| fold_num(vm, a, n, Value::int_unchecked(0), num::add);
        "*" 0 _ => |vm: &mut Vm, a, n| fold_num(vm, a, n, Value::int_unchecked(1), num::mul);
        "-" 1 _ => |vm: &mut Vm, a, n| if n == 1 { num::sub(vm, Value::int_unchecked(0), arg(vm, a, 0)) } else {
            let first = arg(vm, a, 0); fold_num(vm, a + 1, n - 1, first, num::sub) };
        "/" 1 _ => |vm: &mut Vm, a, n| if n == 1 { num::div(vm, Value::int_unchecked(1), arg(vm, a, 0)) } else {
            let first = arg(vm, a, 0); fold_num(vm, a + 1, n - 1, first, num::div) };
        "<" 1 _ => |vm: &mut Vm, a, n| chain(vm, a, n, num::lt);
        "<=" 1 _ => |vm: &mut Vm, a, n| chain(vm, a, n, num::le);
        ">" 1 _ => |vm: &mut Vm, a, n| chain(vm, a, n, |x, y| num::lt(y, x));
        ">=" 1 _ => |vm: &mut Vm, a, n| chain(vm, a, n, |x, y| num::le(y, x));
        "=" 1 _ => |vm: &mut Vm, a, n| chain(vm, a, n, num::num_eq);
        "quotient" 2 2 => |vm: &mut Vm, a, _| num::quotient(vm, arg(vm, a, 0), arg(vm, a, 1));
        "remainder" 2 2 => |vm: &mut Vm, a, _| num::remainder(vm, arg(vm, a, 0), arg(vm, a, 1));
        "modulo" 2 2 => |vm: &mut Vm, a, _| num::modulo(vm, arg(vm, a, 0), arg(vm, a, 1));
        "abs" 1 1 => |vm: &mut Vm, a, _| match num::num(arg(vm, a, 0), "abs")? {
            N::I(i) => Ok(vm.make_int(i.abs())), N::F(f) => Ok(Value::float(f.abs())) };
        "min" 1 _ => |vm: &mut Vm, a, n| (1..n).try_fold(arg(vm, a, 0), |m, i| {
            let x = arg(vm, a, i); Ok(if num::lt(x, m)? { x } else { m }) });
        "max" 1 _ => |vm: &mut Vm, a, n| (1..n).try_fold(arg(vm, a, 0), |m, i| {
            let x = arg(vm, a, i); Ok(if num::lt(m, x)? { x } else { m }) });
        "expt" 2 2 => expt;
        "sqrt" 1 1 => sqrt;
        "exact->inexact" 1 1 => |vm: &mut Vm, a, _| match num::num(arg(vm, a, 0), "exact->inexact")? {
            N::I(i) => Ok(Value::float(i as f64)), N::F(f) => Ok(Value::float(f)) };
        "inexact" 1 1 => |vm: &mut Vm, a, _| match num::num(arg(vm, a, 0), "inexact")? {
            N::I(i) => Ok(Value::float(i as f64)), N::F(f) => Ok(Value::float(f)) };
        "inexact->exact" 1 1 => |vm: &mut Vm, a, _| { let i = num::integer(arg(vm, a, 0), "inexact->exact")?; Ok(vm.make_int(i)) };
        "exact" 1 1 => |vm: &mut Vm, a, _| { let i = num::integer(arg(vm, a, 0), "exact")?; Ok(vm.make_int(i)) };
        "floor" 1 1 => |vm: &mut Vm, a, _| float_fn(vm, a, "floor", f64::floor);
        "ceiling" 1 1 => |vm: &mut Vm, a, _| float_fn(vm, a, "ceiling", f64::ceil);
        "round" 1 1 => |vm: &mut Vm, a, _| float_fn(vm, a, "round", f64::round_ties_even);
        "truncate" 1 1 => |vm: &mut Vm, a, _| float_fn(vm, a, "truncate", f64::trunc);
        "number?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::is_number(arg(vm, a, 0))));
        "integer?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(matches!(num::num(arg(vm, a, 0), ""), Ok(N::I(_))) ||
            matches!(num::num(arg(vm, a, 0), ""), Ok(N::F(f)) if f.fract() == 0.0)));
        "zero?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::num_eq(arg(vm, a, 0), Value::int_unchecked(0))?));
        "positive?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::lt(Value::int_unchecked(0), arg(vm, a, 0))?));
        "negative?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::lt(arg(vm, a, 0), Value::int_unchecked(0))?));
        "even?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::integer(arg(vm, a, 0), "even?")? % 2 == 0));
        "odd?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::integer(arg(vm, a, 0), "odd?")? % 2 != 0));
        "number->string" 1 2 => number_to_string;
        "string->number" 1 1 => string_to_number;

        "cons" 2 2 => |vm: &mut Vm, a, _| { let (x, y) = (arg(vm, a, 0), arg(vm, a, 1)); Ok(vm.alloc_pair(x, y)) };
        "car" 1 1 => car;
        "cdr" 1 1 => cdr;
        "set-car!" 2 2 => |vm: &mut Vm, a, _| set_pair(vm, a, 0, "set-car!");
        "set-cdr!" 2 2 => |vm: &mut Vm, a, _| set_pair(vm, a, 1, "set-cdr!");
        "list" 0 _ => list;
        "length" 1 1 => length;
        "reverse" 1 1 => reverse;
        "append" 0 _ => append;
        "list-tail" 2 2 => list_tail;
        "list-ref" 2 2 => list_ref;
        "null?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0) == Value::NIL));
        "pair?" 1 1 => |vm: &mut Vm, a, _| type_pred(vm, a, Kind::Pair);
        "list?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(list_len(arg(vm, a, 0), "").is_ok()));
        "memq" 2 2 => |vm: &mut Vm, a, _| mem_generic(vm, a, |x, y| x == y);
        "memv" 2 2 => |vm: &mut Vm, a, _| mem_generic(vm, a, eqv);
        "member" 2 2 => |vm: &mut Vm, a, _| mem_generic(vm, a, equal);
        "assq" 2 2 => |vm: &mut Vm, a, _| ass_generic(vm, a, |x, y| x == y);
        "assv" 2 2 => |vm: &mut Vm, a, _| ass_generic(vm, a, eqv);
        "assoc" 2 2 => |vm: &mut Vm, a, _| ass_generic(vm, a, equal);

        "eq?" 2 2 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0) == arg(vm, a, 1)));
        "eqv?" 2 2 => |vm: &mut Vm, a, _| Ok(Value::bool(eqv(arg(vm, a, 0), arg(vm, a, 1))));
        "equal?" 2 2 => |vm: &mut Vm, a, _| Ok(Value::bool(equal(arg(vm, a, 0), arg(vm, a, 1))));
        "not" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0).is_false()));
        "boolean?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(matches!(arg(vm, a, 0), Value::TRUE | Value::FALSE)));
        "symbol?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0).is_symbol()));
        "string?" 1 1 => |vm: &mut Vm, a, _| type_pred(vm, a, Kind::String);
        "char?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0).is_char()));
        "vector?" 1 1 => |vm: &mut Vm, a, _| type_pred(vm, a, Kind::Vector);
        "procedure?" 1 1 => |vm: &mut Vm, a, _| { let v = arg(vm, a, 0); Ok(Value::bool(v.is_native() || is_kind(v, Kind::Closure) || Vm::applicable_proc(v).is_some())) };

        "make-vector" 1 2 => make_vector;
        "vector" 0 _ => vector;
        "vector-length" 1 1 => vector_length;
        "vector-ref" 2 2 => vector_ref;
        "vector-set!" 3 3 => vector_set;
        "vector->list" 1 1 => vector_to_list;
        "list->vector" 1 1 => list_to_vector;

        "string-length" 1 1 => string_length;
        "string-ref" 2 2 => string_ref;
        "substring" 2 3 => substring;
        "string-append" 0 _ => string_append;
        "string=?" 1 _ => |vm: &mut Vm, a, n| string_cmp(vm, a, n, |o| o.is_eq(), "string=?");
        "string<?" 1 _ => |vm: &mut Vm, a, n| string_cmp(vm, a, n, |o| o.is_lt(), "string<?");
        "string->list" 1 1 => string_to_list;
        "list->string" 1 1 => list_to_string;
        "make-string" 1 2 => make_string;
        "string-copy" 1 1 => string_copy;
        "string->symbol" 1 1 => string_to_symbol;
        "symbol->string" 1 1 => symbol_to_string;
        "string-prefix?" 2 2 => string_prefix;
        "char=?" 2 2 => |vm: &mut Vm, a, _| Ok(Value::bool(char_arg(arg(vm, a, 0), "char=?")? == char_arg(arg(vm, a, 1), "char=?")?));
        "char<?" 2 2 => |vm: &mut Vm, a, _| Ok(Value::bool(char_arg(arg(vm, a, 0), "char<?")? < char_arg(arg(vm, a, 1), "char<?")?));
        "char->integer" 1 1 => |vm: &mut Vm, a, _| Ok(Value::int_unchecked(char_arg(arg(vm, a, 0), "char->integer")? as i64));
        "integer->char" 1 1 => |vm: &mut Vm, a, _| {
            let i = int_arg(arg(vm, a, 0), "integer->char")?;
            u32::try_from(i).ok().and_then(char::from_u32).map(Value::char).ok_or_else(|| Error::new("integer->char: invalid code point")) };
        "char-alphabetic?" 1 1 => |vm: &mut Vm, a, _| char_pred(vm, a, "char-alphabetic?", char::is_alphabetic);
        "char-numeric?" 1 1 => |vm: &mut Vm, a, _| char_pred(vm, a, "char-numeric?", char::is_numeric);
        "char-whitespace?" 1 1 => |vm: &mut Vm, a, _| char_pred(vm, a, "char-whitespace?", char::is_whitespace);

        "box" 1 1 => make_box;
        "unbox" 1 1 => |vm: &mut Vm, a, _| Ok(unsafe { field(box_arg(arg(vm, a, 0), "unbox")?, 0) });
        "set-box!" 2 2 => |vm: &mut Vm, a, _| {
            let (b, v) = (box_arg(arg(vm, a, 0), "set-box!")?, arg(vm, a, 1));
            unsafe { set_field(b, 0, v) }; vm.write_barrier(b, v); Ok(Value::VOID) };

        "make-hash-table" 0 0 => make_hash_table;
        "hash-table-ref" 2 3 => hash_ref;
        "hash-table-set!" 3 3 => hash_set;
        "hash-table-count" 1 1 => hash_count;
        "hash-table-contains?" 2 2 => hash_contains;
        "hash-table-delete!" 2 2 => hash_delete;

        "display" 1 2 => |vm: &mut Vm, a, n| output(vm, a, n, false, false);
        "write" 1 2 => |vm: &mut Vm, a, n| output(vm, a, n, true, false);
        "displayln" 0 2 => |vm: &mut Vm, a, n| output(vm, a, n, false, true);
        "newline" 0 1 => |vm: &mut Vm, a, n| { let p = (n > 0).then(|| arg(vm, a, 0)); crate::stdlib::write_out(vm, p, "\n")?; Ok(Value::VOID) };
        "read-lines" 1 1 => read_lines;
        "file->lines" 1 1 => read_lines;
        "error" 1 _ => error;
        "void" 0 _ => |_: &mut Vm, _, _| Ok(Value::VOID);
        "exit" 0 1 => |vm: &mut Vm, a, n| { vm.flush(); std::process::exit(if n > 0 { int_arg(arg(vm, a, 0), "exit")? as i32 } else { 0 }) };
        "gc-stats" 0 0 => gc_stats;
        "collect-garbage" 0 0 => collect_garbage;
    }
    crate::stdlib::install(vm);
    crate::tasks::install(vm);
}
