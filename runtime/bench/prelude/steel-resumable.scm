;; Steel prelude for the resumable runtime, which rejects ports/display.
(define (out x) (stdout-simple-displayln x))
(define (ht-new) (box (hash)))
(define (ht-ref h k d) (let ((v (hash-try-get (unbox h) k))) (if v v d)))
(define (ht-set! h k v) (set-box! h (hash-insert (unbox h) k v)))
(define (ht-count h) (hash-length (unbox h)))
(define (file->lines path) (host-read-lines path))
