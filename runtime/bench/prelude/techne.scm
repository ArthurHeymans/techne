;; techne-vm prelude: native mutable hash tables and a native line reader.
(define (out x) (display x) (newline))
(define ht-new make-hash-table)
(define ht-ref hash-table-ref)
(define ht-set! hash-table-set!)
(define ht-count hash-table-count)
(define file->lines read-lines)
