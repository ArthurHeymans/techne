//! techne-node: serve the node protocol (see `techne_node::protocol`).
//!
//! - `techne-node`: serve one client on stdin/stdout (e.g. `ssh host
//!   techne-node`). When the client goes away (end of input, hangup) it
//!   kills every process it started and exits.
//! - `techne-node --session NAME`: attach stdin/stdout to the session
//!   daemon NAME, starting it if needed. The daemon owns the Lisp VM and
//!   processes; it outlives connections, so a client can reconnect, list the
//!   processes (`ListProcesses`) and attach to them. Processes spawned with
//!   `persist` survive disconnects and keep their newest output
//!   (`RETAIN_BYTES` per stream); the others are killed when the client that
//!   started them goes. The daemon exits on `Shutdown`, or after
//!   `TECHNE_NODE_IDLE_SECS` (default 600) without clients or running
//!   persistent processes.
//! - `techne-node --serve-session NAME`: the daemon itself (started by
//!   `--session`).
//! - `techne-node --nrepl [HOST:]PORT`: an nREPL server for editors (see
//!   `techne_node::nrepl`); port 0 picks a free one. It prints the usual
//!   "nREPL server started on port …" line and writes `.nrepl-port`.
//!
//! Sockets and daemon logs live in `TECHNE_NODE_DIR`, else
//! `$XDG_RUNTIME_DIR/techne-node`, else `/tmp/techne-node-UID` (mode 0700).
//! Lisp output and diagnostics go to stderr (the log for a daemon); stdout
//! carries only protocol frames.

use std::{
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use techne_node::protocol::{Arg, Message, Outcome, ProcInfo, Reply, Request, Response, read_frame, write_frame};
use techne_process::{Process, Retain};
use techne_vm::{api::Root, builtins::repr, vm::Vm};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{Notify, mpsc, oneshot},
};

