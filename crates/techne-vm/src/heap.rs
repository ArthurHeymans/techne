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
//! * Memory held: the nursery, the old blocks and the large objects
//!   (`committed`). A block that sweeping finds empty is set aside, and
//!   released once the VM has dropped what it keeps of the dead objects
//!   (`release_empty`): until then its objects can still be looked at.
//!
//! Object layout: one header word followed by fields. Header bits 0..8 hold the
//! kind, bits 8..16 flags, bits 16..64 the length. Traced kinds store only
//! `Value`s after the header; raw kinds (strings, big integers) are never scanned.
//! A forwarded nursery object's header holds `(new_address << 16) | FORWARDED`;
//! a free old slot's header is `FREE` with the next free slot in its first field.
//! Like value payloads, these address words become pointers again through the
//! heap's cage (`alloc::near`), with its provenance.

use std::{
    alloc::{Allocator, Layout},
    sync::OnceLock,
    time::{Duration, Instant},
};

use crate::{
    alloc::{Budget, Caged, near},
    value::Value,
};

#[cfg(feature = "allocation-census")]
mod census;
mod verify;

/// Keep GC storage as vectors, not boxes: raw object pointers remain usable
/// while the collector mutates their backing buffers. All of it is in the
/// world's cage (`alloc::cage`); none if it has no room, or the budget
/// refuses.
fn try_zeroed_words(words: usize, alloc: Caged) -> Option<Vec<u64, Caged>> {
    let layout = Layout::array::<u64>(words).expect("heap buffer layout");
    let allocation = alloc.allocate_zeroed(layout).ok()?;
    // SAFETY: the allocator returned zeroed storage with exactly this layout;
    // every word is initialised, and the vector retains the same allocator.
    Some(unsafe { Vec::from_raw_parts_in(allocation.cast::<u64>().as_ptr(), words, words, alloc) })
}

/// `try_zeroed_words` where there must be room: a world is refused room
/// (`Vm::room`, what one allocation can have) while its cage keeps a
/// reserve for this, and a collection is admitted before it relocates
/// (`Vm::collect_with`). Refused by the budget, the world is past its
/// memory ceiling; the cage's free bytes in pieces too small, it was
/// admitted past its room; no room in the cage at all means the heap is
/// corrupt (a header asking more than the reserve, for an object copied
/// in a collection). Either way the world is retired, not the process.
fn zeroed_words(words: usize, alloc: Caged) -> Vec<u64, Caged> {
    let budget = alloc.budget().clone();
    let cage = alloc.cage().clone();
    try_zeroed_words(words, alloc).unwrap_or_else(|| {
        let bytes = words.saturating_mul(8);
        if budget.headroom() < bytes {
            crate::alloc::past_ceiling(bytes)
        } else if cage.room() >= bytes {
            panic!("out of memory: {bytes} bytes asked of the world's heap cage, whose largest free piece is {}", cage.largest())
        } else {
            corrupt("an object larger than its cage's room")
        }
    })
}

/// The nursery's largest size, in words.
pub const NURSERY_WORDS: usize = 1 << 20; // 8 MiB
/// Its size when a heap is made, until the first collection: a world
/// that never fills it (a small request) holds no more, one that does
/// gets the largest. Above `LARGE_WORDS`, so that every small object fits.
const NURSERY_START: usize = 1 << 15; // 256 KiB

/// Largest nursery size in words: `TECHNE_NURSERY_KB` or `NURSERY_WORDS`.
fn nursery_words() -> usize {
    std::env::var("TECHNE_NURSERY_KB").ok().and_then(|v| v.parse::<usize>().ok()).map_or(NURSERY_WORDS, |kb| (kb * 128).max(1 << 12))
}

/// Objects at least this large are allocated directly in the old generation.
pub const LARGE_WORDS: usize = 1 << 14;
/// Old-generation size (words) at which the first cycle starts.
const INITIAL_THRESHOLD: usize = 8 << 20; // 64 MiB
/// The largest old-generation block, and of the objects kept in blocks.
const BLOCK_WORDS: usize = 1 << 15; // 256 KiB
const MAX_SMALL: usize = BLOCK_WORDS / 8;
/// A class's first block: they double from there up to `BLOCK_WORDS`, so
/// that a world using a few objects of many classes (copied from a
/// template) does not hold a large block of each.
const FIRST_BLOCK: usize = 1 << 12; // 32 KiB
/// Words marked per slice at least, and per promoted word.
const MARK_SLICE: usize = 1 << 17;
/// An object of more fields than this is scanned in parts, across slices
/// if a slice's budget ends within it (`Heap::partial`).
const SCAN_PART: usize = 1 << 12;
const MARK_PER_PROMOTED: usize = 1;
/// Blocks swept per slice at least.
const SWEEP_SLICE: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Pair = 1,
    Vector = 2,
    /// Fields: code id (fixnum index into `Vm::codes`), then captured values.
    Closure = 3,
    Box = 4,
    /// Fields: count (fixnum), slot vector (key/value pairs).
    Table = 5,
    /// Fields: record type, then the record's fields.
    Record = 6,
    /// Record type descriptor. Fields: name (symbol), field names (list), id.
    Rtd = 7,
    /// Key/value pairs held as ephemerons: a value is kept only while its
    /// key is reachable other than through the pair; entries whose key dies
    /// are cleared (both set to `UNDEFINED`). The slots of weak hash tables.
    Ephemerons = 8,
    /// An exact non-integer. Fields: numerator, denominator (integers, in
    /// lowest terms, the denominator above 1).
    Ratio = 9,
    /// A non-real number. Fields: real and imaginary part (real numbers;
    /// the imaginary part is not an exact zero).
    Complex = 10,
    /// A code object's handle (`Code::handle`): what the code keeps alive,
    /// its constants and the handles of the codes it reaches. A closure
    /// keeps its code's handle alive; a code whose handle dies is freed.
    Code = 11,
    /// A persistent dict (`crate::dict`). Fields: entry count, root node.
    Dict = 12,
    /// A persistent set. Fields: entry count, root node.
    Set = 13,
    /// A node of a dict's or set's trie (`crate::dict`).
    Node = 14,
    String = 16,
    /// Fields: the magnitude's 64-bit limbs; the sign is `NEGATIVE`.
    BigInt = 17,
    /// A Rust value owned by the VM's foreign table; the payload is the index.
    Foreign = 18,
    /// Bytes, the length in bytes.
    Bytevector = 19,
}

impl Kind {
    /// The kind a header's kind byte names.
    pub(crate) fn of(byte: u8) -> Option<Kind> {
        use Kind::*;
        [
            Pair, Vector, Closure, Box, Table, Record, Rtd, Ephemerons, Ratio, Complex, Code, Dict, Set, Node, String, BigInt, Foreign,
            Bytevector,
        ]
        .into_iter()
        .find(|k| *k as u8 == byte)
    }
}

const FORWARDED: u64 = 0xFF;
const FREE: u64 = 0xFE;
const REMEMBERED: u64 = 1 << 8;
pub const ASCII: u64 = 1 << 9;
const MARKED: u64 = 1 << 10;
/// A bignum's sign.
pub const NEGATIVE: u64 = 1 << 11;
/// On strings, this kind-specific bit records reserved index storage. It
/// is a layout property, independent of the mutable ASCII flag.
pub(crate) const STRING_INDEXED: u64 = NEGATIVE;
/// A nursery object whose address is its identity hash: promotion keeps the
/// address in a word after the object's fields.
const HASHED: u64 = 1 << 12;
/// An old object with its identity hash in the word after its fields.
const HASH_STORED: u64 = 1 << 13;
/// A literal (a quoted pair, vector, string or bytevector in code), which
/// mutation refuses.
pub const IMMUTABLE: u64 = 1 << 14;
/// A string whose characters changed size: its one field is another
/// string holding its bytes now. The object keeps its identity.
pub const INDIRECT: u64 = 1 << 15;
const KIND_MASK: u64 = 0xFF;

#[inline(always)]
pub fn header(kind: Kind, len: usize, flags: u64) -> u64 {
    ((len as u64) << 16) | flags | kind as u64
}
/// The length in header `h`, bounded by the cage's size: no object is
/// longer. An object's length is read through `len_of`, which checks that
/// what it claims ends in the cage, as the collector's scans do; bytes are
/// taken from ranges checked so (`bytes_at`).
#[inline(always)]
pub fn header_len(h: u64) -> usize {
    (h >> 16) as usize & crate::alloc::CAGE_MASK
}
#[inline(always)]
pub fn header_kind(h: u64) -> u8 {
    (h & KIND_MASK) as u8
}

