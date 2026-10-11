//! TCP networking for techne Lisp: listeners and connections.
//!
//! - Every wait (accept, connect, read, write) is an async native: it
//!   suspends only the calling task. Cancelling or killing the task drops
//!   the wait; the socket stays usable (a write cancelled halfway may have
//!   sent part of its data).
//! - Nothing is buffered here. A read returns what the kernel has received,
//!   at most `READ_BYTES`, and admits what it may allocate (`Vm::admit`)
//!   before it waits. A write waits until the kernel has taken all of its
//!   data, so a peer that reads slowly makes the writer wait
//!   (back-pressure); it sends the string or bytevector from where it is,
//!   without copying it, so a waiting writer holds no memory of its own.
//! - Data is bytes. `tcp-read-bytes` returns bytevectors; `tcp-read`
//!   decodes UTF-8, joining a multi-byte sequence split between two reads,
//!   with invalid bytes as U+FFFD. The end of the stream is the eof object;
//!   a failure (connection reset, refused) is an error.
//! - The current scope owns each listener and connection: shutting it
//!   closes them. `tcp-close` closes one at once (and wakes tasks waiting
//!   on it, which then fail); a socket no scope owns closes when its Lisp
//!   object is collected. `call-with-socket` closes one when its body
//!   returns, fails or is cancelled.
//! - The procedures need the network capability; in a world without it
//!   they only raise "not granted".
//!
//! ## The I/O runtime
//!
//! Sockets are registered with a tokio runtime, which polls them for
//! readiness and wakes the VM's waiting tasks through their futures' wakers.
//! [`install`] uses a runtime shared by every VM of the process, with one
//! I/O thread ([`runtime`]): any host works, including one that blocks in
//! `Vm::run_tasks`. [`install_on`] uses the host's runtime instead, which
//! must be running. A host with one VM per thread can give each VM a
//! current-thread runtime and drive it with [`run_tasks`], so its sockets
//! are polled on its own thread, with no wakeup crossing threads; listeners
//! on one port with `#:reuse-port` let the kernel spread connections over
//! the VMs. That is what an HTTP server does best: see
//! `runtime/bench/http/README.md`, where it scales like Go's and Rust's
//! servers, while the shared I/O thread falls behind at 8 VMs.
//!
//! ```scheme
//! (define l (tcp-listen 0))
//! (spawn (lambda ()
//!          (call-with-socket (tcp-accept l)
//!            (lambda (c) (tcp-write c (tcp-read c))))))
//! (call-with-tcp-connection "127.0.0.1" (cadr (tcp-local-address l))
//!   (lambda (c) (tcp-write c "hello") (tcp-read c)))
//! ;; => "hello"
//! ```

use std::{
    cell::RefCell,
    future::Future,
    io::{self, ErrorKind},
    net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr},
    pin::Pin,
    rc::Rc,
    sync::{Arc, OnceLock},
    task::{Context, Poll},
    time::Duration,
};

use techne_vm::{
    api::{Bytes, Foreign, IntoValue, Root},
    builtins::type_error,
    heap::{Kind, is_kind, str_bytes},
    tasks::Progress,
    value::Value,
    vm::{Capability, Error, ROOT_MODULE, Vm},
};
use tokio::{
    net::{TcpListener, TcpStream},
    runtime::Handle,
};

/// Most bytes one read returns. Under 64 KiB, what it allocates is admitted
/// without a look at the memory held (`Vm::admit`).
pub const READ_BYTES: usize = 32 * 1024;

/// The runtime [`install`] registers sockets with: one I/O thread, shared
/// by every VM of the process.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread().worker_threads(1).thread_name("techne-net").enable_all().build().expect("tokio runtime")
    })
}

/// Run `vm`'s tasks until they have all finished, on the current thread's
/// tokio runtime (a current-thread runtime in `block_on`, typically): when
/// every task waits, it waits for the runtime to wake one, or for the next
/// timer. It sets the VM's wake notifier, so call it before tasks wait on
/// anything (spawning them before is fine).
pub async fn run_tasks(vm: &mut Vm) {
    let wake = Arc::new(tokio::sync::Notify::new());
    let notify = wake.clone();
    vm.set_wake_notifier(move || notify.notify_one());
    loop {
        match vm.run_tasks_for(Duration::from_millis(2)) {
            Progress::Finished => return,
            // Let the runtime poll its sockets.
            Progress::OutOfTime => tokio::task::yield_now().await,
            Progress::Blocked => match vm.next_timer() {
                Some(t) => {
                    let _ = tokio::time::timeout_at(t.into(), wake.notified()).await;
                }
                None => wake.notified().await,
            },
        }
    }
}

