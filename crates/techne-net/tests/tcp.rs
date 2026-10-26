//! TCP from Lisp, on loopback: echo, many connections in tasks,
//! back-pressure and memory, end of stream against failures, cancellation,
//! closing, scopes, the capability and UTF-8 across reads. Most cases run on
//! both I/O runtimes, each under a deadline, so a lost wakeup fails the test
//! instead of hanging it.

use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

use techne_vm::{tasks::Progress, vm::Vm};

/// How long a test's evaluation may take.
const DEADLINE: Duration = Duration::from_secs(20);

/// Helpers the tests share: a server answering each connection with
/// `(f connection)`, a client's connection to a listener, and reading a
/// connection to its end.
const HELPERS: &str = r#"
(define (serve f)
  (let ((l (tcp-listen 0)))
    (spawn (lambda ()
             (let loop ()
               (let ((c (tcp-accept l)))
                 (spawn (lambda () (call-with-socket c f)))
                 (loop)))))
    l))
(define (port-of l) (cadr (tcp-local-address l)))
(define (connect l) (tcp-connect "127.0.0.1" (port-of l)))
;; A connection and its other end, through the listener L.
(define (pair-of l)
  (let ((client (spawn (lambda () (connect l)))))
    (let ((s (tcp-accept l))) (cons (task-join client) s))))
