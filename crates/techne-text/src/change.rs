//! A change to a text: one pass over it that keeps, deletes or inserts bytes.
//!
//! Positions are byte offsets. A `ChangeSet` is the unit that transactions,
//! undo, anchors and the journal share: it applies to a rope, inverts against
//! the text it was applied to, composes with the change that follows it, maps
//! positions through itself and moves past a concurrent change it does not
//! conflict with.

use std::ops::Range;

use ropey::Rope;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Retain(usize),
    Delete(usize),
    Insert(String),
}

/// Which side of text inserted exactly at a position the position ends up on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Assoc {
    Before,
    After,
}

/// Ops in canonical form: no empty ops, no two adjacent ops of one kind, and
/// at one position an insertion before a deletion. Equal changes are equal
/// values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    ops: Vec<Op>,
    len_before: usize,
    len_after: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    /// Edits must be sorted by position and must not overlap.
    Overlap {
        at: usize,
    },
    OutOfBounds {
        end: usize,
        len: usize,
    },
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::Overlap { at } => write!(f, "edits overlap or are unsorted at byte {at}"),
            EditError::OutOfBounds { end, len } => write!(f, "edit ends at byte {end}, past the end ({len})"),
        }
    }
}

impl std::error::Error for EditError {}

/// The two changes touch the same text: one deletes what the other inserts
/// into or deletes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict;

impl ChangeSet {
    /// The change that keeps all `len` bytes.
    pub fn identity(len: usize) -> ChangeSet {
        let mut b = Builder::default();
        b.push(Op::Retain(len));
        b.finish()
    }

    /// Replace each range (in the original text, sorted and not overlapping)
    /// by its text. Several insertions at one position keep their order.
    pub fn from_edits<I, S>(len: usize, edits: I) -> Result<ChangeSet, EditError>
    where
        I: IntoIterator<Item = (Range<usize>, S)>,
        S: Into<String>,
    {
        let mut b = Builder::default();
        let mut pos = 0;
        for (r, text) in edits {
            if r.start < pos || r.end < r.start {
                return Err(EditError::Overlap { at: r.start });
            }
            if r.end > len {
                return Err(EditError::OutOfBounds { end: r.end, len });
            }
            b.push(Op::Retain(r.start - pos));
            b.push(Op::Insert(text.into()));
            b.push(Op::Delete(r.end - r.start));
            pos = r.end;
        }
        b.push(Op::Retain(len - pos));
        Ok(b.finish())
    }

    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    pub fn len_before(&self) -> usize {
        self.len_before
    }

    pub fn len_after(&self) -> usize {
        self.len_after
    }

    pub fn is_identity(&self) -> bool {
        self.ops.iter().all(|op| matches!(op, Op::Retain(_)))
    }

    /// Each replaced range of the original text with its new text; a pure
    /// insertion has an empty range.
    pub fn edits(&self) -> impl Iterator<Item = (Range<usize>, &str)> {
        let mut pos = 0;
        let mut ops = self.ops.iter().peekable();
        std::iter::from_fn(move || {
            loop {
                match ops.next()? {
                    Op::Retain(n) => pos += n,
                    Op::Delete(n) => {
                        pos += n;
                        return Some((pos - n..pos, ""));
                    }
                    Op::Insert(s) => {
                        let start = pos;
                        if let Some(Op::Delete(n)) = ops.peek() {
                            pos += n;
                            ops.next();
                        }
                        return Some((start..pos, s.as_str()));
                    }
                }
            }
        })
    }

    pub fn apply(&self, rope: &mut Rope) {
        assert_eq!(rope.len_bytes(), self.len_before, "change applied to a text of the wrong length");
        let mut pos = 0;
        for op in &self.ops {
            match op {
                Op::Retain(n) => pos += n,
                Op::Delete(n) => {
                    let (a, b) = (rope.byte_to_char(pos), rope.byte_to_char(pos + n));
                    rope.remove(a..b);
                }
                Op::Insert(s) => {
                    rope.insert(rope.byte_to_char(pos), s);
                    pos += s.len();
                }
            }
        }
    }

