//! The nREPL server, driven over TCP like an editor would.

use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use techne_node::bencode::{B, dict, parse};

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn server() -> Server {
    let dir = std::env::temp_dir().join(format!("techne-nrepl-{}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    let child =
        Command::new(env!("CARGO_BIN_EXE_techne-node")).args(["--nrepl", "0"]).current_dir(&dir).stdout(Stdio::piped()).spawn().unwrap();
    // Owned at once, so a failing check below still kills it.
    let mut server = Server { child, port: 0 };
    let mut line = String::new();
    BufReader::new(server.child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    // "nREPL server started on port N on host H - nrepl://H:N"
    server.port = line.split_whitespace().nth(5).unwrap().parse().unwrap();
    assert_eq!(std::fs::read_to_string(dir.join(".nrepl-port")).unwrap(), server.port.to_string());
    server
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

struct Client {
    stream: TcpStream,
    buf: Vec<u8>,
    pending: VecDeque<B>,
    next: u64,
    session: String,
}

impl Client {
    fn connect(server: &Server) -> Client {
        let stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut c = Client { stream, buf: Vec::new(), pending: VecDeque::new(), next: 1, session: String::new() };
        let replies = c.request(vec![("op", B::str("clone"))]);
        c.session = replies[0].get("new-session").unwrap().as_str().unwrap().to_string();
        c
    }

    /// Send a message (with this client's session unless one is given);
    /// returns its id.
    fn send(&mut self, mut fields: Vec<(&str, B)>) -> String {
        let id = self.next.to_string();
        self.next += 1;
        fields.push(("id", B::str(&id)));
        if !self.session.is_empty() && !fields.iter().any(|(k, _)| *k == "session") {
            fields.push(("session", B::str(&self.session)));
        }
        let mut bytes = Vec::new();
        dict(fields).encode(&mut bytes);
        self.stream.write_all(&bytes).unwrap();
        id
    }

    fn read_message(&mut self) -> B {
        loop {
            if let Some((msg, used)) = parse(&self.buf).unwrap() {
                self.buf.drain(..used);
                return msg;
            }
            let mut chunk = [0u8; 65536];
            let n = self.stream.read(&mut chunk).expect("a reply in time");
            assert!(n > 0, "server closed the connection");
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    /// The next message for `id`.
    fn next_for(&mut self, id: &str) -> B {
        if let Some(i) = self.pending.iter().position(|m| m.get("id").and_then(B::as_str) == Some(id)) {
            return self.pending.remove(i).unwrap();
        }
        loop {
            let msg = self.read_message();
            if msg.get("id").and_then(B::as_str) == Some(id) {
                return msg;
            }
            self.pending.push_back(msg);
        }
    }

    /// Messages for `id` up to and including the one whose status has `done`.
    fn until_done(&mut self, id: &str) -> Vec<B> {
        let mut out = Vec::new();
        loop {
            let msg = self.next_for(id);
            let done = statuses(&msg).contains(&"done".to_string());
            out.push(msg);
            if done {
                return out;
            }
        }
    }

    fn request(&mut self, fields: Vec<(&str, B)>) -> Vec<B> {
        let id = self.send(fields);
        self.until_done(&id)
    }

    fn eval(&mut self, code: &str) -> Vec<B> {
        self.request(vec![("op", B::str("eval")), ("code", B::str(code))])
    }
}

fn statuses(msg: &B) -> Vec<String> {
    match msg.get("status") {
        Some(B::List(l)) => l.iter().filter_map(|s| s.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

/// All values of `key` in `replies`, as strings.
fn field(replies: &[B], key: &str) -> Vec<String> {
    replies.iter().filter_map(|m| m.get(key).and_then(B::as_str).map(str::to_string)).collect()
}

fn has_status(replies: &[B], status: &str) -> bool {
    replies.iter().any(|m| statuses(m).iter().any(|s| s == status))
}

#[test]
fn standard_operations() {
    let server = server();
    let mut c = Client::connect(&server);
    let described = c.request(vec![("op", B::str("describe"))]);
    let ops = described[0].get("ops").unwrap();
    for op in ["eval", "interrupt", "completions", "lookup", "techne-debug-restart", "techne-inspect"] {
        assert!(ops.get(op).is_some(), "describe lists {op}");
    }
    // Values, printed output, *1.
    let r = c.eval("(display \"hi\") (+ 1 2)");
    assert_eq!(field(&r, "out"), ["hi"]);
    assert_eq!(field(&r, "value"), ["3"]);
    assert_eq!(field(&c.eval("(list *1 \"s\")"), "value"), [r#"(3 "s")"#]);
    // Errors: message and frames, ex, eval-error.
    let r = c.eval("(define (inner x) (car x))\n(define (outer y) (list (inner y)))\n(outer 5)");
    let err = field(&r, "err").concat();
    assert!(err.contains("car: expected pair, got 5") && err.contains("in inner") && err.contains("in outer"), "{err}");
    assert!(has_status(&r, "eval-error") && field(&r, "ex") == ["error"]);
    assert_eq!(field(&c.eval("(error-object-message *e)"), "value"), [r#""car: expected pair, got 5""#]);
    // Completions, info, eldoc, lookup with the client's file and line (the
    // column is that of the definition's signature).
    let r = c.request(vec![
        ("op", B::str("eval")),
        ("code", B::str("(define (area w h) \"Area of a W by H rectangle.\" (* w h))")),
        ("file", B::str("/src/shapes.scm")),
        ("line", B::Int(10)),
        ("column", B::Int(3)),
    ]);
    assert!(has_status(&r, "done") && !has_status(&r, "eval-error"));
    let r = c.request(vec![("op", B::str("completions")), ("prefix", B::str("string-app"))]);
    let Some(B::List(cands)) = r[0].get("completions") else { panic!("{r:?}") };
    assert!(
        cands
            .iter()
            .any(|c| c.get("candidate").and_then(B::as_str) == Some("string-append")
                && c.get("type").and_then(B::as_str) == Some("function"))
    );
    let r = c.request(vec![("op", B::str("info")), ("sym", B::str("area"))]);
    assert_eq!(field(&r, "arglists-str"), ["(w h)"]);
    assert_eq!(field(&r, "doc"), ["Area of a W by H rectangle."]);
    assert_eq!(field(&r, "file"), ["/src/shapes.scm"]);
    assert_eq!((r[0].get("line").and_then(B::as_int), r[0].get("column").and_then(B::as_int)), (Some(10), Some(11)));
    let r = c.request(vec![("op", B::str("eldoc")), ("sym", B::str("area"))]);
    assert_eq!(r[0].get("eldoc"), Some(&B::List(vec![B::List(vec![B::str("w"), B::str("h")])])));
    let r = c.request(vec![("op", B::str("lookup")), ("sym", B::str("when"))]);
    assert!(r[0].get("info").and_then(|i| i.get("doc")).and_then(B::as_str).unwrap().contains("special form"));
    assert!(has_status(&c.request(vec![("op", B::str("info")), ("sym", B::str("no-such-thing"))]), "no-info"));
    // load-file names the file in definitions.
    let r = c.request(vec![
        ("op", B::str("load-file")),
        ("file", B::str("(define x 1)\n\n(define (g) \"G.\" x)\n'loaded")),
        ("file-path", B::str("/src/g.scm")),
    ]);
    assert_eq!(field(&r, "value"), ["loaded"]);
    let r = c.request(vec![("op", B::str("lookup")), ("sym", B::str("g"))]);
    let info = r[0].get("info").unwrap();
    assert_eq!((info.get("file").and_then(B::as_str), info.get("line").and_then(B::as_int)), (Some("/src/g.scm"), Some(3)));
    assert!(has_status(&c.request(vec![("op", B::str("frobnicate"))]), "unknown-op"));
    assert!(has_status(&c.request(vec![("op", B::str("eval")), ("code", B::str("1")), ("session", B::str("nope"))]), "unknown-session"));
}

#[test]
fn output_streams_and_interrupts() {
    let server = server();
    let mut c = Client::connect(&server);
    // Output arrives while the evaluation is still running.
    let start = Instant::now();
    let id = c.send(vec![("op", B::str("eval")), ("code", B::str("(display \"early\") (sleep 400) (display \"late\") 'end"))]);
    let first = c.next_for(&id);
    assert_eq!(first.get("out").and_then(B::as_str), Some("early"));
    assert!(start.elapsed() < Duration::from_millis(300), "streamed, not buffered: {:?}", start.elapsed());
    let rest = c.until_done(&id);
    assert_eq!(field(&rest, "out"), ["late"]);
    assert_eq!(field(&rest, "value"), ["end"]);
    // Interrupting a runaway evaluation.
    let id = c.send(vec![("op", B::str("eval")), ("code", B::str("(let loop () (loop))"))]);
    std::thread::sleep(Duration::from_millis(100));
    let r = c.request(vec![("op", B::str("interrupt")), ("interrupt-id", B::str(&id))]);
    assert!(has_status(&r, "done") && !has_status(&r, "session-idle"));
    let r = c.until_done(&id);
    assert!(has_status(&r, "interrupted"), "{r:?}");
    assert_eq!(field(&c.eval("(+ 40 2)"), "value"), ["42"]);
    assert!(has_status(&c.request(vec![("op", B::str("interrupt"))]), "session-idle"));
}

fn debug_eval(c: &mut Client, code: &str) -> (String, B) {
    let id = c.send(vec![("op", B::str("eval")), ("code", B::str(code)), ("techne-debug", B::Int(1))]);
    let paused = c.next_for(&id);
    assert!(statuses(&paused).contains(&"techne-debug".to_string()), "{paused:?}");
    (id, paused)
}

#[test]
fn debugger_with_restarts() {
    let server = server();
    let mut c = Client::connect(&server);
    let code = "(define (risky x) (restart-case (+ 1 (error \"boom\" x)) (use-value (v) v) (skip () 'skipped)))\n(list (risky 5) 'after)";
    // Without the flag an error is an error.
    assert!(has_status(&c.eval(code), "eval-error"));
    // With it, the evaluation pauses at the error with its restarts.
    let (id, paused) = debug_eval(&mut c, code);
    assert_eq!(paused.get("condition").and_then(B::as_str), Some("boom 5"));
    let Some(B::List(restarts)) = paused.get("restarts") else { panic!() };
    let described: Vec<(String, String)> = restarts
        .iter()
        .map(|r| (r.get("name").and_then(B::as_str).unwrap().into(), r.get("params").and_then(B::as_str).unwrap().into()))
        .collect();
    assert_eq!(described, [("use-value".to_string(), "v".to_string()), ("skip".into(), String::new())]);
    let Some(B::List(frames)) = paused.get("frames") else { panic!() };
    assert!(frames.iter().any(|f| f.as_str().unwrap().starts_with("risky")), "{frames:?}");
    // While paused: evaluation and inspection still work.
    assert_eq!(field(&c.eval("(* 6 7)"), "value"), ["42"]);
    let r = c.request(vec![("op", B::str("techne-inspect")), ("code", B::str("(vector 1 2)"))]);
    assert_eq!(field(&r, "title"), ["vector of 2 elements"]);
    // Choose use-value with an argument: the evaluation continues.
    let debug_id = paused.get("debug-id").unwrap().clone();
    let r = c.request(vec![
        ("op", B::str("techne-debug-restart")),
        ("debug-id", debug_id.clone()),
        ("restart", B::Int(0)),
        ("args", B::str("(* 10 4)")),
    ]);
    assert!(has_status(&r, "done"));
    let r = c.until_done(&id);
    assert_eq!(field(&r, "value"), ["(40 after)"]);
    // A stale debugger id is refused.
    let r = c.request(vec![("op", B::str("techne-debug-abort")), ("debug-id", debug_id)]);
    assert!(has_status(&r, "no-debugger"));
    // Abort: the error comes back as an ordinary eval error.
    let (id, paused) = debug_eval(&mut c, "(list (risky 6))");
    c.request(vec![("op", B::str("techne-debug-abort")), ("debug-id", paused.get("debug-id").unwrap().clone())]);
    let r = c.until_done(&id);
    assert!(has_status(&r, "eval-error") && field(&r, "err").concat().contains("boom 6"), "{r:?}");
    // Interrupt also aborts a paused evaluation.
    let (id, _) = debug_eval(&mut c, "(risky 7)");
    c.request(vec![("op", B::str("interrupt")), ("interrupt-id", B::str(&id))]);
    assert!(has_status(&c.until_done(&id), "eval-error"));
    // Errors without restarts do not pause.
    let r = c.request(vec![("op", B::str("eval")), ("code", B::str("(car 1)")), ("techne-debug", B::Int(1))]);
    assert!(has_status(&r, "eval-error"));
    // A client that disappears while paused does not block the server.
    let mut gone = Client::connect(&server);
    debug_eval(&mut gone, "(risky 8)");
    drop(gone);
    assert_eq!(field(&c.eval("(+ 1 1)"), "value"), ["2"]);
}

#[test]
fn inspector() {
    let server = server();
    let mut c = Client::connect(&server);
    c.eval("(define-record-type point (make-point x y) point? (x point-x) (y point-y))");
    let inspect = |c: &mut Client, fields: Vec<(&str, B)>| {
        let r = c.request(fields);
        let parts = match r[0].get("parts") {
            Some(B::List(ps)) => ps
                .iter()
                .map(|p| match p {
                    B::List(lv) => (lv[0].as_str().unwrap().to_string(), lv[1].as_str().unwrap().to_string()),
                    _ => panic!(),
                })
                .collect(),
            _ => Vec::new(),
        };
        (field(&r, "title").concat(), parts, r[0].get("depth").and_then(B::as_int))
    };
    let (title, parts, depth) =
        inspect(&mut c, vec![("op", B::str("techne-inspect")), ("code", B::str("(list (make-point 1 2) (vector 'a \"b\"))"))]);
    assert_eq!((title.as_str(), depth), ("list of 2 elements", Some(1)));
    assert_eq!(parts[1], ("1".to_string(), "#(a \"b\")".to_string()));
    let (title, parts, depth) = inspect(&mut c, vec![("op", B::str("techne-inspect-part")), ("index", B::Int(0))]);
    assert_eq!((title.as_str(), depth), ("record point", Some(2)));
    assert_eq!(parts, [("x".to_string(), "1".to_string()), ("y".to_string(), "2".to_string())]);
    let (title, _, depth) = inspect(&mut c, vec![("op", B::str("techne-inspect-pop"))]);
    assert_eq!((title.as_str(), depth), ("list of 2 elements", Some(1)));
    // Procedures show their signature, documentation and captured values.
    c.eval("(define (adder n) \"Add N.\" (lambda (x) (+ x n)))");
    let (title, parts, _) = inspect(&mut c, vec![("op", B::str("techne-inspect")), ("code", B::str("adder"))]);
    assert_eq!(title, "procedure adder");
    assert!(
        parts.contains(&("parameters".to_string(), "\"(n)\"".to_string()))
            && parts.contains(&("documentation".to_string(), "\"Add N.\"".to_string()))
    );
    let (_, parts, _) = inspect(&mut c, vec![("op", B::str("techne-inspect")), ("code", B::str("(adder 5)"))]);
    assert!(parts.contains(&("captured 1".to_string(), "5".to_string())), "{parts:?}");
    // Hash tables by entry.
    let (title, parts, _) = inspect(
        &mut c,
        vec![("op", B::str("techne-inspect")), ("code", B::str("(let ((h (make-hash-table))) (hash-table-set! h 'k 9) h)"))],
    );
    assert_eq!((title.as_str(), parts), ("hash table of 1 entries", vec![("k".to_string(), "9".to_string())]));
}

#[test]
fn sessions_in_modules() {
    let dir = std::env::temp_dir().join(format!("techne-nrepl-modules-{}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.scm"), "(define (greet) \"Greeting of a.\" \"a\")").unwrap();
    std::fs::write(dir.join("b.scm"), "(define (greet) \"b\")").unwrap();
    let a = dir.join("a.scm").canonicalize().unwrap().to_string_lossy().into_owned();
    let b = dir.join("b.scm").canonicalize().unwrap().to_string_lossy().into_owned();
    let server = server();
    let mut one = Client::connect(&server);
    let mut two = Client::connect(&server);
    let eval_ns =
        |c: &mut Client, code: &str, ns: &str| c.request(vec![("op", B::str("eval")), ("code", B::str(code)), ("ns", B::str(ns))]);

    // An `ns` names a file's module (loaded on first use); replies say where.
    let r = eval_ns(&mut one, "(greet)", &a);
    assert_eq!((field(&r, "value"), field(&r, "ns")), (vec!["\"a\"".to_string()], vec![a.clone()]));
    assert_eq!(field(&eval_ns(&mut two, "(greet)", &b), "value"), ["\"b\""]);
    // Each session stays in its module: redefining in one leaves the other.
    one.eval("(define (greet) \"Second greeting of a.\" \"a2\")");
    assert_eq!(field(&one.eval("(greet)"), "value"), ["\"a2\""]);
    assert_eq!(field(&two.eval("(greet)"), "value"), ["\"b\""]);

    // Completion and lookup follow the session's module, or an `ns`.
    let complete = |c: &mut Client, ns: Option<&str>| {
        let mut req = vec![("op", B::str("completions")), ("prefix", B::str("gree"))];
        if let Some(ns) = ns {
            req.push(("ns", B::str(ns)));
        }
        let r = c.request(req);
        match r[0].get("completions") {
            Some(B::List(l)) => l.iter().filter_map(|d| d.get("candidate").and_then(B::as_str).map(str::to_string)).collect::<Vec<_>>(),
            _ => panic!("{r:?}"),
        }
    };
    assert_eq!(complete(&mut one, None), ["greet"]);
    let mut three = Client::connect(&server);
    assert!(complete(&mut three, None).is_empty(), "greet is not in user");
    assert_eq!(complete(&mut three, Some(&b)), ["greet"]);
    let info = two.request(vec![("op", B::str("info")), ("sym", B::str("greet"))]);
    assert_eq!(field(&info, "file"), [b.clone()]);
    let info = three.request(vec![("op", B::str("eldoc")), ("sym", B::str("greet")), ("ns", B::str(&a))]);
    assert_eq!(field(&info, "docstring"), ["Second greeting of a."]);

    // in-module switches a session for later evaluations.
    let r = three.eval(&format!("(in-module {a:?})"));
    assert!(has_status(&r, "done") && !has_status(&r, "error"), "{r:?}");
    let r = three.eval("(greet)");
    assert_eq!((field(&r, "value"), field(&r, "ns")), (vec!["\"a2\"".to_string()], vec![a.clone()]));
    let r = eval_ns(&mut three, "1", "no-such-module.scm");
    assert!(has_status(&r, "namespace-not-found"), "{r:?}");
}

#[test]
fn exit_does_not_end_the_server() {
    let server = server();
    let mut c = Client::connect(&server);
    let r = c.eval("(exit 3)");
    assert!(field(&r, "err").iter().any(|e| e.contains("not granted")), "{r:?}");
    assert_eq!(field(&c.eval("(+ 1 2)"), "value"), ["3"]);
}
