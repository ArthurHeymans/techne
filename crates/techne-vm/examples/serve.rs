//! The HTTP benchmark's server (`runtime/bench/http/techne/server.scm`) on
//! in-memory connections: what techne does per request, without the
//! kernel's share and deterministic, for `runtime/bench/icount.sh`.
//!
//! The natives `server.scm` uses are async, like the real ones
//! (`crates/techne-net`): `tcp-accept` and `tcp-read` are pending
//! once and wake their task, as on a socket that has nothing yet, so every
//! wait goes through suspending the task and resuming it. Each connection
//! sends its requests one per read, then pipelined, then one split across
//! two reads; `vm.run_tasks` runs the server until every connection is
//! served. The responses are checked.
//!
//! `serve --list` names the workloads; `serve NAME` runs one and prints how
//! many requests it timed. Under callgrind, collecting only in
//! `timed_requests`, that run counts their instructions, which `icount.sh`
//! reports per request; with `TECHNE_JIT=1` the warm-up run before them
//! compiles everything they run. Without arguments, it prints each
//! workload's wall time per request.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
    time::Instant,
};

use techne_vm::{
    api::Value,
    api::{Bytes, Foreign, IntoValue},
    vm::{USER_MODULE, Vm},
};

const SERVER: &str = include_str!("../../../runtime/bench/http/techne/server.scm");

struct Workload {
    name: &'static str,
    path: &'static str,
    /// Connections of the timed run, after a warm-up run with a few.
    conns: usize,
}

const WORKLOADS: &[Workload] =
    &[Workload { name: "plaintext", path: "/", conns: 64 }, Workload { name: "/work", path: "/work", conns: 16 }];

/// A client's connection: what it sends, read by read, and what it got.
struct Conn {
    reads: RefCell<VecDeque<String>>,
    written: RefCell<Vec<u8>>,
    closed: Cell<bool>,
}

struct Listener;

/// What a read returns: the data, or the eof object.
struct Read(Option<String>);

impl IntoValue for Read {
    fn into_value(self, vm: &mut Vm) -> Result<Value, techne_vm::vm::Error> {
        self.0.map_or(Ok(Value::EOF), |s| s.into_value(vm))
    }
}

/// Pending on its first poll, after waking its task; then ready.
struct Later<T>(Option<T>, bool);

