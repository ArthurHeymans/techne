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
    pub truncated: bool,
}

impl Snapshot {
    fn add(&mut self, other: Self) {
        self.nursery_objects += other.nursery_objects;
        self.nursery_words += other.nursery_words;
        self.pairs += other.pairs;
        self.pair_words += other.pair_words;
        self.bigints += other.bigints;
        self.bigint_words += other.bigint_words;
        self.direct_old_objects += other.direct_old_objects;
        self.direct_old_words += other.direct_old_words;
        self.truncated |= other.truncated;
    }
}

pub(super) struct Census {
    total: Snapshot,
    _charged: Option<Reservation>,
}

impl Census {
    pub fn new(budget: &Budget) -> Option<Box<Self>> {
        let _account = budget.enter();
        let charged = budget.charge(size_of::<Self>()).ok()?;
        Some(Box::new(Self { total: Snapshot::default(), _charged: charged }))
    }

    pub fn old(&mut self, words: usize) {
        self.total.direct_old_objects += 1;
        self.total.direct_old_words += words as u64;
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
        assert!(is_kind(list.get(), Kind::Pair));
        assert!(is_kind(big.get(), Kind::BigInt));
        let items = vec![Value::NIL; LARGE_WORDS];
        vm.make_vector_raw(&items);
        let old = vm.heap.allocation_census().unwrap();
        assert_eq!(old.direct_old_objects - second.direct_old_objects, 1);
        assert_eq!(old.direct_old_words - second.direct_old_words, 1 + LARGE_WORDS as u64);
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
