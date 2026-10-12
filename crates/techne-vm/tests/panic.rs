//! Native panics are contained: they poison the world and surface as an
//! uncatchable error instead of unwinding out of `extern "C"` entries
//! (which aborts the process) or out of the interpreter. If containment
//! regresses, these tests do not fail — the test binary aborts.

use std::{cell::Cell, rc::Rc};

use techne_vm::vm::Vm;

/// A panicking stateless native (the `PlainFn` fast path compiled code
/// calls directly), in the interpreter, under synchronous JIT compilation
/// and under a low background threshold.
fn stateless_panic(jit: Option<u32>, program: &str) {
    let mut vm = Vm::new();
    vm.set_jit(jit);
    vm.register_fn("boom", |x: i64| -> i64 {
        if x < 0 {
            panic!("guest-triggered boom");
        }
        x + 1
    });
    let error = vm.eval_source(program).unwrap_err();
    assert!(error.is_poison(), "{error:?}");
    assert!(error.msg.contains("guest-triggered boom"), "{}", error.msg);
    assert_eq!(vm.poisoned(), Some("native panicked: guest-triggered boom"));
    // The world is retired: nothing runs on it again.
    let error = vm.eval_source("(+ 1 2)").unwrap_err();
    assert!(error.is_poison(), "{error:?}");
}

#[test]
fn stateless_native_panic_interpreter() {
    stateless_panic(None, "(boom -1)");
}

#[test]
fn stateless_native_panic_jit_sync() {
    // Threshold 1 compiles the caller synchronously; compiled code calls
    // the native's `PlainFn` itself.
    stateless_panic(Some(1), "(do ((i 0 (+ i 1))) ((= i 50) (boom -1)) (boom i))");
}

#[test]
fn stateless_native_panic_jit_background() {
    stateless_panic(Some(5), "(do ((i 0 (+ i 1))) ((= i 500) (boom -1)) (boom i))");
}

/// A capturing (boxed) native panicking when called from compiled code
/// goes through `jit_call_slow` -> `call_native`.
#[test]
fn boxed_native_panic_from_jit() {
    let mut vm = Vm::new();
    vm.set_jit(Some(1));
    let calls = Rc::new(Cell::new(0i64));
    let seen = calls.clone();
    vm.register_fn("boom", move |x: i64| -> i64 {
        seen.set(seen.get() + 1);
        if x < 0 {
            panic!("boxed boom");
        }
        x
    });
    let error = vm.eval_source("(do ((i 0 (+ i 1))) ((= i 20) (boom -1)) (boom i))").unwrap_err();
    assert!(error.is_poison(), "{error:?}");
    assert!(calls.get() > 20);
}

/// The poison error reaches the host past guest handlers, and
/// `dynamic-wind` after-thunks (guest code) do not run on the poisoned VM.
#[test]
fn poison_is_uncatchable_and_skips_winds() {
    let mut vm = Vm::new();
    vm.set_jit(None);
    vm.register_fn("boom", |_: i64| -> i64 { panic!("wind boom") });
    let wound = Rc::new(Cell::new(false));
    let mark = wound.clone();
    vm.register_fn("mark!", move || {
        mark.set(true);
        true
    });
    let error = vm
        .eval_source(
            "(guard (e (#t 'caught)) \
               (dynamic-wind (lambda () #f) (lambda () (boom 1)) (lambda () (mark!))))",
        )
        .unwrap_err();
    assert!(error.is_poison(), "{error:?}");
    assert!(!wound.get(), "after-thunk ran on a poisoned VM");
}

/// Natives that do not panic are unaffected, in both engines; and one
/// world poisoning does not affect another world on the same thread.
#[test]
fn unaffected_natives_and_worlds() {
    for jit in [None, Some(1)] {
        let mut vm = Vm::new();
        vm.set_jit(jit);
        vm.register_fn("inc", |x: i64| x + 1);
        let v = vm.eval_source("(do ((i 0 (inc i))) ((= i 100) i))").unwrap();
        assert_eq!(v.as_int(), 100);
        assert_eq!(vm.poisoned(), None);
    }
    let mut bad = Vm::new();
    bad.register_fn("boom", |_: i64| -> i64 { panic!("only this world") });
    assert!(bad.eval_source("(boom 0)").unwrap_err().is_poison());
    let mut good = Vm::new();
    assert_eq!(good.eval_source("(+ 20 22)").unwrap().as_int(), 42);
}

/// A panic while a task runs poisons the world; the scheduler refuses to
/// run anything further instead of carrying on with other tasks.
#[test]
fn panic_in_task_stops_scheduler() {
    let mut vm = Vm::new();
    vm.set_jit(None);
    vm.register_fn("boom", |_: i64| -> i64 { panic!("task boom") });
    let ran = Rc::new(Cell::new(false));
    let mark = ran.clone();
    vm.register_fn("mark!", move || {
        mark.set(true);
        true
    });
    vm.eval_source("(spawn (lambda () (boom 1))) (spawn (lambda () (mark!)))").unwrap();
    let error = vm.run_tasks().unwrap_err();
    assert!(error.is_poison(), "{error:?}");
    assert!(vm.poisoned().is_some());
    assert!(!ran.get(), "second task ran on a poisoned VM");
}
