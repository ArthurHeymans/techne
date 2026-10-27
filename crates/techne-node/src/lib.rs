//! Nodes: evaluation, inspection and processes on another machine.
//!
//! A node is a `techne-node` process reached through a transport command
//! (`ssh host techne-node`, or just `techne-node` for a local one) and spoken
//! to with the framed protocol in `protocol`. In Lisp:
//!
//! ```scheme
//! (define n (node-connect '("ssh" "build-box" "techne-node" "--session" "work")))
//! (node-eval n "(define (f x) \"Doc.\" (* x 2)) (f 21)")   ; => 42
//! (node-describe n 'f)                                    ; => "(f x)  procedure, ...\n\nDoc."
//! (node-eval n "(f 1)" #:module "lib/util.scm")           ; in that module of the node
//! (define g (node-eval n "f"))                            ; a remote value
//! (node-apply n g 5)                                      ; => 10
//! (call-with-process "make" '("-j8") (lambda (p) (process-read-all p 'stdout)) #:node n)
//! (process-spawn "make" '("-j8") #:node n #:persist #t)   ; survives disconnects
//! ;; later, from a new connection:
//! (node-processes n)        ; => (((proc . 1) (pid . 4242) (command "make" "-j8") ...))
//! (define p (node-process n 1))
//! ```
//!
//! - Data values cross as their written form: numbers, strings, bytevectors,
//!   symbols, keywords, characters, booleans, pairs and vectors. Both ends
//!   use `Vm::to_written_data`, with a 1 MiB budget for parts and text,
//!   a 1 MiB written form and the reader's depth limit. Other results,
//!   cycles and values past these limits become remote values: handles
//!   the node keeps until the client's handle is garbage-collected.
//!   Their written preview is bounded, or a fixed marker at a data limit.
//!   `node-apply` takes data or remote values from the same node; local
//!   arguments outside the data limits raise an error. `node-describe`
//!   inspects a remote value.
//! - Remote processes are ordinary process objects: `process-read`,
//!   `process-write`, `process-wait`, `call-with-process`… work unchanged.
//! - If the transport dies, every pending and later operation on the node
//!   raises "node connection lost". A plain node (`techne-node`) kills its
//!   processes when its client goes. A session (`techne-node --session
//!   NAME`) keeps its Lisp state and its `#:persist` processes across
//!   connections: reconnect, `node-processes` to see what runs (status,
//!   output dropped while nobody read it), `node-process` to attach.

pub mod bencode;
pub mod cli;
pub mod fs;
pub mod nrepl;
pub mod protocol;

/// Budget for the parts and text of one data value, and its written form.
/// The conversion also refuses cycles and nesting past the reader's limit.
pub const MAX_DATA: usize = 1 << 20;

use std::{
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use protocol::{Message, Reply, Request, Response, handshake, read_frame, write_frame};
use techne_process::{Exit, LocalFuture, ProcessBackend, ProcessRef, Stream};
use techne_vm::{
    api::{Args, Foreign, FromValue, Value},
    vm::{Capability, Error, Vm},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
};

/// Whether a request failed definitively, or its remote outcome is unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// Rejected before sending, or answered by the node after it stopped.
    Failed(String),
    /// The reply was lost: the operation may still be running remotely.
    Unknown(String),
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(reason) | Self::Unknown(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for RequestError {}

/// The client side of a connection.
struct Conn {
    next: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Reply, RequestError>>>>,
    /// Why the connection is unusable, once it is.
    lost: Mutex<Option<String>>,
    /// To the writer task; `None` closes the node's input.
    out: mpsc::UnboundedSender<Option<Message<Request>>>,
    /// What the node reported it is, once its hello came (`None` inside
    /// when it said nothing: it is not a node).
    facts: Mutex<Option<Option<protocol::Facts>>>,
    /// Woken when the handshake ends, well or not.
    greeted: tokio::sync::Notify,
}

impl Conn {
    /// Send `request`; the future resolves with the node's answer.
    fn request(&self, request: Request) -> impl Future<Output = Result<Reply, String>> + use<> {
        let reply = self.request_tracked(request);
        async move { reply.await.map_err(|e| e.to_string()) }
    }

    fn request_tracked(&self, request: Request) -> impl Future<Output = Result<Reply, RequestError>> + use<> {
        let (tx, rx) = oneshot::channel();
        let sent = {
            let mut pending = self.pending.lock().unwrap();
            match self.lost.lock().unwrap().clone() {
                Some(reason) => Err(reason),
                None => {
                    let id = self.next.fetch_add(1, Ordering::Relaxed);
                    pending.insert(id, tx);
                    if self.out.send(Some(Message { id, body: request })).is_err() {
                        pending.remove(&id);
                        Err("node connection lost".to_string())
                    } else {
                        Ok(())
                    }
                }
            }
        };
        async move {
            sent.map_err(RequestError::Failed)?;
            rx.await.map_err(|_| RequestError::Unknown("node connection lost".to_string()))?
        }
    }

    /// Send `request` without waiting for the answer.
    fn notify(&self, request: Request) {
        let _ = self.out.send(Some(Message { id: 0, body: request }));
    }

    fn lose(&self, reason: String) {
        let mut pending = self.pending.lock().unwrap();
        self.lost.lock().unwrap().get_or_insert(reason.clone());
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(RequestError::Unknown(reason.clone())));
        }
        self.greeted.notify_waiters();
    }
}

