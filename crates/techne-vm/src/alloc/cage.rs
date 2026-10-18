//! The cage: one reserved region per thread, holding the heaps of the
//! worlds on that thread.
//!
//! Every heap object lives in its thread's cage, which is aligned to its
//! size: an object's address is the cage's base with the offset in the low
//! bits. `Value::as_ptr` takes only those bits from a value (`at`), word
//! aligned, and compiled code does as much (`jit::Gen::ptr`), so any
//! value, stale (its object freed), forged, or from a world on another
//! thread, names memory of this thread's cage: never host memory, nor
//! memory another thread uses. Its pointers derive from the cage's base,
//! so they carry the cage's provenance.
//!
//! What is reached from an object stays there too, or faults: an object's
//! fields and bytes are found from its header's length, which
//! `heap::header_len` bounds by the cage's size (no object is larger), and
//! after the cage comes a guard, mapped inaccessible, as long as that
//! bound reaches in words, and then some for constant offsets. So a header
//! that is no object's (a stale pointer's, or bytes of a string a forged
//! value points into) cannot lead an access out of the cage. Code that
//! finds an offset otherwise than from a length (`heap::header_len`) or
//! a small constant must bound it itself.
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

/// The bytes of a cage its heaps may use: `SIZE`, or less as
/// `TECHNE_CAGE_MB` says (read once, for every thread), but no less than
/// 64 MiB, room for worlds and the reserve.
pub(crate) fn capacity() -> usize {
    static CAPACITY: OnceLock<usize> = OnceLock::new();
    *CAPACITY.get_or_init(|| {
        let mb = std::env::var("TECHNE_CAGE_MB").ok().and_then(|v| v.parse::<usize>().ok());
        mb.map_or(SIZE, |mb| mb.saturating_mul(1 << 20).clamp((64 << 20).min(SIZE), SIZE))
    })
}

/// What a cage keeps free for collecting: its worlds are refused room
/// before its free bytes fall below it (`Vm::room`), as a collection
/// cannot stop for want of room halfway.
pub(crate) fn reserve() -> usize {
    (capacity() / 16).max((64 << 20).min(capacity() / 2))
}

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
    /// Their bytes.
    free_bytes: usize,
}

/// The thread's cage, reserved (or taken from the pool) on first use.
pub(crate) fn current() -> Rc<Cage> {
    CURRENT.with(|c| {
        c.borrow_mut()
            .get_or_insert_with(|| {
                let base = POOL.lock().unwrap_or_else(|e| e.into_inner()).pop().map_or_else(|| os::reserve(true), |r| r.0);
                BASE.set(base.as_ptr());
                Rc::new(Cage { base, spans: RefCell::new(Spans { top: START, free: BTreeMap::new(), free_bytes: 0 }) })
            })
            .clone()
    })
}

/// Bytes of this thread's cage left for its worlds' heaps.
pub(crate) fn free() -> usize {
    capacity() - START - used()
}

/// Bytes of this thread's cage in use: what its worlds' heaps hold of it,
/// whether touched or not (the cage is `SIZE`).
pub fn used() -> usize {
    CURRENT.with(|c| {
        c.borrow().as_ref().map_or(0, |cage| {
            let s = cage.spans.borrow();
            debug_assert_eq!(s.free_bytes, s.free.values().sum::<usize>());
            s.top - START - s.free_bytes
        })
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
                s.free_bytes -= len;
                o
            }
            None => {
                let o = s.top;
                if capacity() - o < len {
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
        // Counted free again as it merges; what reaches `top` is not.
        s.free_bytes += len;
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
            s.free_bytes -= len;
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

    /// What follows a cage, mapped inaccessible: as far as what an object's
    /// length reaches from anywhere in the cage (`heap::header_len` bounds
    /// a length by the cage's size, in words at most: eight times it), and
    /// constant offsets past that. Address space only.
    pub(super) const GUARD: usize = 8 * SIZE + (64 << 10);

    /// A fresh cage, aligned to its size, with its guard after it: the
    /// whole is mapped inaccessible with a cage's worth more for aligning,
    /// the excess unmapped again, then the cage opened.
    pub(super) fn reserve(writable: bool) -> NonNull<u8> {
        let len = 2 * SIZE + GUARD;
        let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE;
        // SAFETY: a fresh private anonymous mapping.
        let p = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_NONE, flags, -1, 0) };
        if p == libc::MAP_FAILED {
            panic!("cannot reserve a heap cage: {}", std::io::Error::last_os_error());
        }
        let head = p.addr().next_multiple_of(SIZE) - p.addr();
        let prot = if writable { libc::PROT_READ | libc::PROT_WRITE } else { libc::PROT_READ };
        // SAFETY: all three ranges are in the mapping just made, and unused.
        unsafe {
            if head > 0 {
                libc::munmap(p, head);
            }
            let tail = len - head - SIZE - GUARD;
            if tail > 0 {
                libc::munmap(p.byte_add(head + SIZE + GUARD), tail);
            }
            let base = p.byte_add(head);
            if libc::mprotect(base, SIZE, prot) != 0 {
                panic!("cannot open a heap cage: {}", std::io::Error::last_os_error());
            }
            NonNull::new_unchecked(base.cast())
        }
    }

    /// # Safety
    /// `p..p + len` is whole pages of a cage, unused.
    pub(super) unsafe fn release(p: NonNull<u8>, len: usize) {
        // SAFETY: forwarded from the caller; the pages read as zero after,
        // else they are zeroed here: spans are handed out zero.
        unsafe {
            if libc::madvise(p.as_ptr().cast(), len, libc::MADV_DONTNEED) != 0 {
                p.as_ptr().write_bytes(0, len);
            }
        }
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
    /// only never written by the VM. Nor has it a guard: the confinement
    /// past a cage's end is Linux's (Miri checks bounds itself).
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

    #[cfg(all(target_os = "linux", not(miri)))]
    #[test]
    fn a_guard_follows_the_cage() {
        let v = words(8);
        let end = (v.as_ptr().addr() & !MASK) + SIZE;
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        let guarded = maps.lines().any(|l| {
            let (range, perms) = l.split_once(' ').unwrap();
            let (lo, hi) = range.split_once('-').unwrap();
            let (lo, hi) = (usize::from_str_radix(lo, 16).unwrap(), usize::from_str_radix(hi, 16).unwrap());
            lo <= end && end + os::GUARD <= hi && perms.starts_with("---")
        });
        assert!(guarded, "no inaccessible guard of {} bytes from {end:#x}", os::GUARD);
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
