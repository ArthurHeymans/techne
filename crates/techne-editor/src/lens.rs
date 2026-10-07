//! Lenses (EDITOR.md, sections 1 and 3; PLAN.md, "Lenses are views"):
//! presentations whose runs include excerpts of documents, edited through
//! to them.
//!
//! Each excerpt remembers the range of its document it showed, and the
//! document's revision then. An edit inside one excerpt becomes an edit of
//! its document at the excerpt's place now; it is refused, with the reason,
//! when the document changed in that range since (the lens shows text that
//! is no longer there), when it touches text of the presentation's own, or
//! when it spans two excerpts. All edits of one change are checked before
//! any document is changed, and each document gets one transaction.
//!
//! The documents own those edits and their history: the lens keeps only
//! which units of which documents each of its changes made. Undo through
//! the lens undoes those units in their documents, refusing when you have
//! changed a document since (undo there first), and the excerpts show the
//! result.

use std::{cell::RefCell, ops::Range, rc::Rc};

use techne_text::{Actor, Assoc, ChangeSet, Document, Group, Revision};

use crate::presentation::{Presentation, Source};

/// A change made through a lens: the documents it changed, each with the
/// unit it made there, as `Document::undo_top` names it.
pub(crate) type Op = Vec<(Rc<RefCell<Document>>, Revision)>;

/// The range an excerpt showed, in its document now: refused when an edit
/// since touched it (an insertion at either end too).
pub(crate) fn now(s: &Source) -> Result<Range<usize>, String> {
    let doc = s.doc.borrow();
    let entries = doc.entries_since(s.base).ok_or("the history of its source no longer goes back to the lens")?;
    entries.iter().try_fold(s.range.clone(), |r, entry| {
        if entry.changes.edits().any(|(c, _)| c.start <= r.end && r.start <= c.end) {
            let by = &entry.actor;
            return Err(format!("its source changed since the lens showed it (an edit by {by}); refresh the lens"));
        }
        Ok(entry.changes.map_pos(r.start, Assoc::Before)..entry.changes.map_pos(r.end, Assoc::After))
    })
}

/// The range an excerpt showed, in its document now, whatever changed it.
fn followed(s: &Source) -> Result<Range<usize>, String> {
    let doc = s.doc.borrow();
    let entries = doc.entries_since(s.base).ok_or("the history of a source no longer goes back to the lens")?;
    Ok(entries.iter().fold(s.range.clone(), |r, e| e.changes.map_pos(r.start, Assoc::Before)..e.changes.map_pos(r.end, Assoc::After)))
}

/// An edit of a document: the run it is in (row, run), the run's range in
/// the document now, the range edited and its new text.
struct Through {
    run: (usize, usize),
    doc: Rc<RefCell<Document>>,
    now: Range<usize>,
    range: Range<usize>,
    text: String,
}

fn label(doc: &Rc<RefCell<Document>>) -> String {
    doc.borrow().path().and_then(|p| p.file_name()).map_or("a document".into(), |n| n.to_string_lossy().into_owned())
}

/// The documents of `items`, each once, in order.
fn distinct(items: impl Iterator<Item = Rc<RefCell<Document>>>) -> Vec<Rc<RefCell<Document>>> {
    items.fold(Vec::new(), |mut v, d| {
        if !v.iter().any(|x| Rc::ptr_eq(x, &d)) {
            v.push(d);
        }
        v
    })
}

impl Presentation {
    /// What `changes` of the text are as edits of the excerpts' documents.
    fn through(&self, changes: &ChangeSet) -> Result<Vec<Through>, String> {
        let excerpts: Vec<_> = self.placed().filter(|&(i, j, _)| self.rows[i].runs[j].source.is_some()).collect();
        changes
            .edits()
            .map(|(r, text)| {
                let &(i, j, ref at) =
                    excerpts.iter().find(|(_, _, at)| at.start <= r.start && r.end <= at.end).ok_or("Text is read-only")?;
                let s = self.rows[i].runs[j].source.as_ref().expect("an excerpt");
                let now = now(s).map_err(|why| format!("Refused: {why}"))?;
                let range = now.start + (r.start - at.start)..now.start + (r.end - at.start);
                Ok(Through { run: (i, j), doc: s.doc.clone(), now, range, text: text.to_string() })
            })
            .collect()
    }

