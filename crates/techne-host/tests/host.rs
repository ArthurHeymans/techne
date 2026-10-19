//! A front world asking deployments for responses through a host: what
//! instances keep between requests, what is answered for failures, what
//! an instance cannot reach, and deploying anew.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use techne_host::{Deployment, Failure, Host, Limits};
use techne_vm::vm::Vm;

/// A deployment of SOURCE, its entry module, in DIR.
fn deployment(dir: &Path, name: &str, source: &str, limits: Limits) -> Result<Deployment, String> {
    let entry: PathBuf = dir.join(format!("{name}.scm"));
    std::fs::write(&entry, source).unwrap();
    Deployment::load(&entry, limits).map_err(|e| e.msg.clone())
}

/// A handler counting the requests its instance served, and doing what
/// the path says.
const APP: &str = r#"
(provide fetch)
(define served 0)
(define (fetch request env ctx)
  (set! served (+ served 1))
  (let ((path (dict-ref request 'path)))
    (cond ((string=? path "/fail") (error "it broke"))
          ((string=? path "/spin") (let loop () (loop)))
          ((string=? path "/stall") (eval '(define-library (stall) (import (scheme base)) (begin (let loop () (loop))))))
          ((string=? path "/sleep") (sleep 100000))
          ((string=? path "/slow") (sleep 100) (number->string served))
          ((string=? path "/proc") (dict 'body (lambda (w) (w "x") (w (bytevector 121)))))
          ((string=? path "/nodata") (lambda () 1))
          ((string=? path "/stream-fail") (dict 'body (lambda (w) (w "x") (error "mid-stream"))))
          ((string=? path "/echo")
           (let ((body (dict-ref request 'body)))
             (dict 'body (lambda (w)
                           (let loop ()
                             (let ((b (body)))
                               (unless (eof-object? b) (w b) (loop))))))))
          ((string=? path "/file") (open-input-file "/etc/hostname"))
          ((string=? path "/load") (eval '(require "/etc/hostname")))
          ((string=? path "/leave") (spawn (lambda () (sleep 100000))) "left")
          ((string=? path "/stdin") (if (eof-object? (read-line)) "empty" "read"))
          (else (dict 'status 200 'body (number->string served))))))
"#;

/// A front world asking the host for each of `requests`, (deployment
/// path), at once; the responses, written as (status body), in their
/// order.
fn ask(host: &mut Host, requests: &[(&str, &str)]) -> Vec<String> {
    let mut front = Vm::new();
    host.install(&mut front);
    front
        .eval_source(
            "(define (ask name path)
               (spawn (lambda ()
                        ;; /echo is sent a body that streams.
                        (let* ((n 0)
                               (body (lambda ()
                                       (set! n (+ n 1))
                                       (if (> n 3) (eof-object) (string->utf8 \"ab\"))))
                               (r (host-fetch name (dict 'method \"GET\" 'path path 'body (if (string=? path \"/echo\") body #u8())))))
                          (if (string? r)
                              (list 200 r)
                              (list (dict-ref r 'status 200) (read-body (dict-ref r 'body))))))))
             ;; A body that streams, read whole: \"|cut\" after it if it
             ;; failed.
             (define (read-body body)
               (if (procedure? body)
                   (let ((out \"\"))
                     (guard (e (#t (string-append out \"|cut\")))
                       (body (lambda (b) (set! out (string-append out (utf8->string b)))))
                       out))
                   body))",
        )
        .unwrap();
    let list: String = requests.iter().map(|(d, p)| format!("(ask {d:?} {p:?}) ")).collect();
    front.eval_source(&format!("(define tasks (list {list}))")).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(host.run(Some(&mut front)));
    let v = front.eval_source("(map task-join tasks)").unwrap();
    let items = front.eval_source("(lambda (l) (map (lambda (x) (call-with-output-string (lambda (p) (write x p)))) l))").unwrap();
    let written = front.call(&items, &[v]).unwrap();
    front.get::<Vec<String>>(&written).unwrap()
}

fn host_with_failures() -> (Host, Rc<RefCell<Vec<Failure>>>) {
    let host = Host::new();
    let failures = Rc::new(RefCell::new(Vec::new()));
    let seen = failures.clone();
    host.on_failure(move |f| seen.borrow_mut().push(f.clone()));
    (host, failures)
}

#[test]
fn instances_are_kept_between_requests() {
    let dir = tempfile::tempdir().unwrap();
    let (mut host, _) = host_with_failures();
    host.publish("app", deployment(dir.path(), "app", APP, Limits::default()).unwrap());
    // One after another, on the instance kept.
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"1\")"]);
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"2\")"]);
    // At once: an instance each, the one kept and a new one.
    let mut both = ask(&mut host, &[("app", "/slow"), ("app", "/slow")]);
    both.sort();
    assert_eq!(both, ["(200 \"1\")", "(200 \"3\")"]);
}

#[test]
fn requests_wait_for_a_place() {
    let dir = tempfile::tempdir().unwrap();
    let (mut host, _) = host_with_failures();
    let limits = Limits { concurrency: 1, ..Limits::default() };
    host.publish("app", deployment(dir.path(), "app", APP, limits).unwrap());
    let start = std::time::Instant::now();
    assert_eq!(ask(&mut host, &[("app", "/slow"), ("app", "/slow")]), ["(200 \"1\")", "(200 \"2\")"]);
    assert!(start.elapsed() >= Duration::from_millis(200));
}

#[test]
fn failures_are_answered() {
    let dir = tempfile::tempdir().unwrap();
    let (mut host, failures) = host_with_failures();
    let limits = Limits { wall: Duration::from_millis(300), ..Limits::default() };
    host.publish("app", deployment(dir.path(), "app", APP, limits).unwrap());
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"1\")"]);
    // An uncaught condition: answered 500, the instance dropped (its
    // state may be half changed), as after any failure.
    assert_eq!(ask(&mut host, &[("app", "/fail")]), ["(500 \"Internal Server Error\")"]);
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"1\")"]);
    // A request leaving a task behind: answered, the instance dropped.
    assert_eq!(ask(&mut host, &[("app", "/leave")]), ["(200 \"left\")"]);
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"1\")"]);
    // Past its CPU bound, or its deadline: killed, and its instance gone.
    assert_eq!(ask(&mut host, &[("app", "/spin")]), ["(500 \"Internal Server Error\")"]);
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"1\")"]);
    // Looping where its task cannot be preempted: stopped all the same.
    assert_eq!(ask(&mut host, &[("app", "/stall")]), ["(500 \"Internal Server Error\")"]);
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"1\")"]);
    assert_eq!(ask(&mut host, &[("app", "/sleep")]), ["(504 \"Gateway Timeout\")"]);
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"1\")"]);
    // What is no data, and no deployment.
    assert_eq!(ask(&mut host, &[("app", "/nodata")]), ["(500 \"Internal Server Error\")"]);
    // A body that streams, failing after its first chunk: cut short.
    assert_eq!(ask(&mut host, &[("app", "/stream-fail")]), ["(200 \"x|cut\")"]);
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"1\")"]);
    assert_eq!(ask(&mut host, &[("nothing", "/")]), ["(404 \"Not Found\")"]);
    let seen: Vec<(u16, String)> = failures.borrow().iter().map(|f| (f.status, f.message.clone())).collect();
    let says = |status: u16, what: &str| seen.iter().any(|(s, m)| *s == status && m.contains(what));
    assert!(says(500, "it broke"), "{seen:?}");
    assert!(says(500, "CPU bound"), "{seen:?}");
    assert!(says(504, "deadline"), "{seen:?}");
    assert!(says(500, "no data"), "{seen:?}");
    assert!(says(500, "mid-stream"), "{seen:?}");
    assert!(says(404, "no such deployment"), "{seen:?}");
}

/// An instance has no files, loads no modules, reads no console.
#[test]
fn instances_reach_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut host, failures) = host_with_failures();
    host.publish("app", deployment(dir.path(), "app", APP, Limits::default()).unwrap());
    assert_eq!(ask(&mut host, &[("app", "/stdin")]), ["(200 \"empty\")"]);
    assert_eq!(ask(&mut host, &[("app", "/file"), ("app", "/load")]), ["(500 \"Internal Server Error\")"; 2]);
    let seen: Vec<String> = failures.borrow().iter().map(|f| f.message.clone()).collect();
    assert!(seen.iter().any(|m| m.contains("open-input-file: not granted")), "{seen:?}");
    assert!(seen.iter().any(|m| m.contains("loading modules is not granted")), "{seen:?}");
}

#[test]
fn deployments_load_or_say_why() {
    let dir = tempfile::tempdir().unwrap();
    // What its entry requires loads with it.
    std::fs::write(dir.path().join("lib.scm"), "(provide greeting) (define greeting \"hello\")").unwrap();
    let uses = "(require \"lib.scm\") (provide fetch) (define (fetch r e c) greeting)";
    let (mut host, _) = host_with_failures();
    host.publish("app", deployment(dir.path(), "uses", uses, Limits::default()).unwrap());
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"hello\")"]);
    // Publishing anew: the next request is the new deployment's.
    host.publish("app", deployment(dir.path(), "app", APP, Limits::default()).unwrap());
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"1\")"]);

    let none = deployment(dir.path(), "none", "(define x 1)", Limits::default()).err().unwrap();
    assert!(none.contains("provides no fetch"), "{none}");
    let busy =
        deployment(dir.path(), "busy", "(provide fetch) (define (fetch r e c) 1) (spawn (lambda () (sleep 1000)))", Limits::default());
    let busy = busy.err().unwrap();
    // Killed as loading ends, its task is still held: refused.
    assert!(busy.contains("task"), "{busy}");
    let channel =
        deployment(dir.path(), "channel", "(provide fetch) (define (fetch r e c) 1) (define c (make-channel))", Limits::default());
    let channel = channel.err().unwrap();
    assert!(channel.contains("channels were made"), "{channel}");
    let files =
        deployment(dir.path(), "files", "(provide fetch) (define (fetch r e c) 1) (open-input-file \"/etc/hostname\")", Limits::default());
    assert!(files.err().unwrap().contains("not granted"));
    // Only its own directory is read.
    std::fs::write(dir.path().join("part.scm"), "(define part \"inside\")").unwrap();
    let inside = "(include \"part.scm\") (provide fetch) (define (fetch r e c) part)";
    host.publish("app", deployment(dir.path(), "inside", inside, Limits::default()).unwrap());
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"inside\")"]);
    let outside =
        deployment(dir.path(), "outside", "(include \"/etc/hostname\") (provide fetch) (define (fetch r e c) 1)", Limits::default());
    let outside = outside.err().unwrap();
    assert!(outside.contains("/etc/hostname: entity not found"), "{outside}");
    let up = deployment(dir.path(), "up", "(require \"../up.scm\") (provide fetch) (define (fetch r e c) 1)", Limits::default());
    assert!(up.is_err());
    // Loading within its time.
    let quick = Limits { load: Duration::from_millis(200), ..Limits::default() };
    let spins = deployment(dir.path(), "spins", "(let loop () (loop)) (provide fetch) (define (fetch r e c) 1)", quick.clone());
    assert!(spins.err().unwrap().contains("too long to load"));
    let stalls = "(define-library (stall) (import (scheme base)) (begin (let loop () (loop)))) (provide fetch) (define (fetch r e c) 1)";
    let stalls = deployment(dir.path(), "stalls", stalls, quick.clone());
    assert!(stalls.err().unwrap().contains("too long to load"));
    // Loading has no clock, so no timers.
    let sleeps = deployment(dir.path(), "sleeps", "(sleep 100000) (provide fetch) (define (fetch r e c) 1)", quick);
    assert!(sleeps.err().unwrap().contains("needs clock"));
    let none = deployment(dir.path(), "app", APP, Limits { concurrency: 0, ..Limits::default() });
    assert!(none.err().unwrap().contains("one request at once"));
}

