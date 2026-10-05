;; Rust boundary: 2M calls to a registered Rust function vs a Scheme function.
;; Only runs under the bench driver, which registers host-add.
(define (scheme-add a b) (+ a b))
(define (loop-host i acc) (if (= i 0) acc (loop-host (- i 1) (host-add acc 1))))
(define (loop-scheme i acc) (if (= i 0) acc (loop-scheme (- i 1) (scheme-add acc 1))))
(out (loop-host 2000000 0))
(out (loop-scheme 2000000 0))