    /// The change that undoes this one, given the text this one applies to.
    pub fn invert(&self, original: &Rope) -> ChangeSet {
        let mut b = Builder::default();
        let mut pos = 0;
        for op in &self.ops {
            match op {
                Op::Retain(n) => {
                    b.push(Op::Retain(*n));
                    pos += n;
                }
                Op::Delete(n) => {
                    b.push(Op::Insert(original.byte_slice(pos..pos + n).to_string()));
                    pos += n;
                }
                Op::Insert(s) => b.push(Op::Delete(s.len())),
            }
        }
        b.finish()
    }

    /// This change followed by `next`, as one change.
    pub fn compose(&self, next: &ChangeSet) -> ChangeSet {
        assert_eq!(self.len_after, next.len_before, "composed changes do not line up");
        let (mut a, mut b) = (self.ops.iter().cloned(), next.ops.iter().cloned());
        let (mut ha, mut hb) = (a.next(), b.next());
        let mut out = Builder::default();
        loop {
            match (ha.take(), hb.take()) {
                (None, None) => break,
                (x, Some(Op::Insert(s))) => {
                    out.push(Op::Insert(s));
                    (ha, hb) = (x, b.next());
                }
                (Some(Op::Delete(n)), y) => {
                    out.push(Op::Delete(n));
                    (ha, hb) = (a.next(), y);
                }
                (Some(x), Some(y)) => {
                    // x is Retain or Insert (it produces text); y is Retain or
                    // Delete (it consumes text). Take the shorter of the two.
                    let (lx, ly) = (produced(&x), consumed(&y));
                    let m = lx.min(ly);
                    match (&x, &y) {
                        (Op::Retain(_), Op::Retain(_)) => out.push(Op::Retain(m)),
                        (Op::Retain(_), Op::Delete(_)) => out.push(Op::Delete(m)),
                        (Op::Insert(s), Op::Retain(_)) => out.push(Op::Insert(s[..m].to_string())),
                        (Op::Insert(_), Op::Delete(_)) => {}
                        _ => unreachable!(),
                    }
                    ha = if lx > m { Some(rest(x, m)) } else { a.next() };
                    hb = if ly > m { Some(rest(y, m)) } else { b.next() };
                }
                _ => panic!("composed changes do not line up"),
            }
        }
        out.finish()
    }

    /// The same change without the parts that put back what they delete:
    /// each replacement loses the text its old and new contents share at
    /// either end. `original` is the text the change applies to.
    pub fn minimized(&self, original: &Rope) -> ChangeSet {
        let edits: Vec<_> = self
            .edits()
            .map(|(r, new)| {
                let old = original.byte_slice(r.clone()).to_string();
                let prefix = shared(old.chars(), new.chars());
                let (old_rest, new_rest) = (&old[prefix..], &new[prefix..]);
                let suffix = shared(old_rest.chars().rev(), new_rest.chars().rev());
                (r.start + prefix..r.end - suffix, new_rest[..new_rest.len() - suffix].to_string())
            })
            .collect();
        ChangeSet::from_edits(self.len_before, edits).expect("trimmed edits stay ordered")
    }

    /// Where `pos` in the original text is after the change.
    pub fn map_pos(&self, pos: usize, assoc: Assoc) -> usize {
        self.map(pos, assoc).0
    }

    /// Like `map_pos`, also telling whether the change deleted the text
    /// around `pos` (a position at either end of a deletion survives).
    pub fn map(&self, pos: usize, assoc: Assoc) -> (usize, bool) {
        assert!(pos <= self.len_before, "position {pos} past the end ({})", self.len_before);
        let (mut old, mut new) = (0, 0);
        for op in &self.ops {
            match op {
                Op::Retain(n) => {
                    if old + n > pos {
                        return (new + (pos - old), false);
                    }
                    old += n;
                    new += n;
                }
                Op::Delete(n) => {
                    if old + n > pos {
                        return (new, pos > old);
                    }
                    old += n;
                }
                Op::Insert(s) => {
                    if old == pos && assoc == Assoc::Before {
                        return (new, false);
                    }
                    new += s.len();
                }
            }
        }
        (new + (pos - old), false)
    }