/// What a deployment defines cannot change how the host calls it: here a
/// `lambda` of its own, which the host's code would have used.
#[test]
fn the_host_calls_fetch_itself() {
    let dir = tempfile::tempdir().unwrap();
    let source = r#"(provide fetch lambda %host-serve)
                    (define (fetch request env ctx) (string-append "fetch " (number->string (dict-ref ctx 'id))))
                    (define (%host-serve . _) "hijacked")
                    (define-syntax lambda (syntax-rules () ((_ . _) (lambda () "hijacked"))))"#;
    let (mut host, _) = host_with_failures();
    host.publish("app", deployment(dir.path(), "app", source, Limits::default()).unwrap());
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"fetch 1\")"]);
}

/// A front task cancelled while it waits: its request is killed, and its
/// instance dropped, unreported.
#[test]
fn an_abandoned_request_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let (mut host, failures) = host_with_failures();
    host.publish("app", deployment(dir.path(), "app", APP, Limits::default()).unwrap());
    let mut front = Vm::new();
    host.install(&mut front);
    front
        .eval_source(
            "(define t (spawn (lambda () (host-fetch \"app\" (dict 'path \"/sleep\")))))
             (spawn (lambda () (sleep 50) (task-cancel t)))",
        )
        .unwrap();
    let start = std::time::Instant::now();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(host.run(Some(&mut front)));
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(failures.borrow().is_empty(), "{:?}", failures.borrow());
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"1\")"]);
}

