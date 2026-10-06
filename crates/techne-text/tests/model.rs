//! Differential tests: changes, anchors and documents against a plain
//! `String` model that applies one edit at a time.

use std::{ops::Range, sync::Arc};

use proptest::prelude::*;
use ropey::Rope;
use techne_text::{Actor, Assoc, ChangeSet, Document, Group, UndoError};

type Edits = Vec<(Range<usize>, String)>;
type Seeds = Vec<(u16, u16, String)>;

fn boundaries(s: &str) -> Vec<usize> {
    s.char_indices().map(|(i, _)| i).chain([s.len()]).collect()
}

/// Sorted edits at character boundaries with gaps between them (adjacent
/// edits are one edit to a change set, so the model would disagree on
/// positions at their seam).
fn edits_for(text: &str, seeds: &Seeds) -> Edits {
    let b = boundaries(text);
    let mut ranges: Vec<(Range<usize>, String)> = seeds
        .iter()
        .map(|(x, y, t)| {
            let (i, j) = (*x as usize % b.len(), *y as usize % b.len());
            (b[i.min(j)]..b[i.max(j)], t.clone())
        })
        .collect();
    ranges.sort_by_key(|(r, _)| (r.start, r.end));
    let mut out: Edits = Vec::new();
    for (r, t) in ranges {
        if out.last().is_none_or(|(p, _)| r.start > p.end) {
            out.push((r, t));
        }
    }
    out
}

fn model_apply(text: &str, edits: &Edits) -> String {
    let mut s = text.to_string();
    for (r, t) in edits.iter().rev() {
        s.replace_range(r.clone(), t);
    }
    s
}

/// One edit at a time, right to left, so earlier ranges stay valid.
fn model_map(edits: &[(Range<usize>, String)], mut p: usize, assoc: Assoc) -> (usize, bool) {
    let mut deleted = false;
    for (r, t) in edits.iter().rev() {
        let (s, e, n) = (r.start, r.end, t.len());
        p = if p < s {
            p
        } else if p == s {
            if assoc == Assoc::Before { s } else { s + n }
        } else if p < e {
            deleted = true;
            s + n
        } else {
            p - (e - s) + n
        };
    }
    (p, deleted)
}

fn apply(text: &str, c: &ChangeSet) -> String {
    let mut r = Rope::from_str(text);
    c.apply(&mut r);
    r.to_string()
}

fn text_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(vec!['a', 'b', ' ', 'é', '\n', '漢']), 0..40).prop_map(String::from_iter)
}

fn seeds_strategy() -> impl Strategy<Value = Seeds> {
    prop::collection::vec((any::<u16>(), any::<u16>(), "[xé\n]{0,3}"), 0..4)
}

proptest! {
    #[test]
    fn changes_agree_with_the_model(text in text_strategy(), s1 in seeds_strategy(), s2 in seeds_strategy()) {
        let e1 = edits_for(&text, &s1);
        let c1 = ChangeSet::from_edits(text.len(), e1.clone()).unwrap();
        let after1 = apply(&text, &c1);
        prop_assert_eq!(&after1, &model_apply(&text, &e1));
        prop_assert_eq!(apply(&after1, &c1.invert(&Rope::from_str(&text))), text.clone());

        for p in boundaries(&text) {
            for assoc in [Assoc::Before, Assoc::After] {
                prop_assert_eq!(c1.map(p, assoc), model_map(&e1, p, assoc), "position {} {:?}", p, assoc);
            }
        }

        let e2 = edits_for(&after1, &s2);
        let c2 = ChangeSet::from_edits(after1.len(), e2.clone()).unwrap();
        let after2 = model_apply(&after1, &e2);
        prop_assert_eq!(apply(&text, &c1.compose(&c2)), after2);
    }

    #[test]
    fn concurrent_changes_commute(text in text_strategy(), s1 in seeds_strategy(), s2 in seeds_strategy()) {
        let a = ChangeSet::from_edits(text.len(), edits_for(&text, &s1)).unwrap();
        let b = ChangeSet::from_edits(text.len(), edits_for(&text, &s2)).unwrap();
        match (a.transform(&b, Assoc::After), b.transform(&a, Assoc::After)) {
            (Ok(a2), Ok(b2)) => {
                // Insertions at one position are ordered by who came first;
                // without such ties both orders give the same text.
                let starts = |c: &ChangeSet| c.edits().map(|(r, _)| r.start).collect::<Vec<_>>();
                let tie = starts(&a).iter().any(|p| starts(&b).contains(p));
                if !tie {
                    prop_assert_eq!(apply(&apply(&text, &b), &a2), apply(&apply(&text, &a), &b2));
                }
            }
            (Err(_), Err(_)) => prop_assert!(a.conflicts(&b) && b.conflicts(&a)),
            _ => prop_assert!(false, "conflict detection is not symmetric"),
        }
    }
}

#[derive(Clone, Debug)]
enum Step {
    Edit(u8, Seeds, bool),
    Undo(u8),
    Redo(u8),
}

fn steps_strategy() -> impl Strategy<Value = Vec<Step>> {
    let step = prop_oneof![
        3 => (0..2u8, seeds_strategy(), any::<bool>()).prop_map(|(a, s, g)| Step::Edit(a, s, g)),
        1 => (0..2u8).prop_map(Step::Undo),
        1 => (0..2u8).prop_map(Step::Redo),
    ];
    prop::collection::vec(step, 0..30)
}

