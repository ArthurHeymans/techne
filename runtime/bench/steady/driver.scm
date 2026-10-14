;; Steady state: run the program's `run` N times in one world, printing its
;; result once and each iteration's microseconds on one line.
(define (bench n)
  (let loop ((i 0) (times '()) (result #f))
    (if (= i n)
        (begin (out result) (for-each (lambda (t) (display t) (display " ")) (reverse times)) (newline))
        (let* ((t0 (current-jiffy)) (r (run)) (t1 (current-jiffy)))
          (loop (+ i 1) (cons (- t1 t0) times) r)))))
(bench (string->number (or (get-environment-variable "ITERS") "20")))
