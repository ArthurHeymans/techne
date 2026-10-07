//! Child processes for techne Lisp: the process contract probe.
//!
//! A process runs with pipes (separate stdout and stderr) or on a pty (one
//! output stream, a controlling terminal). Every wait happens in the calling
//! task only: reads, writes and `process-wait` are async natives, so other
//! tasks and the host keep running.
//!
//! - Output is pumped into a bounded queue per stream (`QUEUE_CHUNKS` chunks
//!   of at most `CHUNK_BYTES`). A slow reader fills it, the pump stops
//!   reading, and the child blocks on write: memory stays bounded.
//! - Each child leads its own process group; signals go to the group, and
//!   dropping the process (its Lisp object becoming garbage, or
//!   `process-kill`) kills the group. `call-with-process` kills it when the
//!   body returns, fails or its task is cancelled.
//! - Output is bytes. `process-read-bytes` returns them as bytevectors;
//!   `process-read` decodes them as UTF-8, joining a multi-byte sequence
//!   split between two reads, with invalid bytes as U+FFFD. A failed read is
//!   an error, not the end of the output (except EIO on a pty, which is how
//!   it ends when the child is gone). `process-write` takes a string or a
//!   bytevector.
//!
//! The I/O runs on a small tokio runtime owned by this crate (`runtime`).
//! Lisp process objects hold a `ProcessBackend`: a local `Process`, or (from
//! `techne-node`) a process on a remote node, behind the same procedures.
//!
//! ```scheme
//! (call-with-process "sh" '("-c" "echo hi; echo oops >&2")
//!   (lambda (p)
//!     (list (process-read p 'stdout) (process-read p 'stderr) (process-wait p))))
//! ;; => ("hi\n" "oops\n" 0)
//! ```

use std::{
    cell::RefCell,
    future::Future,
    os::unix::process::ExitStatusExt,
    pin::Pin,
    process::{ExitStatus, Stdio},
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
};

use rustix::process::{Pid, Signal, kill_process_group};
use techne_vm::{
    api::{Bytes, Foreign, IntoValue},
    value::Value,
    vm::{Error, Vm},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, watch},
};

/// Largest chunk one read returns.
pub const CHUNK_BYTES: usize = 16 * 1024;
/// Chunks buffered per output stream before the child is made to wait.
pub const QUEUE_CHUNKS: usize = 16;

/// The runtime that runs process (and node transport) I/O.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("techne-process")
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

/// Output retained per stream of a persistent process (`Retain::Ring`).
pub const RETAIN_BYTES: usize = 1 << 20;

/// What happens to output nobody reads yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retain {
    /// A bounded queue: when it is full the child blocks on its output.
    Queue,
    /// Keep the last `n` bytes and drop older ones; the child never waits.
    /// For processes that outlive their reader (a node session).
    Ring(usize),
}

/// The newest output of a stream, bounded.
#[derive(Default)]
struct Ring {
    state: Mutex<RingState>,
    ready: tokio::sync::Notify,
}

#[derive(Default)]
struct RingState {
    chunks: std::collections::VecDeque<Vec<u8>>,
    bytes: usize,
    dropped: u64,
    eof: bool,
    /// Why reading the stream failed, once it has.
    error: Option<String>,
}

/// What a pump sends: output, or why reading failed (the last item).
type Item = Result<Vec<u8>, String>;

#[derive(Clone)]
enum Output {
    Queue(Arc<tokio::sync::Mutex<mpsc::Receiver<Item>>>),
    Ring(Arc<Ring>),
}

impl Output {
    /// The next chunk; `None` at end of file.
    async fn read(&self) -> Result<Option<Vec<u8>>, String> {
        match self {
            Output::Queue(rx) => rx.lock().await.recv().await.transpose(),
            Output::Ring(ring) => loop {
                {
                    let mut st = ring.state.lock().unwrap();
                    if let Some(c) = st.chunks.pop_front() {
                        st.bytes -= c.len();
                        return Ok(Some(c));
                    }
                    if let Some(e) = &st.error {
                        return Err(e.clone());
                    }
                    if st.eof {
                        return Ok(None);
                    }
                }
                // A notification sent since the check is kept as a permit.
                ring.ready.notified().await;
            },
        }
    }

    fn dropped(&self) -> u64 {
        match self {
            Output::Queue(_) => 0,
            Output::Ring(ring) => ring.state.lock().unwrap().dropped,
        }
    }
}

