//! OS/FFI mechanisms for Scheme tools. Query and cache policy lives in Lisp.
use crate::{fs, protocol::Expect};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use techne_syntax::{
    Grammar, grammar,
    tree_sitter::{Language, Node, Query, Tree},
};
use techne_text::ropey::Rope;
use techne_vm::{
    api::Foreign,
    data::Data,
    reader::{self, Syntax, SyntaxKind},
    vm::{Capability, Vm},
};
type R<T> = Result<T, String>;
fn text(s: impl Into<String>) -> Data {
    Data::String(s.into())
}
fn num(n: usize) -> Data {
    Data::Int(n as i64)
}
fn fields(xs: Vec<(&str, Data)>) -> Data {
    Data::List(xs.into_iter().map(|(k, v)| Data::List(vec![Data::Symbol(k.into()), v])).collect())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn hash(bytes: &[u8]) -> String {
    hex(&fs::hash(bytes))
}
fn canonical(root: &str) -> R<PathBuf> {
    let p = std::fs::canonicalize(root).map_err(|e| format!("tool-project: {root}: {e}"))?;
    if !p.is_dir() || p.to_str().is_none() {
        return Err("tool-project: root must be a UTF-8 directory".into());
    }
    Ok(p)
}
fn path(root: &str, name: &str) -> R<PathBuf> {
    let p = std::fs::canonicalize(Path::new(root).join(name)).map_err(|e| format!("tool: {name}: {e}"))?;
    if !p.starts_with(root) {
        return Err("tool: path is outside the project".into());
    }
    Ok(p)
}
fn listing(root: &str, max: usize) -> R<Data> {
    if !(1..=10_000).contains(&max) {
        return Err("tool: invalid traversal limit".into());
    }
    let mut files = vec![];
    let mut complete = true;
    let mut skipped = 0;
    let mut problems = vec![];
    let mut problem = |message: String| {
        skipped += 1;
        if problems.len() < 3 {
            problems.push(text(message.chars().take(240).collect::<String>()));
        }
    };
    for entry in ignore::WalkBuilder::new(root).follow_links(false).require_git(false).build() {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                problem(e.to_string());
                complete = false;
                continue;
            }
        };
        if let Some(e) = entry.error() {
            problem(e.to_string());
            complete = false;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(name) = entry.path().strip_prefix(root).unwrap().to_str() else {
            problem("non-UTF-8 file name".into());
            complete = false;
            continue;
        };
        if files.len() == max {
            complete = false;
            break;
        }
        files.push(name.to_owned());
    }
    files.sort();
    Ok(fields(vec![
        ("items", Data::List(files.iter().map(text).collect())),
        ("revision", text(hash(files.join("\0").as_bytes()))),
        ("complete", Data::Bool(complete)),
        ("skipped", num(skipped)),
        ("problems", Data::List(problems)),
    ]))
}
fn read_file(root: &str, name: &str, max: usize) -> R<Data> {
    let p = path(root, name)?;
    if p.metadata().map_err(|e| e.to_string())?.len() > max as u64 {
        return Err("tool: file exceeds 1 MiB".into());
    }
    let (bytes, rev) = fs::read(p.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?.ok_or("tool: file disappeared")?;
    if bytes.len() > max {
        return Err("tool: file exceeds 1 MiB".into());
    }
    let size = bytes.len();
    Ok(fields(vec![
        ("path", text(p.strip_prefix(root).unwrap().to_str().ok_or("tool: non-UTF-8 file name")?)),
        ("revision", text(hex(&rev))),
        ("bytes", num(size)),
        ("text", String::from_utf8(bytes).map(text).unwrap_or(Data::Bool(false))),
    ]))
}
fn write_file(root: &str, name: &str, expected: &str, source: &str) -> R<String> {
    if source.len() > 1 << 20 {
        return Err("tool-edit!: result exceeds 1 MiB".into());
    }
    let p = path(root, name)?;
    let (_, old) = fs::read(p.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?.ok_or("tool: file disappeared")?;
    if hex(&old) != expected {
        return Err("tool-edit!: stale revision; read again".into());
    }
    let rev = fs::write(p.as_os_str().as_encoded_bytes(), source.as_bytes(), Expect::Hash(old))
        .map_err(|e| e.to_string())?
        .map_err(|_| "tool-edit!: stale revision; read again")?;
    Ok(hex(&rev))
}
struct Parsed {
    tree: Tree,
    rope: Rope,
    language: Language,
}
impl Parsed {
    fn node(&self, node: Node<'_>) -> Data {
        Data::List(vec![
            text(node.kind()),
            num(self.rope.byte_to_char(node.start_byte())),
            num(self.rope.byte_to_char(node.end_byte())),
            Data::Bool(node.is_named()),
        ])
    }
    fn find(&self, kind: &str, from: usize, to: usize) -> R<Node<'_>> {
        if from > to || to > self.rope.len_chars() {
            return Err("source: invalid node range".into());
        }
        let (a, b) = (self.rope.char_to_byte(from), self.rope.char_to_byte(to));
        let mut n = self.tree.root_node().named_descendant_for_byte_range(a, b).ok_or("source: no node here")?;
        loop {
            if n.kind() == kind && n.start_byte() == a && n.end_byte() == b {
                return Ok(n);
            }
            n = n.parent().ok_or("source: no such node")?;
        }
    }
}
fn parse_source(name: &str, source: &str) -> R<Foreign<Parsed>> {
    if source.len() > 1 << 20 {
        return Err("source: text exceeds 1 MiB".into());
    }
    let language = match grammar(name).ok_or("source: unsupported grammar")? {
        Grammar::One { language, .. } => language,
        Grammar::Markdown => tree_sitter_md::LANGUAGE.into(),
    };
    let rope = Rope::from_str(source);
    let until = Instant::now() + Duration::from_secs(2);
    let tree = techne_syntax::parse(&language, &rope, None, &[], &|| Instant::now() >= until).ok_or("source: parse cancelled")?;
    Ok(Foreign::new(Parsed { tree, rope, language }))
}
fn captures(p: &Parsed, query: &str) -> R<Data> {
    if query.len() > 16 << 10 {
        return Err("source-query: query exceeds 16 KiB".into());
    }
    let query = Query::new(&p.language, query).map_err(|e| e.to_string())?;
    let captures =
        techne_syntax::run_query(&p.tree, &p.rope, &query, 0, p.rope.len_bytes(), Instant::now() + Duration::from_secs(2), 20_000)?;
    Ok(fields(vec![
        ("complete", Data::Bool(!p.tree.root_node().has_error())),
        (
            "items",
            Data::List(
                captures
                    .into_iter()
                    .map(|c| Data::List(vec![text(c.name), num(p.rope.byte_to_char(c.start)), num(p.rope.byte_to_char(c.end))]))
                    .collect(),
            ),
        ),
    ]))
}
// Reader syntax, not guessed Scheme grammar: preserve Techne's exact reader.
fn reader_nodes(source: &str) -> R<Data> {
    fn node(s: &Syntax, rope: &Rope, left: &mut usize) -> R<Data> {
        *left = left.checked_sub(1).ok_or("source: more than 20000 syntax nodes")?;
        let (kind, children) = match &s.kind {
            SyntaxKind::List(xs, _) => ("list", xs.as_slice()),
            _ => ("atom", &[][..]),
        };
        Ok(Data::List(vec![
            text(kind),
            s.sym().map(|id| text(reader::symbol_name(id).to_string())).unwrap_or(Data::Bool(false)),
            num(rope.byte_to_char(s.span.0 as usize)),
            num(rope.byte_to_char(s.span.1 as usize)),
            Data::List(children.iter().map(|s| node(s, rope, left)).collect::<R<_>>()?),
        ]))
    }
    if source.len() > 1 << 20 {
        return Err("source: text exceeds 1 MiB".into());
    }
    let rope = Rope::from_str(source);
    let mut left = 20_000;
    Ok(Data::List(reader::read_syntax(source).map_err(|e| e.message)?.iter().map(|s| node(s, &rope, &mut left)).collect::<R<_>>()?))
}
pub(crate) fn install(vm: &mut Vm) {
    vm.name_foreign_type::<Parsed>("source-tree");
    techne_vm::procedures! { vm;
        "(%source-parse grammar source)" => |grammar: String, source: String| -> R<Foreign<Parsed>> { parse_source(&grammar, &source) };
        "(%source-query tree query)" => |p: Foreign<Parsed>, query: String| -> R<Data> { captures(&p, &query) };
        "(%source-complete? tree)" => |p: Foreign<Parsed>| !p.tree.root_node().has_error();
        "(%source-text tree from to)" => |p: Foreign<Parsed>, from: usize, to: usize| -> R<String> {
            if from > to || to > p.rope.len_chars() { return Err("source: invalid text range".into()); }
            Ok(p.rope.slice(from..to).to_string())
        };
        "(%source-node tree from to)" => |p: Foreign<Parsed>, from: usize, to: usize| -> R<Data> {
            if from > to { return Err("source: invalid node range".into()); }
            let a = p.rope.try_char_to_byte(from).map_err(|e| e.to_string())?;
            let b = p.rope.try_char_to_byte(to).map_err(|e| e.to_string())?;
            let n = p.tree.root_node().named_descendant_for_byte_range(a,b).ok_or("source: no node here")?;
            Ok(p.node(n))
        };
        "(%source-relative tree kind from to parent)" => |p: Foreign<Parsed>, kind: String, from: usize, to: usize, parent: bool| -> R<Data> {
            let n = p.find(&kind, from, to)?;
            Ok(if let Some(n) = if parent { n.parent() } else { n.prev_named_sibling() } { p.node(n) } else { Data::Bool(false) })
        };
        "(%source-reader source)" => |source: String| -> R<Data> { reader_nodes(&source) };
    }
    vm.requiring(Capability::Files, |vm| {
        techne_vm::procedures! { vm;
            "(%repository-canonical-root root)" => |root: String| -> R<String> { Ok(canonical(&root)?.to_str().unwrap().to_owned()) };
            "(%repository-nonce)" => || -> R<String> { let mut bytes = [0;16]; getrandom::fill(&mut bytes).map_err(|e| e.to_string())?; Ok(hex(&bytes)) };
            "(%repository-list root max)" => |root: String, max: usize| -> R<Data> { listing(&root, max) };
            "(%repository-read root path max)" => |root: String, path: String, max: usize| -> R<Data> { read_file(&root, &path, max.min(1 << 20)) };
            "(%repository-write root path revision source)" => |root: String, path: String, revision: String, source: String| -> R<String> { write_file(&root, &path, &revision, &source) };
        }
    });
}
