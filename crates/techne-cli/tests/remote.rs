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
    with_methods(Runtime::new().unwrap())
}

/// RT, with the methods of the tests and the node they run.
fn with_methods(mut rt: Runtime) -> Runtime {
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

#[cfg(unix)]
#[test]
fn a_lost_commit_reply_blocks_writes_without_replaying() {
    use std::{os::unix::fs::PermissionsExt, process::Command};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let git = |args: &[&str]| {
        let out = Command::new("git").current_dir(root).args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    git(&["init", "-q"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "user.email", "test@example.test"]);
    git(&["config", "commit.gpgSign", "false"]);
    git(&["config", "core.hooksPath", ".git/hooks"]);
    let file = root.join("a.txt");
    std::fs::write(&file, "first\n").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "initial"]);
    std::fs::write(&file, "second\n").unwrap();
    git(&["add", "a.txt"]);
    let hook = root.join(".git/hooks/post-commit");
    let marker = root.join(".git/commit-recorded");
    std::fs::write(&hook, "#!/bin/sh\n: > .git/commit-recorded\nsleep 30\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();

    let mut rt = runtime();
    rt.attach(techne_editor::runtime::Client(0), None, "emacs").unwrap();
    // Opt-in OpenSSH loopback: the server must see this machine's temporary
    // directory. Port-only input cannot direct this test to another host.
    let ssh_port = std::env::var("TECHNE_VC_SSH_LOOPBACK_PORT").ok().map(|s| s.parse::<u16>().unwrap());
    let route = ssh_port.map_or_else(|| "/pipe::".to_owned(), |p| format!("/ssh:127.0.0.1#{p}:"));
    let loc = format!("{route}{}", file.display());
    let open = Instant::now();
    eval(&mut rt, &format!("(show-document! (current-session) (file-document {loc:?}))"));
    let open_ms = open.elapsed().as_millis();
    let status = Instant::now();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "staged remote file", |s| s.pane().text.to_string().contains("Staged"));
    if ssh_port.is_some() {
        let status_ms = status.elapsed().as_millis();
        let scratch = root.join("refresh-probe.txt");
        let mut warm = Vec::new();
        for i in 0..10 {
            let exists = i % 2 == 0;
            if exists {
                std::fs::write(&scratch, "probe\n").unwrap();
            } else {
                std::fs::remove_file(&scratch).unwrap();
            }
            let start = Instant::now();
            keys(&mut rt, "g");
            until(&mut rt, "new status rows over SSH", |s| s.pane().text.to_string().contains("refresh-probe.txt") == exists);
            warm.push(start.elapsed().as_millis());
        }
        warm.sort_unstable();
        println!("Loopback SSH: connect+open {open_ms} ms; initial status {status_ms} ms; warm refresh samples {warm:?} ms");
    }
    keys(&mut rt, "c c");
    until(&mut rt, "remote commit draft", |s| s.pane().status.contains("vc-commit"));
    type_text(&mut rt, "from Techne");
    keys(&mut rt, "C-c C-c");
    until(&mut rt, "post-commit hook running", |_| marker.is_file());
    {
        let mut other = runtime();
        other.attach(techne_editor::runtime::Client(0), None, "emacs").unwrap();
        let other_loc = format!("{route}{}", root.join("other.txt").display());
        std::fs::write(root.join("other.txt"), "another buffer in the same repository\n").unwrap();
        eval(&mut other, &format!("(show-document! (current-session) (file-document {other_loc:?}))"));
        keys(&mut other, "C-x v d");
        until(&mut other, "other runtime sees pending write", |s| s.pane().text.to_string().contains("outcome unknown"));
        keys(&mut other, "C-x v !");
        until(&mut other, "other runtime inspected evidence", |s| {
            s.pane().text.to_string().contains("Write outcome unknown · writes blocked")
        });
        keys(&mut other, "a");
        type_text(&mut other, "yes");
        keys(&mut other, "RET");
        until(&mut other, "live attempt cannot be acknowledged by another runtime", |s| s.echo.contains("holds this database"));
    }
    eval(&mut rt, &format!("(node-close (location-endpoint {loc:?}))"));
    keys(&mut rt, "C-c C-k");
    until(&mut rt, "unknown write outcome", |s| s.pane().text.to_string().contains("outcome unknown"));
    eval(&mut rt, "(drop-buffer! (current-session) (session-buffer (current-session)))");
    std::fs::write(&file, "third\n").unwrap();
    eval(&mut rt, &format!("(show-document! (current-session) (file-document {loc:?}))"));
    keys(&mut rt, "C-x v d");
    until(&mut rt, "reopened with unknown outcome", |s| {
        s.pane().text.to_string().contains("outcome unknown") && s.pane().text.to_string().contains("Unstaged")
    });
    keys(&mut rt, "n n s");
    until(&mut rt, "second write refused", |s| s.echo.contains("remote write is unresolved"));
    assert_eq!(git(&["show", ":a.txt"]), "second\n");
    drop(rt);
    let mut rt = runtime();
    rt.attach(techne_editor::runtime::Client(0), None, "emacs").unwrap();
    eval(&mut rt, &format!("(show-document! (current-session) (file-document {loc:?}))"));
    keys(&mut rt, "C-x v d");
    until(&mut rt, "restarted with unknown outcome", |s| {
        s.pane().text.to_string().contains("outcome unknown") && s.pane().text.to_string().contains("Unstaged")
    });
    keys(&mut rt, "n n s");
    until(&mut rt, "write refused after restart", |s| s.echo.contains("remote write is unresolved"));
    assert_eq!(git(&["show", ":a.txt"]), "second\n");
    let evidence_start = Instant::now();
    keys(&mut rt, "C-x v !");
    let inspected =
        until(&mut rt, "fresh evidence on reconnect", |s| s.pane().text.to_string().contains("Write outcome unknown · writes blocked"));
    if ssh_port.is_some() {
        println!("Loopback SSH: evidence on reconnected route {} ms", evidence_start.elapsed().as_millis());
    }
    assert!(inspected.minibuffer.is_none());
    let evidence = inspected.pane().text.to_string();
    assert!(evidence.contains("Before dispatch") && evidence.contains("Observed after reconnect"));
    assert!(evidence.contains("HEAD commit changed · index entries unchanged"), "{evidence}");
    assert_eq!(evidence.matches("index entries SHA-256").count(), 2);
    let observed = git(&["rev-parse", "HEAD"]);
    keys(&mut rt, "j");
    until(&mut rt, "exact observed commit", |s| {
        s.panes.iter().any(|p| {
            let patch = p.text.to_string();
            patch.contains(observed.trim()) && patch.contains("+second")
        })
    });
    keys(&mut rt, "a");
    assert_eq!(rt.snapshot().minibuffer.unwrap().rows[0].text(0), "no");
    keys(&mut rt, "RET a");
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "acknowledged without replay", |s| s.echo.contains("no write replayed"));
    assert_eq!(git(&["log", "-2", "--format=%s"]), "from Techne\ninitial\n");
    keys(&mut rt, "C-x v !");
    let s = rt.snapshot();
    let evidence = s.pane().text.to_string();
    assert!(evidence.contains("Attempt: Commit staged Git changes"), "{evidence}");
    assert_eq!(evidence.matches("index entries SHA-256").count(), 2);
    assert!(evidence.contains("refs/heads/"), "{evidence}");
}

