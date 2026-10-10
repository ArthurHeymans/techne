//! Metering reading, macro expansion and compilation (PLAN.md, Stage 1,
//! step 8.3e). They run as Rust, with no check of the VM's, so a macro
//! that expands for ever would not be stopped; text reads as far more
//! than it takes, one expansion may copy its input many times, a
//! constant may hold one string many times, and `eval` turns shared data
//! into a tree. A meter counts the parts they make, and every so many, or
//! after a long copy, looks for a stop pending for the execution and at
//! what the world holds: what they make is admitted as an allocation is
//! (`Vm::admit`), up to the room left when they began (or `SMALL`).

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

/// Copies of at least so many bytes are looked after (`Meter::look`):
/// `STEPS` shorter ones come to 4 MB at most.
pub(crate) const LONG: usize = 4096;

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
            halt: Cell::new(None),
        }
    }

    /// A step: every `STEPS`, `look`.
    #[inline]
    pub(crate) fn step(&self) -> Result<(), String> {
        let n = self.steps.get().wrapping_add(1);
        self.steps.set(n);
        if n.is_multiple_of(STEPS) || self.halt.get().is_some() { self.look() } else { Ok(()) }
    }

    /// Halt if a stop is pending for the execution, or if the world grew
    /// past the room it had (after a copy, which may be large). Once halted
    /// it stays so.
    pub(crate) fn look(&self) -> Result<(), String> {
        if self.halt.get().is_none() {
            let held = self.account.held();
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
