//! Native panics are contained: they poison the world and surface as an
//! uncatchable error instead of unwinding out of `extern "C"` entries
//! (which aborts the process) or out of the interpreter. If containment
//! regresses, these tests do not fail — the test binary aborts.

use std::{cell::Cell, rc::Rc};

use techne_vm::vm::Vm;

/// A panicking stateless native (the `PlainFn` fast path compiled code
/// calls directly), in the interpreter, under synchronous JIT compilation
/// and under a low background threshold. Under JIT the caller is a
/// defined function warmed first, and a live arena is asserted: with
/// synchronous compilation (threshold 1) that very function's warm
/// call compiled it, deterministically; in the background case the
/// arena at least proves the compiler pipeline ran for this hot
/// function rather than the test silently covering the interpreter.
fn stateless_panic(jit: Option<u32>) {
    let mut vm = Vm::new();
    vm.set_jit(jit);
    vm.register_fn("boom", |x: i64| -> i64 {
        if x < 0 {
            panic!("guest-triggered boom");
        }
        x + 1
    });
    let program = if jit.is_some() {
        vm.eval_source("(define (run n k) (do ((i 0 (+ i 1))) ((= i n) (boom k)) (boom i)))").unwrap();
        for _ in 0..500 {
            vm.eval_source("(run 50 0)").unwrap();
            if vm.live_jit_arenas() > 0 {
                break;
            }
        }
        assert!(vm.live_jit_arenas() > 0, "caller never compiled");
        "(run 50 -1)"
    } else {
        "(boom -1)"
    };
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
    stateless_panic(None);
}

#[test]
fn stateless_native_panic_jit_sync() {
    stateless_panic(Some(1));
}

