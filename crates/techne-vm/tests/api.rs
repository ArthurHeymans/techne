//! The Rust embedding API, normally and under GC stress.
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use techne_vm::{
    api::{Foreign, Root},
    value::Value,
    vm::Vm,
};

fn vms() -> Vec<(&'static str, Vm)> {
    let normal = Vm::new();
    let mut minor = Vm::new();
    minor.heap.stress = true;
    let mut full = Vm::new();
    full.heap.stress = true;
    full.heap.stress_full = true;
    vec![("normal", normal), ("minor-stress", minor), ("full-stress", full)]
}

fn eval_str(vm: &mut Vm, src: &str) -> String {
    let v = vm.eval_source(src).unwrap_or_else(|e| panic!("{src}: {e}"));
    techne_vm::builtins::repr(v)
}

#[test]
fn record_primitives_validate_arguments() {
    for (mode, mut vm) in vms() {
        vm.eval_source(
            "(define-record-type point (make-point x y) point? (x point-x) (y point-y set-point-y!)) (define p (make-point 1 2))",
        )
        .unwrap();
        for source in [
            "(%make-rtd 1 '())",
            "(%make-rtd 'bad '(1))",
            "(%make-rtd 'bad '(x) 1)",
            "(%make-rtd 'bad '(x) -1)",
            "(%make-rtd 'bad '(x) 'no)",
            "(%record 5)",
            "(%record point 1)",
            "(%record point 1 2 3)",
            "(%record-ref 1 2 0)",
            "(%record-ref p point -1)",
            "(%record-ref p point 2)",
            "(%record-ref p point #f)",
            "(%record-set! p point 99 3)",
            "(%values->list 0)",
            "(%values->list p)",
        ] {
            assert!(vm.eval_source(source).is_err(), "{mode}: {source}");
        }
        assert_eq!(eval_str(&mut vm, "(point-y p)"), "2", "{mode}: failed writes must not change the record");
        assert!(
            vm.eval_source("(%record (type-of (guard (e (#t e)) (error \"test\"))) 1 '() #f)").is_err(),
            "{mode}: a type name is not a descriptor"
        );
        vm.eval_source("(define names (list 'x)) (define owned-type (%make-rtd 'owned names)) (set-car! names 'changed) (define owned (%record owned-type 7))").unwrap();
        assert_eq!(eval_str(&mut vm, "(record-fields owned)"), "((x . 7))", "{mode}: schema must be owned");
        // A stale accessor and compiled match must not index beyond a new layout.
        vm.eval_source("(define old-y point-y) (define (old-match p) (match p ((point x y) y)))").unwrap();
        vm.eval_source("(define-record-type point (make-point x) point? (x point-x)) (define q (make-point 3))").unwrap();
        assert!(vm.eval_source("(old-y q)").is_err(), "{mode}");
        assert!(vm.eval_source("(old-match q)").is_err(), "{mode}");
    }
}

#[test]
fn record_procedures_capture_their_descriptor() {
    for (mode, mut vm) in vms() {
        for jit in [None, Some(1)] {
            vm.set_jit(jit);
            // A constructor may have the type's name; the public binding then
            // holds a procedure, not the descriptor its procedures need.
            vm.eval_source(
                "(define-record-type packet (packet x) packet? (x packet-x set-packet-x!))
                (define old-packet packet) (define old-packet? packet?) (define old-x packet-x)
                (define p (packet 1)) (set-packet-x! p 2)",
            )
            .unwrap();
            assert_eq!(eval_str(&mut vm, "(list (packet? p) (packet-x p))"), "(#t 2)", "{mode}, {jit:?}");
            vm.eval_source(
                "(define packet (lambda (_) 'rebound))
                (define q (old-packet 3))
                (define-record-type packet (packet x y) packet? (x packet-x) (y packet-y))
                (define fresh (packet 4 5))",
            )
            .unwrap();
            vm.full_collect();
            assert_eq!(
                eval_str(&mut vm, "(list (old-packet? p) (old-x p) (old-x q) (old-packet? fresh) (packet? p))"),
                "(#t 2 3 #f #f)",
                "{mode}, {jit:?}"
            );
            assert!(vm.eval_source("(old-x fresh)").is_err(), "{mode}, {jit:?}");
            // Internal definitions need a distinct lexical descriptor too.
            assert_eq!(
                eval_str(
                    &mut vm,
                    "(let ()
                (define-record-type local (local x) local? (x local-x))
                (define p (local 7))
                (list (local? p) (local-x p)))"
                ),
                "(#t 7)",
                "{mode}, {jit:?}"
            );
        }
    }
}

