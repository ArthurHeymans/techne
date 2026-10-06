;;; syntax-rules patterns beyond the basic cases in tests/language.rs.

(test-begin "syntax-rules")

(define-syntax rest-of (syntax-rules () ((_ . args) 'args)))
(test '(1 2) (rest-of 1 2))
(test '() (rest-of))

(define-syntax tail (syntax-rules () ((_ a . b) 'b)))
(test '(2 3) (tail 1 2 3))
(test 4 (tail 1 . 4))

(define-syntax my-let*
  (syntax-rules ()
    ((_ () body ...) (let () body ...))
    ((_ ((x v) rest ...) body ...) (let ((x v)) (my-let* (rest ...) body ...)))))
(test 3 (my-let* ((a 1) (b (+ a 1))) (+ a b)))

(test-end)
