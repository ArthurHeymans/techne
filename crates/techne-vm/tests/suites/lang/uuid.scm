;;; (techne uuid): RFC 9562 versions 4 and 7.

(import (techne uuid))

(test-begin "uuid")

(define v4 (uuid-v4))
(define v7 (uuid-v7))
(test #t (uuid? v4))
(test #t (uuid? v7))
;; The version digit, and the variant's top bits 10.
(test #\4 (string-ref v4 14))
(test #\7 (string-ref v7 14))
(test #t (and (memv (string-ref v4 19) '(#\8 #\9 #\a #\b)) #t))
(test #t (and (memv (string-ref v7 19) '(#\8 #\9 #\a #\b)) #t))
(test #f (string=? v4 (uuid-v4)))

;; Version 7 begins with the milliseconds, so a later one sorts after.
(define (ms-of u) (string->number (string-append (substring u 0 8) (substring u 9 13)) 16))
(let ((before (current-milliseconds)) (u (uuid-v7)) (after (current-milliseconds)))
  (test #t (<= before (ms-of u) after))
  (let wait () (when (= (current-milliseconds) after) (wait)))
  (test #t (string<? u (uuid-v7))))

(test #f (uuid? "not a uuid"))
(test #f (uuid? (string-append (substring v4 0 35) "g")))
(test #t (uuid? "0189A3B4-C5D6-7E8F-9A0B-1C2D3E4F5A6B"))
(test #f (uuid? 'symbol))

(test-end)
