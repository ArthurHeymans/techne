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
//!   an error the code has restarts for, and with `"techne-debug": 2` (or
//!   `"always"`) on any error: every evaluation has a `retry` restart,
//!   which evaluates it again. The error's handler runs where it was
//!   raised, so its restarts are live: the server sends a message with
//!   status `techne-debug`, a `debug-id`, the `condition` (its message) and
//!   `condition-type`, the `restarts` (name and parameters, innermost first,
//!   this evaluation's only), the `frames` as text and as `frame-list`
//!   (`text`, `name`, `file`, `line`, `column` where known), and the
//!   evaluation waits. The client answers with `techne-debug-restart`
//!   (`debug-id`, `restart` index, optional `args` source) or
//!   `techne-debug-abort`; `techne-debug-inspect` (`debug-id`) opens the
//!   inspector on the condition, and `interrupt` stops the evaluation
//!   where it waits. However a debugger ends, the evaluation's request gets
//!   a message with status `techne-debug-end` and its `debug-id`.
//! - Debuggers for tasks no evaluation waits for (a request a host serves
//!   in the world): such a task's handler calls `(%nrepl-debug-request
//!   condition)`, which opens a debugger with every restart there is for
//!   the client that last sent `techne-debug-subscribe` and is still
//!   connected, and waits for its choice as above; its messages carry the
//!   subscription's id; restart arguments are evaluated in the module
//!   new sessions evaluate in. It returns #f, at once with no client
//!   subscribed, or once the client aborts or the connection ends: the
//!   handler raises the condition on. The server moving to another world
//!   (`Server::replace`) stops the task where it waits. A subscription is
//!   answered `techne-debug-subscribed`, never `done`: it lasts as long as
//!   its connection; a connection has one.
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
//! All sessions share one VM (`*1` `*2` `*3` `*e` are shared). Each
//! evaluation runs in a task of its own, in its session's module (current
//! for that task only): one that waits (in a debugger, for a process, a
//! timer) holds up no other, and `interrupt` stops the one it names (or
//! the session's latest) where it runs or waits. Other requests run on
//! the VM thread between the tasks' slices. The server listens on
//! localhost only.
//!
//! The service can also live in a world someone else drives ([`Server`]):
//! a host installs it into its world, polls it between the world's slices
//! and serves its network side from another thread. `serve` is that, with
//! a world of its own on a thread of its own. `techne-inspect` evaluates
//! its code at once, not in a task: in a host it holds up whatever else
//! the host runs on its thread until it returns.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use techne_vm::{
    api::{Arg, Value},
    complete,
    heap::Kind,
    tasks::{Progress, TaskId},
    vm::{Capability, Error, ExecId, Grants, InterruptHandle, ROOT_MODULE, Stop, USER_MODULE, Vm},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};

use crate::bencode::{B, dict, parse};

/// The file name of the server's own Lisp code (cut from backtraces).
const INTERNAL: &str = "<nrepl-internal>";

