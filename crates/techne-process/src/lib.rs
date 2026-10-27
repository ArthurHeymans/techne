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
//!   body returns, fails or its task is cancelled. Waiting observes the
//!   leader's exit without reaping it: its pid reserves the group id until
//!   kill or drop has signalled the group, or nothing runs in the group any
//!   more (`Process::finished`). Later signals cannot hit a reused
//!   group id. This needs Linux pidfds (Linux 5.3 or later). Children that
//!   leave the group are outside this contract; confining those needs a
//!   cgroup, not process-group signals. No other code may reap these children
//!   (including a SIGCHLD handler that reaps all children).
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
    os::unix::process::{CommandExt, ExitStatusExt},
    pin::Pin,
    process::{ExitStatus, Stdio},
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
};

use rustix::process::{Pid, Signal, kill_process_group};
use techne_vm::{
    api::{Args, Bytes, Foreign, IntoValue, Value},
    vm::{Error, Vm},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot, watch},
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
    /// While present, the observer keeps the leader unreaped. Taking it
    /// ends signalling before the observer may release the pid.
    group: Mutex<Option<oneshot::Sender<()>>>,
}

impl Drop for Process {
    fn drop(&mut self) {
        self.kill();
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
                let items = [Value::symbol("signal"), Value::from(s)];
                vm.make_list(&items)
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
    /// Whether cleanup is finished, without waiting (for a scope forgetting
    /// released jobs). A leader's exit alone does not release its group
    /// while its descendants run.
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
        Process::finished(self)
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

/// Where a process starts: its working directory (the parent's when
/// `None`) and variables set in its environment, besides those it
/// inherits.
#[derive(Clone, Debug, Default)]
pub struct Place {
    pub directory: Option<std::path::PathBuf>,
    pub environment: Vec<(String, String)>,
}

impl Process {
    /// Start `program` with `args`, on a pty (24×80) or with pipes.
    pub fn spawn(program: &str, args: &[String], pty: bool) -> std::io::Result<Process> {
        Process::spawn_with(program, args, pty, Retain::Queue)
    }

    /// `spawn`, with a choice of what happens to unread output.
    pub fn spawn_with(program: &str, args: &[String], pty: bool, retain: Retain) -> std::io::Result<Process> {
        Process::spawn_in(program, args, pty, retain, &Place::default())
    }

    /// `spawn_with`, in `place`'s directory, with its environment.
    pub fn spawn_in(program: &str, args: &[String], pty: bool, retain: Retain, place: &Place) -> std::io::Result<Process> {
        let rt = runtime();
        let _enter = rt.enter();
        let (stdin_tx, stdin_rx) = mpsc::channel::<Vec<u8>>(QUEUE_CHUNKS);
        let (out_tx, stdout) = output(retain);
        let (status_tx, status) = watch::channel(None);
        // Only the pipes use tokio's process types. A tokio Child reaps on
        // wait, or on drop through its orphan reaper, releasing the pid too
        // soon for signals to descendants after the leader's exit.
        let (mut child, terminal) = if pty {
            let (terminal, pts) = pty_process::open().map_err(std::io::Error::other)?;
            terminal.resize(pty_process::Size::new(24, 80)).map_err(std::io::Error::other)?;
            let handle = rustix::io::dup(&terminal)?;
            // SAFETY: a duplicate of the child end of this pty.
            let pts = unsafe { pty_process::blocking::Pts::from_fd(rustix::io::dup(&pts)?) };
            let mut command = pty_process::blocking::Command::new(program).args(args).envs(place.environment.iter().cloned());
            if let Some(dir) = &place.directory {
                command = command.current_dir(dir);
            }
            let child = command.spawn(pts).map_err(std::io::Error::other)?;
            (child, Some((terminal, handle)))
        } else {
            let mut command = std::process::Command::new(program);
            if let Some(dir) = &place.directory {
                command.current_dir(dir);
            }
            let child = command
                .args(args)
                .envs(place.environment.iter().cloned())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .process_group(0)
                .spawn()?;
            (child, None)
        };
        let pid = child.id();
        let group_pid = Pid::from_raw(pid as i32).expect("child pid");
        let setup = (|| {
            let fd = rustix::process::pidfd_open(group_pid, rustix::process::PidfdFlags::empty())?;
            let fd = tokio::io::unix::AsyncFd::new(fd)?;
            let (stderr, terminal) = if let Some((terminal, handle)) = terminal {
                let (read, write) = terminal.into_split();
                rt.spawn(pump(read, out_tx, true));
                rt.spawn(feed(write, stdin_rx, Some(status.clone())));
                (None, Some(handle))
            } else {
                let read = tokio::process::ChildStdout::from_std(child.stdout.take().unwrap())?;
                let err = tokio::process::ChildStderr::from_std(child.stderr.take().unwrap())?;
                let write = tokio::process::ChildStdin::from_std(child.stdin.take().unwrap())?;
                let (err_tx, stderr) = output(retain);
                rt.spawn(pump(read, out_tx, false));
                rt.spawn(pump(err, err_tx, false));
                rt.spawn(feed(write, stdin_rx, None));
                (Some(stderr), None)
            };
            Ok::<_, std::io::Error>((fd, stderr, terminal))
        })();
        let (fd, stderr, terminal) = match setup {
            Ok(parts) => parts,
            Err(e) => {
                let _ = kill_process_group(group_pid, Signal::KILL);
                rt.spawn_blocking(move || child.wait());
                return Err(e);
            }
        };
        let (release, released) = oneshot::channel();
        rt.spawn(async move {
            if fd.readable().await.is_ok() {
                let observed = loop {
                    match rustix::process::waitid(
                        rustix::process::WaitId::Pid(group_pid),
                        rustix::process::WaitIdOptions::EXITED | rustix::process::WaitIdOptions::NOWAIT,
                    ) {
                        Err(rustix::io::Errno::INTR) => continue,
                        result => break result,
                    }
                };
                if let Ok(Some(s)) = observed {
                    let raw = s.exit_status().map_or_else(|| s.terminating_signal().unwrap_or(0), |code| code << 8);
                    let _ = status_tx.send(Some(ExitStatus::from_raw(raw)));
                }
            }
            // kill/drop signals under the group lock before releasing us.
            // Keep status_tx alive too, so late waiters can observe exit.
            let _ = released.await;
            let _ = tokio::task::spawn_blocking(move || child.wait()).await;
            drop(status_tx);
        });
        Ok(Process { pid, stdout, stderr, stdin: Mutex::new(Some(stdin_tx)), pty, terminal, status, group: Mutex::new(Some(release)) })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Signal the child's process group.
    pub fn signal(&self, sig: Signal) -> std::io::Result<()> {
        let group = self.group.lock().unwrap();
        if group.is_none() {
            return Err(std::io::Error::from_raw_os_error(rustix::io::Errno::SRCH.raw_os_error()));
        }
        let pid = Pid::from_raw(self.pid as i32).expect("child pid");
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

    /// Kill the process group, including descendants of an exited leader,
    /// and release it. Later signals fail rather than use a stale group id.
    pub fn kill(&self) {
        let mut group = self.group.lock().unwrap();
        if let Some(release) = group.take() {
            let _ = kill_process_group(Pid::from_raw(self.pid as i32).expect("child pid"), Signal::KILL);
            let _ = release.send(());
        }
    }

    /// Whether the job is over: released, or its leader exited with nothing
    /// left running in its group, which then is released (the leader
    /// reaped). A long-lived scope forgets such a job, as before the leader
    /// was kept. While the leader is unreaped, its group id is not reused,
    /// and only a process of this session could join the group.
    pub fn finished(&self) -> bool {
        let mut group = self.group.lock().unwrap();
        if group.is_some() && self.exited() && !group_runs(self.pid) {
            let _ = group.take().map(|release| release.send(()));
        }
        group.is_none()
    }

    /// Observe the leader's exit without releasing its process group.
    pub async fn wait(&self) -> Exit {
        let mut status = self.status.clone();
        let s = *status.wait_for(|s| s.is_some()).await.expect("status sender lives until exit");
        s.expect("waited for Some").into()
    }
}

/// Whether something runs in process group `group`, by `/proc`: a
/// process in it with a thread that is not a zombie (its main thread may
/// have ended before the others). What cannot be read, but did not go,
/// counts as running: releasing a group too soon would leave it unowned.
fn group_runs(group: u32) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else { return true };
    let group = group.to_string();
    entries.filter_map(Result::ok).filter(|e| e.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))).any(|e| {
        match stat(&e.path()) {
            Gone => false,
            Unknown => true,
            Known { state, pgrp } => pgrp == group.as_bytes() && (!matches!(state, b'Z' | b'X') || threads_run(&e.path())),
        }
    })
}

/// What `stat` in a `/proc` directory says of a process or thread.
enum Stat {
    Gone,
    Unknown,
    Known { state: u8, pgrp: Vec<u8> },
}
use Stat::{Gone, Known, Unknown};

fn stat(dir: &std::path::Path) -> Stat {
    match std::fs::read(dir.join("stat")) {
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) || e.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error()) => {
            Gone
        }
        Err(_) => Unknown,
        // Bytes: after the command, which may hold anything (not UTF-8,
        // parentheses), in parentheses: the state, the parent, the group.
        Ok(bytes) => {
            let rest = bytes.iter().rposition(|&b| b == b')').map_or(&[][..], |i| &bytes[i + 1..]);
            let mut fields = rest.split(|b| b.is_ascii_whitespace()).filter(|f| !f.is_empty());
            match (fields.next(), fields.nth(1)) {
                (Some(&[state]), Some(pgrp)) => Known { state, pgrp: pgrp.to_vec() },
                _ => Unknown,
            }
        }
    }
}

