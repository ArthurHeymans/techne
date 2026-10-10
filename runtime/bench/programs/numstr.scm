;; number->string in a loop: the /work request of the HTTP benchmark
;; (http/), where JIT code calls a native that allocates a string.
(define (work n)
  (let loop ((i 0) (acc 0))
    (if (= i n)
        acc
        (loop (+ i 1) (+ acc (string-length (number->string (* i i))))))))
(define (repeat k acc) (if (= k 0) acc (repeat (- k 1) (+ acc (work 1000)))))
(out (repeat 1000 0))
