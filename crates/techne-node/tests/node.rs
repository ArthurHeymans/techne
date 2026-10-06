//! The node contract: evaluation and inspection in another process, remote
//! processes through the ordinary process procedures, transport loss and
//! cleanup.

use std::time::{Duration, Instant};

use techne_vm::{builtins::repr, vm::Vm};

fn vm() -> Vm {
    let mut vm = Vm::new();
    techne_node::install(&mut vm).unwrap();
    let node = env!("CARGO_BIN_EXE_techne-node");
    vm.eval_source(&format!("(define n (node-connect (list {node:?})))")).unwrap();
    vm
}

fn eval(vm: &mut Vm, src: &str) -> String {
    match vm.eval_source(src) {
        Ok(v) => repr(v),
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
    let err = vm.eval_source(r#"(node-eval n "f")"#).unwrap_err();
    assert!(err.msg.contains("not data") && err.msg.contains("procedure"), "{err}");
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
