//! The memory limit (PLAN.md, Stage 1, step 8.3): what a world allocates
//! is counted, a collection gives back what it freed, and growth that does
//! not fit is refused before anything is allocated.
//!
//! Tests gated on `counting-allocator` verify tier-2 cooperative
//! accounting: ordinary Rust allocations charged through the global
//! interception (see `alloc`'s tiers). They still run wherever the
//! feature is enabled (the workspace's application builds do); the rest
//! — admitted heap growth, metering, refusals — runs in both
//! configurations.

use techne_vm::vm::Vm;

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

const MB: usize = 1 << 20;

/// A request past the limit is a catchable error, and nothing is held for
/// it: not even an overflowing size is attempted.
#[test]
fn a_request_past_the_limit_is_refused() {
    for (name, mut vm) in vms() {
        let held = vm.heap().committed();
        for src in
            ["(make-vector (expt 2 40))", "(make-string (expt 2 40) #\\λ)", "(make-bytevector (expt 2 40))", "(make-vector (expt 2 61))"]
        {
            let caught = eval(&mut vm, &format!("(guard (e (#t (condition/report-string e))) {src})"));
            assert!(caught.contains("out of memory"), "{name}: {src}: {caught}");
        }
        assert!(vm.heap().committed() < held + 16 * MB, "{name}: {} MB held", vm.heap().committed() / MB);
    }
}

/// What a collection frees is given back, so what was refused fits once
/// what held the memory is gone.
#[test]
fn freed_memory_is_given_back() {
    for (name, mut vm) in vms() {
        vm.set_memory_limit(vm.held().total() + 64 * MB);
        // Some 40 MB of small vectors, in blocks, kept by a global.
        eval(&mut vm, "(define kept (let loop ((i 0) (acc '())) (if (= i 500000) acc (loop (+ i 1) (cons (make-vector 8 i) acc)))))");
        let full = vm.heap().committed();
        let refused = eval(&mut vm, "(guard (e (#t 'refused)) (vector-length (make-vector (* 4 1024 1024))))");
        assert_eq!(refused, "refused", "{name}: {} MB held", full / MB);
        eval(&mut vm, "(set! kept #f)");
        assert_eq!(eval(&mut vm, "(vector-length (make-vector (* 4 1024 1024)))"), "4194304", "{name}");
        vm.full_collect();
        assert!(vm.heap().committed() < full - 32 * MB, "{name}: {} MB held, {} MB before", vm.heap().committed() / MB, full / MB);
    }
}

/// A register stack grows only if what it takes fits: deep recursion past
/// the limit is a catchable error, and the stack is usable afterwards.
#[test]
fn a_stack_grows_only_within_the_limit() {
    for (name, mut vm) in vms() {
        eval(&mut vm, "(define (deep n) (if (= n 0) 0 (+ 1 (deep (- n 1)))))");
        vm.set_memory_limit(vm.held().total() + 8 * MB);
        let caught = eval(&mut vm, "(guard (e (#t (condition/report-string e))) (deep 1000000))");
        assert!(caught.contains("out of memory"), "{name}: {caught}");
        assert_eq!(eval(&mut vm, "(deep 1000)"), "1000", "{name}");
    }
}

/// The stacks of suspended tasks count: tasks each within the limit, but
/// not all together, are refused, and their stacks go when they finish.
#[test]
fn suspended_stacks_count() {
    for (name, mut vm) in vms() {
        eval(&mut vm, "(define (deep n) (if (= n 0) (begin (yield) 0) (+ 1 (deep (- n 1)))))");
        let stacks = vm.held().total();
        vm.set_memory_limit(vm.held().total() + 32 * MB);
        let results =
            eval(&mut vm, "(map task-join (map (lambda (i) (spawn (lambda () (guard (e (#t 'refused)) (deep 200000))))) (iota 8)))");
        assert!(results.contains("refused") && results.contains("200000"), "{name}: {results}");
        assert!(vm.held().total() < stacks + MB, "{name}: {} MB held", vm.held().total() / MB);
    }
}

/// Spawning admits the task's stack: a flood of tasks is refused.
#[test]
fn a_flood_of_tasks_is_refused() {
    for (name, mut vm) in vms() {
        vm.set_memory_limit(vm.held().total() + 16 * MB);
        let spawned =
            eval(&mut vm, "(guard (e (#t 'refused)) (let loop ((i 0)) (when (< i 100000) (spawn (lambda () i)) (loop (+ i 1)))) 'spawned)");
        assert_eq!(spawned, "refused", "{name}");
    }
}

/// A long-lived task spawning children (`spawn #:child`) that finish does
/// not keep a record of each.
#[test]
fn finished_children_are_not_held() {
    let mut vm = Vm::new();
    eval(
        &mut vm,
        "(define (children n) (when (> n 0) (task-join (spawn (lambda () n) #:child #t)) (children (- n 1))))
         (define asked 0) (define done 0)
         (define parent (spawn (lambda () (let loop () (when (> asked done) (children 200000) (set! done asked)) (sleep 1) (loop)))))",
    );
    let round = |vm: &mut Vm, n: u32| {
        eval(vm, &format!("(set! asked {n}) (let wait () (unless (= done {n}) (sleep 1) (wait)))"));
        vm.full_collect();
        vm.held().other
    };
    let held = round(&mut vm, 1);
    let now = round(&mut vm, 2);
    assert!(now < held + MB, "{} KB held, {} KB before", now >> 10, held >> 10);
}