/// A connected node; it is closed when the last handle (the node object or
/// one of its processes) is dropped.
pub struct Node {
    conn: Arc<Conn>,
    transport: Mutex<Option<Transport>>,
}

/// What carries a node's connection.
enum Transport {
    /// A local process: `techne-node`, or a command reaching one (ssh).
    Local(tokio::process::Child),
    /// A process `parent` runs, which reaches the node (sudo, a container).
    Via { parent: Arc<Node>, proc: u64 },
}

/// How much of what a node reached through another says on its standard
/// error is kept, to say why it failed.
const KEPT_STDERR: usize = 4096;

impl Node {
    /// Start `command` (program and arguments) and speak the protocol to it.
    pub fn connect(command: &[String]) -> std::io::Result<Node> {
        let program = command.first().ok_or_else(|| std::io::Error::other("empty node command"))?;
        let _rt = techne_process::runtime().enter();
        let mut child = tokio::process::Command::new(program)
            .args(&command[1..])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()?;
        let conn = speak(child.stdout.take().unwrap(), child.stdin.take().unwrap(), None);
        Ok(Node { conn, transport: Mutex::new(Some(Transport::Local(child))) })
    }

    /// Run `program` with `args` on `parent`, and speak the protocol to the
    /// node it starts (as a hop's method does: `sudo ... techne-node`).
    /// Its standard input and output carry the protocol, relayed by
    /// `parent`; what it says on its standard error is kept for an error.
    pub async fn connect_via(parent: Arc<Node>, program: String, args: Vec<String>) -> Result<Node, String> {
        let spawn = Request::Spawn { program, args, pty: false, persist: false, directory: None, environment: Vec::new() };
        let Reply::Spawned { proc, .. } = parent.conn.request(spawn).await? else {
            return Err("unexpected reply from node".into());
        };
        let _rt = techne_process::runtime().enter();
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let (from_node, to_node) = tokio::io::split(ours);
        let (mut relay_in, mut relay_out) = tokio::io::split(theirs);
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let conn = speak(from_node, to_node, Some(stderr.clone()));
        let reading = parent.conn.clone();
        let read = move |stream| {
            let conn = reading.clone();
            async move {
                match conn.request(Request::Read { proc, stream }).await {
                    Ok(Reply::Chunk(Some(c))) => Some(c),
                    _ => None,
                }
            }
        };
        // Its output, to the protocol's reader; its errors, kept.
        tokio::spawn({
            let read = read.clone();
            async move {
                use tokio::io::AsyncWriteExt;
                while let Some(chunk) = read(Stream::Stdout).await {
                    if relay_out.write_all(&chunk).await.is_err() {
                        break;
                    }
                }
                let _ = relay_out.shutdown().await;
            }
        });
        tokio::spawn(async move {
            while let Some(chunk) = read(Stream::Stderr).await {
                let mut kept = stderr.lock().unwrap();
                kept.extend_from_slice(&chunk);
                let excess = kept.len().saturating_sub(KEPT_STDERR);
                kept.drain(..excess);
            }
        });
        // What the protocol's writer sends, to its input. Failing to give
        // it loses the connection: its replies would never come.
        let input = parent.conn.clone();
        let inner = conn.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = vec![0; 64 * 1024];
            loop {
                match relay_in.read(&mut buf).await {
                    Ok(0) | Err(_) => {
                        let _ = input.request(Request::CloseInput { proc }).await;
                        break;
                    }
                    Ok(n) => {
                        if let Err(e) = input.request(Request::Write { proc, data: buf[..n].to_vec() }).await {
                            inner.lose(format!("node connection lost: {e}"));
                            input.notify(Request::Release { proc });
                            break;
                        }
                    }
                }
            }
        });
        Ok(Node { conn, transport: Mutex::new(Some(Transport::Via { parent, proc })) })
    }

    /// Close the connection: close the node's input, so it kills its
    /// processes and exits; kill the transport if it has not exited within
    /// `CLOSE_GRACE`.
    pub fn close(&self) {
        self.conn.lose("node connection closed".into());
        let _ = self.conn.out.send(None);
        match self.transport.lock().unwrap().take() {
            Some(Transport::Local(mut child)) => {
                techne_process::runtime().spawn(async move {
                    if tokio::time::timeout(CLOSE_GRACE, child.wait()).await.is_err() {
                        let _ = child.kill().await;
                    }
                });
            }
            Some(Transport::Via { parent, proc }) => parent.conn.notify(Request::Release { proc }),
            None => {}
        }
    }

    /// What the node reported it is, waiting for its hello.
    pub async fn facts(&self) -> Result<protocol::Facts, String> {
        loop {
            let greeted = self.conn.greeted.notified();
            if let Some(reason) = self.lost() {
                return Err(reason);
            }
            match self.conn.facts.lock().unwrap().clone() {
                Some(Some(f)) => return Ok(f),
                Some(None) => return Err("the peer did not say what it is: not a node".into()),
                None => {}
            }
            greeted.await;
        }
    }

    /// Why the connection is unusable, once it is.
    pub fn lost(&self) -> Option<String> {
        self.conn.lost.lock().unwrap().clone()
    }

    /// The transport process's id, when it is a local process.
    pub fn transport_pid(&self) -> Option<u32> {
        match self.transport.lock().unwrap().as_ref()? {
            Transport::Local(c) => c.id(),
            Transport::Via { .. } => None,
        }
    }

    /// What is at `path` on the node, if anything (a symbolic link is not
    /// followed).
    pub fn stat(&self, path: Vec<u8>) -> impl Future<Output = Result<Option<protocol::FileStat>, String>> + use<> {
        let reply = self.conn.request(Request::Stat { path });
        async move {
            match reply.await? {
                Reply::Stat(s) => Ok(s),
                _ => Err("unexpected reply from node".into()),
            }
        }
    }

    /// The bytes of the file at `path` and their hash, or `None` if there is
    /// no file.
    pub fn read_file(&self, path: Vec<u8>) -> impl Future<Output = Result<Option<(Vec<u8>, fs::Hash)>, String>> + use<> {
        let reply = self.conn.request(Request::ReadFile { path });
        async move {
            match reply.await? {
                Reply::File(f) => Ok(f.map(|f| (f.bytes, f.hash))),
                _ => Err("unexpected reply from node".into()),
            }
        }
    }

    /// Replace the file at `path` with `data` if it is as `expect` says:
    /// `Ok(Ok(hash))` once written, `Ok(Err(found))` (what the file has, its
    /// hash or `None`) when it is not as expected.
    pub fn write_file(
        &self,
        path: Vec<u8>,
        data: Vec<u8>,
        expect: protocol::Expect,
    ) -> impl Future<Output = Result<Result<[u8; 32], Option<[u8; 32]>>, String>> + use<> {
        let reply = self.write_file_tracked(path, data, expect);
        async move { reply.await.map_err(|e| e.to_string()) }
    }

    /// As `write_file`, distinguishing a definitive error from a lost reply.
    pub fn write_file_tracked(
        &self,
        path: Vec<u8>,
        data: Vec<u8>,
        expect: protocol::Expect,
    ) -> impl Future<Output = Result<Result<[u8; 32], Option<[u8; 32]>>, RequestError>> + use<> {
        let reply = self.conn.request_tracked(Request::WriteFile { path, data, expect });
        async move {
            match reply.await? {
                Reply::Written(h) => Ok(Ok(h)),
                Reply::Conflict(now) => Ok(Err(now)),
                _ => Err(RequestError::Unknown("unexpected reply from node".into())),
            }
        }
    }

    /// The entries of the directory at `path`, sorted by name.
    pub fn list_directory(&self, path: Vec<u8>) -> impl Future<Output = Result<Vec<protocol::DirEntry>, String>> + use<> {
        let reply = self.conn.request(Request::ListDirectory { path });
        async move {
            match reply.await? {
                Reply::Entries(e) => Ok(e),
                _ => Err("unexpected reply from node".into()),
            }
        }
    }

    /// The nearest directory from `directory` up with an entry named one of
    /// `names`.
    pub fn find_up(&self, directory: Vec<u8>, names: Vec<String>) -> impl Future<Output = Result<Option<Vec<u8>>, String>> + use<> {
        let reply = self.conn.request(Request::FindUp { directory, names });
        async move {
            match reply.await? {
                Reply::Path(p) => Ok(p),
                _ => Err("unexpected reply from node".into()),
            }
        }
    }
}

