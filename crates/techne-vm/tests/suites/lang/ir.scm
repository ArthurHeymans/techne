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

;; Integer facts do not prove total arithmetic: this final multiplication
;; exceeds the VM's exact-number limit even though its result is discarded.
(define (ir-dead-square binding?)
  (let loop ((i 0) (x 2))
    (if (= i 19)
        (if binding?
            (let ((unused (* x x))) 'survived)
            (begin (* x x) 'survived))
        (loop (+ i 1) (* x x)))))
(test "*: exact numbers are limited to 1048576 bits"
  (guard (e (#t (error-object-message e))) (ir-dead-square #f)))
(test "*: exact numbers are limited to 1048576 bits"
  (guard (e (#t (error-object-message e))) (ir-dead-square #t)))

;; Techne preserves boxed-number identity, even though R7RS leaves numerical
;; eq? unspecified. Propagating a literal must not allocate it again; CSE must
;; not merge fresh arithmetic results. Include both sides of the fixnum range.
(test #t (let ((x 1125899906842624)) (eq? x x)))
(test #t (let ((x -1125899906842624)) (eq? x x)))
(define (ir-fresh-sum x) (eq? (+ x 1) (+ x 1)))
(test #f (ir-fresh-sum 1125899906842624))
(test #f (ir-fresh-sum -1125899906842624))
;; Propagating immediate operands must not fold a per-call allocation into
;; one boxed constant shared by all invocations of the function.
(define (ir-folded-product) (let ((x 100000000)) (* x x)))
(test #f (eq? (ir-folded-product) (ir-folded-product)))
;; Copying a callee or guarded loop exit must not duplicate its heap literal.
(test #t
  (let ((f (lambda () 1125899906842624)))
    (eq? (f) (f))))
(define (ir-literal-exit n seed)
  (let loop ((i 0) (acc 0))
    (if (= i n) 1125899906842624
        (loop (+ i 1) (+ acc (remainder (* seed seed) 97))))))
(test #t (eq? (ir-literal-exit 0 17) (ir-literal-exit 1 17)))

;; A loop back edge can run another task. Immutable bindings do not make the
;; pair/vector they reference immutable, including reads in the exit test.
(define (ir-sum-cell cell n)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc
        (loop (+ i 1) (+ acc (* (vector-ref cell 0) 1))))))
(test-assert
  (let* ((cell (vector 1))
         (reader (spawn (lambda () (ir-sum-cell cell 1000000))))
         (writer (spawn (lambda () (vector-set! cell 0 0))))
         (sum (task-join reader)))
    (task-join writer)
    (and (> sum 0) (< sum 1000000))))
(define (ir-sum-pair cell n)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc
        (loop (+ i 1) (+ acc (* (car cell) 1))))))
(test-assert
  (let* ((cell (cons 1 0))
         (reader (spawn (lambda () (ir-sum-pair cell 1000000))))
         (writer (spawn (lambda () (set-car! cell 0))))
         (sum (task-join reader)))
    (task-join writer)
    (and (> sum 0) (< sum 1000000))))
(define (ir-watch-cell cell)
  (let loop ((i 0) (acc 0))
    (if (= i (* (vector-ref cell 0) 1)) i
        (loop (+ i 1) (+ acc (* (vector-ref cell 0) 1))))))
(test 10000
  (let* ((cell (vector 1000000))
         (reader (spawn (lambda () (ir-watch-cell cell))))
         (writer (spawn (lambda () (vector-set! cell 0 10000))))
         (count (task-join reader)))
    (task-join writer)
    count))

(test-end)
