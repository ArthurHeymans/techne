//! Directories' listings (`lisp/editor/packages/dired.scm`) and the buffer
//! list (`packages/ibuffer.scm`): rows marked and acted on together
//! (`lisp/editor/listings.scm`).

use std::{path::PathBuf, time::Instant};

use techne_editor::{
    present::{Input, Snapshot},
    runtime::Runtime,
};

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn type_text(rt: &mut Runtime, text: &str) {
    for c in text.chars() {
        let key = if c == ' ' { "SPC".to_string() } else { c.to_string() };
        rt.handle(Input::Key { key, at: Instant::now() });
    }
}

fn text(s: &Snapshot) -> String {
    s.pane().text.to_string()
}

/// The line the caret is on in the focused pane.
fn caret_line(s: &Snapshot) -> String {
    let t = &s.pane().text;
    t.line(t.byte_to_line(s.pane().head())).to_string().trim_end().to_string()
}

/// A directory with a.txt, b.txt and sub/ in it, and a runtime on a.txt.
fn directory(profile: &str) -> (tempfile::TempDir, tempfile::TempDir, PathBuf, Runtime) {
    let (dir, state) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(root.join("b.txt"), "beta\n").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/c.txt"), "gamma\n").unwrap();
    let mut rt = Runtime::open(&root.join("a.txt"), &state.path().join("a.journal"), profile).unwrap().0;
    rt.eval("(set-option! 'dired-details #f)").unwrap();
    (dir, state, root, rt)
}

/// The names listed, as the rows show them after their mark.
fn names(s: &Snapshot) -> Vec<String> {
    text(s).lines().skip(1).map(|l| l[1..].trim().to_string()).collect()
}

#[test]
fn a_listing_goes_into_and_out_of_directories() {
    let (_dir, _state, root, mut rt) = directory("emacs");
    keys(&mut rt, "C-x C-j");
    let s = rt.snapshot();
    assert_eq!(names(&s), ["../", "sub/", "a.txt", "b.txt"]);
    assert_eq!(caret_line(&s).trim(), "a.txt", "at the file jumped from");
    assert!(s.pane().status.contains("dired"), "{}", s.pane().status);
    keys(&mut rt, "p RET");
    let s = rt.snapshot();
    assert_eq!(names(&s), ["../", "c.txt"]);
    assert!(text(&s).starts_with(&format!("{}/sub/", root.display())) || text(&s).contains("/sub/"));
    keys(&mut rt, "^");
    let s = rt.snapshot();
    assert_eq!(caret_line(&s).trim(), "sub/", "back on the directory come from");
    // One listing, moved: going into a directory reuses it.
    assert_eq!(rt.eval("(length (filter (lambda (b) (eq? (buffer-mode b) 'dired-mode)) (buffer-list)))").unwrap(), "1");
    keys(&mut rt, "n n RET");
    assert_eq!(text(&rt.snapshot()), "beta\n");
    // C-x d reads a directory: ./ is the one typed, others are gone into.
    keys(&mut rt, "C-x d");
    type_text(&mut rt, "sub/");
    keys(&mut rt, "RET");
    assert_eq!(names(&rt.snapshot()), ["../", "c.txt"]);
}

#[test]
fn marked_entries_are_copied_renamed_and_deleted() {
    let (_dir, _state, root, mut rt) = directory("emacs");
    keys(&mut rt, "C-x C-j");
    // Mark a.txt and b.txt, copy them into sub/.
    keys(&mut rt, "m m");
    assert_eq!(text(&rt.snapshot()).lines().filter(|l| l.starts_with('*')).count(), 2);
    keys(&mut rt, "C");
    keys(&mut rt, "C-a C-k");
    type_text(&mut rt, &format!("{}/sub/", root.display()));
    keys(&mut rt, "RET");
    assert!(rt.snapshot().echo.contains("Copied 2 of 2"), "{}", rt.snapshot().echo);
    assert_eq!(std::fs::read_to_string(root.join("sub/b.txt")).unwrap(), "beta\n");
    // Renaming the one at point when none are marked.
    keys(&mut rt, "U M-< n n n R C-a C-k");
    type_text(&mut rt, &format!("{}/z.txt", root.display()));
    keys(&mut rt, "RET");
    assert!(!root.join("a.txt").exists() && root.join("z.txt").exists());
    assert_eq!(names(&rt.snapshot()), ["../", "sub/", "b.txt", "z.txt"]);
    // Flag sub/ and delete it, with all it holds, after saying yes.
    keys(&mut rt, "M-< n n d x");
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    assert!(!root.join("sub").exists());
    assert_eq!(names(&rt.snapshot()), ["../", "b.txt", "z.txt"]);
}

