//! Opt-in allocation census at collector/native boundaries, not generated
//! allocation sites. Pairs and immutable integers retain their layout, so
//! their nursery counts are exact. Other kinds may include redirect filler.
use super::*;
use crate::alloc::Reservation;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub nursery_objects: u64,
    pub nursery_words: u64,
    pub pairs: u64,
    pub pair_words: u64,
    pub bigints: u64,
    pub bigint_words: u64,
    pub direct_old_objects: u64,
    pub direct_old_words: u64,
    pub promoted_objects: u64,
    pub promoted_words: u64,
    pub promoted_pairs: u64,
    pub promoted_pair_words: u64,
    pub promoted_bigints: u64,
    pub promoted_bigint_words: u64,
    pub reverse_calls: u64,
    pub reverse_minors: u64,
    pub reverse_promoted_words: u64,
    pub reverse_promoted_pair_words: u64,
    pub truncated: bool,
}

impl Snapshot {
    /// Combine disjoint counter intervals, retaining any truncation warning.
    /// Callers must not add the same nursery interval more than once.
    fn add(&mut self, other: Self) {
        self.nursery_objects += other.nursery_objects;
        self.nursery_words += other.nursery_words;
        self.pairs += other.pairs;
        self.pair_words += other.pair_words;
        self.bigints += other.bigints;
        self.bigint_words += other.bigint_words;
        self.direct_old_objects += other.direct_old_objects;
        self.direct_old_words += other.direct_old_words;
        self.promoted_objects += other.promoted_objects;
        self.promoted_words += other.promoted_words;
        self.promoted_pairs += other.promoted_pairs;
        self.promoted_pair_words += other.promoted_pair_words;
        self.promoted_bigints += other.promoted_bigints;
        self.promoted_bigint_words += other.promoted_bigint_words;
        self.reverse_calls += other.reverse_calls;
        self.reverse_minors += other.reverse_minors;
        self.reverse_promoted_words += other.reverse_promoted_words;
        self.reverse_promoted_pair_words += other.reverse_promoted_pair_words;
        self.truncated |= other.truncated;
    }
}

pub(super) struct Census {
    total: Snapshot,
    _charged: Option<Reservation>,
}

impl Census {
    /// Create fixed-size counters charged to the budget owner, declining an
    /// explicit accounting reservation that cannot be admitted.
    pub fn new(budget: &Budget) -> Option<Box<Self>> {
        let _account = budget.enter();
        let charged = budget.charge(size_of::<Self>()).ok()?;
        Some(Box::new(Self { total: Snapshot::default(), _charged: charged }))
    }

    /// Record one direct old-space request in object words, excluding GC
    /// copies and allocator size-class slack.
    pub fn old(&mut self, words: usize) {
        self.total.direct_old_objects += 1;
        self.total.direct_old_words += words as u64;
    }

    /// Record one newly evacuated object using its pre-forwarding header.
    /// Destination words include a stored identity hash when present; already
    /// forwarded objects must not be recorded again. No guest pointer is kept.
    pub fn promoted(&mut self, h: u64, words: usize) {
        self.total.promoted_objects += 1;
        self.total.promoted_words += words as u64;
        match header_kind(h) {
            k if k == Kind::Pair as u8 => {
                self.total.promoted_pairs += 1;
                self.total.promoted_pair_words += words as u64;
            }
            k if k == Kind::BigInt as u8 => {
                self.total.promoted_bigints += 1;
                self.total.promoted_bigint_words += words as u64;
            }
            _ => {}
        }
    }
}

/// Walk completed nursery allocations before forwarding can change headers.
/// No allocation, roots, pointer retention, heap mutation or unchecked reads.
fn nursery(heap: &Heap) -> Snapshot {
    let mut out = Snapshot::default();
    let Some(bytes) = heap.top.addr().checked_sub(heap.nursery_start.addr()) else {
        out.truncated = true;
        return out;
    };
    let used = bytes / 8;
    if !bytes.is_multiple_of(8) || used > heap.nursery.len() {
        out.truncated = true;
        return out;
    }
    let mut at = 0;
    while at < used {
        let h = heap.nursery[at];
        let words = object_words(h);
        if Kind::of(header_kind(h)).is_none() || words == 0 || words > used - at {
            out.truncated = true;
            break;
        }
        out.nursery_objects += 1;
        out.nursery_words += words as u64;
        match header_kind(h) {
            k if k == Kind::Pair as u8 => {
                out.pairs += 1;
                out.pair_words += words as u64;
            }
            k if k == Kind::BigInt as u8 => {
                out.bigints += 1;
                out.bigint_words += words as u64;
            }
            _ => {}
        }
        at += words;
    }
    out
}

impl Heap {
    /// Cumulative nursery census plus direct old allocation requests.
    /// Promotion is not allocation again. A snapshot does not collect/reset.
    pub(crate) fn allocation_census(&self) -> Option<Snapshot> {
        let mut out = self.census.as_ref()?.total;
        out.add(nursery(self));
        Some(out)
    }

    /// Only cumulative counters: unlike allocation_census, this must not
    /// scan a bulk reservation whose objects are not initialized yet.
    pub(crate) fn reverse_checkpoint(&self) -> Option<(u64, u64, u64)> {
        let total = self.census.as_ref()?.total;
        Some((self.stats.minor, total.promoted_words, total.promoted_pair_words))
    }