/// Whether a thread of the process at `dir` is not a zombie.
fn threads_run(dir: &std::path::Path) -> bool {
    let Ok(tasks) = std::fs::read_dir(dir.join("task")) else { return true };
    tasks.filter_map(Result::ok).any(|t| match stat(&t.path()) {
        Gone => false,
        Unknown => true,
        Known { state, .. } => !matches!(state, b'Z' | b'X'),
    })
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
(define (process-spawn program args #:pty [pty #f] #:node [node #f] #:persist [persist #f]
                       #:directory [directory #f] #:environment [environment '()])
  "Start PROGRAM with the list of strings ARGS; return the process.
It runs with pipes or, with PTY, on a terminal; here, or on NODE. With
PERSIST, on a node session, it survives disconnects, keeping its newest
output. It starts in DIRECTORY, a path where it runs (by default the
directory of whoever starts it), with the variables of ENVIRONMENT, a
list of (NAME . VALUE) strings, set besides those it inherits. The
current scope owns a local process: shutting the scope kills it."
  (cond (node (%node-process-spawn node program args pty persist (cons directory environment)))
        (persist (error "process-spawn: #:persist needs #:node (a node session keeps the process)"))
        (else (scope-own! (%process-spawn program args pty directory
                                          (map (lambda (p) (list (car p) (cdr p))) environment))
                          process-kill %process-exited?))))

(define (call-with-process program args f #:pty [pty #f] #:node [node #f] #:persist [persist #f]
                           #:directory [directory #f] #:environment [environment '()])
  "Call F with a new process, killed when F returns, fails or is cancelled.
PROGRAM, ARGS, PTY, NODE, PERSIST, DIRECTORY and ENVIRONMENT are as
`process-spawn` takes them."
  (let ((p (process-spawn program args #:pty pty #:node node #:persist persist
                          #:directory directory #:environment environment)))
    (dynamic-wind (lambda () #f) (lambda () (f p)) (lambda () (process-kill p) (scope-disown! p)))))

(define (process-read-all p stream)
  "Return everything STREAM, `stdout` or `stderr`, of P outputs, as text.
It reads to the end of the stream."
  (let loop ((chunks '()))
    (let ((c (process-read p stream)))
      (if (eof-object? c) (apply string-append (reverse chunks)) (loop (cons c chunks))))))

(define (process-read-all-bytes p stream)
  "Return everything STREAM, `stdout` or `stderr`, of P outputs, as bytes.
It reads to the end of the stream; the result is a bytevector."
  (let loop ((chunks '()))
    (let ((c (process-read-bytes p stream)))
      (if (eof-object? c) (apply bytevector-append (reverse chunks)) (loop (cons c chunks))))))
"#;

/// Register an async native taking a process and `arity - 1` more arguments.
fn process_op<T, F>(vm: &mut Vm, name: &'static str, arity: usize, op: F)
where
    T: IntoValue + 'static,
    F: Fn(&mut Vm, ProcessRef, Args<'_>) -> Result<LocalFuture<T>, Error> + 'static,
{
    vm.register_async(name, arity, move |vm: &mut Vm, args: Args<'_>| {
        let fut = args.get::<Foreign<ProcessRef>>(vm, 0).and_then(|p| op(vm, ProcessRef::clone(&p), args.rest(1)));
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
    techne_vm::procedures! { vm;
        "(%process-spawn program args pty directory environment)" => |program: String, args: Vec<String>, pty: bool, directory: Option<String>, environment: Vec<Vec<String>>| -> Result<Foreign<ProcessRef>, String> {
            let environment = environment.into_iter().map(|p| match <[String; 2]>::try_from(p) {
                Ok([name, value]) => Ok((name, value)),
                Err(_) => Err("process-spawn: an environment is a list of (NAME VALUE)".to_string()),
            }).collect::<Result<Vec<_>, _>>()?;
            let place = Place { directory: directory.map(std::path::PathBuf::from), environment };
            let p = Process::spawn_in(&program, &args, pty, Retain::Queue, &place).map_err(|e| format!("process-spawn: {program}: {e}"))?;
            Ok(Foreign::new(ProcessRef::new(Rc::new(Arc::new(p)))))
        };
    }
    // Replaced by techne-node's `install`.
    techne_vm::procedures! { vm;
        "(%node-process-spawn node program args pty persist place)" => |_: Value, _: String, _: Vec<String>, _: bool, _: bool, _: Value| -> Result<(), String> {
            Err("process-spawn: #:node needs the node library (techne-node)".into())
        };
    }
    techne_vm::procedures! { vm;
        /// Return the operating system's id of PROCESS.
        "(process-pid process)" => |p: Foreign<ProcessRef>| p.0.backend.pid();
    }
    techne_vm::procedures! { vm;
        /// Kill PROCESS and its process group.
        "(process-kill process)" => |p: Foreign<ProcessRef>| p.0.backend.kill();
    }
    techne_vm::procedures! { vm;
        "(%process-exited? process)" => |p: Foreign<ProcessRef>| p.0.backend.known_exited();
    }
    process_op(vm, "process-read", 2, |vm, p, args| {
        let stream = Stream::named(&args.get::<String>(vm, 0)?).map_err(Error::new)?;
        let read = p.read_text(stream);
        Ok(Box::pin(async move { read.await.map(Chunk) }) as LocalFuture<Chunk<String>>)
    });
    process_op(vm, "process-read-bytes", 2, |vm, p, args| {
        let stream = Stream::named(&args.get::<String>(vm, 0)?).map_err(Error::new)?;
        let read = p.read_bytes(stream);
        Ok(Box::pin(async move { read.await.map(|c| Chunk(c.map(Bytes))) }) as LocalFuture<Chunk<Bytes>>)
    });
    process_op(vm, "process-write", 2, |vm, p, args| {
        let Bytes(data) = args.get(vm, 0)?;
        Ok(p.backend.write(data))
    });
    process_op(vm, "process-close-input", 1, |_, p, _| Ok(p.backend.close_input()));
    process_op(vm, "process-signal", 2, |vm, p, args| Ok(p.backend.signal(args.get::<String>(vm, 0)?)));
    process_op(vm, "process-resize", 3, |vm, p, args| {
        let (rows, cols): (i64, i64) = (args.get(vm, 0)?, args.get(vm, 1)?);
        let size = |n: i64| u16::try_from(n).map_err(|_| Error::new(format!("process-resize: bad size {n}")));
        Ok(p.backend.resize(size(rows)?, size(cols)?))
    });
    process_op(vm, "process-wait", 1, |_, p, _| Ok(p.backend.wait()));
    process_op(vm, "process-dropped", 1, |_, p, _| Ok(p.backend.dropped()));
    process_op(vm, "process-exited?", 1, |_, p, _| Ok(p.backend.exited()));
    techne_vm::document! { vm;
        /// Wait for the next output of PROCESS on STREAM, `stdout` or `stderr`.
        /// Return it as a string, or the eof object once the stream is closed.
        "(process-read process stream)";
        /// Wait for the next output of PROCESS on STREAM, as a bytevector.
        /// STREAM is `stdout` or `stderr`; once it is closed, return the eof object.
        "(process-read-bytes process stream)";
        /// Write DATA, a string or bytevector, to the input of PROCESS.
        /// It waits while the input is full.
        "(process-write process data)";
        /// Close the input of PROCESS, which then reads end of file.
        "(process-close-input process)";
        /// Send SIGNAL to PROCESS's group.
        /// SIGNAL is one of the symbols `int`, `term`, `kill`, `hup`, `stop` and
        /// `cont`.
        "(process-signal process signal)";
        /// Tell PROCESS, on a terminal, that it has ROWS and COLUMNS.
        "(process-resize process rows columns)";
        /// Wait for PROCESS to end; return its exit code, or (signal N).
        "(process-wait process)";
        /// Return how many bytes of PROCESS's output were dropped, unread.
        "(process-dropped process)";
        /// Return #t if PROCESS has ended.
        "(process-exited? process)";
    }
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
