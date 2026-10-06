;;; A library for tests/suites/lang/libraries.scm.
(define-library (lib pair-tools)
  (export swap (rename secret answer) twice)
  (import (scheme base))
  (include "pair-tools-body.scm")
  (begin
    (define secret 42)
    (define-syntax twice (syntax-rules () ((_ x) (list x x))))))
