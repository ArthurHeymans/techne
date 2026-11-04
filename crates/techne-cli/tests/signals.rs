//! Signals asking `techne run` and `techne repl` to stop: a script unwinds,
//! its cleanup running, and ends as the signal ended it; the REPL ends at
//! once on SIGTERM. Nothing here waits for ever: a child that does not
//! answer in time fails the test.
#![cfg(unix)]

use std::{
    io::{BufRead, BufReader, Read},
    process::{Child, ChildStdout, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// How long a child has to answer.
const PATIENCE: Duration = Duration::from_secs(10);

/// What OUT says, line by line (the last one perhaps without its line
/// break), read on a thread of its own.
fn lines(out: ChildStdout) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = BufReader::new(out);
        loop {
            let mut line = String::new();
            if out.read_line(&mut line).unwrap_or(0) == 0 || tx.send(line).is_err() {
                return;
            }
        }
    });
    rx
}

/// The next line from LINES, CHILD killed and the test failed if none
/// comes in time.
fn next_line(lines: &mpsc::Receiver<String>, child: &mut Child) -> String {
    lines.recv_timeout(PATIENCE).unwrap_or_else(|e| {
        let _ = child.kill();
        let _ = child.wait();
        panic!("no line from the child: {e}");
    })
}

const SCRIPT: &str = r#"(dynamic-wind (lambda () #f)
  (lambda () (display "started\n") (flush-output-port) (sleep 60000))
  (lambda () (display "cleaned up\n") (flush-output-port)))
"#;

fn signal(child: &Child, name: &str) {
    assert!(Command::new("kill").args([&format!("-{name}"), &child.id().to_string()]).status().unwrap().success());
}

/// Wait for CHILD's exit, at most a while.
fn exit_code(child: &mut Child) -> Option<i32> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code();
        }
        assert!(start.elapsed() < Duration::from_secs(10), "still running");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_script_asked_to_stop_cleans_up_and_ends_as_the_signal_did() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("stop.scm");
    std::fs::write(&script, SCRIPT).unwrap();
    for (name, code) in [("TERM", 143), ("HUP", 129), ("INT", 130)] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_techne"))
            .arg("run")
            .arg(&script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let out = lines(child.stdout.take().unwrap());
        assert_eq!(next_line(&out, &mut child), "started\n");
        signal(&child, name);
        assert_eq!(exit_code(&mut child), Some(code), "{name}");
        assert_eq!(next_line(&out, &mut child), "cleaned up\n", "{name}");
        let mut err = String::new();
        child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
        assert_eq!(err, "", "the interruption is not an error to report");
    }
}

#[test]
fn the_repl_ends_at_once() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_techne"))
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Once it is reading: its prompt, which comes after the signals are
    // watched.
    let mut out = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut prompt = [0u8; "λ> ".len()];
        let _ = tx.send(out.read_exact(&mut prompt).map(|()| prompt));
    });
    let prompt = rx.recv_timeout(PATIENCE).ok().and_then(Result::ok);
    if prompt.is_none() {
        let _ = child.kill();
    }
    assert_eq!(prompt.as_ref().map(|p| &p[..]), Some("λ> ".as_bytes()));
    signal(&child, "TERM");
    assert_eq!(exit_code(&mut child), Some(143));
}

#[test]
fn a_script_that_catches_the_signal_ends_as_it_chooses() {
    let dir = tempfile::tempdir().unwrap();
    for (body, code, said) in [
        // Its own exit, said by nobody.
        ("(guard (e (#t (exit 3))) (sleep 60000))", Some(3), ""),
        // Carrying on to the end: success.
        ("(guard (e (#t #f)) (sleep 60000))", Some(0), ""),
    ] {
        let script = dir.path().join("catch.scm");
        std::fs::write(&script, format!("(display \"started\\n\") (flush-output-port)\n{body}\n")).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_techne"))
            .arg("run")
            .arg(&script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let out = lines(child.stdout.take().unwrap());
        assert_eq!(next_line(&out, &mut child), "started\n");
        signal(&child, "TERM");
        assert_eq!(exit_code(&mut child), code, "{body}");
        let mut err = String::new();
        child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
        assert_eq!(err, said, "{body}");
    }
}

#[test]
fn a_second_signal_ends_the_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("slow.scm");
    std::fs::write(
        &script,
        r#"(dynamic-wind (lambda () #f)
  (lambda () (display "started\n") (flush-output-port) (sleep 60000))
  (lambda () (display "cleaning\n") (flush-output-port) (sleep 60000)))
"#,
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_techne")).arg("run").arg(&script).stdout(Stdio::piped()).spawn().unwrap();
    let out = lines(child.stdout.take().unwrap());
    assert_eq!(next_line(&out, &mut child), "started\n");
    signal(&child, "TERM");
    assert_eq!(next_line(&out, &mut child), "cleaning\n");
    signal(&child, "TERM");
    assert_eq!(exit_code(&mut child), Some(143));
}
