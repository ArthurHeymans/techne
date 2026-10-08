;;; SRFI 27 random sources.

(import (srfi 27))

(test-begin "random")

;; The same seeds give the same numbers, other seeds others.
(define (draws i j)
  (let ((s (make-random-source)))
    (random-source-pseudo-randomize! s i j)
    (let ((next (random-source-make-integers s)))
      (list (next 1000000) (next 1000000) (next 1000000)))))
(test (draws 1 2) (draws 1 2))
(test #f (equal? (draws 1 2) (draws 2 1)))

;; The states and numbers of the reference implementation (run under Chez),
;; which takes I and J modulo 2^28.
(test '(653781 488477 341926) (draws 1 2))
(test '(102706 892705 695795) (draws (+ 5 (expt 2 28)) 3))
(let ((s (make-random-source)))
  (random-source-pseudo-randomize! s 12345 67890)
  (test '(lecuyer-mrg32k3a 672309889 802603289 17969686 1968657792 700149207 871504585)
        (random-source-state-ref s))
  (random-source-pseudo-randomize! s 0 0)
  (test (random-source-state-ref (make-random-source)) (random-source-state-ref s)))

;; A state read back continues where it was read.
(let* ((s (make-random-source)) (next (random-source-make-integers s)) (state (random-source-state-ref s)))
  (let ((first (list (next 100) (next 100))))
    (random-source-state-set! s state)
    (test first (list (next 100) (next 100))))
  (test-error (random-source-state-set! s '(lecuyer-mrg32k3a 0 0 0 1 1 1)))
  (test-error (random-source-state-set! s '(other 1 2 3 4 5 6))))

;; Integers fall below their bound, bignum bounds included.
(define (all-below? n count)
  (let loop ((i 0))
    (or (= i count) (let ((x (random-integer n))) (and (exact-integer? x) (<= 0 x) (< x n) (loop (+ i 1)))))))
(test #t (all-below? 1 10))
(test #t (all-below? 7 1000))
(test #t (all-below? (expt 10 30) 100))
(test-error (random-integer 0))
(test-error (random-integer 1.5))

;; Floats fall strictly between 0 and 1, at any unit.
(define (all-between? next count)
  (let loop ((i 0)) (or (= i count) (let ((x (next))) (and (inexact? x) (< 0 x 1) (loop (+ i 1)))))))
(test #t (all-between? random-real 1000))
(test #t (all-between? (random-source-make-reals (make-random-source) 1e-30) 100))
(test-error (random-source-make-reals (make-random-source) 2))

;; A state whose next two digits are the largest, m1 - 1: their fraction
;; rounds to 1.0, so the float is drawn again.
(define (inverse-mod a m)
  (let loop ((r0 m) (r1 a) (t0 0) (t1 1))
    (if (zero? r1)
        (modulo t0 m)
        (let ((q (quotient r0 r1)))
          (loop r1 (- r0 (* q r1)) t1 (- t0 (* q t1)))))))
(let ((s (make-random-source)) (m1 4294967087) (m2 4294944443))
  (let ((b (inverse-mod 1370589 m2)))
    (random-source-state-set!
     s (list 'lecuyer-mrg32k3a 810728 1403580 (modulo (* 1403580 1403580 (inverse-mod 810728 m1)) m1)
             0 (modulo (* 527611 b) m2) (modulo (- b) m2))))
  (test #t (< 0 ((random-source-make-reals s 1e-12)) 1)))

;; Randomizing moves a source away from where every new one starts.
(let ((s (make-random-source)))
  (random-source-randomize! s)
  (test #t (random-source? s))
  (test #f (equal? (random-source-state-ref s) (random-source-state-ref (make-random-source)))))

(test-end)
