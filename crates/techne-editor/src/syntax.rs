//! Syntax highlighting with tree-sitter: the bottom layer of a pane's
//! highlights, under its modes' layers and a search's (EDITOR.md,
//! section 3, in the small until layers declare classes).
//!
//! A document's tree is kept while a pane shows it and reparsed from
//! the one edit between the text it was parsed from and the text now
//! (the common prefix and suffix; several edits since are one span to
//! tree-sitter). Highlights are upstream queries' captures over the
//! window asked for, each capture name naming a face: the names are
//! the tree-sitter convention's dotted ones, which the face table
//! resolves by prefix, so `keyword.control` needs no face of its own
//! anywhere. Overlapping captures are ordered as the queries expect
//! (smaller ranges over larger, earlier patterns over later ones on
//! one range), then composed under everything else.
//!
//! Nothing here knows about buffers or modes: the runtime says a
//! document's language (Lisp says which, `pane-display`), or none.
//! A document past `MAX` bytes is not parsed: it draws plain rather
//! than make a snapshot wait on it.

use std::collections::HashMap;

use techne_text::ropey::Rope;
use tree_sitter::{InputEdit, Language, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};

use crate::faces::FaceRange;

/// The most text parsed for one document.
const MAX: usize = 4 << 20;

/// A language: its grammar and highlight queries. Markdown is two
/// grammars (block and inline), parsed together by its own parser.
enum Grammar {
    One { language: Language, highlights: &'static str },
    Markdown,
}

fn grammar(language: &str) -> Option<Grammar> {
    let one = |l: tree_sitter_language::LanguageFn, highlights: &'static str| Grammar::One { language: l.into(), highlights };
    Some(match language {
        "rust" => one(tree_sitter_rust::LANGUAGE, tree_sitter_rust::HIGHLIGHTS_QUERY),
        "python" => one(tree_sitter_python::LANGUAGE, tree_sitter_python::HIGHLIGHTS_QUERY),
        "c" => one(tree_sitter_c::LANGUAGE, tree_sitter_c::HIGHLIGHT_QUERY),
        "javascript" => one(tree_sitter_javascript::LANGUAGE, tree_sitter_javascript::HIGHLIGHT_QUERY),
        "typescript" => one(tree_sitter_typescript::LANGUAGE_TYPESCRIPT, tree_sitter_typescript::HIGHLIGHTS_QUERY),
        "tsx" => one(tree_sitter_typescript::LANGUAGE_TSX, tree_sitter_typescript::HIGHLIGHTS_QUERY),
        "go" => one(tree_sitter_go::LANGUAGE, tree_sitter_go::HIGHLIGHTS_QUERY),
        "json" => one(tree_sitter_json::LANGUAGE, tree_sitter_json::HIGHLIGHTS_QUERY),
        "yaml" => one(tree_sitter_yaml::LANGUAGE, tree_sitter_yaml::HIGHLIGHTS_QUERY),
        "toml" => one(tree_sitter_toml_ng::LANGUAGE, tree_sitter_toml_ng::HIGHLIGHTS_QUERY),
        "scheme" => one(tree_sitter_scheme::LANGUAGE, tree_sitter_scheme::HIGHLIGHTS_QUERY),
        "bash" => one(tree_sitter_bash::LANGUAGE, tree_sitter_bash::HIGHLIGHT_QUERY),
        "markdown" => Grammar::Markdown,
        _ => return None,
    })
}

enum Parsed {
    One(Tree),
    Markdown(tree_sitter_md::MarkdownTree),
}

struct Cached {
    language: String,
    /// The revision `parsed` was parsed from: unchanged, it still holds.
    revision: u64,
    /// The text `parsed` was parsed from: the next parse's edit is
    /// measured against it.
    source: String,
    parsed: Parsed,
}

/// A language's state: its queries compiled once, or why not.
enum Queries {
    One(Result<Query, String>),
    Markdown(Result<(Query, Query), String>),
}

/// Parses documents and answers highlights, keeping a tree per
/// document last shown with one. A parse or query that fails is said
/// once, through the error `highlights` hands back with its ranges.
#[derive(Default)]
pub struct Syntax {
    docs: HashMap<u64, Cached>,
    queries: HashMap<String, Queries>,
    /// What has been said already: a failure is handed back once, not
    /// on every snapshot it goes on failing in.
    said: std::collections::HashSet<String>,
}

impl Syntax {
    /// The face ranges of `language`'s syntax in `text` (document `doc`)
    /// between `from` and `to`; with them, what went wrong this call
    /// for the first time, if anything.
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
            self.docs.remove(&doc);
            return (Vec::new(), None);
        }
        if !self.queries.contains_key(language) {
            let compiled = match &grammar {
                Grammar::One { language: l, highlights } => Queries::One(Query::new(l, highlights).map_err(|e| e.to_string())),
                Grammar::Markdown => Queries::Markdown(
                    Query::new(&tree_sitter_md::LANGUAGE.into(), tree_sitter_md::HIGHLIGHT_QUERY_BLOCK)
                        .and_then(|block| {
                            Ok((block, Query::new(&tree_sitter_md::INLINE_LANGUAGE.into(), tree_sitter_md::HIGHLIGHT_QUERY_INLINE)?))
                        })
                        .map_err(|e: tree_sitter::QueryError| e.to_string()),
                ),
            };
            self.queries.insert(language.to_string(), compiled);
        }
        let queries = self.queries.get(language).expect("compiled above or already");
        // Unchanged since parsed: the tree and its source still hold,
        // and the rope is not flattened again for them.
        if self.docs.get(&doc).is_none_or(|c| c.language != language || c.revision != revision) {
            let source = text.to_string();
            let mut old = self.docs.remove(&doc).filter(|c| c.language == language);
            let edit = old.as_ref().and_then(|c| edit_between(&c.source, &source));
            if let (Some(edit), Some(old)) = (edit, &mut old) {
                match &mut old.parsed {
                    Parsed::One(t) => t.edit(&edit),
                    Parsed::Markdown(t) => t.edit(&edit),
                }
            }
            let parsed = match &grammar {
                Grammar::One { language: l, .. } => {
                    let mut parser = Parser::new();
                    let before = edit.and(old.as_ref()).and_then(|c| match &c.parsed {
                        Parsed::One(t) => Some(t),
                        _ => None,
                    });
                    parser.set_language(l).ok().and_then(|_| parser.parse(source.as_bytes(), before)).map(Parsed::One)
                }
                Grammar::Markdown => {
                    let before = edit.and(old.as_ref()).and_then(|c| match &c.parsed {
                        Parsed::Markdown(t) => Some(t),
                        _ => None,
                    });
                    tree_sitter_md::MarkdownParser::default().parse(source.as_bytes(), before).map(Parsed::Markdown)
                }
            };
            let Some(parsed) = parsed else {
                let said = {
                    let m = format!("syntax: {language} did not parse");
                    self.said.insert(m.clone()).then_some(m)
                };
                return (Vec::new(), said);
            };
            self.docs.insert(doc, Cached { language: language.to_string(), revision, source, parsed });
        }
        let cached = self.docs.get(&doc).expect("parsed above or already");
        let bytes = cached.source.as_bytes();
        let mut found: Vec<(usize, FaceRange)> = Vec::new();
        match (&queries, &cached.parsed) {
            (Queries::One(Ok(q)), Parsed::One(t)) => collect(q, t.root_node(), bytes, from, to, &mut found),
            (Queries::Markdown(Ok((block, inline))), Parsed::Markdown(t)) => {
                collect(block, t.block_tree().root_node(), bytes, from, to, &mut found);
                for t in t.inline_trees() {
                    let root = t.root_node();
                    if root.start_byte() < to && root.end_byte() > from {
                        collect(inline, root, bytes, from, to, &mut found);
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
            _ => {}
        }
        // The queries' own precedence, as their highlighters read them:
        // a smaller range paints over a larger one that holds it (an
        // escape in a string), and of two captures over one range the
        // earlier pattern wins (a key over a string, a constant over a
        // constructor). Composed later-wins, that is longest first,
        // highest pattern first.
        found.sort_by(|(pa, ra), (pb, rb)| (rb.to - rb.from).cmp(&(ra.to - ra.from)).then(pb.cmp(pa)));
        (found.into_iter().map(|(_, r)| r).collect(), None)
    }

    /// Forget every document but these (the runtime's shown panes, each
    /// snapshot): a closed document's tree goes with it.
    pub fn retain(&mut self, docs: &[u64]) {
        self.docs.retain(|id, _| docs.contains(id));
    }
}

/// A query's captures over a window, each (pattern, range): the
/// pattern orders them, the name names a face. Captures that draw
/// nothing (private to the query, spelling, the explicit `none`) are
/// left out; markdown's older `@text.*` names map to `markup.*` faces.
fn collect(query: &Query, root: tree_sitter::Node, bytes: &[u8], from: usize, to: usize, out: &mut Vec<(usize, FaceRange)>) {
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(from..to);
    let mut matches = cursor.matches(query, root, bytes);
    while let Some(m) = matches.next() {
        for capture in m.captures() {
            let name = query.capture_names()[capture.index as usize];
            let Some(face) = face_of(name) else { continue };
            let range = capture.node.byte_range();
            if range.start < range.end {
                out.push((m.pattern_index, FaceRange { from: range.start, to: range.end, face }));
            }
        }
    }
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
/// where their common suffix begins. #f (None) when they are the same.
fn edit_between(old: &str, new: &str) -> Option<InputEdit> {
    if old == new {
        return None;
    }
    let (old, new) = (old.as_bytes(), new.as_bytes());
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old.iter().rev().zip(new.iter().rev()).take_while(|(a, b)| a == b).count().min(old.len() - prefix).min(new.len() - prefix);
    let point = |text: &[u8], at: usize| {
        let before = &text[..at];
        Point { row: before.iter().filter(|&&b| b == b'\n').count(), column: before.iter().rev().take_while(|&&b| b != b'\n').count() }
    };
    Some(InputEdit {
        start_byte: prefix,
        old_end_byte: old.len() - suffix,
        new_end_byte: new.len() - suffix,
        start_position: point(old, prefix),
        old_end_position: point(old, old.len() - suffix),
        new_end_position: point(new, new.len() - suffix),
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
}
