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
