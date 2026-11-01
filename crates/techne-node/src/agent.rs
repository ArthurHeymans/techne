//! Bounded repository questions for `(techne agent)`. Snapshots and their
//! expandable ranges live in a project, not in the conversation. This is a
//! convenience boundary for trusted local work, not a filesystem sandbox.

use crate::{fs, protocol::Expect};
use std::{
    cell::RefCell,
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use techne_vm::{
    api::Foreign,
    data::Data,
    reader::{self, Syntax, SyntaxKind},
    vm::{Capability, Vm},
};

const MAX_FILE: usize = 1 << 20;
const CACHE_BYTES: usize = 16 << 20;
const MAX_FILES: usize = 10_000;
const SCAN_BYTES: usize = 64 << 20;
static NEXT_SNAPSHOT: AtomicU64 = AtomicU64::new(1);
type R<T> = Result<T, String>;

fn text(s: impl Into<String>) -> Data {
    Data::String(s.into())
}
fn number(n: usize) -> Data {
    Data::Int(n as i64)
}
fn fields(xs: Vec<(&str, Data)>) -> Data {
    Data::List(xs.into_iter().map(|(k, v)| Data::List(vec![Data::Symbol(k.into()), v])).collect())
}
fn cap(n: i64, max: usize, who: &str) -> R<usize> {
    if n < 1 || n as u64 > max as u64 { Err(format!("{who}: expected a count from 1 to {max}")) } else { Ok(n as usize) }
}
fn offset(n: i64) -> R<usize> {
    usize::try_from(n).map_err(|_| "agent: offset must be non-negative".into())
}
fn clipped(s: &str, n: usize) -> &str {
    &s[..s.char_indices().nth(n).map_or(s.len(), |(at, _)| at)]
}
fn revision(bytes: &[u8]) -> String {
    fs::hash(bytes).iter().map(|b| format!("{b:02x}")).collect()
}
fn page(items: Vec<Data>, total: usize, start: usize, limit: usize, complete: bool) -> Data {
    let next = start.saturating_add(items.len());
    fields(vec![
        ("items", Data::List(items)),
        ("total", number(total)),
        ("complete", Data::Bool(complete)),
        ("truncated", Data::Bool(next < total || !complete)),
        ("next", if next < total && limit > 0 { number(next) } else { Data::Bool(false) }),
    ])
}

fn canonical_root(root: &str) -> R<PathBuf> {
    let root = std::fs::canonicalize(root).map_err(|e| format!("agent-project: {root}: {e}"))?;
    if !root.is_dir() {
        return Err("agent-project: root is not a directory".into());
    }
    if root.to_str().is_none() {
        return Err("agent-project: root is not UTF-8".into());
    }
    Ok(root)
}

struct Snapshot {
    namespace: String,
    id: u64,
    path: String,
    revision: String,
    source: Arc<str>,
}
impl Snapshot {
    fn handle(&self, start: usize, end: usize) -> String {
        format!("{}:{}:{start}:{end}", self.namespace, self.id)
    }
    fn line(&self, at: usize) -> usize {
        self.source[..at].bytes().filter(|&b| b == b'\n').count() + 1
    }
}
struct State {
    namespace: String,
    root: PathBuf,
    files: Vec<String>,
    listed: bool,
    complete: bool,
    listing_skipped: usize,
    listing_problems: Vec<String>,
    snapshots: VecDeque<Arc<Snapshot>>,
    bytes: usize,
}
pub(crate) struct Project {
    state: RefCell<State>,
}
impl State {
    fn new(root: &str) -> R<Self> {
        let root = canonical_root(root)?;
        let mut namespace = [0u8; 16];
        getrandom::fill(&mut namespace).map_err(|e| format!("agent-project: random handle namespace: {e}"))?;
        let namespace = namespace.iter().map(|b| format!("{b:02x}")).collect();
        Ok(Self {
            namespace,
            root,
            files: vec![],
            listed: false,
            complete: true,
            listing_skipped: 0,
            listing_problems: vec![],
            snapshots: VecDeque::new(),
            bytes: 0,
        })
    }
    fn path(&self, name: &str) -> R<PathBuf> {
        let path = std::fs::canonicalize(self.root.join(name)).map_err(|e| format!("agent: {name}: {e}"))?;
        if !path.starts_with(&self.root) {
            return Err("agent: path is outside the project".into());
        }
        Ok(path)
    }
    fn list(&mut self) -> R<()> {
        if self.listed {
            return Ok(());
        }
        let mut files = vec![];
        let mut complete = true;
        let mut skipped = 0;
        let mut problems = vec![];
        for entry in ignore::WalkBuilder::new(&self.root).follow_links(false).require_git(false).build() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    complete = false;
                    skipped += 1;
                    if problems.len() < 3 {
                        problems.push(clipped(&format!("agent-files: {e}"), 240).to_owned());
                    }
                    continue;
                }
            };
            if let Some(e) = entry.error() {
                complete = false;
                skipped += 1;
                if problems.len() < 3 {
                    problems.push(clipped(&format!("agent-files: {e}"), 240).to_owned());
                }
            }
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let Some(name) = entry.path().strip_prefix(&self.root).unwrap().to_str() else {
                complete = false;
                skipped += 1;
                if problems.len() < 3 {
                    problems.push("agent-files: non-UTF-8 file name".into());
                }
                continue;
            };
            if files.len() == MAX_FILES {
                complete = false;
                break;
            }
            files.push(name.to_owned());
        }
        files.sort();
        self.files = files;
        self.complete = complete;
        self.listing_skipped = skipped;
        self.listing_problems = problems;
        self.listed = true;
        Ok(())
    }
    fn keep(&mut self, snapshot: Arc<Snapshot>) {
        if let Some(at) = self.snapshots.iter().position(|s| s.id == snapshot.id) {
            self.bytes -= self.snapshots.remove(at).unwrap().source.len();
        }
        self.bytes += snapshot.source.len();
        self.snapshots.push_back(snapshot);
        while self.bytes > CACHE_BYTES || self.snapshots.len() > 128 {
            self.bytes -= self.snapshots.pop_front().unwrap().source.len();
        }
    }

    fn remember(&mut self, path: String, source: String, revision: String) -> Arc<Snapshot> {
        if let Some(at) = self.snapshots.iter().position(|s| s.path == path && s.revision == revision) {
            let found = self.snapshots.remove(at).unwrap();
            self.snapshots.push_back(found.clone());
            return found;
        }
        let snapshot = Arc::new(Snapshot {
            namespace: self.namespace.clone(),
            id: NEXT_SNAPSHOT.fetch_add(1, Ordering::Relaxed),
            path,
            revision,
            source: source.into(),
        });
        self.keep(snapshot.clone());
        snapshot
    }
    fn read(&mut self, name: &str) -> R<Arc<Snapshot>> {
        self.read_text(name)?.0.ok_or_else(|| format!("agent: {name}: not UTF-8 text"))
    }
    fn read_text(&mut self, name: &str) -> R<(Option<Arc<Snapshot>>, usize)> {
        let path = self.path(name)?;
        let name = path.strip_prefix(&self.root).unwrap().to_str().ok_or("agent: non-UTF-8 file name")?.to_owned();
        if path.metadata().map_err(|e| e.to_string())?.len() > MAX_FILE as u64 {
            return Err("agent: file exceeds 1 MiB".into());
        }
        let (bytes, _) =
            fs::read(path.as_os_str().as_encoded_bytes()).map_err(|e| format!("agent: {name}: {e}"))?.ok_or("agent: file disappeared")?;
        if bytes.len() > MAX_FILE {
            return Err("agent: file exceeds 1 MiB".into());
        }
        let rev = revision(&bytes);
        let count = bytes.len();
        let Ok(source) = String::from_utf8(bytes) else {
            return Ok((None, count));
        };
        Ok((Some(self.remember(name, source, rev)), count))
    }
    fn expand(&self, handle: &str, start: usize, chars: usize) -> R<Data> {
        let (namespace, handle) = handle.split_once(":").ok_or("agent-expand: invalid handle")?;
        if namespace != self.namespace {
            return Err("agent-expand: expired or foreign handle; query again".into());
        }
        let full_handle = format!("{namespace}:{handle}");
        let parts = handle.split(':').map(str::parse::<u64>).collect::<Result<Vec<_>, _>>().map_err(|_| "agent-expand: invalid handle")?;
        if parts.len() != 3 {
            return Err("agent-expand: invalid handle".into());
        }
        let s = self.snapshots.iter().find(|s| s.id == parts[0]).ok_or("agent-expand: expired or foreign handle; query again")?;
        let body = s.source.get(parts[1] as usize..parts[2] as usize).ok_or("agent-expand: invalid range")?;
        let length = body.chars().count();
        let at = body
            .char_indices()
            .nth(start)
            .map(|(i, _)| i)
            .or_else(|| (start == length).then_some(body.len()))
            .ok_or_else(|| format!("agent-expand: offset {start} past end (body has {length} characters)"))?;
        let chunk = clipped(&body[at..], chars);
        let next = start + chunk.chars().count();
        Ok(fields(vec![
            ("path", text(&s.path)),
            ("revision", text(&s.revision)),
            ("handle", text(full_handle)),
            ("line", number(s.line(parts[1] as usize + at))),
            ("text", text(chunk)),
            ("offset", number(start)),
            ("characters", number(length)),
            ("truncated", Data::Bool(at + chunk.len() < body.len())),
            ("next", if at + chunk.len() < body.len() { number(next) } else { Data::Bool(false) }),
        ]))
    }
    fn read_lines(&mut self, name: &str, from: usize, lines: usize, chars: usize) -> R<Data> {
        let s = self.read(name)?;
        let mut starts = vec![0];
        starts.extend(s.source.match_indices('\n').map(|(i, _)| i + 1).filter(|&at| at < s.source.len()));
        if from > starts.len() {
            return Err("agent-read: line past end".into());
        }
        let start = starts[from - 1];
        let end = starts.get(from - 1 + lines).copied().unwrap_or(s.source.len());
        self.expand(&s.handle(start, end), 0, chars)
    }
    fn search(&mut self, query: &str, prefix: &str, limit: usize, start: usize) -> R<Data> {
        if query.is_empty() {
            return Err("agent-search: query must not be empty".into());
        }
        self.list()?;
        let mut items = vec![];
        let mut selected: Vec<Arc<Snapshot>> = vec![];
        let mut selected_bytes = 0;
        let mut page_full = false;
        let mut total = 0;
        let mut scanned = 0;
        let mut bytes = 0;
        let mut skipped = self.listing_skipped;
        let mut problems: Vec<Data> = self.listing_problems.iter().map(text).collect();
        let mut binary = 0;
        let mut complete = self.complete;
        for name in self.files.clone().into_iter().filter(|f| f.starts_with(prefix)) {
            let (snapshot, read_bytes) = match self.read_text(&name) {
                Ok(found) => found,
                Err(e) => {
                    skipped += 1;
                    if problems.len() < 3 {
                        problems.push(text(clipped(&e, 240)));
                    }
                    continue;
                }
            };
            if bytes + read_bytes > SCAN_BYTES {
                complete = false;
                break;
            }
            bytes += read_bytes;
            let Some(s) = snapshot else {
                binary += 1;
                continue;
            };
            scanned += 1;
            let mut at = 0;
            for (line, source) in s.source.split_inclusive('\n').enumerate() {
                if source.contains(query) {
                    if total >= start && items.len() < limit && !page_full && !selected.iter().any(|chosen| chosen.id == s.id) {
                        if selected_bytes + s.source.len() > CACHE_BYTES {
                            page_full = true;
                        } else {
                            selected_bytes += s.source.len();
                            selected.push(s.clone());
                        }
                    }
                    if total >= start && items.len() < limit && !page_full {
                        let hit = source.find(query).unwrap();
                        // Center long-line previews on the match, without splitting UTF-8.
                        let before = source[..hit].char_indices().rev().nth(60).map_or(0, |(i, _)| i);
                        let preview = clipped(&source[before..], 240);
                        items.push(fields(vec![
                            ("path", text(&name)),
                            ("revision", text(&s.revision)),
                            ("line", number(line + 1)),
                            ("text", text(preview)),
                            ("column", number(source[..hit].chars().count() + 1)),
                            ("preview-offset", number(source[..before].chars().count())),
                            ("preview-truncated", Data::Bool(before != 0 || before + preview.len() < source.len())),
                            ("handle", text(s.handle(at, at + source.len()))),
                        ]));
                    }
                    total += 1;
                }
                at += source.len();
            }
        }
        // Counting later files must not evict handles in this very reply.
        for snapshot in selected {
            self.keep(snapshot);
        }
        let mut result = match page(items, total, start, limit, complete && skipped == 0) {
            Data::List(x) => x,
            _ => unreachable!(),
        };
        if let Data::List(extra) = fields(vec![
            ("scanned", number(scanned)),
            ("binary", number(binary)),
            ("skipped", number(skipped)),
            ("problems", Data::List(problems)),
        ]) {
            result.extend(extra);
        }
        Ok(Data::List(result))
    }
    fn symbols(&mut self, path: &str, name: &str, limit: usize, start: usize) -> R<Data> {
        let s = self.read(path)?;
        let mut symbols = vec![];
        let provider;
        let mut complete = true;
        match Path::new(path).extension().and_then(|s| s.to_str()) {
            Some("scm" | "sld") => {
                provider = "scheme-reader";
                let forms = reader::read_syntax(&s.source).map_err(|e| format!("agent-symbols: {}", e.message))?;
                for form in &forms {
                    scheme_symbols(form, "module", &mut symbols);
                }
            }
            Some("rs") => {
                provider = "tree-sitter-rust";
                let mut parser = tree_sitter::Parser::new();
                parser.set_language(&tree_sitter_rust::LANGUAGE.into()).map_err(|e| e.to_string())?;
                let tree = parser.parse(&*s.source, None).ok_or("agent-symbols: parse cancelled")?;
                complete = !tree.root_node().has_error();
                rust_symbols(tree.root_node(), &s.source, &mut symbols);
            }
            _ => return Err("agent-symbols: supported source languages are Scheme and Rust; use agent-search for other files".into()),
        }
        let found: Vec<_> = symbols.into_iter().filter(|d| name.is_empty() || d.name == name).collect();
        let items = found
            .iter()
            .skip(start)
            .take(limit)
            .map(|d| {
                fields(vec![
                    ("name", text(&d.name)),
                    ("kind", text(&d.kind)),
                    ("scope", text(&d.scope)),
                    ("path", text(&s.path)),
                    ("revision", text(&s.revision)),
                    ("provider", text(provider)),
                    ("line", number(s.line(d.start))),
                    ("end-line", number(s.line(s.source[..d.end].char_indices().next_back().map_or(d.start, |(at, _)| at)))),
                    ("handle", text(s.handle(d.start, d.end))),
                ])
            })
            .collect();
        Ok(page(items, found.len(), start, limit, complete))
    }
    fn edit(&mut self, name: &str, expected: &str, edits: Vec<Vec<String>>) -> R<Data> {
        if edits.is_empty() || edits.len() > 100 {
            return Err("agent-edit!: expected 1 to 100 replacements".into());
        }
        let s = self.read(name)?;
        if s.revision != expected {
            return Err("agent-edit!: stale revision; read again".into());
        }
        let mut changes = vec![];
        for edit in edits {
            if edit.len() != 2 || edit[0].is_empty() {
                return Err("agent-edit!: each replacement is (non-empty-old new)".into());
            }
            let mut matches = s.source.match_indices(&edit[0]);
            let at = matches.next().ok_or("agent-edit!: old text not found")?.0;
            // Include overlapping occurrences in the ambiguity check.
            if s.source[at + edit[0].chars().next().unwrap().len_utf8()..].contains(&edit[0]) {
                return Err("agent-edit!: old text is ambiguous".into());
            }
            changes.push((at, at + edit[0].len(), edit[1].clone()));
        }
        changes.sort_by_key(|(a, _, _)| *a);
        if changes.windows(2).any(|xs| xs[0].1 > xs[1].0) {
            return Err("agent-edit!: replacements overlap".into());
        }
        let mut source = s.source.to_string();
        let summaries = changes
            .iter()
            .map(|(a, b, new)| {
                fields(vec![
                    ("line", number(s.line(*a))),
                    ("old", text(clipped(&s.source[*a..*b], 240))),
                    ("new", text(clipped(new, 240))),
                    ("truncated", Data::Bool(s.source[*a..*b].chars().count() > 240 || new.chars().count() > 240)),
                ])
            })
            .collect();
        let size = changes.iter().fold(source.len() as i128, |n, (a, b, new)| n - (b - a) as i128 + new.len() as i128);
        if size > MAX_FILE as i128 {
            return Err("agent-edit!: result exceeds 1 MiB".into());
        }
        for (a, b, new) in changes.iter().rev() {
            source.replace_range(*a..*b, new);
        }
        let path = self.path(&s.path)?;
        fs::write(path.as_os_str().as_encoded_bytes(), source.as_bytes(), Expect::Hash(fs::hash(s.source.as_bytes())))
            .map_err(|e| format!("agent-edit!: {e}"))?
            .map_err(|_| "agent-edit!: stale revision; read again")?;
        let rev = revision(source.as_bytes());
        let next = self.remember(s.path.clone(), source, rev);
        Ok(fields(vec![
            ("path", text(&next.path)),
            ("revision", text(&next.revision)),
            ("previous", text(&s.revision)),
            ("changes", Data::List(summaries)),
            ("handle", text(next.handle(0, next.source.len()))),
        ]))
    }
}

