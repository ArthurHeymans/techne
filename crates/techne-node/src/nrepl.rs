//! An nREPL server for techne Lisp (`techne-node --nrepl PORT`).
//!
//! Standard operations, so generic nREPL clients work: `clone`, `close`,
//! `ls-sessions`, `describe`, `eval` (output streamed as `out` messages
//! while it runs, then a `value`), `load-file`, `interrupt`,
//! `completions`, `lookup`, `info` and `eldoc` (from docstrings,
//! parameter lists and definition sites). Unknown operations get
//! `unknown-op`.
//!
//! Extensions (prefixed `techne-`):
//! - Debugger: an `eval` with `"techne-debug": 1` does not fail at once on
//!   an error that has restarts. The error's handler runs where it was
//!   raised, so its restarts are live: the server sends a message with status
//!   `techne-debug`, a `debug-id`, the condition, the restarts (name and
//!   parameters) and the frames, and waits. The client answers with
//!   `techne-debug-restart` (`debug-id`, `restart` index, optional `args`
//!   source) or `techne-debug-abort`; meanwhile other requests (evaluation,
//!   inspection) run inside the paused evaluation. `interrupt` aborts it.
//! - Inspector, per session: `techne-inspect` (`code`) evaluates and shows
//!   a value: a title, its printed form and labelled parts;
//!   `techne-inspect-part` (`index`) descends into a part,
//!   `techne-inspect-pop` goes back up.
//!
//! Modules: `eval`, `load-file`, `completions`, `lookup`, `info` and
//! `eldoc` take an `ns`: a module name (`user`) or a file path, whose module
//! is loaded first if needed (completion and lookup never load; they fall
//! back to the session's module). Without one, a session uses its current
//! module, `user` at first; `(in-module NAME)` switches it, and every `eval`
//! reply's `ns` names the module after the evaluation.
//!
//! All sessions share one VM (`*1` `*2` `*3` `*e` are shared); their
//! requests run one at a time on the VM thread. The server listens on
//! localhost only.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use techne_vm::{
    api::Root,
    builtins::{list_values, repr},
    complete,
    heap::{Kind, field, is_kind, len_of},
    value::Value,
    vm::{Capability, Error, Grants, InterruptHandle, ROOT_MODULE, USER_MODULE, Vm},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};

use crate::bencode::{B, dict, parse};

/// The file name of the server's own Lisp code (cut from backtraces).
const INTERNAL: &str = "<nrepl-internal>";

