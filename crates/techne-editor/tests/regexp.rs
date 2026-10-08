//! Regular expressions (SRFI 115) searching documents: over the rope's
//! chunks as they are, forward and backward.

use techne_editor::runtime::Runtime;
use techne_text::{Document, ropey::Rope};

fn rt() -> Runtime {
    let mut rt = Runtime::with_document(Document::new(""), "emacs").unwrap();
    rt.eval("(import (srfi 115))").unwrap();
    rt
}

#[test]
fn matches_across_chunks_are_found() {
    let text: String = (0..3000).map(|i| format!("item{i} ")).collect();
    // The rope keeps the text in chunks of about a kilobyte, so some items
    // cross from one chunk into the next.
    let rope = Rope::from_str(&text);
    let mut ends = rope.chunks().scan(0, |at, c| {
        *at += c.len();
        Some(*at)
    });
    assert!(
        ends.any(|end| text[..end].ends_with(|c: char| c.is_ascii_alphanumeric())
            && text[end..].starts_with(|c: char| c.is_ascii_alphanumeric()))
    );
    let mut r = rt();
    r.eval(&format!("(define d (make-document {text:?}))")).unwrap();
    r.eval("(define re (regexp '(: \"item\" (+ num))))").unwrap();
    assert_eq!(r.eval("(length (search-text-regexp-all d re 0 (document-length d)))").unwrap(), "3000");
    // Each match is a whole item.
    let spans =
        r.eval("(map (lambda (s) (document-substring d (car s) (cadr s))) (search-text-regexp-all d re 0 (document-length d)))").unwrap();
    assert_eq!(spans, format!("({})", (0..3000).map(|i| format!("\"item{i}\"")).collect::<Vec<_>>().join(" ")));
}

#[test]
fn backward_finds_the_last_match_before() {
    let mut r = rt();
    r.eval("(define d (make-document \"abc abc aaa\"))").unwrap();
    assert_eq!(r.eval("(search-text-regexp d 0 (regexp \"abc\") #t)").unwrap(), "(0 3)");
    assert_eq!(r.eval("(search-text-regexp d 1 (regexp \"abc\") #t)").unwrap(), "(4 7)");
    assert_eq!(r.eval("(search-text-regexp d 7 (regexp \"abc\") #f)").unwrap(), "(4 7)");
    assert_eq!(r.eval("(search-text-regexp d 6 (regexp \"abc\") #f)").unwrap(), "(0 3)");
    // Matches are found from the start, not overlapping.
    assert_eq!(r.eval("(search-text-regexp d 11 (regexp \"aa\") #f)").unwrap(), "(8 10)");
    assert_eq!(r.eval("(search-text-regexp d 3 (regexp \"x\") #f)").unwrap(), "#f");
    // A search from a position sees the character before it.
    assert_eq!(r.eval("(search-text-regexp-all d (regexp '(or (: bow \"b\") \"a\")) 0 3)").unwrap(), "((0 1))");
}

#[test]
fn searching_is_linear_in_the_text() {
    let mut r = rt();
    // Backward with a pattern matching everywhere, and every match of a
    // frequent one: each search reads on from the last, not from the start.
    r.eval(&format!("(define d (make-document {:?}))", "a".repeat(1 << 20))).unwrap();
    let start = std::time::Instant::now();
    assert_eq!(r.eval("(search-text-regexp d (document-length d) (regexp '(+ any)) #f)").unwrap(), "(0 1048576)");
    assert_eq!(r.eval("(length (search-text-regexp-all d (regexp \"a\") 0 (document-length d)))").unwrap(), "1048576");
    assert!(start.elapsed() < std::time::Duration::from_secs(10), "{:?}", start.elapsed());
}
