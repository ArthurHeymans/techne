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
//! A pointer payload is an address in its world's cage (`alloc::cage`),
//! and becomes a pointer again through the cage of the world entered, with
//! its provenance: only its offset bits count, so whatever the payload,
//! the pointer is into that cage.

use std::{fmt, marker::PhantomData};

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

/// Not `Send`: a heap value's address means something only in its world's
/// cage, which is decoded on its world's thread.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub(crate) struct Value(u64, PhantomData<*const ()>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub(crate) enum Special {
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
        Value((tag << TAG_SHIFT) | (payload & PAYLOAD), PhantomData)
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
    pub(crate) fn from_bits(bits: u64) -> Value {
        Value(bits, PhantomData)
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
        if f.is_nan() { Value(CANONICAL_NAN, PhantomData) } else { Value(f.to_bits(), PhantomData) }
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

    #[inline(always)]
    pub(crate) fn ptr(addr: *mut u64) -> Value {
        debug_assert!(addr.addr() & 7 == 0 && crate::alloc::in_cage(addr), "not a heap object: {addr:p}");
        Value::tagged(TAG_PTR, addr.addr() as u64)
    }
    #[inline(always)]
    pub fn is_ptr(self) -> bool {
        self.tag() == TAG_PTR
    }
    #[inline(always)]
    pub(crate) fn as_ptr(self) -> *mut u64 {
        crate::alloc::caged(self.0 as usize)
    }
    /// As `as_ptr`, through the cage `p` points into: cheaper where such a
    /// pointer is at hand.
    #[inline(always)]
    pub(crate) fn as_ptr_near(self, p: *mut u64) -> *mut u64 {
        crate::alloc::near(p, self.0 as usize)
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
    fn caged_addresses_keep_access_to_live_storage() {
        use crate::alloc::{Budget, Cage, Caged};
        let cage = Cage::new();
        let _entered = crate::alloc::enter_cage(&cage, &Default::default());
        let mut storage: Vec<u64, Caged> = Vec::with_capacity_in(1, Caged::new(Budget::new(usize::MAX), cage.clone()));
        storage.push(42);
        let traced = Value::ptr(storage.as_mut_ptr());
        assert!(traced.is_ptr());
        // SAFETY: the payload came from this still-live allocation, with
        // no conflicting reference accesses between conversion and use.
        unsafe {
            assert_eq!(*traced.as_ptr(), 42);
            *traced.as_ptr() = 7;
        }
        assert_eq!(storage[0], 7);
        // A forged payload still names the cage.
        let forged = Value::tagged(TAG_PTR, traced.0 ^ (1 << 47));
        assert!(crate::alloc::in_cage(crate::alloc::confine(cage.base(), forged.0 as usize)));
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

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    /// Exhaustive over all 2^64 bit patterns: the Value tag predicates partition the
    /// entire 64-bit representation into 9 mutually exclusive, collectively exhaustive
    /// categories, and truthiness agrees with `!= Value::FALSE`.
    #[kani::proof]
    fn tag_partition_is_exhaustive_and_disjoint() {
        let bits: u64 = kani::any();
        let v = Value::from_bits(bits);

        let count = usize::from(v.is_float())
            + usize::from(v.is_int())
            + usize::from(v.is_ptr())
            + usize::from(v.is_char())
            + usize::from(v.is_symbol())
            + usize::from(v.is_native())
            + usize::from(v.is_keyword())
            + usize::from(v.is_cursor())
            + usize::from(v.as_special().is_some());
        assert!(count == 1);

        assert!(v.is_false() == (v == Value::FALSE));
        assert!(v.is_truthy() == !v.is_false());

        let b: bool = kani::any();
        let bv = Value::bool(b);
        assert!(bv.is_truthy() == b);
        assert!(bv.is_false() == !b);
        assert!(bv.as_special() == Some(if b { Special::True } else { Special::False }));
    }

    /// Exhaustive over all 2^64 IEEE-754 f64 bit patterns: every NaN (including negative
    /// quiet/signaling NaNs in 0xFFF8..=0xFFFF) canonicalizes to `CANONICAL_NAN`, every
    /// non-NaN preserves its exact bits (including -0.0, subnormals, and infinities), and
    /// no boxed float ever collides with any tagged non-float category.
    #[kani::proof]
    fn float_canonicalizes_nans_and_preserves_non_nans() {
        let bits: u64 = kani::any();
        let f = f64::from_bits(bits);
        let v = Value::float(f);

        assert!(v.is_float());
        assert!(!v.is_int() && !v.is_ptr() && !v.is_char() && !v.is_symbol() && !v.is_native() && !v.is_keyword() && !v.is_cursor());
        assert!(v.as_special().is_none());

        if f.is_nan() {
            assert!(v.bits() == CANONICAL_NAN);
            assert!(v.as_float().is_nan());
        } else {
            assert!(v.bits() == bits);
            assert!(v.as_float().to_bits() == bits);
        }
    }

    /// Exhaustive over all 2^64 signed integers and all pairs of 64-bit values:
    /// `Value::fixnum` succeeds iff the value lies in `FIXNUM_MIN..=FIXNUM_MAX`, round-trips
    /// with sign extension through `as_int`, matches `int_unchecked`, and `both_int` agrees
    /// with checking `is_int` on each operand.
    #[kani::proof]
    fn fixnum_roundtrip_range_and_both_int() {
        let i: i64 = kani::any();
        match Value::fixnum(i) {
            Some(v) => {
                assert!(i >= FIXNUM_MIN && i <= FIXNUM_MAX);
                assert!(v.is_int());
                assert!(!v.is_float());
                assert!(v.as_int() == i);
                assert!(Value::int_unchecked(i) == v);
            }
            None => {
                assert!(i < FIXNUM_MIN || i > FIXNUM_MAX);
            }
        }

        let a = Value::from_bits(kani::any());
        let b = Value::from_bits(kani::any());
        assert!(Value::both_int(a, b) == (a.is_int() && b.is_int()));
    }

    /// Exhaustive over all `char`, `u32` ids, in-range cursor offsets, and `Special`
    /// constants: every immediate constructor produces the exact documented NaN-boxed word
    /// and round-trips its payload.
    #[kani::proof]
    fn immediates_roundtrip_all_payloads() {
        let c: char = kani::any();
        let vc = Value::char(c);
        assert!(vc.bits() == (0xFFFB_u64 << 48) | (c as u64));
        assert!(vc.is_char() && !vc.is_float() && vc.as_char() == c);

        let id: u32 = kani::any();
        let vs = Value::symbol(id);
        assert!(vs.bits() == (0xFFFC_u64 << 48) | (id as u64));
        assert!(vs.is_symbol() && !vs.is_float() && vs.as_symbol() == id);

        let vk = Value::keyword(id);
        assert!(vk.bits() == (0xFFFF_u64 << 48) | (id as u64));
        assert!(vk.is_keyword() && !vk.is_float() && vk.as_keyword() == id);

        let vn = Value::native(id);
        assert!(vn.bits() == (0xFFFE_u64 << 48) | (id as u64));
        assert!(vn.is_native() && !vn.is_float() && vn.as_native() == id as usize);

        let off: usize = kani::any();
        kani::assume((off as u64) < CURSOR);
        let vcur = Value::cursor(off);
        assert!(vcur.bits() == (0xFFFD_u64 << 48) | (1u64 << 47) | (off as u64));
        assert!(vcur.is_cursor() && !vcur.is_float() && vcur.as_special().is_none() && vcur.as_cursor() == off);

        let special_idx: u8 = kani::any();
        kani::assume(special_idx < 8);
        let (sp, constant) = match special_idx {
            0 => (Special::Nil, Value::NIL),
            1 => (Special::False, Value::FALSE),
            2 => (Special::True, Value::TRUE),
            3 => (Special::Void, Value::VOID),
            4 => (Special::Eof, Value::EOF),
            5 => (Special::Undefined, Value::UNDEFINED),
            6 => (Special::Empty, Value::EMPTY),
            _ => (Special::Unset, Value::UNSET),
        };
        let vsp = Value::special(sp);
        assert!(vsp.bits() == (0xFFFD_u64 << 48) | (sp as u64));
        assert!(vsp == constant);
        assert!(!vsp.is_cursor() && !vsp.is_float());
        assert!(vsp.as_special() == Some(sp));
    }
}