impl<T: Unpin> Future for Later<T> {
    type Output = T;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        if std::mem::replace(&mut self.1, true) {
            Poll::Ready(self.0.take().expect("polled after ready"))
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

fn later<T>(t: T) -> Later<T> {
    Later(Some(t), false)
}

fn request(path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: localhost:8080\r\nUser-Agent: bench\r\nAccept: */*\r\n\r\n")
}

/// What one connection sends, and how many requests that is.
fn reads(path: &str) -> (VecDeque<String>, usize) {
    let r = request(path);
    let mut reads: VecDeque<String> = std::iter::repeat_n(r.clone(), 8).collect();
    reads.extend(std::iter::repeat_n(r.repeat(4), 2));
    let (head, tail) = r.split_at(r.len() / 2);
    reads.extend([head.to_string(), tail.to_string()]);
    (reads, 8 + 2 * 4 + 1)
}

fn response(path: &str) -> String {
    let body = if path == "/work" {
        (0..1000u64).map(|i| (i * i).to_string().len()).sum::<usize>().to_string()
    } else {
        "Hello, World!".to_string()
    };
    format!("HTTP/1.1 200 OK\r\nServer: techne\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}", body.len())
}

/// A VM running `server.scm`, accepting the connections put in the queue.
fn server() -> (Vm, Rc<RefCell<VecDeque<Rc<Conn>>>>) {
    let mut vm = Vm::new();
    let incoming: Rc<RefCell<VecDeque<Rc<Conn>>>> = Rc::default();
    vm.register_fn("%tcp-listen", |_port: i64| Foreign::new(Listener));
    vm.register_fn("tcp-close", |c: Foreign<Conn>| c.closed.set(true));
    let queue = incoming.clone();
    // Fails once every connection is accepted, which ends `serve`.
    vm.register_async("tcp-accept", 1, move |vm: &mut Vm, args| {
        let listener = args.get::<Foreign<Listener>>(vm, 0).map_err(|e| e.to_string());
        let conn = queue.borrow_mut().pop_front();
        later(listener.and_then(|_| conn.map(Foreign).ok_or_else(|| "tcp-accept: no more connections".to_string())))
    });
    vm.register_async("tcp-read", 1, |vm: &mut Vm, args| {
        let conn = args.get::<Foreign<Conn>>(vm, 0).map_err(|e| e.to_string());
        later(conn.map(|c| Read(c.reads.borrow_mut().pop_front())))
    });
    vm.register_async("tcp-write", 2, |vm: &mut Vm, args| {
        let conn = args.get::<Foreign<Conn>>(vm, 0).map_err(|e| e.to_string());
        let data = args.get::<Bytes>(vm, 1).map_err(|e| e.to_string());
        std::future::ready(conn.and_then(|c| data.map(|Bytes(d)| c.written.borrow_mut().extend(d))))
    });
    vm.eval_source("(define (tcp-listen port #:reuse-port [reuse-port #f]) (%tcp-listen port))").unwrap();
    vm.eval_source(SERVER).unwrap_or_else(|e| panic!("server.scm: {e}"));
    // One procedure for every run: compiled in the warm-up, not when timed.
    vm.eval_source("(define (main) (serve 8080))").unwrap_or_else(|e| panic!("{e}"));
    (vm, incoming)
}

/// Serve `conns` connections of `w`'s requests; check the responses and
/// return the number of requests.
fn serve(vm: &mut Vm, incoming: &RefCell<VecDeque<Rc<Conn>>>, w: &Workload, conns: usize, drive: fn(&mut Vm, Value)) -> usize {
    let (sent, per_conn) = reads(w.path);
    let clients: Vec<Rc<Conn>> = (0..conns)
        .map(|_| Rc::new(Conn { reads: RefCell::new(sent.clone()), written: RefCell::default(), closed: Cell::new(false) }))
        .collect();
    incoming.borrow_mut().extend(clients.iter().cloned());
    let main = vm.eval_in(USER_MODULE, "<main>", "main").unwrap_or_else(|e| panic!("{e}"));
    drive(vm, main);
    let expected = response(w.path).repeat(per_conn);
    for c in &clients {
        assert!(c.closed.get(), "a connection was not closed");
        assert_eq!(String::from_utf8_lossy(&c.written.borrow()), expected);
    }
    conns * per_conn
}

/// Run the server until it has served every connection.
fn run_server(vm: &mut Vm, main: Value) {
    vm.spawn(&main);
    vm.run_tasks().unwrap_or_else(|e| panic!("{e}"));
}

/// `run_server` for the timed run: what the benchmark counts.
#[inline(never)]
fn timed_requests(vm: &mut Vm, main: Value) {
    run_server(vm, main);
}

/// The requests of `w`'s timed run (after a warm-up), and its wall time.
fn run(w: &Workload) -> (usize, f64) {
    let (mut vm, incoming) = server();
    serve(&mut vm, &incoming, w, 4, run_server);
    let start = Instant::now();
    let requests = serve(&mut vm, &incoming, w, w.conns, timed_requests);
    (requests, start.elapsed().as_secs_f64())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--list") {
        WORKLOADS.iter().for_each(|w| println!("{}", w.name));
        return;
    }
    if let Some(name) = args.first() {
        let w = WORKLOADS.iter().find(|w| w.name == name).unwrap_or_else(|| panic!("no workload {name}"));
        // The number of requests timed, for dividing instructions by.
        println!("{}", run(w).0);
        return;
    }
    for w in WORKLOADS {
        let (requests, secs) = run(w);
        println!("{:<10} {:>5} requests {:>8.2} µs per request", w.name, requests, secs * 1e6 / requests as f64);
    }
}