/// Move what the pump reads into `ring`, dropping the oldest output beyond
/// `cap` bytes.
async fn fill(mut rx: mpsc::Receiver<Item>, ring: Arc<Ring>, cap: usize) {
    while let Some(item) = rx.recv().await {
        let mut st = ring.state.lock().unwrap();
        let text = match item {
            Ok(bytes) => bytes,
            Err(e) => {
                st.error = Some(e);
                break;
            }
        };
        st.bytes += text.len();
        st.chunks.push_back(text);
        while st.bytes > cap && st.chunks.len() > 1 {
            let old = st.chunks.pop_front().unwrap();
            st.bytes -= old.len();
            st.dropped += old.len() as u64;
        }
        drop(st);
        ring.ready.notify_one();
    }
    ring.state.lock().unwrap().eof = true;
    ring.ready.notify_one();
}

/// A running (or finished) child process.
pub struct Process {
    pid: u32,
    stdout: Output,
    /// `None` on a pty, where stderr is the terminal too.
    stderr: Option<Output>,
    stdin: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    pty: bool,
    /// A handle on the pty's master side, for resizing.
    terminal: Option<std::os::fd::OwnedFd>,
    status: watch::Receiver<Option<ExitStatus>>,
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.status.borrow().is_none() {
            let _ = self.signal(Signal::KILL);
        }
    }
}

/// How a child ended: its exit code, or the signal that killed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Exit {
    Code(i32),
    Signal(i32),
}

impl From<ExitStatus> for Exit {
    fn from(s: ExitStatus) -> Exit {
        match (s.code(), s.signal()) {
            (Some(c), _) => Exit::Code(c),
            (None, Some(sig)) => Exit::Signal(sig),
            (None, None) => Exit::Code(-1),
        }
    }
}

impl IntoValue for Exit {
    /// An exit code, or `(signal N)`.
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        match self {
            Exit::Code(c) => (c as i64).into_value(vm),
            Exit::Signal(s) => {
                let items = [Value::symbol(techne_vm::reader::intern("signal")), Value::int_unchecked(s as i64)];
                Ok(vm.make_list(&items))
            }
        }
    }
}

/// A chunk of output, or (`None`) the end of the stream.
struct Chunk<T>(Option<T>);

impl<T: IntoValue> IntoValue for Chunk<T> {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        match self.0 {
            Some(s) => s.into_value(vm),
            None => Ok(Value::EOF),
        }
    }
}

/// An output stream of a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    pub fn named(name: &str) -> Result<Stream, String> {
        match name {
            "stdout" | "output" => Ok(Stream::Stdout),
            "stderr" => Ok(Stream::Stderr),
            _ => Err(format!("unknown stream {name}; expected stdout or stderr")),
        }
    }
}

/// A future run by the VM thread's scheduler (it need not be `Send`).
pub type LocalFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>>>>;

/// What Lisp process objects run on: a local `Process` or a remote one.
pub trait ProcessBackend {
    fn pid(&self) -> i64;
    /// The next chunk of output; `None` at end of file.
    fn read(&self, stream: Stream) -> LocalFuture<Option<Vec<u8>>>;
    fn write(&self, data: Vec<u8>) -> LocalFuture<()>;
    fn close_input(&self) -> LocalFuture<()>;
    fn signal(&self, signal: String) -> LocalFuture<()>;
    /// Set a pty's size.
    fn resize(&self, rows: u16, cols: u16) -> LocalFuture<()>;
    /// Output bytes dropped unread (persistent processes on a node).
    fn dropped(&self) -> LocalFuture<i64>;
    fn wait(&self) -> LocalFuture<Exit>;
    fn exited(&self) -> LocalFuture<bool>;
    /// Whether it is known to have exited, without waiting (for a scope
    /// forgetting finished processes; a remote process may answer late).
    fn known_exited(&self) -> bool {
        false
    }
    /// Kill the process group now if it still runs; never waits (used by
    /// cleanup code that cannot suspend).
    fn kill(&self);
}

/// A Lisp process object: a backend, and per output stream the start of a
/// UTF-8 sequence `process-read` has read but not yet decoded.
#[derive(Clone)]
pub struct ProcessRef {
    pub backend: Rc<dyn ProcessBackend>,
    pending: Rc<RefCell<[Vec<u8>; 2]>>,
}

