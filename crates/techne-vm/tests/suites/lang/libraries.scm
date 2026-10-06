;;; R7RS libraries over the module system (fixtures/), and cond-expand.

(test-begin "libraries")

(import (scheme base) (fixtures geom point))
(test 25 (squared-norm (make-point 3 4)))
(test 2 (let ((n 0)) (twice (set! n (+ n 1))) n))

(import (prefix (only (fixtures util math) sq) m:))
(test 49 (m:sq 7))
(test #f (guard (e (#t #f)) (cube 2)))

(import (rename (fixtures util math) (cube third-power)))
(test 8 (third-power 2))

(import (except (fixtures util math) cube))
(test 9 (sq 3))

(test 'yes (cond-expand ((and r7rs (not no-such-feature)) 'yes) (else 'no)))
(test 'have (cond-expand ((library (fixtures util math)) 'have) (else 'missing)))
(test 'missing (cond-expand ((library (fixtures none)) 'have) (else 'missing)))
(test #t (guard (e ((error-object? e) #t)) (eval '(import (fixtures none))) #f))

(test-end)
