;;; SRFI 27: sources of random bits.
;;;
;;; L'Ecuyer's MRG32k3a, as in the SRFI's reference implementation: the
;;; same initial state, state representation and jumps ahead for
;;; pseudo-randomizing, so the numbers are the reference's. A library rather
;;; than part of the prelude, so that only programs importing it compile it.

(define-library (srfi 27)
  (export random-integer random-real default-random-source make-random-source random-source?
          random-source-state-ref random-source-state-set! random-source-randomize!
          random-source-pseudo-randomize! random-source-make-integers random-source-make-reals)
  (import (techne))
  (begin
    ;; The state is three integers below m1 and three below m2, not all zero
    ;; in either group, and each step gives an integer below m1.

    (define-record-type random-source
      (%make-random-source state)
      random-source?
      ;; #(x11 x12 x13 x21 x22 x23), newest first, changed in place
      (state %random-source-state))

    (define %m1 4294967087)
    (define %m2 4294944443)

    (define (%random-step! v)
      (let ((x1 (modulo (- (* 1403580 (vector-ref v 1)) (* 810728 (vector-ref v 2))) %m1))
            (x2 (modulo (- (* 527612 (vector-ref v 3)) (* 1370589 (vector-ref v 5))) %m2)))
        (vector-copy! v 1 v 0 2)
        (vector-copy! v 4 v 3 5)
        (vector-set! v 0 x1)
        (vector-set! v 3 x2)
        (modulo (- x1 x2) %m1)))

    ;; An integer below m1^K from K steps.
    (define (%random-digits! v k)
      (do ((i 0 (+ i 1)) (x 0 (+ (* x %m1) (%random-step! v)))) ((= i k) x)))

    (define (%random-below! v n)
      (unless (and (exact-integer? n) (positive? n))
        (error "random-integer: expected a positive exact integer, got" n))
      ;; Enough digits to cover N, rejecting the top of their range that is
      ;; not a whole multiple of N.
      (let loop ((k 1) (mk %m1))
        (if (< mk n)
            (loop (+ k 1) (* mk %m1))
            (let ((q (quotient mk n)))
              (let draw ((x (%random-digits! v k)))
                (if (< x (* q n)) (quotient x q) (draw (%random-digits! v k))))))))

    ;; A float strictly between 0 and 1 from enough digits that consecutive
    ;; values are at most UNIT apart. A fraction closer to 1 than a float
    ;; can be rounds to 1, which is drawn again.
    (define (%random-real! v unit)
      (let loop ((k 1) (mk %m1))
        (if (< (* mk unit) 1)
            (loop (+ k 1) (* mk %m1))
            (let draw ()
              (let ((x (inexact (/ (+ 1 (%random-digits! v k)) (+ mk 1)))))
                (if (< x 1.0) x (draw)))))))

    ;; Six state integers from the exact integers SEEDS, for randomizing: each
    ;; mixes them all by multiplying and adding modulo the prime 2^61 - 1.
    (define (%random-seeded-state seeds)
      (let* ((p 2305843009213693951)
             (mix (lambda (h x) (modulo (+ (* (+ h x) 6364136223846793005) 1442695040888963407) p)))
             (h (fold (lambda (x h) (mix (mix h x) 0)) 0 seeds)))
        (list->vector
         (map (lambda (k m) (+ 1 (modulo (mix (mix h k) 0) (- m 1))))
              '(1 2 3 4 5 6) (list %m1 %m1 %m1 %m2 %m2 %m2)))))

    (define (make-random-source)
      "Return a new random source, in the state every new source starts in."
      ;; 16 steps on, as pseudo-randomizing with 0 and 0.
      (%make-random-source (vector 1062452522 2961816100 342112271 2854655037 3321940838 3542344109)))

    (define (random-source-state-ref s)
      "Return the state of random source S, as data that can be written out."
      (cons 'lecuyer-mrg32k3a (vector->list (%random-source-state s))))

    (define (random-source-state-set! s state)
      "Put random source S back in STATE, from `random-source-state-ref`."
      (define (below? x m) (and (exact-integer? x) (<= 0 x (- m 1))))
      (unless (and (list? state) (= (length state) 7) (eq? (car state) 'lecuyer-mrg32k3a)
                   (every identity (map below? (cdr state) (list %m1 %m1 %m1 %m2 %m2 %m2)))
                   (let ((v (list->vector (cdr state))))
                     (and (positive? (+ (vector-ref v 0) (vector-ref v 1) (vector-ref v 2)))
                          (positive? (+ (vector-ref v 3) (vector-ref v 4) (vector-ref v 5))))))
        (error "random-source-state-set!: not a state:" state))
      (vector-copy! (%random-source-state s) 0 (list->vector (cdr state))))

    (define (random-source-randomize! s)
      "Put random source S in a state that depends on the time."
      (vector-copy! (%random-source-state s) 0
                    (%random-seeded-state (append (list (current-jiffy) (exact (floor (* 1000000 (current-second)))))
                                                  (vector->list (%random-source-state s))))))

    ;; The step as two 3x3 matrices, row by row: the first modulo m1, the
    ;; second modulo m2. The state after N steps from (1 0 0 1 0 0) is the
    ;; first column of their Nth powers.
    (define %random-step-matrix
      '#(0 1403580 4294156359 1 0 0 0 1 0 527612 0 4293573854 1 0 0 0 1 0))

    (define (%random-matrix-product a b)
      (let ((c (make-vector 18)))
        (do ((h 0 (+ h 9))) ((= h 18) c)
          (do ((r h (+ r 3))) ((= r (+ h 9)))
            (do ((k 0 (+ k 1))) ((= k 3))
              (vector-set! c (+ r k)
                           (modulo (+ (* (vector-ref a r) (vector-ref b (+ h k)))
                                      (* (vector-ref a (+ r 1)) (vector-ref b (+ h 3 k)))
                                      (* (vector-ref a (+ r 2)) (vector-ref b (+ h 6 k))))
                                   (if (= h 0) %m1 %m2))))))))

    ;; The state N steps after (1 0 0 1 0 0), by repeated squaring.
    (define (%random-state-after n)
      (let loop ((a %random-step-matrix) (n n) (acc '#(1 0 0 0 1 0 0 0 1 1 0 0 0 1 0 0 0 1)))
        (if (zero? n)
            (vector (vector-ref acc 0) (vector-ref acc 3) (vector-ref acc 6)
                    (vector-ref acc 9) (vector-ref acc 12) (vector-ref acc 15))
            (loop (%random-matrix-product a a) (quotient n 2) (if (odd? n) (%random-matrix-product acc a) acc)))))

    (define (random-source-pseudo-randomize! s i j)
      "Put random source S in the state given by the exact integers I and J.
As in the reference implementation, the state is 16 + I * 2^127 + J *
2^76 steps on, with I and J taken modulo 2^28: the sources of different
I and J are far apart in one sequence, and do not overlap."
      (unless (and (exact-integer? i) (exact-integer? j))
        (error "random-source-pseudo-randomize!: expected exact integers, got" i j))
      (let ((n (+ 16 (* (modulo i (expt 2 28)) (expt 2 127)) (* (modulo j (expt 2 28)) (expt 2 76)))))
        (vector-copy! (%random-source-state s) 0 (%random-state-after n))))

    (define (random-source-make-integers s)
      "Return a procedure of N giving random integers from 0 below N from S."
      (lambda (n) (%random-below! (%random-source-state s) n)))

    (define (random-source-make-reals s [unit #f])
      "Return a procedure giving random floats between 0 and 1 from S.
The floats exclude 0 and 1. With UNIT, a number between 0 and 1, they
are at most UNIT apart, or as close as floats get where that is finer
than their precision."
      (unless (or (not unit) (and (real? unit) (< 0 unit 1)))
        (error "random-source-make-reals: expected a number between 0 and 1, got" unit))
      (lambda () (%random-real! (%random-source-state s) (or unit (/ 1 %m1)))))

    (define default-random-source (make-random-source)
      "The random source of `random-integer` and `random-real`.")

    (define (random-integer n)
      "Return a random integer from 0 below N, from `default-random-source`."
      (%random-below! (%random-source-state default-random-source) n))

    (define (random-real)
      "Return a random float between 0 and 1, from `default-random-source`.
The float is neither 0 nor 1."
      (%random-real! (%random-source-state default-random-source) (/ 1 %m1)))))