/// A connection's error message: what the OS says, without the error
/// number, as in "tcp-read: connection reset by peer".
fn failure(who: &str, e: &io::Error) -> String {
    let text = e.to_string();
    let text = text.split(" (os error").next().unwrap_or(&text);
    let mut chars = text.chars();
    let text: String = chars.next().map(|c| c.to_lowercase().chain(chars).collect()).unwrap_or_default();
    format!("{who}: {text}")
}

/// A Lisp TCP listener: its socket, `None` once closed, and its address.
pub struct Listener {
    socket: RefCell<Option<Rc<TcpListener>>>,
    local: SocketAddr,
}

/// A Lisp TCP connection: its socket, `None` once closed, its addresses,
/// and the start of a UTF-8 sequence `tcp-read` has read but not yet
/// decoded.
pub struct Connection {
    stream: RefCell<Option<Rc<TcpStream>>>,
    local: SocketAddr,
    peer: SocketAddr,
    carry: RefCell<Vec<u8>>,
}

/// Close `socket` now. A wait on it holds a reference: then shutting it
/// down first wakes the wait, which finds it closed and fails.
fn close<T: std::os::fd::AsFd>(socket: &RefCell<Option<Rc<T>>>) {
    if let Some(s) = socket.borrow_mut().take()
        && Rc::strong_count(&s) > 1
    {
        let _ = socket2::SockRef::from(&*s).shutdown(Shutdown::Both);
    }
}

impl Listener {
    fn socket(&self, who: &str) -> Result<Rc<TcpListener>, String> {
        self.socket.borrow().clone().ok_or_else(|| format!("{who}: the listener is closed"))
    }

    fn is_closed(&self) -> bool {
        self.socket.borrow().is_none()
    }

    async fn accept(&self) -> Result<Connection, String> {
        let l = self.socket("tcp-accept")?;
        loop {
            match l.accept().await {
                Ok((s, _)) => {
                    // Closed meanwhile: what woke us was `close`.
                    if self.is_closed() {
                        return Err("tcp-accept: the listener is closed".into());
                    }
                    return Connection::new(s, "tcp-accept");
                }
                Err(_) if self.is_closed() => return Err("tcp-accept: the listener is closed".into()),
                // The peer gave up before we accepted it: wait for another.
                Err(e) if e.kind() == ErrorKind::ConnectionAborted => continue,
                Err(e) => return Err(failure("tcp-accept", &e)),
            }
        }
    }
}

thread_local! {
    /// What reads land in before they are copied out.
    static SCRATCH: RefCell<Vec<u8>> = RefCell::new(vec![0; READ_BYTES]);
}

impl Connection {
    fn new(s: TcpStream, who: &str) -> Result<Connection, String> {
        // Requests and replies go out as soon as they are written.
        s.set_nodelay(true).map_err(|e| failure(who, &e))?;
        let (local, peer) = (s.local_addr(), s.peer_addr());
        let (local, peer) = (local.map_err(|e| failure(who, &e))?, peer.map_err(|e| failure(who, &e))?);
        Ok(Connection { stream: RefCell::new(Some(Rc::new(s))), local, peer, carry: RefCell::default() })
    }

    fn stream(&self, who: &str) -> Result<Rc<TcpStream>, String> {
        self.stream.borrow().clone().ok_or_else(|| closed(who))
    }

    fn is_closed(&self) -> bool {
        self.stream.borrow().is_none()
    }

    /// The error of `e`, or that the connection was closed meanwhile.
    fn failed(&self, who: &str, e: &io::Error) -> String {
        if self.is_closed() { closed(who) } else { failure(who, e) }
    }

