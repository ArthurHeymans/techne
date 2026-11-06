;; Scalar first-class arithmetic control: the accumulator exceeds fixnums,
;; but stays within signed i64 (and JavaScript's exact integer range).
(define (drive f seed)
  (let loop ((i 0) (acc seed))
    (if (= i 1000000) acc
        (let ((j (+ i 1))) (loop j (f acc j))))))
(define (run) (drive + 140737488355328))
