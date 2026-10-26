//! Compiling (`lisp/editor/packages/compile.scm`): a command run in the
//! project's root, its output in *compilation*, and the places it names
//! gone to from there and from anywhere.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use techne_editor::{
    present::{Input, Pane, Snapshot},
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

/// Run background tasks until `done` holds of a snapshot.
fn until(rt: &mut Runtime, what: &str, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let start = Instant::now();
    loop {
        rt.run_tasks(Duration::from_millis(5)).unwrap();
        let s = rt.snapshot();
        if done(&s) {
            return s;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "timed out waiting for {what}: {}", s.echo);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn compilation(s: &Snapshot) -> Option<&Pane> {
    s.panes.iter().find(|p| p.status.contains("*compilation*"))
}

fn finished(s: &Snapshot) -> bool {
    compilation(s).is_some_and(|p| p.text.to_string().contains("\nCompilation exited abnormally with code 2\n"))
}

/// A project whose build names places in src/a.rs, and one in a file
/// that does not exist.
fn project(dir: &Path) {
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.rs"), "one\ntwo two\nthree\n").unwrap();
    std::fs::write(
        dir.join("build.sh"),
        "echo 'warning: unused'\necho '  --> src/a.rs:2:3'\necho 'src/a.rs:3:1: error: no'\necho 'gone.rs:1:1: error: no'\nexit 2\n",
    )
    .unwrap();
}

#[test]
fn compiling_and_going_to_its_errors() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path());
    let a = dir.path().join("src/a.rs");
    let mut rt = Runtime::open(&a, &dir.path().join("a.journal"), "emacs").unwrap().0;
    // The command is read in the project's root, a default to edit.
    keys(&mut rt, "M-x");
    type_text(&mut rt, "compile");
    keys(&mut rt, "M-RET");
    let m = rt.snapshot().minibuffer.expect("the command read");
    let root = rt.eval(&format!("(abbreviate-path \"{}/\")", dir.path().display())).unwrap();
    assert_eq!(m.prompt, format!("Compile in {}: ", root.trim_matches('"')));
    assert_eq!(m.input, "make -k ");
    keys(&mut rt, "C-a C-k");
    type_text(&mut rt, "sh build.sh");
    keys(&mut rt, "RET");
    let s = until(&mut rt, "the compilation", finished);
    assert_eq!(s.pane().text.to_string(), "one\ntwo two\nthree\n", "the focus stays");
    // The places named are highlighted, not the one in a file that does
    // not exist.
    let out = compilation(&s).unwrap();
    let text = out.text.to_string();
    let named: Vec<&str> = out.layers.iter().filter(|h| h.face == "keyword").map(|h| &text[h.from..h.to]).collect();
    assert_eq!(named, ["src/a.rs:2:3", "src/a.rs:3:1"]);
    // M-g n goes to each in turn, in the pane that was focused.
    keys(&mut rt, "M-g n");
    let s = rt.snapshot();
    assert_eq!((s.pane().text.to_string().as_str(), s.pane().head()), ("one\ntwo two\nthree\n", 6), "{} {:?}", s.echo, rt.take_errors());
    let out = compilation(&s).unwrap();
    assert!(out.text.to_string()[out.head()..].starts_with("  --> src/a.rs:2:3"), "its line is the current one");
    keys(&mut rt, "M-g n");
    assert_eq!(rt.snapshot().pane().head(), 12);
    keys(&mut rt, "M-g n");
    assert_eq!(rt.snapshot().echo, "No more errors");
    keys(&mut rt, "M-g p");
    assert_eq!(rt.snapshot().pane().head(), 6);
    // In *compilation*, n moves to the next place named and RET goes to
    // it in the other pane.
    keys(&mut rt, "C-x o M-< n n RET");
    let s = rt.snapshot();
    assert_eq!((s.pane().status.contains("a.rs"), s.pane().head()), (true, 12));
    // The project remembers its command: recompile runs it again, and
    // compile offers it.
    keys(&mut rt, "M-x");
    type_text(&mut rt, "recompile");
    keys(&mut rt, "M-RET");
    assert!(!finished(&rt.snapshot()), "emptied first");
    until(&mut rt, "the compilation run again", finished);
    keys(&mut rt, "M-x");
    type_text(&mut rt, "compile");
    keys(&mut rt, "M-RET");
    assert_eq!(rt.snapshot().minibuffer.unwrap().input, "sh build.sh");
    keys(&mut rt, "C-g");
    assert!(rt.take_errors().is_empty());
}

