//! The small command-line adapter must preserve handles across invocations.
use std::{
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

/// A named node session whose socket, lock and log stay in the test's
/// own directory, so nothing outlives the test.
struct Session {
    name: String,
    cleanup: std::path::PathBuf,
    node_dir: std::path::PathBuf,
}
impl Session {
    fn new(dir: &Path) -> Self {
        let name = format!("tool-test-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
        let cleanup = dir.join(format!("cleanup-{name}.scm"));
        let node_dir = dir.join("node");
        std::fs::create_dir_all(&node_dir).unwrap();
        std::fs::write(
            &cleanup,
            format!(
                r#"
            (define n (node-connect (list {:?} "node" "--session" {:?})))
            (node-eval n "0")
            (node-shutdown n)
            (guard (e (#t #t)) (node-eval n "0"))
        "#,
                env!("CARGO_BIN_EXE_techne"),
                name
            ),
        )
        .unwrap();
        Self { name, cleanup, node_dir }
    }
    fn query(&self, root: &Path, source: &str) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_techne"))
            .args(["tool", root.to_str().unwrap(), source, &self.name])
            .env("TECHNE_NODE_DIR", &self.node_dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_techne")).arg("run").arg(&self.cleanup).env("TECHNE_NODE_DIR", &self.node_dir).output();
    }
}

#[test]
fn bounded_queries_share_snapshots_and_can_wait_for_processes() {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path());
    std::fs::write(dir.path().join("source"), "aé€😀\n").unwrap();
    let handle = session.query(dir.path(), "(tool-ref (tool-read project \"source\" #:chars 2) 'handle)");
    std::fs::write(dir.path().join("source"), "new").unwrap();
    assert_eq!(session.query(dir.path(), &format!("(tool-ref (tool-expand project {handle} #:offset 2) 'text)")), r#""€😀\n""#);
    assert_eq!(session.query(dir.path(), "(tool-ref (tool-read project \"source\") 'text)"), r#""new""#);
    assert_eq!(session.query(dir.path(), "(tool-ref (tool-process-result (tool-run project \"printf\" '(\"done\"))) 'status)"), "0");
    assert_eq!(session.query(dir.path(), "(list (tool-refresh! project) (tool-ref (tool-files project) (quote complete)))"), "(#t #t)");
    let count = session.query(dir.path(), "(list (tool-ref (tool-discover \"tool-read\" #:limit 1) 'total) (length (tool-ref (tool-discover \"tool-read\" #:limit 1) 'items)))");
    assert!(count.ends_with(" 1)"), "{count}");
}

#[test]
fn text_prints_strings_as_they_are_and_stdin_needs_no_escaping() {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path());
    let patch = "made\n<<<<<<< SEARCH\n=======\nsays \"quoted\" \\ back\n>>>>>>> REPLACE\n";
    let out = Command::new(env!("CARGO_BIN_EXE_techne"))
        .args(["tool", "--text", "--stdin", dir.path().to_str().unwrap(), "(tool-show (tool-apply! project input))", &session.name])
        .env("TECHNE_NODE_DIR", &session.node_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(patch.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8(out.stdout).unwrap().starts_with("made: created, revision "));
    assert_eq!(std::fs::read_to_string(dir.path().join("made")).unwrap(), "says \"quoted\" \\ back\n");
    let text = Command::new(env!("CARGO_BIN_EXE_techne"))
        .args(["tool", "--text", dir.path().to_str().unwrap(), "(tool-show (tool-read project \"made\") #:numbered #f)", &session.name])
        .env("TECHNE_NODE_DIR", &session.node_dir)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(text.stdout).unwrap(), "says \"quoted\" \\ back\n");
    // Without --text a string is written, so a handle can be pasted back.
    assert_eq!(session.query(dir.path(), "(tool-ref (tool-read project \"made\") 'path)"), r#""made""#);
}

#[test]
fn scheme_source_rendering_tests() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp/tests/tools/cli.scm");
    let out = Command::new(env!("CARGO_BIN_EXE_techne")).arg("run").arg(suite).arg(env!("CARGO_BIN_EXE_techne")).output().unwrap();
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

/// Copy the directory FROM to TO, recursively.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn a_form_comes_from_stdin_or_a_file_and_a_stale_session_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path());
    // A copy of Techne's Lisp, whose tool sources change below.
    let lisp = dir.path().join("lisp");
    copy_tree(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp"), &lisp);
    std::fs::write(dir.path().join("source"), "text").unwrap();
    let run = |args: &[&str], stdin: &str| {
        use std::io::Write;
        let mut child = Command::new(env!("CARGO_BIN_EXE_techne"))
            .arg("tool")
            .args(args)
            .env("TECHNE_NODE_DIR", &session.node_dir)
            .env("TECHNE_LISP_DIR", &lisp)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{out:?}");
        (String::from_utf8(out.stdout).unwrap().trim().to_owned(), String::from_utf8(out.stderr).unwrap())
    };
    let root = dir.path().to_str().unwrap();
    let (handle, said) = run(&[root, "-", &session.name], "(tool-ref (tool-read project \"source\") 'handle)");
    assert!(handle.starts_with('"') && said.is_empty(), "{handle} {said}");
    // A job whose timeout falls while no request comes: killed all the same.
    let pid_of = |form: &str| -> u32 {
        let (out, _) = run(&[root, form, &session.name], "");
        out.trim_matches(|c: char| !c.is_ascii_digit()).parse().unwrap_or_else(|_| panic!("{out}"))
    };
    let gone = |pid: u32| {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        stat.is_empty() || stat.split(") ").nth(1).is_some_and(|s| s.starts_with('Z'))
    };
    let pid = pid_of(
        "(tool-ref (tool-ref (tool-process-result (tool-run project \"sh\" '(\"-c\" \"echo $$; exec sleep 60\") #:timeout 0.5) #:wait 0.3) 'stdout) 'text)",
    );
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(gone(pid), "killed between requests: {pid}");
    // A job still running when the session is restarted: stopped with it.
    let lasting = pid_of(
        "(tool-ref (tool-ref (tool-process-result (tool-run project \"sh\" '(\"-c\" \"echo $$; exec sleep 60\")) #:wait 0.3) 'stdout) 'text)",
    );
    assert!(!gone(lasting));
    std::fs::write(dir.path().join("form.scm"), format!("(guard (e (#t #f)) (tool-ref (tool-expand project {handle}) 'text))")).unwrap();
    let form = format!("@{}", dir.path().join("form.scm").display());
    assert_eq!(run(&[root, &form, &session.name], "").0, "\"text\"", "the same session, its snapshot");
    // The tools change: the session ran older code, and is restarted.
    let help = lisp.join("techne/tools-help.scm");
    let mut source = std::fs::read_to_string(&help).unwrap();
    source.push_str("\n;; changed\n");
    std::fs::write(&help, source).unwrap();
    let (expired, said) = run(&[root, &form, &session.name], "");
    assert!(said.contains("ran older code; restarting it"), "{said}");
    assert!(gone(lasting), "the old session's processes are stopped: {lasting}");
    assert_eq!(expired, "#f", "a new session: the old snapshot is gone");
    let (_, said) = run(&[root, "(tool-files project)", &session.name], "");
    assert!(said.is_empty(), "restarted once: {said}");
}

/// The pid a session's job printed first, and whether a pid has ended.
fn job_pid(session: &Session, root: &Path, idle: &str) -> u32 {
    let out = Command::new(env!("CARGO_BIN_EXE_techne"))
        .args([
            "tool",
            root.to_str().unwrap(),
            "(tool-ref (tool-ref (tool-process-result (tool-run project \"sh\" '(\"-c\" \"echo $$; exec sleep 60\")) #:wait 0.3) 'stdout) 'text)",
            &session.name,
        ])
        .env("TECHNE_NODE_DIR", &session.node_dir)
        .env("TECHNE_NODE_IDLE_SECS", idle)
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    text.trim_matches(|c: char| !c.is_ascii_digit()).parse().unwrap_or_else(|_| panic!("{text}"))
}
fn ended(pid: u32) -> bool {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    stat.is_empty() || stat.split(") ").nth(1).is_some_and(|s| s.starts_with('Z'))
}
fn until(what: &str, mut done: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !done() {
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "{what}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn a_session_shut_down_or_idle_stops_its_evaluation_and_processes() {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path());
    let pid = job_pid(&session, dir.path(), "600");
    assert!(!ended(pid));
    // An evaluation that does not end holds the VM: shutting down stops it.
    let mut busy = Command::new(env!("CARGO_BIN_EXE_techne"))
        .args(["tool", dir.path().to_str().unwrap(), "(let loop () (loop))", &session.name])
        .env("TECHNE_NODE_DIR", &session.node_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    // Shut it down at once: an evaluation first would wait behind the
    // loop. The one after waits for the node to end, the shutdown sent.
    let shutdown = dir.path().join("shutdown.scm");
    std::fs::write(
        &shutdown,
        format!(
            "(define n (node-connect (list {:?} \"node\" \"--session\" {:?}))) (node-shutdown n) (guard (e (#t #t)) (node-eval n \"0\"))",
            env!("CARGO_BIN_EXE_techne"),
            session.name
        ),
    )
    .unwrap();
    let start = std::time::Instant::now();
    let out =
        Command::new(env!("CARGO_BIN_EXE_techne")).arg("run").arg(&shutdown).env("TECHNE_NODE_DIR", &session.node_dir).output().unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(15), "{out:?}");
    until("the job is stopped with its session", || ended(pid));
    until("the busy client ends", || busy.try_wait().unwrap().is_some());

    // Idle, the session ends by itself, and its job with it.
    let idle = Session::new(dir.path());
    let pid = job_pid(&idle, dir.path(), "1");
    until("an idle session's job is stopped", || ended(pid));
}

#[test]
fn definitions_stay_in_the_session_and_see_each_calls_project() {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path());
    let (one, two) = (dir.path().join("one"), dir.path().join("two"));
    std::fs::create_dir_all(&one).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    std::fs::write(one.join("a"), "").unwrap();
    std::fs::write(two.join("b"), "").unwrap();
    std::fs::write(two.join("c"), "").unwrap();
    // A definition prints nothing, and is there in the next call.
    assert_eq!(session.query(&one, "(define (files) (tool-ref (tool-files project) 'total))"), "");
    assert_eq!(session.query(&one, "(files)"), "1");
    // `project` is each call's, so a helper follows the call's root.
    assert_eq!(session.query(&two, "(files)"), "2");
    assert_eq!(session.query(&one, "(define kept 41)"), "");
    assert_eq!(session.query(&two, "(+ kept 1)"), "42");
}

#[test]
fn handles_cannot_alias_after_connecting_to_another_server() {
    let dir = tempfile::tempdir().unwrap();
    let first = Session::new(dir.path());
    let second = Session::new(dir.path());
    std::fs::write(dir.path().join("source"), "same bytes").unwrap();
    let handle = first.query(dir.path(), "(tool-ref (tool-read project \"source\") (quote handle))");
    second.query(dir.path(), "(tool-read project \"source\")");
    let result = second.query(dir.path(), &format!("(guard (e (#t #t)) (tool-expand project {handle}) #f)"));
    assert_eq!(result, "#t");
}

#[test]
fn concurrent_first_clients_share_one_session_daemon() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("source"), "stable").unwrap();
    for _ in 0..5 {
        let session = Session::new(dir.path());
        let query = "(tool-ref (tool-read project \"source\") (quote handle))";
        let handles = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..3)
                .map(|_| {
                    let session = &session;
                    let root = dir.path();
                    scope.spawn(move || session.query(root, query))
                })
                .collect();
            workers.into_iter().map(|worker| worker.join().unwrap()).collect::<Vec<_>>()
        });
        // Availability alone misses a loser unlinking the live socket. A
        // later client must reach the same world and its original snapshot.
        let expected = session.query(dir.path(), query);
        assert!(handles.iter().all(|handle| handle == &expected), "{handles:?} vs {expected}");
    }
}

#[test]
fn help_needs_neither_a_project_nor_a_session_server() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_str().unwrap();
    let help_with_a_session = [vec!["tool", root, "--help", "some-session"], vec!["tool", "--text", root, "-h", "some-session"]];
    for args in [vec!["tool"], vec!["tool", "--help"], vec!["tool", root, "--help"], vec!["tool", root, "-h"]]
        .into_iter()
        .chain(help_with_a_session)
    {
        let out = Command::new(env!("CARGO_BIN_EXE_techne")).args(args).env("XDG_RUNTIME_DIR", dir.path()).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let help = String::from_utf8(out.stdout).unwrap();
        assert!(help.contains("not an autonomous agent") && help.contains("tool-read") && help.contains("tool-help"), "{help}");
        assert!(help.len() < 8000);
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "help must not create session sockets");
    // Nor read stdin: with --stdin, help comes while stdin stays open.
    let mut child = Command::new(env!("CARGO_BIN_EXE_techne"))
        .args(["tool", "--stdin", root, "--help"])
        .env("XDG_RUNTIME_DIR", dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take();
    // Bounded: reading stdin would wait for ever.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("techne tool --stdin ROOT --help waited for stdin");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    drop(stdin);
    assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("not an autonomous agent"));
}

