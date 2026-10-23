//! Compare complete observable errors, not just messages or two engines
//! consuming the same optimized code. Each process selects its own passes.
#![cfg(feature = "counting-allocator")]

use std::{fs, process::Command};

#[test]
fn optimized_logical_frames_match_source_calls() {
    let cases = [
        "(let ((f (lambda (x) (car x)))) (f 0))",
        "(let ((f (lambda (x) (car x)))) (+ 1 (f 0)))",
        "(let ((f (lambda (x) (car x)))) (let ((g (lambda (x) (f x)))) (+ 1 (g 0))))",
        "(let ((f (lambda (x) (car x)))) (let ((g (lambda (x) (+ 2 (f x))))) (+ 1 (g 0))))",
        "(define (fail x) (car x)) (let ((f (lambda (x) (fail x)))) (+ 1 (f 0)))",
        "(define (fail x) (car x)) (let ((f (lambda (x) (+ 2 (fail x))))) (+ 1 (f 0)))",
        "(let ((f (lambda (x) (x 0)))) (+ 1 (f 0)))",
        "(let ((f (lambda (x) (x)))) (+ 1 (f car)))",
        "(letrec ((f (lambda (n) (if (= n 0) (car n) (g (- n 1)))))
                  (g (lambda (n) (if (= n 0) (car n) (f (- n 1)))))) (f 3))",
        "(+ 1 (letrec ((f (lambda (n) (if (= n 0) (car n) (g (- n 1)))))
                       (g (lambda (n) (if (= n 0) (car n) (f (- n 1)))))) (f 3)))",
        "(with-exception-handler (lambda (e) (raise e))
           (lambda () (let ((f (lambda (x) (car x)))) (+ 1 (f 0)))))",
        "(define (descend n)
           (let ((f (lambda (x) (if (= x 0) (car x) (+ 1 (descend (- x 1))))))) (+ 1 (f n))))
         (descend 25)",
        "(define (descend n)
           (let ((f (lambda (x) (if (= x 0) (car x) (descend (- x 1)))))) (+ 1 (f n))))
         (descend 25)",
        "(define t (spawn (lambda () (sleep 1) (car 0))))
         (let ((f (lambda () (task-join t)))) (+ 1 (f)))",
    ];
    let file = std::env::temp_dir().join(format!("techne-ir-frames-{}.scm", std::process::id()));
    let run = |passes: &str, jit: &str| {
        Command::new(env!("CARGO_BIN_EXE_techne-vm"))
            .arg(&file)
            .env("TECHNE_IR", passes)
            .env("TECHNE_JIT", jit)
            .env_remove("TECHNE_DUMP")
            .env_remove("TECHNE_JIT_LOG")
            .env_remove("TECHNE_GC_STATS")
            .output()
            .expect("run VM")
    };
    for src in cases {
        techne_vm::reader::read_located(src).expect("fixture parses before testing its runtime error");
        fs::write(&file, src).unwrap();
        let reference = run("0", "0");
        assert!(!reference.status.success(), "fixture must fail: {src}");
        for passes in ["local", "contify", "all"] {
            for jit in ["0", "1"] {
                let actual = run(passes, jit);
                assert_eq!(actual.status, reference.status, "{passes}/{jit}: {src}");
                assert_eq!(actual.stdout, reference.stdout, "{passes}/{jit}: {src}");
                assert_eq!(String::from_utf8_lossy(&actual.stderr), String::from_utf8_lossy(&reference.stderr), "{passes}/{jit}: {src}");
            }
        }
    }
    fs::remove_file(file).unwrap();
}