struct Definition {
    name: String,
    kind: String,
    start: usize,
    end: usize,
    scope: String,
}
fn scheme_symbols(form: &Syntax, scope: &str, out: &mut Vec<Definition>) {
    let SyntaxKind::List(items, _) = &form.kind else {
        return;
    };
    let head = items.first().and_then(Syntax::sym).map(reader::symbol_name);
    let head = head.as_deref().unwrap_or("");
    if matches!(head, "quote" | "quasiquote" | "syntax-rules") {
        return;
    }
    if (head == "define" || head.starts_with("define-"))
        && let Some(signature) = items.get(1)
    {
        let names = if head == "define-library" {
            vec![reader::display_sexp(&signature.to_sexp())]
        } else if head == "define-values" {
            signature.list().unwrap_or(&[]).iter().filter_map(Syntax::sym).map(|id| reader::symbol_name(id).to_string()).collect()
        } else {
            let mut name = signature;
            while let SyntaxKind::List(xs, _) = &name.kind {
                let Some(first) = xs.first() else {
                    break;
                };
                name = first;
            }
            name.sym().map(|id| vec![reader::symbol_name(id).to_string()]).unwrap_or_default()
        };
        for name in names {
            out.push(Definition {
                name,
                kind: head.into(),
                start: form.span.0 as usize,
                end: form.span.1 as usize,
                scope: if head == "define-library" { "library" } else { scope }.into(),
            });
        }
    }
    let function_body = (head == "define" || head.starts_with("define-"))
        && !matches!(head, "define-library" | "define-values")
        && items.get(1).is_some_and(|s| s.list().is_some());
    let child_scope = if function_body
        || matches!(head, "lambda" | "case-lambda" | "let" | "let*" | "letrec" | "letrec*" | "let-values" | "let*-values")
    {
        "local"
    } else {
        scope
    };
    for item in items {
        scheme_symbols(item, child_scope, out);
    }
}
// Outer attributes and docs are preceding siblings, not part of an item
// node. Ordinary comments may bridge an attached prefix but do not by
// themselves extend it. Inner docs/attributes belong to the enclosing scope.
fn rust_definition_start(node: tree_sitter::Node<'_>, source: &str) -> usize {
    let mut start = node.start_byte();
    let mut previous = node.prev_named_sibling();
    while let Some(sibling) = previous {
        match sibling.kind() {
            "attribute_item" | "outer_attribute_item" => start = sibling.start_byte(),
            "line_comment" | "block_comment" => {
                let comment = &source[sibling.byte_range()];
                if comment.starts_with("//!") || comment.starts_with("/*!") {
                    break;
                }
                if (comment.starts_with("///") && !comment.starts_with("////"))
                    || (comment.starts_with("/**") && !comment.starts_with("/***") && !comment.starts_with("/**/"))
                {
                    start = sibling.start_byte();
                }
            }
            _ => break,
        }
        previous = sibling.prev_named_sibling();
    }
    start
}

