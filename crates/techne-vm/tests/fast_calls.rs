//! Differential contract for the optional self-recursive numeric kernel.
//! Keep the original VM/JIT as the reference, including errors and traces.
#![cfg(feature = "counting-allocator")]
use std::{fs, process::Command};

const DENSE: &str = "(define (dense n) (if (= n 0) 0 (let ((a (+ n 1)) (b (+ n 2)) (c (+ n 3)) (d (+ n 4)) (e (+ n 5)) (g (+ n 6)) (h (+ n 7)) (i (+ n 8))) (+ a (+ b (+ c (+ d (+ e (+ g (+ h (+ i (dense (- n 1)))))))))))))";

#[test]
fn numeric_kernel_is_compiled_and_can_be_ablated() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("kernel.scm");
    fs::write(
        &file,
        format!("{DENSE} (define (fib n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2))))) (display (list (fib 20) (dense 4000)))"),
    )
    .unwrap();
    for fast in [Some("0"), Some("1"), None] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_techne-vm"));
        command.arg(&file).env("TECHNE_JIT", "1").env("TECHNE_JIT_SYNC", "1").env_remove("TECHNE_JIT_FAST").env("TECHNE_JIT_LOG", "1");
        if let Some(value) = fast {
            command.env("TECHNE_JIT_FAST", value);
        }
        let out = command.output().unwrap();
        let log = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{log}");
        assert_eq!(out.stdout, b"(6765 64160000)");
        for name in ["fib", "dense"] {
            assert!(log.contains(&format!("jit: {name} compiled")), "{log}");
            assert_eq!(log.contains(&format!("fast-jit: {name}")), fast != Some("0"), "{log}");
        }
    }
}

#[test]
fn recursive_fast_calls_preserve_values_exits_and_source_frames() {
    let cases = [
        "(define (fib n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2))))) (display (fib 28))",
        "(define (tak x y z) (if (not (< y x)) z (tak (tak (- x 1) y z) (tak (- y 1) z x) (tak (- z 1) x y)))) (display (tak 18 12 6))",
        "(define (depth n) (if (= n 0) 0 (+ 1 (depth (- n 1))))) (display (depth 4000))",
        "(define (sum n x) (if (= n 0) x (+ 1 (sum (- n 1) (+ x 1))))) (display (sum 20 140737488355322))",
        "(define (prod n x) (if (= n 0) x (* 2 (prod (- n 1) x)))) (display (prod 8 35184372088832))",
        "(define (inc n x) (if (= n 0) x (+ 1 (inc (- n 1) x)))) (display (list (inc 10 1/2) (inc 10 0.5) (inc 10 2) (inc 10 140737488355327)))",
        "(define (f n) (if (= n 0) 0 (+ 1 (f (- n 1))))) (f 16) (define old f) (set! f (lambda (n) (+ n 7))) (display (old 3))",
        "(define (f n) (if (= n 0) 0 (+ 1 (f (- n 1))))) (f 16) (define old f) (set! f car) (old 3)",
        "(define (prod n x) (if (= n 0) x (* 2 (prod (- n 1) x)))) (define (hold) (let ((p (list 42))) (+ (prod 8 35184372088832) (car p)))) (display (hold))",
        "(define (mixed n x) (if (= n 0) x (+ 1 (mixed (- n 1) x)))) (guard (e (else (display 'caught))) (mixed 10 'bad))",
        "(define (f n) (if (= n 0) 0 (if (< n 3) (+ 1 (alias (- n 1))) (+ 1 (f (- n 1)))))) (define alias f) (f 16) (set! alias car) (f 8)",
        "(define hits 0) (define (f n) (if (= n 0) 0 (if (< n 3) (+ 1 (alias (- n 1))) (+ 1 (f (- n 1)))))) (define alias f) (f 16) (set! alias (lambda (n) (set! hits (+ hits 1)) (collect-garbage) 100)) (display (list (f 8) hits))",
        "(define (arith n x y) (if (= n 0) (* x y) (+ (- x y) (arith (- n 1) x y)))) (display (list (arith 4 -140737488355328 -1) (arith 4 140737488355327 140737488355327) (arith 4 -140737488355328 0) (arith 4 70368744177664 -2)))",
    ];
    let mut cases: Vec<String> = cases.into_iter().map(str::to_owned).collect();
    cases.push(format!("{DENSE} (display (dense 4000))"));
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("fast-calls.scm");
    for source in cases {
        fs::write(&file, &source).unwrap();
        let run = |jit: &str, fast: &str, stress: Option<&str>| {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_techne-vm"));
            cmd.arg(&file)
                .env("TECHNE_JIT", jit)
                .env("TECHNE_JIT_SYNC", "1")
                .env("TECHNE_JIT_FAST", fast)
                .env("TECHNE_GC_VERIFY", "1")
                .env_remove("TECHNE_JIT_LOG")
                .env_remove("TECHNE_JIT_STATS")
                .env_remove("TECHNE_JIT_MEMORY")
                .env_remove("TECHNE_JIT_IR")
                .env_remove("TECHNE_JIT_ASM")
                .env_remove("TECHNE_GC_STATS")
                .env_remove("TECHNE_GC_STRESS");
            if let Some(mode) = stress {
                cmd.env("TECHNE_GC_STRESS", mode);
            }
            cmd.output().expect("run VM")
        };
        let reference = run("0", "0", None);
        for (jit, stress) in [("1", None), ("20", None), ("1", Some("full"))] {
            let actual = run(jit, "1", stress);
            assert_eq!(actual.status, reference.status, "{source} ({jit}/{stress:?})");
            assert_eq!(actual.stdout, reference.stdout, "{source} ({jit}/{stress:?})");
            assert_eq!(actual.stderr, reference.stderr, "{source} ({jit}/{stress:?})");
        }
    }
}
