//! A text document: the authoritative text, its revision history and undo.
//!
//! Every change is a revision-checked transaction made by an actor. A
//! transaction must name the revision it was made against; one made against
//! an older revision is refused and can be rebased explicitly. Undo reverses
//! the actor's own last unit of transactions, moved past later edits by
//! anyone, and refuses with the actors involved when those edits touched the
//! same text. With a journal, a transaction is recorded before it is applied.
//!
//! The history is kept whole for now; bounding it, and with that forgetting
//! old revisions, comes with local history (PLAN.md, Stage 2).

use std::{
    collections::HashMap,
    fs, io,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use ropey::Rope;

use crate::{
    change::{Assoc, ChangeSet},
    journal::{self, Journal, Record},
};

pub type Revision = u64;

/// Who made a change: a person, an agent, a package.
pub type Actor = Arc<str>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Edit,
    Undo,
    Redo,
}

/// Whether an edit starts a new undo unit or joins the actor's last one.
/// Joining only happens when nothing else changed the document in between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    New,
    Extend,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    pub base: Revision,
    pub actor: Actor,
    pub changes: ChangeSet,
    pub group: Group,
}

#[derive(Debug)]
pub enum ApplyError {
    /// Made against `base`; the document is at `head`. Rebase it.
    Stale { base: Revision, head: Revision },
    /// The change is for a text of another length.
    Length { expected: usize, actual: usize },
    /// An edit boundary splits a UTF-8 sequence.
    Boundary { at: usize },
    /// Recording the transaction failed; the document is unchanged.
    Journal(io::Error),
}

#[derive(Debug)]
pub enum UndoError {
    Nothing,
    /// Later edits by these actors touch the text the undo would change.
    Conflict {
        actors: Vec<Actor>,
    },
    Apply(ApplyError),
}

#[derive(Debug)]
pub enum RebaseError {
    /// The revision is no longer in the history.
    Forgotten { base: Revision },
    /// Edits since then by these actors touch the same text.
    Conflict { actors: Vec<Actor> },
}

macro_rules! display {
    ($ty:ty, $self:ident, $f:ident, $body:expr) => {
        impl std::fmt::Display for $ty {
            fn fmt(&$self, $f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                $body
            }
        }
        impl std::error::Error for $ty {}
    };
}

display!(
    ApplyError,
    self,
    f,
    match self {
        ApplyError::Stale { base, head } => write!(f, "made against revision {base}, the document is at {head}"),
        ApplyError::Length { expected, actual } => write!(f, "change for {actual} bytes applied to {expected}"),
        ApplyError::Boundary { at } => write!(f, "byte {at} is inside a character"),
        ApplyError::Journal(e) => write!(f, "could not record the change: {e}"),
    }
);

display!(
    UndoError,
    self,
    f,
    match self {
        UndoError::Nothing => write!(f, "nothing to undo"),
        UndoError::Conflict { actors } => write!(f, "later edits by {} touch the same text", join(actors)),
        UndoError::Apply(e) => e.fmt(f),
    }
);

display!(
    RebaseError,
    self,
    f,
    match self {
        RebaseError::Forgotten { base } => write!(f, "revision {base} is no longer in the history"),
        RebaseError::Conflict { actors } => write!(f, "edits by {} touch the same text", join(actors)),
    }
);

fn join(actors: &[Actor]) -> String {
    actors.iter().map(|a| a.as_ref()).collect::<Vec<_>>().join(", ")
}

/// One applied transaction.
#[derive(Clone, Debug)]
pub struct Entry {
    pub actor: Actor,
    pub kind: Kind,
    pub changes: ChangeSet,
    inverse: ChangeSet,
}

/// Contiguous history entries undone or redone together.
type Unit = Range<usize>;

#[derive(Default, Debug)]
struct Stacks {
    undo: Vec<Unit>,
    redo: Vec<Unit>,
}

/// Where the document is stored and recorded.
#[derive(Debug)]
struct Storage {
    path: PathBuf,
    journal_path: PathBuf,
    journal: Journal,
}

#[derive(Debug)]
pub struct Document {
    text: Rope,
    /// The revision before `history[0]`.
    first: Revision,
    history: Vec<Entry>,
    stacks: HashMap<Actor, Stacks>,
    storage: Option<Storage>,
    saved: Revision,
}

/// What opening a document found in its journal.
#[derive(Debug, PartialEq, Eq)]
pub enum Recovery {
    /// No journal, or nothing in it beyond the file.
    Clean,
    /// Unsaved transactions replayed; `torn` when a partial last record was
    /// discarded.
    Replayed { transactions: usize, torn: bool },
}