fn rust_symbols(node: tree_sitter::Node<'_>, source: &str, out: &mut Vec<Definition>) {
    let mut pending = vec![(node, "module")];
    while let Some((node, scope)) = pending.pop() {
        if matches!(
            node.kind(),
            "function_item"
                | "function_signature_item"
                | "struct_item"
                | "enum_item"
                | "union_item"
                | "trait_item"
                | "mod_item"
                | "type_item"
                | "const_item"
                | "static_item"
                | "macro_definition"
        ) && let Some(name) = node.child_by_field_name("name")
        {
            out.push(Definition {
                name: source[name.byte_range()].into(),
                kind: node.kind().into(),
                start: rust_definition_start(node, source),
                end: node.end_byte(),
                scope: scope.into(),
            });
        }
        let child_scope = match node.kind() {
            "function_item" => "local",
            "impl_item" | "trait_item" => "member",
            _ => scope,
        };
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        pending.extend(children.into_iter().rev().map(|node| (node, child_scope)));
    }
}

pub(crate) fn install(vm: &mut Vm) {
    vm.name_foreign_type::<Project>("agent-project");
    vm.requiring(Capability::Files, |vm| {
        techne_vm::procedures! { vm;
            "(%agent-open root)" => |root: String| -> R<Foreign<Project>> { Ok(Foreign::new(Project { state: RefCell::new(State::new(&root)?) })) };
            "(%agent-canonical-root root)" => |root: String| -> R<String> { Ok(canonical_root(&root)?.to_str().unwrap().to_owned()) };
            "(%agent-root project)" => |p: Foreign<Project>| p.state.borrow().root.to_string_lossy().into_owned();
            "(%agent-files project prefix limit offset)" => |p: Foreign<Project>, prefix: String, limit: i64, start: i64| -> R<Data> {
                let limit = cap(limit, 50, "agent-files")?; let start = offset(start)?;
                let mut s = p.state.borrow_mut(); s.list()?;
                let names: Vec<_> = s.files.iter().filter(|n| n.starts_with(&prefix)).collect();
                let mut reply = match page(names.iter().skip(start).take(limit).map(|n| text(*n)).collect(), names.len(), start, limit, s.complete) { Data::List(x) => x, _ => unreachable!() };
                if let Data::List(extra) = fields(vec![("revision", text(revision(s.files.join("\0").as_bytes()))), ("skipped", number(s.listing_skipped)), ("problems", Data::List(s.listing_problems.iter().map(text).collect()))]) { reply.extend(extra); }
                Ok(Data::List(reply))
            };
            "(%agent-read project path from lines chars)" => |p: Foreign<Project>, path: String, from: i64, lines: i64, chars: i64| -> R<Data> {
                p.state.borrow_mut().read_lines(&path, cap(from, usize::MAX, "agent-read")?, cap(lines, 200, "agent-read")?, cap(chars, 16000, "agent-read")?)
            };
            "(%agent-expand project handle offset chars)" => |p: Foreign<Project>, handle: String, start: i64, chars: i64| -> R<Data> {
                p.state.borrow().expand(&handle, offset(start)?, cap(chars, 16000, "agent-expand")?)
            };
            "(%agent-search project query prefix limit offset)" => |p: Foreign<Project>, query: String, prefix: String, limit: i64, start: i64| -> R<Data> {
                p.state.borrow_mut().search(&query, &prefix, cap(limit, 50, "agent-search")?, offset(start)?)
            };
            "(%agent-symbols project path name limit offset)" => |p: Foreign<Project>, path: String, name: String, limit: i64, start: i64| -> R<Data> {
                p.state.borrow_mut().symbols(&path, &name, cap(limit, 50, "agent-symbols")?, offset(start)?)
            };
            "(%agent-edit! project path revision edits)" => |p: Foreign<Project>, path: String, expected: String, edits: Vec<Vec<String>>| -> R<Data> {
                p.state.borrow_mut().edit(&path, &expected, edits)
            };
            "(%agent-refresh! project)" => |p: Foreign<Project>| { let mut s = p.state.borrow_mut(); s.listed = false; s.files.clear(); s.snapshots.clear(); s.bytes = 0; };
        }
    });
}
