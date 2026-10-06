;;; A library with a renamed export, an included file, an exported macro and
;;; an import of another library (tests/suites/lang/libraries.scm).
(define-library (fixtures geom point)
  (export make-point point-x point-y (rename norm2 squared-norm) twice)
  (import (scheme base) (fixtures util math))
  (include "point-impl.scm")
  (begin
    (define-syntax twice (syntax-rules () ((_ e) (begin e e))))
    (define (norm2 p) (+ (sq (point-x p)) (sq (point-y p))))))
