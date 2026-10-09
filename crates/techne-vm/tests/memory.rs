//! The memory limit (PLAN.md, Stage 1, step 8.3): what the heap holds is
//! counted, a collection gives back what it freed, and an allocation that
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
        vm.heap.memory_limit = vm.heap.committed() + 64 * MB;
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
