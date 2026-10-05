;; Call overhead and small-integer arithmetic.
(define (fib n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
(out (fib 32))
