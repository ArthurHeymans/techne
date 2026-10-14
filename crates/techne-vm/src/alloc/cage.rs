//! The cage: one reserved region per thread, holding the heaps of the
//! worlds on that thread.
//!
//! Every heap object lives in its thread's cage, which is aligned to its
//! size: an object's address is the cage's base with the offset in the low
//! bits. `Value::as_ptr` takes only those bits from a value (`at`), word
//! aligned, so any
//! value, stale (its object freed), forged, or from a world on another
//! thread, names memory of this thread's cage: never host memory, nor
//! memory another thread uses. Its pointers derive from the cage's base,
//! so they carry the cage's provenance.
//!
//! The cage is mapped readable and writable without reserving swap
//! (`MAP_NORESERVE`): untouched pages cost nothing and read as zero.
//! Spans are handed out first-fit from free spans kept coalesced, else from
//! the untouched rest. Freed spans go back to the kernel (`MADV_DONTNEED`),
//! so every span handed out is zero.
//!
//! A thread keeps its cage (`CURRENT`) until it exits, and every allocation
//! in the cage holds it too, so it outlives the worlds that use it. Values
//! are not `Send`, and only a heap makes pointer values: a thread holding
//! one has a cage, so `at` need not check for it. When the last holder
//! lets go, as the thread exits, the region is pooled for another thread,
//! never unmapped, and the thread's base moves to a read-only cage shared
//! by exited threads (`retired`): a value a later destructor of the thread
//! still holds reads zeros there, and writing through it faults.

use std::{
    alloc::{AllocError, Allocator, Layout},
    cell::{Cell, RefCell},
    collections::BTreeMap,
    ptr::NonNull,
    rc::Rc,
    sync::{Mutex, OnceLock},
};

use super::Budget;

/// Offset bits: 4 GiB per thread, the heaps of all its worlds together
/// (V8's pointer compression cage is as large). Smaller where the cage is
/// allocated rather than mapped (under Miri, off Linux), since it is then
/// real memory.
#[cfg(all(target_os = "linux", not(miri)))]
const BITS: u32 = 32;
#[cfg(not(all(target_os = "linux", not(miri))))]
const BITS: u32 = 26;
pub(crate) const SIZE: usize = 1 << BITS;
pub(crate) const MASK: usize = SIZE - 1;
/// The offset bits of a word's address: aligned whatever the value.
const WORD: usize = MASK & !7;
const PAGE: usize = 4096;
/// Spans start here: no object is at the base.
const START: usize = 64 << 10;

/// A cage's memory, unused while pooled.
struct Region(NonNull<u8>);

// SAFETY: a pooled region is plain memory nothing refers to; the retired
// one is only read.
unsafe impl Send for Region {}
// SAFETY: as for Send.
unsafe impl Sync for Region {}

static POOL: Mutex<Vec<Region>> = Mutex::new(Vec::new());

/// The cage of exited threads: read-only and zero.
fn retired() -> NonNull<u8> {
    static RETIRED: OnceLock<Region> = OnceLock::new();
    RETIRED.get_or_init(|| Region(os::reserve(false))).0
}

thread_local! {
    /// The thread's cage's base; null while it has none.
    static BASE: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
    static CURRENT: RefCell<Option<Rc<Cage>>> = const { RefCell::new(None) };
}

pub(crate) struct Cage {
    base: NonNull<u8>,
    spans: RefCell<Spans>,
}

struct Spans {
    /// Everything from here on is untouched.
    top: usize,
    /// Free spans below `top` by offset, none adjacent to another or to `top`.
    free: BTreeMap<usize, usize>,
}

/// The thread's cage, reserved (or taken from the pool) on first use.
pub(crate) fn current() -> Rc<Cage> {
    CURRENT.with(|c| {
        c.borrow_mut()
            .get_or_insert_with(|| {
                let base = POOL.lock().unwrap_or_else(|e| e.into_inner()).pop().map_or_else(|| os::reserve(true), |r| r.0);
                BASE.set(base.as_ptr());
                Rc::new(Cage { base, spans: RefCell::new(Spans { top: START, free: BTreeMap::new() }) })
            })
            .clone()
    })
}

