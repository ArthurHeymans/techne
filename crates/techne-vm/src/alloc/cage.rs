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

trait SpanMap: Default {
    fn first_fit(&self, len: usize) -> Option<(usize, usize)>;
    fn prev_before(&self, off: usize) -> Option<(usize, usize)>;
    fn insert_span(&mut self, off: usize, len: usize);
    fn remove_span(&mut self, off: usize) -> Option<usize>;
    fn max_len(&self) -> usize;
    fn sum_len(&self) -> usize;
}

impl SpanMap for BTreeMap<usize, usize> {
    fn first_fit(&self, len: usize) -> Option<(usize, usize)> {
        self.iter().find(|&(_, &l)| l >= len).map(|(&o, &l)| (o, l))
    }

    fn prev_before(&self, off: usize) -> Option<(usize, usize)> {
        self.range(..off).next_back().map(|(&o, &l)| (o, l))
    }

    fn insert_span(&mut self, off: usize, len: usize) {
        self.insert(off, len);
    }

    fn remove_span(&mut self, off: usize) -> Option<usize> {
        self.remove(&off)
    }

    fn max_len(&self) -> usize {
        self.values().copied().max().unwrap_or(0)
    }

    fn sum_len(&self) -> usize {
        self.values().sum()
    }
}

struct Spans<M = BTreeMap<usize, usize>> {
    /// Everything from here on is untouched.
    top: usize,
    /// Free spans below `top` by offset, none adjacent to another or to `top`.
    free: M,
    /// Their bytes.
    free_bytes: usize,
}

impl<M: SpanMap> Spans<M> {
    fn new() -> Self {
        Spans { top: START, free: M::default(), free_bytes: 0 }
    }

    fn largest(&self, cap: usize) -> usize {
        let rest = cap - self.top;
        // No free span is larger than all of them together.
        if rest >= self.free_bytes { rest } else { self.free.max_len().max(rest) }
    }

    fn used(&self) -> usize {
        debug_assert_eq!(self.free_bytes, self.free.sum_len());
        self.top - START - self.free_bytes
    }

    fn alloc_offset(&mut self, len: usize, cap: usize) -> Option<usize> {
        let len = len.next_multiple_of(PAGE);
        let fit = self.free.first_fit(len);
        let off = match fit {
            Some((o, l)) => {
                self.free.remove_span(o);
                if l > len {
                    self.free.insert_span(o + len, l - len);
                }
                self.free_bytes -= len;
                o
            }
            None => {
                let o = self.top;
                if cap - o < len {
                    return None;
                }
                self.top = o + len;
                o
            }
        };
        Some(off)
    }

    fn free_offset(&mut self, mut off: usize, len: usize) {
        let mut len = len.next_multiple_of(PAGE);
        // Counted free again as it merges; what reaches `top` is not.
        self.free_bytes += len;
        if let Some(l) = self.free.remove_span(off + len) {
            len += l;
        }
        if let Some((o, l)) = self.free.prev_before(off)
            && o + l == off
        {
            self.free.remove_span(o);
            (off, len) = (o, len + l);
        }
        if off + len == self.top {
            self.top = off;
            self.free_bytes -= len;
        } else {
            self.free.insert_span(off, len);
        }
    }
}

