//! What a world holds, against its memory limit (PLAN.md, Stage 1,
//! language step 8.3): what the allocator charged to its account
//! (`crate::alloc`), the heap included, and the JIT's machine code, which
//! is mapped apart. Growth of a known size is admitted before it is
//! allocated.

use std::{fmt, mem::size_of};

use rustc_hash::FxHashMap;

use super::{Error, Vm};
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

    /// Room left under the memory limit.
    pub(crate) fn room(&self) -> usize {
        self.memory_limit.saturating_sub(self.held().total())
    }

    /// Admit an allocation of `bytes` (with its Rust-side temporaries) under
    /// the memory limit, before anything is allocated: if it does not fit
    /// even after a full collection, it is refused with a catchable error.
    pub fn admit(&mut self, bytes: usize) -> Result<(), Error> {
        if bytes <= self.room() {
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
        if bytes <= self.room() {
            return Ok(());
        }
        let size = match bytes {
            ..1024 => format!("{bytes} bytes"),
            1024..0x100000 => format!("{} KB", bytes >> 10),
            _ => format!("{} MB", bytes >> 20),
        };
        Err(Error::new(format!("out of memory: {size} more would pass the limit of {} MB ({})", self.memory_limit >> 20, self.held())))
    }
}
