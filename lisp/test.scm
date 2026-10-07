;;; Minimal test harness: (check name expected actual), then (test-summary),
;;; or (test-failures) where the host reports the result.

(provide check check-true test-summary test-failures)

(define %failures 0)
(define %count 0)

(define (check name expected actual)
  "Count a check called NAME: ACTUAL must be `equal?` to EXPECTED.
A failure is printed with both."
  (set! %count (+ %count 1))
  (unless (equal? expected actual)
    (set! %failures (+ %failures 1))
    (display "FAIL ") (displayln name)
    (display "  expected: ") (write expected) (newline)
    (display "  actual:   ") (write actual) (newline)))

(define (check-true name v)
  "Count a check called NAME: V must be true."
  (check name #t (and v #t)))

(define (test-failures)
  "Return how many checks failed."
  %failures)

(define (test-summary)
  "Print how many checks passed, then exit with failure if any failed."
  (displayln (string-append (number->string (- %count %failures)) "/" (number->string %count) " passed"))
  (exit (if (= %failures 0) 0 1)))
