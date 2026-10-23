//! Searching a project's files in a service, as you type.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use techne_editor::{present::Input, runtime::Runtime};

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

/// A project: a repository's root, a file in it and one below, one a
/// `.gitignore` leaves out and a binary one, each with "beta"; a line
/// longer than a candidate shows.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".jj")).unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(root.join("a.txt"), "alpha\nalpha beta\n").unwrap();
    std::fs::write(root.join("sub/b.scm"), "(define beta 1)\n(define Beta 2)\n").unwrap();
    std::fs::write(root.join("target/c.txt"), "beta\n").unwrap();
    std::fs::write(root.join("d.bin"), b"beta\0").unwrap();
    std::fs::write(root.join("long.txt"), format!("{} needle\n", "x".repeat(400))).unwrap();
    dir
}

/// Run the runtime's tasks until `done`, Lisp, is true.
fn until(rt: &mut Runtime, done: &str) {
    let start = Instant::now();
    while rt.eval(done).unwrap() != "#t" {
        assert!(start.elapsed() < Duration::from_secs(20), "{done}");
        rt.run_tasks(Duration::from_millis(5)).unwrap();
    }
}

fn root(dir: &Path) -> String {
    format!("{}/", dir.canonicalize().unwrap().display())
}

/// The service lists what a repository's ignore files leave in, skips
/// binary files, and searches an open document as it is, unsaved edits
/// and all, saying of which revision.
#[test]
fn the_search_service_reads_a_project() {
    let dir = project();
    let root = root(dir.path());
    let a = format!("{root}a.txt");
    let (mut rt, _) = Runtime::open(Path::new(&a), &dir.path().join("journal"), "emacs").unwrap();
    rt.eval(&format!(
        "(define svc (make-service \"search\" {:?} #:files #t))
         (define d (open-file {a:?}))
         (view-edit! (make-view d \"user\") '((0 0 \"beta first\\n\")) \"new\")
         (define found #f)
         (spawn (lambda () (set! found (service-call svc \"search\" (list {root:?} '(\"beta\") 100 (list {a:?}) #t) (list d)))))",
        techne_editor::runtime::lisp_dir().join("services/search.scm").display().to_string()
    ))
    .unwrap();
    until(&mut rt, "(and found #t)");
    let expected = format!(
        "(list (list {a:?} \"a.txt\" 1 0 \"beta first\" (document-id d) (document-revision d))
               (list {a:?} \"a.txt\" 3 17 \"alpha beta\" (document-id d) (document-revision d))
               (list \"{root}sub/b.scm\" \"sub/b.scm\" 1 0 \"(define beta 1)\" #f #f)
               (list \"{root}sub/b.scm\" \"sub/b.scm\" 2 16 \"(define Beta 2)\" #f #f))"
    );
    assert_eq!(rt.eval(&format!("(equal? found {expected})")).unwrap(), "#t", "{}", rt.eval("found").unwrap());
}

/// `search-project` asks the service as you type, from three
/// characters; the first line found is previewed when it comes, and
/// each candidate goes to its line.
#[test]
fn searching_a_project_as_you_type() {
    let dir = project();
    let root = root(dir.path());
    let (mut rt, _) = Runtime::open(&dir.path().join("a.txt"), &dir.path().join("journal"), "emacs").unwrap();
    keys(&mut rt, "C-c s p");
    assert_eq!(rt.snapshot().minibuffer.unwrap().prompt, format!("*/0 Search {root}: "));
    keys(&mut rt, "B e");
    rt.run_tasks(Duration::from_millis(50)).unwrap();
    assert!(rt.snapshot().minibuffer.unwrap().rows.is_empty(), "two characters ask nothing");
    keys(&mut rt, "t a");
    let rows = candidates(&mut rt);
    // An upper-case letter makes the search match case.
    let found: Vec<(String, String)> = rows.iter().map(|r| (r.text(0), r.text(1))).collect();
    assert_eq!(found, [("(define Beta 2)".to_string(), "sub/b.scm:2".to_string())]);
    assert_eq!(rt.snapshot().pane().text.to_string(), "(define beta 1)\n(define Beta 2)\n", "previewed");
    keys(&mut rt, "RET");
    assert_eq!(rt.snapshot().pane().text.to_string(), "(define beta 1)\n(define Beta 2)\n");
}

/// The candidates the minibuffer shows, once the search answers.
fn candidates(rt: &mut Runtime) -> std::sync::Arc<[techne_editor::present::Row]> {
    let start = Instant::now();
    loop {
        let m = rt.snapshot().minibuffer.unwrap();
        if !m.rows.is_empty() {
            return m.rows;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "no candidates");
        rt.run_tasks(Duration::from_millis(5)).unwrap();
    }
}

/// A line found is a candidate though it is shown cut short, past what
/// the input matched.
#[test]
fn a_long_line_found_is_shown() {
    let dir = project();
    let (mut rt, _) = Runtime::open(&dir.path().join("a.txt"), &dir.path().join("journal"), "emacs").unwrap();
    keys(&mut rt, "C-c s p n e e d l e");
    let rows = candidates(&mut rt);
    assert_eq!(rows.iter().map(|r| r.text(1)).collect::<Vec<_>>(), ["long.txt:1"]);
}

/// The search a minibuffer asks for is owned by the scope that opened
/// it, not by whatever asked for the candidates: shutting that scope
/// down cancels it.
#[test]
fn a_search_belongs_to_the_scope_reading() {
    let mut rt = Runtime::with_document(techne_text::Document::new(""), "emacs").unwrap();
    // A key makes its session the current one.
    keys(&mut rt, "C-g");
    rt.eval(
        "(define asked #f)
         ;; Made outside the scope: its shutting down does not close it.
         (define never (make-channel))
         (define reader (make-scope 'reader))
         (with-scope reader
           (completing-read (current-session) \"Wait: \"
                            (async-source (lambda (input) (set! asked #t) (channel-recv never)))))",
    )
    .unwrap();
    keys(&mut rt, "a b c");
    // Showing the minibuffer asks for its candidates.
    rt.snapshot();
    until(&mut rt, "asked");
    let tasks = rt.eval("(%live-task-count)").unwrap();
    rt.eval("(scope-shutdown! reader)").unwrap();
    rt.run_tasks(Duration::from_millis(20)).unwrap();
    assert!(rt.eval("(%live-task-count)").unwrap().parse::<i64>().unwrap() < tasks.parse::<i64>().unwrap());
}
