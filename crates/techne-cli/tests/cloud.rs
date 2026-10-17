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
