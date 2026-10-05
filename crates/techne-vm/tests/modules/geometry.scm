(provide make-point point-x point-y distance2 square-macro)
(define-record-type point (make-point x y) point? (x point-x) (y point-y))
(define (helper v) (* v v))
(define (distance2 a b)
  (+ (helper (- (point-x a) (point-x b))) (helper (- (point-y a) (point-y b)))))
(define-syntax square-macro
  (syntax-rules () ((_ e) (helper e))))
