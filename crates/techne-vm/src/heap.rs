//! Generational heap: a copying nursery and an incremental mark-sweep old
//! generation (the structure of OCaml's heap).
//!
//! * Nursery: one bump-allocated region. A minor collection copies every live
//!   nursery object into the old generation (en-masse promotion), so the
//!   nursery is empty afterwards and no old-to-young pointers survive it.
//! * Old generation: objects never move. Small objects live in blocks of one
//!   size class each, with a free list per class; large ones are allocated
//!   individually. A collection cycle marks incrementally, a slice after each
//!   minor collection, then sweeps blocks lazily (when their class needs
//!   space) and in slices. Pauses are a minor collection plus a bounded slice,
//!   and one remark that rescans the roots.
//! * Write barrier: storing a nursery pointer into an old object records that
//!   object in the remembered set, whose fields are roots for the next minor
//!   collection. While marking, storing an old pointer shades it (an
//!   incremental-update, Dijkstra-style barrier): objects stored into the
//!   heap cannot hide from the marker. Roots are not barriered, which is why
//!   the remark rescans them. Objects promoted or allocated in the old
//!   generation during marking are marked, and their fields shaded when the
//!   next minor collection scans them.
//! * Mark steps only run right after a minor collection, when the nursery is
//!   empty, so the marker only ever sees old objects.
//!
//! Object layout: one header word followed by fields. Header bits 0..8 hold the
//! kind, bits 8..16 flags, bits 16..64 the length. Traced kinds store only
//! `Value`s after the header; raw kinds (strings, big integers) are never scanned.
//! A forwarded nursery object's header holds `(new_address << 16) | FORWARDED`;
//! a free old slot's header is `FREE` with the next free slot in its first field.

use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

use crate::value::Value;

pub const NURSERY_WORDS: usize = 1 << 20; // 8 MiB

/// Nursery size in words: `TECHNE_NURSERY_KB` or `NURSERY_WORDS`.
fn nursery_words() -> usize {
    std::env::var("TECHNE_NURSERY_KB").ok().and_then(|v| v.parse::<usize>().ok()).map_or(NURSERY_WORDS, |kb| (kb * 128).max(1 << 12))
}
/// Objects at least this large are allocated directly in the old generation.
pub const LARGE_WORDS: usize = 1 << 14;
/// Old-generation size (words) at which the first cycle starts.
const INITIAL_THRESHOLD: usize = 8 << 20; // 64 MiB
/// Old-generation block size, and the largest object kept in blocks.
const BLOCK_WORDS: usize = 1 << 15; // 256 KiB
const MAX_SMALL: usize = BLOCK_WORDS / 8;
/// Words marked per slice at least, and per promoted word.
const MARK_SLICE: usize = 1 << 17;
const MARK_PER_PROMOTED: usize = 1;
/// Blocks swept per slice at least.
const SWEEP_SLICE: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Pair = 1,
    Vector = 2,
    /// Fields: code index (fixnum), then captured values.
    Closure = 3,
    Box = 4,
    /// Fields: count (fixnum), slot vector (key/value pairs).
    Table = 5,
    /// Fields: record type, then the record's fields.
    Record = 6,
    /// Record type descriptor. Fields: name (symbol), field names (list), id.
    Rtd = 7,
    String = 16,
    BigInt = 17,
    /// A Rust value owned by the VM's foreign table; the payload is the index.
    Foreign = 18,
}

const FORWARDED: u64 = 0xFF;
const FREE: u64 = 0xFE;
const REMEMBERED: u64 = 1 << 8;
pub const ASCII: u64 = 1 << 9;
const MARKED: u64 = 1 << 10;
const KIND_MASK: u64 = 0xFF;

#[inline(always)]
pub fn header(kind: Kind, len: usize, flags: u64) -> u64 {
    ((len as u64) << 16) | flags | kind as u64
}
#[inline(always)]
pub fn header_len(h: u64) -> usize {
    (h >> 16) as usize
}
#[inline(always)]
pub fn header_kind(h: u64) -> u8 {
    (h & KIND_MASK) as u8
}

