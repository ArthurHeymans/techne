//! The cage: one reserved region per world, holding its heap.
//!
//! Every heap object lives in its world's cage, which is aligned to its
//! size: an object's address is the cage's base with the offset in the low
//! bits. `Value::as_ptr` takes only those bits from a value and adds the
//! base of the cage of the world entered on the thread (`at`, `enter`),
//! word aligned, and compiled code adds its world's (`jit::Gen::ptr`), so
//! any value, stale (its object freed), forged, or another world's, names
//! memory of the cage of the world running: never host memory, nor
//! another world's. Its pointers derive from the cage's base, so they
//! carry the cage's provenance.
//!
//! What is reached from an object stays there too, and never faults: a
//! length read from a header (a stale pointer's or a forged one's) is
//! checked to end in the cage (`heap::len_of`, and as the collector reads
//! one), bytes are taken only from ranges checked so (`heap::bytes_at`),
//! a length that does not fit retiring the world; compiled code takes such
//! a vector as having none, and wraps other indices within the cage
//! (`jit::Gen::wrap`); trusted displacements (a pair's fields, a closure's
//! captures by their 16-bit index, an entry next to a checked one) land at
//! worst in the cage's tail (`TAIL`), the world's own memory that no object
//! has. So a header that is no object's cannot lead an access out of the
//! cage, nor fault: a cage takes its size of address space and the
//! tail's, no guard.
//!
//! The cage is mapped readable and writable without reserving swap
//! (`MAP_NORESERVE`): untouched pages cost nothing and read as zero.
//! Spans are handed out first-fit from free spans kept coalesced, else from
//! the untouched rest. Freed spans go back to the kernel (`MADV_DONTNEED`),
//! so every span handed out is zero.
//!
//! A world's heap holds its cage, and so does every allocation in it and
//! every entry of it (`Entered`). When the last lets go the region is
//! pooled for another world, never unmapped. Values are decoded only with
//! their world entered: the VM enters its world wherever it is entered
//! (`Vm::charging`, executions, collections); with none entered the base
//! is null, and a value decoded then names no cage's memory.

use std::{
    alloc::{AllocError, Allocator, Layout},
    cell::{Cell, RefCell},
    collections::BTreeMap,
    ptr::NonNull,
    rc::Rc,
    sync::{Mutex, OnceLock},
};

use super::Budget;

/// Offset bits: 4 GiB of address space for each world, whose heap is in a
/// cage of its own (`Cage::new`; V8's pointer compression cage is as
/// large). Smaller where the cage is
/// allocated rather than mapped (under Miri, off Linux), since it is then
/// real memory.
#[cfg(all(target_os = "linux", not(miri)))]
const BITS: u32 = 32;
#[cfg(not(all(target_os = "linux", not(miri))))]
const BITS: u32 = 26;
pub(crate) const SIZE: usize = 1 << BITS;
/// What follows a cage in its allocation, the world's own but never its
/// objects': accesses at trusted displacements from an object (a field of
/// a pair, a closure's captures by their 16-bit index) land there from an
/// object at the cage's end. Displacements a heap value may choose (an
/// index below a length read from a header) wrap within the cage
/// instead (`heap::word`).
pub(crate) const TAIL: usize = 1 << 20;
pub(crate) const MASK: usize = SIZE - 1;
/// The offset bits of a word's address: aligned whatever the value.
const WORD: usize = MASK & !7;
const PAGE: usize = 4096;
/// Spans start here: no object is at the base.
const START: usize = 64 << 10;

/// The bytes of a cage its heap may use: `SIZE`, or less as
/// `TECHNE_CAGE_MB` says (read once for the process), but no less than
/// 64 MiB, room for the world and the reserve.
pub(crate) fn capacity() -> usize {
    static CAPACITY: OnceLock<usize> = OnceLock::new();
    *CAPACITY.get_or_init(|| {
        let mb = std::env::var("TECHNE_CAGE_MB").ok().and_then(|v| v.parse::<usize>().ok());
        mb.map_or(SIZE, |mb| mb.saturating_mul(1 << 20).clamp((64 << 20).min(SIZE), SIZE))
    })
}