/// The word at `addr`'s offset in this thread's cage.
#[inline(always)]
pub(crate) fn at(addr: usize) -> *mut u64 {
    let base = BASE.get();
    debug_assert!(!base.is_null(), "a heap value on a thread without a cage");
    // The base is aligned to the cage's size: adding the offset sets its
    // low bits.
    base.wrapping_add(addr & WORD).cast()
}

/// The word at `addr`'s offset in the cage `near` is in: as `at`, from a
/// pointer into the cage at hand rather than the thread's.
#[inline(always)]
pub(crate) fn near(near: *mut u64, addr: usize) -> *mut u64 {
    let base = near.cast::<u8>().wrapping_sub(near.addr() & MASK);
    base.wrapping_add(addr & WORD).cast()
}

/// Whether `p` is in this thread's cage.
pub(crate) fn contains(p: *const u64) -> bool {
    let base = BASE.get();
    !base.is_null() && p.addr() & !MASK == base.addr()
}

impl Cage {
    fn alloc(&self, len: usize) -> Option<NonNull<u8>> {
        let len = len.next_multiple_of(PAGE);
        let mut s = self.spans.borrow_mut();
        let fit = s.free.iter().find(|&(_, &l)| l >= len).map(|(&o, &l)| (o, l));
        let off = match fit {
            Some((o, l)) => {
                s.free.remove(&o);
                if l > len {
                    s.free.insert(o + len, l - len);
                }
                o
            }
            None => {
                let o = s.top;
                if SIZE - o < len {
                    return None;
                }
                s.top = o + len;
                o
            }
        };
        // SAFETY: `off + len` is within the cage.
        Some(unsafe { self.base.add(off) })
    }

    /// # Safety
    /// `p` was returned by `alloc(len)` and is not used again.
    unsafe fn free(&self, p: NonNull<u8>, len: usize) {
        let mut len = len.next_multiple_of(PAGE);
        // SAFETY: forwarded from the caller.
        unsafe { os::release(p, len) };
        let mut off = p.as_ptr().addr() - self.base.as_ptr().addr();
        let mut s = self.spans.borrow_mut();
        if let Some(l) = s.free.remove(&(off + len)) {
            len += l;
        }
        if let Some((&o, &l)) = s.free.range(..off).next_back()
            && o + l == off
        {
            s.free.remove(&o);
            (off, len) = (o, len + l);
        }
        if off + len == s.top {
            s.top = off;
        } else {
            s.free.insert(off, len);
        }
    }
}

impl Drop for Cage {
    fn drop(&mut self) {
        // Every allocation held the cage: all of it is free, and zero.
        debug_assert_eq!(self.spans.get_mut().top, START);
        if BASE.get() == self.base.as_ptr() {
            BASE.set(retired().as_ptr());
        }
        POOL.lock().unwrap_or_else(|e| e.into_inner()).push(Region(self.base));
    }
}

/// Storage in the thread's cage, charged to a budget. Not `Send`: it is
/// freed on the thread whose cage it is in.
#[derive(Clone)]
pub(crate) struct Caged {
    budget: Budget,
    cage: Rc<Cage>,
}

impl Caged {
    pub(crate) fn new(budget: Budget) -> Caged {
        Caged { budget, cage: current() }
    }

    pub(crate) fn budget(&self) -> &Budget {
        &self.budget
    }
}

// SAFETY: blocks are distinct spans of the cage, which outlives them (each
// allocator clone holds it), and are zero when handed out. Resizing uses
// the default, a new block and a copy.
unsafe impl Allocator for Caged {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        if layout.align() > PAGE {
            return Err(AllocError);
        }
        if layout.size() == 0 {
            return Ok(NonNull::slice_from_raw_parts(layout.dangling_ptr(), 0));
        }
        let reserved = self.budget.reserve(layout.size())?;
        let p = self.cage.alloc(layout.size()).ok_or(AllocError)?;
        reserved.commit();
        Ok(NonNull::slice_from_raw_parts(p, layout.size()))
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // Already zero: writing zeros would only touch the pages.
        self.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() != 0 {
            // SAFETY: a live block of this size (Allocator's contract).
            unsafe { self.cage.free(ptr, layout.size()) };
            self.budget.release(layout.size());
        }
    }
}

