;; Negative control: first-class division must remain exact, not a float
;; optimization of a different arithmetic contract.
(define (drive f n)
  (let loop ((i 0) (last 0))
    (if (= i n) last (loop (+ i 1) (f (+ i 7) 3)))))
(define (run) (drive / 200000))
