//! Documents and views for techne Lisp.
//!
//! A view is one interaction with a document (EDITOR.md, section 1): an
//! actor and a selection. Its selection follows other actors' edits on its
//! own (carets stay before their insertions); its own edits move its carets
//! past what they insert. Commands, keymaps and the two key profiles are
//! Lisp, in `lisp/editor`; this crate gives them the text core.
//!
//! Positions are byte offsets, opaque to Lisp: it gets them from motions and
//! hands them back, and reads text through `document-substring`.
//!
//! `runtime` drives one session for a frontend over the data-only protocol
//! in `present`; `segment` is what frontends share to scroll by anchor.

pub mod host;
pub mod present;
pub mod runtime;
pub mod segment;

use std::{cell::RefCell, path::Path, rc::Rc, sync::Arc};

use techne_text::{
    Actor, Assoc, Document, Group, Range, Revision, Selection,
    motion::{self, Words},
};
use techne_vm::{
    api::{Foreign, FromValue},
    value::Value,
    vm::{Error, Vm},
};

type Doc = Foreign<RefCell<Document>>;

pub struct View {
    /// Identifies the view in the presentation protocol.
    id: u64,
    doc: Rc<RefCell<Document>>,
    actor: Actor,
    selection: Selection,
    /// A position on the first visible line (EDITOR.md, section 6). It
    /// follows edits like a caret, so text inserted above the screen does
    /// not move what is shown.
    scroll: usize,
    /// The revision the selection and scroll anchor are for.
    revision: Revision,
}