#[test]
fn names_are_edited_in_place() {
    let (_dir, _state, root, mut rt) = directory("emacs");
    keys(&mut rt, "C-x C-j C-x C-q");
    assert!(rt.snapshot().pane().status.contains("wdired"));
    // The caret is on a.txt's name: change it, and nothing else may be.
    keys(&mut rt, "C-e");
    type_text(&mut rt, ".bak");
    keys(&mut rt, "C-a C-d");
    assert_eq!(rt.snapshot().echo, "Text is read-only");
    keys(&mut rt, "C-c C-c");
    assert!(root.join("a.txt.bak").exists() && !root.join("a.txt").exists(), "{}", rt.snapshot().echo);
    assert!(rt.snapshot().pane().status.contains("dired"));
}

#[test]
fn dirvish_previews_the_entry_at_point() {
    let (_dir, _state, _root, mut rt) = directory("emacs");
    keys(&mut rt, "C-x C-j C-c o v");
    let s = rt.snapshot();
    assert_eq!(s.panes.len(), 3, "{}", s.echo);
    assert_eq!(s.panes[2].text.to_string(), "alpha\n");
    keys(&mut rt, "n");
    assert_eq!(rt.snapshot().panes[2].text.to_string(), "beta\n");
    keys(&mut rt, "M-< n n");
    assert_eq!(rt.snapshot().panes[2].text.to_string(), "c.txt");
    // Into sub/: the parent pane lists the directory it is in.
    keys(&mut rt, "RET");
    let s = rt.snapshot();
    assert!(s.panes[0].text.to_string().contains("sub/"));
    assert_eq!(s.panes[2].text.to_string(), "gamma\n");
    // Opening a file puts the panes back.
    keys(&mut rt, "RET");
    let s = rt.snapshot();
    assert_eq!(s.panes.len(), 1);
    assert_eq!(text(&s), "gamma\n");
}

#[test]
fn the_buffer_list_kills_marked_buffers() {
    let (_dir, _state, root, mut rt) = directory("emacs");
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", root.join("b.txt").display().to_string())).unwrap();
    keys(&mut rt, "C-x C-b");
    let s = rt.snapshot();
    assert!(text(&s).contains("b.txt") && text(&s).contains("a.txt") && text(&s).contains("*Messages*"), "{}", text(&s));
    assert!(caret_line(&s).contains("b.txt"), "at the buffer shown before: {}", caret_line(&s));
    keys(&mut rt, "d x");
    let s = rt.snapshot();
    assert!(!text(&s).contains("b.txt"), "{}", text(&s));
    keys(&mut rt, "/ n");
    type_text(&mut rt, "Mess");
    keys(&mut rt, "M-RET");
    let s = rt.snapshot();
    assert!(!text(&s).contains("a.txt") && text(&s).contains("*Messages*"), "{}", text(&s));
}

#[test]
fn modal_keys_in_a_listing() {
    let (_dir, _state, _root, mut rt) = directory("modal");
    keys(&mut rt, "SPC o -");
    assert_eq!(caret_line(&rt.snapshot()).trim(), "a.txt");
    keys(&mut rt, "k l");
    assert_eq!(names(&rt.snapshot()), ["../", "c.txt"]);
    keys(&mut rt, "h");
    assert_eq!(caret_line(&rt.snapshot()).trim(), "sub/");
}

/// An open file renamed or moved follows: its buffer is named anew, its
/// unsaved edits stay, and saving writes where it is now. A deleted one's
/// buffer goes, unless it has unsaved edits.
#[test]
fn open_files_follow_their_files() {
    let (_dir, _state, root, mut rt) = directory("emacs");
    // a.txt is open, with an edit not saved.
    type_text(&mut rt, "x");
    keys(&mut rt, "C-x C-j R C-a C-k");
    type_text(&mut rt, &format!("{}/sub/", root.display()));
    keys(&mut rt, "RET");
    assert!(root.join("sub/a.txt").exists() && !root.join("a.txt").exists());
    keys(&mut rt, "C-x b");
    type_text(&mut rt, "a.txt");
    keys(&mut rt, "RET");
    let s = rt.snapshot();
    assert_eq!(text(&s), "xalpha\n");
    assert!(s.pane().status.contains("sub/a.txt"), "{}", s.pane().status);
    keys(&mut rt, "C-x C-s");
    assert_eq!(std::fs::read_to_string(root.join("sub/a.txt")).unwrap(), "xalpha\n");
    assert!(!root.join("a.txt").exists(), "nothing written where it was");
    // So does one in a directory moved.
    keys(&mut rt, "C-x C-j ^ M-< n n R C-a C-k");
    type_text(&mut rt, &format!("{}/moved", root.display()));
    keys(&mut rt, "RET");
    keys(&mut rt, "C-x b");
    type_text(&mut rt, "a.txt");
    keys(&mut rt, "RET");
    assert!(rt.snapshot().pane().status.contains("moved/a.txt"), "{}", rt.snapshot().pane().status);
    // A file with unsaved edits is not deleted: they could not be recovered.
    type_text(&mut rt, "y");
    keys(&mut rt, "C-x C-j ^ M-< n n D");
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    assert!(rt.snapshot().echo.contains("Unsaved edits"), "{}", rt.snapshot().echo);
    assert!(root.join("moved/a.txt").exists());
    // b.txt open and saved: deleting it kills its buffer.
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", root.join("b.txt").display().to_string())).unwrap();
    keys(&mut rt, "C-x C-j D");
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    assert!(!root.join("b.txt").exists());
    assert_eq!(rt.eval("(buffer-named \"b.txt\")").unwrap(), "#f");
}