    /// Whether both changes, made to the same text, touch the same text: one
    /// deletes what the other deletes, or inserts strictly inside what the
    /// other deletes. Insertions at one position do not conflict.
    pub fn conflicts(&self, other: &ChangeSet) -> bool {
        assert_eq!(self.len_before, other.len_before, "changes to different texts");
        let a: Vec<Range<usize>> = self.edits().map(|(r, _)| r).collect();
        let b: Vec<Range<usize>> = other.edits().map(|(r, _)| r).collect();
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            if overlap(&a[i], &b[j]) {
                return true;
            }
            // Drop the range that ends first: it cannot reach the other's later ones.
            if (a[i].end, a[i].start) <= (b[j].end, b[j].start) {
                i += 1;
            } else {
                j += 1;
            }
        }
        false
    }

    /// This change moved past `other`, a change to the same text that was
    /// applied first. `ties` says on which side of `other`'s insertions at
    /// the same position this change's insertions go.
    pub fn transform(&self, other: &ChangeSet, ties: Assoc) -> Result<ChangeSet, Conflict> {
        if self.conflicts(other) {
            return Err(Conflict);
        }
        let edits: Vec<_> = self
            .edits()
            .map(|(r, text)| {
                // A deletion keeps away from insertions at its ends.
                let (start, end) = if r.is_empty() {
                    let p = other.map_pos(r.start, ties);
                    (p, p)
                } else {
                    (other.map_pos(r.start, Assoc::After), other.map_pos(r.end, Assoc::Before))
                };
                (start..end, text.to_string())
            })
            .collect();
        Ok(ChangeSet::from_edits(other.len_after, edits).expect("non-conflicting edits stay ordered"))
    }
}

/// Bytes in the common run of two character sequences.
fn shared(a: impl Iterator<Item = char>, b: impl Iterator<Item = char>) -> usize {
    a.zip(b).take_while(|(x, y)| x == y).map(|(x, _)| x.len_utf8()).sum()
}

fn overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => false,
        (true, false) => b.start < a.start && a.start < b.end,
        (false, true) => a.start < b.start && b.start < a.end,
        (false, false) => a.start < b.end && b.start < a.end,
    }
}

fn produced(op: &Op) -> usize {
    match op {
        Op::Retain(n) => *n,
        Op::Insert(s) => s.len(),
        Op::Delete(_) => 0,
    }
}

fn consumed(op: &Op) -> usize {
    match op {
        Op::Retain(n) | Op::Delete(n) => *n,
        Op::Insert(_) => 0,
    }
}

/// What is left of `op` after its first `m` bytes.
fn rest(op: Op, m: usize) -> Op {
    match op {
        Op::Retain(n) => Op::Retain(n - m),
        Op::Delete(n) => Op::Delete(n - m),
        Op::Insert(s) => Op::Insert(s[m..].to_string()),
    }
}

/// Collects ops into canonical form.
#[derive(Default)]
struct Builder {
    ops: Vec<Op>,
    len_before: usize,
    len_after: usize,
}

