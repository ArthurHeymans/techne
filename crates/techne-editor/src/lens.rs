//! Lenses (EDITOR.md, sections 1 and 3; PLAN.md, "Lenses are views"): a
//! document made of generated text and excerpts of other documents. Its
//! text is a document of its own, so views, motions and search work on it
//! as on any; an edit of it is made through to the sources.
//!
//! Each excerpt remembers the range of its source it showed, and the
//! source's revision then. An edit inside one excerpt becomes an edit of
//! its source at the excerpt's place now; it is refused, with the reason,
//! when the source changed in that range since (the lens shows text that
//! is no longer there), when it touches generated text, or when it spans
//! two excerpts. All edits of one change are checked before any source is
//! changed, and each source gets one transaction.
//!
//! A lens of generated text only is a read-only view.

use std::{cell::RefCell, ops::Range, rc::Rc};

use techne_text::{Actor, Assoc, ChangeSet, Document, Group, Revision};

pub enum Item {
    /// Text of the lens's own, not editable.
    Text(String),
    /// The text of `range` of a document, edited through to it.
    Excerpt(Rc<RefCell<Document>>, Range<usize>),
}

struct Excerpt {
    /// Where it is in the lens's text.
    at: Range<usize>,
    source: Rc<RefCell<Document>>,
    /// What it showed of its source, at `base`.
    from: Range<usize>,
    base: Revision,
}

pub struct Lens {
    doc: Rc<RefCell<Document>>,
    excerpts: Vec<Excerpt>,
}

/// An edit of a source: the excerpt it is in and the excerpt's range in
/// its source now, the range edited and its new text.
struct Through {
    excerpt: usize,
    source: Rc<RefCell<Document>>,
    now: Range<usize>,
    range: Range<usize>,
    text: String,
}

impl Lens {
    pub fn new(items: Vec<Item>) -> Result<Lens, String> {
        let (text, excerpts) = items.into_iter().try_fold((String::new(), Vec::new()), |(mut text, mut excerpts), item| {
            match item {
                Item::Text(t) => text.push_str(&t),
                Item::Excerpt(source, from) => {
                    let (piece, base) = {
                        let d = source.borrow();
                        let t = d.text();
                        if from.start > from.end || from.end > t.len_bytes() {
                            return Err(format!("make-lens: {}..{} is not in the document", from.start, from.end));
                        }
                        (t.byte_slice(from.clone()).to_string(), d.revision())
                    };
                    let at = text.len()..text.len() + piece.len();
                    text.push_str(&piece);
                    excerpts.push(Excerpt { at, source, from, base });
                }
            }
            Ok((text, excerpts))
        })?;
        Ok(Lens { doc: Rc::new(RefCell::new(Document::new(&text))), excerpts })
    }

    pub fn document(&self) -> &Rc<RefCell<Document>> {
        &self.doc
    }

    /// The excerpt at a position of the lens's text; its ends are in it.
    pub fn excerpt_at(&self, pos: usize) -> Option<usize> {
        self.excerpts.iter().position(|e| e.at.start <= pos && pos <= e.at.end)
    }

    /// The excerpts' ranges in the lens's text.
    pub fn excerpt_ranges(&self) -> Vec<Range<usize>> {
        self.excerpts.iter().map(|e| e.at.clone()).collect()
    }

    /// The source of an excerpt and the range it shows now, or why it
    /// cannot say: the source changed there since.
    pub fn source(&self, i: usize) -> Result<(Rc<RefCell<Document>>, Range<usize>), String> {
        let e = self.excerpts.get(i).ok_or("no such excerpt")?;
        Ok((e.source.clone(), self.now(e)?))
    }

    /// The excerpts whose source changed in their range since.
    pub fn stale(&self) -> Vec<usize> {
        (0..self.excerpts.len()).filter(|&i| self.now(&self.excerpts[i]).is_err()).collect()
    }

