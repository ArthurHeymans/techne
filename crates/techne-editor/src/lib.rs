//! Documents, presentations and views for techne Lisp.
//!
//! A view is one interaction with a text (EDITOR.md, section 1): an actor
//! and a selection. The text is a document, or a presentation of keyed rows
//! (`presentation`), whose excerpts of documents are edited through to them
//! (`lens`). A view's selection follows other actors' changes on its own
//! (carets stay before insertions into a document, and follow the row they
//! are on through a presentation's); its own edits move its carets past
//! what they insert. Commands, keymaps and the two key profiles are Lisp, in
//! `lisp/editor`; this crate gives them the text core.
//!
//! Positions are byte offsets, opaque to Lisp: it gets them from motions and
//! hands them back, and reads text through `document-substring`. The
//! natives reading text take a document or a presentation alike.
//!
//! `runtime` drives a session for each frontend attached, over the data-only
//! protocol in `present`; `host` runs it for frontends in this process, and
//! `server` opens files in it for other programs. `segment` is what
//! frontends share to scroll by anchor.

mod access;
mod deadline;
pub mod edits;
pub mod faces;
mod fs;
pub mod hints;
pub mod host;
pub mod lens;
mod lisp_indent;
mod lsp_text;
pub mod matcher;
pub mod present;
pub mod presentation;
mod regexp_search;
mod remote_save;
pub mod rpc;
pub mod runtime;
mod search;
pub mod segment;
pub mod server;
pub mod service;
pub mod syntax;