/// Size in words, header included.
#[inline(always)]
fn object_words(h: u64) -> usize {
    match header_kind(h) {
        k if k < Kind::String as u8 => 1 + header_len(h),
        k if k == Kind::String as u8 => 1 + header_len(h).div_ceil(8),
        _ => 2, // BigInt, Foreign
    }
}
#[inline(always)]
fn is_traced(h: u64) -> bool {
    header_kind(h) < Kind::String as u8
}

pub fn string_words(len: usize) -> usize {
    1 + len.div_ceil(8)
}

/// Size classes in words: exact up to 16, then about 12.5% apart.
struct Classes {
    words: Vec<usize>,
    /// Class index by object size (words), up to `MAX_SMALL`.
    of: Vec<u8>,
}

fn classes() -> &'static Classes {
    static C: OnceLock<Classes> = OnceLock::new();
    C.get_or_init(|| {
        let mut words: Vec<usize> = (2..=16).collect();
        while *words.last().unwrap() < MAX_SMALL {
            let w = *words.last().unwrap();
            words.push((w + w.div_ceil(8)).min(MAX_SMALL));
        }
        let mut of = vec![0u8; MAX_SMALL + 1];
        let mut c = 0;
        for (size, slot) in of.iter_mut().enumerate() {
            while words[c] < size {
                c += 1;
            }
            *slot = c as u8;
        }
        Classes { words, of }
    })
}

struct Block {
    mem: Box<[u64]>,
    class: u8,
}

/// The old generation's memory.
struct OldSpace {
    blocks: Vec<Block>,
    /// Free list head per class (null when empty).
    free: Vec<*mut u64>,
    /// Unused rest of the newest block per class: (cursor, end). Slots there
    /// are still zero, which sweeping treats as free.
    fresh: Vec<(*mut u64, *mut u64)>,
    /// Blocks per class not swept since the last mark.
    unswept: Vec<Vec<u32>>,
    large: Vec<Box<[u64]>>,
    /// Words in allocated slots and large objects.
    words: usize,
    /// `classes()`, cached for the allocation fast path.
    class_words: &'static [usize],
    class_of: &'static [u8],
}

impl OldSpace {
    fn new() -> OldSpace {
        let n = classes().words.len();
        let null = std::ptr::null_mut();
        OldSpace {
            blocks: Vec::new(),
            free: vec![null; n],
            fresh: vec![(null, null); n],
            unswept: vec![Vec::new(); n],
            large: Vec::new(),
            words: 0,
            class_words: &classes().words,
            class_of: &classes().of,
        }
    }

    #[inline]
    fn alloc(&mut self, words: usize) -> *mut u64 {
        if words <= MAX_SMALL {
            let c = self.class_of[words] as usize;
            let size = self.class_words[c];
            let p = self.free[c];
            if !p.is_null() {
                self.free[c] = unsafe { *p.add(1) } as *mut u64;
                self.words += size;
                return p;
            }
            let (cursor, end) = self.fresh[c];
            if cursor < end {
                self.fresh[c].0 = unsafe { cursor.add(size) };
                self.words += size;
                return cursor;
            }
        }
        self.alloc_slow(words)
    }

    #[cold]
    fn alloc_slow(&mut self, words: usize) -> *mut u64 {
        if words > MAX_SMALL {
            let mut mem = vec![0u64; words].into_boxed_slice();
            let p = mem.as_mut_ptr();
            self.large.push(mem);
            self.words += words;
            return p;
        }
        let c = self.class_of[words] as usize;
        // Sweep at most a couple of blocks for room: in a mostly live heap,
        // sweeping on until a free slot turns up would take a long pause.
        for _ in 0..2 {
            let Some(b) = self.unswept[c].pop() else { break };
            self.sweep_block(b as usize);
            if !self.free[c].is_null() {
                return self.alloc(words);
            }
        }
        self.new_block(c);
        self.alloc(words)
    }

    fn push_free(&mut self, c: usize, p: *mut u64) {
        unsafe {
            *p = FREE;
            *p.add(1) = self.free[c] as u64;
        }
        self.free[c] = p;
    }

    fn new_block(&mut self, c: usize) {
        let size = classes().words[c];
        let mut mem = vec![0u64; BLOCK_WORDS].into_boxed_slice();
        let base = mem.as_mut_ptr();
        self.blocks.push(Block { mem, class: c as u8 });
        self.fresh[c] = (base, unsafe { base.add(BLOCK_WORDS / size * size) });
    }