/// Symbols count, though they are the thread's: making new ones on past
/// the limit is refused, though nothing collects meanwhile.
#[cfg(feature = "counting-allocator")]
#[test]
fn symbols_count() {
    for (fill, (name, mut vm)) in ['a', 'b'].into_iter().zip(vms()) {
        vm.set_memory_limit(vm.held().total() + 16 * MB);
        // Names of 200 bytes, a new one each time, made without allocating
        // (each VM's its own: the symbols are the thread's).
        let made = eval(
            &mut vm,
            &format!(
                "(let ((s (make-string 200 #\\{fill})))
                   (guard (e (#t 'refused))
                     (let loop ((i 0))
                       (when (< i 400000)
                         (do ((k 0 (+ k 1)) (n i (quotient n 26))) ((= k 4))
                           (string-set! s k (integer->char (+ 97 (modulo n 26)))))
                         (string->symbol s)
                         (loop (+ i 1))))
                     'made))"
            ),
        );
        assert_eq!(made, "refused", "{name}");
    }
}

/// Spawning admits the dynamic state a task inherits: a task inheriting
/// more than there is room for is refused.
#[test]
fn a_task_inherits_within_the_limit() {
    for (name, mut vm) in vms() {
        vm.register_fn_vm("leave-room", |vm: &mut Vm, bytes: i64| -> bool {
            vm.full_collect();
            vm.set_memory_limit(vm.held().total() + bytes as usize);
            true
        });
        eval(&mut vm, "(define parameters (map make-parameter (iota 10000)))");
        // The 10,000 bindings take some 250 KB in each task.
        let spawned = eval(
            &mut vm,
            "(let bind ((ps parameters))
               (if (null? ps)
                   (begin (leave-room 100000) (guard (e (#t 'refused)) (spawn (lambda () 0)) 'spawned))
                   (parameterize (((car ps) 0)) (bind (cdr ps)))))",
        );
        assert_eq!(spawned, "refused", "{name}");
    }
}

/// What a world allocates while it runs is charged to it, whatever makes
/// it (tables of the VM, a drained channel's buffer, the text a macro
/// keeps), and given back when freed.
#[cfg(feature = "counting-allocator")]
#[test]
fn the_world_is_charged_what_it_allocates() {
    let mut vm = Vm::new();
    let held = vm.held().total();
    eval(
        &mut vm,
        &format!(
            "(do ((i 0 (+ i 1))) ((= i 300)) (environment '(scheme base)))
             (define ch (make-channel 300000))
             (do ((i 0 (+ i 1))) ((= i 300000)) (channel-send ch i))
             (do ((i 0 (+ i 1))) ((= i 300000)) (channel-recv ch))
             (define-syntax big (syntax-rules () ((_) \"{}\")))",
            "x".repeat(4 * MB)
        ),
    );
    vm.full_collect();
    let full = vm.held().total();
    assert!(full > held + 12 * MB, "{} MB held, {} MB before", full / MB, held / MB);
    // Redefined, the macro lets go of its rules and its text.
    eval(&mut vm, "(define-syntax big (syntax-rules () ((_) \"x\")))");
    vm.full_collect();
    assert!(vm.held().total() < full - 6 * MB, "{} MB held, {} MB before", vm.held().total() / MB, full / MB);
}

/// What the world allocated and another thread frees is credited to it.
#[cfg(feature = "counting-allocator")]
#[test]
fn frees_on_other_threads_are_credited() {
    let vm = Vm::new();
    let held = vm.held().total();
    // `black_box`: an allocation nothing looks at may be left out.
    let block = {
        let _charged = vm.enter_account();
        std::hint::black_box(vec![1u8; 16 * MB])
    };
    // Not exactly: the world may free a few bytes of its own meanwhile.
    assert!(vm.held().total() >= held + 15 * MB);
    std::thread::spawn(move || drop(block)).join().unwrap();
    assert!(vm.held().total() < held + MB, "{} MB held", vm.held().total() / MB);
}

/// Once a world is gone, freeing what it allocated changes no other
/// world, even one reusing its account's slot.
#[test]
fn a_gone_world_is_credited_nothing() {
    let block = {
        let vm = Vm::new();
        let _charged = vm.enter_account();
        std::hint::black_box(vec![1u8; 16 * MB])
    };
    let vm = Vm::new();
    let held = vm.held().total();
    drop(std::hint::black_box(block));
    // Not credited the 16 MB (its other threads may allocate meanwhile).
    let now = vm.held().total();
    assert!(now + MB > held, "{held} then {now}");
}

/// Compiling is optional: a hot function is not compiled while there is no
/// room for its machine code, and is once there is.
#[test]
fn no_compilation_without_room() {
    let mut vm = Vm::new();
    vm.set_jit(Some(1));
    eval(&mut vm, "(define (count n) (if (= n 0) 0 (count (- n 1))))");
    vm.set_memory_limit(vm.held().total());
    eval(&mut vm, "(count 1000)");
    assert_eq!(vm.live_jit_arenas(), 0);
    vm.set_memory_limit(usize::MAX);
    eval(&mut vm, "(count 1000)");
    assert_eq!(vm.live_jit_arenas(), 1);
}

/// What the host makes in a world between its executions is the world's
/// too: heap objects (old blocks of pairs here) and roots.
#[cfg(feature = "counting-allocator")]
#[test]
fn what_the_host_makes_is_charged() {
    use techne_vm::api::Value;
    let mut vm = Vm::new();
    let held = vm.held().total();
    // 400,000 pairs, some 10 MB of old blocks.
    let items: Vec<Value> = (0..400_000).map(Value::from).collect();
    let _list = vm.make_list(&items).unwrap();
    let listed = vm.held().total();
    assert!(listed > held + 8 * MB, "{} MB held, {} MB before", listed / MB, held / MB);
    let roots: Vec<_> = (0..100_000).map(|_| vm.make_string("").unwrap()).collect();
    assert!(vm.held().total() > listed + 2 * MB, "{} MB held, {} MB before", vm.held().total() / MB, listed / MB);
    drop(roots);
}

/// A native's panic is contained and poisons the world; the execution
/// it interrupted is left all the same, so what the host allocates
/// afterwards is not the world's. (The accounting half needs the
/// counting allocator; the containment half is in `tests/panic.rs`.)
#[cfg(feature = "counting-allocator")]
#[test]
fn a_caught_panic_leaves_the_world() {
    let mut vm = Vm::new();
    vm.register_fn("explode", || -> i64 { panic!("a native's panic") });
    let error = vm.eval_source("(explode)").unwrap_err();
    assert!(error.is_poison(), "{error:?}");
    let held = vm.held().total();
    let block = std::hint::black_box(vec![1u8; 16 * MB]);
    assert!(vm.held().total() < held + MB, "{} MB held, {} MB before", vm.held().total() / MB, held / MB);
    drop(block);
}

/// With two worlds on one thread, what one does is its own even while the
/// other's account is entered: here a native of world A runs world B and,
/// meanwhile, makes roots in A.
#[cfg(feature = "counting-allocator")]
#[test]
fn worlds_on_one_thread_are_charged_apart() {
    use std::{cell::RefCell, rc::Rc};
    let mut a = Vm::new();
    let b = Rc::new(RefCell::new(Vm::new()));
    let (held_a, held_b) = (a.held().total(), b.borrow().held().total());
    let in_b = b.clone();
    a.register_fn_vm("root-while-b-runs", move |a: &mut Vm| -> i64 {
        let mut b = in_b.borrow_mut();
        // Made before B is entered: the vector itself is the host's.
        let mut roots = Vec::with_capacity(100_000);
        let id = b.new_execution();
        b.enter_execution(id);
        roots.extend((0..100_000).map(|_| a.make_string("").unwrap()));
        b.leave_execution(id);
        std::mem::forget(roots);
        0
    });
    eval(&mut a, "(root-while-b-runs)");
    assert!(a.held().total() > held_a + 2 * MB, "A: {} MB held, {} MB before", a.held().total() / MB, held_a / MB);
    eprintln!("A {} / B {}", a.held(), b.borrow().held());
    assert!(b.borrow().held().total() < held_b + MB, "B: {} MB held, {} MB before", b.borrow().held().total() / MB, held_b / MB);
}

/// A flood of small allocations, which nothing admits, is refused once the
/// world is over its limit; code that catches the refusal and goes on is
/// killed.
#[test]
fn a_flood_is_refused_then_killed() {
    for (name, mut vm) in vms() {
        eval(&mut vm, "(define kept '())");
        vm.set_memory_limit(vm.held().total() + 32 * MB);
        let flood = "(let loop ((i 0)) (when (< i 20000000) (set! kept (cons i kept)) (loop (+ i 1))))";
        let refused = vm.eval_source(flood).map(|v| vm.repr(&v));
        assert!(matches!(&refused, Err(e) if e.to_string().contains("out of memory")), "{name}: {refused:?}");
        eval(&mut vm, "(set! kept '())");
        vm.full_collect();
        let caught = vm.eval_source(&format!("(let retry () (guard (e (#t (retry))) {flood}))")).map(|v| vm.repr(&v));
        assert!(matches!(&caught, Err(e) if e.is_kill()), "{name}: {caught:?}");
    }
}

/// An execution that does not grow the world is not refused, though the
/// world is over its limit (more than a step: the limit lowered, say); the
/// pressure is reported.
#[test]
fn what_does_not_grow_is_not_refused() {
    for (name, mut vm) in vms() {
        eval(&mut vm, "(define ballast (make-vector (* 8 1024 1024) 0))");
        vm.set_memory_limit(vm.held().total() - 16 * MB);
        let sum = eval(&mut vm, "(let loop ((i 0) (acc 0)) (if (< i 3000000) (loop (+ i 1) (+ acc (length (list i i i)))) acc))");
        assert_eq!(sum, "9000000", "{name}");
        assert!(vm.pressure().is_some(), "{name}");
    }
}

/// Compiled code that grows the world only through natives is refused as
/// soon as the interpreter would be, not at the end of its slice, by which
/// it may have grown far past the limit.
#[test]
fn compiled_loops_growing_through_natives_are_refused() {
    for jit in [None, Some(1)] {
        let mut vm = Vm::new();
        vm.set_jit(jit);
        eval(&mut vm, "(define kept '())");
        vm.set_memory_limit(vm.held().total() + 32 * MB);
        let grow = "(let loop () (set! kept (cons (make-vector 1000 0) kept)) (loop))";
        let refused = vm.eval_source(grow).unwrap_err();
        assert!(refused.msg.contains("out of memory"), "{jit:?}: {refused}");
        assert!(vm.held().total() < vm.memory_limit() + 32 * MB, "{jit:?}: {}", vm.held());
    }
}

/// Past twice its limit, a world growing on is killed at its first refusal.
#[test]
fn past_twice_the_limit_the_first_refusal_kills() {
    let mut vm = Vm::new();
    eval(&mut vm, "(define ballast (make-vector (* 16 1024 1024) 0)) (define kept '())");
    vm.set_memory_limit(vm.held().total() * 4 / 9);
    let flood = "(let loop ((i 0)) (when (< i 20000000) (set! kept (cons i kept)) (loop (+ i 1))))";
    let caught = vm.eval_source(&format!("(guard (e (#t 'caught)) {flood})")).map(|v| vm.repr(&v));
    assert!(matches!(&caught, Err(e) if e.is_kill()), "{caught:?}");
}

/// A native growing the world each call, called from compiled code,
/// which checks only every so many calls: past the limit the allocator
/// ends its slice, and the world is refused well before its ceiling.
/// Here `environment`, each call a module of some 250 imports, which
/// stays; counted by interception.
#[cfg(feature = "counting-allocator")]
#[test]
fn compiled_code_calling_a_growing_native_is_refused() {
    techne_vm::alloc::contain_out_of_memory();
    for (name, mut vm) in vms() {
        vm.set_memory_limit(vm.held().total() + 64 * MB);
        let made = vm
            .eval_source(
                "(define made 0)
                 (guard (e ((error-object? e) (condition/report-string e)))
                   (let loop ()
                     (when (< made 100000)
                       (environment '(scheme base))
                       (set! made (+ made 1))
                       (loop)))
                   'all)",
            )
            .map(|v| vm.repr(&v));
        assert!(made.as_ref().is_ok_and(|m| m.contains("out of memory")), "{name}: {made:?}");
    }
}

/// A native that allocates on, where no check of the VM reaches, ends its
/// world past three times the limit, found when the thread has counted a
/// chunk, or adds what it counted otherwise (reading what the world holds,
/// here): its next allocation fails, which the application's hook makes a
/// panic its world's boundary contains (`alloc::contain_out_of_memory`).
/// Run in a child process, which installs it. This is the counting
/// allocator's fail-stop, so it exists only with that feature: without
/// interception a native's ordinary allocations are not the VM's to
/// count (process limits are the backstop there).
#[cfg(feature = "counting-allocator")]
#[test]
fn a_native_allocating_on_ends_its_world() {
    const CHILD: &str = "TECHNE_TEST_NATIVE_ALLOCATING_ON";
    if let Some(how) = std::env::var_os(CHILD) {
        techne_vm::alloc::contain_out_of_memory();
        let mut vm = Vm::new();
        vm.set_memory_limit(vm.held().total() + 16 * MB);
        // A gigabyte, and back (when nothing ends it).
        let reading = how == "reading";
        vm.register_fn_vm("allocate-on", move |vm: &mut Vm| -> i64 {
            let blocks: Vec<Vec<u8>> = (0..1024)
                .map(|_| {
                    if reading {
                        vm.held();
                    }
                    std::hint::black_box(vec![1u8; MB])
                })
                .collect();
            blocks.len() as i64
        });
        let e = vm.eval_source("(allocate-on)").unwrap_err();
        assert!(e.is_poison() && e.msg.contains("memory ceiling"), "{e}");
        // Another world goes on.
        assert_eq!(Vm::new().eval_source("(+ 1 2)").unwrap().as_int(), Some(3));
        return;
    }
    for how in ["chunks", "reading"] {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "a_native_allocating_on_ends_its_world", "--nocapture"])
            .env(CHILD, how)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&child.stderr);
        assert!(child.status.success(), "{how}: {:?}: {stderr}", child.status);
    }
}

/// A task that grows its world in a native and yields, so that no
/// collection or check of its own sees it, is refused and killed all the
/// same: what grew is looked at when it switches away.
#[cfg(feature = "counting-allocator")]
#[test]
fn a_task_growing_between_checks_is_killed() {
    use std::{cell::RefCell, rc::Rc};
    let mut vm = Vm::new();
    vm.set_memory_limit(vm.held().total() + 32 * MB);
    let kept: Rc<RefCell<Vec<Vec<u8>>>> = Rc::default();
    let into = kept.clone();
    vm.register_fn("grow", move || -> i64 {
        into.borrow_mut().push(std::hint::black_box(vec![1u8; 8 * MB]));
        0
    });
    let _task = vm.eval_source("(spawn (lambda () (let loop ((i 0)) (when (< i 24) (grow) (yield) (loop (+ i 1))))))").unwrap();
    for _ in 0..100 {
        vm.run_tasks_for(std::time::Duration::from_millis(10));
    }
    let done = eval(&mut vm, "(guard (e (#t (condition/report-string e))) (task-join (car (list (spawn (lambda () 0))))) 'joined)");
    assert_eq!(done, "joined");
    assert!(kept.borrow().len() < 12, "{} blocks of 8 MB kept", kept.borrow().len());
}

/// Once the world is back under its limit, it is under pressure no more.
#[test]
fn pressure_ends_once_back_under() {
    let mut vm = Vm::new();
    eval(&mut vm, "(define ballast (make-vector (* 8 1024 1024) 0))");
    vm.set_memory_limit(vm.held().total() - 16 * MB);
    assert!(vm.pressure().is_some());
    eval(&mut vm, "(set! ballast #f)");
    vm.full_collect();
    assert!(vm.pressure().is_none(), "{}", vm.held());
}

/// What an execution grows its world by is refused to it, though it ends
/// well and no collection saw the growth, not to the next one.
#[cfg(feature = "counting-allocator")]
#[test]
fn growth_is_refused_to_what_made_it() {
    use std::{cell::RefCell, rc::Rc};
    let mut vm = Vm::new();
    eval(&mut vm, "(define ballast (make-vector (* 4 1024 1024) 0))");
    vm.set_memory_limit(vm.held().total() - 8 * MB);
    let kept: Rc<RefCell<Vec<Vec<u8>>>> = Rc::default();
    let into = kept.clone();
    vm.register_fn("grow", move || -> i64 {
        into.borrow_mut().push(std::hint::black_box(vec![1u8; 16 * MB]));
        0
    });
    let grown = vm.eval_source("(grow)").map(|v| vm.repr(&v));
    assert!(matches!(&grown, Err(e) if e.to_string().contains("out of memory")), "{grown:?}");
    assert_eq!(eval(&mut vm, "(let loop ((i 0)) (if (< i 1000000) (loop (+ i 1)) 'done))"), "done");
}

/// What a dying task's cleanups grow is the task's, not the next
/// execution's; and a refusal already pending when an execution ends well
/// (here after a collection) is raised all the same.
#[cfg(feature = "counting-allocator")]
#[test]
fn growth_at_the_end_is_refused_to_what_made_it() {
    use std::{cell::RefCell, rc::Rc};
    let mut vm = Vm::new();
    // Enough that what grows stays below twice the limit: refused, not
    // killed.
    eval(&mut vm, "(define ballast (make-vector (* 8 1024 1024) 0))");
    vm.set_memory_limit(vm.held().total() - 8 * MB);
    let kept: Rc<RefCell<Vec<Vec<u8>>>> = Rc::default();
    let into = kept.clone();
    vm.register_fn_vm("grow", move |vm: &mut Vm, collect: bool| -> i64 {
        into.borrow_mut().push(std::hint::black_box(vec![1u8; 16 * MB]));
        if collect {
            vm.collect();
        }
        0
    });
    eval(&mut vm, "(spawn (lambda () (dynamic-wind (lambda () #f) (lambda () (error \"fails\")) (lambda () (grow #f)))))");
    vm.run_tasks_for(std::time::Duration::from_millis(100));
    assert_eq!(eval(&mut vm, "(let loop ((i 0)) (if (< i 1000000) (loop (+ i 1)) 'done))"), "done");
    let grown = vm.eval_source("(grow #t)").map(|v| vm.repr(&v));
    assert!(matches!(&grown, Err(e) if e.to_string().contains("out of memory")), "{grown:?}");
}

/// A task killed for what its cleanups grew ends killed: its joiners see
/// "task killed", not the error it was dying of.
#[cfg(feature = "counting-allocator")]
#[test]
fn a_task_killed_in_its_cleanup_ends_killed() {
    use std::{cell::RefCell, rc::Rc};
    let mut vm = Vm::new();
    eval(&mut vm, "(define ballast (make-vector (* 4 1024 1024) 0))");
    let held = vm.held().total();
    let limit = held - 8 * MB;
    vm.set_memory_limit(limit);
    let kept: Rc<RefCell<Vec<Vec<u8>>>> = Rc::default();
    let into = kept.clone();
    // Past twice the limit at once, not three times (which ends the program).
    let grow = limit * 5 / 2 - held;
    vm.register_fn("grow", move || -> i64 {
        into.borrow_mut().push(std::hint::black_box(vec![1u8; grow]));
        0
    });
    eval(&mut vm, "(define t (spawn (lambda () (dynamic-wind (lambda () #f) (lambda () (error \"oops\")) (lambda () (grow))))))");
    vm.run_tasks_for(std::time::Duration::from_millis(100));
    assert_eq!(eval(&mut vm, "(guard (e (#t (condition/report-string e))) (task-join t))"), "\"task killed\"");
}

/// Natives admit what they make of their arguments, temporaries included:
/// conversions, copies, replacements, arithmetic, printing, ports and files
/// that would pass the limit are refused before anything is made, rather
/// than after by the pressure check.
#[test]
fn natives_admit_what_they_make() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("line");
    std::fs::write(&file, "a".repeat(64 * MB)).unwrap();
    for (name, mut vm) in vms() {
        eval(
            &mut vm,
            "(define s (make-string (* 4 1024 1024) #\\a))
             (define v (make-vector (* 4 1024 1024) 0))
             (define x (expt 2 (* 1000 1024)))",
        );
        vm.set_memory_limit(vm.held().total() + 32 * MB);
        let path = format!("{:?}", file.display().to_string());
        for src in [
            "(string->list s)".to_string(),
            "(vector->list v)".into(),
            "(string-append s s s s s s s s s s)".into(),
            "(string-replace s \"a\" \"aaaaaaaa\")".into(),
            "(string-split s \"a\")".into(),
            "(* x x)".into(),
            "(let ((p (open-output-string))) (display (make-vector 16 s) p))".into(),
            "(let ((p (open-output-string))) (do ((i 0 (+ i 1))) ((= i 16)) (write-string s p)))".into(),
            format!("(read-line (open-input-file {path}))"),
            format!("(file->string {path})"),
            // The encodings: their output, then its copy on the heap.
            "(%hex-encode (string-append s s s s))".into(),
            "(%base64-encode (string-append s s s s s s) #f)".into(),
            "(%hex-decode (string-append s s s s s s))".into(),
            "(%base64-decode (string-append s s s s s s) #f)".into(),
        ] {
            let caught = eval(&mut vm, &format!("(guard (e (#t (condition/report-string e))) {src} 'made)"));
            // Before allocating, not by the pressure check after it (or as
            // a number past the bits exact numbers may take).
            let refused = ["more would pass the limit", "input of more than", "are limited to"];
            assert!(refused.iter().any(|r| caught.contains(r)), "{name}: {src}: {caught}");
        }
    }
}

/// An error names the values it is about briefly, however large they are
/// or however many.
#[test]
fn error_messages_are_brief() {
    let mut vm = Vm::new();
    let message = eval(&mut vm, "(guard (e (#t (condition/report-string e))) (car (make-string (* 4 1024 1024) #\\a)))");
    assert!(message.len() < 5000 && message.ends_with("...\""), "{} bytes: {}", message.len(), &message[..100]);
    // Many irritants, all one string of 4 KB.
    let message = eval(
        &mut vm,
        "(guard (e (#t (condition/report-string e))) (let ((s (make-string 4096 #\\a))) (apply error \"many\" (map (lambda (i) s) (iota 8192)))))",
    );
    assert!(message.len() < 32 << 10, "{} bytes", message.len());
}

/// Each of `cases` in a child process of this test program (`test`, the
/// test's name), in a world with 32 MB of room: refused, not past three
/// times the limit, which ends the program. `LINES` stands for a file of
/// five million newlines, `SPACES` for one of 100 million spaces, `PAIRS`
/// for three million `()` and `EMPTIES` for 350,000, `ZEROS` for 400,000
/// zeros, `BIG` for a string of a million characters, `HEX` for a number
/// of 150 KB and `XS` for 1,100 `x`.
fn refused_in_children(test: &str, cases: &[&str]) {
    const CHILD: &str = "TECHNE_TEST_CASE";
    if let Some(case) = std::env::var_os(CHILD) {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, text: &dyn Fn() -> String| {
            let path = dir.path().join(name);
            std::fs::write(&path, text()).unwrap();
            format!("{:?}", path.display().to_string())
        };
        let mut src = cases[case.to_str().unwrap().parse::<usize>().unwrap()].to_string();
        let texts: [(&str, &dyn Fn() -> String); 8] = [
            ("XS", &|| "x ".repeat(1100)),
            ("EMPTIES", &|| "() ".repeat(350_000)),
            ("HEX", &|| format!("#x{}", "f".repeat(300_000))),
            ("LINES", &|| file("lines", &|| "\n".repeat(5 * MB))),
            ("SPACES", &|| file("spaces.scm", &|| " ".repeat(100 * MB))),
            ("PAIRS", &|| "() ".repeat(3_000_000)),
            ("ZEROS", &|| "0 ".repeat(400_000)),
            ("BIG", &|| format!("{:?}", "a".repeat(MB))),
        ];
        for (name, text) in texts {
            if src.contains(name) {
                src = src.replace(name, &text());
            }
        }
        let mut vm = Vm::new();
        vm.set_memory_limit(vm.held().total() + 32 * MB);
        // Caught when it runs, or an error of reading or compiling it.
        let caught = match vm.eval_source(&format!("(guard (e (#t (condition/report-string e))) {src} 'made)")) {
            Ok(v) => vm.repr(&v),
            Err(e) => e.to_string(),
        };
        // Or, a number, past the bits exact numbers may take.
        assert!(
            caught.contains("more would pass the limit") || caught.contains("are limited to"),
            "{}: {caught}",
            &src[..src.len().min(200)]
        );
        return;
    }
    for (i, case) in cases.iter().enumerate() {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env(CHILD, i.to_string())
            .output()
            .unwrap();
        assert!(child.status.success(), "{case}: {:?}: {}", child.status, String::from_utf8_lossy(&child.stderr));
    }
}

/// Natives given one large value many times admit what copying it takes
/// before they copy it.
#[test]
fn natives_given_one_value_many_times() {
    refused_in_children(
        "natives_given_one_value_many_times",
        &[
            "(let ((x (expt 2 (* 8 1024 1024)))) (apply gcd (map (lambda (i) x) (iota 256))))",
            "(let ((s (make-string (* 1024 1024) #\\a))) (string-join (map (lambda (i) s) (iota 256))))",
            "(read-lines LINES)",
            "(let ((s (string->symbol (make-string (* 1024 1024) #\\a)))) (string-join (map (lambda (i) s) (iota 256))))",
            "(repr (make-bytevector (* 8 1024 1024) 0))",
        ],
    );
}

/// Reading, macro expansion and compilation are metered: what text reads
/// as, an expansion that copies its input, and code that nests for ever
/// (its stack counted) are refused past the room left.
#[test]
fn reading_and_expanding_are_metered() {
    refused_in_children(
        "reading_and_expanding_are_metered",
        &[
            "'(PAIRS)",
            "(read (open-input-string \"(PAIRS)\"))",
            "(begin (define-syntax d (syntax-rules () ((_ x) (d (x x))))) (d 1))",
            "(begin (define-syntax f (syntax-rules () ((_ x) (list (f (x x)))))) (f 1))",
            "(begin (define-syntax g (syntax-rules () ((_ x) (list (g x))))) (g 1))",
            "(begin (define-syntax s (syntax-rules () ((_ x) '(XS)))) (length (s BIG)))",
            "`(ZEROS)",
            "(eval (list 'quote (let loop ((i 0) (x 0)) (if (= i 24) x (loop (+ i 1) (vector x x))))))",
            "(include SPACES)",
            "(eval '(require SPACES))",
            "(begin (define-syntax m (syntax-rules () ((_ x ...) 1))) (m EMPTIES))",
            "(begin (define-syntax s (syntax-rules () ((_ x) '(XS)))) (length (s HEX)))",
            "(environment (list (let loop ((i 0) (x 0)) (if (= i 24) x (loop (+ i 1) (vector x x))))))",
        ],
    );
}

/// A string read whole, more than the room left, is refused. (Reading
/// it from a string port first copies the port's text in Rust, which
/// only the counting allocator charges, tier 2; the meter's estimate
/// refuses the datum it makes either way.)
#[test]
fn reading_one_long_string_is_metered() {
    let mut vm = Vm::new();
    eval(&mut vm, "(define s (string-append \"\\\"\" (make-string (* 40 1024 1024) #\\a) \"\\\"\"))");
    // Collect first: the append's intermediate is garbage of the same
    // size, and the limit must be measured from what is actually held
    // when the read begins, in either configuration.
    vm.full_collect();
    vm.set_memory_limit(vm.held().total() + 32 * MB);
    let caught = eval(&mut vm, "(guard (e (#t (condition/report-string e))) (read (open-input-string s)) 'made)");
    assert!(caught.contains("more would pass the limit"), "{caught}");
}

/// What escapes make counts as plain text does, and many just-short
/// copies look as often as one long one: a string of `\n` escapes, and
/// one of 4095-char runs separated by escapes, are refused like the
/// plain long string above (in both configurations; without the
/// counting allocator this is the meter's estimate at work).
#[test]
fn reading_escaped_and_chunked_strings_is_metered() {
    for build in [
        // One `\n` escape (the two characters), doubled to 8 Mi escapes:
        // 8 MB of output, no plain run at all.
        "(define s (string-append \"\\\"\" (let loop ((e \"\\\\n\") (k 0)) (if (= k 22) e (loop (string-append e e) (+ k 1)))) \"\\\"\"))",
        // 4095 plain chars and one escape, doubled to 2048 copies.
        "(define s (string-append \"\\\"\" (let loop ((e (string-append (make-string 4095 #\\a) \"\\\\n\")) (k 0)) (if (= k 11) e (loop (string-append e e) (+ k 1)))) \"\\\"\"))",
    ] {
        let mut vm = Vm::new();
        eval(&mut vm, build);
        vm.full_collect();
        vm.set_memory_limit(vm.held().total() + 4 * MB);
        let caught = eval(&mut vm, "(guard (e (#t (condition/report-string e))) (read (open-input-string s)) 'made)");
        assert!(caught.contains("more would pass the limit"), "{caught}");
    }
}

/// A bytevector element's token is charged before it is parsed: tens
/// of megabytes of digits are arbitrary precision work (and would be
/// formatted whole into the refusal error), not one meter step.
#[test]
fn reading_a_bytevector_of_huge_numbers_is_metered() {
    let mut vm = Vm::new();
    eval(&mut vm, "(define s (string-append \"#u8(\" (make-string (* 16 1024 1024) #\\9) \")\"))");
    vm.full_collect();
    vm.set_memory_limit(vm.held().total() + 4 * MB);
    let caught = eval(&mut vm, "(guard (e (#t (condition/report-string e))) (read (open-input-string s)) 'made)");
    assert!(caught.contains("more would pass the limit"), "{caught}");
}

/// Character names and \x escape runs are tokens too: charged before
/// they are copied, parsed or formatted into an error.
#[test]
fn reading_huge_character_names_and_escapes_is_metered() {
    for build in [
        "(define s (string-append \"#\\\\\" (make-string (* 16 1024 1024) #\\z)))",
        "(define s (string-append \"\\\"\\\\x\" (make-string (* 16 1024 1024) #\\z) \";\\\"\"))",
        "(define s (string-append \"#!\" (make-string (* 16 1024 1024) #\\z) \" 1\"))",
    ] {
        let mut vm = Vm::new();
        eval(&mut vm, build);
        vm.full_collect();
        vm.set_memory_limit(vm.held().total() + 4 * MB);
        let caught = eval(&mut vm, "(guard (e (#t (condition/report-string e))) (read (open-input-string s)) 'made)");
        assert!(caught.contains("more would pass the limit"), "{build}: {caught}");
    }
}

/// Documented tier-2 gap (see `alloc`'s tiers): without the counting
/// allocator, in-memory port buffers are admitted one buffer at a
/// time, not in aggregate — many live ports can hold more Rust buffer
/// storage than the limit, while any one buffer is refused past it.
/// With the feature the buffers are charged and this does not hold.
#[cfg(not(feature = "counting-allocator"))]
#[test]
fn port_buffers_are_admitted_per_buffer_not_in_aggregate() {
    const CHUNK: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let mut vm = Vm::new();
    vm.full_collect();
    vm.set_memory_limit(vm.held().total() + 512 * 1024);
    // Eight ports of 128 KB each pass, a megabyte of buffers in all.
    let made = eval(
        &mut vm,
        &format!(
            "(length (map (lambda (i) (let ((p (open-output-string))) (do ((k 0 (+ k 1))) ((= k 2000)) (display \"{CHUNK}\" p)) p)) '(1 2 3 4 5 6 7 8)))"
        ),
    );
    assert_eq!(made, "8");
    // One buffer alone past the allowance does not.
    let caught = eval(
        &mut vm,
        &format!(
            "(guard (e (#t 'refused)) (let ((p (open-output-string))) (do ((k 0 (+ k 1))) ((= k 20000) 'made) (display \"{CHUNK}\" p))))"
        ),
    );
    assert_eq!(caught, "refused");
}

/// Errors show syntax briefly: syntax sharing one string many times
/// would print as far more text than it holds.
#[test]
fn errors_show_syntax_briefly() {
    let mut vm = Vm::new();
    vm.set_memory_limit(vm.held().total() + 32 * MB);
    let (big, xs) = (format!("{:?}", "a".repeat(MB)), "x ".repeat(1000));
    for src in [
        format!("(define-syntax t (syntax-rules () ((_ 1) 1))) (define-syntax s (syntax-rules () ((_ x) (t {xs})))) (s {big})"),
        format!("(define-syntax s (syntax-rules () ((_ x) (let ((y {xs})) y)))) (s {big})"),
    ] {
        let e = vm.eval_source(&src).unwrap_err().to_string();
        assert!(e.len() < 5000, "{}", &e[..200]);
    }
}

/// A long list pattern makes code linear in its length.
#[test]
fn a_long_list_pattern_compiles() {
    let mut vm = Vm::new();
    vm.set_memory_limit(vm.held().total() + 32 * MB);
    let src = format!("(match (iota 2000) ((list {}) 'yes) (_ 'no))", "_ ".repeat(2000));
    assert_eq!(eval(&mut vm, &src), "yes");
}

/// Reading input stops at the room left, an in-memory port's last part
/// and a line held whole too.
#[test]
fn reading_stops_at_the_room_left() {
    let mut vm = Vm::new();
    eval(&mut vm, "(define b (make-bytevector (* 24 1024 1024) 0)) (define s (make-string (* 24 1024 1024) #\\a))");
    vm.set_memory_limit(vm.held().total() + 32 * MB);
    for src in ["(read-bytevector (* 32 1024 1024) (open-input-bytevector b))", "(read-line (open-input-string s))"] {
        let caught = eval(&mut vm, &format!("(guard (e (#t (condition/report-string e))) {src} 'made)"));
        assert!(caught.contains("out of memory"), "{src}: {caught}");
    }
}

/// Admitting what dicts are made of or turned into may collect, and move
/// the list or dict an operation holds.
#[test]
fn dicts_survive_the_collection_admitting_does() {
    for (name, mut vm) in vms() {
        eval(&mut vm, "(define junk (make-vector 2000000 0))");
        eval(&mut vm, "(define (squares n) (let loop ((i 0) (d {})) (if (= i n) d (loop (+ i 1) (dict-set d i i)))))");
        vm.full_collect();
        vm.set_memory_limit(vm.held().total() + 32 * 1024);
        let src = "(begin (set! junk #f) (dict-size (alist->dict (map (lambda (i) (cons i i)) (iota 1100)))))";
        assert_eq!(eval(&mut vm, src), "1100", "{name}");
        vm.set_memory_limit(usize::MAX / 2);
        eval(&mut vm, "(set! junk (make-vector 2000000 0))");
        vm.full_collect();
        vm.set_memory_limit(vm.held().total() + 32 * 1024);
        assert_eq!(eval(&mut vm, "(begin (set! junk #f) (length (dict-keys (squares 3000))))"), "3000", "{name}");
    }
}

/// JSON text and what it reads as may be far more than the other: one long
/// string read, shared data written each time it is met.
#[test]
fn json_is_refused_past_the_limit() {
    for (name, mut vm) in vms() {
        eval(&mut vm, "(define input (string-append \"\\\"\" (make-string (* 6 1024 1024) #\\a) \"\\\"\"))");
        vm.full_collect();
        vm.set_memory_limit(vm.held().total() + MB);
        let caught = eval(&mut vm, "(guard (e (#t (condition/report-string e))) (string-length (string->json input)))");
        assert!(caught.contains("out of memory"), "{name}: {caught}");
        let shared = "(let loop ((n 25) (v \"xxxxxxxxxx\")) (if (= n 0) (json->string v) (loop (- n 1) (vector v v))))";
        let caught = eval(&mut vm, &format!("(guard (e (#t (condition/report-string e))) {shared})"));
        assert!(caught.contains("out of memory"), "{name}: {caught}");
    }
}

/// Captured standard output (`Vm::capture_output`) is admitted as it grows:
/// a capture whose limit is past the room left cannot hold more than fits.
#[test]
fn captured_output_is_admitted() {
    for (name, mut vm) in vms() {
        eval(&mut vm, "(define s (make-string (* 4 1024 1024) #\\a))");
        vm.set_memory_limit(vm.held().total() + 16 * MB);
        vm.capture_output(usize::MAX);
        for src in ["(do ((i 0 (+ i 1))) ((= i 16)) (display s))", "(do ((i 0 (+ i 1))) ((= i 16)) (write-string s (%stderr)))"] {
            let caught = eval(&mut vm, &format!("(guard (e (#t (condition/report-string e))) {src} 'made)"));
            assert!(caught.contains("more would pass the limit"), "{name}: {src}: {caught}");
            vm.take_output();
        }
    }
}
