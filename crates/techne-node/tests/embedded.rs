//! The nREPL service installed in a world its owner drives (`Server`):
//! requests wake the owner, evaluations run in the owner's slices and
//! are reported after them, and they reach only what the world was
//! granted.

use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use techne_node::{
    bencode::{B, dict, parse},
    nrepl::Server,
};
use techne_vm::vm::{Grants, Vm};

/// Send `fields` and read replies until one says done, the owner driving
/// `vm` meanwhile; the replies' `value`s and `err`s.
fn ask(c: &mut TcpStream, vm: &mut Vm, server: &Server, fields: Vec<(&str, B)>) -> (Vec<String>, Vec<String>) {
    let mut bytes = Vec::new();
    dict(fields).encode(&mut bytes);
    c.write_all(&bytes).unwrap();
    let (mut values, mut errs, mut buf) = (Vec::new(), Vec::new(), Vec::new());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        server.poll(vm);
        vm.run_tasks_for(Duration::from_millis(5));
        server.reap(vm);
        let mut chunk = [0u8; 4096];
        match c.read(&mut chunk) {
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => panic!("{e}"),
        }
        while let Some((msg, used)) = parse(&buf).unwrap() {
            buf.drain(..used);
            let text = |k: &str| msg.get(k).and_then(B::as_str).map(str::to_string);
            values.extend(text("value"));
            errs.extend(text("err"));
            let statuses = match msg.get("status") {
                Some(B::List(l)) => l.iter().filter_map(B::as_str).map(str::to_string).collect(),
                _ => vec![],
            };
            if statuses.iter().any(|s| s == "done") {
                return (values, errs);
            }
        }
        assert!(Instant::now() < deadline, "no answer");
    }
}

#[test]
fn a_world_its_owner_drives() {
    let mut vm = Vm::with_grants(Grants::NONE);
    let woken = Arc::new(AtomicUsize::new(0));
    let w = woken.clone();
    let server = Server::install(&mut vm, move || {
        w.fetch_add(1, Ordering::Relaxed);
    })
    .unwrap();
    let (tx, rx) = mpsc::channel();
    let remote = server.remote();
    std::thread::spawn(move || remote.serve("127.0.0.1:0", |addr| tx.send(addr).unwrap()));
    let mut c = TcpStream::connect(rx.recv().unwrap()).unwrap();
    c.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
    let eval = |code: &str| vec![("op", B::str("eval")), ("code", B::str(code))];
    assert_eq!(ask(&mut c, &mut vm, &server, eval("(define (f) 1) (+ (f) 2)")).0, ["3"]);
    assert!(woken.load(Ordering::Relaxed) > 0, "a request arriving wakes the owner");
    // Redefined in the owner's world: the next call sees it.
    ask(&mut c, &mut vm, &server, eval("(define (f) 10)"));
    assert_eq!(ask(&mut c, &mut vm, &server, eval("(+ (f) 2)")).0, ["12"]);
    assert_eq!(eval_global(&mut vm, "(f)"), "10");
    // The service grants nothing: files stay out of reach.
    let (values, errs) = ask(&mut c, &mut vm, &server, eval("(open-input-file \"/etc/hostname\")"));
    assert!(values.is_empty() && errs.iter().any(|e| e.contains("not granted")), "{errs:?}");
    // Moved to a world in place of this one: the connection and its
    // requests go on there, in a world without `f`.
    let mut vm = Vm::with_grants(Grants::NONE);
    let server = server.replace(&mut vm, "replaced").map_err(|(_, e)| e).unwrap();
    let (values, errs) = ask(&mut c, &mut vm, &server, eval("(f)"));
    assert!(values.is_empty() && errs.iter().any(|e| e.contains("unbound")), "{errs:?}");
    assert_eq!(ask(&mut c, &mut vm, &server, eval("(define (f) 5) (f)")).0, ["5"]);
}

fn eval_global(vm: &mut Vm, src: &str) -> String {
    let v = vm.eval_source(src).unwrap();
    vm.repr(&v)
}

/// Send `fields`, then drive the owner's world and read until a message
/// satisfies `until`; that message.
fn wait_for(c: &mut TcpStream, vm: &mut Vm, server: &Server, fields: Option<Vec<(&str, B)>>, until: impl Fn(&B) -> bool) -> B {
    if let Some(fields) = fields {
        let mut bytes = Vec::new();
        dict(fields).encode(&mut bytes);
        c.write_all(&bytes).unwrap();
    }
    let mut buf = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        server.poll(vm);
        vm.run_tasks_for(Duration::from_millis(5));
        server.reap(vm);
        let mut chunk = [0u8; 4096];
        match c.read(&mut chunk) {
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => panic!("{e}"),
        }
        while let Some((msg, used)) = parse(&buf).unwrap() {
            buf.drain(..used);
            if until(&msg) {
                return msg;
            }
        }
        assert!(Instant::now() < deadline, "no such message");
    }
}