const LISP: &str = r#"
(define *1 #f) (define *2 #f) (define *3 #f) (define *e #f)
(define (%nrepl-interrupt? c)
  (and (error-object? c) (equal? (error-object-message c) "interrupted")))
(define (%nrepl-condition-type c)
  (cond ((file-error? c) "file error")
        ((read-error? c) "read error")
        ((error-object? c) "error")
        (else (call-with-output-string (lambda (p) (display (type-of c) p))))))
(define (%nrepl-thunk key port debug name source module)
  (lambda () (%nrepl-run key port debug name source module)))
(define (%nrepl-run key port debug name source module)
  ;; Evaluation KEY, in a task of its own; `%nrepl-done` reports it.
  ;; DEBUG: 0 never pauses, 1 pauses on an error the code has restarts
  ;; for, 2 on every error. A `retry` restart evaluates the input again
  ;; (after a redefinition, say).
  (parameterize ((current-output-port port))
    (in-module module)
    (let* ((trace '())
           (result
            (let again ()
              (guard (e (#t (set! *e e) (list 'error e)))
                (restart-case
                 (let* ((mine (car (compute-restarts)))
                        (v (with-exception-handler
                            (lambda (c)
                              (set! trace (%nrepl-backtrace))
                              (if (and (not (%nrepl-interrupt? c))
                                       (or (= debug 2)
                                           (and (= debug 1) (not (eq? (car (compute-restarts)) mine)))))
                                  (%nrepl-debug key c mine trace)
                                  (raise c)))
                            (lambda () (%eval-forms (%read-forms source module name))))))
                   (unless (eq? v (if #f #f))
                     (set! *3 *2) (set! *2 *1) (set! *1 v))
                   (list 'ok v))
                 (retry () (again)))))))
      (%nrepl-done key result trace (current-module)))))

;; The debugger: the evaluation's task waits for the client's choice, a
;; restart (index . arguments source) or `abort`, on a channel of its own.
(define %nrepl-choices (make-hash-table))
(define (%nrepl-own-restarts mine)
  ;; This evaluation's restarts: down to its own `retry`, MINE.
  (let loop ((rs (compute-restarts)) (acc '()))
    (cond ((null? rs) (reverse acc))
          ((eq? (car rs) mine) (reverse (cons (car rs) acc)))
          (else (loop (cdr rs) (cons (car rs) acc))))))
(define (%nrepl-debug key c mine trace)
  ;; The channel is registered before the client hears of the debugger:
  ;; a choice it sends at once finds it.
  (let* ((restarts (%nrepl-own-restarts mine))
         (choices (make-channel 1))
         (id (%nrepl-debug-id key)))
    (hash-table-set! %nrepl-choices id choices)
    (let ((choice (dynamic-wind
                   (lambda () #f)
                   (lambda () (if (%nrepl-debug-open id c restarts trace) (channel-recv choices) 'abort))
                   (lambda () (%nrepl-forget id) (%nrepl-debug-close key id)))))
      (if (pair? choice)
          (%nrepl-invoke restarts choice (current-module))
          (raise c)))))
(define (%nrepl-invoke restarts choice module)
  ;; The restart CHOICE picks, with its arguments evaluated in MODULE.
  (apply invoke-restart (list-ref restarts (car choice))
         (if (equal? (cdr choice) "")
             '()
             (eval-source (string-append "(list " (cdr choice) ")") module "<restart arguments>"))))
(define (%nrepl-debug-request c)
  ;; A debugger for C, raised in a task no evaluation waits for (a request
  ;; a host serves), with every restart there is, for the client
  ;; subscribed; #f if none is, or it aborts. Restart arguments are
  ;; evaluated in the module new sessions evaluate in.
  (let* ((restarts (compute-restarts))
         (choices (make-channel 1))
         (id (%nrepl-debug-id 0))
         (module #f))
    (hash-table-set! %nrepl-choices id choices)
    (let ((choice (dynamic-wind
                   (lambda () #f)
                   (lambda ()
                     (set! module (%nrepl-debug-request-open id c restarts))
                     (if module (channel-recv choices) 'abort))
                   (lambda () (%nrepl-forget id) (%nrepl-debug-close 0 id)))))
      (and (pair? choice) (%nrepl-invoke restarts choice module)))))
(define (%nrepl-choose id choice)
  (let ((choices (hash-table-ref/default %nrepl-choices id #f)))
    (and choices (begin (channel-send choices choice) #t))))
(define (%nrepl-forget id)
  ;; Closed and drained, the channel's slot is free again.
  (let ((choices (hash-table-ref/default %nrepl-choices id #f)))
    (when choices
      (hash-table-delete! %nrepl-choices id)
      (channel-close choices)
      (let drain () (unless (eof-object? (channel-recv choices)) (drain))))))
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
    /// The condition a debugger waits with.
    Condition(u64),
    Part(usize),
    Pop,
}

enum Job {
    Eval {
        name: String,
        source: String,
        ns: Option<String>,
        debug: u8,
        reply: Reply,
    },
    /// Tasks may run again: a future they wait on is ready, or a stop.
    Wake,
    Complete {
        prefix: String,
        ns: Option<String>,
        reply: Reply,
    },
    Info {
        sym: String,
        kind: &'static str,
        ns: Option<String>,
        reply: Reply,
    },
    Inspect {
        action: Inspect,
        reply: Reply,
    },
    CloseSession(String),
    Debug {
        id: u64,
        cmd: DebugCmd,
    },
}

impl Job {
    /// Where it is answered, if it is.
    fn reply(&self) -> Option<&Reply> {
        match self {
            Job::Eval { reply, .. } | Job::Complete { reply, .. } | Job::Info { reply, .. } | Job::Inspect { reply, .. } => Some(reply),
            Job::Wake | Job::CloseSession(_) | Job::Debug { .. } => None,
        }
    }
}

/// State shared with the network side.
struct Shared {
    jobs: Mutex<std::sync::mpsc::Sender<Job>>,
    /// Called after a job is posted: the world's owner runs `Server::poll`.
    wake: Box<dyn Fn() + Send + Sync>,
    sessions: Mutex<HashSet<String>>,
    /// The `techne-debug-subscribe` requests, in the order they came.
    subscribers: Mutex<Vec<Reply>>,
    /// Evaluations in progress, by key (in the order they started).
    running: Mutex<std::collections::BTreeMap<u64, Running>>,
    debuggers: Mutex<Debuggers>,
    next: AtomicU64,
}

/// The debuggers waiting for a choice, and the connections gone.
#[derive(Default)]
struct Debuggers {
    waiting: HashMap<u64, Waiting>,
    /// Connections that ended: a debugger opened for one afterwards would
    /// wait for ever, so it is not opened.
    gone: HashSet<u64>,
}

struct Waiting {
    conn: u64,
    /// How many restarts it offers.
    restarts: usize,
}

/// An evaluation in progress, for `interrupt`.
struct Running {
    session: Option<String>,
    /// Its request's id.
    id: Option<B>,
    /// The execution of its task, and the interrupt handle of its world:
    /// a world the server moved from keeps its own.
    exec: Option<(ExecId, InterruptHandle)>,
}

/// A debugger of a task no evaluation waits for.
struct External {
    /// The subscription told.
    reply: Reply,
    task: TaskId,
    /// The task's execution, stopped if the server moves to another world.
    exec: Option<ExecId>,
}

/// An evaluation in progress, on the VM thread.
struct Evaluation {
    reply: Reply,
    task: TaskId,
}

impl Shared {
    fn post(&self, job: Job) {
        let _ = self.jobs.lock().unwrap().send(job);
        (self.wake)();
    }
}

/// An inspected value and its labelled parts.
struct Shown {
    value: Value,
    parts: Vec<(String, Value)>,
}

/// State of the VM thread.
struct State {
    /// The interrupt handle of the world.
    interrupt: InterruptHandle,
    shared: Arc<Shared>,
    /// The evaluations in progress, by key.
    evals: RefCell<HashMap<u64, Evaluation>>,
    inspectors: RefCell<HashMap<Option<String>, Vec<Shown>>>,
    /// The conditions of the debuggers waiting, by id.
    conditions: RefCell<HashMap<u64, Value>>,
    /// The evaluation of each debugger, by id, from its id's making to its
    /// end: closed with it if its task ends without closing it (killed).
    debugger_evals: RefCell<HashMap<u64, u64>>,
    /// Each session's current module.
    modules: RefCell<HashMap<Option<String>, u32>>,
    /// The debuggers of tasks no evaluation waits for, by id: the
    /// subscription told, and the task, closed with it if it ends without
    /// closing it (killed).
    external: RefCell<HashMap<u64, External>>,
    /// A new session's module (`Server::set_module`).
    default_module: std::cell::Cell<u32>,
    /// Why the world is retired (poisoned), once it is: what is asked
    /// from then on fails, and no Lisp runs.
    retired: RefCell<Option<String>>,
}

impl State {
    fn session_module(&self, session: &Option<String>) -> u32 {
        self.modules.borrow().get(session).copied().unwrap_or(self.default_module.get())
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

fn condition_text(vm: &mut Vm, c: &Value) -> String {
    vm.call_global("condition/report-string", std::slice::from_ref(c))
        .ok()
        .and_then(|s| vm.get::<String>(&s).ok())
        .unwrap_or_else(|| vm.repr(c))
}

/// Start an evaluation: a task of its own runs it (`%nrepl-run`), so it
/// can wait (for a debugger's choice, a process, a timer) while others go
/// on, and reports it (`finish`).
fn start_eval(vm: &mut Vm, state: &Rc<State>, name: String, source: String, ns: Option<String>, debug: u8, reply: Reply) {
    let module = match state.module(vm, &ns, &reply.session) {
        Ok(m) => m,
        Err(e) => return reply.error(&e.to_string(), &["namespace-not-found"]),
    };
    let key = state.shared.next.fetch_add(1, Ordering::Relaxed);
    let sink = reply.clone();
    let task = techne_vm::stdlib::make_output_port(vm, move |text| sink.send(vec![("out", B::str(text))])).and_then(|port| {
        let module = vm.module_name(module);
        let args = [
            Value::fixnum(key as i64).expect("a key"),
            port,
            Value::fixnum(i64::from(debug)).expect("a small integer"),
            vm.make_string(&name)?,
            vm.make_string(&source)?,
            vm.make_string(&module)?,
        ];
        let thunk = vm.call_global("%nrepl-thunk", &args)?;
        vm.try_spawn(&thunk)
    });
    match task {
        Ok(task) => {
            let exec = vm.task_execution(&task).map(|e| (e, state.interrupt.clone()));
            state.shared.running.lock().unwrap().insert(key, Running { session: reply.session.clone(), id: reply.id.clone(), exec });
            state.evals.borrow_mut().insert(key, Evaluation { reply, task });
        }
        Err(e) => reply.error(&e.to_string(), &[]),
    }
}

/// Report evaluation `key`: RESULT is (ok value) or (error condition),
/// TRACE where the condition was raised, MODULE the one current at its end.
fn finish(vm: &mut Vm, state: &Rc<State>, key: u64, result: &Value, trace: &[String], module: &str) {
    let Some(eval) = state.evals.borrow_mut().remove(&key) else { return };
    state.shared.running.lock().unwrap().remove(&key);
    let reply = eval.reply;
    let after = vm.loaded_module(module).unwrap_or(USER_MODULE);
    state.modules.borrow_mut().insert(reply.session.clone(), after);
    let parts: Vec<Value> = vm.get(result).unwrap_or_default();
    match &parts[..] {
        [tag, v] if tag.symbol_name().as_deref() == Some("ok") => {
            if !v.is_void() {
                reply.send(vec![("value", B::str(vm.repr(v))), ("ns", B::str(module))]);
            }
            reply.status(&["done"]);
        }
        [_, condition] => {
            let message = condition_text(vm, condition);
            report_error(&reply, &message, trace);
        }
        _ => reply.error("the evaluation reported nothing", &[]),
    }
}

/// Report an evaluation that failed with MESSAGE, raised at TRACE.
fn report_error(reply: &Reply, message: &str, trace: &[String]) {
    if message == techne_vm::vm::INTERRUPTED {
        reply.status(&["interrupted"]);
        reply.status(&["done"]);
        return;
    }
    let mut err = format!("error: {message}\n");
    for frame in trace {
        err.push_str(&format!("  in {frame}\n"));
    }
    reply.send(vec![("err", B::str(err))]);
    reply.send(vec![("ex", B::str("error")), ("root-ex", B::str("error")), ("status", B::List(vec![B::str("eval-error")]))]);
    reply.status(&["done"]);
}

/// Report the evaluations whose task ended without reporting (killed,
/// say).
fn report_ended(vm: &mut Vm, state: &Rc<State>) {
    // The debuggers of tasks no evaluation waits for, if they ended
    // without closing them.
    let tasks: Vec<(u64, TaskId)> = state.external.borrow().iter().map(|(id, e)| (*id, e.task.clone())).collect();
    for (id, task) in tasks {
        if vm.task_result(&task).is_some() {
            let _ = vm.call_global("%nrepl-forget", &[Value::fixnum(id as i64).expect("an id")]);
            debug_close(state, 0, id);
        }
    }
    let ended: Vec<(u64, Result<Value, Error>)> = {
        let evals = state.evals.borrow();
        evals.iter().filter_map(|(k, e)| vm.task_result(&e.task).map(|r| (*k, r))).collect()
    };
    for (key, result) in ended {
        // Its debuggers, if it was killed waiting in one.
        let debuggers: Vec<u64> = state.debugger_evals.borrow().iter().filter(|(_, k)| **k == key).map(|(id, _)| *id).collect();
        for id in debuggers {
            let _ = vm.call_global("%nrepl-forget", &[Value::fixnum(id as i64).expect("an id")]);
            debug_close(state, key, id);
        }
        let Some(eval) = state.evals.borrow_mut().remove(&key) else { continue };
        state.shared.running.lock().unwrap().remove(&key);
        match result {
            Ok(_) => eval.reply.status(&["done"]),
            Err(e) => report_error(&eval.reply, &e.msg, &[]),
        }
    }
}

/// `%nrepl-debug-open`: debugger `id` (`%nrepl-debug-id`) for evaluation
/// `key`, paused on `condition` with `restarts` (its own) and `trace`.
/// Tells the client; false if the client has gone.
fn debug_open(
    vm: &mut Vm,
    state: &Rc<State>,
    key: u64,
    id: u64,
    condition: Value,
    restarts: &[Value],
    trace: &[String],
) -> Result<bool, Error> {
    let Some(reply) = state.evals.borrow().get(&key).map(|e| e.reply.clone()) else { return Ok(false) };
    debug_open_for(vm, state, &reply, id, condition, restarts, trace)
}

/// The client that last subscribed to debuggers, still connected.
fn subscriber(shared: &Shared) -> Option<Reply> {
    shared.subscribers.lock().unwrap().last().cloned()
}

/// Debugger `id` told to `reply`: see `debug_open`.
fn debug_open_for(
    vm: &mut Vm,
    state: &Rc<State>,
    reply: &Reply,
    id: u64,
    condition: Value,
    restarts: &[Value],
    trace: &[String],
) -> Result<bool, Error> {
    let mut described = Vec::new();
    for r in restarts {
        let name = vm.call_global("restart-name", std::slice::from_ref(r))?;
        // The formals as written: (v) -> "v", (a . rest) -> "a . rest".
        let formals = vm.call_global("restart-formals", std::slice::from_ref(r))?;
        let written = vm.repr(&formals);
        let params = match written.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            Some(inner) => inner.to_string(),
            None => format!(". {written}"),
        };
        described.push(dict([("name", B::str(vm.repr(&name))), ("params", B::str(params))]));
    }
    {
        let mut debuggers = state.shared.debuggers.lock().unwrap();
        // Its client has gone: no one would answer.
        if debuggers.gone.contains(&reply.conn) {
            return Ok(false);
        }
        debuggers.waiting.insert(id, Waiting { conn: reply.conn, restarts: restarts.len() });
    }
    let message = condition_text(vm, &condition);
    let kind = vm.call_global("%nrepl-condition-type", std::slice::from_ref(&condition)).ok().and_then(|t| vm.get::<String>(&t).ok());
    state.conditions.borrow_mut().insert(id, condition);
    reply.send(vec![
        ("status", B::List(vec![B::str("techne-debug")])),
        ("debug-id", B::Int(id as i64)),
        ("condition", B::str(message)),
        ("condition-type", B::str(kind.unwrap_or_default())),
        ("restarts", B::List(described)),
        ("frames", B::List(trace.iter().map(B::str).collect())),
        ("frame-list", B::List(trace.iter().map(|f| frame_fields(f)).collect())),
    ]);
    Ok(true)
}

/// `%nrepl-debug-close`: debugger `id` of evaluation `key` ended, however
/// it did; the client closes what shows it.
fn debug_close(state: &Rc<State>, key: u64, id: u64) {
    state.shared.debuggers.lock().unwrap().waiting.remove(&id);
    state.conditions.borrow_mut().remove(&id);
    state.debugger_evals.borrow_mut().remove(&id);
    let end = vec![("status", B::List(vec![B::str("techne-debug-end")])), ("debug-id", B::Int(id as i64))];
    if let Some(external) = state.external.borrow_mut().remove(&id) {
        external.reply.send(end);
    } else if let Some(eval) = state.evals.borrow().get(&key) {
        eval.reply.send(end);
    }
}

/// Hand debugger `id` the client's choice; once only.
fn choose(vm: &mut Vm, state: &Rc<State>, id: u64, cmd: DebugCmd) {
    if state.shared.debuggers.lock().unwrap().waiting.remove(&id).is_none() {
        return;
    }
    let choice = match cmd {
        DebugCmd::Abort => Ok(Value::symbol("abort")),
        DebugCmd::Restart(i, args) => {
            let i = Value::fixnum(i as i64).expect("an index");
            vm.make_string(&args).and_then(|args| vm.call_global("cons", &[i, args]))
        }
    };
    // Registered before its debugger was announced, the channel is there
    // unless the debugger ended meanwhile; it holds the one choice.
    let _ = choice.and_then(|c| vm.call_global("%nrepl-choose", &[Value::fixnum(id as i64).expect("an id"), c]));
}

/// The user's frames where the condition a handler runs for was raised:
/// down to the evaluation's own (the server's code, the prelude's loop).
fn user_backtrace(vm: &Vm) -> Vec<String> {
    let mut frames: Vec<String> =
        vm.raise_backtrace().iter().take_while(|f| !f.contains(INTERNAL) && !f.starts_with("%eval-forms ")).cloned().collect();
    while frames.last().is_some_and(|f| f.starts_with("with-exception-handler (")) {
        frames.pop();
    }
    frames
}

/// A frame of a backtrace, "name (file:line:col)", as its parts: `text`,
/// and `name`, `file`, `line` and `column` where it has them. Names have no
/// " (" in them, file names may: the first one ends the name.
fn frame_fields(frame: &str) -> B {
    let mut fields = vec![("text", B::str(frame))];
    if let Some((name, at)) = frame.split_once(" (")
        && let Some(at) = at.strip_suffix(')')
    {
        fields.push(("name", B::str(name)));
        let mut parts = at.rsplitn(3, ':');
        match (parts.next().and_then(|c| c.parse().ok()), parts.next().and_then(|l| l.parse().ok()), parts.next()) {
            (Some(column), Some(line), Some(file)) => {
                fields.extend([("file", B::str(file)), ("line", B::Int(line)), ("column", B::Int(column))]);
            }
            _ => fields.push(("file", B::str(at))),
        }
    }
    dict(fields)
}

fn is_procedure(vm: &mut Vm, v: &Value) -> bool {
    vm.call_global("procedure?", std::slice::from_ref(v)).is_ok_and(|r| r.is_truthy())
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
    let proc_info = v.as_ref().filter(|v| is_procedure(vm, v)).and_then(|v| vm.procedure_info(v));
    let described = vm.describe_binding(module, sym);
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
    let type_name = match (&proc_info, &v) {
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

/// Part `i` of `f`, or void if a failed look gave fewer.
fn field(f: &[Value], i: usize) -> Value {
    f.get(i).cloned().unwrap_or(Value::VOID)
}

/// At most this many parts of a value are shown (and held).
const MAX: usize = 500;

/// The labelled parts of a value, for the inspector: only those shown
/// are held.
fn parts(vm: &mut Vm, v: &Value) -> (String, Vec<(String, Value)>) {
    let numbered = |items: Vec<Value>| items.into_iter().enumerate().map(|(i, x)| (i.to_string(), x)).collect();
    // Up to MAX of `items`, held, and how many there are.
    fn first(items: Vec<Arg<'_>>) -> (usize, Vec<Value>) {
        (items.len(), items.iter().take(MAX).map(Arg::value).collect())
    }
    let kind = vm.inspect(v, |a| a.kind()).ok().flatten();
    match kind {
        Some(Kind::Pair) => match vm.inspect(v, |a| a.items().map(first)).ok().flatten() {
            Some((n, items)) => (format!("list of {n} elements"), numbered(items)),
            None => {
                let f = vm.inspect(v, |a| a.fields().iter().map(Arg::value).collect::<Vec<_>>()).unwrap_or_default();
                ("pair".into(), vec![("car".into(), field(&f, 0)), ("cdr".into(), field(&f, 1))])
            }
        },
        Some(Kind::Vector) => {
            let (n, items) = vm.inspect(v, |a| first(a.fields())).unwrap_or_default();
            (format!("vector of {n} elements"), numbered(items))
        }
        Some(Kind::Record) => {
            // The type's name and field names, then the fields.
            let (name, names, fields) = vm
                .inspect(v, |a| {
                    let f = a.fields();
                    let rtd = f.first().map(Arg::fields).unwrap_or_default();
                    let names = rtd.get(1).and_then(Arg::items).unwrap_or_default().iter().map(Arg::brief).collect::<Vec<_>>();
                    (rtd.first().map(Arg::brief).unwrap_or_default(), names, first(f.get(1..).unwrap_or_default().to_vec()).1)
                })
                .unwrap_or_default();
            let parts = fields
                .into_iter()
                .enumerate()
                .map(|(i, x)| (names.get(i).cloned().unwrap_or_else(|| format!("field {}", i + 1)), x))
                .collect();
            (format!("record {name}"), parts)
        }
        Some(Kind::Rtd | Kind::Box) => {
            let f = vm.inspect(v, |a| a.fields().iter().map(Arg::value).collect::<Vec<_>>()).unwrap_or_default();
            match kind {
                Some(Kind::Rtd) => ("record type".into(), vec![("name".into(), field(&f, 0)), ("fields".into(), field(&f, 1))]),
                _ => ("box".into(), vec![("contents".into(), field(&f, 0))]),
            }
        }
        Some(Kind::Table) => {
            let alist = vm.call_global("hash-table->alist", std::slice::from_ref(v)).unwrap_or(Value::NIL);
            let (n, parts) = vm
                .inspect(&alist, |a| {
                    let entries = a.items().unwrap_or_default();
                    let parts = entries.iter().take(MAX).filter_map(|e| match e.fields()[..] {
                        [k, v] => Some((k.brief(), v.value())),
                        _ => None,
                    });
                    (entries.len(), parts.collect())
                })
                .unwrap_or_default();
            (format!("hash table of {n} entries"), parts)
        }
        Some(Kind::Dict) => {
            let (n, parts) = vm
                .inspect(v, |a| {
                    let entries = a.dict_entries();
                    (entries.len() / 2, entries.chunks(2).take(MAX).map(|e| (e[0].brief(), e[1].value())).collect())
                })
                .unwrap_or_default();
            (format!("dict of {n} entries"), parts)
        }
        Some(Kind::Set) => {
            let (n, items) = vm.inspect(v, |a| first(a.dict_entries())).unwrap_or_default();
            (format!("set of {n} elements"), numbered(items))
        }
        Some(Kind::Closure) => {
            let mut parts = Vec::new();
            let mut title = "procedure".to_string();
            if let Some(i) = vm.procedure_info(v) {
                title = format!("procedure {}", i.name);
                let mut text = vec![("parameters".to_string(), format!("({})", i.params.join(" ")))];
                if let Some(doc) = &i.doc {
                    text.push(("documentation".into(), doc.to_string()));
                }
                if let Some(file) = &i.file {
                    text.push(("defined at".into(), format!("{file}:{}:{}", i.line, i.column)));
                }
                for (label, s) in text {
                    if let Ok(s) = vm.make_string(&s) {
                        parts.push((label, s));
                    }
                }
            }
            let captured = vm.inspect(v, |a| first(a.fields()).1).unwrap_or_default();
            parts.extend(captured.into_iter().enumerate().map(|(i, x)| (format!("captured {}", i + 1), x)));
            (title, parts)
        }
        Some(Kind::String) => ("string".into(), Vec::new()),
        _ => {
            let ty = vm.call_global("type-of", std::slice::from_ref(v)).map(|t| vm.repr(&t)).unwrap_or_default();
            (ty, Vec::new())
        }
    }
}

fn show(vm: &mut Vm, value: Value) -> Shown {
    let (_, parts) = parts(vm, &value);
    Shown { value, parts }
}

fn send_view(vm: &mut Vm, stack: &[Shown], reply: &Reply) {
    let Some(top) = stack.last() else {
        reply.error("nothing is being inspected", &[]);
        return;
    };
    let (title, _) = parts(vm, &top.value);
    let parts = top.parts.iter().map(|(label, v)| B::List(vec![B::str(label), B::str(truncate(vm.repr(v), 200))])).collect();
    reply.send(vec![
        ("title", B::str(title)),
        ("value", B::str(truncate(vm.repr(&top.value), 2000))),
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
        Inspect::Condition(id) => match state.conditions.borrow().get(&id).cloned() {
            Some(c) => {
                let shown = show(vm, c);
                state.inspectors.borrow_mut().insert(key.clone(), vec![shown]);
            }
            None => return reply.error(&format!("no debugger {id} is waiting"), &["no-debugger"]),
        },
        Inspect::Part(i) => {
            let part = state.inspectors.borrow().get(&key).and_then(|s| s.last()).and_then(|top| top.parts.get(i)).map(|(_, v)| v.clone());
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
        Job::Eval { name, source, ns, debug, reply } => start_eval(vm, state, name, source, ns, debug, reply),
        Job::Wake => {}
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
        Job::Debug { id, cmd } => choose(vm, state, id, cmd),
    }
}

/// Most jobs `Server::poll` handles at once.
const JOBS_PER_POLL: usize = 64;

/// The nREPL service of a world someone else drives: [`Server::install`]
/// it, then in turn [`Server::poll`] (requests, which may give the world
/// tasks to run), a slice of the world's tasks, and [`Server::reap`]
/// (evaluations that ended). Go round at once while `poll` says more
/// requests wait or the slice ran out of time; else wait until the `wake`
/// given to `install` is called or the world's next timer
/// (`Vm::next_timer`), whichever comes first. Serve the network side
/// ([`Remote::serve`]) on another thread. Evaluations run as tasks of the
/// world, so the owner's slices run them.
pub struct Server {
    rx: std::sync::mpsc::Receiver<Job>,
    state: Rc<State>,
}

/// The network side of a [`Server`], which can be sent to another thread.
#[derive(Clone)]
pub struct Remote(Arc<Shared>);

impl Server {
    /// Install the service into `vm`: its natives and, in the root module,
    /// its Lisp helpers and `*1` `*2` `*3` `*e`. It grants nothing: what
    /// evaluations reach is what `vm` was granted. `wake` is called, from
    /// any thread, when a request arrives. Set `vm`'s wake notifier first:
    /// the interrupt handle made here calls it to stop an evaluation.
    /// Install into a world itself, not into one that is snapshotted: the
    /// natives hold this server.
    pub fn install(vm: &mut Vm, wake: impl Fn() + Send + Sync + 'static) -> Result<Server, Error> {
        let (jobs, rx) = std::sync::mpsc::channel();
        let shared = Arc::new(Shared {
            jobs: Mutex::new(jobs),
            wake: Box::new(wake),
            sessions: Mutex::new(HashSet::new()),
            subscribers: Mutex::default(),
            running: Mutex::default(),
            debuggers: Mutex::default(),
            next: AtomicU64::new(1),
        });
        Ok(Server { rx, state: attach(vm, shared)? })
    }

    /// Move the service to `vm`, a world in place of the one it was in (a
    /// deployment deployed again): clients stay connected and their
    /// sessions go on, in `vm`; what was running in the world before fails
    /// with `why`, its debuggers closed, without running Lisp there. As for
    /// `install`, set `vm`'s wake notifier first. If the service cannot be
    /// installed in `vm`, it stays where it was, and is handed back.
    pub fn replace(self, vm: &mut Vm, why: &str) -> Result<Server, (Server, Error)> {
        let state = match attach(vm, self.state.shared.clone()) {
            Ok(state) => state,
            Err(e) => return Err((self, e)),
        };
        if self.state.retired.borrow().is_none() {
            retire(&self.state, why.to_string());
        }
        Ok(Server { rx: self.rx, state })
    }

    /// Evaluate in `module` from now on where a request names none and its
    /// session has not changed module (`in-module`): `user` until then.
    /// A host serving a deployment gives the deployment's module, so that
    /// a definition sent from its file redefines what the file defined.
    pub fn set_module(&self, module: u32) {
        self.state.default_module.set(module);
    }

    /// The network side, to serve from another thread.
    pub fn remote(&self) -> Remote {
        Remote(self.state.shared.clone())
    }

    /// Handle the requests waiting, up to a bound: whether more wait. It
    /// runs no task and never blocks, but may give the world tasks to run
    /// (an evaluation, a debugger's choice): run a slice after it.
    pub fn poll(&self, vm: &mut Vm) -> bool {
        for _ in 0..JOBS_PER_POLL {
            match self.rx.try_recv() {
                Ok(job) => self.handle(vm, job),
                Err(_) => return false,
            }
        }
        true
    }

    /// Report the evaluations whose tasks ended: call it after a slice of
    /// the world's tasks. A poisoned world retires the server: its
    /// evaluations fail, and so does whatever is asked after.
    pub fn reap(&self, vm: &mut Vm) {
        let state = &self.state;
        if state.retired.borrow().is_none() {
            match vm.poisoned() {
                Some(why) => retire(state, format!("the world is retired: {why}")),
                None => report_ended(vm, state),
            }
        }
    }

    /// Whether the world is retired: nothing more runs in it.
    pub fn retired(&self) -> bool {
        self.state.retired.borrow().is_some()
    }

    fn handle(&self, vm: &mut Vm, job: Job) {
        match &*self.state.retired.borrow() {
            Some(why) => {
                if let Some(reply) = job.reply() {
                    reply.error(why, &[]);
                }
            }
            None => handle(vm, &self.state, job),
        }
    }

    /// Wait for a request until `until` (for ever without one) and handle
    /// it: false once no one can ask any more.
    fn wait(&self, vm: &mut Vm, until: Option<Instant>) -> bool {
        let job = match until {
            Some(at) => match self.rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(job) => job,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return true,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return false,
            },
            None => match self.rx.recv() {
                Ok(job) => job,
                Err(_) => return false,
            },
        };
        self.handle(vm, job);
        true
    }
}

/// The service's state in `vm`, its natives and Lisp helpers installed
/// there.
fn attach(vm: &mut Vm, shared: Arc<Shared>) -> Result<Rc<State>, Error> {
    let state = Rc::new(State {
        shared,
        interrupt: vm.interrupt_handle(),
        evals: RefCell::new(HashMap::new()),
        inspectors: RefCell::new(HashMap::new()),
        conditions: RefCell::new(HashMap::new()),
        debugger_evals: RefCell::new(HashMap::new()),
        modules: RefCell::new(HashMap::new()),
        default_module: std::cell::Cell::new(USER_MODULE),
        external: RefCell::new(HashMap::new()),
        retired: RefCell::new(None),
    });
    let s = state.clone();
    techne_vm::procedures! { vm;
        #[vm]
        "(%nrepl-done key result trace module)" => move |vm: &mut Vm, key: i64, result: Value, trace: Vec<String>, module: String| {
            finish(vm, &s, key as u64, &result, &trace, &module);
        };
    }
    techne_vm::procedures! { vm;
        #[vm]
        "(%nrepl-backtrace)" => |vm: &mut Vm| -> Vec<String> { user_backtrace(vm) };
    }
    let s = state.clone();
    techne_vm::procedures! { vm;
        #[vm]
        "(%nrepl-debug-id key)" => move |_vm: &mut Vm, key: i64| -> i64 {
            let id = s.shared.next.fetch_add(1, Ordering::Relaxed);
            s.debugger_evals.borrow_mut().insert(id, key as u64);
            id as i64
        };
    }
    let s = state.clone();
    techne_vm::procedures! { vm;
        #[vm]
        "(%nrepl-debug-open id condition restarts trace)" =>
            move |vm: &mut Vm, id: i64, c: Value, restarts: Vec<Value>, trace: Vec<String>| -> Result<bool, Error> {
                let Some(key) = s.debugger_evals.borrow().get(&(id as u64)).copied() else { return Ok(false) };
                debug_open(vm, &s, key, id as u64, c, &restarts, &trace)
            };
    }
    let s = state.clone();
    techne_vm::procedures! { vm;
        #[vm]
        "(%nrepl-debug-request-open id condition restarts)" =>
            move |vm: &mut Vm, id: i64, c: Value, restarts: Vec<Value>| -> Result<Value, Error> {
                // Moved to another world: a choice would reach that one.
                if s.retired.borrow().is_some() {
                    return Ok(Value::FALSE);
                }
                let (Some(reply), Some(task)) = (subscriber(&s.shared), vm.running_task()) else { return Ok(Value::FALSE) };
                // Called from the handler: where the condition was raised.
                let trace = user_backtrace(vm);
                let exec = vm.task_execution(&task);
                s.external.borrow_mut().insert(id as u64, External { reply: reply.clone(), task, exec });
                if !debug_open_for(vm, &s, &reply, id as u64, c, &restarts, &trace)? {
                    return Ok(Value::FALSE);
                }
                let module = vm.module_name(s.default_module.get());
                vm.make_string(&module)
            };
    }
    let s = state.clone();
    techne_vm::procedures! { vm;
        #[vm]
        "(%nrepl-debug-close key id)" => move |_vm: &mut Vm, key: i64, id: i64| { debug_close(&s, key as u64, id as u64); };
    }
    // In the root module, so `*1` and the helpers are seen from every module.
    vm.eval_in(ROOT_MODULE, INTERNAL, LISP)?;
    Ok(state)
}

/// Retire the server: debuggers closed and evaluations failed, without
/// running Lisp.
fn retire(state: &Rc<State>, why: String) {
    // Tasks waiting for a debugger's choice would wait for ever, holding
    // what they serve: they are stopped where they wait.
    let external: Vec<(u64, Option<ExecId>)> = state.external.borrow().iter().map(|(id, e)| (*id, e.exec)).collect();
    for (id, exec) in external {
        if let Some(exec) = exec {
            state.interrupt.stop(exec, Stop::Break);
        }
        debug_close(state, 0, id);
    }
    let debuggers: Vec<(u64, u64)> = state.debugger_evals.borrow().iter().map(|(id, key)| (*id, *key)).collect();
    for (id, key) in debuggers {
        debug_close(state, key, id);
    }
    for (_, eval) in state.evals.borrow_mut().drain() {
        eval.reply.error(&why, &[]);
    }
    state.shared.running.lock().unwrap().clear();
    *state.retired.borrow_mut() = Some(why);
}

impl Remote {
    /// Serve nREPL on `addr` (e.g. "127.0.0.1:7888"; port 0 picks one).
    /// Calls `ready` with the bound address, then serves until the process
    /// exits.
    pub fn serve(self, addr: &str, ready: impl FnOnce(std::net::SocketAddr)) -> std::io::Result<()> {
        let shared = self.0;
        techne_process::runtime().block_on(async {
            let listener = tokio::net::TcpListener::bind(addr).await?;
            ready(listener.local_addr()?);
            loop {
                let (stream, _) = listener.accept().await?;
                let _ = stream.set_nodelay(true);
                tokio::spawn(connection(stream, shared.clone(), NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed)));
            }
        })
    }

    /// Serve one client on `stream`, a connection relayed from elsewhere
    /// (a sandboxed worker's supervisor, which has the network), as one
    /// accepted by `serve`, until it ends.
    pub fn serve_stream(&self, stream: impl AsyncRead + AsyncWrite + Send + 'static) {
        let shared = self.0.clone();
        techne_process::runtime().spawn(connection(stream, shared, NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed)));
    }

    /// Have the world's owner poll, as a request arriving does.
    fn wake(&self) {
        self.0.post(Job::Wake);
    }
}

/// The server's own world, on its thread: requests, then the evaluations'
/// tasks, in turn, waiting for either when there is nothing to do.
fn vm_thread(remote_tx: std::sync::mpsc::Sender<Remote>) {
    // Trusted, but `exit` must not end the server.
    let mut vm = Vm::with_grants(Grants::ALL.without(Capability::HostControl));
    crate::install(&mut vm).expect("node library");
    // Futures tasks wait on, and stops (interrupts), wake the loop below
    // through a request of its own; set before the server is installed,
    // whose interrupt handle calls it too. A request posted wakes the
    // blocking wait by itself.
    let remote: Arc<std::sync::OnceLock<Remote>> = Arc::default();
    let woken = remote.clone();
    vm.set_wake_notifier(move || {
        if let Some(r) = woken.get() {
            r.wake();
        }
    });
    let server = Server::install(&mut vm, || {}).expect("nREPL helpers");
    let _ = remote.set(server.remote());
    let _ = remote_tx.send(server.remote());
    loop {
        server.poll(&mut vm);
        let progress = vm.run_tasks_for(Duration::from_millis(10));
        server.reap(&mut vm);
        if server.retired() {
            while server.wait(&mut vm, None) {}
            return;
        }
        if matches!(progress, Progress::OutOfTime) {
            continue;
        }
        // A request handled while waiting may give tasks to run: the loop
        // goes round to run them.
        let until = vm.next_timer();
        if !server.wait(&mut vm, until) {
            return;
        }
    }
}

fn session_id(shared: &Shared) -> String {
    let n = shared.next.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{:08x}-{:04x}-{:012x}", (t >> 32) as u32, n as u16, t as u64 & 0xffff_ffff_ffff)
}

/// The furthest line and column an `eval` may say its code starts at.
const MAX_LINE: i64 = 1_000_000;
const MAX_COLUMN: i64 = 10_000;

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
    "techne-debug-inspect",
    "techne-debug-subscribe",
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
                // Positions in the client's file, which the source is
                // padded to: bounded, so a message cannot make the server
                // allocate what it likes.
                let position = |k: &str, max: i64| msg.get(k).and_then(B::as_int).unwrap_or(1).clamp(1, max) as usize;
                (text("code"), text("file"), position("line", MAX_LINE), position("column", MAX_COLUMN))
            } else {
                (text("file"), text("file-path").or_else(|| text("file-name")), 1, 1)
            };
            let Some(source) = source else { return reply.error("eval needs code", &["no-code"]) };
            // Padding keeps line and column numbers of the client's file.
            let source = format!("{}{}{}", "\n".repeat(line - 1), " ".repeat(column - 1), source);
            // 1 (or "true"): pause on errors the code has restarts for;
            // 2 (or "always"): on every error.
            let debug = match msg.get("techne-debug") {
                Some(d) if d.as_int() == Some(2) || d.as_str() == Some("always") => 2,
                Some(d) if d.as_int() == Some(1) || d.as_str() == Some("true") => 1,
                _ => 0,
            };
            shared.post(Job::Eval { name: name.unwrap_or_else(|| "<nrepl>".into()), source, ns: text("ns"), debug, reply });
        }
        "interrupt" => {
            // The session's evaluation `interrupt-id` names, or its latest.
            let exec = {
                let running = shared.running.lock().unwrap();
                let mine: Vec<&Running> = running.values().filter(|r| r.session == session).collect();
                match msg.get("interrupt-id") {
                    Some(wanted) => match mine.iter().find(|r| r.id.as_ref() == Some(wanted)) {
                        Some(r) => Ok(r.exec.clone()),
                        None if mine.is_empty() => Err("session-idle"),
                        None => Err("interrupt-id-mismatch"),
                    },
                    None => mine.last().map(|r| r.exec.clone()).ok_or("session-idle"),
                }
            };
            match exec {
                // Stopped where it runs or waits, a debugger's wait too.
                Ok(exec) => {
                    if let Some((exec, interrupt)) = exec {
                        interrupt.stop(exec, Stop::Break);
                    }
                    reply.status(&["done"]);
                }
                Err("session-idle") => reply.status(&["done", "session-idle"]),
                Err(why) => reply.status(&["error", why, "done"]),
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
            let Some(restarts) = shared.debuggers.lock().unwrap().waiting.get(&id).map(|w| w.restarts) else {
                return reply.error(&format!("no debugger {id} is waiting"), &["no-debugger"]);
            };
            let cmd = if op == "techne-debug-abort" {
                DebugCmd::Abort
            } else {
                let index = msg.get("restart").and_then(B::as_int).unwrap_or(-1);
                match usize::try_from(index) {
                    Ok(index) if index < restarts => DebugCmd::Restart(index, text("args").unwrap_or_default()),
                    _ => return reply.error(&format!("no restart {index}"), &["no-restart"]),
                }
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
        "techne-debug-subscribe" => {
            let mut subscribers = shared.subscribers.lock().unwrap();
            subscribers.retain(|r| r.conn != reply.conn);
            subscribers.push(reply.clone());
            drop(subscribers);
            reply.status(&["techne-debug-subscribed"]);
        }
        "techne-debug-inspect" => match msg.get("debug-id").and_then(B::as_int) {
            Some(id) if id >= 0 => shared.post(Job::Inspect { action: Inspect::Condition(id as u64), reply }),
            _ => reply.error("techne-debug-inspect needs a debug-id", &[]),
        },
        _ => reply.status(&["error", "unknown-op", "done"]),
    }
}

/// The number of the next client connected, whichever way.
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

async fn connection(stream: impl AsyncRead + AsyncWrite + Send + 'static, shared: Arc<Shared>, conn: u64) {
    let (mut r, mut w) = tokio::io::split(stream);
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
    let orphaned: Vec<u64> = {
        let mut debuggers = shared.debuggers.lock().unwrap();
        debuggers.gone.insert(conn);
        // Holding its writer open.
        shared.subscribers.lock().unwrap().retain(|r| r.conn != conn);
        debuggers.waiting.iter().filter(|(_, w)| w.conn == conn).map(|(id, _)| *id).collect()
    };
    for id in orphaned {
        shared.post(Job::Debug { id, cmd: DebugCmd::Abort });
    }
    drop(out);
    let _ = writer.await;
}

/// Serve nREPL on `addr` (e.g. "127.0.0.1:7888"; port 0 picks one), with
/// a world of its own on a thread of its own. Calls `ready` with the bound
/// address, then serves until the process exits.
pub fn serve(addr: &str, ready: impl FnOnce(std::net::SocketAddr)) -> std::io::Result<()> {
    let (remote_tx, remote_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("techne-nrepl-vm".into()).spawn(move || vm_thread(remote_tx))?;
    let remote = remote_rx.recv().map_err(|_| std::io::Error::other("the VM did not start"))?;
    remote.serve(addr, ready)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(frame: &str) -> Vec<(String, String)> {
        let B::Dict(d) = frame_fields(frame) else { panic!() };
        d.into_iter()
            .map(|(k, v)| (String::from_utf8(k).unwrap(), v.as_str().map_or_else(|| format!("{:?}", v.as_int()), str::to_string)))
            .collect()
    }

    #[test]
    fn frames_split_into_their_parts() {
        let get = |frame: &str, key: &str| parts(frame).into_iter().find(|(k, _)| k == key).map(|(_, v)| v);
        let f = "risky (/src/a b.scm:12:3)";
        assert_eq!(
            (get(f, "name"), get(f, "file"), get(f, "line"), get(f, "column")),
            (Some("risky".into()), Some("/src/a b.scm".into()), Some("Some(12)".into()), Some("Some(3)".into()))
        );
        // Parentheses and colons in the file's name.
        let f = "f (/tmp/my (copy)/odd:name.scm:3:4)";
        assert_eq!(
            (get(f, "name"), get(f, "file"), get(f, "line")),
            (Some("f".into()), Some("/tmp/my (copy)/odd:name.scm".into()), Some("Some(3)".into()))
        );
        // A file without a position, and no location at all.
        assert_eq!(get("g (<prelude>)", "file"), Some("<prelude>".into()));
        assert_eq!(get("g (<prelude>)", "line"), None);
        assert_eq!((get("native", "name"), get("native", "text")), (None, Some("native".into())));
    }
}
