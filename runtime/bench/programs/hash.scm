;; Hash tables with integer and string keys.
(define n 200000)
(define h (ht-new))
(let loop ((i 0)) (when (< i n) (ht-set! h i (* 2 i)) (loop (+ i 1))))
(define s (ht-new))
(let loop ((i 0)) (when (< i n) (ht-set! s (number->string i) i) (loop (+ i 1))))
(define (sum i acc)
  (if (= i n) acc
      (sum (+ i 1) (+ acc (ht-ref h i 0) (ht-ref s (number->string i) 0)))))
(out (sum 0 0))
(out (+ (ht-count h) (ht-count s)))
