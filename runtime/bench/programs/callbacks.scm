;; Idiomatic list code: lambdas passed to the prelude's higher-order
;; procedures (map, filter, filter-map, fold, for-each, any), as the editor's
;; code is written.
(define (make-items n)
  (let loop ((i 0) (acc '()))
    (if (= i n)
        acc
        (loop (+ i 1) (cons (list i (modulo (* i 7919) 1000) (if (even? i) 'even 'odd)) acc)))))
(define items (make-items 20000))
(define (score item) (+ (car item) (cadr item)))
(define (round k)
  (let* ((kept (filter (lambda (item) (> (cadr item) (* k 10))) items))
         (scores (map (lambda (item) (+ (score item) k)) kept))
         (evens (filter-map (lambda (item) (and (eq? (caddr item) 'even) (cadr item))) kept))
         (total (fold (lambda (s acc) (+ s acc)) 0 scores))
         (high 0))
    (for-each (lambda (s) (when (> s 5000) (set! high (+ high 1)))) scores)
    (list total high (length evens) (if (any (lambda (s) (> s 20990)) scores) 1 0))))
(define (rounds n acc) (if (= n 0) acc (rounds (- n 1) (cons (round n) acc))))
(out (fold (lambda (r acc) (+ acc (apply + r))) 0 (rounds 30 '())))