/// What a cage keeps free for collecting: its world is refused room
/// before its free bytes fall below it (`Vm::room`), as a collection
/// cannot stop for want of room halfway.
pub(crate) fn reserve() -> usize {
    (capacity() / 16).max((64 << 20).min(capacity() / 2))
}

/// A cage's memory, unused while pooled.
struct Region(NonNull<u8>);

// SAFETY: a pooled region is plain memory nothing refers to.
unsafe impl Send for Region {}
// SAFETY: as for Send.
unsafe impl Sync for Region {}

static POOL: Mutex<Vec<Region>> = Mutex::new(Vec::new());

thread_local! {
    /// The base of the cage of the world entered on this thread (`enter`),
    /// against which values are decoded (`at`); null while none is.
    static BASE: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
    /// The cages entered on this thread, newest last, each by its entry's
    /// number: `BASE` is the newest's. Each is held here, so none entered
    /// goes back to the pool.
    static STACK: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    static ENTRIES: Cell<u64> = const { Cell::new(0) };
    /// The newest entry's number (0 while there is none) and its guards,
    /// kept here rather than in `STACK`: entering the world entered
    /// already, as nearly every entry does, counts a guard more without
    /// the stack.
    static TOP: Cell<u64> = const { Cell::new(0) };
    static TOP_GUARDS: Cell<u32> = const { Cell::new(0) };
}

/// One world's heap memory (`Heap`): a region aligned to its size, with
/// its tail after it.
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

impl Cage {
    /// A cage of its own: a pooled region, else one reserved now.
    pub(crate) fn new() -> Rc<Cage> {
        let base = POOL.lock().unwrap_or_else(|e| e.into_inner()).pop().map_or_else(os::reserve, |r| r.0);
        Rc::new(Cage { base, spans: RefCell::new(Spans { top: START, free: BTreeMap::new(), free_bytes: 0 }) })
    }

    /// Its base, which every address in it shares.
    pub(crate) fn base(&self) -> *mut u8 {
        self.base.as_ptr()
    }

    /// Bytes left for its heap.
    pub(crate) fn room(&self) -> usize {
        capacity() - START - self.used()
    }

    /// The offset past every span handed out yet.
    #[cfg(feature = "hostile")]
    pub(crate) fn top(&self) -> usize {
        self.spans.borrow().top
    }

    /// Bytes its heap holds of it, whether touched or not.
    pub(crate) fn used(&self) -> usize {
        let s = self.spans.borrow();
        debug_assert_eq!(s.free_bytes, s.free.values().sum::<usize>());
        s.top - START - s.free_bytes
    }
}

/// A world entered on this thread, by as many guards as are left of it:
/// its cage, and its symbols, which the reader interns in.
struct Entry {
    entry: u64,
    cage: Rc<Cage>,
    symbols: Rc<RefCell<crate::reader::Symbols>>,
    /// Its guards, but the newest's, which are `TOP_GUARDS`.
    guards: u32,
}

/// While it lives, values decode in its cage (`at`, `Value::as_ptr`): its
/// world is entered on this thread. Guards may be dropped in any order: the
/// cage values decode in is always that of the newest entry left, which
/// holds it, so no cage entered goes back to the pool.
pub(crate) struct Entered {
    /// Its entry's number in `STACK`, which it counts in; `LATE` past the
    /// stack's destruction.
    entry: u64,
    /// The base before, restored when `entry` is `LATE`.
    previous: *mut u8,
}

/// An entry made as the thread exits, its stack gone (a world dropped by a
/// thread-local's destructor): it restores the base it found, and the
/// world entered holds its cage meanwhile.
const LATE: u64 = u64::MAX;

