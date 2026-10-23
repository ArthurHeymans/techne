//! Real SRFI code compiled once and reused by the existing Snapshot cache.
//! This is NOT a persistent file-cache or lazy-package startup measurement.
//! ir_cache list|vector|set [SIZE [REPEATS [WORLDS]]]

use std::{hint::black_box, time::Instant};

use techne_vm::vm::{Capability, Grants, USER_MODULE, Vm};

fn source(workload: &str) -> String {
    let (imports, body) = match workload {
        "list" => ("(srfi 1)", "(fold + 0 (map (lambda (x) (* x x)) (filter even? (map (lambda (x) (+ x 3)) (iota n)))))"),
        "vector" => (
            "(srfi 1) (srfi 133)",
            "(vector-fold (lambda (sum x) (+ sum (* x x))) 0 (vector-map (lambda (x) (+ x 3)) (list->vector (iota n))))",
        ),
        "set" => (
            "(srfi 1) (srfi 113) (srfi 128)",
            "(let ((cmp (make-default-comparator))) (set-size (set-union (list->set cmp (iota n)) (list->set cmp (iota n (quotient n 2))))))",
        ),
        _ => panic!("expected list, vector or set"),
    };
    format!("(import (scheme base) {imports}) (define (bench n) {body})")
}

fn expected(workload: &str, n: i64) -> i64 {
    match workload {
        "list" => (0..n).map(|x| x + 3).filter(|x| x % 2 == 0).map(|x| x * x).sum(),
        "vector" => (0..n).map(|x| (x + 3) * (x + 3)).sum(),
        "set" => n + n / 2,
        _ => unreachable!(),
    }
}

/// Includes the ordinary embedding call boundary, but no source compilation.
#[inline(never)]
fn timed_requests(vm: &mut Vm, n: i64, expected: i64, repeats: usize) -> f64 {
    let f = vm.get_global("bench").unwrap();
    let args = [vm.make_int(n).unwrap()];
    let start = Instant::now();
    for _ in 0..repeats {
        assert_eq!(black_box(vm.call(&f, &args).expect("SRFI request runs")).as_int(), Some(expected));
    }
    start.elapsed().as_secs_f64() * 1e6 / repeats as f64
}

fn median(samples: &mut [f64]) -> f64 {
    samples.sort_unstable_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let workload = args.first().map_or("list", String::as_str);
    let n: i64 = args.get(1).map_or(128, |s| s.parse().unwrap());
    let repeats = args.get(2).map_or(50, |s| s.parse().unwrap());
    let worlds = args.get(3).map_or(9, |s| s.parse().unwrap());
    assert!((1..=100_000).contains(&n) && repeats > 0 && worlds > 0);
    let src = source(workload);
    // Boot is deliberately outside the compilation measurement.
    let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
    let start = Instant::now();
    vm.eval_in(USER_MODULE, "ir-cache.scm", &src).expect("SRFI workload compiles");
    let compile_us = start.elapsed().as_secs_f64() * 1e6;
    let start = Instant::now();
    let cache = vm.snapshot().expect("pure library definitions can be snapshotted");
    let freeze_us = start.elapsed().as_secs_f64() * 1e6;
    let oracle = expected(workload, n);
    let mut world_us = Vec::new();
    let mut first_us = Vec::new();
    let mut hot_us = Vec::new();
    for _ in 0..worlds {
        let start = Instant::now();
        let mut vm = cache.world();
        world_us.push(start.elapsed().as_secs_f64() * 1e6);
        first_us.push(timed_requests(&mut vm, n, oracle, 1));
        // Include any first-use native compilation above; warm execution is
        // reported separately so a cache cannot hide latency in the first key.
        timed_requests(&mut vm, n, oracle, 10);
        hot_us.push(timed_requests(&mut vm, n, oracle, repeats));
    }
    println!(
        "{{\"workload\":\"{workload}\",\"cache\":\"snapshot\",\"n\":{n},\"repeats\":{repeats},\"worlds\":{worlds},\"compile_us\":{compile_us:.3},\"freeze_us\":{freeze_us:.3},\"world_us\":{:.3},\"first_us\":{:.3},\"hot_us\":{:.3}}}",
        median(&mut world_us),
        median(&mut first_us),
        median(&mut hot_us)
    );
}
