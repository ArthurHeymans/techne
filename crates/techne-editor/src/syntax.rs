//! Syntax trees with tree-sitter: the bottom layer of a pane's highlights
//! (EDITOR.md, section 3), and the trees Lisp moves through
//! (`syntax-node-at` and the rest, `lisp/editor/syntax.scm`).
//!
//! Lisp says which languages there are (`define-language`): each is a
//! grammar built in here and its highlight queries, the grammar's own
//! unless Lisp gives others. A document's tree is kept with the text it
//! was parsed from and follows the document's edits: each change since
//! is replayed onto the tree (`Tree::edit`), and the edited tree is
//! parsed again incrementally. Until that parse lands, the edited tree
//! itself is highlighted: its nodes have moved with the text, so the
//! colours stay where they were rather than going away.
//!
//! An incremental parse after an edit is short; a first parse of a big
//! document, or a pathological one, is not, and a halted parse gives
//! nothing to resume from. So a parse that does not fit a snapshot's
//! budget goes to a worker thread, which wakes the runtime when the tree
//! lands (`Msg::Wake`).
//!
//! Highlights are the queries' captures, each capture's name naming a
//! face: tree-sitter's dotted names, which faces resolve by prefix, so
//! `keyword.control` needs no face of its own, and renaming one (a JSON
//! key, `string.special.key`) is a face declaration in Lisp. As the
//! queries are written for, a node gets the capture of the first
//! pattern that matches it; nested nodes paint over the nodes holding
//! them. Captures are collected over an aligned window and cached per
//! tree, so scrolling within it costs no query work again.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ops::ControlFlow,
    rc::{Rc, Weak},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use techne_text::{Document, Revision, ropey::Rope};
use tree_sitter::{
    InputEdit, Language, Node, ParseOptions, Parser, Point, Query, QueryCursor, QueryCursorOptions, Range, StreamingIterator, Tree,
};

use techne_vm::api::{FromValue, IntoValue};

use crate::{
    edits,
    faces::{FaceName, FaceRange, Faces},
    runtime::Msg,
};

/// The most text parsed for one document. Past it the text is plain and
/// commands moving by its tree say so, as Emacs's
/// `treesit-max-buffer-size`: a megabyte of generated or logged text would
/// cost every keystroke a parse's worth of the tree's upkeep, and its
/// first parse, freeing its trees, its whole-text queries, far more than
/// a keystroke may.
const MAX: usize = 512 << 10;

/// The most text a first parse is tried for in the snapshot that asks;
/// past it, the worker parses it. Later parses are incremental and are
/// tried here whatever the size.
const FIRST_SYNC_MAX: usize = 64 << 10;

/// Captures are collected over windows aligned to this many bytes.
const BUCKET: usize = 16 << 10;

/// How long one snapshot may spend parsing and collecting captures, in
/// step with the parts' deadlines being ten times as long in a debug
/// build (`runtime::PART_DEADLINE`).
const BUDGET: Duration = Duration::from_millis(if cfg!(debug_assertions) { 150 } else { 15 });

/// How long Lisp may wait for a tree it asked for.
const LISP_BUDGET: Duration = Duration::from_secs(2);

/// How long a collection that ran out of budget is not tried again:
/// a pathological document costs one budget per second rather than one
/// per snapshot, and a transient overrun (a loaded machine) colours
/// again on its own.
const COOLDOWN: Duration = Duration::from_secs(1);

/// The grammars built in.
pub const GRAMMARS: [&str; 13] =
    ["rust", "python", "c", "javascript", "typescript", "tsx", "go", "json", "yaml", "toml", "scheme", "bash", "markdown"];

/// A grammar and its own highlight queries. Markdown is two grammars,
/// block and inline, the inline one parsed over the block tree's inline
/// nodes.
enum Grammar {
    One { language: Language, highlights: &'static [&'static str] },
    Markdown,
}

fn grammar(name: &str) -> Option<Grammar> {
    let one = |l: tree_sitter_language::LanguageFn, highlights: &'static [&'static str]| Grammar::One { language: l.into(), highlights };
    Some(match name {
        "rust" => one(tree_sitter_rust::LANGUAGE, &[tree_sitter_rust::HIGHLIGHTS_QUERY]),
        "python" => one(tree_sitter_python::LANGUAGE, &[tree_sitter_python::HIGHLIGHTS_QUERY]),
        "c" => one(tree_sitter_c::LANGUAGE, &[tree_sitter_c::HIGHLIGHT_QUERY]),
        "javascript" => {
            one(tree_sitter_javascript::LANGUAGE, &[tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY])
        }
        // The TypeScript queries hold only what TypeScript adds; the
        // JavaScript base rides under them (and JSX, for TSX). These
        // queries are written for the last pattern to win a node, so the
        // specialisations go last.
        "typescript" => one(
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            &[tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY],
        ),
        "tsx" => one(
            tree_sitter_typescript::LANGUAGE_TSX,
            &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ],
        ),
        "go" => one(tree_sitter_go::LANGUAGE, &[tree_sitter_go::HIGHLIGHTS_QUERY]),
        "json" => one(tree_sitter_json::LANGUAGE, &[tree_sitter_json::HIGHLIGHTS_QUERY]),
        "yaml" => one(tree_sitter_yaml::LANGUAGE, &[tree_sitter_yaml::HIGHLIGHTS_QUERY]),
        "toml" => one(tree_sitter_toml_ng::LANGUAGE, &[tree_sitter_toml_ng::HIGHLIGHTS_QUERY]),
        "scheme" => one(tree_sitter_scheme::LANGUAGE, &[tree_sitter_scheme::HIGHLIGHTS_QUERY]),
        "bash" => one(tree_sitter_bash::LANGUAGE, &[tree_sitter_bash::HIGHLIGHT_QUERY]),
        "markdown" => Grammar::Markdown,
        _ => return None,
    })
}

/// A language as Lisp defined it.
struct Lang {
    /// Which definition of its name this is: a tree the worker parsed
    /// for an earlier one (another grammar, maybe) is not this one's.
    generation: u64,
    /// What its documents are parsed with (markdown's block grammar).
    parser: Language,
    /// The text of its highlight queries, each with the grammar it is
    /// for: one, or markdown's block and inline ones.
    sources: Vec<(Language, String)>,
    /// Those queries compiled, the first time the language colours a
    /// buffer: compiling a grammar's own queries takes some 200 M
    /// instructions, and thirteen of them at every start took a second.
    queries: Option<Vec<Query>>,
    /// Per query, each capture's face, or none for a capture private
    /// to the query (`_name`); made when first highlighting.
    faces: Option<Vec<Vec<Option<FaceName>>>>,
    markdown: bool,
    /// Whether the last pattern matching a node wins it, as Neovim's
    /// queries are written, rather than the first, as tree-sitter's
    /// own highlighter reads them. Upstream queries are written either
    /// way, so the language says.
    last_wins: bool,
}

impl Lang {
    /// The language of the grammar `grammar_name`, highlighted by
    /// `highlights` or the grammar's own queries; nothing is compiled
    /// yet (`check` compiles what a definition gives).
    fn new(grammar_name: &str, highlights: Option<&str>, last_wins: bool) -> Result<Lang, String> {
        let grammar = grammar(grammar_name).ok_or_else(|| format!("no grammar {grammar_name}"))?;
        Ok(match grammar {
            Grammar::One { language, highlights: own } => {
                let source = highlights.map_or_else(|| own.join("\n"), str::to_string);
                Lang {
                    generation: 0,
                    sources: vec![(language.clone(), source)],
                    queries: None,
                    parser: language,
                    faces: None,
                    markdown: false,
                    last_wins,
                }
            }
            Grammar::Markdown => {
                let block: Language = tree_sitter_md::LANGUAGE.into();
                let inline: Language = tree_sitter_md::INLINE_LANGUAGE.into();
                let sources = vec![
                    (block.clone(), highlights.unwrap_or(tree_sitter_md::HIGHLIGHT_QUERY_BLOCK).to_string()),
                    (inline, tree_sitter_md::HIGHLIGHT_QUERY_INLINE.to_string()),
                ];
                Lang { generation: 0, parser: block, sources, queries: None, faces: None, markdown: true, last_wins }
            }
        })
    }

