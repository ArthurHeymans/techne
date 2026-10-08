//! NaN-boxed values.
//!
//! A `Value` is one `u64`. Every non-NaN `f64` is stored as itself; NaNs are
//! canonicalised to `CANONICAL_NAN`. All other values live in the negative
//! quiet-NaN space above `0xFFF8 << 48`, distinguished by the top 16 bits:
//!
//! | top 16 bits | payload (low 48 bits)                 |
//! |-------------|---------------------------------------|
//! | `0xFFF9`    | 48-bit two's complement fixnum        |
//! | `0xFFFA`    | heap object address (8-byte aligned)  |
//! | `0xFFFB`    | character (Unicode scalar)            |
//! | `0xFFFC`    | interned symbol id                    |
//! | `0xFFFD`    | special constant (see `Special`), or  |
//! |             | with bit 47 set a string cursor       |
//! | `0xFFFE`    | native (Rust) procedure index         |
//! | `0xFFFF`    | keyword (`#:name`), symbol id of name |
//!
//! Values carry no destructor: memory is owned by the tracing GC in `heap`.

use std::fmt;

const TAG_SHIFT: u32 = 48;
const PAYLOAD: u64 = (1 << TAG_SHIFT) - 1;
const TAG_INT: u64 = 0xFFF9;
const TAG_PTR: u64 = 0xFFFA;
const TAG_CHAR: u64 = 0xFFFB;
const TAG_SYM: u64 = 0xFFFC;
const TAG_SPECIAL: u64 = 0xFFFD;
const TAG_NATIVE: u64 = 0xFFFE;
const TAG_KEYWORD: u64 = 0xFFFF;
const CANONICAL_NAN: u64 = 0x7FF8_0000_0000_0000;
/// The payload bit of a special that makes it a string cursor.
const CURSOR: u64 = 1 << 47;

