;; Deep non-tail recursion with three arguments.
(define (tak x y z)
  (if (not (< y x)) z
      (tak (tak (- x 1) y z) (tak (- y 1) z x) (tak (- z 1) x y))))
(define (repeat n acc) (if (= n 0) acc (repeat (- n 1) (+ acc (tak 22 16 8)))))
(out (repeat 10 0))
