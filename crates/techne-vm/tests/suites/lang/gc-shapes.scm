;;; Shapes of heap the collector takes apart across pauses: a vector too
;;; large to scan in one slice, changed while it is scanned; chains of
;;; ephemerons, each value the key of the next, kept as far as their first
;;; key is and no further; weak tables found while copying others. Run
;;; under GC stress too, where every pause is a tiny slice.

(test-begin "gc-shapes")

;; A large vector (more fields than one part, `heap::SCAN_PART`), its
;; elements moved about while collections run: every element is still
;; there, each once.
(define size 5000)
(define big (make-vector size #f))
(do ((i 0 (+ i 1))) ((= i size)) (vector-set! big i (list i)))
(do ((round 0 (+ round 1))) ((= round 1))
  (do ((i 0 (+ i 1))) ((= i size))
    (let ((j (modulo (* 7919 (+ i round)) size)))
      (let ((a (vector-ref big i)))
        (vector-set! big i (vector-ref big j))
        (vector-set! big j a)))
    ;; Garbage, so that collections run in between.
    (make-string 16 #\x)))
(test size (length (filter (lambda (x) x) (vector->list big))))
(test (iota size) (sort (map car (vector->list big)) <))

;; A chain of ephemerons: each value is the key of the next.
(define (chain table n)
  (let ((first (list 0)))
    (let loop ((i 0) (key first))
      (if (= i n)
          first
          (let ((next (list (+ i 1))))
            (hash-table-set! table key next)
            (loop (+ i 1) next))))))

(define (churn) (let loop ((i 0)) (when (< i 20000) (cons i i) (loop (+ i 1)))))

;; Kept through its first key: every link.
(define kept (make-weak-hash-table))
(define head (chain kept 300))
(churn)
(collect-garbage)
(test 300 (hash-table-size kept))
(test 300 (let walk ((key head) (n 0)) (let ((next (hash-table-ref/default kept key #f))) (if next (walk next (+ n 1)) n))))

;; Its first key let go: no link is kept.
(define dropped (make-weak-hash-table))
(chain dropped 300)
(churn)
(collect-garbage)
(collect-garbage)
(test 0 (hash-table-size dropped))

;; A weak table reached only through another's value, both young: the
;; collection that copies it finds it while copying, and keeps its
;; entries too.
(define held (list 'held))
(define outer (make-weak-hash-table))
(let ((inner (make-weak-hash-table)))
  (hash-table-set! inner 'live (list 42))
  (hash-table-set! outer held inner))
(collect-garbage)
(test '(42) (hash-table-ref/default (hash-table-ref/default outer held #f) 'live #f))

(test-end)
