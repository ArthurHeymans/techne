//! Stopping executions (PLAN.md, Stage 1, step 8): a break is a catchable
//! request to the execution it is addressed to; a kill ends it, past every
//! handler and cleanup, and never reaches another execution.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use techne_vm::vm::{Error, ExecId, Stop, Vm};

fn vms() -> Vec<(String, Vm)> {
    let mut vms = Vec::new();
    for jit in [None, Some(1)] {
        let mut normal = Vm::new();
        normal.set_jit(jit);
        let mut stress = Vm::new();
        stress.heap.stress = true;
        stress.set_jit(jit);
        vms.push((format!("jit {jit:?}"), normal));
        vms.push((format!("gc stress, jit {jit:?}"), stress));
    }
    vms
}

fn eval(vm: &mut Vm, src: &str) -> String {
    techne_vm::builtins::repr(vm.eval_source(src).unwrap_or_else(|e| panic!("{src}: {e}")))
}

/// Evaluate `src` as a top-level execution, sending `stops` to it from
/// another thread, one every 30 ms. One still running 20 s later ends the
/// test program rather than hang it.
fn eval_stopped(vm: &mut Vm, src: &str, stops: &[Stop]) -> Result<String, Error> {
    let id = vm.new_execution();
    let (handle, stops) = (vm.interrupt_handle(), stops.to_vec());
    let sender = std::thread::spawn(move || {
        for stop in stops {
            std::thread::sleep(Duration::from_millis(30));
            handle.stop(id, stop);
        }
    });
    let (done, ended) = mpsc::channel::<()>();
    let what = src.to_string();
    let watchdog = std::thread::spawn(move || {
        if let Err(mpsc::RecvTimeoutError::Timeout) = ended.recv_timeout(Duration::from_secs(20)) {
            eprintln!("{what}: not stopped after 20 s");
            std::process::exit(1);
        }
    });
    vm.enter_execution(id);
    let result = vm.eval_source(src).map(techne_vm::builtins::repr);
    vm.leave_execution(id);
    drop(done);
    sender.join().unwrap();
    watchdog.join().unwrap();
    result
}