const LISP: &str = r#"
(define *1 #f) (define *2 #f) (define *3 #f) (define *e #f)
(define (%nrepl-interrupt? c)
  (and (error-object? c) (equal? (error-object-message c) "interrupted")))
(define (%nrepl-run port debug name source)
  ;; (ok value) or (error condition)
  (parameterize ((current-output-port port))
    (guard (e (#t (set! *e e) (list 'error e)))
      (let ((v (with-exception-handler
                (lambda (c)
                  (%nrepl-note-trace)
                  (if (and debug (pair? (compute-restarts)) (not (%nrepl-interrupt? c)))
                      (%nrepl-debug c)
                      (raise c)))
                (lambda () (%nrepl-eval-source name source)))))
        (unless (eq? v (if #f #f))
          (set! *3 *2) (set! *2 *1) (set! *1 v))
        (list 'ok v)))))
"#;

/// Answers to one request: every message carries its id and session.
#[derive(Clone)]
struct Reply {
    out: mpsc::UnboundedSender<B>,
    id: Option<B>,
    session: Option<String>,
    conn: u64,
}

impl Reply {
    fn send(&self, fields: Vec<(&str, B)>) {
        let mut d: Vec<(&str, B)> = fields;
        if let Some(id) = &self.id {
            d.push(("id", id.clone()));
        }
        if let Some(s) = &self.session {
            d.push(("session", B::str(s)));
        }
        let _ = self.out.send(dict(d));
    }

    fn status(&self, statuses: &[&str]) {
        self.send(vec![("status", B::List(statuses.iter().map(B::str).collect()))]);
    }

    fn error(&self, message: &str, statuses: &[&str]) {
        self.send(vec![("err", B::str(format!("{message}\n")))]);
        let mut s = vec!["error"];
        s.extend_from_slice(statuses);
        s.push("done");
        self.status(&s);
    }
}

enum DebugCmd {
    Restart(usize, String),
    Abort,
}

enum Inspect {
    Code(String),
    Part(usize),
    Pop,
}

enum Job {
    Eval { name: String, source: String, ns: Option<String>, debug: bool, reply: Reply },
    Complete { prefix: String, ns: Option<String>, reply: Reply },
    Info { sym: String, kind: &'static str, ns: Option<String>, reply: Reply },
    Inspect { action: Inspect, reply: Reply },
    CloseSession(String),
    Debug { id: u64, cmd: DebugCmd },
}

/// State shared with the network side.
struct Shared {
    jobs: Mutex<std::sync::mpsc::Sender<Job>>,
    interrupt: InterruptHandle,
    sessions: Mutex<HashSet<String>>,
    /// Evaluations in progress (nested ones while a debugger waits),
    /// innermost last: (session, request id).
    running: Mutex<Vec<(Option<String>, Option<B>)>>,
    /// Debuggers waiting for a choice: id -> (session, connection).
    debuggers: Mutex<HashMap<u64, (Option<String>, u64)>>,
    next: AtomicU64,
}

impl Shared {
    fn post(&self, job: Job) {
        let _ = self.jobs.lock().unwrap().send(job);
    }
}

/// An inspected value and its labelled parts.
struct Shown {
    value: Root,
    parts: Vec<(String, Root)>,
}

/// State of the VM thread.
struct State {
    rx: std::sync::mpsc::Receiver<Job>,
    shared: Arc<Shared>,
    evals: RefCell<Vec<Reply>>,
    inspectors: RefCell<HashMap<Option<String>, Vec<Shown>>>,
    /// Where the last condition was raised.
    trace: RefCell<Vec<String>>,
    /// Each session's current module.
    modules: RefCell<HashMap<Option<String>, u32>>,
    /// The module `%nrepl-eval-source` evaluates in, and after it the module
    /// current at its end.
    eval_module: Cell<u32>,
}

impl State {
    fn session_module(&self, session: &Option<String>) -> u32 {
        self.modules.borrow().get(session).copied().unwrap_or(USER_MODULE)
    }

    /// The module a request names with `ns` (loaded if needed), or else the
    /// session's.
    fn module(&self, vm: &mut Vm, ns: &Option<String>, session: &Option<String>) -> Result<u32, Error> {
        match ns {
            Some(ns) => vm.find_module(ns),
            None => Ok(self.session_module(session)),
        }
    }

    /// Like `module`, for requests that must not load files.
    fn loaded_module(&self, vm: &Vm, ns: &Option<String>, session: &Option<String>) -> u32 {
        ns.as_deref().and_then(|ns| vm.loaded_module(ns)).unwrap_or_else(|| self.session_module(session))
    }
}

fn truncate(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("...");
    }
    s
}

fn condition_text(vm: &mut Vm, c: Value) -> String {
    vm.call_global("condition/report-string", &[c]).ok().and_then(|s| vm.get::<String>(s).ok()).unwrap_or_else(|| repr(c))
}

fn run_eval(vm: &mut Vm, state: &Rc<State>, name: String, source: String, ns: Option<String>, debug: bool, reply: Reply) {
    vm.clear_interrupt();
    let module = match state.module(vm, &ns, &reply.session) {
        Ok(m) => m,
        Err(e) => return reply.error(&e.to_string(), &["namespace-not-found"]),
    };
    let outer = state.eval_module.replace(module);
    state.shared.running.lock().unwrap().push((reply.session.clone(), reply.id.clone()));
    state.evals.borrow_mut().push(reply.clone());
    let sink = reply.clone();
    let result = techne_vm::stdlib::make_output_port(vm, move |text| sink.send(vec![("out", B::str(text))]))
        .and_then(|port| {
            let port = vm.root(port);
            let name = vm.make_string(name.as_bytes());
            let name = vm.root(name);
            let source = vm.make_string(source.as_bytes());
            vm.call_global("%nrepl-run", &[port.get(), Value::bool(debug), name.get(), source])
        })
        .and_then(|r| vm.get::<Vec<Root>>(r));
    state.evals.borrow_mut().pop();
    state.shared.running.lock().unwrap().pop();
    let after = state.eval_module.replace(outer);
    state.modules.borrow_mut().insert(reply.session.clone(), after);
    match result {
        Ok(parts) if repr(parts[0].get()) == "ok" => {
            let v = parts[1].get();
            if v != Value::VOID {
                reply.send(vec![("value", B::str(repr(v))), ("ns", B::str(&*vm.module_name(after)))]);
            }
            reply.status(&["done"]);
        }
        Ok(parts) => {
            let c = parts[1].get();
            let message = condition_text(vm, c);
            if message == techne_vm::vm::INTERRUPTED {
                reply.status(&["interrupted"]);
                reply.status(&["done"]);
                return;
            }
            let mut err = format!("error: {message}\n");
            for frame in state.trace.borrow().iter() {
                err.push_str(&format!("  in {frame}\n"));
            }
            reply.send(vec![("err", B::str(err))]);
            reply.send(vec![("ex", B::str("error")), ("root-ex", B::str("error")), ("status", B::List(vec![B::str("eval-error")]))]);
            reply.status(&["done"]);
        }
        Err(e) => reply.error(&e.to_string(), &[]),
    }
}

/// The `%nrepl-debug` handler: report the condition and its restarts, then
/// serve requests until the client picks a restart or aborts.
fn debug(vm: &mut Vm, state: &Rc<State>, condition: Value) -> Result<Value, Error> {
    let condition = vm.root(condition);
    let Some(reply) = state.evals.borrow().last().cloned() else { return Err(vm.raise_error(condition.get())) };
    let restarts: Vec<Root> = {
        let list = vm.call_global("compute-restarts", &[])?;
        list_values(list).unwrap_or_default().into_iter().map(|r| vm.root(r)).collect()
    };
    let mut described = Vec::new();
    for r in &restarts {
        let name = vm.call_global("restart-name", &[r.get()])?;
        // The formals as written: (v) -> "v", (a . rest) -> "a . rest".
        let formals = vm.call_global("restart-formals", &[r.get()])?;
        let written = repr(formals);
        let params = match written.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            Some(inner) => inner.to_string(),
            None => format!(". {written}"),
        };
        described.push(dict([("name", B::str(repr(name))), ("params", B::str(params))]));
    }
    let id = state.shared.next.fetch_add(1, Ordering::Relaxed);
    state.shared.debuggers.lock().unwrap().insert(id, (reply.session.clone(), reply.conn));
    let message = condition_text(vm, condition.get());
    let frames = state.trace.borrow().iter().map(B::str).collect();
    reply.send(vec![
        ("status", B::List(vec![B::str("techne-debug")])),
        ("debug-id", B::Int(id as i64)),
        ("condition", B::str(message)),
        ("restarts", B::List(described)),
        ("frames", B::List(frames)),
    ]);
    let choice = loop {
        match state.rx.recv() {
            Err(_) => break DebugCmd::Abort,
            Ok(Job::Debug { id: target, cmd }) if target == id => match cmd {
                DebugCmd::Restart(i, _) if i >= restarts.len() => {
                    reply.send(vec![("err", B::str(format!("no restart {i}\n")))]);
                }
                cmd => break cmd,
            },
            Ok(job) => handle(vm, state, job),
        }
    };
    state.shared.debuggers.lock().unwrap().remove(&id);
    match choice {
        DebugCmd::Abort => Err(vm.raise_error(condition.get())),
        DebugCmd::Restart(i, args) => {
            let mut values = vec![restarts[i].clone()];
            if !args.trim().is_empty() {
                let list = vm.eval_in(vm.current_module(), "<restart arguments>", &format!("(list {args})"))?;
                values.extend(list_values(list).unwrap_or_default().into_iter().map(|v| vm.root(v)));
            }
            let args: Vec<Value> = values.iter().map(Root::get).collect();
            // Escapes to the restart-case.
            vm.call_global("invoke-restart", &args)
        }
    }
}

fn is_procedure(vm: &mut Vm, v: Value) -> bool {
    vm.call_global("procedure?", &[v]).is_ok_and(|r| r.is_truthy())
}

fn complete(vm: &mut Vm, module: u32, prefix: &str, reply: &Reply) {
    let mut out = Vec::new();
    let ns = vm.module_name(module);
    for (name, kind) in vm.completions(module).into_iter().filter(|(n, _)| n.starts_with(prefix)).take(500) {
        // nREPL's names for the kinds.
        let kind = match kind {
            complete::Kind::Syntax => "special-form",
            complete::Kind::Macro => "macro",
            complete::Kind::Procedure => "function",
            complete::Kind::Variable => "var",
        };
        out.push(dict([("candidate", B::str(&*name)), ("type", B::str(kind)), ("ns", B::str(&*ns))]));
    }
    reply.send(vec![("completions", B::List(out))]);
    reply.status(&["done"]);
}

fn info(vm: &mut Vm, module: u32, sym: &str, kind: &str, reply: &Reply) {
    let v = vm.get_global_in(module, sym);
    let proc_info = v.filter(|v| is_procedure(vm, *v)).and_then(|v| vm.procedure_info(v));
    let described = vm.describe_binding(module, techne_vm::reader::intern(sym));
    if described.ends_with(": unbound") {
        reply.status(&["done", "no-info"]);
        return;
    }
    let ns = vm.module_name(module);
    let mut fields = vec![("name", B::str(sym)), ("ns", B::str(&*ns))];
    let mut params = Vec::new();
    match &proc_info {
        Some(i) if !i.native => {
            params = i.params.iter().map(|p| B::str(&**p)).collect();
            fields.push(("arglists-str", B::str(format!("({})", i.params.join(" ")))));
            fields.push(("doc", B::str(i.doc.as_deref().unwrap_or(""))));
            if let Some(file) = &i.file {
                fields.push(("file", B::str(&**file)));
                fields.push(("line", B::Int(i.line as i64)));
                fields.push(("column", B::Int(i.column as i64)));
            }
        }
        _ => fields.push(("doc", B::str(&described))),
    }
    let type_name = match (&proc_info, v) {
        (Some(_), _) => "function",
        (None, Some(_)) => "variable",
        (None, None) if techne_vm::compiler::is_special_form(sym) => "special-form",
        _ => "macro",
    };
    match kind {
        "lookup" => reply.send(vec![("info", dict(fields))]),
        "eldoc" => {
            let doc = proc_info.as_ref().and_then(|i| i.doc.as_deref()).unwrap_or("");
            reply.send(vec![
                ("eldoc", B::List(vec![B::List(params)])),
                ("name", B::str(sym)),
                ("ns", B::str(&*ns)),
                ("type", B::str(type_name)),
                ("docstring", B::str(doc)),
            ]);
        }
        _ => reply.send(fields),
    }
    reply.status(&["done"]);
}

/// The labelled parts of a value, for the inspector.
fn parts(vm: &mut Vm, v: Value) -> (String, Vec<(String, Value)>) {
    const MAX: usize = 500;
    let numbered = |items: Vec<Value>| items.into_iter().take(MAX).enumerate().map(|(i, x)| (i.to_string(), x)).collect();
    unsafe {
        if is_kind(v, Kind::Pair) {
            return match list_values(v) {
                Some(items) => (format!("list of {} elements", items.len()), numbered(items)),
                None => ("pair".into(), vec![("car".into(), field(v.as_ptr(), 0)), ("cdr".into(), field(v.as_ptr(), 1))]),
            };
        }
        if is_kind(v, Kind::Vector) {
            let n = len_of(v.as_ptr());
            return (format!("vector of {n} elements"), numbered((0..n).map(|i| field(v.as_ptr(), i)).collect()));
        }
        if is_kind(v, Kind::Record) {
            let rtd = field(v.as_ptr(), 0);
            let name = repr(field(rtd.as_ptr(), 0));
            let names = list_values(field(rtd.as_ptr(), 1)).unwrap_or_default();
            let fields = (1..len_of(v.as_ptr()))
                .map(|i| (names.get(i - 1).map_or_else(|| format!("field {i}"), |n| repr(*n)), field(v.as_ptr(), i)))
                .collect();
            return (format!("record {name}"), fields);
        }
        if is_kind(v, Kind::Rtd) {
            return ("record type".into(), vec![("name".into(), field(v.as_ptr(), 0)), ("fields".into(), field(v.as_ptr(), 1))]);
        }
        if is_kind(v, Kind::Box) {
            return ("box".into(), vec![("contents".into(), field(v.as_ptr(), 0))]);
        }
        if is_kind(v, Kind::Table) {
            let alist = vm.call_global("hash-table->alist", &[v]).unwrap_or(Value::NIL);
            let entries = list_values(alist).unwrap_or_default();
            let n = entries.len();
            let parts = entries.into_iter().take(MAX).map(|e| (repr(field(e.as_ptr(), 0)), field(e.as_ptr(), 1))).collect();
            return (format!("hash table of {n} entries"), parts);
        }
        if is_kind(v, Kind::Closure) {
            let info = vm.procedure_info(v);
            let mut parts = Vec::new();
            let mut title = "procedure".to_string();
            if let Some(i) = info {
                title = format!("procedure {}", i.name);
                let mut text = vec![("parameters".to_string(), format!("({})", i.params.join(" ")))];
                if let Some(doc) = &i.doc {
                    text.push(("documentation".into(), doc.to_string()));
                }
                if let Some(file) = &i.file {
                    text.push(("defined at".into(), format!("{file}:{}:{}", i.line, i.column)));
                }
                for (label, s) in text {
                    parts.push((label, vm.make_string(s.as_bytes())));
                }
            }
            for i in 1..len_of(v.as_ptr()) {
                parts.push((format!("captured {i}"), field(v.as_ptr(), i)));
            }
            return (title, parts);
        }
        if is_kind(v, Kind::String) {
            return ("string".into(), Vec::new());
        }
    }
    let ty = vm.call_global("type-of", &[v]).map(repr).unwrap_or_default();
    (ty, Vec::new())
}

fn show(vm: &mut Vm, v: Value) -> Shown {
    let value = vm.root(v);
    let (_, parts) = parts(vm, v);
    // Rooted at once: computing the parts may have allocated.
    let parts = parts.into_iter().map(|(l, p)| (l, vm.root(p))).collect();
    Shown { value, parts }
}

fn send_view(vm: &mut Vm, stack: &[Shown], reply: &Reply) {
    let Some(top) = stack.last() else {
        reply.error("nothing is being inspected", &[]);
        return;
    };
    let (title, _) = parts(vm, top.value.get());
    let parts = top.parts.iter().map(|(label, v)| B::List(vec![B::str(label), B::str(truncate(repr(v.get()), 200))])).collect();
    reply.send(vec![
        ("title", B::str(title)),
        ("value", B::str(truncate(repr(top.value.get()), 2000))),
        ("parts", B::List(parts)),
        ("depth", B::Int(stack.len() as i64)),
    ]);
    reply.status(&["done"]);
}

fn inspect(vm: &mut Vm, state: &Rc<State>, action: Inspect, reply: &Reply) {
    let key = reply.session.clone();
    match action {
        Inspect::Code(code) => match vm.eval_in(state.session_module(&key), "<inspect>", &code) {
            Ok(v) => {
                let shown = show(vm, v);
                state.inspectors.borrow_mut().insert(key.clone(), vec![shown]);
            }
            Err(e) => return reply.error(&e.to_string(), &[]),
        },
        Inspect::Part(i) => {
            let part = state.inspectors.borrow().get(&key).and_then(|s| s.last()).and_then(|top| top.parts.get(i)).map(|(_, v)| v.get());
            match part {
                Some(v) => {
                    let shown = show(vm, v);
                    state.inspectors.borrow_mut().entry(key.clone()).or_default().push(shown);
                }
                None => return reply.error(&format!("no part {i}"), &[]),
            }
        }
        Inspect::Pop => {
            let mut inspectors = state.inspectors.borrow_mut();
            if let Some(stack) = inspectors.get_mut(&key)
                && stack.len() > 1
            {
                stack.pop();
            }
        }
    }
    let stack = state.inspectors.borrow_mut().remove(&key).unwrap_or_default();
    send_view(vm, &stack, reply);
    state.inspectors.borrow_mut().insert(key, stack);
}

fn handle(vm: &mut Vm, state: &Rc<State>, job: Job) {
    match job {
        Job::Eval { name, source, ns, debug, reply } => run_eval(vm, state, name, source, ns, debug, reply),
        Job::Complete { prefix, ns, reply } => {
            let module = state.loaded_module(vm, &ns, &reply.session);
            complete(vm, module, &prefix, &reply)
        }
        Job::Info { sym, kind, ns, reply } => {
            let module = state.loaded_module(vm, &ns, &reply.session);
            info(vm, module, &sym, kind, &reply)
        }
        Job::Inspect { action, reply } => inspect(vm, state, action, &reply),
        Job::CloseSession(s) => {
            let s = Some(s);
            state.inspectors.borrow_mut().remove(&s);
            state.modules.borrow_mut().remove(&s);
        }
        // A debugger that is no longer waiting.
        Job::Debug { .. } => {}
    }
}

fn vm_thread(rx: std::sync::mpsc::Receiver<Job>, shared_tx: std::sync::mpsc::Sender<Arc<Shared>>, jobs: std::sync::mpsc::Sender<Job>) {
    // Trusted, but `exit` must not end the server.
    let mut vm = Vm::with_grants(Grants::ALL.without(Capability::HostControl));
    crate::install(&mut vm).expect("node library");
    let shared = Arc::new(Shared {
        jobs: Mutex::new(jobs),
        interrupt: vm.interrupt_handle(),
        sessions: Mutex::new(HashSet::new()),
        running: Mutex::new(Vec::new()),
        debuggers: Mutex::new(HashMap::new()),
        next: AtomicU64::new(1),
    });
    let state = Rc::new(State {
        rx,
        shared: shared.clone(),
        evals: RefCell::new(Vec::new()),
        inspectors: RefCell::new(HashMap::new()),
        trace: RefCell::new(Vec::new()),
        modules: RefCell::new(HashMap::new()),
        eval_module: Cell::new(USER_MODULE),
    });
    let s = state.clone();
    vm.register_fn_vm("%nrepl-eval-source", move |vm: &mut Vm, name: String, source: String| -> Result<Value, Error> {
        let mut module = s.eval_module.get();
        let result = vm.eval_interactive(&mut module, &name, &source);
        s.eval_module.set(module);
        result
    });
    let s = state.clone();
    vm.register_fn_vm("%nrepl-note-trace", move |vm: &mut Vm| {
        // The user's frames: up to the server's own evaluation wrapper.
        let mut frames: Vec<String> = vm.raise_backtrace().iter().take_while(|f| !f.contains(INTERNAL)).cloned().collect();
        while frames.last().is_some_and(|f| f.starts_with("with-exception-handler (")) {
            frames.pop();
        }
        *s.trace.borrow_mut() = frames;
    });
    let s = state.clone();
    vm.register_fn_vm("%nrepl-debug", move |vm: &mut Vm, c: Value| debug(vm, &s, c));
    // In the root module, so `*1` and the helpers are seen from every module.
    vm.eval_in(ROOT_MODULE, INTERNAL, LISP).expect("nREPL helpers");
    let _ = shared_tx.send(shared);
    while let Ok(job) = state.rx.recv() {
        handle(&mut vm, &state, job);
    }
}

fn session_id(shared: &Shared) -> String {
    let n = shared.next.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{:08x}-{:04x}-{:012x}", (t >> 32) as u32, n as u16, t as u64 & 0xffff_ffff_ffff)
}

const OPS: &[&str] = &[
    "clone",
    "close",
    "ls-sessions",
    "describe",
    "eval",
    "load-file",
    "interrupt",
    "completions",
    "lookup",
    "info",
    "eldoc",
    "techne-debug-restart",
    "techne-debug-abort",
    "techne-inspect",
    "techne-inspect-part",
    "techne-inspect-pop",
];

fn dispatch(shared: &Arc<Shared>, msg: &B, out: &mpsc::UnboundedSender<B>, conn: u64) {
    let text = |k: &str| msg.get(k).and_then(B::as_str).map(str::to_string);
    let session = text("session");
    let reply = Reply { out: out.clone(), id: msg.get("id").cloned(), session: session.clone(), conn };
    if let Some(s) = &session
        && !shared.sessions.lock().unwrap().contains(s)
    {
        return reply.error(&format!("unknown session {s}"), &["unknown-session"]);
    }
    let op = text("op").unwrap_or_default();
    match op.as_str() {
        "clone" => {
            let id = session_id(shared);
            shared.sessions.lock().unwrap().insert(id.clone());
            reply.send(vec![("new-session", B::str(&id)), ("status", B::List(vec![B::str("done")]))]);
        }
        "close" => {
            if let Some(s) = session {
                shared.sessions.lock().unwrap().remove(&s);
                shared.post(Job::CloseSession(s));
            }
            reply.status(&["done", "session-closed"]);
        }
        "ls-sessions" => {
            let sessions = shared.sessions.lock().unwrap().iter().map(B::str).collect();
            reply.send(vec![("sessions", B::List(sessions))]);
            reply.status(&["done"]);
        }
        "describe" => {
            let ops = B::Dict(OPS.iter().map(|o| (o.as_bytes().to_vec(), dict([]))).collect());
            let version = |major: i64, minor: i64| dict([("major", B::Int(major)), ("minor", B::Int(minor)), ("incremental", B::Int(0))]);
            reply.send(vec![
                ("ops", ops),
                ("versions", dict([("nrepl", version(1, 0)), ("techne", version(0, 1))])),
                ("aux", dict([("current-ns", B::str("user"))])),
            ]);
            reply.status(&["done"]);
        }
        "eval" | "load-file" => {
            let (source, name, line, column) = if op == "eval" {
                let line = msg.get("line").and_then(B::as_int).unwrap_or(1).max(1) as usize;
                let column = msg.get("column").and_then(B::as_int).unwrap_or(1).max(1) as usize;
                (text("code"), text("file"), line, column)
            } else {
                (text("file"), text("file-path").or_else(|| text("file-name")), 1, 1)
            };
            let Some(source) = source else { return reply.error("eval needs code", &["no-code"]) };
            // Padding keeps line and column numbers of the client's file.
            let source = format!("{}{}{}", "\n".repeat(line - 1), " ".repeat(column - 1), source);
            let debug = msg.get("techne-debug").is_some_and(|d| d.as_int() == Some(1) || d.as_str() == Some("true"));
            shared.post(Job::Eval { name: name.unwrap_or_else(|| "<nrepl>".into()), source, ns: text("ns"), debug, reply });
        }
        "interrupt" => {
            let running = shared.running.lock().unwrap().last().cloned();
            match running {
                Some((s, id)) if s == session => {
                    if let Some(wanted) = msg.get("interrupt-id")
                        && Some(wanted) != id.as_ref()
                        && wanted.as_str() != id.as_ref().and_then(B::as_str)
                    {
                        return reply.status(&["error", "interrupt-id-mismatch", "done"]);
                    }
                    let paused = shared.debuggers.lock().unwrap().iter().find(|(_, (ds, _))| *ds == session).map(|(id, _)| *id);
                    match paused {
                        Some(id) => shared.post(Job::Debug { id, cmd: DebugCmd::Abort }),
                        None => shared.interrupt.interrupt(),
                    }
                    reply.status(&["done"]);
                }
                _ => reply.status(&["done", "session-idle"]),
            }
        }
        "completions" => shared.post(Job::Complete { prefix: text("prefix").unwrap_or_default(), ns: text("ns"), reply }),
        "lookup" | "info" | "eldoc" => {
            let kind = match op.as_str() {
                "lookup" => "lookup",
                "info" => "info",
                _ => "eldoc",
            };
            match text("sym").or_else(|| text("symbol")) {
                Some(sym) => shared.post(Job::Info { sym, kind, ns: text("ns"), reply }),
                None => reply.error("no symbol", &["no-symbol"]),
            }
        }
        "techne-debug-restart" | "techne-debug-abort" => {
            let id = msg.get("debug-id").and_then(B::as_int).unwrap_or(-1) as u64;
            if !shared.debuggers.lock().unwrap().contains_key(&id) {
                return reply.error(&format!("no debugger {id} is waiting"), &["no-debugger"]);
            }
            let cmd = if op == "techne-debug-abort" {
                DebugCmd::Abort
            } else {
                let index = msg.get("restart").and_then(B::as_int).unwrap_or(-1);
                let Ok(index) = usize::try_from(index) else { return reply.error("restart needs an index", &[]) };
                DebugCmd::Restart(index, text("args").unwrap_or_default())
            };
            shared.post(Job::Debug { id, cmd });
            reply.status(&["done"]);
        }
        "techne-inspect" => match text("code") {
            Some(code) => shared.post(Job::Inspect { action: Inspect::Code(code), reply }),
            None => reply.error("techne-inspect needs code", &[]),
        },
        "techne-inspect-part" => match msg.get("index").and_then(B::as_int) {
            Some(i) if i >= 0 => shared.post(Job::Inspect { action: Inspect::Part(i as usize), reply }),
            _ => reply.error("techne-inspect-part needs an index", &[]),
        },
        "techne-inspect-pop" => shared.post(Job::Inspect { action: Inspect::Pop, reply }),
        _ => reply.status(&["error", "unknown-op", "done"]),
    }
}

async fn connection(stream: tokio::net::TcpStream, shared: Arc<Shared>, conn: u64) {
    let (mut r, mut w) = stream.into_split();
    let (out, mut rx) = mpsc::unbounded_channel::<B>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let mut bytes = Vec::new();
            msg.encode(&mut bytes);
            if w.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    'read: loop {
        loop {
            match parse(&buf) {
                Ok(Some((msg, used))) => {
                    buf.drain(..used);
                    dispatch(&shared, &msg, &out, conn);
                }
                Ok(None) => break,
                Err(_) => break 'read,
            }
        }
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    // A debugger waiting for this client would wait forever.
    let orphaned: Vec<u64> = shared.debuggers.lock().unwrap().iter().filter(|(_, (_, c))| *c == conn).map(|(id, _)| *id).collect();
    for id in orphaned {
        shared.post(Job::Debug { id, cmd: DebugCmd::Abort });
    }
    drop(out);
    let _ = writer.await;
}

/// Serve nREPL on `addr` (e.g. "127.0.0.1:7888"; port 0 picks one). Calls
/// `ready` with the bound address, then serves until the process exits.
pub fn serve(addr: &str, ready: impl FnOnce(std::net::SocketAddr)) -> std::io::Result<()> {
    let (jobs, rx) = std::sync::mpsc::channel();
    let (shared_tx, shared_rx) = std::sync::mpsc::channel();
    let posted = jobs.clone();
    std::thread::Builder::new().name("techne-nrepl-vm".into()).spawn(move || vm_thread(rx, shared_tx, posted))?;
    let shared = shared_rx.recv().map_err(|_| std::io::Error::other("the VM did not start"))?;
    drop(jobs);
    techne_process::runtime().block_on(async {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        ready(listener.local_addr()?);
        let next = AtomicU64::new(1);
        loop {
            let (stream, _) = listener.accept().await?;
            let _ = stream.set_nodelay(true);
            tokio::spawn(connection(stream, shared.clone(), next.fetch_add(1, Ordering::Relaxed)));
        }
    })
}