#[test]
fn native_datum_conversion_rejects_cycles_but_allows_sharing() {
    for (mode, mut vm) in vms() {
        vm.eval_source("(define cycle (cons 1 '())) (set-cdr! cycle cycle) (define v (vector #f)) (vector-set! v 0 v)").unwrap();
        for source in ["(apply + cycle)", "(eval cycle)", "(eval v)"] {
            assert!(vm.eval_source(source).is_err(), "{mode}: {source}");
        }
        assert_eq!(eval_str(&mut vm, "(let ((shared (list 1 2))) (eval (list 'quote (list shared shared))))"), "((1 2) (1 2))", "{mode}");
        assert!(vm.eval_source("(eval (let loop ((n 300) (v 1)) (if (= n 0) v (loop (- n 1) (vector v)))))").is_err(), "{mode}");
    }
}

#[test]
fn loaded_module_names_survive_collection() {
    for (mode, mut vm) in vms() {
        let expected: Vec<String> = vm.loaded_module_names().into_iter().map(|name| name.to_string()).collect();
        let value = vm.eval_source("(loaded-modules)").unwrap();
        let names: Vec<String> = vm.get(value).unwrap();
        assert_eq!(names, expected, "{mode}");
    }
}

#[test]
fn typed_functions() {
    for (mode, mut vm) in vms() {
        vm.register_fn("rs-add", |a: i64, b: f64| a as f64 + b);
        vm.register_fn("rs-greet", |name: String| format!("hello {name}"));
        vm.register_fn("rs-sum", |xs: Vec<i64>| xs.iter().sum::<i64>());
        vm.register_fn("rs-range", |n: usize| (0..n as i64).collect::<Vec<_>>());
        vm.register_fn("rs-find", |xs: Vec<String>, x: String| xs.iter().position(|y| *y == x));
        vm.register_fn("rs-parse", |s: String| s.parse::<i64>());
        vm.register_fn("rs-big", || 1i64 << 60);
        assert_eq!(eval_str(&mut vm, "(rs-add 1 2.5)"), "3.5", "{mode}");
        assert_eq!(eval_str(&mut vm, "(rs-greet \"techne\")"), "\"hello techne\"", "{mode}");
        assert_eq!(eval_str(&mut vm, "(rs-sum (list 1 2 3))"), "6", "{mode}");
        assert_eq!(eval_str(&mut vm, "(rs-sum (vector 4 5))"), "9", "{mode}");
        assert_eq!(eval_str(&mut vm, "(rs-range 4)"), "(0 1 2 3)", "{mode}");
        assert_eq!(eval_str(&mut vm, "(list (rs-find '(\"a\" \"b\") \"b\") (rs-find '() \"x\"))"), "(1 #f)", "{mode}");
        assert_eq!(eval_str(&mut vm, "(rs-big)"), "1152921504606846976", "{mode}");
        // Rust errors are catchable Scheme errors.
        assert_eq!(
            eval_str(&mut vm, "(guard (e (#t (error-object-message e))) (rs-parse \"x1\"))"),
            "\"rs-parse: invalid digit found in string\"",
            "{mode}"
        );
        // Wrong argument types are reported, not panics.
        let e = vm.eval_source("(rs-add \"no\" 1)").unwrap_err();
        assert!(e.msg.contains("expected"), "{mode}: {e}");
    }
}

#[test]
fn roots_survive_collection() {
    for (mode, mut vm) in vms() {
        let list = vm.eval_source("(map (lambda (i) (number->string i)) (iota 50))").unwrap();
        let root = vm.root(list);
        vm.eval_source("(define (churn n) (if (> n 0) (begin (make-vector 10 0) (churn (- n 1))))) (churn 20000)").unwrap();
        vm.full_collect();
        vm.collect();
        let back: Vec<String> = vm.get(root.get()).unwrap();
        assert_eq!(back.len(), 50, "{mode}");
        assert_eq!(back[49], "49", "{mode}");
    }
}

