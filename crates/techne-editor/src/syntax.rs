//! Syntax highlighting with tree-sitter: the bottom layer of a pane's
//! highlights, under its modes' layers and a search's (EDITOR.md,
//! section 3, in the small until layers declare classes).
//!
//! Highlights are upstream queries' captures, each capture name
//! naming a face: the names are the tree-sitter convention's dotted
//! ones, which the face table resolves by prefix, so
//! `keyword.control` needs no face of its own anywhere. Overlapping
//! captures are ordered as the queries expect (smaller ranges over
//! larger, more specific names over general ones, earlier patterns
//! over later ones on one range), then composed under everything else.
//!
//! Parsing is the expensive part and its cost has no useful bound — a
//! grammar over a pathological document outruns any budget a snapshot
//! can offer, and a halted parse in this API yields nothing to resume
//! from. So a document past `SYNC_MAX` bytes (or one whose synchronous
//! parse ran out of its budget) goes to a worker thread of its own:
//! it parses with the previous tree, publishes revision-stamped
//! trees, and wakes the runtime (`Msg::Wake`, as a Lisp task would) so
//! the colours land in the next snapshot. Until then the text draws
//! plain — never in a stale tree's positions, which the runtime would
//! have to refuse as it refuses other stale work. Smaller documents
//! parse here, bounded like the snapshot's Lisp parts are, and are
//! coloured in the snapshot that asks.
//!
//! Collected captures are cached per document and tree revision over
//! an aligned window: scrolling within it costs no query work again.
//!
//! Nothing here knows about buffers or modes: the runtime says a
//! document's language (Lisp says which, `pane-display`), or none.
//! A document past `MAX` bytes is not parsed at all.

use std::{
    collections::HashMap,
    ops::ControlFlow,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use techne_text::ropey::Rope;
use tree_sitter::{InputEdit, Language, ParseOptions, Parser, Point, Query, QueryCursor, Range, StreamingIterator, Tree};

use crate::{faces::FaceRange, runtime::Msg};

/// The most text parsed for one document.
const MAX: usize = 4 << 20;

/// The most text parsed in the snapshot that asks; past it, the worker.
/// A mid-size file's first parse is already most of a keystroke's
/// budget many times over, and an aborted parse would be paid again
/// on every key: the band stays small on purpose.
const SYNC_MAX: usize = 64 << 10;

/// Collected captures are cached over windows aligned to this many bytes.
const BUCKET: usize = 16 << 10;

/// How long one snapshot may spend parsing, in step with the parts'
/// deadlines being ten times as long in a debug build
/// (`runtime::PART_DEADLINE`).
const BUDGET: Duration = Duration::from_millis(if cfg!(debug_assertions) { 150 } else { 15 });

/// A language: its grammar and highlight queries. Markdown is two
/// grammars (block and inline), parsed by hand below.
enum Grammar {
    One { language: Language, highlights: &'static [&'static str] },
    Markdown,
}