#[cfg(unix)]
#[test]
fn a_lost_push_reply_shows_the_requested_destination_tip() {
    use std::{os::unix::fs::PermissionsExt, process::Command};

    let checkout = tempfile::tempdir().unwrap();
    let upstream = tempfile::tempdir().unwrap();
    let fetch_only = tempfile::tempdir().unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    let root = checkout.path();
    let bare = upstream.path();
    git(bare, &["init", "--bare", "-q"]);
    git(fetch_only.path(), &["init", "--bare", "-q"]);
    git(bare, &["config", "core.hooksPath", "hooks"]);
    git(root, &["init", "-qb", "main"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "user.email", "test@example.test"]);
    git(root, &["config", "commit.gpgSign", "false"]);
    git(root, &["remote", "add", "origin", fetch_only.path().to_str().unwrap()]);
    let file = root.join("a.txt");
    std::fs::write(&file, "first\n").unwrap();
    git(root, &["add", "a.txt"]);
    git(root, &["commit", "-qm", "initial"]);
    git(root, &["push", "-qu", "origin", "main"]);
    git(root, &["remote", "set-url", "--push", "origin", bare.to_str().unwrap()]);
    git(root, &["push", "-q", "origin", "main"]);
    let before = git(bare, &["rev-parse", "refs/heads/main"]);
    std::fs::write(&file, "second\n").unwrap();
    git(root, &["commit", "-qam", "publish this"]);
    let wanted = git(root, &["rev-parse", "HEAD"]);
    let hook = bare.join("hooks/post-receive");
    let marker = bare.join("push-recorded");
    std::fs::write(&hook, "#!/bin/sh\n: > push-recorded\nsleep 30\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();

    let mut rt = runtime();
    rt.attach(techne_editor::runtime::Client(0), None, "emacs").unwrap();
    let loc = format!("/pipe::{}", file.display());
    eval(&mut rt, &format!("(show-document! (current-session) (file-document {loc:?}))"));
    keys(&mut rt, "C-x v d");
    until(&mut rt, "remote status", |s| s.pane().text.to_string().contains("ahead 1"));
    keys(&mut rt, "P p");
    until(&mut rt, "push confirmation", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("origin:refs/heads/main")));
    git(root, &["remote", "set-url", "--push", "origin", fetch_only.path().to_str().unwrap()]);
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "changed destination refused", |s| s.echo.contains("push destination changed"));
    assert_eq!(git(fetch_only.path(), &["rev-parse", "refs/heads/main"]), before);
    assert!(!marker.is_file());
    git(root, &["remote", "set-url", "--push", "origin", bare.to_str().unwrap()]);
    keys(&mut rt, "P p");
    until(&mut rt, "new push confirmation", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("origin:refs/heads/main")));
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "remote ref updated", |_| marker.is_file());
    eval(&mut rt, &format!("(node-close (location-endpoint {loc:?}))"));
    until(&mut rt, "unknown push outcome", |s| s.pane().text.to_string().contains("outcome unknown"));
    keys(&mut rt, "C-x v !");
    let inspected =
        until(&mut rt, "inspect push evidence", |s| s.pane().text.to_string().contains("Write outcome unknown · writes blocked"));
    assert!(inspected.pane().text.to_string().contains("matches requested commit"));
    keys(&mut rt, "a");
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "acknowledge without replay", |s| s.echo.contains("no write replayed"));
    keys(&mut rt, "C-x v !");
    let evidence = rt.snapshot().pane().text.to_string();
    assert!(evidence.contains("origin:refs/heads/main"), "{evidence}");
    assert!(evidence.contains(&format!("observed tip {}", before.trim())), "{evidence}");
    assert!(evidence.contains(&format!("observed tip {} (matches requested commit)", wanted.trim())), "{evidence}");
    assert_eq!(git(bare, &["rev-parse", "refs/heads/main"]), wanted);
    assert_eq!(git(fetch_only.path(), &["rev-parse", "refs/heads/main"]), before);
}