#[test]
fn weak_tables_lose_nursery_keys_in_a_minor_collection() {
    // Without stress, keys made since the last collection are in the
    // nursery: a minor collection finds the unreachable ones dead, values
    // referring to their key included, and keeps the others' values.
    let mut vm = Vm::new();
    vm.eval_source(
        "(define weak (make-weak-hash-table eqv?))
         (define kept (list 'kept))
         (collect-garbage)
         (do ((i 0 (+ i 1))) ((= i 100))
           (let ((k (vector i))) (hash-table-set! weak k (cons k i))))
         (hash-table-set! weak kept (vector kept))",
    )
    .unwrap();
    assert_eq!(eval_str(&mut vm, "(hash-table-count weak)"), "101");
    vm.collect();
    assert_eq!(eval_str(&mut vm, "(hash-table-count weak)"), "1");
    assert_eq!(eval_str(&mut vm, "(hash-table-ref weak kept)"), "#((kept))");
}

struct Tracked(Rc<Cell<usize>>);
impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn foreign_objects_are_finalized() {
    for (mode, mut vm) in vms() {
        let drops = Rc::new(Cell::new(0));
        let d = drops.clone();
        vm.register_fn("make-tracked", move || Foreign::new(Tracked(d.clone())));
        vm.register_fn("make-counter", || Foreign::new(RefCell::new(0i64)));
        vm.register_fn("bump!", |c: Foreign<RefCell<i64>>| {
            *c.borrow_mut() += 1;
            *c.borrow()
        });
        vm.eval_source("(define keep (make-tracked)) (define (spawn n) (if (> n 0) (begin (make-tracked) (spawn (- n 1))))) (spawn 100)")
            .unwrap();
        vm.full_collect();
        assert_eq!(drops.get(), 100, "{mode}: unreachable foreign objects are dropped");
        assert_eq!(eval_str(&mut vm, "(define c (make-counter)) (bump! c) (bump! c)"), "2", "{mode}");
        // Type mismatch between foreign types is an error.
        assert!(vm.eval_source("(bump! keep)").is_err(), "{mode}");
        vm.eval_source("(set! keep #f)").unwrap();
        vm.full_collect();
        assert_eq!(drops.get(), 101, "{mode}");
    }
}

#[test]
fn calling_scheme_from_rust() {
    for (mode, mut vm) in vms() {
        vm.eval_source("(define (scale k xs) (map (lambda (x) (* k x)) xs))").unwrap();
        let xs = vm.to_value(vec![1i64, 2, 3]).unwrap();
        let v = vm.call_global("scale", &[Value::int_unchecked(10), xs]).unwrap();
        let out: Vec<i64> = vm.get(v).unwrap();
        assert_eq!(out, vec![10, 20, 30], "{mode}");
        // Callbacks stored in Rust (as Roots) and invoked from a native.
        let hooks: Rc<RefCell<Vec<Root>>> = Rc::default();
        let h = hooks.clone();
        vm.register_fn("add-hook!", move |f: Root| h.borrow_mut().push(f));
        let h = hooks.clone();
        vm.register_fn_vm("run-hooks", move |vm: &mut Vm, arg: i64| -> Result<Vec<i64>, String> {
            let fs: Vec<Root> = h.borrow().clone();
            fs.iter()
                .map(|f| {
                    vm.call(f.get(), &[Value::int_unchecked(arg)])
                        .map_err(|e| e.into_inner().msg)
                        .and_then(|v| vm.get(v).map_err(|e| e.into_inner().msg))
                })
                .collect()
        });
        vm.eval_source("(add-hook! (lambda (x) (+ x 1))) (add-hook! (lambda (x) (length (make-list x 'a))))").unwrap();
        assert_eq!(eval_str(&mut vm, "(run-hooks 5)"), "(6 5)", "{mode}");
        // Scheme errors come back to Rust with the raised object.
        let e = vm.call_global("car", &[Value::int_unchecked(1)]).unwrap_err();
        assert!(e.msg.contains("car"), "{mode}: {e}");
        let e = vm.eval_source("(raise 'custom)").unwrap_err();
        assert_eq!(techne_vm::builtins::repr(e.into_inner().payload.unwrap().get()), "custom", "{mode}");
    }
}

#[test]
fn jit_native_callbacks() {
    for (mode, mut vm) in vms() {
        vm.set_jit(Some(1));
        vm.register_fn_vm("call-with-int", |vm: &mut Vm, f: Root, x: i64| -> Result<i64, String> {
            let v = vm.call(f.get(), &[Value::int_unchecked(x)]).map_err(|e| e.into_inner().msg)?;
            vm.get(v).map_err(|e| e.into_inner().msg)
        });
        // The callback's recursion grows (and moves) the register stack while
        // the compiled loop waits for the native to return.
        let src = "(define (deep n) (if (= n 0) 0 (+ 1 (deep (- n 1)))))
            (let loop ((i 0) (acc 0)) (if (< i 3) (loop (+ i 1) (+ acc (call-with-int deep (* i 100000)))) acc))";
        assert_eq!(eval_str(&mut vm, src), "300000", "{mode}");
    }
}

/// A future completed by an external producer (as an I/O library would).
#[derive(Clone)]
struct Oneshot {
    state: std::sync::Arc<std::sync::Mutex<(Option<i64>, Option<std::task::Waker>)>>,
}

impl Oneshot {
    fn new() -> Self {
        Self { state: std::sync::Arc::new(std::sync::Mutex::new((None, None))) }
    }

    fn complete(&self, value: i64) {
        let wake = {
            let mut state = self.state.lock().unwrap();
            state.0 = Some(value);
            state.1.take()
        };
        if let Some(wake) = wake {
            wake.wake();
        }
    }
}

impl std::future::Future for Oneshot {
    type Output = Result<i64, String>;
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let mut s = self.state.lock().unwrap();
        match s.0.take() {
            Some(v) => std::task::Poll::Ready(Ok(v)),
            None => {
                s.1 = Some(cx.waker().clone());
                std::task::Poll::Pending
            }
        }
    }
}

fn delayed(value: i64, ms: u64) -> Oneshot {
    let future = Oneshot::new();
    let producer = future.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        producer.complete(value);
    });
    future
}