    /// Free the unmarked objects of block `b` and unmark the others.
    fn sweep_block(&mut self, b: usize) {
        let c = self.blocks[b].class as usize;
        let size = classes().words[c];
        let base = self.blocks[b].mem.as_mut_ptr();
        for i in 0..BLOCK_WORDS / size {
            let p = unsafe { base.add(i * size) };
            let h = unsafe { *p };
            if h == 0 || header_kind(h) == FREE as u8 {
                // Free, or never used.
                self.push_free(c, p);
            } else if h & MARKED != 0 {
                unsafe { *p = h & !MARKED };
            } else {
                self.words -= size;
                self.push_free(c, p);
            }
        }
    }

    /// Start sweeping: every block's free slots are found again by sweeping it.
    fn begin_sweep(&mut self) {
        let null = std::ptr::null_mut();
        self.free.iter_mut().for_each(|f| *f = null);
        // Sweeping finds the unused rest of each newest block too.
        self.fresh.iter_mut().for_each(|f| *f = (null, null));
        self.unswept.iter_mut().for_each(Vec::clear);
        for (i, b) in self.blocks.iter().enumerate() {
            self.unswept[b.class as usize].push(i as u32);
        }
        let mut freed = 0;
        self.large.retain_mut(|mem| {
            let h = mem[0];
            if h & MARKED != 0 {
                mem[0] = h & !MARKED;
                true
            } else {
                freed += mem.len();
                false
            }
        });
        self.words -= freed;
    }

    /// Sweep up to `n` blocks; returns whether all are swept.
    fn sweep_some(&mut self, mut n: usize) -> bool {
        for c in 0..self.unswept.len() {
            while n > 0 {
                let Some(b) = self.unswept[c].pop() else { break };
                self.sweep_block(b as usize);
                n -= 1;
            }
        }
        self.unswept.iter().all(Vec::is_empty)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Marking,
    Sweeping,
}

#[derive(Default, Debug, Clone)]
pub struct GcStats {
    pub minor: u64,
    /// Completed old-generation cycles.
    pub full: u64,
    pub promoted_words: u64,
    pub minor_time: Duration,
    /// Time in old-generation marking and sweeping.
    pub full_time: Duration,
    /// Longest single pause (a minor collection with its old-generation slice).
    pub max_pause: Duration,
    /// Longest minor collection, and longest old-generation slice.
    pub max_minor: Duration,
    pub max_slice: Duration,
    /// Most words marked in one pause (slice or remark).
    pub max_slice_words: usize,
}

pub struct Heap {
    _nursery: Box<[u64]>,
    nursery_start: *mut u64,
    nursery_end: *mut u64,
    top: *mut u64,
    old: OldSpace,
    remembered: Vec<*mut u64>,
    /// Promoted objects whose fields still need scanning (minor collection).
    scan: Vec<*mut u64>,
    /// Gray objects: marked, fields not yet shaded.
    gray: Vec<*mut u64>,
    phase: Phase,
    /// `phase == Marking`, for the barrier's fast path.
    marking: bool,
    marked_words: usize,
    /// Words scanned by the marker in the current pause.
    slice_words: usize,
    /// Old-generation size at which the next cycle starts.
    threshold: usize,
    /// Addresses of live foreign objects, to detect their death.
    foreign: Vec<*mut u64>,
    /// Foreign-table indices whose objects died in the last collection.
    pub dead_foreign: Vec<usize>,
    /// `TECHNE_GC_STRESS`: collect on every allocation, with a cycle always in
    /// progress and tiny slices; `=full` completes a whole cycle every time.
    pub stress: bool,
    pub stress_full: bool,
    pub stats: GcStats,
}

/// Visitor over root slots, supplied by the VM.
pub trait Roots {
    fn visit(&mut self, f: &mut dyn FnMut(&mut Value));
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    pub fn new() -> Heap {
        let words = nursery_words();
        let mut nursery = vec![0u64; words].into_boxed_slice();
        let start = nursery.as_mut_ptr();
        let stress = std::env::var_os("TECHNE_GC_STRESS").is_some();
        Heap {
            nursery_start: start,
            nursery_end: unsafe { start.add(words) },
            top: start,
            _nursery: nursery,
            old: OldSpace::new(),
            remembered: Vec::new(),
            scan: Vec::new(),
            gray: Vec::new(),
            phase: Phase::Idle,
            marking: false,
            marked_words: 0,
            slice_words: 0,
            threshold: INITIAL_THRESHOLD,
            foreign: Vec::new(),
            dead_foreign: Vec::new(),
            stress,
            stress_full: std::env::var("TECHNE_GC_STRESS").is_ok_and(|v| v == "full"),
            stats: GcStats::default(),
        }
    }

