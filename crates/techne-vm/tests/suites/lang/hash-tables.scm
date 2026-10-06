;;; Hash tables with keys of every kind of value.

(test-begin "hash-tables")

(define (store key)
  (let ((h (make-hash-table)))
    (hash-table-set! h key 'v)
    h))

(test 'v (hash-table-ref/default (store 1) 1 #f))
(test 'v (hash-table-ref/default (store "s") "s" #f))
(test 'v (hash-table-ref/default (store 'sym) 'sym #f))
(test 'v (hash-table-ref/default (store 1.5) 1.5 #f))
(test 'v (hash-table-ref/default (store (expt 2 70)) (expt 2 70) #f))
(test 'v (hash-table-ref/default (store '(1 2)) (list 1 2) #f))
(test 'v (hash-table-ref/default (store (vector 1 2)) (vector 1 2) #f))
(let ((p (cons 1 2)))
  (test 'v (hash-table-ref/default (store p) p #f)))
(let ((f (lambda () 1)))
  (test 'v (hash-table-ref/default (store f) f #f)))

(test-end)
