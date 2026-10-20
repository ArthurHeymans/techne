;;; The output a shell command is waited for keeps: at most its limit,
;;; truncation said as soon as anything is dropped, also within a piece.
;;;
;;; Run by crates/techne-editor/tests/lisp.rs, which provides the natives.

(require "../../test.scm")
(require "../shell.scm")

(define (collect limit . pieces)
  (let ((c (output-collector #:limit limit)))
    (for-each (car c) pieces)
    (list ((cadr c)) ((caddr c)))))

(check "within the limit" '("abc" #f) (collect 4 "ab" "c"))
(check "exactly the limit" '("abcd" #f) (collect 4 "ab" "cd"))
(check "a piece crossing the limit" '("abcd" #t) (collect 4 "ab" "cde"))
(check "pieces after the limit" '("abcd" #t) (collect 4 "abcd" "e" "f"))

(test-failures)