    /// The range an excerpt showed, in its source now: refused when an edit
    /// since touched it (an insertion at either end too).
    fn now(&self, e: &Excerpt) -> Result<Range<usize>, String> {
        let source = e.source.borrow();
        let entries = source.entries_since(e.base).ok_or("the history of its source no longer goes back to the lens")?;
        entries.iter().try_fold(e.from.clone(), |r, entry| {
            if entry.changes.edits().any(|(c, _)| c.start <= r.end && r.start <= c.end) {
                let by = &entry.actor;
                return Err(format!("its source changed since the lens showed it (an edit by {by}); refresh the lens"));
            }
            Ok(entry.changes.map_pos(r.start, Assoc::Before)..entry.changes.map_pos(r.end, Assoc::After))
        })
    }

    /// What `changes` of the lens's text are as edits of the sources.
    fn through(&self, changes: &ChangeSet) -> Result<Vec<Through>, String> {
        changes
            .edits()
            .map(|(r, text)| {
                let i = self.excerpts.iter().position(|e| e.at.start <= r.start && r.end <= e.at.end).ok_or_else(|| {
                    if self.excerpts.iter().any(|e| r.start < e.at.end && e.at.start < r.end) {
                        // Across excerpts: through the text between them.
                        "Text is read-only".to_string()
                    } else {
                        "Text is read-only".to_string()
                    }
                })?;
                let e = &self.excerpts[i];
                let now = self.now(e).map_err(|why| format!("Refused: {why}"))?;
                let range = now.start + (r.start - e.at.start)..now.start + (r.end - e.at.start);
                Ok(Through { excerpt: i, source: e.source.clone(), now, range, text: text.to_string() })
            })
            .collect()
    }

    /// Make `changes` of the lens's text in the sources: one transaction
    /// for each. Excerpts edited show their sources as of after it.
    fn apply_through(&mut self, actor: &Actor, changes: &ChangeSet, group: Group) -> Result<(), String> {
        let through = self.through(changes)?;
        let sources = through.iter().map(|t| t.source.clone()).fold(Vec::<Rc<RefCell<Document>>>::new(), |mut v, s| {
            if !v.iter().any(|d| Rc::ptr_eq(d, &s)) {
                v.push(s);
            }
            v
        });
        // Check every transaction before applying any.
        let txs = sources
            .into_iter()
            .map(|s| {
                let edits = through.iter().filter(|t| Rc::ptr_eq(&t.source, &s)).map(|t| (t.range.clone(), t.text.clone()));
                let mut tx = s.borrow().edit(actor, edits).map_err(|_| "Two excerpts of this lens show the same text".to_string())?;
                tx.group = group;
                Ok((s, tx))
            })
            .collect::<Result<Vec<_>, String>>()?;
        for (s, tx) in txs {
            let rev = s.borrow_mut().apply(tx).map_err(|e| e.to_string())?;
            let applied = s.borrow().entries_since(rev - 1).expect("just applied")[0].changes.clone();
            for t in through.iter().filter(|t| Rc::ptr_eq(&t.source, &s)) {
                let e = &mut self.excerpts[t.excerpt];
                e.from = applied.map_pos(t.now.start, Assoc::Before)..applied.map_pos(t.now.end, Assoc::After);
                e.base = rev;
            }
        }
        Ok(())
    }

    /// The excerpts move with a change of the lens's text; text inserted at
    /// an excerpt's ends goes into it.
    fn follow(&mut self, changes: &ChangeSet) {
        for e in &mut self.excerpts {
            e.at = changes.map_pos(e.at.start, Assoc::Before)..changes.map_pos(e.at.end, Assoc::After);
        }
    }

    /// Edit the lens's text as `actor`, through to the sources. `None`
    /// when the edits change nothing.
    pub fn edit(
        &mut self,
        actor: &Actor,
        edits: Vec<(Range<usize>, String)>,
        group: Group,
    ) -> Result<Option<(Revision, ChangeSet)>, String> {
        let mut tx = self.doc.borrow().edit(actor, edits).map_err(|e| e.to_string())?;
        if tx.changes.is_identity() {
            return Ok(None);
        }
        tx.group = group;
        let changes = tx.changes.clone();
        self.apply_through(actor, &changes, group)?;
        let rev = self.doc.borrow_mut().apply(tx).map_err(|e| e.to_string())?;
        self.follow(&changes);
        Ok(Some((rev, changes)))
    }