#[test]
fn async_natives_and_tasks() {
    for (mode, mut vm) in vms() {
        let started = Rc::new(Cell::new(0));
        let calls = started.clone();
        let pending = RefCell::new(Vec::new());
        vm.register_async("fetch", 2, move |vm: &mut Vm, args: &[Value]| {
            let (v, ms): (i64, i64) = (vm.get(args[0]).unwrap(), vm.get(args[1]).unwrap());
            if calls.get() >= 3 {
                return delayed(v, ms as u64);
            }
            let future = Oneshot::new();
            let mut pending = pending.borrow_mut();
            pending.push((v, future.clone()));
            calls.set(calls.get() + 1);
            if calls.get() == 3 {
                for (value, producer) in pending.drain(..) {
                    producer.complete(value);
                }
            }
            future
        });
        // None of the first three fetches can finish until all have started:
        // completing this program proves overlap, independent of machine load.
        // A generous watchdog detects deadlock, not a performance regression.
        let interrupt = vm.interrupt_handle();
        let (cancel, deadline) = std::sync::mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            if matches!(deadline.recv_timeout(std::time::Duration::from_secs(20)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
                interrupt.interrupt();
            }
        });
        let result = vm.eval_source("(define ts (map (lambda (i) (spawn (lambda () (* 10 (fetch i 0))))) '(0 1 2))) (map task-join ts)");
        let _ = cancel.send(());
        watchdog.join().unwrap();
        let v = result.unwrap();
        assert_eq!(techne_vm::builtins::repr(v), "(0 10 20)", "{mode}");
        assert_eq!(started.get(), 3, "{mode}: three fetches must be pending together");
        // From the main program the call blocks while other tasks run.
        assert_eq!(eval_str(&mut vm, "(fetch 7 5)"), "7", "{mode}");
        // Tasks spawned from Rust.
        let f = vm.eval_source("(lambda () (sleep 2) (string-append \"from \" \"rust\"))").unwrap();
        let id = vm.spawn(f);
        vm.run_tasks().unwrap();
        let s: String = vm.get(vm.task_result(id).unwrap().unwrap()).unwrap();
        assert_eq!(s, "from rust", "{mode}");
    }
}