/// Speak the protocol over `input` and `output`: the hello, then requests
/// and their replies. With `stderr`, what the node said there is added to
/// the reason a connection is lost.
fn speak(
    mut input: impl AsyncRead + Unpin + Send + 'static,
    mut output: impl AsyncWrite + Unpin + Send + 'static,
    stderr: Option<Arc<Mutex<Vec<u8>>>>,
) -> Arc<Conn> {
    let (out, mut out_rx) = mpsc::unbounded_channel::<Option<Message<Request>>>();
    let conn = Arc::new(Conn {
        next: AtomicU64::new(1),
        pending: Mutex::new(HashMap::new()),
        lost: Mutex::new(None),
        out,
        facts: Mutex::new(None),
        greeted: tokio::sync::Notify::new(),
    });
    let said = move |reason: String| match stderr.as_ref().map(|e| String::from_utf8_lossy(&e.lock().unwrap()).trim().to_string()) {
        Some(e) if !e.is_empty() => format!("{reason}: {e}"),
        _ => reason,
    };
    let reader = conn.clone();
    tokio::spawn(async move {
        match handshake(&mut input, &mut output, None).await {
            Ok(facts) => *reader.facts.lock().unwrap() = Some(facts),
            Err(e) => {
                // Give what it said on its way out time to arrive.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                reader.lose(said(e.to_string()));
                return;
            }
        }
        reader.greeted.notify_waiters();
        let writer = reader.clone();
        tokio::spawn(async move {
            while let Some(Some(msg)) = out_rx.recv().await {
                if let Err(e) = write_frame(&mut output, &msg).await {
                    if e.kind() == std::io::ErrorKind::InvalidInput {
                        if let Some(tx) = writer.pending.lock().unwrap().remove(&msg.id) {
                            let _ = tx.send(Err(RequestError::Failed(e.to_string())));
                        }
                    } else {
                        writer.lose(format!("node connection lost: {e}"));
                        break;
                    }
                }
            }
            let _ = output.shutdown().await;
        });
        let reason = loop {
            match read_frame::<Message<Response>>(&mut input).await {
                Ok(Some(Message { id, body })) => {
                    if let Some(tx) = reader.pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(body.map_err(RequestError::Failed));
                    }
                }
                Ok(None) => break "node connection lost".to_string(),
                Err(e) => break format!("node connection lost: {e}"),
            }
        };
        reader.lose(said(reason));
    });
    conn
}

