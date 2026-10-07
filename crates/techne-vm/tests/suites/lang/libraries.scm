;;; R7RS libraries: define-library, import sets, include and cond-expand.

(test-begin "libraries")

(import (scheme base) (lib pair-tools))
(test '(2 . 1) (swap '(1 . 2)))
(test 42 answer)
(test '(a a) (twice 'a))

(import (prefix (only (lib pair-tools) swap) p:))
(test '(b . a) (p:swap '(a . b)))

(define-library (inline counter)
  (export next!)
  (import (scheme base))
  (begin
    (define n 0)
    (define (next!) (set! n (+ n 1)) n)))
(import (rename (inline counter) (next! tick)))
(test 1 (tick))
(test 2 (tick))

;; A library sees only what it imports, may re-export it, and knows all of
;; its own definitions before any of them compiles.
(define-library (isolated)
  (export probe first-of (rename car head))
  (import (only (scheme base) define lambda list car guard))
  (begin
    (define (probe) (guard (e (#t 'unbound)) (cadr (list 1 2))))
    (define (first-of l) (map l))
    (define (map l) (car l))))
(import (prefix (isolated) i:))
(test 'unbound (i:probe))
(test 1 (i:first-of '(1 2)))
(test 'x (i:head '(x)))

;; Environments are separate, and see only their imports.
(define env1 (environment '(scheme base)))
(eval '(define shared 1) env1)
(test #f (guard (e (#t #f)) (eval 'shared (environment '(scheme base)))))
(test 1 (eval 'shared env1))
(test 1024 (eval '(expt 2 10) (environment '(scheme base))))
(test #f (guard (e (#t #f)) (eval '(sin 0) (environment '(scheme base)))))

(test 'yes (cond-expand ((and r7rs (library (lib pair-tools))) 'yes) (else 'no)))
(test 'no (cond-expand ((library (scheme complex)) 'yes) (else 'no)))
(test 'no (cond-expand ((library (no such)) 'yes) (else 'no)))
(test-error (eval '(import (no such))))

(test-end)
