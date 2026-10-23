;; Paired source transformations, NOT compiler implementations. Each procedure
;; takes a bounded iteration count n and a runtime seed. The Rust driver checks
;; both variants against independent arithmetic oracles. No unsafe primitives.

;; Propagate immutable lexical constants, then fold and prune the branch.
(define (constant-let-before n seed)
  (let loop ((i 0) (acc seed))
    (if (= i n) acc
        (let ((k 17) (enabled #t))
          (loop (+ i 1) (+ acc (if enabled (+ i k) (- i k))))))))
(define (constant-let-after n seed)
  (let loop ((i 0) (acc seed))
    (if (= i n) acc (loop (+ i 1) (+ acc (+ i 17))))))

;; Drop unused arithmetic only when its types/ranges prove it cannot raise.
(define (dead-arithmetic-before n seed)
  (let loop ((i 0) (acc seed))
    (if (= i n) acc
        (begin (* i i) (loop (+ i 1) (+ acc i))))))
(define (dead-arithmetic-after n seed)
  (let loop ((i 0) (acc seed))
    (if (= i n) acc (loop (+ i 1) (+ acc i)))))

;; A lexical, immutable, nonescaping function: inline without a global guard.
(define (local-closure-before n seed)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc
        (let ((f (lambda (x) (+ (* x 3) seed))))
          (loop (+ i 1) (+ acc (f i)))))))
(define (local-closure-after n seed)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc (loop (+ i 1) (+ acc (+ (* i 3) seed))))))

;; Separate closure allocation from local-call overhead. Identity is unobserved.
(define (closure-hoist-before n seed)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc
        (let ((f (lambda (x) (+ x seed))))
          (loop (+ i 1) (+ acc (f i)))))))
(define (closure-hoist-after n seed)
  (let ((f (lambda (x) (+ x seed))))
    (let loop ((i 0) (acc 0))
      (if (= i n) acc (loop (+ i 1) (+ acc (f i)))))))

;; Contify a tail-only local recursive group; preserve the per-step arithmetic.
(define (mutual-tail-before n seed)
  (letrec ((even-step (lambda (i acc)
                       (if (= i n) acc (odd-step (+ i 1) (+ acc i)))))
           (odd-step (lambda (i acc)
                      (if (= i n) acc (even-step (+ i 1) (+ acc i))))))
    (even-step 0 seed)))
(define (mutual-tail-after n seed)
  (let loop ((i 0) (acc seed))
    (if (= i n) acc (loop (+ i 1) (+ acc i)))))

;; Scalar-replace a fresh pair whose identity and mutation never escape.
(define (pair-scalar-before n seed)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc
        (let ((p (cons i seed)))
          (loop (+ i 1) (+ acc (+ (car p) (cdr p))))))))
(define (pair-scalar-after n seed)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc (loop (+ i 1) (+ acc (+ i seed))))))

;; Scalar-replace a fresh mutable cell used only by one local body.
(define (vector-scalar-before n seed)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc
        (let ((v (vector i seed)))
          (vector-set! v 0 (+ (vector-ref v 0) 1))
          (loop (+ i 1) (+ acc (+ (vector-ref v 0) (vector-ref v 1))))))))
(define (vector-scalar-after n seed)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc (loop (+ i 1) (+ acc (+ (+ i 1) seed))))))

;; Pure arithmetic CSE. The backend may already find this redundancy.
(define (cse-arithmetic-before n seed)
  (let loop ((i 0) (acc seed))
    (if (= i n) acc
        (loop (+ i 1) (+ acc (+ (* i i) (* i i)))))))
(define (cse-arithmetic-after n seed)
  (let loop ((i 0) (acc seed))
    (if (= i n) acc
        (let ((square (* i i)))
          (loop (+ i 1) (+ acc (+ square square)))))))

;; Load CSE with no intervening write, call, collection or possible alias store.
(define (cse-vector-before n seed)
  (let ((v (vector seed)))
    (let loop ((i 0) (acc 0))
      (if (= i n) acc
          (loop (+ i 1) (+ acc (+ (vector-ref v 0) (vector-ref v 0))))))))
(define (cse-vector-after n seed)
  (let ((v (vector seed)))
    (let loop ((i 0) (acc 0))
      (if (= i n) acc
          (let ((x (vector-ref v 0)))
            (loop (+ i 1) (+ acc (+ x x))))))))

;; Hoist total invariant arithmetic on an immutable runtime argument. Not a
;; literal: moving a possibly raising operation out of a zero-trip loop is
;; generally wrong. Here seed is an integer and divisor is a nonzero literal.
(define (licm-before n seed)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc
        (loop (+ i 1) (+ acc (+ i (remainder (* seed seed) 97)))))))
(define (licm-after n seed)
  (let ((k (remainder (* seed seed) 97)))
    (let loop ((i 0) (acc 0))
      (if (= i n) acc (loop (+ i 1) (+ acc (+ i k)))))))

;; Local rest-list elimination requires the same nonescape proof as inlining.
(define (rest-list-before n seed)
  (let ((f (lambda (x . rest) (+ x (car rest)))))
    (let loop ((i 0) (acc 0))
      (if (= i n) acc (loop (+ i 1) (+ acc (f i seed)))))))
(define (rest-list-after n seed)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc (loop (+ i 1) (+ acc (+ i seed))))))

;; Negative controls: immediate-lambda beta reduction and literal folding are
;; already done in Techne's front end. Their two sides should cost the same.
(define (beta-control-before n seed)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc
        (loop (+ i 1) (+ acc ((lambda (x) (+ x seed)) i))))))
(define (beta-control-after n seed)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc (let ((x i)) (loop (+ i 1) (+ acc (+ x seed)))))))
(define (fold-control-before n seed)
  (let loop ((i 0) (acc seed))
    (if (= i n) acc (loop (+ i 1) (+ acc (* (+ 2 3) 7))))))
(define (fold-control-after n seed)
  (let loop ((i 0) (acc seed))
    (if (= i n) acc (loop (+ i 1) (+ acc 35)))))