impl ProcessRef {
    pub fn new(backend: Rc<dyn ProcessBackend>) -> ProcessRef {
        ProcessRef { backend, pending: Rc::default() }
    }

    /// The next chunk of `stream` as text; `None` at end of file.
    fn read_text(&self, stream: Stream) -> LocalFuture<Option<String>> {
        let p = self.clone();
        Box::pin(async move {
            loop {
                let chunk = p.backend.read(stream).await?;
                let mut pending = p.pending.borrow_mut();
                let carry = &mut pending[stream as usize];
                let Some(bytes) = chunk else {
                    // An incomplete sequence at the end is invalid.
                    let rest = std::mem::take(carry);
                    return Ok((!rest.is_empty()).then(|| String::from_utf8_lossy(&rest).into_owned()));
                };
                carry.extend_from_slice(&bytes);
                let keep = incomplete_tail(carry);
                let tail = carry.split_off(carry.len() - keep);
                let text = String::from_utf8_lossy(carry).into_owned();
                *carry = tail;
                if !text.is_empty() {
                    return Ok(Some(text));
                }
            }
        })
    }

    /// The next chunk of `stream` as bytes, starting with what a text read
    /// left undecoded; `None` at end of file.
    fn read_bytes(&self, stream: Stream) -> LocalFuture<Option<Vec<u8>>> {
        let p = self.clone();
        Box::pin(async move {
            let carry = std::mem::take(&mut p.pending.borrow_mut()[stream as usize]);
            if !carry.is_empty() {
                return Ok(Some(carry));
            }
            p.backend.read(stream).await
        })
    }
}

impl ProcessBackend for Arc<Process> {
    fn pid(&self) -> i64 {
        self.pid as i64
    }
    fn read(&self, stream: Stream) -> LocalFuture<Option<Vec<u8>>> {
        let p = self.clone();
        Box::pin(async move { Process::read(&p, stream).await })
    }
    fn write(&self, data: Vec<u8>) -> LocalFuture<()> {
        let p = self.clone();
        Box::pin(async move { Process::write(&p, data).await })
    }
    fn close_input(&self) -> LocalFuture<()> {
        Process::close_input(self);
        Box::pin(async { Ok(()) })
    }
    fn signal(&self, signal: String) -> LocalFuture<()> {
        let result = signal_named(&signal).and_then(|s| Process::signal(self, s).map_err(|e| e.to_string()));
        Box::pin(async move { result })
    }
    fn resize(&self, rows: u16, cols: u16) -> LocalFuture<()> {
        let result = Process::resize(self, rows, cols);
        Box::pin(async move { result })
    }
    fn dropped(&self) -> LocalFuture<i64> {
        let dropped = Process::dropped(self) as i64;
        Box::pin(async move { Ok(dropped) })
    }
    fn wait(&self) -> LocalFuture<Exit> {
        let p = self.clone();
        Box::pin(async move { Ok(Process::wait(&p).await) })
    }
    fn exited(&self) -> LocalFuture<bool> {
        let exited = Process::exited(self);
        Box::pin(async move { Ok(exited) })
    }
    fn known_exited(&self) -> bool {
        Process::exited(self)
    }
    fn kill(&self) {
        Process::kill(self);
    }
}

/// Bytes at the end of `data` that start an incomplete UTF-8 sequence.
fn incomplete_tail(data: &[u8]) -> usize {
    for back in 1..=3.min(data.len()) {
        let b = data[data.len() - back];
        if b & 0xC0 != 0x80 {
            // A lead byte: how long its sequence is.
            let len = match b {
                b if b >= 0xF0 => 4,
                b if b >= 0xE0 => 3,
                b if b >= 0xC0 => 2,
                _ => 1,
            };
            return if len > back { back } else { 0 };
        }
    }
    0
}

/// Read `reader` into `tx` until EOF, waiting whenever the queue is full. A
/// failed read ends the stream with its error, except EIO on a pty (`pty`),
/// which is how a pty's output ends once its child has gone.
async fn pump(mut reader: impl AsyncRead + Unpin, tx: mpsc::Sender<Item>, pty: bool) {
    let mut buf = vec![0u8; CHUNK_BYTES];
    loop {
        let item = match reader.read(&mut buf).await {
            Ok(0) => return,
            Ok(n) => Ok(buf[..n].to_vec()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if pty && e.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()) => return,
            Err(e) => Err(format!("process-read: {e}")),
        };
        let failed = item.is_err();
        if tx.send(item).await.is_err() || failed {
            return;
        }
    }
}

