//! The nREPL server serving clients for principals
//! (`Remote::serve_stream_as`): what one has, sessions and debuggers, is
//! not there for another.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    time::Duration,
};

use techne_node::{
    bencode::{B, dict, parse},
    nrepl::{self, Remote},
};

struct Client {
    stream: TcpStream,
    buf: Vec<u8>,
    next: u64,
}

impl Client {
    /// A client of `remote`, served for `principal`.
    fn connect(remote: &Remote, principal: Option<&str>) -> Client {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (served, _) = listener.accept().unwrap();
        served.set_nonblocking(true).unwrap();
        let _runtime = techne_process::runtime().enter();
        remote.serve_stream_as(tokio::net::TcpStream::from_std(served).unwrap(), principal.map(str::to_string));
        stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        Client { stream, buf: Vec::new(), next: 1 }
    }

    /// Send `fields`, and read what is answered until a message has
    /// `status`: the messages, that one last.
    fn ask(&mut self, mut fields: Vec<(&str, B)>, status: &str) -> Vec<B> {
        let mut answered = Vec::new();
        let id = self.next.to_string();
        self.next += 1;
        fields.push(("id", B::str(&id)));
        let mut bytes = Vec::new();
        dict(fields).encode(&mut bytes);
        self.stream.write_all(&bytes).unwrap();
        loop {
            while let Some((msg, used)) = parse(&self.buf).unwrap() {
                self.buf.drain(..used);
                // What an earlier request is still answered is not this
                // one's.
                if msg.get("id").and_then(B::as_str) != Some(id.as_str()) {
                    continue;
                }
                let last = statuses(&msg).iter().any(|s| s == status);
                answered.push(msg);
                if last {
                    return answered;
                }
            }
            let mut chunk = [0u8; 65536];
            let n = self.stream.read(&mut chunk).expect("a reply in time");
            assert!(n > 0, "the server closed the connection");
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    fn session(&mut self) -> String {
        self.ask(vec![("op", B::str("clone"))], "done")[0].get("new-session").unwrap().as_str().unwrap().to_string()
    }
}

fn statuses(msg: &B) -> Vec<String> {
    match msg.get("status") {
        Some(B::List(l)) => l.iter().filter_map(|s| s.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

fn has(answered: &[B], status: &str) -> bool {
    answered.iter().any(|msg| statuses(msg).iter().any(|s| s == status))
}

#[test]
fn what_a_principal_has_is_not_there_for_another() {
    let remote = nrepl::start().unwrap();
    let mut alice = Client::connect(&remote, Some("alice"));
    let mut bob = Client::connect(&remote, Some("bob"));
    let mut anyone = Client::connect(&remote, None);
    // For a principal, nothing without a session.
    let refused = alice.ask(vec![("op", B::str("eval")), ("code", B::str("1"))], "done");
    assert!(has(&refused, "no-session"), "{refused:?}");
    let (hers, his) = (alice.session(), bob.session());
    let eval = |c: &mut Client, session: &str, code: &str, status: &str| {
        c.ask(vec![("op", B::str("eval")), ("code", B::str(code)), ("session", B::str(session)), ("techne-debug", B::Int(2))], status)
    };
    assert!(!has(&eval(&mut alice, &hers, "(+ 1 2)", "done"), "error"));
    // Another's session is unknown, to a principal and to no one.
    assert!(has(&eval(&mut bob, &hers, "1", "done"), "unknown-session"));
    assert!(has(&eval(&mut anyone, &hers, "1", "done"), "unknown-session"));
    let listed = bob.ask(vec![("op", B::str("ls-sessions"))], "done");
    let Some(B::List(listed)) = listed.iter().find_map(|m| m.get("sessions")) else { panic!("{listed:?}") };
    assert_eq!(listed.iter().filter_map(B::as_str).collect::<Vec<_>>(), [his.as_str()]);
    // Her debugger is hers to answer and to look into.
    let debugger = eval(&mut alice, &hers, "(car 1)", "techne-debug");
    let id = debugger.last().and_then(|m| m.get("debug-id")).and_then(B::as_int).unwrap();
    let on = |op: &str, session: &str| vec![("op", B::str(op)), ("debug-id", B::Int(id)), ("session", B::str(session))];
    assert!(has(&bob.ask(on("techne-debug-abort", &his), "done"), "no-debugger"));
    assert!(has(&bob.ask(on("techne-debug-inspect", &his), "done"), "no-debugger"));
    assert!(has(&anyone.ask(vec![("op", B::str("techne-debug-abort")), ("debug-id", B::Int(id))], "done"), "no-debugger"));
    assert!(!has(&alice.ask(on("techne-debug-abort", &hers), "done"), "error"));
    // A bounded number of sessions each.
    for _ in 1..16 {
        alice.session();
    }
    assert!(has(&alice.ask(vec![("op", B::str("clone"))], "done"), "too-many-sessions"));
    bob.session();
}
