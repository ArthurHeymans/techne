//! Nodes: evaluation, inspection and processes on another machine.
//!
//! A node is a `techne-node` process reached through a transport command
//! (`ssh host techne-node`, or just `techne-node` for a local one) and spoken
//! to with the framed protocol in `protocol`. In Lisp:
//!
//! ```scheme
//! (define n (node-connect '("ssh" "build-box" "techne-node")))
//! (node-eval n "(define (f x) \"Doc.\" (* x 2)) (f 21)")   ; => 42
//! (node-describe n 'f)                                    ; => "(f x)  procedure, ...\n\nDoc."
//! (call-with-process "make" '("-j8") (lambda (p) (process-read-all p 'stdout)) #:node n)
//! ```
//!
//! - Values cross as their written form: numbers, strings, symbols,
//!   characters, booleans, lists and vectors. Other values (procedures,
//!   records) are an error; remote handles are future work.
//! - Remote processes are ordinary process objects: `process-read`,
//!   `process-write`, `process-wait`, `call-with-process`… work unchanged.
//! - If the transport dies, every pending and later operation on the node
//!   raises "node connection lost". The node kills its processes when its
//!   client goes away. Reconnecting to running processes is not supported.

pub mod protocol;

use std::{
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use protocol::{Message, Reply, Request, Response, read_frame, write_frame};
use techne_process::{Exit, LocalFuture, ProcessBackend, ProcessRef, Stream};
use techne_vm::{
    api::Foreign,
    value::Value,
    vm::{Error, Vm},
};
use tokio::sync::{mpsc, oneshot};

/// The client side of a connection.
struct Conn {
    next: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Response>>>,
    /// Why the connection is unusable, once it is.
    lost: Mutex<Option<String>>,
    /// To the writer task; `None` closes the node's input.
    out: mpsc::UnboundedSender<Option<Message<Request>>>,
}

impl Conn {
    /// Send `request`; the future resolves with the node's answer.
    fn request(&self, request: Request) -> impl Future<Output = Result<Reply, String>> + use<> {
        let (tx, rx) = oneshot::channel();
        let sent = {
            let mut pending = self.pending.lock().unwrap();
            match self.lost.lock().unwrap().clone() {
                Some(reason) => Err(reason),
                None => {
                    let id = self.next.fetch_add(1, Ordering::Relaxed);
                    pending.insert(id, tx);
                    self.out.send(Some(Message { id, body: request })).map_err(|_| "node connection lost".to_string())
                }
            }
        };
        async move {
            sent?;
            rx.await.map_err(|_| "node connection lost".to_string())?
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
            let _ = tx.send(Err(reason.clone()));
        }
    }
}

/// A connected node; it is closed when the last handle (the node object or
/// one of its processes) is dropped.
pub struct Node {
    conn: Arc<Conn>,
    transport: Mutex<Option<tokio::process::Child>>,
}

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
        let (out, mut out_rx) = mpsc::unbounded_channel::<Option<Message<Request>>>();
        let conn = Arc::new(Conn { next: AtomicU64::new(1), pending: Mutex::new(HashMap::new()), lost: Mutex::new(None), out });
        let mut stdin = child.stdin.take().unwrap();
        let writer = conn.clone();
        tokio::spawn(async move {
            while let Some(Some(msg)) = out_rx.recv().await {
                if let Err(e) = write_frame(&mut stdin, &msg).await {
                    writer.lose(format!("node connection lost: {e}"));
                    break;
                }
            }
        });
        let mut stdout = child.stdout.take().unwrap();
        let reader = conn.clone();
        tokio::spawn(async move {
            let reason = loop {
                match read_frame::<Message<Response>>(&mut stdout).await {
                    Ok(Some(Message { id, body })) => {
                        if let Some(tx) = reader.pending.lock().unwrap().remove(&id) {
                            let _ = tx.send(body);
                        }
                    }
                    Ok(None) => break "node connection lost".to_string(),
                    Err(e) => break format!("node connection lost: {e}"),
                }
            };
            reader.lose(reason);
        });
        Ok(Node { conn, transport: Mutex::new(Some(child)) })
    }

    /// Close the connection: close the node's input, so it kills its
    /// processes and exits; kill the transport if it has not exited within
    /// `CLOSE_GRACE`.
    pub fn close(&self) {
        self.conn.lose("node connection closed".into());
        let _ = self.conn.out.send(None);
        if let Some(mut child) = self.transport.lock().unwrap().take() {
            techne_process::runtime().spawn(async move {
                if tokio::time::timeout(CLOSE_GRACE, child.wait()).await.is_err() {
                    let _ = child.kill().await;
                }
            });
        }
    }

    /// The transport process's id.
    pub fn transport_pid(&self) -> Option<u32> {
        self.transport.lock().unwrap().as_ref().and_then(|c| c.id())
    }
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
    fn read(&self, stream: Stream) -> LocalFuture<Option<String>> {
        let fut = self.node.conn.request(Request::Read { proc: self.proc, stream });
        expect(fut, |r| if let Reply::Chunk(c) = r { Some(c) } else { None })
    }
    fn write(&self, data: String) -> LocalFuture<()> {
        expect(self.node.conn.request(Request::Write { proc: self.proc, data }), unit)
    }
    fn close_input(&self) -> LocalFuture<()> {
        expect(self.node.conn.request(Request::CloseInput { proc: self.proc }), unit)
    }
    fn signal(&self, signal: String) -> LocalFuture<()> {
        expect(self.node.conn.request(Request::Signal { proc: self.proc, signal }), unit)
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

/// Scheme wrappers over the natives.
const PRELUDE: &str = r#"
(define (node-eval node source)
  "Evaluate SOURCE (a string) on NODE: print what it printed and return its value, which must be data."
  (let ((r (%node-eval node source)))
    (display (cadr r))
    (if (car r) (read (open-input-string (car r))) (if #f #f))))
"#;

fn node(vm: &mut Vm, v: Value) -> Result<Arc<Node>, Error> {
    let n: Foreign<NodeRef> = vm.get(v)?;
    Ok(n.0.0.clone())
}

/// Define the node procedures in `vm` (and the process procedures they
/// extend).
pub fn install(vm: &mut Vm) -> Result<(), Error> {
    techne_process::install(vm)?;
    vm.name_foreign_type::<NodeRef>("node");
    vm.register_fn("node-connect", |command: Vec<String>| -> Result<Foreign<NodeRef>, String> {
        let node = Node::connect(&command).map_err(|e| format!("node-connect: {}: {e}", command.join(" ")))?;
        Ok(Foreign::new(NodeRef(Arc::new(node))))
    });
    vm.register_fn("node-close", |n: Foreign<NodeRef>| n.0.0.close());
    vm.register_fn("node-transport-pid", |n: Foreign<NodeRef>| n.0.0.transport_pid().map(|p| p as i64));
    vm.register_fn("node-interrupt", |n: Foreign<NodeRef>| n.0.0.conn.notify(Request::Interrupt));
    vm.register_async("%node-eval", 2, |vm: &mut Vm, args: &[Value]| {
        let fut = node(vm, args[0]).and_then(|n| Ok(n.conn.request(Request::Eval { source: vm.get(args[1])? })));
        async move {
            match fut.map_err(|e| e.msg)?.await? {
                // (written-form-or-#f output)
                Reply::Value { written, output } => Ok(vec![written, Some(output)]),
                _ => Err("unexpected reply from node".to_string()),
            }
        }
    });
    vm.register_async("node-describe", 2, |vm: &mut Vm, args: &[Value]| {
        let name = if args[1].is_symbol() { Ok(techne_vm::reader::symbol_name(args[1].as_symbol()).to_string()) } else { vm.get(args[1]) };
        let fut = node(vm, args[0]).and_then(|n| Ok(n.conn.request(Request::Describe { name: name? })));
        async move {
            match fut.map_err(|e| e.msg)?.await? {
                Reply::Text(t) => Ok(t),
                _ => Err("unexpected reply from node".to_string()),
            }
        }
    });
    vm.register_async("%node-process-spawn", 4, |vm: &mut Vm, args: &[Value]| {
        let spawn = node(vm, args[0]).and_then(|n| {
            let request = Request::Spawn { program: vm.get(args[1])?, args: vm.get(args[2])?, pty: vm.get(args[3])? };
            Ok((n.clone(), n.conn.request(request)))
        });
        async move {
            let (node, fut) = spawn.map_err(|e| e.msg)?;
            match fut.await? {
                Reply::Spawned { proc, pid } => {
                    let p: Rc<dyn ProcessBackend> = Rc::new(RemoteProcess { node, proc, pid });
                    Ok(Foreign::new(ProcessRef(p)))
                }
                _ => Err("unexpected reply from node".to_string()),
            }
        }
    });
    vm.eval_source(PRELUDE).map(|_| ())
}
