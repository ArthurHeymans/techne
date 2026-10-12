//! The register stack and the globals: values native code keeps the
//! address of while it runs.

use std::{
    collections::TryReserveError,
    ops::{Deref, DerefMut},
};

use crate::{alloc::Budget, value::Value};

/// A vector of values whose buffer moves only through `push` and
/// `try_resize`, which count each move in the VM's `Moves` (the VM's fields
/// holding them are private to the crate, so nothing outside replaces
/// them either). Native code holds
/// the addresses of the register stack's and the globals' buffers while
/// it runs, and after calling a Rust native compares the count with the
/// one it started with (`JitCtx::moves`) rather than both addresses.
/// (Swapping a task's register stack in or out moves no buffer: a native
/// returns with the stack it was called with swapped in.)
pub(crate) struct Slots(Vec<Value, Budget>);

/// How often the buffers of the VM's `Slots` moved (`Vm::moves`).
#[derive(Default, Clone, Copy, PartialEq, Debug)]
#[repr(transparent)]
pub struct Moves(u64);

impl Slots {
    pub(crate) fn new_in(budget: Budget) -> Self {
        Self(Vec::new_in(budget))
    }

    /// `n` copies of `v`, with exactly the requested capacity.
    pub(crate) fn filled_in(v: Value, n: usize, budget: Budget) -> Result<Self, TryReserveError> {
        let mut slots = Self::new_in(budget);
        slots.try_resize(n, v, &mut Moves::default())?;
        Ok(slots)
    }

    pub(crate) fn budget(&self) -> &Budget {
        self.0.allocator()
    }

    #[inline]
    pub(crate) fn push(&mut self, v: Value, moves: &mut Moves) {
        let before = self.0.as_ptr();
        self.0.push(v);
        moves.count(before, self.0.as_ptr());
    }

    /// Failure leaves contents, capacity, address and `moves` unchanged.
    /// Exact reservation keeps the VM's admission estimate accurate.
    #[inline]
    pub(crate) fn try_resize(&mut self, len: usize, v: Value, moves: &mut Moves) -> Result<(), TryReserveError> {
        let before = self.0.as_ptr();
        if len > self.0.len() {
            self.0.try_reserve_exact(len - self.0.len())?;
        }
        self.0.resize(len, v);
        moves.count(before, self.0.as_ptr());
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growth_failure_preserves_slots_and_moves() {
        let budget = Budget::new(4 * size_of::<Value>());
        let mut slots = Slots::filled_in(Value::TRUE, 4, budget.clone()).unwrap();
        let mut moves = Moves::default();
        let before = slots.as_ptr();
        assert!(slots.try_resize(8, Value::VOID, &mut moves).is_err());
        assert_eq!(&*slots, &[Value::TRUE; 4]);
        assert_eq!(slots.as_ptr(), before);
        assert_eq!(slots.0.capacity(), 4);
        assert_eq!(moves, Moves::default());
        assert_eq!(budget.held(), 4 * size_of::<Value>());
        budget.set_limit(8 * size_of::<Value>());
        slots.try_resize(8, Value::VOID, &mut moves).unwrap();
        assert_eq!(&slots[..4], &[Value::TRUE; 4]);
        assert_eq!(&slots[4..], &[Value::VOID; 4]);
        assert_eq!(moves.0, u64::from(before != slots.as_ptr()));
        assert_eq!(budget.held(), 8 * size_of::<Value>());
        drop(slots);
        assert_eq!(budget.held(), 0);
    }

    #[test]
    fn shrinking_and_regrowing_reuse_capacity_above_a_lowered_limit() {
        let budget = Budget::new(4 * size_of::<Value>());
        let mut slots = Slots::filled_in(Value::TRUE, 4, budget.clone()).unwrap();
        budget.set_limit(0);
        let mut moves = Moves::default();
        slots.try_resize(0, Value::VOID, &mut moves).unwrap();
        slots.try_resize(4, Value::FALSE, &mut moves).unwrap();
        assert_eq!(&*slots, &[Value::FALSE; 4]);
        assert_eq!(moves, Moves::default());
        assert_eq!(budget.held(), 4 * size_of::<Value>());
        assert!(Slots::filled_in(Value::VOID, usize::MAX, budget.clone()).is_err());
        drop(slots);
        assert_eq!(budget.held(), 0);
    }
}