    /// Its highlight queries, compiled now if they are not yet.
    fn queries(&mut self) -> Result<&[Query], String> {
        if self.queries.is_none() {
            let compiled = self
                .sources
                .iter()
                .map(|(language, source)| Query::new(language, source).map_err(|e| e.to_string()))
                .collect::<Result<Vec<_>, _>>()?;
            self.queries = Some(compiled);
        }
        Ok(self.queries.as_deref().expect("just compiled"))
    }

    fn faces(&mut self, faces: &mut Faces) -> Result<Vec<Vec<Option<FaceName>>>, String> {
        if self.faces.is_none() {
            let names: Vec<Vec<String>> =
                self.queries()?.iter().map(|q| q.capture_names().iter().map(|n| n.to_string()).collect()).collect();
            self.faces = Some(names.iter().map(|q| q.iter().map(|n| (!n.starts_with('_')).then(|| faces.name(n))).collect()).collect());
        }
        Ok(self.faces.clone().expect("just made"))
    }
}

/// Check that the grammar `grammar` compiles `highlights` and
/// `definitions`, each a query's text or none: what a definition gives
/// fails where it is written. The grammar's own queries are not compiled
/// here, the first highlighting does.
fn check(grammar_name: &str, highlights: Option<&str>, definitions: Option<&str>) -> Result<(), String> {
    let lang = Lang::new(grammar_name, highlights, false)?;
    if highlights.is_some() {
        Query::new(&lang.sources[0].0, &lang.sources[0].1).map_err(|e| format!("{grammar_name}: {e}"))?;
    }
    if let Some(q) = definitions {
        Query::new(&lang.parser, q).map_err(|e| format!("{grammar_name}: {e}"))?;
    }
    Ok(())
}

/// A tree that is freed on a thread of its own when dropped: a big
/// document's has millions of nodes, and freeing them where a keystroke
/// is being answered took a third of a billion instructions.
pub(crate) struct Freed(Option<Tree>);

impl Freed {
    fn new(tree: Tree) -> Freed {
        Freed(Some(tree))
    }
}

impl std::ops::Deref for Freed {
    type Target = Tree;
    fn deref(&self) -> &Tree {
        self.0.as_ref().expect("a tree until dropped")
    }
}

impl std::ops::DerefMut for Freed {
    fn deref_mut(&mut self) -> &mut Tree {
        self.0.as_mut().expect("a tree until dropped")
    }
}

impl Drop for Freed {
    fn drop(&mut self) {
        static REAPER: std::sync::OnceLock<Mutex<mpsc::Sender<Tree>>> = std::sync::OnceLock::new();
        let Some(tree) = self.0.take() else { return };
        let reaper = REAPER.get_or_init(|| {
            let (send, trees) = mpsc::channel::<Tree>();
            let started = std::thread::Builder::new().name("syntax-free".into()).spawn(move || for _ in trees {});
            if started.is_err() {
                // No thread: the tree is freed here, as any value is.
                let (send, _) = mpsc::channel();
                return Mutex::new(send);
            }
            Mutex::new(send)
        });
        // Should the thread be gone, the tree comes back and is freed here.
        let _ = reaper.lock().map(|s| s.send(tree));
    }
}

/// A document's tree.
struct Entry {
    document: Weak<RefCell<Document>>,
    language: Rc<str>,
    revision: Revision,
    /// The text at `revision`.
    rope: Rope,
    tree: Freed,
    /// Whether `tree` was parsed from `rope`, or only edited toward it.
    parsed: bool,
    /// Changes whenever `tree` does.
    generation: u64,
    /// Captures collected over [from, to) of the tree of `generation`:
    /// a few windows, the latest first, so panes showing different
    /// parts of one document each find theirs.
    collected: Vec<(u64, usize, usize, Vec<FaceRange>)>,
    /// When collecting over this tree last ran out of budget.
    gave_up: Option<Instant>,
}

/// What the worker is asked, and answers: owned, `Send` by what it
/// holds (the language's name is a `String`, not the `Rc` the runtime
/// shares).
struct Work {
    doc: u64,
    revision: Revision,
    language: String,
    generation: u64,
    parser: Language,
    rope: Rope,
    old: Option<Tree>,
    cancelled: Arc<AtomicBool>,
}

struct Done {
    doc: u64,
    revision: Revision,
    language: String,
    /// The language's generation it was parsed for.
    generation: u64,
    rope: Rope,
    tree: Freed,
}

/// A parse the worker has.
struct Pending {
    revision: Revision,
    language: String,
    document: Weak<RefCell<Document>>,
    cancelled: Arc<AtomicBool>,
}

/// How many windows of captures a document keeps.
const WINDOWS: usize = 4;

/// The most captures `syntax-query` gives: a query matching every pair
/// of siblings in a big file is an error, not a list of millions.
const MAX_QUERY_CAPTURES: usize = 1 << 18;

/// The languages Lisp defined, and the trees of documents.
pub struct Syntax {
    languages: HashMap<Rc<str>, Lang>,
    docs: HashMap<u64, Entry>,
    pending: HashMap<u64, Pending>,
    worker: Option<(mpsc::Sender<Work>, mpsc::Receiver<Done>)>,
    /// How the worker wakes the runtime when a tree lands, set by
    /// `serve`: shared with the worker, which may have started first.
    wake: Arc<Mutex<Option<mpsc::Sender<Msg>>>>,
    /// The deadline of the snapshot being made (`begin_snapshot`): one
    /// budget for all its panes, not one each.
    snapshot: Option<Instant>,
    /// Queries Lisp asked with, compiled, by grammar and text: a motion
    /// asks the same definitions query again and again, and compiling
    /// it took more than running it.
    compiled: HashMap<(String, String), Arc<Query>>,
    /// One snapshot's budget, and the part of it parsing may take: the
    /// rest is collecting's, so an edited tree whose parse did not fit
    /// is still coloured.
    budget: Duration,
    parse_budget: Duration,
    generation: u64,
}

impl Default for Syntax {
    fn default() -> Self {
        Syntax {
            languages: HashMap::new(),
            docs: HashMap::new(),
            pending: HashMap::new(),
            worker: None,
            wake: Arc::default(),
            snapshot: None,
            compiled: HashMap::new(),
            budget: BUDGET,
            parse_budget: BUDGET / 2,
            generation: 0,
        }
    }
}

impl Drop for Syntax {
    fn drop(&mut self) {
        for p in self.pending.values() {
            p.cancelled.store(true, Ordering::Relaxed);
        }
    }
}

/// A position of `rope` as tree-sitter counts: a row, and bytes into it.
fn point(rope: &Rope, at: usize) -> Point {
    let row = rope.byte_to_line(at);
    Point { row, column: at - rope.line_to_byte(row) }
}

/// Feed `rope` to a parse, a chunk at a time.
fn reader<'r>(rope: &'r Rope) -> impl FnMut(usize, Point) -> &'r [u8] {
    move |at, _| match rope.get_chunk_at_byte(at) {
        Some((chunk, start, _, _)) if at < rope.len_bytes() => &chunk.as_bytes()[at - start..],
        _ => &[],
    }
}

/// Parse `rope`, from `old` edited toward it, until `stop` says so.
fn parse(language: &Language, rope: &Rope, old: Option<&Tree>, ranges: &[Range], stop: &dyn Fn() -> bool) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    if !ranges.is_empty() {
        parser.set_included_ranges(ranges).ok()?;
    }
    let mut progress = |_: &tree_sitter::ParseState| if stop() { ControlFlow::Break(()) } else { ControlFlow::Continue(()) };
    let options = ParseOptions::new().progress_callback(&mut progress);
    parser.parse_with_options(&mut reader(rope), old, Some(options))
}