impl Builder {
    fn push(&mut self, op: Op) {
        match &op {
            Op::Retain(0) | Op::Delete(0) => return,
            Op::Insert(s) if s.is_empty() => return,
            Op::Retain(n) => {
                self.len_before += n;
                self.len_after += n;
            }
            Op::Delete(n) => self.len_before += n,
            Op::Insert(s) => self.len_after += s.len(),
        }
        match (self.ops.last_mut(), op) {
            (Some(Op::Retain(a)), Op::Retain(b)) | (Some(Op::Delete(a)), Op::Delete(b)) => *a += b,
            (Some(Op::Insert(a)), Op::Insert(b)) => a.push_str(&b),
            (Some(Op::Delete(_)), Op::Insert(s)) => {
                // An insertion goes before a deletion at the same position.
                let delete = self.ops.pop().expect("last op exists");
                match self.ops.last_mut() {
                    Some(Op::Insert(a)) => a.push_str(&s),
                    _ => self.ops.push(Op::Insert(s)),
                }
                self.ops.push(delete);
            }
            (_, op) => self.ops.push(op),
        }
    }

    fn finish(self) -> ChangeSet {
        ChangeSet { ops: self.ops, len_before: self.len_before, len_after: self.len_after }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, c: &ChangeSet) -> String {
        let mut r = Rope::from_str(text);
        c.apply(&mut r);
        r.to_string()
    }

    #[test]
    fn replacement_is_canonical() {
        let c = ChangeSet::from_edits(5, [(1..3, "xy")]).unwrap();
        assert_eq!(c.ops(), &[Op::Retain(1), Op::Insert("xy".into()), Op::Delete(2), Op::Retain(2)]);
        let inv = c.invert(&Rope::from_str("abcde"));
        assert_eq!(inv.ops(), &[Op::Retain(1), Op::Insert("bc".into()), Op::Delete(2), Op::Retain(2)]);
    }

    #[test]
    fn insertions_at_one_position_keep_their_order() {
        let c = ChangeSet::from_edits(2, [(1..1, "x"), (1..1, "y")]).unwrap();
        assert_eq!(apply("ab", &c), "axyb");
        assert_eq!(c.edits().collect::<Vec<_>>(), [(1..1, "xy")]);
    }

    #[test]
    fn positions_take_sides_at_insertions() {
        let c = ChangeSet::from_edits(4, [(2..2, "XX")]).unwrap();
        assert_eq!(c.map_pos(2, Assoc::Before), 2);
        assert_eq!(c.map_pos(2, Assoc::After), 4);
        assert_eq!(c.map_pos(3, Assoc::Before), 5);
        let d = ChangeSet::from_edits(4, [(1..3, "")]).unwrap();
        assert_eq!(d.map(2, Assoc::After), (1, true));
        assert_eq!(d.map(1, Assoc::After), (1, false));
        assert_eq!(d.map(3, Assoc::Before), (1, false));
    }

    #[test]
    fn rejects_overlap_and_out_of_bounds() {
        assert_eq!(ChangeSet::from_edits(5, [(1..3, ""), (2..4, "")]), Err(EditError::Overlap { at: 2 }));
        assert_eq!(ChangeSet::from_edits(5, [(4..6, "")]), Err(EditError::OutOfBounds { end: 6, len: 5 }));
    }

    #[test]
    fn minimizing_drops_what_is_put_back() {
        let text = Rope::from_str("abcde");
        let c = ChangeSet::from_edits(5, [(0..1, "a"), (2..5, "cXe")]).unwrap();
        assert_eq!(c.minimized(&text), ChangeSet::from_edits(5, [(3..4, "X")]).unwrap());
    }

    #[test]
    fn conflicts_need_shared_text() {
        let del = ChangeSet::from_edits(6, [(1..4, "")]).unwrap();
        let ins_inside = ChangeSet::from_edits(6, [(2..2, "x")]).unwrap();
        let ins_edge = ChangeSet::from_edits(6, [(4..4, "x")]).unwrap();
        let del_after = ChangeSet::from_edits(6, [(4..5, "")]).unwrap();
        assert!(del.conflicts(&ins_inside));
        assert!(ins_inside.conflicts(&del));
        assert!(!del.conflicts(&ins_edge));
        assert!(!del.conflicts(&del_after));
        assert!(!ins_edge.conflicts(&ChangeSet::from_edits(6, [(4..4, "y")]).unwrap()));
    }
}

