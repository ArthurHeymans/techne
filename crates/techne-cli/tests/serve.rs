//! `techne cloud serve`: tenants invited, each deploying its projects by
//! upload, served by host name from a worker of its own, kept apart from
//! the others', and there again when the service is started again.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const PATIENCE: Duration = Duration::from_secs(10);

const APP: &str = r#"(import (techne http))
(provide fetch)
(define (fetch request env ctx)
  (let ((path (dict-ref request 'path))
        (notes (dict-ref env 'notes #f)))
    (cond ((equal? path "/put") (kv-put! notes "who" "WHO") (response "kept"))
          ((equal? path "/get") (response (kv-get notes "who" "nothing")))
          ((equal? path "/secret") (response (dict-ref env 'key "none")))
          (else (response "WHO VERSION")))))
"#;

const MANIFEST: &str = r#"{bindings {notes (kv "notes") key (secret "KEY")}}"#;

struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn techne() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_techne"));
    command.env_remove("TECHNE_TOKEN");
    command
}

/// `techne cloud ARGS`, with `token` if given: whether it succeeded, and
/// what it said.
fn cloud(args: &[&str], token: Option<&str>) -> (bool, String) {
    let mut command = techne();
    command.arg("cloud").args(args);
    if let Some(token) = token {
        command.env("TECHNE_TOKEN", token);
    }
    let out = command.output().unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// The status and body of the response to GET `path` for `host` on
/// `port`.
fn get(port: u16, host: &str, path: &str) -> (u16, String) {
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(PATIENCE)).unwrap();
    write!(c, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
    let mut answer = String::new();
    c.read_to_string(&mut answer).unwrap();
    let status = answer.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    (status, answer.split_once("\r\n\r\n").map(|(_, body)| body.to_string()).unwrap_or_default())
}

/// `techne cloud serve` on `state`: the process, and the ports it
/// serves requests and operations on.
fn serve(state: &Path) -> (Killed, u16, String) {
    let mut child = Killed(
        techne()
            .args(["cloud", "serve", "--state", state.to_str().unwrap(), "--domain", "cloud.test"])
            .args(["--port", "0", "--ops", "0", "--idle", "1", "--unconfined"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let (tx, lines) = mpsc::channel();
    let out = child.0.stdout.take().unwrap();
    std::thread::spawn(move || BufReader::new(out).lines().map_while(Result::ok).try_for_each(|l| tx.send(l)));
    let (mut port, mut ops) = (None, None);
    while port.is_none() || ops.is_none() {
        let line = lines.recv_timeout(PATIENCE).expect("the ports");
        let number = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
        if line.starts_with("Serving") {
            port = number;
        } else if line.starts_with("Operations") {
            ops = number;
        }
    }
    (child, port.unwrap(), format!("http://127.0.0.1:{}", ops.unwrap()))
}

/// A directory of `who`'s app at `version`, under `dir`: its entry.
fn app(dir: &Path, who: &str, version: &str) -> String {
    let at = dir.join(who);
    std::fs::create_dir_all(at.join("lib")).unwrap();
    std::fs::write(at.join("app.scm"), APP.replace("WHO", who).replace("VERSION", version)).unwrap();
    std::fs::write(at.join("manifest.scm"), MANIFEST).unwrap();
    // Not uploaded: hidden.
    std::fs::write(at.join(".secrets.scm"), r#"{"KEY" "local"}"#).unwrap();
    at.join("app.scm").to_str().unwrap().to_string()
}

/// Where workers' resources cannot be bounded (in a test run they
/// cannot: it has no cgroup of its own), the service does not start,
/// and listens nowhere, unless told it may run unconfined.
#[test]
fn it_does_not_start_where_workers_cannot_be_confined() {
    let dir = tempfile::tempdir().unwrap();
    let out = techne()
        .args(["cloud", "serve", "--state", dir.path().to_str().unwrap(), "--domain", "cloud.test", "--port", "0", "--ops", "0"])
        .output()
        .unwrap();
    // Confined after all, where it can be: then it runs, and is not this
    // test's to judge.
    if out.status.success() {
        return;
    }
    let (said, err) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!said.contains("Serving") && !said.contains("Operations"), "{said}");
    assert!(err.contains("cgroup") || err.contains("sandbox") || err.contains("worker"), "{err}");
}

#[test]
fn tenants_deploy_by_upload_and_are_served_by_host_name() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let invite = |tenant: &str| {
        let (ok, token) = cloud(&["invite", "--state", state.to_str().unwrap(), tenant], None);
        assert!(ok, "{token}");
        token.trim().to_string()
    };
    let (acme, bolt) = (invite("acme"), invite("bolt"));
    assert!(!cloud(&["invite", "--state", state.to_str().unwrap(), "Not/A-Name"], None).0);
    let (child, port, ops) = serve(&state);
    let on = |tenant: &'static str, project: &'static str| ["--to", ops.as_str(), "--tenant", tenant, "--project", project];
    let with = |command: &[&str], target: [&str; 6], token: Option<&str>| cloud(&[command, &target[..]].concat(), token);
    // Nothing deployed: nothing served.
    assert_eq!(get(port, "shop.acme.cloud.test", "/").0, 404);
    // A secret, then a deployment that binds it, uploaded.
    let entry = app(dir.path(), "acme", "1");
    let (ok, said) = with(&["secret", "KEY", "s3cret"], on("acme", "shop"), Some(&acme));
    assert!(ok, "{said}");
    let (ok, said) = with(&["deploy", &entry], on("acme", "shop"), Some(&acme));
    assert!(ok, "{said}");
    assert_eq!(get(port, "shop.acme.cloud.test", "/"), (200, "acme 1".into()));
    assert_eq!(get(port, "Shop.Acme.Cloud.Test:80", "/secret"), (200, "s3cret".into()));
    assert_eq!(get(port, "shop.acme.cloud.test", "/put").1, "kept");
    // Not another's: by host name, nor by token.
    for host in ["shop.bolt.cloud.test", "other.acme.cloud.test", "shop.acme.elsewhere.test", "acme.cloud.test", "cloud.test"] {
        assert_eq!(get(port, host, "/").0, 404, "{host}");
    }
    let bolts = app(dir.path(), "bolt", "1");
    let (ok, said) = with(&["deploy", &bolts], on("bolt", "shop"), Some(&acme));
    assert!(!ok && said.contains("refused"), "{said}");
    let (ok, said) = with(&["deployments"], on("acme", "shop"), Some(&bolt));
    assert!(!ok && said.contains("refused"), "{said}");
    let (ok, said) = with(&["deploy", &bolts], on("ghost", "shop"), Some(&acme));
    assert!(!ok && said.contains("refused"), "{said}");
    let (ok, said) = with(&["deployments"], on("acme", "shop"), Some("tk1.none.none"));
    assert!(!ok && said.contains("unauthenticated"), "{said}");
    assert_eq!(get(port, "shop.bolt.cloud.test", "/").0, 404);
    // Its own: the same project's name, and namespace's, another's.
    // (A secret it has not put is not bound: its deployment is refused.)
    let (ok, said) = with(&["deploy", &bolts], on("bolt", "shop"), Some(&bolt));
    assert!(!ok && said.contains("KEY"), "{said}");
    assert!(with(&["secret", "KEY", "bolts"], on("bolt", "shop"), Some(&bolt)).0);
    let (ok, said) = with(&["deploy", &bolts], on("bolt", "shop"), Some(&bolt));
    assert!(ok, "{said}");
    assert_eq!(get(port, "shop.bolt.cloud.test", "/secret"), (200, "bolts".into()));
    assert_eq!(get(port, "shop.bolt.cloud.test", "/get"), (200, "nothing".into()));
    assert_eq!(get(port, "shop.acme.cloud.test", "/get"), (200, "acme".into()));
    // Idle, its worker stopped; the next request is served all the same.
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(get(port, "shop.acme.cloud.test", "/get"), (200, "acme".into()));
    // Deployed again, and rolled back.
    let entry = app(dir.path(), "acme", "2");
    assert!(with(&["deploy", &entry], on("acme", "shop"), Some(&acme)).0);
    assert_eq!(get(port, "shop.acme.cloud.test", "/"), (200, "acme 2".into()));
    let (ok, listed) = with(&["deployments"], on("acme", "shop"), Some(&acme));
    assert!(ok && listed.matches("\"id\"").count() == 2, "{listed}");
    let (ok, said) = with(&["rollback"], on("acme", "shop"), Some(&acme));
    assert!(ok, "{said}");
    assert_eq!(get(port, "shop.acme.cloud.test", "/"), (200, "acme 1".into()));
    // Started again: what was published is, with what was kept.
    drop(child);
    let (_child, port, _) = serve(&state);
    let start = Instant::now();
    while get(port, "shop.acme.cloud.test", "/") != (200, "acme 1".into()) {
        assert!(start.elapsed() < PATIENCE, "{:?}", get(port, "shop.acme.cloud.test", "/"));
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(get(port, "shop.acme.cloud.test", "/get"), (200, "acme".into()));
    assert_eq!(get(port, "shop.bolt.cloud.test", "/"), (200, "bolt 1".into()));
    assert_eq!(get(port, "shop.bolt.cloud.test", "/secret"), (200, "bolts".into()));
}