impl Syntax {
    /// Wake the runtime (a snapshot with no input) when the worker lands
    /// a tree.
    pub fn set_wake(&mut self, wake: mpsc::Sender<Msg>) {
        *self.wake.lock().expect("the wake") = Some(wake);
    }

    /// QUERY compiled for LANGUAGE, once.
    fn compiled(&mut self, language: &Language, query: &str) -> Result<Arc<Query>, String> {
        let key = (language.name().unwrap_or("").to_string(), query.to_string());
        if let Some(q) = self.compiled.get(&key) {
            return Ok(q.clone());
        }
        let q = Arc::new(Query::new(language, query).map_err(|e| e.to_string())?);
        // A few kinds of query are asked; a package generating new ones
        // without end is not kept from all of them.
        if self.compiled.len() >= 64 {
            self.compiled.clear();
        }
        self.compiled.insert(key, q.clone());
        Ok(q)
    }

    /// Start a snapshot: its panes share one budget from now.
    pub fn begin_snapshot(&mut self) {
        self.snapshot = Some(Instant::now() + self.budget);
    }

    /// Change one snapshot's budget (`BUDGET`): for a caller whose calls
    /// are not keystrokes being answered, as a benchmark under an
    /// instrumenting profiler.
    pub fn set_budget(&mut self, budget: Duration) {
        self.budget = budget;
        self.parse_budget = budget / 2;
    }

    /// Define the language `name`, parsed with the grammar `grammar`,
    /// highlighted by `highlights` or the grammar's own queries, the last
    /// pattern matching a node winning it if `last_wins`, else the first.
    /// A query that does not compile is an error, and leaves the language
    /// as it was.
    pub fn define_language(&mut self, name: &str, grammar: &str, highlights: Option<&str>, last_wins: bool) -> Result<(), String> {
        // What the definition gives is checked now; the grammar's own
        // queries compile when first used.
        if highlights.is_some() {
            check(grammar, highlights, None)?;
        }
        let mut lang = Lang::new(grammar, highlights, last_wins)?;
        self.forget_language(name);
        lang.generation = self.next_generation();
        self.languages.insert(name.into(), lang);
        Ok(())
    }

    /// Forget the language `name`, its documents' trees, and the parses
    /// the worker has of them.
    pub fn forget_language(&mut self, name: &str) {
        self.languages.remove(name);
        self.docs.retain(|_, e| &*e.language != name);
        self.pending.retain(|_, p| {
            let keep = p.language != name;
            if !keep {
                p.cancelled.store(true, Ordering::Relaxed);
            }
            keep
        });
    }

    /// The capture names of the language's highlight queries.
    pub fn capture_names(&mut self, language: &str) -> Option<Vec<String>> {
        let lang = self.languages.get_mut(language)?;
        let mut names: Vec<String> = lang.queries().ok()?.iter().flat_map(|q| q.capture_names().iter().map(|n| n.to_string())).collect();
        names.sort();
        names.dedup();
        Some(names)
    }

    fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// Bring the document's entry to its revision: its changes since
    /// replayed onto the tree, which is then edited, not parsed. An entry
    /// of another language, or whose revision the history no longer
    /// reaches, is dropped.
    fn follow(&mut self, d: &Document, language: &str) {
        let id = d.id();
        let Some(entry) = self.docs.get_mut(&id) else { return };
        if &*entry.language != language {
            self.docs.remove(&id);
            return;
        }
        if entry.revision == d.revision() {
            return;
        }
        let Some(changes) = edits::since(d, entry.revision) else {
            self.docs.remove(&id);
            return;
        };
        for change in &changes {
            for step in edits::steps(change) {
                let (start, old_end) = (point(&entry.rope, step.start), point(&entry.rope, step.end));
                edits::apply(&mut entry.rope, &step);
                let new_end_byte = step.start + step.text.len();
                entry.tree.edit(&InputEdit {
                    start_byte: step.start,
                    old_end_byte: step.end,
                    new_end_byte,
                    start_position: start,
                    old_end_position: old_end,
                    new_end_position: point(&entry.rope, new_end_byte),
                });
            }
        }
        debug_assert_eq!(entry.rope, *d.text());
        // The document's own rope, which shares its chunks.
        entry.rope = d.text().clone();
        entry.revision = d.revision();
        entry.parsed = false;
        self.generation += 1;
        entry.generation = self.generation;
        entry.collected.clear();
        entry.gave_up = None;
    }

    /// Install the trees the worker has landed, each brought to its
    /// document's revision.
    fn drain(&mut self) {
        let Some((_, done)) = &self.worker else { return };
        let landed: Vec<Done> = done.try_iter().collect();
        for done in landed {
            let Some(pending) = self.pending.remove(&done.doc) else { continue };
            let Some(document) = pending.document.upgrade() else { continue };
            // The language as it is defined now, not one since replaced.
            let Some((language, _)) = self.languages.get_key_value(done.language.as_str()).filter(|(_, l)| l.generation == done.generation)
            else {
                continue;
            };
            let language = language.clone();
            let better = self.docs.get(&done.doc).is_some_and(|e| e.parsed && e.revision >= done.revision && e.language == language);
            if better {
                continue;
            }
            let generation = self.next_generation();
            self.docs.insert(
                done.doc,
                Entry {
                    document: pending.document,
                    language: language.clone(),
                    revision: done.revision,
                    rope: done.rope,
                    tree: done.tree,
                    parsed: true,
                    generation,
                    collected: Vec::new(),
                    gave_up: None,
                },
            );
            self.follow(&document.borrow(), &language);
        }
    }

    /// Ask the worker for a tree of the document as it is, from `old`,
    /// unless it is parsing one of it already.
    fn request(&mut self, d: &Rc<RefCell<Document>>, language: &Rc<str>, old: Option<Tree>) {
        let doc = d.borrow();
        if self.pending.contains_key(&doc.id()) {
            return;
        }
        let Some(lang) = self.languages.get(language) else { return };
        let wake = self.wake.clone();
        let (send, _) = self.worker.get_or_insert_with(|| worker(wake));
        let cancelled = Arc::new(AtomicBool::new(false));
        let work = Work {
            doc: doc.id(),
            revision: doc.revision(),
            language: language.to_string(),
            generation: lang.generation,
            parser: lang.parser.clone(),
            rope: doc.text().clone(),
            old,
            cancelled: cancelled.clone(),
        };
        if send.send(work).is_ok() {
            self.pending.insert(
                doc.id(),
                Pending { revision: doc.revision(), language: language.to_string(), document: Rc::downgrade(d), cancelled },
            );
        }
    }

    /// Bring the document's tree up to date by `deadline`: parsed here if
    /// it can be, else edited toward the text (or none yet, on a first
    /// parse) and the worker asked. True when the tree is parsed.
    fn update(&mut self, d: &Rc<RefCell<Document>>, language: &Rc<str>, deadline: Instant, worker: bool) -> bool {
        self.drain();
        let doc = d.borrow();
        let id = doc.id();
        self.follow(&doc, language);
        if self.docs.get(&id).is_some_and(|e| e.parsed) {
            return true;
        }
        let Some(lang) = self.languages.get(language) else { return false };
        let old = self.docs.get(&id);
        // Parsed here only the first time, and only a small document: so a
        // file opens coloured. Every parse after it is the worker's, the
        // edited tree colouring meanwhile — a parse cannot be stopped
        // inside a token, and one edit can make a token of the rest of
        // the text (a `|` opening a Scheme symbol, a quote a string), so
        // no budget bounds what a parse here would cost a keystroke.
        let here = old.is_none() && (doc.len() <= FIRST_SYNC_MAX || !worker);
        let busy = self.pending.get(&id).is_some_and(|p| p.revision >= doc.revision() || worker);
        if here && !(worker && busy) {
            let parsed = parse(&lang.parser, doc.text(), old.map(|e| &*e.tree), &[], &|| Instant::now() >= deadline);
            if let Some(tree) = parsed {
                let generation = self.next_generation();
                self.docs.insert(
                    id,
                    Entry {
                        document: Rc::downgrade(d),
                        language: language.clone(),
                        revision: doc.revision(),
                        rope: doc.text().clone(),
                        tree: Freed::new(tree),
                        parsed: true,
                        generation,
                        collected: Vec::new(),
                        gave_up: None,
                    },
                );
                if let Some(p) = self.pending.remove(&id) {
                    p.cancelled.store(true, Ordering::Relaxed);
                }
                return true;
            }
        }
        if worker {
            let old = self.docs.get(&id).map(|e| (*e.tree).clone());
            drop(doc);
            self.request(d, language, old);
        }
        false
    }