    /// Wait for data and return `take` of it; `None` at end of stream.
    async fn read_with<T>(&self, who: &str, mut take: impl FnMut(&[u8]) -> T) -> Result<Option<T>, String> {
        let s = self.stream(who)?;
        loop {
            let read = SCRATCH.with_borrow_mut(|buf| s.try_read(buf).map(|n| (n > 0).then(|| take(&buf[..n]))));
            match read {
                Ok(r) => return Ok(r),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(self.failed(who, &e)),
            }
            s.readable().await.map_err(|e| self.failed(who, &e))?;
            if self.is_closed() {
                return Err(closed(who));
            }
        }
    }

    /// The next data as text; `None` at end of stream.
    async fn read_text(&self) -> Result<Option<String>, String> {
        loop {
            match self.read_with("tcp-read", |data| decode(&mut self.carry.borrow_mut(), data)).await? {
                Some(text) if text.is_empty() => continue,
                Some(text) => return Ok(Some(text)),
                None => {
                    // An incomplete sequence at the end is invalid.
                    let rest = std::mem::take(&mut *self.carry.borrow_mut());
                    return Ok((!rest.is_empty()).then(|| String::from_utf8_lossy(&rest).into_owned()));
                }
            }
        }
    }

    /// The next data as bytes, starting with what a text read left
    /// undecoded; `None` at end of stream.
    async fn read_bytes(&self) -> Result<Option<Vec<u8>>, String> {
        self.stream("tcp-read-bytes")?;
        let carry = std::mem::take(&mut *self.carry.borrow_mut());
        if !carry.is_empty() {
            return Ok(Some(carry));
        }
        self.read_with("tcp-read-bytes", <[u8]>::to_vec).await
    }

    /// Close it now, and forget what a text read left undecoded.
    fn close(&self) {
        close(&self.stream);
        self.carry.borrow_mut().clear();
    }

    /// Write what the kernel takes now of `bytes` from `at` on, moving
    /// `at`; whether all of it is written.
    fn write_some(&self, s: &TcpStream, bytes: &[u8], at: &mut usize) -> Result<bool, String> {
        // Shorter than when it began: a string changed meanwhile.
        while *at < bytes.len() {
            match s.try_write(&bytes[*at..]) {
                Ok(n) => *at += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(false),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(self.failed("tcp-write", &e)),
            }
        }
        Ok(true)
    }

    /// Write the rest of `data`, a string or a bytevector, from `at` on,
    /// waiting while the kernel's buffer is full. Each write takes the
    /// bytes from the heap, where a collection may have moved them since
    /// the last.
    async fn write_rest(&self, data: Root, mut at: usize) -> Result<(), String> {
        let s = self.stream("tcp-write")?;
        loop {
            s.writable().await.map_err(|e| self.failed("tcp-write", &e))?;
            if self.is_closed() {
                return Err(closed("tcp-write"));
            }
            // SAFETY: the root holds a string or a bytevector, and nothing
            // runs on the VM's thread (no collection) while this writes.
            if self.write_some(&s, unsafe { str_bytes(data.get().as_ptr()) }, &mut at)? {
                return Ok(());
            }
        }
    }
}

/// How a write goes on once the kernel has taken what it could at once.
enum Writing {
    Done(Result<(), String>),
    /// The connection, the data, rooted, and how much of it is written.
    Waiting(Rc<Connection>, Root, usize),
}

fn closed(who: &str) -> String {
    format!("{who}: the connection is closed")
}

/// Append `data` to `carry` and return the text of all but an incomplete
/// UTF-8 sequence at the end, which stays in `carry`.
fn decode(carry: &mut Vec<u8>, data: &[u8]) -> String {
    if carry.is_empty() {
        let keep = incomplete_tail(data);
        carry.extend_from_slice(&data[data.len() - keep..]);
        return String::from_utf8_lossy(&data[..data.len() - keep]).into_owned();
    }
    carry.extend_from_slice(data);
    let keep = incomplete_tail(carry);
    let tail = carry.split_off(carry.len() - keep);
    let text = String::from_utf8_lossy(carry).into_owned();
    *carry = tail;
    text
}

