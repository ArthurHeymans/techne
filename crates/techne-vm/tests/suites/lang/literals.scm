;;; Literals in code cannot be changed; data built at run time can.

(test-begin "literals")

(define (quoted-list) '(1 2 3))
(define (quoted-vector) '#(1 2 3))
(define (set-first! v) (vector-set! v 0 9))

(test-error (set-car! (quoted-list) 9))
(test-error (set-cdr! (quoted-list) 9))
(test-error (list-set! (quoted-list) 1 9))
(test-error (vector-set! (quoted-vector) 0 9))
(test-error (vector-fill! (quoted-vector) 0))
(test-error (vector-copy! (quoted-vector) 0 #(4)))
(test-error (string-set! "abc" 0 #\x))
(test-error (bytevector-u8-set! #u8(1) 0 2))
;; Also through code the JIT compiles, after it has run on vectors that
;; can be changed.
(let loop ((i 0)) (when (< i 2000) (set-first! (vector 1 2)) (loop (+ i 1))))
(test-error (set-first! (quoted-vector)))
(test '(1 2 3) (quoted-list))
(test '#(1 2 3) (quoted-vector))

;; What read, list, vector and list-copy make can be changed.
(test '(9 2) (let ((l (read (open-input-string "(1 2)")))) (set-car! l 9) l))
(test '(9 2) (let ((l (list-copy '(1 2)))) (set-car! l 9) l))
(test '#(9 2) (let ((v (vector-copy '#(1 2)))) (set-first! v) v))
(test '#(9 2) (let ((v (vector 1 2))) (set-first! v) v))

(test-end)
