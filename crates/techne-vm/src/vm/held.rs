//! What a world holds, against its memory limit (PLAN.md, Stage 1,
//! language step 8.3): what the allocator charged to its account
//! (`crate::alloc`), the heap included, and the JIT's machine code, which
//! is mapped apart. Growth of a known size is admitted before it is
//! allocated.
//!
//! The rest is caught where memory grows: collections and large objects
//! check what the world holds, and past the limit the allocator raises a
//! flag whenever bytes are added to the world's account, for what grows
//! the world without collecting; the VM looks at it at its next check,
//! when a task switches away and when an execution ends, without
//! collecting (`memory_looked`). Found over its limit, the world collects
//! fully and is under pressure from then on, until it is back under. A
//! step (`step`) past what it held after its last collection for it, it
//! collects again; still that far, the running execution is refused at
//! its next check with a catchable "out of memory", and killed at its
//! third refusal, or at once past twice the limit. So growth is refused,
//! not whoever runs when the world is found over (the limit lowered,
//! say), and with no execution running nothing is: what the world's state
//! holds is the host's to report (`Vm::pressure`).

use std::{fmt, mem::size_of};

use rustc_hash::FxHashMap;

use super::{Error, ExecId, Stop, Vm};
use crate::value::Value;

/// What a world holds, in bytes (`Vm::held`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Held {
    /// The nursery, the old blocks and the large objects.
    pub heap: usize,
    /// Machine code.
    pub jit: usize,
    /// Everything else the world allocated: stacks, codes, tables, symbols
    /// it made, what natives and the host made for it.
    pub other: usize,
}

impl Held {
    pub fn total(&self) -> usize {
        self.heap + self.jit + self.other
    }
}

impl fmt::Display for Held {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mb = |b: usize| b >> 20;
        write!(f, "{} MB held: heap {}, jit {}, other {}", mb(self.total()), mb(self.heap), mb(self.jit), mb(self.other))
    }
}

/// A world over its memory limit (`Vm::check_memory`).
#[derive(Default)]
pub(crate) struct Pressure {
    /// What it held after its last full collection here: refusals are for
    /// growth past it.
    mark: usize,
    /// The refusals of each execution since the pressure began.
    refused: Vec<(ExecId, u32)>,
}

/// Refusals an execution gets; the next ends it.
const REFUSALS: u32 = 2;

/// Below this, `admit` admits without checking: small allocations stay
/// infallible, as allocating in the nursery does, and what they add up to
/// is the pressure check's.
pub(crate) const SMALL: usize = 64 << 10;

/// The most memory a world holds by default (`Vm::memory_limit`):
/// `TECHNE_MEMORY_MB` or 4 GiB.
pub(super) fn default_limit() -> usize {
    std::env::var("TECHNE_MEMORY_MB").ok().and_then(|v| v.parse::<usize>().ok()).map_or(4 << 30, |mb| mb << 20)
}

fn map<K, V>(m: &FxHashMap<K, V>) -> usize {
    m.capacity() * (size_of::<(K, V)>() + 1)
}

/// What `v` takes more when a push makes it grow.
fn growth<T>(v: &Vec<T>) -> usize {
    if v.len() == v.capacity() { v.capacity().max(4) * size_of::<T>() } else { 0 }
}

impl Vm {
    /// What the world holds.
    pub fn held(&self) -> Held {
        let heap = self.heap.committed();
        let charged = self.account.held();
        Held { heap, jit: self.jit.as_ref().map_or(0, |j| j.held()), other: charged.saturating_sub(heap) }
    }

    /// What a new task takes: its record, stack and dynamic state, its
    /// root, and the growth of the tables they go in.
    pub(crate) fn task_bytes(&self) -> usize {
        size_of::<crate::tasks::Task>()
            + crate::tasks::TASK_REGS * size_of::<Value>()
            + map(&self.locals)
            + 24
            + growth(&self.tasks)
            + growth(&self.live_tasks)
            + growth(&self.roots)
    }

    /// What a new channel takes, with the channel table's growth.
    pub(crate) fn channel_bytes(&self) -> usize {
        size_of::<crate::tasks::Channel>() + growth(&self.channels)
    }

