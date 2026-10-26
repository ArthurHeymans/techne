//! Host entry owns one scheduler; Scheme inputs run on resumable task frames.
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    task::Poll,
    time::Duration,
};

use techne_vm::{
    api::{IntoValue, Value},
    vm::{Error, Stop, Vm},
};

#[test]
fn inputs_keep_their_task_identity_across_waits_and_collection() {
    for bare in [false, true] {
        for jit in [None, Some(1)] {
            let mut vm = if bare { Vm::bare() } else { Vm::new() };
            vm.set_jit(jit);
            vm.set_gc_stress(true, true);
            let value = vm
                .eval_source(
                    "(define input (current-task))
                 (define before (task-done? input))
                 (yield)
                 (collect-garbage)
                 (list before (eq? input (current-task)))",
                )
                .unwrap();
            assert_eq!(vm.repr(&value), "(#f #t)", "bare={bare} jit={jit:?}");
            drop(value);
            let value = vm.eval_source("(task-done? input)").unwrap();
            assert_eq!(vm.repr(&value), "#t");
            let f = vm.eval_source("(lambda () (let ((here (current-task))) (yield) (eq? here (current-task))))").unwrap();
            let value = vm.call_input(&f, &[]).unwrap();
            assert_eq!(vm.repr(&value), "#t");
            drop((value, f));
            vm.snapshot().unwrap();
        }
    }
}

#[test]
fn a_host_call_can_await_a_native_entry() {
    for jit in [None, Some(1)] {
        let mut vm = Vm::new();
        vm.set_jit(jit);
        let polls = Rc::new(Cell::new(0));
        let count = polls.clone();
        vm.register_async("later", 0, move |_, _| {
            let count = count.clone();
            std::future::poll_fn(move |cx| {
                let n = count.get() + 1;
                count.set(n);
                if n == 1 {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    Poll::Ready(Ok::<i64, String>(42))
                }
            })
        });
        let f = vm.get_global("later").unwrap();
        let value = vm.call(&f, &[]).unwrap();
        assert_eq!(vm.repr(&value), "42");
        assert_eq!(polls.get(), 2);
    }
}

