//! Native procedures.
//!
//! Natives receive their arguments as a window of the (rooted) register stack.
//! Allocation can move objects, so natives read arguments again after
//! allocating. Natives that build several objects reserve one block with
//! `Bulk` and carve the objects out of it, so only one collection can happen.

use std::hash::{Hash, Hasher};

use rustc_hash::{FxHashMap, FxHasher};

use crate::{
    heap::{self, Kind, field, header, is_kind, kind_of, len_of, set_field, str_bytes},
    num::{self, N},
    reader::{self, intern, symbol_name},
    value::{Special, Value},
    vm::{Capability, Error, Native, NativeFn, NativeImpl, SpecialObj, Vm, init_string},
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
    /// Room for objects totalling `words`: one nursery block when it fits
    /// (at most one collection, here), else objects allocated one by one in
    /// the old generation (which never collects).
    pub fn new(vm: &mut Vm, words: usize) -> Bulk {
        if words < vm.heap.nursery_capacity() / 2 {
            Bulk { p: vm.reserve_nursery(words), old: false }
        } else {
            Bulk { p: std::ptr::null_mut(), old: true }
        }
    }
    pub fn take(&mut self, vm: &mut Vm, words: usize) -> *mut u64 {
        if self.old {
            return vm.heap.alloc_old(words);
        }
        let p = self.p;
        self.p = unsafe { p.add(words) };
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

/// The length of a proper list; an error for an improper or circular one.
fn list_len(l: Value, who: &str) -> Result<usize, Error> {
    let next = |v: Value| unsafe { field(v.as_ptr(), 1) };
    let (mut fast, mut slow, mut n) = (l, l, 0);
    while is_kind(fast, Kind::Pair) {
        fast = next(fast);
        n += 1;
        if n % 2 == 0 {
            slow = next(slow);
            if fast == slow {
                return Err(type_error(who, "proper list", l));
            }
        }
    }
    if fast != Value::NIL {
        return Err(type_error(who, "proper list", l));
    }
    Ok(n)
}

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

/// Which objects `write` marks with datum labels.
#[derive(Clone, Copy, PartialEq)]
pub enum Sharing {
    /// Only where a datum contains itself, so that printing ends (`write`,
    /// `display`).
    Cycles,
    /// Every object reached twice (`write-shared`).
    All,
    /// None (`write-simple`): a cycle does not end.
    None,
}

pub fn print(out: &mut String, v: Value, write: bool) {
    print_with(out, v, write, Sharing::Cycles)
}

pub fn print_with(out: &mut String, v: Value, write: bool, sharing: Sharing) {
    let labels = if sharing == Sharing::None { FxHashMap::default() } else { labeled(v, sharing == Sharing::All) };
    Printer { out, write, labels, next: 0 }.print(v)
}

/// Fields of an object `print` descends into.
fn children(v: Value) -> &'static [Value] {
    if !v.is_ptr() {
        return &[];
    }
    let p = v.as_ptr();
    unsafe {
        let k = kind_of(p);
        let (from, to) = match k {
            k if k == Kind::Pair as u8 => (0, 2),
            k if k == Kind::Vector as u8 || k == Kind::Box as u8 => (0, len_of(p)),
            k if k == Kind::Record as u8 => (1, len_of(p)),
            _ => return &[],
        };
        std::slice::from_raw_parts(p.add(1 + from) as *const Value, to - from)
    }
}

/// The objects of `v` that need a label: those inside themselves, or with
/// `shared` every one reached more than once.
fn labeled(v: Value, shared: bool) -> FxHashMap<u64, Option<u32>> {
    enum Step {
        Enter(Value),
        Leave(Value),
    }
    // Absent: unvisited; false: on the current path; true: done.
    let mut state: FxHashMap<u64, bool> = FxHashMap::default();
    let mut labels = FxHashMap::default();
    let mut todo = vec![Step::Enter(v)];
    while let Some(step) = todo.pop() {
        match step {
            Step::Leave(x) => {
                state.insert(x.bits(), true);
            }
            Step::Enter(x) => {
                let fields = children(x);
                if fields.is_empty() {
                    continue;
                }
                match state.get(&x.bits()) {
                    Some(false) => {
                        labels.insert(x.bits(), None);
                    }
                    Some(true) if shared => {
                        labels.insert(x.bits(), None);
                    }
                    Some(true) => {}
                    None => {
                        state.insert(x.bits(), false);
                        todo.push(Step::Leave(x));
                        todo.extend(fields.iter().rev().map(|&f| Step::Enter(f)));
                    }
                }
            }
        }
    }
    labels
}

struct Printer<'a> {
    out: &'a mut String,
    write: bool,
    /// Objects printed with a label, and the label once printed.
    labels: FxHashMap<u64, Option<u32>>,
    next: u32,
}

