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

;; A single non-ASCII character must not turn indexed scans quadratic.
;; Include string-length in the condition: it too must be constant time.
(define mixed (string-append (make-string 40000 #\a) "é"))
(test 40000
  (let loop ((i 0) (ascii 0))
    (if (= i (string-length mixed))
        ascii
        (loop (+ i 1) (+ ascii (if (char=? (string-ref mixed i) #\a) 1 0))))))
(test #\é (string-ref mixed 40000))
(test-error (string-ref mixed 40001))

;; Equal-byte-size replacements can move internal character boundaries.
;; Rebuild checkpoints even when the byte buffer itself does not grow.
(define indexed (string-append (make-string 63 #\a) "a€" (make-string 130 #\b)))
(string-copy! indexed 63 "éé")
(test "éébb" (substring indexed 63 67))
(test #\é (string-ref indexed 64))
(test 195 (string-length indexed))
(collect-garbage 'full)
(test #\b (string-ref indexed 128))
;; Growing, shrinking, and mutation through an existing indirection.
(string-set! indexed 0 #\😀)
(string-set! indexed 1 #\é)
(string-set! indexed 0 #\a)
(string-copy! indexed 63 "a€")
(test "a€bb" (substring indexed 63 67))
(test 195 (string-length indexed))
(test #\é (string-ref indexed 1))
(collect-garbage 'full)
(test #\€ (string-ref indexed 64))

(test-end)
