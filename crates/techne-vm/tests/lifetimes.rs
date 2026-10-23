//! Lifetimes: what a request and every task it spawns share, their CPU
//! time and its bound, how many tasks there are, what the host attached,
//! and their end.
//! On Linux, where the thread's CPU time is what is charged.
#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use techne_vm::{
    tasks::{LifetimeLimits, Progress},
    vm::Vm,
};

fn vms() -> Vec<(String, Vm)> {
    [None, Some(1)]
        .into_iter()
        .map(|jit| {
            let mut vm = Vm::new();
            vm.set_jit(jit);
            (format!("jit {jit:?}"), vm)
        })
        .collect()
}

fn eval(vm: &mut Vm, src: &str) -> String {
    let v = vm.eval_source(src).unwrap_or_else(|e| panic!("{src}: {e}"));
    vm.repr(&v)
}

/// Run the tasks until none can run, `until` holds, or `limit` passes.
fn run(vm: &mut Vm, limit: Duration, until: impl Fn(&Vm) -> bool) {
    let start = Instant::now();
    while start.elapsed() < limit && !until(vm) {
        match vm.run_tasks_for(Duration::from_millis(5)).unwrap() {
            Progress::OutOfTime => {}
            _ => match vm.next_timer() {
                Some(_) => std::thread::sleep(Duration::from_millis(1)),
                None => return,
            },
        }
    }
}