/// Size in words of the header and fields.
#[inline(always)]
fn base_words(h: u64) -> usize {
    match header_kind(h) {
        k if k < Kind::String as u8 => 1 + header_len(h),
        _ if h & INDIRECT != 0 => 1 + header_len(h),
        k if k == Kind::String as u8 => string_words(header_len(h), h & STRING_INDEXED == 0),
        k if k == Kind::Bytevector as u8 => bytevector_words(header_len(h)),
        k if k == Kind::BigInt as u8 => 1 + header_len(h),
        _ => 2, // Foreign
    }
}

/// Size in words, header and stored identity hash included.
#[inline(always)]
fn object_words(h: u64) -> usize {
    base_words(h) + (h & HASH_STORED != 0) as usize
}
#[inline(always)]
fn is_traced(h: u64) -> bool {
    header_kind(h) < Kind::String as u8 || h & INDIRECT != 0
}

/// A byte offset every 64 characters bounds indexing work independently
/// of string length. Reserve by byte length (an upper bound on characters)
/// so construction needs no preliminary character count. ASCII strings
/// need no index; non-ASCII strings below 64 bytes need only bounded
/// decoding. The trailer of a longer non-ASCII string is raw data copied
/// by GC, not a side cache retaining the string.
pub(crate) const STRING_INDEX_STRIDE: usize = 64;

pub fn bytevector_words(len: usize) -> usize {
    1 + len.div_ceil(8)
}

pub fn string_words(len: usize, ascii: bool) -> usize {
    bytevector_words(len) + if ascii || len < STRING_INDEX_STRIDE { 0 } else { 1 + len / STRING_INDEX_STRIDE }
}

/// Developer ablation, fixed for the process. Read once per minor, never
/// per field; do not change Heap/Vm layout just to select a scanner.
fn field_scan_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("TECHNE_GC_FIELD_SCAN").as_deref() != Ok("0"))
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
    mem: Vec<u64, Caged>,
    class: u8,
    /// Found empty, to be released (`OldSpace::empty`).
    empty: bool,
}

/// The old generation's memory.
struct OldSpace {
    /// Where blocks and large objects come from.
    alloc: Caged,
    /// By index; a released block's memory is empty, and its index vacant.
    blocks: Vec<Block>,
    vacant: Vec<u32>,
    /// Blocks found empty by sweeping, not released yet.
    empty: Vec<u32>,
    /// Free list head per class (null when empty).
    free: Vec<*mut u64>,
    /// The size of the next block made per class (`FIRST_BLOCK`, doubling).
    next_block: Vec<usize>,
    /// Unused rest of the newest block per class: (cursor, end). Slots there
    /// are still zero, which sweeping treats as free.
    fresh: Vec<(*mut u64, *mut u64)>,
    /// Blocks per class not swept since the last mark.
    unswept: Vec<Vec<u32>>,
    large: Vec<Vec<u64, Caged>>,
    /// Words in allocated slots and large objects.
    words: usize,
    /// Words of the blocks held, and of the large objects.
    held: usize,
    /// `classes()`, cached for the allocation fast path.
    class_words: &'static [usize],
    class_of: &'static [u8],
}

