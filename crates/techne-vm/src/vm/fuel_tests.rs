//! Native boundaries must see the exact loop counter and retain a helper's
//! request to end the slice. No timing assertions or VM-wide env mutation.
use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn loop_fuel_is_published_and_a_native_can_shorten_the_slice() {
    for shorten in [false, true] {
        let mut vm = Vm::new();
        vm.set_jit(Some(1));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let trace = seen.clone();
        vm.register_fn_vm("probe-fuel", move |vm: &mut Vm| {
            let mut trace = trace.lock().unwrap();
            assert!(!vm.jit_fuel.is_null(), "probe must run under a native entry");
            // The native-run guard owns this live, same-thread counter.
            trace.push(unsafe { *vm.jit_fuel });
            if shorten && trace.len() == 1 {
                // The same thread-local notification used by allocation
                // pressure. It must not be overwritten by stale SSA fuel.
                hurry_compiled_code();
            }
        });
        vm.eval_source_raw(
            "(define (inc x) (+ x 1))
             (define (drive n)
               (let loop ((i 0) (sum 0))
                 (probe-fuel)
                 (if (= i n) sum
                     (loop (+ i 1) (+ sum (inc i))))))
             (drive 5)",
        )
        .unwrap();
        let trace = seen.lock().unwrap();
        assert_eq!(trace.len(), 6, "{trace:?}");
        if shorten {
            // The next counted call polls, rather than going on with
            // the pre-notification count. Reentry refills the slice.
            assert!(trace[1] > trace[0] - 2, "{trace:?}");
            assert!(trace[1..].windows(2).all(|p| p[0] - p[1] == 2), "{trace:?}");
        } else {
            // One inlined Scheme call and one loop back-edge each turn.
            assert!(trace.windows(2).all(|p| p[0] - p[1] == 2), "{trace:?}");
        }
    }
}
