;;; UUIDs (RFC 9562) as strings in their usual form,
;;; "xxxxxxxx-xxxx-Vxxx-Nxxx-xxxxxxxxxxxx": version 4, random, and
;;; version 7, which begins with the time in milliseconds, so that they
;;; sort about in the order they were made and index well.
;;; Both need entropy granted; version 7 the clock too.

(define-library (techne uuid)
  (export uuid-v4 uuid-v7 uuid?)
  (import (techne) (techne crypto))
  (begin

    ;; BYTES, 16 of them, with VERSION in the high half of byte 6 and the
    ;; RFC's variant (binary 10) in the top bits of byte 8, written out.
    (define (%uuid bytes version)
      (bytevector-u8-set! bytes 6 (+ (* 16 version) (modulo (bytevector-u8-ref bytes 6) 16)))
      (bytevector-u8-set! bytes 8 (+ #x80 (modulo (bytevector-u8-ref bytes 8) 64)))
      (let ((h (hex-encode bytes)))
        (string-append (substring h 0 8) "-" (substring h 8 12) "-" (substring h 12 16) "-"
                       (substring h 16 20) "-" (substring h 20 32))))

    (define (uuid-v4)
      "Return a new random UUID (version 4), as a string."
      (%uuid (random-bytes 16) 4))

    (define (uuid-v7)
      "Return a new UUID (version 7) beginning with the time, as a string.
Its first 48 bits are the milliseconds since the Unix epoch, the rest
random: UUIDs made in different milliseconds sort as they were made, as
long as the system's clock is not set back."
      (let ((bytes (random-bytes 16)))
        (do ((i 5 (- i 1)) (ms (current-milliseconds) (quotient ms 256)))
            ((< i 0))
          (bytevector-u8-set! bytes i (modulo ms 256)))
        (%uuid bytes 7)))

    (define (uuid? obj)
      "Return #t if OBJ is a string writing a UUID.
That is 32 hexadecimal digits, in either case, in groups of 8, 4, 4, 4
and 12 separated by `-`."
      (and (string? obj)
           (= (string-length obj) 36)
           (let loop ((i 0))
             (or (= i 36)
                 (and (if (memv i '(8 13 18 23))
                          (char=? (string-ref obj i) #\-)
                          (and (memv (char-downcase (string-ref obj i))
                                     (string->list "0123456789abcdef"))
                               #t))
                      (loop (+ i 1)))))))))
