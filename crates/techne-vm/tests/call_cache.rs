//! Invariant indirect guards must cache code identity only, and reject
//! mutable/merged sources while preserving generic errors and GC roots.
#![cfg(feature = "counting-allocator")]
use std::{fs, process::Command};

const DRIVE: &str = "
(define (make-adder k) (lambda (x) (+ x k)))
(define (drive f n)
  (let loop ((i 0) (sum 0))
    (if (= i n) sum (loop (+ i 1) (+ sum (f i))))))
";

fn run(file: &std::path::Path, jit: &str, cache: &str, log: bool, stress: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_techne-vm"));
    command
        .arg(file)
        .env("TECHNE_JIT", jit)
        .env("TECHNE_JIT_SYNC", "1")
        .env_remove("TECHNE_JIT_CALL_CACHE")
        .env("TECHNE_JIT_FUEL", "1")
        .env("TECHNE_GC_VERIFY", "1")
        .env_remove("TECHNE_JIT_LOG")
        .env_remove("TECHNE_JIT_IR")
        .env_remove("TECHNE_JIT_ASM")
        .env_remove("TECHNE_JIT_STATS")
        .env_remove("TECHNE_JIT_MEMORY")
        .env_remove("TECHNE_GC_STATS")
        .env_remove("TECHNE_GC_STRESS")
        .env_remove("TECHNE_JIT_INLINE")
        .env_remove("TECHNE_JIT_FEEDBACK");
    if cache != "default" {
        command.env("TECHNE_JIT_CALL_CACHE", cache);
    }
    if log {
        command.env("TECHNE_JIT_LOG", "1");
    }
    if stress {
        command.env("TECHNE_GC_STRESS", "full");
    }
    command.output().expect("run VM")
}

#[test]
fn stable_argument_guard_is_cached_and_ablated() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("call-cache.scm");
    fs::write(&file, format!("{DRIVE} (display (drive (make-adder 7) 3000))")).unwrap();
    for cache in ["0", "1", "default"] {
        let out = run(&file, "20", cache, true, false);
        let log = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{log}");
        assert_eq!(out.stdout, b"4519500");
        assert_eq!(log.contains("jit-call-cache: drive"), cache != "0", "{log}");
    }
}

#[test]
fn cached_guard_preserves_captures_rebinds_errors_and_collection() {
    let cases = [
        format!(
            "{DRIVE} (define a (make-adder 1)) (drive a 3000) (display (list (drive (make-adder 7) 100) (drive (lambda (x) (- x 1)) 100) (drive 42 0)))"
        ),
        format!("{DRIVE} (drive (make-adder 1) 3000) (drive 42 3)"),
        // Same physical self-tail jump, but a new formal callback argument.
        "(define (inc x) (+ x 1)) (define (dec x) (- x 1))
         (define (step f n acc)
           (if (= n 0) acc
             (let ((v (let loop ((i 0) (s 0))
                        (if (= i 64) s (loop (+ i 1) (+ s (f i)))))))
               (step dec (- n 1) (+ acc v)))))
         (display (step inc 2 0))"
            .to_string(),
        // An assignment boxes the argument: it is not an invariant value.
        "(define (inc x) (+ x 1)) (define (dec x) (- x 1))
         (define (changing f n)
           (let loop ((i 0) (s 0))
             (if (= i n) s
               (begin (if (= i 1000) (set! f dec))
                      (loop (+ i 1) (+ s (f i)))))))
         (display (changing inc 2000))"
            .to_string(),
        // Before the join both branches can define the callee. Do not guess
        // an argument alias from only the first path seen during warm-up.
        "(define (inc x) (+ x 1)) (define (twice x) (* x 2))
         (define (joined f n)
           (let loop ((i 0) (s 0))
             (if (= i n) s
               (let ((p (if (< i 1000) f twice)))
                 (loop (+ i 1) (+ s (p i)))))))
         (display (joined inc 2000))"
            .to_string(),
        // A collecting native inside the actual inlinee moves its closure
        // and captured pair, not the private boolean code-identity fact.
        format!("{DRIVE} (define (make k) (lambda (x) (collect-garbage) (+ x (car k)))) (display (drive (make (list 7)) 200))"),
    ];
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("call-cache.scm");
    let expected = [Some("(5650 4850 0)"), None, Some("4032"), Some("1999000"), Some("3499500"), Some("21300")];
    let guards = [("drive", true), ("drive", true), ("step", true), ("changing", false), ("joined", false), ("drive", true)];
    for ((source, expected), (name, cached)) in cases.into_iter().zip(expected).zip(guards) {
        fs::write(&file, &source).unwrap();
        let reference = run(&file, "0", "0", false, false);
        if let Some(expected) = expected {
            assert!(reference.status.success(), "{source}: {}", String::from_utf8_lossy(&reference.stderr));
            assert_eq!(reference.stdout, expected.as_bytes(), "{source}");
        } else {
            assert!(!reference.status.success(), "{source}");
        }
        let probe = run(&file, "20", "1", true, false);
        assert_eq!(String::from_utf8_lossy(&probe.stderr).contains(&format!("jit-call-cache: {name}")), cached, "{source}");
        for stress in [false, true] {
            let out = run(&file, "20", "1", false, stress);
            assert_eq!(out.status, reference.status, "{source}");
            assert_eq!(out.stdout, reference.stdout, "{source}");
            assert_eq!(out.stderr, reference.stderr, "{source}");
        }
    }
}
