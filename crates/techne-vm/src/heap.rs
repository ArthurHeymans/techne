//! Generational copying heap.
//!
//! * Nursery: one bump-allocated region. A minor collection copies every live
//!   nursery object into the old space (en-masse promotion), so the nursery is
//!   empty afterwards and no old-to-young pointers survive a minor collection.
//! * Old space: a list of bump-allocated chunks. A full collection copies all
//!   live objects (old and young) into fresh chunks (Cheney).
//! * Write barrier: storing a nursery pointer into an old object records that
//!   object in the remembered set, whose fields are roots for the next minor GC.
//!
//! Object layout: one header word followed by fields. Header bits 0..8 hold the
//! kind, bits 8..16 flags, bits 16..64 the length. Traced kinds store only
//! `Value`s after the header; raw kinds (strings, big integers) are never scanned.
//! A forwarded object's header holds `(new_address << 16) | FORWARDED`.

use std::time::{Duration, Instant};

use crate::value::Value;

pub const NURSERY_WORDS: usize = 1 << 20; // 8 MiB
const CHUNK_WORDS: usize = 1 << 19; // 4 MiB
/// Objects at least this large are allocated directly in the old space.
pub const LARGE_WORDS: usize = 1 << 14;
const INITIAL_FULL_THRESHOLD: usize = 8 << 20; // words (64 MiB)

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
const REMEMBERED: u64 = 1 << 8;
pub const ASCII: u64 = 1 << 9;
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

struct Chunk {
    mem: Box<[u64]>,
    used: usize,
}

#[derive(Default)]
struct Space {
    chunks: Vec<Chunk>,
    words: usize,
}

impl Space {
    fn alloc(&mut self, words: usize) -> *mut u64 {
        self.words += words;
        if let Some(c) = self.chunks.last_mut()
            && c.used + words <= c.mem.len()
        {
            let p = unsafe { c.mem.as_mut_ptr().add(c.used) };
            c.used += words;
            return p;
        }
        let mut mem = vec![0u64; words.max(CHUNK_WORDS)].into_boxed_slice();
        let p = mem.as_mut_ptr();
        self.chunks.push(Chunk { mem, used: words });
        p
    }
    /// Allocation cursor, used as a Cheney scan start.
    fn cursor(&self) -> (usize, usize) {
        self.chunks.last().map_or((0, 0), |c| (self.chunks.len() - 1, c.used))
    }
}

#[derive(Default, Debug, Clone)]
pub struct GcStats {
    pub minor: u64,
    pub full: u64,
    pub promoted_words: u64,
    pub minor_time: Duration,
    pub full_time: Duration,
    pub max_pause: Duration,
}