    /// The face ranges of the document's syntax between `from` and `to`,
    /// as `language` highlights them: none for a language not defined,
    /// or while a first parse is on its way.
    pub fn highlights(&mut self, d: &Rc<RefCell<Document>>, language: &str, from: usize, to: usize, faces: &mut Faces) -> Vec<FaceRange> {
        let Some((language, _)) = self.languages.get_key_value(language) else { return Vec::new() };
        let language = language.clone();
        if d.borrow().len() > MAX {
            return Vec::new();
        }
        // The language's queries compiled the first time it colours, and
        // not on its budget: compiling one takes longer than a snapshot's
        // budget, which would then give up and leave the text plain.
        if let Some(lang) = self.languages.get_mut(&language)
            && lang.queries.is_none()
        {
            if lang.queries().is_err() {
                return Vec::new();
            }
            self.snapshot = self.snapshot.map(|_| Instant::now() + self.budget);
        }
        let start = Instant::now();
        // This pane's budget, within what the snapshot has left.
        let deadline = self.snapshot.map_or(start + self.budget, |s| s.min(start + self.budget));
        self.update(d, &language, (start + self.parse_budget).min(deadline), true);
        let id = d.borrow().id();
        let (Some(entry), Some(lang)) = (self.docs.get_mut(&id), self.languages.get_mut(&language)) else {
            return Vec::new();
        };
        let inside = |ranges: &[FaceRange]| ranges.iter().filter(|r| r.from < to && r.to > from).copied().collect::<Vec<_>>();
        if let Some(i) =
            entry.collected.iter().position(|(generation, cf, ct, _)| *generation == entry.generation && *cf <= from && to <= *ct)
        {
            let hit = entry.collected.remove(i);
            let answer = inside(&hit.3);
            entry.collected.insert(0, hit);
            return answer;
        }
        if entry.gave_up.is_some_and(|at| at.elapsed() < COOLDOWN) {
            return Vec::new();
        }
        // Over the aligned window the cache keeps, not only what was
        // asked: the next scroll asks from inside it.
        let (af, at) = (from / BUCKET * BUCKET, (to.div_ceil(BUCKET) * BUCKET).min(entry.rope.len_bytes()));
        match collect(lang, faces, entry, af, at, deadline) {
            Some(ranges) => {
                let answer = inside(&ranges);
                entry.collected.retain(|c| c.0 == entry.generation);
                entry.collected.insert(0, (entry.generation, af, at, ranges));
                entry.collected.truncate(WINDOWS);
                answer
            }
            None => {
                entry.gave_up = Some(Instant::now());
                Vec::new()
            }
        }
    }

    /// The document's tree, parsed from its text as it is, for Lisp.
    fn tree(&mut self, d: &Rc<RefCell<Document>>, language: &str) -> Result<(Tree, Rope), String> {
        let Some((language, _)) = self.languages.get_key_value(language) else { return Err(format!("no language {language}")) };
        let language = language.clone();
        if d.borrow().len() > MAX {
            return Err("syntax: the text is too long to parse".into());
        }
        // As for a snapshot: parsed here only within its budget, a big
        // document's first parse left to the worker. A command never
        // waits seconds on the main thread for a tree: it takes the one
        // there is (edited toward the text, if its parse did not fit),
        // or, before the first, says to try again.
        self.update(d, &language, Instant::now() + self.parse_budget, true);
        match self.docs.get(&d.borrow().id()) {
            Some(entry) => Ok(((*entry.tree).clone(), entry.rope.clone())),
            None => Err("syntax: this buffer is still being parsed; try again in a moment".into()),
        }
    }

    /// Forget the trees of documents that are gone.
    pub fn retain(&mut self) {
        self.docs.retain(|_, e| e.document.strong_count() > 0);
        self.pending.retain(|_, p| {
            let keep = p.document.strong_count() > 0;
            if !keep {
                p.cancelled.store(true, Ordering::Relaxed);
            }
            keep
        });
    }
}

/// The captures of the entry's tree over [from, to), as face ranges
/// ordered to compose: a node holding another before it. None when the
/// deadline ran out partway.
fn collect(lang: &mut Lang, faces: &mut Faces, entry: &Entry, from: usize, to: usize, deadline: Instant) -> Option<Vec<FaceRange>> {
    let (markdown, last_wins) = (lang.markdown, lang.last_wins);
    let capture_faces = lang.faces(faces).ok()?;
    let queries = lang.queries().ok()?;
    // The winning capture of each range.
    let mut won: HashMap<(usize, usize), (usize, FaceName)> = HashMap::new();
    let rope = &entry.rope;
    let root = entry.tree.root_node();
    if !captures(&queries[0], &capture_faces[0], last_wins, root, rope, from, to, deadline, &mut won) {
        return None;
    }
    if markdown {
        // The inline grammar over the window's inline nodes, each parsed
        // alone: one parse over every range would read unrelated
        // paragraphs as one stream.
        let inline: Language = tree_sitter_md::INLINE_LANGUAGE.into();
        for range in inline_ranges(root, from, to, deadline)? {
            let tree = parse(&inline, rope, None, &[range], &|| Instant::now() >= deadline)?;
            if !captures(&queries[1], &capture_faces[1], last_wins, tree.root_node(), rope, from, to, deadline, &mut won) {
                return None;
            }
        }
    }
    let mut out: Vec<FaceRange> = won.into_iter().map(|((from, to), (_, face))| FaceRange { from, to, face }).collect();
    out.sort_by(|a, b| (b.to - b.from).cmp(&(a.to - a.from)).then(a.from.cmp(&b.from)));
    Some(out)
}

/// A query's captures under `root` over [from, to) into `won`: for each
/// range, the capture of the first pattern matching it, or the last if
/// `last_wins`. False when the deadline ran out partway.
#[allow(clippy::too_many_arguments)]
fn captures(
    query: &Query,
    faces: &[Option<FaceName>],
    last_wins: bool,
    root: Node,
    rope: &Rope,
    from: usize,
    to: usize,
    deadline: Instant,
    won: &mut HashMap<(usize, usize), (usize, FaceName)>,
) -> bool {
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(from..to);
    let ran_out = Cell::new(false);
    let mut progress = |_: &tree_sitter::QueryCursorState| {
        if Instant::now() < deadline {
            ControlFlow::Continue(())
        } else {
            ran_out.set(true);
            ControlFlow::Break(())
        }
    };
    let options = QueryCursorOptions::new().progress_callback(&mut progress);
    let text = |node: Node| rope.get_byte_slice(node.byte_range()).into_iter().flat_map(|s| s.chunks()).map(str::as_bytes);
    let mut matches = cursor.matches_with_options(query, root, text, options);
    let mut seen = 0usize;
    while let Some(m) = matches.next() {
        seen += 1;
        if seen.is_multiple_of(512) && Instant::now() >= deadline {
            return false;
        }
        for capture in m.captures() {
            let Some(face) = faces[capture.index as usize] else { continue };
            let range = capture.node.byte_range();
            if range.start >= range.end {
                continue;
            }
            let entry = won.entry((range.start, range.end)).or_insert((m.pattern_index, face));
            if (m.pattern_index < entry.0) != last_wins && m.pattern_index != entry.0 {
                *entry = (m.pattern_index, face);
            }
        }
    }
    !ran_out.get()
}

