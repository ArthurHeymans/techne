//! Lifetimes: what a request and every task it spawns share, their CPU
//! time and its bound, how many tasks there are, and their end.
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
        match vm.run_tasks_for(Duration::from_millis(5)) {
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

/// A task of a lifetime running other tasks inside it (a scheduler run
/// from a native, as the program's top level waits) is not charged for
/// their slices: here, of a task of no lifetime. (`run-tasks` in a task
/// runs the others, then finds that the task running it can never finish.)
#[test]
fn time_run_inside_a_task_is_not_its_own() {
    for (mode, mut vm) in vms() {
        eval(
            &mut vm,
            "(define (busy ms) (let ((end (+ (current-jiffy) (* ms 1000)))) (let loop () (when (< (current-jiffy) end) (loop)))))
             (define other (spawn (lambda () (sleep 1) (busy 300))))",
        );
        let request = vm.eval_source("(lambda () (guard (e (#t 'done)) (member 1 '(1) (lambda (a b) (run-tasks) #t))))").unwrap();
        let lifetime = vm.new_lifetime(LifetimeLimits { cpu: Some(Duration::from_millis(150)), tasks: None });
        let root = vm.spawn_in(lifetime, &request).unwrap();
        run(&mut vm, Duration::from_secs(5), |vm| vm.lifetime_usage(lifetime).is_some_and(|u| u.live == 0));
        let usage = vm.lifetime_usage(lifetime).unwrap();
        assert!(!usage.cpu_exceeded && usage.cpu < Duration::from_millis(100), "{mode}: {usage:?}");
        let done = vm.task_result(&root).unwrap().unwrap();
        assert_eq!(vm.repr(&done), "done", "{mode}");
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
