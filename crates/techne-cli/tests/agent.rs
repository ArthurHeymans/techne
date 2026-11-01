//! The small command-line adapter must preserve handles across invocations.
use std::{
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Session {
    name: String,
    cleanup: std::path::PathBuf,
}
impl Session {
    fn new(dir: &Path) -> Self {
        let name = format!("agent-test-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
        let cleanup = dir.join(format!("cleanup-{name}.scm"));
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
        Self { name, cleanup }
    }
    fn query(&self, root: &Path, source: &str) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_techne")).args(["agent", root.to_str().unwrap(), source, &self.name]).output().unwrap();
        assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_techne")).arg("run").arg(&self.cleanup).output();
    }
}

#[test]
fn bounded_queries_share_snapshots_and_can_wait_for_processes() {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path());
    std::fs::write(dir.path().join("source"), "aé€😀\n").unwrap();
    let handle = session.query(dir.path(), "(agent-ref (agent-read project \"source\" #:chars 2) 'handle)");
    std::fs::write(dir.path().join("source"), "new").unwrap();
    assert_eq!(session.query(dir.path(), &format!("(agent-ref (agent-expand project {handle} #:offset 2) 'text)")), r#""€😀\n""#);
    assert_eq!(session.query(dir.path(), "(agent-ref (agent-read project \"source\") 'text)"), r#""new""#);
    assert_eq!(session.query(dir.path(), "(agent-ref (agent-process-result (agent-run project \"printf\" '(\"done\"))) 'status)"), "0");
    assert_eq!(session.query(dir.path(), "(list (agent-refresh! project) (agent-ref (agent-files project) (quote complete)))"), "(#t #t)");
    let count = session.query(dir.path(), "(list (agent-ref (agent-discover \"agent-read\" #:limit 1) 'total) (length (agent-ref (agent-discover \"agent-read\" #:limit 1) 'items)))");
    assert!(count.ends_with(" 1)"), "{count}");
}

#[test]
fn handles_cannot_alias_after_connecting_to_another_server() {
    let dir = tempfile::tempdir().unwrap();
    let first = Session::new(dir.path());
    let second = Session::new(dir.path());
    std::fs::write(dir.path().join("source"), "same bytes").unwrap();
    let handle = first.query(dir.path(), "(agent-ref (agent-read project \"source\") (quote handle))");
    second.query(dir.path(), "(agent-read project \"source\")");
    let result = second.query(dir.path(), &format!("(guard (e (#t #t)) (agent-expand project {handle}) #f)"));
    assert_eq!(result, "#t");
}

#[test]
fn concurrent_first_clients_share_one_session_daemon() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("source"), "stable").unwrap();
    for _ in 0..5 {
        let session = Session::new(dir.path());
        let query = "(agent-ref (agent-read project \"source\") (quote handle))";
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
fn one_form_is_required_and_errors_are_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    for source in ["", "1 2", "(agent-read project \"absent\")"] {
        let session = Session::new(dir.path());
        let out = Command::new(env!("CARGO_BIN_EXE_techne"))
            .args(["agent", dir.path().to_str().unwrap(), source, &session.name])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{source}: {out:?}");
    }
}
