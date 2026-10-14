//! Metering reading, macro expansion and compilation (PLAN.md, Stage 1,
//! step 8.3e). They run as Rust, with no check of the VM's, so a macro
//! that expands for ever would not be stopped; text reads as far more
//! than it takes, one expansion may copy its input many times, a
//! constant may hold one string many times, and `eval` turns shared data
//! into a tree. A meter counts the parts they make, and every so many, or
//! after a long copy, looks for a stop pending for the execution and at
//! what the world holds: what they make is admitted as an allocation is
//! (`Vm::admit`), up to the room left when they began (or `SMALL`).
//!
//! What the world holds is its account's count. With the counting
//! allocator that count sees the Rust allocations metering makes;
//! without it (the library default) it does not, so the meter ALSO
//! estimates what it made itself (`PART_BYTES` a part, copies at their
//! size) and refuses past the room on the estimate. The estimate is
//! cumulative where the account's is current-held, so without
//! interception refusals come somewhat earlier; with interception the
//! estimate is ignored and behaviour is exactly the account's.

use std::{
    cell::Cell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use super::{Error, ExecId, Vm, held::SMALL};
use crate::{alloc::AccountRef, stop::Stops};

/// Steps between looks.
const STEPS: u32 = 1024;

/// So many cumulatively copied bytes are looked after (`Meter::look`):
/// many shorter copies (a string of one-byte escapes, say) look as
/// often as one long copy does.
pub(crate) const LONG: usize = 4096;

/// What one part made while reading, expanding or compiling is estimated
/// to hold when no counting allocator reports it: a syntax node and its
/// share of the vectors and strings around it. Deliberately rough — it
/// bounds, it does not account (see the module's tiers).
pub(crate) const PART_BYTES: usize = 96;

/// The error a metered step returns once halted; `Meter::error` has the
/// one to raise.
pub(crate) const HALTED: &str = "halted";

pub(crate) struct Meter {
    attention: Arc<AtomicBool>,
    stops: Arc<Stops>,
    exec: ExecId,
    account: AccountRef,
    /// What the world held when it began, and past which it is refused.
    start: usize,
    end: usize,
    steps: Cell<u32>,
    /// Estimated bytes made (parts at `PART_BYTES`, copies at their
    /// size); the refusal basis when no counting allocator is installed.
    made: Cell<usize>,
    /// Copied bytes since the last `look`.
    since: Cell<usize>,
    halt: Cell<Option<Halt>>,
}

#[derive(Clone, Copy)]
enum Halt {
    Stopped,
    /// What it made.
    Memory(usize),
}

impl Meter {
    pub(crate) fn new(vm: &Vm) -> Meter {
        let account = vm.account.reference();
        let start = account.held();
        Meter {
            attention: vm.interrupt.clone(),
            stops: vm.stops.clone(),
            exec: vm.running_execution(),
            account,
            start,
            end: start.saturating_add(vm.room().max(SMALL)),
            steps: Cell::new(0),
            made: Cell::new(0),
            since: Cell::new(0),
            halt: Cell::new(None),
        }
    }

    /// A step: every `STEPS`, `look`.
    #[inline]
    pub(crate) fn step(&self) -> Result<(), String> {
        self.made.set(self.made.get().saturating_add(PART_BYTES));
        let n = self.steps.get().wrapping_add(1);
        self.steps.set(n);
        if n.is_multiple_of(STEPS) || self.halt.get().is_some() { self.look() } else { Ok(()) }
    }

    /// `bytes` copied (a long string read, one escape of it, a bound
    /// value cloned into a template, a JSON string): estimated at
    /// their size, and looked at once copies come to `LONG` since the
    /// last look, however small each was.
    pub(crate) fn copied(&self, bytes: usize) -> Result<(), String> {
        self.made.set(self.made.get().saturating_add(bytes));
        let since = self.since.get().saturating_add(bytes);
        self.since.set(since);
        if since >= LONG || self.halt.get().is_some() { self.look() } else { Ok(()) }
    }

    /// Look before making something of `bytes` in one piece (a vector of
    /// many parts grows by doubling, between looks): halt if the world
    /// holds so much that it would pass the room.
    pub(crate) fn reserve(&self, bytes: usize) -> Result<(), String> {
        self.check(bytes)
    }

    /// Halt if a stop is pending for the execution, or if the world grew
    /// past the room it had (after a copy, which may be large). Once halted
    /// it stays so.
    pub(crate) fn look(&self) -> Result<(), String> {
        self.check(0)
    }

    /// `look`, with `bytes` about to be made.
    fn check(&self, bytes: usize) -> Result<(), String> {
        self.since.set(0);
        if self.halt.get().is_none() {
            // With interception the account is the truth. Without it the
            // account still moves for explicit Budget charges (heap
            // buffers made while compiling), so refuse on whichever of
            // the account and the estimate is further.
            let held = if cfg!(feature = "counting-allocator") {
                self.account.held()
            } else {
                self.account.held().max(self.start.saturating_add(self.made.get()))
            }
            .saturating_add(bytes);
            if self.attention.load(Ordering::Relaxed) && self.stops.pending(self.exec).is_some() {
                self.halt.set(Some(Halt::Stopped));
            } else if held > self.end {
                self.halt.set(Some(Halt::Memory(held - self.start)));
            }
        }
        if self.halt.get().is_some() { Err(HALTED.into()) } else { Ok(()) }
    }

    /// The error to raise if it halted: the stop's, or a refusal.
    pub(crate) fn error(&self, vm: &mut Vm) -> Option<Error> {
        self.halt.get().map(|h| match h {
            Halt::Stopped => vm.take_interrupt(),
            Halt::Memory(made) => vm.refusal(made),
        })
    }
}
