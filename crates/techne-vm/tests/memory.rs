//! The memory limit (PLAN.md, Stage 1, step 8.3): what the heap and the VM
//! hold is counted, a collection gives back what it freed, and growth that
//! does not fit is refused before anything is allocated.

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
        vm.memory_limit = vm.held().total() + 64 * MB;
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
        vm.memory_limit = vm.held().total() + 8 * MB;
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
        let stacks = vm.held().stacks;
        vm.memory_limit = vm.held().total() + 32 * MB;
        let results =
            eval(&mut vm, "(map task-join (map (lambda (i) (spawn (lambda () (guard (e (#t 'refused)) (deep 200000))))) (iota 8)))");
        assert!(results.contains("refused") && results.contains("200000"), "{name}: {results}");
        assert!(vm.held().stacks < stacks + MB, "{name}: {} MB of stacks", vm.held().stacks / MB);
    }
}

/// Spawning admits the task's stack: a flood of tasks is refused.
#[test]
fn a_flood_of_tasks_is_refused() {
    for (name, mut vm) in vms() {
        vm.memory_limit = vm.held().total() + 16 * MB;
        let spawned =
            eval(&mut vm, "(guard (e (#t 'refused)) (let loop ((i 0)) (when (< i 100000) (spawn (lambda () i)) (loop (+ i 1)))) 'spawned)");
        assert_eq!(spawned, "refused", "{name}");
    }
}

/// Symbols count, though they are the thread's: making new ones past the
/// limit is refused.
#[test]
fn symbols_count() {
    for (fill, (name, mut vm)) in ['a', 'b'].into_iter().zip(vms()) {
        vm.memory_limit = vm.held().total() + 16 * MB;
        // Names of 200 bytes, a new one each time, made without allocating
        // (each VM's its own: the symbols are the thread's).
        let made = eval(
            &mut vm,
            &format!(
                "(let ((s (make-string 200 #\\{fill})))
                   (guard (e (#t 'refused))
                     (let loop ((i 0))
                       (when (< i 100000)
                         (do ((k 0 (+ k 1)) (n i (quotient n 26))) ((= k 4))
                           (string-set! s k (integer->char (+ 97 (modulo n 26)))))
                         (string->symbol s)
                         (loop (+ i 1))))
                     'made))"
            ),
        );
        assert_eq!(made, "refused", "{name}");
        // A symbol that exists takes nothing more.
        vm.memory_limit = 0;
        assert_eq!(eval(&mut vm, "(eq? (string->symbol \"car\") 'car)"), "#t", "{name}");
    }
}

/// Spawning admits the dynamic state a task inherits: a task inheriting
/// more than there is room for is refused.
#[test]
fn a_task_inherits_within_the_limit() {
    for (name, mut vm) in vms() {
        vm.register_fn_vm("leave-room", |vm: &mut Vm, bytes: i64| -> bool {
            vm.full_collect();
            vm.memory_limit = vm.held().total() + bytes as usize;
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

/// Macros count, and so does the source text they keep after the code
/// compiled from it has gone.
#[test]
fn macros_count_with_their_text() {
    let mut vm = Vm::new();
    vm.full_collect();
    let code = vm.held().code;
    // 4 MB in its rules, and as many in its text.
    eval(&mut vm, &format!("(define-syntax big (syntax-rules () ((_) \"{}\")))", "x".repeat(4 * MB)));
    vm.full_collect();
    assert!(vm.held().code > code + 8 * MB, "{} bytes of code, {code} before", vm.held().code);
    // Redefined, it lets go of both.
    eval(&mut vm, "(define-syntax big (syntax-rules () ((_) \"x\")))");
    vm.full_collect();
    assert!(vm.held().code < code + MB, "{} bytes of code, {code} before", vm.held().code);
    // A macro made from data has no text: its 4 MB integer counts.
    eval(&mut vm, "(eval `(define-syntax big (syntax-rules () ((_) ,(expt 2 (* 32 1024 1024))))))");
    assert!(vm.held().code > code + 4 * MB, "{} bytes of code, {code} before", vm.held().code);
}

/// Modules count with what they import: environments are never freed.
#[test]
fn environments_count() {
    let mut vm = Vm::new();
    let tables = vm.held().tables;
    eval(&mut vm, "(do ((i 0 (+ i 1))) ((= i 1000)) (environment '(scheme base)))");
    assert!(vm.held().tables > tables + 4 * MB, "{} bytes of tables, {tables} before", vm.held().tables);
}

/// A module's name counts, also given afterwards.
#[test]
fn module_names_count() {
    let mut vm = Vm::new();
    let tables = vm.held().tables;
    eval(&mut vm, "(%name-library (make-string (* 2 1024 1024) #\\x))");
    assert!(vm.held().tables > tables + MB, "{} bytes of tables, {tables} before", vm.held().tables);
}

/// A select's operations count while it waits.
#[test]
fn waiting_selects_count() {
    let mut vm = Vm::new();
    let tables = vm.held().tables;
    eval(&mut vm, "(define ch (make-channel)) (define t (spawn (lambda () (%select (map (lambda (i) (list 'recv ch)) (iota 100000))))))");
    use techne_vm::tasks::Progress;
    loop {
        match vm.run_tasks_for(std::time::Duration::from_secs(1)) {
            Progress::OutOfTime => {}
            Progress::Blocked => break,
            Progress::Finished => panic!("finished: {}", eval(&mut vm, "(guard (e (#t (condition/report-string e))) (task-join t))")),
        }
    }
    assert!(vm.held().tables > tables + 2 * MB, "{} bytes of tables, {tables} before", vm.held().tables);
    eval(&mut vm, "(channel-send ch 1) (task-join t)");
    assert!(vm.held().tables < tables + MB, "{} bytes of tables, {tables} before", vm.held().tables);
}

/// A channel's buffer counts at the size it grew to, also once drained.
#[test]
fn channel_buffers_count() {
    let mut vm = Vm::new();
    let tables = vm.held().tables;
    eval(
        &mut vm,
        "(define ch (make-channel 300000))
         (do ((i 0 (+ i 1))) ((= i 300000)) (channel-send ch i))
         (do ((i 0 (+ i 1))) ((= i 300000)) (channel-recv ch))",
    );
    vm.full_collect();
    // Some 8 MB, and 4 for the table of roots the messages had.
    assert!(vm.held().tables > tables + 8 * MB, "{} bytes of tables, {tables} before", vm.held().tables);
}

/// Compiling is optional: a hot function is not compiled while there is no
/// room for its machine code, and is once there is.
#[test]
fn no_compilation_without_room() {
    let mut vm = Vm::new();
    vm.set_jit(Some(1));
    eval(&mut vm, "(define (count n) (if (= n 0) 0 (count (- n 1))))");
    vm.memory_limit = vm.held().total();
    eval(&mut vm, "(count 1000)");
    assert_eq!(vm.live_jit_arenas(), 0);
    vm.memory_limit = usize::MAX;
    eval(&mut vm, "(count 1000)");
    assert_eq!(vm.live_jit_arenas(), 1);
}