impl Drop for Node {
    fn drop(&mut self) {
        self.close();
    }
}

/// How long a closed node may take to exit before its transport is killed.
pub const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// A Lisp node object.
#[derive(Clone)]
pub struct NodeRef(pub Arc<Node>);

/// A process on a node.
struct RemoteProcess {
    node: Arc<Node>,
    proc: u64,
    pid: i64,
}

impl Drop for RemoteProcess {
    fn drop(&mut self) {
        self.node.conn.notify(Request::Release { proc: self.proc });
    }
}

fn expect<T: 'static>(fut: impl Future<Output = Result<Reply, String>> + 'static, f: fn(Reply) -> Option<T>) -> LocalFuture<T> {
    Box::pin(async move { f(fut.await?).ok_or_else(|| "unexpected reply from node".to_string()) })
}

fn unit(r: Reply) -> Option<()> {
    matches!(r, Reply::Unit).then_some(())
}

impl ProcessBackend for RemoteProcess {
    fn pid(&self) -> i64 {
        self.pid
    }
    fn read(&self, stream: Stream) -> LocalFuture<Option<Vec<u8>>> {
        let fut = self.node.conn.request(Request::Read { proc: self.proc, stream });
        expect(fut, |r| if let Reply::Chunk(c) = r { Some(c) } else { None })
    }
    fn write(&self, data: Vec<u8>) -> LocalFuture<()> {
        expect(self.node.conn.request(Request::Write { proc: self.proc, data }), unit)
    }
    fn close_input(&self) -> LocalFuture<()> {
        expect(self.node.conn.request(Request::CloseInput { proc: self.proc }), unit)
    }
    fn signal(&self, signal: String) -> LocalFuture<()> {
        expect(self.node.conn.request(Request::Signal { proc: self.proc, signal }), unit)
    }
    fn resize(&self, rows: u16, cols: u16) -> LocalFuture<()> {
        expect(self.node.conn.request(Request::Resize { proc: self.proc, rows, cols }), unit)
    }
    fn dropped(&self) -> LocalFuture<i64> {
        expect(self.node.conn.request(Request::Dropped { proc: self.proc }), |r| if let Reply::Int(n) = r { Some(n) } else { None })
    }
    fn wait(&self) -> LocalFuture<Exit> {
        expect(self.node.conn.request(Request::Wait { proc: self.proc }), |r| if let Reply::Exit(e) = r { Some(e) } else { None })
    }
    fn exited(&self) -> LocalFuture<bool> {
        expect(self.node.conn.request(Request::Exited { proc: self.proc }), |r| if let Reply::Bool(b) = r { Some(b) } else { None })
    }
    fn kill(&self) {
        self.node.conn.notify(Request::Kill { proc: self.proc });
    }
}

/// A value held by a node for this client.
pub struct RemoteValue {
    node: Arc<Node>,
    id: u64,
    written: String,
}

impl Drop for RemoteValue {
    fn drop(&mut self) {
        self.node.conn.notify(Request::ReleaseHandle { id: self.id });
    }
}

