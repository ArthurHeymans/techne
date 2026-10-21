;;; Higher-order natives must leave callbacks on the Scheme stack.

(test-begin "suspending comparators")

(test '(1 2)
      (task-join
       (spawn (lambda ()
                (member 1 (list 1 2)
                        (lambda (a b) (yield) (= a b)))))))
(test '(2 . found)
      (task-join
       (spawn (lambda ()
                (assoc 2 (list 'skip (cons 1 'no) (cons 2 'found))
                       (lambda (a b) (yield) (= a b)))))))
(test #f (task-join (spawn (lambda () (member 3 '(1 2) (lambda (a b) (yield) (= a b)))))))

;; Both hashing and equality can wait, including while growth rehashes all
;; the keys. One hash value makes the probe visit collisions and tombstones.
(test '(40 17 #f missing 23)
      (task-join
       (spawn
        (lambda ()
          (let ((table (make-hash-table
                        (lambda (a b) (yield) (string=? a b))
                        (lambda (key) (yield) 0))))
            (do ((i 0 (+ i 1))) ((= i 40))
              (hash-table-set! table (number->string i) i))
            (let ((count (hash-table-count table))
                  (value (hash-table-ref/default table (string-copy "17") #f)))
              (hash-table-delete! table "17")
              (hash-table-set! table "23" 23)
              (list count value
                    (hash-table-contains? table "17")
                    (hash-table-ref/default table "17" 'missing)
                    (hash-table-ref/default (hash-table-copy table) "23" #f))))))))

;; A callback that really waits, rather than just relinquishing its slice.
(let ((channel (make-channel)))
  (define query
    (spawn (lambda ()
             (let ((table (make-hash-table (lambda (a b) (channel-recv channel) (string=? a b))
                                           (lambda (key) 0))))
               (hash-table-set! table (string-copy "a") 7)
               (hash-table-ref/default table (string-copy "a") #f)))))
  (spawn (lambda () (channel-send channel 'ready)))
  (test 7 (task-join query)))

;; Growth must not commit hashes for keys a callback replaced meanwhile.
(test #t
      (let ((table #f) (change #f))
        (set! table (make-hash-table
                     (lambda (a b) (string=? a b))
                     (lambda (key)
                       (when change
                         (set! change #f)
                         (hash-table-delete! table "0")
                         (hash-table-set! table "replacement" 99))
                       0)))
        (do ((i 0 (+ i 1))) ((= i 6))
          (hash-table-set! table (number->string i) i))
        (set! change #t)
        (guard (e (#t #t)) (hash-table-set! table "grow" 6) #f)))

;; A comparator changing a slot the probe already passed, deleting a key
;; and inserting the one sought there, refuses the probe: no key twice.
(test '(refused 2 #t inner)
      (let ((h #f) (change #f))
        (set! h (make-hash-table
                 (lambda (x y)
                   (when (and change (eq? x 'k) (eq? y 'b))
                     (set! change #f)
                     (hash-table-delete! h 'a)
                     (hash-table-set! h 'k 'inner))
                   (eq? x y))
                 (lambda (_) 0)))
        (hash-table-set! h 'a 1)
        (hash-table-set! h 'b 2)
        (set! change #t)
        (let ((outcome (guard (e (#t 'refused)) (hash-table-set! h 'k 'outer) 'set)))
          (list outcome (hash-table-size h) (hash-table-contains? h 'k) (hash-table-ref h 'k)))))

(test-end)