    /// Edit the text as `actor`, through to the excerpts' documents: one
    /// transaction for each. `None` when the edits change nothing.
    pub fn edit(
        &mut self,
        actor: &Actor,
        edits: Vec<(Range<usize>, String)>,
        group: Group,
    ) -> Result<Option<(Revision, ChangeSet)>, String> {
        let changes = ChangeSet::from_edits(self.len(), edits).map_err(|e| e.to_string())?;
        if changes.is_identity() {
            return Ok(None);
        }
        let through = self.through(&changes)?;
        let docs = distinct(through.iter().map(|t| t.doc.clone()));
        // Check every transaction before applying any.
        let txs = docs
            .into_iter()
            .map(|d| {
                let edits = through.iter().filter(|t| Rc::ptr_eq(&t.doc, &d)).map(|t| (t.range.clone(), t.text.clone()));
                let mut tx = d.borrow().edit(actor, edits).map_err(|_| "Two excerpts of this lens show the same text".to_string())?;
                tx.group = group;
                Ok((d, tx))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut op = Op::new();
        for (d, tx) in txs {
            let rev = d.borrow_mut().apply(tx).map_err(|e| e.to_string())?;
            let applied = d.borrow().entries_since(rev - 1).expect("just applied")[0].changes.clone();
            for t in through.iter().filter(|t| Rc::ptr_eq(&t.doc, &d)) {
                let (i, j) = t.run;
                let range = applied.map_pos(t.now.start, Assoc::Before)..applied.map_pos(t.now.end, Assoc::After);
                let run = &mut self.rows[i].runs[j];
                run.text = d.borrow().text().byte_slice(range.clone()).to_string();
                run.source = Some(Source { doc: d.clone(), range, base: rev });
            }
            let unit = d.borrow().undo_top(actor).expect("an edit makes a unit");
            op.push((d, unit));
        }
        // Typing joins the change before when it extends the same units.
        let joins = group == Group::Extend
            && self
                .undo
                .last()
                .is_some_and(|last| last.len() == op.len() && last.iter().zip(&op).all(|((a, u), (b, v))| Rc::ptr_eq(a, b) && u == v));
        if !joins {
            self.undo.push(op);
        }
        self.redo.clear();
        self.commit(changes.clone());
        Ok(Some((self.revision(), changes)))
    }

    /// Undo or redo `actor`'s last change made through the lens, in its
    /// documents; the excerpts show the result. Refused, changing nothing,
    /// when a document's last unit is no longer that change's.
    pub fn revert(&mut self, actor: &Actor, undo: bool) -> Result<Revision, String> {
        let top = |d: &Document| if undo { d.undo_top(actor) } else { d.redo_top(actor) };
        let op =
            (if undo { self.undo.last() } else { self.redo.last() }).ok_or(if undo { "Nothing to undo" } else { "Nothing to redo" })?;
        if let Some((d, _)) = op.iter().find(|(d, unit)| top(&d.borrow()) != Some(*unit)) {
            return Err(format!("You changed {} since; {} there first", label(d), if undo { "undo" } else { "redo" }));
        }
        let op = if undo { self.undo.pop() } else { self.redo.pop() }.expect("checked");
        let mut done = Op::new();
        let mut failed = None;
        for (i, (d, _)) in op.iter().enumerate() {
            let result = if undo { d.borrow_mut().undo(actor) } else { d.borrow_mut().redo(actor) };
            match result {
                Ok(_) => {
                    let other = if undo { d.borrow().redo_top(actor) } else { d.borrow().undo_top(actor) };
                    done.push((d.clone(), other.expect("a revert makes a unit")));
                }
                Err(e) => {
                    failed = Some((i, label(d), e.to_string()));
                    // What is left of the change stays to be done.
                    let rest: Op = op[i..].to_vec();
                    if undo {
                        self.undo.push(rest)
                    } else {
                        self.redo.push(rest)
                    }
                    break;
                }
            }
        }
        let docs: Vec<_> = done.iter().map(|(d, _)| d.clone()).collect();
        if !done.is_empty() {
            if undo { self.redo.push(done) } else { self.undo.push(done) }
        }
        let changes = self.follow(Some(&docs))?;
        self.commit(changes);
        match failed {
            None => Ok(self.revision()),
            Some((0, doc, why)) => Err(format!("Refused in {doc}: {why}")),
            Some((_, doc, why)) => Err(format!("Done in part, refused in {doc}: {why}")),
        }
    }

    /// Show the excerpts' documents as they are now, also where they
    /// changed under the lens.
    pub fn refresh(&mut self) -> Result<(), String> {
        let changes = self.follow(None)?;
        self.commit(changes);
        Ok(())
    }

    /// The excerpts of `docs` (all, with `None`) read again from their
    /// documents as they are now: the change of the text that makes.
    fn follow(&mut self, docs: Option<&[Rc<RefCell<Document>>]>) -> Result<ChangeSet, String> {
        let placed: Vec<_> = self.placed().collect();
        let mut edits = Vec::new();
        for (i, j, at) in placed {
            let run = &mut self.rows[i].runs[j];
            let Some(s) = &run.source else { continue };
            if docs.is_some_and(|docs| !docs.iter().any(|d| Rc::ptr_eq(d, &s.doc))) {
                continue;
            }
            let range = followed(s)?;
            let (text, base) = {
                let d = s.doc.borrow();
                (d.text().byte_slice(range.clone()).to_string(), d.revision())
            };
            if text != run.text {
                edits.push((at, text.clone()));
            }
            run.text = text;
            run.source = Some(Source { doc: s.doc.clone(), range, base });
        }
        Ok(self.change(edits))
    }

    /// The excerpts' documents, each once.
    pub fn sources(&self) -> Vec<Rc<RefCell<Document>>> {
        distinct(self.rows.iter().flat_map(|r| r.runs.iter().filter_map(|run| run.source.as_ref().map(|s| s.doc.clone()))))
    }

    /// Where `pos` is in an excerpt's document now: in the excerpt at
    /// `pos`, else the first of its row (for a position in its label).
    pub fn source_at(&self, pos: usize) -> Option<(Rc<RefCell<Document>>, usize)> {
        let placed: Vec<_> = self.placed().filter(|&(i, j, _)| self.rows[i].runs[j].source.is_some()).collect();
        let row = self.key_at(pos)?;
        let (i, j, at) = placed
            .iter()
            .find(|(_, _, at)| at.start <= pos && pos <= at.end)
            .or_else(|| placed.iter().find(|(i, _, _)| self.rows[*i].key == row))?
            .clone();
        let s = self.rows[i].runs[j].source.as_ref()?;
        let range = followed(s).ok()?;
        Some((s.doc.clone(), (range.start + pos.saturating_sub(at.start)).min(range.end)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::{Content, RowSpec, RunSpec};

    fn doc(text: &str) -> Rc<RefCell<Document>> {
        Rc::new(RefCell::new(Document::new(text)))
    }

    fn text(d: &Rc<RefCell<Document>>) -> String {
        d.borrow().text().to_string()
    }

    fn label(t: &str) -> RunSpec {
        RunSpec { content: Content::Text(t.into()), face: Some("comment".into()) }
    }

    fn excerpt(d: &Rc<RefCell<Document>>, r: Range<usize>) -> RunSpec {
        RunSpec { content: Content::Excerpt(d.clone(), r), face: None }
    }

    /// A lens of the first lines of two documents and the second of the
    /// first: "a:1: one\nb:1: uno\na:2: two".
    fn lens(a: &Rc<RefCell<Document>>, b: &Rc<RefCell<Document>>) -> Presentation {
        let row = |key: &str, l: &str, d: &Rc<RefCell<Document>>, r: Range<usize>| RowSpec {
            key: key.into(),
            columns: vec![vec![label(l), excerpt(d, r)]],
        };
        let mut p = Presentation::new();
        p.set_rows(vec![row("a1", "a:1: ", a, 0..3), row("b1", "b:1: ", b, 0..3), row("a2", "a:2: ", a, 4..7)]).unwrap();
        p
    }

    fn user() -> Actor {
        "user".into()
    }

    fn shown(p: &Presentation) -> String {
        p.text().to_string()
    }

    #[test]
    fn edits_go_through_to_the_sources() {
        let (a, b) = (doc("one\ntwo\n"), doc("uno\n"));
        let mut l = lens(&a, &b);
        assert_eq!(shown(&l), "a:1: one\nb:1: uno\na:2: two");
        // One change in three excerpts of two documents.
        let edits = vec![(5..8, "ONE".to_string()), (14..14, "x".to_string()), (26..26, "!".to_string())];
        l.edit(&user(), edits, Group::New).unwrap();
        assert_eq!((text(&a), text(&b)), ("ONE\ntwo!\n".to_string(), "xuno\n".to_string()));
        assert_eq!(shown(&l), "a:1: ONE\nb:1: xuno\na:2: two!");
        assert_eq!(l.source_at(14).map(|(_, p)| p), Some(0), "the insertion at its start is in the excerpt");
        // Undone in the sources, as the sources' own units.
        l.revert(&user(), true).unwrap();
        assert_eq!((text(&a), text(&b)), ("one\ntwo\n".to_string(), "uno\n".to_string()));
        assert_eq!(shown(&l), "a:1: one\nb:1: uno\na:2: two");
        l.revert(&user(), false).unwrap();
        assert_eq!(text(&a), "ONE\ntwo!\n");
        // The source's own undo takes it back too: it is the source's.
        a.borrow_mut().undo(&user()).unwrap();
        assert_eq!(text(&a), "one\ntwo\n");
    }

    #[test]
    fn undo_through_the_lens_waits_for_the_source() {
        let (a, b) = (doc("one\ntwo\n"), doc("uno\n"));
        let mut l = lens(&a, &b);
        l.edit(&user(), vec![(8..8, "!".to_string())], Group::New).unwrap();
        // You change a directly after.
        let tx = a.borrow().edit(&user(), [(0..0, ">")]).unwrap();
        a.borrow_mut().apply(tx).unwrap();
        let why = l.revert(&user(), true).unwrap_err();
        assert!(why.contains("changed a document since"), "{why}");
        assert_eq!(text(&a), ">one!\ntwo\n", "nothing was undone");
        a.borrow_mut().undo(&user()).unwrap();
        l.revert(&user(), true).unwrap();
        assert_eq!(text(&a), "one\ntwo\n");
    }

    #[test]
    fn typing_joins_one_change() {
        let (a, b) = (doc("one\ntwo\n"), doc("uno\n"));
        let mut l = lens(&a, &b);
        for (i, c) in "abc".chars().enumerate() {
            let group = if i == 0 { Group::New } else { Group::Extend };
            l.edit(&user(), vec![(8 + i..8 + i, c.to_string())], group).unwrap();
        }
        assert_eq!(text(&a), "oneabc\ntwo\n");
        l.revert(&user(), true).unwrap();
        assert_eq!(text(&a), "one\ntwo\n");
        assert!(l.revert(&user(), true).is_err());
    }

    #[test]
    fn what_cannot_be_edited_is_refused() {
        let (a, b) = (doc("one\ntwo\n"), doc("uno\n"));
        let mut l = lens(&a, &b);
        let refused = |l: &mut Presentation, r: Range<usize>, t: &str| l.edit(&user(), vec![(r, t.to_string())], Group::New).unwrap_err();
        assert_eq!(refused(&mut l, 0..1, ""), "Text is read-only", "the lens's own text");
        assert_eq!(refused(&mut l, 7..15, ""), "Text is read-only", "across excerpts");
        // Another actor changes the second line of a under the lens.
        let other: Actor = "agent".into();
        let tx = a.borrow().edit(&other, [(5..5, "w")]).unwrap();
        a.borrow_mut().apply(tx).unwrap();
        assert!(l.highlights(0, l.len()).iter().any(|h| (h.from, h.face.as_str()) == (23, "warning")));
        let why = refused(&mut l, 24..24, "X");
        assert!(why.contains("changed since") && why.contains("agent"), "{why}");
        assert_eq!(text(&a), "one\ntwwo\n", "nothing was applied");
        // The first line was not touched: it is edited where it is now.
        l.edit(&user(), vec![(8..8, "!".to_string())], Group::New).unwrap();
        assert_eq!(text(&a), "one!\ntwwo\n");
        // A refresh shows the change, and the excerpt can be edited again.
        l.refresh().unwrap();
        assert_eq!(shown(&l), "a:1: one!\nb:1: uno\na:2: twwo");
        l.edit(&user(), vec![(25..27, "w".to_string())], Group::New).unwrap();
        assert_eq!(text(&a), "one!\ntwo\n");
    }

    #[test]
    fn excerpts_of_the_same_text_are_not_edited_twice() {
        let a = doc("same\n");
        let mut l = Presentation::new();
        let row = |key: &str| RowSpec { key: key.into(), columns: vec![vec![excerpt(&a, 0..4)]] };
        l.set_rows(vec![row("1"), row("2")]).unwrap();
        l.edit(&user(), vec![(0..0, "x".to_string())], Group::New).unwrap();
        assert_eq!(text(&a), "xsame\n");
        // The other excerpt showed the text just changed.
        assert!(l.edit(&user(), vec![(6..6, "y".to_string())], Group::New).is_err());
    }
}