#[cfg(kani)]
impl ChangeSet {
    pub(crate) fn from_canonical_ops(ops: Vec<Op>) -> Self {
        let (mut before, mut after) = (0usize, 0usize);
        for op in &ops {
            match op {
                Op::Retain(n) => {
                    before += n;
                    after += n;
                }
                Op::Delete(n) => before += n,
                Op::Insert(s) => after += s.len(),
            }
        }
        let c = ChangeSet { ops, len_before: before, len_after: after };
        kani::assume(kani_proofs::is_canonical(&c));
        c
    }
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    pub(super) fn is_canonical(c: &ChangeSet) -> bool {
        let (mut before, mut after) = (0usize, 0usize);
        let mut prev: Option<&Op> = None;
        for op in &c.ops {
            match op {
                Op::Retain(0) | Op::Delete(0) => return false,
                Op::Insert(s) if s.is_empty() => return false,
                Op::Retain(n) => {
                    before += n;
                    after += n;
                }
                Op::Delete(n) => before += n,
                Op::Insert(s) => after += s.len(),
            }
            if let Some(p) = prev {
                match (p, op) {
                    (Op::Retain(_), Op::Retain(_))
                    | (Op::Delete(_), Op::Delete(_))
                    | (Op::Insert(_), Op::Insert(_))
                    | (Op::Delete(_), Op::Insert(_)) => return false,
                    _ => {}
                }
            }
            prev = Some(op);
        }
        before == c.len_before && after == c.len_after
    }

    fn cap_string(s: &str) -> String {
        let mut out = String::with_capacity(4);
        out.push_str(s);
        out
    }

    fn any_nonempty_op(tag: u8, ins: &str) -> Op {
        match tag {
            0 => {
                let n: usize = kani::any();
                kani::assume(n >= 1 && n <= 4);
                Op::Retain(n)
            }
            1 => {
                let n: usize = kani::any();
                kani::assume(n >= 1 && n <= 4);
                Op::Delete(n)
            }
            _ => Op::Insert(cap_string(ins)),
        }
    }

    fn changeset_from_ops(ops: Vec<Op>) -> ChangeSet {
        let (mut before, mut after) = (0usize, 0usize);
        for op in &ops {
            match op {
                Op::Retain(n) => {
                    before += n;
                    after += n;
                }
                Op::Delete(n) => before += n,
                Op::Insert(s) => after += s.len(),
            }
        }
        ChangeSet { ops, len_before: before, len_after: after }
    }

    fn any_assoc() -> Assoc {
        if kani::any() { Assoc::Before } else { Assoc::After }
    }

    fn collect_inserted_bytes(ops: &[Op]) -> ([u8; 4], usize) {
        let mut buf = [0u8; 4];
        let mut len = 0usize;
        for op in ops {
            if let Op::Insert(s) = op {
                for &b in s.as_bytes() {
                    buf[len] = b;
                    len += 1;
                }
            }
        }
        (buf, len)
    }

    fn check_builder_push(ops: Vec<Op>) {
        let init = changeset_from_ops(ops);
        kani::assume(is_canonical(&init));

        let (mut exp_bytes, mut exp_bytes_len) = collect_inserted_bytes(&init.ops);
        let mut b = Builder { ops: init.ops, len_before: init.len_before, len_after: init.len_after };
        let (mut exp_before, mut exp_after) = (b.len_before, b.len_after);

        let next_tag: u8 = kani::any();
        kani::assume(next_tag < 3);
        match next_tag {
            0 => {
                let n: usize = kani::any();
                kani::assume(n <= 4);
                exp_before += n;
                exp_after += n;
                b.push(Op::Retain(n));
            }
            1 => {
                let n: usize = kani::any();
                kani::assume(n <= 4);
                exp_before += n;
                b.push(Op::Delete(n));
            }
            _ => {
                let s = if kani::any() { String::new() } else { cap_string("c") };
                exp_after += s.len();
                for &byte in s.as_bytes() {
                    exp_bytes[exp_bytes_len] = byte;
                    exp_bytes_len += 1;
                }
                b.push(Op::Insert(s));
            }
        }

        let c = b.finish();
        assert!(is_canonical(&c));
        assert_eq!(c.len_before(), exp_before);
        assert_eq!(c.len_after(), exp_after);
        let (got_bytes, got_bytes_len) = collect_inserted_bytes(c.ops());
        assert_eq!(got_bytes_len, exp_bytes_len);
        assert_eq!(got_bytes[0], exp_bytes[0]);
        assert_eq!(got_bytes[1], exp_bytes[1]);
        assert_eq!(got_bytes[2], exp_bytes[2]);
        assert_eq!(got_bytes[3], exp_bytes[3]);
    }

