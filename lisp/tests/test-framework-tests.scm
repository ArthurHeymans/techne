;;; Unit tests for `(techne test)` assertions, diffs, fixtures, and helpers.
(import (techne test))

(define-suite "assertions"
  (define-test "assert-equal passes on equal values"
    (assert-equal 4 (+ 2 2))
    (assert-equal '(a (b 3)) (list 'a (list 'b (+ 1 2))))
    (assert-equal #(1 2 3) (vector 1 2 3)))

  (define-test "assert-true and assert-false"
    (assert-true #t)
    (assert-true 42)
    (assert-false #f))

  (define-test "assert-contains finds substring"
    (assert-contains "world" "hello world")
    (assert-contains "42" "answer is 42"))

  (define-test "assert-raises catches errors and returns condition"
    (let ((c (assert-raises (error "boom" 1 2))))
      (assert-contains "boom" (condition/report-string c)))
    (assert-raises "specific failure" (error "a specific failure happened"))
    (assert-raises error-object? (error "is an error object")))

  (define-test "assert-eventually succeeds when predicate becomes truthy"
    (let ((n 0))
      (assert-eventually
        (lambda ()
          (set! n (+ n 1))
          (>= n 3))
        #:timeout 2000 #:interval 1)
      (assert-equal 3 n))))

(define-suite "diffs"
  (define-test "scalar diff shows expected and actual"
    (let ((d (format-diff 10 20)))
      (assert-contains "expected: 10" d)
      (assert-contains "actual:   20" d)))

  (define-test "multiline string diff shows line numbers"
    (let ((d (format-diff "line1\nexpected-line\nline3" "line1\nactual-line\nline3")))
      (assert-contains "- 2: \"expected-line\"" d)
      (assert-contains "+ 2: \"actual-line\"" d)))

  (define-test "structural diff reports first differing index"
    (let ((d (format-diff '(1 (2 3) 4) '(1 (2 99) 4))))
      (assert-contains "first diff at (1 1)" d)
      (assert-contains "expected 3, actual 99" d))))

(define-suite "fixtures"
  (define-test "call-with-temporary-directory creates and cleans up on return"
    (let ((created #f))
      (call-with-temporary-directory
        (lambda (dir)
          (set! created dir)
          (assert-true (file-exists? dir))
          (call-with-output-file (string-append dir "/hello.txt")
            (lambda (p) (display "hi" p)))
          (assert-equal "hi" (file->string (string-append dir "/hello.txt")))))
      (assert-false (file-exists? created))))

  (define-test "call-with-temporary-directory cleans up on error"
    (let ((created #f))
      (assert-raises "abort inside temp"
        (call-with-temporary-directory
          (lambda (dir)
            (set! created dir)
            (assert-true (file-exists? dir))
            (error "abort inside temp"))))
      (assert-false (file-exists? created))))

  (define-test "assert-golden compares against file"
    (call-with-temporary-directory
      (lambda (dir)
        (let ((path (string-append dir "/expected.txt")))
          (call-with-output-file path (lambda (p) (display "golden text\n" p)))
          (assert-golden path "golden text\n")))))

  (define-test "test-file-directory points to lisp/tests"
    (assert-contains "lisp/tests" (test-file-directory)))

  (define-test "current-backtrace captures frames inside with-exception-handler"
    (define (inner-boom) (error "trace me"))
    (define (outer-boom) (inner-boom))
    (let ((bt '()))
      (guard (e (#t #t))
        (with-exception-handler
          (lambda (e)
            (when (null? bt) (set! bt (current-backtrace)))
            (raise e))
          outer-boom))
      (assert-true (pair? bt))
      (assert-contains "inner-boom" (string-join bt "\n")))))