/// Interrupt `vm`'s evaluation of `src` from another thread after `ms`.
fn eval_interrupted(vm: &mut Vm, src: &str, ms: u64) -> Result<String, techne_vm::vm::Error> {
    let handle = vm.interrupt_handle();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        handle.interrupt();
    });
    let result = vm.eval_source(src).map(techne_vm::builtins::repr);
    t.join().unwrap();
    result
}

#[test]
fn interrupts() {
    for (mode, mut vm) in vms() {
        for jit in [None, Some(1)] {
            vm.set_jit(jit);
            let what = format!("{mode}, jit {jit:?}");
            // Endless loops, interpreted or compiled, in the main program.
            for src in [
                "(let loop ((i 0)) (loop (+ i 1)))",
                "(define (ping n) (pong (+ n 1))) (define (pong n) (ping (+ n 1))) (ping 0)",
                "(define (spin) (let loop ((x 0.5)) (if (< x 2.0) (loop (* x 1.0)) x))) (spin)",
                "(sleep 100000)",
            ] {
                let start = std::time::Instant::now();
                let e = eval_interrupted(&mut vm, src, 30).unwrap_err();
                assert!(e.is_interrupt(), "{what}: {src}: {e}");
                // Delivered promptly (30 ms until the interrupt, generous margin).
                let latency = start.elapsed().saturating_sub(std::time::Duration::from_millis(30));
                assert!(latency < std::time::Duration::from_millis(100), "{what}: {src}: {latency:?}");
            }
            // The condition is catchable, and the VM keeps working.
            let caught = eval_interrupted(&mut vm, "(guard (e (#t (condition/report-string e))) (let loop () (loop)))", 30);
            assert_eq!(caught.unwrap(), "\"interrupted\"", "{what}");
            assert_eq!(eval_str(&mut vm, "(+ 1 2)"), "3", "{what}");
        }
    }
}

#[test]
fn host_driven_scheduling() {
    use std::time::{Duration, Instant};
    use techne_vm::tasks::Progress;
    for (mode, mut vm) in vms() {
        // A CPU-bound task does not keep the host waiting.
        let busy = vm.eval_source("(lambda () (let loop ((i 0)) (if (< i 2000000000) (loop (+ i 1)) i)))").unwrap();
        let id = vm.spawn(busy);
        let start = Instant::now();
        assert_eq!(vm.run_tasks_for(Duration::from_millis(2)), Progress::OutOfTime, "{mode}");
        assert!(start.elapsed() < Duration::from_millis(100), "{mode}: {:?}", start.elapsed());
        vm.cancel_task(id).unwrap();
        assert_eq!(vm.run_tasks_for(Duration::from_millis(50)), Progress::Finished, "{mode}");
        assert!(vm.task_result(id).unwrap().unwrap_err().is_cancellation(), "{mode}");
        // A sleeping task: blocked, with a timer the host can wait for.
        let sleeper = vm.eval_source("(lambda () (sleep 20) 'woke)").unwrap();
        let id = vm.spawn(sleeper);
        assert_eq!(vm.run_tasks_for(Duration::from_millis(50)), Progress::Blocked, "{mode}");
        let timer = vm.next_timer().expect("sleeping task has a timer");
        std::thread::sleep(timer.saturating_duration_since(Instant::now()));
        assert_eq!(vm.run_tasks_for(Duration::from_millis(50)), Progress::Finished, "{mode}");
        assert_eq!(techne_vm::builtins::repr(vm.task_result(id).unwrap().unwrap()), "woke", "{mode}");
        // A Rust future completed by another thread notifies the host.
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        vm.set_wake_notifier(move || {
            let _ = tx.lock().unwrap().send(());
        });
        vm.register_async("fetch", 2, |vm: &mut Vm, args: &[Value]| {
            let (v, ms): (i64, i64) = (vm.get(args[0]).unwrap(), vm.get(args[1]).unwrap());
            delayed(v, ms as u64)
        });
        let fetcher = vm.eval_source("(lambda () (* 2 (fetch 21 10)))").unwrap();
        let id = vm.spawn(fetcher);
        assert_eq!(vm.run_tasks_for(Duration::from_millis(5)), Progress::Blocked, "{mode}");
        rx.recv_timeout(Duration::from_secs(5)).expect("wake notification");
        assert_eq!(vm.run_tasks_for(Duration::from_millis(50)), Progress::Finished, "{mode}");
        assert_eq!(techne_vm::builtins::repr(vm.task_result(id).unwrap().unwrap()), "42", "{mode}");
    }
}

