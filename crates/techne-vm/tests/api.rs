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