    /// Attribute one successful reverse preparation to a checkpoint from
    /// this census. Consume each checkpoint once; counters must not reset
    /// between samples. Disabled recording is a no-op. This neither allocates
    /// nor scans the unfinished reservation that preparation just returned.
    pub(crate) fn count_reverse(&mut self, before: Option<(u64, u64, u64)>) {
        if let (Some(census), Some((minor, words, pair_words))) = (&mut self.census, before) {
            census.total.reverse_calls += 1;
            census.total.reverse_minors += self.stats.minor - minor;
            census.total.reverse_promoted_words += census.total.promoted_words - words;
            census.total.reverse_promoted_pair_words += census.total.promoted_pair_words - pair_words;
        }
    }

    /// Commit the completed nursery interval before forwarding and top reset.
    /// Call once per minor, never during construction of a reserved span.
    pub(super) fn count_nursery(&mut self) {
        if self.census.is_some() {
            let scanned = nursery(self);
            if let Some(census) = &mut self.census {
                census.total.add(scanned);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocations_are_counted_once_across_snapshots_and_collection() {
        let mut vm = crate::vm::Vm::bare();
        let _world = vm.enter_account();
        vm.heap.census = Census::new(vm.heap.budget());
        let before = vm.heap.allocation_census().unwrap();
        let list = vm.make_list_raw(&[Value::NIL; 3]);
        let list = vm.root(list);
        let big = vm.make_int_raw(1i64 << 50);
        let big = vm.root(big);
        let first = vm.heap.allocation_census().unwrap();
        assert_eq!(first.pairs - before.pairs, 3);
        assert_eq!(first.pair_words - before.pair_words, 9);
        assert_eq!(first.bigints - before.bigints, 1);
        assert_eq!(first.bigint_words - before.bigint_words, 2);
        assert!(!first.truncated);
        assert_eq!(first, vm.heap.allocation_census().unwrap());
        vm.full_collect_raw();
        let second = vm.heap.allocation_census().unwrap();
        assert_eq!(first.pairs, second.pairs);
        assert_eq!(first.bigints, second.bigints);
        assert_eq!(first.direct_old_objects, second.direct_old_objects, "promotion is not a direct allocation");
        assert_eq!(second.promoted_pairs - first.promoted_pairs, 3);
        assert_eq!(second.promoted_pair_words - first.promoted_pair_words, 9);
        assert_eq!(second.promoted_bigints - first.promoted_bigints, 1);
        assert_eq!(second.promoted_bigint_words - first.promoted_bigint_words, 2);
        vm.full_collect_raw();
        assert_eq!(second, vm.heap.allocation_census().unwrap(), "old survivors are not promoted twice");
        assert!(is_kind(list.get(), Kind::Pair));
        assert!(is_kind(big.get(), Kind::BigInt));
        let items = vec![Value::NIL; LARGE_WORDS];
        vm.make_vector_raw(&items);
        let old = vm.heap.allocation_census().unwrap();
        assert_eq!(old.direct_old_objects - second.direct_old_objects, 1);
        assert_eq!(old.direct_old_words - second.direct_old_words, 1 + LARGE_WORDS as u64);
    }

    #[test]
    fn reverse_preparation_counts_promotion_without_scanning_an_uninitialized_reservation() {
        let mut vm = crate::vm::Vm::bare();
        let _world = vm.enter_account();
        vm.heap.census = Census::new(vm.heap.budget());
        vm.full_collect_raw();
        vm.heap.set_nursery_window(8192);
        let list = vm.make_list_raw(&[Value::NIL; 2000]);
        let list = vm.root(list);
        let before = vm.heap.reverse_checkpoint();
        // Just as Bulk::list does: this collects, then reserves an unfinished
        // span. Reading a nursery census here would mistake its zero headers
        // for malformed objects. The reverse checkpoint reads totals only.
        vm.reserve_nursery(6000);
        vm.heap.count_reverse(before);
        for pair in vm.heap.nursery[..6000].as_chunks_mut::<3>().0 {
            *pair = [header(Kind::Pair, 2, 0), Value::NIL.bits(), Value::NIL.bits()];
        }
        let snapshot = vm.heap.allocation_census().unwrap();
        assert!(!snapshot.truncated);
        assert_eq!(snapshot.reverse_calls, 1);
        assert_eq!(snapshot.reverse_minors, 1);
        assert_eq!(snapshot.reverse_promoted_pair_words, 6000);
        assert_eq!(snapshot.reverse_promoted_words, 6000);
        assert!(is_kind(list.get(), Kind::Pair));
    }

    #[test]
    fn malformed_nursery_is_reported_without_walking_past_it() {
        let mut heap = Heap::new_in(Budget::new(usize::MAX));
        heap.census = Census::new(heap.budget());
        heap.bump(3);
        heap.nursery[0] = header(Kind::Pair, 100, 0);
        assert!(heap.allocation_census().unwrap().truncated);
        heap.nursery[0] = 0;
        assert!(heap.allocation_census().unwrap().truncated);
    }
}