/// The symbols of the world entered on this thread, if one is.
pub(crate) fn entered_symbols() -> Option<Rc<RefCell<crate::reader::Symbols>>> {
    STACK.try_with(|s| s.borrow().last().map(|e| e.symbols.clone())).ok().flatten()
}

/// Enter the world of `cage` and `symbols` on this thread until the guard
/// returned is dropped.
#[inline]
pub(crate) fn enter(cage: &Rc<Cage>, symbols: &Rc<RefCell<crate::reader::Symbols>>) -> Entered {
    let previous = BASE.get();
    let top = TOP.get();
    // Entered already (it is the newest, as `BASE` is its): one guard
    // more counts in the same entry, which lasts while any does.
    if top != 0 && previous == cage.base() {
        TOP_GUARDS.set(TOP_GUARDS.get() + 1);
        return Entered { entry: top, previous };
    }
    enter_anew(cage, symbols, previous)
}

#[cold]
fn enter_anew(cage: &Rc<Cage>, symbols: &Rc<RefCell<crate::reader::Symbols>>, previous: *mut u8) -> Entered {
    let entry = STACK.try_with(|s| {
        let mut s = s.borrow_mut();
        if let Some(top) = s.last_mut() {
            top.guards = TOP_GUARDS.get();
        }
        let entry = ENTRIES.get() + 1;
        ENTRIES.set(entry);
        s.push(Entry { entry, cage: cage.clone(), symbols: symbols.clone(), guards: 0 });
        TOP.set(entry);
        TOP_GUARDS.set(1);
        entry
    });
    BASE.set(cage.base());
    Entered { entry: entry.unwrap_or(LATE), previous }
}

impl Drop for Entered {
    #[inline]
    fn drop(&mut self) {
        if self.entry == TOP.get() && TOP_GUARDS.get() > 1 {
            TOP_GUARDS.set(TOP_GUARDS.get() - 1);
            return;
        }
        self.leave();
    }
}

impl Entered {
    /// Drop the last guard of the newest entry, or a guard of an older
    /// one.
    #[cold]
    fn leave(&self) {
        if self.entry == LATE {
            BASE.set(self.previous);
            return;
        }
        // Gone with the thread, the stack holds nothing more: the base
        // found is restored.
        let left = STACK.try_with(|s| {
            let mut s = s.borrow_mut();
            if self.entry == TOP.get() {
                s.pop();
                let newest = s.last();
                TOP.set(newest.map_or(0, |e| e.entry));
                TOP_GUARDS.set(newest.map_or(0, |e| e.guards));
            } else if let Some(i) = s.iter().rposition(|e| e.entry == self.entry) {
                s[i].guards -= 1;
                if s[i].guards == 0 {
                    s.remove(i);
                }
            }
            BASE.set(s.last().map_or(std::ptr::null_mut(), |e| e.cage.base()));
        });
        if left.is_err() {
            BASE.set(self.previous);
        }
    }
}

/// The word at `addr`'s offset in the cage of the world entered. A value
/// of the world's has its cage's address in its other bits, which debug
/// builds check: a value decoded in another world's cage is a missing
/// entry (`enter`), though confined either way.
#[inline(always)]
pub(crate) fn at(addr: usize) -> *mut u64 {
    let base = BASE.get();
    debug_assert!(!base.is_null(), "a heap value decoded with no world entered");
    debug_assert_eq!(addr & ADDRESS & !MASK, base.addr(), "a value of another cage decoded in the cage of the world entered");
    confine(base, addr)
}

/// The bits of a value that are an address.
const ADDRESS: usize = (1 << 48) - 1;

/// The word at `addr`'s offset in the cage at `base`, whatever `addr`'s
/// other bits: `at` without its checks, for what is forged on purpose.
#[inline(always)]
pub(crate) fn confine(base: *mut u8, addr: usize) -> *mut u64 {
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

/// Whether `p` is in the cage of the world entered.
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
        // Every allocation held the cage: all of it is free. Its pages are
        // zero but for what a world whose heap was corrupt wrote outside
        // its objects, anywhere in the cage: the next world must not find
        // that, so the whole of it is scrubbed before it is pooled.
        debug_assert_eq!(self.spans.get_mut().top, START);
        if let Some(region) = os::scrub(self.base) {
            POOL.lock().unwrap_or_else(|e| e.into_inner()).push(Region(region));
        }
    }
}

