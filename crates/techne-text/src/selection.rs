//! Selections (EDITOR.md, section 4): an ordered set of directed ranges with
//! a primary one. Overlapping ranges merge; the primary keeps its identity
//! across merges. An empty range is a caret.

use crate::change::{Assoc, ChangeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Range {
    pub anchor: usize,
    pub head: usize,
}

impl Range {
    pub fn new(anchor: usize, head: usize) -> Range {
        Range { anchor, head }
    }

    pub fn caret(pos: usize) -> Range {
        Range { anchor: pos, head: pos }
    }

    pub fn from(self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn to(self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    pub fn span(self) -> std::ops::Range<usize> {
        self.from()..self.to()
    }

    /// Through another actor's change: a caret stays before text inserted at
    /// it; a range does not grow to include insertions at its ends.
    pub fn map(self, c: &ChangeSet) -> Range {
        if self.is_empty() {
            return Range::caret(c.map_pos(self.head, Assoc::Before));
        }
        let (from, to) = (c.map_pos(self.from(), Assoc::After), c.map_pos(self.to(), Assoc::Before));
        let to = to.max(from);
        if self.anchor <= self.head { Range::new(from, to) } else { Range::new(to, from) }
    }

    fn overlaps(self, next: Range) -> bool {
        next.from() < self.to() || (self.is_empty() && next.is_empty() && self.head == next.head)
    }

    fn merge(self, other: Range) -> Range {
        let (from, to) = (self.from().min(other.from()), self.to().max(other.to()));
        if self.anchor <= self.head { Range::new(from, to) } else { Range::new(to, from) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    ranges: Vec<Range>,
    primary: usize,
}

impl Selection {
    pub fn single(range: Range) -> Selection {
        Selection { ranges: vec![range], primary: 0 }
    }

    /// Sorted and merged; `primary` indexes `ranges` as given.
    pub fn new(ranges: Vec<Range>, primary: usize) -> Selection {
        assert!(primary < ranges.len(), "the primary range must exist");
        let mut indexed: Vec<(usize, Range)> = ranges.into_iter().enumerate().collect();
        indexed.sort_by_key(|(_, r)| (r.from(), r.to()));
        let mut out: Vec<Range> = Vec::with_capacity(indexed.len());
        let mut new_primary = 0;
        for (i, r) in indexed {
            match out.last_mut() {
                Some(last) if last.overlaps(r) => {
                    // The merged range takes the direction of the primary.
                    *last = if i == primary { r.merge(*last) } else { last.merge(r) };
                }
                _ => out.push(r),
            }
            if i == primary {
                new_primary = out.len() - 1;
            }
        }
        Selection { ranges: out, primary: new_primary }
    }

    pub fn ranges(&self) -> &[Range] {
        &self.ranges
    }

    pub fn primary_index(&self) -> usize {
        self.primary
    }

    pub fn primary(&self) -> Range {
        self.ranges[self.primary]
    }

    /// Each range replaced by `f` of it, merged again.
    pub fn transform(&self, f: impl FnMut(Range) -> Range) -> Selection {
        Selection::new(self.ranges.iter().copied().map(f).collect(), self.primary)
    }

    pub fn map(&self, c: &ChangeSet) -> Selection {
        self.transform(|r| r.map(c))
    }

    /// Through the view's own edit: carets move past what was inserted at
    /// them, so typing advances them.
    pub fn map_own(&self, c: &ChangeSet) -> Selection {
        self.transform(|r| if r.is_empty() { Range::caret(c.map_pos(r.head, Assoc::After)) } else { r.map(c) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_ranges_merge_and_keep_the_primary() {
        let s = Selection::new(vec![Range::new(6, 9), Range::new(0, 2), Range::new(8, 4), Range::caret(1)], 2);
        assert_eq!(s.ranges(), &[Range::new(0, 2), Range::new(9, 4)]);
        assert_eq!(s.primary(), Range::new(9, 4));
    }

    #[test]
    fn touching_ranges_and_carets_at_ends_stay_apart() {
        let s = Selection::new(vec![Range::new(0, 2), Range::new(2, 4), Range::caret(4)], 0);
        assert_eq!(s.ranges().len(), 3);
        let carets = Selection::new(vec![Range::caret(3), Range::caret(3)], 1);
        assert_eq!(carets.ranges(), &[Range::caret(3)]);
    }

    #[test]
    fn mapping_does_not_swallow_insertions() {
        let c = ChangeSet::from_edits(10, [(2..2, "ab"), (6..6, "cd")]).unwrap();
        assert_eq!(Range::new(2, 6).map(&c), Range::new(4, 8));
        assert_eq!(Range::caret(2).map(&c), Range::caret(2));
        let gone = ChangeSet::from_edits(10, [(1..8, "")]).unwrap();
        assert_eq!(Range::new(6, 3).map(&gone), Range::caret(1));
    }
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    /// Independent validity oracle (does not call production `Range::overlaps`):
    /// non-empty, valid primary index, sorted by `(from, to)`, and every
    /// adjacent pair strictly separated or touching without duplicate carets.
    fn is_valid_selection(s: &Selection) -> bool {
        let ranges = s.ranges();
        if ranges.is_empty() || s.primary_index() >= ranges.len() {
            return false;
        }
        for w in ranges.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a.from() > b.from() || a.to() > b.from() {
                return false;
            }
            // Touching duplicate carets (`a.from() == a.to() == b.from() == b.to()`) must be merged.
            if a.from() == a.to() && b.from() == b.to() && a.from() == b.from() {
                return false;
            }
        }
        true
    }

    fn is_covered(s: &Selection, r: Range) -> bool {
        s.ranges().iter().any(|out| out.from() <= r.from() && r.to() <= out.to())
    }

    fn any_range(max: usize) -> Range {
        let anchor: usize = kani::any();
        let head: usize = kani::any();
        kani::assume(anchor <= max && head <= max);
        Range::new(anchor, head)
    }

    fn any_nonempty_op(tag: u8, ins: &str) -> crate::change::Op {
        match tag {
            0 => {
                let n: usize = kani::any();
                kani::assume(n >= 1 && n <= 3);
                crate::change::Op::Retain(n)
            }
            1 => {
                let n: usize = kani::any();
                kani::assume(n >= 1 && n <= 3);
                crate::change::Op::Delete(n)
            }
            _ => crate::change::Op::Insert(String::from(ins)),
        }
    }

    fn any_changeset() -> ChangeSet {
        let t0: u8 = kani::any();
        let t1: u8 = kani::any();
        let t2: u8 = kani::any();
        kani::assume(t0 < 3 && t1 < 3 && t2 < 3);
        ChangeSet::from_canonical_ops(vec![any_nonempty_op(t0, "a"), any_nonempty_op(t1, "b"), any_nonempty_op(t2, "c")])
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn selection_new_maintains_sorted_non_overlapping_primary() {
        let r0 = any_range(6);
        let r1 = any_range(6);
        let r2 = any_range(6);
        let primary: usize = kani::any();
        kani::assume(primary < 3);
        let orig_primary = match primary {
            0 => r0,
            1 => r1,
            _ => r2,
        };

        let s = Selection::new(vec![r0, r1, r2], primary);
        assert!(is_valid_selection(&s));
        let p = s.primary();
        assert!(p.from() <= orig_primary.from() && orig_primary.to() <= p.to());
        if !orig_primary.is_empty() {
            assert_eq!(p.anchor <= p.head, orig_primary.anchor <= orig_primary.head);
        }
        assert!(is_covered(&s, r0));
        assert!(is_covered(&s, r1));
        assert!(is_covered(&s, r2));
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn range_map_preserves_bounds_direction_and_affinity() {
        let c = any_changeset();
        let len = c.len_before();

        let r0 = any_range(len);
        let mapped = r0.map(&c);
        assert!(mapped.to() <= c.len_after());
        if r0.is_empty() {
            assert_eq!(mapped, Range::caret(c.map_pos(r0.head, Assoc::Before)));
        } else {
            let exp_from = c.map_pos(r0.from(), Assoc::After);
            let exp_to = c.map_pos(r0.to(), Assoc::Before).max(exp_from);
            assert_eq!(mapped.from(), exp_from);
            assert_eq!(mapped.to(), exp_to);
            if !mapped.is_empty() {
                assert_eq!(mapped.anchor <= mapped.head, r0.anchor <= r0.head);
            }
        }
    }

    fn any_changeset_2() -> ChangeSet {
        let t0: u8 = kani::any();
        let t1: u8 = kani::any();
        kani::assume(t0 < 3 && t1 < 3);
        ChangeSet::from_canonical_ops(vec![any_nonempty_op(t0, "a"), any_nonempty_op(t1, "b")])
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn selection_map_preserves_invariants_and_affinity() {
        let c = any_changeset_2();
        let len = c.len_before();

        let r0 = any_range(len);
        let r1 = any_range(len);
        kani::assume(r0.to() <= r1.from() && !(r0.is_empty() && r1.is_empty() && r0.from() == r1.from()));
        let primary = usize::from(kani::any::<bool>());
        let s = Selection { ranges: vec![r0, r1], primary };
        assert!(is_valid_selection(&s));

        let m0 = r0.map(&c);
        let m1 = r1.map(&c);
        let exp_primary = if primary == 0 { m0 } else { m1 };

        let mapped = s.map(&c);
        assert!(is_valid_selection(&mapped));
        assert!(mapped.ranges().iter().all(|x| x.to() <= c.len_after()));
        assert!(is_covered(&mapped, m0));
        assert!(is_covered(&mapped, m1));
        let p = mapped.primary();
        assert!(p.from() <= exp_primary.from() && exp_primary.to() <= p.to());
        if !exp_primary.is_empty() {
            assert_eq!(p.anchor <= p.head, exp_primary.anchor <= exp_primary.head);
        }
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn selection_map_own_advances_carets_and_preserves_invariants() {
        let c = any_changeset_2();
        let len = c.len_before();

        let r0 = any_range(len);
        let r1 = any_range(len);
        kani::assume(r0.to() <= r1.from() && !(r0.is_empty() && r1.is_empty() && r0.from() == r1.from()));
        let primary = usize::from(kani::any::<bool>());
        let s = Selection { ranges: vec![r0, r1], primary };
        assert!(is_valid_selection(&s));

        let map_own_one = |r: Range| if r.is_empty() { Range::caret(c.map_pos(r.head, Assoc::After)) } else { r.map(&c) };
        let m0 = map_own_one(r0);
        let m1 = map_own_one(r1);
        let exp_primary = if primary == 0 { m0 } else { m1 };

        let mapped = s.map_own(&c);
        assert!(is_valid_selection(&mapped));
        assert!(mapped.ranges().iter().all(|x| x.to() <= c.len_after()));
        assert!(is_covered(&mapped, m0));
        assert!(is_covered(&mapped, m1));
        let p = mapped.primary();
        assert!(p.from() <= exp_primary.from() && exp_primary.to() <= p.to());
        if !exp_primary.is_empty() {
            assert_eq!(p.anchor <= p.head, exp_primary.anchor <= exp_primary.head);
        }
    }
}
