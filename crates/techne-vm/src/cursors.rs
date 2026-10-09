//! String cursors, as SRFI 130 has them.
//!
//! A cursor is a byte offset into a string's UTF-8, an immediate value of
//! its own type, so stepping through a string is constant time per
//! character whatever the characters (an index is linear in non-ASCII
//! strings). A procedure taking a cursor also takes a character index, as
//! SRFI 130 says. A cursor is only good for the string it came from, and
//! not after the string is changed: a cursor that is not at a character of
//! the string is an error.
//!
//! The rest of SRFI 130 is the library `(srfi 130)`, in Scheme over these.

use crate::{
    builtins::{index_arg, str_arg, type_error},
    heap::{self, str_bytes},
    value::Value,
    vm::{Error, Vm, init_string},
};

type R = Result<Value, Error>;

#[inline(always)]
fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

/// The string argument at `i`.
fn text<'a>(vm: &Vm, args: usize, i: usize, who: &str) -> Result<&'a str, Error> {
    str_arg(arg(vm, args, i), who)
}

fn is_ascii(vm: &Vm, args: usize, i: usize) -> bool {
    unsafe { heap::str_is_ascii(arg(vm, args, i).as_ptr()) }
}

/// The byte offset of cursor or index `c` in the string argument at `s`.
pub(crate) fn offset(vm: &Vm, args: usize, s: usize, c: Value, who: &str) -> Result<usize, Error> {
    let t = text(vm, args, s, who)?;
    if c.is_cursor() {
        let at = c.as_cursor();
        return if t.is_char_boundary(at) { Ok(at) } else { Err(Error::new(format!("{who}: cursor {at} is not in the string"))) };
    }
    if !c.is_int() {
        return Err(type_error(who, "string cursor or index", c));
    }
    let i = index_arg(c, who)?;
    let at = if is_ascii(vm, args, s) {
        (i <= t.len()).then_some(i)
    } else {
        t.char_indices().map(|(at, _)| at).chain(std::iter::once(t.len())).nth(i)
    };
    at.ok_or_else(|| Error::new(format!("{who}: index {i} is not in the string")))
}

/// The character index of byte offset `at` of the string argument at `s`.
fn index(vm: &Vm, args: usize, s: usize, at: usize) -> usize {
    if is_ascii(vm, args, s) {
        at
    } else {
        unsafe { std::str::from_utf8_unchecked(&str_bytes(arg(vm, args, s).as_ptr())[..at]) }.chars().count()
    }
}

/// The offset `n` characters after (or, negative, before) `at`.
fn step(t: &str, at: usize, n: i64, who: &str) -> Result<usize, Error> {
    let found = if n >= 0 {
        t[at..].char_indices().map(|(i, _)| at + i).chain(std::iter::once(t.len())).nth(n as usize)
    } else {
        t[..at].char_indices().map(|(i, _)| i).rev().nth((-n - 1) as usize)
    };
    found.ok_or_else(|| Error::new(format!("{who}: no character {n} away from cursor {at}")))
}

/// `string-cursor-next`, `-prev`, `-forward` and `-back`: `sign` times the
/// count at argument 2 (or 1) characters on. An index gives an index.
fn advance(vm: &mut Vm, args: usize, n: usize, sign: i64, who: &str) -> R {
    let count = if n > 2 { index_arg(arg(vm, args, 2), who)? as i64 } else { 1 };
    let c = arg(vm, args, 1);
    let at = offset(vm, args, 0, c, who)?;
    let to = step(text(vm, args, 0, who)?, at, sign * count, who)?;
    Ok(if c.is_cursor() { Value::cursor(to) } else { Value::int_unchecked(index(vm, args, 0, to) as i64) })
}

