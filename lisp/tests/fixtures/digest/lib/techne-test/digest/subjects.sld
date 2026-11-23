;;; What a program without the editor makes of mail's libraries: the
;;; subjects of messages, decoded.

(define-library (techne-test digest subjects)
  (export subjects)
  (import (scheme base) (techne mail mime))
  (begin

    (define (subjects messages)
      "Return the subject of each of MESSAGES, raw mail as text, decoded."
      (map (lambda (raw) (mail-header (mail-parse raw) "subject")) messages))))