impl Printer<'_> {
    /// `#n#` for an object already printed, else its `#n=` if it needs one.
    /// Whether the object remains to be printed.
    fn label(&mut self, v: Value) -> bool {
        use std::fmt::Write as _;
        match self.labels.get(&v.bits()) {
            None => true,
            Some(Some(n)) => {
                let _ = write!(self.out, "#{n}#");
                false
            }
            Some(None) => {
                let n = self.next;
                self.next += 1;
                self.labels.insert(v.bits(), Some(n));
                let _ = write!(self.out, "#{n}=");
                true
            }
        }
    }

    fn print(&mut self, v: Value) {
        use std::fmt::Write as _;
        let write = self.write;
        let out = &mut *self.out;
        if v.is_int() {
            let _ = write!(out, "{}", v.as_int());
        } else if v.is_float() {
            out.push_str(&reader::float_repr(v.as_float()));
        } else if v.is_char() {
            if write {
                out.push_str(&reader::char_repr(v.as_char()));
            } else {
                out.push(v.as_char());
            }
        } else if v.is_symbol() {
            let name = symbol_name(v.as_symbol());
            if write {
                out.push_str(&reader::symbol_repr(&name));
            } else {
                out.push_str(&name);
            }
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
                    if !self.label(v) {
                        return;
                    }
                    self.out.push('(');
                    self.print(unsafe { field(p, 0) });
                    let mut l = unsafe { field(p, 1) };
                    while is_kind(l, Kind::Pair) && !self.labels.contains_key(&l.bits()) {
                        self.out.push(' ');
                        self.print(unsafe { field(l.as_ptr(), 0) });
                        l = unsafe { field(l.as_ptr(), 1) };
                    }
                    if l != Value::NIL {
                        self.out.push_str(" . ");
                        self.print(l);
                    }
                    self.out.push(')');
                }
                k if k == Kind::Vector as u8 => {
                    if !self.label(v) {
                        return;
                    }
                    self.out.push_str("#(");
                    for (i, &x) in children(v).iter().enumerate() {
                        if i > 0 {
                            self.out.push(' ');
                        }
                        self.print(x);
                    }
                    self.out.push(')');
                }
                k if k == Kind::String as u8 => {
                    let s = unsafe { std::str::from_utf8_unchecked(str_bytes(p)) };
                    if write {
                        out.push_str(&reader::string_repr(s));
                    } else {
                        out.push_str(s);
                    }
                }
                k if k == Kind::BigInt as u8 => {
                    let _ = write!(out, "{}", num::to_string_radix(&num::heap_int(Value::ptr(p)), 10));
                }
                k if k == Kind::Closure as u8 => out.push_str("#<procedure>"),
                k if k == Kind::Box as u8 => {
                    if !self.label(v) {
                        return;
                    }
                    self.out.push_str("#&");
                    self.print(unsafe { field(p, 0) });
                }
                k if k == Kind::Table as u8 => out.push_str("#<hash-table>"),
                k if k == Kind::Record as u8 => {
                    if !self.label(v) {
                        return;
                    }
                    let rtd = unsafe { field(p, 0) };
                    self.out.push_str("#<");
                    self.out.push_str(&symbol_name(unsafe { field(rtd.as_ptr(), 0) }.as_symbol()));
                    let saved = std::mem::replace(&mut self.write, true);
                    for &x in children(v) {
                        self.out.push(' ');
                        self.print(x);
                    }
                    self.write = saved;
                    self.out.push('>');
                }
                k if k == Kind::Rtd as u8 => {
                    let _ = write!(out, "#<record-type {}>", symbol_name(unsafe { field(p, 0) }.as_symbol()));
                }
                k if k == Kind::Foreign as u8 => out.push_str("#<foreign>"),
                _ => out.push_str("#<unknown>"),
            }
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
    a == b || (is_kind(a, Kind::BigInt) && is_kind(b, Kind::BigInt) && unsafe { bignum_key(a) == bignum_key(b) })
}

/// A bignum's sign and limbs (the header's GC flags vary between copies).
unsafe fn bignum_key<'a>(v: Value) -> (bool, &'a [u64]) {
    unsafe {
        let p = v.as_ptr();
        (*p & heap::NEGATIVE != 0, std::slice::from_raw_parts(p.add(1), len_of(p)))
    }
}

/// `equal?`: the same structure down to `eqv?` leaves. Iterative, and
/// after a budget of steps each pair of objects is compared once, so that
/// circular structure and heavily shared DAGs finish (a pair met again is
/// assumed equal: any difference fails the whole comparison anyway).
pub fn equal(a: Value, b: Value) -> bool {
    const BUDGET: usize = 100_000;
    // Leaves without the work list.
    if eqv(a, b) {
        return true;
    }
    if !a.is_ptr() || !b.is_ptr() {
        return false;
    }
    unsafe {
        let (k, l) = (kind_of(a.as_ptr()), kind_of(b.as_ptr()));
        if k != l {
            return false;
        }
        if k == Kind::String as u8 {
            return str_bytes(a.as_ptr()) == str_bytes(b.as_ptr());
        }
    }
    let mut todo = vec![(a, b)];
    let mut seen: rustc_hash::FxHashSet<(u64, u64)> = Default::default();
    let mut steps = 0;
    while let Some((a, b)) = todo.pop() {
        if eqv(a, b) {
            continue;
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
            let compound = k == Kind::Pair as u8 || k == Kind::Vector as u8 || k == Kind::Box as u8;
            if !compound {
                if k == Kind::String as u8 && str_bytes(p) == str_bytes(q) {
                    continue;
                }
                return false;
            }
            if len_of(p) != len_of(q) && k == Kind::Vector as u8 {
                return false;
            }
            steps += 1;
            if steps > BUDGET && !seen.insert((a.bits(), b.bits())) {
                continue;
            }
            let n = if k == Kind::Pair as u8 { 2 } else { len_of(p) };
            todo.extend((0..n).rev().map(|i| (field(p, i), field(q, i))));
        }
    }
    true
}

// ----- hash tables -----
//
// A table's fields: live count, slots, used slots (live and deleted) and
// its mode: an equivalence (`eq?`, `eqv?`, `equal?`) and whether keys are
// weak. Slots are key/value pairs, open addressing with linear probing;
// `EMPTY` keys end a probe and `UNDEFINED` ones are deleted. Weak tables
// keep their slots in an ephemeron object, whose entries the collector
// clears when their key dies, so their live count is counted on demand.

#[derive(Clone, Copy, PartialEq)]
enum Equiv {
    Eq = 0,
    Eqv = 1,
    Equal = 2,
}

const WEAK: i64 = 4;

/// Hash of `v` consistent with `equiv`: identity for objects the
/// equivalence compares by identity, else their contents.
#[inline(always)]
fn hash_key(vm: &mut Vm, v: Value, equiv: Equiv) -> u64 {
    let mut h = FxHasher::default();
    if !v.is_ptr() {
        v.bits().hash(&mut h);
        return h.finish();
    }
    if equiv == Equiv::Equal && is_kind(v, Kind::String) {
        unsafe { str_bytes(v.as_ptr()) }.hash(&mut h);
        return h.finish();
    }
    hash_into(vm, v, equiv, &mut h, &mut 32);
    h.finish()
}

/// Feeds `v` to `h`; `budget` bounds the nodes of a structure hashed, so
/// circular structure hashes too (equal structures share their start).
fn hash_into(vm: &mut Vm, v: Value, equiv: Equiv, h: &mut FxHasher, budget: &mut usize) {
    if !v.is_ptr() {
        v.bits().hash(h);
        return;
    }
    let p = v.as_ptr();
    let k = unsafe { kind_of(p) };
    match equiv {
        Equiv::Eqv | Equiv::Equal if k == Kind::BigInt as u8 => unsafe { bignum_key(v) }.hash(h),
        Equiv::Equal if k == Kind::String as u8 => unsafe { str_bytes(p) }.hash(h),
        Equiv::Equal if k == Kind::Pair as u8 || k == Kind::Vector as u8 || k == Kind::Box as u8 => {
            k.hash(h);
            let n = if k == Kind::Pair as u8 { 2 } else { unsafe { len_of(p) } };
            n.hash(h);
            for i in 0..n {
                if *budget == 0 {
                    return;
                }
                *budget -= 1;
                hash_into(vm, unsafe { field(p, i) }, equiv, h, budget);
            }
        }
        _ => vm.heap.identity_hash(p).hash(h),
    }
}