#[test]
fn the_modal_profile_has_doom_keys() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path());
    let a = dir.path().join("src/a.rs");
    let mut rt = Runtime::open(&a, &dir.path().join("a.journal"), "modal").unwrap().0;
    keys(&mut rt, "SPC c c C-a C-k");
    type_text(&mut rt, "sh build.sh");
    keys(&mut rt, "RET");
    until(&mut rt, "the compilation", finished);
    keys(&mut rt, "] e");
    assert_eq!(rt.snapshot().pane().head(), 6);
    keys(&mut rt, "[ e");
    assert_eq!(rt.snapshot().echo, "No errors before");
    keys(&mut rt, "] e");
    assert_eq!(rt.snapshot().pane().head(), 12);
    keys(&mut rt, "[ e");
    assert_eq!(rt.snapshot().pane().head(), 6);
    assert!(rt.take_errors().is_empty());
}

#[test]
fn a_compilation_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path());
    let mut rt = Runtime::open(&dir.path().join("src/a.rs"), &dir.path().join("a.journal"), "emacs").unwrap().0;
    keys(&mut rt, "C-c c c C-a C-k");
    type_text(&mut rt, "echo started; sleep 30");
    keys(&mut rt, "RET");
    until(&mut rt, "the output", |s| compilation(s).is_some_and(|p| p.text.to_string().contains("\nstarted\n")));
    keys(&mut rt, "M-x");
    type_text(&mut rt, "kill-compilation");
    keys(&mut rt, "M-RET");
    let s = rt.snapshot();
    assert_eq!(s.echo, "Compilation killed");
    assert!(compilation(&s).unwrap().text.to_string().ends_with("\nCompilation killed\n"));
    keys(&mut rt, "M-x");
    type_text(&mut rt, "kill-compilation");
    keys(&mut rt, "M-RET");
    assert_eq!(rt.snapshot().echo, "No compilation is running");
}

/// The current error keeps its line while much more output comes than a
/// document's history keeps, and counts go that many places on.
#[test]
fn the_current_error_through_much_output() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path());
    std::fs::write(
        dir.path().join("slow.sh"),
        "echo 'src/a.rs:1:1: one'\necho 'src/a.rs:2:1: two'\necho 'src/a.rs:3:1: three'\nsleep 0.3\n\
         for i in $(seq 300); do echo \"line $i\"; sleep 0.002; done\nexit 2\n",
    )
    .unwrap();
    let mut rt = Runtime::open(&dir.path().join("src/a.rs"), &dir.path().join("a.journal"), "emacs").unwrap().0;
    keys(&mut rt, "C-c c c C-a C-k");
    type_text(&mut rt, "sh slow.sh");
    keys(&mut rt, "RET");
    until(&mut rt, "the errors", |s| compilation(s).is_some_and(|p| p.text.to_string().contains("three")));
    keys(&mut rt, "M-g n");
    assert_eq!(rt.snapshot().pane().head(), 0);
    until(&mut rt, "the rest", finished);
    keys(&mut rt, "M-g n");
    assert_eq!(rt.snapshot().pane().head(), 4, "the second, not the first again");
    // Two back from the second goes as far as there are, to the first;
    // two on from there to the third.
    keys(&mut rt, "C-u 2 M-g p");
    assert_eq!(rt.snapshot().pane().head(), 0);
    keys(&mut rt, "C-u 2 M-g n");
    assert_eq!(rt.snapshot().pane().head(), 12);
    assert!(rt.take_errors().is_empty());
}