/// Bytes at the end of `data` that start a valid but incomplete UTF-8
/// sequence (the rest may come with the next read); 0 if there are none.
fn incomplete_tail(data: &[u8]) -> usize {
    // The sequence's first byte: the last that does not continue one.
    let Some(back) = (1..=3.min(data.len())).find(|&back| data[data.len() - back] & 0xC0 != 0x80) else { return 0 };
    let tail = &data[data.len() - back..];
    match std::str::from_utf8(tail) {
        // Cut short, not invalid: `error_len` would say how many bytes are.
        Err(e) if e.valid_up_to() == 0 && e.error_len().is_none() => back,
        _ => 0,
    }
}

/// The host named by `v`, a string, refused before it is copied if it is
/// too long to be a host name or an address.
fn host(vm: &mut Vm, v: Value, who: &str) -> Result<String, Error> {
    if !is_kind(v, Kind::String) {
        return Err(type_error(who, "string", v));
    }
    // SAFETY: `v` is a string.
    let len = unsafe { str_bytes(v.as_ptr()) }.len();
    if len > 253 {
        return Err(Error::new(format!("{who}: a host name is at most 253 bytes, not {len}")));
    }
    vm.get(v)
}

/// A listener on `host`:`port`, registered with the runtime `io`.
fn listen(io: &Handle, host: &str, port: u16, backlog: i32, reuse_port: bool) -> io::Result<TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let ip: IpAddr = match host {
        "localhost" => Ipv4Addr::LOCALHOST.into(),
        _ => host.parse().map_err(|_| io::Error::new(ErrorKind::InvalidInput, "expected an IP address or localhost"))?,
    };
    let addr = SocketAddr::new(ip, port);
    let s = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    // A restarted server can listen at once, past connections closing.
    s.set_reuse_address(true)?;
    s.set_reuse_port(reuse_port)?;
    s.bind(&addr.into())?;
    s.listen(backlog)?;
    s.set_nonblocking(true)?;
    let _io = io.enter();
    TcpListener::from_std(s.into())
}

/// A future polled inside the runtime `io`: one that registers a socket
/// with "the current runtime" (tokio's connect and accept) finds it.
struct Entered<F> {
    io: Handle,
    fut: Pin<Box<F>>,
}

impl<F: Future> Future for Entered<F> {
    type Output = F::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.get_mut();
        let _io = this.io.enter();
        this.fut.as_mut().poll(cx)
    }
}

fn entered<F: Future>(io: &Handle, fut: F) -> Entered<F> {
    Entered { io: io.clone(), fut: Box::pin(fut) }
}

/// Connect to `port` on `host`, a name or an IP address, trying each
/// address it has in turn.
async fn connect(host: String, port: u16) -> Result<Connection, String> {
    let who = format!("tcp-connect: {host}:{port}");
    let addrs = tokio::net::lookup_host((host.as_str(), port)).await.map_err(|e| failure(&who, &e))?;
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect(addr).await {
            Ok(s) => return Connection::new(s, "tcp-connect"),
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(e) => failure(&who, &e),
        None => format!("{who}: no address"),
    })
}

/// A chunk read, or (`None`) the end of the stream.
struct Chunk<T>(Option<T>);

impl<T: IntoValue> IntoValue for Chunk<T> {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        match self.0 {
            Some(s) => s.into_value(vm),
            None => Ok(Value::EOF),
        }
    }
}

/// A socket address as `(host port)`.
struct Address(SocketAddr);

impl IntoValue for Address {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        let host = vm.make_string(&self.0.ip().to_string());
        Ok(vm.make_list(&[host, Value::int_unchecked(self.0.port() as i64)]))
    }
}

fn foreign<T: 'static>(vm: &Vm, v: Value) -> Option<Rc<T>> {
    vm.foreign(v).and_then(|(rc, _)| rc.clone().downcast::<T>().ok())
}

fn connection(vm: &Vm, v: Value, who: &str) -> Result<Rc<Connection>, Error> {
    foreign(vm, v).ok_or_else(|| type_error(who, "tcp-connection", v))
}

fn port(n: i64, who: &str) -> Result<u16, String> {
    u16::try_from(n).map_err(|_| format!("{who}: bad port {n}"))
}