/// What a deployment writes to its standard output and error, however it
/// names them, is captured: a request's goes to the output hook with the
/// request, cut short past the limit; loading's stays with the deployment.
#[test]
fn output_is_captured() {
    let dir = tempfile::tempdir().unwrap();
    let source = r#"
(provide fetch)
(display "loaded")
(define (fetch request env ctx)
  (let ((path (dict-ref request 'path)))
    (display "out ")
    (display "err " (current-error-port))
    (write-string "raw " (%stdout))
    (cond ((string=? path "/long") (display (make-string 100 #\é)) (display "after") "long")
          ((string=? path "/fail") (error "it broke"))
          (else "ok"))))
"#;
    let limits = Limits { output: 63, ..Limits::default() };
    let app = deployment(dir.path(), "app", source, limits).unwrap();
    assert_eq!(app.output().text, "loaded");
    let (mut host, _) = host_with_failures();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let log = seen.clone();
    host.on_output(move |p| log.borrow_mut().push((p.deployment.clone(), p.request, p.output.text.clone(), p.output.truncated)));
    host.publish("app", app);
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"ok\")"]);
    assert_eq!(ask(&mut host, &[("app", "/fail")]), ["(500 \"Internal Server Error\")"]);
    assert_eq!(ask(&mut host, &[("app", "/long")]), ["(200 \"long\")"]);
    let seen = seen.borrow();
    assert_eq!(seen[0], ("app".into(), 1, "out err raw ".into(), false));
    assert_eq!(seen[1], ("app".into(), 2, "out err raw ".into(), false));
    // Cut at a character's boundary, 12 bytes and 25 two-byte characters,
    // and nothing after: what is kept is a prefix of what was written.
    let (_, id, text, truncated) = &seen[2];
    assert_eq!((*id, text.len(), *truncated), (3, 62, true));
    assert_eq!(text.as_str(), format!("out err raw {}", "é".repeat(25)));
}

/// A deployment, confined to its own files, imports the libraries Techne
/// ships: `(techne http)` to answer, `(techne crypto)` to hash.
#[test]
fn deployments_import_techne_libraries() {
    let dir = tempfile::tempdir().unwrap();
    let source = r#"
(import (techne http) (techne crypto))
(provide fetch)
(define (fetch request env ctx)
  (json-response (dict "sha" (hex-encode (sha-256 (dict-ref request 'path)))) #:status 201))
"#;
    let (mut host, failures) = host_with_failures();
    host.publish("app", deployment(dir.path(), "app", source, Limits::default()).unwrap());
    assert_eq!(
        ask(&mut host, &[("app", "abc")]),
        [r#"(201 "{\"sha\":\"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\"}")"#]
    );
    assert!(failures.borrow().is_empty());
}

/// What a test attaches to a dev world: it evaluates there the sources
/// pushed to it, as a REPL would, and counts how often it is polled.
struct Repl {
    /// The module evaluated in, and the source.
    sources: Rc<RefCell<Vec<(String, String)>>>,
    polls: Rc<RefCell<usize>>,
}

impl techne_host::Attached for Repl {
    fn poll(&mut self, vm: &mut Vm) -> bool {
        *self.polls.borrow_mut() += 1;
        for (module, src) in self.sources.borrow_mut().drain(..) {
            let module = vm.loaded_module(&module).unwrap();
            // A failure, as a REPL's, is no failure of the world's.
            let _ = vm.eval_in(module, "<repl>", &src);
        }
        false
    }

    fn reap(&mut self, _: &mut Vm) {}
}

/// A deployment published for development is served by one world, kept
/// after a request that fails and driven between requests: what is
/// attached to it redefines `fetch`, and the next request sees it.
#[test]
fn a_dev_world_is_kept_and_redefined() {
    let dir = tempfile::tempdir().unwrap();
    let (mut host, failures) = host_with_failures();
    let sources = Rc::new(RefCell::new(Vec::new()));
    let polls = Rc::new(RefCell::new(0));
    let repl = Repl { sources: sources.clone(), polls: polls.clone() };
    let app = deployment(dir.path(), "app", APP, Limits::default()).unwrap();
    host.publish_dev("app", app, move |_| Ok(Box::new(repl))).unwrap();
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"1\")"]);
    // Failed, and the world kept: its count goes on.
    assert_eq!(ask(&mut host, &[("app", "/fail")]), ["(500 \"Internal Server Error\")"]);
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"3\")"]);
    assert_eq!(failures.borrow().len(), 1);
    // Redefined between requests: the next one runs the new `fetch`.
    // In the deployment's module, as a REPL evaluates in the file's.
    let module = dir.path().join("app.scm").canonicalize().unwrap().display().to_string();
    sources.borrow_mut().push((module.clone(), "(define (fetch request env ctx) (string-append \"new \" (number->string served)))".into()));
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"new 3\")"]);
    assert!(*polls.borrow() > 0);
    // One at a time: the second waits for the first.
    assert_eq!(ask(&mut host, &[("app", "/"), ("app", "/")]), ["(200 \"new 3\")", "(200 \"new 3\")"]);
    // No `fetch` to call: refused, and the world kept to put one back.
    sources.borrow_mut().push((module.clone(), "(define fetch #f)".into()));
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(500 \"Internal Server Error\")"]);
    sources.borrow_mut().push((module, "(define (fetch request env ctx) \"back\")".into()));
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"back\")"]);
}

/// A dev world poisoned (a native panicked) is dropped: nothing runs in it
/// any more, and its timers keep no host awake; requests are refused until
/// the deployment is deployed again.
#[test]
fn a_poisoned_dev_world_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (mut host, _) = host_with_failures();
    let sources = Rc::new(RefCell::new(Vec::new()));
    let polls = Rc::new(RefCell::new(0));
    let repl = Repl { sources: sources.clone(), polls: polls.clone() };
    let app = deployment(dir.path(), "app", APP, Limits::default()).unwrap();
    host.publish_dev("app", app, move |vm| {
        vm.register_fn("boom", |x: i64| -> i64 {
            if x < 0 {
                panic!("boom");
            }
            x
        });
        Ok(Box::new(repl))
    })
    .unwrap();
    let module = dir.path().join("app.scm").canonicalize().unwrap().display().to_string();
    // A task sleeping, then the panic.
    sources.borrow_mut().push((module.clone(), "(spawn (lambda () (sleep 20)))".into()));
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(200 \"1\")"]);
    sources.borrow_mut().push((module, "(boom -1)".into()));
    let before = *polls.borrow();
    let mut front = Vm::new();
    host.install(&mut front);
    front.eval_source("(spawn (lambda () (sleep 300)))").unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(host.run(Some(&mut front)));
    assert!(*polls.borrow() - before < 10, "polled {} times", *polls.borrow() - before);
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(500 \"Internal Server Error\")"]);
}

