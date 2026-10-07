;;; Bytevectors beyond the R7RS suite: literals, UTF-8 policy, hashing,
;;; binary ports.

(test-begin "bytevectors")

(define (round-trip x)
  (read (open-input-string (call-with-output-string (lambda (p) (write x p))))))

;; Syntax: bytes in any radix, written back in decimal.
(test #u8(255 0 3) (read (open-input-string "#u8(#xff 0 #b11)")))
(test "#u8(0 128 255)" (call-with-output-string (lambda (p) (write (bytevector 0 128 255) p))))
(test #u8(1 2 3) (round-trip (bytevector 1 2 3)))
(test-error (read (open-input-string "#u8(256)")))
(test-error (read (open-input-string "#u8(1.0)")))

;; Literals cannot be changed; made ones can.
(test-error (bytevector-u8-set! #u8(1 2) 0 9))
(test #u8(9 2) (let ((b (bytevector 1 2))) (bytevector-u8-set! b 0 9) b))
(test-error (bytevector 256))
(test-error (bytevector -1))

;; equal? compares bytes, eqv? identity; equal? tables find by contents.
(test #t (equal? (bytevector 1 2) #u8(1 2)))
(test #f (eqv? (bytevector 1 2) (bytevector 1 2)))
(test #f (equal? #u8(97) "a"))
(test 'found (let ((h (make-hash-table)))
               (hash-table-set! h (list (bytevector 1 2) 3) 'found)
               (hash-table-ref/default h (list #u8(1 2) 3) #f)))

;; utf8->string refuses invalid UTF-8; ranges are bytes there, characters
;; for string->utf8.
(test-error (utf8->string #u8(#xce)))
(test "λ" (utf8->string #u8(0 #xce #xbb 0) 1 3))
(test #u8(#xce #xbb) (string->utf8 "aλb" 1 2))

;; Binary ports: bytes in, bytes out, partial reads, end of input.
(test '(#u8(1 2) #u8(3) #t)
      (let ((p (open-input-bytevector #u8(1 2 3))))
        (let* ((a (read-bytevector 2 p)) (b (read-bytevector 2 p)))
          (list a b (eof-object? (read-bytevector 2 p))))))
(test '(#t #f #f)
      (let ((p (open-output-bytevector)))
        (list (binary-port? p) (textual-port? p) (begin (close-port p) (textual-port? p)))))
(test-error (read-char (open-input-bytevector #u8(65))))
(test-error (write-u8 1 (open-output-string)))
(test #u8(1 2 3) (let ((p (open-output-bytevector)))
                   (parameterize ((current-output-port p)) (write-u8 1) (write-bytevector #u8(2 3)))
                   (get-output-bytevector p)))

(test-end)
