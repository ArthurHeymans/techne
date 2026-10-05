;; Floating-point loop.
(define (iter cr ci max)
  (let loop ((zr 0.0) (zi 0.0) (i 0))
    (if (or (= i max) (> (+ (* zr zr) (* zi zi)) 4.0)) i
        (loop (+ (- (* zr zr) (* zi zi)) cr) (+ (* 2.0 zr zi) ci) (+ i 1)))))
(define size 250)
(define (row y acc)
  (let loop ((x 0) (acc acc))
    (if (= x size) acc
        (loop (+ x 1)
              (+ acc (iter (- (* 3.0 (/ (exact->inexact x) size)) 2.0)
                           (- (* 2.0 (/ (exact->inexact y) size)) 1.0) 200))))))
(define (rows y acc) (if (= y size) acc (rows (+ y 1) (row y acc))))
(out (rows 0 0))
