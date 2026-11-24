//! String cursors, as SRFI 130 has them.
//!
//! A cursor is a byte offset into a string's UTF-8, an immediate value of
//! its own type, so stepping through a string is constant time per
//! character whatever the characters. Character indexes use a sparse
//! index, also with bounded decoding. A procedure taking a cursor also
//! takes a character index, as SRFI 130 says. A cursor is only good for
//! the string it came from, and not after the string is changed: a cursor
//! that is not at a character of the string is an error.
//!
//! The rest of SRFI 130 is the library `(srfi 130)`, in Scheme over these.

use crate::{
    builtins::{index_arg, string_arg, type_error},
    heap::{self, str_bytes},
    value::Value,
    vm::{Error, Vm, init_string},
};

type R = Result<Value, Error>;

#[inline(always)]
fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

/// A string's bytes, its characters decoded only where a cursor reads or
/// steps: bytes a world changed to no UTF-8 are corruption (`heap::corrupt`)
/// where they are met, and the rest costs nothing, as stepping is constant
/// time per character.
#[derive(Clone, Copy)]
struct Text<'a>(&'a [u8]);

impl<'a> Text<'a> {
    fn len(self) -> usize {
        self.0.len()
    }

    /// Whether a character starts at `at`, or it is the end.
    fn is_boundary(self, at: usize) -> bool {
        at == self.len() || self.0.get(at).is_some_and(|&b| (b as i8) >= -0x40)
    }

    /// Decode the character at `at` and its length in bytes: `None` at or past
    /// the end, `Some(Ok((c, len)))` on valid UTF-8, `Some(Err(()))` if the bytes
    /// at `at` are not valid UTF-8.
    fn try_char_at(self, at: usize) -> Option<Result<(char, usize), ()>> {
        let rest = self.0.get(at..).filter(|r| !r.is_empty())?;
        let window = &rest[..rest.len().min(4)];
        let valid = window.utf8_chunks().next().map_or("", |chunk| chunk.valid());
        Some(valid.chars().next().map(|c| (c, c.len_utf8())).ok_or(()))
    }

    /// The character at `at` and its length in bytes; none at the end.
    fn char_at(self, at: usize) -> Option<(char, usize)> {
        Some(self.try_char_at(at)?.unwrap_or_else(|()| heap::corrupt("a string's bytes are no UTF-8")))
    }

    /// The text of bytes `a..b`, checked.
    fn str(self, a: usize, b: usize) -> &'a str {
        heap::utf8(&self.0[a..b])
    }

    /// The offset `n` characters on from `at` (the end counting as one).
    fn forward(self, mut at: usize, n: usize) -> Option<usize> {
        for _ in 0..n {
            at += self.char_at(at)?.1;
        }
        Some(at)
    }

    /// The offset `n` characters back from `at`.
    fn back(self, mut at: usize, n: usize) -> Option<usize> {
        for _ in 0..n {
            let start = (at.saturating_sub(4)..at).rev().find(|&i| self.is_boundary(i))?;
            if self.char_at(start)?.1 != at - start {
                heap::corrupt("a string's bytes are no UTF-8");
            }
            at = start;
        }
        Some(at)
    }
}

/// The string argument at `i`.
fn text<'a>(vm: &'a Vm, args: usize, i: usize, who: &str) -> Result<Text<'a>, Error> {
    string_arg(vm, arg(vm, args, i), who).map(Text)
}

fn is_ascii(vm: &Vm, args: usize, i: usize) -> bool {
    unsafe { heap::str_is_ascii(arg(vm, args, i).as_ptr()) }
}

/// The byte offset of cursor or index `c` in the string argument at `s`.
pub(crate) fn offset(vm: &Vm, args: usize, s: usize, c: Value, who: &str) -> Result<usize, Error> {
    let t = text(vm, args, s, who)?;
    if c.is_cursor() {
        let at = c.as_cursor();
        return if t.is_boundary(at) { Ok(at) } else { Err(Error::new(format!("{who}: cursor {at} is not in the string"))) };
    }
    if !c.is_int() {
        return Err(type_error(who, "string cursor or index", c));
    }
    let i = index_arg(c, who)?;
    let at = vm.string_offset(arg(vm, args, s), i);
    at.ok_or_else(|| Error::new(format!("{who}: index {i} is not in the string")))
}

/// The character index of byte offset `at` of the string argument at `s`.
fn index(vm: &Vm, args: usize, s: usize, at: usize) -> usize {
    if is_ascii(vm, args, s) { at } else { Text(unsafe { str_bytes(arg(vm, args, s).as_ptr()) }).str(0, at).chars().count() }
}

