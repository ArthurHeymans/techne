;; Same work as direct.scm, but call a procedure passed as an argument.
(define (inc x) (+ x 1))
(define (drive f n)
  (let loop ((i 0) (sum 0))
    (if (= i n) sum (loop (+ i 1) (+ sum (f i))))))
(define (run) (drive inc 1000000))