/// Scheme wrappers over the natives.
const PRELUDE: &str = r#"
(define (%node-result r)
  ;; (outcome output): outcome is a written datum, a remote value or #f.
  (display (cadr r))
  (let ((v (car r)))
    (cond ((string? v) (read (open-input-string v)))
          (v v)
          (else (if #f #f)))))

(define (node-eval node source #:module [module #f])
  "Evaluate the string SOURCE on NODE, in its module MODULE.
MODULE is a name or a file's path, \"user\" by default. Print what the
evaluation printed and return its value: data, or a remote value."
  (%node-result (%node-eval node source module)))

(define (node-apply node f . args)
  "Call the remote procedure F on NODE with ARGS.
Each of ARGS is data or a remote value from NODE."
  (%node-result
   (%node-apply node f
     (map (lambda (a)
            (cond ((remote-value? a) a)
                  (else (%node-write-data a))))
          args))))

(define (node-connect-via node program args)
  "Run PROGRAM with ARGS on NODE and return the node it starts.
Its standard input and output carry the protocol, through NODE: so a
hop's method reaches a node as another user (sudo) or in a container.
What it says on its standard error is in the error if it fails."
  (%node-connect-via node program args))

(define (node-facts node)
  "Return what NODE reported it is, as an alist.
Its keys are `machine` and `boot` (the machine and boot ids), `uid`,
`user`, `home`, `shell`, `mount-namespace` and `pid-namespace` (their
ids), `arch`, `os`, `host`, `program` (the node's executable) and
`arguments` (those after it starting a node); a fact NODE could not
tell is #f. It
waits for NODE's hello, raising an error if the connection failed."
  (read (open-input-string (%node-facts node))))

(define (node-file-stat node path)
  "Return what is at PATH on NODE as an alist, or #f if nothing is.
Its keys are `kind` (`file`, `directory`, `symlink` or `other`; a link
is not followed), `size`, `modified` (seconds since the epoch), `mode`,
`device` and `inode`."
  (let ((s (%node-file-stat node path)))
    (and s (read (open-input-string s)))))

(define (node-processes node)
  "Return the processes NODE runs, each an alist.
Its keys are `proc`, `pid`, `command`, `pty`, `persistent`, `status`
(`running` or how it exited) and `dropped` (the output bytes nobody
read)."
  (read (open-input-string (%node-processes node))))
"#;

/// A Lisp datum written as a string literal (for `node-processes`).
fn lisp_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Define what `f` defines requiring every one of `caps`: in a world not
/// granted one of them, they only raise an error naming it. A node runs
/// local commands (`node-connect`) and starts processes and reads and
/// writes files where it is, which need what doing so here needs.
fn requiring_all(vm: &mut Vm, caps: &[Capability], f: impl FnOnce(&mut Vm)) {
    let missing = caps.iter().copied().find(|c| !vm.grants().has(*c)).unwrap_or(caps[0]);
    vm.requiring(missing, f)
}

/// A request about one path: NODE and PATH are the arguments.
fn path_request<F: Future + 'static>(vm: &mut Vm, args: Args<'_>, f: impl FnOnce(&Node, Vec<u8>) -> F) -> Result<F, String> {
    let n = node(vm, args, 0).map_err(|e| e.into_inner().msg)?;
    let path: String = args.get(vm, 1).map_err(|e| e.into_inner().msg)?;
    Ok(f(&n, path.into_bytes()))
}

/// An optional string as a datum.
fn maybe_string(s: &Option<String>) -> String {
    s.as_deref().map_or("#f".into(), lisp_string)
}

/// A node's facts as an alist datum, for `node-facts`.
fn facts_datum(f: &protocol::Facts) -> String {
    let number = |n: Option<u64>| n.map_or("#f".into(), |n| n.to_string());
    format!(
        "((machine . {}) (boot . {}) (uid . {}) (user . {}) (home . {}) (shell . {}) (mount-namespace . {}) (pid-namespace . {}) (arch . {}) (os . {}) (host . {}) (program . {}) (arguments {}))",
        maybe_string(&f.machine),
        maybe_string(&f.boot),
        f.uid,
        maybe_string(&f.user),
        lisp_string(&String::from_utf8_lossy(&f.home)),
        maybe_string(&f.shell),
        number(f.mount_namespace),
        number(f.pid_namespace),
        lisp_string(&f.arch),
        lisp_string(&f.os),
        maybe_string(&f.host),
        lisp_string(&String::from_utf8_lossy(&f.program)),
        f.arguments.iter().map(|a| lisp_string(a)).collect::<Vec<_>>().join(" "),
    )
}

/// A file's metadata as an alist datum, for `node-file-stat`.
fn stat_datum(s: &protocol::FileStat) -> String {
    let kind = match s.kind {
        protocol::FileKind::File => "file",
        protocol::FileKind::Directory => "directory",
        protocol::FileKind::Symlink => "symlink",
        protocol::FileKind::Other => "other",
    };
    format!(
        "((kind . {kind}) (size . {}) (modified . {}) (mode . {}) (device . {}) (inode . {}))",
        s.size, s.modified, s.mode, s.device, s.inode
    )
}

fn exit_datum(e: &Exit) -> String {
    match e {
        Exit::Code(c) => c.to_string(),
        Exit::Signal(s) => format!("(signal {s})"),
    }
}

fn processes_datum(list: &[protocol::ProcInfo]) -> String {
    let items: Vec<String> = list
        .iter()
        .map(|p| {
            let command: Vec<String> = p.command.iter().map(|c| lisp_string(c)).collect();
            let status = p.exit.as_ref().map_or("running".to_string(), exit_datum);
            let b = |x: bool| if x { "#t" } else { "#f" };
            format!(
                "((proc . {}) (pid . {}) (command {}) (pty . {}) (persistent . {}) (status . {status}) (dropped . {}))",
                p.proc,
                p.pid,
                command.join(" "),
                b(p.pty),
                b(p.persistent),
                p.dropped
            )
        })
        .collect();
    format!("({})", items.join(" "))
}

/// The node argument `i` names.
fn node(vm: &mut Vm, args: Args<'_>, i: usize) -> Result<Arc<Node>, Error> {
    let n: Foreign<NodeRef> = args.get(vm, i)?;
    Ok(n.0.0.clone())
}

/// An environment: a list of (NAME . VALUE) strings.
fn environment(vm: &mut Vm, list: Value) -> Result<Vec<(String, String)>, Error> {
    let pairs: Vec<Value> = vm.get(&list)?;
    pairs
        .iter()
        .map(|p| {
            let name: String = vm.call_global("car", std::slice::from_ref(p)).and_then(|v| vm.get(&v))?;
            let value: String = vm.call_global("cdr", std::slice::from_ref(p)).and_then(|v| vm.get(&v))?;
            Ok((name, value))
        })
        .collect()
}

/// A reply to `Eval`/`Apply` as `(outcome output)` for `%node-result`.
fn evaluated(node: Arc<Node>, reply: Reply) -> Result<Vec<EvalPart>, String> {
    match reply {
        Reply::Evaluated { outcome, output } => {
            let first = match outcome {
                protocol::Outcome::Void => EvalPart::Nothing,
                protocol::Outcome::Data(s) => EvalPart::Text(s),
                protocol::Outcome::Handle { id, written } => EvalPart::Remote(RemoteValue { node, id, written }),
            };
            Ok(vec![first, EvalPart::Text(output)])
        }
        _ => Err("unexpected reply from node".to_string()),
    }
}

enum EvalPart {
    Nothing,
    Text(String),
    Remote(RemoteValue),
}

impl techne_vm::api::IntoValue for EvalPart {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        match self {
            EvalPart::Nothing => Ok(Value::FALSE),
            EvalPart::Text(s) => s.into_value(vm),
            EvalPart::Remote(r) => Foreign::new(r).into_value(vm),
        }
    }
}

/// Define the node procedures in `vm` (and the process procedures they
/// extend). Node procedures need the network capability; those starting
/// processes (`node-connect` too) also the processes capability, those of
/// files also the files capability. Without it they only raise "not
/// granted".
pub fn install(vm: &mut Vm) -> Result<(), Error> {
    techne_process::install(vm)?;
    vm.requiring(Capability::Network, natives);
    // In the root module, so every module sees it.
    vm.eval_in(techne_vm::vm::ROOT_MODULE, "<techne-node>", PRELUDE).map(|_| ())
}

fn natives(vm: &mut Vm) {
    techne_vm::procedures! { vm;
        /// Write OBJ as bounded data for a node argument.
        #[vm]
        "(%node-write-data obj)" => |vm: &mut Vm, v: Value| -> Result<String, Error> {
            vm.to_written_data(&v, MAX_DATA)?.ok_or_else(|| Error::new("node-apply: only data and remote values can be sent"))
        };
    }
    vm.name_foreign_type::<NodeRef>("node");
    vm.name_foreign_type::<RemoteValue>("remote-value");
    requiring_all(vm, &[Capability::Network, Capability::Processes], |vm| {
        techne_vm::procedures! { vm;
            /// Start COMMAND, a list of strings, and return the node it serves.
            /// COMMAND runs `techne-node`, here or over ssh.
            "(node-connect command)" => |command: Vec<String>| -> Result<Foreign<NodeRef>, String> {
                let node = Node::connect(&command).map_err(|e| format!("node-connect: {}: {e}", command.join(" ")))?;
                Ok(Foreign::new(NodeRef(Arc::new(node))))
            };
        }
    });
    techne_vm::procedures! { vm;
        /// Close the connection to NODE.
        "(node-close node)" => |n: Foreign<NodeRef>| n.0.0.close();
    }
    techne_vm::procedures! { vm;
        /// Return why NODE's connection is unusable, or #f while it is not.
        "(node-lost node)" => |n: Foreign<NodeRef>| n.0.0.lost();
    }
    techne_vm::procedures! { vm;
        /// Return the id of the process carrying NODE's connection, or #f.
        "(node-transport-pid node)" => |n: Foreign<NodeRef>| n.0.0.transport_pid().map(|p| p as i64);
    }
    techne_vm::procedures! { vm;
        /// Interrupt what NODE is evaluating.
        "(node-interrupt node)" => |n: Foreign<NodeRef>| n.0.0.conn.notify(Request::Interrupt);
    }
    techne_vm::procedures! { vm;
        /// Ask NODE to end, with its processes.
        "(node-shutdown node)" => |n: Foreign<NodeRef>| n.0.0.conn.notify(Request::Shutdown);
    }
    techne_vm::procedures! { vm;
        /// Return #t if OBJ is a value held by a node.
        #[vm]
        "(remote-value? obj)" => |vm: &mut Vm, v: Value| vm.inspect(&v, |a| a.foreign::<RemoteValue>().is_some()).unwrap_or(false);
    }
    techne_vm::procedures! { vm;
        /// Return the node's bounded written preview of REMOTE.
        /// At a data limit this is a fixed marker, not a readable datum.
        "(remote-value-written remote)" => |r: Foreign<RemoteValue>| r.written.clone();
    }
    vm.register_async("%node-eval", 3, |vm: &mut Vm, args: Args<'_>| {
        let fut = node(vm, args, 0).and_then(|n| {
            let module: Option<String> = args.get(vm, 2)?;
            Ok((n.clone(), n.conn.request(Request::Eval { source: args.get(vm, 1)?, module })))
        });
        async move {
            let (node, fut) = fut.map_err(|e| e.into_inner().msg)?;
            evaluated(node, fut.await?)
        }
    });
    vm.register_async("%node-apply", 3, |vm: &mut Vm, args: Args<'_>| {
        let request = (|| {
            let n = node(vm, args, 0)?;
            let f: Foreign<RemoteValue> = args.get(vm, 1)?;
            if !Arc::ptr_eq(&f.node, &n) {
                return Err(Error::new("node-apply: the procedure belongs to another node"));
            }
            // Strings go as data, remote values as their handles.
            let sent = args.inspect(vm, 2, |list| {
                let items = list.items().ok_or_else(|| list.type_error("list"))?;
                items
                    .into_iter()
                    .map(|item| match item.as_str() {
                        Some(s) => Ok(protocol::Arg::Data(s.to_owned())),
                        None => {
                            let r = Foreign::<RemoteValue>::from_arg(item)?;
                            if !Arc::ptr_eq(&r.node, &n) {
                                return Err(Error::new("node-apply: a remote value belongs to another node"));
                            }
                            Ok(protocol::Arg::Handle(r.id))
                        }
                    })
                    .collect::<Result<Vec<_>, Error>>()
            })??;
            Ok((n.clone(), n.conn.request(Request::Apply { f: f.id, args: sent })))
        })();
        async move {
            let (node, fut) = request.map_err(|e| e.into_inner().msg)?;
            evaluated(node, fut.await?)
        }
    });
    vm.register_async("node-describe", 2, |vm: &mut Vm, args: Args<'_>| {
        let request = node(vm, args, 0).and_then(|n| {
            let request = args.inspect(vm, 1, |what| match what.foreign::<RemoteValue>() {
                Some(r) => Ok(Request::DescribeHandle { id: r.id }),
                // A symbol's name, or a string.
                None => String::from_arg(what).map(|name| Request::Describe { name }),
            })??;
            Ok(n.conn.request(request))
        });
        async move {
            match request.map_err(|e| e.into_inner().msg)?.await? {
                Reply::Text(t) => Ok(t),
                _ => Err("unexpected reply from node".to_string()),
            }
        }
    });
    requiring_all(vm, &[Capability::Network, Capability::Processes], |vm| {
        vm.register_async("%node-processes", 1, |vm: &mut Vm, args: Args<'_>| {
            let fut = node(vm, args, 0).map(|n| n.conn.request(Request::ListProcesses));
            async move {
                match fut.map_err(|e| e.into_inner().msg)?.await? {
                    Reply::Processes(list) => Ok(processes_datum(&list)),
                    _ => Err("unexpected reply from node".to_string()),
                }
            }
        });
    });
    requiring_all(vm, &[Capability::Network, Capability::Processes], |vm| {
        vm.register_async("%node-connect-via", 3, |vm: &mut Vm, args: Args<'_>| {
            let start = (|| Ok::<_, Error>((node(vm, args, 0)?, args.get::<String>(vm, 1)?, args.get::<Vec<String>>(vm, 2)?)))();
            async move {
                let (parent, program, rest) = start.map_err(|e| e.into_inner().msg)?;
                let node = Node::connect_via(parent, program, rest).await?;
                Ok::<_, String>(Foreign::new(NodeRef(Arc::new(node))))
            }
        });
    });
    vm.register_async("%node-facts", 1, |vm: &mut Vm, args: Args<'_>| {
        let n = node(vm, args, 0);
        async move {
            let facts = n.map_err(|e| e.into_inner().msg)?.facts().await?;
            Ok::<_, String>(facts_datum(&facts))
        }
    });
    requiring_all(vm, &[Capability::Network, Capability::Files], |vm| {
        vm.register_async("%node-file-stat", 2, |vm: &mut Vm, args: Args<'_>| {
            let fut = path_request(vm, args, |n, path| n.stat(path));
            async move { Ok::<_, String>(fut?.await?.map(|s| stat_datum(&s))) }
        });
    });
    requiring_all(vm, &[Capability::Network, Capability::Files], |vm| {
        vm.register_async("node-file-read", 2, |vm: &mut Vm, args: Args<'_>| {
            let fut = path_request(vm, args, |n, path| n.read_file(path));
            async move { Ok::<_, String>(fut?.await?.map(|(bytes, _)| techne_vm::api::Bytes(bytes))) }
        });
    });
    requiring_all(vm, &[Capability::Network, Capability::Files], |vm| {
        vm.register_async("node-file-write", 3, |vm: &mut Vm, args: Args<'_>| {
            let request = (|| {
                let n = node(vm, args, 0)?;
                let path: String = args.get(vm, 1)?;
                let data: techne_vm::api::Bytes = args.get(vm, 2)?;
                Ok::<_, Error>(n.write_file(path.into_bytes(), data.0, protocol::Expect::Any))
            })();
            async move {
                match request.map_err(|e| e.into_inner().msg)?.await? {
                    Ok(_) => Ok(()),
                    Err(_) => Err("node-file-write: the file was not as expected".to_string()),
                }
            }
        });
    });
    requiring_all(vm, &[Capability::Network, Capability::Files], |vm| {
        vm.register_async("node-directory-list", 2, |vm: &mut Vm, args: Args<'_>| {
            let fut = path_request(vm, args, |n, path| n.list_directory(path));
            async move {
                Ok::<_, String>(
                    fut?.await?
                        .into_iter()
                        .map(|e| String::from_utf8_lossy(&e.name).into_owned() + if e.directory { "/" } else { "" })
                        .collect::<Vec<_>>(),
                )
            }
        });
    });
    requiring_all(vm, &[Capability::Network, Capability::Files], |vm| {
        vm.register_async("node-find-up", 3, |vm: &mut Vm, args: Args<'_>| {
            let request = (|| {
                let n = node(vm, args, 0)?;
                let dir: String = args.get(vm, 1)?;
                let names: Vec<String> = args.get(vm, 2)?;
                Ok::<_, Error>(n.find_up(dir.into_bytes(), names))
            })();
            async move {
                let found = request.map_err(|e| e.into_inner().msg)?.await?;
                Ok::<_, String>(found.map(|p| String::from_utf8_lossy(&p).into_owned()))
            }
        });
    });
    requiring_all(vm, &[Capability::Network, Capability::Processes], |vm| {
        vm.register_async("node-process", 2, |vm: &mut Vm, args: Args<'_>| {
            let attach = node(vm, args, 0).and_then(|n| {
                let proc: i64 = args.get(vm, 1)?;
                Ok((n.clone(), n.conn.request(Request::Attach { proc: proc as u64 })))
            });
            async move {
                let (node, fut) = attach.map_err(|e| e.into_inner().msg)?;
                remote_process(node, fut.await?)
            }
        });
    });
    requiring_all(vm, &[Capability::Network, Capability::Processes], |vm| {
        vm.register_async("%node-process-spawn", 6, |vm: &mut Vm, args: Args<'_>| {
            let spawn = node(vm, args, 0).and_then(|n| {
                // (directory . environment)
                let place: Value = args.get(vm, 5)?;
                let directory: Option<String> = vm.call_global("car", std::slice::from_ref(&place)).and_then(|v| vm.get(&v))?;
                let pairs = vm.call_global("cdr", &[place])?;
                let environment = environment(vm, pairs)?;
                let request = Request::Spawn {
                    program: args.get(vm, 1)?,
                    args: args.get(vm, 2)?,
                    pty: args.get(vm, 3)?,
                    persist: args.get(vm, 4)?,
                    directory: directory.map(String::into_bytes),
                    environment,
                };
                Ok((n.clone(), n.conn.request(request)))
            });
            async move {
                let (node, fut) = spawn.map_err(|e| e.into_inner().msg)?;
                remote_process(node, fut.await?)
            }
        });
    });
    techne_vm::document! { vm;
        "(%node-eval node source module)";
        "(%node-apply node procedure args)";
        /// Return NODE's description of WHAT, a name or a remote value.
        "(node-describe node what)";
        "(%node-processes node)";
        /// Return the process ID that NODE runs, to read from and write to again.
        "(node-process node id)";
        "(%node-process-spawn node program args pty persist place)";
        "(%node-connect-via node program args)";
        "(%node-facts node)";
        "(%node-file-stat node path)";
        /// Return the bytes of the file at PATH on NODE, or #f if there is none.
        /// PATH is NODE's: `~` and a relative path start at its home.
        "(node-file-read node path)";
        /// Replace the file at PATH on NODE with the bytevector BYTES.
        /// It is written through a temporary file and a rename.
        "(node-file-write node path bytes)";
        /// Return the entries of the directory PATH on NODE, sorted.
        /// Subdirectories have a slash after their name.
        "(node-directory-list node path)";
        /// Return the nearest directory on NODE from DIRECTORY up with an entry NAMES names.
        /// NAMES is a list of strings; #f when no directory has one.
        "(node-find-up node directory names)";
    }
}

