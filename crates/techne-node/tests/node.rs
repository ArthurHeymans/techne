//! The node contract: evaluation and inspection in another process, remote
//! processes through the ordinary process procedures, transport loss and
//! cleanup.

use std::time::{Duration, Instant};

use techne_vm::vm::Vm;

fn vm() -> Vm {
    let mut vm = Vm::new();
    techne_node::install(&mut vm).unwrap();
    let node = env!("CARGO_BIN_EXE_techne-node");
    vm.eval_source(&format!("(define n (node-connect (list {node:?})))")).unwrap();
    vm
}

fn eval(vm: &mut Vm, src: &str) -> String {
    match vm.eval_source(src) {
        Ok(v) => vm.repr(&v),
        Err(e) => panic!("{src}\n{e}"),
    }
}

fn gone(pid: i64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::path::Path::new(&format!("/proc/{pid}")).exists() {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

#[test]
fn evaluation_crosses_processes() {
    let mut vm = vm();
    assert_eq!(eval(&mut vm, r#"(node-eval n "(+ 1 2)")"#), "3");
    // Data round-trips; definitions persist on the node.
    assert_eq!(
        eval(&mut vm, r#"(node-eval n "(define (f x) \"Double X.\" (* x 2)) (list (f 21) \"s\" 'sym #\\c 1.5 (vector 1 #t) '())")"#),
        r#"(42 "s" sym #\c 1.5 #(1 #t) ())"#
    );
    // What the evaluation printed is printed here.
    assert_eq!(eval(&mut vm, r#"(with-output-to-string (lambda () (node-eval n "(display \"hi\") (newline)")))"#), "\"hi\\n\"");
    // Errors arrive as conditions; non-data values are refused.
    assert_eq!(eval(&mut vm, r#"(guard (e (#t (condition/report-string e))) (node-eval n "(car 1)"))"#), "\"car: expected pair, got 1\"");
    assert_eq!(eval(&mut vm, r#"(remote-value? (node-eval n "f"))"#), "#t");
    // Inspection of the remote definition.
    let help = eval(&mut vm, "(node-describe n 'f)");
    assert!(help.contains("(f x)") && help.contains("Double X."), "{help}");
}

#[test]
fn remote_processes_use_the_process_procedures() {
    let mut vm = vm();
    let out = eval(
        &mut vm,
        r#"(call-with-process "sh" '("-c" "echo out; echo err >&2; exit 4")
             (lambda (p) (list (process-read-all p 'stdout) (process-read-all p 'stderr) (process-wait p)))
             #:node n)"#,
    );
    assert_eq!(out, r#"("out\n" "err\n" 4)"#);
    let out = eval(
        &mut vm,
        r#"(call-with-process "cat" '()
             (lambda (p) (process-write p "x\n") (process-close-input p) (list (process-read-all p 'stdout) (process-wait p)))
             #:pty #t #:node n)"#,
    );
    assert_eq!(out, r#"("x\r\nx\r\n" 0)"#);
    let out = eval(
        &mut vm,
        r#"(call-with-process "sh" '("-c" "read x; stty size")
             (lambda (p) (process-resize p 50 132) (process-write p "\n") (process-read-all p 'stdout))
             #:pty #t #:node n)"#,
    );
    assert_eq!(out, r#""\r\n50 132\r\n""#);
    let out = eval(
        &mut vm,
        r#"(call-with-process "sleep" '("30")
             (lambda (p) (process-signal p 'term) (list (process-wait p) (process-exited? p))) #:node n)"#,
    );
    assert_eq!(out, "((signal 15) #t)");
}

#[test]
fn interrupting_a_remote_evaluation() {
    let mut vm = vm();
    let out = eval(
        &mut vm,
        r#"(define t (spawn (lambda () (guard (e (#t (condition/report-string e))) (node-eval n "(let loop () (loop))")))))
           (sleep 100)
           (node-interrupt n)
           (task-join t)"#,
    );
    assert_eq!(out, "\"interrupted\"");
    // The node still works.
    assert_eq!(eval(&mut vm, r#"(node-eval n "(* 6 7)")"#), "42");
}

/// Children of `pid` whose command is `name`.
fn children_named(pid: u32, name: &str) -> Vec<u32> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|p| {
            let stat = std::fs::read_to_string(format!("/proc/{p}/stat")).unwrap_or_default();
            // pid (comm) state ppid ...
            let Some((comm, rest)) = stat.split_once(") ") else { return false };
            comm.ends_with(&format!("({name}")) && rest.split_whitespace().nth(1) == Some(&pid.to_string())
        })
        .collect()
}

#[test]
fn transport_loss_fails_pending_operations() {
    // Like ssh, the transport relays the node's stdio; the relays dying is
    // the connection dropping.
    let mut vm = Vm::new();
    techne_node::install(&mut vm).unwrap();
    let node = env!("CARGO_BIN_EXE_techne-node");
    vm.eval_source(&format!(r#"(define n (node-connect (list "sh" "-c" "cat | {node} | cat")))"#)).unwrap();
    let child: i64 = eval(
        &mut vm,
        r#"(define p (process-spawn "sleep" '("30") #:node n))
           (define t (spawn (lambda () (guard (e (#t (condition/report-string e))) (process-wait p)))))
           (sleep 50)
           (process-pid p)"#,
    )
    .parse()
    .unwrap();
    let shell: u32 = eval(&mut vm, "(node-transport-pid n)").parse().unwrap();
    let relays = children_named(shell, "cat");
    assert_eq!(relays.len(), 2, "two relays");
    for r in relays {
        rustix::process::kill_process(rustix::process::Pid::from_raw(r as i32).unwrap(), rustix::process::Signal::KILL).unwrap();
    }
    // Pending and later operations fail; the node notices too and kills
    // what it started.
    assert_eq!(eval(&mut vm, "(task-join t)"), "\"node connection lost\"");
    let err = vm.eval_source(r#"(node-eval n "1")"#).unwrap_err();
    assert!(err.msg.contains("node connection lost"), "{err}");
    assert!(gone(child), "remote child {child} survived the lost connection");
}

#[test]
fn nodes_kill_their_processes_when_the_client_goes() {
    let mut vm = vm();
    let child: i64 = eval(&mut vm, r#"(define p (process-spawn "sleep" '("30") #:node n)) (process-pid p)"#).parse().unwrap();
    assert!(std::path::Path::new(&format!("/proc/{child}")).exists());
    eval(&mut vm, "(node-close n)");
    assert!(gone(child), "remote child {child} survived its client");
    let err = vm.eval_source("(process-wait p)").unwrap_err();
    assert!(err.msg.contains("node connection"), "{err}");
}

#[test]
fn remote_values() {
    let mut vm = vm();
    let out = eval(
        &mut vm,
        r#"(define double (node-eval n "(lambda (x) (* x 2))"))
           (node-eval n "(define (make-counter) (let ((c 0)) (lambda () (set! c (+ c 1)) c)))")
           (define counter (node-eval n "(make-counter)"))
           (node-apply n counter)
           (define twice (node-eval n "(lambda (f x) (f (f x)))"))
           (define pt (node-eval n "(define-record-type point (make-point x y) point? (x point-x) (y point-y)) (make-point 3 4)"))
           (list (remote-value? double) (node-apply n double 21) (node-apply n counter)
                 (node-apply n twice double 5) (node-apply n (node-eval n "point-y") pt)
                 (remote-value? (node-apply n (node-eval n "list") double 1)))"#,
    );
    assert_eq!(out, "(#t 42 2 20 4 #t)");
    // Inspection of a remote value, and its printed form.
    eval(&mut vm, r#"(node-eval n "(define (area w h) \"Area of a W by H rectangle.\" (* w h))")"#);
    let help = eval(&mut vm, r#"(node-describe n (node-eval n "area"))"#);
    assert!(help.contains("(area w h)") && help.contains("Area of a W by H rectangle."), "{help}");
    // The client prints a remote value the way the node writes it.
    let written = eval(&mut vm, "(remote-value-written pt)");
    assert_eq!(written, format!("{:?}", eval_written(&mut vm)));
    // Errors arrive as conditions; local non-data and other nodes' values are refused.
    let out = eval(&mut vm, r#"(guard (e (#t (condition/report-string e))) (node-apply n double "x"))"#);
    assert!(out.contains("*: expected number"), "{out}");
    let err = vm.eval_source("(node-apply n double car)").unwrap_err();
    assert!(err.msg.contains("only data and remote values"), "{err}");
    let node = env!("CARGO_BIN_EXE_techne-node");
    let err = vm.eval_source(&format!(r#"(define n2 (node-connect (list {node:?}))) (node-apply n2 double 1)"#)).unwrap_err();
    assert!(err.msg.contains("another node"), "{err}");
    // Dropped handles are released on the node.
    let before: i64 = eval(&mut vm, r#"(node-eval n "(%node-handle-count)")"#).parse().unwrap();
    eval(&mut vm, "(set! double #f) (set! counter #f) (set! twice #f) (set! pt #f) (collect-garbage)");
    std::thread::sleep(Duration::from_millis(200));
    let after: i64 = eval(&mut vm, r#"(node-eval n "(%node-handle-count)")"#).parse().unwrap();
    assert!(after < before, "handles released: {before} -> {after}");
}

/// How the node writes a record value.
fn eval_written(vm: &mut Vm) -> String {
    eval(vm, r#"(node-eval n "(call-with-output-string (lambda (p) (write (make-point 3 4) p)))")"#).trim_matches('"').to_string()
}

/// A session test environment: a private socket directory.
fn session_command(dir: &std::path::Path, name: &str) -> String {
    let node = env!("CARGO_BIN_EXE_techne-node");
    // A short idle timeout: a failing test does not leave the daemon behind.
    format!(r#"(list "env" "TECHNE_NODE_DIR={}" "TECHNE_NODE_IDLE_SECS=30" {node:?} "--session" {name:?})"#, dir.display())
}

#[test]
fn sessions_survive_disconnects() {
    let dir = std::env::temp_dir().join(format!("techne-node-test-{}", std::process::id()));
    let mut vm = Vm::new();
    techne_node::install(&mut vm).unwrap();
    let connect = format!("(define n (node-connect {}))", session_command(&dir, "s1"));
    eval(&mut vm, &connect);
    // State, a persistent process and an ordinary one.
    let out = eval(
        &mut vm,
        r#"(node-eval n "(define counter 41)")
           (define held (node-eval n "car"))
           (define ticker (process-spawn "sh" '("-c" "i=0; while [ $i -lt 1000 ]; do i=$((i+1)); echo $i; sleep 0.02; done") #:node n #:persist #t))
           (define sleeper (process-spawn "sleep" '("30") #:node n))
           (list (process-read ticker 'stdout) (process-pid ticker) (process-pid sleeper))"#,
    );
    let parts: Vec<&str> = out.trim_matches(|c| c == '(' || c == ')').split_whitespace().collect();
    let (ticker, sleeper): (i64, i64) = (parts[1].parse().unwrap(), parts[2].parse().unwrap());
    // Disconnect: the ordinary process dies, the persistent one runs on.
    eval(&mut vm, "(node-close n) (set! ticker #f) (set! sleeper #f)");
    assert!(gone(sleeper), "the ordinary process outlived its connection");
    std::thread::sleep(Duration::from_millis(300));
    assert!(std::path::Path::new(&format!("/proc/{ticker}")).exists(), "the persistent process died with the connection");
    // Reconnect: state, the process list, and output produced meanwhile.
    eval(&mut vm, &connect);
    assert_eq!(eval(&mut vm, r#"(node-eval n "(+ counter 1)")"#), "42");
    // The old connection's remote values were released with it.
    assert_eq!(eval(&mut vm, r#"(node-eval n "(%node-handle-count)")"#), "0");
    let out = eval(
        &mut vm,
        r#"(define info (car (filter (lambda (p) (cdr (assq 'persistent p))) (node-processes n))))
           (define p (node-process n (cdr (assq 'proc info))))
           (define (read-lines k acc) (if (= k 0) acc (read-lines (- k 1) (string-append acc (process-read p 'stdout)))))
           (list (cdr (assq 'status info)) (cdr (assq 'dropped info)) (cdr (assq 'command info)) (= (process-pid p) (cdr (assq 'pid info)))
                 (> (length (string-split (read-lines 3 "") #\newline)) 3))"#,
    );
    assert_eq!(out, r#"(running 0 ("sh" "-c" "i=0; while [ $i -lt 1000 ]; do i=$((i+1)); echo $i; sleep 0.02; done") #t #t)"#);
    // Bounded replay: a persistent process that floods output nobody reads
    // finishes anyway; the oldest output is dropped.
    let out = eval(
        &mut vm,
        r#"(define flood (process-spawn "sh" '("-c" "head -c 10000000 /dev/zero | tr '\\0' x") #:node n #:persist #t))
           (define status (process-wait flood))
           (define kept (string-length (process-read-all flood 'stdout)))
           (list status (+ kept (process-dropped flood)) (<= kept 1048576) (> (process-dropped flood) 0))"#,
    );
    assert_eq!(out, "(0 10000000 #t #t)");
    // Shutdown ends the session and its processes.
    eval(&mut vm, "(process-kill p) (node-shutdown n)");
    assert!(gone(ticker), "the session's process outlived its shutdown");
    std::thread::sleep(Duration::from_millis(200));
    assert!(!dir.join("s1.sock").exists(), "the daemon removes its socket");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn evaluation_in_a_node_module() {
    let dir = std::env::temp_dir().join(format!("techne-node-modules-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.scm"), "(define (where) 'lib)").unwrap();
    let lib = dir.join("lib.scm").canonicalize().unwrap();
    let lib = lib.to_str().unwrap();
    let mut vm = vm();
    eval(&mut vm, r#"(node-eval n "(define (where) 'user)")"#);
    assert_eq!(eval(&mut vm, &format!(r#"(node-eval n "(where)" #:module {lib:?})"#)), "lib");
    assert_eq!(eval(&mut vm, r#"(node-eval n "(where)")"#), "user");
    // in-module lasts for one request only.
    assert_eq!(eval(&mut vm, &format!(r#"(node-eval n "(in-module \"{lib}\") (current-module)")"#)), format!("{lib:?}"));
    assert_eq!(eval(&mut vm, r#"(node-eval n "(current-module)")"#), "\"user\"");
    let err = vm.eval_source(r#"(node-eval n "1" #:module "no-such.scm")"#).unwrap_err();
    assert!(err.msg.contains("no module"), "{err}");
}

#[test]
fn nodes_need_the_capability() {
    use techne_vm::vm::{Capability, Grants};
    let mut vm = Vm::with_grants(Grants::ALL.without(Capability::Network));
    techne_node::install(&mut vm).unwrap();
    let node = env!("CARGO_BIN_EXE_techne-node");
    let err = vm.eval_source(&format!("(node-connect (list {node:?}))")).unwrap_err();
    assert!(err.msg.contains("needs network"), "{err}");
    // Local processes are a separate grant.
    assert_eq!(eval(&mut vm, r#"(call-with-process "true" '() process-wait)"#), "0");
}

#[test]
fn bytevectors_and_the_text_data_set_round_trip() {
    let mut vm = vm();
    assert_eq!(eval(&mut vm, r##"(node-eval n "#u8(0 1 255)")"##), "#u8(0 1 255)");
    assert_eq!(
        eval(
            &mut vm,
            r#"(node-apply n (node-eval n "(lambda (x) x)")
                         (list #u8(0 1 255) '(a . b) #\c '#:key (expt 2 80) 1/2 1+2i))"#
        ),
        "(#u8(0 1 255) (a . b) #\\c #:key 1208925819614629174706176 1/2 1+2i)"
    );
    assert_eq!(eval(&mut vm, r#"(remote-value? (node-eval n "{a 1}"))"#), "#t");
}

#[test]
fn cyclic_deep_and_large_results_are_handles() {
    // A timeout also catches a native classifier stuck outside the VM's
    // interrupt checks. Dropping the raw child kills it on failure.
    use techne_node::protocol::{Message, Outcome, Reply, Request, Response, handshake, read_frame, write_frame};
    techne_process::runtime().block_on(async {
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_techne-node"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdout.take().unwrap();
        let mut out = child.stdin.take().unwrap();
        handshake(&mut input, &mut out, None).await.unwrap();
        for source in [
            "(let ((p (cons 1 '()))) (set-cdr! p p) p)",
            "(let ((v (vector 1))) (vector-set! v 0 v) v)",
            "(define-record-type cell (cell value) cell? (value get-value set-value!)) (let ((c (cell 1))) (set-value! c c) c)",
            "(let loop ((i 0) (v 1)) (if (= i 1001) v (loop (+ i 1) (vector v))))",
            "(make-vector 1000000 1)",
            "(make-list 100000 1)",
            "(let loop ((i 0) (v 1)) (if (= i 40) v (loop (+ i 1) (vector v v))))",
        ] {
            write_frame(&mut out, &Message { id: 1, body: Request::Eval { source: source.into(), module: None } }).await.unwrap();
            let reply = tokio::time::timeout(Duration::from_secs(5), read_frame::<Message<Response>>(&mut input))
                .await
                .expect("classification did not finish")
                .unwrap()
                .unwrap();
            match reply.body.unwrap() {
                Reply::Evaluated { outcome: Outcome::Handle { written, .. }, .. } => {
                    assert_eq!(written, "#<value past data limits>");
                }
                reply => panic!("{source}: {reply:?}"),
            }
        }
        // The VM remains usable, without an interrupt.
        write_frame(&mut out, &Message { id: 2, body: Request::Eval { source: "42".into(), module: None } }).await.unwrap();
        let reply =
            tokio::time::timeout(Duration::from_secs(5), read_frame::<Message<Response>>(&mut input)).await.unwrap().unwrap().unwrap();
        assert!(matches!(reply.body, Ok(Reply::Evaluated { outcome: Outcome::Data(ref s), .. }) if s == "42"));
    });
}

#[test]
fn cyclic_and_large_arguments_are_refused_locally() {
    let mut vm = vm();
    eval(&mut vm, r#"(define identity (node-eval n "(lambda (x) x)"))"#);
    for (src, says) in [
        ("(let ((p (cons 1 '()))) (set-cdr! p p) (node-apply n identity p))", "cyclic"),
        ("(node-apply n identity (make-vector 1000000 1))", "size limit"),
    ] {
        let err = vm.eval_source(src).unwrap_err();
        assert!(err.msg.contains(says), "{err}");
    }
    assert_eq!(eval(&mut vm, "(node-apply n identity 42)"), "42");
    // A bounded refusal of data does not prevent using the remote value.
    assert_eq!(
        eval(
            &mut vm,
            r#"(define cyclic (node-eval n "(let ((p (cons 1 '()))) (set-cdr! p p) p)"))
                         (node-apply n (node-eval n "(lambda (p) (eq? (cdr p) p))") cyclic)"#
        ),
        "#t"
    );
}

#[test]
fn oversized_frames_become_errors_without_losing_the_connection() {
    let mut vm = vm();
    let err = vm.eval_source(r#"(node-eval n "(display (make-string 16777216 #\\x)) 1")"#).unwrap_err();
    assert!(err.msg.contains("frame exceeds") && err.msg.contains("limit"), "{err}");
    let err = vm.eval_source("(node-eval n (make-string 16777216 #\\space))").unwrap_err();
    assert!(err.msg.contains("frame exceeds") && err.msg.contains("limit"), "{err}");
    assert_eq!(eval(&mut vm, r#"(node-eval n "42")"#), "42");
    // A remote value a reply too large named is let go, not kept unnamed.
    let held = eval(&mut vm, r#"(node-eval n "(%node-handle-count)")"#);
    let err = vm.eval_source(r#"(node-eval n "(display (make-string 16777216 #\\x)) (make-vector 10 car)")"#).unwrap_err();
    assert!(err.msg.contains("frame exceeds"), "{err}");
    assert_eq!(eval(&mut vm, r#"(node-eval n "(%node-handle-count)")"#), held);
}

#[test]
fn the_server_refuses_a_different_protocol_version() {
    use techne_node::protocol::{Hello, VERSION, read_frame, write_frame};
    techne_process::runtime().block_on(async {
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_techne-node"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdout.take().unwrap();
        let mut out = child.stdin.take().unwrap();
        assert_eq!(read_frame::<Hello>(&mut input).await.unwrap().unwrap().version, VERSION);
        write_frame(&mut out, &Hello { version: VERSION + 1, facts: None }).await.unwrap();
        let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await.unwrap().unwrap();
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("node protocol version handshake failed: version mismatch"), "{error}");
        assert!(error.contains(&format!("expected {VERSION}, received {}", VERSION + 1)), "{error}");
    });
}

#[test]
fn the_client_reports_a_different_protocol_version() {
    use techne_node::protocol::{Hello, VERSION};
    let hello = rmp_serde::to_vec_named(&Hello { version: VERSION + 1, facts: None }).unwrap();
    let bytes = (hello.len() as u32).to_be_bytes().into_iter().chain(hello);
    let escaped: String = bytes.map(|b| format!("\\{b:03o}")).collect();
    let command = format!("printf '{escaped}'; cat >/dev/null");
    let mut vm = Vm::new();
    techne_node::install(&mut vm).unwrap();
    eval(&mut vm, &format!("(define n (node-connect (list \"sh\" \"-c\" {command:?})))"));
    let err = vm.eval_source(r#"(node-eval n "1")"#).unwrap_err();
    assert!(err.msg.contains("node protocol version handshake failed: version mismatch"), "{err}");
    eval(&mut vm, "(node-close n)");
}

#[test]
fn files_are_read_written_and_listed_on_the_node() {
    let mut vm = vm();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().display().to_string();
    std::fs::create_dir_all(dir.path().join("p/.git")).unwrap();
    std::fs::create_dir_all(dir.path().join("p/src")).unwrap();
    eval(&mut vm, &format!("(define d {:?})", d));
    assert_eq!(eval(&mut vm, r#"(node-file-read n (string-append d "/p/f"))"#), "#f");
    eval(&mut vm, r#"(node-file-write n (string-append d "/p/f") (string->utf8 "hello"))"#);
    assert_eq!(eval(&mut vm, r#"(utf8->string (node-file-read n (string-append d "/p/f")))"#), "\"hello\"");
    assert_eq!(eval(&mut vm, r#"(cdr (assq 'kind (node-file-stat n (string-append d "/p/f"))))"#), "file");
    assert_eq!(eval(&mut vm, r#"(cdr (assq 'size (node-file-stat n (string-append d "/p/f"))))"#), "5");
    assert_eq!(eval(&mut vm, r#"(node-file-stat n (string-append d "/p/none"))"#), "#f");
    assert_eq!(eval(&mut vm, r#"(node-directory-list n (string-append d "/p"))"#), r#"(".git/" "f" "src/")"#);
    assert_eq!(eval(&mut vm, r#"(equal? (node-find-up n (string-append d "/p/src") '(".jj" ".git")) (string-append d "/p"))"#), "#t");
    // A directory is not read as a file.
    let err = vm.eval_source(r#"(node-file-read n (string-append d "/p"))"#).unwrap_err();
    assert!(err.msg.contains("directory"), "{err}");
}

#[test]
fn the_node_reports_what_it_is() {
    let mut vm = vm();
    assert_eq!(eval(&mut vm, "(cdr (assq 'uid (node-facts n)))"), rustix::process::getuid().as_raw().to_string());
    assert_eq!(eval(&mut vm, "(cdr (assq 'os (node-facts n)))"), "\"linux\"");
    let home = std::env::var("HOME").unwrap();
    assert_eq!(eval(&mut vm, "(cdr (assq 'home (node-facts n)))"), format!("{home:?}"));
}

#[test]
fn a_node_is_reached_through_another() {
    let mut vm = vm();
    let node = env!("CARGO_BIN_EXE_techne-node");
    // The trivial hop: run the node binary on the first node.
    eval(&mut vm, &format!("(define m (node-connect-via n {node:?} '()))"));
    assert_eq!(eval(&mut vm, r#"(node-eval m "(+ 1 2)")"#), "3");
    assert_eq!(eval(&mut vm, "(equal? (node-facts m) (node-facts n))"), "#t");
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("f").display().to_string();
    eval(&mut vm, &format!(r#"(node-file-write m {f:?} (make-bytevector 300000 65))"#));
    assert_eq!(eval(&mut vm, &format!("(bytevector-length (node-file-read m {f:?}))")), "300000");
    // Processes on it, with a directory and an environment.
    let out = eval(
        &mut vm,
        &format!(
            r#"(call-with-process "sh" '("-c" "pwd; echo $X") (lambda (p) (process-read-all p 'stdout))
                 #:node m #:directory {:?} #:environment '(("X" . "y")))"#,
            dir.path().display().to_string()
        ),
    );
    assert_eq!(out, format!("{:?}", format!("{}\ny\n", dir.path().display())));
    // A hop that fails says why.
    eval(&mut vm, r#"(define bad (node-connect-via n "sh" '("-c" "echo no such node >&2; exit 3")))"#);
    let err = vm.eval_source("(node-facts bad)").unwrap_err();
    assert!(err.msg.contains("no such node"), "{err}");
}

#[test]
fn local_processes_take_a_directory_and_an_environment() {
    let mut vm = Vm::new();
    techne_process::install(&mut vm).unwrap();
    let out = eval(
        &mut vm,
        r#"(call-with-process "sh" '("-c" "pwd; echo $X") (lambda (p) (process-read-all p 'stdout))
             #:directory "/" #:environment '(("X" . "z")))"#,
    );
    assert_eq!(out, "\"/\\nz\\n\"");
}