    /// The most bytes the world is to hold.
    pub fn memory_limit(&self) -> usize {
        self.memory_limit
    }

    /// Set the most bytes the world is to hold. Past it, the allocator
    /// raises the attention flag, so that what grows the world without
    /// collecting is checked too; past three times it, the allocator ends
    /// the program (`crate::alloc::Account::set_limits`): for what no check
    /// of the VM reaches, a native allocating on.
    pub fn set_memory_limit(&mut self, bytes: usize) {
        self.memory_limit = bytes;
        self.account.set_limits(bytes, bytes.saturating_mul(3), self.memory_waker());
        // Collecting once, if the world is over it already.
        self.check_memory();
    }

    /// What the world holds, if it is over its limit.
    pub fn pressure(&self) -> Option<Held> {
        self.pressure.as_ref().map(|_| self.held())
    }

    /// Growth over the limit after which the world collects again.
    fn step(&self) -> usize {
        (self.memory_limit / 64).max(8 << 20)
    }

    /// Where memory grows (a collection, a large object): if the world is
    /// over its limit, found so now or a step past what it held after it
    /// last collected for it, collect fully; if it grew that step all the
    /// same, refuse the running execution (`Stop::OutOfMemory`), or past
    /// its refusals or the ceiling, kill it.
    pub(crate) fn check_memory(&mut self) {
        self.check_memory_collecting(true);
    }

    /// What the allocator calls past the limit: it raises the memory flag
    /// and the attention flag, for the VM to look at its next check.
    pub(crate) fn memory_waker(&self) -> std::sync::Arc<dyn Fn() + Send + Sync> {
        let (memory, attention) = (self.stops.memory.clone(), self.interrupt.clone());
        std::sync::Arc::new(move || {
            memory.store(true, std::sync::atomic::Ordering::SeqCst);
            attention.store(true, std::sync::atomic::Ordering::SeqCst);
        })
    }

