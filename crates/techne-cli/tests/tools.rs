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
        .args(["tool", "--text", dir.path().to_str().unwrap(), "(tool-show (tool-read project \"made\"))", &session.name])
        .env("TECHNE_NODE_DIR", &session.node_dir)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(text.stdout).unwrap(), "says \"quoted\" \\ back\n");
    // Without --text a string is written, so a handle can be pasted back.
    assert_eq!(session.query(dir.path(), "(tool-ref (tool-read project \"made\") 'path)"), r#""made""#);
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
    for args in [vec!["tool"], vec!["tool", "--help"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_techne")).args(args).env("XDG_RUNTIME_DIR", dir.path()).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let help = String::from_utf8(out.stdout).unwrap();
        assert!(help.contains("not an autonomous agent") && help.contains("tool-read") && help.contains("tool-help"), "{help}");
        assert!(help.len() < 8000);
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "help must not create session sockets");
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
