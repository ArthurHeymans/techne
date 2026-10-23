;;; Semantic boundaries for resolved-tree optimization.
(test-begin "ir")

;; Argument staging is left-to-right and exactly once, including rest args.
(test '(1 2 3)
  (let ((n 0))
    (let ((f (lambda (a . rest) (cons a rest))))
      (f (begin (set! n (+ n 1)) n)
         (begin (set! n (+ n 1)) n)
         (begin (set! n (+ n 1)) n)))))
(test 8 (let ((f (lambda (x) (set! x (+ x 1)) (+ x x)))) (f 3)))
(test 9 (let ((x 1)) (let ((f (lambda () x))) (set! x 9) (f))))
(test 7 (let ((f (lambda () 1))) (set! f (lambda () 7)) (f)))
(test '(2 3) (let ((f (lambda (x) (lambda () (+ x 1))))) (list ((f 1)) ((f 2)))))

;; Nonescaping scalar fields can change; escaping aliases must remain objects.
(test 11 (let ((v (vector 1 2))) (vector-set! v 0 9) (+ (vector-ref v 0) (vector-ref v 1))))
(test '(9 . 2) (let ((p (cons 1 2))) (let ((q p)) (set-car! q 9) p)))
(test 18 (let ((v (vector 1 2))) (let ((q v)) (vector-set! q 0 9) (+ (vector-ref v 0) (vector-ref v 0)))))
(test 5 (let ((p (cons 3 4))) (let ((f (lambda () (set-car! p 5)))) (f) (car p))))
(test #t (let ((p (cons 1 2))) (eq? p p)))
(test-error (let ((v (vector 1))) (vector-ref v 2)))
(test-error (let ((v (vector 1))) (vector-set! v -1 0)))

;; Equal reads cannot be shared across writes/calls or mutable captures.
(test '(1 2) (let ((p (cons 1 0))) (list (car p) (begin (set-car! p 2) (car p)))))
(test 18 (let ((p (cons 9 0))) (+ (car p) (car p))))
(test 200000000000000000000 (let ((x 10000000000)) (+ (* x x) (* x x))))
(test 0.5 (let ((x 0.5)) (+ (* x x) (* x x))))

;; Dead values still raise, even when their results are unused.
(test-error (begin (car 0) 1))
(test-error (begin (* 'bad 2) 1))
(test-error (begin (quotient 1 0) 1))

;; Contify only nonescaping, correctly applied, tail-only local groups.
(test 1000
  (letrec ((a (lambda (n acc) (if (= n 0) acc (b (- n 1) (+ acc 1)))))
           (b (lambda (n acc) (if (= n 0) acc (a (- n 1) (+ acc 1))))))
    (a 1000 0)))
(test 10 (letrec ((f (lambda (n) (if (= n 0) 0 (+ 1 (f (- n 1))))))) (f 10)))
(test 7 (letrec ((f (lambda () 1))) (set! f (lambda () 7)) (f)))
(test-error (letrec ((f (lambda (n) (if (= n 0) 0 (f))))) (f 1)))
(test 3 (letrec ((f (lambda (n) (if (= n 0) 3 (f (- n 1)))))) ((lambda (g) (g 1)) f)))

;; Invariant arithmetic must not run in a zero-trip loop. Its original test
;; and competing effects/errors retain their order.
(define (ir-invariant n seed)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc
        (loop (+ i 1) (+ acc (+ i (remainder (* seed seed) 97)))))))
(test 0 (ir-invariant 0 'bad))
(test-error (ir-invariant 1 'bad))
(test 95 (ir-invariant 1 17))
(test 191 (ir-invariant 2 17))
(test 'first
  (guard (e ((eq? e 'first) e) (else 'wrong))
    (let loop ((i 0) (acc 0))
      (if (= i 2) acc
          (begin (raise 'first) (loop (+ i 1) (+ acc (remainder (* 'bad 'bad) 97))))))))

(test-end)