fn has_status(msg: &B, status: &str) -> bool {
    matches!(msg.get("status"), Some(B::List(l)) if l.iter().any(|s| s.as_str() == Some(status)))
}

/// A task no evaluation waits for (a request a host serves) calls
/// `%nrepl-debug-request` from its handler: with no client subscribed it
/// gets #f at once; with one, that client gets the debugger, with the
/// task's restarts, and its choice continues the task.
#[test]
fn a_debugger_for_a_task_of_the_world() {
    let mut vm = Vm::with_grants(Grants::NONE);
    let server = Server::install(&mut vm, || {}).unwrap();
    let (tx, rx) = mpsc::channel();
    let remote = server.remote();
    std::thread::spawn(move || remote.serve("127.0.0.1:0", |addr| tx.send(addr).unwrap()));
    let mut c = TcpStream::connect(rx.recv().unwrap()).unwrap();
    c.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
    let request = vm
        .eval_source(
            "(lambda ()
               (with-exception-handler
                (lambda (c) (or (%nrepl-debug-request c) (raise c)))
                (lambda () (restart-case (+ 1 (error \"boom\")) (use-value (v) v)))))",
        )
        .unwrap();
    // No one subscribed: the condition goes on.
    let task = vm.spawn(&request);
    vm.run_tasks_for(Duration::from_millis(50));
    assert!(vm.task_result(&task).unwrap().is_err());
    // Subscribed: the debugger opens there, and its choice is taken.
    let subscribe = vec![("op", B::str("techne-debug-subscribe")), ("id", B::str("sub"))];
    wait_for(&mut c, &mut vm, &server, Some(subscribe), |m| has_status(m, "techne-debug-subscribed"));
    let task = vm.spawn(&request);
    let debugger = wait_for(&mut c, &mut vm, &server, None, |m| has_status(m, "techne-debug"));
    assert_eq!(debugger.get("id").and_then(B::as_str), Some("sub"));
    assert_eq!(debugger.get("condition").and_then(B::as_str), Some("boom"));
    let Some(B::List(restarts)) = debugger.get("restarts") else { panic!("no restarts") };
    assert_eq!(restarts[0].get("name").and_then(B::as_str), Some("use-value"));
    let id = debugger.get("debug-id").and_then(B::as_int).unwrap();
    let choose = vec![("op", B::str("techne-debug-restart")), ("debug-id", B::Int(id)), ("restart", B::Int(0)), ("args", B::str("42"))];
    wait_for(&mut c, &mut vm, &server, Some(choose), |m| has_status(m, "techne-debug-end"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while vm.task_result(&task).is_none() && Instant::now() < deadline {
        vm.run_tasks_for(Duration::from_millis(5));
    }
    let result = vm.task_result(&task).unwrap().unwrap();
    assert_eq!(vm.repr(&result), "42");
    // Killed while its debugger waits (past a deadline): the debugger
    // closes, though no Lisp cleanup ran.
    let task = vm.spawn(&request);
    wait_for(&mut c, &mut vm, &server, None, |m| has_status(m, "techne-debug"));
    vm.kill_task(&task).unwrap();
    wait_for(&mut c, &mut vm, &server, None, |m| has_status(m, "techne-debug-end"));
    // The server moved to another world: the task waiting in this one is
    // stopped, not left waiting for a choice that cannot come.
    let task = vm.spawn(&request);
    wait_for(&mut c, &mut vm, &server, None, |m| has_status(m, "techne-debug"));
    let mut other = Vm::with_grants(Grants::NONE);
    let server = server.replace(&mut other, "moved").map_err(|(_, e)| e).unwrap();
    wait_for(&mut c, &mut other, &server, None, |m| has_status(m, "techne-debug-end"));
    vm.run_tasks_for(Duration::from_millis(50));
    assert_eq!(vm.task_result(&task).unwrap().unwrap_err().msg, "interrupted");
    // And one failing there now gets no debugger.
    let task = vm.spawn(&request);
    vm.run_tasks_for(Duration::from_millis(50));
    assert_eq!(vm.task_result(&task).unwrap().unwrap_err().msg, "boom");
}
