//! Parser mechanisms shared by editor documents and headless source tools.
use std::{cell::Cell, ops::ControlFlow, time::Instant};
use techne_text::ropey::Rope;
pub use tree_sitter;
use tree_sitter::{Language, Node, ParseOptions, Parser, Point, Query, QueryCursor, QueryCursorOptions, Range, StreamingIterator, Tree};

/// The grammars built in.
pub const GRAMMARS: [&str; 13] =
    ["rust", "python", "c", "javascript", "typescript", "tsx", "go", "json", "yaml", "toml", "scheme", "bash", "markdown"];

/// A grammar and its own highlight queries. Markdown is two grammars,
/// block and inline, the inline one parsed over the block tree's inline
/// nodes.
pub enum Grammar {
    One { language: Language, highlights: &'static [&'static str] },
    Markdown,
}

pub fn grammar(name: &str) -> Option<Grammar> {
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

/// Feed `rope` to a parse, a chunk at a time.
fn reader<'r>(rope: &'r Rope) -> impl FnMut(usize, Point) -> &'r [u8] {
    move |at, _| match rope.get_chunk_at_byte(at) {
        Some((chunk, start, _, _)) if at < rope.len_bytes() => &chunk.as_bytes()[at - start..],
        _ => &[],
    }
}

/// Parse `rope`, from `old` edited toward it, until `stop` says so.
pub fn parse(language: &Language, rope: &Rope, old: Option<&Tree>, ranges: &[Range], stop: &dyn Fn() -> bool) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    if !ranges.is_empty() {
        parser.set_included_ranges(ranges).ok()?;
    }
    let mut progress = |_: &tree_sitter::ParseState| if stop() { ControlFlow::Break(()) } else { ControlFlow::Continue(()) };
    let options = ParseOptions::new().progress_callback(&mut progress);
    parser.parse_with_options(&mut reader(rope), old, Some(options))
}

pub struct Capture {
    pub name: String,
    pub start: usize,
    pub end: usize,
}

/// The captures of `query` over `from`..`to` of `tree` (whose text is
/// `rope`), each (capture start end), bounded by `deadline` and by
/// `max`.
pub fn run_query(
    tree: &Tree,
    rope: &Rope,
    query: &Query,
    from: usize,
    to: usize,
    deadline: Instant,
    max: usize,
) -> Result<Vec<Capture>, String> {
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
        if out.len() >= max {
            return Err(format!("syntax-query: more than {max} captures"));
        }
        if out.len() % 1024 == 1023 && Instant::now() >= deadline {
            ran_out.set(true);
            break;
        }
        let c = m.captures()[*i];
        out.push(Capture { name: query.capture_names()[c.index as usize].to_owned(), start: c.node.start_byte(), end: c.node.end_byte() });
    }
    if ran_out.get() {
        return Err("syntax-query: the query took too long".into());
    }
    Ok(out)
}