    #[inline(always)]
    pub fn in_nursery(&self, p: *mut u64) -> bool {
        p >= self.nursery_start && p < self.nursery_end
    }

    /// True if `words` can be bump-allocated without collecting.
    #[inline(always)]
    pub fn has_room(&self, words: usize) -> bool {
        !self.stress && (self.nursery_end as usize - self.top as usize) / 8 >= words
    }

    /// The bump pointer's address and the nursery end, for allocation inlined
    /// in JIT code; null under GC stress, where every allocation collects.
    pub fn bump_pointers(&mut self) -> (*mut *mut u64, *mut u64) {
        if self.stress { (std::ptr::null_mut(), std::ptr::null_mut()) } else { (&mut self.top, self.nursery_end) }
    }

    /// Bump-allocate in the nursery. Caller guarantees `has_room(words)` or has
    /// just collected with the nursery large enough.
    #[inline(always)]
    pub fn bump(&mut self, words: usize) -> *mut u64 {
        let p = self.top;
        debug_assert!((self.nursery_end as usize - p as usize) / 8 >= words);
        self.top = unsafe { p.add(words) };
        p
    }

    /// Allocate one object directly in the old generation. The caller writes
    /// its header and fields before the next collection. It is remembered: its
    /// fields may be initialised with nursery pointers (or, while marking,
    /// unshaded old ones) without a barrier, and the next minor collection
    /// scans it.
    pub fn alloc_old(&mut self, words: usize) -> *mut u64 {
        let p = self.old.alloc(words);
        self.remembered.push(p);
        p
    }

    /// Record an old object whose fields may point into the nursery.
    pub fn remember(&mut self, obj: *mut u64) {
        self.remembered.push(obj);
    }

    /// Old-generation allocation for objects only referencing old/immediate
    /// values (compile-time constants).
    pub fn alloc_old_unremembered(&mut self, words: usize) -> *mut u64 {
        if self.marking {
            // Marked when the next minor collection scans it.
            return self.alloc_old(words);
        }
        self.old.alloc(words)
    }

    pub fn register_foreign(&mut self, obj: *mut u64) {
        self.foreign.push(obj);
    }

    /// After a minor collection: follow promoted foreign objects and record
    /// dead ones.
    fn sweep_foreign_nursery(&mut self) {
        let mut live = Vec::with_capacity(self.foreign.len());
        for &p in &self.foreign {
            if !self.in_nursery(p) {
                live.push(p);
                continue;
            }
            let h = unsafe { *p };
            if header_kind(h) == FORWARDED as u8 {
                live.push((h >> 16) as *mut u64);
            } else {
                self.dead_foreign.push(unsafe { *p.add(1) } as usize);
            }
        }
        self.foreign = live;
    }

    /// At the end of marking: old foreign objects left unmarked are dead.
    fn sweep_foreign_old(&mut self) {
        let mut live = Vec::with_capacity(self.foreign.len());
        for &p in &self.foreign {
            if unsafe { *p } & MARKED != 0 {
                live.push(p);
            } else {
                self.dead_foreign.push(unsafe { *p.add(1) } as usize);
            }
        }
        self.foreign = live;
    }

    pub fn nursery_capacity(&self) -> usize {
        (self.nursery_end as usize - self.nursery_start as usize) / 8
    }

    /// Words in use in the old generation.
    pub fn old_words(&self) -> usize {
        self.old.words
    }

    #[inline(always)]
    pub fn barrier(&mut self, obj: *mut u64, v: Value) {
        if !v.is_ptr() {
            return;
        }
        let p = v.as_ptr();
        if self.in_nursery(p) {
            if !self.in_nursery(obj) {
                unsafe {
                    if *obj & REMEMBERED == 0 {
                        *obj |= REMEMBERED;
                        self.remembered.push(obj);
                    }
                }
            }
        } else if self.marking {
            self.shade(p);
        }
    }

