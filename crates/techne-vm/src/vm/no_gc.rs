//! Heap access between collection, reentry and register-growth boundaries.
//!
//! The callback's invariant lifetime brands its values. Neither a value nor
//! a borrowed heap slice can leave the callback or enter another VM's scope.
//! This context deliberately exposes no `&mut Vm` or collecting operation.
//! Reserved nursery allocation returns `None` rather than collecting.

use std::marker::PhantomData;

use super::Vm;
use crate::{
    heap::{self, Kind, field, header, set_field},
    value::Value,
};

// Invariant, not merely a borrow that Rust could shorten to another scope.
type Brand<'gc> = PhantomData<fn(&'gc ()) -> &'gc ()>;

/// Scoped heap access. Created only by [`Vm::with_heap`].
pub struct HeapAccess<'gc> {
    vm: &'gc mut Vm,
    _brand: Brand<'gc>,
}

/// A value that can only be inspected while its heap-access scope is live.
/// Unlike raw `Value`, it cannot be carried across a collection boundary.
#[derive(Clone, Copy)]
pub struct HeapValue<'gc> {
    value: Value,
    /// Its world's cage, which it is decoded in whichever world is
    /// entered (another's, in a nested access).
    cage: *mut u64,
    brand: Brand<'gc>,
}

impl<'gc> HeapAccess<'gc> {
    #[inline(always)]
    pub(super) fn new(vm: &'gc mut Vm) -> Self {
        Self { vm, _brand: PhantomData }
    }

    #[inline(always)]
    fn value(&self, value: Value) -> HeapValue<'gc> {
        HeapValue { value, cage: self.vm.heap.cage_base().cast(), brand: PhantomData }
    }

    /// A value the host holds, if it is of this world. A heap object is
    /// rooted by it, and nothing moves it while the scope lives.
    pub fn import(&self, value: &crate::api::Value) -> Option<HeapValue<'gc>> {
        self.vm.import_value(value).ok().map(|v| self.value(v))
    }

    /// A raw value, for the VM's tests.
    #[cfg(test)]
    pub(crate) fn immediate(&self, value: Value) -> Option<HeapValue<'gc>> {
        (!value.is_ptr()).then(|| self.value(value))
    }

    /// Allocate a pair only if the reserved nursery has room. Does not
    /// collect, grow registers, call a native, or enter Scheme.
    #[inline(always)]
    pub fn try_cons(&mut self, a: HeapValue<'gc>, b: HeapValue<'gc>) -> Option<HeapValue<'gc>> {
        if !self.vm.heap.has_room(3) {
            return None;
        }
        let p = self.vm.heap.bump(3);
        // The fresh reservation is disjoint from every existing heap view.
        Some(unsafe { self.initialize_pair(p, a, b) })
    }

    /// Copy UTF-8 into the reserved nursery, or return `None` without
    /// collecting when it does not fit.
    #[inline(always)]
    pub fn try_string(&mut self, text: &str) -> Option<HeapValue<'gc>> {
        let words = heap::string_words(text.len());
        if !self.vm.heap.has_room(words) {
            return None;
        }
        let p = self.vm.heap.bump(words);
        // The source is valid UTF-8 and the reservation holds its full copy.
        unsafe { super::init_string(p, text.as_bytes()) };
        Some(self.value(Value::ptr(p)))
    }

    /// Read-only UTF-8. The reference borrows this access context, so it
    /// also excludes mutation and allocation through the context until its
    /// last use. Copy the text when work must cross that borrow boundary.
    #[inline(always)]
    pub fn as_str(&self, value: HeapValue<'gc>) -> Option<&str> {
        value.is(Kind::String).then(|| heap::utf8(unsafe { heap::str_bytes(value.ptr()) }))
    }

    #[inline(always)]
    pub(super) fn register(&self, index: usize) -> HeapValue<'gc> {
        self.value(self.vm.regs[index])
    }

    #[inline(always)]
    pub(super) fn store(&mut self, index: usize, value: HeapValue<'gc>) {
        self.vm.regs[index] = value.value;
    }

    /// # Safety
    /// `p` is a fresh, uninitialised three-word allocation in this VM. It
    /// has not crossed a collecting boundary since allocation.
    #[inline(always)]
    pub(super) unsafe fn initialize_pair(&mut self, p: *mut u64, a: HeapValue<'gc>, b: HeapValue<'gc>) -> HeapValue<'gc> {
        unsafe {
            *p = header(Kind::Pair, 2, 0);
            set_field(p, 0, a.value);
            set_field(p, 1, b.value);
        }
        self.value(Value::ptr(p))
    }

    /// # Safety
    /// `p` is a fresh, uninitialised two-word allocation in this VM. It
    /// has not crossed a collecting boundary since allocation.
    #[inline(always)]
    pub(super) unsafe fn initialize_box(&mut self, p: *mut u64, value: HeapValue<'gc>) -> HeapValue<'gc> {
        unsafe {
            *p = header(Kind::Box, 1, 0);
            set_field(p, 0, value.value);
        }
        self.value(Value::ptr(p))
    }
}

impl<'gc> HeapValue<'gc> {
    /// Its object's address, in its world's cage.
    #[inline(always)]
    fn ptr(&self) -> *mut u64 {
        self.value.as_ptr_near(self.cage)
    }

    /// Whether it is an object of kind `k`, read in its world's cage.
    #[inline(always)]
    fn is(&self, k: Kind) -> bool {
        self.value.is_ptr() && unsafe { heap::kind_of(self.ptr()) } == k as u8
    }

    #[inline(always)]
    pub fn as_int(self) -> Option<i64> {
        self.value.is_int().then(|| self.value.as_int())
    }

    #[inline(always)]
    pub fn as_float(self) -> Option<f64> {
        self.value.is_float().then(|| self.value.as_float())
    }

    #[inline(always)]
    pub fn as_pair(self) -> Option<(Self, Self)> {
        self.is(Kind::Pair).then(|| unsafe {
            let p = self.ptr();
            (Self { value: field(p, 0), ..self }, Self { value: field(p, 1), ..self })
        })
    }

    #[inline(always)]
    pub fn as_box(self) -> Option<Self> {
        self.is(Kind::Box).then(|| unsafe { Self { value: field(self.ptr(), 0), ..self } })
    }
}
