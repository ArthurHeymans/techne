;;; Strings change in place at any UTF-8 size and keep their identity.

(test-begin "strings")

(define s (string #\a #\b #\c))
(define same s)
(string-set! s 1 #\🜀)
(test "a🜀c" s)
(test #t (eq? s same))
(test '(3 #\🜀 #\c) (list (string-length s) (string-ref s 1) (string-ref s 2)))
(string-set! s 1 #\b)
(test "abc" s)
(test "λλλ" (let ((u (make-string 3 #\x))) (string-fill! u #\λ) u))
(test "-αβγ-" (let ((v (make-string 5 #\-))) (string-copy! v 1 "αβγ") v))
(test-error (string-set! "abc" 0 #\λ))

;; An eq? table finds a string by identity after it changed size, also
;; once collections have moved it.
(define h (make-hash-table eq?))
(define k (make-string 30 #\a))
(hash-table-set! h k 'found)
(collect-garbage)
(string-set! k 0 #\é)
(collect-garbage 'full)
(test 'found (hash-table-ref/default h k #f))
(test #\é (string-ref k 0))

;; Many strings changing size while collections run.
(define strings
  (let loop ((i 0) (acc '()))
    (if (= i 2000)
        acc
        (let ((x (make-string 20 #\a)))
          (string-set! x 3 #\λ)
          (string-set! x 5 #\😀)
          (loop (+ i 1) (cons x acc))))))
(collect-garbage 'full)
(test 40000 (apply + (map string-length strings)))
(test #t (every (lambda (x) (equal? x "aaaλa😀aaaaaaaaaaaaaa")) strings))

(test-end)
