//! Remote access through the editor's own Lisp (doc/remote.md): files of
//! another machine opened, saved and read again, its project and its
//! processes, over a node reached through a pipe, as ssh would reach one,
//! and through a hop that node opens. The machine is this one.

use std::time::{Duration, Instant};

use techne_editor::{
    present::{Input, Snapshot},
    runtime::Runtime,
};

/// One state directory, for the journals, for every test of this
/// process: the environment is the process's, and tests run together.
fn state() {
    static STATE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    STATE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: once, before any runtime of this process reads it.
        unsafe { std::env::set_var("XDG_STATE_HOME", dir.path()) };
        dir
    });
}

fn runtime() -> Runtime {
    state();
    let mut rt = Runtime::new().unwrap();
    let node = env!("CARGO_BIN_EXE_techne");
    // A method running the node here, as ssh would run it there, and one
    // a node runs on its own machine, as sudo is.
    rt.eval(&format!(
        r#"(begin
             (define-method 'pipe "Here." #:jump #t #:command (lambda (h jumps program) program))
             (define-method 'again "On the same machine." #:command (lambda (h jumps program) program))
             (define-method 'named "Here, if its host is here."
               #:command (lambda (h jumps program)
                           (if (equal? (hop-host h) "here") program (error "no such host" (hop-host h)))))
             (set-option! 'remote-node-command (list {node:?} "node")))"#
    ))
    .unwrap();
    rt
}

fn eval(rt: &mut Runtime, src: &str) -> String {
    rt.eval(src).unwrap_or_else(|e| panic!("{src}\n{e}"))
}

fn eval_err(rt: &mut Runtime, src: &str) -> String {
    rt.eval(src).map_or_else(|e| e.to_string(), |v| panic!("{src} returned {v}"))
}

#[test]
fn a_remote_file_is_opened_saved_and_read_again() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "one\n").unwrap();
    for route in ["/pipe::", "/pipe:|again::"] {
        let mut rt = runtime();
        let loc = format!("{route}{}", file.display());
        eval(&mut rt, &format!("(define d (file-document {loc:?}))"));
        assert_eq!(eval(&mut rt, "(document-path d)"), format!("{loc:?}"));
        assert_eq!(eval(&mut rt, "(document-substring d 0 (document-length d))"), "\"one\\n\"");
        // The same file is the same document.
        assert_eq!(eval(&mut rt, &format!("(eq? d (file-document {loc:?}))")), "#t");
        eval(&mut rt, r#"(view-edit! (make-view d "me") (list (list 0 0 "zero\n")) "new")"#);
        assert_eq!(eval(&mut rt, "(document-dirty? d)"), "#t");
        eval(&mut rt, "(save-document! d)");
        assert_eq!(eval(&mut rt, "(document-dirty? d)"), "#f");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "zero\none\n");
        // Changed on the other machine: a save is refused, a reload reads it.
        std::fs::write(&file, "elsewhere\n").unwrap();
        eval(&mut rt, r#"(view-edit! (make-view d "me") (list (list 0 0 "x")) "new")"#);
        assert!(eval_err(&mut rt, "(save-document! d)").contains("changed on disk"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "elsewhere\n");
        eval(&mut rt, "(save-document! d #:overwrite #t)");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "xzero\none\n");
        std::fs::write(&file, "one\n").unwrap();
        assert_eq!(eval(&mut rt, "(reload-document! d)"), "#t");
        assert_eq!(eval(&mut rt, "(document-substring d 0 (document-length d))"), "\"one\\n\"");
    }
}

#[test]
fn a_remote_project_has_its_root_its_directories_and_processes_there() {
    let mut rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("p/.jj")).unwrap();
    std::fs::create_dir_all(dir.path().join("p/src")).unwrap();
    std::fs::write(dir.path().join("p/src/main.rs"), "").unwrap();
    let p = format!("/pipe::{}/p/", dir.path().display());
    assert_eq!(eval(&mut rt, &format!("(project-root \"{p}src/\")")), format!("{p:?}"));
    assert_eq!(eval(&mut rt, &format!("(directory-entries \"{p}\")")), r#"(".jj/" "src/")"#);
    assert_eq!(eval(&mut rt, &format!("(path-exists? \"{p}src/main.rs\")")), "#t");
    assert_eq!(eval(&mut rt, &format!("(path-exists? \"{p}none\")")), "#f");
    assert_eq!(eval(&mut rt, &format!("(normal-directory \"{p}src/../src/./\")")), format!("\"{p}src/\""));
    let out = eval(&mut rt, &format!(r#"(call-with-process-at "{p}src/" "pwd" '() (lambda (proc) (process-read-all proc 'stdout)))"#));
    assert_eq!(out, format!("\"{}/p/src\\n\"", dir.path().display()));
    // ~ is the home directory of the other machine, which is this one's.
    let home = std::env::var("HOME").unwrap();
    assert_eq!(eval(&mut rt, r#"(absolute-path "/pipe::~/x")"#), format!("\"/pipe::{home}/x\""));
    assert_eq!(eval(&mut rt, r#"(absolute-path "/pipe::")"#), format!("\"/pipe::{home}/\""));
}

#[test]
fn locations_are_written_as_tramp_writes_them() {
    let mut rt = runtime();
    assert_eq!(eval(&mut rt, r#"(length (location-route "/ssh:a@b#22|sudo:root@b:/etc/x"))"#), "2");
    assert_eq!(eval(&mut rt, r#"(location-path "/ssh:a@b#22|sudo:root@b:/etc/x")"#), "\"/etc/x\"");
    assert_eq!(eval(&mut rt, r#"(location-prefix "/ssh:a@b#22|sudo:root@b:/etc/x")"#), "\"/ssh:a@b#22|sudo:root@b:\"");
    assert_eq!(eval(&mut rt, r#"(location-at "/ssh:b:/etc/x" "/home")"#), "\"/ssh:b:/home\"");
    assert_eq!(eval(&mut rt, r#"(remote-path? "/home/a:b/c")"#), "#f");
    assert_eq!(eval(&mut rt, r#"(remote-path? "/nomethod:b:/c")"#), "#f");
    assert_eq!(eval(&mut rt, r#"(remote-path? "/sudo::/etc")"#), "#t");
    assert_eq!(eval(&mut rt, r#"(location-path "/home/x")"#), "\"/home/x\"");
    // A host or user that a command would read as an option is no location.
    assert_eq!(eval(&mut rt, r#"(remote-path? "/ssh:-oProxyCommand=touch:/x")"#), "#f");
    assert_eq!(eval(&mut rt, r#"(remote-path? "/ssh:-l@b:/x")"#), "#f");
    assert_eq!(eval(&mut rt, r#"(remote-path? "/ssh:a|podman:-v:/x")"#), "#f");
}

/// Saves of one document are one at a time, and a reload that a save
/// overtook, or an edit, is refused rather than putting back older text.
#[test]
fn saves_and_reloads_do_not_overtake_each_other() {
    let mut rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "one\n").unwrap();
    let loc = format!("/pipe::{}", file.display());
    eval(&mut rt, &format!("(define d (file-document {loc:?}))"));
    eval(&mut rt, r#"(view-edit! (make-view d "me") (list (list 0 0 "zero\n")) "new")"#);
    let outcome = r#"(lambda (thunk) (spawn (lambda () (guard (e (#t (error-text e))) (thunk) 'done))))"#;
    let both = eval(
        &mut rt,
        &format!(
            "(let* ((run {outcome}) (a (run (lambda () (save-document! d)))) (b (run (lambda () (save-document! d))))) (list (task-join a) (task-join b)))"
        ),
    );
    assert_eq!(both, r#"(done "a save of this document is under way")"#);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "zero\none\n");
    eval(&mut rt, r#"(view-edit! (make-view d "me") (list (list 0 0 "x")) "new")"#);
    let both = eval(
        &mut rt,
        &format!(
            "(let* ((run {outcome}) (a (run (lambda () (save-document! d)))) (b (run (lambda () (reload-document! d))))) (list (task-join a) (task-join b)))"
        ),
    );
    assert_eq!(both, r#"(done "the document changed while its file was read; read it again")"#);
    assert_eq!(eval(&mut rt, "(document-substring d 0 (document-length d))"), "\"xzero\\none\\n\"");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "xzero\none\n");
}

/// A connection lost, or never made, is made again when next needed;
/// one through another is too, when the other was lost.
#[test]
fn lost_endpoints_are_connected_again() {
    let mut rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().display().to_string();
    let first = format!("/pipe::{d}/");
    let second = format!("/pipe:|again::{d}/");
    assert_eq!(eval(&mut rt, &format!("(path-exists? {second:?})")), "#t");
    eval(&mut rt, &format!("(node-close (location-endpoint {first:?}))"));
    assert_eq!(eval(&mut rt, &format!("(path-exists? {second:?})")), "#t");
    assert_eq!(eval(&mut rt, &format!("(path-exists? {first:?})")), "#t");
    // A node that does not start is not kept.
    let node = env!("CARGO_BIN_EXE_techne");
    eval(&mut rt, "(set-option! 'remote-node-command '(\"false\"))");
    let other = format!("/named:here:{d}/");
    assert!(eval_err(&mut rt, &format!("(path-exists? {other:?})")).contains("node"));
    eval(&mut rt, &format!("(set-option! 'remote-node-command (list {node:?} \"node\"))"));
    assert_eq!(eval(&mut rt, &format!("(path-exists? {other:?})")), "#t");
    // A method that cannot jump starts the first hop here, and sees its hop.
    assert!(eval_err(&mut rt, &format!(r#"(path-exists? "/named:there:{d}/")"#)).contains("no such host"));
}

/// A name no other run of these tests uses, made with `prefix`.
fn unique(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

/// Against a real machine, when `TECHNE_REMOTE_TEST` names a route to a
/// directory there that can be written in (`/ssh:host:/tmp/`), with its
/// node deployed (`remote-node-command`); else skipped.
#[test]
fn a_file_on_another_machine() {
    let Ok(dir) = std::env::var("TECHNE_REMOTE_TEST") else { return };
    state();
    let mut rt = Runtime::new().unwrap();
    let name = unique("techne-remote-test");
    let loc = format!("{dir}{name}");
    eval(&mut rt, &format!(r#"(call-with-process-at {dir:?} "sh" (list "-c" "printf 'one\n' > {name}") process-wait)"#));
    eval(&mut rt, &format!("(define d (file-document {loc:?}))"));
    assert_eq!(eval(&mut rt, "(document-substring d 0 (document-length d))"), "\"one\\n\"");
    eval(&mut rt, r#"(view-edit! (make-view d "me") (list (list 0 0 "zero\n")) "new")"#);
    eval(&mut rt, "(save-document! d)");
    let read = format!(r#"(call-with-process-at {dir:?} "cat" '({name:?}) (lambda (p) (process-read-all p 'stdout)))"#);
    assert_eq!(eval(&mut rt, &read), "\"zero\\none\\n\"");
    eval(&mut rt, &format!(r#"(call-with-process-at {dir:?} "rm" '({name:?}) process-wait)"#));
}

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
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out waiting for {what}: {:?} / {} / {:?}",
            s.pane().text.to_string(),
            s.echo,
            s.minibuffer
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The commands, by their keys, on a repository of another machine, as
/// `a_file_on_another_machine` says.
#[test]
fn commands_on_another_machine() {
    let Ok(dir) = std::env::var("TECHNE_REMOTE_TEST") else { return };
    state();
    let state = tempfile::tempdir().unwrap();
    let scratch = state.path().join("scratch");
    let mut rt = Runtime::open(&scratch, &state.path().join("scratch.journal"), "emacs").unwrap().0;
    rt.eval("(set-option! 'vc-refresh-interval #f)").unwrap();
    let name = unique("techne-remote-repo");
    let repo = format!("{dir}{name}/");
    let setup = format!(
        "mkdir {name} && cd {name} && git init -q && printf 'one\\n' > a.txt \
         && git add a.txt && git -c user.name=t -c user.email=t@example.com -c commit.gpgsign=false commit -q -m start"
    );
    eval(&mut rt, &format!(r#"(call-with-process-at {dir:?} "sh" (list "-c" {setup:?}) process-wait)"#));
    // C-x C-f the file there, completing its directory's entries.
    keys(&mut rt, "C-x C-f M-DEL M-DEL M-DEL M-DEL M-DEL M-DEL M-DEL M-DEL");
    type_text(&mut rt, &repo);
    let s = until(&mut rt, "the directory's entries", |s| format!("{:?}", s.minibuffer).contains("a.txt"));
    drop(s);
    type_text(&mut rt, "a.txt");
    keys(&mut rt, "RET");
    let s = until(&mut rt, "the file", |s| s.pane().text == "one\n");
    assert!(s.pane().status.contains("a.txt"), "{}", s.pane().status);
    keys(&mut rt, "M->");
    type_text(&mut rt, "two");
    keys(&mut rt, "RET C-x C-s");
    until(&mut rt, "the save", |s| s.echo.starts_with("Wrote"));
    // M-! runs there, in the file's directory.
    keys(&mut rt, "M-!");
    type_text(&mut rt, "cat a.txt; pwd");
    keys(&mut rt, "RET");
    let path = repo.rsplit_once(':').unwrap().1.trim_end_matches('/').to_string();
    until(&mut rt, "the command", |s| s.echo.starts_with("Shell command finished"));
    let output = eval(
        &mut rt,
        r#"(let ((d (buffer-document (buffer-named "*Shell Command Output*")))) (document-substring d 0 (document-length d)))"#,
    );
    assert_eq!(output, format!("{:?}", format!("one\ntwo\n{path}\n")));
    // The repository's status, from its tool there.
    keys(&mut rt, "C-x v d");
    let s = until(&mut rt, "the status", |s| s.pane().text.to_string().contains("a.txt"));
    assert!(
        s.pane().text.to_string().starts_with(&format!("git {repo}").trim_end_matches('/').to_string()),
        "{}",
        s.pane().text.to_string()
    );
    eval(&mut rt, &format!(r#"(call-with-process-at {dir:?} "rm" '("-rf" {name:?}) process-wait)"#));
}
