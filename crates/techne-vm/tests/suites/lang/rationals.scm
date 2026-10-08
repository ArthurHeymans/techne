;;; Exact rationals: `/` stays exact, conversions to floats round
;;; correctly, and ratios behave as numbers everywhere.

(test-begin "rationals")

(define (round-trip x)
  (read (open-input-string (call-with-output-string (lambda (p) (write x p))))))

;; `/` of exact numbers is exact: an integer when it divides, else a ratio
;; in lowest terms.
(test 2 (/ 6 3))
(test "3/2" (number->string (/ 6 4)))
(test "-1/2" (number->string (/ 1 -2)))
(test #t (exact? (* (/ 1 2) 2)))
(test 1 (+ 1/3 2/3))
(test #t (exact-integer? (+ 1/3 2/3)))
(test 7381/2520 (let loop ((i 1) (acc 0)) (if (> i 10) acc (loop (+ i 1) (+ acc (/ 1 i))))))
(test-error (/ 1 0))
(test-error (/ 1/2 0))
(test 0.5 (/ 1.0 2))

;; Reading and writing.
(test 3/2 (round-trip 3/2))
(test -3/2 (read (open-input-string "-12/8")))
(test 3/2 (string->number "#e1.5"))
(test 17/2 (string->number "#x11/2"))
(test "11/10" (number->string 3/2 2))
;; Exponents beyond any float, up to and past the 64-bit range.
(test 0.0 (string->number "1e-9223372036854775808"))
(test 0.0 (string->number "1.5e-99999999999999999999"))
(test +inf.0 (string->number "1e9223372036854775807"))
(test +inf.0 (string->number "1e99999999999999999999"))

;; Exact and inexact.
(test 3602879701896397/36028797018963968 (exact 0.1))
(test 0.1 (inexact (exact 0.1)))
(test (expt 2.0 -1070) (inexact (/ 1 (expt 2 1070))))
(test 1.0 (inexact (/ (expt 10 400) (expt 10 400))))
(test 0.3333333333333333 (inexact 1/3))
(test-error (exact +inf.0))

;; Comparison is exact, also against floats.
(test #t (= 1/2 0.5))
(test #f (< 1/3 0.3333333333333333))
(test #t (< 0.3333333333333333 1/3))
(test #f (eqv? 1/2 0.5))
(test #t (eqv? 1/2 (/ 2 4)))
(test 0.5 (max 1/2 0.3))
(test 'found (let ((h (make-hash-table))) (hash-table-set! h (/ 1 3) 'found) (hash-table-ref/default h 1/3 #f)))
(test 'half (case (/ 2 4) ((1/2) 'half) (else 'other)))

;; Rounding is to an exact integer, halves to even.
(test '(4 2 -2 -4 -3 -3) (list (round 7/2) (round 5/2) (round -5/2) (floor -7/2) (ceiling -7/2) (truncate -7/2)))
(test '(3 2) (list (numerator 6/4) (denominator 6/4)))
(test 1/3 (rationalize (exact .3) 1/10))
(test '(8/27 1/4 8 2/3) (list (expt 2/3 3) (expt 2 -2) (expt 1/2 -3) (sqrt 4/9)))
(test-error (quotient 1/2 1))
(test #f (integer? 1/2))
(test #t (rational? 1/2))

(test-end)