pub struct Heap {
    _nursery: Box<[u64]>,
    nursery_start: *mut u64,
    nursery_end: *mut u64,
    top: *mut u64,
    old: Space,
    remembered: Vec<*mut u64>,
    /// Addresses of live foreign objects, to detect their death.
    foreign: Vec<*mut u64>,
    /// Foreign-table indices whose objects died in the last collection.
    pub dead_foreign: Vec<usize>,
    full_threshold: usize,
    /// `TECHNE_GC_STRESS`: collect on every allocation; `=full` makes every
    /// collection a full one.
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
        let mut nursery = vec![0u64; NURSERY_WORDS].into_boxed_slice();
        let start = nursery.as_mut_ptr();
        Heap {
            nursery_start: start,
            nursery_end: unsafe { start.add(NURSERY_WORDS) },
            top: start,
            _nursery: nursery,
            old: Space::default(),
            remembered: Vec::new(),
            foreign: Vec::new(),
            dead_foreign: Vec::new(),
            full_threshold: INITIAL_FULL_THRESHOLD,
            stress: std::env::var_os("TECHNE_GC_STRESS").is_some(),
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

    /// Allocate directly in the old space. The object is conservatively
    /// remembered because its fields may later be initialised with nursery
    /// pointers without a barrier.
    pub fn alloc_old(&mut self, words: usize) -> *mut u64 {
        let p = self.old.alloc(words);
        self.remembered.push(p);
        p
    }

    /// Record an old object whose fields may point into the nursery.
    pub fn remember(&mut self, obj: *mut u64) {
        self.remembered.push(obj);
    }

    /// Old-space allocation for objects only referencing old/immediate values
    /// (compile-time constants).
    pub fn alloc_old_unremembered(&mut self, words: usize) -> *mut u64 {
        self.old.alloc(words)
    }

    pub fn register_foreign(&mut self, obj: *mut u64) {
        self.foreign.push(obj);
    }

    /// After evacuation: follow forwarded foreign objects and record dead ones.
    /// Must run before from-space memory is released or reused.
    fn sweep_foreign(&mut self, full: bool) {
        let mut live = Vec::with_capacity(self.foreign.len());
        for &p in &self.foreign {
            if !full && !self.in_nursery(p) {
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

    pub fn nursery_capacity(&self) -> usize {
        NURSERY_WORDS
    }

    #[inline(always)]
    pub fn barrier(&mut self, obj: *mut u64, v: Value) {
        if v.is_ptr() && self.in_nursery(v.as_ptr()) && !self.in_nursery(obj) {
            unsafe {
                if *obj & REMEMBERED == 0 {
                    *obj |= REMEMBERED;
                    self.remembered.push(obj);
                }
            }
        }
    }

    /// Minor collection, escalating to a full one when the old space is large.
    pub fn collect(&mut self, roots: &mut dyn Roots) {
        if self.stress_full {
            return self.full_collect(roots);
        }
        let start = Instant::now();
        let before = self.old.words;
        let scan = self.old.cursor();
        unsafe {
            roots.visit(&mut |v| *v = self.evacuate(*v, false));
            for obj in std::mem::take(&mut self.remembered) {
                // Only old objects are remembered; the header might carry a
                // stale REMEMBERED bit from a large object promoted earlier.
                *obj &= !REMEMBERED;
                self.scan_object(obj, false);
            }
            self.cheney(scan, false);
        }
        self.sweep_foreign(false);
        self.top = self.nursery_start;
        let pause = start.elapsed();
        self.stats.minor += 1;
        self.stats.promoted_words += (self.old.words - before) as u64;
        self.stats.minor_time += pause;
        self.stats.max_pause = self.stats.max_pause.max(pause);
        if self.old.words > self.full_threshold {
            self.full_collect(roots);
        }
    }

    pub fn full_collect(&mut self, roots: &mut dyn Roots) {
        let start = Instant::now();
        let from = std::mem::take(&mut self.old);
        self.remembered.clear();
        unsafe {
            roots.visit(&mut |v| *v = self.evacuate(*v, true));
            self.cheney((0, 0), true);
        }
        self.sweep_foreign(true);
        drop(from);
        self.top = self.nursery_start;
        self.full_threshold = (self.old.words * 2).max(INITIAL_FULL_THRESHOLD);
        let pause = start.elapsed();
        self.stats.full += 1;
        self.stats.full_time += pause;
        self.stats.max_pause = self.stats.max_pause.max(pause);
    }

    /// Copy the object `v` points to into the old space unless it is already
    /// there (minor) or already copied.
    #[inline]
    unsafe fn evacuate(&mut self, v: Value, full: bool) -> Value {
        if !v.is_ptr() {
            return v;
        }
        let p = v.as_ptr();
        if !full && !self.in_nursery(p) {
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
            *dst &= !REMEMBERED;
            *p = ((dst as u64) << 16) | FORWARDED;
            Value::ptr(dst)
        }
    }

    unsafe fn scan_object(&mut self, obj: *mut u64, full: bool) {
        unsafe {
            let h = *obj;
            if is_traced(h) {
                for i in 1..=header_len(h) {
                    let slot = obj.add(i);
                    *slot = self.evacuate(Value::from_bits(*slot), full).bits();
                }
            }
        }
    }

    /// Scan copied objects from `start` until the allocation cursor stops moving.
    unsafe fn cheney(&mut self, start: (usize, usize), full: bool) {
        let (mut chunk, mut offset) = start;
        while chunk < self.old.chunks.len() {
            while offset < self.old.chunks[chunk].used {
                let obj = unsafe { self.old.chunks[chunk].mem.as_mut_ptr().add(offset) };
                let words = object_words(unsafe { *obj });
                unsafe { self.scan_object(obj, full) };
                offset += words;
            }
            chunk += 1;
            offset = 0;
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
