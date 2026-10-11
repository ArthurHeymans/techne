//! The register stack and the globals: values native code keeps the
//! address of while it runs.

use std::ops::{Deref, DerefMut};

use crate::value::Value;

/// A vector of values whose buffer moves only through `push` and
/// `resize`, which count each move in the VM's `Moves` (the VM's fields
/// holding them are private to the crate, so nothing outside replaces
/// them either). Native code holds
/// the addresses of the register stack's and the globals' buffers while
/// it runs, and after calling a Rust native compares the count with the
/// one it started with (`JitCtx::moves`) rather than both addresses.
/// (Swapping a task's register stack in or out moves no buffer: a native
/// returns with the stack it was called with swapped in.)
#[derive(Default)]
pub(crate) struct Slots(Vec<Value>);

/// How often the buffers of the VM's `Slots` moved (`Vm::moves`).
#[derive(Default, Clone, Copy, PartialEq, Debug)]
#[repr(transparent)]
pub struct Moves(u64);

impl Slots {
    /// `n` copies of `v`.
    #[inline]
    pub(crate) fn filled(v: Value, n: usize) -> Slots {
        Slots(vec![v; n])
    }

    #[inline]
    pub(crate) fn push(&mut self, v: Value, moves: &mut Moves) {
        let before = self.0.as_ptr();
        self.0.push(v);
        moves.count(before, self.0.as_ptr());
    }

    #[inline]
    pub(crate) fn resize(&mut self, len: usize, v: Value, moves: &mut Moves) {
        let before = self.0.as_ptr();
        self.0.resize(len, v);
        moves.count(before, self.0.as_ptr());
    }
}

impl Moves {
    #[inline]
    fn count(&mut self, before: *const Value, after: *const Value) {
        if before != after {
            self.0 += 1;
        }
    }
}

impl Deref for Slots {
    type Target = [Value];
    #[inline(always)]
    fn deref(&self) -> &[Value] {
        &self.0
    }
}

impl DerefMut for Slots {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut [Value] {
        &mut self.0
    }
}
