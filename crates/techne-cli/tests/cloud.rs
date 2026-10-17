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
