(define-library (fixtures util math)
  (export sq cube)
  (import (scheme base))
  (cond-expand
    (techne (begin (define (sq x) (* x x))))
    (else (begin (define (sq x) 'not-techne))))
  (begin (define (cube x) (* x (sq x)))))
