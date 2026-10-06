;;; Minimal test harness: (check name expected actual), then (test-summary),
;;; or (test-failures) where the host reports the result.

(provide check check-true test-summary test-failures)

(define %failures 0)
(define %count 0)

(define (check name expected actual)
  (set! %count (+ %count 1))
  (unless (equal? expected actual)
    (set! %failures (+ %failures 1))
    (display "FAIL ") (displayln name)
    (display "  expected: ") (write expected) (newline)
    (display "  actual:   ") (write actual) (newline)))

(define (check-true name v) (check name #t (and v #t)))

(define (test-failures) %failures)

(define (test-summary)
  (displayln (string-append (number->string (- %count %failures)) "/" (number->string %count) " passed"))
  (exit (if (= %failures 0) 0 1)))