#[test]
fn flags_may_come_anywhere_and_unknown_ones_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path());
    let root = dir.path().to_str().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_techne")).arg("tool").args(args).env("TECHNE_NODE_DIR", &session.node_dir).output().unwrap()
    };
    // A flag after FORM and SESSION is a flag, not a session of that
    // name: the string answer is printed as it is.
    let out = run(&[root, "(string-append \"a\" \"b\")", &session.name, "--text"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "ab");
    // And one before FORM, with `--` keeping the rest positional.
    let out = run(&[root, "--text", "--", "(string-append \"c\" \"d\")", &session.name]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "cd");
    // A form that is a flag's name stays a form after `--`.
    session.query(dir.path(), "(define --text 42)");
    let out = run(&["--", root, "--text", &session.name]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "42");
    // An unknown flag fails and names itself, wherever it stands
    // (the trailing one once silently named a session and succeeded).
    for args in [vec!["--tex", root, "1"], vec![root, "1", "--bogus"], vec![root, "1", &session.name, "--bogus"]] {
        let out = run(&args);
        let flag = args.iter().find(|a| a.starts_with("--")).unwrap();
        assert!(!out.status.success(), "{args:?} succeeded");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(&format!("unknown flag {flag}")), "{args:?}: {stderr}");
    }
}