#[inline]
fn same_key(a: Value, b: Value, equiv: Equiv) -> bool {
    // Immediates are the same exactly when their bits are.
    a == b
        || a.is_ptr()
            && b.is_ptr()
            && match equiv {
                Equiv::Eq => false,
                Equiv::Eqv => eqv(a, b),
                Equiv::Equal => equal(a, b),
            }
}

/// Slot index of `key`, or of the slot where it would be inserted (the first
/// deleted slot passed, else the empty slot that ends the probe).
#[inline(always)]
unsafe fn probe(slots: *mut u64, key: Value, hash: u64, equiv: Equiv) -> usize {
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
            } else if same_key(k, key, equiv) {
                return i;
            }
            i = (i + 1) & (cap - 1);
        }
    }
}

fn is_free(k: Value) -> bool {
    k == Value::EMPTY || k == Value::UNDEFINED
}

#[inline(always)]
fn table_arg(v: Value, who: &str) -> Result<*mut u64, Error> {
    if is_kind(v, Kind::Table) { Ok(v.as_ptr()) } else { Err(type_error(who, "hash table", v)) }
}

#[inline(always)]
fn table_mode(t: *mut u64) -> (Equiv, bool) {
    let mode = unsafe { field(t, 3) }.as_int();
    let equiv = match mode & 3 {
        0 => Equiv::Eq,
        1 => Equiv::Eqv,
        _ => Equiv::Equal,
    };
    (equiv, mode & WEAK != 0)
}

/// The slot of `key` in the table argument at 0, and its slots, after
/// hashing (which can set an identity-hash flag but does not allocate).
#[inline(always)]
fn find(vm: &mut Vm, args: usize, key: Value, who: &str) -> Result<(*mut u64, usize), Error> {
    let t = table_arg(arg(vm, args, 0), who)?;
    let (equiv, _) = table_mode(t);
    let hash = hash_key(vm, key, equiv);
    let slots = unsafe { field(t, 1).as_ptr() };
    Ok((slots, unsafe { probe(slots, key, hash, equiv) }))
}

/// A slot vector of `cap` key/value pairs, all empty.
fn new_slots(b: &mut Bulk, vm: &mut Vm, cap: usize, weak: bool) -> *mut u64 {
    let slots = b.take(vm, 1 + 2 * cap);
    unsafe {
        *slots = header(if weak { Kind::Ephemerons } else { Kind::Vector }, 2 * cap, 0);
        for i in 0..2 * cap {
            set_field(slots, i, Value::EMPTY);
        }
    }
    slots
}

/// `(make-hash-table [equivalence])`, `(make-weak-hash-table [equivalence])`:
/// the builtin `equal?` (the default for strong tables), `eqv?`, `eq?` (the
/// default for weak ones) or `string=?`.
fn make_table(vm: &mut Vm, args: usize, n: usize, weak: bool) -> R {
    let equiv = if n == 0 {
        if weak { Equiv::Eq } else { Equiv::Equal }
    } else {
        // The builtin procedures themselves, whatever they are bound to now.
        let f = arg(vm, args, 0);
        let name = if f.is_native() { vm.procedure_name(f).unwrap_or_default() } else { "".into() };
        match &*name {
            "eq?" => Equiv::Eq,
            "eqv?" | "=" | "char=?" => Equiv::Eqv,
            "equal?" | "string=?" => Equiv::Equal,
            _ => return Err(type_error("make-hash-table", "the builtin eq?, eqv?, equal? or string=?", f)),
        }
    };
    if n > 1 {
        return Err(Error::new("make-hash-table: hash functions are not supported; the equivalence decides the hash"));
    }
    let cap = 8;
    let mut b = Bulk::new(vm, 5 + 1 + 2 * cap);
    let t = b.take(vm, 5);
    let slots = new_slots(&mut b, vm, cap, weak);
    unsafe {
        *t = header(Kind::Table, 4, 0);
        set_field(t, 0, Value::int_unchecked(0));
        set_field(t, 1, Value::ptr(slots));
        set_field(t, 2, Value::int_unchecked(0));
        set_field(t, 3, Value::int_unchecked(equiv as i64 | if weak { WEAK } else { 0 }));
    }
    Ok(Value::ptr(t))
}

fn hash_delete(vm: &mut Vm, args: usize, _: usize) -> R {
    let (slots, i) = find(vm, args, arg(vm, args, 1), "hash-table-delete!")?;
    let t = arg(vm, args, 0).as_ptr();
    unsafe {
        if !is_free(field(slots, 2 * i)) {
            set_field(slots, 2 * i, Value::UNDEFINED);
            set_field(slots, 2 * i + 1, Value::VOID);
            set_field(t, 0, Value::int_unchecked(field(t, 0).as_int() - 1));
        }
    }
    Ok(Value::VOID)
}

fn hash_ref(vm: &mut Vm, args: usize, n: usize) -> R {
    let key = arg(vm, args, 1);
    let (slots, i) = find(vm, args, key, "hash-table-ref")?;
    if !is_free(unsafe { field(slots, 2 * i) }) {
        return Ok(unsafe { field(slots, 2 * i + 1) });
    }
    if n > 2 { Ok(arg(vm, args, 2)) } else { Err(Error::new(format!("hash-table-ref: key not found: {}", repr(key)))) }
}

fn hash_contains(vm: &mut Vm, args: usize, _: usize) -> R {
    let (slots, i) = find(vm, args, arg(vm, args, 1), "hash-table-contains?")?;
    Ok(Value::bool(!is_free(unsafe { field(slots, 2 * i) })))
}