#[test]
fn guest_drainers_are_passive_and_ignore_other_tasks_failures() {
    for jit in [None, Some(1)] {
        let mut vm = Vm::new();
        vm.set_jit(jit);
        let value = vm
            .eval_source(
                "(define log '())
             (define a (spawn (lambda () (run-tasks) (set! log (cons 'a log)))))
             (define b (spawn (lambda () (run-tasks) (set! log (cons 'b log)))))
             (spawn (lambda () (sleep 1) (set! log (cons 'work log))))
             (spawn (lambda () (error \"ordinary failure\")))
             (run-tasks)
             (car (reverse log))",
            )
            .unwrap();
        assert_eq!(vm.repr(&value), "work");
        vm.run_tasks().unwrap();
        let value = vm.eval_source("(list (task-done? a) (task-done? b) (length log))").unwrap();
        assert_eq!(vm.repr(&value), "(#t #t 3)");
    }
}

#[test]
fn an_input_borrows_its_hosts_execution_and_finishes_before_background_work() {
    let mut vm = Vm::new();
    let observations = Rc::new(RefCell::new(Vec::new()));
    let seen = observations.clone();
    vm.register_fn_vm("observe", move |vm: &mut Vm| {
        seen.borrow_mut().push((vm.running_execution(), vm.running_task().is_some()));
    });
    let owner = vm.new_execution();
    vm.enter_execution(owner);
    vm.eval_source("(observe) (yield) (observe)").unwrap();
    vm.leave_execution(owner);
    assert_eq!(&*observations.borrow(), &[(owner, true), (owner, true)]);
    vm.eval_source("(define ran #f) (spawn (lambda () (set! ran #t)))").unwrap();
    let f = vm.eval_source("(lambda () #t)").unwrap();
    vm.call_input(&f, &[]).unwrap();
    let ran = vm.get_global("ran").unwrap();
    assert_eq!(vm.repr(&ran), "#f");
    vm.run_tasks().unwrap();
    let ran = vm.get_global("ran").unwrap();
    assert_eq!(vm.repr(&ran), "#t");
}

fn reject_reentry(vm: &mut Vm) {
    assert!(vm.run_tasks().unwrap_err().msg.contains("cannot reenter the scheduler"));
    assert!(vm.run_tasks_for(Duration::ZERO).unwrap_err().msg.contains("cannot reenter the scheduler"));
}

struct Converted;
impl IntoValue for Converted {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        reject_reentry(vm);
        42_i64.into_value(vm)
    }
}

#[test]
fn neither_a_guest_native_nor_a_future_converter_can_reenter_the_scheduler() {
    let mut vm = Vm::new();
    vm.register_fn_vm("check-reentry", |vm: &mut Vm| {
        reject_reentry(vm);
        true
    });
    vm.register_async("converted", 0, |_, _| {
        let mut polled = false;
        std::future::poll_fn(move |cx| {
            if polled {
                Poll::Ready(Ok::<_, String>(Converted))
            } else {
                polled = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
    });
    let value = vm.eval_source("(list (check-reentry) (converted))").unwrap();
    assert_eq!(vm.repr(&value), "(#t 42)");
    vm.run_tasks().unwrap();
}

#[test]
fn an_input_is_not_preempted_for_background_tasks() {
    for jit in [None, Some(1)] {
        let mut vm = Vm::new();
        vm.set_jit(jit);
        let value = vm
            .eval_source(
                "(define ran #f)
                 (spawn (lambda () (set! ran #t)))
                 (let loop ((i 0)) (when (< i 100000) (loop (+ i 1))))
                 (let ((before ran)) (yield) (list before ran))",
            )
            .unwrap();
        assert_eq!(vm.repr(&value), "(#f #t)", "jit {jit:?}");
    }
}

#[test]
fn a_ready_future_late_in_an_input_does_not_let_background_tasks_in() {
    let mut vm = Vm::new();
    vm.register_fn("slow", || std::thread::sleep(Duration::from_millis(2)));
    vm.register_async("ready", 0, |_, _| std::future::ready(Ok::<i64, String>(1)));
    let value = vm
        .eval_source(
            "(define ran #f)
             (spawn (lambda () (set! ran #t)))
             (slow)
             (ready)
             (let ((before ran)) (yield) (list before ran))",
        )
        .unwrap();
    assert_eq!(vm.repr(&value), "(#f #t)");
}

#[test]
fn a_stop_for_another_execution_does_not_keep_a_deadlocked_input_spinning() {
    let mut vm = Vm::new();
    let other = vm.new_execution();
    assert!(vm.interrupt_handle().stop(other, Stop::Break));
    let (send, done) = std::sync::mpsc::channel();
    let handle = vm.interrupt_handle();
    let input = vm.new_execution();
    // A watchdog: the input must end by itself, as a deadlock.
    let watchdog = std::thread::spawn(move || {
        if done.recv_timeout(Duration::from_secs(5)).is_err() {
            handle.stop(input, Stop::Kill);
        }
    });
    vm.enter_execution(input);
    let error = vm.eval_source("(channel-recv (make-channel))").unwrap_err();
    vm.leave_execution(input);
    send.send(()).unwrap();
    watchdog.join().unwrap();
    assert!(error.msg.contains("deadlock"), "{}", error.msg);
}

#[test]
fn a_module_the_host_loads_can_wait() {
    let dir = std::env::temp_dir().join(format!("techne-input-module-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("waits.scm");
    std::fs::write(&path, "(define answer (task-join (spawn (lambda () (sleep 1) 42))))").unwrap();
    let mut vm = Vm::new();
    let module = vm.find_module(path.to_str().unwrap()).unwrap();
    let value = vm.eval_in(module, "<test>", "answer").unwrap();
    assert_eq!(vm.repr(&value), "42");
    std::fs::remove_dir_all(dir).unwrap();
}
