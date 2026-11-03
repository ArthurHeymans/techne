//! Markdown rendered (`lisp/editor/packages/markdown.scm`): a preview of
//! a markdown buffer whose text is excerpts of it, without its markup.

use std::time::{Duration, Instant};

use techne_editor::{
    present::{Input, Snapshot},
    runtime::Runtime,
};

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn text(s: &Snapshot, pane: usize) -> String {
    s.panes[pane].text.to_string()
}

fn preview(markdown: &str) -> (tempfile::TempDir, Runtime) {
    preview_with(markdown, &[])
}

/// A preview of `markdown`, the files `beside` (name, contents) next to it.
fn preview_with(markdown: &str, beside: &[(&str, &str)]) -> (tempfile::TempDir, Runtime) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("README.md"), markdown).unwrap();
    for (name, contents) in beside {
        std::fs::write(root.join(name), contents).unwrap();
    }
    let mut rt = Runtime::open(&root.join("README.md"), &root.join("README.journal"), "emacs").unwrap().0;
    rt.eval("(run-command (current-session) 'markdown-preview 1)").unwrap();
    assert_eq!(rt.take_errors(), Vec::<String>::new());
    (dir, rt)
}

/// What the preview shows, once the runtime has rendered it.
fn rendered(rt: &mut Runtime) -> String {
    text(&rt.snapshot(), 1)
}

#[test]
fn markup_goes_and_blocks_are_drawn() {
    let (_dir, mut rt) = preview(
        "# Title\n\nSome *emph* and **strong** text,\na `code` span and [a link](https://example.com).\n\n\
         - one\n- two\n  - nested\n\n1. first\n2. second\n\n- [ ] open\n- [x] done\n\n> quoted\n> text\n\n\
         ```rust\nfn main() {}\n```\n\n| a | bb |\n|---|---:|\n| 1 | 2 |\n\n---\n",
    );
    let shown = rendered(&mut rt);
    let expected = "\
Title

Some emph and strong text, a code span and a link.

• one
• two
  ◦ nested

1. first
2. second

☐ open
☑ done

┃ quoted text

 fn main() {}

┌───┬────┐
│ a │ bb │
├───┼────┤
│ 1 │  2 │
└───┴────┘


";
    assert_eq!(shown, expected);
}