#[test]
fn stateless_native_panic_jit_background() {
    stateless_panic(Some(5));
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
    vm.eval_source("(define (run n k) (do ((i 0 (+ i 1))) ((= i n) (boom k)) (boom i)))").unwrap();
    vm.eval_source("(run 20 0)").unwrap();
    assert!(vm.live_jit_arenas() > 0, "caller never compiled");
    let error = vm.eval_source("(run 20 -1)").unwrap_err();
    assert!(error.is_poison(), "{error:?}");
    assert!(calls.get() > 40);
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

/// A native that reenters the VM, poisons it in there and swallows the
/// error does not get guest execution resumed after it: poison
/// dominates a successful native return, in both engines.
#[test]
fn reentrant_native_cannot_swallow_poison() {
    for jit in [None, Some(1)] {
        let mut vm = Vm::new();
        vm.set_jit(jit);
        vm.register_fn("boom", |_: i64| -> i64 { panic!("nested boom") });
        vm.register_fn_vm("swallow", |vm: &mut Vm| -> i64 {
            let _ = vm.eval_source("(boom 0)");
            0
        });
        let error = vm.eval_source("(define changed #f) (swallow) (set! changed #t) changed").unwrap_err();
        assert!(error.is_poison(), "{error:?}");
        assert!(vm.poisoned().is_some());
    }
}

/// Poison that originates in a `dynamic-wind` after-thunk while an
/// ordinary error unwinds replaces that error: the guard it unwinds
/// to must not run.
#[test]
fn poison_in_cleanup_replaces_the_error_unwinding() {
    let mut vm = Vm::new();
    vm.set_jit(None);
    vm.register_fn("boom", |_: i64| -> i64 { panic!("cleanup boom") });
    let handled = Rc::new(Cell::new(false));
    let mark = handled.clone();
    vm.register_fn("mark!", move || {
        mark.set(true);
        true
    });
    let error = vm
        .eval_source(
            "(guard (e (#t (begin (mark!) 42))) \
               (dynamic-wind (lambda () #f) (lambda () (error \"ordinary failure\")) (lambda () (boom 0))))",
        )
        .unwrap_err();
    assert!(error.is_poison(), "{error:?}");
    assert!(!handled.get(), "guard ran on a poisoned VM");
}

/// A registered future that panics when polled by the scheduler (and
/// its converter likewise) is contained there: the world is poisoned
/// and `run_tasks` reports it, instead of the panic unwinding out of
/// the scheduler with the task left half-moved.
#[test]
fn async_future_panic_is_contained_in_the_scheduler() {
    use std::{future::poll_fn, task::Poll};
    let mut vm = Vm::new();
    vm.set_jit(None);
    vm.register_async("later-boom", 0, |_, _| {
        let polls = Rc::new(Cell::new(0));
        poll_fn(move |cx| -> Poll<Result<i64, String>> {
            polls.set(polls.get() + 1);
            if polls.get() == 1 {
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                panic!("future boom")
            }
        })
    });
    vm.eval_source("(spawn (lambda () (later-boom)))").unwrap();
    let error = vm.run_tasks().unwrap_err();
    assert!(error.is_poison(), "{error:?}");
    assert!(error.msg.contains("future boom"), "{}", error.msg);
    assert!(vm.poisoned().is_some());
}

/// A registered future whose *destructor* panics (its poll completed
/// normally) is contained where the satisfied wait is dropped: the
/// world is poisoned, the task does not resume, and nothing unwinds
/// out of the scheduler. A panicking result converter likewise.
#[test]
fn async_future_destructor_and_converter_panics_are_contained() {
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll},
    };
    use techne_vm::{api::IntoValue, value::Value, vm::Error};

    struct NastyDrop {
        polled: bool,
    }
    impl Future for NastyDrop {
        type Output = Result<i64, String>;
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.polled {
                Poll::Ready(Ok(7))
            } else {
                self.polled = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
    impl Drop for NastyDrop {
        fn drop(&mut self) {
            panic!("future destructor");
        }
    }
    struct BadConvert;
    impl IntoValue for BadConvert {
        fn into_value(self, _: &mut Vm) -> Result<Value, Error> {
            panic!("converter boom")
        }
    }

    // Destructor panic, scheduler-driven: the guest continuation of
    // the await must not run (it would call mark!).
    let mut vm = Vm::new();
    vm.set_jit(None);
    let ran = Rc::new(Cell::new(false));
    let mark = ran.clone();
    vm.register_fn("mark!", move || {
        mark.set(true);
        true
    });
    vm.register_async("nasty-drop", 0, |_, _| NastyDrop { polled: false });
    vm.eval_source("(spawn (lambda () (nasty-drop) (mark!)))").unwrap();
    let error = vm.run_tasks().unwrap_err();
    assert!(error.is_poison(), "{error:?}");
    assert!(error.msg.contains("future destructor"), "{}", error.msg);
    assert!(!ran.get(), "task resumed on a poisoned VM");

    // Destructor panic at world teardown: dropping the VM with the
    // future still waited on does not unwind out of `drop`.
    let mut vm = Vm::new();
    vm.set_jit(None);
    vm.register_async("nasty-drop", 0, |_, _| NastyDrop { polled: false });
    vm.eval_source("(spawn (lambda () (nasty-drop)))").unwrap();
    drop(vm);

    // Converter panic.
    let mut vm = Vm::new();
    vm.set_jit(None);
    vm.register_async("bad-convert", 0, |_, _| std::future::ready(Ok::<BadConvert, String>(BadConvert)));
    vm.eval_source("(spawn (lambda () (bad-convert)))").unwrap();
    let error = vm.run_tasks().unwrap_err();
    assert!(error.is_poison(), "{error:?}");
    assert!(error.msg.contains("converter boom"), "{}", error.msg);
}

/// `run_tasks_for` reports a poisoning round as `Finished`, even when
/// the budget is already spent: `OutOfTime` must not mask poison.
#[test]
fn scheduler_progress_does_not_mask_poison() {
    use techne_vm::tasks::Progress;
    let mut vm = Vm::new();
    vm.set_jit(None);
    vm.register_async("poll-boom", 0, |_, _| std::future::poll_fn(|_| -> std::task::Poll<Result<i64, String>> { panic!("poll boom") }));
    vm.eval_source("(spawn (lambda () (poll-boom)))").unwrap();
    let progress = vm.run_tasks_for(std::time::Duration::ZERO);
    assert_eq!(progress, Progress::Finished);
    assert!(vm.poisoned().is_some());
}

/// A panic payload whose destructor panics does not unwind out of
/// containment (across the `extern "C"` entry it would abort): the
/// payload is dropped under protection and leaked if even that fails.
#[test]
fn payload_with_panicking_destructor_is_contained() {
    struct Nasty;
    impl Drop for Nasty {
        fn drop(&mut self) {
            panic!("payload destructor");
        }
    }
    // Both through the interpreter and through compiled `PlainFn` code.
    for jit in [None, Some(1)] {
        let mut vm = Vm::new();
        vm.set_jit(jit);
        vm.register_fn("boom", |x: i64| -> i64 {
            if x < 0 {
                std::panic::panic_any(Nasty);
            }
            x + 1
        });
        if jit.is_some() {
            vm.eval_source("(define (run n k) (do ((i 0 (+ i 1))) ((= i n) (boom k)) (boom i)))").unwrap();
            vm.eval_source("(run 20 0)").unwrap();
            assert!(vm.live_jit_arenas() > 0, "caller never compiled");
        }
        let error = vm.eval_source(if jit.is_some() { "(run 20 -1)" } else { "(boom -1)" }).unwrap_err();
        assert!(error.is_poison(), "{error:?}");
        assert!(vm.poisoned().is_some());
    }
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