    /// Bounded check that `Builder::push` preserves canonicality, lengths, and
    /// inserted byte order from canonical prefixes of length 0 and 1 (with
    /// bounded op sizes and pre-reserved capacities).
    #[kani::proof]
    #[kani::unwind(4)]
    fn builder_push_preserves_canonical_prefix_0_1() {
        let mut ops0 = Vec::with_capacity(4);
        check_builder_push(ops0);

        let t0: u8 = kani::any();
        kani::assume(t0 < 3);
        ops0 = Vec::with_capacity(4);
        ops0.push(any_nonempty_op(t0, "a"));
        check_builder_push(ops0);
    }

    /// Bounded check covering all 2-op canonical suffixes (including
    /// `[Insert, Delete]` followed by `Insert`) with bounded op sizes and
    /// pre-reserved capacities: verifies canonicality, lengths, and exact
    /// inserted byte order across merges and swaps.
    #[kani::proof]
    #[kani::unwind(4)]
    fn builder_push_preserves_canonical_prefix_2() {
        let t0: u8 = kani::any();
        let t1: u8 = kani::any();
        kani::assume(t0 < 3 && t1 < 3);
        let mut ops = Vec::with_capacity(4);
        ops.push(any_nonempty_op(t0, "a"));
        ops.push(any_nonempty_op(t1, "b"));
        check_builder_push(ops);
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn edits_iterator_matches_ops_semantics() {
        let t0: u8 = kani::any();
        let t1: u8 = kani::any();
        let t2: u8 = kani::any();
        kani::assume(t0 < 3 && t1 < 3 && t2 < 3);
        let c = ChangeSet::from_canonical_ops(vec![any_nonempty_op(t0, "a"), any_nonempty_op(t1, "b"), any_nonempty_op(t2, "c")]);

        // Collect independent expected edits directly from the canonical op list.
        let mut exp_ranges = [(0usize, 0usize); 3];
        let mut exp_ins = [0u8; 3];
        let mut exp_ins_len = [0usize; 3];
        let mut exp_count = 0usize;
        let mut pos = 0usize;
        let mut idx = 0usize;
        while idx < c.ops.len() {
            match &c.ops[idx] {
                Op::Retain(n) => {
                    pos += *n;
                    idx += 1;
                }
                Op::Delete(n) => {
                    exp_ranges[exp_count] = (pos, pos + *n);
                    exp_ins_len[exp_count] = 0;
                    exp_count += 1;
                    pos += *n;
                    idx += 1;
                }
                Op::Insert(s) => {
                    let start = pos;
                    if let Some(Op::Delete(n)) = c.ops.get(idx + 1) {
                        pos += *n;
                        idx += 2;
                    } else {
                        idx += 1;
                    }
                    exp_ranges[exp_count] = (start, pos);
                    exp_ins[exp_count] = s.as_bytes()[0];
                    exp_ins_len[exp_count] = s.len();
                    exp_count += 1;
                }
            }
        }

        let mut prev_end = 0usize;
        let mut got_count = 0usize;
        let mut deleted = 0usize;
        let mut inserted = 0usize;
        for (r, s) in c.edits() {
            assert!(got_count < exp_count);
            assert_eq!((r.start, r.end), exp_ranges[got_count]);
            assert_eq!(s.len(), exp_ins_len[got_count]);
            if !s.is_empty() {
                assert_eq!(s.as_bytes()[0], exp_ins[got_count]);
            }
            assert!(r.start >= prev_end);
            assert!(r.start <= r.end);
            assert!(r.end <= c.len_before());
            assert!(!r.is_empty() || !s.is_empty());
            prev_end = r.end;
            deleted += r.end - r.start;
            inserted += s.len();
            got_count += 1;
        }
        assert_eq!(got_count, exp_count);
        assert_eq!(c.len_before() - deleted + inserted, c.len_after());
    }

    #[kani::proof]
    fn produced_consumed_rest_retain_and_delete() {
        let n: usize = kani::any();
        let m: usize = kani::any();
        kani::assume(m < n);
        if kani::any() {
            let op = Op::Retain(n);
            assert_eq!(produced(&op), n);
            assert_eq!(consumed(&op), n);
            assert!(matches!(rest(op, m), Op::Retain(rem) if rem == n - m));
        } else {
            let op = Op::Delete(n);
            assert_eq!(produced(&op), 0);
            assert_eq!(consumed(&op), n);
            assert!(matches!(rest(op, m), Op::Delete(rem) if rem == n - m));
        }
    }

    /// Caller precondition: `rest` on `Op::Insert` requires `m` to be a UTF-8
    /// char boundary in `0..=s.len()`. Verify exact suffix bytes for ASCII and
    /// multibyte UTF-8 splits.
    #[kani::proof]
    #[kani::unwind(4)]
    fn produced_consumed_rest_insert_ascii() {
        let op = Op::Insert(String::from("ab"));
        assert_eq!(produced(&op), 2);
        assert_eq!(consumed(&op), 0);
        let Op::Insert(s) = rest(op, 1) else { panic!("expected Insert") };
        assert_eq!(s.len(), 1);
        assert_eq!(s.as_bytes()[0], b'b');
    }

    #[kani::proof]
    #[kani::unwind(4)]
    fn produced_consumed_rest_insert_multibyte() {
        let op = Op::Insert(String::from("éc"));
        assert_eq!(produced(&op), 3);
        assert_eq!(consumed(&op), 0);
        let Op::Insert(s) = rest(op, 2) else { panic!("expected Insert") };
        assert_eq!(s.len(), 1);
        assert_eq!(s.as_bytes()[0], b'c');
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn map_pos_is_bounded_monotone_and_anchored() {
        let t0: u8 = kani::any();
        let t1: u8 = kani::any();
        let t2: u8 = kani::any();
        kani::assume(t0 < 3 && t1 < 3 && t2 < 3);
        let c = ChangeSet::from_canonical_ops(vec![any_nonempty_op(t0, "a"), any_nonempty_op(t1, "b"), any_nonempty_op(t2, "c")]);

        assert_eq!(c.map_pos(0, Assoc::Before), 0);
        assert_eq!(c.map_pos(c.len_before(), Assoc::After), c.len_after());

        let p1: usize = kani::any();
        let p2: usize = kani::any();
        kani::assume(p1 <= p2 && p2 <= c.len_before());
        let assoc = any_assoc();
        let m1 = c.map_pos(p1, assoc);
        let m2 = c.map_pos(p2, assoc);
        assert!(m1 <= m2);
        assert!(m2 <= c.len_after());
        assert!(c.map_pos(p1, Assoc::Before) <= c.map_pos(p1, Assoc::After));
    }

    /// Full `usize` range check for `ChangeSet::map` on an identity change
    /// (verifies no intermediate `new + pos` overflow even at `usize::MAX`).
    #[kani::proof]
    #[kani::unwind(3)]
    fn identity_map_pos_full_usize_range() {
        let len: usize = kani::any();
        let pos: usize = kani::any();
        kani::assume(pos <= len);
        let c = if len == 0 {
            ChangeSet { ops: Vec::new(), len_before: 0, len_after: 0 }
        } else {
            ChangeSet { ops: vec![Op::Retain(len)], len_before: len, len_after: len }
        };
        let assoc = any_assoc();
        assert_eq!(c.map(pos, assoc), (pos, false));
    }

    #[kani::proof]
    fn overlap_matches_spec_for_all_ranges() {
        let s0: usize = kani::any();
        let e0: usize = kani::any();
        let s1: usize = kani::any();
        let e1: usize = kani::any();
        kani::assume(s0 <= e0 && s1 <= e1);
        let r0 = s0..e0;
        let r1 = s1..e1;
        let expected = if r0.is_empty() && r1.is_empty() {
            false
        } else if r0.is_empty() {
            s1 < s0 && s0 < e1
        } else if r1.is_empty() {
            s0 < s1 && s1 < e0
        } else {
            s0.max(s1) < e0.min(e1)
        };
        assert_eq!(overlap(&r0, &r1), expected);
        assert_eq!(overlap(&r0, &r1), overlap(&r1, &r0));
    }

    /// Multi-edit `ChangeSet::conflicts` scan: `a` has two disjoint deletions
    /// (`0..n0` and `n0+n1..len`) and `b` deletes `m0..len`, forcing cursor
    /// advancement past `a`'s first edit when `n0 <= m0`; compares against an
    /// independent pairwise overlap oracle.
    #[kani::proof]
    #[kani::unwind(4)]
    fn conflicts_multi_edit_scan_matches_oracle() {
        let n0: usize = kani::any();
        let n1: usize = kani::any();
        let n2: usize = kani::any();
        let m0: usize = kani::any();
        let m1: usize = kani::any();
        kani::assume(n0 >= 1 && n0 <= 4 && n1 >= 1 && n1 <= 4 && n2 >= 1 && n2 <= 4);
        kani::assume(m0 >= 1 && m0 <= 8 && m1 >= 1 && m1 <= 8);
        let len = n0 + n1 + n2;
        kani::assume(m0 + m1 == len);

        let a = ChangeSet { ops: vec![Op::Delete(n0), Op::Retain(n1), Op::Delete(n2)], len_before: len, len_after: n1 };
        let b = ChangeSet { ops: vec![Op::Retain(m0), Op::Delete(m1)], len_before: len, len_after: m0 };

        let a0 = 0..n0;
        let a1 = (n0 + n1)..len;
        let b0 = m0..len;
        let expected = overlap(&a0, &b0) || overlap(&a1, &b0);
        assert_eq!(a.conflicts(&b), expected);
    }

    /// Pure insertion at `n0` vs deletion `0..m0`: verifies the strict
    /// interior rule (`0 < n0 < m0`) vs non-conflicting edge (`n0 == m0`) and
    /// outside (`n0 > m0`).
    #[kani::proof]
    #[kani::unwind(4)]
    fn conflicts_interior_insert_vs_delete_matches_oracle() {
        let n0: usize = kani::any();
        let n1: usize = kani::any();
        let m0: usize = kani::any();
        let m1: usize = kani::any();
        kani::assume(n0 >= 1 && n0 <= 6 && n1 >= 1 && n1 <= 6);
        kani::assume(m0 >= 1 && m0 <= 10 && m1 >= 1 && m1 <= 10);
        let len = n0 + n1;
        kani::assume(m0 + m1 == len);

        let a = ChangeSet { ops: vec![Op::Retain(n0), Op::Insert(cap_string("a")), Op::Retain(n1)], len_before: len, len_after: len + 1 };
        let b = ChangeSet { ops: vec![Op::Delete(m0), Op::Retain(m1)], len_before: len, len_after: m1 };

        let expected = n0 < m0;
        assert_eq!(a.conflicts(&b), expected);
    }
}