#[test]
fn editing_the_rendering_edits_the_markdown() {
    let (_dir, mut rt) = preview("Some **strong** text.\n");
    keys(&mut rt, "C-x o");
    // After "Some ", in the strong text.
    rt.eval("(move! (current-session) (lambda (h) 7))").unwrap();
    keys(&mut rt, "X");
    let s = rt.snapshot();
    assert_eq!(text(&s, 0), "Some **stXrong** text.\n");
    let deadline = Instant::now() + Duration::from_secs(5);
    while rendered(&mut rt) != "Some stXrong text.\n" {
        assert!(Instant::now() < deadline, "{:?}", rendered(&mut rt));
        rt.run_tasks(Duration::from_millis(20)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_paragraph_of_several_lines_stays_one_row_after_an_edit() {
    let (_dir, mut rt) = preview("one two\nthree four\n");
    assert_eq!(rendered(&mut rt), "one two three four\n");
    keys(&mut rt, "C-x o");
    rt.eval("(move! (current-session) (lambda (h) 3))").unwrap();
    keys(&mut rt, "X");
    assert_eq!(text(&rt.snapshot(), 0), "oneX two\nthree four\n");
    let deadline = Instant::now() + Duration::from_secs(5);
    while rendered(&mut rt) != "oneX two three four\n" {
        assert!(Instant::now() < deadline, "{:?} {:?}", rendered(&mut rt), rt.take_errors());
        rt.run_tasks(Duration::from_millis(20)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn items_hang_under_their_text_and_pictures_show_above_their_line() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#ff0000"/></svg>"##;
    let (_dir, mut rt) = preview_with("- an item\n\n> quoted\n\n![the logo](logo.svg)\n", &[("logo.svg", svg)]);
    let s = rt.snapshot();
    let pane = &s.panes[1];
    let text = pane.text.to_string();
    assert_eq!(text, "• an item\n\n┃ quoted\n\n▣ the logo\n");
    let hangs: Vec<(usize, usize)> = pane.hangs.iter().map(|h| (h.line, h.at)).collect();
    assert_eq!(hangs, [(0, "• ".len()), (text.find('┃').unwrap(), text.find('┃').unwrap() + "┃ ".len())]);
    // The picture loads in a task; then the snapshots show it.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = rt.snapshot();
        if let Some(b) = s.panes[1].blocks.first() {
            assert_eq!(b.at, text.find('▣').unwrap());
            assert_eq!(b.image.size(), (40, 20));
            break;
        }
        assert!(Instant::now() < deadline, "no picture: {:?}", rt.take_errors());
        rt.run_tasks(Duration::from_millis(20)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn parsing_again_after_an_edit_gives_what_parsing_whole_does() {
    // Chunks kept from the last parse, moved by an edit above them, and
    // the chunk edited parsed again: the same as parsing it all.
    let md = "# One\n\npara *a*\n\n- item\n\n  more of it\n- two\n\n```rust\nfn f() {}\n\n```\n\n> q\n\n| a |\n|---|\n| 1 |\n";
    let (_dir, mut rt) = preview(md);
    rt.eval("(define d (buffer-document (buffer-named \"README.md\")))").unwrap();
    rt.eval("(define (norm x) (cond ((target? x) (list 'target (target-kind x) (target-value x))) ((pair? x) (cons (norm (car x)) (norm (cdr x)))) (else x)))").unwrap();
    rt.eval("(define cache (make-hash-table))").unwrap();
    let same = "(equal? (norm (markdown->rich d)) (norm (markdown->rich d #:cache cache)))";
    assert_eq!(rt.eval(same).unwrap(), "#t");
    for (at, text) in [(2, "Two "), (14, "b "), (40, "\n\nnew paragraph"), (0, "")] {
        rt.eval(&format!("(view-edit! (make-view d \"test\") (list (list {at} {at} {text:?})) \"new\")")).unwrap();
        assert_eq!(rt.eval(same).unwrap(), "#t", "after inserting {text:?} at {at}");
    }
}

/// What the preview shows of `markdown`.
fn shown_of(markdown: &str) -> String {
    let (_dir, mut rt) = preview(markdown);
    rendered(&mut rt)
}

#[test]
fn emphasis_code_spans_and_hard_breaks_as_commonmark_has_them() {
    // A run closing two emphases at once; emphasis inside emphasis.
    assert_eq!(shown_of("**bold and *italic***\n"), "bold and italic\n");
    assert_eq!(shown_of("*a **b** c*\n"), "a b c\n");
    assert_eq!(shown_of("*a *b c*\n"), "*a b c\n");
    // A code span of spaces keeps them all; others lose one at each end.
    assert_eq!(shown_of("x`   `y\n"), "x   y\n");
    assert_eq!(shown_of("` a `\n"), "a\n");
    // Hard breaks: two spaces or a backslash; a soft one is a space.
    assert_eq!(shown_of("foo  \nbar\\\nbaz\nqux\n"), "foo\nbar\nbaz qux\n");
}

#[test]
fn a_cached_parse_is_the_whole_parse_around_fences_in_containers() {
    // A list's fence closing, then a top-level fence with a blank line
    // in it: cut into chunks by blank lines, this parsed apart.
    let md = "- ```\n  a\n  ```\n\n```\nb\n\nc\n```\n\n> ```\n> q\n\nafter\n";
    let (_dir, mut rt) = preview(md);
    rt.eval("(define d (buffer-document (buffer-named \"README.md\")))").unwrap();
    rt.eval("(define (norm x) (cond ((target? x) (list 'target (target-kind x) (target-value x))) ((pair? x) (cons (norm (car x)) (norm (cdr x)))) (else x)))").unwrap();
    rt.eval("(define cache (make-hash-table))").unwrap();
    let same = "(equal? (norm (markdown->rich d)) (norm (markdown->rich d #:cache cache)))";
    assert_eq!(rt.eval(same).unwrap(), "#t");
    for (at, text) in [(0, "x"), (22, "\n\n"), (md.len() - 6, "```\n")] {
        rt.eval(&format!("(view-edit! (make-view d \"test\") (list (list {at} {at} {text:?})) \"new\")")).unwrap();
        assert_eq!(rt.eval(same).unwrap(), "#t", "after inserting {text:?} at {at}");
    }
}

#[test]
fn a_caret_stays_on_its_text_when_rows_are_added_above() {
    let (_dir, mut rt) = preview("alpha\n\nbeta\n\ngamma\n");
    keys(&mut rt, "C-x o");
    rt.eval("(move! (current-session) (lambda (h) 8))").unwrap();
    assert_eq!(rendered(&mut rt), "alpha\n\nbeta\n\ngamma\n");
    rt.eval("(view-edit! (make-view (buffer-document (buffer-named \"README.md\")) \"test\") (list (list 0 0 \"new\\n\\n\")) \"new\")")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while rendered(&mut rt) != "new\n\nalpha\n\nbeta\n\ngamma\n" {
        assert!(Instant::now() < deadline, "{:?}", rendered(&mut rt));
        rt.run_tasks(Duration::from_millis(20)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(rt.snapshot().panes[1].head(), 13, "still after the b of beta");
}

#[test]
fn the_last_row_keeps_its_rule() {
    let (_dir, mut rt) = preview("# H\n");
    let s = rt.snapshot();
    let pane = &s.panes[1];
    // Over the heading's line break: there it reaches the pane's edge.
    let ruled = pane.layers.iter().any(|h| {
        let st = s.style(h.style);
        h.from <= 1 && h.to == 2 && pane.text.len_bytes() == 2 && st.extend == Some(true) && st.underline.is_some()
    });
    assert!(ruled, "the heading's rule, through its line break: {:?}", pane.layers);
}

#[test]
fn an_empty_code_line_is_edited_through() {
    let (_dir, mut rt) = preview("```\n\n```\n");
    assert_eq!(rendered(&mut rt), " \n");
    keys(&mut rt, "C-x o");
    rt.eval("(move! (current-session) (lambda (h) 1))").unwrap();
    keys(&mut rt, "x");
    assert_eq!(text(&rt.snapshot(), 0), "```\nx\n```\n");
}

#[test]
fn every_picture_of_a_row_shows() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"##;
    let (_dir, mut rt) = preview_with("![one](one.svg) ![two](two.svg)\n", &[("one.svg", svg), ("two.svg", svg)]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while rt.snapshot().panes[1].blocks.len() < 2 {
        assert!(Instant::now() < deadline, "{:?}", rt.snapshot().panes[1].blocks);
        rt.run_tasks(Duration::from_millis(20)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn plain_text_reads_as_it_is() {
    let (_dir, mut rt) = preview("");
    let plain = "a **b** _c_ `d`\n# not a heading\n  - not an item\n\n<x> & [y](z)";
    rt.eval(&format!(
        "(define b (make-rich-buffer! \"*plain*\")) (rich-present! b (markdown->rich (make-document (markdown-escape {plain:?}))))"
    ))
    .unwrap();
    let expected = "a **b** _c_ `d`\n# not a heading\n\u{a0}\u{a0}- not an item\n\n<x> & [y](z)\n";
    // In Scheme's notation: a no-break space is \xA0;.
    let lisp = format!("{expected:?}").replace("\\u{a0}", "\\xA0;");
    assert_eq!(rt.eval(&format!("(string=? (document-string (buffer-document b)) {lisp})")).unwrap(), "#t");
}

#[test]
fn a_killed_preview_stops_following() {
    let (_dir, mut rt) = preview("text\n");
    let live = "(length (filter (lambda (r) (guard (e (#t #f)) (not (task-done? r)))) (scope-resources (package-scope (find-package 'markdown-preview)))))";
    assert_eq!(rt.eval(live).unwrap(), "1", "the follower");
    rt.eval("(drop-buffer! (current-session) (buffer-named \"*preview README.md*\"))").unwrap();
    for _ in 0..10 {
        rt.run_tasks(Duration::from_millis(20)).unwrap();
    }
    assert_eq!(rt.eval(live).unwrap(), "0", "gone with its buffer, though the markdown never changed");
}
