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

 rust
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
    while rendered(&mut rt) != "Some stXrong text." {
        assert!(Instant::now() < deadline, "{:?}", rendered(&mut rt));
        rt.run_tasks(Duration::from_millis(20)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_paragraph_of_several_lines_stays_one_row_after_an_edit() {
    let (_dir, mut rt) = preview("one two\nthree four\n");
    assert_eq!(rendered(&mut rt), "one two three four");
    keys(&mut rt, "C-x o");
    rt.eval("(move! (current-session) (lambda (h) 3))").unwrap();
    keys(&mut rt, "X");
    assert_eq!(text(&rt.snapshot(), 0), "oneX two\nthree four\n");
    let deadline = Instant::now() + Duration::from_secs(5);
    while rendered(&mut rt) != "oneX two three four" {
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
    assert_eq!(text, "• an item\n\n┃ quoted\n\n▣ the logo");
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