    /// If the allocator raised the memory flag: `check_memory` where no
    /// collection can run (a check, a task switching away, an execution
    /// ending), for what grows the world without collecting; growth is
    /// judged as it is, and refused to what runs, which made it.
    pub(crate) fn memory_looked(&mut self) {
        if self.stops.memory.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.check_memory_collecting(false);
        }
    }

    /// What this thread counted, added to the world (which raises the memory
    /// flag past the limit), and looked at (`memory_looked`).
    pub(crate) fn look_at_growth(&mut self) {
        let _ = self.account.held();
        self.memory_looked();
    }

    /// Before the running execution `exec` ends or is switched away from:
    /// what this thread counted is added to the world (which raises the
    /// memory flag past the limit) and looked at, while what grew it is
    /// still the one running. The refusal or kill pending for it as the
    /// error it raises: an execution ending well is refused all the same.
    pub(crate) fn growth_stop(&mut self, exec: ExecId) -> Option<Error> {
        self.look_at_growth();
        match self.stops.pending(exec) {
            Some(Stop::Kill) => Some(Error::new(crate::stop::KILLED).with_kind(crate::vm::ErrorKind::Killed)),
            Some(Stop::OutOfMemory) => {
                self.stops.take_catchable(exec);
                Some(self.out_of_memory())
            }
            _ => None,
        }
    }

    fn check_memory_collecting(&mut self, collect: bool) {
        let limit = self.memory_limit;
        if self.held().total() <= limit {
            self.pressure = None;
            return;
        }
        let past = self.pressure.as_ref().map(|p| p.mark.saturating_add(self.step()));
        if past.is_some_and(|past| self.held().total() < past) {
            return;
        }
        if collect {
            self.full_collect();
        }
        let held = self.held().total();
        if held <= limit {
            self.pressure = None;
            return;
        }
        let exec = self.running_execution();
        let p = self.pressure.get_or_insert_default();
        p.mark = held;
        if past.is_none_or(|past| held < past) || exec == ExecId(0) {
            return;
        }
        let refused = match p.refused.iter_mut().find(|(e, _)| *e == exec) {
            Some((_, n)) => {
                *n += 1;
                *n
            }
            None => {
                p.refused.push((exec, 1));
                1
            }
        };
        let stop = if refused > REFUSALS || held / 2 > limit { Stop::Kill } else { Stop::OutOfMemory };
        self.interrupt_handle().stop(exec, stop);
    }

    /// The condition an execution refused for memory sees.
    pub(crate) fn out_of_memory(&self) -> Error {
        Error::new(format!("out of memory: past the limit of {} MB ({})", self.memory_limit >> 20, self.held()))
    }

    /// Room left under the memory limit.
    pub(crate) fn room(&self) -> usize {
        self.memory_limit.saturating_sub(self.held().total())
    }

    /// Admit an allocation of `bytes` (with its Rust-side temporaries) under
    /// the memory limit, before anything is allocated: if it does not fit
    /// even after a full collection, it is refused with a catchable error.
    pub fn admit(&mut self, bytes: usize) -> Result<(), Error> {
        // What the pressure check is for (`check_memory`).
        if bytes < SMALL || bytes <= self.room() {
            return Ok(());
        }
        self.full_collect();
        self.admit_without_collecting(bytes)
    }

    /// `admit` for `n` items of `size` bytes each, and `extra`.
    pub fn admit_items(&mut self, n: usize, size: usize, extra: usize) -> Result<(), Error> {
        self.admit(n.checked_mul(size).and_then(|b| b.checked_add(extra)).unwrap_or(usize::MAX))
    }

    /// `admit` where a collection cannot run: in the middle of a call.
    pub(crate) fn admit_without_collecting(&self, bytes: usize) -> Result<(), Error> {
        if bytes <= self.room() { Ok(()) } else { Err(self.refusal(bytes)) }
    }

    /// The error refusing `bytes`.
    pub(crate) fn refusal(&self, bytes: usize) -> Error {
        let size = match bytes {
            ..1024 => format!("{bytes} bytes"),
            1024..0x100000 => format!("{} KB", bytes >> 10),
            _ => format!("{} MB", bytes >> 20),
        };
        Error::new(format!("out of memory: {size} more would pass the limit of {} MB ({})", self.memory_limit >> 20, self.held()))
    }

    /// What the value `value` reads prints as (`builtins::print_within`),
    /// if it fits the room left with a copy of it (or is small), after a
    /// full collection if need be: what is shared many times prints as far
    /// more text than is held. `value` reads it anew after the collection,
    /// which moves it.
    pub(crate) fn printed(
        &mut self,
        value: impl Fn(&Vm) -> Value,
        write: bool,
        sharing: crate::builtins::Sharing,
    ) -> Result<String, Error> {
        let limit = |vm: &Vm| (vm.room() / 2).max(SMALL);
        let print = |vm: &Vm| {
            let mut text = String::new();
            crate::builtins::print_within(&mut text, value(vm), write, sharing, limit(vm)).then_some(text)
        };
        if let Some(text) = print(self) {
            return Ok(text);
        }
        self.full_collect();
        print(self).ok_or_else(|| self.refusal(2 * limit(self)))
    }

    /// The text of file `path`, admitted with `factor` bytes for each of its
    /// bytes (the text, and what the caller makes of it). It is read no
    /// further than there is room for: a file that grows as it is read, or
    /// a device that never ends, is refused.
    pub(crate) fn read_file_admitted(&mut self, path: &str, factor: usize) -> Result<String, Error> {
        use std::io::Read;
        let failed = |e: std::io::Error| Error::new(format!("{path}: {e}"));
        let len = std::fs::metadata(path).map_err(failed)?.len();
        self.admit_items(usize::try_from(len).unwrap_or(usize::MAX), factor, 0)?;
        let room = self.room().max(SMALL) / factor.max(1);
        let mut text = String::new();
        std::fs::File::open(path).and_then(|f| f.take(room as u64 + 1).read_to_string(&mut text)).map_err(failed)?;
        if text.len() > room {
            return Err(self.refusal(text.len().saturating_mul(factor)));
        }
        Ok(text)
    }
}