/// Code that catches every condition is killed, also in a handler, inside a
/// native's call into Scheme or while it waits; no after thunk runs, and the
/// VM goes on, its dynamic state as it was.
#[test]
fn a_kill_ends_what_breaks_cannot() {
    for (mode, mut vm) in vms() {
        eval(
            &mut vm,
            "(define p (make-parameter 'outside)) (define after-ran #f) (define waited #f)
             (define (stubborn) (let l () (guard (e (#t #f)) (let s () (s))) (l)))",
        );
        for src in [
            "(stubborn)",
            "(with-exception-handler (lambda (e) (stubborn)) (lambda () (raise 'x)))",
            "(dynamic-wind (lambda () #f) stubborn (lambda () (set! after-ran #t)))",
            "(parameterize ((p 'inside)) (stubborn))",
            // Inside calls into Scheme from natives.
            "(member 1 '(2 3) (lambda (a b) (stubborn)))",
            "(eval '(stubborn))",
            "(let ((t (make-hash-table (lambda (a b) (stubborn))))) (hash-table-set! t 1 1) (hash-table-ref/default t 2 #f))",
            "(eval-source \"(stubborn)\" \"user\" \"f.scm\")",
            // Waiting for a task, catching breaks there.
            "(set! waited (spawn stubborn)) (let l () (guard (e (#t #f)) (task-join waited)) (l))",
        ] {
            let e = eval_stopped(&mut vm, src, &[Stop::Kill]).unwrap_err();
            assert!(e.is_kill(), "{mode}: {src}: {e}");
            assert_eq!(eval(&mut vm, "(list (+ 1 2) (p) after-ran)"), "(3 outside #f)", "{mode}: {src}");
            eval(&mut vm, "(if waited (task-kill waited))");
        }
    }
}

/// A break is the catchable condition "interrupted".
#[test]
fn a_break_is_caught() {
    for (mode, mut vm) in vms() {
        let caught = eval_stopped(&mut vm, "(guard (e (#t (condition/report-string e))) (let s () (s)))", &[Stop::Break]);
        assert_eq!(caught.unwrap(), "\"interrupted\"", "{mode}");
    }
}

/// A stop reaches the execution it is addressed to and no other.
#[test]
fn stops_reach_their_execution_only() {
    for (mode, mut vm) in vms() {
        // A background task that counts the breaks it catches, until the
        // channel `stop` is closed.
        eval(&mut vm, "(define breaks 0) (define stop (make-channel 1))");
        let counter = vm
            .eval_source(
                "(lambda ()
                   (let l ()
                     (guard (e (#t (set! breaks (+ breaks 1))))
                       (let s ((i 0)) (if (= 0 (modulo i 1000)) (yield)) (if (channel-closed? stop) 'done (s (+ i 1)))))
                     (if (channel-closed? stop) 'done (l))))",
            )
            .unwrap();
        let counter = vm.spawn(counter);
        // Breaking the evaluation while it waits on another task does not
        // reach the counter.
        eval(&mut vm, "(define spinner (spawn (lambda () (let s () (s)))))");
        let e = eval_stopped(&mut vm, "(task-join spinner)", &[Stop::Break]).unwrap_err();
        assert!(e.is_interrupt(), "{mode}: {e}");
        assert_eq!(eval(&mut vm, "breaks"), "0", "{mode}");
        eval(&mut vm, "(task-kill spinner)");
        // Breaking the counter leaves the evaluation alone.
        let (task, handle) = (vm.task_execution(counter), vm.interrupt_handle());
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            handle.stop(task, Stop::Break);
        });
        assert_eq!(eval(&mut vm, "(let l () (if (= breaks 0) (begin (sleep 1) (l)) 'counted))"), "counted", "{mode}");
        sender.join().unwrap();
        eval(&mut vm, "(channel-close stop)");
        vm.run_tasks().unwrap();
        assert_eq!(eval(&mut vm, "breaks"), "1", "{mode}");
        // A stop for an execution that ended reaches nothing.
        let ended = vm.new_execution();
        vm.enter_execution(ended);
        vm.leave_execution(ended);
        assert!(!vm.interrupt_handle().stop(ended, Stop::Kill), "{mode}");
        assert!(!vm.interrupt_handle().stop(ExecId(0), Stop::Kill), "{mode}");
        assert_eq!(eval(&mut vm, "(let l ((i 0)) (if (< i 100000) (l (+ i 1)) i))"), "100000", "{mode}");
    }
}

/// A killed task ends where it waits: its offers are withdrawn, its Rust
/// future dropped, and a task joining it can go on.
#[test]
fn a_killed_task_ends_where_it_waits() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    for (mode, mut vm) in vms() {
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = dropped.clone();
        vm.register_async("forever", 0, move |_, _| {
            let guard = Dropped(flag.clone());
            async move {
                let _guard = guard;
                std::future::pending::<Result<i64, String>>().await
            }
        });
        let result = eval(
            &mut vm,
            "(define ch (make-channel))
             (define sender (spawn (lambda () (channel-send ch 'from-the-killed))))
             (define waiter (spawn (lambda () (forever))))
             (sleep 1)
             (task-kill sender)
             (task-kill waiter)
             (spawn (lambda () (channel-send ch 'later)))
             (list (channel-recv ch)
                   (guard (e (#t (condition/report-string e))) (task-join sender))
                   (task-done? waiter))",
        );
        assert_eq!(result, "(later \"task killed\" #t)", "{mode}");
        assert!(dropped.load(Ordering::SeqCst), "{mode}: the future is dropped");
    }
}

/// A task killing itself ends at once; its cleanup does not run.
#[test]
fn a_task_kills_itself() {
    for (mode, mut vm) in vms() {
        let start = Instant::now();
        let result = eval(
            &mut vm,
            "(define cleaned #f)
             (define t (spawn (lambda () (dynamic-wind (lambda () #f) (lambda () (task-kill (current-task)) 'went-on) (lambda () (set! cleaned #t))))))
             (list (guard (e (#t (condition/report-string e))) (task-join t)) cleaned)",
        );
        assert_eq!(result, "(\"task killed\" #f)", "{mode}");
        assert!(start.elapsed() < Duration::from_secs(5), "{mode}");
    }
}

/// Killing a task while it is active below what runs (it called
/// `run-tasks`, which runs the killer) ends it when it goes on.
#[test]
fn killing_an_active_task() {
    for (mode, mut vm) in vms() {
        let result = eval(
            &mut vm,
            "(define parent (spawn (lambda () (spawn (lambda () (task-kill parent))) (run-tasks) 'went-on)))
             (guard (e (#t (condition/report-string e))) (task-join parent))",
        );
        assert_eq!(result, "\"task killed\"", "{mode}");
    }
}

/// A kill in an after thunk, while an error unwinds, is not caught as that
/// error would be.
#[test]
fn a_kill_while_unwinding_is_not_caught() {
    for (mode, mut vm) in vms() {
        let result = eval(
            &mut vm,
            "(define t (spawn (lambda () (guard (e (#t 'caught))
                                          (dynamic-wind (lambda () #f) (lambda () (error \"boom\")) (lambda () (task-kill (current-task))))))))
             (guard (e (#t (condition/report-string e))) (task-join t))",
        );
        assert_eq!(result, "\"task killed\"", "{mode}");
    }
}

/// `run-tasks` hears its caller's stops while tasks keep it busy, and ends
/// when the last task is killed.
#[test]
fn running_tasks_is_stopped_and_ends() {
    for (mode, mut vm) in vms() {
        eval(&mut vm, "(define spinner (spawn (lambda () (let s () (s)))))");
        let e = eval_stopped(&mut vm, "(run-tasks)", &[Stop::Kill]).unwrap_err();
        assert!(e.is_kill(), "{mode}: {e}");
        eval(&mut vm, "(task-kill spinner)");
        let result =
            eval(&mut vm, "(define t (spawn (lambda () (channel-recv (make-channel))))) (sleep 1) (task-kill t) (run-tasks) 'ended");
        assert_eq!(result, "ended", "{mode}");
    }
}

/// A task whose compiled loop catches a break, asked for while it runs, is
/// still preempted after it: the scheduler's rounds go on ending.
#[test]
fn a_task_catching_a_break_is_still_preempted() {
    let mut vm = Vm::new();
    vm.set_jit(Some(1));
    eval(&mut vm, "(define caught 0)");
    let f = vm.eval_source("(lambda () (let retry () (guard (e (#t (set! caught (+ caught 1)) (retry))) (let loop () (loop)))))").unwrap();
    let task = vm.spawn(f);
    let (handle, exec) = (vm.interrupt_handle(), vm.task_execution(task));
    let breaker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        handle.stop(exec, Stop::Break);
    });
    // Until the break is caught (while the task runs, as it does all the
    // time), then rounds after it.
    let started = Instant::now();
    while eval(&mut vm, "caught") == "0" {
        assert!(started.elapsed() < Duration::from_secs(5), "the break is not caught");
        vm.run_tasks_for(Duration::from_millis(20));
    }
    for _ in 0..10 {
        let started = Instant::now();
        vm.run_tasks_for(Duration::from_millis(20));
        assert!(started.elapsed() < Duration::from_secs(2), "a round took {:?}", started.elapsed());
    }
    breaker.join().unwrap();
}
