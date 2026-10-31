//! A document's changes as a sequence of single replacements, each in
//! the text the ones before it left: what an incremental parser and a
//! language server's incremental sync both take.

use techne_text::{ChangeSet, Document, Revision, ropey::Rope};

/// One replacement of `[start, end)` by `text`, in the text the
/// replacements before it left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step<'a> {
    pub start: usize,
    pub end: usize,
    pub text: &'a str,
}

/// The replacements of `changes`, in order.
pub fn steps(changes: &ChangeSet) -> impl Iterator<Item = Step<'_>> {
    let mut delta = 0isize;
    changes.edits().map(move |(r, text)| {
        let step = Step { start: r.start.wrapping_add_signed(delta), end: r.end.wrapping_add_signed(delta), text };
        delta += text.len() as isize - (r.end - r.start) as isize;
        step
    })
}

/// Apply one replacement to `rope`.
pub fn apply(rope: &mut Rope, step: &Step<'_>) {
    let (a, b) = (rope.byte_to_char(step.start), rope.byte_to_char(step.end));
    rope.remove(a..b);
    rope.insert(a, step.text);
}

/// The change sets that took `doc` from `revision` to now, oldest first;
/// none when that revision is no longer in its history.
pub fn since(doc: &Document, revision: Revision) -> Option<Vec<ChangeSet>> {
    if revision == doc.revision() {
        return Some(Vec::new());
    }
    doc.entries_since(revision).map(|es| es.iter().map(|e| e.changes.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_replay_a_change() {
        let before = Rope::from_str("one two three");
        let changes = ChangeSet::from_edits(before.len_bytes(), [(0..3, "1"), (4..7, "deux"), (13..13, "!")]).unwrap();
        let mut after = before.clone();
        changes.apply(&mut after);
        let mut replayed = before.clone();
        for step in steps(&changes) {
            apply(&mut replayed, &step);
        }
        assert_eq!(replayed, after);
        assert_eq!(steps(&changes).map(|s| (s.start, s.end)).collect::<Vec<_>>(), [(0, 3), (2, 5), (12, 12)]);
    }
}