impl Cage {
    /// A cage of its own: a pooled region, else one reserved now.
    pub(crate) fn new() -> Rc<Cage> {
        let base = POOL.lock().unwrap_or_else(|e| e.into_inner()).pop().map_or_else(os::reserve, |r| r.0);
        Rc::new(Cage { base, spans: RefCell::new(Spans::new()) })
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

    /// The most one allocation can have of it now: its largest free span,
    /// or the untouched rest. Spans are handed out whole, so an object
    /// larger than every piece of its room does not fit, whatever their
    /// total (`room`).
    pub(crate) fn largest(&self) -> usize {
        self.spans.borrow().largest(capacity())
    }

    /// Bytes its heap holds of it, whether touched or not.
    pub(crate) fn used(&self) -> usize {
        self.spans.borrow().used()
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
        let off = self.spans.borrow_mut().alloc_offset(len, capacity())?;
        // SAFETY: `off + len.next_multiple_of(PAGE)` is within the cage.
        Some(unsafe { self.base.add(off) })
    }

    /// # Safety
    /// `p` was returned by `alloc(len)` and is not used again.
    unsafe fn free(&self, p: NonNull<u8>, len: usize) {
        let rounded = len.next_multiple_of(PAGE);
        // SAFETY: forwarded from the caller.
        unsafe { os::release(p, rounded) };
        let off = p.as_ptr().addr() - self.base.as_ptr().addr();
        self.spans.borrow_mut().free_offset(off, len);
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

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    /// Exhaustive over all cage-aligned non-null 64-bit bases, all offsets within the cage,
    /// and all arbitrary 64-bit addresses (using synthetic provenance-free pointers to verify
    /// address arithmetic, not dereference or provenance safety): `confine` and `near` always
    /// land inside the cage, align to 8 bytes, agree with each other, and satisfy `contains`
    /// while rejecting null or other-cage addresses.
    #[kani::proof]
    fn confine_near_and_contains_enforce_cage_bounds() {
        let base_addr: usize = kani::any();
        kani::assume(base_addr != 0 && (base_addr & MASK) == 0);
        let addr: usize = kani::any();
        let near_off: usize = kani::any();
        kani::assume(near_off <= MASK);

        let base = std::ptr::without_provenance_mut::<u8>(base_addr);
        let p = confine(base, addr);

        assert!(p.addr() & !MASK == base_addr);
        assert!(p.addr() & 7 == 0);
        assert!(p.addr() - base_addr == (addr & MASK & !7));
        assert!(confine(base, p.addr()) == p);

        let near_ptr = std::ptr::without_provenance_mut::<u64>(base_addr | near_off);
        let q = near(near_ptr, addr);
        assert!(q.addr() == p.addr());

        // With no cage entered (null BASE), `contains` rejects every pointer.
        BASE.set(std::ptr::null_mut());
        assert!(!contains(p));
        assert!(!contains(std::ptr::without_provenance::<u64>(addr)));

        // With `base` entered, `contains` holds iff the pointer's high bits match `base_addr`.
        BASE.set(base);
        assert!(contains(p));
        assert!(contains(q));
        let probe = std::ptr::without_provenance::<u64>(addr);
        assert!(contains(probe) == ((addr & !MASK) == base_addr));
        BASE.set(std::ptr::null_mut());
    }

    /// Sorted fixed-capacity ordered map modeling `BTreeMap<usize, usize>`'s `SpanMap`
    /// contract without `std::collections::BTreeMap`'s internal node-rebalancing pointer
    /// machinery, allowing CBMC to verify production `Spans` logic directly.
    struct ArraySpanMap<const N: usize> {
        entries: [(usize, usize); N],
        len: usize,
    }

    impl<const N: usize> Default for ArraySpanMap<N> {
        fn default() -> Self {
            Self { entries: [(0, 0); N], len: 0 }
        }
    }

    impl<const N: usize> ArraySpanMap<N> {
        fn is_empty(&self) -> bool {
            self.len == 0
        }

        fn len(&self) -> usize {
            self.len
        }
    }

    impl<const N: usize> SpanMap for ArraySpanMap<N> {
        fn first_fit(&self, len: usize) -> Option<(usize, usize)> {
            for i in 0..N {
                if i < self.len && self.entries[i].1 >= len {
                    return Some(self.entries[i]);
                }
            }
            None
        }

        fn prev_before(&self, off: usize) -> Option<(usize, usize)> {
            let mut prev = None;
            for i in 0..N {
                if i < self.len && self.entries[i].0 < off {
                    prev = Some(self.entries[i]);
                }
            }
            prev
        }

        fn insert_span(&mut self, off: usize, len: usize) {
            let mut pos = self.len;
            for i in 0..N {
                if i < self.len {
                    if self.entries[i].0 == off {
                        self.entries[i].1 = len;
                        return;
                    }
                    if self.entries[i].0 > off && pos == self.len {
                        pos = i;
                    }
                }
            }
            assert!(self.len < N, "ArraySpanMap capacity exceeded");
            let mut k = N - 1;
            while k > 0 {
                if k <= self.len && k > pos {
                    self.entries[k] = self.entries[k - 1];
                }
                k -= 1;
            }
            self.entries[pos] = (off, len);
            self.len += 1;
        }

        fn remove_span(&mut self, off: usize) -> Option<usize> {
            let mut pos = N;
            for i in 0..N {
                if i < self.len && self.entries[i].0 == off {
                    pos = i;
                    break;
                }
            }
            if pos == N {
                return None;
            }
            let removed = self.entries[pos].1;
            for i in 0..N {
                if i >= pos && i + 1 < self.len {
                    self.entries[i] = self.entries[i + 1];
                }
            }
            self.len -= 1;
            Some(removed)
        }

        fn max_len(&self) -> usize {
            let mut m = 0;
            for i in 0..N {
                if i < self.len && self.entries[i].1 > m {
                    m = self.entries[i].1;
                }
            }
            m
        }

        fn sum_len(&self) -> usize {
            let mut s = 0usize;
            for i in 0..N {
                if i < self.len {
                    s += self.entries[i].1;
                }
            }
            s
        }
    }

    fn assert_spans_invariants<const N: usize>(spans: &Spans<ArraySpanMap<N>>, cap: usize, expected_used: usize, live: &[(usize, usize)]) {
        assert!(spans.top >= START && spans.top <= cap);
        assert!(spans.top % PAGE == 0);
        assert!(spans.used() == expected_used);

        let mut live_sum = 0usize;
        for &(lo, ll) in live {
            if ll > 0 {
                assert!(lo >= START && lo + ll <= spans.top);
                live_sum += ll;
            }
        }
        assert!(live_sum == expected_used);

        let mut sum = 0usize;
        let mut max_hole = 0usize;
        let mut prev_end: Option<usize> = None;
        for i in 0..N {
            if i < spans.free.len {
                let (o, l) = spans.free.entries[i];
                assert!(o >= START);
                assert!(l > 0);
                assert!(o % PAGE == 0);
                assert!(l % PAGE == 0);
                // Every free span stays strictly below `top` (never adjacent to `top`).
                assert!(o + l < spans.top);
                // Adjacent free spans must always have been coalesced.
                if let Some(pe) = prev_end {
                    assert!(pe < o);
                }
                // Every free hole must be disjoint from every live allocation.
                for &(lo, ll) in live {
                    if ll > 0 {
                        assert!(o + l <= lo || lo + ll <= o);
                    }
                }
                prev_end = Some(o + l);
                sum += l;
                if l > max_hole {
                    max_hole = l;
                }
            }
        }
        assert!(sum == spans.free_bytes);
        // Live spans + free holes partition `START..spans.top` with zero overlap and zero gap.
        assert!(live_sum + sum == spans.top - START);
        assert!(spans.largest(cap) == max_hole.max(cap - spans.top));
    }

    /// Bounded check of production `Spans` (parameterized over `ArraySpanMap`, with a
    /// companion `BTreeMap` unit test covering the production adapter) for 3 symbolic
    /// page-rounded allocations (`1..=2 * PAGE` each) and all 6 free orders: spans are
    /// disjoint and page-aligned inside `START..cap`, live spans and free holes partition
    /// `START..top` exactly, left/right/top coalescing maintains strict non-adjacency and
    /// `free_bytes`/`used`/`largest` conservation, and freeing all allocations returns `top`
    /// to `START` with an empty free map.
    #[kani::proof]
    #[kani::unwind(5)]
    fn spans_three_alloc_all_free_orders_preserve_invariants() {
        let len0: usize = kani::any();
        let len1: usize = kani::any();
        let len2: usize = kani::any();
        kani::assume((1..=2 * PAGE).contains(&len0));
        kani::assume((1..=2 * PAGE).contains(&len1));
        kani::assume((1..=2 * PAGE).contains(&len2));
        let r0 = len0.next_multiple_of(PAGE);
        let r1 = len1.next_multiple_of(PAGE);
        let r2 = len2.next_multiple_of(PAGE);

        let cap = START + 6 * PAGE;
        let mut spans = Spans::<ArraySpanMap<3>>::new();
        let mut live = [(0usize, 0usize); 3];
        assert_spans_invariants(&spans, cap, 0, &live);

        let o0 = spans.alloc_offset(len0, cap).unwrap();
        assert!(o0 == START);
        live[0] = (o0, r0);
        assert_spans_invariants(&spans, cap, r0, &live);

        let o1 = spans.alloc_offset(len1, cap).unwrap();
        assert!(o1 == START + r0);
        live[1] = (o1, r1);
        assert_spans_invariants(&spans, cap, r0 + r1, &live);

        let o2 = spans.alloc_offset(len2, cap).unwrap();
        assert!(o2 == START + r0 + r1);
        live[2] = (o2, r2);
        assert_spans_invariants(&spans, cap, r0 + r1 + r2, &live);

        let allocs = [(o0, len0, r0), (o1, len1, r1), (o2, len2, r2)];
        let perm: usize = kani::any();
        kani::assume(perm < 6);
        let order: [usize; 3] = match perm {
            0 => [0, 1, 2],
            1 => [0, 2, 1],
            2 => [1, 0, 2],
            3 => [1, 2, 0],
            4 => [2, 0, 1],
            _ => [2, 1, 0],
        };

        let mut expected_used = r0 + r1 + r2;
        for &idx in &order {
            let (off, len, rounded) = allocs[idx];
            spans.free_offset(off, len);
            live[idx] = (0, 0);
            expected_used -= rounded;
            assert_spans_invariants(&spans, cap, expected_used, &live);
        }

        assert!(spans.top == START);
        assert!(spans.free.is_empty());
        assert!(spans.free_bytes == 0);
        assert!(spans.largest(cap) == cap - START);
    }

    /// Bounded check of production `Spans` (over `ArraySpanMap`) covering multi-hole
    /// first-fit lowest-offset selection, both-side coalescing below `top`, exact hole-map
    /// ranges on reuse (split and exact match), bump fallback when a request exceeds the
    /// hole, and snapshotted free-map preservation on capacity refusal.
    #[kani::proof]
    #[kani::unwind(5)]
    fn spans_both_side_coalesce_and_first_fit_split_preserve_invariants() {
        // 4 live spans of 1 page each, plus 4 pages of headroom above `top`.
        let cap = START + 8 * PAGE;
        let mut spans = Spans::<ArraySpanMap<3>>::new();

        let o0 = spans.alloc_offset(1, cap).unwrap();
        let o1 = spans.alloc_offset(PAGE, cap).unwrap();
        let o2 = spans.alloc_offset(PAGE - 1, cap).unwrap();
        let o3 = spans.alloc_offset(PAGE, cap).unwrap();
        assert!(o0 == START && o1 == START + PAGE && o2 == START + 2 * PAGE && o3 == START + 3 * PAGE);
        let mut live = [(o0, PAGE), (o1, PAGE), (o2, PAGE), (o3, PAGE)];
        assert_spans_invariants(&spans, cap, 4 * PAGE, &live);

        // Free 0 and 2 (leaving 1 and 3 live): two non-adjacent 1-page holes below `top`.
        spans.free_offset(o0, 1);
        live[0] = (0, 0);
        assert_spans_invariants(&spans, cap, 3 * PAGE, &live);
        spans.free_offset(o2, PAGE - 1);
        live[2] = (0, 0);
        assert_spans_invariants(&spans, cap, 2 * PAGE, &live);
        assert!(spans.free.len() == 2);
        assert!(spans.free.entries[0] == (o0, PAGE));
        assert!(spans.free.entries[1] == (o2, PAGE));

        // Free 1: must coalesce with BOTH left (o0) and right (o2) into one 3-page hole.
        spans.free_offset(o1, PAGE);
        live[1] = (0, 0);
        assert_spans_invariants(&spans, cap, PAGE, &live);
        assert!(spans.free.len() == 1);
        assert!(spans.free.entries[0] == (o0, 3 * PAGE));
        assert!(spans.largest(cap) == 4 * PAGE);

        // Allocate symbolic `len_d` in `1..=5 * PAGE`:
        // - 1..=2 pages splits the 3-page hole at `o0`
        // - 3 pages consumes the 3-page hole at `o0` exactly
        // - 4 pages exceeds the hole and falls back to bumping from `top`
        // - 5 pages cannot fit in either the 3-page hole or the 4-page tail => `None`
        let snap_top = spans.top;
        let snap_free_bytes = spans.free_bytes;
        let snap_free_len = spans.free.len;
        let snap_free_entries = spans.free.entries;

        let len_d: usize = kani::any();
        kani::assume((1..=5 * PAGE).contains(&len_d));
        let r_d = len_d.next_multiple_of(PAGE);
        let res = spans.alloc_offset(len_d, cap);
        if r_d < 3 * PAGE {
            let od = res.unwrap();
            assert!(od == o0);
            live[0] = (od, r_d);
            assert_spans_invariants(&spans, cap, PAGE + r_d, &live);
            assert!(spans.free.len() == 1);
            assert!(spans.free.entries[0] == (o0 + r_d, 3 * PAGE - r_d));
            spans.free_offset(od, len_d);
            live[0] = (0, 0);
            assert_spans_invariants(&spans, cap, PAGE, &live);
            assert!(spans.free.len() == 1 && spans.free.entries[0] == (o0, 3 * PAGE));
        } else if r_d == 3 * PAGE {
            let od = res.unwrap();
            assert!(od == o0);
            live[0] = (od, r_d);
            assert_spans_invariants(&spans, cap, PAGE + r_d, &live);
            assert!(spans.free.is_empty());
            spans.free_offset(od, len_d);
            live[0] = (0, 0);
            assert_spans_invariants(&spans, cap, PAGE, &live);
            assert!(spans.free.len() == 1 && spans.free.entries[0] == (o0, 3 * PAGE));
        } else if r_d == 4 * PAGE {
            let od = res.unwrap();
            assert!(od == START + 4 * PAGE);
            live[0] = (od, r_d);
            assert_spans_invariants(&spans, cap, PAGE + r_d, &live);
            assert!(spans.free.len() == 1 && spans.free.entries[0] == (o0, 3 * PAGE));
            spans.free_offset(od, len_d);
            live[0] = (0, 0);
            assert_spans_invariants(&spans, cap, PAGE, &live);
            assert!(spans.free.len() == 1 && spans.free.entries[0] == (o0, 3 * PAGE));
        } else {
            assert!(res.is_none());
            assert!(spans.top == snap_top);
            assert!(spans.free_bytes == snap_free_bytes);
            assert!(spans.free.len == snap_free_len);
            assert!(spans.free.entries == snap_free_entries);
            assert_spans_invariants(&spans, cap, PAGE, &live);
        }

        // Free the top anchor `o3`: the 3-page hole and `o3` collapse `top` back to `START`.
        spans.free_offset(o3, PAGE);
        live[3] = (0, 0);
        assert_spans_invariants(&spans, cap, 0, &live);
        assert!(spans.top == START);
        assert!(spans.free.is_empty());
    }

    /// Bounded check of production `Spans` (over `ArraySpanMap`, paired with the
    /// `BTreeMap` unit test `spans_btreemap_adapter_first_fit_coalescing_and_refusal`)
    /// with two non-adjacent free holes of symbolic sizes (`1` or `2` pages each) and
    /// symbolic allocation request (`1..=3` pages): distinguishes first-fit from last-fit
    /// by asserting lowest-offset selection when both holes fit, skipping the lower hole
    /// when only the upper hole fits, and exact snapshotted state preservation on refusal.
    #[kani::proof]
    #[kani::unwind(5)]
    fn spans_multi_hole_first_fit_selects_lowest_offset() {
        let h0_pages: usize = if kani::any() { 1 } else { 2 };
        let h2_pages: usize = if kani::any() { 1 } else { 2 };
        let r0 = h0_pages * PAGE;
        let r2 = h2_pages * PAGE;
        let top_init = START + r0 + PAGE + r2 + PAGE;
        // Leave exactly 2 pages of tail room above `top_init`.
        let cap = top_init + 2 * PAGE;

        let mut spans = Spans::<ArraySpanMap<3>>::new();
        let o0 = spans.alloc_offset(r0, cap).unwrap();
        let o1 = spans.alloc_offset(PAGE, cap).unwrap();
        let o2 = spans.alloc_offset(r2, cap).unwrap();
        let o3 = spans.alloc_offset(PAGE, cap).unwrap();
        let mut live = [(o0, r0), (o1, PAGE), (o2, r2), (o3, PAGE)];

        spans.free_offset(o0, r0);
        live[0] = (0, 0);
        spans.free_offset(o2, r2);
        live[2] = (0, 0);
        assert_spans_invariants(&spans, cap, 2 * PAGE, &live);
        assert!(spans.free.len() == 2);
        assert!(spans.free.entries[0] == (o0, r0));
        assert!(spans.free.entries[1] == (o2, r2));

        let snap_top = spans.top;
        let snap_free_bytes = spans.free_bytes;
        let snap_free_len = spans.free.len;
        let snap_free_entries = spans.free.entries;

        let req: usize = kani::any();
        kani::assume((1..=3 * PAGE).contains(&req));
        let r_req = req.next_multiple_of(PAGE);
        let res = spans.alloc_offset(req, cap);

        if r_req <= r0 {
            // Lowest fitting hole `o0` must be chosen even when `o2` also fits.
            let got = res.unwrap();
            assert!(got == o0);
            live[0] = (got, r_req);
            assert_spans_invariants(&spans, cap, 2 * PAGE + r_req, &live);
            if r_req < r0 {
                assert!(spans.free.len() == 2);
                assert!(spans.free.entries[0] == (o0 + r_req, r0 - r_req));
                assert!(spans.free.entries[1] == (o2, r2));
            } else {
                assert!(spans.free.len() == 1);
                assert!(spans.free.entries[0] == (o2, r2));
            }
        } else if r_req <= r2 {
            // Only the second hole `o2` fits: `o0` remains untouched and `o2` is selected.
            let got = res.unwrap();
            assert!(got == o2);
            live[2] = (got, r_req);
            assert_spans_invariants(&spans, cap, 2 * PAGE + r_req, &live);
            assert!(spans.free.len() == 1);
            assert!(spans.free.entries[0] == (o0, r0));
        } else if r_req <= cap - top_init {
            // Neither hole fits, tail fits: bump from `top_init`.
            let got = res.unwrap();
            assert!(got == top_init);
            live[0] = (got, r_req);
            assert_spans_invariants(&spans, cap, 2 * PAGE + r_req, &live);
            assert!(spans.free.len() == 2);
            assert!(spans.free.entries[0] == (o0, r0));
            assert!(spans.free.entries[1] == (o2, r2));
        } else {
            // 3 pages exceeds both holes and the 2-page tail => refusal leaves state unchanged.
            assert!(res.is_none());
            assert!(spans.top == snap_top);
            assert!(spans.free_bytes == snap_free_bytes);
            assert!(spans.free.len == snap_free_len);
            assert!(spans.free.entries == snap_free_entries);
            assert_spans_invariants(&spans, cap, 2 * PAGE, &live);
        }
    }
}
