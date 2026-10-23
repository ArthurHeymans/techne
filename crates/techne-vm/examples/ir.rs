//! Paired language-IR opportunities, not optimizer implementations.
//!
//! `ir --check` checks both sides against independent integer oracles.
//! `ir CASE before|after [ITERATIONS [REPEATS]]` warms the selected kernel,
//! then prints JSON for its timed calls. Callgrind can collect just
//! `*timed_kernel*`, excluding boot, reading, compilation and warm-up.
//! See runtime/bench/ir/README.md for the campaign and its limitations.

use std::{hint::black_box, time::Instant};

use techne_vm::{api::Value, vm::Vm};

const SOURCE: &str = include_str!("../../../runtime/bench/ir/kernels.scm");
const CASES: &[&str] = &[
    "constant-let",
    "dead-arithmetic",
    "local-closure",
    "closure-hoist",
    "mutual-tail",
    "pair-scalar",
    "vector-scalar",
    "cse-arithmetic",
    "cse-vector",
    "licm",
    "rest-list",
    "beta-control",
    "fold-control",
];

/// Deliberately independent of the Scheme loops and their transformations.
fn expected(case: &str, n: i64, seed: i64) -> i64 {
    let sum = n * (n - 1) / 2;
    match case {
        "constant-let" => seed + sum + 17 * n,
        "dead-arithmetic" | "mutual-tail" => seed + sum,
        "local-closure" => 3 * sum + seed * n,
        "closure-hoist" | "pair-scalar" | "rest-list" | "beta-control" => sum + seed * n,
        "vector-scalar" => sum + (seed + 1) * n,
        "cse-arithmetic" => seed + n * (n - 1) * (2 * n - 1) / 3,
        "cse-vector" => 2 * seed * n,
        "licm" => sum + (seed * seed % 97) * n,
        "fold-control" => seed + 35 * n,
        _ => panic!("unknown kernel {case}"),
    }
}

fn world() -> Vm {
    let mut vm = Vm::new();
    vm.eval_source(SOURCE).expect("kernel definitions compile");
    vm
}

fn check() {
    let mut vm = world();
    for case in CASES {
        for variant in ["before", "after"] {
            let f = vm.get_global(&format!("{case}-{variant}")).unwrap();
            // Zero-trip loops, odd/even recursion, negative seeds, varying
            // arguments after JIT compilation: not only the measured inputs.
            for n in [0, 1, 2, 31, 128] {
                for seed in [17, -7, 0, 99] {
                    let args = [vm.make_int(n).unwrap(), vm.make_int(seed).unwrap()];
                    let result = vm.call(&f, &args).unwrap();
                    assert_eq!(result.as_int(), Some(expected(case, n, seed)), "{case}-{variant}, n={n}, seed={seed}");
                }
            }
        }
    }
    println!("checked {} kernels, both variants, 20 inputs each", CASES.len());
}

/// Keep this boundary visible to Callgrind. Each repeat is one Scheme call
/// doing n iterations, not n host/guest transitions. Returning the last rooted
/// result is safe across collections in subsequent calls.
#[inline(never)]
fn timed_kernel(vm: &mut Vm, f: &Value, args: &[Value], repeats: usize) -> Value {
    (0..repeats).fold(Value::VOID, |_, _| black_box(vm.call(f, args).expect("kernel runs")))
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--list") {
        CASES.iter().for_each(|case| println!("{case}"));
        return;
    }
    if args.first().is_some_and(|a| a == "--check") {
        check();
        return;
    }
    assert!((2..=4).contains(&args.len()), "usage: ir CASE before|after [ITERATIONS [REPEATS]]; ir --list; ir --check");
    let (case, variant) = (args[0].as_str(), args[1].as_str());
    assert!(CASES.contains(&case), "unknown kernel {case}");
    assert!(["before", "after"].contains(&variant), "unknown variant {variant}");
    let n: i64 = args.get(2).map_or(20_000, |n| n.parse().expect("iterations is an integer"));
    let repeats: usize = args.get(3).map_or(10, |n| n.parse().expect("repeats is an integer"));
    // The bounded domain keeps the oracles and every intermediate in fixnums.
    assert!((1..=20_000).contains(&n) && repeats > 0);
    let seed = 17;
    let mut vm = world();
    let f = vm.get_global(&format!("{case}-{variant}")).unwrap();
    let inputs = [vm.make_int(n).unwrap(), vm.make_int(seed).unwrap()];
    for _ in 0..3 {
        assert_eq!(vm.call(&f, &inputs).unwrap().as_int(), Some(expected(case, n, seed)));
    }
    let start = Instant::now();
    let result = timed_kernel(&mut vm, &f, &inputs, repeats);
    let elapsed = start.elapsed().as_nanos();
    assert_eq!(result.as_int(), Some(expected(case, n, seed)));
    println!(
        "{{\"case\":\"{case}\",\"variant\":\"{variant}\",\"iterations\":{n},\"repeats\":{repeats},\"elapsed_ns\":{elapsed},\"result\":{}}}",
        result.as_int().unwrap()
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn paired_kernels_match_oracles() {
        super::check();
    }
}
