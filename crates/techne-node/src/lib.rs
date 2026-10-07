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
//! - Data values cross as their written form: numbers, strings, symbols,
//!   characters, booleans, lists and vectors. Other values become remote
//!   values: handles to an object the node keeps until the client's handle
//!   is garbage-collected. `node-apply` calls one with data or remote
//!   values from the same node as arguments; `node-describe` inspects one.
//! - Remote processes are ordinary process objects: `process-read`,
//!   `process-write`, `process-wait`, `call-with-process`… work unchanged.
//! - If the transport dies, every pending and later operation on the node
//!   raises "node connection lost". A plain node (`techne-node`) kills its
//!   processes when its client goes. A session (`techne-node --session
//!   NAME`) keeps its Lisp state and its `#:persist` processes across
//!   connections: reconnect, `node-processes` to see what runs (status,
//!   output dropped while nobody read it), `node-process` to attach.

pub mod bencode;
pub mod nrepl;
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
(define (%data? v)
  (cond ((or (number? v) (string? v) (bytevector? v) (symbol? v) (char? v) (boolean? v) (null? v) (keyword? v)) #t)
        ((pair? v) (and (%data? (car v)) (%data? (cdr v))))
        ((vector? v) (let loop ((i 0)) (or (= i (vector-length v)) (and (%data? (vector-ref v i)) (loop (+ i 1))))))
        (else #f)))

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
                  ((%data? a) (call-with-output-string (lambda (p) (write a p))))
                  (else (error "node-apply: only data and remote values can be sent:" a))))
          args))))

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

fn node(vm: &mut Vm, v: Value) -> Result<Arc<Node>, Error> {
    let n: Foreign<NodeRef> = vm.get(v)?;
    Ok(n.0.0.clone())
}