impl OldSpace {
    fn new(alloc: Caged) -> OldSpace {
        let n = classes().words.len();
        let null = std::ptr::null_mut();
        OldSpace {
            blocks: Vec::new(),
            vacant: Vec::new(),
            empty: Vec::new(),
            free: vec![null; n],
            fresh: vec![(null, null); n],
            next_block: classes().words.iter().map(|&w| (8 * w).clamp(FIRST_BLOCK, BLOCK_WORDS)).collect(),
            unswept: vec![Vec::new(); n],
            large: Vec::new(),
            words: 0,
            held: 0,
            alloc,
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
                // The last slot's link is null; others are cage addresses.
                let next = unsafe { *p.add(1) } as usize;
                self.free[c] = if next == 0 { std::ptr::null_mut() } else { near(p, next) };
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
        let _charged = self.alloc.budget().enter();
        if words > MAX_SMALL {
            let mut mem = zeroed_words(words, self.alloc.clone());
            let p = mem.as_mut_ptr();
            self.large.push(mem);
            self.words += words;
            self.held += words;
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
            *p.add(1) = self.free[c].addr() as u64;
        }
        self.free[c] = p;
    }

    fn new_block(&mut self, c: usize) {
        let size = classes().words[c];
        let words = self.next_block[c];
        self.next_block[c] = (2 * words).min(BLOCK_WORDS);
        let mut mem = zeroed_words(words, self.alloc.clone());
        let base = mem.as_mut_ptr();
        let block = Block { mem, class: c as u8, empty: false };
        match self.vacant.pop() {
            Some(i) => self.blocks[i as usize] = block,
            None => self.blocks.push(block),
        }
        self.held += words;
        self.fresh[c] = (base, unsafe { base.add(words / size * size) });
    }

    /// Free the unmarked objects of block `b` and unmark the others; set the
    /// block aside to be released if none is marked.
    fn sweep_block(&mut self, b: usize) {
        let c = self.blocks[b].class as usize;
        let size = classes().words[c];
        let base = self.blocks[b].mem.as_mut_ptr();
        let slots = self.blocks[b].mem.len() / size;
        let slot = |i: usize| unsafe { *base.add(i * size) };
        if (0..slots).all(|i| slot(i) & MARKED == 0 || header_kind(slot(i)) == FREE as u8) {
            let dead = (0..slots).filter(|&i| slot(i) != 0 && header_kind(slot(i)) != FREE as u8).count();
            self.words -= dead * size;
            self.blocks[b].empty = true;
            self.empty.push(b as u32);
            return;
        }
        for i in 0..slots {
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
        for (i, b) in self.blocks.iter().enumerate().filter(|(_, b)| !b.mem.is_empty() && !b.empty) {
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
        self.held -= freed;
    }

    /// Release the blocks found empty.
    fn release_empty(&mut self) {
        for b in std::mem::take(&mut self.empty) {
            let block = &mut self.blocks[b as usize];
            self.held -= block.mem.len();
            block.mem = Vec::new_in(self.alloc.clone());
            block.empty = false;
            self.vacant.push(b);
        }
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
    /// Most nursery words one minor collection copied (bounded by the
    /// nursery window).
    pub max_copied_words: usize,
    /// Ephemeron entries examined while resolving live keys and values.
    pub ephemeron_entries: u64,
    /// Most such entries examined in one collection (minor and old-generation work).
    pub max_ephemeron_entries: u64,
}

pub struct Heap {
    nursery: Vec<u64, Caged>,
    /// The size the nursery grows to, in words (`nursery_words`).
    nursery_max: usize,
    /// The window asked for (`set_nursery_window`), in words.
    window: usize,
    nursery_start: *mut u64,
    nursery_end: *mut u64,
    top: *mut u64,
    /// Allocation stops here: the end of the nursery window (see
    /// `set_nursery_window`).
    limit: *mut u64,
    old: OldSpace,
    remembered: Vec<*mut u64>,
    /// Promoted objects whose fields still need scanning (minor collection).
    scan: Vec<*mut u64>,
    /// Words promoted by the minor collection in progress. Not the growth of
    /// the old generation: promotion can sweep blocks and free dead words.
    promoted: usize,
    /// Gray objects: marked, fields not yet shaded.
    gray: Vec<*mut u64>,
    /// A large object being scanned, and its next field: the rest is
    /// scanned by the next slice (`mark`). Its fields written meanwhile
    /// are shaded by the barrier, as any are while marking.
    partial: Option<(*mut u64, usize)>,
    /// During the remark: the values of ephemeron entries whose key is not
    /// marked, by key, released (`released`) when the key is marked, so
    /// that a chain of them takes one pass, not one per link.
    waiting: rustc_hash::FxHashMap<usize, Vec<Value>>,
    released: Vec<Value>,
    /// The remark is under way: ephemeron objects marked enter `waiting`.
    remarking: bool,
    /// In a minor collection's ephemeron pass: the entries (object, key's
    /// field) whose key is in the nursery, not copied yet, by the key's
    /// address, released (`copied`) when it is copied.
    waiting_copy: rustc_hash::FxHashMap<usize, Vec<(*mut u64, usize)>>,
    copied: Vec<(*mut u64, usize)>,
    phase: Phase,
    /// `phase == Marking`, for the barrier's fast path.
    marking: bool,
    marked_words: usize,
    /// Words scanned by the marker in the current pause.
    slice_words: usize,
    /// Old-generation size at which the next cycle starts.
    threshold: usize,
    /// Addresses of live foreign objects, to detect their death: those of
    /// the old generation, and those of the nursery, which a minor
    /// collection sweeps alone, so that its cost is the young ones' and
    /// not every foreign object's (the editor keeps thousands alive).
    foreign: Vec<*mut u64>,
    young_foreign: Vec<*mut u64>,
    /// Foreign-table indices whose objects died in the last collection.
    pub(crate) dead_foreign: Vec<usize>,
    /// Ephemeron objects met by the minor collection in progress.
    weak_minor: Vec<*mut u64>,
    /// Ephemeron objects marked in the cycle in progress.
    weak_marked: Vec<*mut u64>,
    /// Handle objects of the VM's codes, by code id (`Vm::install_code`
    /// keeps it in step with `Vm::codes`): a closure's first field is its
    /// code's id, and shading a closure shades its code's handle, which
    /// keeps the code. Handles are old objects that do not move, so a
    /// plain copy here stays right. `Value::VOID`: no code (freed).
    pub(crate) code_handles: Vec<Value>,
    /// `TECHNE_GC_STRESS`: collect on every allocation, with a cycle always in
    /// progress and tiny slices; `=full` completes a whole cycle every time.
    pub stress: bool,
    pub stress_full: bool,
    /// `TECHNE_GC_VERIFY`: check the heap before and after every
    /// collection (`verify`).
    pub verify: bool,
    pub stats: GcStats,
    /// Developer-only census; no guest roots or per-allocation JIT probes.
    #[cfg(feature = "allocation-census")]
    census: Option<Box<census::Census>>,
}

/// Visitor over root slots, supplied by the VM.
pub(crate) trait Roots {
    fn visit(&mut self, f: &mut dyn FnMut(&mut Value));
    /// `visit`, naming each slot, for reports (`TECHNE_GC_VERIFY`).
    fn visit_named(&mut self, f: &mut dyn FnMut(&dyn std::fmt::Display, &mut Value)) {
        self.visit(&mut |v| f(&"slot", v));
    }
    /// Slots that are not roots of the old generation but may hold nursery
    /// pointers, which minor collections update: values kept only while
    /// something live refers to them (`retained`).
    fn visit_unrooted(&mut self, _f: &mut dyn FnMut(&mut Value)) {}
    /// At the end of marking, while `marked` tells what is reachable: the
    /// values of unrooted slots that marked objects make reachable, shaded
    /// by `shade`. Called until nothing more is marked.
    fn retained(&mut self, _marked: &dyn Fn(Value) -> bool, _shade: &mut dyn FnMut(Value)) {}
    /// After marking, before sweeping: release what refers to unmarked
    /// objects that are about to be freed.
    fn reclaim(&mut self, _marked: &dyn Fn(Value) -> bool) {}
}

/// Whether the old object `v` is marked (when marking has finished: every
/// reachable old object is), or `v` is not a heap object.
pub(crate) fn marked(v: Value) -> bool {
    !v.is_ptr() || unsafe { *v.as_ptr() & MARKED != 0 }
}

impl Heap {
    /// Construct an isolated cage and nursery under `budget`, initializing
    /// process-level scanner configuration before any collection can run.
    pub(crate) fn new_in(budget: Budget) -> Heap {
        // Initialize outside collection: a minor only reads the ready flag.
        let _ = field_scan_enabled();
        #[cfg(feature = "allocation-census")]
        let census = if std::env::var("TECHNE_ALLOC_CENSUS").as_deref() == Ok("1") { census::Census::new(&budget) } else { None };
        let nursery_max = nursery_words();
        let words = NURSERY_START.min(nursery_max);
        // A cage of its own: no value of another world names its memory.
        let alloc = Caged::new(budget, crate::alloc::Cage::new());
        let mut nursery = try_zeroed_words(words, alloc.clone()).expect("room in a new cage for a nursery");
        let start = nursery.as_mut_ptr();
        let stress = std::env::var_os("TECHNE_GC_STRESS").is_some();
        Heap {
            nursery_start: start,
            nursery_end: unsafe { start.add(words) },
            top: start,
            limit: unsafe { start.add(words) },
            nursery,
            nursery_max,
            window: usize::MAX,
            old: OldSpace::new(alloc),
            remembered: Vec::new(),
            scan: Vec::new(),
            promoted: 0,
            gray: Vec::new(),
            partial: None,
            waiting: Default::default(),
            released: Vec::new(),
            remarking: false,
            waiting_copy: Default::default(),
            copied: Vec::new(),
            phase: Phase::Idle,
            marking: false,
            marked_words: 0,
            slice_words: 0,
            threshold: INITIAL_THRESHOLD,
            foreign: Vec::new(),
            young_foreign: Vec::new(),
            dead_foreign: Vec::new(),
            weak_minor: Vec::new(),
            weak_marked: Vec::new(),
            code_handles: Vec::new(),
            stress,
            stress_full: std::env::var("TECHNE_GC_STRESS").is_ok_and(|v| v == "full"),
            verify: std::env::var_os("TECHNE_GC_VERIFY").is_some(),
            stats: GcStats::default(),
            #[cfg(feature = "allocation-census")]
            census,
        }
    }

    /// The base of the cage this heap is in (`JitCtx::cage`).
    pub(crate) fn cage_base(&self) -> *mut u8 {
        self.old.alloc.cage().base()
    }

    /// Its cage, which the VM enters wherever it is entered.
    /// What its memory is charged to and bounded by.
    pub(crate) fn budget(&self) -> &Budget {
        self.old.alloc.budget()
    }

    /// The bytes the nursery holds: the most a minor collection copies.
    pub(crate) fn nursery_used(&self) -> usize {
        self.top as usize - self.nursery_start as usize
    }

    pub(crate) fn cage(&self) -> &std::rc::Rc<crate::alloc::Cage> {
        self.old.alloc.cage()
    }

    #[inline(always)]
    pub(crate) fn in_nursery(&self, p: *mut u64) -> bool {
        p >= self.nursery_start && p < self.nursery_end
    }

    /// True if `words` can be bump-allocated without collecting.
    #[inline(always)]
    pub fn has_room(&self, words: usize) -> bool {
        !self.stress && (self.limit as usize).saturating_sub(self.top as usize) / 8 >= words
    }

    /// Make the next allocation in the nursery collect, compiled code's
    /// too (`Vm::collect`): for what the heap's size does not show.
    pub(crate) fn request_collection(&mut self) {
        self.limit = self.top;
    }

    /// The addresses of the bump pointer and of the allocation limit, for
    /// allocation inlined in JIT code, which reads both anew each time (a
    /// collection may replace the nursery); null under GC stress, where
    /// every allocation collects.
    pub(crate) fn bump_pointers(&mut self) -> (*mut *mut u64, *const *mut u64) {
        if self.stress { (std::ptr::null_mut(), std::ptr::null()) } else { (&mut self.top, &self.limit) }
    }

    /// Bump-allocate in the nursery. Caller guarantees `has_room(words)` or has
    /// just collected with the nursery large enough.
    #[inline(always)]
    pub(crate) fn bump(&mut self, words: usize) -> *mut u64 {
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
    pub(crate) fn alloc_old(&mut self, words: usize) -> *mut u64 {
        let p = self.old.alloc(words);
        #[cfg(feature = "allocation-census")]
        if let Some(census) = &mut self.census {
            census.old(words);
        }
        self.remembered.push(p);
        p
    }

    /// `alloc_old` for an object whose fields will be `fields`: remembered
    /// only if one of them is in the nursery (or while marking).
    pub(crate) fn alloc_old_holding(&mut self, words: usize, fields: &[Value]) -> *mut u64 {
        if fields.iter().any(|v| v.is_ptr() && self.in_nursery(v.as_ptr_near(self.nursery_start))) {
            self.alloc_old(words)
        } else {
            self.alloc_old_unremembered(words)
        }
    }

    /// Old-generation allocation for objects only referencing old/immediate
    /// values (compile-time constants).
    pub(crate) fn alloc_old_unremembered(&mut self, words: usize) -> *mut u64 {
        if self.marking {
            // Marked when the next minor collection scans it.
            return self.alloc_old(words);
        }
        #[cfg(feature = "allocation-census")]
        {
            let p = self.old.alloc(words);
            if let Some(census) = &mut self.census {
                census.old(words);
            }
            p
        }
        #[cfg(not(feature = "allocation-census"))]
        self.old.alloc(words)
    }

    /// The identity hash of a heap object: its address, which an old object
    /// keeps, and a nursery object keeps through promotion.
    pub(crate) fn identity_hash(&mut self, obj: *mut u64) -> u64 {
        unsafe {
            let h = *obj;
            if h & HASH_STORED != 0 {
                return *word(obj, base_words(h));
            }
            if self.in_nursery(obj) {
                *obj = h | HASHED;
            }
            obj.addr() as u64
        }
    }

    pub(crate) fn register_foreign(&mut self, obj: *mut u64) {
        if self.in_nursery(obj) {
            self.young_foreign.push(obj);
        } else {
            self.foreign.push(obj);
        }
    }

    /// After a minor collection: follow promoted foreign objects into the
    /// old generation's and record dead ones. Only the nursery's are looked
    /// at.
    fn sweep_foreign_nursery(&mut self) {
        for p in std::mem::take(&mut self.young_foreign) {
            let h = unsafe { *p };
            if header_kind(h) == FORWARDED as u8 {
                self.foreign.push(near(p, (h >> 16) as usize));
            } else {
                self.dead_foreign.push(unsafe { *word(p, 1) } as usize);
            }
        }
    }

    /// At the end of marking: old foreign objects left unmarked are dead.
    fn sweep_foreign_old(&mut self) {
        let mut live = Vec::with_capacity(self.foreign.len());
        for &p in &self.foreign {
            if unsafe { *p } & MARKED != 0 {
                live.push(p);
            } else {
                self.dead_foreign.push(unsafe { *word(p, 1) } as usize);
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

    /// Bytes of memory held: the nursery, the old blocks (used or not) and
    /// the large objects.
    pub fn committed(&self) -> usize {
        (self.nursery_capacity() + self.old.held) * 8
    }

    /// Release the blocks sweeping found empty: when nothing refers to their
    /// objects any more, not even a list of dead ones (the VM's codes).
    pub(crate) fn release_empty(&mut self) {
        self.old.release_empty();
    }

    #[inline(always)]
    pub(crate) fn barrier(&mut self, obj: *mut u64, v: Value) {
        if !v.is_ptr() {
            return;
        }
        let p = v.as_ptr_near(self.nursery_start);
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
                if !self.waiting.is_empty()
                    && let Some(values) = self.waiting.remove(&(p as usize))
                {
                    self.released.extend(values);
                }
            }
        }
    }

    fn shade_value(&mut self, v: Value) {
        let p = v.as_ptr_near(self.nursery_start);
        if v.is_ptr() && !self.in_nursery(p) {
            self.shade(p);
        }
    }

    /// A minor collection, followed by a slice of old-generation work.
    pub(crate) fn collect(&mut self, roots: &mut dyn Roots) {
        if self.verify {
            self.verify(roots, "before a collection");
        }
        let start = Instant::now();
        let ephemeron_entries = self.stats.ephemeron_entries;
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
        self.stats.max_ephemeron_entries = self.stats.max_ephemeron_entries.max(self.stats.ephemeron_entries - ephemeron_entries);
        if self.verify {
            self.verify(roots, "after a collection");
        }
    }

    /// Complete a whole old-generation cycle now (after a minor collection).
    pub(crate) fn full_collect(&mut self, roots: &mut dyn Roots) {
        if self.verify {
            self.verify(roots, "before a full collection");
        }
        let start = Instant::now();
        let ephemeron_entries = self.stats.ephemeron_entries;
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
        self.stats.max_ephemeron_entries = self.stats.max_ephemeron_entries.max(self.stats.ephemeron_entries - ephemeron_entries);
        if self.verify {
            self.verify(roots, "after a full collection");
        }
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
        self.weak_marked.clear();
        roots.visit(&mut |v| self.shade_value(*v));
    }

    /// Shade the fields of gray objects until about `budget` words are
    /// scanned; returns whether no gray object is left. An object larger
    /// than `SCAN_PART` is scanned a part at a time, so a slice ends within
    /// its budget however large the objects it meets.
    fn mark(&mut self, budget: usize) -> bool {
        let mut done = 0;
        while done < budget {
            while let Some(v) = self.released.pop() {
                self.shade_value(v);
            }
            let (obj, from) = match self.partial.take() {
                Some(part) => part,
                None => match self.gray.pop() {
                    Some(obj) => (obj, 1),
                    None => {
                        self.slice_words += done;
                        return true;
                    }
                },
            };
            unsafe {
                let n = len_of(obj);
                if header_kind(*obj) == Kind::Ephemerons as u8 {
                    // Values of live keys now; the rest at the remark.
                    self.weak_marked.push(obj);
                    if self.remarking {
                        self.keep_or_wait(obj);
                    } else {
                        self.shade_live_values(obj);
                    }
                    done += n + 1;
                    continue;
                }
                if from == 1 {
                    self.shade_code(obj);
                }
                let to = if n + 1 - from > SCAN_PART { from.saturating_add(SCAN_PART.max(budget - done)).min(n + 1) } else { n + 1 };
                for i in from..to {
                    self.shade_value(Value::from_bits(*obj.add(i)));
                }
                if to <= n {
                    self.partial = Some((obj, to));
                }
                done += to - from + usize::from(from == 1);
            }
        }
        self.slice_words += done;
        self.gray.is_empty() && self.partial.is_none() && self.released.is_empty()
    }

    /// Rescan the roots (they have no barrier), finish marking, release dead
    /// foreign objects and start sweeping.
    fn remark(&mut self, roots: &mut dyn Roots) {
        roots.visit(&mut |v| self.shade_value(*v));
        self.mark(usize::MAX);
        // Ephemerons: a value lives if its key does, which marking a value
        // can cause, and the same for the slots the VM keeps only while
        // something refers to them; so repeat until nothing more is marked.
        // Then clear the entries whose key is dead.
        // The entries of unmarked keys wait for their key (`waiting`), also
        // those of ephemeron objects marked from here: marking finds a
        // chain of them in one pass.
        self.remarking = true;
        for i in 0..self.weak_marked.len() {
            self.keep_or_wait(self.weak_marked[i]);
        }
        let mut found = Vec::new();
        loop {
            let before = self.marked_words;
            roots.retained(&marked, &mut |v| found.push(v));
            for v in found.drain(..) {
                self.shade_value(v);
            }
            self.mark(usize::MAX);
            if self.marked_words == before {
                break;
            }
        }
        self.remarking = false;
        self.waiting.clear();
        for obj in std::mem::take(&mut self.weak_marked) {
            unsafe { clear_entries(obj, |k| k.is_ptr() && *k.as_ptr_near(obj) & MARKED == 0) };
        }
        roots.reclaim(&marked);
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
        #[cfg(feature = "allocation-census")]
        self.count_nursery();
        self.promoted = 0;
        unsafe {
            roots.visit(&mut |v| *v = self.evacuate(*v));
            roots.visit_unrooted(&mut |v| *v = self.evacuate(*v));
            if field_scan_enabled() {
                self.scan_minor::<true>();
            } else {
                self.scan_minor::<false>();
            }
        }
        self.sweep_foreign_nursery();
        if self.verify {
            self.poison_nursery((self.top as usize - self.nursery_start as usize) / 8);
        }
        self.top = self.nursery_start;
        // A collection asked for (`request_collection`) is done.
        self.limit = unsafe { self.nursery_start.add(self.window.min(self.nursery_capacity())) };
        let promoted = self.promoted;
        self.stats.max_copied_words = self.stats.max_copied_words.max(promoted);
        promoted
    }

    /// Allocate at most `words` in the nursery between minor collections
    /// (up to its capacity). A minor collection copies at most that much,
    /// so this bounds its pause; smaller windows cost throughput on
    /// allocation-heavy code. Takes effect from the next collection.
    pub(crate) fn set_nursery_window(&mut self, words: usize) {
        self.window = words.max(1 << 12);
        self.limit = unsafe { self.nursery_start.add(self.window.min(self.nursery_capacity())) };
    }

    /// The bytes `grow_nursery` would add: none at its largest.
    pub(crate) fn nursery_growth(&self) -> usize {
        let words = self.nursery_capacity();
        (self.nursery_max - words.min(self.nursery_max)) * 8
    }

    /// After a collection, the nursery empty: make it its largest (the VM
    /// admits it, `Vm::collect_with`). A world that never fills its first
    /// one keeps it small (its cage's room and its memory account are what
    /// it reserves). Growing by steps instead costs a busy world more
    /// collections, each rescanning the large old objects it remembers
    /// (hash's tables: 7% more instructions doubling from 256 KiB).
    pub(crate) fn grow_nursery(&mut self) {
        debug_assert_eq!(self.top, self.nursery_start, "the nursery is empty");
        let words = self.nursery_capacity();
        if words >= self.nursery_max {
            return;
        }
        let words = self.nursery_max;
        let _charged = self.old.alloc.budget().enter();
        // Without room, the small nursery serves on.
        let Some(nursery) = try_zeroed_words(words, self.old.alloc.clone()) else { return };
        self.nursery = nursery;
        let start = self.nursery.as_mut_ptr();
        self.nursery_start = start;
        // SAFETY: within the new nursery.
        unsafe {
            self.nursery_end = start.add(words);
            self.limit = start.add(words.min(self.window));
        }
        self.top = start;
    }

    pub fn nursery_window(&self) -> usize {
        (self.limit as usize - self.nursery_start as usize) / 8
    }

    /// Copy the nursery object `v` points to into the old generation unless
    /// it is old or already copied.
    ///
    /// # Safety
    /// `v` must be an initialized value belonging to this heap. Collection
    /// must be exclusive, with valid object layouts and forwarding state.
    #[inline(always)]
    unsafe fn evacuate(&mut self, v: Value) -> Value {
        if !v.is_ptr() {
            return v;
        }
        let p = v.as_ptr_near(self.nursery_start);
        if !self.in_nursery(p) {
            return v;
        }
        unsafe {
            let h = *p;
            if header_kind(h) == FORWARDED as u8 {
                return Value::ptr(near(p, (h >> 16) as usize));
            }
            let words = base_words(h);
            let hashed = h & HASHED != 0;
            // A corrupt header's object runs past the nursery: it is no
            // object, the heap is corrupt.
            if words > (self.nursery_end.addr() - p.addr()) / 8 {
                corrupt("an object of the nursery runs past it");
            }
            let dst = self.old.alloc(words + hashed as usize);
            #[cfg(feature = "allocation-census")]
            if let Some(census) = &mut self.census {
                census.promoted(h, words + hashed as usize);
            }
            self.promoted += words + hashed as usize;
            // A corrupt free list may have placed `dst` over `p`: a copy that
            // allows that.
            std::ptr::copy(p, dst, words);
            if hashed {
                *dst.add(words) = p.addr() as u64;
            }
            // Promoted while marking: marked, and its fields shaded below.
            let flags = if hashed { HASH_STORED } else { 0 } | if self.marking { MARKED } else { 0 };
            *dst = (h & !(REMEMBERED | MARKED | HASHED)) | flags;
            if self.marking {
                self.marked_words += words + hashed as usize;
            }
            *p = ((dst.addr() as u64) << 16) | FORWARDED;
            if is_traced(h) {
                self.scan.push(dst);
            }
            if !self.waiting_copy.is_empty()
                && let Some(entries) = self.waiting_copy.remove(&p.addr())
            {
                self.copied.extend(entries);
            }
            Value::ptr(dst)
        }
    }

    /// Select the scanner before walking objects, keeping configuration
    /// tests out of the field loop. Both variants keep the same visit order.
    ///
    /// # Safety
    /// Roots must already have been evacuated. Remembered, promoted and weak
    /// queues must name live objects in this heap, with the mutator stopped.
    unsafe fn scan_minor<const FAST: bool>(&mut self) {
        unsafe {
            if self.marking {
                self.scan_promoted::<true, FAST>();
            } else {
                self.scan_promoted::<false, FAST>();
            }
            self.minor_ephemerons::<FAST>();
        }
    }

    /// Scan remembered and promoted objects: evacuate the nursery objects
    /// they point to and, while marking, shade the old ones.
    ///
    /// # Safety
    /// Queued objects must be initialized and writable in this heap. The
    /// mutator must be stopped and `MARKING` must match the collector phase.
    unsafe fn scan_promoted<const MARKING: bool, const FAST: bool>(&mut self) {
        unsafe {
            for obj in std::mem::take(&mut self.remembered) {
                // Remembered objects are old (or freshly allocated old, which
                // while marking must be marked themselves).
                *obj &= !REMEMBERED;
                if MARKING {
                    self.shade(obj);
                }
                self.scan_object::<MARKING, FAST>(obj);
            }
            while let Some(obj) = self.scan.pop() {
                self.scan_object::<MARKING, FAST>(obj);
            }
        }
    }

    /// After a minor collection's scan: evacuate the values of ephemerons
    /// whose key survived (old, or copied), until no more are found, then
    /// clear the entries whose key stayed behind in the nursery.
    ///
    /// # Safety
    /// Strong scanning must be complete; weak queues must contain valid
    /// ephemeron objects and entries from this minor collection. No mutator
    /// may change fields or forwarding state during the weak fixed point.
    unsafe fn minor_ephemerons<const FAST: bool>(&mut self) {
        unsafe {
            // Each entry once: its value is kept if its key survived;
            // otherwise it waits for the key to be copied (`evacuate`), as
            // keeping one value may keep the key of another. A chain of
            // them takes one pass, not one per link. Copying values finds
            // more ephemeron objects (`scan_object`), whose entries come
            // in turn.
            let mut next = 0;
            loop {
                if let Some(&obj) = self.weak_minor.get(next) {
                    next += 1;
                    self.stats.ephemeron_entries += len_of(obj) as u64 / 2;
                    for i in (1..=len_of(obj)).step_by(2) {
                        let key = Value::from_bits(*word(obj, i));
                        if self.surviving(key).is_some() {
                            self.copied.push((obj, i));
                        } else {
                            self.waiting_copy.entry(key.as_ptr_near(self.nursery_start).addr()).or_default().push((obj, i));
                        }
                    }
                } else if let Some((obj, i)) = self.copied.pop() {
                    let key = self.surviving(Value::from_bits(*word(obj, i))).expect("a surviving key");
                    *word(obj, i) = key.bits();
                    let v = Value::from_bits(*word(obj, i + 1));
                    *word(obj, i + 1) = self.evacuate(v).bits();
                } else if let Some(obj) = self.scan.pop() {
                    if self.marking { self.scan_object::<true, FAST>(obj) } else { self.scan_object::<false, FAST>(obj) }
                } else {
                    break;
                }
            }
            self.waiting_copy.clear();
            for obj in std::mem::take(&mut self.weak_minor) {
                let nursery = (self.nursery_start, self.nursery_end);
                clear_entries(obj, |k| {
                    k.is_ptr() && (nursery.0..nursery.1).contains(&k.as_ptr()) && header_kind(*k.as_ptr()) != FORWARDED as u8
                });
            }
        }
    }

    /// Where `v` is after this minor collection, if it survives it.
    unsafe fn surviving(&self, v: Value) -> Option<Value> {
        let p = v.as_ptr_near(self.nursery_start);
        if !v.is_ptr() || !self.in_nursery(p) {
            return Some(v);
        }
        let h = unsafe { *p };
        (header_kind(h) == FORWARDED as u8).then(|| Value::ptr(near(p, (h >> 16) as usize)))
    }

    /// A closure's code is kept by its handle: the closure's first field
    /// is the code's id, and `code_handles` has the handle for it
    /// (`Kind::Closure`, `code_slot`).
    #[inline(always)]
    fn shade_code(&mut self, obj: *mut u64) {
        unsafe {
            if header_kind(*obj) == Kind::Closure as u8 {
                let id = code_slot(Value::from_bits(*word(obj, 1)));
                if let Some(&handle) = self.code_handles.get(id) {
                    self.shade_value(handle);
                }
            }
        }
    }

    /// Shade the values of ephemeron object `obj`'s entries whose key is
    /// marked, and have the others wait for their key (`waiting`), in one
    /// pass: a key that shading a value marks is then either seen marked
    /// when its entry comes, or releases its entry waiting already.
    fn keep_or_wait(&mut self, obj: *mut u64) {
        unsafe {
            self.stats.ephemeron_entries += len_of(obj) as u64 / 2;
            for i in (1..=len_of(obj)).step_by(2) {
                let key = Value::from_bits(*word(obj, i));
                let value = Value::from_bits(*word(obj, i + 1));
                if !key.is_ptr() || *key.as_ptr_near(obj) & MARKED != 0 {
                    self.shade_value(value);
                } else {
                    self.waiting.entry(key.as_ptr_near(obj) as usize).or_default().push(value);
                }
            }
        }
    }

    /// Shade the values of an ephemeron object's entries whose key is marked
    /// (or not a heap object).
    fn shade_live_values(&mut self, obj: *mut u64) {
        unsafe {
            self.stats.ephemeron_entries += len_of(obj) as u64 / 2;
            for i in (1..=len_of(obj)).step_by(2) {
                let key = Value::from_bits(*word(obj, i));
                if !key.is_ptr() || *key.as_ptr_near(obj) & MARKED != 0 {
                    self.shade_value(Value::from_bits(*word(obj, i + 1)));
                }
            }
        }
    }

    /// Relocate a pointer field and shade its referent when marking. The
    /// fast scanner leaves immediate bits untouched: they cannot move or shade.
    ///
    /// # Safety
    /// `slot` must address a writable, initialized value field of a live
    /// object in this heap. Collection must be exclusive and `MARKING` must
    /// match the collector phase.
    #[inline(always)]
    unsafe fn scan_field<const MARKING: bool, const FAST: bool>(&mut self, slot: *mut u64) {
        unsafe {
            let v = Value::from_bits(*slot);
            // An immediate cannot move or shade anything.
            if !FAST || v.is_ptr() {
                let v = self.evacuate(v);
                *slot = v.bits();
                if MARKING {
                    self.shade_value(v);
                }
            }
        }
    }

    /// Visit traced fields in their original order, preserving closure-code
    /// roots and deferring ephemerons. Fixed pairs still pass the cage check.
    ///
    /// # Safety
    /// `obj` must name an initialized, writable live object in this heap,
    /// with valid fields for its layout. The mutator must be stopped and
    /// `MARKING` must match the collector phase.
    #[inline(always)]
    unsafe fn scan_object<const MARKING: bool, const FAST: bool>(&mut self, obj: *mut u64) {
        unsafe {
            let h = *obj;
            // Pairs dominate promotion in list workloads. Their fixed two
            // fields need neither the dynamic loop nor closure/weak dispatch.
            // Keep malformed shapes on the original generic path.
            if FAST && header_kind(h) == Kind::Pair as u8 && checked_field_len(obj, h) == 2 {
                self.scan_field::<MARKING, FAST>(obj.add(1));
                self.scan_field::<MARKING, FAST>(obj.add(2));
                return;
            }
            if header_kind(h) == Kind::Ephemerons as u8 {
                // Its entries wait for the rest of the scan.
                self.weak_minor.push(obj);
                if MARKING {
                    self.weak_marked.push(obj);
                }
                return;
            }
            if MARKING {
                self.shade_code(obj);
            }
            if is_traced(h) {
                for i in 1..=len_of(obj) {
                    self.scan_field::<MARKING, FAST>(obj.add(i));
                }
            }
        }
    }
}

/// Bits of a code id: the VM's table of codes has `CODE_SLOTS` slots
/// (`Vm::code_ptrs`). Fewer where its storage is real memory.
#[cfg(all(target_os = "linux", not(miri)))]
pub(crate) const CODE_BITS: u32 = 20;
#[cfg(not(all(target_os = "linux", not(miri))))]
pub(crate) const CODE_BITS: u32 = 16;
pub(crate) const CODE_SLOTS: usize = 1 << CODE_BITS;
pub(crate) const CODE_MASK: usize = CODE_SLOTS - 1;

/// The slot of the table of codes a closure's first field names: its low
/// bits, as all that reads one takes them (the interpreter, compiled code,
/// the collector, the verifier), so that whatever the field holds, it
/// names a slot, which holds a live code or is null; and the code a
/// closure runs is the one the collector keeps for it.
#[inline(always)]
pub(crate) fn code_slot(id: Value) -> usize {
    id.bits() as usize & CODE_MASK
}

/// Clears the entries of an ephemeron object whose key is `dead`.
unsafe fn clear_entries(obj: *mut u64, dead: impl Fn(Value) -> bool) {
    unsafe {
        for i in (1..=len_of(obj)).step_by(2) {
            if dead(Value::from_bits(*word(obj, i))) {
                *word(obj, i) = Value::UNDEFINED.bits();
                *word(obj, i + 1) = Value::UNDEFINED.bits();
            }
        }
    }
}

// ----- object access helpers -----

/// The heap is corrupt: what was found says so. The world is retired (the
/// panic reaches its boundary), the others go on.
#[cold]
#[inline(never)]
pub(crate) fn corrupt(what: &str) -> ! {
    panic!("the heap is corrupt: {what}")
}

/// The word `k` words past object `obj`, in its cage, whatever `k`: the
/// offset wraps within the cage, so no index a heap value chooses (one
/// below a length read from a header, a forged object's) leads out of it.
#[inline(always)]
pub(crate) fn word(obj: *mut u64, k: usize) -> *mut u64 {
    use crate::alloc::CAGE_MASK;
    let at = obj.addr();
    obj.with_addr((at & !CAGE_MASK) | (at.wrapping_add(k.wrapping_mul(8)) & CAGE_MASK))
}

/// `len` bytes from `at` bytes past object `obj`, which must end in its
/// cage, else the heap is corrupt: what a slice of them may be made from.
#[inline(always)]
pub(crate) fn bytes_at(obj: *mut u64, at: usize, len: usize) -> *mut u8 {
    // `at` and `len` are below 2^35 (lengths are bounded by the cage), so
    // their sum does not overflow.
    let left = crate::alloc::CAGE_MASK + 1 - (obj.addr() & crate::alloc::CAGE_MASK);
    if at + len > left {
        corrupt("an object runs past its cage");
    }
    obj.cast::<u8>().wrapping_add(at)
}

/// A string's bytes as text: checked, as a world may have changed them to
/// any (a corrupt heap, whose world is retired).
#[inline]
pub(crate) fn utf8(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap_or_else(|_| corrupt("a string's bytes are no UTF-8"))
}

#[inline(always)]
pub(crate) unsafe fn field(obj: *mut u64, i: usize) -> Value {
    unsafe { Value::from_bits(*obj.add(1 + i)) }
}
#[inline(always)]
pub(crate) unsafe fn set_field(obj: *mut u64, i: usize, v: Value) {
    unsafe { *obj.add(1 + i) = v.bits() }
}
#[inline(always)]
pub(crate) unsafe fn kind_of(obj: *mut u64) -> u8 {
    unsafe { header_kind(*obj) }
}
/// The length of object `obj` in fields (words), checked to end in its
/// cage as words, whatever its header's kind: every index below it, and a
/// few words past (the cage's tail), is in its world's memory. A header of
/// no object's that claims more is corruption. A string's or bytevector's
/// length in bytes is `bytes_len`'s.
#[inline(always)]
pub(crate) unsafe fn len_of(obj: *mut u64) -> usize {
    unsafe { checked_field_len(obj, *obj) }
}

/// The same extent check for an already loaded header. Even a fixed-size
/// pair must pass it before its raw field addresses are formed.
#[inline(always)]
fn checked_field_len(obj: *mut u64, h: u64) -> usize {
    let len = header_len(h);
    if len >= (crate::alloc::CAGE_MASK - (obj.addr() & crate::alloc::CAGE_MASK)) / 8 {
        corrupt("an object runs past its cage");
    }
    len
}

/// The length in bytes of string or bytevector `obj` holding its bytes
/// (not an indirect string), checked to end in its cage.
#[inline(always)]
pub(crate) unsafe fn bytes_len(obj: *mut u64) -> usize {
    unsafe {
        let len = header_len(*obj);
        bytes_at(obj, 8, len);
        len
    }
}

/// The object holding the bytes of a string or bytevector.
#[inline(always)]
unsafe fn bytes_obj(obj: *mut u64) -> *mut u64 {
    unsafe { if *obj & INDIRECT != 0 { field(obj, 0).as_ptr_near(obj) } else { obj } }
}
/// The bytes of a string or bytevector.
#[inline(always)]
pub(crate) unsafe fn str_bytes<'a>(obj: *mut u64) -> &'a [u8] {
    unsafe {
        let b = bytes_obj(obj);
        let len = header_len(*b);
        std::slice::from_raw_parts(bytes_at(b, 8, len), len)
    }
}
/// The bytes of a bytevector, to change.
pub(crate) unsafe fn bytes_mut<'a>(obj: *mut u64) -> &'a mut [u8] {
    unsafe {
        let len = header_len(*obj);
        std::slice::from_raw_parts_mut(bytes_at(obj, 8, len), len)
    }
}
/// Rebuild the character count and sparse index after creation or mutation.
/// The trailer follows the padded UTF-8 bytes, leaving their layout (and
/// the JIT's ASCII fast path) unchanged. All reserved words are initialized.
pub(crate) unsafe fn str_reindex(obj: *mut u64, ascii: bool) {
    unsafe {
        *obj = (*obj & !ASCII) | if ascii { ASCII } else { 0 };
        if *obj & STRING_INDEXED == 0 {
            return;
        }
        let bytes = str_bytes(obj);
        let trailer = bytevector_words(bytes.len());
        let slots = bytes.len() / STRING_INDEX_STRIDE;
        for i in 0..=slots {
            *word(obj, trailer + i) = 0;
        }
        if ascii {
            *word(obj, trailer) = bytes.len() as u64;
            return;
        }
        let mut count = 0;
        for (i, (at, _)) in utf8(bytes).char_indices().enumerate() {
            if i != 0 && i % STRING_INDEX_STRIDE == 0 {
                *word(obj, trailer + i / STRING_INDEX_STRIDE) = at as u64;
            }
            count = i + 1;
        }
        *word(obj, trailer) = count as u64;
    }
}

/// Character count, including for an indirect string, in constant time.
pub(crate) unsafe fn str_len(obj: *mut u64) -> usize {
    unsafe {
        let b = bytes_obj(obj);
        let bytes = bytes_len(b);
        if *b & ASCII != 0 {
            return bytes;
        }
        if bytes < STRING_INDEX_STRIDE {
            return utf8(str_bytes(b)).chars().count();
        }
        let count = *word(b, bytevector_words(bytes)) as usize;
        if count > bytes {
            corrupt("a string's character count exceeds its bytes");
        }
        count
    }
}

/// A bounded UTF-8 window starting at the checkpoint before character `k`.
/// Its last byte may split a character; only that incomplete suffix is
/// omitted. No whole-string UTF-8 validation happens on an indexed read.
unsafe fn str_index_window<'a>(obj: *mut u64, k: usize) -> (usize, &'a str, usize) {
    unsafe {
        let b = bytes_obj(obj);
        let bytes = str_bytes(b);
        let block = k / STRING_INDEX_STRIDE;
        let at = if block == 0 { 0 } else { *word(b, bytevector_words(bytes.len()) + block) as usize };
        let end = at.saturating_add(4 * STRING_INDEX_STRIDE).min(bytes.len());
        let window = bytes.get(at..end).unwrap_or_else(|| corrupt("a string's index is outside its bytes"));
        let text = match std::str::from_utf8(window) {
            Ok(s) => s,
            Err(e) if e.error_len().is_none() => utf8(&window[..e.valid_up_to()]),
            Err(_) => corrupt("a string's index does not name UTF-8"),
        };
        (at, text, k % STRING_INDEX_STRIDE)
    }
}

/// Byte offset of character `k` (or the end), with at most 63 characters
/// skipped, even in an otherwise-ASCII string with one non-ASCII character.
pub(crate) unsafe fn str_offset(obj: *mut u64, k: usize) -> Option<usize> {
    unsafe {
        let b = bytes_obj(obj);
        let len = str_len(b);
        if k > len {
            return None;
        }
        if k == len {
            return Some(bytes_len(b));
        }
        if *b & ASCII != 0 {
            return Some(k);
        }
        let (at, text, skip) = str_index_window(b, k);
        Some(at + text.char_indices().nth(skip).unwrap_or_else(|| corrupt("a string's index has too few characters")).0)
    }
}

/// The character at `k`, with bounded decoding and no full-string scan.
#[inline]
pub(crate) unsafe fn str_ref(obj: *mut u64, k: usize) -> Option<char> {
    unsafe {
        if str_is_ascii(obj) {
            return str_bytes(obj).get(k).map(|&b| b as char);
        }
        if k >= str_len(obj) {
            return None;
        }
        let (_, text, skip) = str_index_window(obj, k);
        Some(text.chars().nth(skip).unwrap_or_else(|| corrupt("a string's index has too few characters")))
    }
}

/// Replaces bytes `at..at + bytes.len()` of a string, keeping its length,
/// and rebuilds its character index and ASCII flag.
pub(crate) unsafe fn str_replace(obj: *mut u64, at: usize, bytes: &[u8]) {
    unsafe {
        let b = bytes_obj(obj);
        let len = header_len(*b);
        if at.checked_add(bytes.len()).is_none_or(|end| end > len) {
            corrupt("a string's bytes changed past its length");
        }
        let data = bytes_at(b, 8, len);
        std::ptr::copy(bytes.as_ptr(), data.add(at), bytes.len());
        str_reindex(b, str_bytes(b).is_ascii());
        *obj = (*obj & !ASCII) | (*b & ASCII);
    }
}
/// Gives string `obj` the bytes of the plain string `text`, for a change
/// of size: `obj` then points to `text`. `obj` must have room for a field
/// (be at least one byte long). The caller applies the write barrier.
pub(crate) unsafe fn str_redirect(obj: *mut u64, text: Value) {
    unsafe {
        let h = *obj;
        let hash = (h & HASH_STORED != 0).then(|| *word(obj, base_words(h)));
        let kept = h & (REMEMBERED | MARKED | HASHED | HASH_STORED | IMMUTABLE);
        let ascii = *text.as_ptr() & ASCII;
        *obj = header(Kind::String, 1, kept | INDIRECT | ascii);
        set_field(obj, 0, text);
        if let Some(hash) = hash {
            *word(obj, 2) = hash;
        }
        // What the bytes held becomes a raw object, so that the nursery
        // stays parseable from end to end (`TECHNE_GC_VERIFY` walks it).
        let (was, now) = (object_words(h), object_words(*obj));
        if was > now {
            *word(obj, now) = header(Kind::Bytevector, (was - now - 1) * 8, 0);
        }
    }
}

/// The ASCII property through a checked string value.
pub(crate) fn is_ascii_string(v: Value) -> bool {
    is_kind(v, Kind::String) && unsafe { str_is_ascii(v.as_ptr()) }
}

pub(crate) unsafe fn str_is_ascii(obj: *mut u64) -> bool {
    unsafe { *obj & ASCII != 0 }
}

/// Whether `v` is a heap object of kind `k` that is not a literal: one test
/// of the header, as `is_kind`.
#[inline(always)]
pub(crate) fn is_changeable(v: Value, k: Kind) -> bool {
    v.is_ptr() && unsafe { *v.as_ptr() } & (KIND_MASK | IMMUTABLE) == k as u64
}

#[inline(always)]
pub(crate) fn is_kind(v: Value, k: Kind) -> bool {
    v.is_ptr() && unsafe { kind_of(v.as_ptr()) } == k as u8
}

/// Copies what a frozen world's values reach from its heap into another
/// heap's old generation (`Vm::instantiate`). The source is only read: its
/// objects get no forwarding headers, so it can start any number of worlds.
///
/// Every copy keeps its original's identity hash (stored after its fields,
/// as for a promoted object that was hashed): an `eq` hash table built in
/// the source finds its keys in the copy.
pub(crate) struct Copier {
    /// The source heap's cage, which its values are decoded in.
    source: *mut u64,
    copies: rustc_hash::FxHashMap<usize, *mut u64>,
    unscanned: Vec<*mut u64>,
    /// Copied foreign objects and their source foreign-table indices: the
    /// caller gives each copy its own entry (`Copier::foreign`).
    foreign: Vec<(*mut u64, usize)>,
}

impl Copier {
    /// A copier from the heap `source`, which must outlive it unchanged.
    pub(crate) fn new(source: &Heap) -> Copier {
        Copier { source: source.cage_base().cast(), copies: Default::default(), unscanned: Vec::new(), foreign: Vec::new() }
    }

    /// `v`, with an object of the source replaced by its copy in `dst`.
    pub(crate) fn value(&mut self, dst: &mut Heap, v: Value) -> Value {
        if !v.is_ptr() {
            return v;
        }
        let src = v.as_ptr_near(self.source);
        if let Some(&copy) = self.copies.get(&src.addr()) {
            return Value::ptr(copy);
        }
        // SAFETY: a live object of the frozen source heap.
        unsafe {
            let h = *src;
            debug_assert!(h & KIND_MASK != FORWARDED && h & KIND_MASK != FREE, "copying a dead object");
            let words = base_words(h);
            let from = bytes_at(src, 0, 8 * words).cast::<u64>();
            let hash = if h & HASH_STORED != 0 { *word(src, words) } else { src.addr() as u64 };
            let copy = dst.old.alloc(words + 1);
            std::ptr::copy_nonoverlapping(from, copy, words);
            *copy = (h & !(MARKED | REMEMBERED | HASHED)) | HASH_STORED;
            *copy.add(words) = hash;
            self.copies.insert(src.addr(), copy);
            if header_kind(h) == Kind::Foreign as u8 {
                self.foreign.push((copy, *word(src, 1) as usize));
            }
            self.unscanned.push(copy);
            Value::ptr(copy)
        }
    }

    /// Copy what the copies so far refer to, transitively.
    pub(crate) fn finish(&mut self, dst: &mut Heap) {
        while let Some(obj) = self.unscanned.pop() {
            // SAFETY: a copy made above, whose fields are values of the
            // source until replaced here.
            unsafe {
                let h = *obj;
                if is_traced(h) {
                    for i in 1..=header_len(h) {
                        let v = Value::from_bits(*word(obj, i));
                        *word(obj, i) = self.value(dst, v).bits();
                    }
                }
            }
        }
    }

    /// The foreign objects copied, each with the source's foreign-table
    /// index (still in its payload): the caller gives each its own entry
    /// and registers it (`Heap::register_foreign`).
    pub(crate) fn foreign(&mut self) -> Vec<(*mut u64, usize)> {
        std::mem::take(&mut self.foreign)
    }
}

#[cfg(test)]
impl Heap {
    /// Overwrite every word of the old generation and the nursery with
    /// garbage: any later use of this heap's objects shows (tests of
    /// worlds that must not refer into a template).
    pub(crate) fn scribble(&mut self) {
        const JUNK: u64 = 0xDEAD_BEEF_DEAD_BEEF;
        for b in &mut self.old.blocks {
            b.mem.fill(JUNK);
        }
        for l in &mut self.old.large {
            l.fill(JUNK);
        }
        let n = self.nursery_capacity();
        // SAFETY: the nursery's own words.
        unsafe { std::slice::from_raw_parts_mut(self.nursery_start, n).fill(JUNK) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_short_strings_have_no_index_storage() {
        let mut vm = crate::vm::Vm::new();
        let _world = vm.enter_account();
        for text in [String::new(), "123456".to_owned(), "a".repeat(40_000), "é😀".repeat(10)] {
            let value = vm.make_string_raw(&text);
            // SAFETY: no allocation or mutation while inspecting this string.
            unsafe {
                assert_eq!(*value.as_ptr() & STRING_INDEXED, 0);
                assert_eq!(object_words(*value.as_ptr()), bytevector_words(text.len()));
                assert_eq!(str_len(value.as_ptr()), text.chars().count());
                for (k, ch) in text.chars().enumerate() {
                    assert_eq!(str_ref(value.as_ptr(), k), Some(ch));
                }
            }
        }
        assert_eq!(string_words(6, true), 2);
        assert_eq!(string_words(63, false), bytevector_words(63));
        assert!(string_words(64, false) > bytevector_words(64));
    }

    #[test]
    fn unicode_indexing_decodes_a_bounded_window() {
        let mut vm = crate::vm::Vm::new();
        let _world = vm.enter_account();
        // The reported cliff: one non-ASCII character after an ASCII prefix.
        // Also exercise every UTF-8 width and checkpoints in non-ASCII text.
        for text in ["a".repeat(40_000) + "é", "aé€😀".repeat(10_001)] {
            let value = vm.make_string_raw(&text);
            let obj = value.as_ptr();
            let mut decoded_bytes = 0;
            for (k, (byte, expected)) in text.char_indices().enumerate() {
                // SAFETY: the string remains live and no allocation occurs.
                unsafe {
                    let (start, window, skip) = str_index_window(obj, k);
                    decoded_bytes += window.len();
                    assert!(window.len() <= 4 * STRING_INDEX_STRIDE);
                    assert!(skip < STRING_INDEX_STRIDE);
                    assert!(start <= byte && byte - start <= 4 * skip);
                    assert_eq!(str_ref(obj, k), Some(expected));
                    assert_eq!(str_offset(obj, k), Some(byte));
                }
            }
            // Deterministic work bound, not a wall-clock timeout: a full
            // scan cannot decode a quadratic number of bytes.
            let count = text.chars().count();
            assert!(decoded_bytes <= count * 4 * STRING_INDEX_STRIDE);
            unsafe {
                assert_eq!(str_len(obj), count);
                assert_eq!(str_offset(obj, count), Some(text.len()));
                assert_eq!(str_offset(obj, count + 1), None);
                assert_eq!(str_ref(obj, count), None);
            }
        }
    }

    #[test]
    fn field_scanners_preserve_immediate_bits_sharing_cycles_and_identity_hashes() {
        let immediates = [
            Value::float(0.0),
            Value::float(-0.0),
            Value::float(f64::NAN),
            Value::float(f64::INFINITY),
            Value::float(f64::NEG_INFINITY),
            Value::int_unchecked(crate::value::FIXNUM_MIN),
            Value::int_unchecked(crate::value::FIXNUM_MAX),
            Value::char('😀'),
            Value::NIL,
            Value::FALSE,
            Value::TRUE,
            Value::VOID,
        ];
        // The validation matrix runs this in separate default/reference
        // processes: no unsafe mutation of process environment in tests.
        let mut vm = crate::vm::Vm::bare();
        let _world = vm.enter_account();
        let list = vm.make_list_raw(&immediates);
        let list = vm.root(list);
        let cycle = vm.make_list_raw(&[list.get()]);
        let cycle = vm.root(cycle);
        // The newly allocated pair is young; no old-to-young barrier.
        unsafe { set_field(cycle.get().as_ptr(), 1, cycle.get()) };
        let vector = vm.make_vector_raw(&[list.get(), list.get(), cycle.get()]);
        let vector = vm.root(vector);
        let hash = vm.heap.identity_hash(list.get().as_ptr());
        for _ in 0..3 {
            vm.full_collect_raw();
            assert_eq!(vm.heap.identity_hash(list.get().as_ptr()), hash);
            unsafe {
                assert_eq!(field(vector.get().as_ptr(), 0), list.get());
                assert_eq!(field(vector.get().as_ptr(), 1), list.get());
                assert_eq!(field(vector.get().as_ptr(), 2), cycle.get());
                assert_eq!(field(cycle.get().as_ptr(), 0), list.get());
                assert_eq!(field(cycle.get().as_ptr(), 1), cycle.get());
                let mut p = list.get();
                for value in immediates {
                    assert_eq!(field(p.as_ptr(), 0).bits(), value.bits());
                    p = field(p.as_ptr(), 1);
                }
                assert_eq!(p, Value::NIL);
            }
        }
    }

    #[test]
    fn what_an_object_reaches_stays_in_its_cage() {
        use crate::alloc::CAGE_MASK;
        assert_eq!(header_len(header(Kind::Vector, 5, 0)), 5);
        assert!(header_len(u64::MAX) <= CAGE_MASK);
        // Words wrap within the cage, whatever the index; bytes past it are
        // corruption.
        let cage = crate::alloc::Cage::new();
        let base = cage.base().cast::<u64>();
        let last = base.wrapping_byte_add(CAGE_MASK + 1 - 8);
        let pair_header = header(Kind::Pair, 2, 0);
        // No dereference: exercise the exact shared extent check used by
        // both field-scanner variants, including fixed two-field pairs.
        assert_eq!(checked_field_len(last.wrapping_sub(3), pair_header), 2);
        for obj in [last.wrapping_sub(2), last.wrapping_sub(1), last] {
            assert!(std::panic::catch_unwind(|| checked_field_len(obj, pair_header)).is_err());
        }
        assert_eq!(word(last, 1), base);
        assert_eq!(word(base, usize::MAX), last);
        assert_eq!(word(last, (CAGE_MASK + 1) / 8 + 3), base.wrapping_add(2));
        assert_eq!(bytes_at(last, 0, 8), last.cast());
        let past = std::panic::catch_unwind(|| bytes_at(last, 8, 1));
        assert!(past.is_err(), "bytes past the cage are corruption");
    }
}