/// Past its CPU bound, every task of a lifetime is killed: those its tasks
/// spawned, in other scopes and as children too, and only those.
#[test]
fn past_its_cpu_bound_a_lifetime_is_killed() {
    for (mode, mut vm) in vms() {
        eval(
            &mut vm,
            "(define spun '())
             (define (spin) (let loop ((i 0)) (loop (+ i 1))))
             (define bystander (spawn spin))
             (define (request)
               (set! spun (list (spawn spin)
                                (spawn spin #:child #t)
                                (with-scope (make-scope 'elsewhere) (spawn spin))))
               (spin))",
        );
        let request = vm.eval_source("request").unwrap();
        let lifetime = vm.new_lifetime(LifetimeLimits { cpu: Some(Duration::from_millis(50)), tasks: None });
        let root = vm.spawn_in(lifetime, &request).unwrap();
        run(&mut vm, Duration::from_secs(10), |vm| vm.lifetime_usage(lifetime).is_some_and(|u| u.live == 0));
        let usage = vm.lifetime_usage(lifetime).unwrap();
        assert!(usage.cpu_exceeded, "{mode}: {usage:?}");
        assert!(usage.cpu > Duration::from_millis(50) && usage.cpu < Duration::from_millis(500), "{mode}: {usage:?}");
        assert_eq!((usage.live, usage.spawned), (0, 4), "{mode}");
        assert!(vm.task_result(&root).unwrap().unwrap_err().msg.contains("killed"), "{mode}");
        assert_eq!(eval(&mut vm, "(map task-done? spun)"), "(#t #t #t)", "{mode}");
        assert_eq!(eval(&mut vm, "(task-done? bystander)"), "#f", "{mode}");
        eval(&mut vm, "(task-kill bystander)");
        assert!(vm.end_lifetime(lifetime).is_some());
    }
}

/// Waiting uses no CPU; a lifetime's spawns past its bound of tasks are
/// refused; ending it kills what is left, and nothing spawns in it after.
#[test]
fn bounds_waits_and_the_end() {
    for (mode, mut vm) in vms() {
        let request = vm
            .eval_source(
                "(lambda ()
                   (sleep 100)
                   (spawn (lambda () (sleep 100000)))
                   (guard (e (#t (condition/report-string e))) (spawn (lambda () #t))))",
            )
            .unwrap();
        let lifetime = vm.new_lifetime(LifetimeLimits { cpu: Some(Duration::from_millis(50)), tasks: Some(2) });
        let root = vm.spawn_in(lifetime, &request).unwrap();
        run(&mut vm, Duration::from_millis(500), |_| false);
        let refused = vm.task_result(&root).unwrap().unwrap();
        assert!(vm.repr(&refused).contains("bound of 2 tasks"), "{mode}: {}", vm.repr(&refused));
        let usage = vm.lifetime_usage(lifetime).unwrap();
        assert!(!usage.cpu_exceeded && usage.cpu < Duration::from_millis(50), "{mode}: {usage:?}");
        assert_eq!((usage.live, usage.spawned), (1, 2), "{mode}");
        let ended = vm.end_lifetime(lifetime).unwrap();
        assert_eq!(ended.live, 1, "{mode}");
        assert_eq!(eval(&mut vm, "(%live-task-count)"), "0", "{mode}: the sleeper is killed");
        assert!(vm.end_lifetime(lifetime).is_none());
        assert!(vm.lifetime_usage(lifetime).is_none());
        assert!(vm.spawn_in(lifetime, &request).is_err());
        // Its slot is reused, under another id.
        let next = vm.new_lifetime(LifetimeLimits::default());
        assert_ne!(next, lifetime);
        assert!(vm.lifetime_usage(lifetime).is_none());
    }
}

/// A task of a lifetime waiting in `run-tasks` (here inside a callback
/// of `member`, which can wait) is not charged for the slices of the
/// tasks that run meanwhile: here, of a task of no lifetime.
#[test]
fn time_run_inside_a_task_is_not_its_own() {
    for (mode, mut vm) in vms() {
        eval(
            &mut vm,
            "(define (busy ms) (let ((end (+ (current-jiffy) (* ms 1000)))) (let loop () (when (< (current-jiffy) end) (loop)))))
             (define other (spawn (lambda () (sleep 1) (busy 300))))",
        );
        let request = vm.eval_source("(lambda () (member 1 '(1) (lambda (a b) (run-tasks) #t)))").unwrap();
        let lifetime = vm.new_lifetime(LifetimeLimits { cpu: Some(Duration::from_millis(150)), tasks: None });
        let root = vm.spawn_in(lifetime, &request).unwrap();
        run(&mut vm, Duration::from_secs(5), |vm| vm.lifetime_usage(lifetime).is_some_and(|u| u.live == 0));
        let usage = vm.lifetime_usage(lifetime).unwrap();
        assert!(!usage.cpu_exceeded && usage.cpu < Duration::from_millis(100), "{mode}: {usage:?}");
        let done = vm.task_result(&root).unwrap().unwrap();
        assert_eq!(vm.repr(&done), "(1)", "{mode}");
    }
}

/// The CPU time this thread used, as lifetimes count it.
fn thread_cpu() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a valid clock and a timespec to write to.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// Work on the thread for `cpu` of its CPU time (wall time is more on a
/// loaded machine).
fn work(cpu: Duration) {
    let end = thread_cpu() + cpu;
    while thread_cpu() < end {
        std::hint::spin_loop();
    }
}

/// A future that is pending once, then works `ms` on the thread when
/// polled: the polling is its task's lifetime's time too.
struct Spins {
    polled: bool,
    ms: u64,
}

impl std::future::Future for Spins {
    type Output = Result<i64, String>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        if !self.polled {
            self.polled = true;
            cx.waker().wake_by_ref();
            return std::task::Poll::Pending;
        }
        work(Duration::from_millis(self.ms));
        std::task::Poll::Ready(Ok(42))
    }
}

/// What the bound is charged with, and what is past it: a task finishing
/// over its bound is killed rather than giving its value; nothing more is
/// spawned in a lifetime past its bound; the host's futures its tasks wait
/// on are charged; ending a lifetime from its task charges what that ran,
/// and kills the task.
#[test]
fn what_counts_against_the_bound() {
    for (mode, mut vm) in vms() {
        let quick = vm.eval_source("(lambda () 42)").unwrap();
        let lifetime = vm.new_lifetime(LifetimeLimits { cpu: Some(Duration::ZERO), tasks: None });
        let root = vm.spawn_in(lifetime, &quick).unwrap();
        run(&mut vm, Duration::from_secs(1), |_| false);
        assert!(vm.task_result(&root).unwrap().unwrap_err().msg.contains("killed"), "{mode}");
        assert!(vm.lifetime_usage(lifetime).unwrap().cpu_exceeded, "{mode}");
        assert!(vm.spawn_in(lifetime, &quick).unwrap_err().msg.contains("CPU bound"), "{mode}");
        vm.end_lifetime(lifetime);

        vm.register_async("spins", 0, |_: &mut Vm, _| Spins { polled: false, ms: 200 });
        let waits = vm.eval_source("(lambda () (spins))").unwrap();
        let lifetime = vm.new_lifetime(LifetimeLimits { cpu: Some(Duration::from_millis(20)), tasks: None });
        let root = vm.spawn_in(lifetime, &waits).unwrap();
        run(&mut vm, Duration::from_secs(2), |vm| vm.lifetime_usage(lifetime).is_some_and(|u| u.live == 0));
        let usage = vm.lifetime_usage(lifetime).unwrap();
        assert!(usage.cpu_exceeded && usage.cpu >= Duration::from_millis(200), "{mode}: {usage:?}");
        assert!(vm.task_result(&root).unwrap().is_err(), "{mode}");
        vm.end_lifetime(lifetime);

        // Ended by a native its task calls, after 100 ms of work: counted.
        let lifetime = vm.new_lifetime(LifetimeLimits::default());
        let ended = std::rc::Rc::new(std::cell::Cell::new(None));
        let (id, seen) = (lifetime, ended.clone());
        vm.register_fn_vm("work-then-end", move |vm: &mut Vm| {
            work(Duration::from_millis(100));
            seen.set(vm.end_lifetime(id));
        });
        let ends = vm.eval_source("(lambda () (work-then-end) 'after)").unwrap();
        let root = vm.spawn_in(lifetime, &ends).unwrap();
        run(&mut vm, Duration::from_secs(1), |_| false);
        let usage = ended.get().expect("ended");
        assert!(usage.cpu >= Duration::from_millis(100), "{mode}: {usage:?}");
        // The task ends with its lifetime, killed, not with its value.
        assert!(vm.task_result(&root).unwrap().unwrap_err().msg.contains("killed"), "{mode}");
    }
}

/// `spawn_in_with` applies a procedure to arguments in its task: a
/// generic too (an applicable record), which may wait there as a closure
/// may.
#[test]
fn spawned_with_arguments() {
    for (mode, mut vm) in vms() {
        let lifetime = vm.new_lifetime(LifetimeLimits::default());
        let add = vm.eval_source("(lambda (a b) (sleep 5) (+ a b))").unwrap();
        let generic = vm.eval_source("(define-generic slow) (define-method (slow (x t) y) (sleep 5) (list x y)) slow").unwrap();
        let (one, two) = (vm.make_int(1).unwrap(), vm.make_int(2).unwrap());
        let sum = vm.spawn_in_with(lifetime, &add, &[one.clone(), two.clone()]).unwrap();
        let listed = vm.spawn_in_with(lifetime, &generic, &[one.clone(), two]).unwrap();
        let wrong = vm.spawn_in_with(lifetime, &add, &[one]).unwrap();
        run(&mut vm, Duration::from_secs(2), |vm| vm.lifetime_usage(lifetime).is_some_and(|u| u.live == 0));
        let sum = vm.task_result(&sum).unwrap().unwrap();
        assert_eq!(vm.repr(&sum), "3", "{mode}");
        let listed = vm.task_result(&listed).unwrap().unwrap_or_else(|e| panic!("{mode}: {e}"));
        assert_eq!(vm.repr(&listed), "(1 2)", "{mode}");
        assert!(vm.task_result(&wrong).unwrap().unwrap_err().msg.contains("arguments"), "{mode}");
    }
}

/// Lisp looping where its task cannot be preempted (a `define-library`
/// body, compiled by `eval`) is stopped by the watchdog past its
/// lifetime's bound, and its lifetime ends past it, as at any other time.
#[test]
fn a_slice_that_does_not_end_is_stopped() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut vm = Vm::new();
        let stall =
            vm.eval_source("(lambda () (eval '(define-library (stall) (import (scheme base)) (begin (let loop () (loop))))))").unwrap();
        let lifetime = vm.new_lifetime(LifetimeLimits { cpu: Some(Duration::from_millis(50)), tasks: None });
        let task = vm.spawn_in(lifetime, &stall).unwrap();
        let start = Instant::now();
        run(&mut vm, Duration::from_secs(5), |vm| vm.lifetime_usage(lifetime).is_some_and(|u| u.live == 0));
        let usage = vm.lifetime_usage(lifetime).unwrap();
        let result = vm.task_result(&task).map(|r| r.map(|_| ()).map_err(|e| e.msg.clone()));
        let _ = tx.send((start.elapsed(), usage, result));
    });
    let (took, usage, result) = rx.recv_timeout(Duration::from_secs(20)).expect("the slice was never stopped");
    assert!(usage.cpu_exceeded, "{usage:?}");
    assert!(result.is_some_and(|r| r.is_err_and(|m| m.contains("killed"))));
    assert!(took < Duration::from_secs(2), "{took:?}");
}

/// A task of a lifetime running others inside it (`run-tasks` from a
/// native), then looping where it cannot be preempted, is stopped too:
/// after an inner task of no lifetime, which the watchdog may sleep
/// through, and after one of its own lifetime, whose time counts against
/// the bound it shares, not again.
#[test]
fn stopped_after_running_others_inside() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        for shared in [false, true] {
            let mut vm = Vm::new();
            eval(
                &mut vm,
                "(define (busy ms) (let ((end (+ (current-jiffy) (* ms 1000)))) (let loop () (when (< (current-jiffy) end) (loop)))))
                 (define (inside thunk) (spawn thunk) (guard (e (#t #f)) (member 1 '(1) (lambda (a b) (run-tasks) #t))))
                 (define (stall) (eval '(define-library (stall) (import (scheme base)) (begin (let loop () (loop))))))",
            );
            if !shared {
                eval(&mut vm, "(define other (spawn (lambda () (sleep 1) (busy 100))))");
            }
            let request = vm
                .eval_source(if shared {
                    "(lambda () (inside (lambda () (busy 80))) (stall))"
                } else {
                    "(lambda () (inside (lambda () #t)) (stall))"
                })
                .unwrap();
            let lifetime = vm.new_lifetime(LifetimeLimits { cpu: Some(Duration::from_millis(100)), tasks: None });
            vm.spawn_in(lifetime, &request).unwrap();
            run(&mut vm, Duration::from_secs(5), |vm| vm.lifetime_usage(lifetime).is_some_and(|u| u.live == 0));
            out.push((shared, vm.lifetime_usage(lifetime).unwrap()));
        }
        let _ = tx.send(out);
    });
    let out = rx.recv_timeout(Duration::from_secs(20)).expect("a slice was never stopped");
    for (shared, usage) in out {
        assert!(usage.cpu_exceeded, "{shared}: {usage:?}");
        // Stopped soon past its bound: not given the inner task's time again.
        assert!(usage.cpu < Duration::from_millis(150), "{shared}: {usage:?}");
    }
}

/// What the host attaches to a lifetime its tasks find, those they spawn
/// too; nothing else does, nor anything once it ended.
#[test]
fn a_lifetimes_tasks_find_what_is_attached_to_it() {
    let mut vm = Vm::new();
    vm.register_fn_vm("attached", |vm: &mut Vm| -> i64 { vm.lifetime_data().and_then(|d| d.downcast::<i64>().ok()).map_or(-1, |n| *n) });
    let lifetime = vm.new_lifetime(LifetimeLimits::default());
    vm.set_lifetime_data(lifetime, std::rc::Rc::new(7_i64));
    let f = vm.eval_source("(define seen '()) (lambda () (set! seen (cons (attached) seen)) (task-join (spawn (lambda () (set! seen (cons (attached) seen))))))").unwrap();
    vm.spawn_in(lifetime, &f).unwrap();
    run(&mut vm, Duration::from_secs(5), |_| false);
    assert_eq!(eval(&mut vm, "seen"), "(7 7)");
    assert_eq!(eval(&mut vm, "(attached)"), "-1");
    vm.end_lifetime(lifetime);
    let other = vm.new_lifetime(LifetimeLimits::default());
    let g = vm.eval_source("(lambda () (set! seen (attached)))").unwrap();
    vm.spawn_in(other, &g).unwrap();
    run(&mut vm, Duration::from_secs(5), |_| false);
    assert_eq!(eval(&mut vm, "seen"), "-1", "a slot used again has nothing of the lifetime before");
}