    /// Undo or redo `actor`'s last change of the lens, through to the
    /// sources; nothing changes when a source refuses.
    pub fn revert(&mut self, actor: &Actor, undo: bool) -> Result<Revision, String> {
        let revert = |d: &mut Document, undo: bool| if undo { d.undo(actor) } else { d.redo(actor) };
        let rev = revert(&mut self.doc.borrow_mut(), undo).map_err(|e| e.to_string())?;
        let changes = self.doc.borrow().entries_since(rev - 1).expect("just applied")[0].changes.clone();
        if let Err(e) = self.apply_through(actor, &changes, Group::New) {
            revert(&mut self.doc.borrow_mut(), !undo).map_err(|e| e.to_string())?;
            return Err(e);
        }
        self.follow(&changes);
        Ok(rev)
    }

    /// Insert text of the lens's own at `pos`, not inside an excerpt: an
    /// excerpt starting there comes after it. Made as the lens (a REPL adds
    /// its transcript before its input so).
    pub fn insert_text(&mut self, pos: usize, text: &str) -> Result<(), String> {
        if pos > self.doc.borrow().len() || self.excerpts.iter().any(|e| e.at.start < pos && pos < e.at.end) {
            return Err(format!("lens-insert-text!: {pos} is not between excerpts"));
        }
        let actor: Actor = "lens".into();
        let tx = self.doc.borrow().edit(&actor, [(pos..pos, text)]).map_err(|e| e.to_string())?;
        self.doc.borrow_mut().apply(tx).map_err(|e| e.to_string())?;
        for e in self.excerpts.iter_mut().filter(|e| e.at.start >= pos) {
            e.at = e.at.start + text.len()..e.at.end + text.len();
        }
        Ok(())
    }