fn grammar(language: &str) -> Option<Grammar> {
    let one = |l: tree_sitter_language::LanguageFn, highlights: &'static [&'static str]| Grammar::One { language: l.into(), highlights };
    Some(match language {
        "rust" => one(tree_sitter_rust::LANGUAGE, &[tree_sitter_rust::HIGHLIGHTS_QUERY]),
        "python" => one(tree_sitter_python::LANGUAGE, &[tree_sitter_python::HIGHLIGHTS_QUERY]),
        "c" => one(tree_sitter_c::LANGUAGE, &[tree_sitter_c::HIGHLIGHT_QUERY]),
        "javascript" => {
            one(tree_sitter_javascript::LANGUAGE, &[tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY])
        }
        // The TypeScript queries hold only what TypeScript adds; the
        // JavaScript base rides under them (and JSX, for TSX): on one
        // range the earlier pattern wins, so the specialisations go
        // first and the base last.
        "typescript" => one(
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            &[tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY],
        ),
        "tsx" => one(
            tree_sitter_typescript::LANGUAGE_TSX,
            &[
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_javascript::HIGHLIGHT_QUERY,
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

/// The grammar to parse `language` with (markdown's is its block one).
fn parser_language(language: &str) -> Option<Language> {
    Some(match grammar(language)? {
        Grammar::One { language, .. } => language,
        Grammar::Markdown => tree_sitter_md::LANGUAGE.into(),
    })
}

/// What the worker is asked, and answers.
enum Work {
    Parse { doc: u64, revision: u64, language: String, grammar: Language, rope: Rope, superseded: Arc<AtomicBool> },
    Retain(Vec<u64>),
}

struct Done {
    doc: u64,
    revision: u64,
    language: String,
    source: String,
    tree: Tree,
}

struct Cached {
    language: String,
    revision: u64,
    /// The text `tree` was parsed from, which captures are read over.
    source: String,
    tree: Tree,
    /// Captures collected for this tree over [from, to), a `BUCKET`
    /// window aligned down and up.
    collected: Option<(usize, usize, Vec<FaceRange>)>,
}

/// A language's state: its queries compiled once, or why not.
enum Queries {
    One(Result<Query, String>),
    Markdown(Result<(Query, Query), String>),
}

/// Parses documents and answers highlights, from trees the worker or
/// the fast path parsed. A parse or query that fails is said once,
/// through the error `highlights` hands back with its ranges; running
/// out of budget is not said at all — plain text is answer enough.
#[derive(Default)]
pub struct Syntax {
    docs: HashMap<u64, Cached>,
    queries: HashMap<String, Queries>,
    /// Synchronous parses and collections that ran out of budget, as
    /// (document, language, revision): not retried until the text
    /// changes. The revision falls back to the worker instead.
    aborted: std::collections::HashSet<(u64, String, u64)>,
    /// Markdown inline parses that ran out of budget, as (document,
    /// revision): the block highlights still show for that revision.
    inline_aborted: std::collections::HashSet<(u64, u64)>,
    /// What has been said already: a failure is handed back once, not
    /// on every snapshot it goes on failing in.
    said: std::collections::HashSet<String>,
    /// The worker thread's channels, once a parse first goes to it.
    work: Option<(mpsc::Sender<Work>, mpsc::Receiver<Done>)>,
    /// The request each document is waiting on, by revision, with the
    /// flag that stops it when a newer one takes its place.
    pending: HashMap<u64, (u64, String, Arc<AtomicBool>)>,
    /// How this syntax wakes the runtime when a tree lands, set by
    /// `serve` (there is no loop to wake before it).
    wake: Option<mpsc::Sender<Msg>>,
}

impl Syntax {
    /// Wake the runtime (a snapshot with no input) when the worker
    /// lands a tree, through `wake` (the runtime's own queue).
    pub fn set_wake(&mut self, wake: mpsc::Sender<Msg>) {
        self.wake = Some(wake);
    }

    /// The face ranges of `language`'s syntax in `text` (document `doc`
    /// at `revision`) between `from` and `to`; with them, what went
    /// wrong this call for the first time, if anything. A tree still
    /// being parsed (or none) answers no ranges.
    pub fn highlights(
        &mut self,
        doc: u64,
        revision: u64,
        text: &Rope,
        language: &str,
        from: usize,
        to: usize,
    ) -> (Vec<FaceRange>, Option<String>) {
        let Some(grammar) = grammar(language) else { return (Vec::new(), None) };
        if text.len_bytes() > MAX {
            return (Vec::new(), None);
        }
        // One deadline for everything this call may spend: parsing,
        // markdown's inline parses, and collecting captures.
        let deadline = Instant::now() + BUDGET;
        if !self.queries.contains_key(language) {
            let compiled = compile_queries(&grammar);
            self.queries.insert(language.to_string(), compiled);
        }
        self.drain();
        // Unchanged since parsed: the tree and its source still hold,
        // and the rope is not flattened again for them.
        if self.docs.get(&doc).is_none_or(|c| c.language != language || c.revision != revision) {
            let parsed = if text.len_bytes() <= SYNC_MAX && !self.aborted.contains(&(doc, language.to_string(), revision)) {
                self.parse_sync(doc, revision, text, language, &grammar, deadline)
            } else {
                None
            };
            if parsed.is_none() {
                self.request(doc, revision, text, language);
                return (Vec::new(), None);
            }
        }
        let Some(cached) = self.docs.get_mut(&doc).filter(|c| c.language == language && c.revision == revision) else {
            return (Vec::new(), None);
        };
        if let Some((cf, ct, ranges)) = &cached.collected
            && *cf <= from
            && to <= *ct
        {
            return (ranges.iter().filter(|r| r.from < to && r.to > from).cloned().collect(), None);
        }
        // Collect over the aligned window the cache keeps, not only
        // what was asked: the next scroll asks from inside it.
        let (af, at) = (from / BUCKET * BUCKET, (to.div_ceil(BUCKET) * BUCKET).min(cached.source.len()));
        let queries = self.queries.get(language).expect("compiled above or already");
        let bytes = cached.source.as_bytes();
        let mut found: Vec<(usize, usize, FaceRange)> = Vec::new();
        let mut collected: bool;
        match (&queries, &grammar) {
            (Queries::One(Ok(q)), _) => collected = collect(q, cached.tree.root_node(), bytes, af, at, deadline, &mut found),
            (Queries::Markdown(Ok((block, inline))), _) => {
                collected = collect(block, cached.tree.root_node(), bytes, af, at, deadline, &mut found);
                // The inline grammar over this window's inline nodes,
                // each parsed alone: one parse over every range would
                // read unrelated paragraphs as one stream, an emphasis
                // opening in one and closing in the next. The work is
                // the window's, not the document's, and shares the
                // call's deadline.
                if collected && !self.inline_aborted.contains(&(doc, revision)) {
                    let language_of: Language = tree_sitter_md::INLINE_LANGUAGE.into();
                    for range in inline_ranges(cached.tree.root_node(), af, at) {
                        let Some(tree) = parse_bounded_ranges(&language_of, bytes, &[range], deadline) else {
                            self.inline_aborted.insert((doc, revision));
                            break;
                        };
                        if !collect(inline, tree.root_node(), bytes, af, at, deadline, &mut found) {
                            collected = false;
                            break;
                        }
                    }
                }
            }
            (Queries::One(Err(e)), _) | (Queries::Markdown(Err(e)), _) => {
                let said = {
                    let m = format!("syntax: the {language} queries do not compile: {e}");
                    self.said.insert(m.clone()).then_some(m)
                };
                return (Vec::new(), said);
            }
        }
        if !collected {
            self.aborted.insert((doc, language.to_string(), revision));
            return (Vec::new(), None);
        }
        // The queries' own precedence, as their highlighters read them:
        // a smaller range paints over a larger one that holds it (an
        // escape in a string); on one range the more specific capture
        // name paints over the less (a parameter over a variable), and
        // among equals the earlier pattern wins (a key over a string).
        // Composed later-wins, that is longest first, least specific
        // first, highest pattern first.
        found.sort_by(|(pa, sa, ra), (pb, sb, rb)| (rb.to - rb.from).cmp(&(ra.to - ra.from)).then(sa.cmp(sb)).then(pb.cmp(pa)));
        let collected: Vec<FaceRange> = found.into_iter().map(|(_, _, r)| r).collect();
        let answer = collected.iter().filter(|r| r.from < to && r.to > from).cloned().collect();
        if let Some(cached) = self.docs.get_mut(&doc) {
            cached.collected = Some((af, at, collected));
        }
        (answer, None)
    }

    /// Parse here, bounded: the installed tree, or none — the revision
    /// recorded as not to retry, and the worker asked instead by the
    /// caller. Only for documents small enough that a first parse is
    /// expected inside the budget.
    fn parse_sync(&mut self, doc: u64, revision: u64, text: &Rope, language: &str, grammar: &Grammar, deadline: Instant) -> Option<()> {
        let source = text.to_string();
        let mut old = self.docs.remove(&doc).filter(|c| c.language == language);
        let edit = old.as_ref().and_then(|c| edit_between(&c.source, &source, text));
        if let (Some(edit), Some(old)) = (edit, &mut old) {
            old.tree.edit(&edit);
        }
        let language_of = match grammar {
            Grammar::One { language: l, .. } => l.clone(),
            Grammar::Markdown => tree_sitter_md::LANGUAGE.into(),
        };
        let before = edit.and(old.as_ref()).map(|c| &c.tree);
        let Some(tree) = parse_bounded(&language_of, source.as_bytes(), before, deadline) else {
            self.aborted.insert((doc, language.to_string(), revision));
            if let Some(old) = old {
                self.docs.insert(doc, old);
            }
            return None;
        };
        self.docs.insert(doc, Cached { language: language.to_string(), revision, source, tree, collected: None });
        Some(())
    }

    /// Ask the worker for this revision's tree, unless it is on its way
    /// already; an older request for the document is superseded.
    fn request(&mut self, doc: u64, revision: u64, text: &Rope, language: &str) {
        if self.pending.get(&doc).is_some_and(|(at, in_language, _)| *at >= revision && in_language == language) {
            return;
        }
        let Some(grammar) = parser_language(language) else { return };
        if self.work.is_none() {
            self.work = Some(worker(self.wake.clone()));
        }
        let superseded = Arc::new(AtomicBool::new(false));
        if let Some((_, _, old)) = self.pending.insert(doc, (revision, language.to_string(), superseded.clone())) {
            old.store(true, Ordering::Relaxed);
        }
        let work = Work::Parse { doc, revision, language: language.to_string(), grammar, rope: text.clone(), superseded };
        let _ = self.work.as_ref().expect("just started").0.send(work);
    }

    /// Install whatever trees the worker has landed since last asked.
    fn drain(&mut self) {
        let Some((_, done)) = &self.work else { return };
        while let Ok(done) = done.try_recv() {
            if self.pending.get(&done.doc).is_some_and(|(at, _, _)| *at <= done.revision) {
                self.pending.remove(&done.doc);
            }
            if self.docs.get(&done.doc).is_none_or(|c| c.revision < done.revision || c.language != done.language) {
                self.docs.insert(
                    done.doc,
                    Cached { language: done.language, revision: done.revision, source: done.source, tree: done.tree, collected: None },
                );
            }
        }
    }

    /// Forget every document but these (the runtime's shown panes, each
    /// snapshot): a closed document's tree goes with it.
    pub fn retain(&mut self, docs: &[u64]) {
        self.docs.retain(|id, _| docs.contains(id));
        self.aborted.retain(|(id, _, _)| docs.contains(id));
        self.inline_aborted.retain(|(id, _)| docs.contains(id));
        self.pending.retain(|id, _| docs.contains(id));
        if let Some((work, _)) = &self.work {
            let _ = work.send(Work::Retain(docs.to_vec()));
        }
    }
}

fn compile_queries(grammar: &Grammar) -> Queries {
    match grammar {
        Grammar::One { language: l, highlights } => Queries::One(Query::new(l, &highlights.join("\n")).map_err(|e| e.to_string())),
        Grammar::Markdown => Queries::Markdown(
            Query::new(&tree_sitter_md::LANGUAGE.into(), tree_sitter_md::HIGHLIGHT_QUERY_BLOCK)
                .and_then(|block| Ok((block, Query::new(&tree_sitter_md::INLINE_LANGUAGE.into(), tree_sitter_md::HIGHLIGHT_QUERY_INLINE)?)))
                .map_err(|e: tree_sitter::QueryError| e.to_string()),
        ),
    }
}

/// The worker: parses documents off the snapshot's thread, newest
/// revision per document winning, and wakes the runtime as trees land.
fn worker(wake: Option<mpsc::Sender<Msg>>) -> (mpsc::Sender<Work>, mpsc::Receiver<Done>) {
    let (ask, asked) = mpsc::channel::<Work>();
    let (answer, answers) = mpsc::channel::<Done>();
    std::thread::Builder::new()
        .name("syntax".into())
        .spawn(move || {
            let mut last: HashMap<u64, Cached> = HashMap::new();
            while let Ok(work) = asked.recv() {
                match work {
                    Work::Retain(docs) => last.retain(|id, _| docs.contains(id)),
                    Work::Parse { doc, revision, language, grammar, rope, superseded } => {
                        if superseded.load(Ordering::Relaxed) {
                            continue;
                        }
                        let source = rope.to_string();
                        let mut old = last.remove(&doc).filter(|c| c.language == language);
                        let edit = old.as_ref().and_then(|c| edit_between(&c.source, &source, &rope));
                        if let (Some(edit), Some(old)) = (edit, &mut old) {
                            old.tree.edit(&edit);
                        }
                        let before = edit.and(old.as_ref()).map(|c| &c.tree);
                        let mut parser = Parser::new();
                        let parsed = parser.set_language(&grammar).ok().and_then(|_| {
                            let mut progress = |_: &tree_sitter::ParseState| {
                                if superseded.load(Ordering::Relaxed) { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
                            };
                            let options = ParseOptions::new().progress_callback(&mut progress);
                            parser.parse_with_options(&mut |i, _| &source.as_bytes()[i.min(source.len())..], before, Some(options))
                        });
                        let Some(tree) = parsed else {
                            if let Some(old) = old {
                                last.insert(doc, old);
                            }
                            continue;
                        };
                        if superseded.load(Ordering::Relaxed) {
                            continue;
                        }
                        last.insert(
                            doc,
                            Cached { language: language.clone(), revision, source: source.clone(), tree: tree.clone(), collected: None },
                        );
                        if answer.send(Done { doc, revision, language, source, tree }).is_err() {
                            return;
                        }
                        if let Some(wake) = &wake {
                            let _ = wake.send(Msg::Wake);
                        }
                    }
                }
            }
        })
        .expect("the syntax worker starts");
    (ask, answers)
}

/// Parse until `deadline`, then stop: the parse is given up on, none
/// of it kept (a halted parse in this API yields no tree to resume).
fn parse_bounded(language: &Language, source: &[u8], old: Option<&Tree>, deadline: Instant) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    let mut progress =
        |_: &tree_sitter::ParseState| if Instant::now() < deadline { ControlFlow::Continue(()) } else { ControlFlow::Break(()) };
    let options = ParseOptions::new().progress_callback(&mut progress);
    parser.parse_with_options(&mut |i, _| &source[i.min(source.len())..], old, Some(options))
}

/// As `parse_bounded`, over the included ranges only.
fn parse_bounded_ranges(language: &Language, source: &[u8], ranges: &[Range], deadline: Instant) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    parser.set_included_ranges(ranges).ok()?;
    let mut progress =
        |_: &tree_sitter::ParseState| if Instant::now() < deadline { ControlFlow::Continue(()) } else { ControlFlow::Break(()) };
    let options = ParseOptions::new().progress_callback(&mut progress);
    parser.parse_with_options(&mut |i, _| &source[i.min(source.len())..], None, Some(options))
}

/// The block tree's inline containers over a window, as ranges:
/// inline nodes, and table cells, whose text the inline grammar
/// reads (as the upstream wrapper treats them).
fn inline_ranges(root: tree_sitter::Node, from: usize, to: usize) -> Vec<Range> {
    let mut out = Vec::new();
    let mut walk = vec![root];
    while let Some(node) = walk.pop() {
        if node.end_byte() < from || node.start_byte() > to {
            continue;
        }
        if node.kind() == "inline" || node.kind() == "pipe_table_cell" {
            out.push(node.range());
            continue;
        }
        walk.extend(node.children(&mut node.walk()));
    }
    out.sort_by_key(|r| r.start_byte);
    out
}

/// A query's captures over a window, each (pattern, specificity,
/// range): the two order them, the name names a face. Captures that
/// draw nothing (private to the query, spelling, the explicit `none`)
/// are left out; markdown's older `@text.*` names map to `markup.*`
/// faces. False when the deadline ran out partway: what was gathered
/// is not to be trusted as the window's picture.
fn collect(
    query: &Query,
    root: tree_sitter::Node,
    bytes: &[u8],
    from: usize,
    to: usize,
    deadline: Instant,
    out: &mut Vec<(usize, usize, FaceRange)>,
) -> bool {
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(from..to);
    let mut matches = cursor.matches(query, root, bytes);
    let mut seen = 0;
    while let Some(m) = matches.next() {
        seen += 1;
        if seen % 512 == 0 && Instant::now() >= deadline {
            return false;
        }
        for capture in m.captures() {
            let name = query.capture_names()[capture.index as usize];
            let Some(face) = face_of(name) else { continue };
            let range = capture.node.byte_range();
            if range.start < range.end {
                out.push((m.pattern_index, name.matches('.').count(), FaceRange { from: range.start, to: range.end, face }));
            }
        }
    }
    true
}

/// The face a capture name asks for, if it draws anything.
fn face_of(name: &str) -> Option<String> {
    if name.starts_with('_') || name == "none" || name == "spell" || name == "nospell" || name == "embedded" {
        return None;
    }
    Some(match name {
        // A key is a property that happens to be written as a string.
        "string.special.key" => "property".into(),
        "text.title" => "markup.heading".into(),
        "text.strong" => "markup.strong".into(),
        "text.emphasis" => "markup.emphasis".into(),
        "text.literal" => "markup.raw".into(),
        "text.uri" | "text.reference" | "text.link" => "markup.link".into(),
        "text" => "markup".into(),
        name => name.strip_prefix("text.").map_or_else(|| name.to_string(), |rest| format!("markup.{rest}")),
    })
}

/// The one edit between two texts: where their common prefix ends and
/// where their common suffix begins. None when they are the same. The
/// points are read off the new text's rope (its lines are indexed),
/// not counted over the whole text again.
fn edit_between(old: &str, new: &str, rope: &Rope) -> Option<InputEdit> {
    if old == new {
        return None;
    }
    let (old, new) = (old.as_bytes(), new.as_bytes());
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old.iter().rev().zip(new.iter().rev()).take_while(|(a, b)| a == b).count().min(old.len() - prefix).min(new.len() - prefix);
    let point = |at: usize| {
        let row = rope.byte_to_line(at);
        Point { row, column: at - rope.line_to_byte(row) }
    };
    // The old end's point is the start's, advanced over what was there.
    let start = point(prefix);
    let gone = &old[prefix..old.len() - suffix];
    let rows = gone.iter().filter(|&&b| b == b'\n').count();
    let old_end = Point {
        row: start.row + rows,
        column: if rows == 0 { start.column + gone.len() } else { gone.iter().rev().take_while(|&&b| b != b'\n').count() },
    };
    Some(InputEdit {
        start_byte: prefix,
        old_end_byte: old.len() - suffix,
        new_end_byte: new.len() - suffix,
        start_position: start,
        old_end_position: old_end,
        new_end_position: point(new.len() - suffix),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn faces(language: &str, source: &str) -> Vec<(usize, usize, String)> {
        let mut syntax = Syntax::default();
        let rope = Rope::from_str(source);
        let (ranges, wrong) = syntax.highlights(1, 1, &rope, language, 0, source.len());
        assert_eq!(wrong, None);
        ranges.into_iter().map(|r| (r.from, r.to, r.face)).collect()
    }

    fn has(language: &str, source: &str, text: &str, face: &str) {
        let at = source.find(text).unwrap_or_else(|| panic!("{text} in {source:?}"));
        assert!(
            faces(language, source).iter().any(|(from, to, f)| *from <= at && at < *to && f == face),
            "{text:?} is {face} in {language}: {:?}",
            faces(language, source)
        );
    }

    #[test]
    fn languages_colour_their_keywords_comments_and_strings() {
        has("rust", "fn main() { let s = \"hi\"; } // done", "fn", "keyword");
        has("rust", "fn main() { let s = \"hi\"; } // done", "hi", "string");
        has("rust", "fn main() { let s = \"hi\"; } // done", "done", "comment");
        has("python", "def f(x):\n    return \"s\"  # c", "def", "keyword");
        has("python", "def f(x):\n    return \"s\"  # c", "# c", "comment");
        has("scheme", "(define (f x) \"doc\" x) ; note", "define", "keyword");
        has("scheme", "(define (f x) \"doc\" x) ; note", "note", "comment");
        has("c", "int main(void) { return 0; }", "int", "type");
        has("javascript", "const x = `t`;", "const", "keyword");
        has("go", "package main\nfunc main() {}", "func", "keyword");
        has("bash", "echo \"hi\" # there", "there", "comment");
    }

    #[test]
    fn typescript_inherits_the_javascript_queries() {
        // The TypeScript query alone colours almost nothing: comments,
        // strings and keywords come from the JavaScript base under it.
        let src = "// note\nconst f = (x: number) => \"hello\";";
        has("typescript", src, "note", "comment");
        has("typescript", src, "const", "keyword");
        has("typescript", src, "hello", "string");
        has("typescript", src, "number", "type.builtin");
        let tsx = "const el = <div className=\"x\">{name}</div>;";
        has("tsx", tsx, "const", "keyword");
        has("tsx", tsx, "div", "tag");
    }

    #[test]
    fn data_languages_colour_keys_and_values() {
        has("json", "{\"key\": 42, \"ok\": true}", "key", "property");
        has("json", "{\"key\": 42, \"ok\": true}", "42", "number");
        has("toml", "[pkg]\nname = \"x\"\nn = 3", "name", "property");
        has("toml", "[pkg]\nname = \"x\"\nn = 3", "\"x\"", "string");
        has("yaml", "key: value\nn: 3", "key", "property");
        has("markdown", "# Title\n\nSome *emph* and **strong** text.\n", "Title", "markup.heading");
    }

    #[test]
    fn specific_captures_paint_over_general_ones() {
        // On one range the earlier pattern wins; a smaller range wins
        // over the one holding it. What is last here is what shows.
        let top = |language: &str, source: &str, text: &str| {
            let at = source.find(text).expect("the text");
            faces(language, source).into_iter().rev().find(|(from, to, _)| *from <= at && at < *to).map(|(_, _, f)| f)
        };
        assert_eq!(top("json", "{\"key\": 1}", "key").as_deref(), Some("property"));
        assert_eq!(top("python", "s = \"a\\nb\"", "\\n").as_deref(), Some("escape"));
        // Inherited queries under their specialisations, and the more
        // specific capture over the general one it arrived with.
        assert_eq!(top("typescript", "f = (count: number) => count", "count").as_deref(), Some("variable.parameter"));
        assert_eq!(top("tsx", "const el = <div className=\"x\"/>;", "className").as_deref(), Some("attribute"));
    }

    #[test]
    fn an_aborted_parse_holds_only_its_language_back() {
        let mut syntax = Syntax::default();
        syntax.aborted.insert((5, "markdown".into(), 1));
        let rope = Rope::from_str("{\"key\": 1}");
        let (ranges, wrong) = syntax.highlights(5, 1, &rope, "json", 0, rope.len_bytes());
        assert_eq!(wrong, None);
        assert!(!ranges.is_empty(), "the same text in another language still colours");
        let (ranges, _) = syntax.highlights(5, 1, &rope, "markdown", 0, rope.len_bytes());
        assert!(ranges.is_empty(), "and the aborted one stays plain for that revision");
    }

    #[test]
    fn an_edit_reparses_to_the_new_text() {
        let mut syntax = Syntax::default();
        let one = Rope::from_str("fn a() {}\n");
        let _ = syntax.highlights(7, 1, &one, "rust", 0, one.len_bytes());
        let two = Rope::from_str("fn longer_name() { let x = \"s\"; }\n");
        let (ranges, wrong) = syntax.highlights(7, 2, &two, "rust", 0, two.len_bytes());
        assert_eq!(wrong, None);
        assert!(ranges.iter().any(|r| r.face == "string" && &two.byte_slice(r.from..r.to).to_string() == "\"s\""), "{ranges:?}");
    }

    #[test]
    fn markdown_edits_keep_their_emphasis() {
        // Successive appends across three revisions: what the last one
        // parses to is what a fresh parse finds.
        let mut syntax = Syntax::default();
        for (revision, source) in [(1, "/*"), (2, "/*("), (3, "/*(*")] {
            let rope = Rope::from_str(source);
            let (ranges, wrong) = syntax.highlights(9, revision, &rope, "markdown", 0, source.len());
            assert_eq!(wrong, None);
            if revision == 3 {
                assert!(ranges.iter().any(|r| (r.from, r.to, r.face.as_str()) == (1, 4, "markup.emphasis")), "{ranges:?}");
            }
        }
    }

    #[test]
    fn a_big_document_is_parsed_by_the_worker() {
        // Past SYNC_MAX the first answer is plain, the worker's tree
        // colours the next asks once it lands.
        let mut syntax = Syntax::default();
        let source = "fn f() { let s = \"x\"; }\n".repeat(20_000);
        assert!(source.len() > SYNC_MAX);
        let rope = Rope::from_str(&source);
        let (ranges, _) = syntax.highlights(11, 1, &rope, "rust", 0, 4096);
        assert!(ranges.is_empty(), "nothing yet: the worker has it");
        let mut coloured = Vec::new();
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(10));
            let (ranges, _) = syntax.highlights(11, 1, &rope, "rust", 0, 4096);
            if !ranges.is_empty() {
                coloured = ranges;
                break;
            }
        }
        assert!(coloured.iter().any(|r| r.face == "keyword"), "the worker's tree landed: {coloured:?}");
    }
}
