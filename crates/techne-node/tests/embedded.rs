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
