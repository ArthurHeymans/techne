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
    vm::{Capability, Error, Native, NativeImpl, SpecialObj, Vm, init_string},
};

type R = Result<Value, Error>;

pub fn type_error(who: &str, expected: &str, got: Value) -> Error {
    Error::new(format!("{who}: expected {expected}, got {}", brief(got)))
}

pub fn index_error(who: &str, v: Value, k: Value) -> Error {
    Error::new(format!("{who}: bad index {} for {}", brief(k), brief(v)))
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
    /// the old generation (which never collects), once admitted
    /// (`Vm::admit`).
    pub fn new(vm: &mut Vm, words: usize) -> Result<Bulk, Error> {
        if words >= vm.heap.nursery_capacity() / 2 {
            vm.admit_items(words, 8, 0)?;
        }
        Ok(Bulk::reserve(vm, words))
    }

    /// `new` without admitting: for what was admitted before.
    pub fn reserve(vm: &mut Vm, words: usize) -> Bulk {
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
    pub fn pair(&mut self, vm: &mut Vm, car: Value, cdr: Value) -> Value {
        // In the old generation, long lists of old or immediate values need
        // no remembering (and the next minor collection no scanning).
        let p = if self.old { vm.heap.alloc_old_holding(3, &[car, cdr]) } else { self.take(vm, 3) };
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
    // Reject cycles before allocating an ever-growing result.
    let len = list_len(l, "list conversion").ok()?;
    let mut out = Vec::with_capacity(len);
    let mut l = l;
    while is_kind(l, Kind::Pair) {
        out.push(unsafe { field(l.as_ptr(), 0) });
        l = unsafe { field(l.as_ptr(), 1) };
    }
    (l == Value::NIL).then_some(out)
}

/// Human-readable description of a raised object: its irritants each
/// brief, and all of them cut short after four times as much (one object
/// many irritants refer to would make far more text than it holds).
pub fn condition_message(vm: &Vm, v: Value) -> String {
    if let Some((msg, irritants)) = error_object_parts(vm, v) {
        let msg = unsafe { str_bytes(msg.as_ptr()) };
        let mut s = String::from_utf8_lossy(&msg[..msg.len().min(BRIEF)]).into_owned();
        if msg.len() > BRIEF {
            s.push_str("...");
        }
        let end = s.len() + 4 * BRIEF;
        for i in list_items(irritants) {
            if s.len() > end {
                s.push_str(" ...");
                break;
            }
            s.push(' ');
            s.push_str(&brief(i));
        }
        s
    } else {
        format!("uncaught exception: {}", brief(v))
    }
}

/// Message and irritants of an error object.
pub fn error_object_parts(vm: &Vm, v: Value) -> Option<(Value, Value)> {
    if !is_kind(v, Kind::Record)
        || unsafe { len_of(v.as_ptr()) } != 4
        || unsafe { field(v.as_ptr(), 0) } != vm.special(SpecialObj::ErrorRtd)
    {
        return None;
    }
    let (message, irritants) = unsafe { (field(v.as_ptr(), 1), field(v.as_ptr(), 2)) };
    // Records can be constructed or mutated through Lisp's internal APIs.
    // A descriptor alone does not make these payloads safe to interpret.
    (is_kind(message, Kind::String) && list_len(irritants, "error irritants").is_ok()).then_some((message, irritants))
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

pub(crate) fn string_arg<'a>(v: Value, who: &str) -> Result<&'a [u8], Error> {
    if is_kind(v, Kind::String) { Ok(unsafe { str_bytes(v.as_ptr()) }) } else { Err(type_error(who, "string", v)) }
}

pub(crate) fn str_arg<'a>(v: Value, who: &str) -> Result<&'a str, Error> {
    Ok(unsafe { std::str::from_utf8_unchecked(string_arg(v, who)?) })
}

pub(crate) fn int_arg(v: Value, who: &str) -> Result<i64, Error> {
    if v.is_int() { Ok(v.as_int()) } else { num::integer(v, who) }
}

pub(crate) fn index_arg(v: Value, who: &str) -> Result<usize, Error> {
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
    print_within(out, v, write, sharing, usize::MAX);
}

/// `print_with`, stopping once `out` holds about `limit` bytes (a
/// datum's shared parts can make far more text than the datum holds, and
/// `Sharing::None` none that ends); whether all of `v` was printed.
pub fn print_within(out: &mut String, v: Value, write: bool, sharing: Sharing, limit: usize) -> bool {
    let labels = if sharing == Sharing::None { FxHashMap::default() } else { labeled(v, sharing == Sharing::All) };
    let mut printer = Printer { out, write, labels, next: 0, limit, over: false };
    printer.print(v);
    !printer.over
}

/// Bytes of `brief`.
const BRIEF: usize = 4096;

/// `repr` cut short after about 4 KB, with `...`: for messages.
pub fn brief(v: Value) -> String {
    let mut s = String::new();
    if !print_within(&mut s, v, true, Sharing::None, BRIEF) {
        s.push_str("...");
    }
    s
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
    /// Where it stops (`print_within`), and whether it did.
    limit: usize,
    over: bool,
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
        crate::nested(|| self.print_step(v));
        // Escapes, a symbol's name: what an atom printed past the limit goes.
        if self.out.len() > self.limit {
            let end = self.out.floor_char_boundary(self.limit);
            self.out.truncate(end);
            self.over = true;
        }
    }

    /// The bytes left before the limit; none marks the printer over it.
    fn left(&mut self) -> usize {
        let left = self.limit.saturating_sub(self.out.len());
        self.over |= left == 0;
        left
    }

    fn print_step(&mut self, v: Value) {
        use std::fmt::Write as _;
        if self.left() == 0 {
            return;
        }
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
        } else if v.is_cursor() {
            out.push_str(&format!("#<string-cursor {}>", v.as_cursor()));
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
                    while is_kind(l, Kind::Pair) && !self.labels.contains_key(&l.bits()) && !self.over {
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
                        if self.over {
                            break;
                        }
                        if i > 0 {
                            self.out.push(' ');
                        }
                        self.print(x);
                    }
                    self.out.push(')');
                }
                k if k == Kind::String as u8 => {
                    let s = unsafe { std::str::from_utf8_unchecked(str_bytes(p)) };
                    // No more than what fits: a long string's text is not copied.
                    let left = self.limit.saturating_sub(out.len());
                    let s = if s.len() > left {
                        self.over = true;
                        &s[..s.floor_char_boundary(left)]
                    } else {
                        s
                    };
                    if write {
                        out.push_str(&reader::string_repr(s));
                    } else {
                        out.push_str(s);
                    }
                }
                k if k == Kind::Bytevector as u8 => {
                    // Written byte by byte, up to the limit: a long one's
                    // text is never made whole.
                    out.push_str("#u8(");
                    for (i, b) in unsafe { str_bytes(p) }.iter().enumerate() {
                        if out.len() > self.limit {
                            self.over = true;
                            return;
                        }
                        if i > 0 {
                            out.push(' ');
                        }
                        let _ = write!(out, "{b}");
                    }
                    out.push(')');
                }
                k if k == Kind::BigInt as u8 => {
                    let n = num::heap_int(Value::ptr(p));
                    // A decimal digit takes more than three bits.
                    if n.bits() / 3 > self.limit.saturating_sub(out.len()) as u64 {
                        self.over = true;
                        return;
                    }
                    let _ = write!(out, "{}", num::to_string_radix(&n, 10));
                }
                k if k == Kind::Ratio as u8 || k == Kind::Complex as u8 => {
                    if let Ok(n) = num::num(v, "write") {
                        // As for a big integer: no digits past the limit.
                        if n.bits() / 3 > self.limit.saturating_sub(out.len()) as u64 {
                            self.over = true;
                            return;
                        }
                        out.push_str(&num::to_string_radix(&n, 10));
                    }
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
                        if self.over {
                            break;
                        }
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
    if n == 0 {
        crate::stdlib::write_out(vm, None, if newline { "\n" } else { "" })?;
    } else {
        crate::ports::display_args(vm, args, (n > 1).then_some(args + 1), write, newline)?;
    }
    Ok(Value::VOID)
}

// ----- equality and hashing -----

pub fn eqv(a: Value, b: Value) -> bool {
    if a == b {
        return true;
    }
    if !a.is_ptr() || !b.is_ptr() {
        return false;
    }
    let (p, q) = (a.as_ptr(), b.as_ptr());
    let k = unsafe { kind_of(p) };
    if k != unsafe { kind_of(q) } {
        return false;
    }
    match k {
        k if k == Kind::BigInt as u8 => unsafe { bignum_key(a) == bignum_key(b) },
        // Ratios are in lowest terms: equal ones have equal parts. Complex
        // numbers are equal when their parts are.
        k if k == Kind::Ratio as u8 || k == Kind::Complex as u8 => unsafe {
            eqv(field(p, 0), field(q, 0)) && eqv(field(p, 1), field(q, 1))
        },
        _ => false,
    }
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
        if k == Kind::String as u8 || k == Kind::Bytevector as u8 {
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
                if (k == Kind::String as u8 || k == Kind::Bytevector as u8) && str_bytes(p) == str_bytes(q) {
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
// A table's fields: live count, slots, used slots (live and deleted), its
// mode, and its equivalence and hash procedures. The mode is how keys are
// compared, `eq?`, `eqv?` or `equal?` natively or else by calling the
// equivalence and hash procedures, and whether keys are weak. Slots are
// key/value pairs, open addressing with linear probing; `EMPTY` keys end a
// probe and `UNDEFINED` ones are deleted. Weak tables keep their slots in
// an ephemeron object, whose entries the collector clears when their key
// dies, so their live count is counted on demand.

#[derive(Clone, Copy, PartialEq)]
enum Equiv {
    Eq = 0,
    Eqv = 1,
    Equal = 2,
    /// The table's own procedures (fields 4 and 5), called from here.
    Custom = 3,
}

const TABLE_FIELDS: usize = 6;

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
        Equiv::Eqv | Equiv::Equal if k == Kind::Ratio as u8 || k == Kind::Complex as u8 => {
            k.hash(h);
            hash_into(vm, unsafe { field(p, 0) }, equiv, h, budget);
            hash_into(vm, unsafe { field(p, 1) }, equiv, h, budget);
        }
        Equiv::Equal if k == Kind::String as u8 || k == Kind::Bytevector as u8 => unsafe { str_bytes(p) }.hash(h),
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
                Equiv::Eq | Equiv::Custom => false,
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
        2 => Equiv::Equal,
        _ => Equiv::Custom,
    };
    (equiv, mode & WEAK != 0)
}

/// The slot of the key argument (at 1) in the table argument (at 0), as
/// `probe` finds it, and the table's slots. Native equivalences hash
/// without allocating (at most setting an identity-hash flag); a table's
/// own procedures are called, so the slots are read after them.
#[inline(always)]
fn find(vm: &mut Vm, args: usize, who: &str) -> Result<(*mut u64, usize), Error> {
    let t = table_arg(arg(vm, args, 0), who)?;
    let (equiv, _) = table_mode(t);
    if equiv == Equiv::Custom {
        let i = find_custom(vm, args)?;
        return Ok((unsafe { field(arg(vm, args, 0).as_ptr(), 1).as_ptr() }, i));
    }
    let key = arg(vm, args, 1);
    let hash = hash_key(vm, key, equiv);
    let slots = unsafe { field(t, 1).as_ptr() };
    Ok((slots, unsafe { probe(slots, key, hash, equiv) }))
}

/// `find` for a table with its own procedures. Calling them can allocate,
/// which moves the table, its slots and the key, so they are read again
/// after every call; a table only grows, so a slot index stays in range.
fn find_custom(vm: &mut Vm, args: usize) -> Result<usize, Error> {
    let hash = custom_hash(vm, unsafe { field(arg(vm, args, 0).as_ptr(), 5) }, arg(vm, args, 1))?;
    let mut i = hash as usize;
    let mut tomb = None;
    loop {
        let t = arg(vm, args, 0).as_ptr();
        let slots = unsafe { field(t, 1).as_ptr() };
        i &= unsafe { len_of(slots) } / 2 - 1;
        let (k, key) = (unsafe { field(slots, 2 * i) }, arg(vm, args, 1));
        if k == Value::EMPTY {
            return Ok(tomb.unwrap_or(i));
        }
        if k == Value::UNDEFINED {
            tomb.get_or_insert(i);
        } else if k == key || vm.call(unsafe { field(t, 4) }, &[key, k])?.is_truthy() {
            return Ok(i);
        }
        i += 1;
    }
}

/// The slot hash of `key` by a table's hash procedure `f`, whatever number
/// (or other value) it returns.
fn custom_hash(vm: &mut Vm, f: Value, key: Value) -> Result<u64, Error> {
    let h = vm.call(f, &[key])?;
    Ok(hash_key(vm, h, Equiv::Eqv))
}

/// The first free slot from `hash` on, for a key not in the table.
unsafe fn free_slot(slots: *mut u64, hash: u64) -> usize {
    unsafe {
        let cap = len_of(slots) / 2;
        let mut i = (hash as usize) & (cap - 1);
        while !is_free(field(slots, 2 * i)) {
            i = (i + 1) & (cap - 1);
        }
        i
    }
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

/// `(make-hash-table [equivalence] [hash])`, `(make-weak-hash-table
/// [equivalence])`. The builtin `eq?`, `eqv?`, `equal?`, `string=?` and
/// `char=?` compare natively, and a hash procedure given with them goes
/// unused; any other equivalence is called, with HASH (by default
/// `string-ci-hash` for `string-ci=?`, else `hash`). Strong tables compare
/// with `equal?` by default, weak ones with `eq?`.
fn make_table(vm: &mut Vm, args: usize, n: usize, weak: bool) -> R {
    let who = if weak { "make-weak-hash-table" } else { "make-hash-table" };
    let default_equiv = if weak { "eq?" } else { "equal?" };
    let f = if n > 0 { arg(vm, args, 0) } else { vm.get_global(default_equiv).unwrap_or(Value::FALSE) };
    // The builtin procedures themselves, whatever they are bound to now.
    let name = if f.is_native() { vm.procedure_name(f).unwrap_or_default() } else { "".into() };
    let equiv = match &*name {
        "eq?" => Equiv::Eq,
        "eqv?" | "char=?" => Equiv::Eqv,
        "equal?" | "string=?" => Equiv::Equal,
        _ if vm.is_procedure(f) => Equiv::Custom,
        _ => return Err(type_error(who, "procedure", f)),
    };
    if n > 1 && !vm.is_procedure(arg(vm, args, 1)) {
        return Err(type_error(who, "hash procedure", arg(vm, args, 1)));
    }
    if weak && equiv == Equiv::Custom {
        return Err(type_error(who, "the builtin eq?, eqv?, equal?, string=? or char=?", f));
    }
    let default_hash = match (equiv, &*name) {
        (Equiv::Eq, _) => "hash-by-identity",
        (_, "string-ci=?") => "string-ci-hash",
        _ => "hash",
    };
    let cap = 8;
    let mut b = Bulk::new(vm, 1 + TABLE_FIELDS + 1 + 2 * cap)?;
    let t = b.take(vm, 1 + TABLE_FIELDS);
    let slots = new_slots(&mut b, vm, cap, weak);
    // Read after allocating.
    let equiv_proc = if n > 0 { arg(vm, args, 0) } else { vm.get_global(default_equiv).unwrap_or(Value::FALSE) };
    let hash_proc = if n > 1 { arg(vm, args, 1) } else { vm.get_global(default_hash).unwrap_or(Value::FALSE) };
    unsafe {
        *t = header(Kind::Table, TABLE_FIELDS, 0);
        set_field(t, 0, Value::int_unchecked(0));
        set_field(t, 1, Value::ptr(slots));
        set_field(t, 2, Value::int_unchecked(0));
        set_field(t, 3, Value::int_unchecked(equiv as i64 | if weak { WEAK } else { 0 }));
        set_field(t, 4, equiv_proc);
        set_field(t, 5, hash_proc);
    }
    Ok(Value::ptr(t))
}

/// `(hash-table-copy table [mutable?])`: a table with the same procedures
/// and entries; tables are always mutable.
fn hash_copy(vm: &mut Vm, args: usize, _: usize) -> R {
    let t = table_arg(arg(vm, args, 0), "hash-table-copy")?;
    let words = 1 + unsafe { len_of(field(t, 1).as_ptr()) };
    let mut b = Bulk::new(vm, 1 + TABLE_FIELDS + words)?;
    let (copy, slots) = (b.take(vm, 1 + TABLE_FIELDS), b.take(vm, words));
    unsafe {
        let t = arg(vm, args, 0).as_ptr();
        std::ptr::copy_nonoverlapping(t, copy, 1 + TABLE_FIELDS);
        std::ptr::copy_nonoverlapping(field(t, 1).as_ptr(), slots, words);
        // A copied header would carry the original's identity-hash flags.
        *copy = header(Kind::Table, TABLE_FIELDS, 0);
        *slots = header(if table_mode(t).1 { Kind::Ephemerons } else { Kind::Vector }, words - 1, 0);
        set_field(copy, 1, Value::ptr(slots));
    }
    Ok(Value::ptr(copy))
}

/// `hash-table-equivalence-function` (field 4) or
/// `hash-table-hash-function` (field 5).
fn table_proc(vm: &mut Vm, args: usize, i: usize, who: &str) -> R {
    Ok(unsafe { field(table_arg(arg(vm, args, 0), who)?, i) })
}

fn hash_delete(vm: &mut Vm, args: usize, _: usize) -> R {
    let (slots, i) = find(vm, args, "hash-table-delete!")?;
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

fn hash_ref(vm: &mut Vm, args: usize, _: usize) -> R {
    let (slots, i) = find(vm, args, "hash-table-ref/default")?;
    if !is_free(unsafe { field(slots, 2 * i) }) {
        return Ok(unsafe { field(slots, 2 * i + 1) });
    }
    Ok(arg(vm, args, 2))
}

fn hash_contains(vm: &mut Vm, args: usize, _: usize) -> R {
    let (slots, i) = find(vm, args, "hash-table-contains?")?;
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
            // A table's own hash procedure runs before anything moves here.
            let mut hashes = Vec::new();
            if equiv == Equiv::Custom {
                for i in 0..cap {
                    let t = arg(vm, args, 0).as_ptr();
                    let k = field(field(t, 1).as_ptr(), 2 * i);
                    if !is_free(k) {
                        hashes.push(custom_hash(vm, field(t, 5), k)?);
                    }
                }
            }
            let mut b = Bulk::new(vm, 1 + 2 * new_cap)?;
            let new = new_slots(&mut b, vm, new_cap, weak);
            let t = arg(vm, args, 0).as_ptr();
            let old = field(t, 1).as_ptr();
            if len_of(old) != 2 * cap || equiv == Equiv::Custom && field(t, 0).as_int() as usize != hashes.len() {
                return Err(Error::new("hash-table-set!: the hash procedure changed the table"));
            }
            let mut live = 0;
            for i in 0..cap {
                let k = field(old, 2 * i);
                if !is_free(k) {
                    let hash = if equiv == Equiv::Custom { hashes[live as usize] } else { hash_key(vm, k, equiv) };
                    let j = free_slot(new, hash);
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
        let (slots, i) = find(vm, args, "hash-table-set!")?;
        let t = arg(vm, args, 0).as_ptr();
        let (key, value) = (arg(vm, args, 1), arg(vm, args, 2));
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

fn string_hash(s: &str) -> u64 {
    let mut h = FxHasher::default();
    s.as_bytes().hash(&mut h);
    h.finish()
}

/// A hash as a non-negative fixnum, below the optional bound at 1.
fn bounded_hash(vm: &mut Vm, args: usize, n: usize, h: u64, who: &str) -> R {
    let h = h >> 17;
    if n < 2 {
        return Ok(Value::int_unchecked(h as i64));
    }
    let bound = arg(vm, args, 1);
    match num::num(bound, who) {
        Ok(N::I(b)) if b > 0 && bound.is_int() => Ok(Value::int_unchecked((h % b as u64) as i64)),
        // A bignum bound is above every hash.
        Ok(N::B(b)) if b.sign() == num_bigint::Sign::Plus => Ok(Value::int_unchecked(h as i64)),
        _ => Err(type_error(who, "positive exact integer", bound)),
    }
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

/// `min` or `max` (`before` orders the one to keep first); inexact if any
/// argument is.
fn extremum(vm: &mut Vm, args: usize, n: usize, before: fn(Value, Value) -> Result<bool, Error>) -> R {
    let best = (1..n).try_fold(arg(vm, args, 0), |m, i| {
        let x = arg(vm, args, i);
        Ok::<_, Error>(if before(x, m)? { x } else { m })
    })?;
    let inexact = (0..n).any(|i| arg(vm, args, i).is_float());
    Ok(if inexact { Value::float(num::num(best, "min")?.f()) } else { best })
}

fn chain(vm: &mut Vm, args: usize, n: usize, cmp: fn(Value, Value) -> Result<bool, Error>) -> R {
    for i in 1..n {
        if !cmp(arg(vm, args, i - 1), arg(vm, args, i))? {
            return Ok(Value::FALSE);
        }
    }
    Ok(Value::TRUE)
}

/// `floor`, `ceiling`, `truncate` or `round`: of a float a float, of a
/// ratio an exact integer.
fn rounding(vm: &mut Vm, args: usize, who: &str, how: num::Rounding, f: fn(f64) -> f64) -> R {
    let v = arg(vm, args, 0);
    match num::real(v, who)? {
        N::F(x) => Ok(Value::float(f(x))),
        N::R(r) => Ok(num::make_integer(vm, &num::round_ratio(&r, how))),
        _ => Ok(v),
    }
}

fn number_to_string(vm: &mut Vm, args: usize, n: usize) -> R {
    let v = arg(vm, args, 0);
    let radix = if n > 1 { int_arg(arg(vm, args, 1), "number->string")? } else { 10 };
    if v.is_int() && radix == 10 {
        let s = v.as_int().to_string();
        return Ok(vm.make_string(&s));
    }
    let n = num::num(v, "number->string")?;
    // A digit takes at least a bit; then the string made of them.
    vm.admit_items(usize::try_from(n.bits()).unwrap_or(usize::MAX), 2, 64)?;
    let v = arg(vm, args, 0);
    let s = match radix {
        _ if !n.is_exact() => repr(v),
        2..=36 => num::to_string_radix(&n, radix as u32),
        r => return Err(Error::new(format!("number->string: unsupported radix {r}"))),
    };
    Ok(vm.make_string(&s))
}

fn exact(vm: &mut Vm, v: Value, who: &str) -> R {
    let n = num::exact(&num::num(v, who)?, who)?;
    Ok(num::from_n(vm, n))
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
    // A digit takes at most four bits: the number, its temporaries.
    num::admit_result(vm, 4 * s.len() as u64)?;
    let s = str_arg(arg(vm, args, 0), "string->number")?;
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

/// `v` as a pair or vector to change: of `kind`, and not a literal.
fn changeable(v: Value, kind: Kind, who: &str) -> Result<*mut u64, Error> {
    if heap::is_changeable(v, kind) {
        return Ok(v.as_ptr());
    }
    let what = if kind == Kind::Pair { "pair" } else { "vector" };
    if is_kind(v, kind) {
        return Err(Error::new(format!("{who}: {what} literals cannot be changed")));
    }
    Err(type_error(who, what, v))
}

/// Why `vector-set!` of `v` at `k` failed.
pub fn vector_set_error(v: Value, k: Value) -> Error {
    match changeable(v, Kind::Vector, "vector-set!") {
        Err(e) => e,
        Ok(_) => index_error("vector-set!", v, k),
    }
}

fn set_pair(vm: &mut Vm, args: usize, i: usize, who: &str) -> R {
    let (p, v) = (arg(vm, args, 0), arg(vm, args, 1));
    changeable(p, Kind::Pair, who)?;
    unsafe { set_field(p.as_ptr(), i, v) };
    vm.write_barrier(p.as_ptr(), v);
    Ok(Value::VOID)
}

fn list(vm: &mut Vm, args: usize, n: usize) -> R {
    let mut b = Bulk::new(vm, 3 * n)?;
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
    let mut b = Bulk::new(vm, 3 * len)?;
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
    // Pairs, and the items gathered first.
    vm.admit_items(total, 32, 0)?;
    let mut b = Bulk::new(vm, 3 * total)?;
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

/// The first pair of list `l` whose car satisfies `hit`, or #f; an error
/// for an improper or circular list without one.
fn find_pair(l: Value, who: &str, hit: impl Fn(Value) -> bool) -> R {
    let next = |v: Value| unsafe { field(v.as_ptr(), 1) };
    let (mut fast, mut slow, mut n) = (l, l, 0usize);
    while is_kind(fast, Kind::Pair) {
        if hit(unsafe { field(fast.as_ptr(), 0) }) {
            return Ok(fast);
        }
        fast = next(fast);
        n += 1;
        if n % 2 == 0 {
            slow = next(slow);
            if fast == slow {
                return Err(type_error(who, "proper list", l));
            }
        }
    }
    if fast == Value::NIL { Ok(Value::FALSE) } else { Err(type_error(who, "proper list", l)) }
}

fn mem_generic(vm: &mut Vm, args: usize, who: &str, eq: fn(Value, Value) -> bool) -> R {
    let x = arg(vm, args, 0);
    find_pair(arg(vm, args, 1), who, |item| eq(x, item))
}

fn ass_generic(vm: &mut Vm, args: usize, who: &str, eq: fn(Value, Value) -> bool) -> R {
    let x = arg(vm, args, 0);
    let pair = find_pair(arg(vm, args, 1), who, |e| is_kind(e, Kind::Pair) && eq(x, unsafe { field(e.as_ptr(), 0) }))?;
    Ok(if pair.is_truthy() { unsafe { field(pair.as_ptr(), 0) } } else { pair })
}

/// `member` or `assoc` with the comparison procedure at argument 2.
fn find_with(vm: &mut Vm, args: usize, assoc: bool) -> R {
    // The walk calls Scheme, which may move the pairs: check the list first.
    list_len(arg(vm, args, 1), if assoc { "assoc" } else { "member" })?;
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
    vm.admit_items(len, 8, 8)?;
    let p = Bulk::new(vm, 1 + len)?.take(vm, 1 + len);
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
    let p = Bulk::new(vm, 1 + n)?.take(vm, 1 + n);
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
    let p = changeable(v, Kind::Vector, "vector-set!")?;
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
    let p = Bulk::new(vm, 1 + len)?.take(vm, 1 + len);
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
/// argument 0 by `text`, in place; literals are refused. When the new
/// characters take as many UTF-8 bytes as the old (always for ASCII) the
/// bytes change where they are; otherwise the string gets new bytes, as
/// another string it points to, and keeps its identity.
fn string_mutate(vm: &mut Vm, args: usize, start: usize, text: &str, who: &str) -> R {
    let s = arg(vm, args, 0);
    let bytes = string_arg(s, who)?;
    if unsafe { *s.as_ptr() } & heap::IMMUTABLE != 0 {
        return Err(Error::new(format!("{who}: string literals cannot be changed")));
    }
    let (a, b) = char_range(s, start, start + text.chars().count(), who)?;
    if b - a == text.len() {
        unsafe { heap::str_replace(s.as_ptr(), a, text.as_bytes()) };
        return Ok(Value::VOID);
    }
    let new = [&bytes[..a], text.as_bytes(), &bytes[b..]].concat();
    let replacement = vm.make_string(std::str::from_utf8(&new).expect("character-aligned string replacement"));
    // Allocating may have moved the string.
    let s = arg(vm, args, 0);
    unsafe { heap::str_redirect(s.as_ptr(), replacement) };
    vm.write_barrier(s.as_ptr(), replacement);
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
    vm.admit_items(b - a, 2 * c.len_utf8(), 0)?;
    string_mutate(vm, args, a, &c.to_string().repeat(b - a), "string-fill!")
}

/// `(string-copy! to at from [start end])`.
fn string_copy_into(vm: &mut Vm, args: usize, n: usize) -> R {
    let at = index_arg(arg(vm, args, 1), "string-copy!")?;
    // The characters, the text, and the string's new bytes.
    vm.admit_items(string_arg(arg(vm, args, 2), "string-copy!")?.len(), 12, 0)?;
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
    let mut offsets = s.char_indices().map(|(i, _)| i).chain(std::iter::once(s.len()));
    let a = offsets.nth(start);
    let b = if end > start { offsets.nth(end - start - 1) } else { a };
    match (a, b) {
        (Some(a), Some(b)) if start <= end => Ok((a, b)),
        _ => Err(Error::new(format!("{who}: range {start}..{end} out of bounds"))),
    }
}

fn substring(vm: &mut Vm, args: usize, n: usize) -> R {
    let v = arg(vm, args, 0);
    let start = index_arg(arg(vm, args, 1), "substring")?;
    let end = if n > 2 { index_arg(arg(vm, args, 2), "substring")? } else { string_length(vm, args, 1)?.as_int() as usize };
    let (a, b) = char_range(v, start, end, "substring")?;
    vm.admit_items(b - a, 1, 64)?;
    let p = vm.alloc(heap::string_words(b - a));
    unsafe {
        let src = str_bytes(arg(vm, args, 0).as_ptr());
        init_string(p, &src[a..b]);
    }
    Ok(Value::ptr(p))
}

fn string_append(vm: &mut Vm, args: usize, n: usize) -> R {
    let total = (0..n).map(|i| string_arg(arg(vm, args, i), "string-append").map(|s| s.len())).sum::<Result<usize, _>>()?;
    // The string, and its bytes gathered first.
    vm.admit_items(total, 2, 64)?;
    let p = Bulk::new(vm, heap::string_words(total))?.take(vm, heap::string_words(total));
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

/// Admit what changing the case of string argument 0 takes: up to three
/// times its bytes (`ß` to `SS`, ligatures), and the string made of them.
pub(crate) fn admit_changed(vm: &mut Vm, args: usize, who: &str) -> Result<(), Error> {
    vm.admit_items(string_arg(arg(vm, args, 0), who)?.len(), 6, 64)
}

fn list_to_string(vm: &mut Vm, args: usize, _: usize) -> R {
    // The items, the text of at most four bytes a character, and the string.
    let len = list_len(arg(vm, args, 0), "list->string")?;
    vm.admit_items(len, 16, 64)?;
    let l = arg(vm, args, 0);
    let items = list_values(l).ok_or_else(|| type_error("list->string", "proper list", l))?;
    let s = items.into_iter().map(|c| char_arg(c, "list->string")).collect::<Result<String, _>>()?;
    Ok(vm.make_string(&s))
}

fn make_string(vm: &mut Vm, args: usize, n: usize) -> R {
    let len = index_arg(arg(vm, args, 0), "make-string")?;
    let c = if n > 1 { char_arg(arg(vm, args, 1), "make-string")? } else { ' ' };
    // The text, then the string made of it.
    vm.admit_items(len, 2 * c.len_utf8(), 64)?;
    Ok(vm.make_string(&c.to_string().repeat(len)))
}

fn string_to_symbol(vm: &mut Vm, args: usize, _: usize) -> R {
    let name = str_arg(arg(vm, args, 0), "string->symbol")?;
    if let Some(id) = crate::reader::lookup(name) {
        return Ok(Value::symbol(id));
    }
    vm.admit(name.len() + crate::reader::SYMBOL_BYTES)?;
    // The collection `admit` may run moves the string.
    Ok(Value::symbol(intern(str_arg(arg(vm, args, 0), "string->symbol")?)))
}

fn symbol_to_string(vm: &mut Vm, args: usize, _: usize) -> R {
    let v = arg(vm, args, 0);
    if !v.is_symbol() {
        return Err(type_error("symbol->string", "symbol", v));
    }
    let name = symbol_name(v.as_symbol());
    Ok(vm.make_string(&name))
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
    // The text, the strings of its lines and the pairs (for lines of at
    // least eight bytes; shorter ones are admitted with the list).
    let text = vm.read_file_admitted(&path, 6)?;
    // For each line, short ones too: its slice, a string's header, a pair.
    vm.admit_items(text.lines().count(), 64, 0)?;
    let lines: Vec<&str> = text.lines().collect();
    let words: usize = lines.iter().map(|l| 3 + heap::string_words(l.len())).sum();
    let mut b = Bulk::new(vm, words)?;
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
    if !print_within(&mut msg, arg(vm, args, 0), false, Sharing::None, BRIEF) {
        msg.push_str("...");
    }
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
pub(crate) fn range_args(vm: &Vm, args: usize, n: usize, i: usize, len: usize, who: &str) -> Result<(usize, usize), Error> {
    let start = if n > i { index_arg(arg(vm, args, i), who)? } else { 0 };
    let end = if n > i + 1 { index_arg(arg(vm, args, i + 1), who)? } else { len };
    if start > end || end > len {
        return Err(Error::new(format!("{who}: range {start}..{end} out of bounds 0..{len}")));
    }
    Ok((start, end))
}

/// The characters of string argument 0 in the optional range at 1, once
/// admitted with `per` more bytes for each (what the caller makes of them).
fn string_range(vm: &mut Vm, args: usize, n: usize, who: &str, per: usize) -> Result<Vec<char>, Error> {
    let len = str_arg(arg(vm, args, 0), who)?.chars().count();
    let (a, b) = range_args(vm, args, n, 1, len, who)?;
    vm.admit_items(b - a, 4 + per, 0)?;
    Ok(str_arg(arg(vm, args, 0), who)?.chars().skip(a).take(b - a).collect())
}

/// The elements of vector argument 0 in the optional range at `i`, once
/// admitted with `per` more bytes for each.
fn vector_range(vm: &mut Vm, args: usize, n: usize, i: usize, who: &str, per: usize) -> Result<Vec<Value>, Error> {
    let len = unsafe { len_of(vector_arg(arg(vm, args, 0), who)?) };
    let (a, b) = range_args(vm, args, n, i, len, who)?;
    vm.admit_items(b - a, 8 + per, 0)?;
    let p = vector_arg(arg(vm, args, 0), who)?;
    Ok((a..b).map(|k| unsafe { field(p, k) }).collect())
}

/// `(vector-copy! to at from [start end])`, overlapping ranges included.
fn vector_copy_into(vm: &mut Vm, args: usize, n: usize) -> R {
    let to = changeable(arg(vm, args, 0), Kind::Vector, "vector-copy!")?;
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
    let (p, x) = (changeable(arg(vm, args, 0), Kind::Vector, "vector-fill!")?, arg(vm, args, 1));
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
/// The c[ad]r combinations of three and four letters, but `caddr` and
/// `cdddr`: natives cost nothing at startup, unlike prelude definitions.
/// (Those two, and the shorter ones, are in the prelude, where the compiler
/// inlines `car` and `cdr`.)
fn install_cxrs(vm: &mut Vm) {
    for len in 3..=4 {
        for bits in 0..1u32 << len {
            // Letters from the left; the rightmost is applied first.
            let path: String = (0..len).map(|i| if bits >> (len - 1 - i) & 1 == 0 { 'a' } else { 'd' }).collect();
            let name = format!("c{path}r");
            if name == "caddr" || name == "cdddr" {
                continue;
            }
            let who = name.clone();
            // `caddr` is (car (cdr (cdr PAIR))).
            let nested = path.chars().rev().fold("PAIR".to_string(), |acc, step| format!("(c{step}r {acc})"));
            let f = std::rc::Rc::new(move |vm: &mut Vm, a: usize, _: usize| {
                path.chars().rev().try_fold(arg(vm, a, 0), |x, step| {
                    if !is_kind(x, Kind::Pair) {
                        return Err(type_error(&who, "pair", x));
                    }
                    Ok(unsafe { field(x.as_ptr(), if step == 'a' { 0 } else { 1 }) })
                })
            });
            let doc: &'static crate::vm::NativeDoc = Box::leak(Box::new(crate::vm::NativeDoc {
                params: &["pair"],
                doc: Box::leak(format!("Return {nested}.").into_boxed_str()),
                file: concat!(env!("CARGO_MANIFEST_DIR"), "/src/builtins.rs"),
                line: line!(),
            }));
            vm.define_native(Native { name: name.as_str().into(), f: NativeImpl::Boxed(f), min: 1, max: Some(1), doc: Some(doc) });
        }
    }
}

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
                N::R(_) | N::F(_) | N::C(_) => Err(type_error(who, "integer", v)),
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
    use num_traits::Signed;
    use num_traits::Zero;
    // Before copying them: the arguments may be one number many times.
    num::admit_result(vm, (0..n).map(|i| num::value_bits(arg(vm, args, i))).sum())?;
    let (ints, inexact) = integers(vm, args, n, if lcm { "lcm" } else { "gcd" })?;
    let start = num_bigint::BigInt::from(if lcm { 1 } else { 0 });
    let r = ints.iter().fold(start, |acc, x| {
        if !lcm {
            num::gcd(&acc, x)
        } else if acc.is_zero() || x.is_zero() {
            0.into()
        } else {
            (&acc / num::gcd(&acc, x) * x).abs()
        }
    });
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

/// `numerator` or `denominator`: of an exact number, or of a float's exact
/// binary fraction (as floats).
fn ratio_part(vm: &mut Vm, args: usize, numerator: bool) -> R {
    let v = arg(vm, args, 0);
    match num::real(v, "numerator")? {
        N::R(r) => Ok(num::make_integer(vm, if numerator { r.numer() } else { r.denom() })),
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

/// The simplest number in `[lo, hi]` of floats: fewest digits in the
/// continued fraction (Stern-Brocot).
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
    let (x, y) = (num::real(arg(vm, args, 0), "rationalize")?, num::real(arg(vm, args, 1), "rationalize")?);
    if x.is_exact() && y.is_exact() {
        let (x, y) = (x.rat(), num_traits::Signed::abs(&y.rat()));
        let r = num::simplest(&num::rat_add(x.clone(), -y.clone()), &num::rat_add(x, y));
        return Ok(num::from_n(vm, num::from_rational(r)));
    }
    Ok(Value::float(simplest(x.f() - y.f().abs(), x.f() + y.f().abs())))
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

pub fn install(vm: &mut Vm) {
    install_cxrs(vm);
    crate::natives! { vm;
        /// Return the sum of NUMBERS, 0 if there are none.
        "(+ . numbers)" => |vm: &mut Vm, a, n| fold_num(vm, a, n, Value::int_unchecked(0), num::add);
        /// Return the product of NUMBERS, 1 if there are none.
        "(* . numbers)" => |vm: &mut Vm, a, n| fold_num(vm, a, n, Value::int_unchecked(1), num::mul);
        /// Subtract NUMBERS from NUMBER, or negate NUMBER if alone.
        "(- number . numbers)" => |vm: &mut Vm, a, n| if n == 1 { num::sub(vm, Value::int_unchecked(0), arg(vm, a, 0)) } else {
            let first = arg(vm, a, 0); fold_num(vm, a + 1, n - 1, first, num::sub) };
        /// Divide NUMBER by NUMBERS, or return its reciprocal if alone.
        "(/ number . numbers)" => |vm: &mut Vm, a, n| if n == 1 { num::div(vm, Value::int_unchecked(1), arg(vm, a, 0)) } else {
            let first = arg(vm, a, 0); fold_num(vm, a + 1, n - 1, first, num::div) };
        /// Return #t if NUMBER and NUMBERS are strictly increasing.
        "(< number . numbers)" => |vm: &mut Vm, a, n| chain(vm, a, n, num::lt);
        /// Return #t if NUMBER and NUMBERS never decrease.
        "(<= number . numbers)" => |vm: &mut Vm, a, n| chain(vm, a, n, num::le);
        /// Return #t if NUMBER and NUMBERS are strictly decreasing.
        "(> number . numbers)" => |vm: &mut Vm, a, n| chain(vm, a, n, |x, y| num::lt(y, x));
        /// Return #t if NUMBER and NUMBERS never increase.
        "(>= number . numbers)" => |vm: &mut Vm, a, n| chain(vm, a, n, |x, y| num::le(y, x));
        /// Return #t if NUMBER and NUMBERS are all numerically equal.
        "(= number . numbers)" => |vm: &mut Vm, a, n| chain(vm, a, n, num::num_eq);
        /// Return N divided by D, truncated toward zero.
        "(quotient n d)" => |vm: &mut Vm, a, _| num::quotient(vm, arg(vm, a, 0), arg(vm, a, 1));
        /// Return the remainder of N by D, with the sign of N.
        "(remainder n d)" => |vm: &mut Vm, a, _| num::remainder(vm, arg(vm, a, 0), arg(vm, a, 1));
        /// Return N modulo D, with the sign of D.
        "(modulo n d)" => |vm: &mut Vm, a, _| num::modulo(vm, arg(vm, a, 0), arg(vm, a, 1));
        /// Return the absolute value of X.
        "(abs x)" => |vm: &mut Vm, a, _| { let n = num::real(arg(vm, a, 0), "abs")?; Ok(num::abs(vm, n)) };
        /// Return the smallest of X and XS.
        /// The result is inexact if any of them is.
        "(min x . xs)" => |vm: &mut Vm, a, n| extremum(vm, a, n, num::lt);
        /// Return the largest of X and XS.
        /// The result is inexact if any of them is.
        "(max x . xs)" => |vm: &mut Vm, a, n| extremum(vm, a, n, |x, y| num::lt(y, x));
        /// Return Z as an exact number; the old name of `exact`.
        "(inexact->exact z)" => |vm: &mut Vm, a, _| exact(vm, arg(vm, a, 0), "inexact->exact");
        /// Return Z as an exact number.
        "(exact z)" => |vm: &mut Vm, a, _| exact(vm, arg(vm, a, 0), "exact");
        /// Return the largest integer not greater than X.
        "(floor x)" => |vm: &mut Vm, a, _| rounding(vm, a, "floor", num::Rounding::Floor, f64::floor);
        /// Return the smallest integer not less than X.
        "(ceiling x)" => |vm: &mut Vm, a, _| rounding(vm, a, "ceiling", num::Rounding::Ceiling, f64::ceil);
        /// Return the integer nearest X, the even one on a tie.
        "(round x)" => |vm: &mut Vm, a, _| rounding(vm, a, "round", num::Rounding::Round, f64::round_ties_even);
        /// Return X with its fraction dropped, toward zero.
        "(truncate x)" => |vm: &mut Vm, a, _| rounding(vm, a, "truncate", num::Rounding::Truncate, f64::trunc);
        /// Return #t if OBJ is a number.
        "(number? obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(num::is_number(arg(vm, a, 0))));
        /// Return #t if OBJ is an integer, exact or not.
        "(integer? obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(num::is_integer(arg(vm, a, 0))));
        /// Return #t if Z is zero.
        "(zero? z)" => |vm: &mut Vm, a, _| Ok(Value::bool(num::num_eq(arg(vm, a, 0), Value::int_unchecked(0))?));
        /// Return #t if X is greater than zero.
        "(positive? x)" => |vm: &mut Vm, a, _| Ok(Value::bool(num::lt(Value::int_unchecked(0), arg(vm, a, 0))?));
        /// Return #t if X is less than zero.
        "(negative? x)" => |vm: &mut Vm, a, _| Ok(Value::bool(num::lt(arg(vm, a, 0), Value::int_unchecked(0))?));
        /// Return #t if the integer N is even.
        "(even? n)" => |vm: &mut Vm, a, _| Ok(Value::bool(parity(vm, a, "even?")?));
        /// Return #t if the integer N is odd.
        "(odd? n)" => |vm: &mut Vm, a, _| Ok(Value::bool(!parity(vm, a, "odd?")?));
        /// Return Z written as a string, in RADIX (from 2 to 36, default 10).
        "(number->string z [radix])" => number_to_string;
        /// Return the greatest common divisor of NS, 0 if there are none.
        "(gcd . ns)" => |vm: &mut Vm, a, n| gcd_lcm(vm, a, n, false);
        /// Return the least common multiple of NS, 1 if there are none.
        "(lcm . ns)" => |vm: &mut Vm, a, n| gcd_lcm(vm, a, n, true);
        /// Return N divided by D, rounded down.
        "(floor-quotient n d)" => floor_quotient;
        /// Return the remainder of N by D, with the sign of D.
        "(floor-remainder n d)" => |vm: &mut Vm, a, _| num::modulo(vm, arg(vm, a, 0), arg(vm, a, 1));
        /// Return N divided by D, truncated toward zero.
        "(truncate-quotient n d)" => |vm: &mut Vm, a, _| num::quotient(vm, arg(vm, a, 0), arg(vm, a, 1));
        /// Return the remainder of N by D, with the sign of N.
        "(truncate-remainder n d)" => |vm: &mut Vm, a, _| num::remainder(vm, arg(vm, a, 0), arg(vm, a, 1));
        "(%exact-integer-sqrt n)" => isqrt;
        /// Return the numerator of Q in lowest terms.
        "(numerator q)" => |vm: &mut Vm, a, _| ratio_part(vm, a, true);
        /// Return the denominator of Q in lowest terms.
        "(denominator q)" => |vm: &mut Vm, a, _| ratio_part(vm, a, false);
        /// Return the simplest rational that differs from X by at most Y.
        "(rationalize x y)" => rationalize;
        /// Return the number STRING writes in RADIX (2, 8, 10 or 16), or #f.
        "(string->number string [radix])" => string_to_number;

        /// Return a new pair of A and B.

        "(cons a b)" => |vm: &mut Vm, a, _| { let (x, y) = (arg(vm, a, 0), arg(vm, a, 1)); Ok(vm.alloc_pair(x, y)) };
        /// Return the first element of PAIR.
        "(car pair)" => car;
        /// Return the second element of PAIR.
        "(cdr pair)" => cdr;
        /// Store OBJ as the first element of PAIR.
        "(set-car! pair obj)" => |vm: &mut Vm, a, _| set_pair(vm, a, 0, "set-car!");
        /// Store OBJ as the second element of PAIR.
        "(set-cdr! pair obj)" => |vm: &mut Vm, a, _| set_pair(vm, a, 1, "set-cdr!");
        /// Return a new list of OBJS.
        "(list . objs)" => list;
        /// Return the number of elements of LIST.
        "(length list)" => length;
        /// Return a new list of the elements of LIST in reverse order.
        "(reverse list)" => reverse;
        /// Return the elements of LISTS in one list.
        /// The last of them is shared, not copied, and may be any object.
        "(append . lists)" => append;
        /// Return LIST without its first K elements.
        "(list-tail list k)" => list_tail;
        /// Return element K of LIST, counting from 0.
        "(list-ref list k)" => list_ref;
        /// Return #t if OBJ is the empty list.
        "(null? obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0) == Value::NIL));
        /// Return #t if OBJ is a pair.
        "(pair? obj)" => |vm: &mut Vm, a, _| type_pred(vm, a, Kind::Pair);
        /// Return #t if OBJ is a proper list: finite, ending in ().
        "(list? obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(list_len(arg(vm, a, 0), "").is_ok()));
        /// Return the first tail of LIST whose car is OBJ by `eq?`, or #f.
        "(memq obj list)" => |vm: &mut Vm, a, _| mem_generic(vm, a, "memq", |x, y| x == y);
        /// Return the first tail of LIST whose car is OBJ by `eqv?`, or #f.
        "(memv obj list)" => |vm: &mut Vm, a, _| mem_generic(vm, a, "memv", eqv);
        /// Return the first tail of LIST whose car is OBJ, or #f.
        /// Elements are compared with COMPARE, `equal?` by default.
        "(member obj list [compare])" => |vm: &mut Vm, a, n| if n == 3 { find_with(vm, a, false) } else { mem_generic(vm, a, "member", equal) };
        /// Return the first pair of ALIST whose car is KEY by `eq?`, or #f.
        "(assq key alist)" => |vm: &mut Vm, a, _| ass_generic(vm, a, "assq", |x, y| x == y);
        /// Return the first pair of ALIST whose car is KEY by `eqv?`, or #f.
        "(assv key alist)" => |vm: &mut Vm, a, _| ass_generic(vm, a, "assv", eqv);
        /// Return the first pair of ALIST whose car is KEY, or #f.
        /// Keys are compared with COMPARE, `equal?` by default.
        "(assoc key alist [compare])" => |vm: &mut Vm, a, n| if n == 3 { find_with(vm, a, true) } else { ass_generic(vm, a, "assoc", equal) };

        /// Return #t if A and B are the same object.

        "(eq? a b)" => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0) == arg(vm, a, 1)));
        /// Return #t if A and B are the same object or equal numbers or chars.
        "(eqv? a b)" => |vm: &mut Vm, a, _| Ok(Value::bool(eqv(arg(vm, a, 0), arg(vm, a, 1))));
        /// Return #t if A and B have the same structure and contents.
        "(equal? a b)" => |vm: &mut Vm, a, _| Ok(Value::bool(equal(arg(vm, a, 0), arg(vm, a, 1))));
        /// Return #t if OBJ is #f, else #f.
        "(not obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0).is_false()));
        /// Return #t if OBJ is #t or #f.
        "(boolean? obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(matches!(arg(vm, a, 0), Value::TRUE | Value::FALSE)));
        /// Return #t if OBJ is a symbol.
        "(symbol? obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0).is_symbol()));
        /// Return #t if OBJ is a string.
        "(string? obj)" => |vm: &mut Vm, a, _| type_pred(vm, a, Kind::String);
        /// Return #t if OBJ is a character.
        "(char? obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0).is_char()));
        /// Return #t if OBJ is a vector.
        "(vector? obj)" => |vm: &mut Vm, a, _| type_pred(vm, a, Kind::Vector);
        /// Return #t if OBJ can be called.
        "(procedure? obj)" => |vm: &mut Vm, a, _| { let v = arg(vm, a, 0); Ok(Value::bool(vm.is_procedure(v))) };

        /// Return a new vector of K elements, each FILL.

        "(make-vector k [fill])" => make_vector;
        /// Return a new vector of OBJS.
        "(vector . objs)" => vector;
        /// Return the number of elements of VECTOR.
        "(vector-length vector)" => vector_length;
        /// Return element K of VECTOR, counting from 0.
        "(vector-ref vector k)" => vector_ref;
        /// Store OBJ as element K of VECTOR.
        "(vector-set! vector k obj)" => vector_set;
        /// Return a list of the elements of VECTOR from START to END.
        "(vector->list vector [start] [end])" => |vm: &mut Vm, a, n| { let items = vector_range(vm, a, n, 1, "vector->list", 24)?; Ok(vm.make_list(&items)) };
        /// Return a string of the characters of VECTOR from START to END.
        "(vector->string vector [start] [end])" => |vm: &mut Vm, a, n| {
            let s = vector_range(vm, a, n, 1, "vector->string", 8)?.into_iter().map(|c| char_arg(c, "vector->string")).collect::<Result<String, _>>()?;
            Ok(vm.make_string(&s)) };
        /// Return a vector of the characters of STRING from START to END.
        "(string->vector string [start] [end])" => |vm: &mut Vm, a, n| { let items: Vec<Value> = string_range(vm, a, n, "string->vector", 16)?.into_iter().map(Value::char).collect(); Ok(vm.make_vector(&items)) };
        /// Copy the elements of FROM from START to END into TO at AT.
        "(vector-copy! to at from [start] [end])" => vector_copy_into;
        /// Store FILL in the elements of VECTOR from START to END.
        "(vector-fill! vector fill [start] [end])" => vector_fill_range;
        /// Return a new vector of the elements of LIST.
        "(list->vector list)" => list_to_vector;

        /// Return the number of characters of STRING.

        "(string-length string)" => string_length;
        /// Return character K of STRING, counting from 0.
        "(string-ref string k)" => string_ref;
        /// Return a new string of the characters of STRING from START to END.
        "(substring string start [end])" => substring;
        /// Return a new string of the characters of STRINGS in order.
        "(string-append . strings)" => string_append;
        /// Return #t if STRING and STRINGS are all the same.
        "(string=? string . strings)" => |vm: &mut Vm, a, n| string_cmp(vm, a, n, |o| o.is_eq(), "string=?");
        /// Return #t if STRING and STRINGS are in increasing order.
        "(string<? string . strings)" => |vm: &mut Vm, a, n| string_cmp(vm, a, n, |o| o.is_lt(), "string<?");
        /// Return a list of the characters of STRING from START to END.
        "(string->list string [start] [end])" => |vm: &mut Vm, a, n| { let items: Vec<Value> = string_range(vm, a, n, "string->list", 32)?.into_iter().map(Value::char).collect(); Ok(vm.make_list(&items)) };
        /// Return a new string of the characters of LIST.
        "(list->string list)" => list_to_string;
        /// Return a new string of K characters, each CHAR.
        "(make-string k [char])" => make_string;
        /// Store CHAR as character K of STRING.
        "(string-set! string k char)" => string_set;
        /// Store CHAR in the characters of STRING from START to END.
        "(string-fill! string char [start] [end])" => string_fill;
        /// Copy the characters of FROM from START to END into TO at AT.
        "(string-copy! to at from [start] [end])" => string_copy_into;
        /// Return a new string of the characters of STRING from START to END.
        "(string-copy string [start] [end])" => |vm: &mut Vm, a, n| { let s: String = string_range(vm, a, n, "string-copy", 8)?.into_iter().collect(); Ok(vm.make_string(&s)) };
        /// Return the symbol named STRING.
        "(string->symbol string)" => string_to_symbol;
        /// Return the name of SYMBOL as a string.
        "(symbol->string symbol)" => symbol_to_string;
        /// Return #t if STRING starts with PREFIX.
        "(string-prefix? prefix string)" => string_prefix;
        /// Return #t if CHAR and CHARS are all the same.
        "(char=? char . chars)" => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char=?", false, |o| o.is_eq());
        /// Return #t if CHAR and CHARS are in increasing order.
        "(char<? char . chars)" => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char<?", false, |o| o.is_lt());
        /// Return #t if CHAR and CHARS are in decreasing order.
        "(char>? char . chars)" => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char>?", false, |o| o.is_gt());
        /// Return #t if CHAR and CHARS never decrease.
        "(char<=? char . chars)" => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char<=?", false, |o| o.is_le());
        /// Return #t if CHAR and CHARS never increase.
        "(char>=? char . chars)" => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char>=?", false, |o| o.is_ge());
        /// Return #t if CHAR and CHARS are all the same, ignoring case.
        "(char-ci=? char . chars)" => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char-ci=?", true, |o| o.is_eq());
        /// Return #t if CHAR and CHARS are in increasing order, ignoring case.
        "(char-ci<? char . chars)" => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char-ci<?", true, |o| o.is_lt());
        /// Return #t if CHAR and CHARS are in decreasing order, ignoring case.
        "(char-ci>? char . chars)" => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char-ci>?", true, |o| o.is_gt());
        /// Return #t if CHAR and CHARS never decrease, ignoring case.
        "(char-ci<=? char . chars)" => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char-ci<=?", true, |o| o.is_le());
        /// Return #t if CHAR and CHARS never increase, ignoring case.
        "(char-ci>=? char . chars)" => |vm: &mut Vm, a, n| char_chain(vm, a, n, "char-ci>=?", true, |o| o.is_ge());
        /// Return CHAR case-folded, for comparing without case.
        "(char-foldcase char)" => |vm: &mut Vm, a, _| Ok(Value::char(fold_char(char_arg(arg(vm, a, 0), "char-foldcase")?)));
        /// Return the digit CHAR stands for, or #f if it is not a digit.
        "(digit-value char)" => |vm: &mut Vm, a, _| Ok(digit_value(char_arg(arg(vm, a, 0), "digit-value")?).map_or(Value::FALSE, Value::int_unchecked));
        /// Return STRING case-folded, for comparing without case.
        "(string-foldcase string)" => |vm: &mut Vm, a, _| { admit_changed(vm, a, "string-foldcase")?; let s = fold_string(str_arg(arg(vm, a, 0), "string-foldcase")?); Ok(vm.make_string(&s)) };
        /// Return the Unicode scalar value of CHAR.
        "(char->integer char)" => |vm: &mut Vm, a, _| Ok(Value::int_unchecked(char_arg(arg(vm, a, 0), "char->integer")? as i64));
        /// Return the character whose Unicode scalar value is N.
        "(integer->char n)" => |vm: &mut Vm, a, _| {
            let i = int_arg(arg(vm, a, 0), "integer->char")?;
            u32::try_from(i).ok().and_then(char::from_u32).map(Value::char).ok_or_else(|| Error::new("integer->char: invalid code point")) };
        /// Return #t if CHAR is a letter.
        "(char-alphabetic? char)" => |vm: &mut Vm, a, _| char_pred(vm, a, "char-alphabetic?", char::is_alphabetic);
        /// Return #t if CHAR is a digit.
        "(char-numeric? char)" => |vm: &mut Vm, a, _| char_pred(vm, a, "char-numeric?", char::is_numeric);
        /// Return #t if CHAR is whitespace.
        "(char-whitespace? char)" => |vm: &mut Vm, a, _| char_pred(vm, a, "char-whitespace?", char::is_whitespace);

        /// Return a new box holding OBJ.

        "(box obj)" => make_box;
        /// Return what BOX holds.
        "(unbox box)" => |vm: &mut Vm, a, _| Ok(unsafe { field(box_arg(arg(vm, a, 0), "unbox")?, 0) });
        /// Make BOX hold OBJ.
        "(set-box! box obj)" => |vm: &mut Vm, a, _| {
            let (b, v) = (box_arg(arg(vm, a, 0), "set-box!")?, arg(vm, a, 1));
            unsafe { set_field(b, 0, v) }; vm.write_barrier(b, v); Ok(Value::VOID) };

        /// Return a new hash table comparing keys with EQUIVALENCE.

        /// EQUIVALENCE is `equal?` by default. The built-in `eq?`, `eqv?`,

        /// `equal?`, `string=?` and `char=?` compare natively; any other
        /// equivalence procedure is called, with HASH giving equal hashes
        /// for equivalent keys (by default `string-ci-hash` for `string-ci=?`
        /// and `hash` otherwise).
        "(make-hash-table [equivalence] [hash])" => |vm: &mut Vm, a, n| make_table(vm, a, n, false);
        /// Return a new hash table whose keys do not keep their entries alive.
        /// EQUIVALENCE is a built-in one `make-hash-table` takes, `eq?` by
        /// default; an entry goes when nothing else holds its key.
        "(make-weak-hash-table [equivalence])" => |vm: &mut Vm, a, n| make_table(vm, a, n, true);
        /// Return a new hash table with the procedures and entries of TABLE.
        /// MUTABLE? is ignored: tables are always mutable.
        "(hash-table-copy table [mutable?])" => hash_copy;
        /// Return the equivalence procedure TABLE compares keys with.
        "(hash-table-equivalence-function table)" => |vm: &mut Vm, a, _| table_proc(vm, a, 4, "hash-table-equivalence-function");
        /// Return the hash procedure of TABLE's keys.
        "(hash-table-hash-function table)" => |vm: &mut Vm, a, _| table_proc(vm, a, 5, "hash-table-hash-function");
        /// Return a hash of OBJ by identity, as `eq?` compares.
        /// With BOUND, the hash is below it.
        "(hash-by-identity obj [bound])" => |vm: &mut Vm, a, n| { let h = hash_key(vm, arg(vm, a, 0), Equiv::Eq); bounded_hash(vm, a, n, h, "hash-by-identity") };
        /// Return a hash of OBJ by contents, as `equal?` compares.
        /// With BOUND, the hash is below it.
        "(hash obj [bound])" => |vm: &mut Vm, a, n| { let h = hash_key(vm, arg(vm, a, 0), Equiv::Equal); bounded_hash(vm, a, n, h, "hash") };
        /// Return a hash of STRING, as `string=?` compares.
        /// With BOUND, the hash is below it.
        "(string-hash string [bound])" => |vm: &mut Vm, a, n| { let h = string_hash(str_arg(arg(vm, a, 0), "string-hash")?); bounded_hash(vm, a, n, h, "string-hash") };
        /// Return a hash of STRING ignoring case, as `string-ci=?` compares.
        /// With BOUND, the hash is below it.
        "(string-ci-hash string [bound])" => |vm: &mut Vm, a, n| { admit_changed(vm, a, "string-ci-hash")?; let h = string_hash(&fold_string(str_arg(arg(vm, a, 0), "string-ci-hash")?)); bounded_hash(vm, a, n, h, "string-ci-hash") };
        /// Return the value of KEY in TABLE, else DEFAULT.
        "(hash-table-ref/default table key default)" => hash_ref;
        /// Make VALUE the value of KEY in TABLE.
        "(hash-table-set! table key value)" => hash_set;
        /// Return the number of entries of TABLE.
        "(hash-table-count table)" => hash_count;
        /// Return #t if TABLE has an entry for KEY.
        "(hash-table-contains? table key)" => hash_contains;
        /// Remove the entry for KEY from TABLE, if there is one.
        "(hash-table-delete! table key)" => hash_delete;

        /// Write OBJ to PORT for people: strings and chars as they are.

        "(display obj [port])" => |vm: &mut Vm, a, n| output(vm, a, n, false, false);
        /// Write OBJ to PORT as `read` reads it back.
        /// Shared structure and cycles are written with datum labels.
        "(write obj [port])" => |vm: &mut Vm, a, n| output(vm, a, n, true, false);
        /// Write OBJ to PORT as `display` does, then a newline.
        "(displayln [obj] [port])" => |vm: &mut Vm, a, n| output(vm, a, n, false, true);
        /// Write a newline to PORT.
        "(newline [port])" => |vm: &mut Vm, a, n| { let p = (n > 0).then(|| arg(vm, a, 0)); crate::stdlib::write_out(vm, p, "\n")?; Ok(Value::VOID) };
        /// Raise an error object with MESSAGE and IRRITANTS.
        "(error message . irritants)" => error;
        /// Return the unspecified value, whatever IGNORED is.
        "(void . ignored)" => |_: &mut Vm, _, _| Ok(Value::VOID);
        /// Print the garbage collector's counts and times to the error port.
        "(gc-stats)" => gc_stats;
        /// Collect garbage: a minor collection and a slice of the old space.
        /// With HOW the symbol `full`, complete a whole cycle.
        "(collect-garbage [how])" => collect_garbage;
    }
    vm.requiring(Capability::Files, |vm| {
        crate::natives! { vm;
            /// Return the lines of the file at PATH, without their newlines.
            "(read-lines path)" => read_lines;
            /// Return the lines of the file at PATH, without their newlines.
            "(file->lines path)" => read_lines;
        }
    });
    vm.requiring(Capability::HostControl, |vm| {
        crate::natives! { vm;
            /// End the program with STATUS: an integer, #t (success) or #f.
            /// Handlers and `dynamic-wind` exits run first; the host decides what
            /// ending means.
            "(exit [status])" => exit;
            /// End the program with STATUS, as `exit` does.
            "(emergency-exit [status])" => exit;
        }
    });
    crate::stdlib::install(vm);
    crate::bytes::install(vm);
    crate::complex::install(vm);
    crate::cursors::install(vm);
    crate::regexp::install(vm);
    crate::ports::install(vm);
    crate::tasks::install(vm);
}
