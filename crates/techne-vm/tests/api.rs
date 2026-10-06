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
        vm.eval_source("(define keep (make-tracked)) (define (spawn n) (if (> n 0) (begin (make-tracked) (spawn (- n 1))))) (spawn 100)").unwrap();
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
                .map(|f| vm.call(f.get(), &[Value::int_unchecked(arg)]).map_err(|e| e.msg).and_then(|v| vm.get(v).map_err(|e| e.msg)))
                .collect()
        });
        vm.eval_source("(add-hook! (lambda (x) (+ x 1))) (add-hook! (lambda (x) (length (make-list x 'a))))").unwrap();
        assert_eq!(eval_str(&mut vm, "(run-hooks 5)"), "(6 5)", "{mode}");
        // Scheme errors come back to Rust with the raised object.
        let e = vm.call_global("car", &[Value::int_unchecked(1)]).unwrap_err();
        assert!(e.msg.contains("car"), "{mode}: {e}");
        let e = vm.eval_source("(raise 'custom)").unwrap_err();
        assert_eq!(techne_vm::builtins::repr(e.payload.unwrap().get()), "custom", "{mode}");
    }
}

#[test]
fn jit_native_callbacks() {
    for (mode, mut vm) in vms() {
        vm.set_jit(Some(1));
        vm.register_fn_vm("call-with-int", |vm: &mut Vm, f: Root, x: i64| -> Result<i64, String> {
            let v = vm.call(f.get(), &[Value::int_unchecked(x)]).map_err(|e| e.msg)?;
            vm.get(v).map_err(|e| e.msg)
        });
        // The callback's recursion grows (and moves) the register stack while
        // the compiled loop waits for the native to return.
        let src = "(define (deep n) (if (= n 0) 0 (+ 1 (deep (- n 1)))))
            (let loop ((i 0) (acc 0)) (if (< i 3) (loop (+ i 1) (+ acc (call-with-int deep (* i 100000)))) acc))";
        assert_eq!(eval_str(&mut vm, src), "300000", "{mode}");
    }
}

/// A future completed by another OS thread (as an I/O library would).
struct Oneshot {
    state: std::sync::Arc<std::sync::Mutex<(Option<i64>, Option<std::task::Waker>)>>,
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
    let state = std::sync::Arc::new(std::sync::Mutex::new((None, None::<std::task::Waker>)));
    let s = state.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        let mut g = s.lock().unwrap();
        g.0 = Some(value);
        if let Some(w) = g.1.take() {
            w.wake();
        }
    });
    Oneshot { state }
}

#[test]
fn async_natives_and_tasks() {
    for (mode, mut vm) in vms() {
        vm.register_async("fetch", 2, |vm: &mut Vm, args: &[Value]| {
            let (v, ms): (i64, i64) = (vm.get(args[0]).unwrap(), vm.get(args[1]).unwrap());
            delayed(v, ms as u64)
        });
        // Three tasks wait concurrently: total time is about the longest wait.
        let start = std::time::Instant::now();
        let v = vm
            .eval_source("(define ts (map (lambda (i) (spawn (lambda () (* 10 (fetch i (* 30 (- 3 i))))))) '(0 1 2))) (map task-join ts)")
            .unwrap();
        assert_eq!(techne_vm::builtins::repr(v), "(0 10 20)", "{mode}");
        // Waits of 90, 60 and 30 ms: sequential would take 180 ms.
        assert!(start.elapsed() < std::time::Duration::from_millis(150), "{mode}: waits must overlap ({:?})", start.elapsed());
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
        let mut eval_in = |vm: &mut Vm, m: u32, src: &str| {
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
