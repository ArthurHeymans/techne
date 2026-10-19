//! `TECHNE_GC_VERIFY`: check the heap's invariants before and after every
//! collection, and abort naming the first slots that break them.
//!
//! A missing or stale root shows only when a later collection trips over
//! what it left behind, far from the code that lost it. This walks every
//! root and every object reachable from them and checks, of each pointer:
//!
//! * a nursery pointer names an object's header below the bump pointer
//!   (after a minor collection the nursery is empty, so there are none);
//! * an old pointer names a slot of a block that is in use, or a large
//!   object;
//! * while sweeping, a reachable object in a block not swept yet is marked
//!   (else sweeping frees it);
//! * an old object holding a nursery pointer is remembered (or is a
//!   code's handle, which its code's constants update).
//!
//! Slots kept only while something refers to them (`Roots::visit_unrooted`)
//! may name dead old objects: of those, only nursery pointers are checked.
//!
//! After a minor collection the nursery's old contents are poisoned, so
//! that a stale pointer the walk cannot reach (a missing root) fails at
//! its next use instead of reading an object that happens to be there.
//!
//! The walk costs a full traversal per collection: combine it with
//! `TECHNE_GC_STRESS` to check at every allocation.

use std::fmt::Write as _;

use rustc_hash::{FxHashMap, FxHashSet};

use super::*;

/// What the nursery's dead words are overwritten with: an invalid kind, a
/// length that makes any copy of it fail loudly.
const POISON: u64 = 0xDEAD_0000_0000_00FD;

/// Where a value was found, to print the path to a bad one.
#[derive(Clone, Copy)]
enum Origin {
    /// The `n`th slot `Roots::visit_named` visits.
    Root(usize),
    /// The `n`th slot of `Roots::visit_unrooted`.
    Unrooted(usize),
    Remembered,
    /// The handle of code `id`, which the closure at this address runs.
    CodeHandle(usize, usize),
    /// Field `i` (from 0) of the object at this address.
    Field(usize, usize),
}

/// Where the objects are, to tell a pointer to one from any other address.
struct Index {
    /// One bit per nursery word below the bump pointer: an object starts there.
    nursery_starts: Vec<u64>,
    /// Where walking the nursery stopped at a header it cannot read, if it did.
    nursery_unparsed: Option<usize>,
    /// Block memory (start, end, slot words, block index), sorted by start.
    blocks: Vec<(usize, usize, usize, usize)>,
    /// Large objects' starts and lengths in words.
    large: FxHashMap<usize, usize>,
    empty: FxHashSet<usize>,
    unswept: FxHashSet<usize>,
    remembered: FxHashSet<usize>,
}

impl Index {
    fn new(heap: &Heap) -> Index {
        let used = (heap.top as usize - heap.nursery_start as usize) / 8;
        let mut nursery_starts = vec![0u64; used.div_ceil(64)];
        let mut nursery_unparsed = None;
        let mut w = 0;
        while w < used {
            // SAFETY: below the bump pointer, within the nursery.
            let h = unsafe { *heap.nursery_start.add(w) };
            if Kind::of(header_kind(h)).is_none() || w + base_words(h) > used {
                nursery_unparsed = Some(w);
                break;
            }
            nursery_starts[w / 64] |= 1 << (w % 64);
            w += base_words(h);
        }
        let mut blocks: Vec<_> = (heap.old.blocks.iter().enumerate())
            .filter(|(_, b)| !b.mem.is_empty())
            .map(|(i, b)| {
                let start = b.mem.as_ptr().addr();
                (start, start + b.mem.len() * 8, classes().words[b.class as usize], i)
            })
            .collect();
        blocks.sort_unstable();
        let unswept = heap.old.unswept.iter().flatten().map(|&b| b as usize).collect();
        Index {
            nursery_starts,
            nursery_unparsed,
            blocks,
            large: heap.old.large.iter().map(|l| (l.as_ptr().addr(), l.len())).collect(),
            empty: heap.old.empty.iter().map(|&b| b as usize).collect(),
            unswept,
            remembered: heap.remembered.iter().map(|p| p.addr()).collect(),
        }
    }
}

impl Heap {
    /// Check the heap's invariants (`verify`); abort with a report if one
    /// is broken. `when` says where in the collection this runs.
    pub(crate) fn verify(&mut self, roots: &mut dyn Roots, when: &str) {
        // What the walk allocates is not the world's.
        crate::alloc::uncharged(|| self.verify_charged_to_none(roots, when));
    }

