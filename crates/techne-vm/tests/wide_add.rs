//! Exact first-class +: use an independent i128 oracle at representation
//! boundaries, not just two engines sharing the same numeric slow path.
#![cfg(feature = "counting-allocator")]
use std::{fs, process::Command};

fn run(path: &std::path::Path, jit: &str, wide: &str, stress: bool, log: bool) -> std::process::Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_techne-vm"));
    c.arg(path)
        .env("TECHNE_JIT", jit)
        .env("TECHNE_JIT_SYNC", "1")
        .env_remove("TECHNE_JIT_WIDE_ADD")
        .env("TECHNE_GC_VERIFY", "1")
        .env_remove("TECHNE_GC_STRESS")
        .env_remove("TECHNE_GC_STATS")
        .env_remove("TECHNE_JIT_LOG")
        .env_remove("TECHNE_JIT_IR")
        .env_remove("TECHNE_JIT_ASM")
        .env_remove("TECHNE_JIT_STATS")
        .env_remove("TECHNE_JIT_MEMORY");
    if wide != "default" {
        c.env("TECHNE_JIT_WIDE_ADD", wide);
    }
    if stress {
        c.env("TECHNE_GC_STRESS", "full");
    }
    if log {
        c.env("TECHNE_JIT_LOG", "1");
    }
    c.output().expect("run VM")
}

const PRELUDE: &str = "
(define (add f a b) (f a b))
(let loop ((n 100)) (if (> n 0) (begin (add + 1 2) (loop (- n 1)))))
";

#[test]
fn indirect_integer_add_is_exact_across_fixnum_i64_and_bigint_edges() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wide-add.scm");
    let edges: Vec<i128> = [-64, -63, -47, 0, 47, 63, 64]
        .into_iter()
        .flat_map(|power: i32| {
            let edge = if power == 0 {
                0
            } else if power < 0 {
                -(1i128 << -power)
            } else {
                1i128 << power
            };
            [-1, 0, 1].into_iter().map(move |delta| edge + delta)
        })
        .collect();
    let mut source = PRELUDE.to_string();
    let mut expected = String::new();
    for &a in &edges {
        for &b in &edges {
            source.push_str(&format!("(write (add + {a} {b})) (newline)\n"));
            expected.push_str(&format!("{}\n", a + b));
        }
    }
    fs::write(&path, source).unwrap();
    for (jit, wide, stress) in [("0", "0", false), ("20", "0", false), ("20", "1", false), ("20", "1", true), ("20", "default", false)] {
        let out = run(&path, jit, wide, stress, false);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(out.stdout, expected.as_bytes(), "{jit}/{wide}/{stress}");
        assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    for wide in ["0", "1", "default"] {
        let out = run(&path, "20", wide, false, true);
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stderr).contains("jit-wide-add:"), wide != "0");
    }
}

#[test]
fn indirect_add_keeps_other_numeric_kinds_aliases_and_error_frames() {
    let cases = [
        "(write (list (add + 1/2 1/3) (add + 1.5 2) (add + 2+3i 4) (add * 6 7) (add - 6 7)))",
        "(define a 140737488355328) (write (list (add + a 7) (add + a -7) a))",
        "(write (list (add + +inf.0 -inf.0) (add + +nan.0 1) (add + -0.0 -0.0) (add + 140737488355328 0.5)))",
        "(guard (e (else (write 'caught))) (add + 'bad 2))",
        "(add + 'bad 2)",
    ];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wide-add.scm");
    for tail in cases {
        fs::write(&path, format!("{PRELUDE}\n{tail}")).unwrap();
        let reference = run(&path, "0", "0", false, false);
        assert_eq!(reference.status.success(), tail != "(add + 'bad 2)", "{}", String::from_utf8_lossy(&reference.stderr));
        for stress in [false, true] {
            let out = run(&path, "20", "1", stress, false);
            assert_eq!(out.status, reference.status, "{tail}");
            assert_eq!(out.stdout, reference.stdout, "{tail}");
            assert_eq!(out.stderr, reference.stderr, "{tail}");
        }
    }
}