/// Compares two cursors or two indexes.
fn compare(vm: &mut Vm, args: usize, who: &str, ok: fn(std::cmp::Ordering) -> bool) -> R {
    let (a, b) = (arg(vm, args, 0), arg(vm, args, 1));
    let key = |v: Value| match () {
        _ if v.is_cursor() => Ok((true, v.as_cursor())),
        _ if v.is_int() => Ok((false, index_arg(v, who)?)),
        _ => Err(type_error(who, "string cursor or index", v)),
    };
    match (key(a)?, key(b)?) {
        ((x, i), (y, j)) if x == y => Ok(Value::bool(ok(i.cmp(&j)))),
        _ => Err(Error::new(format!("{who}: cannot compare a cursor with an index"))),
    }
}

/// The byte range of the optional start and end arguments at `i`, cursors
/// or indexes, in the string argument at `s`.
fn range(vm: &Vm, args: usize, n: usize, s: usize, i: usize, who: &str) -> Result<(usize, usize), Error> {
    let len = text(vm, args, s, who)?.len();
    let start = if n > i { offset(vm, args, s, arg(vm, args, i), who)? } else { 0 };
    let end = if n > i + 1 { offset(vm, args, s, arg(vm, args, i + 1), who)? } else { len };
    if start > end {
        return Err(Error::new(format!("{who}: start {start} is after end {end}")));
    }
    Ok((start, end))
}

/// A new string of bytes `a..b` of the string argument at 0.
fn copy(vm: &mut Vm, args: usize, a: usize, b: usize) -> R {
    vm.admit_items(b - a, 1, 64)?;
    let p = vm.alloc(heap::string_words(b - a));
    unsafe { init_string(p, &str_bytes(arg(vm, args, 0).as_ptr())[a..b]) };
    Ok(Value::ptr(p))
}

/// `string-contains` and `string-contains-right` of SRFI 130: the cursor of
/// the first or last STRING2 in STRING1, or #f.
fn contains(vm: &mut Vm, args: usize, n: usize, right: bool, who: &str) -> R {
    let (a1, b1) = range(vm, args, n, 0, 2, who)?;
    let (a2, b2) = range(vm, args, n, 1, 4, who)?;
    let (hay, needle) = (&text(vm, args, 0, who)?[a1..b1], &text(vm, args, 1, who)?[a2..b2]);
    let found = if right { hay.rfind(needle) } else { hay.find(needle) };
    Ok(found.map_or(Value::FALSE, |i| Value::cursor(a1 + i)))
}

