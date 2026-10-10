//! The memory limit (PLAN.md, Stage 1, step 8.3): what a world allocates
//! is counted, a collection gives back what it freed, and growth that does
//! not fit is refused before anything is allocated.

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
    techne_vm::builtins::repr(vm.eval_source(src).unwrap_or_else(|e| panic!("{src}: {e}")))
}

const MB: usize = 1 << 20;

/// A request past the limit is a catchable error, and nothing is held for
/// it: not even an overflowing size is attempted.
#[test]
fn a_request_past_the_limit_is_refused() {
    for (name, mut vm) in vms() {
        let held = vm.heap.committed();
        for src in
            ["(make-vector (expt 2 40))", "(make-string (expt 2 40) #\\λ)", "(make-bytevector (expt 2 40))", "(make-vector (expt 2 61))"]
        {
            let caught = eval(&mut vm, &format!("(guard (e (#t (condition/report-string e))) {src})"));
            assert!(caught.contains("out of memory"), "{name}: {src}: {caught}");
        }
        assert!(vm.heap.committed() < held + 16 * MB, "{name}: {} MB held", vm.heap.committed() / MB);
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
        let full = vm.heap.committed();
        let refused = eval(&mut vm, "(guard (e (#t 'refused)) (vector-length (make-vector (* 4 1024 1024))))");
        assert_eq!(refused, "refused", "{name}: {} MB held", full / MB);
        eval(&mut vm, "(set! kept #f)");
        assert_eq!(eval(&mut vm, "(vector-length (make-vector (* 4 1024 1024)))"), "4194304", "{name}");
        vm.full_collect();
        assert!(vm.heap.committed() < full - 32 * MB, "{name}: {} MB held, {} MB before", vm.heap.committed() / MB, full / MB);
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

/// Symbols count, though they are the thread's: making new ones on past
/// the limit is refused, though nothing collects meanwhile.
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
#[test]
fn frees_on_other_threads_are_credited() {
    let vm = Vm::new();
    let held = vm.held().total();
    // `black_box`: an allocation nothing looks at may be left out.
    let block = {
        let _charged = vm.enter_account();
        std::hint::black_box(vec![1u8; 16 * MB])
    };
    assert!(vm.held().total() >= held + 16 * MB);
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
    assert_eq!(vm.held().total(), held);
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
#[test]
fn what_the_host_makes_is_charged() {
    use techne_vm::value::Value;
    let mut vm = Vm::new();
    let held = vm.held().total();
    // 400,000 pairs, some 10 MB of old blocks.
    let items: Vec<Value> = (0..400_000).map(Value::int_unchecked).collect();
    let list = vm.make_list(&items);
    let _kept = vm.root(list);
    let listed = vm.held().total();
    assert!(listed > held + 8 * MB, "{} MB held, {} MB before", listed / MB, held / MB);
    let roots: Vec<_> = (0..100_000).map(|i| vm.root(Value::int_unchecked(i))).collect();
    assert!(vm.held().total() > listed + 2 * MB, "{} MB held, {} MB before", vm.held().total() / MB, listed / MB);
    drop(roots);
}

/// A native's panic, caught by the host, leaves the world's execution:
/// what the host allocates afterwards is not the world's.
#[test]
fn a_caught_panic_leaves_the_world() {
    let mut vm = Vm::new();
    vm.register_fn("explode", || -> i64 { panic!("a native's panic") });
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| vm.eval_source("(explode)").is_ok()));
    assert!(caught.is_err());
    let held = vm.held().total();
    let block = std::hint::black_box(vec![1u8; 16 * MB]);
    assert!(vm.held().total() < held + MB, "{} MB held, {} MB before", vm.held().total() / MB, held / MB);
    drop(block);
}

/// With two worlds on one thread, what one does is its own even while the
/// other's account is entered: here a native of world A runs world B and,
/// meanwhile, makes roots in A.
#[test]
fn worlds_on_one_thread_are_charged_apart() {
    use std::{cell::RefCell, rc::Rc};
    use techne_vm::value::Value;
    let mut a = Vm::new();
    let b = Rc::new(RefCell::new(Vm::new()));
    let (held_a, held_b) = (a.held().total(), b.borrow().held().total());
    let in_b = b.clone();
    a.register_fn_vm("root-while-b-runs", move |a: &mut Vm| -> i64 {
        let mut b = in_b.borrow_mut();
        let id = b.new_execution();
        b.enter_execution(id);
        let roots: Vec<_> = (0..100_000).map(|i| a.root(Value::int_unchecked(i))).collect();
        b.leave_execution(id);
        std::mem::forget(roots);
        0
    });
    eval(&mut a, "(root-while-b-runs)");
    assert!(a.held().total() > held_a + 2 * MB, "A: {} MB held, {} MB before", a.held().total() / MB, held_a / MB);
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
        let refused = vm.eval_source(flood).map(techne_vm::builtins::repr);
        assert!(matches!(&refused, Err(e) if e.to_string().contains("out of memory")), "{name}: {refused:?}");
        eval(&mut vm, "(set! kept '())");
        vm.full_collect();
        let caught = vm.eval_source(&format!("(let retry () (guard (e (#t (retry))) {flood}))")).map(techne_vm::builtins::repr);
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

/// Past twice its limit, a world growing on is killed at its first refusal.
#[test]
fn past_twice_the_limit_the_first_refusal_kills() {
    let mut vm = Vm::new();
    eval(&mut vm, "(define ballast (make-vector (* 16 1024 1024) 0)) (define kept '())");
    vm.set_memory_limit(vm.held().total() * 4 / 9);
    let flood = "(let loop ((i 0)) (when (< i 20000000) (set! kept (cons i kept)) (loop (+ i 1))))";
    let caught = vm.eval_source(&format!("(guard (e (#t 'caught)) {flood})")).map(techne_vm::builtins::repr);
    assert!(matches!(&caught, Err(e) if e.is_kill()), "{caught:?}");
}

/// A native that allocates on, where no check of the VM reaches, ends the
/// program past three times the limit, found when the thread has counted
/// a chunk, or adds what it counted otherwise (reading what the world
/// holds, here). Run in a child process.
#[test]
fn a_native_allocating_on_ends_the_program() {
    const CHILD: &str = "TECHNE_TEST_NATIVE_ALLOCATING_ON";
    if let Some(how) = std::env::var_os(CHILD) {
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
        let _ = vm.eval_source("(allocate-on)");
        return;
    }
    for how in ["chunks", "reading"] {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "a_native_allocating_on_ends_the_program", "--nocapture"])
            .env(CHILD, how)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&child.stderr);
        assert!(!child.status.success() && stderr.contains("three times its memory limit"), "{how}: {:?}: {stderr}", child.status);
    }
}