    /// Mark an old object gray if it is not marked yet.
    #[inline]
    fn shade(&mut self, p: *mut u64) {
        unsafe {
            let h = *p;
            if h & MARKED == 0 {
                *p = h | MARKED;
                self.marked_words += object_words(h);
                if is_traced(h) {
                    self.gray.push(p);
                }
            }
        }
    }

    fn shade_value(&mut self, v: Value) {
        if v.is_ptr() && !self.in_nursery(v.as_ptr()) {
            self.shade(v.as_ptr());
        }
    }

    /// A minor collection, followed by a slice of old-generation work.
    pub fn collect(&mut self, roots: &mut dyn Roots) {
        let start = Instant::now();
        let promoted = self.minor(roots);
        let minor = start.elapsed();
        self.stats.minor += 1;
        self.stats.promoted_words += promoted as u64;
        self.stats.minor_time += minor;
        self.slice_words = 0;
        if self.stress_full {
            self.finish_cycle(roots);
        } else {
            let budget = if self.stress { 64 } else { MARK_SLICE + MARK_PER_PROMOTED * promoted };
            self.old_slice(roots, budget);
        }
        let pause = start.elapsed();
        self.stats.full_time += pause - minor;
        self.stats.max_pause = self.stats.max_pause.max(pause);
        self.stats.max_minor = self.stats.max_minor.max(minor);
        self.stats.max_slice = self.stats.max_slice.max(pause - minor);
        self.stats.max_slice_words = self.stats.max_slice_words.max(self.slice_words);
    }

    /// Complete a whole old-generation cycle now (after a minor collection).
    pub fn full_collect(&mut self, roots: &mut dyn Roots) {
        let start = Instant::now();
        let promoted = self.minor(roots);
        self.stats.minor += 1;
        self.stats.promoted_words += promoted as u64;
        if self.phase == Phase::Marking {
            // Finish the cycle in progress, then run a fresh one: objects that
            // died during it are only found by the next.
            self.finish_cycle(roots);
        }
        self.finish_cycle(roots);
        let pause = start.elapsed();
        self.stats.full_time += pause;
        self.stats.max_pause = self.stats.max_pause.max(pause);
    }

    /// Run the current cycle (starting one if idle) to its end.
    fn finish_cycle(&mut self, roots: &mut dyn Roots) {
        if self.phase == Phase::Sweeping {
            self.old.sweep_some(usize::MAX);
            self.end_sweep();
        }
        if self.phase == Phase::Idle {
            self.begin_mark(roots);
        }
        self.mark(usize::MAX);
        self.remark(roots);
        self.old.sweep_some(usize::MAX);
        self.end_sweep();
    }

    /// One bounded slice of marking or sweeping, or starting a cycle.
    fn old_slice(&mut self, roots: &mut dyn Roots, budget: usize) {
        match self.phase {
            Phase::Idle => {
                // Under stress a cycle is always in progress.
                if self.stress || self.old.words > self.threshold {
                    self.begin_mark(roots);
                    self.mark(budget);
                }
            }
            Phase::Marking => {
                if self.mark(budget) {
                    self.remark(roots);
                }
            }
            Phase::Sweeping => {
                let blocks = if self.stress { 1 } else { SWEEP_SLICE + budget / BLOCK_WORDS };
                if self.old.sweep_some(blocks) {
                    self.end_sweep();
                }
            }
        }
    }

    fn begin_mark(&mut self, roots: &mut dyn Roots) {
        self.phase = Phase::Marking;
        self.marking = true;
        self.marked_words = 0;
        roots.visit(&mut |v| self.shade_value(*v));
    }

    /// Shade the fields of gray objects until about `budget` words are
    /// scanned; returns whether no gray object is left.
    fn mark(&mut self, budget: usize) -> bool {
        let mut done = 0;
        while done < budget {
            let Some(obj) = self.gray.pop() else {
                self.slice_words += done;
                return true;
            };
            unsafe {
                let n = header_len(*obj);
                for i in 1..=n {
                    self.shade_value(Value::from_bits(*obj.add(i)));
                }
                done += n + 1;
            }
        }
        self.slice_words += done;
        self.gray.is_empty()
    }