pub fn install(vm: &mut Vm) {
    crate::natives! { vm;
        /// Return #t if OBJ is a string cursor.
        "(string-cursor? obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0).is_cursor()));
        /// Return the cursor of the first character of STRING.
        "(string-cursor-start string)" => |vm: &mut Vm, a, _| { text(vm, a, 0, "string-cursor-start")?; Ok(Value::cursor(0)) };
        /// Return the cursor after the last character of STRING.
        "(string-cursor-end string)" => |vm: &mut Vm, a, _| Ok(Value::cursor(text(vm, a, 0, "string-cursor-end")?.len()));
        /// Return the cursor of the character after CURSOR in STRING.
        /// For an index, return the next index.
        "(string-cursor-next string cursor)" => |vm: &mut Vm, a, n| advance(vm, a, n, 1, "string-cursor-next");
        /// Return the cursor of the character before CURSOR in STRING.
        /// For an index, return the index before.
        "(string-cursor-prev string cursor)" => |vm: &mut Vm, a, n| advance(vm, a, n, -1, "string-cursor-prev");
        /// Return the cursor N characters after CURSOR in STRING.
        /// For an index, return the index N after.
        "(string-cursor-forward string cursor n)" => |vm: &mut Vm, a, n| advance(vm, a, n, 1, "string-cursor-forward");
        /// Return the cursor N characters before CURSOR in STRING.
        /// For an index, return the index N before.
        "(string-cursor-back string cursor n)" => |vm: &mut Vm, a, n| advance(vm, a, n, -1, "string-cursor-back");
        /// Return #t if the cursors (or indexes) A and B are the same.
        "(string-cursor=? a b)" => |vm: &mut Vm, a, _| compare(vm, a, "string-cursor=?", |o| o.is_eq());
        /// Return #t if cursor (or index) A is before B.
        "(string-cursor<? a b)" => |vm: &mut Vm, a, _| compare(vm, a, "string-cursor<?", |o| o.is_lt());
        /// Return #t if cursor (or index) A is after B.
        "(string-cursor>? a b)" => |vm: &mut Vm, a, _| compare(vm, a, "string-cursor>?", |o| o.is_gt());
        /// Return #t if cursor (or index) A is not after B.
        "(string-cursor<=? a b)" => |vm: &mut Vm, a, _| compare(vm, a, "string-cursor<=?", |o| o.is_le());
        /// Return #t if cursor (or index) A is not before B.
        "(string-cursor>=? a b)" => |vm: &mut Vm, a, _| compare(vm, a, "string-cursor>=?", |o| o.is_ge());
        /// Return the number of characters of STRING from START to END.
        /// START and END are cursors or indexes.
        "(string-cursor-diff string start end)" => |vm: &mut Vm, a, _| {
            let who = "string-cursor-diff";
            let (start, end) = (offset(vm, a, 0, arg(vm, a, 1), who)?, offset(vm, a, 0, arg(vm, a, 2), who)?);
            let t = text(vm, a, 0, who)?;
            let count = |x: usize, y: usize| if is_ascii(vm, a, 0) { y - x } else { t[x..y].chars().count() } as i64;
            Ok(Value::int_unchecked(if start <= end { count(start, end) } else { -count(end, start) }))
        };
        /// Return the index of CURSOR in STRING; an index is returned as it is.
        "(string-cursor->index string cursor)" => |vm: &mut Vm, a, _| {
            let c = arg(vm, a, 1);
            if c.is_int() { offset(vm, a, 0, c, "string-cursor->index")?; return Ok(c) }
            let at = offset(vm, a, 0, c, "string-cursor->index")?;
            Ok(Value::int_unchecked(index(vm, a, 0, at) as i64))
        };
        /// Return the cursor of INDEX in STRING; a cursor is returned as it is.
        "(string-index->cursor string index)" => |vm: &mut Vm, a, _| { let c = arg(vm, a, 1); Ok(Value::cursor(offset(vm, a, 0, c, "string-index->cursor")?)) };
        /// Return the character at CURSOR (or index) in STRING.
        "(string-ref/cursor string cursor)" => |vm: &mut Vm, a, _| {
            let at = offset(vm, a, 0, arg(vm, a, 1), "string-ref/cursor")?;
            text(vm, a, 0, "string-ref/cursor")?[at..].chars().next().map(Value::char)
                .ok_or_else(|| Error::new("string-ref/cursor: the end cursor has no character"))
        };
        /// Return a new string of the characters of STRING from START to END.
        /// START and END are cursors or indexes.
        "(substring/cursors string start end)" => |vm: &mut Vm, a, n| { let (x, y) = range(vm, a, n, 0, 1, "substring/cursors")?; copy(vm, a, x, y) };
        /// Return a new string of the characters of STRING from START to END.
        /// START and END are cursors or indexes, by default the whole string.
        "(string-copy/cursors string [start] [end])" => |vm: &mut Vm, a, n| { let (x, y) = range(vm, a, n, 0, 1, "string-copy/cursors")?; copy(vm, a, x, y) };
        "(%string-contains/cursor string1 string2 [start1] [end1] [start2] [end2])" => |vm: &mut Vm, a, n| contains(vm, a, n, false, "string-contains");
        "(%string-contains-right/cursor string1 string2 [start1] [end1] [start2] [end2])" => |vm: &mut Vm, a, n| contains(vm, a, n, true, "string-contains-right");
    }
}
