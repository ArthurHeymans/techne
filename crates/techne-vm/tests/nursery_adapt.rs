//! The opt-in survivor-sized nursery (`TECHNE_NURSERY_ADAPT`): the
//! collection-path behaviour that a unit test of `adapt_nursery` cannot see.
//!
//! A separate process per case, because the policy is read from the
//! environment when a heap is made. See runtime/bench/ADAPTIVE-NURSERY.md.
#![cfg(feature = "counting-allocator")]
use std::{fs, process::Command};

fn run(program: &str, env: &[(&str, &str)]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("program.scm");
    fs::write(&path, program).unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_techne-vm"));
    c.arg(&path)
        .env("TECHNE_JIT_SYNC", "1")
        .env_remove("TECHNE_NURSERY_KB")
        .env_remove("TECHNE_NURSERY_ADAPT")
        .env_remove("TECHNE_NURSERY_ADAPT_KB")
        .env_remove("TECHNE_GC_STRESS");
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().expect("run VM");
    assert!(out.status.success(), "{}: {}", program.lines().next().unwrap_or("program"), String::from_utf8_lossy(&out.stderr));
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// A bulk reservation is checked against the nursery capacity, then collects
/// to make its span. The policy must not shrink the nursery in that
/// collection: the span the caller reserved would no longer fit, and the
/// release build would bump past the end of the nursery.
#[test]
fn a_bulk_reservation_survives_a_nursery_that_would_shrink() {
    let text = run(
        "(define xs (let loop ((n 800000) (xs '())) (if (= n 0) xs (loop (- n 1) (cons n xs)))))
         (collect-garbage)
         (let loop ((n 650000)) (unless (= n 0) (cons n n) (loop (- n 1))))
         (display (length (reverse xs))) (newline)",
        &[("TECHNE_NURSERY_ADAPT", "1")],
    );
    assert!(text.starts_with("800000\n"), "{text}");
}

/// An explicit capacity pins the nursery and switches adaptation off, even
/// when the opt-in is also set. The program fills that 16 MiB many times over
/// (4M pairs = 96 MB), so a policy that adapted would grow past it.
#[test]
fn an_explicit_capacity_pins_and_disables_adaptation() {
    let text = run(
        "(let loop ((n 4000000) (acc '()))
           (if (= n 0) (begin (gc-stats) (display (length acc))) (loop (- n 1) (cons n acc))))",
        &[("TECHNE_NURSERY_KB", "16384"), ("TECHNE_NURSERY_ADAPT", "1")],
    );
    assert!(text.starts_with("4000000"), "{text}");
    assert!(text.contains("nursery_words: 2097152, nursery_target_words: 2097152"), "{text}");
}

/// GC stress collects on every allocation; those collections are forced, not
/// a full window, so they must not grow the nursery.
#[test]
fn gc_stress_does_not_grow_the_nursery() {
    let text = run(
        "(begin (define v (make-vector 200000 42)) (cons 1 2) (gc-stats) (display (vector-ref v 199999)))",
        &[("TECHNE_NURSERY_ADAPT", "1"), ("TECHNE_GC_STRESS", "1")],
    );
    assert!(text.starts_with("42"), "{text}");
    assert!(text.contains("nursery_words: 1048576, nursery_target_words: 1048576"), "{text}");
}

/// A ceiling too large to be a nursery falls back to the default rather than
/// wrapping, and a program that runs out of window does not panic.
#[test]
fn an_oversized_ceiling_is_ignored() {
    let text = run(
        "(define (spin n) (let loop ((i 0) (acc 0)) (if (= i n) acc (loop (+ i 1) (+ acc (car (cons i '())))))))
         (display (spin 20000)) (newline)",
        &[("TECHNE_NURSERY_ADAPT", "1"), ("TECHNE_NURSERY_ADAPT_KB", "144115188075855872")],
    );
    assert!(text.starts_with("199990000\n"), "{text}");
}

/// Off by default: a fresh world's nursery is the base even after the
/// prelude's forced collection, which is not a window exhaustion.
#[test]
fn the_default_nursery_stays_at_the_base() {
    let text = run("(gc-stats)", &[]);
    assert!(text.contains("nursery_words: 1048576, nursery_target_words: 1048576"), "{text}");
}
