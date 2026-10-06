//! techne-node: serve the node protocol (see `techne_node::protocol`) on
//! stdin/stdout, e.g. as `ssh host techne-node`.
//!
//! It evaluates Lisp in its own VM (on a dedicated thread) and runs
//! processes for the client. When the client goes away (end of input, or a
//! hangup) it kills every process it started and exits. Lisp output and
//! diagnostics go to stderr; stdout carries only protocol frames.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use techne_node::protocol::{Message, Reply, Request, Response, read_frame, write_frame};
use techne_process::Process;
use techne_vm::{builtins::repr, vm::Vm};
use tokio::sync::{mpsc, oneshot};

/// Evaluation with captured output. Returns `(status payload output)`:
/// status `value` (payload: the written form), `void`, `error` (payload: the
/// message) or `opaque` (payload: how the value prints).
const EVAL: &str = r#"
(define (%data? v)
  (cond ((or (number? v) (string? v) (symbol? v) (char? v) (boolean? v) (null? v) (keyword? v)) #t)
        ((pair? v) (and (%data? (car v)) (%data? (cdr v))))
        ((vector? v) (let loop ((i 0)) (or (= i (vector-length v)) (and (%data? (vector-ref v i)) (loop (+ i 1))))))
        (else #f)))
(define (%written v) (call-with-output-string (lambda (p) (write v p))))
(define (%eval-string source)
  (let ((in (open-input-string source)))
    (let loop ((last (if #f #f)))
      (let ((form (read in)))
        (if (eof-object? form) last (loop (eval form)))))))
(define (%node-eval source)
  (let* ((port (open-output-string))
         (result (parameterize ((current-output-port port))
                   (guard (e (#t (list 'error (if (error-object? e) (condition/report-string e) (%written e)))))
                     (let ((v (%eval-string source)))
                       (cond ((eq? v (if #f #f)) (list 'void ""))
                             ((%data? v) (list 'value (%written v)))
                             (else (list 'opaque (%written v)))))))))
    (append result (list (get-output-string port)))))
"#;

enum Job {
    Eval(String, oneshot::Sender<Response>),
    Describe(String, oneshot::Sender<Response>),
}

fn eval(vm: &mut Vm, source: &str) -> Response {
    let src = vm.make_string(source.as_bytes());
    let result = vm.call_global("%node-eval", &[src]).map_err(|e| e.to_string())?;
    let parts: Vec<techne_vm::api::Root> = vm.get(result).map_err(|e| e.to_string())?;
    let status = repr(parts[0].get());
    let payload: String = vm.get(parts[1].get()).map_err(|e| e.to_string())?;
    let output: String = vm.get(parts[2].get()).map_err(|e| e.to_string())?;
    match status.as_str() {
        "value" => Ok(Reply::Value { written: Some(payload), output }),
        "void" => Ok(Reply::Value { written: None, output }),
        "opaque" => Err(format!("the value is not data and cannot be sent: {payload}")),
        _ => Err(payload),
    }
}

fn vm_thread(jobs: std::sync::mpsc::Receiver<Job>, ready: std::sync::mpsc::Sender<techne_vm::vm::InterruptHandle>) {
    let mut vm = Vm::new();
    techne_process::install(&mut vm).expect("process library");
    vm.eval_source(EVAL).expect("node evaluation helpers");
    let _ = ready.send(vm.interrupt_handle());
    for job in jobs {
        match job {
            Job::Eval(source, reply) => {
                let _ = reply.send(eval(&mut vm, &source));
            }
            Job::Describe(name, reply) => {
                let _ = reply.send(Ok(Reply::Text(vm.describe_binding(techne_vm::reader::intern(&name)))));
            }
        }
    }
}

struct Node {
    jobs: Mutex<std::sync::mpsc::Sender<Job>>,
    interrupt: techne_vm::vm::InterruptHandle,
    procs: Mutex<HashMap<u64, Arc<Process>>>,
    next: AtomicU64,
}

impl Node {
    fn process(&self, id: u64) -> Result<Arc<Process>, String> {
        self.procs.lock().unwrap().get(&id).cloned().ok_or_else(|| format!("no process {id} on this node"))
    }

    async fn vm(&self, job: impl FnOnce(oneshot::Sender<Response>) -> Job) -> Response {
        let (tx, rx) = oneshot::channel();
        self.jobs.lock().unwrap().send(job(tx)).map_err(|_| "the node's VM has stopped".to_string())?;
        rx.await.map_err(|_| "the node's VM has stopped".to_string())?
    }

    async fn handle(&self, request: Request) -> Response {
        match request {
            Request::Eval { source } => self.vm(|tx| Job::Eval(source, tx)).await,
            Request::Describe { name } => self.vm(|tx| Job::Describe(name, tx)).await,
            Request::Interrupt => {
                self.interrupt.interrupt();
                Ok(Reply::Unit)
            }
            Request::Spawn { program, args, pty } => {
                let p = Process::spawn(&program, &args, pty).map_err(|e| format!("process-spawn: {program}: {e}"))?;
                let pid = p.pid() as i64;
                let proc = self.next.fetch_add(1, Ordering::Relaxed);
                self.procs.lock().unwrap().insert(proc, Arc::new(p));
                Ok(Reply::Spawned { proc, pid })
            }
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
            Request::Wait { proc } => Ok(Reply::Exit(self.process(proc)?.wait().await)),
            Request::Exited { proc } => Ok(Reply::Bool(self.process(proc)?.exited())),
            Request::Kill { proc } => {
                self.process(proc)?.kill();
                Ok(Reply::Unit)
            }
            Request::Release { proc } => {
                if let Some(p) = self.procs.lock().unwrap().remove(&proc) {
                    p.kill();
                }
                Ok(Reply::Unit)
            }
        }
    }
}

async fn serve(out: std::fs::File, node: Arc<Node>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Message<Response>>();
    let mut out = tokio::fs::File::from_std(out);
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if write_frame(&mut out, &msg).await.is_err() {
                break;
            }
        }
    });
    let mut input = tokio::io::stdin();
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()).expect("SIGHUP handler");
    loop {
        tokio::select! {
            frame = read_frame::<Message<Request>>(&mut input) => {
                let Ok(Some(Message { id, body })) = frame else { break };
                let (node, tx) = (node.clone(), tx.clone());
                tokio::spawn(async move {
                    let body = node.handle(body).await;
                    let _ = tx.send(Message { id, body });
                });
            }
            _ = hangup.recv() => break,
        }
    }
    // The client is gone: its processes must not outlive it.
    for (_, p) in node.procs.lock().unwrap().drain() {
        p.kill();
    }
}

fn main() {
    // Keep the real stdout for frames; anything else printed goes to stderr.
    let stdout = rustix::io::dup(std::io::stdout()).expect("dup stdout");
    rustix::stdio::dup2_stdout(std::io::stderr()).expect("redirect stdout");
    let (jobs, job_rx) = std::sync::mpsc::channel();
    let (ready, ready_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("techne-node-vm".into()).spawn(move || vm_thread(job_rx, ready)).expect("VM thread");
    let interrupt = ready_rx.recv().expect("VM started");
    let node = Arc::new(Node { jobs: Mutex::new(jobs), interrupt, procs: Mutex::new(HashMap::new()), next: AtomicU64::new(1) });
    techne_process::runtime().block_on(serve(std::fs::File::from(stdout), node));
    // Exit without waiting for an evaluation that may still run.
    std::process::exit(0);
}
