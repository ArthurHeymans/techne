;;; The output a shell command is waited for keeps: at most its limit,
;;; truncation said as soon as anything is dropped, also within a piece.
;;; The output of one in the background: its last bytes, cut at a line's
;;; start, else a character's, so each cut drops some.
;;;
;;; Run by crates/techne-editor/tests/lisp.rs, which provides the natives.

(require "../../test.scm")
(require "../shell.scm")

(define (collect limit . pieces)
  (let ((c (output-collector #:limit limit)))
    (for-each (car c) pieces)
    (list ((cadr c)) ((caddr c)))))

(check "within the limit" '("abc" #f) (collect 4 "ab" "c"))
(check "exactly the limit" '("abcd" #f) (collect 4 "ab" "cd"))
(check "a piece crossing the limit" '("abcd" #t) (collect 4 "ab" "cde"))
(check "pieces after the limit" '("abcd" #t) (collect 4 "abcd" "e" "f"))

(define note "[Earlier output dropped]\n")

;; OUT after the PIECES of output, kept to MAX bytes and an eighth.
(define (bounded max . pieces)
  (let* ((d (make-document "")) (out (cons d (make-view d "process"))))
    (for-each (lambda (p) (append-bounded-output! out p max)) pieces)
    (document-string d)))

(define (repeat n s) (apply string-append (make-list n s)))

(check "within the bound" "ab\ncd\n" (bounded 1024 "ab\n" "cd\n"))
(check "from a line's start" (string-append note (repeat 256 "xyz\n"))
       (bounded 1024 (repeat 300 "xyz\n") (repeat 300 "xyz\n")))
;; One long line of two-byte characters: cut between characters.
(let ((kept (bounded 1024 (repeat 600 "é"))))
  (check "between characters" #t (string-prefix? (string-append note "é") kept))
  (check "its last bytes" #t (<= 1024 (bytevector-length (string->utf8 kept)) 1200)))
;; One grapheme longer than the bound still goes.
(let ((kept (bounded 1025 (string-append "a" (repeat 700 "\x301;")) (repeat 100 "\x301;"))))
  (check "past one long grapheme" #t (<= (bytevector-length (string->utf8 kept)) (+ 1025 128 (string-length note)))))
;; A long line after a short one: cut within it, not at its start.
(for-each (lambda (char)
            (let ((kept (bounded 1024 (string-append (repeat 30 "x") "\n" (repeat 100000 char)))))
              (check "within a long line" #t (<= (bytevector-length (string->utf8 kept)) (+ 1024 128 (string-length note))))
              (check "one note" #t (and (string-prefix? note kept)
                                        (not (string-contains (substring kept 1 (string-length kept)) "[Earlier"))))))
          '("a" "\x301;"))

(test-failures)