#[test]
fn names_are_swapped_in_place() {
    let (_dir, _state, root, mut rt) = directory("emacs");
    keys(&mut rt, "C-x C-j C-x C-q");
    // a.txt and b.txt, one under the other: swap their first letters.
    keys(&mut rt, "C-d");
    type_text(&mut rt, "b");
    keys(&mut rt, "C-b C-n C-d");
    type_text(&mut rt, "a");
    keys(&mut rt, "C-c C-c");
    assert_eq!(rt.snapshot().echo, "Renamed 2");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "beta\n");
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "alpha\n");
    assert_eq!(names(&rt.snapshot()), ["../", "sub", "a.txt", "b.txt"].map(|n| if n == "sub" { "sub/" } else { n }));
}

#[test]
fn entries_are_marked_by_regexp() {
    let (_dir, _state, _root, mut rt) = directory("emacs");
    keys(&mut rt, "C-x C-j % m");
    type_text(&mut rt, "^[ab]\\.txt$");
    keys(&mut rt, "RET");
    let s = rt.snapshot();
    assert_eq!(s.echo, "Marked 2");
    let marked: Vec<_> = text(&s).lines().filter(|l| l.starts_with('*')).map(|l| l[1..].trim().to_string()).collect();
    assert_eq!(marked, ["a.txt", "b.txt"]);
}

/// The parent's row goes up, and is never marked or acted on.
#[test]
fn the_parent_is_never_acted_on() {
    let (_dir, _state, root, mut rt) = directory("emacs");
    keys(&mut rt, "C-x C-j M-< n d x");
    assert_eq!(rt.snapshot().echo, "No entries flagged");
    keys(&mut rt, "M-< n D");
    assert_eq!(rt.snapshot().echo, "Not on an entry");
    assert!(root.join("a.txt").exists());
}

/// A symbolic link moved or deleted is the link: the file it points to
/// keeps its buffer, where it is.
#[test]
fn a_link_is_not_its_target() {
    let (_dir, _state, root, mut rt) = directory("emacs");
    std::os::unix::fs::symlink("a.txt", root.join("link")).unwrap();
    type_text(&mut rt, "x");
    keys(&mut rt, "C-x C-j g M-> R C-a C-k");
    type_text(&mut rt, &format!("{}/sub/link", root.display()));
    keys(&mut rt, "RET");
    assert!(root.join("sub/link").is_symlink());
    keys(&mut rt, "C-x b");
    type_text(&mut rt, "a.txt");
    keys(&mut rt, "RET");
    let s = rt.snapshot();
    assert!(s.pane().status.starts_with(&format!("{}/a.txt", root.display())), "{}", s.pane().status);
    assert_eq!(text(&s), "xalpha\n");
}

/// Swapping names, with one renamed to the temporary name a swap would
/// take, still renames each to what was typed.
#[test]
fn a_swap_does_not_take_a_name_typed() {
    let (_dir, _state, root, mut rt) = directory("emacs");
    keys(&mut rt, "C-x C-j C-x C-q C-p C-k");
    type_text(&mut rt, ".wdired-0-a.txt");
    keys(&mut rt, "C-n C-e DEL DEL DEL DEL DEL");
    type_text(&mut rt, "b.txt");
    keys(&mut rt, "C-n C-e DEL DEL DEL DEL DEL");
    type_text(&mut rt, "a.txt");
    keys(&mut rt, "C-c C-c");
    assert_eq!(rt.snapshot().echo, "Renamed 3");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "beta\n");
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "alpha\n");
    assert!(root.join(".wdired-0-a.txt/c.txt").exists());
}

/// As find-file in Emacs with vertico: the input naming a directory is
/// selected, and taking it lists the directory.
#[test]
fn finding_a_directory_lists_it() {
    let (_dir, _state, root, mut rt) = directory("emacs");
    keys(&mut rt, "C-x C-f");
    assert!(rt.snapshot().minibuffer.unwrap().input_selected, "the directory itself is selected");
    keys(&mut rt, "RET");
    assert_eq!(names(&rt.snapshot()), ["../", "sub/", "a.txt", "b.txt"]);
    // Typed by hand, then taken: its listing.
    keys(&mut rt, "C-x C-f");
    type_text(&mut rt, "sub/");
    keys(&mut rt, "RET");
    assert_eq!(names(&rt.snapshot()), ["../", "c.txt"]);
    // A name typed selects the first candidate again: a file opens.
    keys(&mut rt, "C-x C-f");
    type_text(&mut rt, "c.t");
    keys(&mut rt, "RET");
    assert_eq!(text(&rt.snapshot()), "gamma\n");
    assert!(root.join("sub/c.txt").exists());
}