    /// Show the sources as they are now: each excerpt's text is replaced by
    /// what its range holds now, also where it changed.
    pub fn refresh(&mut self) -> Result<(), String> {
        let current: Vec<(Range<usize>, String, Revision)> = self
            .excerpts
            .iter()
            .map(|e| {
                let source = e.source.borrow();
                let entries = source.entries_since(e.base).ok_or("the history of a source no longer goes back to the lens")?;
                let r = entries.iter().fold(e.from.clone(), |r, entry| {
                    entry.changes.map_pos(r.start, Assoc::Before)..entry.changes.map_pos(r.end, Assoc::After)
                });
                Ok((r.clone(), source.text().byte_slice(r).to_string(), source.revision()))
            })
            .collect::<Result<_, String>>()?;
        let edits: Vec<(Range<usize>, String)> = {
            let doc = self.doc.borrow();
            let text = doc.text();
            self.excerpts
                .iter()
                .zip(&current)
                .filter(|(e, (_, t, _))| text.byte_slice(e.at.clone()) != t.as_str())
                .map(|(e, (_, t, _))| (e.at.clone(), t.clone()))
                .collect()
        };
        let actor: Actor = "lens".into();
        let tx = self.doc.borrow().edit(&actor, edits).map_err(|e| e.to_string())?;
        let changes = tx.changes.clone();
        self.doc.borrow_mut().apply(tx).map_err(|e| e.to_string())?;
        // Excerpts follow the replacement of their own text whole.
        let lens_len = self.doc.borrow().len();
        for (e, (from, t, base)) in self.excerpts.iter_mut().zip(current) {
            let start = changes.map_pos(e.at.start, Assoc::Before).min(lens_len);
            e.at = start..start + t.len();
            (e.from, e.base) = (from, base);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Rc<RefCell<Document>> {
        Rc::new(RefCell::new(Document::new(text)))
    }

    fn text(d: &Rc<RefCell<Document>>) -> String {
        d.borrow().text().to_string()
    }

    /// A lens of the first lines of two documents and the second of the
    /// first: "a:1: one\nb:1: uno\na:2: two\n".
    fn lens(a: &Rc<RefCell<Document>>, b: &Rc<RefCell<Document>>) -> Lens {
        let items = vec![
            Item::Text("a:1: ".into()),
            Item::Excerpt(a.clone(), 0..3),
            Item::Text("\nb:1: ".into()),
            Item::Excerpt(b.clone(), 0..3),
            Item::Text("\na:2: ".into()),
            Item::Excerpt(a.clone(), 4..7),
            Item::Text("\n".into()),
        ];
        Lens::new(items).unwrap()
    }

    fn user() -> Actor {
        "user".into()
    }

    #[test]
    fn edits_go_through_to_the_sources() {
        let (a, b) = (doc("one\ntwo\n"), doc("uno\n"));
        let mut l = lens(&a, &b);
        assert_eq!(text(l.document()), "a:1: one\nb:1: uno\na:2: two\n");
        // One change in three excerpts of two documents.
        let edits = vec![(5..8, "ONE".to_string()), (14..14, "x".to_string()), (26..26, "!".to_string())];
        l.edit(&user(), edits, Group::New).unwrap();
        assert_eq!((text(&a), text(&b)), ("ONE\ntwo!\n".to_string(), "xuno\n".to_string()));
        assert_eq!(text(l.document()), "a:1: ONE\nb:1: xuno\na:2: two!\n");
        assert_eq!(l.source(1).unwrap().1, 0..4, "the insertion at its start is in the excerpt");
        // Undone through the lens too.
        l.revert(&user(), true).unwrap();
        assert_eq!((text(&a), text(&b)), ("one\ntwo\n".to_string(), "uno\n".to_string()));
        assert_eq!(text(l.document()), "a:1: one\nb:1: uno\na:2: two\n");
    }

    #[test]
    fn what_cannot_be_edited_is_refused() {
        let (a, b) = (doc("one\ntwo\n"), doc("uno\n"));
        let mut l = lens(&a, &b);
        let refused = |l: &mut Lens, r: Range<usize>, t: &str| l.edit(&user(), vec![(r, t.to_string())], Group::New).unwrap_err();
        assert_eq!(refused(&mut l, 0..1, ""), "Text is read-only", "the lens's own text");
        assert_eq!(refused(&mut l, 7..15, ""), "Text is read-only", "across excerpts");
        // Another actor changes the second line of a under the lens.
        let other: Actor = "agent".into();
        let tx = a.borrow().edit(&other, [(5..5, "w")]).unwrap();
        a.borrow_mut().apply(tx).unwrap();
        assert_eq!(l.stale(), [2]);
        let why = refused(&mut l, 24..24, "X");
        assert!(why.contains("changed since") && why.contains("agent"), "{why}");
        assert_eq!(text(&a), "one\ntwwo\n", "nothing was applied");
        // The first line was not touched: it is edited where it is now.
        l.edit(&user(), vec![(8..8, "!".to_string())], Group::New).unwrap();
        assert_eq!(text(&a), "one!\ntwwo\n");
        // A refresh shows the change, and the excerpt can be edited again.
        l.refresh().unwrap();
        assert_eq!(text(l.document()), "a:1: one!\nb:1: uno\na:2: twwo\n");
        assert!(l.stale().is_empty());
        l.edit(&user(), vec![(25..27, "w".to_string())], Group::New).unwrap();
        assert_eq!(text(&a), "one!\ntwo\n");
    }

    #[test]
    fn text_of_its_own_goes_between_excerpts() {
        let (a, b) = (doc("one\ntwo\n"), doc("uno\n"));
        let mut l = lens(&a, &b);
        l.insert_text(9, ">> ").unwrap();
        assert_eq!(text(l.document()), "a:1: one\n>> b:1: uno\na:2: two\n");
        assert!(l.insert_text(6, "x").is_err(), "not inside an excerpt");
        // The excerpts after it moved.
        l.edit(&user(), vec![(17..17, "!".to_string())], Group::New).unwrap();
        assert_eq!(text(&b), "!uno\n");
    }

    #[test]
    fn excerpts_of_the_same_text_are_not_edited_twice() {
        let a = doc("same\n");
        let items = vec![Item::Excerpt(a.clone(), 0..4), Item::Text("\n".into()), Item::Excerpt(a.clone(), 0..4)];
        let mut l = Lens::new(items).unwrap();
        l.edit(&user(), vec![(0..0, "x".to_string())], Group::New).unwrap();
        assert_eq!(text(&a), "xsame\n");
        // The other excerpt showed the text just changed.
        assert_eq!(l.stale(), [1]);
        assert!(l.edit(&user(), vec![(6..6, "y".to_string())], Group::New).is_err());
    }
}