#[cfg(unix)]
#[test]
fn jj_publish_recovery_observes_the_bookmark_after_a_lost_node_reply() {
    use std::{os::unix::fs::PermissionsExt, path::Path, process::Command};

    if !Command::new("jj").arg("version").output().is_ok_and(|out| out.status.success()) {
        eprintln!("skipping jj fixture: jj is not available");
        return;
    }
    let checkout = tempfile::tempdir().unwrap();
    let upstream = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let run = |command: &str, dir: &Path, args: &[&str]| {
        let out = Command::new(command).current_dir(dir).args(args).output().unwrap();
        assert!(out.status.success(), "{command} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    let root = checkout.path();
    let bare = upstream.path();
    run("git", bare, &["init", "--bare", "-q"]);
    run("git", other.path(), &["init", "--bare", "-q"]);
    run("git", bare, &["config", "core.hooksPath", "hooks"]);
    let jj = |args: &[&str]| run("jj", root, args);
    jj(&["git", "init", "--no-colocate"]);
    jj(&["config", "set", "--repo", "user.name", "Test"]);
    jj(&["config", "set", "--repo", "user.email", "test@example.test"]);
    jj(&["git", "remote", "add", "origin", bare.to_str().unwrap()]);
    let file = root.join("a.txt");
    std::fs::write(&file, "published through jj\n").unwrap();
    jj(&["describe", "-m", "Recorded topic"]);
    jj(&["bookmark", "set", "topic", "-r", "@"]);
    let tip = jj(&["--ignore-working-copy", "log", "--no-graph", "-r", "topic", "-T", "commit_id"]);
    let hook = bare.join("hooks/post-receive");
    let marker = bare.join("push-recorded");
    std::fs::write(&hook, "#!/bin/sh\n: > push-recorded\nsleep 30\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();

    let mut rt = runtime();
    rt.attach(techne_editor::runtime::Client(0), None, "emacs").unwrap();
    let loc = format!("/pipe::{}", file.display());
    eval(&mut rt, &format!("(show-document! (current-session) (file-document {loc:?}))"));
    keys(&mut rt, "C-x v d");
    until(&mut rt, "jj graph", |s| s.pane().text.to_string().contains("Recorded topic"));
    for change_endpoint in [true, false] {
        keys(&mut rt, "M-< n P p");
        until(&mut rt, "bookmark picker", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Publish bookmark")));
        keys(&mut rt, "RET");
        until(&mut rt, "remote picker", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Push to")));
        keys(&mut rt, "RET");
        until(&mut rt, "push confirmation", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Push topic")));
        if change_endpoint {
            jj(&["git", "remote", "set-url", "origin", other.path().to_str().unwrap()]);
        }
        type_text(&mut rt, "yes");
        keys(&mut rt, "RET");
        if change_endpoint {
            until(&mut rt, "changed endpoint refused", |s| s.echo.contains("changed"));
            assert!(run("git", other.path(), &["for-each-ref"]).is_empty());
            assert!(!marker.is_file());
            jj(&["git", "remote", "set-url", "origin", bare.to_str().unwrap()]);
            keys(&mut rt, "g");
        }
    }
    until(&mut rt, "bookmark reached destination", |_| marker.is_file());
    eval(&mut rt, &format!("(node-close (location-endpoint {loc:?}))"));
    until(&mut rt, "publish outcome unknown", |s| s.pane().text.to_string().contains("outcome unknown"));
    keys(&mut rt, "C-x v !");
    let inspected =
        until(&mut rt, "review after reconnect", |s| s.pane().text.to_string().contains("Write outcome unknown · writes blocked"));
    assert!(inspected.pane().text.to_string().contains("matches requested commit"));
    keys(&mut rt, "a");
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "acknowledged without replay", |s| s.echo.contains("no write replayed"));
    keys(&mut rt, "C-x v !");
    let evidence = rt.snapshot().pane().text.to_string();
    assert!(evidence.contains("origin:refs/heads/topic"), "{evidence}");
    assert!(evidence.contains("observed tip missing"), "{evidence}");
    assert!(evidence.contains(&format!("observed tip {tip} (matches requested commit)")), "{evidence}");
    assert_eq!(run("git", bare, &["rev-parse", "refs/heads/topic"]).trim(), tip);
}

/// A save refused on the other machine offers choices that act there:
/// comparing reads the file there, reverting takes its text.
#[test]
fn a_remote_save_refused_offers_to_compare_and_revert_there() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "one\n").unwrap();
    // A session, the choices' commands run in.
    state();
    let mut rt = with_methods(Runtime::open(&dir.path().join("scratch"), &dir.path().join("scratch.journal"), "emacs").unwrap().0);
    let loc = format!("/pipe::{}", file.display());
    eval(&mut rt, &format!("(define d (file-document {loc:?}))"));
    eval(&mut rt, r#"(view-edit! (make-view d "user") (list (list 0 0 "x")) "new")"#);
    std::fs::write(&file, "elsewhere\n").unwrap();
    eval(&mut rt, "(define cs (guard (e (#t (condition-choices e))) (save-document! d) '()))");
    assert_eq!(eval(&mut rt, "(map choice-label cs)"), "(\"Overwrite\" \"Revert\" \"Compare\")");
    let choose = |rt: &mut Runtime, label: &str| {
        eval(
            rt,
            &format!(
                "(invoke (current-session) (choice-command (find (lambda (c) (string=? (choice-label c) {label:?})) cs)) (choice-arguments (find (lambda (c) (string=? (choice-label c) {label:?})) cs)))"
            ),
        )
    };
    choose(&mut rt, "Compare");
    until(&mut rt, "remote diff", |s| s.panes.iter().any(|p| p.text.to_string().contains("-elsewhere\n+xone\n")));
    let diff = eval(&mut rt, "(document-string (buffer-document (buffer-named \"*diff*\")))");
    assert!(diff.contains("-elsewhere\\n+xone\\n"), "{diff}");
    choose(&mut rt, "Revert");
    let start = Instant::now();
    while eval(&mut rt, "(document-dirty? d)") != "#f" {
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for remote revert");
        rt.run_tasks(Duration::from_millis(10)).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(eval(&mut rt, "(list (document-string d) (document-dirty? d))"), "(\"elsewhere\\n\" #f)");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "elsewhere\n");
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

/// Listing a remote buffer never reconnects its lost endpoint.
#[test]
fn ibuffer_redisplay_does_not_connect_to_remote_hosts() {
    let mut rt = runtime();
    rt.attach(techne_editor::runtime::Client(0), None, "emacs").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("remote.txt");
    std::fs::write(&file, "remote text").unwrap();
    let loc = format!("/pipe::{}", file.display());
    eval(&mut rt, &format!("(show-document! (current-session) (file-document {loc:?}))"));
    eval(&mut rt, &format!("(node-close (location-endpoint {loc:?}))"));
    eval(
        &mut rt,
        r#"(define attempts 0)
           (define-method 'pipe "Unavailable."
             #:command (lambda (h jumps program)
                         (set! attempts (+ attempts 1))
                         (error "host is unavailable")))
           (run-command (current-session) 'ibuffer 1)"#,
    );
    for _ in 0..2 {
        let snapshot = rt.snapshot();
        assert!(snapshot.pane().text.to_string().contains("remote.txt"));
        assert_eq!(eval(&mut rt, "attempts"), "0");
    }
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
    // Refuse before reading while the save is still owned, rather than
    // fetching bytes that would have to be rejected afterward.
    assert_eq!(both, r#"(done "a save of this document is under way")"#);
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

/// Endpoint caching keys by structured hop tuples, so routes sharing a
/// rendered display string (user+host vs host containing `@`) never collide
/// while equivalent routes still reuse the same connection.
#[test]
fn distinct_hops_with_same_display_string_do_not_collide() {
    let mut rt = runtime();
    eval(
        &mut rt,
        "(import (only (techne location) make-hop route->string) (only (techne endpoint) default-remote-node-command route-endpoint))",
    );
    assert_eq!(
        eval(
            &mut rt,
            r#"(parameterize ((default-remote-node-command (option #f 'remote-node-command)))
                 (let* ((r1 (list (make-hop 'pipe "lab" "build" #f)))
                        (r2 (list (make-hop 'pipe #f "lab@build" #f)))
                        (r1-copy (list (make-hop 'pipe "lab" "build" #f)))
                        (n1 (route-endpoint r1))
                        (n2 (route-endpoint r2))
                        (n1-again (route-endpoint r1-copy)))
                   (list (string=? (route->string r1) (route->string r2))
                         (eq? n1 n1-again)
                         (not (eq? n1 n2)))))"#
        ),
        "(#t #t #t)"
    );
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
            "timed out waiting for {what}: {} / {:?} / {} / {:?}",
            s.pane().status,
            s.pane().text.to_string(),
            s.echo,
            s.minibuffer
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

// The keyboard reader accepts a remote directory in Find file, closes the
// prompt, and opens a listing whose links still point to the same route.
#[test]
fn find_file_browses_a_remote_directory() {
    state();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("remote");
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::write(root.join("nested/note.txt"), "from the node\n").unwrap();
    let mut rt = with_methods(Runtime::open(&dir.path().join("scratch"), &dir.path().join("journal"), "emacs").unwrap().0);
    let location = format!("/pipe::{}/", root.display());
    keys(&mut rt, "C-x C-f");
    type_text(&mut rt, &location);
    assert!(rt.snapshot().minibuffer.unwrap().input_selected);
    keys(&mut rt, "RET");
    let listing = rt.snapshot();
    assert!(listing.minibuffer.is_none(), "{}", listing.echo);
    assert!(listing.pane().text.to_string().contains("nested/"));
    keys(&mut rt, "D");
    let refused = rt.snapshot();
    assert!(refused.echo.contains("only supported on local paths"), "{}", refused.echo);
    assert!(refused.minibuffer.is_none());

    keys(&mut rt, "RET");
    assert!(rt.snapshot().pane().text.to_string().contains("note.txt"));
    keys(&mut rt, "^");
    assert_eq!(eval(&mut rt, "(default-directory (current-session))"), format!("{location:?}"));
    keys(&mut rt, "RET RET");
    let opened = until(&mut rt, "remote note.txt", |s| s.pane().text == "from the node\n");
    assert_eq!(opened.pane().text.to_string(), "from the node\n");
    assert_eq!(eval(&mut rt, "(document-path (doc (current-session)))"), format!("\"{location}nested/note.txt\""));
    keys(&mut rt, "C-x C-f");
    type_text(&mut rt, "/pipe::/");
    keys(&mut rt, "RET ^");
    let at_root = rt.snapshot();
    assert!(at_root.minibuffer.is_none());
    assert_eq!(eval(&mut rt, "(default-directory (current-session))"), r#""/pipe::/""#);
    assert!(!at_root.pane().text.to_string().contains("../"));

    // Switching from local Dirvish to an open remote listing keeps the
    // listing, but must not show local previews.
    let local = dir.path().join("local.txt");
    std::fs::write(&local, "local\n").unwrap();
    eval(&mut rt, &format!("(show-document! (current-session) (file-document {:?}))", local.to_str().unwrap()));
    eval(&mut rt, "(run-command (current-session) 'dirvish 1)");
    assert_eq!(eval(&mut rt, "(length (session-panes (current-session)))"), "3");
    keys(&mut rt, "C-x b");
    type_text(&mut rt, &location);
    keys(&mut rt, "M-RET");
    assert_eq!(eval(&mut rt, "(length (session-panes (current-session)))"), "1");
    assert_eq!(eval(&mut rt, "(default-directory (current-session))"), format!("{location:?}"));
    assert!(rt.take_errors().is_empty());
}

#[test]
fn terminal_uses_the_remote_directory_and_pty() {
    state();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("remote.txt");
    std::fs::write(&file, "remote\n").unwrap();
    let mut rt = with_methods(Runtime::open(&dir.path().join("local"), &dir.path().join("journal"), "emacs").unwrap().0);
    eval(&mut rt, &format!("(show-document! (current-session) (file-document {:?}))", format!("/pipe::{}", file.display())));
    rt.handle(Input::Viewport { columns: 42, rows: 12, split_gap: 1 });
    eval(&mut rt, "(run-command (current-session) 'terminal-command 1)");
    type_text(&mut rt, "stty -echo; cat remote.txt; stty size; read -r line; printf 'got:%s\\n' \"$line\"; stty size");
    keys(&mut rt, "RET");
    let ready = until(&mut rt, "remote PTY output", |s| s.pane().text.to_string().contains("10 42"));
    assert!(ready.pane().text.to_string().starts_with("remote"));
    assert!(eval(&mut rt, "(default-directory (current-session))").starts_with("\"/pipe::"));
    rt.handle(Input::Viewport { columns: 56, rows: 16, split_gap: 1 });
    type_text(&mut rt, "hello");
    keys(&mut rt, "RET");
    let done = until(&mut rt, "remote terminal exits", |s| s.pane().status.contains("Exit 0"));
    let text = done.pane().text.to_string();
    assert!(text.contains("got:hello") && text.contains("14 56"), "{text}");
    assert!(rt.take_errors().is_empty());
}

#[test]
fn remote_terminal_keeps_output_after_the_node_disconnects() {
    state();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("remote.txt");
    std::fs::write(&file, "remote\n").unwrap();
    let loc = format!("/pipe::{}", file.display());
    let mut rt = with_methods(Runtime::open(&dir.path().join("local"), &dir.path().join("journal"), "emacs").unwrap().0);
    eval(
        &mut rt,
        &format!("(begin (define endpoint (location-endpoint {loc:?})) (show-document! (current-session) (file-document {loc:?})))"),
    );
    eval(&mut rt, "(run-command (current-session) 'terminal-command 1)");
    type_text(&mut rt, "printf 'last useful line\\n'; read -r go");
    keys(&mut rt, "RET");
    until(&mut rt, "remote terminal ready", |s| s.pane().text.to_string().contains("last useful line"));
    let pkg = "(package-module (find-package 'terminal))";
    eval(&mut rt, &format!("(define reader-task (eval '(reader (session-of (current-buffer (current-session)))) {pkg}))"));
    eval(&mut rt, "(node-close endpoint)");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        rt.run_tasks(Duration::from_millis(10)).unwrap();
        if eval(&mut rt, "(task-done? reader-task)") == "#t" {
            eval(&mut rt, "(task-join reader-task)");
            break;
        }
        assert!(Instant::now() < deadline, "reader still waiting: {}", eval(&mut rt, "(node-lost endpoint)"));
        std::thread::sleep(Duration::from_millis(5));
    }
    let retained = rt.snapshot();
    assert!(!retained.pane().status.contains("SEMI"));
    assert!(retained.pane().text.to_string().contains("last useful line"));
    assert!(
        eval(&mut rt, "(eval '(process (session-of (current-buffer (current-session)))) (package-module (find-package 'terminal)))")
            == "#f"
    );
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
        s.pane().text.to_string().starts_with(&format!("Git status\n{}\n", repo.trim_end_matches('/'))),
        "{}",
        s.pane().text.to_string()
    );
    eval(&mut rt, &format!(r#"(call-with-process-at {dir:?} "rm" '("-rf" {name:?}) process-wait)"#));
}

#[cfg(unix)]
#[test]
fn remote_symlink_to_private_codec_and_new_private_file_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("vault.rsec");
    let alias = dir.path().join("alias.txt");
    let new_priv = dir.path().join("brand-new.rsec");
    std::fs::write(&target, "ENC:remote-secret\n").unwrap();
    symlink(&target, &alias).unwrap();

    let mut rt = runtime();
    let alias_loc = format!("/pipe::{}", alias.display());
    let target_loc = format!("/pipe::{}", target.display());
    let new_loc = format!("/pipe::{}", new_priv.display());
    let res = eval(
        &mut rt,
        &format!(
            "(begin\n\
               (define-codec rsec \"Remote private test codec.\"\n\
                 #:match (lambda (p) (string-suffix? \".rsec\" p))\n\
                 #:decode (lambda (_p r)\n\
                            (if (not r) (make-codec-result \"\")\n\
                                (let ((s (utf8->string r)))\n\
                                  (make-codec-result (if (string-prefix? \"ENC:\" s) (substring s 4) s)))))\n\
                 #:encode (lambda (_p t _e) (string->utf8 (string-append \"ENC:\" t)))\n\
                 #:private #t)\n\
               (let* ((d1 (file-document {alias_loc:?}))\n\
                      (d2 (file-document {target_loc:?}))\n\
                      (d3 (file-document {new_loc:?}))\n\
                      (v3 (make-view d3 \"user\")))\n\
                 (view-edit! v3 (list (list 0 0 \"created-privately\\n\")) \"new\")\n\
                 (save-document! d3)\n\
                 (list (eq? d1 d2)\n\
                       (codec-name (document-codec d1))\n\
                       (document-private? d1)\n\
                       (document-retention d1)\n\
                       (document-string d1)\n\
                       (document-private? d3)\n\
                       (document-retention d3))))"
        ),
    );
    assert_eq!(res, "(#t rsec #t none \"remote-secret\\n\" #t none)");

    let mode = std::fs::metadata(&new_priv).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "newly created remote private file should be mode 0o600, got {:o}", mode);
    assert_eq!(std::fs::read_to_string(&new_priv).unwrap(), "ENC:created-privately\n");

    // Swapping an already-open remote public file to a symlink pointing at a
    // private file must fail closed on reload, revert, file-text, and save.
    let pub_file = dir.path().join("public.txt");
    std::fs::write(&pub_file, "public-remote\n").unwrap();
    let pub_loc = format!("/pipe::{}", pub_file.display());
    eval(
        &mut rt,
        &format!(
            "(begin\n\
               (define %test-pub-doc (file-document {pub_loc:?}))\n\
               (define %test-pub-session (start-session (make-view %test-pub-doc \"user\") \"emacs\")))"
        ),
    );
    std::fs::remove_file(&pub_file).unwrap();
    symlink(&target, &pub_file).unwrap();
    let swapped = eval(
        &mut rt,
        "(let* ((reload-err (guard (e (#t (error-object-message e))) (reload-document! %test-pub-doc) #f))\n\
                (revert-err (guard (e (#t (error-object-message e))) (revert-document! %test-pub-doc) #f))\n\
                (read-err (guard (e (#t (error-object-message e))) (file-text %test-pub-doc) #f))\n\
                (_ (insert-text! %test-pub-session \"!\" \"new\"))\n\
                (save-err (guard (e (#t (error-object-message e))) (save-document! %test-pub-doc) #f))\n\
                (ow-err (guard (e (#t (error-object-message e))) (save-document! %test-pub-doc #:overwrite #t) #f)))\n\
           (list (and (string? reload-err) (string-contains reload-err \"file target changed on disk while reading\") #t)\n\
                 (and (string? revert-err) (string-contains revert-err \"file target changed on disk while reading\") #t)\n\
                 (and (string? read-err) (string-contains read-err \"file target changed on disk while reading\") #t)\n\
                 (and (string? save-err) (string-contains save-err \"file target changed on disk while saving\") #t)\n\
                 (and (string? ow-err) (string-contains ow-err \"file target changed on disk while saving\") #t)\n\
                 (document-string %test-pub-doc)))",
    );
    assert_eq!(swapped, "(#t #t #t #t #t \"!public-remote\\n\")");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "ENC:remote-secret\n");
}
