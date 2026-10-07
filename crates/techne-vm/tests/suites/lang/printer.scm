;;; write produces text read gives back as an equal datum.

(define (round-trip x)
  (read (open-input-string (call-with-output-string (lambda (p) (write x p))))))

(test-begin "printer")

(test '(1 "two" #\3 4.5 sym) (round-trip '(1 "two" #\3 4.5 sym)))
(test "a\nb\t\"c\"\\" (round-trip "a\nb\t\"c\"\\"))
(test (string #\x0 #\x7f) (round-trip (string #\x0 #\x7f)))
(test "\"\\x0;\"" (call-with-output-string (lambda (p) (write (string #\x0) p))))
(test (string->symbol "a b") (round-trip (string->symbol "a b")))
(test (string->symbol "") (round-trip (string->symbol "")))
(test (string->symbol "1") (round-trip (string->symbol "1")))
(test '#(1 (2 . 3) #(4)) (round-trip '#(1 (2 . 3) #(4))))
(test (list #\space #\newline #\x3bb) (round-trip (list #\space #\newline #\x3bb)))
(test (expt 2 100) (round-trip (expt 2 100)))
(test -0.0 (round-trip -0.0))

;; Cycles are written with datum labels, and read back.
(define (circular . xs) (let ((l (list-copy xs))) (set-cdr! (last-pair l) l) l))
(test "#0=(1 2 . #0#)" (call-with-output-string (lambda (p) (write (circular 1 2) p))))
(test #t (equal? (circular 1 2) (round-trip (circular 1 2))))
(test #t (let ((v (vector 1 #f))) (vector-set! v 1 v) (equal? v (round-trip v))))
(test "((1) (1))" (call-with-output-string (lambda (p) (let ((s (list 1))) (write (list s s) p)))))
(test #f (list? (circular 1 2)))
(test #t (equal? (circular 1 2) (circular 1 2 1 2)))

(test-end)
