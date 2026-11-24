;;; Shared, entirely synthetic cache for editor, terminal and window navigation.
(import (scheme base) (srfi 1) (techne editor) (mail store))
(provide mail-test-seed-navigation!)

(define (mail-test-seed-navigation! path count)
  "Seed PATH with COUNT synthetic messages, of no configured account."
  (let ((db (mail-store-open path)))
    (let loop ((remaining (reverse (iota count))))
      (unless (null? remaining)
        (let ((n (min 15 (length remaining))))
          (mail-store-import-batch! db "synthetic" "INBOX"
            (map (lambda (i)
                   (let* ((key (number->string i))
                          (subject (string-append "Nav item " (make-string (- 3 (string-length key)) #\0) key)))
                     `((key . ,key) (flags . ())
                       (raw . ,(string-append "From: Example <sender@example.invalid>\nTo: reader@example.invalid\nDate: Fri, 06 Nov 2026 12:00:00 +0000\nSubject: " subject
                                              "\nMessage-ID: <nav-" key "@example.invalid>\n\nSynthetic body for " subject ".\n")))))
                 (take remaining n)))
          (loop (drop remaining n)))))
    (mail-store-close db))
  (set-option! 'mail-database path)
  (set-option! 'mail-accounts '()))
