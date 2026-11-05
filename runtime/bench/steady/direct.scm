;; Control for indirect.scm: only the callback's call form differs.
(define (inc x) (+ x 1))
(define (drive f n)
  (let loop ((i 0) (sum 0))
    (if (= i n) sum (loop (+ i 1) (+ sum (inc i))))))
(define (run) (drive inc 1000000))
