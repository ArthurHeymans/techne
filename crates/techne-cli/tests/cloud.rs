//! `techne cloud dev`: a deployment served over HTTP, deployed again when
//! its file is saved, and kept, serving, while a save loads and when it
//! does not load.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const APP: &str = r#"(import (techne http))
(provide fetch)
(define (fetch request env ctx)
  (json-response (dict "path" (dict-ref request 'path) "v" VERSION)))
"#;

/// How long anything here may take.
const PATIENCE: Duration = Duration::from_secs(5);

/// The body of the response to GET `path` on `port`.
fn get(port: u16, path: &str) -> String {
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(PATIENCE)).unwrap();
    write!(c, "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut answer = String::new();
    c.read_to_string(&mut answer).unwrap();
    answer.split_once("\r\n\r\n").map(|(_, body)| body.to_string()).unwrap_or(answer)
}

/// Ask `path` until the body is `want`.
fn until(port: u16, path: &str, want: &str) {
    let start = Instant::now();
    loop {
        let body = get(port, path);
        if body == want {
            return;
        }
        assert!(start.elapsed() < PATIENCE, "still {body}, not {want}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The next line of `lines` containing `text`, within `within`.
fn line_with(lines: &mpsc::Receiver<String>, text: &str, within: Duration) -> String {
    let deadline = Instant::now() + within;
    loop {
        let line = lines.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap_or_else(|_| panic!("no line with {text}"));
        if line.contains(text) {
            return line;
        }
    }
}

/// The lines `read` gives, as they come.
fn lines(read: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || BufReader::new(read).lines().map_while(Result::ok).try_for_each(|l| tx.send(l)));
    rx
}

struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn serves_and_deploys_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app.scm");
    std::fs::write(&app, APP.replace("VERSION", "1")).unwrap();
    let mut child = Killed(
        Command::new(env!("CARGO_BIN_EXE_techne"))
            .args(["cloud", "dev", app.to_str().unwrap(), "--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let out = lines(child.0.stdout.take().unwrap());
    let err = lines(child.0.stderr.take().unwrap());
    let line = line_with(&out, "Serving", PATIENCE);
    let port: u16 = line.rsplit(':').next().unwrap().parse().unwrap_or_else(|_| panic!("{line}"));
    assert_eq!(get(port, "/a"), r#"{"path":"/a","v":1}"#);
    // Saved: the next requests go to the new deployment.
    std::fs::write(&app, APP.replace("VERSION", "2")).unwrap();
    until(port, "/b", r#"{"path":"/b","v":2}"#);
    line_with(&err, "techne cloud: deployed", PATIENCE);
    // Saved with the same length and time: deployed all the same.
    let modified = std::fs::metadata(&app).unwrap().modified().unwrap();
    std::fs::write(&app, APP.replace("VERSION", "3")).unwrap();
    std::fs::File::options().write(true).open(&app).unwrap().set_modified(modified).unwrap();
    until(port, "/b", r#"{"path":"/b","v":3}"#);
    line_with(&err, "techne cloud: deployed", PATIENCE);
    // Saved looping for ever: served by the last deployment while it
    // loads, and after, once its loading is given up.
    std::fs::write(&app, "(let loop () (loop))").unwrap();
    line_with(&err, "loading", PATIENCE);
    assert_eq!(get(port, "/c"), r#"{"path":"/c","v":3}"#);
    line_with(&err, "not deployed", Duration::from_secs(15));
    assert_eq!(get(port, "/d"), r#"{"path":"/d","v":3}"#);
}

/// Send an nREPL eval of `code` (no module named: the deployment's) and
/// read its replies until done: the values.
fn nrepl_eval(c: &mut TcpStream, code: &str) -> Vec<String> {
    use techne_node::bencode::{B, dict, parse};
    let mut bytes = Vec::new();
    dict(vec![("op", B::str("eval")), ("code", B::str(code)), ("id", B::str("1"))]).encode(&mut bytes);
    c.write_all(&bytes).unwrap();
    let (mut buf, mut values) = (Vec::new(), Vec::new());
    loop {
        let mut chunk = [0u8; 4096];
        let n = c.read(&mut chunk).unwrap();
        assert!(n > 0, "the nREPL server closed the connection");
        buf.extend_from_slice(&chunk[..n]);
        while let Some((msg, used)) = parse(&buf).unwrap() {
            buf.drain(..used);
            values.extend(msg.get("value").and_then(B::as_str).map(str::to_string));
            if let Some(B::List(statuses)) = msg.get("status")
                && statuses.iter().any(|s| s.as_str() == Some("done"))
            {
                return values;
            }
        }
    }
}

const GREETER: &str = r#"(import (techne http))
(provide fetch)
(define (greeting) "GREETING")
(define (fetch request env ctx) (response (greeting)))
"#;

/// With `--nrepl`, a REPL into the dev world: what it evaluates, in the
/// deployment's module, changes the next request; deployed again on save,
/// the same connection evaluates in the new deployment's world.
#[test]
fn a_repl_into_the_dev_world() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app.scm");
    std::fs::write(&app, GREETER.replace("GREETING", "hello")).unwrap();
    let mut child = Killed(
        Command::new(env!("CARGO_BIN_EXE_techne"))
            .args(["cloud", "dev", app.to_str().unwrap(), "--port", "0", "--nrepl", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let out = lines(child.0.stdout.take().unwrap());
    let err = lines(child.0.stderr.take().unwrap());
    let (mut port, mut nrepl) = (None, None);
    while port.is_none() || nrepl.is_none() {
        let line = out.recv_timeout(PATIENCE).expect("the ports");
        if line.starts_with("Serving") {
            port = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
        } else if line.starts_with("nREPL server started on port") {
            nrepl = line.split_whitespace().nth(5).and_then(|p| p.parse::<u16>().ok());
        }
    }
    let (port, nrepl) = (port.unwrap(), nrepl.unwrap());
    assert_eq!(std::fs::read_to_string(dir.path().join(".nrepl-port")).unwrap(), nrepl.to_string());
    assert_eq!(get(port, "/"), "hello");
    let mut c = TcpStream::connect(("127.0.0.1", nrepl)).unwrap();
    c.set_read_timeout(Some(PATIENCE)).unwrap();
    nrepl_eval(&mut c, "(define (greeting) \"hi\")");
    assert_eq!(get(port, "/"), "hi");
    // Deployed again: a new world, which the same connection reaches.
    std::fs::write(&app, GREETER.replace("GREETING", "bonjour")).unwrap();
    line_with(&err, "techne cloud: deployed", PATIENCE);
    assert_eq!(get(port, "/"), "bonjour");
    assert_eq!(nrepl_eval(&mut c, "(greeting)"), ["\"bonjour\""]);
}

/// Send `fields` on `c` and read until a message has `status`.
fn nrepl_until(c: &mut TcpStream, fields: Vec<(&str, techne_node::bencode::B)>, status: &str) -> techne_node::bencode::B {
    use techne_node::bencode::{B, dict, parse};
    let mut bytes = Vec::new();
    dict(fields).encode(&mut bytes);
    c.write_all(&bytes).unwrap();
    let mut buf = Vec::new();
    loop {
        let mut chunk = [0u8; 4096];
        let n = c.read(&mut chunk).unwrap();
        assert!(n > 0, "the nREPL server closed the connection");
        buf.extend_from_slice(&chunk[..n]);
        while let Some((msg, used)) = parse(&buf).unwrap() {
            buf.drain(..used);
            if matches!(msg.get("status"), Some(B::List(l)) if l.iter().any(|s| s.as_str() == Some(status))) {
                return msg;
            }
        }
    }
}

/// A request failing in the dev world breaks into the debugger of the
/// client subscribed and waits: answered with a response made in the
/// deployment's module, or, `fetch` fixed, retried.
#[test]
fn a_failing_request_breaks_into_the_debugger() {
    use techne_node::bencode::B;
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app.scm");
    std::fs::write(&app, GREETER.replace("(greeting))", "(car (greeting)))")).unwrap();
    let mut child = Killed(
        Command::new(env!("CARGO_BIN_EXE_techne"))
            .args(["cloud", "dev", app.to_str().unwrap(), "--port", "0", "--nrepl", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let out = lines(child.0.stdout.take().unwrap());
    let (mut port, mut nrepl) = (None, None);
    while port.is_none() || nrepl.is_none() {
        let line = out.recv_timeout(PATIENCE).expect("the ports");
        if line.starts_with("Serving") {
            port = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
        } else if line.starts_with("nREPL server started on port") {
            nrepl = line.split_whitespace().nth(5).and_then(|p| p.parse::<u16>().ok());
        }
    }
    let (port, nrepl) = (port.unwrap(), nrepl.unwrap());
    let mut c = TcpStream::connect(("127.0.0.1", nrepl)).unwrap();
    c.set_read_timeout(Some(PATIENCE)).unwrap();
    nrepl_until(&mut c, vec![("op", B::str("techne-debug-subscribe")), ("id", B::str("sub"))], "techne-debug-subscribed");
    // Restart `name` of the debugger the next request opens, with `args`;
    // then its answer.
    let mut choose = |name: &str, args: &str, before: &dyn Fn(&mut TcpStream)| {
        let answer = std::thread::spawn(move || get(port, "/"));
        let debugger = nrepl_until(&mut c, Vec::new(), "techne-debug");
        assert!(debugger.get("condition").and_then(B::as_str).unwrap().contains("car"));
        let Some(B::List(restarts)) = debugger.get("restarts") else { panic!("no restarts") };
        let index = restarts.iter().position(|r| r.get("name").and_then(B::as_str) == Some(name)).unwrap();
        before(&mut c);
        let id = debugger.get("debug-id").and_then(B::as_int).unwrap();
        let restart = vec![
            ("op", B::str("techne-debug-restart")),
            ("id", B::str("2")),
            ("debug-id", B::Int(id)),
            ("restart", B::Int(index as i64)),
            ("args", B::str(args)),
        ];
        nrepl_until(&mut c, restart, "techne-debug-end");
        answer.join().unwrap()
    };
    assert_eq!(choose("use-value", "(response \"chosen\")", &|_| {}), "chosen");
    let fix = |c: &mut TcpStream| drop(nrepl_eval(c, "(define (fetch request env ctx) (response \"fixed\"))"));
    assert_eq!(choose("retry", "", &fix), "fixed");
}

/// The head and body of the response to GET `path` on `port`, sent with
/// the routing credential `credential`.
fn get_routed(port: u16, path: &str, credential: &str) -> (String, String) {
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(PATIENCE)).unwrap();
    write!(c, "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\ntechne-debug-route: {credential}\r\n\r\n").unwrap();
    let mut answer = String::new();
    c.read_to_string(&mut answer).unwrap();
    let (head, body) = answer.split_once("\r\n\r\n").unwrap_or_else(|| panic!("{answer}"));
    (head.to_string(), body.to_string())
}

/// With `--nrepl` and `--debug`, the deployment is served as in
/// production and the REPL is into a debug instance beside: what it
/// evaluates changes what requests carrying the credential are answered,
/// marked as the debug instance's, and nothing of what the others are.
#[test]
fn a_repl_into_a_debug_instance() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app.scm");
    std::fs::write(&app, GREETER.replace("GREETING", "hello")).unwrap();
    let mut child = Killed(
        Command::new(env!("CARGO_BIN_EXE_techne"))
            .args(["cloud", "dev", app.to_str().unwrap(), "--port", "0", "--nrepl", "0", "--debug"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let out = lines(child.0.stdout.take().unwrap());
    let err = lines(child.0.stderr.take().unwrap());
    let (mut port, mut nrepl, mut credential) = (None, None, None);
    while port.is_none() || nrepl.is_none() || credential.is_none() {
        let line = out.recv_timeout(PATIENCE).expect("the ports and the credential");
        if line.starts_with("Serving") {
            port = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
        } else if line.starts_with("nREPL server started on port") {
            nrepl = line.split_whitespace().nth(5).and_then(|p| p.parse::<u16>().ok());
        } else if line.starts_with("Debug instance") {
            credential = line.rsplit(' ').next().map(str::to_string);
        }
    }
    let (port, nrepl, credential) = (port.unwrap(), nrepl.unwrap(), credential.unwrap());
    let mut c = TcpStream::connect(("127.0.0.1", nrepl)).unwrap();
    c.set_read_timeout(Some(PATIENCE)).unwrap();
    nrepl_eval(&mut c, "(define (greeting) \"hi\")");
    // Redefined in the debug instance alone.
    assert_eq!(get(port, "/"), "hello");
    let (head, body) = get_routed(port, "/", &credential);
    assert_eq!(body, "hi");
    assert!(head.contains("\r\ntechne-debug: ") && head.contains("\r\ncache-control: private, no-store"), "{head}");
    // Another credential routes nothing, and marks nothing.
    let (head, body) = get_routed(port, "/", "0123456789abcdef0123456789abcdef");
    assert_eq!(body, "hello");
    // Not marked as a debug instance's, and said to vary with the
    // routing header, so that no cache answers a routed request with it.
    assert!(!head.contains("\r\ntechne-debug: ") && head.contains("\r\nvary: techne-debug-route"), "{head}");
    // Deployed again: a new debug instance, which the same connection
    // reaches, of the new deployment.
    std::fs::write(&app, GREETER.replace("GREETING", "bonjour")).unwrap();
    line_with(&err, "techne cloud: deployed", PATIENCE);
    assert_eq!(get(port, "/"), "bonjour");
    assert_eq!(get_routed(port, "/", &credential).1, "bonjour");
    nrepl_eval(&mut c, "(define (greeting) \"salut\")");
    assert_eq!(get_routed(port, "/", &credential).1, "salut");
    assert_eq!(get(port, "/"), "bonjour");
}

/// With `--auth`, a REPL client reaches the dev world only once it has
/// shown a token that holds: its evaluations, in a session of its own,
/// change the next request as before.
#[test]
fn a_repl_authenticates_with_a_token() {
    use techne_node::bencode::B;
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app.scm");
    std::fs::write(&app, GREETER.replace("GREETING", "hello")).unwrap();
    let mut child = Killed(
        Command::new(env!("CARGO_BIN_EXE_techne"))
            .args(["cloud", "dev", app.to_str().unwrap(), "--port", "0", "--nrepl", "0", "--auth"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let out = lines(child.0.stdout.take().unwrap());
    let (mut port, mut nrepl, mut token) = (None, None, None);
    while port.is_none() || nrepl.is_none() || token.is_none() {
        let line = out.recv_timeout(PATIENCE).expect("the ports and the token");
        if line.starts_with("Serving") {
            port = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
        } else if line.starts_with("nREPL server started on port") {
            nrepl = line.split_whitespace().nth(5).and_then(|p| p.parse::<u16>().ok());
        } else if let Some(t) = line.strip_prefix("Token: ") {
            token = Some(t.to_string());
        }
    }
    let (port, nrepl, token) = (port.unwrap(), nrepl.unwrap(), token.unwrap());
    let connect = || {
        let c = TcpStream::connect(("127.0.0.1", nrepl)).unwrap();
        c.set_read_timeout(Some(PATIENCE)).unwrap();
        c
    };
    let refused = |msg: &B| matches!(msg.get("status"), Some(B::List(l)) if l.iter().any(|s| s.as_str() == Some("unauthenticated")));
    // Not authenticated: nothing is evaluated, and the connection ends.
    let mut c = connect();
    let eval = vec![("op", B::str("eval")), ("code", B::str("(define (greeting) \"pwned\")")), ("id", B::str("1"))];
    assert!(refused(&nrepl_until(&mut c, eval, "done")));
    assert_eq!(c.read(&mut [0u8; 16]).unwrap(), 0);
    // Nor with a token that is none, or one changed.
    for bad in ["nothing", &format!("{token}x")] {
        let mut c = connect();
        assert!(refused(&nrepl_until(&mut c, vec![("op", B::str("authenticate")), ("token", B::str(bad)), ("id", B::str("1"))], "done")));
    }
    // Nor with the token in a first message past what one may be: closed,
    // unanswered.
    let mut c = connect();
    let mut long = Vec::new();
    let pad = "x".repeat(9000);
    techne_node::bencode::dict(vec![("op", B::str("authenticate")), ("token", B::str(&token)), ("pad", B::str(&pad))]).encode(&mut long);
    c.write_all(&long).unwrap();
    assert_eq!(c.read(&mut [0u8; 16]).unwrap_or(0), 0);
    assert_eq!(get(port, "/"), "hello");
    // Authenticated: a session of its own, and what it evaluates serves.
    let mut c = connect();
    assert!(!refused(&nrepl_until(&mut c, vec![("op", B::str("authenticate")), ("token", B::str(&token)), ("id", B::str("1"))], "done")));
    let cloned = nrepl_until(&mut c, vec![("op", B::str("clone")), ("id", B::str("2"))], "done");
    let session = cloned.get("new-session").and_then(B::as_str).unwrap().to_string();
    let eval =
        vec![("op", B::str("eval")), ("code", B::str("(define (greeting) \"hi\")")), ("session", B::str(&session)), ("id", B::str("3"))];
    nrepl_until(&mut c, eval, "done");
    assert_eq!(get(port, "/"), "hi");
    // The key is kept, not the token.
    assert!(dir.path().join(".techne/token-key").is_file());
}

const KEEPER: &str = r#"(import (techne http))
(provide fetch)
(define (fetch request env ctx)
  (let ((path (dict-ref request 'path))
        (notes (dict-ref env 'notes #f)))
    (cond ((equal? path "/put") (kv-put! notes "greeting" (dict "said" (dict-ref env 'greeting))) (response "kept"))
          ((equal? path "/get") (json-response (kv-get notes "greeting" "nothing")))
          ((equal? path "/keys") (json-response (kv-list notes)))
          ((equal? path "/secret") (response (dict-ref env 'key)))
          ((equal? path "/forged")
           (response (guard (e (#t (condition/report-string e)))
                       (kv-get (dict 'kind 'kv 'name "other") "greeting"))))
          (else (response "?")))))
"#;

/// `techne cloud dev` on `app`: the process, and the port it serves on.
fn serving(app: &std::path::Path) -> (Killed, u16) {
    let mut child = Killed(
        Command::new(env!("CARGO_BIN_EXE_techne"))
            .args(["cloud", "dev", app.to_str().unwrap(), "--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let out = lines(child.0.stdout.take().unwrap());
    let line = line_with(&out, "Serving", PATIENCE);
    let port = line.rsplit(':').next().unwrap().parse().unwrap_or_else(|_| panic!("{line}"));
    (child, port)
}

/// A deployment is given the bindings its manifest names: a secret's
/// value, a text, and a KV namespace, which the supervisor keeps: what a
/// request kept there another reads, also after `techne cloud dev` is
/// started again. A binding the deployment was not given reaches
/// nothing.
#[test]
fn bindings_from_the_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app.scm");
    std::fs::write(&app, KEEPER).unwrap();
    std::fs::write(dir.path().join("manifest.scm"), r#"{bindings {notes (kv "notes") key (secret "KEY") greeting (text "hello")}}"#)
        .unwrap();
    std::fs::write(dir.path().join(".secrets.scm"), r#"{"KEY" "s3cret"}"#).unwrap();
    let (child, port) = serving(&app);
    assert_eq!(get(port, "/secret"), "s3cret");
    assert_eq!(get(port, "/get"), r#""nothing""#);
    assert_eq!(get(port, "/put"), "kept");
    assert_eq!(get(port, "/get"), r#"{"said":"hello"}"#);
    assert_eq!(get(port, "/keys"), r#"["greeting"]"#);
    let forged = get(port, "/forged");
    assert!(forged.contains("no KV binding other"), "{forged}");
    // Kept by the supervisor, in a file: there for the next.
    drop(child);
    assert!(dir.path().join(".techne/kv/notes.kv").is_file());
    let (_child, port) = serving(&app);
    assert_eq!(get(port, "/get"), r#"{"said":"hello"}"#);
}

/// `techne cloud` asked for `args` about `app`, with `token` if given in
/// place of the one kept: whether it succeeded, and what it said.
fn cloud(args: &[&str], app: &std::path::Path, token: Option<&str>) -> (bool, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_techne"));
    command.arg("cloud").arg(args[0]).arg(app).args(&args[1..]).env_remove("TECHNE_TOKEN");
    if let Some(token) = token {
        command.env("TECHNE_TOKEN", token);
    }
    let out = command.output().unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// The ids in `answer`, a `deployments` reply, the latest first, and the
/// one published.
fn deployments(answer: &str) -> (Vec<String>, String) {
    let hex = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
    let after =
        |key: &str| answer.split(key).skip(1).filter_map(|rest| rest.split('"').next()).filter(|id| hex(id)).map(str::to_string).collect();
    let ids: Vec<String> = after("\"id\":\"");
    let current: Vec<String> = after("\"current\":\"");
    (ids, current.first().cloned().unwrap_or_default())
}

/// Deployments are recorded and their bundles kept: `techne cloud
/// deployments` lists them, `techne cloud rollback` publishes the one
/// before again, its code and what it was bound to, and the record
/// outlives `techne cloud dev`. Without a token that holds, nothing.
#[test]
fn deployments_are_listed_and_rolled_back() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app.scm");
    std::fs::write(&app, APP.replace("VERSION", "1")).unwrap();
    let serve = || {
        let mut child = Killed(
            Command::new(env!("CARGO_BIN_EXE_techne"))
                .args(["cloud", "dev", app.to_str().unwrap(), "--port", "0", "--ops", "0"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let out = lines(child.0.stdout.take().unwrap());
        let (mut port, mut ops) = (None, false);
        while port.is_none() || !ops {
            let line = out.recv_timeout(PATIENCE).expect("the ports");
            if line.starts_with("Serving") {
                port = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
            }
            ops |= line.starts_with("Operations on");
        }
        (child, port.unwrap())
    };
    let (child, port) = serve();
    assert_eq!(get(port, "/a"), r#"{"path":"/a","v":1}"#);
    let (ok, listed) = cloud(&["deployments"], &app, None);
    assert!(ok, "{listed}");
    let (ids, current) = deployments(&listed);
    assert_eq!((ids.len(), &ids[0]), (1, &current), "{listed}");
    let first = current;
    // Nothing to go back to yet; and nothing without a token that holds.
    assert!(!cloud(&["rollback"], &app, None).0);
    let (ok, refused) = cloud(&["deployments"], &app, Some("tk1.nothing.at-all"));
    assert!(!ok && refused.contains("unauthenticated"), "{refused}");
    // Saved: another deployment, the one before kept.
    std::fs::write(&app, APP.replace("VERSION", "2")).unwrap();
    until(port, "/b", r#"{"path":"/b","v":2}"#);
    let (ids, second) = deployments(&cloud(&["deployments"], &app, None).1);
    assert_eq!(ids, [second.clone(), first.clone()]);
    // Rolled back to the one before: its code serves again.
    let (ok, rolled) = cloud(&["rollback"], &app, None);
    assert!(ok && rolled.contains(&first), "{rolled}");
    assert_eq!(get(port, "/c"), r#"{"path":"/c","v":1}"#);
    assert_eq!(deployments(&cloud(&["deployments"], &app, None).1), (vec![first.clone(), second.clone()], first.clone()));
    // And forward again, by id; one that is not there is refused.
    assert!(cloud(&["rollback", &second], &app, None).0);
    assert_eq!(get(port, "/d"), r#"{"path":"/d","v":2}"#);
    let (ok, missing) = cloud(&["rollback", &"0".repeat(64)], &app, None);
    assert!(!ok && missing.contains("No bundle is kept"), "{missing}");
    assert_eq!(get(port, "/d"), r#"{"path":"/d","v":2}"#);
    // Deployed as it is: the one published already.
    let (ok, same) = cloud(&["deploy"], &app, None);
    assert!(ok && same.contains(&second), "{same}");
    // A bundle kept that is none, or larger than one may be, is refused,
    // and a record that cannot be read: nothing published changes.
    let kept = dir.path().join(".techne/bundles").join(format!("{first}.bundle"));
    let bundle = std::fs::read(&kept).unwrap();
    std::fs::write(&kept, vec![7u8; 33 << 20]).unwrap();
    let (ok, large) = cloud(&["rollback", &first], &app, None);
    assert!(!ok && large.contains("too large"), "{large}");
    std::fs::write(&kept, b"junk").unwrap();
    assert!(!cloud(&["rollback", &first], &app, None).0);
    std::fs::write(&kept, bundle).unwrap();
    let record = dir.path().join(".techne/deployments.scm");
    let recorded = std::fs::read(&record).unwrap();
    std::fs::write(&record, b"((").unwrap();
    let (ok, unread) = cloud(&["rollback", &first], &app, None);
    assert!(!ok && unread.contains("cannot be read"), "{unread}");
    assert_eq!(get(port, "/d"), r#"{"path":"/d","v":2}"#);
    std::fs::write(&record, recorded).unwrap();
    // The record outlives the supervisor.
    drop(child);
    let (_child, port) = serve();
    assert_eq!(get(port, "/e"), r#"{"path":"/e","v":2}"#);
    let (ids, current) = deployments(&cloud(&["deployments"], &app, None).1);
    assert_eq!((ids, current), (vec![second.clone(), first.clone()], second));
    assert!(cloud(&["rollback", &first], &app, None).0);
    assert_eq!(get(port, "/f"), r#"{"path":"/f","v":1}"#);
}

/// A manifest naming a secret that is not there deploys nothing.
#[test]
fn a_manifest_that_cannot_be_bound_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app.scm");
    std::fs::write(&app, KEEPER).unwrap();
    std::fs::write(dir.path().join("manifest.scm"), r#"{bindings {key (secret "KEY")}}"#).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_techne")).args(["cloud", "dev", app.to_str().unwrap(), "--port", "0"]).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("No such secret") && err.contains("KEY"), "{err}");
}

const ECHO: &str = r#"(import (techne http))
(provide fetch)
(define (fetch request env ctx)
  (if (equal? (dict-ref request 'path) "/text")
      (string-append "got " (request-text request))
      (let ((body (dict-ref request 'body)))
        (response (lambda (write)
                    (let loop ()
                      (let ((b (body)))
                        (unless (eof-object? b) (write b) (loop)))))))))
"#;

/// The head and body of the response to POST `body` to `path` on `port`,
/// a chunked body put together.
fn post(port: u16, path: &str, body: &[u8]) -> (String, Vec<u8>) {
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(PATIENCE)).unwrap();
    write!(c, "POST {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
    c.write_all(body).unwrap();
    let mut answer = Vec::new();
    c.read_to_end(&mut answer).unwrap();
    let at = answer.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&answer[..at]).into_owned();
    let mut rest = &answer[at + 4..];
    if !head.contains("transfer-encoding: chunked") {
        return (head, rest.to_vec());
    }
    let mut body = Vec::new();
    loop {
        let line = rest.windows(2).position(|w| w == b"\r\n").unwrap();
        let n = usize::from_str_radix(std::str::from_utf8(&rest[..line]).unwrap(), 16).unwrap();
        if n == 0 {
            return (head, body);
        }
        body.extend_from_slice(&rest[line + 2..line + 2 + n]);
        rest = &rest[line + 4 + n..];
    }
}

#[test]
fn bodies_stream_through_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app.scm");
    std::fs::write(&app, ECHO).unwrap();
    let mut child = Killed(
        Command::new(env!("CARGO_BIN_EXE_techne"))
            .args(["cloud", "dev", app.to_str().unwrap(), "--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let out = lines(child.0.stdout.take().unwrap());
    let line = line_with(&out, "Serving", PATIENCE);
    let port: u16 = line.rsplit(':').next().unwrap().parse().unwrap_or_else(|_| panic!("{line}"));
    assert_eq!(post(port, "/text", b"hello").1, b"got hello");
    let sent: Vec<u8> = (0..500_000u32).map(|i| (i % 251) as u8).collect();
    let (head, body) = post(port, "/echo", &sent);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, sent);
}

fn git_init_commit(dir: &std::path::Path, msg: &str) {
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

fn write_pkg_file(root: &std::path::Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

const PACKAGED_APP: &str = r#"(import (techne http) (acme util))
(provide fetch)
(define (label) "LABEL")
(define (fetch request env ctx)
  (let ((path (dict-ref request 'path))
        (notes (dict-ref env 'notes #f)))
    (cond ((equal? path "/put") (kv-put! notes "k" (format-greeting (label) (dict-ref env 'greeting))) (response "kept"))
          ((equal? path "/get") (response (kv-get notes "k" "nothing")))
          ((equal? path "/secret") (response (dict-ref env 'key)))
          (else (response (format-greeting (label) (dict-ref env 'greeting)))))))
"#;

/// A packaged cloud app (directory with `techne-package.scm` + `techne.lock`)
/// serves under `techne cloud dev` with locked dependencies, binding contract
/// enforcement, KV storage inside the package directory, nREPL evaluation in
/// the virtual entry module, hot-redeploy on save, and `--ops` rollback.
#[test]
fn packaged_cloud_dev_with_locked_deps_nrepl_and_ops() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let dep_repo = tmp.path().join("dep-util");
    let app_dir = tmp.path().join("app");

    // Dependency package also declares its own `(cloud ...)` clause — must be ignored.
    write_pkg_file(
        &dep_repo,
        "techne-package.scm",
        r#"(package (acme util)
             (version "0.1.0")
             (cloud "ignored.scm" (bindings (unused secret))))"#,
    );
    write_pkg_file(
        &dep_repo,
        "lib/acme/util.sld",
        r#"(define-library (acme util)
             (export format-greeting)
             (import (scheme base))
             (begin
               (define (format-greeting label msg)
                 (string-append label ":" msg))))"#,
    );
    write_pkg_file(&dep_repo, "ignored.scm", "(define (unused) #f)");
    git_init_commit(&dep_repo, "init dep");

    write_pkg_file(
        &app_dir,
        "techne-package.scm",
        &format!(
            r#"(package (demo app)
                 (version "0.1.0")
                 (depends
                   ((acme util) (local {:?})))
                 (cloud "app.scm"
                   (bindings
                     (notes kv)
                     (key secret)
                     (greeting text))))"#,
            dep_repo.display().to_string()
        ),
    );
    write_pkg_file(&app_dir, "app.scm", &PACKAGED_APP.replace("LABEL", "v1"));
    write_pkg_file(&app_dir, "manifest.scm", r#"{bindings {notes (kv "notes") key (secret "KEY") greeting (text "hello")}}"#);
    write_pkg_file(&app_dir, ".secrets.scm", r#"{"KEY" "dev-secret"}"#);

    // Lock and hydrate into the temporary store.
    let lock_out = Command::new(env!("CARGO_BIN_EXE_techne"))
        .args(["lock", app_dir.to_str().unwrap()])
        .env("TECHNE_PACKAGE_STORE", &store)
        .output()
        .unwrap();
    assert!(
        lock_out.status.success(),
        "techne lock failed: {}{}",
        String::from_utf8_lossy(&lock_out.stdout),
        String::from_utf8_lossy(&lock_out.stderr)
    );

    let mut child = Killed(
        Command::new(env!("CARGO_BIN_EXE_techne"))
            .args(["cloud", "dev", app_dir.to_str().unwrap(), "--port", "0", "--nrepl", "0", "--ops", "0"])
            .env("TECHNE_PACKAGE_STORE", &store)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let out = lines(child.0.stdout.take().unwrap());
    let err = lines(child.0.stderr.take().unwrap());
    let (mut port, mut nrepl, mut ops) = (None, None, false);
    while port.is_none() || nrepl.is_none() || !ops {
        let line = out.recv_timeout(PATIENCE).expect("the ports");
        if line.starts_with("Serving") {
            port = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
        } else if line.starts_with("nREPL server started on port") {
            nrepl = line.split_whitespace().nth(5).and_then(|p| p.parse::<u16>().ok());
        } else if line.starts_with("Operations on") {
            ops = true;
        }
    }
    let (port, nrepl) = (port.unwrap(), nrepl.unwrap());
    assert_eq!(std::fs::read_to_string(app_dir.join(".nrepl-port")).unwrap(), nrepl.to_string());
    assert_eq!(get(port, "/"), "v1:hello");
    assert_eq!(get(port, "/secret"), "dev-secret");
    assert_eq!(get(port, "/put"), "kept");
    assert_eq!(get(port, "/get"), "v1:hello");
    assert!(app_dir.join(".techne/kv/notes.kv").is_file());

    // nREPL evaluates inside the virtual entry module (`/pkg/demo/app/app.scm`).
    let mut c = TcpStream::connect(("127.0.0.1", nrepl)).unwrap();
    c.set_read_timeout(Some(PATIENCE)).unwrap();
    nrepl_eval(&mut c, r#"(define (label) "repl")"#);
    assert_eq!(get(port, "/"), "repl:hello");

    // Save v2 -> hot-redeployed.
    write_pkg_file(&app_dir, "app.scm", &PACKAGED_APP.replace("LABEL", "v2"));
    until(port, "/", "v2:hello");
    line_with(&err, "techne cloud: deployed", PATIENCE);

    // Ops subcommands accept the package directory directly.
    let (ok, listed) = cloud(&["deployments"], &app_dir, None);
    assert!(ok, "{listed}");
    let (ids, current) = deployments(&listed);
    assert_eq!(ids.len(), 2, "{listed}");
    assert_eq!(ids[0], current);

    let (ok, rolled) = cloud(&["rollback"], &app_dir, None);
    assert!(ok && rolled.contains(&ids[1]), "{rolled}");
    assert_eq!(get(port, "/"), "v1:hello");
}

/// A packaged cloud app whose `manifest.scm` violates the root package's
/// `(cloud ... (bindings ...))` contract is refused before serving.
#[test]
fn packaged_cloud_dev_rejects_binding_contract_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let app_dir = tmp.path().join("bad-app");
    write_pkg_file(
        &app_dir,
        "techne-package.scm",
        r#"(package (demo bad)
             (version "0.1.0")
             (cloud "app.scm" (bindings (key secret))))"#,
    );
    write_pkg_file(&app_dir, "app.scm", &APP.replace("VERSION", "1"));
    write_pkg_file(&app_dir, "manifest.scm", r#"{bindings {key (text "wrong-kind")}}"#);

    let out = Command::new(env!("CARGO_BIN_EXE_techne")).args(["cloud", "dev", app_dir.to_str().unwrap(), "--port", "0"]).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("key") && err.contains("secret") && err.contains("text"), "{err}");
}

/// Mode 2 live library patching via `techne cloud patch-library`:
/// - refuses production-only `cloud dev` (no `--nrepl`) with `no-mutable-world`,
/// - patches a dev world's imported package library via file and `--code` while
///   preserving mutable state,
/// - refuses structural changes with typed errors without disturbing live state,
/// - and resets cleanly on a fresh Mode 1 deploy.
#[test]
fn cloud_patch_library_dev_world_and_production_refusal() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let dep_repo = tmp.path().join("dep-util");
    let app_dir = tmp.path().join("app");

    write_pkg_file(&dep_repo, "techne-package.scm", r#"(package (acme util) (version "0.1.0"))"#);
    let util_v1 = r#"(define-library (acme util)
  (export next-val)
  (import (techne))
  (begin
    (define counter (vector 0))
    (define (next-val)
      (vector-set! counter 0 (+ (vector-ref counter 0) 1))
      (string-append "v1#" (number->string (vector-ref counter 0))))))"#;
    write_pkg_file(&dep_repo, "lib/acme/util.sld", util_v1);
    git_init_commit(&dep_repo, "init dep");

    write_pkg_file(
        &app_dir,
        "techne-package.scm",
        &format!(
            r#"(package (demo app)
                 (version "0.1.0")
                 (depends ((acme util) (local {:?})))
                 (cloud "app.scm"))"#,
            dep_repo.display().to_string()
        ),
    );
    let app_src = r#"(import (techne http) (acme util))
(provide fetch)
(define (fetch request env ctx)
  (response (next-val)))
"#;
    write_pkg_file(&app_dir, "app.scm", app_src);

    let lock_out = Command::new(env!("CARGO_BIN_EXE_techne"))
        .args(["lock", app_dir.to_str().unwrap()])
        .env("TECHNE_PACKAGE_STORE", &store)
        .output()
        .unwrap();
    assert!(lock_out.status.success(), "{}{}", String::from_utf8_lossy(&lock_out.stdout), String::from_utf8_lossy(&lock_out.stderr));

    let util_v2 = util_v1.replace("\"v1#\"", "\"v2#\"");
    let util_v3 = util_v1.replace("\"v1#\"", "\"v3#\"");
    let util_bad = util_v1.replace("(vector 0)", "(vector 99)");
    let patch_file = tmp.path().join("util-v2.sld");
    std::fs::write(&patch_file, &util_v2).unwrap();

    // 1. Production-only (`cloud dev` without `--nrepl`): patch-library is refused.
    {
        let mut child = Killed(
            Command::new(env!("CARGO_BIN_EXE_techne"))
                .args(["cloud", "dev", app_dir.to_str().unwrap(), "--port", "0", "--ops", "0"])
                .env("TECHNE_PACKAGE_STORE", &store)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let out = lines(child.0.stdout.take().unwrap());
        let (mut port, mut ops) = (None, false);
        while port.is_none() || !ops {
            let line = out.recv_timeout(PATIENCE).expect("the ports");
            if line.starts_with("Serving") {
                port = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
            } else if line.starts_with("Operations on") {
                ops = true;
            }
        }
        let port = port.unwrap();
        assert_eq!(get(port, "/"), "v1#1");

        let (ok, reply) = cloud(&["patch-library", "(acme util)", patch_file.to_str().unwrap()], &app_dir, None);
        assert!(!ok && reply.contains("no-mutable-world"), "{reply}");
        assert!(get(port, "/").starts_with("v1#"));
    }

    // 2. Dev world (`cloud dev --nrepl 0 --ops 0`): patch-library succeeds and preserves state.
    {
        let mut child = Killed(
            Command::new(env!("CARGO_BIN_EXE_techne"))
                .args(["cloud", "dev", app_dir.to_str().unwrap(), "--port", "0", "--nrepl", "0", "--ops", "0"])
                .env("TECHNE_PACKAGE_STORE", &store)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let out = lines(child.0.stdout.take().unwrap());
        let err = lines(child.0.stderr.take().unwrap());
        let (mut port, mut nrepl, mut ops) = (None, None, false);
        while port.is_none() || nrepl.is_none() || !ops {
            let line = out.recv_timeout(PATIENCE).expect("the ports");
            if line.starts_with("Serving") {
                port = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
            } else if line.starts_with("nREPL server started on port") {
                nrepl = line.split_whitespace().nth(5).and_then(|p| p.parse::<u16>().ok());
            } else if line.starts_with("Operations on") {
                ops = true;
            }
        }
        let port = port.unwrap();
        assert_eq!(get(port, "/"), "v1#1");

        // Patch from file path.
        let (ok, reply) = cloud(&["patch-library", "(acme util)", patch_file.to_str().unwrap()], &app_dir, None);
        assert!(ok, "{reply}");
        assert!(reply.contains("\"dirty\":true") && reply.contains("\"next-val\""), "{reply}");
        assert_eq!(get(port, "/"), "v2#2");

        // Patch inline via --code and --no-check-retained.
        let (ok, reply) = cloud(&["patch-library", "(acme util)", "--code", &util_v3, "--no-check-retained"], &app_dir, None);
        assert!(ok, "{reply}");
        assert_eq!(get(port, "/"), "v3#3");

        // Structural change is refused cleanly without mutating live world.
        let (ok, reply) = cloud(&["patch-library", "(acme util)", "--code", &util_bad], &app_dir, None);
        assert!(!ok, "{reply}");
        assert!(reply.contains("patch-refused") && reply.contains("state-initializer-changed"), "{reply}");
        assert_eq!(get(port, "/"), "v3#4");

        // Mode 1 redeploy resets the dev world cleanly back to the disk snapshot.
        write_pkg_file(&app_dir, "app.scm", &format!("; redeploy\n{app_src}"));
        line_with(&err, "techne cloud: deployed", PATIENCE);
        assert_eq!(get(port, "/"), "v1#1");
    }
}

/// With `--nrepl --debug --ops`, `techne cloud patch-library` patches only the
/// attached debug instance; unrouted production requests keep serving the
/// immutable snapshot untouched.
#[test]
fn cloud_patch_library_isolates_debug_instance_from_production() {
    let tmp = tempfile::tempdir().unwrap();
    let app_dir = tmp.path().join("app");

    write_pkg_file(&app_dir, "techne-package.scm", r#"(package (demo app) (version "0.1.0") (cloud "app.scm"))"#);
    let lib_v1 = r#"(define-library (demo app util)
  (export tag)
  (import (techne))
  (begin
    (define counter (vector 0))
    (define (tag)
      (vector-set! counter 0 (+ (vector-ref counter 0) 1))
      (string-append "prod#" (number->string (vector-ref counter 0))))))"#;
    write_pkg_file(&app_dir, "lib/demo/app/util.sld", lib_v1);
    write_pkg_file(
        &app_dir,
        "app.scm",
        r#"(import (techne http) (demo app util))
(provide fetch)
(define (fetch request env ctx)
  (response (tag)))
"#,
    );

    let mut child = Killed(
        Command::new(env!("CARGO_BIN_EXE_techne"))
            .args(["cloud", "dev", app_dir.to_str().unwrap(), "--port", "0", "--nrepl", "0", "--debug", "--ops", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let out = lines(child.0.stdout.take().unwrap());
    let (mut port, mut credential, mut ops) = (None, None, false);
    while port.is_none() || credential.is_none() || !ops {
        let line = out.recv_timeout(PATIENCE).expect("the ports and credential");
        if line.starts_with("Serving") {
            port = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
        } else if line.starts_with("Debug instance") {
            credential = line.rsplit(' ').next().map(str::to_string);
        } else if line.starts_with("Operations on") {
            ops = true;
        }
    }
    let (port, credential) = (port.unwrap(), credential.unwrap());

    assert_eq!(get(port, "/"), "prod#1");
    assert_eq!(get_routed(port, "/", &credential).1, "prod#1");

    let lib_v2 = lib_v1.replace("\"prod#\"", "\"debug#\"");
    let (ok, reply) = cloud(&["patch-library", "(demo app util)", "--code", &lib_v2], &app_dir, None);
    assert!(ok, "{reply}");
    assert!(reply.contains("\"dirty\":true"), "{reply}");

    // Routed debug request uses patched procedure with preserved debug-instance counter.
    assert_eq!(get_routed(port, "/", &credential).1, "debug#2");
    // Unrouted production request still runs from the untouched immutable snapshot.
    assert!(get(port, "/").starts_with("prod#"));
}

/// A failing HTTP request calling an imported library pauses in the debugger;
/// patching that library over nREPL (`techne-patch-library`) and invoking
/// `retry` completes the in-flight HTTP request with the fixed response.
#[test]
fn cloud_dev_paused_debugger_retry_after_nrepl_patch_library() {
    use techne_node::bencode::B;
    let tmp = tempfile::tempdir().unwrap();
    let app_dir = tmp.path().join("app");

    write_pkg_file(&app_dir, "techne-package.scm", r#"(package (demo app) (version "0.1.0") (cloud "app.scm"))"#);
    let lib_v1 = r#"(define-library (demo app buggy)
  (export answer)
  (import (techne))
  (begin
    (define (answer x) (car x))))"#;
    write_pkg_file(&app_dir, "lib/demo/app/buggy.sld", lib_v1);
    write_pkg_file(
        &app_dir,
        "app.scm",
        r#"(import (techne http) (demo app buggy))
(provide fetch)
(define (fetch request env ctx)
  (response (answer 41)))
"#,
    );

    let mut child = Killed(
        Command::new(env!("CARGO_BIN_EXE_techne"))
            .args(["cloud", "dev", app_dir.to_str().unwrap(), "--port", "0", "--nrepl", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let out = lines(child.0.stdout.take().unwrap());
    let (mut port, mut nrepl) = (None, None);
    while port.is_none() || nrepl.is_none() {
        let line = out.recv_timeout(PATIENCE).expect("the ports");
        if line.starts_with("Serving") {
            port = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
        } else if line.starts_with("nREPL server started on port") {
            nrepl = line.split_whitespace().nth(5).and_then(|p| p.parse::<u16>().ok());
        }
    }
    let (port, nrepl) = (port.unwrap(), nrepl.unwrap());

    let mut c = TcpStream::connect(("127.0.0.1", nrepl)).unwrap();
    c.set_read_timeout(Some(PATIENCE)).unwrap();
    nrepl_until(&mut c, vec![("op", B::str("techne-debug-subscribe")), ("id", B::str("sub"))], "techne-debug-subscribed");

    let answer = std::thread::spawn(move || get(port, "/"));
    let debugger = nrepl_until(&mut c, Vec::new(), "techne-debug");
    let Some(B::List(restarts)) = debugger.get("restarts") else { panic!("no restarts") };
    let retry_idx = restarts.iter().position(|r| r.get("name").and_then(B::as_str) == Some("retry")).unwrap();
    let debug_id = debugger.get("debug-id").and_then(B::as_int).unwrap();

    // Patch `(demo app buggy)` while the HTTP request is paused in the debugger.
    let lib_fixed = r#"(define-library (demo app buggy)
  (export answer)
  (import (techne))
  (begin
    (define (answer x) (string-append "patched:" (number->string (+ x 1))))))"#;
    let patched = nrepl_until(
        &mut c,
        vec![
            ("op", B::str("techne-patch-library")),
            ("id", B::str("patch1")),
            ("library", B::str("(demo app buggy)")),
            ("code", B::str(lib_fixed)),
        ],
        "done",
    );
    assert_eq!(patched.get("patched"), Some(&B::List(vec![B::str("answer")])), "{patched:?}");

    // Retry the paused request and verify it finishes with the patched library response.
    nrepl_until(
        &mut c,
        vec![
            ("op", B::str("techne-debug-restart")),
            ("id", B::str("retry1")),
            ("debug-id", B::Int(debug_id)),
            ("restart", B::Int(retry_idx as i64)),
            ("args", B::str("")),
        ],
        "techne-debug-end",
    );
    assert_eq!(answer.join().unwrap(), "patched:42");
}

fn mint_scoped_token(key_dir: &std::path::Path, project: &str, scopes: &[&str]) -> String {
    let mut vm = techne_vm::vm::Vm::new();
    let cloud = techne_vm::library::lisp_dir().join("cloud");
    let scopes_scm: String = scopes.iter().map(|s| techne_vm::reader::string_repr(s)).collect::<Vec<_>>().join(" ");
    let val = vm
        .eval_source(&format!(
            r#"(require {} {})
               (let ((k (token-key-in {})))
                 (make-token k "tester"
                             #:projects (list {})
                             #:scopes (list {scopes_scm})
                             #:expires (+ (current-second) 3600)))"#,
            techne_vm::reader::string_repr(&cloud.join("auth.scm").display().to_string()),
            techne_vm::reader::string_repr(&cloud.join("tokens.scm").display().to_string()),
            techne_vm::reader::string_repr(&key_dir.display().to_string()),
            techne_vm::reader::string_repr(project),
        ))
        .unwrap();
    vm.get::<String>(&val).unwrap()
}

fn post_dev_ops(ops_url: &str, token: &str, body: &str) -> (u16, String) {
    let addr = ops_url.strip_prefix("http://").unwrap();
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(PATIENCE)).unwrap();
    write!(
        c,
        "POST /v1/ops HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = String::new();
    c.read_to_string(&mut answer).unwrap();
    let status = answer.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let resp_body = answer.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, resp_body)
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

/// Regression tests for Phase 4 hardening in `techne cloud dev`:
/// - No-op `patch-library` reports `"dirty":false`.
/// - Redeploying identical bundle bytes (same deployment ID) resets a `dirty`
///   patched dev world cleanly back to the unpatched snapshot!
/// - Rolling back to a cached earlier dev generation resets a `dirty` patched
///   dev world cleanly back to the unpatched snapshot!
/// - Under `--debug`, `/v1/ops` `patch-library` refuses `deploy`-only and
///   `eval`-only tokens with `403`, and succeeds with a `prod`-scoped token.
#[test]
fn cloud_dev_patch_dirty_reset_on_identical_redeploy_and_rollback_and_debug_prod_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let app_dir = tmp.path().join("app");
    write_pkg_file(&app_dir, "techne-package.scm", r#"(package (demo app) (version "0.1.0") (cloud "app.scm"))"#);
    let lib_v1 = r#"(define-library (demo app util)
  (export label)
  (import (techne))
  (begin
    (define counter (vector 0))
    (define (label)
      (vector-set! counter 0 (+ (vector-ref counter 0) 1))
      (string-append "v1#" (number->string (vector-ref counter 0))))))"#;
    let lib_patched = lib_v1.replace("\"v1#\"", "\"patched#\"");
    let app_v1 = r#"(import (techne http) (demo app util))
(provide fetch)
(define (fetch req env ctx) (response (label)))
"#;
    let app_v2 = r#"(import (techne http) (demo app util))
(provide fetch)
(define (fetch req env ctx) (response (string-append "gen2:" (label))))
"#;
    write_pkg_file(&app_dir, "lib/demo/app/util.sld", lib_v1);
    write_pkg_file(&app_dir, "app.scm", app_v1);

    // Part A: Dev-only mode (`--nrepl 0 --ops 0`)
    {
        let mut child = Killed(
            Command::new(env!("CARGO_BIN_EXE_techne"))
                .args(["cloud", "dev", app_dir.to_str().unwrap(), "--port", "0", "--nrepl", "0", "--ops", "0"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let out = lines(child.0.stdout.take().unwrap());
        let (mut port, mut ops_url) = (None, None);
        while port.is_none() || ops_url.is_none() {
            let line = out.recv_timeout(PATIENCE).expect("the ports");
            if line.starts_with("Serving") {
                port = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
            } else if let Some(url) = line.strip_prefix("Operations on ") {
                ops_url = Some(url.trim().to_string());
            }
        }
        let (port, ops_url) = (port.unwrap(), ops_url.unwrap());
        assert_eq!(get(port, "/"), "v1#1");

        let eval_tok = mint_scoped_token(&app_dir.join(".techne"), "dev", &["eval"]);
        let deploy_tok = mint_scoped_token(&app_dir.join(".techne"), "dev", &["deploy"]);

        // 1. No-op patch via eval-only token succeeds and reports `"dirty":false`.
        let noop_body =
            format!(r#"{{"op":"patch-library","library":"(demo app util)","code":{}}}"#, json_str(&format!("; noop\n{lib_v1}")));
        let (status, reply) = post_dev_ops(&ops_url, &eval_tok, &noop_body);
        assert_eq!(status, 200, "{reply}");
        assert!(reply.contains("\"dirty\":false"), "{reply}");

        // 2. Real patch makes dev world dirty (`patched#2`).
        let patch_body = format!(r#"{{"op":"patch-library","library":"(demo app util)","code":{}}}"#, json_str(&lib_patched));
        let (status, reply) = post_dev_ops(&ops_url, &eval_tok, &patch_body);
        assert_eq!(status, 200, "{reply}");
        assert!(reply.contains("\"dirty\":true"), "{reply}");
        assert_eq!(get(port, "/"), "patched#2");

        // 3. Redeploying IDENTICAL bundle bytes (same deployment ID!) resets the dirty dev world back to clean `v1#1`!
        let (status, reply) = post_dev_ops(&ops_url, &deploy_tok, r#"{"op":"deploy"}"#);
        assert_eq!(status, 200, "{reply}");
        assert_eq!(get(port, "/"), "v1#1");

        // 4. Deploy gen2, patch gen2 so it is dirty (`gen2:patched#2`), then rollback to cached gen1 -> clean `v1#1`!
        write_pkg_file(&app_dir, "app.scm", app_v2);
        let (status, reply) = post_dev_ops(&ops_url, &deploy_tok, r#"{"op":"deploy"}"#);
        assert_eq!(status, 200, "{reply}");
        assert_eq!(get(port, "/"), "gen2:v1#1");

        let (status, reply) = post_dev_ops(&ops_url, &eval_tok, &patch_body);
        assert_eq!(status, 200, "{reply}");
        assert!(reply.contains("\"dirty\":true"), "{reply}");
        assert_eq!(get(port, "/"), "gen2:patched#2");

        let (status, reply) = post_dev_ops(&ops_url, &deploy_tok, r#"{"op":"rollback"}"#);
        assert_eq!(status, 200, "{reply}");
        assert_eq!(get(port, "/"), "v1#1");
    }

    // Part B: `--debug` mode (`--nrepl 0 --debug --ops 0`) requires `prod` scope for `patch-library`.
    {
        write_pkg_file(&app_dir, "app.scm", app_v1);
        let mut child = Killed(
            Command::new(env!("CARGO_BIN_EXE_techne"))
                .args(["cloud", "dev", app_dir.to_str().unwrap(), "--port", "0", "--nrepl", "0", "--debug", "--ops", "0"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let out = lines(child.0.stdout.take().unwrap());
        let (mut port, mut ops_url, mut credential) = (None, None, None);
        while port.is_none() || ops_url.is_none() || credential.is_none() {
            let line = out.recv_timeout(PATIENCE).expect("the ports and credential");
            if line.starts_with("Serving") {
                port = line.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
            } else if let Some(url) = line.strip_prefix("Operations on ") {
                ops_url = Some(url.trim().to_string());
            } else if line.starts_with("Debug instance") {
                credential = line.rsplit(' ').next().map(str::to_string);
            }
        }
        let (port, ops_url, credential) = (port.unwrap(), ops_url.unwrap(), credential.unwrap());

        let deploy_only = mint_scoped_token(&app_dir.join(".techne"), "dev", &["deploy"]);
        let eval_only = mint_scoped_token(&app_dir.join(".techne"), "dev", &["eval"]);
        let prod_only = mint_scoped_token(&app_dir.join(".techne"), "dev", &["prod"]);

        let patch_body = format!(r#"{{"op":"patch-library","library":"(demo app util)","code":{}}}"#, json_str(&lib_patched));

        // Deploy-only and eval-only tokens MUST be refused with 403!
        let (status, reply) = post_dev_ops(&ops_url, &deploy_only, &patch_body);
        assert_eq!(status, 403, "deploy-only token must be refused under --debug: {reply}");
        let (status, reply) = post_dev_ops(&ops_url, &eval_only, &patch_body);
        assert_eq!(status, 403, "eval-only token must be refused under --debug: {reply}");

        // Token with `prod` scope succeeds (200) and patches the debug instance alone.
        let (status, reply) = post_dev_ops(&ops_url, &prod_only, &patch_body);
        assert_eq!(status, 200, "{reply}");
        assert!(reply.contains("\"dirty\":true"), "{reply}");
        assert_eq!(get_routed(port, "/", &credential).1, "patched#1");
        assert!(get(port, "/").starts_with("v1#"));
    }
}
