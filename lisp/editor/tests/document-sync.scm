;;; What a language server's client following a document stands on:
;;; the changes since a revision and positions, as LSP counts them, and
;;; waiting for the next change without asking every so often.
;;;
;;; Run by crates/techne-editor/tests/lisp.rs, which provides the natives.

(require "../../test.scm")

(define d (make-document "ab\ncdéf\n"))
(define v (make-view d "user"))
(define start (document-revision d))

;; Changes as a server applies them: each range in the text the ones
;; before it left, columns in the encoding agreed.
(view-edit! v '((0 0 "x")) "new")
(view-edit! v '((3 5 "")) "new")
(view-edit! v '((7 7 "😀")) "new")
(check "the changes since a revision, oldest first"
       '((0 0 0 0 "x") (0 3 1 1 "") (0 6 0 6 "😀")) (document-lsp-changes d start "utf-16"))
(check "in bytes for a server counting them"
       '((0 0 0 0 "x") (0 3 1 1 "") (0 7 0 7 "😀")) (document-lsp-changes d start "utf-8"))
(check "none since now" '() (document-lsp-changes d (document-revision d) "utf-16"))
(check "a revision the history does not have" #f (document-lsp-changes d 999 "utf-16"))

;; Positions: lines from 0, columns in UTF-16 code units or bytes.
(define p (make-document "ab\ncdéf\na😀b\n"))
(check "a position on a line" '(1 3) (document-lsp-position p 7 "utf-16"))
(check "in bytes" '(1 4) (document-lsp-position p 7 "utf-8"))
(check "past a supplementary character" '(2 3) (document-lsp-position p 14 "utf-16"))
(check "the end" '(3 0) (document-lsp-position p (document-length p) "utf-16"))
(check "and back, several at once" '(7 14 10 2 16)
       (document-lsp-offsets p (document-revision p) "utf-16" '((1 3) (2 3) (2 2) (0 99) (9 0))))
;; What a server said of a revision since edited is read against that
;; revision's text.
(define pv (make-view p "user"))
(define then (document-revision p))
(view-edit! pv '((0 0 "😀😀\n")) "new")
(check "read against the text it was said of" '(7) (document-lsp-offsets p then "utf-16" '((1 3))))
(check "a revision gone" #f (document-lsp-offsets p 999 "utf-16" '((0 0))))

;; Waiting for a change: woken by the edit, with the revision then.
(define w (make-document "a"))
(define wv (make-view w "user"))
(define seen (make-channel 1))
(define watcher (spawn (lambda () (channel-send seen (document-await-change w (document-revision w))))))
(sleep 20)
(check "nothing yet while the text holds still" 0 (channel-length seen))
(view-edit! wv '((1 1 "b")) "new")
(check "woken by the edit" (document-revision w) (channel-recv seen))
(check "a revision already past answers at once" (document-revision w)
       (task-join (spawn (lambda () (document-await-change w 0)))))

(test-failures)
