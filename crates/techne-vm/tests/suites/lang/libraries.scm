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

(test 'yes (cond-expand ((and r7rs (library (lib pair-tools))) 'yes) (else 'no)))
(test 'no (cond-expand ((library (no such)) 'yes) (else 'no)))
(test-error (eval '(import (no such))))

(test-end)