/// Write what arrives on `rx` to `writer`; closing the channel closes the
/// child's input. On a pty it sends end-of-file instead, and keeps the
/// terminal open until the child exits: closing the last handle to the
/// master hangs up the session, and the child would die of SIGHUP when it
/// closes its own end before exiting (as `cat` does).
async fn feed(mut writer: impl AsyncWrite + Unpin, mut rx: mpsc::Receiver<Vec<u8>>, exit: Option<watch::Receiver<Option<ExitStatus>>>) {
    while let Some(data) = rx.recv().await {
        if writer.write_all(&data).await.is_err() {
            break;
        }
    }
    match exit {
        Some(mut exit) => {
            let _ = writer.write_all(b"\x04").await;
            let _ = exit.wait_for(|s| s.is_some()).await;
        }
        None => {
            let _ = writer.shutdown().await;
        }
    }
}

fn output(retain: Retain) -> (mpsc::Sender<Item>, Output) {
    let (tx, rx) = mpsc::channel(QUEUE_CHUNKS);
    match retain {
        Retain::Queue => (tx, Output::Queue(Arc::new(tokio::sync::Mutex::new(rx)))),
        Retain::Ring(cap) => {
            let ring = Arc::new(Ring::default());
            runtime().spawn(fill(rx, ring.clone(), cap));
            (tx, Output::Ring(ring))
        }
    }
}

impl Process {
    /// Start `program` with `args`, on a pty (24×80) or with pipes.
    pub fn spawn(program: &str, args: &[String], pty: bool) -> std::io::Result<Process> {
        Process::spawn_with(program, args, pty, Retain::Queue)
    }

    /// `spawn`, with a choice of what happens to unread output.
    pub fn spawn_with(program: &str, args: &[String], pty: bool, retain: Retain) -> std::io::Result<Process> {
        let rt = runtime();
        let _enter = rt.enter();
        let (stdin_tx, stdin_rx) = mpsc::channel::<Vec<u8>>(QUEUE_CHUNKS);
        let (out_tx, stdout) = output(retain);
        let (status_tx, status) = watch::channel(None);
        let (mut child, stderr, terminal) = if pty {
            let (terminal, pts) = pty_process::open().map_err(std::io::Error::other)?;
            terminal.resize(pty_process::Size::new(24, 80)).map_err(std::io::Error::other)?;
            let handle = rustix::io::dup(&terminal)?;
            let child = pty_process::Command::new(program).args(args).spawn(pts).map_err(std::io::Error::other)?;
            let (read, write) = terminal.into_split();
            rt.spawn(pump(read, out_tx, true));
            rt.spawn(feed(write, stdin_rx, Some(status.clone())));
            (child, None, Some(handle))
        } else {
            let mut child = tokio::process::Command::new(program)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .process_group(0)
                .spawn()?;
            let (err_tx, stderr) = output(retain);
            rt.spawn(pump(child.stdout.take().unwrap(), out_tx, false));
            rt.spawn(pump(child.stderr.take().unwrap(), err_tx, false));
            rt.spawn(feed(child.stdin.take().unwrap(), stdin_rx, None));
            (child, Some(stderr), None)
        };
        let pid = child.id().unwrap_or(0);
        rt.spawn(async move {
            if let Ok(s) = child.wait().await {
                let _ = status_tx.send(Some(s));
            }
        });
        Ok(Process { pid, stdout, stderr, stdin: Mutex::new(Some(stdin_tx)), pty, terminal, status })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Signal the child's process group.
    pub fn signal(&self, sig: Signal) -> std::io::Result<()> {
        let pid = Pid::from_raw(self.pid as i32).ok_or_else(|| std::io::Error::other("no pid"))?;
        Ok(kill_process_group(pid, sig)?)
    }

    fn stream(&self, which: Stream) -> Result<Output, String> {
        match which {
            Stream::Stdout => Ok(self.stdout.clone()),
            Stream::Stderr if self.pty => Err("a pty process has one output stream; read 'stdout".into()),
            Stream::Stderr => Ok(self.stderr.clone().unwrap()),
        }
    }

    /// The next chunk of `stream`; `None` at end of file.
    pub async fn read(&self, stream: Stream) -> Result<Option<Vec<u8>>, String> {
        self.stream(stream)?.read().await
    }

    /// Bytes of output dropped because nobody read them in time (only with
    /// `Retain::Ring`).
    pub fn dropped(&self) -> u64 {
        self.stdout.dropped() + self.stderr.as_ref().map_or(0, Output::dropped)
    }

    /// Queue `data` for the child's input (waits while the queue is full).
    pub async fn write(&self, data: Vec<u8>) -> Result<(), String> {
        let input = self.stdin.lock().unwrap().clone().ok_or("process-write: input is closed")?;
        input.send(data).await.map_err(|_| "process-write: the process closed its input".to_string())
    }

    pub fn close_input(&self) {
        self.stdin.lock().unwrap().take();
    }

    /// Set the pty's size; the child gets SIGWINCH.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        let terminal = self.terminal.as_ref().ok_or("process-resize: not a pty process")?;
        let size = rustix::termios::Winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        rustix::termios::tcsetwinsize(terminal, size).map_err(|e| format!("process-resize: {e}"))
    }