    fn verify_charged_to_none(&mut self, roots: &mut dyn Roots, when: &str) {
        let index = Index::new(self);
        let mut errors: Vec<(String, Option<Origin>)> = Vec::new();
        if let Some(w) = index.nursery_unparsed {
            // SAFETY: below the bump pointer.
            let h = unsafe { *self.nursery_start.add(w) };
            let used = (self.top as usize - self.nursery_start as usize) / 8;
            errors.push((format!("nursery word {w} (of {used}) is no header: {h:#x}"), None));
        }
        let mut seen: FxHashMap<usize, Origin> = FxHashMap::default();
        let mut pending: Vec<*mut u64> = Vec::new();
        let mut n = 0;
        roots.visit_named(&mut |_, v| {
            self.check(&index, *v, Origin::Root(n), &mut seen, &mut pending, &mut errors);
            n += 1;
        });
        // Unrooted slots may still name old objects that died (their
        // holders let go of them after the collection), but a minor
        // collection follows their nursery pointers.
        let mut n = 0;
        roots.visit_unrooted(&mut |v| {
            if v.is_ptr() && self.in_nursery(v.as_ptr_near(self.nursery_start)) {
                self.check(&index, *v, Origin::Unrooted(n), &mut seen, &mut pending, &mut errors);
            }
            n += 1;
        });
        for &obj in &self.remembered {
            self.check(&index, Value::ptr(obj), Origin::Remembered, &mut seen, &mut pending, &mut errors);
        }
        while let Some(obj) = pending.pop() {
            // SAFETY: `check` pushes only valid objects.
            let h = unsafe { *obj };
            if !is_traced(h) {
                continue;
            }
            if header_kind(h) == Kind::Closure as u8 {
                // A closure keeps its code's handle (`shade_code`).
                // SAFETY: a closure's first field is its code's id.
                let id = code_slot(unsafe { field(obj, 0) });
                if let Some(&handle) = self.code_handles.get(id) {
                    self.check(&index, handle, Origin::CodeHandle(id, obj.addr()), &mut seen, &mut pending, &mut errors);
                }
            }
            let old = !self.in_nursery(obj);
            // A code's handle is updated from its code's constants instead
            // (`Vm::young_consts`, unrooted slots).
            let remembered = h & REMEMBERED != 0 || header_kind(h) == Kind::Code as u8 || index.remembered.contains(&obj.addr());
            for i in 0..header_len(h) {
                // SAFETY: a field of a traced object.
                let v = unsafe { field(obj, i) };
                if old && !remembered && v.is_ptr() && self.in_nursery(v.as_ptr_near(self.nursery_start)) {
                    errors.push(("old object holds a nursery pointer and is not remembered".into(), Some(Origin::Field(obj.addr(), i))));
                }
                self.check(&index, v, Origin::Field(obj.addr(), i), &mut seen, &mut pending, &mut errors);
            }
        }
        if errors.is_empty() {
            return;
        }
        let mut report =
            format!("TECHNE_GC_VERIFY: heap invariants broken {when} (minor #{}, phase {:?}):\n", self.stats.minor, self.phase);
        for (problem, origin) in errors.iter().take(20) {
            let _ = writeln!(report, "  {problem}");
            if let Some(origin) = origin {
                let _ = writeln!(report, "    at {}", self.path(roots, &seen, *origin));
            }
        }
        if errors.len() > 20 {
            let _ = writeln!(report, "  ... and {} more", errors.len() - 20);
        }
        eprint!("{report}");
        std::process::abort();
    }

    /// Check one value found at `origin`; queue the objects first met.
    fn check(
        &self,
        index: &Index,
        v: Value,
        origin: Origin,
        seen: &mut FxHashMap<usize, Origin>,
        pending: &mut Vec<*mut u64>,
        errors: &mut Vec<(String, Option<Origin>)>,
    ) {
        if !v.is_ptr() {
            return;
        }
        let p = v.as_ptr_near(self.nursery_start);
        if seen.contains_key(&p.addr()) {
            return;
        }
        if let Some(problem) = self.problem(index, p) {
            // The path is printed at the end, when every object's origin is known.
            errors.push((format!("{problem}: {:#x}", p.addr()), Some(origin)));
            return;
        }
        seen.insert(p.addr(), origin);
        pending.push(p);
    }