impl View {
    pub fn new(doc: Rc<RefCell<Document>>, actor: &str) -> View {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let revision = doc.borrow().revision();
        View { id, doc, actor: Arc::from(actor), selection: Selection::single(Range::caret(0)), scroll: 0, revision }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// Another view of the same document, as this one is now.
    pub fn split(&mut self) -> View {
        self.sync();
        View { selection: self.selection.clone(), scroll: self.scroll, ..View::new(self.doc.clone(), &self.actor) }
    }

    /// Follow edits made since the selection was last updated.
    fn sync(&mut self) {
        let doc = self.doc.borrow();
        if doc.revision() == self.revision {
            return;
        }
        (self.selection, self.scroll) = match doc.entries_since(self.revision) {
            Some(entries) => entries
                .iter()
                .fold((self.selection.clone(), self.scroll), |(s, p), e| (s.map(&e.changes), e.changes.map_pos(p, Assoc::Before))),
            None => (Selection::single(Range::caret(0)), 0),
        };
        self.revision = doc.revision();
    }

    pub fn selection(&mut self) -> &Selection {
        self.sync();
        &self.selection
    }

    pub fn document(&self) -> &Rc<RefCell<Document>> {
        &self.doc
    }

    pub fn scroll(&mut self) -> usize {
        self.sync();
        self.scroll
    }

    /// Scroll to `pos` of the text at `revision`, mapped to the current text.
    /// Refused when that revision is no longer in the history.
    pub fn scroll_to(&mut self, pos: usize, revision: Revision) -> Result<(), String> {
        self.sync();
        let doc = self.doc.borrow();
        let len = doc.len();
        let (p, _) = doc.map_pos(pos, Assoc::Before, revision).ok_or("the scrolled text is gone")?;
        self.scroll = p.min(len);
        Ok(())
    }

    pub fn set_selection(&mut self, ranges: Vec<Range>, primary: usize) -> Result<(), String> {
        self.sync();
        let doc = self.doc.borrow();
        let text = doc.text();
        let on_boundary = |p: usize| p <= text.len_bytes() && text.char_to_byte(text.byte_to_char(p)) == p;
        if let Some(r) = ranges.iter().find(|r| !on_boundary(r.anchor) || !on_boundary(r.head)) {
            return Err(format!("view-set-ranges!: ({} {}) is not a position in the document", r.anchor, r.head));
        }
        if primary >= ranges.len() {
            return Err("view-set-ranges!: no primary range".into());
        }
        self.selection = Selection::new(ranges, primary);
        Ok(())
    }

    /// Apply edits as this view's actor; the view's carets move past them.
    /// Edits that change nothing make no transaction.
    pub fn edit(&mut self, edits: Vec<(std::ops::Range<usize>, String)>, group: Group) -> Result<Revision, String> {
        self.sync();
        let mut doc = self.doc.borrow_mut();
        let mut tx = doc.edit(&self.actor, edits).map_err(|e| e.to_string())?;
        if tx.changes.is_identity() {
            return Ok(doc.revision());
        }
        tx.group = group;
        let changes = tx.changes.clone();
        let rev = doc.apply(tx).map_err(|e| e.to_string())?;
        self.selection = self.selection.map_own(&changes);
        self.scroll = changes.map_pos(self.scroll, Assoc::Before);
        self.revision = rev;
        Ok(rev)
    }

    /// Undo or redo this actor's last unit; the caret goes where it changed.
    pub fn revert(&mut self, undo: bool) -> Result<Revision, String> {
        self.sync();
        let mut doc = self.doc.borrow_mut();
        let rev = if undo { doc.undo(&self.actor) } else { doc.redo(&self.actor) }.map_err(|e| e.to_string())?;
        let changes = &doc.entries_since(rev - 1).expect("just applied")[0].changes;
        self.scroll = changes.map_pos(self.scroll, Assoc::Before);
        self.selection = match changes.edits().next() {
            // Nothing before the first edit moves.
            Some((r, _)) => Selection::single(Range::caret(r.start)),
            None => self.selection.map(changes),
        };
        self.revision = rev;
        Ok(rev)
    }
}

/// An edit from Lisp: `(from to text)`.
struct EditArg(usize, usize, String);

impl FromValue for EditArg {
    fn from_value(vm: &mut Vm, v: Value) -> Result<Self, Error> {
        let items = Vec::<Value>::from_value(vm, v)?;
        match items[..] {
            [from, to, text] => Ok(EditArg(usize::from_value(vm, from)?, usize::from_value(vm, to)?, String::from_value(vm, text)?)),
            _ => Err(Error::new("an edit is (from to text)".to_string())),
        }
    }
}

/// Define the editor procedures in `vm`.
pub fn install(vm: &mut Vm) {
    vm.name_foreign_type::<RefCell<Document>>("document");
    vm.name_foreign_type::<RefCell<View>>("view");

    vm.register_fn("make-document", |text: String| Foreign::new(RefCell::new(Document::new(&text))));
    vm.register_fn("open-document", |path: String, journal: String| -> Result<Doc, String> {
        let (doc, _) = Document::open(Path::new(&path), Path::new(&journal)).map_err(|e| format!("{path}: {e}"))?;
        Ok(Foreign::new(RefCell::new(doc)))
    });
    // Documents are the same when their ids are (each handle Lisp gets is a
    // new object).
    vm.register_fn("document-id", |d: Doc| Rc::as_ptr(&d.0) as usize as i64);
    vm.register_fn("document-string", |d: Doc| d.borrow().text().to_string());
    vm.register_fn("document-length", |d: Doc| d.borrow().len());
    vm.register_fn("document-revision", |d: Doc| d.borrow().revision() as i64);
    vm.register_fn("document-dirty?", |d: Doc| d.borrow().is_dirty());
    vm.register_fn("document-path", |d: Doc| d.borrow().path().map(|p| p.display().to_string()));
    vm.register_fn("document-substring", |d: Doc, from: usize, to: usize| -> Result<String, String> {
        let doc = d.borrow();
        let t = doc.text();
        let ok = |p: usize| p <= t.len_bytes() && t.char_to_byte(t.byte_to_char(p)) == p;
        if from > to || !ok(from) || !ok(to) {
            return Err(format!("document-substring: bad range {from} {to}"));
        }
        Ok(t.byte_slice(from..to).to_string())
    });
    vm.register_fn("document-save!", |d: Doc| d.borrow_mut().save().map_err(|e| e.to_string()));
    // A file with its unsaved edits from the journal.
    vm.register_fn("open-file", |path: String| -> Result<Doc, String> {
        let p = Path::new(&path);
        let journal = runtime::journal_for(p).map_err(|e| format!("{path}: {e}"))?;
        let (doc, _) = Document::open(p, &journal).map_err(|e| format!("{path}: {e}"))?;
        Ok(Foreign::new(RefCell::new(doc)))
    });
    // The entries of a directory, sorted, directories with a slash after
    // their name, for completing file names.
    vm.requiring(techne_vm::vm::Capability::Files, |vm| {
        vm.register_fn("directory-list", |dir: String| -> Result<Vec<String>, String> {
            let entries = std::fs::read_dir(&dir).map_err(|e| format!("{dir}: {e}"))?;
            let mut names: Vec<String> = entries
                .filter_map(|e| {
                    let e = e.ok()?;
                    let dir = e.path().is_dir();
                    Some(e.file_name().to_string_lossy().into_owned() + if dir { "/" } else { "" })
                })
                .collect();
            names.sort();
            Ok(names)
        });
    });
    // The spans (start end) of the document's top-level data, as the VM's
    // reader finds them; up to a malformed datum.
    vm.register_fn("document-forms", |d: Doc| {
        let text = d.borrow().text().to_string();
        let forms = techne_vm::reader::read_syntax(&text).or_else(|e| {
            let end = e.pos.map_or(0, |p| p as usize).min(text.len());
            techne_vm::reader::read_syntax(&text[..end])
        });
        forms.unwrap_or_default().iter().map(|f| vec![f.span.0 as usize, f.span.1 as usize]).collect::<Vec<_>>()
    });
    // Ends the runtime thread at once, as a crash would (for testing that a
    // frontend recovers: the restarted runtime replays the journal).
    vm.register_fn("%crash-runtime", || -> i64 { panic!("%crash-runtime") });

    // Motions: (motion document position ...) -> position.
    let at = |d: &Doc, pos: usize| -> Result<(), String> {
        let len = d.borrow().len();
        if pos > len { Err(format!("position {pos} past the end ({len})")) } else { Ok(()) }
    };
    macro_rules! motion {
        ($name:literal, |$t:ident, $p:ident $(, $a:ident: $ty:ty)*| $body:expr) => {
            vm.register_fn($name, move |d: Doc, $p: usize $(, $a: $ty)*| -> Result<_, String> {
                at(&d, $p)?;
                let doc = d.borrow();
                let $t = doc.text();
                Ok($body)
            });
        };
    }
    motion!("next-grapheme", |t, p| motion::next_grapheme(t, p));
    motion!("prev-grapheme", |t, p| motion::prev_grapheme(t, p));
    motion!("word-end", |t, p, style: String| motion::word_end(t, p, words(&style)?));
    motion!("next-word-start", |t, p, style: String| motion::next_word_start(t, p, words(&style)?));
    motion!("word-start", |t, p, style: String| motion::word_start(t, p, words(&style)?));
    motion!("word-object", |t, p, style: String, around: bool| span(motion::word_object(t, p, words(&style)?, around)));
    motion!("line-start", |t, p| motion::line_start(t, p));
    motion!("line-end", |t, p| motion::line_end(t, p));
    motion!("line-span", |t, p| span(motion::line_span(t, p)));
    motion!("column", |t, p| motion::column(t, p));
    motion!("line-number", |t, p| t.byte_to_line(p) + 1);
    motion!("line-down", |t, p, count: i64, goal: usize| motion::line_down(t, p, count as isize, goal));
    motion!("search-text", |t, p, needle: String, forward: bool| motion::search(t, p, &needle, forward).map(span));

    // Views.
    vm.register_fn("make-view", |d: Doc, actor: String| Foreign::new(RefCell::new(View::new(d.0.clone(), &actor))));
    vm.register_fn("view-split", |v: Foreign<RefCell<View>>| Foreign::new(RefCell::new(v.borrow_mut().split())));
    vm.register_fn("view-id", |v: Foreign<RefCell<View>>| v.borrow().id as i64);
    vm.register_fn("view-scroll", |v: Foreign<RefCell<View>>| v.borrow_mut().scroll());
    vm.register_fn("view-set-scroll!", |v: Foreign<RefCell<View>>, pos: usize| {
        let revision = v.borrow().doc.borrow().revision();
        v.borrow_mut().scroll_to(pos, revision)
    });
    vm.register_fn("view-document", |v: Foreign<RefCell<View>>| Foreign(Rc::clone(&v.borrow().doc)));
    vm.register_fn("view-ranges", |v: Foreign<RefCell<View>>| {
        v.borrow_mut().selection().ranges().iter().map(|r| vec![r.anchor, r.head]).collect::<Vec<_>>()
    });
    vm.register_fn("view-primary", |v: Foreign<RefCell<View>>| v.borrow_mut().selection().primary_index());
    vm.register_fn("view-set-ranges!", |v: Foreign<RefCell<View>>, ranges: Vec<Vec<usize>>, primary: usize| {
        let ranges = ranges
            .into_iter()
            .map(|r| match r[..] {
                [anchor, head] => Ok(Range::new(anchor, head)),
                _ => Err("view-set-ranges!: a range is (anchor head)".to_string()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        v.borrow_mut().set_selection(ranges, primary)
    });
    vm.register_fn("view-edit!", |v: Foreign<RefCell<View>>, edits: Vec<EditArg>, group: String| -> Result<i64, String> {
        let group = match group.as_str() {
            "new" => Group::New,
            "extend" => Group::Extend,
            g => return Err(format!("view-edit!: group is new or extend, not {g}")),
        };
        let edits = edits.into_iter().map(|EditArg(from, to, text)| (from..to, text)).collect();
        v.borrow_mut().edit(edits, group).map(|r| r as i64)
    });
    vm.register_fn("view-undo!", |v: Foreign<RefCell<View>>| v.borrow_mut().revert(true).map(|r| r as i64));
    vm.register_fn("view-redo!", |v: Foreign<RefCell<View>>| v.borrow_mut().revert(false).map(|r| r as i64));
}

fn words(style: &str) -> Result<Words, String> {
    match style {
        "emacs" => Ok(Words::Emacs),
        "vim" => Ok(Words::Vim),
        s => Err(format!("word style is emacs or vim, not {s}")),
    }
}

fn span(r: std::ops::Range<usize>) -> Vec<usize> {
    vec![r.start, r.end]
}