#[test]
fn incremental_collection_bounds_pauses() {
    let mut vm = Vm::new();
    // About 100 MB live in the old generation, then garbage to collect.
    vm.eval_source("(define keep (let loop ((i 0) (acc '())) (if (< i 2000000) (loop (+ i 1) (cons (make-vector 2 i) acc)) acc)))")
        .unwrap();
    vm.eval_source("(let loop ((i 0) (acc '())) (if (< i 20000000) (loop (+ i 1) (if (= 0 (modulo i 1000)) '() (cons i acc))) 0))")
        .unwrap();
    let stats = vm.heap.stats.clone();
    assert!(stats.full >= 1, "an old-generation cycle ran incrementally: {stats:?}");
    // No single pause marked more than a fraction of the live heap.
    let live = vm.heap.old_words();
    assert!(stats.max_slice_words * 4 < live, "{} words marked in one pause, {live} live", stats.max_slice_words);
    assert_eq!(eval_str(&mut vm, "(length keep)"), "2000000");
}

#[test]
fn promotion_counts_copied_words_while_sweeping() {
    let mut vm = Vm::new();
    // Collect on every allocation with tiny slices, so blocks are still
    // unswept when promotion needs room in the old generation.
    vm.heap.stress = true;
    // Objects that survive a minor collection and then die there.
    vm.eval_source(
        "(let loop ((i 0) (window '())) \
           (when (< i 20000) (loop (+ i 1) (if (= 0 (modulo i 500)) '() (cons (make-vector 3 i) window)))))",
    )
    .unwrap();
    let stats = &vm.heap.stats;
    assert!(stats.full >= 2, "old-generation cycles ran: {stats:?}");
    // A minor collection promotes at most what the nursery held.
    assert!(stats.max_copied_words <= vm.heap.nursery_capacity(), "{} words promoted at once", stats.max_copied_words);
}

#[test]
fn nursery_window_bounds_minor_collections() {
    for window in [8usize << 20, 1 << 20] {
        let mut vm = Vm::new();
        vm.set_nursery_window(window);
        // Everything allocated survives: the worst case for a minor collection.
        vm.eval_source("(define keep (let loop ((i 0) (acc '())) (if (< i 1000000) (loop (+ i 1) (cons (make-vector 2 i) acc)) acc)))")
            .unwrap();
        let stats = &vm.heap.stats;
        assert!(stats.minor > 0);
        assert!(stats.max_copied_words * 8 <= window, "copied {} words with a {window}-byte window", stats.max_copied_words);
        assert!(stats.max_copied_words * 8 > window / 2, "the window is used: {} words", stats.max_copied_words);
    }
}

/// Two file modules that define the same names.
fn two_modules(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("techne-modules-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.scm");
    let b = dir.join("b.scm");
    std::fs::write(&a, "(define (greet) \"Greeting of a.\" \"a\") (define x 1)").unwrap();
    std::fs::write(&b, "(define (greet) \"b\") (define x 2)").unwrap();
    (a.canonicalize().unwrap(), b.canonicalize().unwrap())
}