    /// Rescan the roots (they have no barrier), finish marking, release dead
    /// foreign objects and start sweeping.
    fn remark(&mut self, roots: &mut dyn Roots) {
        roots.visit(&mut |v| self.shade_value(*v));
        self.mark(usize::MAX);
        self.sweep_foreign_old();
        self.marking = false;
        self.phase = Phase::Sweeping;
        self.old.begin_sweep();
    }

    fn end_sweep(&mut self) {
        self.phase = Phase::Idle;
        self.stats.full += 1;
        self.threshold = (self.marked_words * 2).max(INITIAL_THRESHOLD);
    }

    /// Copy live nursery objects into the old generation; returns the words
    /// promoted.
    fn minor(&mut self, roots: &mut dyn Roots) -> usize {
        let before = self.old.words;
        unsafe {
            roots.visit(&mut |v| *v = self.evacuate(*v));
            if self.marking { self.scan_promoted::<true>() } else { self.scan_promoted::<false>() }
        }
        self.sweep_foreign_nursery();
        self.top = self.nursery_start;
        self.old.words - before
    }

    /// Copy the nursery object `v` points to into the old generation unless
    /// it is old or already copied.
    #[inline(always)]
    unsafe fn evacuate(&mut self, v: Value) -> Value {
        if !v.is_ptr() {
            return v;
        }
        let p = v.as_ptr();
        if !self.in_nursery(p) {
            return v;
        }
        unsafe {
            let h = *p;
            if header_kind(h) == FORWARDED as u8 {
                return Value::ptr((h >> 16) as *mut u64);
            }
            let words = object_words(h);
            let dst = self.old.alloc(words);
            std::ptr::copy_nonoverlapping(p, dst, words);
            // Promoted while marking: marked, and its fields shaded below.
            *dst = (h & !(REMEMBERED | MARKED)) | if self.marking { MARKED } else { 0 };
            if self.marking {
                self.marked_words += words;
            }
            *p = ((dst as u64) << 16) | FORWARDED;
            if is_traced(h) {
                self.scan.push(dst);
            }
            Value::ptr(dst)
        }
    }

    /// Scan remembered and promoted objects: evacuate the nursery objects
    /// they point to and, while marking, shade the old ones.
    unsafe fn scan_promoted<const MARKING: bool>(&mut self) {
        unsafe {
            for obj in std::mem::take(&mut self.remembered) {
                // Remembered objects are old (or freshly allocated old, which
                // while marking must be marked themselves).
                *obj &= !REMEMBERED;
                if MARKING {
                    self.shade(obj);
                }
                self.scan_object::<MARKING>(obj);
            }
            while let Some(obj) = self.scan.pop() {
                self.scan_object::<MARKING>(obj);
            }
        }
    }

    #[inline(always)]
    unsafe fn scan_object<const MARKING: bool>(&mut self, obj: *mut u64) {
        unsafe {
            let h = *obj;
            if is_traced(h) {
                for i in 1..=header_len(h) {
                    let slot = obj.add(i);
                    let v = self.evacuate(Value::from_bits(*slot));
                    *slot = v.bits();
                    if MARKING {
                        self.shade_value(v);
                    }
                }
            }
        }
    }
}

// ----- object access helpers -----

#[inline(always)]
pub unsafe fn field(obj: *mut u64, i: usize) -> Value {
    unsafe { Value::from_bits(*obj.add(1 + i)) }
}
#[inline(always)]
pub unsafe fn set_field(obj: *mut u64, i: usize, v: Value) {
    unsafe { *obj.add(1 + i) = v.bits() }
}
#[inline(always)]
pub unsafe fn kind_of(obj: *mut u64) -> u8 {
    unsafe { header_kind(*obj) }
}
#[inline(always)]
pub unsafe fn len_of(obj: *mut u64) -> usize {
    unsafe { header_len(*obj) }
}
pub unsafe fn str_bytes<'a>(obj: *mut u64) -> &'a [u8] {
    unsafe { std::slice::from_raw_parts(obj.add(1) as *const u8, header_len(*obj)) }
}
pub unsafe fn str_is_ascii(obj: *mut u64) -> bool {
    unsafe { *obj & ASCII != 0 }
}

#[inline(always)]
pub fn is_kind(v: Value, k: Kind) -> bool {
    v.is_ptr() && unsafe { kind_of(v.as_ptr()) } == k as u8
}
