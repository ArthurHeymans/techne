//! Explicit conversion and genuinely inexact division, never implicit
//! exact-to-float arithmetic. Include native aliases and complete errors.
#![cfg(feature = "counting-allocator")]
use std::{fs, process::Command};

const PRELUDE: &str = r#"
(define (cast x) (inexact x))
(define (old-cast x) (exact->inexact x))
(define (cast-through f x) (f x))
(define (divide a b) (/ a b))
(define (divide-through f a b) (f a b))
(let loop ((n 100))
  (when (> n 0) (cast 1) (old-cast 1) (cast-through inexact 1)
        (divide 1.0 2) (divide-through / 1.0 2) (loop (- n 1))))
"#;

fn run(path: &std::path::Path, jit: &str, enabled: Option<bool>, stress: bool, log: bool) -> std::process::Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_techne-vm"));
    c.arg(path)
        .env("TECHNE_JIT", jit)
        .env("TECHNE_JIT_SYNC", "1")
        .env("TECHNE_GC_VERIFY", "1")
        .env_remove("TECHNE_GC_STRESS")
        .env_remove("TECHNE_GC_STATS")
        .env_remove("TECHNE_JIT_LOG")
        .env_remove("TECHNE_JIT_IR")
        .env_remove("TECHNE_JIT_ASM")
        .env_remove("TECHNE_JIT_MEMORY")
        .env_remove("TECHNE_JIT_STATS");
    for flag in ["TECHNE_JIT_INEXACT", "TECHNE_JIT_FLOAT_DIV"] {
        if let Some(enabled) = enabled {
            c.env(flag, if enabled { "1" } else { "0" });
        } else {
            c.env_remove(flag);
        }
    }
    if stress {
        c.env("TECHNE_GC_STRESS", "full");
    }
    if log {
        c.env("TECHNE_JIT_LOG", "1");
    }
    c.output().expect("run VM")
}

#[test]
fn scalar_conversion_and_division_are_selected_and_ablated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("real.scm");
    fs::write(
        &path,
        format!(
            r#"{PRELUDE}
(write (list (cast 0) (old-cast -17) (cast 140737488355327)
             (divide 9.0 2) (divide 9 2) (divide-through / 12 3)
             (divide-through * 6 7) (cast-through abs -7)))
"#
        ),
    )
    .unwrap();
    for enabled in [Some(false), Some(true), None] {
        let out = run(&path, "20", enabled, false, true);
        let log = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{log}");
        assert_eq!(out.stdout, b"(0.0 -17.0 140737488355327.0 4.5 9/2 4 42 7)");
        assert_eq!(log.contains("jit-inexact:"), enabled != Some(false), "{log}");
        assert_eq!(log.contains("jit-float-div:"), enabled != Some(false), "{log}");
    }
}

#[test]
fn inexact_boundaries_preserve_signed_zero_exactness_fallbacks_and_traces() {
    let cases = [
        r#"(write (list (cast -140737488355328) (cast 140737488355328) (cast 9223372036854775808) (old-cast 1/3) (cast 2+3i) (cast 1.25) (cast -0.0) (cast +inf.0) (cast +nan.0)))"#,
        r#"(write (list (divide 1.0 0) (divide 1 -0.0) (divide 0.0 0.0) (divide -0.0 2) (divide 1.0 +inf.0) (divide +inf.0 +inf.0) (divide +nan.0 2)))"#,
        r#"(write (list (divide 140737488355328 2.0) (divide 1/3 2.0) (divide 2+3i 2.0) (divide 12 4) (divide 3 4) (divide-through / 3 4)))"#,
        r#"(define p (list 41)) (write (list (cast-through (lambda (x) (collect-garbage) (+ x (car p))) 1) (divide-through (lambda (a b) (collect-garbage) (+ a b (car p))) 1 2)))"#,
        r#"(guard (e (else (write 'caught))) (divide 1 0))"#,
        r#"(divide 1 0)"#,
        r#"(divide 'bad 1.0)"#,
        r#"(divide 1.0 'bad)"#,
        r#"(cast 'bad)"#,
        r#"(old-cast 'bad)"#,
        r#"(cast 1 2)"#,
    ];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("real.scm");
    for (index, tail) in cases.into_iter().enumerate() {
        fs::write(&path, format!("{PRELUDE}\n{tail}")).unwrap();
        let reference = run(&path, "0", Some(false), false, false);
        assert_eq!(reference.status.success(), index < 5, "{tail}: {}", String::from_utf8_lossy(&reference.stderr));
        for stress in [false, true] {
            let actual = run(&path, "20", Some(true), stress, false);
            assert_eq!(actual.status, reference.status, "{tail}");
            assert_eq!(actual.stdout, reference.stdout, "{tail}");
            assert_eq!(actual.stderr, reference.stderr, "{tail}");
        }
    }
}