fn actors() -> [Actor; 2] {
    [Arc::from("human"), Arc::from("agent")]
}

fn revert(doc: &mut Document, actor: &Actor, undo: bool) -> Result<u64, UndoError> {
    if undo { doc.undo(actor) } else { doc.redo(actor) }
}

/// Run the steps, checking each against the model.
fn run(doc: &mut Document, steps: &[Step]) -> Result<(), TestCaseError> {
    let actors = actors();
    for step in steps {
        let (text, rev) = (doc.text().to_string(), doc.revision());
        match step {
            Step::Edit(a, seeds, extend) => {
                let edits = edits_for(&text, seeds);
                let mut tx = doc.edit(&actors[*a as usize], edits.clone()).unwrap();
                tx.group = if *extend { Group::Extend } else { Group::New };
                doc.apply(tx).unwrap();
                prop_assert_eq!(doc.text().to_string(), model_apply(&text, &edits));
            }
            Step::Undo(a) | Step::Redo(a) => {
                let actor = &actors[*a as usize];
                let undo = matches!(step, Step::Undo(_));
                let result = revert(doc, actor, undo);
                match result {
                    Ok(_) => {
                        // Nothing intervened, so reversing it again succeeds
                        // and restores the text exactly.
                        let undone = doc.text().to_string();
                        revert(doc, actor, !undo).unwrap();
                        prop_assert_eq!(doc.text().to_string(), text);
                        revert(doc, actor, undo).unwrap();
                        prop_assert_eq!(doc.text().to_string(), undone);
                    }
                    Err(UndoError::Nothing | UndoError::Conflict { .. }) => {
                        prop_assert_eq!(doc.text().to_string(), text);
                        prop_assert_eq!(doc.revision(), rev);
                    }
                    Err(e) => return Err(TestCaseError::fail(e.to_string())),
                }
            }
        }
    }
    Ok(())
}

proptest! {
    #[test]
    fn documents_agree_with_the_model(text in text_strategy(), steps in steps_strategy()) {
        let mut doc = Document::new(&text);
        run(&mut doc, &steps)?;

        // Anchors from the start map like the model through every entry.
        let entries: Vec<Edits> = doc
            .entries_since(0)
            .unwrap()
            .iter()
            .map(|e| e.changes.edits().map(|(r, t)| (r, t.to_string())).collect())
            .collect();
        for p in boundaries(&text) {
            for assoc in [Assoc::Before, Assoc::After] {
                let model = entries.iter().fold((p, false), |(p, d), e| {
                    let (q, d2) = model_map(e, p, assoc);
                    (q, d || d2)
                });
                prop_assert_eq!(doc.map_pos(p, assoc, 0), Some(model));
            }
        }
    }

    #[test]
    fn one_actor_can_undo_everything(text in text_strategy(), steps in steps_strategy()) {
        let steps: Vec<Step> = steps
            .into_iter()
            .map(|s| match s {
                Step::Edit(_, seeds, g) => Step::Edit(0, seeds, g),
                Step::Undo(_) => Step::Undo(0),
                Step::Redo(_) => Step::Redo(0),
            })
            .collect();
        let mut doc = Document::new(&text);
        run(&mut doc, &steps)?;
        let human = &actors()[0];
        let end = doc.text().to_string();
        let mut undone = 0;
        while doc.undo(human).is_ok() {
            undone += 1;
        }
        prop_assert_eq!(doc.text().to_string(), text);
        for _ in 0..undone {
            doc.redo(human).unwrap();
        }
        prop_assert_eq!(doc.text().to_string(), end);
    }

    #[test]
    fn the_journal_recovers_text_and_undo(text in text_strategy(), steps in steps_strategy(), save_at in any::<prop::sample::Index>()) {
        let dir = tempfile::tempdir().unwrap();
        let (path, journal) = (dir.path().join("f.txt"), dir.path().join("f.journal"));
        std::fs::write(&path, &text).unwrap();
        let (mut doc, _) = Document::open(&path, &journal).unwrap();
        let cut = save_at.index(steps.len() + 1);
        run(&mut doc, &steps[..cut])?;
        doc.save().unwrap();
        run(&mut doc, &steps[cut..])?;
        let (text_before, rev, dirty) = (doc.text().to_string(), doc.revision(), doc.is_dirty());

        // Drop it as a crash would: no save.
        drop(doc);
        let (mut back, _) = Document::open(&path, &journal).unwrap();
        prop_assert_eq!(back.text().to_string(), text_before.clone());
        prop_assert_eq!(back.revision(), rev);
        prop_assert_eq!(back.is_dirty(), dirty);

        // Unsaved undo units survive, and recovery is repeatable.
        let mut redone = Vec::new();
        for actor in &actors() {
            while back.undo(actor).is_ok() {
                redone.push(actor.clone());
            }
        }
        let undone_text = back.text().to_string();
        drop(back);
        let (mut again, _) = Document::open(&path, &journal).unwrap();
        prop_assert_eq!(again.text().to_string(), undone_text);
        for actor in redone.iter().rev() {
            again.redo(actor).unwrap();
        }
        prop_assert_eq!(again.text().to_string(), text_before);
    }
}