fn hash_set(vm: &mut Vm, args: usize, _: usize) -> R {
    let t = table_arg(arg(vm, args, 0), "hash-table-set!")?;
    let (equiv, weak) = table_mode(t);
    unsafe {
        let used = field(t, 2).as_int() as usize;
        let cap = len_of(field(t, 1).as_ptr()) / 2;
        if (used + 1) * 4 > cap * 3 {
            // Grow: allocate first, then re-read everything. Double the
            // number of key/value slot pairs (or rehash at the same size
            // when most used slots are deleted). The collector clears dead
            // entries of weak tables without counting, so count them here.
            let count = if weak { hash_count(vm, args, 1)?.as_int() as usize } else { field(t, 0).as_int() as usize };
            let new_cap = if count * 2 < cap { cap } else { 2 * cap };
            let mut b = Bulk::new(vm, 1 + 2 * new_cap);
            let new = new_slots(&mut b, vm, new_cap, weak);
            let t = arg(vm, args, 0).as_ptr();
            let old = field(t, 1).as_ptr();
            let mut live = 0;
            for i in 0..cap {
                let k = field(old, 2 * i);
                if !is_free(k) {
                    let j = probe(new, k, hash_key(vm, k, equiv), equiv);
                    set_field(new, 2 * j, k);
                    set_field(new, 2 * j + 1, field(old, 2 * i + 1));
                    live += 1;
                }
            }
            set_field(t, 0, Value::int_unchecked(live));
            set_field(t, 1, Value::ptr(new));
            set_field(t, 2, Value::int_unchecked(live));
            vm.write_barrier(t, Value::ptr(new));
        }
        let key = arg(vm, args, 1);
        let (slots, i) = find(vm, args, key, "hash-table-set!")?;
        let t = arg(vm, args, 0).as_ptr();
        let value = arg(vm, args, 2);
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
    if !table_mode(t).1 {
        return Ok(unsafe { field(t, 0) });
    }
    let slots = unsafe { field(t, 1).as_ptr() };
    let live = (0..unsafe { len_of(slots) } / 2).filter(|&i| !is_free(unsafe { field(slots, 2 * i) })).count();
    Ok(Value::int_unchecked(live as i64))
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
        N::F(x) => Ok(Value::float(f(x))),
        _ => Ok(v),
    }
}

fn expt(vm: &mut Vm, args: usize, _: usize) -> R {
    let (a, b) = (num::num(arg(vm, args, 0), "expt")?, num::num(arg(vm, args, 1), "expt")?);
    match (&a, &b) {
        (x, N::I(y)) if x.is_exact() && *y >= 0 => {
            let y = u32::try_from(*y).map_err(|_| Error::new("expt: exponent too large"))?;
            Ok(num::expt_int(vm, x, y))
        }
        _ => Ok(Value::float(a.f().powf(b.f()))),
    }
}

fn sqrt(vm: &mut Vm, args: usize, _: usize) -> R {
    let n = num::num(arg(vm, args, 0), "sqrt")?;
    if n.is_exact()
        && n.f() >= 0.0
        && let Some(r) = num::exact_sqrt(&n)
    {
        return Ok(num::make_integer(vm, &r));
    }
    Ok(Value::float(n.f().sqrt()))
}

fn number_to_string(vm: &mut Vm, args: usize, n: usize) -> R {
    let v = arg(vm, args, 0);
    let radix = if n > 1 { int_arg(arg(vm, args, 1), "number->string")? } else { 10 };
    let n = num::num(v, "number->string")?;
    let s = match radix {
        _ if !n.is_exact() => repr(v),
        2..=36 => num::to_string_radix(&n, radix as u32),
        r => return Err(Error::new(format!("number->string: unsupported radix {r}"))),
    };
    Ok(vm.make_string(s.as_bytes()))
}

fn exact(vm: &mut Vm, v: Value, who: &str) -> R {
    match num::num(v, who)? {
        N::F(f) => num::exact_of_float(vm, f, who),
        _ => Ok(v),
    }
}

/// Whether the integer argument is even.
fn parity(vm: &Vm, args: usize, who: &str) -> Result<bool, Error> {
    let v = arg(vm, args, 0);
    if !num::is_integer(v) {
        return Err(type_error(who, "integer", v));
    }
    Ok(num::big_parity_even(&num::num(v, who)?))
}