    /// How it ended, once it has.
    pub fn exit(&self) -> Option<Exit> {
        self.status.borrow().map(Exit::from)
    }

    pub fn exited(&self) -> bool {
        self.status.borrow().is_some()
    }

    /// Kill the process group if it still runs.
    pub fn kill(&self) {
        if !self.exited() {
            let _ = self.signal(Signal::KILL);
        }
    }

    /// Wait for the child to exit.
    pub async fn wait(&self) -> Exit {
        let mut status = self.status.clone();
        let s = *status.wait_for(|s| s.is_some()).await.expect("status sender lives until exit");
        s.expect("waited for Some").into()
    }
}

pub fn signal_named(name: &str) -> Result<Signal, String> {
    Ok(match name {
        "int" | "interrupt" => Signal::INT,
        "term" | "terminate" => Signal::TERM,
        "kill" => Signal::KILL,
        "hup" | "hangup" => Signal::HUP,
        "stop" => Signal::STOP,
        "cont" | "continue" => Signal::CONT,
        _ => return Err(format!("unknown signal {name}")),
    })
}

/// Scheme wrappers over the natives.
const PRELUDE: &str = r#"
(define (process-spawn program args #:pty [pty #f] #:node [node #f] #:persist [persist #f])
  "Start PROGRAM with the list of strings ARGS, with pipes or (#:pty #t) on a terminal, here or on NODE.
With #:persist #t (on a node session) it survives disconnects, keeping its newest output.
The current scope owns a local process: shutting the scope kills it."
  (cond (node (%node-process-spawn node program args pty persist))
        (persist (error "process-spawn: #:persist needs #:node (a node session keeps the process)"))
        (else (scope-own! (%process-spawn program args pty) process-kill %process-exited?))))

(define (call-with-process program args f #:pty [pty #f] #:node [node #f] #:persist [persist #f])
  "Call F with a new process; the process is killed when F returns, fails or its task is cancelled."
  (let ((p (process-spawn program args #:pty pty #:node node #:persist persist)))
    (dynamic-wind (lambda () #f) (lambda () (f p)) (lambda () (process-kill p) (scope-disown! p)))))

(define (process-read-all p stream)
  "Everything STREAM ('stdout or 'stderr) of P outputs until end of file, as text."
  (let loop ((chunks '()))
    (let ((c (process-read p stream)))
      (if (eof-object? c) (apply string-append (reverse chunks)) (loop (cons c chunks))))))

(define (process-read-all-bytes p stream)
  "Everything STREAM ('stdout or 'stderr) of P outputs until end of file, as a bytevector."
  (let loop ((chunks '()))
    (let ((c (process-read-bytes p stream)))
      (if (eof-object? c) (apply bytevector-append (reverse chunks)) (loop (cons c chunks))))))
"#;

fn process(vm: &mut Vm, v: Value) -> Result<ProcessRef, Error> {
    let p: Foreign<ProcessRef> = vm.get(v)?;
    Ok(ProcessRef::clone(&p))
}

fn symbol(vm: &mut Vm, v: Value) -> Result<String, Error> {
    if v.is_symbol() { Ok(techne_vm::reader::symbol_name(v.as_symbol()).to_string()) } else { vm.get(v) }
}

/// Register an async native taking a process and `arity - 1` more arguments.
fn process_op<T, F>(vm: &mut Vm, name: &'static str, arity: usize, op: F)
where
    T: IntoValue + 'static,
    F: Fn(&mut Vm, ProcessRef, &[Value]) -> Result<LocalFuture<T>, Error> + 'static,
{
    vm.register_async(name, arity, move |vm: &mut Vm, args: &[Value]| {
        let fut = process(vm, args[0]).and_then(|p| op(vm, p, &args[1..]));
        async move { fut.map_err(|e| e.into_inner().msg)?.await }
    });
}

/// Define the process procedures in `vm`; in a world without the processes
/// capability they only raise "not granted".
pub fn install(vm: &mut Vm) -> Result<(), Error> {
    vm.requiring(techne_vm::vm::Capability::Processes, natives);
    // In the root module, so every module sees it.
    vm.eval_in(techne_vm::vm::ROOT_MODULE, "<techne-process>", PRELUDE).map(|_| ())
}

fn natives(vm: &mut Vm) {
    vm.name_foreign_type::<ProcessRef>("process");
    vm.register_fn("%process-spawn", |program: String, args: Vec<String>, pty: bool| -> Result<Foreign<ProcessRef>, String> {
        let p = Process::spawn(&program, &args, pty).map_err(|e| format!("process-spawn: {program}: {e}"))?;
        Ok(Foreign::new(ProcessRef::new(Rc::new(Arc::new(p)))))
    });
    // Replaced by techne-node's `install`.
    vm.register_fn("%node-process-spawn", |_: techne_vm::api::Root, _: String, _: Vec<String>, _: bool, _: bool| -> Result<(), String> {
        Err("process-spawn: #:node needs the node library (techne-node)".into())
    });
    vm.register_fn("process-pid", |p: Foreign<ProcessRef>| p.0.backend.pid());
    vm.register_fn("process-kill", |p: Foreign<ProcessRef>| p.0.backend.kill());
    vm.register_fn("%process-exited?", |p: Foreign<ProcessRef>| p.0.backend.known_exited());
    process_op(vm, "process-read", 2, |vm, p, args| {
        let stream = Stream::named(&symbol(vm, args[0])?).map_err(Error::new)?;
        let read = p.read_text(stream);
        Ok(Box::pin(async move { read.await.map(Chunk) }) as LocalFuture<Chunk<String>>)
    });
    process_op(vm, "process-read-bytes", 2, |vm, p, args| {
        let stream = Stream::named(&symbol(vm, args[0])?).map_err(Error::new)?;
        let read = p.read_bytes(stream);
        Ok(Box::pin(async move { read.await.map(|c| Chunk(c.map(Bytes))) }) as LocalFuture<Chunk<Bytes>>)
    });
    process_op(vm, "process-write", 2, |vm, p, args| {
        let Bytes(data) = vm.get(args[0])?;
        Ok(p.backend.write(data))
    });
    process_op(vm, "process-close-input", 1, |_, p, _| Ok(p.backend.close_input()));
    process_op(vm, "process-signal", 2, |vm, p, args| Ok(p.backend.signal(symbol(vm, args[0])?)));
    process_op(vm, "process-resize", 3, |vm, p, args| {
        let (rows, cols): (i64, i64) = (vm.get(args[0])?, vm.get(args[1])?);
        let size = |n: i64| u16::try_from(n).map_err(|_| Error::new(format!("process-resize: bad size {n}")));
        Ok(p.backend.resize(size(rows)?, size(cols)?))
    });
    process_op(vm, "process-wait", 1, |_, p, _| Ok(p.backend.wait()));
    process_op(vm, "process-dropped", 1, |_, p, _| Ok(p.backend.dropped()));
    process_op(vm, "process-exited?", 1, |_, p, _| Ok(p.backend.exited()));
}

#[cfg(test)]
mod tests {
    use super::incomplete_tail;

    #[test]
    fn utf8_tails() {
        let s = "aé€😀".as_bytes();
        assert_eq!(incomplete_tail(s), 0);
        assert_eq!(incomplete_tail(&s[..s.len() - 1]), 3);
        assert_eq!(incomplete_tail(&s[..s.len() - 2]), 2);
        assert_eq!(incomplete_tail(&s[..s.len() - 3]), 1);
        assert_eq!(incomplete_tail(&s[..2]), 1);
        assert_eq!(incomplete_tail(b"abc"), 0);
    }
}