#[cfg(all(target_os = "linux", not(miri)))]
mod os {
    use std::ptr::NonNull;

    use super::SIZE;

    /// A fresh cage, aligned to its size: twice as much is mapped, the
    /// excess unmapped again.
    pub(super) fn reserve(writable: bool) -> NonNull<u8> {
        let len = 2 * SIZE;
        let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE;
        let prot = if writable { libc::PROT_READ | libc::PROT_WRITE } else { libc::PROT_READ };
        // SAFETY: a fresh private anonymous mapping.
        let p = unsafe { libc::mmap(std::ptr::null_mut(), len, prot, flags, -1, 0) };
        if p == libc::MAP_FAILED {
            panic!("cannot reserve a heap cage: {}", std::io::Error::last_os_error());
        }
        let head = p.addr().next_multiple_of(SIZE) - p.addr();
        // SAFETY: both ends are in the mapping just made, and unused.
        unsafe {
            if head > 0 {
                libc::munmap(p, head);
            }
            if len - head - SIZE > 0 {
                libc::munmap(p.byte_add(head + SIZE), len - head - SIZE);
            }
            NonNull::new_unchecked(p.byte_add(head).cast())
        }
    }

    /// # Safety
    /// `p..p + len` is whole pages of a cage, unused.
    pub(super) unsafe fn release(p: NonNull<u8>, len: usize) {
        // SAFETY: forwarded from the caller; the pages read as zero after.
        unsafe { libc::madvise(p.as_ptr().cast(), len, libc::MADV_DONTNEED) };
    }
}

#[cfg(not(all(target_os = "linux", not(miri))))]
mod os {
    use std::{
        alloc::{Layout, alloc_zeroed, handle_alloc_error},
        ptr::NonNull,
    };

    use super::SIZE;

    /// Allocated memory cannot be made read-only: here a retired cage is
    /// only never written by the VM.
    pub(super) fn reserve(_writable: bool) -> NonNull<u8> {
        let layout = Layout::from_size_align(SIZE, SIZE).expect("cage layout");
        // SAFETY: a non-zero size.
        NonNull::new(unsafe { alloc_zeroed(layout) }).unwrap_or_else(|| handle_alloc_error(layout))
    }

    /// # Safety
    /// `p..p + len` is part of a cage, unused.
    pub(super) unsafe fn release(p: NonNull<u8>, len: usize) {
        // SAFETY: forwarded from the caller.
        unsafe { p.as_ptr().write_bytes(0, len) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(n: usize) -> Vec<u64, Caged> {
        let mut v = Vec::with_capacity_in(n, Caged::new(Budget::new(usize::MAX)));
        v.resize(n, 0);
        v
    }

    #[test]
    fn freed_spans_are_reused_zeroed_and_merged() {
        let mut a = words(1 << 14);
        let b = words(1 << 14);
        a.fill(7);
        let (pa, pb) = (a.as_ptr(), b.as_ptr());
        assert!(contains(pa) && contains(pb));
        drop(a);
        let c = words(1 << 14);
        assert_eq!(c.as_ptr(), pa, "first fit reuses the freed span");
        assert!(c.iter().all(|&w| w == 0));
        drop((b, c));
        let d = words(1 << 15);
        assert_eq!(d.as_ptr(), pa, "freed neighbours merge back into the untouched rest");
    }

    #[test]
    fn any_address_lands_in_the_cage() {
        let v = words(8);
        let p = v.as_ptr();
        assert_eq!(at(p.addr()).cast_const(), p);
        for forged in [0, usize::MAX, p.addr() ^ (1 << 40), 0xdead_beef] {
            assert!(contains(at(forged)) && at(forged).is_aligned());
        }
    }

    #[test]
    fn budgets_are_charged() {
        let budget = Budget::new(1 << 20);
        let mut v: Vec<u64, Caged> = Vec::new_in(Caged::new(budget.clone()));
        v.try_reserve_exact(1000).unwrap();
        assert_eq!(budget.held(), 8000);
        assert!(v.try_reserve_exact(1 << 20).is_err());
        drop(v);
        assert_eq!(budget.held(), 0);
    }
}