/// Scheme wrappers over the natives.
const PRELUDE: &str = r#"
(define (tcp-listen port #:host [host "127.0.0.1"] #:backlog [backlog 1024] #:reuse-port [reuse-port #f])
  "Return a listener for TCP connections to PORT on HOST.
HOST is an IP address (\"0.0.0.0\" or \"::\" for every interface) or
\"localhost\"; PORT 0 picks a free port, which `tcp-local-address` tells.
BACKLOG connections wait to be accepted at most. With REUSE-PORT, other
listeners may share the port, and the kernel spreads connections over them.
The current scope owns the listener: shutting the scope closes it."
  (scope-own! (%tcp-listen host port backlog reuse-port) tcp-close tcp-closed?))

(define (tcp-accept listener)
  "Wait for a connection to LISTENER and return it.
The current scope owns the connection: shutting the scope closes it."
  (scope-own! (%tcp-accept listener) tcp-close tcp-closed?))

(define (tcp-connect host port)
  "Connect to PORT on HOST, a name or an IP address; return the connection.
The current scope owns the connection: shutting the scope closes it."
  (scope-own! (%tcp-connect host port) tcp-close tcp-closed?))

(define (call-with-socket socket f)
  "Call F with SOCKET, closed when F returns, fails or is cancelled.
SOCKET is a connection or a listener; return what F returns."
  (dynamic-wind (lambda () #f) (lambda () (f socket)) (lambda () (tcp-close socket))))

(define (call-with-tcp-connection host port f)
  "Call F with a connection to PORT on HOST, closed when F is done.
It is closed when F returns, fails or is cancelled, as `call-with-socket`
does; return what F returns."
  (call-with-socket (tcp-connect host port) f))
"#;

/// Define the networking procedures in `vm`, with sockets on the shared
/// I/O runtime ([`runtime`]). In a world without the network capability
/// they only raise "not granted".
pub fn install(vm: &mut Vm) -> Result<(), Error> {
    install_on(vm, runtime().handle().clone())
}

/// [`install`], with sockets on the runtime `io`: a multi-thread runtime,
/// or a current-thread one that the host drives around the VM (with
/// [`run_tasks`], say). Sockets are polled only while it runs.
pub fn install_on(vm: &mut Vm, io: Handle) -> Result<(), Error> {
    vm.requiring(Capability::Network, |vm| natives(vm, io));
    // In the root module, so every module sees it.
    vm.eval_in(ROOT_MODULE, "<techne-net>", PRELUDE).map(|_| ())
}

fn natives(vm: &mut Vm, io: Handle) {
    vm.name_foreign_type::<Listener>("tcp-listener");
    vm.name_foreign_type::<Connection>("tcp-connection");
    let listen_io = io.clone();
    techne_vm::procedures! { vm;
        #[vm]
        "(%tcp-listen host port backlog reuse-port)" => move |vm: &mut Vm, host: Value, port: i64, backlog: i64, reuse_port: bool| -> Result<Foreign<Listener>, Error> {
            let host = self::host(vm, host, "tcp-listen")?;
            let who = format!("tcp-listen: {host}:{port}");
            let port = self::port(port, "tcp-listen").map_err(Error::new)?;
            let backlog = i32::try_from(backlog).map_err(|_| Error::new(format!("tcp-listen: bad backlog {backlog}")))?;
            let socket = listen(&listen_io, &host, port, backlog, reuse_port).map_err(|e| Error::new(failure(&who, &e)))?;
            let local = socket.local_addr().map_err(|e| Error::new(failure(&who, &e)))?;
            Ok(Foreign::new(Listener { socket: RefCell::new(Some(Rc::new(socket))), local }))
        };
    }
    let accept_io = io.clone();
    vm.register_async("%tcp-accept", 1, move |vm: &mut Vm, args: &[Value]| {
        let l = foreign::<Listener>(vm, args[0]).ok_or_else(|| type_error("tcp-accept", "tcp-listener", args[0]).msg.clone());
        entered(&accept_io, async move { l?.accept().await.map(Foreign::new) })
    });
    vm.register_async("%tcp-connect", 2, move |vm: &mut Vm, args: &[Value]| {
        let target = (|| -> Result<(String, u16), Error> {
            let host = host(vm, args[0], "tcp-connect")?;
            let port: i64 = vm.get(args[1])?;
            Ok((host, self::port(port, "tcp-connect").map_err(Error::new)?))
        })();
        entered(&io, async move {
            let (host, port) = target.map_err(|e| e.msg.clone())?;
            connect(host, port).await.map(Foreign::new)
        })
    });
    vm.register_async("tcp-read", 1, |vm: &mut Vm, args: &[Value]| {
        // What READ_BYTES and an incomplete sequence decode to, each byte
        // at worst the three of U+FFFD: as Rust's string, then the VM's.
        let c = connection(vm, args[0], "tcp-read").and_then(|c| vm.admit(2 * 3 * (READ_BYTES + 3)).map(|_| c));
        async move { c.map_err(|e| e.msg.clone())?.read_text().await.map(Chunk) }
    });
    vm.register_async("tcp-read-bytes", 1, |vm: &mut Vm, args: &[Value]| {
        // As a Rust vector, then the VM's bytevector.
        let c = connection(vm, args[0], "tcp-read-bytes").and_then(|c| vm.admit(2 * READ_BYTES).map(|_| c));
        async move { c.map_err(|e| e.msg.clone())?.read_bytes().await.map(|b| Chunk(b.map(Bytes))) }
    });
    vm.register_async("tcp-write", 2, |vm: &mut Vm, args: &[Value]| {
        let data = args[1];
        // What the kernel takes now, from the heap; a root for the rest.
        let writing = connection(vm, args[0], "tcp-write")
            .and_then(|c| {
                if !(is_kind(data, Kind::String) || is_kind(data, Kind::Bytevector)) {
                    return Err(type_error("tcp-write", "string or bytevector", data));
                }
                let s = c.stream("tcp-write").map_err(Error::new)?;
                let mut at = 0;
                // SAFETY: `data` is a string or a bytevector.
                let done = c.write_some(&s, unsafe { str_bytes(data.as_ptr()) }, &mut at).map_err(Error::new)?;
                Ok(if done { Writing::Done(Ok(())) } else { Writing::Waiting(c, vm.root(data), at) })
            })
            .unwrap_or_else(|e| Writing::Done(Err(e.msg.clone())));
        async move {
            match writing {
                Writing::Done(r) => r,
                Writing::Waiting(c, data, at) => c.write_rest(data, at).await,
            }
        }
    });
    techne_vm::document! { vm;
        /// Wait for data from CONNECTION and return it as a string.
        /// Return the eof object once the peer has closed its side. Bytes that
        /// are not UTF-8 read as U+FFFD; a character split between two reads is
        /// returned whole, with the second.
        "(tcp-read connection)";
        /// Wait for data from CONNECTION and return it as a bytevector.
        /// Return the eof object once the peer has closed its side.
        "(tcp-read-bytes connection)";
        /// Write DATA, a string or a bytevector, to CONNECTION.
        /// It waits until the kernel has taken all of it, so a peer that reads
        /// slowly makes it wait. DATA is sent from where it is, not copied:
        /// changing it meanwhile changes what is sent.
        "(tcp-write connection data)";
    }
    techne_vm::procedures! { vm;
        /// Close the sending side of CONNECTION: the peer reads end of file.
        /// Reading from CONNECTION goes on.
        #[vm]
        "(tcp-close-output connection)" => |vm: &mut Vm, c: Value| -> Result<(), Error> {
            let s = connection(vm, c, "tcp-close-output")?.stream("tcp-close-output").map_err(Error::new)?;
            socket2::SockRef::from(&*s).shutdown(Shutdown::Write).map_err(|e| Error::new(failure("tcp-close-output", &e)))
        };
    }
    techne_vm::procedures! { vm;
        /// Close SOCKET, a connection or a listener, if it is open.
        /// Tasks waiting on it fail.
        #[vm]
        "(tcp-close socket)" => |vm: &mut Vm, s: Value| -> Result<(), Error> {
            if let Some(c) = foreign::<Connection>(vm, s) {
                c.close();
            } else if let Some(l) = foreign::<Listener>(vm, s) {
                close(&l.socket);
            } else {
                return Err(type_error("tcp-close", "tcp-connection or tcp-listener", s));
            }
            Ok(())
        };
    }
    techne_vm::procedures! { vm;
        /// Return #t if SOCKET, a connection or a listener, is closed.
        #[vm]
        "(tcp-closed? socket)" => |vm: &mut Vm, s: Value| -> Result<bool, Error> {
            if let Some(c) = foreign::<Connection>(vm, s) {
                Ok(c.is_closed())
            } else if let Some(l) = foreign::<Listener>(vm, s) {
                Ok(l.is_closed())
            } else {
                Err(type_error("tcp-closed?", "tcp-connection or tcp-listener", s))
            }
        };
    }
    techne_vm::procedures! { vm;
        /// Return the address of SOCKET, a connection or a listener, here.
        /// It is a list of the host, a string, and the port.
        #[vm]
        "(tcp-local-address socket)" => |vm: &mut Vm, s: Value| -> Result<Address, Error> {
            if let Some(c) = foreign::<Connection>(vm, s) {
                Ok(Address(c.local))
            } else if let Some(l) = foreign::<Listener>(vm, s) {
                Ok(Address(l.local))
            } else {
                Err(type_error("tcp-local-address", "tcp-connection or tcp-listener", s))
            }
        };
    }
    techne_vm::procedures! { vm;
        /// Return the address of the peer of CONNECTION.
        /// It is a list of the host, a string, and the port.
        #[vm]
        "(tcp-peer-address connection)" => |vm: &mut Vm, c: Value| -> Result<Address, Error> {
            Ok(Address(connection(vm, c, "tcp-peer-address")?.peer))
        };
    }
    techne_vm::procedures! { vm;
        /// Return #t if OBJ is a TCP connection.
        #[vm]
        "(tcp-connection? obj)" => |vm: &mut Vm, v: Value| foreign::<Connection>(vm, v).is_some();
    }
    techne_vm::procedures! { vm;
        /// Return #t if OBJ is a TCP listener.
        #[vm]
        "(tcp-listener? obj)" => |vm: &mut Vm, v: Value| foreign::<Listener>(vm, v).is_some();
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, failure, incomplete_tail};

    #[test]
    fn utf8_tails() {
        let s = "aé€😀".as_bytes();
        assert_eq!(incomplete_tail(s), 0);
        assert_eq!(incomplete_tail(&s[..s.len() - 1]), 3);
        assert_eq!(incomplete_tail(&s[..s.len() - 2]), 2);
        assert_eq!(incomplete_tail(&s[..s.len() - 3]), 1);
        assert_eq!(incomplete_tail(&s[..2]), 1);
        assert_eq!(incomplete_tail(b"abc"), 0);
        // Bytes that start no valid sequence are not held back.
        for invalid in [&b"\xff"[..], b"\xc0", b"\xc1", b"\xf5", b"\xe0\x80", b"\xed\xa0", b"\xf0\x80", b"\xf4\x90", b"\xf0\x90\x20"] {
            assert_eq!(incomplete_tail(invalid), 0, "{invalid:x?}");
        }
        // Valid beginnings, including the restricted second bytes.
        for start in [&b"\xc2"[..], b"\xe0\xa0", b"\xed\x9f", b"\xf0\x90", b"\xf4\x8f\xbf", b"a\xe2\x82"] {
            assert_eq!(incomplete_tail(start), start.len() - start.iter().take_while(|b| b.is_ascii()).count(), "{start:x?}");
        }
    }

    #[test]
    fn decoding_joins_split_characters() {
        let bytes = "λ€".as_bytes();
        let mut carry = Vec::new();
        let parts: Vec<String> = bytes.iter().map(|b| decode(&mut carry, std::slice::from_ref(b))).collect();
        assert_eq!(parts.concat(), "λ€");
        assert!(carry.is_empty());
        assert_eq!(decode(&mut carry, b"a\xffb"), "a\u{fffd}b");
        // An invalid byte at the end is decoded at once.
        assert_eq!(decode(&mut carry, b"\xff"), "\u{fffd}");
        assert!(carry.is_empty());
        // At worst, three bytes for each one read.
        assert_eq!(decode(&mut carry, &[0x80; 4]).len(), 12);
    }

    #[test]
    fn messages_drop_the_error_number() {
        let e = std::io::Error::from_raw_os_error(111);
        assert_eq!(failure("tcp-connect", &e), "tcp-connect: connection refused");
    }
}