/// A task that grows its world in a native and yields, so that no
/// collection or check of its own sees it, is refused and killed all the
/// same: what grew is looked at when it switches away.
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
    let task = vm.eval_source("(spawn (lambda () (let loop ((i 0)) (when (< i 24) (grow) (yield) (loop (+ i 1))))))").unwrap();
    let _task = vm.root(task);
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
    let grown = vm.eval_source("(grow)").map(techne_vm::builtins::repr);
    assert!(matches!(&grown, Err(e) if e.to_string().contains("out of memory")), "{grown:?}");
    assert_eq!(eval(&mut vm, "(let loop ((i 0)) (if (< i 1000000) (loop (+ i 1)) 'done))"), "done");
}

/// What a dying task's cleanups grow is the task's, not the next
/// execution's; and a refusal already pending when an execution ends well
/// (here after a collection) is raised all the same.
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
    let grown = vm.eval_source("(grow #t)").map(techne_vm::builtins::repr);
    assert!(matches!(&grown, Err(e) if e.to_string().contains("out of memory")), "{grown:?}");
}

/// A task killed for what its cleanups grew ends killed: its joiners see
/// "task killed", not the error it was dying of.
#[test]
fn a_task_killed_in_its_cleanup_ends_killed() {
    use std::{cell::RefCell, rc::Rc};
    let mut vm = Vm::new();
    eval(&mut vm, "(define ballast (make-vector (* 4 1024 1024) 0))");
    vm.set_memory_limit(vm.held().total() - 8 * MB);
    let kept: Rc<RefCell<Vec<Vec<u8>>>> = Rc::default();
    let into = kept.clone();
    // Past twice the limit at once.
    vm.register_fn("grow", move || -> i64 {
        into.borrow_mut().push(std::hint::black_box(vec![1u8; 48 * MB]));
        0
    });
    eval(&mut vm, "(define t (spawn (lambda () (dynamic-wind (lambda () #f) (lambda () (error \"oops\")) (lambda () (grow))))))");
    vm.run_tasks_for(std::time::Duration::from_millis(100));
    assert_eq!(eval(&mut vm, "(guard (e (#t (condition/report-string e))) (task-join t))"), "\"task killed\"");
}