fn string_to_number(vm: &mut Vm, args: usize, n: usize) -> R {
    let s = str_arg(arg(vm, args, 0), "string->number")?;
    let radix = if n > 1 { int_arg(arg(vm, args, 1), "string->number")? } else { 10 };
    if !matches!(radix, 2 | 8 | 10 | 16) {
        return Err(Error::new(format!("string->number: unsupported radix {radix}")));
    }
    Ok(match num::parse(s, radix as u32) {
        num::Parsed::Number(n) => num::from_n(vm, n),
        _ => Value::FALSE,
    })
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

/// `member` or `assoc` with the comparison procedure at argument 2.
fn find_with(vm: &mut Vm, args: usize, assoc: bool) -> R {
    let cur = vm.root(arg(vm, args, 1));
    while is_kind(cur.get(), Kind::Pair) {
        let item = unsafe { field(cur.get().as_ptr(), 0) };
        let candidate = !assoc || is_kind(item, Kind::Pair);
        if candidate {
            let key = if assoc { unsafe { field(item.as_ptr(), 0) } } else { item };
            let (f, x) = (arg(vm, args, 2), arg(vm, args, 0));
            if vm.call(f, &[x, key])?.is_truthy() {
                let l = cur.get();
                return Ok(if assoc { unsafe { field(l.as_ptr(), 0) } } else { l });
            }
        }
        cur.set(unsafe { field(cur.get().as_ptr(), 1) });
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

/// Replaces characters `start..start + text's length` of the string at
/// argument 0 by `text`, in place. Strings are UTF-8, so this works when the
/// replacement takes as many bytes as the characters it replaces (always
/// for ASCII), and is refused otherwise; literals are refused too.
fn string_mutate(vm: &Vm, args: usize, start: usize, text: &str, who: &str) -> R {
    let s = arg(vm, args, 0);
    string_arg(s, who)?;
    let p = s.as_ptr();
    if unsafe { *p } & heap::IMMUTABLE != 0 {
        return Err(Error::new(format!("{who}: string literals cannot be changed")));
    }
    let (a, b) = char_range(s, start, start + text.chars().count(), who)?;
    if b - a != text.len() {
        return Err(Error::new(format!(
            "{who}: the new characters take {} bytes where the old take {}; strings change in place only at the same UTF-8 size",
            text.len(),
            b - a
        )));
    }
    unsafe { heap::str_replace(p, a, text.as_bytes()) };
    Ok(Value::VOID)
}

fn string_set(vm: &mut Vm, args: usize, _: usize) -> R {
    let k = index_arg(arg(vm, args, 1), "string-set!")?;
    let c = char_arg(arg(vm, args, 2), "string-set!")?;
    string_mutate(vm, args, k, c.encode_utf8(&mut [0; 4]), "string-set!")
}

fn string_fill(vm: &mut Vm, args: usize, n: usize) -> R {
    let c = char_arg(arg(vm, args, 1), "string-fill!")?;
    let len = str_arg(arg(vm, args, 0), "string-fill!")?.chars().count();
    let (a, b) = range_args(vm, args, n, 2, len, "string-fill!")?;
    string_mutate(vm, args, a, &c.to_string().repeat(b - a), "string-fill!")
}

/// `(string-copy! to at from [start end])`.
fn string_copy_into(vm: &mut Vm, args: usize, n: usize) -> R {
    let at = index_arg(arg(vm, args, 1), "string-copy!")?;
    let from: Vec<char> = str_arg(arg(vm, args, 2), "string-copy!")?.chars().collect();
    let (a, b) = range_args(vm, args, n, 3, from.len(), "string-copy!")?;
    let text: String = from[a..b].iter().collect();
    string_mutate(vm, args, at, &text, "string-copy!")
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
    let end = if n > 2 { index_arg(arg(vm, args, 2), "substring")? } else { string_length(vm, args, 1)?.as_int() as usize };
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

fn list_to_string(vm: &mut Vm, args: usize, _: usize) -> R {
    let s = list_items(arg(vm, args, 0)).map(|c| char_arg(c, "list->string")).collect::<Result<String, _>>()?;
    Ok(vm.make_string(s.as_bytes()))
}

fn make_string(vm: &mut Vm, args: usize, n: usize) -> R {
    let len = index_arg(arg(vm, args, 0), "make-string")?;
    let c = if n > 1 { char_arg(arg(vm, args, 1), "make-string")? } else { ' ' };
    Ok(vm.make_string(c.to_string().repeat(len).as_bytes()))
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

/// `(collect-garbage)`: a minor collection and a slice of old-generation
/// work; `(collect-garbage 'full)` completes a whole cycle.
fn collect_garbage(vm: &mut Vm, args: usize, n: usize) -> R {
    if n > 0 && arg(vm, args, 0) == Value::symbol(intern("full")) {
        vm.full_collect();
    } else {
        vm.collect();
    }
    Ok(Value::VOID)
}

fn type_pred(vm: &mut Vm, args: usize, k: Kind) -> R {
    Ok(Value::bool(is_kind(arg(vm, args, 0), k)))
}

// ----- R7RS procedures beyond the core -----

/// Optional `start`/`end` arguments at `i` and `i + 1`, within `0..=len`.
fn range_args(vm: &Vm, args: usize, n: usize, i: usize, len: usize, who: &str) -> Result<(usize, usize), Error> {
    let start = if n > i { index_arg(arg(vm, args, i), who)? } else { 0 };
    let end = if n > i + 1 { index_arg(arg(vm, args, i + 1), who)? } else { len };
    if start > end || end > len {
        return Err(Error::new(format!("{who}: range {start}..{end} out of bounds 0..{len}")));
    }
    Ok((start, end))
}

/// The characters of string argument 0 in the optional range at 1.
fn string_range(vm: &Vm, args: usize, n: usize, who: &str) -> Result<Vec<char>, Error> {
    let chars: Vec<char> = str_arg(arg(vm, args, 0), who)?.chars().collect();
    let (a, b) = range_args(vm, args, n, 1, chars.len(), who)?;
    Ok(chars[a..b].to_vec())
}

/// The elements of vector argument 0 in the optional range at `i`.
fn vector_range(vm: &Vm, args: usize, n: usize, i: usize, who: &str) -> Result<Vec<Value>, Error> {
    let p = vector_arg(arg(vm, args, 0), who)?;
    let (a, b) = range_args(vm, args, n, i, unsafe { len_of(p) }, who)?;
    Ok((a..b).map(|k| unsafe { field(p, k) }).collect())
}

/// `(vector-copy! to at from [start end])`, overlapping ranges included.
fn vector_copy_into(vm: &mut Vm, args: usize, n: usize) -> R {
    let to = vector_arg(arg(vm, args, 0), "vector-copy!")?;
    let at = index_arg(arg(vm, args, 1), "vector-copy!")?;
    let from = vector_arg(arg(vm, args, 2), "vector-copy!")?;
    let (a, b) = range_args(vm, args, n, 3, unsafe { len_of(from) }, "vector-copy!")?;
    if at + (b - a) > unsafe { len_of(to) } {
        return Err(Error::new("vector-copy!: not enough room in the target"));
    }
    let items: Vec<Value> = (a..b).map(|k| unsafe { field(from, k) }).collect();
    for (k, &x) in items.iter().enumerate() {
        unsafe { set_field(to, at + k, x) };
        vm.write_barrier(to, x);
    }
    Ok(Value::VOID)
}

fn vector_fill_range(vm: &mut Vm, args: usize, n: usize) -> R {
    let (p, x) = (vector_arg(arg(vm, args, 0), "vector-fill!")?, arg(vm, args, 1));
    let (a, b) = range_args(vm, args, n, 2, unsafe { len_of(p) }, "vector-fill!")?;
    for k in a..b {
        unsafe { set_field(p, k, x) };
    }
    vm.write_barrier(p, x);
    Ok(Value::VOID)
}

/// Simple case folding: the full folding when it is one character, else
/// the lowercase character (`ẞ` to `ß`, where full folding gives `ss`).
fn fold_char(c: char) -> char {
    let single = |it: &mut dyn Iterator<Item = char>| match (it.next(), it.next()) {
        (Some(f), None) => Some(f),
        _ => None,
    };
    single(&mut caseless::Caseless::default_case_fold(std::iter::once(c))).or_else(|| single(&mut c.to_lowercase())).unwrap_or(c)
}

/// Unicode full case folding.
pub fn fold_string(s: &str) -> String {
    caseless::default_case_fold_str(s)
}

/// `char=?` and the like, any number of arguments, folded with `ci`.
fn char_chain(vm: &mut Vm, args: usize, n: usize, who: &str, ci: bool, ok: fn(std::cmp::Ordering) -> bool) -> R {
    if n == 2 && !ci {
        return Ok(Value::bool(ok(char_arg(arg(vm, args, 0), who)?.cmp(&char_arg(arg(vm, args, 1), who)?))));
    }
    let get = |i| char_arg(arg(vm, args, i), who).map(|c| if ci { fold_char(c) } else { c });
    for i in 1..n {
        if !ok(get(i - 1)?.cmp(&get(i)?)) {
            return Ok(Value::FALSE);
        }
    }
    Ok(Value::TRUE)
}

/// Zeros of the Unicode decimal digit (Nd) runs, which have ten digits each.
const DIGIT_ZEROS: &[u32] = &[
    0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66, 0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090,
    0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90, 0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
    0xff10, 0x104a0, 0x10d30, 0x10d40, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450, 0x114d0, 0x11650, 0x116c0, 0x116d0, 0x116da,
    0x11730, 0x118e0, 0x11950, 0x11bf0, 0x11c50, 0x11d50, 0x11da0, 0x11f50, 0x16130, 0x16a60, 0x16ac0, 0x16b50, 0x16d70, 0x1ccf0, 0x1d7ce,
    0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e5f1, 0x1e950, 0x1fbf0,
];

fn digit_value(c: char) -> Option<i64> {
    let c = c as u32;
    DIGIT_ZEROS.iter().find(|&&z| (z..z + 10).contains(&c)).map(|z| (c - z) as i64)
}

/// Integer arguments as big integers, and whether any was inexact.
fn integers(vm: &Vm, args: usize, n: usize, who: &str) -> Result<(Vec<num_bigint::BigInt>, bool), Error> {
    let mut inexact = false;
    let ints = (0..n)
        .map(|i| {
            let v = arg(vm, args, i);
            match num::num(v, who)? {
                N::I(i) => Ok(i.into()),
                N::B(b) => Ok(b),
                N::F(f) if f.fract() == 0.0 && f.is_finite() => {
                    inexact = true;
                    Ok(<num_bigint::BigInt as num_traits::FromPrimitive>::from_f64(f).expect("finite"))
                }
                N::F(_) => Err(type_error(who, "integer", v)),
            }
        })
        .collect::<Result<_, _>>()?;
    Ok((ints, inexact))
}

fn int_result(vm: &mut Vm, b: &num_bigint::BigInt, inexact: bool) -> Value {
    use num_traits::ToPrimitive;
    if inexact { Value::float(b.to_f64().unwrap_or(f64::NAN)) } else { num::make_integer(vm, b) }
}

fn gcd_lcm(vm: &mut Vm, args: usize, n: usize, lcm: bool) -> R {
    use num_integer::Integer;
    use num_traits::Signed;
    let (ints, inexact) = integers(vm, args, n, if lcm { "lcm" } else { "gcd" })?;
    let start = num_bigint::BigInt::from(if lcm { 1 } else { 0 });
    let r = ints.iter().fold(start, |acc, x| if lcm { acc.lcm(x) } else { acc.gcd(x) }).abs();
    Ok(int_result(vm, &r, inexact))
}

/// `floor-quotient`: the quotient rounded towards negative infinity.
fn floor_quotient(vm: &mut Vm, args: usize, _: usize) -> R {
    use num_integer::Integer;
    let (ints, inexact) = integers(vm, args, 2, "floor-quotient")?;
    if ints[1] == 0.into() {
        return Err(Error::new("floor-quotient: division by zero"));
    }
    let q = ints[0].div_floor(&ints[1]);
    Ok(int_result(vm, &q, inexact))
}

/// `(exact-integer-sqrt n)`'s root.
fn isqrt(vm: &mut Vm, args: usize, _: usize) -> R {
    let v = arg(vm, args, 0);
    let (ints, inexact) = integers(vm, args, 1, "exact-integer-sqrt")?;
    if inexact || ints[0] < 0.into() {
        return Err(type_error("exact-integer-sqrt", "exact non-negative integer", v));
    }
    Ok(num::make_integer(vm, &ints[0].sqrt()))
}

/// `numerator` or `denominator`: of an integer, or of a float's exact
/// binary fraction (as floats).
fn ratio_part(vm: &mut Vm, args: usize, numerator: bool) -> R {
    let v = arg(vm, args, 0);
    match num::num(v, "numerator")? {
        N::F(f) if f.is_finite() && f.fract() != 0.0 => {
            let (mut m, mut e) = (f, 0);
            while m.fract() != 0.0 {
                m *= 2.0;
                e += 1;
            }
            Ok(Value::float(if numerator { m } else { 2f64.powi(e) }))
        }
        _ if numerator => Ok(v),
        N::F(_) => Ok(Value::float(1.0)),
        _ => Ok(Value::int_unchecked(1)),
    }
}

/// The simplest number in `[lo, hi]`: fewest digits in the continued
/// fraction (Stern-Brocot).
fn simplest(lo: f64, hi: f64) -> f64 {
    if lo > 0.0 {
        let fl = lo.floor();
        if fl == lo || fl < hi.floor() {
            if fl == lo { fl } else { fl + 1.0 }
        } else {
            fl + 1.0 / simplest(1.0 / (hi - fl), 1.0 / (lo - fl))
        }
    } else if hi < 0.0 {
        -simplest(-hi, -lo)
    } else {
        0.0
    }
}

fn rationalize(vm: &mut Vm, args: usize, _: usize) -> R {
    let (x, y) = (num::num(arg(vm, args, 0), "rationalize")?, num::num(arg(vm, args, 1), "rationalize")?);
    let r = simplest(x.f() - y.f().abs(), x.f() + y.f().abs());
    if x.is_exact() && y.is_exact() { exact(vm, Value::float(r), "rationalize") } else { Ok(Value::float(r)) }
}

fn float_args(vm: &Vm, args: usize, n: usize, who: &str) -> Result<Vec<f64>, Error> {
    (0..n).map(|i| num::num(arg(vm, args, i), who).map(|x| x.f())).collect()
}

/// `exit` and `emergency-exit`: a request to the host to end the program
/// with a status (an integer, `#t` for success or `#f` for failure). It
/// unwinds past every handler to the host, which decides what ending means.
fn exit(vm: &mut Vm, args: usize, n: usize) -> R {
    let code = match (n > 0).then(|| arg(vm, args, 0)) {
        None | Some(Value::TRUE) => 0,
        Some(Value::FALSE) => 1,
        Some(v) => int_arg(v, "exit")? as i32,
    };
    vm.flush();
    Err(Error::new(format!("exit {code}")).with_kind(crate::vm::ErrorKind::Exit(code)))
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
        "abs" 1 1 => |vm: &mut Vm, a, _| { let n = num::num(arg(vm, a, 0), "abs")?; Ok(num::abs(vm, n)) };
        "min" 1 _ => |vm: &mut Vm, a, n| (1..n).try_fold(arg(vm, a, 0), |m, i| {
            let x = arg(vm, a, i); Ok(if num::lt(x, m)? { x } else { m }) });
        "max" 1 _ => |vm: &mut Vm, a, n| (1..n).try_fold(arg(vm, a, 0), |m, i| {
            let x = arg(vm, a, i); Ok(if num::lt(m, x)? { x } else { m }) });
        "expt" 2 2 => expt;
        "sqrt" 1 1 => sqrt;
        "exact->inexact" 1 1 => |vm: &mut Vm, a, _| Ok(Value::float(num::num(arg(vm, a, 0), "exact->inexact")?.f()));
        "inexact" 1 1 => |vm: &mut Vm, a, _| Ok(Value::float(num::num(arg(vm, a, 0), "inexact")?.f()));
        "inexact->exact" 1 1 => |vm: &mut Vm, a, _| exact(vm, arg(vm, a, 0), "inexact->exact");
        "exact" 1 1 => |vm: &mut Vm, a, _| exact(vm, arg(vm, a, 0), "exact");
        "floor" 1 1 => |vm: &mut Vm, a, _| float_fn(vm, a, "floor", f64::floor);
        "ceiling" 1 1 => |vm: &mut Vm, a, _| float_fn(vm, a, "ceiling", f64::ceil);
        "round" 1 1 => |vm: &mut Vm, a, _| float_fn(vm, a, "round", f64::round_ties_even);
        "truncate" 1 1 => |vm: &mut Vm, a, _| float_fn(vm, a, "truncate", f64::trunc);
        "number?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::is_number(arg(vm, a, 0))));
        "integer?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::is_integer(arg(vm, a, 0))));
        "zero?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::num_eq(arg(vm, a, 0), Value::int_unchecked(0))?));
        "positive?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::lt(Value::int_unchecked(0), arg(vm, a, 0))?));
        "negative?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::lt(arg(vm, a, 0), Value::int_unchecked(0))?));
        "even?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(parity(vm, a, "even?")?));
        "odd?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(!parity(vm, a, "odd?")?));
        "number->string" 1 2 => number_to_string;
        "complex?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::is_number(arg(vm, a, 0))));
        "real?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(num::is_number(arg(vm, a, 0))));
        "rational?" 1 1 => |vm: &mut Vm, a, _| { let v = arg(vm, a, 0); Ok(Value::bool(num::is_number(v) && (!v.is_float() || v.as_float().is_finite()))) };
        "finite?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(match num::num(arg(vm, a, 0), "finite?")? { N::F(f) => f.is_finite(), _ => true }));
        "infinite?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(matches!(num::num(arg(vm, a, 0), "infinite?")?, N::F(f) if f.is_infinite())));
        "gcd" 0 _ => |vm: &mut Vm, a, n| gcd_lcm(vm, a, n, false);
        "lcm" 0 _ => |vm: &mut Vm, a, n| gcd_lcm(vm, a, n, true);
        "floor-quotient" 2 2 => floor_quotient;
        "floor-remainder" 2 2 => |vm: &mut Vm, a, _| num::modulo(vm, arg(vm, a, 0), arg(vm, a, 1));
        "truncate-quotient" 2 2 => |vm: &mut Vm, a, _| num::quotient(vm, arg(vm, a, 0), arg(vm, a, 1));
        "truncate-remainder" 2 2 => |vm: &mut Vm, a, _| num::remainder(vm, arg(vm, a, 0), arg(vm, a, 1));
        "%exact-integer-sqrt" 1 1 => isqrt;
        "numerator" 1 1 => |vm: &mut Vm, a, _| ratio_part(vm, a, true);
        "denominator" 1 1 => |vm: &mut Vm, a, _| ratio_part(vm, a, false);
        "rationalize" 2 2 => rationalize;
        "exp" 1 1 => |vm: &mut Vm, a, _| Ok(Value::float(float_args(vm, a, 1, "exp")?[0].exp()));
        "log" 1 2 => |vm: &mut Vm, a, n| { let x = float_args(vm, a, n, "log")?; Ok(Value::float(if n == 2 { x[0].ln() / x[1].ln() } else { x[0].ln() })) };
        "sin" 1 1 => |vm: &mut Vm, a, _| Ok(Value::float(float_args(vm, a, 1, "sin")?[0].sin()));
        "cos" 1 1 => |vm: &mut Vm, a, _| Ok(Value::float(float_args(vm, a, 1, "cos")?[0].cos()));
        "tan" 1 1 => |vm: &mut Vm, a, _| Ok(Value::float(float_args(vm, a, 1, "tan")?[0].tan()));
        "asin" 1 1 => |vm: &mut Vm, a, _| Ok(Value::float(float_args(vm, a, 1, "asin")?[0].asin()));
        "acos" 1 1 => |vm: &mut Vm, a, _| Ok(Value::float(float_args(vm, a, 1, "acos")?[0].acos()));
        "atan" 1 2 => |vm: &mut Vm, a, n| { let x = float_args(vm, a, n, "atan")?; Ok(Value::float(if n == 2 { x[0].atan2(x[1]) } else { x[0].atan() })) };
        "string->number" 1 2 => string_to_number;

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
        "member" 2 3 => |vm: &mut Vm, a, n| if n == 3 { find_with(vm, a, false) } else { mem_generic(vm, a, equal) };
        "assq" 2 2 => |vm: &mut Vm, a, _| ass_generic(vm, a, |x, y| x == y);
        "assv" 2 2 => |vm: &mut Vm, a, _| ass_generic(vm, a, eqv);
        "assoc" 2 3 => |vm: &mut Vm, a, n| if n == 3 { find_with(vm, a, true) } else { ass_generic(vm, a, equal) };

        "eq?" 2 2 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0) == arg(vm, a, 1)));
        "eqv?" 2 2 => |vm: &mut Vm, a, _| Ok(Value::bool(eqv(arg(vm, a, 0), arg(vm, a, 1))));
        "equal?" 2 2 => |vm: &mut Vm, a, _| Ok(Value::bool(equal(arg(vm, a, 0), arg(vm, a, 1))));
        "not" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0).is_false()));
        "boolean?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(matches!(arg(vm, a, 0), Value::TRUE | Value::FALSE)));
        "symbol?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0).is_symbol()));
        "string?" 1 1 => |vm: &mut Vm, a, _| type_pred(vm, a, Kind::String);
        "char?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0).is_char()));
        "vector?" 1 1 => |vm: &mut Vm, a, _| type_pred(vm, a, Kind::Vector);
        "procedure?" 1 1 => |vm: &mut Vm, a, _| { let v = arg(vm, a, 0); Ok(Value::bool(vm.is_procedure(v))) };

        "make-vector" 1 2 => make_vector;
        "vector" 0 _ => vector;
        "vector-length" 1 1 => vector_length;
        "vector-ref" 2 2 => vector_ref;
        "vector-set!" 3 3 => vector_set;
        "vector->list" 1 3 => |vm: &mut Vm, a, n| { let items = vector_range(vm, a, n, 1, "vector->list")?; Ok(vm.make_list(&items)) };
        "vector->string" 1 3 => |vm: &mut Vm, a, n| {
            let s = vector_range(vm, a, n, 1, "vector->string")?.into_iter().map(|c| char_arg(c, "vector->string")).collect::<Result<String, _>>()?;
            Ok(vm.make_string(s.as_bytes())) };
        "string->vector" 1 3 => |vm: &mut Vm, a, n| { let items: Vec<Value> = string_range(vm, a, n, "string->vector")?.into_iter().map(Value::char).collect(); Ok(vm.make_vector(&items)) };
        "vector-copy!" 3 5 => vector_copy_into;
        "vector-fill!" 2 4 => vector_fill_range;
        "list->vector" 1 1 => list_to_vector;

        "string-length" 1 1 => string_length;
        "string-ref" 2 2 => string_ref;
        "substring" 2 3 => substring;
        "string-append" 0 _ => string_append;
        "string=?" 1 _ => |vm: &mut Vm, a, n| string_cmp(vm, a, n, |o| o.is_eq(), "string=?");
        "string<?" 1 _ => |vm: &mut Vm, a, n| string_cmp(vm, a, n, |o| o.is_lt(), "string<?");
        "string->list" 1 3 => |vm: &mut Vm, a, n| { let items: Vec<Value> = string_range(vm, a, n, "string->list")?.into_iter().map(Value::char).collect(); Ok(vm.make_list(&items)) };
        "list->string" 1 1 => list_to_string;
        "make-string" 1 2 => make_string;
        "string-set!" 3 3 => string_set;
        "string-fill!" 2 4 => string_fill;
        "string-copy!" 3 5 => string_copy_into;
        "string-copy" 1 3 => |vm: &mut Vm, a, n| { let s: String = string_range(vm, a, n, "string-copy")?.into_iter().collect(); Ok(vm.make_string(s.as_bytes())) };
        "string->symbol" 1 1 => string_to_symbol;
        "symbol->string" 1 1 => symbol_to_string;
        "string-prefix?" 2 2 => string_prefix;
        "char=?" 1 _ => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char=?", false, |o| o.is_eq());
        "char<?" 1 _ => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char<?", false, |o| o.is_lt());
        "char>?" 1 _ => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char>?", false, |o| o.is_gt());
        "char<=?" 1 _ => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char<=?", false, |o| o.is_le());
        "char>=?" 1 _ => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char>=?", false, |o| o.is_ge());
        "char-ci=?" 1 _ => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char-ci=?", true, |o| o.is_eq());
        "char-ci<?" 1 _ => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char-ci<?", true, |o| o.is_lt());
        "char-ci>?" 1 _ => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char-ci>?", true, |o| o.is_gt());
        "char-ci<=?" 1 _ => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char-ci<=?", true, |o| o.is_le());
        "char-ci>=?" 1 _ => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char-ci>=?", true, |o| o.is_ge());
        "char-foldcase" 1 1 => |vm: &mut Vm, a, _| Ok(Value::char(fold_char(char_arg(arg(vm, a, 0), "char-foldcase")?)));
        "digit-value" 1 1 => |vm: &mut Vm, a, _| Ok(digit_value(char_arg(arg(vm, a, 0), "digit-value")?).map_or(Value::FALSE, Value::int_unchecked));
        "string-foldcase" 1 1 => |vm: &mut Vm, a, _| { let s = fold_string(str_arg(arg(vm, a, 0), "string-foldcase")?); Ok(vm.make_string(s.as_bytes())) };
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

        "make-hash-table" 0 2 => |vm: &mut Vm, a, n| make_table(vm, a, n, false);
        "make-weak-hash-table" 0 1 => |vm: &mut Vm, a, n| make_table(vm, a, n, true);
        "hash-by-identity" 1 2 => |vm: &mut Vm, a, _| { let h = hash_key(vm, arg(vm, a, 0), Equiv::Eq); Ok(Value::int_unchecked((h >> 17) as i64)) };
        "hash" 1 2 => |vm: &mut Vm, a, _| { let h = hash_key(vm, arg(vm, a, 0), Equiv::Equal); Ok(Value::int_unchecked((h >> 17) as i64)) };
        "hash-table-ref" 2 3 => hash_ref;
        "hash-table-set!" 3 3 => hash_set;
        "hash-table-count" 1 1 => hash_count;
        "hash-table-contains?" 2 2 => hash_contains;
        "hash-table-delete!" 2 2 => hash_delete;

        "display" 1 2 => |vm: &mut Vm, a, n| output(vm, a, n, false, false);
        "write" 1 2 => |vm: &mut Vm, a, n| output(vm, a, n, true, false);
        "displayln" 0 2 => |vm: &mut Vm, a, n| output(vm, a, n, false, true);
        "newline" 0 1 => |vm: &mut Vm, a, n| { let p = (n > 0).then(|| arg(vm, a, 0)); crate::stdlib::write_out(vm, p, "\n")?; Ok(Value::VOID) };
        "error" 1 _ => error;
        "void" 0 _ => |_: &mut Vm, _, _| Ok(Value::VOID);
        "gc-stats" 0 0 => gc_stats;
        "collect-garbage" 0 1 => collect_garbage;
    }
    vm.requiring(Capability::Files, |vm| {
        natives! { vm;
            "read-lines" 1 1 => read_lines;
            "file->lines" 1 1 => read_lines;
        }
    });
    vm.requiring(Capability::HostControl, |vm| {
        natives! { vm;
            "exit" 0 1 => exit;
            "emergency-exit" 0 1 => exit;
        }
    });
    crate::stdlib::install(vm);
    crate::ports::install(vm);
    crate::tasks::install(vm);
}