#[test]
fn one_form_is_required_and_errors_are_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    for source in ["", "1 2", "(tool-read project \"absent\")"] {
        let session = Session::new(dir.path());
        let out = Command::new(env!("CARGO_BIN_EXE_techne"))
            .args(["tool", dir.path().to_str().unwrap(), source, &session.name])
            .env("TECHNE_NODE_DIR", &session.node_dir)
            .output()
            .unwrap();
        assert!(!out.status.success(), "{source}: {out:?}");
    }
}

#[test]
fn the_default_session_is_this_binarys_own() {
    let dir = tempfile::tempdir().unwrap();
    let node_dir = dir.path().join("node");
    std::fs::create_dir_all(&node_dir).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_techne"))
        .args(["tool", dir.path().to_str().unwrap(), "(+ 1 2)"])
        .env("TECHNE_NODE_DIR", &node_dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "3");
    // Named for this binary, not "tools", which any other techne would
    // share.
    let names: Vec<String> = std::fs::read_dir(&node_dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    let session = names.iter().find_map(|n| n.strip_suffix(".sock")).expect("a session socket").to_owned();
    assert!(session.starts_with("tools-") && session.len() == "tools-".len() + 12, "{names:?}");
    // Stop it, so nothing outlives the test.
    let cleanup = dir.path().join("cleanup.scm");
    std::fs::write(
        &cleanup,
        format!(
            r#"(define n (node-connect (list {:?} "node" "--session" {session:?}))) (node-shutdown n) (guard (e (#t #t)) (node-eval n "0"))"#,
            env!("CARGO_BIN_EXE_techne")
        ),
    )
    .unwrap();
    let _ = Command::new(env!("CARGO_BIN_EXE_techne")).arg("run").arg(&cleanup).env("TECHNE_NODE_DIR", &node_dir).output();
}