/// Storage in a cage, charged to a budget. Not `Send`: a cage belongs to
/// one world, on one thread.
#[derive(Clone)]
pub(crate) struct Caged {
    budget: Budget,
    cage: Rc<Cage>,
}

impl Caged {
    pub(crate) fn new(budget: Budget, cage: Rc<Cage>) -> Caged {
        Caged { budget, cage }
    }

    pub(crate) fn cage(&self) -> &Rc<Cage> {
        &self.cage
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

    use super::{SIZE, TAIL};

    /// A fresh cage and its tail, aligned to the cage's size: a cage's
    /// worth more is mapped for aligning, and the excess unmapped again.
    /// Address space only, until touched.
    pub(super) fn reserve() -> NonNull<u8> {
        let len = 2 * SIZE + TAIL;
        let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE;
        let prot = libc::PROT_READ | libc::PROT_WRITE;
        // SAFETY: a fresh private anonymous mapping.
        let p = unsafe { libc::mmap(std::ptr::null_mut(), len, prot, flags, -1, 0) };
        if p == libc::MAP_FAILED {
            panic!("cannot reserve a heap cage: {}", std::io::Error::last_os_error());
        }
        let head = p.addr().next_multiple_of(SIZE) - p.addr();
        // SAFETY: both ranges are in the mapping just made, and unused.
        unsafe {
            if head > 0 {
                libc::munmap(p, head);
            }
            let tail = len - head - SIZE - TAIL;
            if tail > 0 {
                libc::munmap(p.byte_add(head + SIZE + TAIL), tail);
            }
            NonNull::new_unchecked(p.byte_add(head).cast())
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

    /// A cage no world uses, all of it and its tail zero again, to pool.
    pub(super) fn scrub(base: NonNull<u8>) -> Option<NonNull<u8>> {
        // SAFETY: the whole cage, which nothing uses any more.
        unsafe { release(base, SIZE + TAIL) };
        Some(base)
    }
}

#[cfg(not(all(target_os = "linux", not(miri))))]
mod os {
    use std::{
        alloc::{Layout, alloc_zeroed, handle_alloc_error},
        ptr::NonNull,
    };

    use super::{SIZE, TAIL};

    fn layout() -> Layout {
        Layout::from_size_align(SIZE + TAIL, SIZE).expect("cage layout")
    }

    /// A cage and its tail, allocated.
    pub(super) fn reserve() -> NonNull<u8> {
        // SAFETY: a non-zero size.
        NonNull::new(unsafe { alloc_zeroed(layout()) }).unwrap_or_else(|| handle_alloc_error(layout()))
    }

    /// # Safety
    /// `p..p + len` is part of a cage, unused.
    pub(super) unsafe fn release(p: NonNull<u8>, len: usize) {
        // SAFETY: forwarded from the caller.
        unsafe { p.as_ptr().write_bytes(0, len) };
    }

    /// Allocated cages are freed, not pooled: zeroing one is a cage's
    /// worth of writes.
    pub(super) fn scrub(base: NonNull<u8>) -> Option<NonNull<u8>> {
        // SAFETY: allocated by `reserve` with this layout, unused now.
        unsafe { std::alloc::dealloc(base.as_ptr(), layout()) };
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enter_bare(cage: &Rc<Cage>) -> Entered {
        enter(cage, &Rc::default())
    }

    fn words_in(cage: &Rc<Cage>, n: usize) -> Vec<u64, Caged> {
        let mut v = Vec::with_capacity_in(n, Caged::new(Budget::new(usize::MAX), cage.clone()));
        v.resize(n, 0);
        v
    }

    #[test]
    fn freed_spans_are_reused_zeroed_and_merged() {
        let cage = Cage::new();
        let _entered = enter_bare(&cage);
        let mut a = words_in(&cage, 1 << 14);
        let b = words_in(&cage, 1 << 14);
        a.fill(7);
        let (pa, pb) = (a.as_ptr(), b.as_ptr());
        assert!(contains(pa) && contains(pb));
        drop(a);
        let c = words_in(&cage, 1 << 14);
        assert_eq!(c.as_ptr(), pa, "first fit reuses the freed span");
        assert!(c.iter().all(|&w| w == 0));
        drop((b, c));
        let d = words_in(&cage, 1 << 15);
        assert_eq!(d.as_ptr(), pa, "freed neighbours merge back into the untouched rest");
    }

    #[test]
    fn any_address_lands_in_the_cage_entered() {
        let (one, two) = (Cage::new(), Cage::new());
        let (v, w) = (words_in(&one, 8), words_in(&two, 8));
        let (p, q) = (v.as_ptr(), w.as_ptr());
        let entered = enter_bare(&one);
        assert_eq!(at(p.addr()).cast_const(), p);
        // Another world's address names memory of this one's cage.
        assert!(contains(confine(one.base(), q.addr())) && !contains(q));
        for forged in [0, usize::MAX, p.addr() ^ (1 << 40), 0xdead_beef] {
            let at = confine(one.base(), forged);
            assert!(contains(at) && at.is_aligned());
        }
        {
            let _inner = enter_bare(&two);
            assert_eq!(at(q.addr()).cast_const(), q);
        }
        assert_eq!(at(p.addr()).cast_const(), p, "the cage entered before, back");
        drop(entered);
        assert!(!contains(p), "none entered");
    }

    #[test]
    fn entries_may_end_in_any_order() {
        let (one, two) = (Cage::new(), Cage::new());
        let first = enter_bare(&one);
        let second = enter_bare(&two);
        let again = enter_bare(&two);
        drop(first);
        assert_eq!(BASE.get(), two.base(), "the newest entry left decides");
        drop(second);
        assert_eq!(BASE.get(), two.base(), "entered again, by a guard left");
        drop(again);
        assert!(BASE.get().is_null(), "none entered");
        // One cage entered twice: the first guard's end leaves it entered.
        let (first, second) = (enter_bare(&one), enter_bare(&one));
        drop(first);
        assert_eq!(BASE.get(), one.base());
        drop(second);
        assert!(BASE.get().is_null());
    }

    #[cfg(all(target_os = "linux", not(miri)))]
    #[test]
    fn a_pooled_cage_is_zero_wherever_its_world_wrote() {
        // What a corrupt heap wrote outside its objects, past every span:
        // a cage is scrubbed whole before it is pooled (`Drop for Cage`).
        let base = os::reserve();
        // SAFETY: a page of the cage no span has, as a forged header leads to.
        unsafe { base.as_ptr().add(SIZE / 2).write(0xa5) };
        let base = os::scrub(base).expect("pooled");
        // SAFETY: as above.
        assert_eq!(unsafe { base.as_ptr().add(SIZE / 2).read() }, 0);
        POOL.lock().unwrap().push(Region(base));
    }

    #[test]
    fn the_tail_is_the_cages() {
        let cage = Cage::new();
        // SAFETY: the tail's last word, in the cage's allocation.
        let last = unsafe { cage.base().add(SIZE + TAIL - 8).cast::<u64>() };
        unsafe { last.write(7) };
        assert_eq!(unsafe { last.read() }, 7);
    }

    #[test]
    fn budgets_are_charged() {
        let budget = Budget::new(1 << 20);
        let mut v: Vec<u64, Caged> = Vec::new_in(Caged::new(budget.clone(), Cage::new()));
        v.try_reserve_exact(1000).unwrap();
        assert_eq!(budget.held(), 8000);
        assert!(v.try_reserve_exact(1 << 20).is_err());
        drop(v);
        assert_eq!(budget.held(), 0);
    }
}
