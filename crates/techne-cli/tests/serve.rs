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
