//! What a world holds, against its memory limit (PLAN.md, Stage 1,
//! language step 8.3): the heap, and what the VM holds besides. Counted as
//! it changes or read off capacities, never by walking what is held, so
//! that checking is cheap on every slow path that grows something. The
//! counts are estimates: allocator overhead and small parts are left out.

use std::{fmt, mem::size_of};

use rustc_hash::FxHashMap;

use super::{Error, Frame, Handler, Module, Stack, Vm};
use crate::{api::Root, value::Value};

/// What a world holds, in bytes (`Vm::held`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Held {
    /// The nursery, the old blocks and the large objects.
    pub heap: usize,
    /// Register, frame and handler stacks: the running one and the
    /// suspended tasks'.
    pub stacks: usize,
    /// Codes, the source text they were compiled from, macros and
    /// docstrings.
    pub code: usize,
    /// Machine code, and compilations queued.
    pub jit: usize,
    /// The symbols of the VM's thread, which every VM on it shares.
    pub symbols: usize,
    /// Tables of globals, modules, codes, files, tasks, channels, roots and
    /// foreign objects, with what modules, channels and waiting selects
    /// hold.
    pub tables: usize,
}

/// What `Vm::held` counts as it changes.
#[derive(Default)]
pub(crate) struct Counted {
    /// The stacks not running.
    pub stacks: usize,
    /// Live codes, source files, macros and docstrings.
    pub code: usize,
    /// Modules' imports, exports and definitions; channels' buffers and
    /// waiting senders.
    pub tables: usize,
}

impl Held {
    pub fn total(&self) -> usize {
        self.heap + self.stacks + self.code + self.jit + self.symbols + self.tables
    }
}

impl fmt::Display for Held {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let parts = [
            ("heap", self.heap),
            ("stacks", self.stacks),
            ("code", self.code),
            ("jit", self.jit),
            ("symbols", self.symbols),
            ("tables", self.tables),
        ];
        write!(f, "{} MB held", self.total() >> 20)?;
        parts.iter().try_for_each(|(name, bytes)| write!(f, ", {name} {}", bytes >> 20))
    }
}

/// Bytes counted while this lives (`Vm::charge`): for what goes many
/// ways, as a select's operations do.
pub(crate) struct Charge {
    counter: std::rc::Rc<std::cell::Cell<usize>>,
    bytes: usize,
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.counter.set(self.counter.get() - self.bytes);
    }
}

/// The most memory a world holds by default (`Vm::memory_limit`):
/// `TECHNE_MEMORY_MB` or 4 GiB.
pub(super) fn default_limit() -> usize {
    std::env::var("TECHNE_MEMORY_MB").ok().and_then(|v| v.parse::<usize>().ok()).map_or(4 << 30, |mb| mb << 20)
}

pub(crate) fn vec<T>(v: &Vec<T>) -> usize {
    v.capacity() * size_of::<T>()
}

pub(crate) fn map<K, V>(m: &FxHashMap<K, V>) -> usize {
    m.capacity() * (size_of::<(K, V)>() + 1)
}

/// A stack's registers, frames, handlers and dynamic state.
fn stack(regs: &Vec<Value>, frames: &Vec<Frame>, handlers: &Vec<Handler>, locals: &FxHashMap<i64, Root>) -> usize {
    vec(regs) + vec(frames) + vec(handlers) + map(locals)
}

/// What `v` takes more when a push makes it grow.
pub(crate) fn growth<T>(v: &Vec<T>) -> usize {
    if v.len() == v.capacity() { v.capacity().max(4) * size_of::<T>() } else { 0 }
}

impl Stack {
    pub(crate) fn bytes(&self) -> usize {
        stack(&self.regs, &self.frames, &self.handlers, &self.locals)
    }
}

impl Module {
    pub(super) fn bytes(&self) -> usize {
        self.name.len() + map(&self.imports) + self.exports.as_ref().map_or(0, vec) + vec(&self.defined)
    }
}

impl Vm {
    /// What the world holds.
    pub fn held(&self) -> Held {
        // A root's cell, and its entry in `roots` (until a collection finds
        // it dropped).
        let roots = self.roots.len() * 24 + vec(&self.roots);
        let globals = vec(&self.globals)
            + vec(&self.global_rooted)
            + vec(&self.global_names)
            + vec(&self.global_module)
            + vec(&self.user_defined)
            + vec(&self.free_globals)
            + map(&self.bindings);
        let tables = vec(&self.modules)
            + vec(&self.codes)
            + vec(&self.files)
            + vec(&self.tasks)
            + vec(&self.channels)
            + vec(&self.foreign)
            + map(&self.variable_docs)
            + self.offers.bytes();
        Held {
            heap: self.heap.committed(),
            stacks: stack(&self.regs, &self.frames, &self.handlers, &self.locals) + self.counted.stacks,
            code: self.counted.code,
            jit: self.jit.as_ref().map_or(0, |j| j.held()),
            symbols: crate::reader::held(),
            tables: globals + tables + roots + self.counted.tables + self.charged.get(),
        }
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

    /// Count `bytes` while the `Charge` returned lives.
    pub(crate) fn charge(&self, bytes: usize) -> Charge {
        self.charged.set(self.charged.get() + bytes);
        Charge { counter: self.charged.clone(), bytes }
    }

    /// What a new channel takes, with the channel table's growth.
    pub(crate) fn channel_bytes(&self) -> usize {
        size_of::<crate::tasks::Channel>() + growth(&self.channels)
    }

    /// Change module `m`, counting what it holds.
    pub(crate) fn changing_module<T>(&mut self, m: u32, f: impl FnOnce(&mut Module) -> T) -> T {
        let module = &mut self.modules[m as usize];
        let result = f(module);
        let bytes = module.bytes();
        self.counted.tables = self.counted.tables - module.held + bytes;
        module.held = bytes;
        result
    }

    /// Set or clear the docstring of global `g`.
    pub(crate) fn set_variable_doc(&mut self, g: u32, doc: Option<std::rc::Rc<str>>) {
        self.counted.code += doc.as_ref().map_or(0, |d| d.len());
        let old = match doc {
            Some(doc) => self.variable_docs.insert(g, doc),
            None => self.variable_docs.remove(&g),
        };
        self.counted.code -= old.map_or(0, |d| d.len());
    }

    /// Count macro `m`, defined at top level, while it lives.
    pub(crate) fn count_macro(&mut self, m: &std::rc::Rc<crate::expand::Macro>) {
        let bytes = m.bytes();
        self.counted.code += bytes;
        self.macros.push((std::rc::Rc::downgrade(m), bytes));
    }

    /// Stop counting the macros and the source text kept for them that
    /// have gone.
    pub(super) fn release_kept(&mut self) {
        let Counted { code, .. } = &mut self.counted;
        self.macros.retain(|(m, bytes)| {
            let live = m.strong_count() > 0;
            if !live {
                *code -= bytes;
            }
            live
        });
        self.kept_files.retain(|f| {
            let live = std::rc::Rc::strong_count(&f.text) > 1;
            if !live {
                *code -= f.name.len() + f.text.len();
            }
            live
        });
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
