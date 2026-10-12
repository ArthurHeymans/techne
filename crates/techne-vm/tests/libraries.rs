//! Shipped libraries work without the development shell or upstream sources.
use std::{
    fs,
    process::{Command, Stdio},
    time::Duration,
};

use wait_timeout::ChildExt;

const CASES: &[(&str, &str, &str)] = &[
    ("'(scheme base) '(srfi 1)", "(list (zip '(a b) '(1 2)) (fold + 0 (iota 5)) (take '(a b c) 2))", "(((a 1) (b 2)) 10 (a b))"),
    (
        "'(scheme base) '(srfi 1)",
        "(let* ((tail (list 4 5))
                (joined (append-reverse! (list 1 2 3) tail)))
           (list joined (append-reverse! '() tail)
                 (append-reverse! (list 1 2) '())
                 (append-reverse! (list 1 2) 'tail)
                 (eq? (drop joined 3) tail)))",
        "((3 2 1 4 5) (4 5) (2 1) (2 1 . tail) #t)",
    ),
    (
        "'(scheme base) '(srfi 1)",
        "(let ((circle (circular-list 1 2)))
           (map (lambda (x) (list (proper-list? x) (dotted-list? x) (circular-list? x)))
                (list '() '(1) '(1 2) '(1 . 2) 'atom
                      (circular-list 1) circle (cons 0 circle))))",
        "((#t #f #f) (#t #f #f) (#t #f #f) (#f #t #f) (#f #t #f) (#f #f #t) (#f #f #t) (#f #f #t))",
    ),
    (
        "'(scheme base) '(srfi 1)",
        "(list (call-with-values (lambda () (lset-diff+intersection = '(1 2 3) '(1) '(2))) list)
               (call-with-values (lambda () (lset-diff+intersection! = '(1 2 3) '(1) '(2))) list)
               (call-with-values (lambda () (lset-diff+intersection = '(1 2))) list)
               (call-with-values (lambda () (lset-diff+intersection = '(1 1 2) '(1) '(2))) list)
               (call-with-values (lambda () (lset-diff+intersection = '() '(1))) list))",
        "(((3) (1 2)) ((3) (1 2)) ((1 2) ()) (() (1 1 2)) (() ()))",
    ),
    (
        "'(scheme base) '(srfi 1)",
        "(list (every (lambda (a b) b) '(1 2) '(x y))
               (every (lambda (a b) b) '(1 2) '(x))
               (every (lambda args (error \"empty input\")) '() '(1))
               (let* ((calls 0)
                      (result (every (lambda (a b) (set! calls (+ calls 1)) (and (< a 2) b))
                                     '(1 2 3) '(x y z))))
                 (list result calls)))",
        "(y x #t (#f 2))",
    ),
    (
        "'(scheme base) '(scheme char) '(srfi 113) '(srfi 128)",
        "(let* ((cmp (make-comparator string? string-ci=? string-ci<? string-ci-hash))
                (s (set cmp \"Hello\" \"HELLO\"))
                (b (bag cmp \"Hello\" \"HELLO\")))
           (list (set-size s) (set-contains? s \"hello\") (bag-element-count b \"hello\")))",
        "(1 #t 2)",
    ),
    (
        "'(scheme base) '(srfi 117)",
        "(let ((q (list-queue 1 2)))
           (list-queue-add-back! q 3)
           (list (list-queue-remove-front! q) (list-queue-list q)))",
        "(1 (2 3))",
    ),
    (
        "'(scheme base) '(srfi 128)",
        "(let ((cmp (make-default-comparator)))
           (list (<? cmp '(1 2) '(1 3)) (=? cmp '(1 2) '(1 2))
                 (<? cmp '() '()) (<? cmp '(1 2) '(1 2)) (<? cmp '(1 3) '(1 2))
                 (= (comparator-hash cmp (list 1 2)) (comparator-hash cmp (list 1 2)))))",
        "(#t #t #f #f #f #t)",
    ),
    (
        "'(scheme base) '(srfi 132)",
        "(list (list-sort < '(3 1 2))
               (list-stable-sort (lambda (a b) (< (car a) (car b))) '((2 . a) (1 . b) (2 . c)))
               (vector-sort < '#(3 1 2)))",
        "((1 2 3) ((1 . b) (2 . a) (2 . c)) #(1 2 3))",
    ),
    // Import the vector API without (scheme base), so missing re-exports
    // cannot be hidden by the base library or the REPL's root bindings.
    (
        "'(only (scheme base) list +) '(srfi 133)",
        "(list (vector-fold + 0 (vector 1 2 3))
               (vector-copy (vector 1 2 3) 1)
               (vector-map (lambda (x) (+ x 1)) (vector 1 2))
               (vector->string (string->vector \"hello\")))",
        "(6 #(2 3) #(2 3) \"hello\")",
    ),
    (
        "'(scheme base) '(srfi 151)",
        "(list (bitwise-and 255 15) (bitwise-ior 1 4)
               (arithmetic-shift 1 80) (bit-count -1) (bits #t #f #t))",
        "(15 5 1208925819614629174706176 0 5)",
    ),
];

#[test]
fn bundled_libraries() {
    let dir = std::env::temp_dir().join(format!("techne-bundled-libraries-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    for (i, (imports, expression, expected)) in CASES.iter().enumerate() {
        let file = dir.join(format!("{i}.scm"));
        fs::write(&file, format!("(write (eval '{expression} (environment {imports}))) (newline)\n")).unwrap();
        for (jit, stress) in [("0", "0"), ("1", "0"), ("0", "1")] {
            let mut child = Command::new(env!("CARGO_BIN_EXE_techne-vm"))
                .arg(&file)
                .current_dir(&dir)
                .env_remove("TECHNE_LIBRARY_PATH")
                .env_remove("TECHNE_R7RS_TESTS")
                .env_remove("TECHNE_SRFI_SOURCES")
                .env("TECHNE_JIT", jit)
                .env("TECHNE_GC_STRESS", stress)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let finished = child.wait_timeout(Duration::from_secs(30)).unwrap().is_some();
            if !finished {
                child.kill().unwrap();
            }
            let out = child.wait_with_output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                finished && out.status.success() && stdout.trim_end() == *expected,
                "imports={imports}, jit={jit}, gc={stress}, finished={finished}:\n{stdout}\n{}",
                String::from_utf8_lossy(&out.stderr),
            );
        }
    }
    fs::remove_dir_all(dir).unwrap();
}
