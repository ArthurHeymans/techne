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
(define (said) "WHO VERSION")
(define (fetch request env ctx)
  (let ((path (dict-ref request 'path))
        (notes (dict-ref env 'notes #f)))
    (cond ((equal? path "/put") (kv-put! notes "who" "WHO") (response "kept"))
          ((equal? path "/get") (response (kv-get notes "who" "nothing")))
          ((equal? path "/secret") (response (dict-ref env 'key "none")))
          (else (response (said))))))
"#;

/// Send `fields` on `c`, an nREPL connection, and read until a message
/// has `status`: that message, or none if the connection ended first.
fn ask_repl(c: &mut TcpStream, fields: Vec<(&str, techne_node::bencode::B)>, status: &str) -> Option<techne_node::bencode::B> {
    use techne_node::bencode::{B, dict, parse};
    let mut bytes = Vec::new();
    dict(fields).encode(&mut bytes);
    c.write_all(&bytes).ok()?;
    let mut buf = Vec::new();
    loop {
        let mut chunk = [0u8; 4096];
        let n = c.read(&mut chunk).ok().filter(|n| *n > 0)?;
        buf.extend_from_slice(&chunk[..n]);
        while let Some((msg, used)) = parse(&buf).unwrap() {
            buf.drain(..used);
            if matches!(msg.get("status"), Some(B::List(l)) if l.iter().any(|s| s.as_str() == Some(status))) {
                return Some(msg);
            }
        }
    }
}

/// The status, head and body of the response to GET `path` for `host`
/// on `port`, sent with the routing credential `credential`.
fn get_routed(port: u16, host: &str, path: &str, credential: &str) -> (u16, String, String) {
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(PATIENCE)).unwrap();
    write!(c, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\ntechne-debug-route: {credential}\r\n\r\n").unwrap();
    let mut answer = String::new();
    c.read_to_string(&mut answer).unwrap();
    let status = answer.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let (head, body) = answer.split_once("\r\n\r\n").unwrap_or_default();
    (status, head.to_string(), body.to_string())
}

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
    cloud_given(args, token, "")
}

/// `cloud`, given `input` on its standard input (a secret's value).
fn cloud_given(args: &[&str], token: Option<&str>, input: &str) -> (bool, String) {
    let mut command = techne();
    command.arg("cloud").args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(token) = token {
        command.env("TECHNE_TOKEN", token);
    }
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
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

/// `techne cloud serve` on `state`: the process, the port it serves
/// requests on, where its operations are, and its nREPL port.
fn serve(state: &Path) -> (Killed, u16, String, u16) {
    let mut child = Killed(
        techne()
            .args(["cloud", "serve", "--state", state.to_str().unwrap(), "--domain", "cloud.test"])
            .args(["--port", "0", "--ops", "0", "--nrepl", "0", "--idle", "1", "--unconfined"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let (tx, lines) = mpsc::channel();
    let out = child.0.stdout.take().unwrap();
    std::thread::spawn(move || BufReader::new(out).lines().map_while(Result::ok).try_for_each(|l| tx.send(l)));
    let (mut port, mut ops, mut nrepl) = (None, None, None);
    while port.is_none() || ops.is_none() || nrepl.is_none() {
        let line = lines.recv_timeout(PATIENCE).expect("the ports");
        let number = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
        if line.starts_with("Serving") {
            port = number;
        } else if line.starts_with("Operations") {
            ops = number;
        } else if line.starts_with("nREPL") {
            nrepl = number;
        }
    }
    (child, port.unwrap(), format!("http://127.0.0.1:{}", ops.unwrap()), nrepl.unwrap())
}

/// A tenant attaches a REPL to its project in production: what it
/// evaluates changes the project's debug instance alone, which serves
/// the requests that show the session's credential, marked, and no
/// other; another tenant's token attaches nothing; the session over,
/// its credential routes nowhere.
#[test]
fn a_repl_into_a_project_in_production() {
    use techne_node::bencode::B;
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let invite = |tenant: &str| cloud(&["invite", "--state", state.to_str().unwrap(), tenant], None).1.trim().to_string();
    let (acme, bolt) = (invite("acme"), invite("bolt"));
    let (_child, port, ops, nrepl) = serve(&state);
    let entry = app(dir.path(), "acme", "1");
    let target = ["--to", ops.as_str(), "--tenant", "acme", "--project", "shop"];
    assert!(cloud_given(&[&["secret", "KEY"], &target[..]].concat(), Some(&acme), "s3cret\n").0);
    let (ok, said) = cloud(&[&["deploy", &entry], &target[..]].concat(), Some(&acme));
    assert!(ok, "{said}");
    let host = "shop.acme.cloud.test";
    let connect = || {
        let c = TcpStream::connect(("127.0.0.1", nrepl)).unwrap();
        c.set_read_timeout(Some(PATIENCE)).unwrap();
        c
    };
    let authenticate = |c: &mut TcpStream, token: &str, project: &str| {
        let fields = vec![("op", B::str("authenticate")), ("token", B::str(token)), ("project", B::str(project)), ("id", B::str("1"))];
        ask_repl(c, fields, "done")
    };
    let refused = |answer: Option<B>| match answer {
        Some(m) => matches!(m.get("status"), Some(B::List(l)) if l.iter().any(|s| s.as_str() == Some("unauthenticated"))),
        None => true,
    };
    // Not another tenant's token, nor a project that is not there, nor
    // without authenticating.
    assert!(refused(authenticate(&mut connect(), &bolt, "acme/shop")));
    assert!(refused(authenticate(&mut connect(), &acme, "acme/none")));
    assert!(refused(authenticate(&mut connect(), &acme, "bolt/shop")));
    assert!(refused(ask_repl(&mut connect(), vec![("op", B::str("eval")), ("code", B::str("1")), ("id", B::str("1"))], "done")));
    // Its own: a session, and its credential.
    let mut c = connect();
    let opened = authenticate(&mut c, &acme, "acme/shop").expect("an answer");
    let credential = opened.get("techne-debug-route").and_then(B::as_str).expect("a credential").to_string();
    let session = ask_repl(&mut c, vec![("op", B::str("clone")), ("id", B::str("2"))], "done").unwrap();
    let session = session.get("new-session").and_then(B::as_str).unwrap().to_string();
    let eval = |c: &mut TcpStream, code: &str| {
        ask_repl(c, vec![("op", B::str("eval")), ("code", B::str(code)), ("session", B::str(&session)), ("id", B::str("3"))], "done")
            .unwrap()
    };
    eval(&mut c, "(define (said) \"debugged\")");
    // Redefined in the debug instance alone, which the credential routes
    // to: marked, and with what production is bound to.
    assert_eq!(get(port, host, "/"), (200, "acme 1".into()));
    let (status, head, body) = get_routed(port, host, "/", &credential);
    assert_eq!((status, body.as_str()), (200, "debugged"));
    assert!(head.contains("\r\ntechne-debug: ") && head.contains("private, no-store"), "{head}");
    assert_eq!(get_routed(port, host, "/secret", &credential).2, "s3cret");
    // No other credential, and not another project with this one.
    assert_eq!(get_routed(port, host, "/", "0123456789abcdef0123456789abcdef").2, "acme 1");
    assert_eq!(get_routed(port, "shop.bolt.cloud.test", "/", &credential).0, 404);
    // Kept awake by its session, however long idle.
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(get_routed(port, host, "/", &credential).2, "debugged");
    // The session over: its credential routes nowhere, and the debug
    // instance is gone, what it redefined with it.
    drop(c);
    let start = Instant::now();
    while get_routed(port, host, "/", &credential).2 != "acme 1" {
        assert!(start.elapsed() < PATIENCE, "still routed");
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut c = connect();
    let again = authenticate(&mut c, &acme, "acme/shop").expect("an answer");
    let credential = again.get("techne-debug-route").and_then(B::as_str).unwrap().to_string();
    assert_eq!(get_routed(port, host, "/", &credential).2, "acme 1");
    // Deployed again: the debug instance goes with the deployment it was
    // of, its client's connection is closed, and its credential routes to
    // no debug instance of the next, whoever attaches to that.
    let entry = app(dir.path(), "acme", "2");
    assert!(cloud(&[&["deploy", &entry], &target[..]].concat(), Some(&acme)).0);
    assert_eq!(get(port, host, "/"), (200, "acme 2".into()));
    let mut end = [0u8; 64];
    let start = Instant::now();
    while c.read(&mut end).is_ok_and(|n| n > 0) {
        assert!(start.elapsed() < PATIENCE, "the connection is still open");
    }
    let mut next = connect();
    let opened = authenticate(&mut next, &acme, "acme/shop").expect("an answer");
    let fresh = opened.get("techne-debug-route").and_then(B::as_str).unwrap().to_string();
    let session = ask_repl(&mut next, vec![("op", B::str("clone")), ("id", B::str("2"))], "done").unwrap();
    let session = session.get("new-session").and_then(B::as_str).unwrap().to_string();
    let redefine =
        vec![("op", B::str("eval")), ("code", B::str("(define (said) \"again\")")), ("session", B::str(&session)), ("id", B::str("3"))];
    ask_repl(&mut next, redefine, "done").unwrap();
    assert_eq!(get_routed(port, host, "/", &fresh).2, "again");
    let start = Instant::now();
    while get_routed(port, host, "/", &credential).2 != "acme 2" {
        assert!(start.elapsed() < PATIENCE, "the credential of the session before still routes");
        std::thread::sleep(Duration::from_millis(100));
    }
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
    let (child, port, ops, _) = serve(&state);
    let on = |tenant: &'static str, project: &'static str| ["--to", ops.as_str(), "--tenant", tenant, "--project", project];
    let with = |command: &[&str], target: [&str; 6], token: Option<&str>| cloud(&[command, &target[..]].concat(), token);
    // Nothing deployed: nothing served.
    assert_eq!(get(port, "shop.acme.cloud.test", "/").0, 404);
    // A secret, then a deployment that binds it, uploaded.
    let entry = app(dir.path(), "acme", "1");
    let secret = |target: [&str; 6], token: &str, value: &str| cloud_given(&[&["secret", "KEY"], &target[..]].concat(), Some(token), value);
    let (ok, said) = secret(on("acme", "shop"), &acme, "s3cret\n");
    assert!(ok, "{said}");
    // Its value is on no command line, and its token goes to no other
    // machine in the clear.
    assert!(!with(&["secret", "KEY", "on-the-command-line"], on("acme", "shop"), Some(&acme)).0);
    let (ok, said) = cloud(&["deployments", "--to", "http://cloud.example:8788", "--tenant", "acme", "--project", "shop"], Some(&acme));
    assert!(!ok && said.contains("not on this machine"), "{said}");
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
    assert!(secret(on("bolt", "shop"), &bolt, "bolts").0);
    // A project refused for what was uploaded is not one: nothing of it
    // is there to attach to, or to take one of the tenant's places.
    let (ok, said) = with(&["deploy", "/dev/null"], on("bolt", "ghost"), Some(&bolt));
    assert!(!ok, "{said}");
    assert!(!state.join("tenants/bolt/ghost").exists());
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
    let (_child, port, _, _) = serve(&state);
    let start = Instant::now();
    while get(port, "shop.acme.cloud.test", "/") != (200, "acme 1".into()) {
        assert!(start.elapsed() < PATIENCE, "{:?}", get(port, "shop.acme.cloud.test", "/"));
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(get(port, "shop.acme.cloud.test", "/get"), (200, "acme".into()));
    assert_eq!(get(port, "shop.bolt.cloud.test", "/"), (200, "bolt 1".into()));
    assert_eq!(get(port, "shop.bolt.cloud.test", "/secret"), (200, "bolts".into()));
}

fn git_init_commit(dir: &Path, msg: &str) {
    let run = |args: &[&str]| {
        let out = Command::new("git").args(args).current_dir(dir).env("GIT_TERMINAL_PROMPT", "0").output().unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    };
    if !dir.join(".git").exists() {
        run(&["init", "--quiet"]);
    }
    run(&["add", "."]);
    run(&["-c", "user.name=Test", "-c", "user.email=test@example.org", "-c", "commit.gpgSign=false", "commit", "-qm", msg]);
}

fn write_pkg_file(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn cloud_with_store(args: &[&str], token: Option<&str>, store: &Path) -> (bool, String) {
    let mut command = techne();
    command.arg("cloud").args(args).env("TECHNE_PACKAGE_STORE", store).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(token) = token {
        command.env("TECHNE_TOKEN", token);
    }
    let out = command.output().unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

fn post_ops(ops_url: &str, token: &str, project_header: &str, body: &str) -> (u16, String) {
    let addr = ops_url.strip_prefix("http://").unwrap();
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(PATIENCE)).unwrap();
    write!(
        c,
        "POST /v1/ops HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nAuthorization: Bearer {token}\r\nTechne-Project: {project_header}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = String::new();
    c.read_to_string(&mut answer).unwrap();
    let status = answer.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let body = answer.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = ((c[0] as u32) << 16) | ((*c.get(1).unwrap_or(&0) as u32) << 8) | (*c.get(2).unwrap_or(&0) as u32);
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if c.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn has_file_named(dir: &Path, target: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    for entry in entries.flatten() {
        if entry.file_name().to_str() == Some(target) {
            return true;
        }
        if entry.file_type().is_ok_and(|t| t.is_dir()) && has_file_named(&entry.path(), target) {
            return true;
        }
    }
    false
}

const PKG_SERVE_APP: &str = r#"(import (techne http) (acme util))
(provide fetch)
(define (fetch request env ctx)
  (let ((path (dict-ref request 'path))
        (notes (dict-ref env 'notes #f)))
    (cond ((equal? path "/put") (kv-put! notes "v" (tag-value "VER" (dict-ref env 'tag))) (response "kept"))
          ((equal? path "/get") (response (kv-get notes "v" "nothing")))
          ((equal? path "/secret") (response (dict-ref env 'key "none")))
          (else (response (tag-value "VER" (dict-ref env 'tag)))))))
"#;

/// Packaged cloud apps deploy to `techne cloud serve` with content-addressed
/// `tree-sha256` caching (`has-trees` + `<state>/trees/<sha>/`), omit cached
/// dependency trees on subsequent deploys, validate binding contracts before
/// upload, keep `.secrets.scm` out of trees, and survive rollback and restart.
#[test]
fn packaged_deploy_caches_trees_and_enforces_contracts() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    let store = tmp.path().join("store");
    let dep_repo = tmp.path().join("dep-util");
    let app_dir = tmp.path().join("web");

    write_pkg_file(
        &dep_repo,
        "techne-package.scm",
        r#"(package (acme util)
             (version "0.1.0")
             (cloud "ignored.scm" (bindings (never secret))))"#,
    );
    write_pkg_file(
        &dep_repo,
        "lib/acme/util.sld",
        r#"(define-library (acme util)
             (export tag-value)
             (import (scheme base))
             (begin
               (define (tag-value ver tag)
                 (string-append ver "/" tag))))"#,
    );
    write_pkg_file(&dep_repo, "ignored.scm", "(define (unused) #f)");
    git_init_commit(&dep_repo, "init dep");

    write_pkg_file(
        &app_dir,
        "techne-package.scm",
        &format!(
            r#"(package (demo web)
                 (version "0.1.0")
                 (depends
                   ((acme util) (local {:?})))
                 (cloud "app.scm"
                   (bindings
                     (notes kv)
                     (key secret)
                     (tag text))))"#,
            dep_repo.display().to_string()
        ),
    );
    write_pkg_file(&app_dir, "app.scm", &PKG_SERVE_APP.replace("VER", "v1"));
    write_pkg_file(&app_dir, "manifest.scm", r#"{bindings {notes (kv "notes") key (secret "KEY") tag (text "prod")}}"#);
    // Present locally initially; must never be uploaded or hashed into trees.
    write_pkg_file(&app_dir, ".secrets.scm", r#"{"KEY" "local-only"}"#);

    let lock_out = techne().args(["lock", app_dir.to_str().unwrap()]).env("TECHNE_PACKAGE_STORE", &store).output().unwrap();
    assert!(
        lock_out.status.success(),
        "techne lock failed: {}{}",
        String::from_utf8_lossy(&lock_out.stdout),
        String::from_utf8_lossy(&lock_out.stderr)
    );

    let (ok, token) = cloud(&["invite", "--state", state.to_str().unwrap(), "acme"], None);
    assert!(ok, "{token}");
    let acme = token.trim().to_string();
    let (child, port, ops, _) = serve(&state);
    let target = ["--to", ops.as_str(), "--tenant", "acme", "--project", "web"];
    let host = "web.acme.cloud.test";

    // Binding contract mismatch fails fast before uploading or creating the project.
    write_pkg_file(&app_dir, "manifest.scm", r#"{bindings {notes (kv "notes") key (text "wrong-kind") tag (text "prod")}}"#);
    let (ok, said) = cloud_with_store(&[&["deploy", app_dir.to_str().unwrap()], &target[..]].concat(), Some(&acme), &store);
    assert!(!ok && said.contains("key") && said.contains("secret") && said.contains("text"), "{said}");
    assert!(!state.join("tenants/acme/web").exists());

    // Restore valid manifest and delete local `.secrets.scm`: remote deploy uses server secret only.
    write_pkg_file(&app_dir, "manifest.scm", r#"{bindings {notes (kv "notes") key (secret "KEY") tag (text "prod")}}"#);
    std::fs::remove_file(app_dir.join(".secrets.scm")).unwrap();
    let (ok, said) = cloud_given(&[&["secret", "KEY"], &target[..]].concat(), Some(&acme), "prod-s3cret\n");
    assert!(ok, "{said}");

    // First deploy uploads both root and dep trees and caches non-root dep trees under `<state>/tenants/acme/trees/<sha>/`.
    let (ok, said) = cloud_with_store(&[&["deploy", app_dir.to_str().unwrap()], &target[..]].concat(), Some(&acme), &store);
    assert!(ok, "{said}");
    assert_eq!(get(port, host, "/"), (200, "v1/prod".into()));
    assert_eq!(get(port, host, "/secret"), (200, "prod-s3cret".into()));
    assert_eq!(get(port, host, "/put"), (200, "kept".into()));
    assert_eq!(get(port, host, "/get"), (200, "v1/prod".into()));

    let dep_files = techne_host::package::read_clean_package_tree(&dep_repo, &mut 0, 16 << 20).unwrap();
    let dep_sha = techne_host::package::canonical_tree_sha256(&dep_files);
    let root_v1_files = techne_host::package::read_clean_package_tree(&app_dir, &mut 0, 16 << 20).unwrap();
    let root_v1_sha = techne_host::package::canonical_tree_sha256(&root_v1_files);
    let acme_trees = state.join("tenants/acme/trees");
    assert!(acme_trees.join(&dep_sha).is_dir(), "expected cached dep tree {dep_sha}");
    assert!(!acme_trees.join(&root_v1_sha).exists(), "root tree must never be cached in tenant trees");
    assert!(!state.join("trees").exists(), "global trees directory must not be created");
    assert!(!has_file_named(&acme_trees, ".secrets.scm"), "secrets must never be stored in trees");

    // Second deploy after editing root `app.scm` reuses cached dep tree and serves v2.
    write_pkg_file(&app_dir, "app.scm", &PKG_SERVE_APP.replace("VER", "v2"));
    let (ok, said) = cloud_with_store(&[&["deploy", app_dir.to_str().unwrap()], &target[..]].concat(), Some(&acme), &store);
    assert!(ok, "{said}");
    assert_eq!(get(port, host, "/"), (200, "v2/prod".into()));

    // Explicitly verify `has-trees` and direct `deploy` omitting `"files"` for the cached dep.
    let uncached_sha = "0".repeat(64);
    let (status, body) = post_ops(
        &ops,
        &acme,
        "acme/web",
        &format!(r#"{{"op":"has-trees","tenant":"acme","project":"web","trees":[{},{}]}}"#, json_str(&dep_sha), json_str(&uncached_sha)),
    );
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(&format!("\"have\":[{}]", json_str(&dep_sha))), "{body}");
    assert!(body.contains(&format!("\"missing\":[{}]", json_str(&uncached_sha))), "{body}");

    // Build root v3 files and deploy with dep `"files"` omitted.
    write_pkg_file(&app_dir, "app.scm", &PKG_SERVE_APP.replace("VER", "v3"));
    let root_v3_files = techne_host::package::read_clean_package_tree(&app_dir, &mut 0, 16 << 20).unwrap();
    let root_v3_sha = techne_host::package::canonical_tree_sha256(&root_v3_files);
    let root_v3_json: Vec<String> = root_v3_files.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(&b64(v)))).collect();
    let deploy_v3 = format!(
        r#"{{"op":"deploy","tenant":"acme","project":"web","entry":"app.scm","package":{{"root":["demo","web"],"packages":[{{"owner":"demo","pkg":"web","tree_sha256":{},"depends":[["acme","util"]],"files":{{{}}}}},{{"owner":"acme","pkg":"util","tree_sha256":{},"depends":[]}}]}}}}"#,
        json_str(&root_v3_sha),
        root_v3_json.join(","),
        json_str(&dep_sha),
    );
    let (status, body) = post_ops(&ops, &acme, "acme/web", &deploy_v3);
    assert_eq!(status, 200, "{body}");
    assert_eq!(get(port, host, "/"), (200, "v3/prod".into()));

    // Forged `tree_sha256` must be rejected.
    let forged = format!(
        r#"{{"op":"deploy","tenant":"acme","project":"web","entry":"app.scm","package":{{"root":["demo","web"],"packages":[{{"owner":"demo","pkg":"web","tree_sha256":{},"depends":[["acme","util"]],"files":{{{}}}}},{{"owner":"acme","pkg":"util","tree_sha256":{},"depends":[]}}]}}}}"#,
        json_str(&"f".repeat(64)),
        root_v3_json.join(","),
        json_str(&dep_sha),
    );
    let (status, body) = post_ops(&ops, &acme, "acme/web", &forged);
    assert_eq!(status, 422, "{body}");
    assert!(body.contains("tree-sha256 mismatch"), "{body}");

    // Omitting `"files"` for an uncached tree must be rejected.
    let missing_tree = format!(
        r#"{{"op":"deploy","tenant":"acme","project":"web","entry":"app.scm","package":{{"root":["demo","web"],"packages":[{{"owner":"demo","pkg":"web","tree_sha256":{},"depends":[["acme","util"]],"files":{{{}}}}},{{"owner":"acme","pkg":"util","tree_sha256":{},"depends":[]}}]}}}}"#,
        json_str(&root_v3_sha),
        root_v3_json.join(","),
        json_str(&uncached_sha),
    );
    let (status, body) = post_ops(&ops, &acme, "acme/web", &missing_tree);
    assert_eq!(status, 422, "{body}");
    assert!(body.contains("Missing cached package tree"), "{body}");

    // Rollback restores v2, and restart restores v2 with KV intact.
    let (ok, said) = cloud(&[&["rollback"], &target[..]].concat(), Some(&acme));
    assert!(ok, "{said}");
    assert_eq!(get(port, host, "/"), (200, "v2/prod".into()));

    drop(child);
    let (_child, port, _, _) = serve(&state);
    let start = Instant::now();
    while get(port, host, "/") != (200, "v2/prod".into()) {
        assert!(start.elapsed() < PATIENCE, "{:?}", get(port, host, "/"));
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(get(port, host, "/get"), (200, "v1/prod".into()));
    assert_eq!(get(port, host, "/secret"), (200, "prod-s3cret".into()));
}

/// Multi-tenant cloud security & hardening regressions:
/// - Hosted legacy uploads NEVER consult `techne-package.scm` / `techne.lock`
///   or ancestor directories (stay strictly in legacy mode).
/// - Project name `"trees"` is reserved and rejected.
/// - Per-tenant tree caches (`tenants/<tenant>/trees/`) are isolated across
///   tenants (`has-trees` and packaged `deploy`).
/// - Rejected packaged deploys never persist uploaded dependency trees.
/// - Corrupted cached dependency trees are quarantined on `has-trees` and
///   cleanly re-uploaded on the next deploy.
/// - Combined cached + uploaded file count cannot exceed 1000 files.
#[test]
fn hosted_legacy_isolation_tenant_tree_cache_quarantine_and_combined_caps() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    let store = tmp.path().join("store");

    let invite = |tenant: &str| {
        let (ok, token) = cloud(&["invite", "--state", state.to_str().unwrap(), tenant], None);
        assert!(ok, "{token}");
        token.trim().to_string()
    };
    let (acme, bolt) = (invite("acme"), invite("bolt"));
    let (_child, port, ops, _) = serve(&state);

    // 1. Project name "trees" is reserved and rejected.
    let (status, body) = post_ops(&ops, &acme, "acme/trees", r#"{"op":"deployments","tenant":"acme","project":"trees"}"#);
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("not-a-project") || body.contains("project"), "{body}");

    // 2. Hosted legacy upload containing a crafted techne-package.scm + techne.lock
    //    referencing `../escape` stays strictly in legacy mode and succeeds.
    let crafted_legacy = format!(
        r#"{{"op":"deploy","tenant":"acme","project":"legacy-crafted","entry":"app.scm","files":{{"app.scm":{},"techne-package.scm":{},"techne.lock":{}}}}}"#,
        json_str(&b64(b"(import (techne http)) (provide fetch) (define (fetch req env ctx) (response \"legacy-isolated\"))")),
        json_str(&b64(b"(package (evil app) (version \"0.1.0\") (depends ((evil esc) (path \"../escape\"))) (cloud \"app.scm\" (bindings (never secret))))")),
        json_str(&b64(b"(lock (techne \"0.1.0\") (package (evil esc) \"0.1.0\" (source (path \"../escape\")) (tree-sha256 \"0000000000000000000000000000000000000000000000000000000000000000\")))")),
    );
    let (status, body) = post_ops(&ops, &acme, "acme/legacy-crafted", &crafted_legacy);
    assert_eq!(status, 200, "{body}");
    assert_eq!(get(port, "legacy-crafted.acme.cloud.test", "/"), (200, "legacy-isolated".into()));

    // 3. Set up a packaged app for acme and deploy it so acme caches `dep_sha`.
    let dep_repo = tmp.path().join("dep-shared");
    let app_dir = tmp.path().join("app-acme");
    write_pkg_file(&dep_repo, "techne-package.scm", r#"(package (acme shared) (version "0.1.0"))"#);
    write_pkg_file(
        &dep_repo,
        "lib/acme/shared.sld",
        r#"(define-library (acme shared) (export shared-val) (import (scheme base)) (begin (define shared-val "S1")))"#,
    );
    git_init_commit(&dep_repo, "init shared dep");

    write_pkg_file(
        &app_dir,
        "techne-package.scm",
        &format!(
            r#"(package (acme app)
                 (version "0.1.0")
                 (depends ((acme shared) (local {:?})))
                 (cloud "app.scm" (bindings (greeting text))))"#,
            dep_repo.display().to_string()
        ),
    );
    write_pkg_file(
        &app_dir,
        "app.scm",
        r#"(import (techne http) (acme shared))
(provide fetch)
(define (fetch req env ctx)
  (response (string-append (dict-ref env 'greeting) ":" shared-val)))"#,
    );
    write_pkg_file(&app_dir, "manifest.scm", r#"{bindings {greeting (text "hi")}}"#);

    let lock_out = techne().args(["lock", app_dir.to_str().unwrap()]).env("TECHNE_PACKAGE_STORE", &store).output().unwrap();
    assert!(lock_out.status.success(), "{}", String::from_utf8_lossy(&lock_out.stderr));

    let acme_target = ["--to", ops.as_str(), "--tenant", "acme", "--project", "pkg"];
    let (ok, said) = cloud_with_store(&[&["deploy", app_dir.to_str().unwrap()], &acme_target[..]].concat(), Some(&acme), &store);
    assert!(ok, "{said}");
    assert_eq!(get(port, "pkg.acme.cloud.test", "/"), (200, "hi:S1".into()));

    let dep_files = techne_host::package::read_clean_package_tree(&dep_repo, &mut 0, 16 << 20).unwrap();
    let dep_sha = techne_host::package::canonical_tree_sha256(&dep_files);
    let root_files = techne_host::package::read_clean_package_tree(&app_dir, &mut 0, 16 << 20).unwrap();
    let root_sha = techne_host::package::canonical_tree_sha256(&root_files);
    let root_json: Vec<String> = root_files.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(&b64(v)))).collect();
    let dep_json: Vec<String> = dep_files.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(&b64(v)))).collect();

    assert!(state.join("tenants/acme/trees").join(&dep_sha).is_dir());

    // 4. Cross-tenant tree cache isolation: bolt must NOT see acme's cached dep_sha
    //    via `has-trees` and cannot reference it without uploading `files`.
    let (status, body) = post_ops(
        &ops,
        &bolt,
        "bolt/pkg",
        &format!(r#"{{"op":"has-trees","tenant":"bolt","project":"pkg","trees":[{}]}}"#, json_str(&dep_sha)),
    );
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""have":[]"#), "{body}");
    assert!(body.contains(&format!(r#""missing":[{}]"#, json_str(&dep_sha))), "{body}");

    let bolt_borrow = format!(
        r#"{{"op":"deploy","tenant":"bolt","project":"pkg","entry":"app.scm","package":{{"root":["acme","app"],"packages":[{{"owner":"acme","pkg":"app","tree_sha256":{},"depends":[["acme","shared"]],"files":{{{}}}}},{{"owner":"acme","pkg":"shared","tree_sha256":{},"depends":[]}}]}}}}"#,
        json_str(&root_sha),
        root_json.join(","),
        json_str(&dep_sha),
    );
    let (status, body) = post_ops(&ops, &bolt, "bolt/pkg", &bolt_borrow);
    assert_eq!(status, 422, "{body}");
    assert!(body.contains("Missing cached package tree"), "{body}");

    // 5. Stage-before-commit: a rejected packaged deploy must NOT persist uploaded
    //    dependency trees into `<state>/tenants/bolt/trees/`.
    let mut bad_root_files = root_files.clone();
    bad_root_files.retain(|(k, _)| k != "manifest.scm");
    bad_root_files.push(("manifest.scm".into(), b"{bindings {greeting (kv \"wrong\")}}".to_vec()));
    bad_root_files.sort_by(|a, b| a.0.cmp(&b.0));
    let bad_root_sha = techne_host::package::canonical_tree_sha256(&bad_root_files);
    let bad_root_json: Vec<String> = bad_root_files.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(&b64(v)))).collect();
    let bolt_rejected = format!(
        r#"{{"op":"deploy","tenant":"bolt","project":"bad","entry":"app.scm","package":{{"root":["acme","app"],"packages":[{{"owner":"acme","pkg":"app","tree_sha256":{},"depends":[["acme","shared"]],"files":{{{}}}}},{{"owner":"acme","pkg":"shared","tree_sha256":{},"depends":[],"files":{{{}}}}}]}}}}"#,
        json_str(&bad_root_sha),
        bad_root_json.join(","),
        json_str(&dep_sha),
        dep_json.join(","),
    );
    let (status, body) = post_ops(&ops, &bolt, "bolt/bad", &bolt_rejected);
    assert_eq!(status, 422, "{body}");
    assert!(!state.join("tenants/bolt/trees").join(&dep_sha).exists(), "rejected deploy must not commit dep tree");
    assert!(!state.join("tenants/bolt/bad/source").exists(), "rejected deploy must not commit project source");
    let leftovers: Vec<_> = std::fs::read_dir(state.join("tenants/bolt/bad"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".stage-"))
        .collect();
    assert!(leftovers.is_empty(), "rejected deploy left ephemeral stage dirs behind: {leftovers:?}");

    // 6. Corrupt cached tree quarantine + recovery: corrupting a file inside
    //    acme's cached dep causes `has-trees` to delete it and report it missing;
    //    a subsequent deploy re-uploads it cleanly.
    let cached_dep_dir = state.join("tenants/acme/trees").join(&dep_sha);
    std::fs::write(cached_dep_dir.join("lib/acme/shared.sld"), "CORRUPTED").unwrap();
    let (status, body) = post_ops(
        &ops,
        &acme,
        "acme/pkg",
        &format!(r#"{{"op":"has-trees","tenant":"acme","project":"pkg","trees":[{}]}}"#, json_str(&dep_sha)),
    );
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""have":[]"#), "{body}");
    assert!(body.contains(&format!(r#""missing":[{}]"#, json_str(&dep_sha))), "{body}");
    assert!(!cached_dep_dir.exists(), "corrupted cached tree must be quarantined (deleted)");

    // Next deploy via CLI re-uploads the missing dep cleanly.
    let (ok, said) = cloud_with_store(&[&["deploy", app_dir.to_str().unwrap()], &acme_target[..]].concat(), Some(&acme), &store);
    assert!(ok, "{said}");
    assert!(cached_dep_dir.is_dir(), "re-uploaded dep tree should be cached again");
    assert_eq!(get(port, "pkg.acme.cloud.test", "/"), (200, "hi:S1".into()));

    // 7. Combined cached + uploaded file count cap (> 1000 files total rejected):
    //    Deploy a dep with 600 files (cached), then attempt a deploy referencing
    //    that cached 600-file dep alongside a 405-file root (total 1005 > 1000).
    let big_dep = tmp.path().join("big-dep");
    write_pkg_file(&big_dep, "techne-package.scm", r#"(package (acme bigdep) (version "0.1.0"))"#);
    write_pkg_file(
        &big_dep,
        "lib/acme/bigdep.sld",
        r#"(define-library (acme bigdep) (export big-ok) (import (scheme base)) (begin (define big-ok "BIG")))"#,
    );
    for i in 0..598 {
        write_pkg_file(&big_dep, &format!("d/f{i:03}.txt"), "x");
    }
    git_init_commit(&big_dep, "init big dep");

    let big_app = tmp.path().join("big-app");
    write_pkg_file(
        &big_app,
        "techne-package.scm",
        &format!(
            r#"(package (acme bigapp)
                 (version "0.1.0")
                 (depends ((acme bigdep) (local {:?})))
                 (cloud "app.scm"))"#,
            big_dep.display().to_string()
        ),
    );
    write_pkg_file(
        &big_app,
        "app.scm",
        r#"(import (techne http) (acme bigdep)) (provide fetch) (define (fetch req env ctx) (response big-ok))"#,
    );
    let lock_out = techne().args(["lock", big_app.to_str().unwrap()]).env("TECHNE_PACKAGE_STORE", &store).output().unwrap();
    assert!(lock_out.status.success(), "{}", String::from_utf8_lossy(&lock_out.stderr));

    let big_target = ["--to", ops.as_str(), "--tenant", "acme", "--project", "big"];
    let (ok, said) = cloud_with_store(&[&["deploy", big_app.to_str().unwrap()], &big_target[..]].concat(), Some(&acme), &store);
    assert!(ok, "{said}");
    assert_eq!(get(port, "big.acme.cloud.test", "/"), (200, "BIG".into()));

    let big_dep_files = techne_host::package::read_clean_package_tree(&big_dep, &mut 0, 16 << 20).unwrap();
    let big_dep_sha = techne_host::package::canonical_tree_sha256(&big_dep_files);
    assert!(state.join("tenants/acme/trees").join(&big_dep_sha).is_dir());

    // Now add 405 root files (600 cached + 408 uploaded = 1008 > 1000) and deploy omitting dep files.
    for i in 0..405 {
        write_pkg_file(&big_app, &format!("r/g{i:03}.txt"), "y");
    }
    let big_root_files = techne_host::package::read_clean_package_tree(&big_app, &mut 0, 16 << 20).unwrap();
    let big_root_sha = techne_host::package::canonical_tree_sha256(&big_root_files);
    let big_root_json: Vec<String> = big_root_files.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(&b64(v)))).collect();
    let over_cap = format!(
        r#"{{"op":"deploy","tenant":"acme","project":"big","entry":"app.scm","package":{{"root":["acme","bigapp"],"packages":[{{"owner":"acme","pkg":"bigapp","tree_sha256":{},"depends":[["acme","bigdep"]],"files":{{{}}}}},{{"owner":"acme","pkg":"bigdep","tree_sha256":{},"depends":[]}}]}}}}"#,
        json_str(&big_root_sha),
        big_root_json.join(","),
        json_str(&big_dep_sha),
    );
    let (status, body) = post_ops(&ops, &acme, "acme/big", &over_cap);
    assert_eq!(status, 422, "{body}");
    assert!(body.contains("1000 files at most"), "{body}");
}