use std::{
    cell::{Ref, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::Arc,
};

use presentation::Presentation;
use techne_text::{
    Actor, Assoc, ChangeSet, Document, Group, Range, Revision, Selection, Transaction,
    motion::{self, Words},
    ropey::Rope,
};
use techne_vm::{
    api::{Arg, Foreign, FromValue, IntoValue, Value},
    regexp::Regexp,
    vm::{Error, Vm},
};

type Doc = Foreign<RefCell<Document>>;
type Pres = Foreign<RefCell<Presentation>>;
type Mat = Foreign<RefCell<matcher::Matcher>>;
type Specs = Foreign<RefCell<lisp_indent::Specs>>;
pub(crate) type Documents = Rc<RefCell<HashMap<PathBuf, Weak<RefCell<Document>>>>>;

/// The document of the file at `path`: the one open already, or opened with
/// the journal `journal` gives, and what was recovered from it then.
pub(crate) fn open_shared(
    documents: &Documents,
    path: &Path,
    journal: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<(Rc<RefCell<Document>>, Option<techne_text::Recovery>), String> {
    let canonical = techne_text::document::file_path(path).map_err(|e| e.to_string())?;
    if let Some(doc) = documents.borrow().get(&canonical).and_then(Weak::upgrade) {
        return Ok((doc, None));
    }
    let (doc, recovery) = Document::open(&canonical, &journal()?).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc = Rc::new(RefCell::new(doc));
    documents.borrow_mut().insert(canonical, Rc::downgrade(&doc));
    Ok((doc, Some(recovery)))
}

/// What a view shows: a document, or a presentation.
#[derive(Clone)]
pub enum Text {
    Document(Rc<RefCell<Document>>),
    Presentation(Rc<RefCell<Presentation>>),
}

impl Text {
    pub fn rope(&self) -> Ref<'_, Rope> {
        match self {
            Text::Document(d) => Ref::map(d.borrow(), Document::text),
            Text::Presentation(p) => Ref::map(p.borrow(), Presentation::text),
        }
    }

    pub fn len(&self) -> usize {
        self.rope().len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn revision(&self) -> Revision {
        match self {
            Text::Document(d) => d.borrow().revision(),
            Text::Presentation(p) => p.borrow().revision(),
        }
    }

    /// Where `pos` of the text at `rev` is now (`Document::map_pos`).
    pub fn map_pos(&self, pos: usize, assoc: Assoc, rev: Revision) -> Option<(usize, bool)> {
        match self {
            Text::Document(d) => d.borrow().map_pos(pos, assoc, rev),
            Text::Presentation(p) => p.borrow().map_pos(pos, assoc, rev),
        }
    }

    /// The changes since `rev`, oldest first, or `None` when it is too far
    /// back.
    fn changes_since(&self, rev: Revision) -> Option<Vec<ChangeSet>> {
        match self {
            Text::Document(d) => d.borrow().entries_since(rev).map(|es| es.iter().map(|e| e.changes.clone()).collect()),
            Text::Presentation(p) => p.borrow().changes_since(rev).map(<[ChangeSet]>::to_vec),
        }
    }

    /// Identifies the text: the same for every handle to it, never
    /// another text's, also after it is gone.
    pub fn id(&self) -> u64 {
        match self {
            Text::Document(d) => d.borrow().id(),
            Text::Presentation(p) => p.borrow().id(),
        }
    }

    pub fn document(&self) -> Option<&Rc<RefCell<Document>>> {
        match self {
            Text::Document(d) => Some(d),
            Text::Presentation(_) => None,
        }
    }
}

pub struct View {
    /// Identifies the view in the presentation protocol.
    id: u64,
    text: Text,
    actor: Actor,
    selection: Selection,
    /// A position on the first visible line (EDITOR.md, section 6). It
    /// follows changes like a caret, so text inserted above the screen does
    /// not move what is shown.
    scroll: usize,
    /// The revision the selection and scroll anchor are for.
    revision: Revision,
    /// An explicit editing restriction of this view. Scheme commands also
    /// check the buffer's read-only option at mutation time (EDITOR.md,
    /// section 3); generated writers use separate, unrestricted views.
    read_only: bool,
}

impl View {
    pub fn new(text: Text, actor: &str) -> View {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let revision = text.revision();
        View { id, text, actor: Arc::from(actor), selection: Selection::single(Range::caret(0)), scroll: 0, revision, read_only: false }
    }

    pub fn of_document(doc: Rc<RefCell<Document>>, actor: &str) -> View {
        View::new(Text::Document(doc), actor)
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// Another view of the same text, as this one is now.
    pub fn split(&mut self) -> View {
        self.sync();
        View {
            selection: self.selection.clone(),
            scroll: self.scroll,
            read_only: self.read_only,
            ..View::new(self.text.clone(), &self.actor)
        }
    }

    /// Follow changes made since the selection was last updated. Through a
    /// presentation, a caret where rows were added stays on its row, after
    /// them.
    fn sync(&mut self) {
        let rev = self.text.revision();
        if rev == self.revision {
            return;
        }
        let rows = matches!(self.text, Text::Presentation(_));
        let anchor = if rows { Assoc::After } else { Assoc::Before };
        (self.selection, self.scroll) = match self.text.changes_since(self.revision) {
            Some(changes) => changes.iter().fold((self.selection.clone(), self.scroll), |(s, p), c| {
                (if rows { s.map_own(c) } else { s.map(c) }, c.map_pos(p, anchor))
            }),
            None => (Selection::single(Range::caret(0)), 0),
        };
        self.revision = rev;
    }

    pub fn selection(&mut self) -> &Selection {
        self.sync();
        &self.selection
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn scroll(&mut self) -> usize {
        self.sync();
        self.scroll
    }

    /// Scroll to `pos` of the text at `revision`, mapped to the current text.
    /// Refused when that revision is no longer in the history.
    pub fn scroll_to(&mut self, pos: usize, revision: Revision) -> Result<(), String> {
        self.sync();
        let (p, _) = self.text.map_pos(pos, Assoc::Before, revision).ok_or("the scrolled text is gone")?;
        self.scroll = p.min(self.text.len());
        Ok(())
    }

    pub fn set_selection(&mut self, ranges: Vec<Range>, primary: usize) -> Result<(), String> {
        self.sync();
        {
            let text = self.text.rope();
            let on_boundary = |p: usize| p <= text.len_bytes() && text.char_to_byte(text.byte_to_char(p)) == p;
            if let Some(r) = ranges.iter().find(|r| !on_boundary(r.anchor) || !on_boundary(r.head)) {
                return Err(format!("view-set-ranges!: ({} {}) is not a position in the document", r.anchor, r.head));
            }
        }
        if primary >= ranges.len() {
            return Err("view-set-ranges!: no primary range".into());
        }
        self.selection = Selection::new(ranges, primary);
        Ok(())
    }

    /// Apply edits as this view's actor; the view's carets move past them.
    /// Edits that change nothing make no transaction. Through a
    /// presentation they go to its excerpts' documents.
    pub fn edit(&mut self, edits: Vec<(std::ops::Range<usize>, String)>, group: Group) -> Result<Revision, String> {
        self.sync();
        if self.read_only {
            return Err("Buffer is read-only".into());
        }
        let (rev, changes) = match &self.text {
            Text::Presentation(p) => match p.borrow_mut().edit(&self.actor, edits, group)? {
                Some(edited) => edited,
                None => return Ok(self.revision),
            },
            Text::Document(d) => {
                let mut doc = d.borrow_mut();
                let mut tx = doc.edit(&self.actor, edits).map_err(|e| e.to_string())?;
                if tx.changes.is_identity() {
                    return Ok(doc.revision());
                }
                tx.group = group;
                let changes = tx.changes.clone();
                (doc.apply(tx).map_err(|e| e.to_string())?, changes)
            }
        };
        self.selection = self.selection.map_own(&changes);
        self.scroll = changes.map_pos(self.scroll, Assoc::Before);
        self.revision = rev;
        Ok(rev)
    }

    /// Apply edits made against the text at `base`, as this view's actor:
    /// moved past what changed since, or refused, saying by whom, when that
    /// touched the same text. For a command that looked at the text, waited
    /// (for a process, a service) and then edits what it saw.
    pub fn edit_at(&mut self, base: Revision, edits: Vec<(std::ops::Range<usize>, String)>, group: Group) -> Result<Revision, String> {
        let gone = "the text the edit was made against is no longer in the history";
        let later = self.text.changes_since(base).ok_or(gone)?;
        let len = later.first().map_or(self.text.len(), ChangeSet::len_before);
        let changes = ChangeSet::from_edits(len, edits).map_err(|e| e.to_string())?;
        let moved = match &self.text {
            Text::Document(d) => {
                let tx = Transaction { base, actor: self.actor.clone(), changes, group };
                d.borrow().rebase(tx).map_err(|e| format!("Refused: {e} since"))?.changes
            }
            Text::Presentation(p) => p.borrow().rebase(base, changes)?,
        };
        self.edit(moved.edits().map(|(r, t)| (r, t.to_string())).collect(), group)
    }

    /// Undo or redo this actor's last unit; the caret goes where it changed.
    pub fn revert(&mut self, undo: bool) -> Result<Revision, String> {
        self.sync();
        if self.read_only {
            return Err("Buffer is read-only".into());
        }
        let before = self.revision;
        let rev = match &self.text {
            Text::Presentation(p) => p.borrow_mut().revert(&self.actor, undo)?,
            Text::Document(d) => {
                let mut doc = d.borrow_mut();
                if undo { doc.undo(&self.actor) } else { doc.redo(&self.actor) }.map_err(|e| e.to_string())?
            }
        };
        if rev == before {
            return Ok(rev);
        }
        let changes = self.text.changes_since(rev - 1).expect("just changed").pop().expect("one change");
        self.scroll = changes.map_pos(self.scroll, Assoc::Before);
        self.selection = match changes.edits().next() {
            // Nothing before the first edit moves.
            Some((r, _)) => Selection::single(Range::caret(r.start)),
            None => self.selection.map(&changes),
        };
        self.revision = rev;
        Ok(rev)
    }
}

/// A text from Lisp: a document or a presentation.
pub(crate) struct TextArg(pub(crate) Text);

impl FromValue for TextArg {
    fn from_arg(v: Arg<'_>) -> Result<Self, Error> {
        if let Some(d) = v.foreign::<RefCell<Document>>() {
            return Ok(TextArg(Text::Document(d)));
        }
        match v.foreign::<RefCell<Presentation>>() {
            Some(p) => Ok(TextArg(Text::Presentation(p))),
            None => Err(Error::new(format!("expected a document or a presentation, got {}", v.brief()))),
        }
    }
}

impl IntoValue for Text {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        match self {
            Text::Document(d) => Foreign(d).into_value(vm),
            Text::Presentation(p) => Foreign(p).into_value(vm),
        }
    }
}

/// An integer, a string or a document, for lists of them handed to Lisp.
enum Datum {
    Int(usize),
    Str(String),
    Doc(Rc<RefCell<Document>>),
}

impl IntoValue for Datum {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        match self {
            Datum::Int(n) => n.into_value(vm),
            Datum::Str(s) => s.into_value(vm),
            Datum::Doc(d) => Foreign(d).into_value(vm),
        }
    }
}

/// A row of a presentation from Lisp: `(key column ...)`, a column a run or
/// a list of runs, a run a string, `(text face)`, or an excerpt `(document
/// from to)` or `(document from to face)`; a face a symbol or #f.
struct RowArg(presentation::RowSpec);

impl FromValue for RowArg {
    fn from_arg(v: Arg<'_>) -> Result<Self, Error> {
        use presentation::{Content, RunSpec};
        let face = |v: Arg<'_>| v.symbol_name().map(|s| s.to_string());
        let run = |r: Arg<'_>| -> Result<RunSpec, Error> {
            if let Some(text) = r.as_str() {
                return Ok(RunSpec { content: Content::Text(text.to_owned()), face: None });
            }
            let items = list(r)?;
            match items[..] {
                [text, f] => Ok(RunSpec { content: Content::Text(String::from_arg(text)?), face: face(f) }),
                [d, from, to] | [d, from, to, _] => {
                    let f = items.get(3).copied().and_then(face);
                    let range = usize::from_arg(from)?..usize::from_arg(to)?;
                    Ok(RunSpec { content: Content::Excerpt(Doc::from_arg(d)?.0, range), face: f })
                }
                _ => Err(Error::new("a run is a string, (text face) or (document from to [face])")),
            }
        };
        let items = list(v)?;
        let (key, columns) = items.split_first().ok_or_else(|| Error::new("a row is (key column ...)"))?;
        let key = String::from_arg(*key)?;
        let columns = columns
            .iter()
            .map(|&c| {
                // A column of one run, or a list of runs.
                let one = c.as_str().is_some()
                    || c.is_symbol()
                    || c.items().is_some_and(|items| {
                        items.first().is_some_and(|x| x.foreign::<RefCell<Document>>().is_some())
                            || (items.len() == 2 && items[0].as_str().is_some() && (items[1].is_symbol() || items[1].is_false()))
                    });
                if one { run(c).map(|r| vec![r]) } else { list(c)?.into_iter().map(run).collect() }
            })
            .collect::<Result<_, Error>>()?;
        Ok(RowArg(presentation::RowSpec { key, columns }))
    }
}

/// The elements of a list (or a vector).
fn list(v: Arg<'_>) -> Result<Vec<Arg<'_>>, Error> {
    v.items().ok_or_else(|| v.type_error("list"))
}

/// An edit from Lisp: `(from to text)`.
pub(crate) struct EditArg(usize, usize, String);

impl FromValue for EditArg {
    fn from_arg(v: Arg<'_>) -> Result<Self, Error> {
        match list(v)?[..] {
            [from, to, text] => Ok(EditArg(usize::from_arg(from)?, usize::from_arg(to)?, String::from_arg(text)?)),
            _ => Err(Error::new("an edit is (from to text)".to_string())),
        }
    }
}

type ViewArg = Foreign<RefCell<View>>;

/// Define the editor procedures in `vm`.
pub fn install(vm: &mut Vm) {
    install_with(vm, Documents::default(), syntax::Shared::default());
}

pub(crate) fn install_with(vm: &mut Vm, documents: Documents, trees: syntax::Shared) {
    syntax::install(vm, trees);
    rpc::install(vm);
    vm.name_foreign_type::<RefCell<Document>>("document");
    vm.name_foreign_type::<RefCell<Presentation>>("presentation");
    vm.name_foreign_type::<RefCell<View>>("view");
    let access = access::Shared::default();
    service::install(vm, access.clone());
    access::install(vm, access);

    techne_vm::procedures! { vm;
        /// Return a new document holding TEXT, with no file.
        "(make-document text)" => |text: String| Foreign::new(RefCell::new(Document::new(&text)));
    }
    techne_vm::procedures! { vm;
        /// Keep at most REVISIONS of DOCUMENT's history, or all of it when
        /// REVISIONS is #f: a log's, which would otherwise keep every line
        /// it ever had, deleted ones too. Older revisions are forgotten,
        /// with what undo could reverse there. Refused for a file's document,
        /// whose journal keeps it whole.
        "(document-limit-history! document revisions)" => |d: Doc, revisions: Option<usize>| -> Result<(), String> {
            d.borrow_mut().limit_history(revisions).map_err(|e| format!("document-limit-history!: {e}"))
        };
    }
    let explicit_documents = documents.clone();
    vm.requiring(techne_vm::vm::Capability::Files, |vm| {
        techne_vm::procedures! { vm;
            /// Open the file at PATH as a document, journaling its edits to JOURNAL.
            /// Unsaved edits found in JOURNAL are replayed.
            "(open-document path journal)" => move |path: String, journal: String| -> Result<Doc, String> {
                open_shared(&explicit_documents, Path::new(&path), || Ok(PathBuf::from(journal))).map(|(d, _)| Foreign(d))
            };
        }
    });
    // The text natives take a document or a presentation: both are text a
    // view shows. Texts are the same when their ids are (each handle Lisp
    // gets is a new object).
    techne_vm::procedures! { vm;
        /// Return a number identifying TEXT, a document or a presentation.
        /// Each handle Lisp gets is a new object; compare texts by this.
        "(document-id text)" => |t: TextArg| t.0.id() as i64;
    }
    techne_vm::procedures! { vm;
        /// Return the whole of TEXT, a document or a presentation.
        "(document-string text)" => |t: TextArg| t.0.rope().to_string();
    }
    techne_vm::procedures! { vm;
        /// Return the length of TEXT in bytes.
        "(document-length text)" => |t: TextArg| t.0.len();
    }
    techne_vm::procedures! { vm;
        /// Return the revision of TEXT, which each change increases.
        "(document-revision text)" => |t: TextArg| t.0.revision() as i64;
    }
    techne_vm::procedures! { vm;
        /// Return #t if TEXT is a document with edits not yet saved.
        "(document-dirty? text)" => |t: TextArg| t.0.document().is_some_and(|d| d.borrow().is_dirty());
    }
    techne_vm::procedures! { vm;
        /// Return the file TEXT was opened from, or #f.
        "(document-path text)" => |t: TextArg| t.0.document().and_then(|d| match d.borrow().place()? {
            techne_text::Place::File(p) => Some(p.display().to_string()),
            techne_text::Place::Remote(name) => Some(name.clone()),
        });
    }
    techne_vm::procedures! { vm;
        /// Return the part of TEXT between the byte positions FROM and TO.
        "(document-substring text from to)" => |t: TextArg, from: usize, to: usize| -> Result<String, String> {
            let t = t.0.rope();
            let ok = |p: usize| p <= t.len_bytes() && t.char_to_byte(t.byte_to_char(p)) == p;
            if from > to || !ok(from) || !ok(to) {
                return Err(format!("document-substring: bad range {from} {to}"));
            }
            Ok(t.byte_slice(from..to).to_string())
        };
    }
    // Positions and changes as a language server counts them
    // (`lsp_text`): over the rope, without making the text one string.
    techne_vm::procedures! { vm;
        /// Return POSITION of TEXT as (line column), from 0 as LSP counts
        /// them: COLUMN in ENCODING's units, "utf-16" or "utf-8".
        "(document-lsp-position text position encoding)" => |t: TextArg, pos: usize, encoding: String| -> Vec<usize> {
            let (line, column) = lsp_text::position(&t.0.rope(), pos, lsp_text::Encoding::named(Some(&encoding)));
            vec![line, column]
        };
        /// Return the positions of DOCUMENT as of REVISION at POINTS, each
        /// (line column) as `document-lsp-position` counts them in ENCODING.
        /// A column past its line's end is the end, one inside a character
        /// before it. Return #f when that revision is no longer in the
        /// history. What a server says it says of the text it was sent:
        /// read against that text, then mapped through the edits since.
        "(document-lsp-offsets document revision encoding points)" => |d: Doc, revision: i64, encoding: String, points: Vec<Vec<usize>>| -> Result<Option<Vec<usize>>, String> {
            let encoding = lsp_text::Encoding::named(Some(&encoding));
            let d = d.0.borrow();
            let text = if revision as Revision == d.revision() { d.text().clone() } else {
                match d.text_at(revision as Revision) {
                    Some(text) => text,
                    None => return Ok(None),
                }
            };
            points
                .into_iter()
                .map(|p| match p[..] {
                    [line, column] => Ok(lsp_text::offset(&text, line, column, encoding)),
                    _ => Err("document-lsp-offsets: a point is (line column)".to_string()),
                })
                .collect::<Result<_, _>>()
                .map(Some)
        };
        /// Return the changes that took DOCUMENT from REVISION to now, as a
        /// language server applies them: each (start-line start-column
        /// end-line end-column text), its range in the text the ones before
        /// it left, columns in ENCODING. Return #f when that revision is no
        /// longer in the history.
        "(document-lsp-changes document revision encoding)" => |d: Doc, revision: i64, encoding: String| -> Option<Vec<Vec<Datum>>> {
            let encoding = lsp_text::Encoding::named(Some(&encoding));
            let d = d.0.borrow();
            let before = d.text_at(revision as Revision)?;
            let changes = crate::edits::since(&d, revision as Revision)?;
            Some(
                lsp_text::changes(before, &changes, encoding)
                    .into_iter()
                    .map(|c| {
                        vec![Datum::Int(c.start.0), Datum::Int(c.start.1), Datum::Int(c.end.0), Datum::Int(c.end.1), Datum::Str(c.text)]
                    })
                    .collect(),
            )
        };
    }
    // Waiting for a document to change, woken by the change itself.
    vm.register_async("document-await-change", 2, |vm: &mut Vm, args: techne_vm::api::Args<'_>| {
        let watched = args.get::<Doc>(vm, 0).and_then(|d| Ok((Rc::downgrade(&d.0), args.get::<i64>(vm, 1)?)));
        async move {
            let (doc, revision) = watched.map_err(|e| e.msg.clone())?;
            Ok::<_, String>(
                std::future::poll_fn(move |cx| {
                    let Some(d) = doc.upgrade() else { return std::task::Poll::Ready(None) };
                    let mut d = d.borrow_mut();
                    if d.revision() as i64 != revision {
                        return std::task::Poll::Ready(Some(d.revision() as i64));
                    }
                    d.wake_on_change(cx.waker());
                    std::task::Poll::Pending
                })
                .await,
            )
        }
    });
    techne_vm::document! { vm;
        /// Wait until DOCUMENT is past REVISION; return its revision then.
        /// Return #f once the document is gone. The waiting is the calling
        /// task's, woken by the edit: following a document costs nothing
        /// while it holds still.
        "(document-await-change document revision)";
    }
    // Where `pos` of the text at `revision` is now; #f when that revision
    // is no longer in the history.
    techne_vm::procedures! { vm;
        /// Return where POSITION of TEXT as of REVISION is now.
        /// Return #f when that revision is no longer in the history.
        "(document-map-position text position revision)" => |t: TextArg, pos: usize, revision: i64| {
            t.0.map_pos(pos, Assoc::Before, revision as Revision).map(|(p, _)| p)
        };
    }
    // Matching the minibuffer's candidates (`matcher`).
    techne_vm::procedures! { vm;
        /// Return a matcher of the strings TEXTS, for the minibuffer.
        /// Entries are numbered from 0, in the order of TEXTS.
        "(make-matcher texts)" => |texts: Vec<String>| Foreign::new(RefCell::new(matcher::Matcher::new(texts)));
        /// Return a matcher of the non-empty lines of TEXT.
        /// `matcher-line` gives each entry's start and line number.
        "(lines-matcher text)" => |t: TextArg| Foreign::new(RefCell::new(matcher::Matcher::of_lines(&t.0.rope())));
        /// Return a matcher of the non-empty lines of each of TEXTS, in
        /// order. `matcher-line` gives each entry's start and line number in
        /// its text, and the index of its text in TEXTS.
        "(texts-lines-matcher texts)" => |ts: Vec<TextArg>| {
            let ropes: Vec<_> = ts.iter().map(|t| t.0.rope()).collect();
            Foreign::new(RefCell::new(matcher::Matcher::of_lines_of(ropes.iter().map(|r| &**r))))
        };
        /// Match PATTERN against MATCHER's entries; return how many match.
        /// Each word of PATTERN must occur, without case unless it has an
        /// upper-case letter.
        "(matcher-filter! matcher pattern)" => |m: Mat, pattern: String| m.0.borrow_mut().filter(&pattern);
        /// Return a matcher of MATCHER's entries that filters on its own.
        "(matcher-copy matcher)" => |m: Mat| Foreign::new(RefCell::new(m.0.borrow().copy()));
        /// Return the number of entries of MATCHER.
        "(matcher-size matcher)" => |m: Mat| m.0.borrow().len();
        /// Return the entry that match K of MATCHER's last pattern is.
        /// K counts the matches from 0, in the entries' order.
        "(matcher-match matcher k)" => |m: Mat, k: usize| -> Result<usize, String> {
            m.0.borrow().matches().get(k).map(|&e| e as usize).ok_or_else(|| format!("no match {k}"))
        };
        /// Return the first entry of MATCHER whose text is TEXT, or #f.
        "(matcher-find matcher text)" => |m: Mat, text: String| m.0.borrow().find(&text);
        /// Return the text of ENTRY of MATCHER.
        "(matcher-text matcher entry)" => |m: Mat, e: usize| -> Result<String, String> {
            m.0.borrow().text(e).map(str::to_string).ok_or_else(|| format!("no entry {e}"))
        };
        /// Return the start and line number of ENTRY of MATCHER.
        /// MATCHER is a lines matcher; the result is a list (start number
        /// text), numbers counting from 1, TEXT the index of its text.
        "(matcher-line matcher entry)" => |m: Mat, e: usize| -> Result<Vec<usize>, String> {
            m.0.borrow().line(e).map(|(start, n, text)| vec![start, n, text]).ok_or_else(|| format!("no line entry {e}"))
        };
    }
    // Every line: (start text number), the text without its line break.
    techne_vm::procedures! { vm;
        /// Return every line of TEXT as (start line number).
        /// LINE is without its line break; numbers count from 1.
        "(document-lines text)" => |t: TextArg| {
            let t = t.0.rope();
            (0..t.len_lines())
                .map(|i| {
                    let line = t.line(i).to_string();
                    let text = line.strip_suffix('\n').unwrap_or(&line);
                    (t.line_to_byte(i), text.strip_suffix('\r').unwrap_or(text).to_string(), i + 1)
                })
                .filter(|(start, text, _)| !(text.is_empty() && *start == t.len_bytes() && *start > 0))
                .map(|(start, text, n)| vec![Datum::Int(start), Datum::Str(text), Datum::Int(n)])
                .collect::<Vec<_>>()
        };
    }
    let moved_documents = documents.clone();
    let remote_documents = documents.clone();
    vm.requiring(techne_vm::vm::Capability::Files, |vm| {
        techne_vm::procedures! { vm;
            /// Prepare DOCUMENT to follow its file when it moves to PATH.
            /// PATH is the true name it will have (`file-truename`'s, which
            /// may not exist yet). The journal of its unsaved edits there is
            /// locked until `document-relocate!` or `relocation-cancel!`;
            /// it is an error if it cannot be, or holds unsaved edits.
            "(document-prepare-relocation document path)" => |d: Doc, path: String| -> Result<Foreign<RefCell<Option<techne_text::Relocation>>>, String> {
                let p = Path::new(&path);
                let journal = runtime::journal_named(p).map_err(|e| format!("{path}: {e}"))?;
                d.borrow().prepare_relocation(p, &journal).map(|r| Foreign::new(RefCell::new(Some(r)))).map_err(|e| format!("{path}: {e}"))
            };
            /// Give up RELOCATION, unlocking the journal it prepared.
            "(relocation-cancel! relocation)" => |r: Foreign<RefCell<Option<techne_text::Relocation>>>| { r.borrow_mut().take(); };
            /// Make DOCUMENT follow its file, moved as RELOCATION prepared.
            /// Saving writes there from now on. Return #f, or what kept its
            /// journal where it was, which it is written in still.
            "(document-relocate! document relocation)" => move |d: Doc, r: Foreign<RefCell<Option<techne_text::Relocation>>>| -> Result<Option<String>, String> {
                if r.borrow().as_ref().is_some_and(|r| r.document() != d.borrow().id()) {
                    return Err("document-relocate!: the relocation was prepared for another document".into());
                }
                let r = r.borrow_mut().take().ok_or("document-relocate!: the relocation was used")?;
                let problem = d.borrow_mut().relocate(r).err().map(|e| e.to_string());
                let canonical = d.borrow().path().map(Path::to_path_buf);
                let mut documents = moved_documents.borrow_mut();
                documents.retain(|_, w| !w.upgrade().is_some_and(|o| Rc::ptr_eq(&o, &d.0)));
                if let Some(c) = canonical {
                    documents.insert(c, Rc::downgrade(&d.0));
                }
                Ok(problem)
            };
            /// Write DOCUMENT to its file, refusing an external change.
            "(document-save! document)" => |d: Doc| d.borrow_mut().save().map_err(|e| e.to_string());
            /// Write DOCUMENT to its file even if it changed on disk.
            "(document-save-overwriting! document)" => |d: Doc| d.borrow_mut().save_with(techne_text::SaveMode::Overwrite).map_err(|e| e.to_string());
            /// Read DOCUMENT's file again if it changed on disk.
            /// It is an edit by "disk", refused while DOCUMENT has unsaved
            /// edits. Return #t if the text changed.
            "(document-reload! document)" => |d: Doc| d.borrow_mut().reload(&Arc::from("disk")).map_err(|e| e.to_string());
            /// Return the file at PATH as a document, with its unsaved edits.
            /// The edits come from its journal in the state directory.
            "(open-file path)" => move |path: String| -> Result<Doc, String> {
                let p = Path::new(&path);
                open_shared(&documents, p, || runtime::journal_for(p).map_err(|e| format!("{path}: {e}"))).map(|(d, _)| Foreign(d))
            };
        }
    });
    // Files on other machines (doc/remote.md): read and written through
    // their node, which Lisp gives with the path there.
    vm.requiring(techne_vm::vm::Capability::Files, |vm| {
        let saving = remote_save::Saves::default();
        let open_saving = saving.clone();
        vm.register_async("%open-remote-file", 3, move |vm: &mut Vm, args: techne_vm::api::Args<'_>| {
            let documents = remote_documents.clone();
            let saving = open_saving.clone();
            let request = (|| {
                let node: Foreign<techne_node::NodeRef> = args.get(vm, 0)?;
                let path: String = args.get(vm, 1)?;
                let name: String = args.get(vm, 2)?;
                Ok::<_, Error>((node, path, name))
            })();
            async move {
                let (node, path, name) = request.map_err(|e| e.into_inner().msg)?;
                let key = PathBuf::from(&name);
                saving
                    .open(
                        &documents,
                        &name,
                        move || {
                            let read = node.0.0.read_file(path.into_bytes());
                            async move { read.await.map(|file| file.map(|(bytes, _)| bytes)) }
                        },
                        || runtime::journal_named(&key).map_err(|e| format!("{name}: {e}")),
                    )
                    .await
                    .map(Foreign)
            }
        });
        let reload_saving = saving.clone();
        vm.register_async("%document-save-remote!", 4, move |vm: &mut Vm, args: techne_vm::api::Args<'_>| {
            let saving = saving.clone();
            let request = (|| {
                let d: Doc = args.get(vm, 0)?;
                let node: Foreign<techne_node::NodeRef> = args.get(vm, 1)?;
                let path: String = args.get(vm, 2)?;
                let overwrite: bool = args.get(vm, 3)?;
                saving
                    .start(&d.0, move |save| {
                        let expect = match (overwrite, save.base) {
                            (true, _) => techne_node::protocol::Expect::Any,
                            (false, None) => techne_node::protocol::Expect::Absent,
                            (false, Some(h)) => techne_node::protocol::Expect::Hash(h),
                        };
                        node.0.0.write_file_tracked(path.into_bytes(), save.bytes.clone(), expect)
                    })
                    .map_err(Error::new)
            })();
            async move { request.map_err(|e| e.into_inner().msg)?.await }
        });
        vm.register_async("%document-reload-remote!", 3, move |vm: &mut Vm, args: techne_vm::api::Args<'_>| {
            let saving = reload_saving.clone();
            let request = (|| {
                let d: Doc = args.get(vm, 0)?;
                let node: Foreign<techne_node::NodeRef> = args.get(vm, 1)?;
                let path: String = args.get(vm, 2)?;
                saving.settle(&d.0).map_err(Error::new)?;
                // What the document was when the file was asked for.
                let before = (d.borrow().revision(), d.borrow().disk_hash(), saving.generation(&d.0).map_err(Error::new)?);
                Ok::<_, Error>((d, before, node.0.0.read_file(path.into_bytes())))
            })();
            async move {
                let (d, before, read) = request.map_err(|e| e.into_inner().msg)?;
                let bytes = read.await?.map(|(bytes, _)| bytes);
                saving.settle(&d.0)?;
                let now = (d.borrow().revision(), d.borrow().disk_hash(), saving.generation(&d.0)?);
                if now != before {
                    return Err("the document changed while its file was read; read it again".to_string());
                }
                d.borrow_mut().reload_bytes(&Arc::from("disk"), bytes).map_err(|e| e.to_string())
            }
        });
        techne_vm::document! { vm;
            /// Return the document of the file at PATH on NODE, called NAME.
            /// NAME is its location; it is opened once, with its journal.
            "(%open-remote-file node path name)";
            /// Write the remote DOCUMENT to PATH on NODE, refusing an external
            /// change unless OVERWRITE.
            "(%document-save-remote! document node path overwrite)";
            /// Read the remote DOCUMENT's file at PATH on NODE again.
            "(%document-reload-remote! document node path)";
        }
    });
    // The entries of a directory, sorted, directories with a slash after
    // their name, for completing file names.
    vm.requiring(techne_vm::vm::Capability::Files, |vm| {
        techne_vm::procedures! { vm;
            /// Return the entries of DIRECTORY, sorted.
            /// Subdirectories have a slash after their name.
            "(directory-list directory)" => |dir: String| -> Result<Vec<String>, String> {
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
            };
            /// Return what is at PATH, or #f if nothing is there.
            /// It is (kind size modified mode target): KIND `file`,
            /// `directory`, `symlink` or `other`, a symbolic link not
            /// followed; MODIFIED in seconds since the epoch; MODE the
            /// permission bits; TARGET a link's target, else #f.
            "(file-attributes path)" => |path: String| fs::attributes(Path::new(&path)).map_err(|e| format!("{path}: {e}"));
            /// Move the file or directory FROM to TO, which must not exist.
            "(rename-file from to)" => |from: String, to: String| fs::rename(Path::new(&from), Path::new(&to)).map_err(|e| format!("{from} to {to}: {e}"));
            /// Write TEXT to the file PATH in place of what is there, at once:
            /// whole to a new file beside it, readable by its user only, then
            /// renamed over it. A write that fails leaves the file as it was.
            "(replace-file path text)" => |path: String, text: String| fs::replace(Path::new(&path), &text).map_err(|e| format!("{path}: {e}"));
            /// Copy the file or directory FROM to TO, which must not exist.
            /// A directory is copied with all it holds, a symbolic link as
            /// a link.
            "(copy-file from to)" => |from: String, to: String| fs::copy(Path::new(&from), Path::new(&to)).map_err(|e| format!("{from} to {to}: {e}"));
            /// Return PATH absolute, its symbolic links and . and .. resolved.
            /// A file that does not exist yet is named in its directory's
            /// true name.
            "(file-truename path)" => |path: String| techne_text::document::file_path(Path::new(&path)).map(|p| p.display().to_string()).map_err(|e| format!("{path}: {e}"));
            /// Make the directory PATH, and those above it that are missing.
            "(make-directory path)" => |path: String| std::fs::create_dir_all(&path).map_err(|e| format!("{path}: {e}"));
            /// Set the permission bits of the file or directory PATH to MODE.
            "(set-file-mode! path mode)" => |path: String, mode: usize| -> Result<(), String> {
                use std::os::unix::fs::PermissionsExt;
                let mode = u32::try_from(mode).map_err(|_| format!("set-file-mode!: {mode} is not a mode"))?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).map_err(|e| format!("{path}: {e}"))
            };
            /// Delete the file or directory at PATH.
            /// A directory must be empty unless RECURSIVE; a symbolic link
            /// is deleted, not what it points to.
            "(delete-path path recursive)" => |path: String, recursive: bool| fs::delete(Path::new(&path), recursive).map_err(|e| format!("{path}: {e}"));
            /// Return the first BYTES bytes of the file at PATH, as text.
            /// What is not UTF-8 is replaced; a character cut at the end is
            /// left out.
            "(file-head path bytes)" => |path: String, bytes: usize| fs::head(Path::new(&path), bytes).map_err(|e| format!("{path}: {e}"));
        }
    });
    // The spans (start end) of the text's top-level data, as the VM's
    // reader finds them; up to a malformed datum.
    techne_vm::procedures! { vm;
        /// Return the spans (start end) of the top-level data of TEXT.
        /// They are read as the VM reads them, up to a malformed datum.
        "(document-forms text)" => |t: TextArg| {
            syntax(&t.0).iter().map(|f| vec![f.span.0 as usize, f.span.1 as usize]).collect::<Vec<_>>()
        };
    }
    // The span (start end) of the datum that ends last before a position,
    // in the innermost list around it, as Emacs's eval-last-sexp takes it.
    techne_vm::procedures! { vm;
        /// Return the span (start end) of the datum before POSITION in TEXT.
        /// It is the last datum ending before POSITION in the innermost list
        /// around it, as Emacs's `eval-last-sexp` takes it; #f if none.
        "(document-datum-before text position)" => |t: TextArg, pos: usize| -> Option<Vec<usize>> {
            use techne_vm::reader::{Syntax, SyntaxKind};
            fn before(items: &[&Syntax], pos: u32) -> Option<(u32, u32)> {
                match items.iter().find(|s| s.span.0 < pos && pos < s.span.1) {
                    Some(s) => match &s.kind {
                        SyntaxKind::List(items, tail) => before(&items.iter().chain(tail.as_deref()).collect::<Vec<_>>(), pos),
                        SyntaxKind::Vector(items) | SyntaxKind::Dict(items) | SyntaxKind::Set(items) => before(&items.iter().collect::<Vec<_>>(), pos),
                        SyntaxKind::Labeled(_, inner) => before(&[inner], pos),
                        SyntaxKind::Atom(_) => None,
                    },
                    None => items.iter().rev().find(|s| s.span.1 <= pos).map(|s| s.span),
                }
            }
            let forms = syntax(&t.0);
            before(&forms.iter().collect::<Vec<_>>(), pos as u32).map(|(a, b)| vec![a as usize, b as usize])
        };
    }
    // Lisp indentation (`lisp_indent`): the specs of the heads indenting a
    // body, and the edits indenting lines.
    techne_vm::procedures! { vm;
        /// Return a table of how forms indent, holding Scheme's own.
        /// See `indent-spec-set!`.
        "(make-indent-specs)" => || Foreign::new(RefCell::new(lisp_indent::default_specs()));
        /// Make the form named NAME, a string, have N distinguished
        /// arguments in SPECS: they indent by 4, its body by 2. With N #f,
        /// it is a call again (but names starting with \"def\" have a body).
        "(indent-spec-set! specs name n)" => |specs: Specs, name: String, n: Option<usize>| {
            let mut specs = specs.borrow_mut();
            match n {
                Some(n) => specs.insert(name, n),
                None => specs.remove(&name),
            };
        };
        /// Return the span (start end) of the top-level form around POSITION
        /// in TEXT, or the last before it: from the line opening a list in
        /// its first column to where it closes, the text's end if it does
        /// not. #f if there is none.
        "(lisp-form-around text position)" => |t: TextArg, pos: usize| {
            lisp_indent::form_around(&t.0.rope(), pos).map(|r| vec![r.start, r.end])
        };
        /// Return the edits (from to text) indenting the lines starting at
        /// STARTS, sorted, in TEXT as Lisp: each replaces a line's leading
        /// blanks. Lines right already, and in strings, are left.
        "(lisp-indent-edits text starts specs)" => |t: TextArg, starts: Vec<usize>, specs: Specs| -> Result<Vec<Vec<Datum>>, String> {
            let rope = t.0.rope();
            if let Some(p) = starts.iter().find(|&&p| !char_boundary(&rope, p)) {
                return Err(format!("position {p} is not a character's start in the text"));
            }
            Ok(lisp_indent::edits(&rope, &starts, &specs.borrow())
                .into_iter()
                .map(|(from, to, text)| vec![Datum::Int(from), Datum::Int(to), Datum::Str(text)])
                .collect::<Vec<_>>())
        };
    }
    // Panics, as a native with a bug would: the world is retired and the
    // runtime ends (for testing that a frontend recovers: the restarted
    // runtime replays the journal).
    techne_vm::procedures! { vm;
        "(%crash-runtime)" => || -> i64 { panic!("%crash-runtime") };
    }
    // Returns once the file at `path` exists, as a native stuck in a system
    // call: no kill reaches it (for testing that the host replaces such a
    // runtime, and ignores it once it goes on).
    techne_vm::procedures! { vm;
        "(%hang-runtime path)" => |path: String| -> i64 {
            while !std::path::Path::new(&path).exists() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            0
        };
    }

    // Motions: (motion text position ...) -> position. A position is a
    // character's boundary: inside one, a motion would panic.
    let at = |t: &Text, pos: usize| -> Result<(), String> {
        let len = t.len();
        let rope = t.rope();
        if pos > len {
            Err(format!("position {pos} past the end ({len})"))
        } else if rope.char_to_byte(rope.byte_to_char(pos)) != pos {
            Err(format!("position {pos} is inside a character"))
        } else {
            Ok(())
        }
    };
    macro_rules! motion {
        ($name:literal, |$t:ident, $p:ident $(, $a:ident: $ty:ty)*| $body:expr) => {
            vm.register_fn($name, move |t: TextArg, $p: usize $(, $a: $ty)*| -> Result<_, String> {
                at(&t.0, $p)?;
                let rope = t.0.rope();
                let $t = &*rope;
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
    motion!("search-text", |t, p, needle: String, forward: bool, fold: bool| motion::search(t, p, &needle, forward, fold).map(span));
    // The start of the character a byte is in: a position for the motions,
    // from a byte count.
    techne_vm::procedures! { vm;
        /// Return the start of the character byte POSITION of TEXT is in.
        /// That is POSITION itself when a character starts there.
        "(char-start text position)" => |t: TextArg, pos: usize| -> Result<usize, String> {
            let rope = t.0.rope();
            if pos > rope.len_bytes() {
                return Err(format!("position {pos} past the end ({})", rope.len_bytes()));
            }
            Ok(rope.char_to_byte(rope.byte_to_char(pos)))
        };
    }
    techne_vm::document! { vm;
        /// Return the position after the grapheme at POSITION in TEXT.
        "(next-grapheme text position)";
        /// Return the position of the grapheme before POSITION in TEXT.
        "(prev-grapheme text position)";
        /// Return the end of the word at or after POSITION in TEXT.
        /// STYLE, "emacs" or "vim", says what a word is.
        "(word-end text position style)";
        /// Return the start of the word after POSITION in TEXT.
        /// STYLE, "emacs" or "vim", says what a word is.
        "(next-word-start text position style)";
        /// Return the start of the word at or before POSITION in TEXT.
        /// STYLE, "emacs" or "vim", says what a word is.
        "(word-start text position style)";
        /// Return the span (start end) of the word at POSITION in TEXT.
        /// STYLE, "emacs" or "vim", says what a word is; with AROUND, the
        /// space after it is included, as Vim's aw.
        "(word-object text position style around)";
        /// Return the start of the line of POSITION in TEXT.
        "(line-start text position)";
        /// Return the end of the line of POSITION in TEXT, before its break.
        "(line-end text position)";
        /// Return the span (start end) of the line of POSITION in TEXT.
        /// It includes the line break.
        "(line-span text position)";
        /// Return the column of POSITION in TEXT, in graphemes from 0.
        "(column text position)";
        /// Return the line of POSITION in TEXT, counting from 1.
        "(line-number text position)";
        /// Return the position COUNT lines below POSITION in TEXT.
        /// A negative COUNT goes up; the position is at the column GOAL, or the
        /// line's end if it is shorter.
        "(line-down text position count goal)";
        /// Return the span (start end) of the next NEEDLE from POSITION, or #f.
        /// It searches TEXT forward or, with FORWARD #f, backward; with FOLD,
        /// letters match whatever their case.
        "(search-text text position needle forward fold)";
    }
    // Regular expressions (SRFI 115's regexps) over the rope.
    let stop = vm.interrupt_handle();
    vm.register_fn("search-text-regexp", move |t: TextArg, pos: usize, re: Foreign<Regexp>, forward: bool| {
        at(&t.0, pos)?;
        let rope = t.0.rope();
        let found = regexp_search::search(&rope, &re, pos, forward, stop.flag());
        stopped(&stop, found).map(|m| m.map(span))
    });
    let stop = vm.interrupt_handle();
    vm.register_fn("search-text-regexp-all", move |t: TextArg, re: Foreign<Regexp>, from: usize, to: usize| {
        let rope = t.0.rope();
        let found = regexp_search::search_all(&rope, &re, from.min(rope.len_bytes()), to.min(rope.len_bytes()), stop.flag());
        stopped(&stop, found).map(|ms| ms.into_iter().map(span).collect::<Vec<_>>())
    });
    let stop = vm.interrupt_handle();
    vm.register_fn("string-search-regexp", move |text: String, re: Foreign<Regexp>| {
        let found = regexp_search::search(&Rope::from_str(&text), &re, 0, true, stop.flag());
        stopped(&stop, found).map(|m| m.map(span))
    });
    techne_vm::document! { vm;
        /// Return the span (start end) of REGEXP's first match in STRING, or #f.
        /// The span is in bytes.
        "(string-search-regexp string regexp)";
        /// Return the span (start end) of the next REGEXP from POSITION, or #f.
        /// It searches TEXT forward or, with FORWARD #f, backward: then the
        /// match is the last before POSITION of those found, not overlapping,
        /// from the start of TEXT. REGEXP is compiled by `regexp` of `(srfi 115)`.
        "(search-text-regexp text position regexp forward)";
        /// Return the spans (start end) of REGEXP's matches in TEXT.
        /// The matches start from FROM to before TO and do not overlap; empty
        /// ones are left out.
        "(search-text-regexp-all text regexp from to)";
    }
    techne_vm::procedures! { vm;
        /// Return the regular expression PATTERN, written as Perl and Rust's
        /// regex crate write them, compiled, for `string-search-regexp`
        /// and `search-text-regexp`, and as the regexps of `(srfi 115)`;
        /// it has no SRE, so `regexp->sre` of it is #f.
        "(string->regexp pattern)" => |pattern: String| -> Result<Foreign<Regexp>, String> {
            Regexp::new(&pattern, "#f".into(), Vec::new()).map(Foreign::new).map_err(|e| format!("string->regexp: {e}"))
        };
    }
    techne_vm::procedures! { vm;
        /// Return the spans (start end) of NEEDLE in TEXT from FROM to TO.
        /// With FOLD, letters match whatever their case.
        "(search-text-all text needle from to fold)" => |t: TextArg, needle: String, from: usize, to: usize, fold: bool| -> Result<Vec<Vec<usize>>, String> {
            let rope = t.0.rope();
            let to = to.min(rope.len_bytes());
            if !char_boundary(&rope, from) || !char_boundary(&rope, to) {
                return Err(format!("{from} {to} is not a span of characters in the text"));
            }
            Ok(motion::search_all(&rope, from, to, &needle, fold).into_iter().map(span).collect::<Vec<_>>())
        };
    }
    techne_vm::procedures! { vm;
        /// Return the starts of the lines of TEXT containing NEEDLE, in
        /// order, at most LIMIT of them. With FOLD, letters match whatever
        /// their case.
        "(lines-containing text needle fold limit)" => |t: TextArg, needle: String, fold: bool, limit: usize| {
            motion::lines_containing(&t.0.rope(), &needle, fold, limit)
        };
    }

    // Views.
    techne_vm::procedures! { vm;
        /// Return a new view of TEXT, a document or a presentation.
        /// Its edits are recorded as ACTOR's.
        "(make-view text actor)" => |t: TextArg, actor: String| Foreign::new(RefCell::new(View::new(t.0, &actor)));
    }
    techne_vm::procedures! { vm;
        /// Return a new view of VIEW's document with VIEW's carets and scroll.
        "(view-split view)" => |v: ViewArg| Foreign::new(RefCell::new(v.borrow_mut().split()));
    }
    techne_vm::procedures! { vm;
        /// Return a number identifying VIEW; compare views by it.
        "(view-id view)" => |v: ViewArg| v.borrow().id as i64;
    }
    techne_vm::procedures! { vm;
        /// Return the position of the first text VIEW shows.
        "(view-scroll view)" => |v: ViewArg| v.borrow_mut().scroll();
    }
    techne_vm::procedures! { vm;
        /// Make VIEW show its document from POSITION.
        "(view-set-scroll! view position)" => |v: ViewArg, pos: usize| {
            let revision = v.borrow().text.revision();
            v.borrow_mut().scroll_to(pos, revision)
        };
    }
    techne_vm::procedures! { vm;
        /// Return the text VIEW shows: a document or a presentation.
        "(view-document view)" => |v: ViewArg| v.borrow().text.clone();
    }
    techne_vm::procedures! { vm;
        /// Return the ranges of VIEW's selection, each (anchor head).
        "(view-ranges view)" => |v: ViewArg| {
            v.borrow_mut().selection().ranges().iter().map(|r| vec![r.anchor, r.head]).collect::<Vec<_>>()
        };
    }
    techne_vm::procedures! { vm;
        /// Return the index of the primary range of VIEW's selection.
        "(view-primary view)" => |v: ViewArg| v.borrow_mut().selection().primary_index();
    }
    techne_vm::procedures! { vm;
        /// Make RANGES, each (anchor head), the selection of VIEW.
        /// PRIMARY is the index of the primary range.
        "(view-set-ranges! view ranges primary)" => |v: ViewArg, ranges: Vec<Vec<usize>>, primary: usize| {
            let ranges = ranges
                .into_iter()
                .map(|r| match r[..] {
                    [anchor, head] => Ok(Range::new(anchor, head)),
                    _ => Err("view-set-ranges!: a range is (anchor head)".to_string()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            v.borrow_mut().set_selection(ranges, primary)
        };
    }
    // Edits of the text as it is now (a command that edits what it just
    // looked at), and of the text at a revision (one that waited between).
    techne_vm::procedures! { vm;
        /// Make EDITS, each (from to text), in VIEW's document as one change.
        /// GROUP is "new" for a new undo unit or "extend" to join the last one.
        /// Return the new revision; raise an error if the edit is refused (the
        /// view is read-only, or a lens cannot make it).
        "(view-edit! view edits group)" => |v: ViewArg, edits: Vec<EditArg>, group: String| -> Result<i64, String> {
            v.borrow_mut().edit(edit_args(edits), self::group("view-edit!", &group)?).map(|r| r as i64)
        };
    }
    techne_vm::procedures! { vm;
        /// Make EDITS, made against the text at revision BASE, in VIEW.
        /// They are moved past what changed since, or refused, saying by whom,
        /// when that touched the same text. This is for a command that looked at
        /// the text, waited, then edits what it saw. GROUP is as `view-edit!`
        /// takes it. Return the new revision.
        "(view-edit-at! view base edits group)" => |v: ViewArg, base: i64, edits: Vec<EditArg>, group: String| -> Result<i64, String> {
            v.borrow_mut().edit_at(base as Revision, edit_args(edits), self::group("view-edit-at!", &group)?).map(|r| r as i64)
        };
    }
    techne_vm::procedures! { vm;
        /// Return #t if VIEW refuses edits.
        "(view-read-only? view)" => |v: ViewArg| v.borrow().read_only;
    }
    techne_vm::procedures! { vm;
        /// Make VIEW refuse edits if READ-ONLY is true.
        "(set-view-read-only! view read-only)" => |v: ViewArg, read_only: bool| v.borrow_mut().read_only = read_only;
    }
    techne_vm::procedures! { vm;
        /// Undo the last change of VIEW's document; return the new revision.
        "(view-undo! view)" => |v: ViewArg| v.borrow_mut().revert(true).map(|r| r as i64);
    }
    techne_vm::procedures! { vm;
        /// Redo the last change undone in VIEW's document; return the revision.
        "(view-redo! view)" => |v: ViewArg| v.borrow_mut().revert(false).map(|r| r as i64);
    }

    // Presentations: rows by key (presentation.rs), excerpts edited through
    // (lens.rs).
    techne_vm::procedures! { vm;
        /// Return a new presentation, without rows.
        "(make-presentation)" => || Foreign::new(RefCell::new(Presentation::new()));
    }
    techne_vm::procedures! { vm;
        /// Return #t if OBJ is a presentation.
        #[vm]
        "(presentation? obj)" => |vm: &mut Vm, v: Value| vm.get::<Pres>(&v).is_ok();
    }
    techne_vm::procedures! { vm;
        /// Make ROWS what PRESENTATION shows, changing its text where they differ.
        /// A row is (key column ...), its key a string, unique; a column is a
        /// run or a list of runs, a run a string, (text face), or an excerpt
        /// (document from to [face]). Carets on a row that stays follow it.
        "(presentation-set-rows! presentation rows)" => |p: Pres, rows: Vec<RowArg>| p.borrow_mut().set_rows(rows.into_iter().map(|r| r.0).collect());
    }
    techne_vm::procedures! { vm;
        /// Return the key of the row of PRESENTATION at POSITION, or #f.
        "(presentation-key-at presentation position)" => |p: Pres, pos: usize| p.borrow().key_at(pos).map(str::to_string);
    }
    techne_vm::procedures! { vm;
        /// Return the span (start end) of the row KEY of PRESENTATION, or #f.
        /// It is without the line break.
        "(presentation-row-span presentation key)" => |p: Pres, key: String| p.borrow().row_span(&key).map(span);
    }
    techne_vm::procedures! { vm;
        /// Show the documents of PRESENTATION's excerpts as they are now.
        /// This includes where they changed under it.
        "(presentation-refresh! presentation)" => |p: Pres| p.borrow_mut().refresh();
    }
    techne_vm::procedures! { vm;
        /// Return the documents PRESENTATION's excerpts show, each once.
        "(presentation-sources presentation)" => |p: Pres| p.borrow().sources().into_iter().map(Foreign).collect::<Vec<_>>();
    }
    // Where a position is in the document an excerpt shows: (document
    // position), or #f.
    techne_vm::procedures! { vm;
        /// Return where POSITION of PRESENTATION is in an excerpt's document.
        /// The result is (document position): in the excerpt at POSITION, else
        /// the first of its row (for a position in its label); #f if none.
        "(presentation-source-at presentation position)" => |p: Pres, pos: usize| {
            p.borrow().source_at(pos).map(|(d, at)| vec![Datum::Doc(d), Datum::Int(at)])
        };
    }
}

/// The document's data as the VM's reader finds them, up to a malformed
/// datum.
fn syntax(t: &Text) -> Vec<techne_vm::reader::Syntax> {
    let text = t.rope().to_string();
    let forms = techne_vm::reader::read_syntax(&text).or_else(|e| {
        let end = e.pos.map_or(0, |p| p as usize).min(text.len());
        techne_vm::reader::read_syntax(&text[..end])
    });
    forms.unwrap_or_default()
}

pub(crate) fn edit_args(edits: Vec<EditArg>) -> Vec<(std::ops::Range<usize>, String)> {
    edits.into_iter().map(|EditArg(from, to, text)| (from..to, text)).collect()
}

fn group(name: &str, group: &str) -> Result<Group, String> {
    match group {
        "new" => Ok(Group::New),
        "extend" => Ok(Group::Extend),
        g => Err(format!("{name}: group is new or extend, not {g}")),
    }
}

fn words(style: &str) -> Result<Words, String> {
    match style {
        "emacs" => Ok(Words::Emacs),
        "vim" => Ok(Words::Vim),
        s => Err(format!("word style is emacs or vim, not {s}")),
    }
}

/// A search's result; an interrupt that stopped it is taken, as the
/// condition the search raises instead.
fn stopped<T>(stop: &techne_vm::vm::InterruptHandle, found: Result<T, String>) -> Result<T, String> {
    if found.as_ref().is_err_and(|e| e == techne_vm::vm::INTERRUPTED) {
        stop.take();
    }
    found
}

/// Whether byte `p` of `rope` is a character's start (or its end): a
/// position natives may slice at.
fn char_boundary(rope: &techne_text::ropey::Rope, p: usize) -> bool {
    p <= rope.len_bytes() && rope.char_to_byte(rope.byte_to_char(p)) == p
}

fn span(r: std::ops::Range<usize>) -> Vec<usize> {
    vec![r.start, r.end]
}
