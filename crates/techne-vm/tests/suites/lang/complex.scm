;;; Complex numbers: exact and inexact parts, results that leave the reals.

(test-begin "complex")

(define (round-trip x)
  (read (open-input-string (call-with-output-string (lambda (p) (write x p))))))

;; Exact arithmetic stays exact; an exact zero imaginary part is real.
(test -1 (* +i +i))
(test 2 (+ 1+2i 1-2i))
(test #t (exact-integer? (+ 1+2i 1-2i)))
(test 11/25+2/25i (/ 1+2i 3+4i))
(test +2i (expt 1+i 2))
(test-error (/ 1+i 0))

;; Writing and reading.
(test "1+2i" (number->string 1+2i))
(test "+i" (number->string (make-rectangular 0 1)))
(test "1/2-3/4i" (number->string (make-rectangular 1/2 -3/4)))
(test "1.5-2.5i" (number->string 1.5-2.5i))
(test 1/2+3/4i (round-trip 1/2+3/4i))
(test 17+2i (string->number "#x11+2i"))
(test 1 (string->number "1@0"))

;; Leaving the reals.
(test +2i (sqrt -4))
(test 0.0+2.0i (sqrt -4.0))
(test 0.0+1.0i (sqrt -1.0-0.0i))
(test 0.0+3.141592653589793i (log -1))
(test 2.0 (sqrt 4.0))
(test 0.0 (asin 0))

;; Predicates and parts.
(test '(#t #f #f #t) (list (complex? 1+2i) (real? 1+2i) (real? -2.5+0.0i) (real? -2.5+0i)))
(test '(#t #f) (list (exact? 1+2i) (exact? 1.0+2i)))
(test '(1 2 5) (list (real-part 1+2i) (imag-part 1+2i) (magnitude 3+4i)))
(test 0 (angle 1))
(test #t (= 1 1.0 1.0+0.0i))
(test #f (eqv? 1+2i 1.0+2i))
(test 'found (let ((h (make-hash-table))) (hash-table-set! h (make-rectangular 1 2) 'found) (hash-table-ref/default h 1+2i #f)))
(test 3/2+5/2i (exact 1.5+2.5i))
(test-error (< 1+i 2))
(test-error (floor 1+i))

(test-end)