/// The offset `n` characters after (or, negative, before) `at`.
fn step(t: Text, at: usize, n: i64, who: &str) -> Result<usize, Error> {
    let found = if n >= 0 { t.forward(at, n as usize) } else { t.back(at, n.unsigned_abs() as usize) };
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
    vm.admit_string(b - a)?;
    let ascii = text(vm, args, 0, "string-copy/cursors")?.str(a, b).is_ascii();
    let p = vm.alloc(heap::string_words(b - a, ascii));
    unsafe { init_string(p, &str_bytes(arg(vm, args, 0).as_ptr())[a..b], ascii) };
    Ok(Value::ptr(p))
}

/// `string-contains` and `string-contains-right` of SRFI 130: the cursor of
/// the first or last STRING2 in STRING1, or #f.
fn contains(vm: &mut Vm, args: usize, n: usize, right: bool, who: &str) -> R {
    let (a1, b1) = range(vm, args, n, 0, 2, who)?;
    let (a2, b2) = range(vm, args, n, 1, 4, who)?;
    let (hay, needle) = (text(vm, args, 0, who)?.str(a1, b1), text(vm, args, 1, who)?.str(a2, b2));
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
            let count = |x: usize, y: usize| if is_ascii(vm, a, 0) { y - x } else { t.str(x, y).chars().count() } as i64;
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
            text(vm, a, 0, "string-ref/cursor")?.char_at(at).map(|(c, _)| Value::char(c))
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

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    /// Exhaustive over arbitrary byte slices up to 8 bytes (possibly invalid UTF-8) and
    /// all 64-bit offsets `at`: `Text::is_boundary` never panics, always returns `false`
    /// past `len`, and on valid UTF-8 slices up to 4 bytes matches `str::is_char_boundary`.
    #[kani::proof]
    #[kani::unwind(6)]
    fn text_is_boundary_matches_utf8_and_handles_arbitrary_bytes() {
        let check_utf8: bool = kani::any();
        if check_utf8 {
            let buf: [u8; 4] = kani::any();
            let len: usize = kani::any();
            kani::assume(len <= buf.len());
            let Ok(s) = std::str::from_utf8(&buf[..len]) else {
                return;
            };
            let t = Text(s.as_bytes());
            let at: usize = kani::any();
            assert!(t.is_boundary(at) == s.is_char_boundary(at));
        } else {
            let buf: [u8; 8] = kani::any();
            let len: usize = kani::any();
            kani::assume(len <= buf.len());
            let t = Text(&buf[..len]);
            let at: usize = kani::any();
            let res = t.is_boundary(at);
            if at > len {
                assert!(!res);
            } else if at == len {
                assert!(res);
            } else {
                // UTF-8 continuation bytes have top two bits `10` (0x80..=0xBF).
                assert!(res == !(0x80..=0xBF).contains(&buf[at]));
            }
        }
    }

    /// Exhaustive over all arbitrary byte windows up to 4 bytes (valid and malformed UTF-8,
    /// including invalid lead bytes, lone continuations, overlong sequences, surrogates, and
    /// truncated multibyte scalars) and offsets `at in 0..=5`: `Text::try_char_at` (using
    /// `utf8_chunks`) returns the exact same `Option<Result<(char, usize), ()>>` as the
    /// original `std::str::from_utf8` valid-prefix rule.
    #[kani::proof]
    #[kani::unwind(6)]
    fn text_try_char_at_matches_from_utf8_on_arbitrary_bytes() {
        let buf: [u8; 4] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= buf.len());
        let at: usize = kani::any();
        kani::assume(at <= 5);

        let slice = &buf[..len];
        let actual = Text(slice).try_char_at(at);
        let expected = slice.get(at..).filter(|r| !r.is_empty()).map(|rest| {
            let window = &rest[..rest.len().min(4)];
            let valid = match std::str::from_utf8(window) {
                Ok(s) => s,
                Err(e) => std::str::from_utf8(&window[..e.valid_up_to()]).unwrap(),
            };
            valid.chars().next().map(|c| (c, c.len_utf8())).ok_or(())
        });

        assert!(actual == expected);
    }

    /// Exhaustive over all pairs of Unicode scalar values `(c0, c1)` (1-, 2-, 3-, and 4-byte
    /// UTF-8), both standalone `c0` and `c0` followed by `c1` (exercising every 4-byte window
    /// truncation combination inside `Text::char_at`): `Text::char_at` decodes `(c0, w0)` at
    /// offset 0 and `(c1, w1)` at the nonzero offset `w0`, returns `None` at the end, and
    /// `Text::is_boundary` holds at character boundaries and fails on every interior
    /// continuation byte.
    #[kani::proof]
    #[kani::unwind(5)]
    fn text_char_at_decodes_all_unicode_scalars_and_truncated_windows() {
        let c0: char = kani::any();
        let with_trailer: bool = kani::any();
        let mut buf = [0u8; 8];
        let w0 = c0.encode_utf8(&mut buf).len();
        let (len, trailer) = if with_trailer {
            let c1: char = kani::any();
            let w1 = c1.encode_utf8(&mut buf[w0..]).len();
            (w0 + w1, Some((c1, w1)))
        } else {
            (w0, None)
        };
        let t = Text(&buf[..len]);

        assert!(t.is_boundary(0));
        assert!(t.is_boundary(w0));
        assert!(t.is_boundary(len));
        for i in 1..4 {
            if i < w0 {
                assert!(!t.is_boundary(i));
            }
        }
        assert!(t.char_at(0) == Some((c0, w0)));
        if let Some((c1, w1)) = trailer {
            assert!(t.char_at(w0) == Some((c1, w1)));
        }
        assert!(t.char_at(len).is_none());
    }

    /// Exhaustive over all 1,112,064 Unicode scalar values `c` (1-, 2-, 3-, and 4-byte UTF-8):
    /// `Text::forward(0, 1)` advances to `c.len_utf8()`, `Text::back(w, 1)` returns `Some(0)`,
    /// zero-step calls are identities, and stepping past either end returns `None`.
    #[kani::proof]
    #[kani::unwind(5)]
    fn text_single_step_forward_and_back_all_unicode_scalars() {
        let c: char = kani::any();
        let mut buf = [0u8; 4];
        let w = c.encode_utf8(&mut buf).len();
        let t = Text(&buf[..w]);

        assert!(t.forward(0, 0) == Some(0));
        assert!(t.forward(w, 0) == Some(w));
        assert!(t.back(0, 0) == Some(0));
        assert!(t.back(w, 0) == Some(w));
        assert!(t.back(0, 1).is_none());
        assert!(t.forward(w, 1).is_none());

        let check_back: bool = kani::any();
        if check_back {
            assert!(t.back(w, 1) == Some(0));
        } else {
            assert!(t.forward(0, 1) == Some(w));
        }
    }

    /// Helper building a 2-character UTF-8 buffer from two arbitrary Unicode scalars
    /// (covering all 1,112,064 × 1,112,064 pairs across 1-, 2-, 3-, and 4-byte encodings).
    fn two_scalar_sample() -> ([u8; 8], [usize; 3]) {
        let c0: char = kani::any();
        let c1: char = kani::any();
        let mut buf = [0u8; 8];
        let w0 = c0.encode_utf8(&mut buf).len();
        let w1 = c1.encode_utf8(&mut buf[w0..]).len();
        (buf, [0, w0, w0 + w1])
    }

    /// Exhaustive over all pairs of Unicode scalars `(c0, c1)` (1-, 2-, 3-, and 4-byte UTF-8),
    /// every character boundary `idx in 0..=2`, and every step count `n in 0..=3`:
    /// `Text::forward` lands on exact character boundaries and returns `None` iff `idx + n > 2`.
    #[kani::proof]
    #[kani::unwind(5)]
    fn text_multi_step_forward_two_scalars() {
        let (buf, bounds) = two_scalar_sample();
        let t = Text(&buf[..bounds[2]]);
        let idx: usize = kani::any();
        kani::assume(idx <= 2);
        let n: usize = kani::any();
        kani::assume(n <= 3);

        let expected = (idx + n <= 2).then(|| bounds[idx + n]);
        assert!(t.forward(bounds[idx], n) == expected);
    }

    /// Exhaustive over all pairs of Unicode scalars `(c0, c1)` (1-, 2-, 3-, and 4-byte UTF-8),
    /// every character boundary `idx in 0..=2`, and every step count `n in 0..=3`:
    /// `Text::back` lands on exact character boundaries and returns `None` iff `n > idx`.
    #[kani::proof]
    #[kani::unwind(5)]
    fn text_multi_step_back_two_scalars() {
        let (buf, bounds) = two_scalar_sample();
        let t = Text(&buf[..bounds[2]]);
        let idx: usize = kani::any();
        kani::assume(idx <= 2);
        let n: usize = kani::any();
        kani::assume(n <= 3);

        let expected = (n <= idx).then(|| bounds[idx - n]);
        assert!(t.back(bounds[idx], n) == expected);
    }
}