#[derive(Debug)]
pub enum OpenError {
    Io(io::Error),
    NotUtf8,
    /// The file changed outside Techne since the journal was started; the
    /// journal is left in place.
    JournalMismatch,
    /// The journal is not one Techne can read; it is left in place.
    BadJournal,
}

display!(
    OpenError,
    self,
    f,
    match self {
        OpenError::Io(e) => e.fmt(f),
        OpenError::NotUtf8 => write!(f, "the file is not UTF-8"),
        OpenError::JournalMismatch => write!(f, "the file changed since its unsaved edits were journaled"),
        OpenError::BadJournal => write!(f, "the journal is unreadable"),
    }
);

impl From<io::Error> for OpenError {
    fn from(e: io::Error) -> Self {
        OpenError::Io(e)
    }
}

impl Document {
    /// An in-memory document, not stored anywhere.
    pub fn new(text: &str) -> Document {
        Document { text: Rope::from_str(text), first: 0, history: Vec::new(), stacks: HashMap::new(), storage: None, saved: 0 }
    }

    /// Open `path` (empty if it does not exist), recording edits in
    /// `journal_path`. Unsaved edits a previous session journaled are
    /// replayed.
    pub fn open(path: &Path, journal_path: &Path) -> Result<(Document, Recovery), OpenError> {
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let text = String::from_utf8(bytes).map_err(|_| OpenError::NotUtf8)?;
        let hash = journal::hash(text.as_bytes());
        let mut doc = Document::new(&text);
        let mut unsaved = Vec::new();
        let recovery = match journal::read(journal_path)? {
            None => Recovery::Clean,
            Some(found) => {
                // Replay from the journal's base, or from the last save that
                // completed (the file then holds that save's text).
                let start = found.records.iter().rposition(|r| matches!(r, Record::Saving { hash: h } if *h == hash));
                let (skip, first) = match start {
                    Some(i) => (i + 1, found.base_revision + count_tx(&found.records[..i]) as u64),
                    None if found.base_hash == hash => (0, found.base_revision),
                    None => return Err(OpenError::JournalMismatch),
                };
                doc.first = first;
                doc.saved = first;
                for record in found.records.into_iter().skip(skip) {
                    if let Record::Tx { tx, kind } = &record {
                        doc.replay(tx.clone(), *kind).map_err(|_| OpenError::BadJournal)?;
                        unsaved.push(record);
                    }
                }
                match (unsaved.len(), found.torn) {
                    (0, false) => Recovery::Clean,
                    (transactions, torn) => Recovery::Replayed { transactions, torn },
                }
            }
        };
        // Start a fresh journal from the file. Recovered edits stay unsaved:
        // they are recorded again after the journal's base.
        let mut journal = Journal::create(journal_path, doc.first, &hash)?;
        for record in &unsaved {
            journal.append(record)?;
        }
        doc.storage = Some(Storage { path: path.to_owned(), journal_path: journal_path.to_owned(), journal });
        Ok((doc, recovery))
    }