(define (read-all c)
  (let loop ((acc '()))
    (let ((s (tcp-read c)))
      (if (eof-object? s) (apply string-append (reverse acc)) (loop (cons s acc))))))
(define (echo c)
  (let loop ()
    (let ((b (tcp-read-bytes c)))
      (unless (eof-object? b) (tcp-write c b) (loop)))))
(define (report thunk) (guard (e (#t (condition/report-string e))) (thunk)))
;; Run THUNK in a task, and once it waits, stop the task with STOP; return
;; how the task ended.
(define (stopped stop thunk)
  (let ((t (spawn thunk)))
    (yield)
    (stop t)
    (report (lambda () (task-join t)))))
"#;

#[derive(Clone, Copy, Debug)]
enum Io {
    /// techne-net's shared I/O thread; the VM waits by parking.
    Shared,
    /// A current-thread runtime the test drives, as a host would.
    Inline,
}

const BOTH: [Io; 2] = [Io::Shared, Io::Inline];

struct Net {
    vm: Vm,
    io: Io,
    rt: tokio::runtime::Runtime,
    wake: Arc<tokio::sync::Notify>,
}

fn net(io: Io) -> Net {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut vm = Vm::new();
    match io {
        Io::Shared => techne_net::install(&mut vm),
        Io::Inline => techne_net::install_on(&mut vm, rt.handle().clone()),
    }
    .unwrap();
    let wake = Arc::new(tokio::sync::Notify::new());
    let notify = wake.clone();
    vm.set_wake_notifier(move || notify.notify_one());
    let mut net = Net { vm, io, rt, wake };
    net.define(HELPERS);
    net
}

impl Net {
    /// Evaluate `src`, which does not wait, at top level.
    fn define(&mut self, src: &str) {
        self.vm.eval_source(src).unwrap_or_else(|e| panic!("{src}\n{e}"));
    }

    /// Run `src`, a body (definitions, then expressions), in a task; return
    /// its value printed, or its error.
    fn run(&mut self, src: &str) -> Result<String, String> {
        let result = match self.io {
            Io::Shared => {
                // Parked in the scheduler; interrupted past the deadline.
                let handle = self.vm.interrupt_handle();
                let (done, watch) = mpsc::channel::<()>();
                std::thread::spawn(move || {
                    if let Err(mpsc::RecvTimeoutError::Timeout) = watch.recv_timeout(DEADLINE) {
                        handle.interrupt();
                    }
                });
                let result = self.vm.eval_source(&format!("(task-join (spawn (lambda () {src})))"));
                drop(done);
                result.map(|v| self.vm.repr(&v)).map_err(|e| e.to_string())
            }
            Io::Inline => {
                let main = self.vm.eval_source(&format!("(lambda () {src})")).unwrap_or_else(|e| panic!("{src}\n{e}"));
                let task = self.vm.spawn(&main);
                let (vm, wake) = (&mut self.vm, &self.wake);
                let driven = self.rt.block_on(async {
                    tokio::time::timeout(DEADLINE, async {
                        loop {
                            let progress = vm.run_tasks_for(Duration::from_millis(2)).unwrap();
                            if let Some(r) = vm.task_result(&task) {
                                return r.map(|v| vm.repr(&v)).map_err(|e| e.to_string());
                            }
                            match progress {
                                Progress::Finished => unreachable!("the task has no result"),
                                Progress::OutOfTime => tokio::task::yield_now().await,
                                Progress::Blocked => match vm.next_timer() {
                                    Some(t) => {
                                        let _ = tokio::time::timeout_at(t.into(), wake.notified()).await;
                                    }
                                    None => wake.notified().await,
                                },
                            }
                        }
                    })
                    .await
                });
                driven.unwrap_or_else(|_| panic!("{:?}: the evaluation passed its deadline:\n{src}", self.io))
            }
        };
        if result.as_ref().is_err_and(|e| e.contains("interrupted")) {
            panic!("{:?}: the evaluation passed its deadline:\n{src}", self.io);
        }
        result
    }

    fn eval(&mut self, src: &str) -> String {
        self.run(src).unwrap_or_else(|e| panic!("{:?}: {src}\n{e}", self.io))
    }
}

#[test]
fn echo() {
    for io in BOTH {
        let mut n = net(io);
        let out = n.eval(
            r#"(define l (serve echo))
               (call-with-tcp-connection "localhost" (port-of l)
                 (lambda (c)
                   (tcp-write c "hello, ")
                   (tcp-write c (string->utf8 "wörld"))
                   (tcp-close-output c)
                   (list (read-all c) (tcp-connection? c) (tcp-listener? l)
                         (equal? (tcp-peer-address c) (tcp-local-address l)))))"#,
        );
        assert_eq!(out, r#"("hello, wörld" #t #t #t)"#, "{io:?}");
        // Every byte value round-trips, over IPv6 too.
        let out = n.eval(
            r#"(define bytes (apply bytevector (iota 256)))
               (define l (tcp-listen 0 #:host "::1"))
               (spawn (lambda () (call-with-socket (tcp-accept l) echo)))
               (call-with-tcp-connection "::1" (port-of l)
                 (lambda (c)
                   (tcp-write c bytes)
                   (tcp-close-output c)
                   (let loop ((acc (bytevector)))
                     (let ((b (tcp-read-bytes c)))
                       (if (eof-object? b) (equal? acc bytes) (loop (bytevector-append acc b)))))))"#,
        );
        assert_eq!(out, "#t", "{io:?}");
    }
}

#[test]
fn many_connections_in_tasks() {
    for io in BOTH {
        // 200 clients at once, each its own task and message, against a
        // task per connection on the server.
        let out = net(io).eval(
            r#"(define l (serve echo))
               (define clients
                 (map (lambda (i)
                        (spawn (lambda ()
                                 (call-with-socket (connect l)
                                   (lambda (c)
                                     (let ((msg (string-append "client " (number->string i) (make-string 1000 #\x))))
                                       (tcp-write c msg)
                                       (tcp-close-output c)
                                       (string=? (read-all c) msg)))))))
                      (iota 200)))
               (list (length clients) (every task-join clients))"#,
        );
        assert_eq!(out, "(200 #t)", "{io:?}");
    }
}

#[test]
fn slow_readers_make_writers_wait() {
    for io in BOTH {
        let mut n = net(io);
        n.define("(define chunk (make-string 65536 #\\x)) (define written 0) (define peer #f) (define writer #f)");
        // What the world holds (other tests run in worlds of their own).
        let before = n.vm.held().total();
        // 64 MiB written to a peer that does not read yet: the writer waits
        // once the kernel's buffers are full, and nothing piles up here.
        let out = n.eval(
            r#"(define l (tcp-listen 0))
               (define p (pair-of l))
               (set! peer (cdr p))
               (set! writer
                 (spawn (lambda ()
                          (do ((i 0 (+ i 1))) ((= i 1024)) (tcp-write (car p) chunk) (set! written (+ written 1)))
                          (tcp-close-output (car p)))))
               (sleep 300)
               (list (task-done? writer) (< written 1024))"#,
        );
        assert_eq!(out, "(#f #t)", "{io:?}: the writer must wait for the reader");
        let grown = n.vm.held().total().saturating_sub(before);
        assert!(grown < 16 << 20, "{io:?}: unread data must not accumulate: grew {} KiB", grown >> 10);
        // Reading drains it all, in bounded chunks, and the writer finishes.
        let out = n.eval(
            r#"(let loop ((total 0) (largest 0))
                 (let ((s (tcp-read peer)))
                   (if (eof-object? s)
                       (list total (<= largest 32768) (begin (task-join writer) written))
                       (loop (+ total (string-length s)) (max largest (string-length s))))))"#,
        );
        assert_eq!(out, "(67108864 #t 1024)", "{io:?}");
    }
}

#[test]
fn waiting_writers_hold_no_copy_of_their_data() {
    for io in BOTH {
        let mut n = net(io);
        n.define("(define big (make-string (* 16 1024 1024) #\\x)) (define p #f) (define writers '())");
        n.eval("(set! p (pair-of (tcp-listen 0)))");
        n.define("(collect-garbage 'full)");
        // Room for 2 MiB more: ten writers of 16 MiB each, waiting for a
        // peer that does not read, fit, and do not end the program.
        let before = n.vm.held().total();
        n.vm.set_memory_limit(before + (2 << 20));
        let out = n.eval(
            r#"(set! writers (map (lambda (_) (spawn (lambda () (report (lambda () (tcp-write (car p) big)))))) (iota 10)))
               (sleep 100)
               (map task-done? writers)"#,
        );
        assert_eq!(out, format!("({})", ["#f"; 10].join(" ")), "{io:?}");
        let grown = n.vm.held().total().saturating_sub(before);
        assert!(grown < 1 << 20, "{io:?}: waiting writers grew the world by {} KiB", grown >> 10);
        let out = n.eval("(tcp-close (car p)) (delete-duplicates (map task-join writers))");
        assert_eq!(out, r#"("tcp-write: the connection is closed")"#, "{io:?}");
        // A host too long to be one is refused before it is copied.
        for src in [r#"(tcp-listen 0 #:host big)"#, r#"(tcp-connect big 80)"#] {
            let out = n.eval(&format!("(report (lambda () {src}))"));
            assert!(out.contains("a host name is at most 253 bytes, not 16777216"), "{io:?}: {out}");
        }
    }
}

#[test]
fn end_of_stream_is_not_a_failure() {
    for io in BOTH {
        let mut n = net(io);
        // The peer closing is the eof object, every time it is read.
        let out = n.eval(
            r#"(define l (serve (lambda (c) (tcp-write c "bye"))))
               (call-with-socket (connect l)
                 (lambda (c) (list (read-all c) (tcp-read c) (tcp-read-bytes c))))"#,
        );
        assert_eq!(out, r#"("bye" #<eof> #<eof>)"#, "{io:?}");
        // A peer closing with data it has not read resets the connection.
        let out = n.eval(
            r#"(define l (tcp-listen 0))
               (define p (pair-of l))
               (tcp-write (car p) "unread")
               (tcp-close (cdr p))
               (report (lambda () (tcp-read (car p))))"#,
        );
        assert_eq!(out, r#""tcp-read: connection reset by peer""#, "{io:?}");
        // Nothing listening: refused. The port stays bound meanwhile, so
        // no one else listens on it.
        let bound = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        bound.bind(&std::net::SocketAddr::from(([127, 0, 0, 1], 0)).into()).unwrap();
        let port = bound.local_addr().unwrap().as_socket().unwrap().port();
        let out = n.eval(&format!(r#"(report (lambda () (tcp-connect "127.0.0.1" {port})))"#));
        assert_eq!(out, format!(r#""tcp-connect: 127.0.0.1:{port}: connection refused""#), "{io:?}");
        let out = n.eval(r#"(report (lambda () (tcp-connect "no-such-host.invalid" 80)))"#);
        assert!(out.starts_with(r#""tcp-connect: no-such-host.invalid:80: "#), "{io:?}: {out}");
        let out = n.eval(r#"(report (lambda () (tcp-listen 80 #:host "nowhere")))"#);
        assert_eq!(out, r#""tcp-listen: nowhere:80: expected an IP address or localhost""#, "{io:?}");
    }
}

#[test]
fn cancelling_or_killing_a_waiting_task_drops_its_wait() {
    for io in BOTH {
        let out = net(io).eval(
            r#"(define l (tcp-listen 0))
               (define accepting (stopped task-cancel (lambda () (tcp-accept l))))
               (define accept-killed (stopped task-kill (lambda () (tcp-accept l))))
               ;; The listener still works.
               (define p (pair-of l))
               (define c (car p))
               (define s (cdr p))
               (define reading (stopped task-cancel (lambda () (tcp-read s))))
               (define read-killed (stopped task-kill (lambda () (tcp-read s))))
               ;; Writers waiting for a peer that does not read.
               (define big (make-string (* 64 1024 1024) #\x))
               (define writing (stopped task-cancel (lambda () (tcp-write c big))))
               (define write-killed (stopped task-kill (lambda () (tcp-write c big))))
               ;; A connection not yet made: the listener's queue is full
               ;; (one connection, with a backlog of 0).
               (define full (tcp-listen 0 #:backlog 0))
               (define queued (connect full))
               (define pending (spawn (lambda () (connect full))))
               (sleep 100)
               (define was-pending (not (task-done? pending)))
               (task-cancel pending)
               (define connecting (report (lambda () (task-join pending))))
               (define connect-killed (stopped task-kill (lambda () (connect full))))
               ;; The connection still works: what the stopped writes sent is
               ;; there, then what comes next.
               (define got-more (spawn (lambda () (tcp-write c "end") (tcp-close c))))
               (define got (let loop ((n 0)) (let ((b (tcp-read-bytes s))) (if (eof-object? b) n (loop (+ n (bytevector-length b)))))))
               (task-join got-more)
               (list accepting accept-killed reading read-killed writing write-killed
                     was-pending connecting connect-killed (< 3 got (* 2 (string-length big))))"#,
        );
        assert_eq!(
            out,
            r#"("task cancelled" "task killed" "task cancelled" "task killed" "task cancelled" "task killed" #t "task cancelled" "task killed" #t)"#,
            "{io:?}"
        );
    }
}

#[test]
fn closing_wakes_every_waiting_task() {
    for io in BOTH {
        let out = net(io).eval(
            r#"(define l (tcp-listen 0))
               (define p (pair-of l))
               (define (waiting thunk) (map (lambda (_) (spawn (lambda () (report thunk)))) (iota 3)))
               (define accepting (waiting (lambda () (tcp-accept l))))
               (define reading (waiting (lambda () (tcp-read (cdr p)))))
               (define writing (waiting (lambda () (tcp-write (cdr p) (make-string (* 16 1024 1024) #\x)))))
               (yield)
               (tcp-close l)
               (tcp-close (cdr p))
               (map (lambda (ts) (delete-duplicates (map task-join ts))) (list accepting reading writing))"#,
        );
        assert_eq!(
            out,
            r#"(("tcp-accept: the listener is closed") ("tcp-read: the connection is closed") ("tcp-write: the connection is closed"))"#,
            "{io:?}"
        );
        // Closed is closed, whatever a text read left undecoded.
        let out = net(io).eval(
            r#"(define p (pair-of (tcp-listen 0)))
               (tcp-write (car p) (bytevector #xe2))
               (define reader (spawn (lambda () (tcp-read (cdr p)))))
               (sleep 50)
               (task-cancel reader)
               (tcp-close (cdr p))
               (list (tcp-closed? (cdr p)) (report (lambda () (tcp-read-bytes (cdr p)))) (report (lambda () (tcp-write (cdr p) "x"))))"#,
        );
        assert_eq!(out, r#"(#t "tcp-read-bytes: the connection is closed" "tcp-write: the connection is closed")"#, "{io:?}");
    }
}

#[test]
fn shutting_a_scope_closes_its_sockets() {
    for io in BOTH {
        let mut n = net(io);
        // A sample service, started and shut a hundred times: its listener
        // and connections are closed, its tasks gone, and the peers see the
        // end.
        let out = n.eval(
            r#"(define l (serve read-all))
               (define base (%live-task-count))
               (define sockets
                 (append-map (lambda (i)
                               (let ((s (make-scope 'service)))
                                 (let ((owned (with-scope s
                                                (let* ((own (tcp-listen 0)) (c (connect l)))
                                                  (spawn (lambda () (tcp-read c)))
                                                  (list own c)))))
                                   (scope-shutdown! s)
                                   owned)))
                             (iota 100)))
               ;; The server's tasks for the connections end as they close.
               (let wait ((n 0))
                 (unless (or (= base (%live-task-count)) (= n 100)) (sleep 10) (wait (+ n 1))))
               (list (length sockets) (every tcp-closed? sockets) (= base (%live-task-count)))"#,
        );
        assert_eq!(out, "(200 #t #t)", "{io:?}");
        // A task outside the scope, waiting on a connection the scope owned,
        // fails when the scope is shut.
        let out = n.eval(
            r#"(define l (serve read-all))
               (define s (make-scope 'service))
               (define c (with-scope s (connect l)))
               (define reader (spawn (lambda () (report (lambda () (tcp-read c))))))
               (yield)
               (scope-shutdown! s)
               (task-join reader)"#,
        );
        assert_eq!(out, r#""tcp-read: the connection is closed""#, "{io:?}");
    }
}

#[test]
fn collected_sockets_close() {
    for io in BOTH {
        // A connection no scope owns closes when it is garbage: its peer
        // reads the end of the stream.
        let out = net(io).eval(
            r#"(define l (tcp-listen 0))
               (define peer #f)
               (define accepting (spawn (lambda () (set! peer (tcp-accept l)))))
               ;; Made in a task that ends, so nothing it ran keeps it.
               (task-join (spawn (lambda () (scope-disown! (connect l)) #t)))
               (task-join accepting)
               (collect-garbage 'full)
               (tcp-read peer)"#,
        );
        assert_eq!(out, "#<eof>", "{io:?}");
    }
}

#[test]
fn utf8_across_reads() {
    for io in BOTH {
        let mut n = net(io);
        // "λ€" sent a byte at a time: however the bytes arrive, every read
        // is whole characters, and together they are the text.
        let out = n.eval(
            r#"(define l (serve (lambda (c)
                                  (let ((bytes (string->utf8 "λ€")))
                                    (do ((i 0 (+ i 1))) ((= i (bytevector-length bytes)))
                                      (tcp-write c (bytevector (bytevector-u8-ref bytes i)))
                                      (sleep 10))))))
               (define reads
                 (call-with-socket (connect l)
                   (lambda (c)
                     (let loop ((reads '()))
                       (let ((s (tcp-read c)))
                         (if (eof-object? s) (reverse reads) (loop (cons s reads))))))))
               (list (apply string-append reads) (every (lambda (s) (not (string-index s #\xfffd))) reads))"#,
        );
        assert_eq!(out, r#"("λ€" #t)"#, "{io:?}");
        // A byte that starts no character is not held back for more: the
        // read returns it at once, as U+FFFD.
        let out = n.eval(
            r#"(define p (pair-of (tcp-listen 0)))
               (tcp-write (car p) (bytevector #xff))
               (tcp-read (cdr p))"#,
        );
        assert_eq!(out, "\"\u{fffd}\"", "{io:?}");
        // An incomplete character at the end reads as U+FFFD.
        let out = n.eval(
            r#"(define l (serve (lambda (c) (tcp-write c (bytevector 97 #xe2 #x82)))))
               (call-with-socket (connect l) read-all)"#,
        );
        assert_eq!(out, "\"a\u{fffd}\"", "{io:?}");
    }
}

#[test]
fn the_network_capability_is_needed() {
    use techne_vm::vm::{Capability, Grants};
    let mut vm = Vm::with_grants(Grants::ALL.without(Capability::Network));
    techne_net::install(&mut vm).unwrap();
    for src in ["(tcp-listen 0)", r#"(tcp-connect "127.0.0.1" 80)"#, r#"(call-with-tcp-connection "127.0.0.1" 80 tcp-read)"#] {
        let err = vm.eval_source(src).unwrap_err();
        assert!(err.msg.contains("not granted in this world (needs network)"), "{src}: {err}");
    }
}

#[test]
fn run_tasks_drives_a_host_runtime() {
    // `techne_net::run_tasks` runs every task to its end on a runtime the
    // host drives.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut vm = Vm::new();
    techne_net::install_on(&mut vm, rt.handle().clone()).unwrap();
    vm.eval_source(HELPERS).unwrap();
    let main = vm
        .eval_source(
            r#"(lambda ()
                 (let* ((l (serve echo))
                        (answers (map (lambda (i)
                                        (spawn (lambda ()
                                                 (call-with-socket (connect l)
                                                   (lambda (c) (tcp-write c (number->string i)) (tcp-close-output c) (read-all c))))))
                                      (iota 50)))
                        (result (map task-join answers)))
                   ;; The accept loop fails once its listener is closed, and ends.
                   (tcp-close l)
                   result))"#,
        )
        .unwrap();
    let task = vm.spawn(&main);
    let result = rt.block_on(async { tokio::time::timeout(DEADLINE, techne_net::run_tasks(&mut vm)).await });
    result.expect("the tasks did not finish").unwrap();
    let answers = vm.task_result(&task).unwrap().unwrap();
    let expected: Vec<String> = (0..50).map(|i| format!("\"{i}\"")).collect();
    assert_eq!(vm.repr(&answers), format!("({})", expected.join(" ")));
}

#[test]
fn procedures_are_documented() {
    let mut n = net(Io::Shared);
    let out = n.eval(
        r#"(filter-map
             (lambda (name)
               (let* ((d (binding-description name))
                      (doc (cdr (assq 'doc d)))
                      (params (cdr (assq 'params d)))
                      (problems (if (string? doc) (docstring-problems doc 'procedure params) '("no docstring"))))
                 (and (pair? problems) (cons name problems))))
             '(tcp-listen tcp-accept tcp-connect call-with-socket call-with-tcp-connection tcp-read
               tcp-read-bytes tcp-write tcp-close-output tcp-close tcp-closed? tcp-local-address
               tcp-peer-address tcp-connection? tcp-listener?))"#,
    );
    assert_eq!(out, "()");
}