/// The block tree's inline containers over a window, as ranges: inline
/// nodes, and table cells, whose text the inline grammar reads. None
/// when the deadline ran out partway.
///
/// A node starting before the window enters its children at the first
/// one holding or following the window's start, and stops at the first
/// past its end: a big document's root has a child per block, and
/// enumerating them all for a window's worth would be most of a budget.
fn inline_ranges(root: Node, from: usize, to: usize, deadline: Instant) -> Option<Vec<Range>> {
    let container = |n: Node| n.kind() == "inline" || n.kind() == "pipe_table_cell";
    let mut out = Vec::new();
    let mut walk = vec![root];
    let mut seen = 0usize;
    let mut tick = || {
        seen += 1;
        !(seen.is_multiple_of(128) && Instant::now() >= deadline)
    };
    while let Some(node) = walk.pop() {
        if !tick() {
            return None;
        }
        if node.end_byte() <= from || node.start_byte() >= to {
            continue;
        }
        if container(node) {
            out.push(node.range());
            continue;
        }
        let mut cursor = node.walk();
        let entered = if node.start_byte() < from { cursor.goto_first_child_for_byte(from).is_some() } else { cursor.goto_first_child() };
        if !entered {
            continue;
        }
        let mut children = Vec::new();
        loop {
            let child = cursor.node();
            if child.start_byte() >= to {
                break;
            }
            if child.end_byte() > from {
                children.push(child);
            }
            if !tick() {
                return None;
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        walk.extend(children.into_iter().rev());
    }
    out.sort_by_key(|r| r.start_byte);
    Some(out)
}

/// The worker: parses documents off the runtime's thread, and wakes the
/// runtime as trees land.
fn worker(wake: Arc<Mutex<Option<mpsc::Sender<Msg>>>>) -> (mpsc::Sender<Work>, mpsc::Receiver<Done>) {
    let (ask, asked) = mpsc::channel::<Work>();
    let (answer, answers) = mpsc::channel::<Done>();
    std::thread::Builder::new()
        .name("syntax".into())
        .spawn(move || {
            while let Ok(work) = asked.recv() {
                let cancelled = work.cancelled.clone();
                if cancelled.load(Ordering::Relaxed) {
                    continue;
                }
                let Some(tree) = parse(&work.parser, &work.rope, work.old.as_ref(), &[], &|| cancelled.load(Ordering::Relaxed)) else {
                    continue;
                };
                let done = Done {
                    doc: work.doc,
                    revision: work.revision,
                    language: work.language,
                    generation: work.generation,
                    rope: work.rope,
                    tree: Freed::new(tree),
                };
                if answer.send(done).is_err() {
                    return;
                }
                if let Some(wake) = &*wake.lock().expect("the wake") {
                    let _ = wake.send(Msg::Wake);
                }
            }
        })
        .expect("the syntax worker starts");
    (ask, answers)
}

/// The syntax shared by the runtime and Lisp's natives.
pub type Shared = Rc<RefCell<Syntax>>;

/// A node as Lisp sees it: (kind start end named?).
type NodeArg = (String, usize, usize, bool);

fn node_arg(n: Node) -> LispNode {
    LispNode((n.kind().to_string(), n.start_byte(), n.end_byte(), n.is_named()))
}

/// An element of a list given to Lisp.
enum Item {
    Str(String),
    Int(usize),
    Bool(bool),
}

impl IntoValue for Item {
    fn into_value(self, vm: &mut techne_vm::vm::Vm) -> Result<techne_vm::api::Value, techne_vm::vm::Error> {
        match self {
            Item::Str(s) => s.into_value(vm),
            Item::Int(n) => n.into_value(vm),
            Item::Bool(b) => b.into_value(vm),
        }
    }
}

/// A node, to and from Lisp.
struct LispNode(NodeArg);

impl IntoValue for LispNode {
    fn into_value(self, vm: &mut techne_vm::vm::Vm) -> Result<techne_vm::api::Value, techne_vm::vm::Error> {
        let (kind, start, end, named) = self.0;
        vec![Item::Str(kind), Item::Int(start), Item::Int(end), Item::Bool(named)].into_value(vm)
    }
}

impl FromValue for LispNode {
    fn from_arg(arg: techne_vm::api::Arg<'_>) -> Result<Self, techne_vm::vm::Error> {
        match arg.items().as_deref() {
            Some([kind, start, end, named]) => {
                Ok(LispNode((String::from_arg(*kind)?, usize::from_arg(*start)?, usize::from_arg(*end)?, bool::from_arg(*named)?)))
            }
            _ => Err(arg.type_error("syntax node (kind start end named?)")),
        }
    }
}

/// Where `down-list` goes from `from` under `root`: forward, the first
/// node (in the text's order) starting at or after `from` that holds
/// others and whose inside starts after `from`; backward, of those
/// ending at or before `from`, the one ending last (the outer of two
/// ending alike), its inside's end before `from`.
fn down(root: Node, from: usize, forward: bool) -> Option<usize> {
    // Past an opening token (an anonymous first child, with more after
    // it), else at the first child; the mirror backward.
    let inside = |n: Node| -> Option<usize> {
        let count = n.child_count();
        if count == 0 {
            return None;
        }
        if forward {
            let first = n.child(0)?;
            Some(if !first.is_named() && count > 1 { first.end_byte() } else { first.start_byte() })
        } else {
            let last = n.child(count - 1)?;
            Some(if !last.is_named() && count > 1 { last.start_byte() } else { last.end_byte() })
        }
    };
    let mut cursor = root.walk();
    let mut best: Option<(usize, usize)> = None;
    // Depth first, in the text's order, past subtrees that cannot hold
    // the answer.
    loop {
        let n = cursor.node();
        let skip = if forward { n.end_byte() <= from } else { n.start_byte() >= from };
        if !skip {
            if n.id() != root.id() {
                if forward && n.start_byte() >= from {
                    if let Some(at) = inside(n).filter(|&at| at > from) {
                        return Some(at);
                    }
                } else if !forward
                    && n.end_byte() <= from
                    && let Some(at) = inside(n).filter(|&at| at < from)
                    && best.is_none_or(|(end, _)| n.end_byte() > end)
                {
                    best = Some((n.end_byte(), at));
                }
            }
            if cursor.goto_first_child() {
                continue;
            }
        }
        if cursor.goto_next_sibling() {
            continue;
        }
        loop {
            if !cursor.goto_parent() {
                return best.map(|(_, at)| at);
            }
            if cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// The node of `tree` that `node` describes.
fn find<'t>(tree: &'t Tree, node: &NodeArg) -> Option<Node<'t>> {
    let mut n = tree.root_node().descendant_for_byte_range(node.1, node.2)?;
    loop {
        if (n.start_byte(), n.end_byte()) != (node.1, node.2) {
            return None;
        }
        if n.kind() == node.0 && n.is_named() == node.3 {
            return Some(n);
        }
        n = n.parent()?;
    }
}

type Doc = techne_vm::api::Foreign<RefCell<Document>>;

/// Define the syntax natives in `vm`, on `syntax`.
pub fn install(vm: &mut techne_vm::vm::Vm, syntax: Shared) {
    let (s10, s11, s12) = (syntax.clone(), syntax.clone(), syntax.clone());
    let (s1, s2, s3, s4, s5, s6, s7, s8, s9) = (
        syntax.clone(),
        syntax.clone(),
        syntax.clone(),
        syntax.clone(),
        syntax.clone(),
        syntax.clone(),
        syntax.clone(),
        syntax.clone(),
        syntax,
    );
    techne_vm::procedures! { vm;
        /// Return the names of the grammars built in, as strings.
        "(syntax-grammars)" => || GRAMMARS.iter().map(|g| g.to_string()).collect::<Vec<_>>();
        /// Define the syntax language NAME, a string, parsed with GRAMMAR,
        /// one of `syntax-grammars`. HIGHLIGHTS is the text of its
        /// highlight queries, or #f for the grammar's own; of the patterns
        /// matching a node, the last wins it if LAST-WINS, else the first.
        /// A query that does not compile is an error.
        "(syntax-define-language! name grammar highlights last-wins)" => move |name: String, grammar: String, highlights: Option<String>, last_wins: bool| -> Result<(), String> {
            s1.borrow_mut().define_language(&name, &grammar, highlights.as_deref(), last_wins)
        };
        /// Check that GRAMMAR, one of `syntax-grammars`, compiles its
        /// HIGHLIGHTS and DEFINITIONS queries, each the text of one or #f:
        /// raise the error of one that does not.
        "(syntax-check-language grammar highlights definitions)" => |grammar: String, highlights: Option<String>, definitions: Option<String>| -> Result<(), String> {
            check(&grammar, highlights.as_deref(), definitions.as_deref())
        };
        /// Forget the syntax language NAME and its documents' trees.
        "(syntax-forget-language! name)" => move |name: String| s2.borrow_mut().forget_language(&name);
        /// Return the capture names of the highlight queries of the syntax
        /// language NAME, or #f when there is no such language.
        "(syntax-capture-names name)" => move |name: String| s3.borrow_mut().capture_names(&name);
        /// Return the root node of DOCUMENT's tree in LANGUAGE.
        /// A node is (kind start end named?), KIND a string. The tree is
        /// parsed from the document's text as it is, if it was not.
        "(syntax-root document language)" => move |d: Doc, language: String| -> Result<LispNode, String> {
            let (tree, _) = s4.borrow_mut().tree(&d.0, &language)?;
            Ok(node_arg(tree.root_node()))
        };
        /// Return the smallest node of DOCUMENT's tree in LANGUAGE that
        /// holds FROM to TO, or #f. Only named nodes when NAMED is true.
        "(syntax-node-at document language from to named)" => move |d: Doc, language: String, from: usize, to: usize, named: bool| -> Result<Option<LispNode>, String> {
            let (tree, _) = s5.borrow_mut().tree(&d.0, &language)?;
            let root = tree.root_node();
            let n = if named { root.named_descendant_for_byte_range(from, to) } else { root.descendant_for_byte_range(from, to) };
            Ok(n.map(node_arg))
        };
        /// Return the parent of NODE in DOCUMENT's tree in LANGUAGE, or #f.
        "(syntax-node-parent document language node)" => move |d: Doc, language: String, node: LispNode| -> Result<Option<LispNode>, String> {
            let (tree, _) = s6.borrow_mut().tree(&d.0, &language)?;
            Ok(find(&tree, &node.0).and_then(|n| n.parent()).map(node_arg))
        };
        /// Return where `down-list` goes from FROM in DOCUMENT's tree in
        /// LANGUAGE: into the first node after FROM that holds others, past
        /// its opening token (or to its first child), or with FORWARD #f
        /// into the last one before FROM, before its closing. Return #f
        /// when there is none. The search is the tree's, not one call per
        /// node: a big file with no list in it answers at once.
        "(syntax-down document language from forward)" => move |d: Doc, language: String, from: usize, forward: bool| -> Result<Option<usize>, String> {
            let (tree, _) = s9.borrow_mut().tree(&d.0, &language)?;
            Ok(down(tree.root_node(), from, forward))
        };
        /// Return the children of NODE in DOCUMENT's tree in LANGUAGE, in
        /// order, named and anonymous.
        "(syntax-node-children document language node)" => move |d: Doc, language: String, node: LispNode| -> Result<Vec<LispNode>, String> {
            let (tree, _) = s7.borrow_mut().tree(&d.0, &language)?;
            let Some(n) = find(&tree, &node.0) else { return Ok(Vec::new()) };
            let mut cursor = n.walk();
            Ok(n.children(&mut cursor).map(node_arg).collect())
        };
        /// Return the child of NODE nearest POSITION in DOCUMENT's tree in
        /// LANGUAGE: the first starting at or after it, or with FORWARD #f
        /// the last ending at or before it; #f when there is none. A node
        /// with ten thousand children (a big file's top level) answers
        /// without making a list of them.
        "(syntax-node-child-near document language node position forward)" => move |d: Doc, language: String, node: LispNode, position: usize, forward: bool| -> Result<Option<LispNode>, String> {
            let (tree, _) = s11.borrow_mut().tree(&d.0, &language)?;
            let Some(n) = find(&tree, &node.0) else { return Ok(None) };
            // The child holding or following POSITION, found by the tree
            // (a search, not a walk), then its neighbours.
            let count = n.child_count();
            if count == 0 {
                return Ok(None);
            }
            let around = n.first_child_for_byte(position).unwrap_or_else(|| n.child(count - 1).expect("a last child"));
            if forward {
                let mut c = Some(around);
                while let Some(x) = c {
                    if x.start_byte() >= position {
                        return Ok(Some(node_arg(x)));
                    }
                    c = x.next_sibling();
                }
                Ok(None)
            } else {
                let mut c = Some(around);
                while let Some(x) = c {
                    if x.end_byte() <= position {
                        return Ok(Some(node_arg(x)));
                    }
                    c = x.prev_sibling();
                }
                Ok(None)
            }
        };
        /// Return how many children NODE has in DOCUMENT's tree in LANGUAGE.
        "(syntax-node-child-count document language node)" => move |d: Doc, language: String, node: LispNode| -> Result<usize, String> {
            let (tree, _) = s12.borrow_mut().tree(&d.0, &language)?;
            Ok(find(&tree, &node.0).map_or(0, |n| n.child_count() as usize))
        };
        /// Return the captures of QUERY, a tree-sitter query's text, over
        /// FROM to TO of DOCUMENT's tree in LANGUAGE: a list of (capture
        /// start end), CAPTURE the name without its @, in the order they
        /// occur. A query that does not compile is an error.
        "(syntax-query document language query from to)" => move |d: Doc, language: String, query: String, from: usize, to: usize| -> Result<Vec<Vec<Item>>, String> {
            let (tree, rope) = s8.borrow_mut().tree(&d.0, &language)?;
            let query = s8.borrow_mut().compiled(&tree.language(), &query).map_err(|e| format!("syntax-query: {e}"))?;
            run_query(&tree, &rope, &query, from, to, Instant::now() + LISP_BUDGET)
        };
    }
    // The same on a thread of its own, the calling task waiting: for what
    // goes over the whole of a big document (imenu), which the main thread
    // must not.
    vm.register_async("syntax-query-later", 5, move |vm: &mut techne_vm::vm::Vm, args: techne_vm::api::Args<'_>| {
        let asked = (|| -> Result<_, String> {
            let d: Doc = args.get(vm, 0).map_err(|e| e.msg.clone())?;
            let language: String = args.get(vm, 1).map_err(|e| e.msg.clone())?;
            let query: String = args.get(vm, 2).map_err(|e| e.msg.clone())?;
            let from: usize = args.get(vm, 3).map_err(|e| e.msg.clone())?;
            let to: usize = args.get(vm, 4).map_err(|e| e.msg.clone())?;
            let (tree, rope) = s10.borrow_mut().tree(&d.0, &language)?;
            let query = s10.borrow_mut().compiled(&tree.language(), &query).map_err(|e| format!("syntax-query-later: {e}"))?;
            Ok((tree, rope, query, from, to))
        })();
        async move {
            let (tree, rope, query, from, to) = asked?;
            OnThread::spawn(move || run_query(&tree, &rope, &query, from, to, Instant::now() + LATER_BUDGET)).await
        }
    });
    techne_vm::document! { vm;
        /// As `syntax-query`, on a thread of its own: the calling task waits,
        /// the editor does not. For a query over the whole of a big document.
        "(syntax-query-later document language query from to)";
    }
}

/// How long a query on its own thread may run.
const LATER_BUDGET: Duration = Duration::from_secs(10);

/// The captures of `query` over `from`..`to` of `tree` (whose text is
/// `rope`), each (capture start end), bounded by `deadline` and by
/// `MAX_QUERY_CAPTURES`.
fn run_query(tree: &Tree, rope: &Rope, query: &Query, from: usize, to: usize, deadline: Instant) -> Result<Vec<Vec<Item>>, String> {
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(from..to);
    let ran_out = Cell::new(false);
    let mut progress = |_: &tree_sitter::QueryCursorState| {
        if Instant::now() < deadline {
            ControlFlow::Continue(())
        } else {
            ran_out.set(true);
            ControlFlow::Break(())
        }
    };
    let options = QueryCursorOptions::new().progress_callback(&mut progress);
    let text = |node: Node| rope.get_byte_slice(node.byte_range()).into_iter().flat_map(|s| s.chunks()).map(str::as_bytes);
    let mut out = Vec::new();
    let mut captures = cursor.captures_with_options(query, tree.root_node(), text, options);
    while let Some((m, i)) = captures.next() {
        if out.len() >= MAX_QUERY_CAPTURES {
            return Err(format!("syntax-query: more than {MAX_QUERY_CAPTURES} captures"));
        }
        if out.len() % 1024 == 1023 && Instant::now() >= deadline {
            ran_out.set(true);
            break;
        }
        let c = m.captures()[*i];
        out.push(vec![
            Item::Str(query.capture_names()[c.index as usize].to_string()),
            Item::Int(c.node.start_byte()),
            Item::Int(c.node.end_byte()),
        ]);
    }
    if ran_out.get() {
        return Err("syntax-query: the query took too long".into());
    }
    Ok(out)
}

/// What a thread of its own computes, as a future the calling task waits on.
struct OnThread<T> {
    shared: Arc<Mutex<(Option<T>, Option<std::task::Waker>)>>,
}

impl<T: Send + 'static> OnThread<T> {
    fn spawn(work: impl FnOnce() -> T + Send + 'static) -> OnThread<T> {
        let shared = Arc::new(Mutex::new((None, None::<std::task::Waker>)));
        let theirs = shared.clone();
        std::thread::Builder::new()
            .name("syntax-query".into())
            .spawn(move || {
                let result = work();
                let mut slot = theirs.lock().expect("the result");
                slot.0 = Some(result);
                if let Some(w) = slot.1.take() {
                    w.wake();
                }
            })
            .expect("a query thread starts");
        OnThread { shared }
    }
}

impl<T> std::future::Future for OnThread<T> {
    type Output = T;
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<T> {
        let mut slot = self.shared.lock().expect("the result");
        match slot.0.take() {
            Some(t) => std::task::Poll::Ready(t),
            None => {
                slot.1 = Some(cx.waker().clone());
                std::task::Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every grammar as a language of its name, its queries read as
    /// `lisp/editor/syntax.scm` has them read.
    fn rust() -> Syntax {
        let mut s = Syntax::default();
        for g in GRAMMARS {
            s.define_language(g, g, None, !matches!(g, "rust" | "json" | "go")).unwrap();
        }
        s
    }

    fn doc(text: &str) -> Rc<RefCell<Document>> {
        Rc::new(RefCell::new(Document::new(text)))
    }

    fn edit(d: &Rc<RefCell<Document>>, at: std::ops::Range<usize>, text: &str) {
        let actor: techne_text::Actor = "test".into();
        let tx = d.borrow().edit(&actor, [(at, text)]).unwrap();
        d.borrow_mut().apply(tx).unwrap();
    }

    fn faces_of(s: &mut Syntax, faces: &mut Faces, d: &Rc<RefCell<Document>>, language: &str) -> Vec<(usize, usize, String)> {
        let len = d.borrow().len();
        s.highlights(d, language, 0, len, faces).into_iter().map(|r| (r.from, r.to, faces.name_text(r.face).to_string())).collect()
    }

    /// The face that shows at `text` in `source`: the last range over it.
    fn top(language: &str, source: &str, text: &str) -> Option<String> {
        let mut s = rust();
        let mut faces = Faces::empty();
        let d = doc(source);
        let at = source.find(text).unwrap_or_else(|| panic!("{text} in {source:?}"));
        faces_of(&mut s, &mut faces, &d, language).into_iter().rev().find(|(from, to, _)| *from <= at && at < *to).map(|(_, _, f)| f)
    }

    #[test]
    fn languages_colour_their_keywords_comments_and_strings() {
        let cases = [
            ("rust", "fn main() { let s = \"hi\"; } // done", "fn", "keyword"),
            ("rust", "fn main() { let s = \"hi\"; } // done", "hi", "string"),
            ("rust", "fn main() { let s = \"hi\"; } // done", "done", "comment"),
            ("python", "def f(x):\n    return \"s\"  # c", "def", "keyword"),
            ("python", "def f(x):\n    return \"s\"  # c", "# c", "comment"),
            ("scheme", "(define (f x) \"doc\" x) ; note", "note", "comment"),
            ("c", "int main(void) { return 0; }", "int", "type"),
            ("javascript", "const x = `t`;", "const", "keyword"),
            ("go", "package main\nfunc main() {}", "func", "keyword"),
            ("bash", "echo \"hi\" # there", "there", "comment"),
            ("json", "{\"key\": 42, \"ok\": true}", "42", "number"),
            ("toml", "[pkg]\nname = \"x\"\nn = 3", "\"x\"", "string"),
            ("yaml", "key: value\nn: 3", "key", "property"),
            ("markdown", "# Title\n\nSome *emph* and **strong** text.\n", "Title", "text.title"),
            ("markdown", "# Title\n\nSome *emph* and **strong** text.\n", "emph", "text.emphasis"),
        ];
        for (language, source, text, face) in cases {
            let got = top(language, source, text);
            assert!(got.as_deref().is_some_and(|f| f == face || f.starts_with(&format!("{face}."))), "{text:?} in {language}: {got:?}");
        }
    }

    #[test]
    fn the_pattern_the_queries_mean_wins_a_node() {
        // A JSON key is a string the queries capture first as a key.
        assert_eq!(top("json", "{\"key\": 1}", "key").as_deref(), Some("string.special.key"));
        // Rust's first patterns win: an upper-case name called is a
        // constructor, not a function.
        assert_eq!(top("rust", "fn f() { Some(1); }", "Some").as_deref(), Some("constructor"));
        // Python's last ones do: a call is a function, not the
        // variable every identifier is first.
        assert_eq!(top("python", "print(x)", "print").as_deref(), Some("function.builtin"));
        assert_eq!(top("python", "f(x)", "f").as_deref(), Some("function"));
        // Go's first ones do again: a call is a function, not the
        // variable its last pattern makes every identifier.
        assert_eq!(top("go", "package p\nfunc f() { call(x) }", "call").as_deref(), Some("function"));
        assert_eq!(top("javascript", "foo(x)", "foo").as_deref(), Some("function"));
        assert_eq!(top("c", "int main(void) { foo(1); }", "foo").as_deref(), Some("function"));
        assert_eq!(top("bash", "ls -l", "-l").as_deref(), Some("constant"));
        assert_eq!(top("scheme", "(define (f x) x)", "define").as_deref(), Some("keyword"));
        assert_eq!(top("yaml", "key: value", "key").as_deref(), Some("property"));
        // A smaller node paints over the one holding it.
        assert_eq!(top("python", "s = \"a\\nb\"", "\\n").as_deref(), Some("escape"));
        // TypeScript's own patterns over the JavaScript base under them.
        assert_eq!(top("typescript", "f = (count: number) => count", "count").as_deref(), Some("variable.parameter"));
        assert_eq!(top("typescript", "// note\nconst f = 1;", "const").as_deref(), Some("keyword"));
        assert_eq!(top("tsx", "const el = <div className=\"x\"/>;", "className").as_deref(), Some("attribute"));
    }

    #[test]
    fn one_capture_per_node() {
        // No range is given twice: the losers of a node are dropped, not
        // composed under its winner.
        let mut s = rust();
        let mut faces = Faces::empty();
        let d = doc("fn f() { let x = Some(1); x.unwrap(); }");
        let ranges = faces_of(&mut s, &mut faces, &d, "rust");
        let mut seen = std::collections::HashSet::new();
        for (from, to, _) in &ranges {
            assert!(seen.insert((*from, *to)), "{ranges:?}");
        }
    }

    #[test]
    fn edits_follow_the_document_without_a_full_parse() {
        let mut s = rust();
        let mut faces = Faces::empty();
        let d = doc("fn a() {}\n");
        faces_of(&mut s, &mut faces, &d, "rust");
        edit(&d, 0..0, "// c\n");
        edit(&d, 14..14, " let x = \"s\";");
        // Parsed again by the worker; the edited tree colours meanwhile.
        let text = d.borrow().text().to_string();
        let mut ranges = Vec::new();
        for _ in 0..500 {
            ranges = faces_of(&mut s, &mut faces, &d, "rust");
            if ranges.iter().any(|(f, t, face)| face == "string" && &text[*f..*t] == "\"s\"") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ranges.iter().any(|(f, t, face)| face == "string" && &text[*f..*t] == "\"s\""), "{ranges:?}");
        assert!(ranges.iter().any(|(f, t, face)| face == "comment" && &text[*f..*t] == "// c"), "{ranges:?}");
        // And it agrees with a fresh parse of the same text.
        let fresh = doc(&text);
        assert_eq!(ranges, faces_of(&mut rust(), &mut Faces::empty(), &fresh, "rust"));
    }

    #[test]
    fn an_edited_tree_colours_until_its_parse_lands() {
        // A parse that cannot run here (no budget left for it) leaves the
        // edited tree: its colours have moved with the text.
        let mut s = rust();
        let mut faces = Faces::empty();
        let source = format!("fn a() {{ \"s\" }}\n{}", "fn f() { let s = \"x\"; }\n".repeat(2_000));
        let d = doc(&source);
        s.set_budget(Duration::from_secs(30));
        let len = d.borrow().len();
        s.highlights(&d, "rust", 0, len.min(4096), &mut faces);
        s.parse_budget = Duration::ZERO;
        edit(&d, 0..0, "\n\n");
        let ranges: Vec<(usize, usize, String)> = s
            .highlights(&d, "rust", 0, 4096, &mut faces)
            .into_iter()
            .map(|r| (r.from, r.to, faces.name_text(r.face).to_string()))
            .collect();
        assert!(!s.docs.values().next().unwrap().parsed, "edited, not parsed");
        let text = d.borrow().text().to_string();
        assert!(ranges.iter().any(|(f, t, face)| face == "string" && (*f, &text[*f..*t]) == (11, "\"s\"")), "{ranges:?}");
    }

    /// A tree landing wakes the runtime, also when the worker started
    /// before the runtime said how (`serve` comes after the first
    /// snapshot).
    #[test]
    fn the_worker_wakes_the_runtime_when_a_tree_lands() {
        let mut s = rust();
        let mut faces = Faces::empty();
        let d = doc(&"fn f() {}\n".repeat(10_000));
        assert!(s.highlights(&d, "rust", 0, 4096, &mut faces).is_empty(), "the worker has it");
        let (wake, woken) = mpsc::channel();
        s.set_wake(wake);
        assert!(matches!(woken.recv_timeout(Duration::from_secs(10)), Ok(Msg::Wake)), "woken without being asked again");
        // The snapshot the wake brings installs the tree and colours (on a
        // loaded machine, maybe one more after it).
        let coloured = (0..200).any(|_| {
            let done = !s.highlights(&d, "rust", 0, 4096, &mut faces).is_empty();
            if !done {
                std::thread::sleep(Duration::from_millis(10));
            }
            done
        });
        assert!(coloured);
    }

    /// A language defined again while the worker parses for its old
    /// definition does not get the old grammar's tree.
    #[test]
    fn a_language_redefined_drops_the_old_definitions_parse() {
        let mut s = rust();
        let mut faces = Faces::empty();
        let d = doc(&"fn f() { let s = 1; }\n".repeat(10_000));
        assert!(s.highlights(&d, "rust", 0, 4096, &mut faces).is_empty());
        s.define_language("rust", "python", None, true).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        s.drain();
        let id = d.borrow().id();
        assert!(s.docs.get(&id).is_none_or(|e| e.tree.language().name() == Some("python")), "no tree of the old grammar");
    }

    #[test]
    fn a_big_document_is_parsed_by_the_worker() {
        let mut s = rust();
        let mut faces = Faces::empty();
        let source = "fn f() { let s = \"x\"; }\n".repeat(20_000);
        assert!(source.len() > FIRST_SYNC_MAX);
        let d = doc(&source);
        let ranges = s.highlights(&d, "rust", 0, 4096, &mut faces);
        assert!(ranges.is_empty(), "nothing yet: the worker has it");
        let mut coloured = Vec::new();
        for _ in 0..500 {
            std::thread::sleep(Duration::from_millis(10));
            let ranges = s.highlights(&d, "rust", 0, 4096, &mut faces);
            if !ranges.is_empty() {
                coloured = ranges;
                break;
            }
        }
        let keyword = faces.name("keyword");
        assert!(coloured.iter().any(|r| r.face == keyword), "the worker's tree landed");
        // An edit after it is parsed incrementally: here within the
        // budget, or by the worker on a machine too busy for it.
        edit(&d, 0..0, "// top\n");
        let commented = (0..500).any(|_| {
            let ranges = s.highlights(&d, "rust", 0, 4096, &mut faces);
            let done = ranges.iter().any(|r| faces.name_text(r.face) == "comment" && r.from == 0);
            if !done {
                std::thread::sleep(Duration::from_millis(10));
            }
            done
        });
        assert!(commented, "the edit's tree landed");
    }

    #[test]
    fn markdown_edits_keep_their_emphasis() {
        let mut s = rust();
        let mut faces = Faces::empty();
        let d = doc("/*");
        faces_of(&mut s, &mut faces, &d, "markdown");
        edit(&d, 2..2, "(");
        faces_of(&mut s, &mut faces, &d, "markdown");
        edit(&d, 3..3, "*");
        let ranges = faces_of(&mut s, &mut faces, &d, "markdown");
        assert!(ranges.iter().any(|r| (r.0, r.1, r.2.as_str()) == (1, 4, "text.emphasis")), "{ranges:?}");
    }

    #[test]
    fn inline_ranges_match_a_brute_force_walk() {
        let source = "A *emph* and **strong** paragraph, with a [link](https://x.y).\n\n".repeat(4_000);
        let rope = Rope::from_str(&source);
        let block: Language = tree_sitter_md::LANGUAGE.into();
        let tree = parse(&block, &rope, None, &[], &|| false).expect("the block parse");
        let deadline = Instant::now() + Duration::from_secs(30);
        let brute = |from: usize, to: usize| {
            let mut out = Vec::new();
            let mut walk = vec![tree.root_node()];
            while let Some(node) = walk.pop() {
                if node.end_byte() <= from || node.start_byte() >= to {
                    continue;
                }
                if node.kind() == "inline" || node.kind() == "pipe_table_cell" {
                    out.push((node.start_byte(), node.end_byte()));
                    continue;
                }
                walk.extend(node.children(&mut node.walk()));
            }
            out.sort_unstable();
            out
        };
        let mid = source.len() / 2 + 13;
        for (from, to) in [(0, source.len()), (0, BUCKET), (mid, mid + BUCKET), (1, 2), (source.len() - 50, source.len())] {
            let got: Vec<(usize, usize)> =
                inline_ranges(tree.root_node(), from, to, deadline).unwrap().iter().map(|r| (r.start_byte, r.end_byte)).collect();
            assert_eq!(got, brute(from, to), "window {from}..{to}");
        }
    }

    #[test]
    fn a_query_that_does_not_compile_is_refused() {
        let mut s = rust();
        assert!(s.define_language("rust", "rust", Some("(no_such_node) @x"), false).is_err());
        assert!(s.languages.contains_key("rust"), "the language stays as it was");
        assert!(s.define_language("x", "cobol", None, false).is_err());
    }

    #[test]
    fn gone_documents_are_forgotten() {
        let mut s = rust();
        let mut faces = Faces::empty();
        let d = doc("fn a() {}");
        faces_of(&mut s, &mut faces, &d, "rust");
        assert_eq!(s.docs.len(), 1);
        drop(d);
        s.retain();
        assert!(s.docs.is_empty());
    }
}