    /// What is wrong with a pointer to `p`, if anything: one it accepts
    /// names an object whose fields are in bounds, safe to walk.
    fn problem(&self, index: &Index, p: *mut u64) -> Option<String> {
        if self.in_nursery(p) {
            let w = (p as usize - self.nursery_start as usize) / 8;
            let used = (self.top as usize - self.nursery_start as usize) / 8;
            return if !(p as usize).is_multiple_of(8) {
                Some("misaligned nursery pointer".into())
            } else if w >= used {
                Some(format!("nursery pointer above the bump pointer (word {w} of {used} used)"))
            } else if index.nursery_starts[w / 64] & (1 << (w % 64)) == 0 {
                if index.nursery_unparsed.is_some_and(|u| w >= u) {
                    Some(format!("nursery pointer past where the nursery could be read (word {w})"))
                } else {
                    Some(format!("nursery pointer into the middle of an object (word {w})"))
                }
            } else {
                None
            };
        }
        let a = p.addr();
        if let Some(&words) = index.large.get(&a) {
            // SAFETY: the start of a large object.
            let h = unsafe { *p };
            return match Kind::of(header_kind(h)) {
                None => Some(format!("pointer to a large object with an invalid header {h:#x}")),
                Some(_) if object_words(h) > words => Some(format!("large object of {words} words with a longer header {h:#x}")),
                Some(_) => None,
            };
        }
        let i = index.blocks.partition_point(|b| b.0 <= a);
        let Some(&(start, end, size, b)) = i.checked_sub(1).map(|i| &index.blocks[i]) else {
            return Some("pointer outside the heap".into());
        };
        if a >= end {
            return Some("pointer outside the heap".into());
        }
        if !(a - start).is_multiple_of(size * 8) || a + size * 8 > end {
            return Some(format!("pointer into the middle of an old slot (block {b}, {size}-word slots)"));
        }
        if index.empty.contains(&b) {
            return Some(format!("pointer into block {b}, found empty by sweeping"));
        }
        // SAFETY: a slot of a block in use.
        let h = unsafe { *p };
        if h == 0 || header_kind(h) == FREE as u8 {
            return Some(format!("pointer to a free old slot (block {b}, header {h:#x})"));
        }
        if Kind::of(header_kind(h)).is_none() {
            return Some(format!("pointer to an old slot with an invalid header {h:#x}"));
        }
        if object_words(h) > size {
            return Some(format!("object longer than its {size}-word slot (block {b}, header {h:#x})"));
        }
        if self.phase == Phase::Sweeping && index.unswept.contains(&b) && h & MARKED == 0 {
            return Some(format!("unmarked object in block {b}, not swept yet: sweeping will free it"));
        }
        None
    }

    /// The path from a root to `origin`, for a report.
    fn path(&self, roots: &mut dyn Roots, seen: &FxHashMap<usize, Origin>, origin: Origin) -> String {
        let mut steps = Vec::new();
        let mut at = origin;
        loop {
            match at {
                Origin::Field(obj, i) => {
                    // SAFETY: an object the walk validated.
                    let h = unsafe { *(obj as *const u64) };
                    let kind = Kind::of(header_kind(h)).map_or("?".into(), |k| format!("{k:?}"));
                    let space = if self.in_nursery(obj as *mut u64) { "young" } else { "old" };
                    steps.push(format!("field {i} of {space} {kind} {obj:#x}"));
                    match seen.get(&obj) {
                        Some(&o) if steps.len() < 32 => at = o,
                        _ => break,
                    }
                }
                Origin::Root(n) => {
                    let mut label = format!("root #{n}");
                    let mut k = 0;
                    roots.visit_named(&mut |name, _| {
                        if k == n {
                            label = name.to_string();
                        }
                        k += 1;
                    });
                    steps.push(label);
                    break;
                }
                Origin::Unrooted(n) => {
                    steps.push(format!("unrooted slot #{n}"));
                    break;
                }
                Origin::Remembered => {
                    steps.push("the remembered set".into());
                    break;
                }
                Origin::CodeHandle(i, closure) => {
                    steps.push(format!("the handle of code {i}, run by closure {closure:#x}"));
                    match seen.get(&closure) {
                        Some(&o) if steps.len() < 32 => at = o,
                        _ => break,
                    }
                }
            }
        }
        steps.join(" <- ")
    }

    /// After a minor collection: overwrite the nursery words it emptied.
    pub(crate) fn poison_nursery(&mut self, used: usize) {
        // SAFETY: the nursery's own words, below the old bump pointer.
        unsafe { std::slice::from_raw_parts_mut(self.nursery_start, used).fill(POISON) };
    }
}