pub const FIXNUM_MIN: i64 = -(1 << 47);
pub const FIXNUM_MAX: i64 = (1 << 47) - 1;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Value(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Special {
    Nil = 0,
    False = 1,
    True = 2,
    Void = 3,
    Eof = 4,
    /// Unassigned global or letrec slot.
    Undefined = 5,
    /// Empty hash-table slot; never visible to Scheme code.
    Empty = 6,
    /// An optional/keyword argument that was not supplied.
    Unset = 7,
}

impl Value {
    pub const NIL: Value = Value::special(Special::Nil);
    pub const FALSE: Value = Value::special(Special::False);
    pub const TRUE: Value = Value::special(Special::True);
    pub const VOID: Value = Value::special(Special::Void);
    pub const EOF: Value = Value::special(Special::Eof);
    pub const UNDEFINED: Value = Value::special(Special::Undefined);
    pub const EMPTY: Value = Value::special(Special::Empty);
    pub const UNSET: Value = Value::special(Special::Unset);

    #[inline(always)]
    const fn tagged(tag: u64, payload: u64) -> Value {
        Value((tag << TAG_SHIFT) | (payload & PAYLOAD))
    }
    #[inline(always)]
    fn tag(self) -> u64 {
        self.0 >> TAG_SHIFT
    }
    #[inline(always)]
    pub fn bits(self) -> u64 {
        self.0
    }
    /// Only for values previously obtained from `bits` (e.g. raw heap words).
    #[inline(always)]
    pub fn from_bits(bits: u64) -> Value {
        Value(bits)
    }

    pub const fn special(s: Special) -> Value {
        Value::tagged(TAG_SPECIAL, s as u64)
    }
    #[inline(always)]
    pub fn bool(b: bool) -> Value {
        if b { Value::TRUE } else { Value::FALSE }
    }
    #[inline(always)]
    pub fn is_false(self) -> bool {
        self == Value::FALSE
    }
    #[inline(always)]
    pub fn is_truthy(self) -> bool {
        self != Value::FALSE
    }

    #[inline(always)]
    pub fn float(f: f64) -> Value {
        if f.is_nan() { Value(CANONICAL_NAN) } else { Value(f.to_bits()) }
    }
    #[inline(always)]
    pub fn is_float(self) -> bool {
        self.tag() < TAG_INT
    }
    #[inline(always)]
    pub fn as_float(self) -> f64 {
        f64::from_bits(self.0)
    }

    /// Fixnum if `i` fits in 48 bits; callers handle overflow (see `num`).
    #[inline(always)]
    pub fn fixnum(i: i64) -> Option<Value> {
        (FIXNUM_MIN..=FIXNUM_MAX).contains(&i).then(|| Value::tagged(TAG_INT, i as u64))
    }
    #[inline(always)]
    pub fn int_unchecked(i: i64) -> Value {
        debug_assert!((FIXNUM_MIN..=FIXNUM_MAX).contains(&i));
        Value::tagged(TAG_INT, i as u64)
    }
    #[inline(always)]
    pub fn is_int(self) -> bool {
        self.tag() == TAG_INT
    }
    #[inline(always)]
    pub fn as_int(self) -> i64 {
        ((self.0 << 16) as i64) >> 16
    }
    /// Both fixnums: one combined tag check.
    #[inline(always)]
    pub fn both_int(a: Value, b: Value) -> bool {
        (a.0 >> TAG_SHIFT) == TAG_INT && (b.0 >> TAG_SHIFT) == TAG_INT
    }

    /// A non-GC address (e.g. a boxed `Code`), stored with the integer tag so
    /// the collector leaves it alone. Unlike a fixnum, its 48-bit payload is
    /// unsigned: bit 47 can be set in an ARM64 userspace address.
    #[inline(always)]
    pub fn untraced_ptr<T>(addr: *const T) -> Value {
        debug_assert!(addr as u64 <= PAYLOAD);
        Value::tagged(TAG_INT, addr as u64)
    }
    #[inline(always)]
    pub fn as_untraced_ptr<T>(self) -> *const T {
        debug_assert!(self.is_int());
        (self.0 & PAYLOAD) as *const T
    }

    #[inline(always)]
    pub fn ptr(addr: *mut u64) -> Value {
        debug_assert!(addr as u64 & 7 == 0 && addr as u64 <= PAYLOAD);
        Value::tagged(TAG_PTR, addr as u64)
    }
    #[inline(always)]
    pub fn is_ptr(self) -> bool {
        self.tag() == TAG_PTR
    }
    #[inline(always)]
    pub fn as_ptr(self) -> *mut u64 {
        (self.0 & PAYLOAD) as *mut u64
    }

    pub fn char(c: char) -> Value {
        Value::tagged(TAG_CHAR, c as u64)
    }
    pub fn is_char(self) -> bool {
        self.tag() == TAG_CHAR
    }
    pub fn as_char(self) -> char {
        char::from_u32((self.0 & PAYLOAD) as u32).expect("valid char payload")
    }

    pub fn symbol(id: u32) -> Value {
        Value::tagged(TAG_SYM, id as u64)
    }
    pub fn is_symbol(self) -> bool {
        self.tag() == TAG_SYM
    }
    pub fn as_symbol(self) -> u32 {
        (self.0 & PAYLOAD) as u32
    }

    pub fn keyword(id: u32) -> Value {
        Value::tagged(TAG_KEYWORD, id as u64)
    }
    pub fn is_keyword(self) -> bool {
        self.tag() == TAG_KEYWORD
    }
    pub fn as_keyword(self) -> u32 {
        (self.0 & PAYLOAD) as u32
    }

    pub fn native(index: u32) -> Value {
        Value::tagged(TAG_NATIVE, index as u64)
    }
    #[inline(always)]
    pub fn is_native(self) -> bool {
        self.tag() == TAG_NATIVE
    }
    #[inline(always)]
    pub fn as_native(self) -> usize {
        (self.0 & PAYLOAD) as usize
    }

    /// A string cursor: a byte offset into a string's UTF-8, which the
    /// string itself does not record.
    #[inline(always)]
    pub fn cursor(offset: usize) -> Value {
        debug_assert!((offset as u64) < CURSOR);
        Value::tagged(TAG_SPECIAL, CURSOR | offset as u64)
    }
    #[inline(always)]
    pub fn is_cursor(self) -> bool {
        self.tag() == TAG_SPECIAL && self.0 & CURSOR != 0
    }
    #[inline(always)]
    pub fn as_cursor(self) -> usize {
        (self.0 & (CURSOR - 1)) as usize
    }

    pub fn as_special(self) -> Option<Special> {
        if self.tag() != TAG_SPECIAL || self.is_cursor() {
            return None;
        }
        Some(match self.0 & PAYLOAD {
            0 => Special::Nil,
            1 => Special::False,
            2 => Special::True,
            3 => Special::Void,
            4 => Special::Eof,
            5 => Special::Undefined,
            6 => Special::Empty,
            _ => Special::Unset,
        })
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_float() {
            write!(f, "Float({:?})", self.as_float())
        } else if self.is_int() {
            write!(f, "Int({})", self.as_int())
        } else if self.is_ptr() {
            write!(f, "Ptr({:p})", self.as_ptr())
        } else if self.is_char() {
            write!(f, "Char({:?})", self.as_char())
        } else if self.is_symbol() {
            write!(f, "Sym({})", self.as_symbol())
        } else if self.is_native() {
            write!(f, "Native({})", self.as_native())
        } else if self.is_keyword() {
            write!(f, "Keyword({})", self.as_keyword())
        } else if self.is_cursor() {
            write!(f, "Cursor({})", self.as_cursor())
        } else {
            write!(f, "{:?}", self.as_special())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untraced_addresses_round_trip() {
        for addr in [0x1234_5678_u64, 0x7fff_ffff_fff8, 0x8000_0000_0000, 0xffff_ffff_fff8] {
            let ptr = addr as *const u64;
            let v = Value::untraced_ptr(ptr);
            assert!(!v.is_ptr(), "the GC must not trace this address");
            assert_eq!(v.as_untraced_ptr::<u64>(), ptr);
        }
    }

    #[test]
    fn round_trips() {
        for i in [0, 1, -1, FIXNUM_MIN, FIXNUM_MAX, 123456789] {
            let v = Value::fixnum(i).unwrap();
            assert!(v.is_int() && !v.is_float() && v.as_int() == i);
        }
        assert!(Value::fixnum(FIXNUM_MAX + 1).is_none());
        for f in [0.0, -0.0, 1.5, f64::INFINITY, f64::NEG_INFINITY, f64::MIN_POSITIVE] {
            let v = Value::float(f);
            assert!(v.is_float() && v.as_float().to_bits() == f.to_bits());
        }
        let nan = Value::float(f64::from_bits(0xFFFF_FFFF_FFFF_FFFF));
        assert!(nan.is_float() && nan.as_float().is_nan());
        assert_eq!(Value::char('λ').as_char(), 'λ');
        assert!(Value::NIL.as_special() == Some(Special::Nil) && !Value::NIL.is_float());
        assert!(Value::both_int(Value::int_unchecked(3), Value::int_unchecked(-4)));
        assert!(!Value::both_int(Value::int_unchecked(3), Value::float(1.0)));
    }
}
