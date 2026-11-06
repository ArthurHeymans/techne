;; Same first-class sum as wide.scm, with an inexact accumulator.
(define (drive f seed)
  (let loop ((i 0) (acc seed))
    (if (= i 1000000) acc
        (let ((j (+ i 1))) (loop j (f acc j))))))
(define (run) (drive + 0.5))