fn remote_process(node: Arc<Node>, reply: Reply) -> Result<Foreign<ProcessRef>, String> {
    match reply {
        Reply::Spawned { proc, pid } => {
            let p: Rc<dyn ProcessBackend> = Rc::new(RemoteProcess { node, proc, pid });
            Ok(Foreign::new(ProcessRef::new(p)))
        }
        _ => Err("unexpected reply from node".to_string()),
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;

    fn connection() -> (Conn, mpsc::UnboundedReceiver<Option<Message<Request>>>) {
        let (out, rx) = mpsc::unbounded_channel();
        (
            Conn {
                next: AtomicU64::new(1),
                pending: Mutex::default(),
                lost: Mutex::default(),
                out,
                facts: Mutex::default(),
                greeted: tokio::sync::Notify::new(),
            },
            rx,
        )
    }

    fn write() -> Request {
        Request::WriteFile { path: b"file".to_vec(), data: b"text".to_vec(), expect: protocol::Expect::Any }
    }

    #[test]
    fn transport_loss_is_unknown_only_after_admitting_the_request() {
        let (conn, mut outgoing) = connection();
        let pending = conn.request_tracked(write());
        assert!(outgoing.try_recv().unwrap().is_some());
        conn.lose("lost".into());
        assert!(matches!(techne_process::runtime().block_on(pending), Err(RequestError::Unknown(_))));
        assert!(matches!(techne_process::runtime().block_on(conn.request_tracked(write())), Err(RequestError::Failed(_))));

        let (conn, outgoing) = connection();
        drop(outgoing);
        assert!(matches!(techne_process::runtime().block_on(conn.request_tracked(write())), Err(RequestError::Failed(_))));
        assert!(conn.pending.lock().unwrap().is_empty());
    }
}
