;; Steel prelude. Steel has no mutable hash table: a box holding a persistent map
;; is the idiomatic equivalent.
(define (out x) (display x) (newline))
(define (ht-new) (box (hash)))
(define (ht-ref h k d) (let ((v (hash-try-get (unbox h) k))) (if v v d)))
(define (ht-set! h k v) (set-box! h (hash-insert (unbox h) k v)))
(define (ht-count h) (hash-length (unbox h)))
(define (file->lines path)
  (let ((p (open-input-file path)))
    (let loop ((acc '()))
      (let ((l (read-line p)))
        (if (eof-object? l) (reverse acc) (loop (cons l acc)))))))
