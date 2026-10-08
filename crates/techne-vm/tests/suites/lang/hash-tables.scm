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

;;; Identity hashes survive collections; equivalences; weak tables.

(test-begin "identity and weak tables")

;; An object hashed in the nursery keeps its hash once promoted.
(let* ((p (cons 1 2)) (h (hash-by-identity p)))
  (collect-garbage)
  (test h (hash-by-identity p))
  (collect-garbage 'full)
  (test h (hash-by-identity p)))

(define (fill table keys)
  (for-each (lambda (k) (hash-table-set! table k (list 'value k))) keys)
  table)

;; Lookups by identity survive minor and full collections.
(let* ((keys (list-tabulate 1000 (lambda (i) (vector i))))
       (t (fill (make-hash-table eq?) keys)))
  (collect-garbage)
  (test #t (every (lambda (k) (equal? (list 'value k) (hash-table-ref/default t k #f))) keys))
  (collect-garbage 'full)
  (test #t (every (lambda (k) (equal? (list 'value k) (hash-table-ref/default t k #f))) keys))
  (test #f (hash-table-ref/default t (vector 0) #f)))

;; eqv? tells bignums by value, eq? by identity; equal? looks inside.
(let ((t (make-hash-table eqv?)))
  (hash-table-set! t (expt 2 70) 'big)
  (test 'big (hash-table-ref/default t (expt 2 70) #f)))
(let ((t (make-hash-table)))
  (hash-table-set! t (list "a" #(1 2)) 'structure)
  (test 'structure (hash-table-ref/default t (list "a" (vector 1 2)) #f)))
(let ((t (make-hash-table eq?)))
  (hash-table-set! t (list 1) 'one)
  (test #f (hash-table-ref/default t (list 1) #f)))

;; A circular key hashes.
(let ((c (list 1 2)) (t (make-hash-table)))
  (set-cdr! (cdr c) c)
  (hash-table-set! t c 'cycle)
  (test 'cycle (hash-table-ref/default t c #f)))

;; Weak tables: an entry lasts while its key is reachable from elsewhere,
;; and a value that refers to its own key does not keep it.
(define weak (make-weak-hash-table))
(define kept (vector 'kept))
(hash-table-set! weak kept 'still-here)
(define (add-garbage! n)
  (do ((i 0 (+ i 1))) ((= i n))
    (let ((k (vector i)))
      (hash-table-set! weak k (cons k 'refers-to-its-key)))))
;; (A minor collection may already have promoted the keys, so only a full
;; one is sure to find them dead: tests/api.rs checks minor collections.)
(add-garbage! 100)
(collect-garbage 'full)
(test 1 (hash-table-count weak))
(test 'still-here (hash-table-ref/default weak kept #f))
(add-garbage! 100)
(collect-garbage)
(collect-garbage 'full)
(test 1 (hash-table-count weak))

;; A chain: the value of one entry is the only reference to the next key.
(define chain (make-weak-hash-table))
(let loop ((i 0) (key kept))
  (when (< i 10)
    (let ((next (vector i)))
      (hash-table-set! chain key next)
      (loop (+ i 1) next))))
(collect-garbage 'full)
(test 10 (hash-table-count chain))

(test-end)

;;; SRFI 69: tables with their own equivalence and hash, and the
;;; procedures of the SRFI.

(test-begin "srfi 69")

;; Keys compared by a Scheme procedure stay found as the table grows,
;; collects and loses entries, even when comparing and hashing allocate.
(let ((t (make-hash-table (lambda (a b) (string=? (string-downcase a) (string-downcase b)))
                          (lambda (s) (string-length (string-append s s))))))
  (do ((i 0 (+ i 1))) ((= i 200)) (hash-table-set! t (string-append "K" (number->string i)) i))
  (collect-garbage)
  (do ((i 0 (+ i 2))) ((= i 200)) (hash-table-delete! t (string-append "k" (number->string i))))
  (test 100 (hash-table-count t))
  (test 199 (hash-table-ref/default t "k199" #f))
  (test #f (hash-table-ref/default t "k198" #f)))

;; string-ci=? hashes with string-ci-hash by default; = takes any hash.
(let ((t (make-hash-table string-ci=?)))
  (hash-table-set! t "Cat" 'black)
  (test 'black (hash-table-ref t "CAT"))
  (test string-ci=? (hash-table-equivalence-function t))
  (test string-ci-hash (hash-table-hash-function t)))
(let ((t (make-hash-table = (lambda (x) (exact (truncate x))))))
  (hash-table-set! t 1 'one)
  (test 'one (hash-table-ref/default t 1.0 #f)))
(test-error (make-weak-hash-table string-ci=?))

;; hash-table-ref calls its failure thunk, or its success procedure.
(let ((t (alist->hash-table '((a . 1) (b . 2) (a . 3)) eq?)))
  (test 1 (hash-table-ref t 'a))
  (test 'none (hash-table-ref t 'c (lambda () 'none)))
  (test 10 (hash-table-ref t 'a (lambda () 'none) (lambda (v) (* 10 v))))
  (test-error (hash-table-ref t 'c))
  (test 'escaped (call/cc (lambda (k) (hash-table-ref t 'c (lambda () (k 'escaped))))))
  (hash-table-update! t 'c (lambda (v) (+ v 1)) (lambda () 0))
  (test 1 (hash-table-ref t 'c))
  (test 4 (hash-table-fold t (lambda (k v acc) (+ v acc)) 0))
  (test 4 (hash-table-fold (lambda (k v acc) (+ v acc)) 0 t))
  (test #t (hash-table-exists? t 'b)))

;; A copy has its own entries; merging adds those of another table.
(let* ((t (alist->hash-table '((a . 1)) eq?)) (c (hash-table-copy t #t)))
  (hash-table-set! c 'b 2)
  (test 1 (hash-table-count t))
  (test eq? (hash-table-equivalence-function c))
  (hash-table-merge! t c)
  (test '(1 2) (sort (hash-table-values t) <)))

;; Hashes are below their bound, and equal for strings equal ignoring case.
(test #t (< (hash (list 1 2) 7) 7))
(test (string-ci-hash "abc") (string-ci-hash "ABC"))
(test (string-hash "abc" 100) (string-hash "abc" 100))

(test-end)
