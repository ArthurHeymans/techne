(define (fib n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
(define (run) (fib 32))