/// A dev world's failing request calls `%host-debugger` with its condition
/// where it was raised, with the host's restarts there: `use-value`
/// answers it, `retry` calls `fetch` anew; a debugger declining (#f) lets
/// the request fail.
#[test]
fn a_dev_world_debugger_chooses_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (mut host, _) = host_with_failures();
    let repl = Repl { sources: Rc::default(), polls: Rc::default() };
    let app = deployment(dir.path(), "app", APP, Limits::default()).unwrap();
    host.publish_dev("app", app, move |vm| {
        // First `fetch` redefined (to fail otherwise) and retried, then
        // answered with a value, then declined.
        vm.eval_in(
            0,
            "<test>",
            "(define tries 0)
             (set! %host-debugger
               (lambda (c)
                 (set! tries (+ tries 1))
                 (case tries
                   ((1) (eval '(define (fetch r e c) (error \"again\")) \"user\")
                        (invoke-restart 'retry))
                   ((2) (and (equal? (error-object-message c) \"again\")
                             (invoke-restart 'use-value \"used\")))
                   (else #f))))",
        )
        .unwrap();
        Ok(Box::new(repl))
    })
    .unwrap();
    // Retried with the new `fetch`, which failed too, then answered.
    assert_eq!(ask(&mut host, &[("app", "/fail")]), ["(200 \"used\")"]);
    assert_eq!(ask(&mut host, &[("app", "/")]), ["(500 \"Internal Server Error\")"]);
}

#[test]
fn bodies_stream() {
    let dir = tempfile::tempdir().unwrap();
    let (mut host, failures) = host_with_failures();
    host.publish("app", deployment(dir.path(), "app", APP, Limits::default()).unwrap());
    assert_eq!(ask(&mut host, &[("app", "/proc"), ("app", "/echo")]), ["(200 \"xy\")", "(200 \"ababab\")"]);
    assert!(failures.borrow().is_empty(), "{:?}", failures.borrow());
}