/// Evaluation and application with captured output. `%node-classify`
/// returns `(status payload output)`: status `value` (payload: the written
/// form), `void`, `error` (payload: the message), or `(handle value written
/// output)` for values that are not data.
const LISP: &str = r#"
(define (%data? v)
  (cond ((or (number? v) (string? v) (symbol? v) (char? v) (boolean? v) (null? v) (keyword? v)) #t)
        ((pair? v) (and (%data? (car v)) (%data? (cdr v))))
        ((vector? v) (let loop ((i 0)) (or (= i (vector-length v)) (and (%data? (vector-ref v i)) (loop (+ i 1))))))
        (else #f)))
(define (%written v) (call-with-output-string (lambda (p) (write v p))))
(define (%node-read s) (read (open-input-string s)))
(define (%node-classify thunk)
  (let* ((port (open-output-string))
         (result (parameterize ((current-output-port port))
                   (guard (e (#t (list 'error (if (error-object? e) (condition/report-string e) (%written e)))))
                     (let ((v (thunk)))
                       (cond ((eq? v (if #f #f)) (list 'void ""))
                             ((%data? v) (list 'value (%written v)))
                             (else (list 'handle v (%written v)))))))))
    (append result (list (get-output-string port)))))
(define (%node-eval source module) (%node-classify (lambda () (%node-eval-source source module))))
(define (%node-apply f args) (%node-classify (lambda () (apply f args))))
"#;

/// Connection id 0: requests not tied to a connection.
type ConnId = u64;

enum Job {
    Eval(String, Option<String>, ConnId, oneshot::Sender<Response>),
    Apply(u64, Vec<Arg>, ConnId, oneshot::Sender<Response>),
    Describe(String, oneshot::Sender<Response>),
    DescribeHandle(u64, oneshot::Sender<Response>),
    Release(u64),
    /// A connection ended: release the handles it held.
    ReleaseConn(ConnId),
}

/// Values the node holds for clients.
#[derive(Default)]
struct Handles {
    values: HashMap<u64, (Root, ConnId)>,
    next: u64,
}

/// Turn `%node-classify`'s result into a reply.
fn classified(vm: &mut Vm, handles: &mut Handles, conn: ConnId, result: Result<techne_vm::value::Value, techne_vm::vm::Error>) -> Response {
    let result = result.map_err(|e| e.to_string())?;
    let parts: Vec<Root> = vm.get(result).map_err(|e| e.to_string())?;
    let status = repr(parts[0].get());
    let text = |vm: &mut Vm, i: usize| -> Result<String, String> { vm.get(parts[i].get()).map_err(|e: techne_vm::vm::Error| e.to_string()) };
    match status.as_str() {
        "value" => Ok(Reply::Evaluated { outcome: Outcome::Data(text(vm, 1)?), output: text(vm, 2)? }),
        "void" => Ok(Reply::Evaluated { outcome: Outcome::Void, output: text(vm, 2)? }),
        "handle" => {
            handles.next += 1;
            let id = handles.next;
            handles.values.insert(id, (parts[1].clone(), conn));
            Ok(Reply::Evaluated { outcome: Outcome::Handle { id, written: text(vm, 2)? }, output: text(vm, 3)? })
        }
        _ => Err(text(vm, 1)?),
    }
}

fn apply(vm: &mut Vm, handles: &mut Handles, conn: ConnId, f: u64, args: Vec<Arg>) -> Response {
    let held = |handles: &Handles, id: u64| handles.values.get(&id).map(|(r, _)| r.clone()).ok_or_else(|| format!("no remote value {id} on this node (released?)"));
    let f = held(handles, f)?;
    let mut roots = Vec::with_capacity(args.len());
    for a in args {
        roots.push(match a {
            Arg::Handle(id) => held(handles, id)?,
            Arg::Data(s) => {
                let s = vm.make_string(s.as_bytes());
                let v = vm.call_global("%node-read", &[s]).map_err(|e| e.to_string())?;
                vm.root(v)
            }
        });
    }
    let values: Vec<_> = roots.iter().map(Root::get).collect();
    let list = vm.make_list(&values);
    let result = vm.call_global("%node-apply", &[f.get(), list]);
    classified(vm, handles, conn, result)
}

fn vm_thread(jobs: std::sync::mpsc::Receiver<Job>, ready: std::sync::mpsc::Sender<techne_vm::vm::InterruptHandle>) {
    // Trusted, but `exit` must not end the node and its processes.
    let mut vm = Vm::with_grants(techne_vm::vm::Grants::ALL.without(techne_vm::vm::Capability::HostControl));
    techne_process::install(&mut vm).expect("process library");
    vm.eval_in(techne_vm::vm::ROOT_MODULE, "<techne-node>", LISP).expect("node helpers");
    // One request's source, in its module; `in-module` lasts for the request.
    vm.register_fn_vm("%node-eval-source", |vm: &mut Vm, source: String, module: String| {
        let mut m = vm.find_module(&module)?;
        vm.eval_interactive(&mut m, "<node>", &source)
    });
    let mut h = Handles::default();
    // Handles held, as of the end of the last job (for tests).
    let count = Rc::new(std::cell::Cell::new(0i64));
    let reader = count.clone();
    vm.register_fn("%node-handle-count", move || reader.get());
    let _ = ready.send(vm.interrupt_handle());
    for job in jobs {
        match job {
            Job::Eval(source, module, conn, reply) => {
                let src = vm.make_string(source.as_bytes());
                let src = vm.root(src);
                let module = vm.make_string(module.as_deref().unwrap_or("user").as_bytes());
                let result = vm.call_global("%node-eval", &[src.get(), module]);
                let _ = reply.send(classified(&mut vm, &mut h, conn, result));
            }
            Job::Apply(f, args, conn, reply) => {
                let _ = reply.send(apply(&mut vm, &mut h, conn, f, args));
            }
            Job::Describe(name, reply) => {
                let _ = reply.send(Ok(Reply::Text(vm.describe_binding(techne_vm::vm::USER_MODULE, techne_vm::reader::intern(&name)))));
            }
            Job::DescribeHandle(id, reply) => {
                let text = h.values.get(&id).map(|(r, _)| {
                    let v = r.get();
                    let name = vm.procedure_name(v).unwrap_or_else(|| "value".into());
                    vm.describe_value(&name, v)
                });
                let _ = reply.send(text.map(Reply::Text).ok_or_else(|| format!("no remote value {id} on this node")));
            }
            Job::Release(id) => {
                h.values.remove(&id);
            }
            Job::ReleaseConn(conn) => h.values.retain(|_, (_, c)| *c != conn),
        }
        count.set(h.values.len() as i64);
    }
}

struct Entry {
    process: Arc<Process>,
    command: Vec<String>,
    pty: bool,
    persistent: bool,
    /// The connection that started it (killed with it unless persistent).
    owner: ConnId,
}

struct Node {
    jobs: Mutex<std::sync::mpsc::Sender<Job>>,
    interrupt: techne_vm::vm::InterruptHandle,
    procs: Mutex<HashMap<u64, Entry>>,
    next: AtomicU64,
    clients: AtomicUsize,
    shutdown: Notify,
}

impl Node {
    fn start() -> Arc<Node> {
        let (jobs, job_rx) = std::sync::mpsc::channel();
        let (ready, ready_rx) = std::sync::mpsc::channel();
        std::thread::Builder::new().name("techne-node-vm".into()).spawn(move || vm_thread(job_rx, ready)).expect("VM thread");
        let interrupt = ready_rx.recv().expect("VM started");
        Arc::new(Node {
            jobs: Mutex::new(jobs),
            interrupt,
            procs: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            clients: AtomicUsize::new(0),
            shutdown: Notify::new(),
        })
    }

    fn process(&self, id: u64) -> Result<Arc<Process>, String> {
        self.procs.lock().unwrap().get(&id).map(|e| e.process.clone()).ok_or_else(|| format!("no process {id} on this node"))
    }

    fn post(&self, job: Job) {
        let _ = self.jobs.lock().unwrap().send(job);
    }

    async fn vm(&self, job: impl FnOnce(oneshot::Sender<Response>) -> Job) -> Response {
        let (tx, rx) = oneshot::channel();
        self.jobs.lock().unwrap().send(job(tx)).map_err(|_| "the node's VM has stopped".to_string())?;
        rx.await.map_err(|_| "the node's VM has stopped".to_string())?
    }

    fn running_persistent(&self) -> bool {
        self.procs.lock().unwrap().values().any(|e| e.persistent && !e.process.exited())
    }

    /// A connection ended: kill what it started (unless persistent) and
    /// release its handles.
    fn disconnect(&self, conn: ConnId) {
        self.procs.lock().unwrap().retain(|_, e| {
            if e.owner == conn && !e.persistent {
                e.process.kill();
                false
            } else {
                true
            }
        });
        self.post(Job::ReleaseConn(conn));
    }

    fn kill_all(&self) {
        for (_, e) in self.procs.lock().unwrap().drain() {
            e.process.kill();
        }
    }

    async fn handle(&self, conn: ConnId, request: Request) -> Response {
        match request {
            Request::Eval { source, module } => self.vm(|tx| Job::Eval(source, module, conn, tx)).await,
            Request::Apply { f, args } => self.vm(|tx| Job::Apply(f, args, conn, tx)).await,
            Request::Describe { name } => self.vm(|tx| Job::Describe(name, tx)).await,
            Request::DescribeHandle { id } => self.vm(|tx| Job::DescribeHandle(id, tx)).await,
            Request::ReleaseHandle { id } => {
                self.post(Job::Release(id));
                Ok(Reply::Unit)
            }
            Request::Interrupt => {
                self.interrupt.interrupt();
                Ok(Reply::Unit)
            }
            Request::Shutdown => {
                self.shutdown.notify_one();
                Ok(Reply::Unit)
            }
            Request::Spawn { program, args, pty, persist } => {
                let retain = if persist { Retain::Ring(techne_process::RETAIN_BYTES) } else { Retain::Queue };
                let p = Process::spawn_with(&program, &args, pty, retain).map_err(|e| format!("process-spawn: {program}: {e}"))?;
                let pid = p.pid() as i64;
                let proc = self.next.fetch_add(1, Ordering::Relaxed);
                let command = std::iter::once(program).chain(args).collect();
                let entry = Entry { process: Arc::new(p), command, pty, persistent: persist, owner: conn };
                self.procs.lock().unwrap().insert(proc, entry);
                Ok(Reply::Spawned { proc, pid })
            }
            Request::ListProcesses => {
                let procs = self.procs.lock().unwrap();
                let mut list: Vec<ProcInfo> = procs
                    .iter()
                    .map(|(id, e)| ProcInfo {
                        proc: *id,
                        pid: e.process.pid() as i64,
                        command: e.command.clone(),
                        pty: e.pty,
                        persistent: e.persistent,
                        exit: e.process.exit(),
                        dropped: e.process.dropped(),
                    })
                    .collect();
                list.sort_by_key(|p| p.proc);
                Ok(Reply::Processes(list))
            }
            Request::Attach { proc } => Ok(Reply::Spawned { proc, pid: self.process(proc)?.pid() as i64 }),
            Request::Read { proc, stream } => Ok(Reply::Chunk(self.process(proc)?.read(stream).await?)),
            Request::Write { proc, data } => self.process(proc)?.write(data.into_bytes()).await.map(|_| Reply::Unit),
            Request::CloseInput { proc } => {
                self.process(proc)?.close_input();
                Ok(Reply::Unit)
            }
            Request::Signal { proc, signal } => {
                let sig = techne_process::signal_named(&signal)?;
                self.process(proc)?.signal(sig).map_err(|e| format!("process-signal: {e}"))?;
                Ok(Reply::Unit)
            }
            Request::Resize { proc, rows, cols } => self.process(proc)?.resize(rows, cols).map(|_| Reply::Unit),
            Request::Dropped { proc } => Ok(Reply::Int(self.process(proc)?.dropped() as i64)),
            Request::Wait { proc } => Ok(Reply::Exit(self.process(proc)?.wait().await)),
            Request::Exited { proc } => Ok(Reply::Bool(self.process(proc)?.exited())),
            Request::Kill { proc } => {
                self.process(proc)?.kill();
                Ok(Reply::Unit)
            }
            Request::Release { proc } => {
                let mut procs = self.procs.lock().unwrap();
                if let Some(e) = procs.get(&proc) {
                    if !e.persistent {
                        e.process.kill();
                    }
                    if !e.persistent || e.process.exited() {
                        procs.remove(&proc);
                    }
                }
                Ok(Reply::Unit)
            }
        }
    }
}

/// Serve one connection until it ends.
async fn serve(mut input: impl AsyncRead + Unpin, out: impl AsyncWrite + Unpin + Send + 'static, node: Arc<Node>, conn: ConnId) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Message<Response>>();
    let writer = tokio::spawn(async move {
        let mut out = out;
        while let Some(msg) = rx.recv().await {
            if write_frame(&mut out, &msg).await.is_err() {
                break;
            }
        }
    });
    while let Ok(Some(Message { id, body })) = read_frame::<Message<Request>>(&mut input).await {
        let (node, tx) = (node.clone(), tx.clone());
        tokio::spawn(async move {
            let body = node.handle(conn, body).await;
            if id != 0 {
                let _ = tx.send(Message { id, body });
            }
        });
    }
    drop(tx);
    writer.abort();
}

/// One client on stdin/stdout; everything ends with it.
fn serve_stdio() {
    // Keep the real stdout for frames; anything else printed goes to stderr.
    let stdout = rustix::io::dup(std::io::stdout()).expect("dup stdout");
    rustix::stdio::dup2_stdout(std::io::stderr()).expect("redirect stdout");
    let node = Node::start();
    techne_process::runtime().block_on(async {
        let out = tokio::fs::File::from_std(std::fs::File::from(stdout));
        let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()).expect("SIGHUP handler");
        tokio::select! {
            _ = serve(tokio::io::stdin(), out, node.clone(), 1) => {}
            _ = hangup.recv() => {}
            _ = node.shutdown.notified() => {}
        }
    });
    // The client is gone: its processes must not outlive it.
    node.kill_all();
    std::process::exit(0);
}

fn session_dir() -> PathBuf {
    let dir = std::env::var_os("TECHNE_NODE_DIR").map(PathBuf::from).unwrap_or_else(|| match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(run) => PathBuf::from(run).join("techne-node"),
        None => PathBuf::from(format!("/tmp/techne-node-{}", rustix::process::getuid().as_raw())),
    });
    std::fs::create_dir_all(&dir).expect("session directory");
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    dir
}

fn check_name(name: &str) {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) || name.starts_with('.') {
        eprintln!("techne-node: bad session name {name:?}");
        std::process::exit(2);
    }
}

/// The session daemon: serve connections on the session socket.
fn serve_session(name: &str) {
    let dir = session_dir();
    let socket = dir.join(format!("{name}.sock"));
    // Another daemon already serves this session.
    if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
        return;
    }
    let _ = std::fs::remove_file(&socket);
    let idle = Duration::from_secs(std::env::var("TECHNE_NODE_IDLE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(600));
    let node = Node::start();
    techne_process::runtime().block_on(async {
        let Ok(listener) = tokio::net::UnixListener::bind(&socket) else { return };
        let next_conn = AtomicU64::new(1);
        let mut idle_since = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    let conn = next_conn.fetch_add(1, Ordering::Relaxed);
                    let node = node.clone();
                    node.clients.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(async move {
                        let (r, w) = stream.into_split();
                        serve(r, w, node.clone(), conn).await;
                        node.disconnect(conn);
                        node.clients.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                _ = node.shutdown.notified() => break,
                _ = tick.tick() => {
                    if node.clients.load(Ordering::SeqCst) > 0 || node.running_persistent() {
                        idle_since = Instant::now();
                    } else if idle_since.elapsed() >= idle {
                        break;
                    }
                }
            }
        }
    });
    let _ = std::fs::remove_file(&socket);
    node.kill_all();
    std::process::exit(0);
}

/// Relay stdin/stdout to the session daemon, starting it if needed.
fn attach(name: &str) {
    let dir = session_dir();
    let socket = dir.join(format!("{name}.sock"));
    let connect = || std::os::unix::net::UnixStream::connect(&socket);
    let stream = connect().or_else(|_| {
        let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("{name}.log")))?;
        let mut daemon = std::process::Command::new(std::env::current_exe()?);
        daemon.args(["--serve-session", name]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(log);
        // Its own session: it must not die with this connection's terminal
        // or process group.
        unsafe {
            use std::os::unix::process::CommandExt;
            daemon.pre_exec(|| rustix::process::setsid().map(|_| ()).map_err(Into::into));
        }
        daemon.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match connect() {
                Ok(s) => return Ok(s),
                Err(e) if Instant::now() > deadline => return Err(e),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    });
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            eprintln!("techne-node: cannot reach session {name}: {e}");
            std::process::exit(1);
        }
    };
    stream.set_nonblocking(true).expect("nonblocking socket");
    techne_process::runtime().block_on(async {
        let stream = tokio::net::UnixStream::from_std(stream).expect("socket");
        let (mut r, mut w) = stream.into_split();
        let (mut stdin, mut stdout) = (tokio::io::stdin(), tokio::io::stdout());
        tokio::select! {
            _ = tokio::io::copy(&mut stdin, &mut w) => {}
            _ = tokio::io::copy(&mut r, &mut stdout) => {}
        }
    });
    std::process::exit(0);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => serve_stdio(),
        ["--session", name] => {
            check_name(name);
            attach(name)
        }
        ["--serve-session", name] => {
            check_name(name);
            serve_session(name)
        }
        ["--nrepl", addr] => {
            let addr = if addr.contains(':') { addr.to_string() } else { format!("127.0.0.1:{addr}") };
            let result = techne_node::nrepl::serve(&addr, |bound| {
                // The port file first: clients may act on the line at once.
                let _ = std::fs::write(".nrepl-port", bound.port().to_string());
                println!("nREPL server started on port {} on host {} - nrepl://{bound}", bound.port(), bound.ip());
            });
            if let Err(e) = result {
                eprintln!("techne-node: nREPL on {addr}: {e}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("usage: techne-node [--session NAME | --nrepl [HOST:]PORT]");
            std::process::exit(2);
        }
    }
}
