//! End-to-end programs: each case's printed output must match exactly, with
//! and without GC stress (every allocation collects; `full` forces full GCs).
use std::process::Command;

const CASES: &[(&str, &str)] = &[
    ("(define (fib n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2))))) (displayln (fib 20))", "6765\n"),
    (
        "(define (mk) (let ((n 0)) (lambda () (set! n (+ n 1)) n))) (define c (mk)) (c) (c) (displayln (c))",
        "3\n",
    ),
    ("(define (s n) (let loop ((i 0) (acc 0)) (if (= i n) acc (loop (+ i 1) (+ acc i))))) (displayln (s 100000))", "4999950000\n"),
    ("(define (e n) (if (= n 0) #t (o (- n 1)))) (define (o n) (if (= n 0) #f (e (- n 1)))) (displayln (e 100001))", "#f\n"),
    ("(define (f . xs) xs) (displayln (f 1 2 3)) (define (g a . r) (list a r)) (displayln (g 1)) (displayln (g 1 2))", "(1 2 3)\n(1 ())\n(1 (2))\n"),
    ("(displayln (cond ((assv 2 '((1 . a) (2 . b))) => cdr) (else 'no)))", "b\n"),
    ("(displayln (case 3 ((1 2) 'low) ((3 4) 'mid) (else 'high)))", "mid\n"),
    ("(displayln (list (and 1 2 3) (or #f 5) (and 1 #f 3)))", "(3 5 #f)\n"),
    ("(displayln (map (lambda (x) (* x x)) (iota 5)))", "(0 1 4 9 16)\n"),
    ("(displayln (let loop ((i 0) (acc '())) (if (= i 3) acc (loop (+ i 1) (cons (lambda () i) acc)))))", "(#<procedure> #<procedure> #<procedure>)\n"),
    ("(displayln (map (lambda (f) (f)) (let loop ((i 0) (acc '())) (if (= i 3) acc (loop (+ i 1) (cons (lambda () i) acc))))))", "(2 1 0)\n"),
    ("(displayln (list (* 99999999 99999999) (+ 140737488355327 1) (- -140737488355328 1)))", "(9999999800000001 140737488355328 -140737488355329)\n"),
    ("(displayln (list (/ 6 3) (/ 1 2) (expt 2 10) 1.5 (* 1.5 2) (< 1 1.5) (= 2 2.0) (modulo -7 3) (remainder -7 3)))", "(2 0.5 1024 1.5 3.0 #t #t 2 -1)\n"),
    ("(define (deep n) (if (= n 0) 0 (+ 1 (deep (- n 1))))) (displayln (deep 200000))", "200000\n"),
    ("(displayln (list (substring \"héllo\" 1 3) (string-ref \"héllo\" 1) (string-length \"héllo\") (string-append \"a\" \"b\")))", "(él é 5 ab)\n"),
    (
        "(define h (make-hash-table)) (let loop ((i 0)) (when (< i 1000) (hash-table-set! h (number->string i) (list i)) (loop (+ i 1)))) (displayln (list (hash-table-ref h \"999\" #f) (hash-table-count h)))",
        "((999) 1000)\n",
    ),
    (
        "(define v (make-vector 1000 '())) (let loop ((i 0)) (when (< i 1000) (vector-set! v i (cons i (number->string i))) (loop (+ i 1)))) (define (churn n) (if (= n 0) 0 (begin (cons n n) (churn (- n 1))))) (churn 5000) (displayln (cdr (vector-ref v 999)))",
        "999\n",
    ),
    ("(define x 1) (define (get) x) (set! x 2) (displayln (get))", "2\n"),
    ("(define (f) (define a 1) (define (g) (+ a 1)) (g)) (displayln (f))", "2\n"),
    ("(define (f x) (letrec ((ev? (lambda (n) (if (= n 0) #t (od? (- n 1))))) (od? (lambda (n) (if (= n 0) #f (ev? (- n 1)))))) (ev? x))) (displayln (f 10))", "#t\n"),
    ("(define (f) (let ((x 1) (g (lambda (v) (+ v 10)))) (set! x (g x)) x)) (displayln (f))", "11\n"),
    ("(define (inc v) (+ v 1)) (define (f) (let ((x 1)) (set! x (inc x)) (set! x (inc (inc x))) x)) (displayln (f))", "4\n"),
    ("(define (f x) (if (not (< x 3)) 'big 'small)) (displayln (list (f 1) (f 5)))", "(small big)\n"),
];

fn run(src: &str, stress: Option<&str>) -> (String, bool) {
    let dir = std::env::temp_dir().join(format!("techne-vm-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{:x}.scm", src.len() * 31 + src.bytes().map(usize::from).sum::<usize>()));
    std::fs::write(&file, src).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_techne-vm"));
    cmd.arg(&file);
    if let Some(mode) = stress {
        cmd.env("TECHNE_GC_STRESS", mode);
    }
    let out = cmd.output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.success())
}

#[test]
fn programs() {
    for (src, expected) in CASES {
        for stress in [None, Some("1"), Some("full")] {
            let (out, ok) = run(src, stress);
            assert!(ok && out == *expected, "stress={stress:?}\n{src}\nexpected {expected:?}, got {out:?} (ok={ok})");
        }
    }
}

#[test]
fn errors_report_and_fail() {
    let (out, ok) = run("(define (h x) (car x)) (define (g y) (+ 1 (h y))) (g 5)", None);
    assert!(!ok && out.is_empty());
}