#[test]
fn evaluation_in_modules() {
    for (mode, mut vm) in vms() {
        let (a_path, b_path) = two_modules(mode);
        let (a_name, b_name) = (a_path.to_str().unwrap(), b_path.to_str().unwrap());
        let a = vm.find_module(a_name).unwrap();
        let b = vm.find_module(b_name).unwrap();
        assert_eq!(vm.find_module(a_name).unwrap(), a, "{mode}: loaded once");
        let eval_in = |vm: &mut Vm, m: u32, src: &str| {
            let mut m = m;
            let v = vm.eval_interactive(&mut m, "<test>", src).unwrap_or_else(|e| panic!("{mode} {src}: {e}"));
            techne_vm::builtins::repr(v)
        };
        assert_eq!(eval_in(&mut vm, a, "(greet)"), "\"a\"");
        assert_eq!(eval_in(&mut vm, b, "(greet)"), "\"b\"");
        // Redefining in one module leaves the other, and user, alone.
        eval_in(&mut vm, a, "(define (greet) \"a2\") (set! x 10)");
        assert_eq!(eval_in(&mut vm, b, "(list (greet) x)"), "(\"b\" 2)");
        assert_eq!(eval_in(&mut vm, a, "(list (greet) x)"), "(\"a2\" 10)");
        assert!(vm.eval_source("(greet)").is_err(), "{mode}: greet is not in user");
        // eval: in a named module, or by default the current one.
        assert_eq!(eval_str(&mut vm, &format!("(eval '(greet) {b_name:?})")), "\"b\"");
        assert_eq!(eval_in(&mut vm, a, "(eval '(greet))"), "\"a2\"");
        // help, completion and lookup follow the module.
        assert!(eval_in(&mut vm, b, "(%describe 'greet)").contains("b.scm"), "{mode}");
        assert!(vm.global_names(a).iter().any(|n| &**n == "greet"));
        assert!(!vm.global_names(techne_vm::vm::USER_MODULE).iter().any(|n| &**n == "greet"));
        assert_eq!(vm.get_global_in(b, "x").map(techne_vm::builtins::repr).as_deref(), Some("2"));
        // in-module switches the REPL's module for later evaluations.
        let mut m = techne_vm::vm::USER_MODULE;
        vm.eval_interactive(&mut m, "<test>", &format!("(in-module {b_name:?})")).unwrap();
        assert_eq!(m, b, "{mode}");
        assert_eq!(eval_in(&mut vm, m, "(list (greet) (current-module))"), format!("(\"b\" {b_name:?})"));
        // A module that names no file is an error.
        assert!(vm.find_module("no/such/module.scm").is_err());
    }
}

#[test]
fn restricted_worlds() {
    use techne_vm::vm::{Capability, Grants};
    let dir = std::env::temp_dir().join(format!("techne-worlds-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let secret = dir.join("secret.txt");
    std::fs::write(&secret, "secret").unwrap();
    std::fs::write(dir.join("m.scm"), "(define x 1)").unwrap();
    let (secret, module) = (secret.to_str().unwrap().to_owned(), dir.join("m.scm").to_str().unwrap().to_owned());

    let mut vm = Vm::with_grants(Grants::NONE);
    let refused = [
        format!("(open-input-file {secret:?})"),
        format!("(define (slurp p) (read-line (open-input-file p))) (slurp {secret:?})"),
        format!("(file->string {secret:?})"),
        format!("(read-lines {secret:?})"),
        format!("(file-exists? {secret:?})"),
        format!("(open-output-file {secret:?})"),
        "(getenv \"HOME\")".into(),
        "(command-line)".into(),
        "(exit 3)".into(),
        format!("(require {module:?})"),
        format!("(include {module:?})"),
        format!("(define-library (l) (include {module:?}))"),
        format!("(eval 1 {module:?})"),
        format!("(in-module {module:?})"),
        // Through eval, or a procedure value passed along: still refused.
        "(eval '(getenv \"HOME\"))".into(),
        format!("((lambda (f) (f {secret:?})) open-input-file)"),
    ];
    for src in &refused {
        let e = vm.eval_source(src).expect_err(src);
        assert!(e.msg.contains("not granted"), "{src}: {e}");
    }
    // A refusal is an ordinary condition; pure computation works.
    assert_eq!(eval_str(&mut vm, "(guard (e (#t 'refused)) (getenv \"HOME\"))"), "refused");
    assert_eq!(eval_str(&mut vm, "(map (lambda (x) (* x x)) '(1 2 3))"), "(1 4 9)");
    assert_eq!(std::fs::read_to_string(dir.join("secret.txt")).unwrap(), "secret", "not truncated");

    // Grants are separate: files without the environment.
    let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Files));
    assert_eq!(eval_str(&mut vm, &format!("(file->string {secret:?})")), "\"secret\"");
    assert!(vm.eval_source("(getenv \"HOME\")").unwrap_err().msg.contains("needs environment"));
    assert!(vm.grants().has(Capability::Files) && !vm.grants().has(Capability::Loading));
}
