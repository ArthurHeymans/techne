//! Every grammar the crate names resolves; each single-language
//! grammar's highlight queries compile against it, and parsing plus
//! querying a rope works. (Markdown is its two-grammar case, resolved
//! here and exercised where it is parsed.)

use std::time::{Duration, Instant};

use techne_syntax::{GRAMMARS, Grammar, grammar, parse, run_query, tree_sitter::Query};
use techne_text::ropey::Rope;

#[test]
fn every_named_grammar_resolves() {
    for name in GRAMMARS {
        assert!(grammar(name).is_some(), "{name} is named but does not resolve");
    }
    assert!(grammar("cobol").is_none());
    assert!(matches!(grammar("markdown"), Some(Grammar::Markdown)));
}

#[test]
fn every_single_language_highlight_query_compiles_against_its_language() {
    for name in GRAMMARS {
        let Some(Grammar::One { language, highlights }) = grammar(name) else { continue };
        assert!(!highlights.is_empty(), "{name} has no highlight queries");
        let joined = highlights.concat();
        Query::new(&language, &joined).unwrap_or_else(|e| panic!("{name}: highlight queries do not compile: {e}"));
    }
}

fn captures(language_name: &str, text: &str) -> Vec<(String, String)> {
    let Some(Grammar::One { language, highlights }) = grammar(language_name) else { panic!("{language_name} is not one grammar") };
    let rope = Rope::from_str(text);
    let tree = parse(&language, &rope, None, &[], &|| false).unwrap();
    assert!(!tree.root_node().has_error(), "{language_name}: {text:?} parses with errors: {}", tree.root_node().to_sexp());
    let query = Query::new(&language, &highlights.concat()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    run_query(&tree, &rope, &query, 0, text.len(), deadline, 1000)
        .unwrap()
        .into_iter()
        .map(|c| (c.name, text[c.start..c.end].to_string()))
        .collect()
}

#[test]
fn parsing_and_querying_a_rope() {
    assert!(captures("rust", "fn main() {}").iter().any(|(n, t)| n.starts_with("keyword") && t == "fn"));
    assert!(captures("scheme", "(define x 1)").iter().any(|(_, t)| t == "define"));
    assert!(captures("json", r#"{"a": 1}"#).iter().any(|(_, t)| t == "\"a\""));
}

#[test]
fn a_query_over_its_capture_limit_is_an_error() {
    let Some(Grammar::One { language, highlights }) = grammar("rust") else { unreachable!() };
    let text = "fn a() {} fn b() {} fn c() {}";
    let rope = Rope::from_str(text);
    let tree = parse(&language, &rope, None, &[], &|| false).unwrap();
    let query = Query::new(&language, &highlights.concat()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let err = match run_query(&tree, &rope, &query, 0, text.len(), deadline, 1) {
        Ok(_) => panic!("one capture was allowed, the rest not reported"),
        Err(e) => e,
    };
    assert!(err.contains("more than 1 captures"), "{err}");
}

#[test]
fn a_stop_that_has_come_ends_the_parse() {
    let Some(Grammar::One { language, .. }) = grammar("rust") else { unreachable!() };
    // A parse asks as it goes; give it enough to be asked about.
    let rope = Rope::from_str(&"fn main() { let x = 1; }\n".repeat(5000));
    assert!(parse(&language, &rope, None, &[], &|| true).is_none());
}