    pub fn text(&self) -> &Rope {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn revision(&self) -> Revision {
        self.first + self.history.len() as u64
    }

    pub fn path(&self) -> Option<&Path> {
        self.storage.as_ref().map(|s| s.path.as_path())
    }

    /// Whether the text changed since it was opened or saved.
    pub fn is_dirty(&self) -> bool {
        self.saved != self.revision()
    }

    /// The history entries after `rev`, oldest first.
    pub fn entries_since(&self, rev: Revision) -> Option<&[Entry]> {
        let i = rev.checked_sub(self.first)? as usize;
        self.history.get(i..)
    }

    /// Where `pos` at revision `rev` is now, and whether the text around it
    /// was deleted since. `None` when `rev` is no longer in the history.
    pub fn map_pos(&self, pos: usize, assoc: Assoc, rev: Revision) -> Option<(usize, bool)> {
        self.entries_since(rev)?.iter().try_fold((pos, false), |(p, deleted), e| {
            let (q, d) = e.changes.map(p, assoc);
            Some((q, deleted || d))
        })
    }

    /// A transaction against the current revision that makes the edits.
    pub fn edit<I, S>(&self, actor: &Actor, edits: I) -> Result<Transaction, crate::change::EditError>
    where
        I: IntoIterator<Item = (Range<usize>, S)>,
        S: Into<String>,
    {
        Ok(Transaction {
            base: self.revision(),
            actor: actor.clone(),
            changes: ChangeSet::from_edits(self.len(), edits)?,
            group: Group::New,
        })
    }

    pub fn apply(&mut self, tx: Transaction) -> Result<Revision, ApplyError> {
        self.check(&tx)?;
        if let Some(s) = &mut self.storage {
            s.journal.append(&Record::Tx { tx: tx.clone(), kind: Kind::Edit }).map_err(ApplyError::Journal)?;
        }
        self.commit(tx, Kind::Edit);
        Ok(self.revision())
    }

    /// The transaction moved past every revision since its base.
    pub fn rebase(&self, mut tx: Transaction) -> Result<Transaction, RebaseError> {
        let later = self.entries_since(tx.base).ok_or(RebaseError::Forgotten { base: tx.base })?;
        tx.changes =
            moved_past(tx.changes, later, &self.text, &tx.actor, Assoc::After).map_err(|actors| RebaseError::Conflict { actors })?;
        tx.base = self.revision();
        Ok(tx)
    }

    pub fn undo(&mut self, actor: &Actor) -> Result<Revision, UndoError> {
        self.revert(actor, Kind::Undo)
    }

    pub fn redo(&mut self, actor: &Actor) -> Result<Revision, UndoError> {
        self.revert(actor, Kind::Redo)
    }

    /// Write the text to its file, atomically, and start a new journal.
    pub fn save(&mut self) -> io::Result<()> {
        let s = self.storage.as_mut().ok_or_else(|| io::Error::other("the document has no file"))?;
        let bytes = self.text.to_string().into_bytes();
        let hash = journal::hash(&bytes);
        // Recorded first: a crash after the rename finds the file matching it.
        s.journal.append(&Record::Saving { hash })?;
        journal::write_atomically(&s.path, &bytes)?;
        s.journal = Journal::create(&s.journal_path, self.first + self.history.len() as u64, &hash)?;
        self.saved = self.revision();
        Ok(())
    }

    fn check(&self, tx: &Transaction) -> Result<(), ApplyError> {
        let head = self.revision();
        if tx.base != head {
            return Err(ApplyError::Stale { base: tx.base, head });
        }
        if tx.changes.len_before() != self.len() {
            return Err(ApplyError::Length { expected: self.len(), actual: tx.changes.len_before() });
        }
        let inside = |b: usize| self.text.char_to_byte(self.text.byte_to_char(b)) != b;
        match tx.changes.edits().flat_map(|(r, _)| [r.start, r.end]).find(|&b| inside(b)) {
            Some(at) => Err(ApplyError::Boundary { at }),
            None => Ok(()),
        }
    }

    fn commit(&mut self, tx: Transaction, kind: Kind) {
        let inverse = tx.changes.invert(&self.text);
        tx.changes.apply(&mut self.text);
        let index = self.history.len();
        let extends = kind == Kind::Edit
            && tx.group == Group::Extend
            && self.history.last().is_some_and(|e| e.actor == tx.actor && e.kind == Kind::Edit);
        let stacks = self.stacks.entry(tx.actor.clone()).or_default();
        match kind {
            Kind::Edit => {
                stacks.redo.clear();
                match stacks.undo.last_mut() {
                    Some(unit) if extends && unit.end == index => unit.end += 1,
                    _ => stacks.undo.push(index..index + 1),
                }
            }
            Kind::Undo => stacks.redo.push(index..index + 1),
            Kind::Redo => stacks.undo.push(index..index + 1),
        }
        self.history.push(Entry { actor: tx.actor, kind, changes: tx.changes, inverse });
    }

    fn revert(&mut self, actor: &Actor, kind: Kind) -> Result<Revision, UndoError> {
        let stacks = self.stacks.get(actor);
        let unit = match kind {
            Kind::Undo => stacks.and_then(|s| s.undo.last()),
            _ => stacks.and_then(|s| s.redo.last()),
        }
        .cloned()
        .ok_or(UndoError::Nothing)?;
        let inverse =
            self.history[unit.clone()].iter().rev().map(|e| e.inverse.clone()).reduce(|a, b| a.compose(&b)).expect("units are not empty");
        // Restored text goes back before insertions at its position: those
        // came after it was first there.
        let changes = moved_past(inverse, &self.history[unit.end..], &self.text, actor, Assoc::Before)
            .map_err(|actors| UndoError::Conflict { actors })?;
        let tx = Transaction { base: self.revision(), actor: actor.clone(), changes, group: Group::New };
        if let Some(s) = &mut self.storage {
            s.journal.append(&Record::Tx { tx: tx.clone(), kind }).map_err(|e| UndoError::Apply(ApplyError::Journal(e)))?;
        }
        let stacks = self.stacks.get_mut(actor).expect("actor has stacks");
        match kind {
            Kind::Undo => stacks.undo.pop(),
            _ => stacks.redo.pop(),
        };
        self.commit(tx, kind);
        Ok(self.revision())
    }

    /// Apply a journaled transaction, rebuilding the undo stacks as they were.
    fn replay(&mut self, tx: Transaction, kind: Kind) -> Result<(), ApplyError> {
        self.check(&tx)?;
        if kind != Kind::Edit {
            let stacks = self.stacks.entry(tx.actor.clone()).or_default();
            match kind {
                Kind::Undo => stacks.undo.pop(),
                _ => stacks.redo.pop(),
            };
        }
        self.commit(tx, kind);
        Ok(())
    }
}

fn count_tx(records: &[Record]) -> usize {
    records.iter().filter(|r| matches!(r, Record::Tx { .. })).count()
}

/// `changes` moved past `later` (the entries up to `head`, the current
/// text), or the other actors whose edits it conflicts with. Later edits are
/// composed and reduced to what they actually changed, so edits that cancel
/// (an undone change and its undo) do not conflict.
fn moved_past(changes: ChangeSet, later: &[Entry], head: &Rope, actor: &Actor, ties: Assoc) -> Result<ChangeSet, Vec<Actor>> {
    // Composed backwards from `head`, where the text is known, then inverted.
    let Some(back) = later.iter().rev().map(|e| e.inverse.clone()).reduce(|a, b| a.compose(&b)) else {
        return Ok(changes);
    };
    let composed = back.minimized(head).invert(head);
    changes.transform(&composed, ties).map_err(|_| {
        let mut actors: Vec<Actor> = Vec::new();
        for e in later.iter().filter(|e| &e.actor != actor) {
            if !actors.contains(&e.actor) {
                actors.push(e.actor.clone());
            }
        }
        if actors.is_empty() {
            actors.push(actor.clone());
        }
        actors
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(name: &str) -> Actor {
        Arc::from(name)
    }

    fn edit(doc: &mut Document, who: &Actor, range: Range<usize>, text: &str) -> Revision {
        let tx = doc.edit(who, [(range, text)]).unwrap();
        doc.apply(tx).unwrap()
    }

    #[test]
    fn stale_transactions_are_refused_and_rebase() {
        let (me, agent) = (actor("me"), actor("agent"));
        let mut doc = Document::new("hello world");
        let mine = doc.edit(&me, [(0..0, ">> ")]).unwrap();
        edit(&mut doc, &agent, 6..11, "there");
        assert!(matches!(doc.apply(mine.clone()), Err(ApplyError::Stale { base: 0, head: 1 })));
        doc.apply(doc.rebase(mine).unwrap()).unwrap();
        assert_eq!(doc.text().to_string(), ">> hello there");
    }

    #[test]
    fn undo_steps_around_other_actors() {
        let (me, agent) = (actor("me"), actor("agent"));
        let mut doc = Document::new("ab");
        edit(&mut doc, &me, 1..1, "XYZ");
        edit(&mut doc, &agent, 5..5, "!");
        edit(&mut doc, &agent, 0..0, "<");
        doc.undo(&me).unwrap();
        assert_eq!(doc.text().to_string(), "<ab!");
        doc.redo(&me).unwrap();
        assert_eq!(doc.text().to_string(), "<aXYZb!");
    }

    #[test]
    fn undo_refuses_when_others_touched_the_text() {
        let (me, agent) = (actor("me"), actor("agent"));
        let mut doc = Document::new("ab");
        edit(&mut doc, &me, 1..1, "XYZ");
        edit(&mut doc, &agent, 2..3, "y");
        let before = doc.text().to_string();
        match doc.undo(&me) {
            Err(UndoError::Conflict { actors }) => assert_eq!(actors, [agent]),
            other => panic!("expected a conflict, got {other:?}"),
        }
        assert_eq!(doc.text().to_string(), before);
    }

    #[test]
    fn undo_units_group_and_undo_in_order() {
        let me = actor("me");
        let mut doc = Document::new("");
        for (i, c) in "abc".chars().enumerate() {
            let mut tx = doc.edit(&me, [(i..i, c.to_string())]).unwrap();
            tx.group = if i == 0 { Group::New } else { Group::Extend };
            doc.apply(tx).unwrap();
        }
        edit(&mut doc, &me, 3..3, " d");
        doc.undo(&me).unwrap();
        assert_eq!(doc.text().to_string(), "abc");
        doc.undo(&me).unwrap();
        assert_eq!(doc.text().to_string(), "");
        assert!(matches!(doc.undo(&me), Err(UndoError::Nothing)));
        doc.redo(&me).unwrap();
        doc.redo(&me).unwrap();
        assert_eq!(doc.text().to_string(), "abc d");
    }

    #[test]
    fn a_new_edit_drops_the_redo_stack() {
        let me = actor("me");
        let mut doc = Document::new("");
        edit(&mut doc, &me, 0..0, "a");
        doc.undo(&me).unwrap();
        edit(&mut doc, &me, 0..0, "b");
        assert!(matches!(doc.redo(&me), Err(UndoError::Nothing)));
    }

    #[test]
    fn edits_inside_characters_are_refused() {
        let me = actor("me");
        let mut doc = Document::new("é");
        let tx = doc.edit(&me, [(1..1, "x")]).unwrap();
        assert!(matches!(doc.apply(tx), Err(ApplyError::Boundary { at: 1 })));
    }

    fn stored(text: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let (path, journal) = (dir.path().join("f.txt"), dir.path().join("f.journal"));
        fs::write(&path, text).unwrap();
        (dir, path, journal)
    }

    #[test]
    fn a_crash_inside_save_keeps_the_saved_text() {
        let me = actor("me");
        let (_dir, path, journal) = stored("a");
        let (mut doc, _) = Document::open(&path, &journal).unwrap();
        edit(&mut doc, &me, 1..1, "b");
        // What save does before starting a new journal.
        let hash = journal::hash(b"ab");
        doc.storage.as_mut().unwrap().journal.append(&Record::Saving { hash }).unwrap();
        journal::write_atomically(&path, b"ab").unwrap();
        drop(doc);
        let (doc, recovery) = Document::open(&path, &journal).unwrap();
        assert_eq!((doc.text().to_string(), recovery, doc.is_dirty()), ("ab".into(), Recovery::Clean, false));
    }

    #[test]
    fn a_failed_save_keeps_the_edits_unsaved() {
        let me = actor("me");
        let (_dir, path, journal) = stored("a");
        let (mut doc, _) = Document::open(&path, &journal).unwrap();
        edit(&mut doc, &me, 1..1, "b");
        let hash = journal::hash(b"ab");
        doc.storage.as_mut().unwrap().journal.append(&Record::Saving { hash }).unwrap();
        edit(&mut doc, &me, 2..2, "c");
        drop(doc);
        let (doc, recovery) = Document::open(&path, &journal).unwrap();
        assert_eq!(doc.text().to_string(), "abc");
        assert_eq!(recovery, Recovery::Replayed { transactions: 2, torn: false });
    }

    #[test]
    fn a_file_changed_elsewhere_leaves_the_journal_alone() {
        let me = actor("me");
        let (_dir, path, journal) = stored("a");
        let (mut doc, _) = Document::open(&path, &journal).unwrap();
        edit(&mut doc, &me, 1..1, "b");
        drop(doc);
        fs::write(&path, "changed").unwrap();
        let kept = fs::read(&journal).unwrap();
        assert!(matches!(Document::open(&path, &journal), Err(OpenError::JournalMismatch)));
        assert_eq!(fs::read(&journal).unwrap(), kept);
    }

    #[test]
    fn saving_writes_the_bytes_and_cleans_the_journal() {
        let me = actor("me");
        let (_dir, path, journal) = stored("line\r\nnext");
        let (mut doc, _) = Document::open(&path, &journal).unwrap();
        edit(&mut doc, &me, 4..4, "!");
        assert!(doc.is_dirty());
        doc.save().unwrap();
        assert!(!doc.is_dirty());
        assert_eq!(fs::read(&path).unwrap(), b"line!\r\nnext");
        drop(doc);
        let (doc, recovery) = Document::open(&path, &journal).unwrap();
        assert_eq!((recovery, doc.is_dirty()), (Recovery::Clean, false));
    }

    #[test]
    fn positions_map_across_revisions() {
        let (me, agent) = (actor("me"), actor("agent"));
        let mut doc = Document::new("one two three");
        edit(&mut doc, &agent, 0..4, "");
        edit(&mut doc, &me, 0..0, "zero ");
        assert_eq!(doc.map_pos(4, Assoc::Before, 0), Some((0, false)));
        assert_eq!(doc.map_pos(4, Assoc::After, 0), Some((5, false)));
        assert_eq!(doc.map_pos(2, Assoc::After, 0), Some((5, true)));
        assert_eq!(doc.map_pos(2, Assoc::Before, 7), None);
    }
}
