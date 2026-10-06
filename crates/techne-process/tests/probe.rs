//! The process contract, driven from Lisp: concurrent output,
//! separate stderr, signals, EOF, slow readers and cleanup, with pipes and
//! on a pty. Every case runs on a VM where the waiting happens in tasks.

use std::time::{Duration, Instant};

use techne_vm::{builtins::repr, vm::Vm};

fn vm() -> Vm {
    let mut vm = Vm::new();
    techne_process::install(&mut vm).unwrap();
    vm
}

fn eval(vm: &mut Vm, src: &str) -> String {
    match vm.eval_source(src) {
        Ok(v) => repr(v),
        Err(e) => panic!("{src}\n{e}"),
    }
}

#[test]
fn pipes_keep_stdout_and_stderr_apart() {
    let mut vm = vm();
    let out = eval(
        &mut vm,
        r#"(call-with-process "sh" '("-c" "echo out; echo err >&2; exit 3")
             (lambda (p) (list (process-read-all p 'stdout) (process-read-all p 'stderr) (process-wait p))))"#,
    );
    assert_eq!(out, r#"("out\n" "err\n" 3)"#);
}

#[test]
fn closing_input_is_end_of_file() {
    let mut vm = vm();
    let out = eval(
        &mut vm,
        r#"(call-with-process "cat" '()
             (lambda (p)
               (process-write p "line 1\n")
               (process-write p "line 2\n")
               (process-close-input p)
               (list (process-read-all p 'stdout) (process-wait p))))"#,
    );
    assert_eq!(out, r#"("line 1\nline 2\n" 0)"#);
}

#[test]
fn processes_run_concurrently_in_tasks() {
    let mut vm = vm();
    let start = Instant::now();
    // Three children sleeping 0.3 s each, read by three tasks.
    let out = eval(
        &mut vm,
        r#"(define (run i)
             (spawn (lambda ()
               (call-with-process "sh" (list "-c" (string-append "sleep 0.3; echo " (number->string i)))
                 (lambda (p) (process-read-all p 'stdout))))))
           (map task-join (map run '(1 2 3)))"#,
    );
    assert_eq!(out, r#"("1\n" "2\n" "3\n")"#);
    assert!(start.elapsed() < Duration::from_millis(800), "children must overlap: {:?}", start.elapsed());
}

#[test]
fn signals_reach_the_process_group() {
    let mut vm = vm();
    // The shell's child (sleep) is in the same group and dies too.
    let out = eval(
        &mut vm,
        r#"(call-with-process "sh" '("-c" "sleep 30; echo not-reached")
             (lambda (p) (sleep 50) (process-signal p 'term) (list (process-wait p) (process-read-all p 'stdout))))"#,
    );
    assert_eq!(out, r#"((signal 15) "")"#);
}

/// Resident memory of this process, in KiB.
fn rss_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|l| l.starts_with("VmRSS:")).unwrap();
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

#[test]
fn slow_readers_bound_memory() {
    let mut vm = vm();
    let before = rss_kb();
    // 64 MiB of output against a reader that has not started: the queue
    // holds at most QUEUE_CHUNKS chunks and the child waits.
    let out = eval(
        &mut vm,
        r#"(define p (process-spawn "sh" '("-c" "head -c 67108864 /dev/zero | tr '\\0' x")))
           (sleep 300)
           (process-exited? p)"#,
    );
    assert_eq!(out, "#f", "the child must be blocked on its output");
    let grown = rss_kb().saturating_sub(before);
    assert!(grown < 16 * 1024, "unread output must not accumulate: grew {grown} KiB");
    let bound = techne_process::QUEUE_CHUNKS * techne_process::CHUNK_BYTES;
    assert!(bound < 1 << 20);
    // Reading drains it all, in bounded chunks.
    let out = eval(
        &mut vm,
        r#"(let loop ((total 0) (largest 0))
             (let ((c (process-read p 'stdout)))
               (if (eof-object? c)
                   (list total largest (process-wait p))
                   (loop (+ total (string-length c)) (max largest (string-length c))))))"#,
    );
    let expected = format!("(67108864 {} 0)", techne_process::CHUNK_BYTES);
    assert_eq!(out, expected);
}

#[test]
fn pty_processes_have_a_terminal() {
    let mut vm = vm();
    let out = eval(
        &mut vm,
        r#"(call-with-process "sh" '("-c" "test -t 0 && test -t 1 && echo tty; stty size")
             (lambda (p) (list (process-read-all p 'stdout) (process-wait p)))
             #:pty #t)"#,
    );
    assert_eq!(out, r#"("tty\r\n24 80\r\n" 0)"#);
    // Input echoes, and end of input is ^D.
    let out = eval(
        &mut vm,
        r#"(call-with-process "cat" '()
             (lambda (p)
               (process-write p "hello\n")
               (process-close-input p)
               (list (process-read-all p 'stdout) (process-wait p)))
             #:pty #t)"#,
    );
    assert_eq!(out, r#"("hello\r\nhello\r\n" 0)"#);
    // Resizing: the child sees the new size.
    let out = eval(
        &mut vm,
        r#"(call-with-process "sh" '("-c" "read x; stty size")
             (lambda (p)
               (process-resize p 40 120)
               (process-write p "\n")
               (list (process-read-all p 'stdout) (process-wait p)))
             #:pty #t)"#,
    );
    assert_eq!(out, r#"("\r\n40 120\r\n" 0)"#);
    let err = vm.eval_source(r#"(call-with-process "true" '() (lambda (p) (process-resize p 1 1)))"#).unwrap_err();
    assert!(err.msg.contains("not a pty"), "{err}");
    let err = vm.eval_source(r#"(call-with-process "true" '() (lambda (p) (process-read p 'stderr)) #:pty #t)"#).unwrap_err();
    assert!(err.msg.contains("one output stream"), "{err}");
}

#[test]
fn cancelled_tasks_kill_their_processes() {
    let mut vm = vm();
    let out = eval(
        &mut vm,
        r#"(define pid #f)
           (define t (spawn (lambda ()
             (call-with-process "sleep" '("30")
               (lambda (p) (set! pid (process-pid p)) (process-wait p))))))
           (sleep 50)
           (task-cancel t)
           (guard (e (#t (condition/report-string e))) (task-join t))"#,
    );
    assert_eq!(out, r#""task cancelled""#);
    let pid = vm.eval_source("pid").unwrap();
    let pid: i64 = vm.get(pid).unwrap();
    // Killed and reaped.
    let deadline = Instant::now() + Duration::from_secs(2);
    while std::path::Path::new(&format!("/proc/{pid}")).exists() {
        assert!(Instant::now() < deadline, "process {pid} survived its task");
        std::thread::sleep(Duration::from_millis(10));
    }
    // A failing body kills its process too.
    let out = eval(
        &mut vm,
        r#"(define q #f)
           (guard (e (#t (list (condition/report-string e) (begin (sleep 50) (process-exited? q)))))
             (call-with-process "sleep" '("30") (lambda (p) (set! q p) (car '()))))"#,
    );
    assert_eq!(out, r#"("car: expected pair, got ()" #t)"#);
}

#[test]
fn utf8_split_across_reads_is_joined() {
    let mut vm = vm();
    // "é" is two bytes; print the halves with a pause between them.
    let out = eval(
        &mut vm,
        r#"(call-with-process "sh" '("-c" "printf '\\303'; sleep 0.1; printf '\\251\\n'")
             (lambda (p) (process-read-all p 'stdout)))"#,
    );
    assert_eq!(out, "\"é\\n\"");
}

#[test]
fn interrupts_reach_a_waiting_evaluation() {
    let mut vm = vm();
    let handle = vm.interrupt_handle();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        handle.interrupt();
    });
    let err = vm.eval_source(r#"(call-with-process "sleep" '("30") (lambda (p) (process-wait p)))"#).unwrap_err();
    assert!(err.is_interrupt(), "{err}");
}

#[test]
fn processes_need_the_capability() {
    use techne_vm::vm::{Capability, Grants};
    let mut vm = Vm::with_grants(Grants::ALL.without(Capability::Processes));
    techne_process::install(&mut vm).unwrap();
    for src in [r#"(process-spawn "true" '())"#, r#"(call-with-process "true" '() process-wait)"#] {
        let err = vm.eval_source(src).unwrap_err();
        assert!(err.msg.contains("not granted in this world (needs processes)"), "{src}: {err}");
    }
}
