//! Bounded string natives and known numeric indices keep the interpreter's
//! values, fresh mutable identity, errors, roots and mutation behavior.
#![cfg(feature = "counting-allocator")]
use std::{fs, process::Command};

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
        .env_remove("TECHNE_JIT_STATS")
        .env_remove("TECHNE_JIT_MEMORY");
    for flag in ["TECHNE_JIT_INDEX", "TECHNE_JIT_SUBSTRING", "TECHNE_JIT_STRING_EQ", "TECHNE_JIT_NATIVE_CACHE"] {
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

const PRELUDE: &str = r#"
(define (cut s a z) (substring s a z))
(define (same a z) (string=? a z))
(define (at s i) (string-ref s i))
(define (through f s a z) (f s a z))
(let loop ((n 100))
  (when (> n 0) (cut "warm" 0 2) (same "ab" "ab") (at "abc" 1)
        (through substring "warm" 0 2) (loop (- n 1))))
"#;

#[test]
fn ranges_and_equalities_have_an_independent_ascii_oracle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("strings.scm");
    let mut source = PRELUDE.to_string();
    let mut expected = String::new();
    for len in [0, 1, 7, 8, 9, 15, 16, 17, 63, 64, 65, 128, 257] {
        let s: String = (0..len).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        for a in [0, len / 2, len] {
            for z in [a, len] {
                source.push_str(&format!("(write (cut {s:?} {a} {z})) (newline)\n"));
                expected.push_str(&format!("{:?}\n", &s[a..z]));
            }
        }
        let other: String = (0..len).map(|i| if i + 1 == len { 'Z' } else { s.as_bytes()[i] as char }).collect();
        source.push_str(&format!("(write (list (same {s:?} {s:?}) (same {s:?} {other:?}) (same {s:?} \"different\"))) (newline)\n"));
        expected.push_str(&format!("(#t {} #f)\n", if s == other { "#t" } else { "#f" }));
    }
    fs::write(&path, source).unwrap();
    for (jit, enabled, stress) in
        [("0", Some(false), false), ("20", Some(false), false), ("20", Some(true), false), ("20", Some(true), true), ("20", None, false)]
    {
        let out = run(&path, jit, enabled, stress, false);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(out.stdout, expected.as_bytes());
        assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    for enabled in [Some(false), Some(true), None] {
        let out = run(&path, "20", enabled, false, true);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let log = String::from_utf8_lossy(&out.stderr);
        assert_eq!(log.contains("jit-substring:"), enabled != Some(false), "{log}");
        assert_eq!(log.contains("jit-string-eq:"), enabled != Some(false), "{log}");
    }
}

#[test]
fn strings_keep_unicode_redirects_fresh_identity_errors_and_gc() {
    let cases = [
        r#"(write (list (cut "aλ🦀z" 1 3) (same "λ🦀" "λ🦀") (same "λ🦀" "λx") (at "aλ🦀z" 2)))"#,
        r#"(define s (string-copy "abc")) (define t (cut s 0 3)) (string-set! t 0 #\Z) (write (list s t (eq? s t)))"#,
        r#"(define s (string-copy "abc")) (at s 1) (string-set! s 1 #\λ) (write (list (cut s 1 3) (same s "aλc") (at s 2)))"#,
        r#"(define s (string-copy "λbc")) (string-set! s 0 #\a) (write (list (cut s 0 3) (same s "abc")))"#,
        r#"(define callback substring) (through callback "abc" 0 1) (set! callback (lambda (s a z) (collect-garbage) 'changed)) (write (list (through callback "abc" 0 1) (through substring "abc" 0 1)))"#,
        r#"(define (cmp f a z) (f a z)) (let loop ((n 100)) (when (> n 0) (cmp string=? "a" "b") (loop (- n 1)))) (write (cmp (lambda (a z) 'changed) "a" "b"))"#,
        // Many small nursery copies, held through periodic collections.
        r#"(let loop ((n 512) (held '())) (if (= n 0) (begin (collect-garbage) (write (list (length held) (car held)))) (let ((s (cut "abcdefghijk" 1 10))) (loop (- n 1) (cons s held)))))"#,
        r#"(define (scan s) (let loop ((i 0) (r '())) (if (= i (string-length s)) r (loop (+ i 1) (cons (at s i) r))))) (write (list (scan "abc") (scan "λ🦀")))"#,
        r#"(define s (make-string 33 #\λ)) (write (list (same s (make-string 33 #\λ)) (same s (string-append (make-string 32 #\λ) "z")) (cut s 31 33)))"#,
        r#"(define s (string #\a (integer->char 0) #\b)) (write (list (same s (string-copy s)) (cut s 0 2)))"#,
        r#"(cut "abc" -1 2)"#,
        r#"(cut "abc" 2 1)"#,
        r#"(cut "abc" 0 4)"#,
        r#"(cut "abc" 1/2 2)"#,
        r#"(cut 42 0 1)"#,
        r#"(same "a" 42)"#,
        r#"(at "abc" -1)"#,
        r#"(at "abc" 3)"#,
        r#"(at "abc" 1/2)"#,
        r#"(cut "abc" 0)"#,
    ];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("strings.scm");
    for (index, tail) in cases.into_iter().enumerate() {
        fs::write(&path, format!("{PRELUDE}\n{tail}")).unwrap();
        let reference = run(&path, "0", Some(false), false, false);
        assert_eq!(reference.status.success(), index < 10, "{tail}: {}", String::from_utf8_lossy(&reference.stderr));
        for stress in [false, true] {
            let out = run(&path, "20", Some(true), stress, false);
            assert_eq!(out.status, reference.status, "{tail}: {}", String::from_utf8_lossy(&out.stderr));
            assert_eq!(out.stdout, reference.stdout, "{tail}");
            assert_eq!(out.stderr, reference.stderr, "{tail}");
        }
    }
}

#[test]
fn native_guard_cache_refreshes_after_calls_and_rejects_local_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native-guards.scm");
    for (inside, expected) in [(false, "11000"), (true, "11000")] {
        let change = if inside { "(set! ref (lambda (s i) #\\z))" } else { "(change)" };
        let source = format!(
            r#"
(define ref string-ref)
(define (change) (guard (e (else #f)) (collect-garbage) (set! ref (lambda (s i) #\z))))
(define (drive s n)
  (let loop ((i 0) (sum 0))
    (if (= i n) sum
      (begin (when (= i 1000) {change})
             (loop (+ i 1) (+ sum (if (char=? (ref s 0) #\a) 1 10)))))))
(display (drive "abc" 2000))
"#
        );
        fs::write(&path, source).unwrap();
        for enabled in [false, true] {
            let out = run(&path, "20", Some(enabled), false, false);
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            assert_eq!(out.stdout, expected.as_bytes());
        }
        let out = run(&path, "20", Some(true), false, true);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stderr).contains("jit-native-cache: drive"));
    }
}

#[test]
fn nursery_exhaustion_collects_only_after_complete_string_publication() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nursery-strings.scm");
    fs::write(
        &path,
        format!(
            r#"{PRELUDE}
(define (copies n)
  (let loop ((i n) (held '()))
    (if (= i 0) (begin (collect-garbage) (list (length held) (car held)))
        (loop (- i 1) (cons (cut "abcdefghijklmnop" 1 16) held)))))
(write (copies 30000))
"#
        ),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_techne-vm"))
        .arg(&path)
        .env("TECHNE_JIT", "20")
        .env("TECHNE_JIT_SYNC", "1")
        .env("TECHNE_JIT_SUBSTRING", "1")
        .env("TECHNE_JIT_NATIVE_CACHE", "1")
        .env("TECHNE_GC_VERIFY", "1")
        .env("TECHNE_NURSERY_KB", "64")
        .env_remove("TECHNE_GC_STRESS")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.stdout, b"(30000 \"bcdefghijklmnop\")");
}