fn remote(vm: &mut Vm, v: Value) -> Result<Rc<RemoteValue>, Error> {
    let r: Foreign<RemoteValue> = vm.get(v)?;
    Ok(r.0.clone())
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
/// extend). Node procedures need the network capability, process ones the
/// processes capability; without it they only raise "not granted".
pub fn install(vm: &mut Vm) -> Result<(), Error> {
    techne_process::install(vm)?;
    vm.requiring(techne_vm::vm::Capability::Network, natives);
    // In the root module, so every module sees it.
    vm.eval_in(techne_vm::vm::ROOT_MODULE, "<techne-node>", PRELUDE).map(|_| ())
}

fn natives(vm: &mut Vm) {
    vm.name_foreign_type::<NodeRef>("node");
    vm.name_foreign_type::<RemoteValue>("remote-value");
    techne_vm::procedures! { vm;
        /// Start COMMAND, a list of strings, and return the node it serves.
        /// COMMAND runs `techne-node`, here or over ssh.
        "(node-connect command)" => |command: Vec<String>| -> Result<Foreign<NodeRef>, String> {
            let node = Node::connect(&command).map_err(|e| format!("node-connect: {}: {e}", command.join(" ")))?;
            Ok(Foreign::new(NodeRef(Arc::new(node))))
        };
    }
    techne_vm::procedures! { vm;
        /// Close the connection to NODE.
        "(node-close node)" => |n: Foreign<NodeRef>| n.0.0.close();
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
        "(remote-value? obj)" => |vm: &mut Vm, v: Value| remote(vm, v).is_ok();
    }
    techne_vm::procedures! { vm;
        /// Return REMOTE written by its node, as `write` writes it.
        "(remote-value-written remote)" => |r: Foreign<RemoteValue>| r.written.clone();
    }
    vm.register_async("%node-eval", 3, |vm: &mut Vm, args: &[Value]| {
        let fut = node(vm, args[0]).and_then(|n| {
            let module = if args[2].is_truthy() { Some(vm.get(args[2])?) } else { None };
            Ok((n.clone(), n.conn.request(Request::Eval { source: vm.get(args[1])?, module })))
        });
        async move {
            let (node, fut) = fut.map_err(|e| e.into_inner().msg)?;
            evaluated(node, fut.await?)
        }
    });
    vm.register_async("%node-apply", 3, |vm: &mut Vm, args: &[Value]| {
        let request = (|| {
            let n = node(vm, args[0])?;
            let f = remote(vm, args[1])?;
            let items: Vec<techne_vm::api::Root> = vm.get(args[2])?;
            if !Arc::ptr_eq(&f.node, &n) {
                return Err(Error::new("node-apply: the procedure belongs to another node"));
            }
            let mut sent = Vec::with_capacity(items.len());
            for item in items.iter().map(|r| r.get()) {
                if item.is_ptr() && techne_vm::heap::is_kind(item, techne_vm::heap::Kind::String) {
                    sent.push(protocol::Arg::Data(vm.get(item)?));
                } else {
                    let r = remote(vm, item)?;
                    if !Arc::ptr_eq(&r.node, &n) {
                        return Err(Error::new("node-apply: a remote value belongs to another node"));
                    }
                    sent.push(protocol::Arg::Handle(r.id));
                }
            }
            Ok((n.clone(), n.conn.request(Request::Apply { f: f.id, args: sent })))
        })();
        async move {
            let (node, fut) = request.map_err(|e| e.into_inner().msg)?;
            evaluated(node, fut.await?)
        }
    });
    vm.register_async("node-describe", 2, |vm: &mut Vm, args: &[Value]| {
        let request = node(vm, args[0]).and_then(|n| {
            let what = args[1];
            let request = if let Ok(r) = remote(vm, what) {
                Request::DescribeHandle { id: r.id }
            } else if what.is_symbol() {
                Request::Describe { name: techne_vm::reader::symbol_name(what.as_symbol()).to_string() }
            } else {
                Request::Describe { name: vm.get(what)? }
            };
            Ok(n.conn.request(request))
        });
        async move {
            match request.map_err(|e| e.into_inner().msg)?.await? {
                Reply::Text(t) => Ok(t),
                _ => Err("unexpected reply from node".to_string()),
            }
        }
    });
    vm.register_async("%node-processes", 1, |vm: &mut Vm, args: &[Value]| {
        let fut = node(vm, args[0]).map(|n| n.conn.request(Request::ListProcesses));
        async move {
            match fut.map_err(|e| e.into_inner().msg)?.await? {
                Reply::Processes(list) => Ok(processes_datum(&list)),
                _ => Err("unexpected reply from node".to_string()),
            }
        }
    });
    vm.register_async("node-process", 2, |vm: &mut Vm, args: &[Value]| {
        let attach = node(vm, args[0]).and_then(|n| {
            let proc: i64 = vm.get(args[1])?;
            Ok((n.clone(), n.conn.request(Request::Attach { proc: proc as u64 })))
        });
        async move {
            let (node, fut) = attach.map_err(|e| e.into_inner().msg)?;
            remote_process(node, fut.await?)
        }
    });
    vm.register_async("%node-process-spawn", 5, |vm: &mut Vm, args: &[Value]| {
        let spawn = node(vm, args[0]).and_then(|n| {
            let request =
                Request::Spawn { program: vm.get(args[1])?, args: vm.get(args[2])?, pty: vm.get(args[3])?, persist: vm.get(args[4])? };
            Ok((n.clone(), n.conn.request(request)))
        });
        async move {
            let (node, fut) = spawn.map_err(|e| e.into_inner().msg)?;
            remote_process(node, fut.await?)
        }
    });
    techne_vm::document! { vm;
        "(%node-eval node source module)";
        "(%node-apply node procedure args)";
        /// Return NODE's description of WHAT, a name or a remote value.
        "(node-describe node what)";
        "(%node-processes node)";
        /// Return the process ID that NODE runs, to read from and write to again.
        "(node-process node id)";
        "(%node-process-spawn node program args pty persist)";
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
