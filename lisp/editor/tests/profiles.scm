;;; The two key profiles, headless (PLAN.md, Stage 1, slice 1): the same
;;; scenario in each gives the same document, selections and undo grouping,
;;; also when cancelled midway. Then each profile's own behavior.
;;;
;;; Run by crates/techne-editor/tests/lisp.rs, which provides the natives.

(require "../../test.scm")
(require "../session.scm")
(require "../dispatch.scm")
(require "../commands.scm")
(require "../emacs.scm")
(require "../modal.scm")

(define (emacs text) (make-session (make-document text) "user" emacs-profile))
(define (modal text) (make-session (make-document text) "user" modal-profile))

(define (text s) (document-string (session-document s)))
(define (sel s) (view-ranges (session-view s)))
(define (run s keys) (press-keys s keys) s)
(define (typed s keys text) (press-keys s keys) (type-text s text) s)

;; The texts seen undoing until there is nothing left to undo; the document
;; and the selection are restored afterwards.
(define (undo-trail s)
  (let* ((v (session-view s))
         (before (sel s))
         (trail (let loop ((acc '()))
                  (if (guard (e (#t #f)) (view-undo! v) #t)
                      (loop (cons (text s) acc))
                      (reverse acc)))))
    (for-each (lambda (_) (view-redo! v)) trail)
    (view-set-ranges! v before 0)
    trail))

(define (same name e m)
  (check (string-append name ": text") (text e) (text m))
  (check (string-append name ": selection") (sel e) (sel m))
  (check (string-append name ": undo units") (undo-trail e) (undo-trail m)))

;;; One scenario, both profiles

;; Replace the second word, then move to the start of the new one.
(define e1 (emacs "hello brave world"))
(run e1 "M-f M-f M-b M-d")
(type-text e1 "new")
(run e1 "M-b")
(define m1 (modal "hello brave world"))
(run m1 "w d e i")
(type-text m1 "new")
(run m1 "ESC b")
(check "replace a word" "hello new world" (text e1))
(same "replace a word" e1 m1)
(check "undo units" '("hello  world" "hello brave world") (undo-trail e1))

;; The same, cancelled midway: a prefix key, a count, a pending operator and
;; an incremental search, each cancelled, change nothing.
(define e2 (emacs "hello brave world"))
(run e2 "M-f C-x C-g M-f C-s wor C-g M-b M-d")
(type-text e2 "new")
(run e2 "M-b")
(define m2 (modal "hello brave world"))
(run m2 "2 ESC w d ESC / w o r ESC d e i")
(type-text m2 "new")
(run m2 "ESC b")
(same "cancelled midway" e2 m2)
(same "cancelling changes nothing" e1 e2)
(check "the search left no message" #f (sget e2 'message))

;; Kill a word, yank it at the end of the line; undo, redo.
(define e3 (run (emacs "hello world") "M-d C-e C-y"))
(define m3 (run (modal "hello world") "d e $ p"))
(check "kill and yank" " worldhello" (text e3))
(check "kill and yank: same text" (text e3) (text m3))
(run e3 "C-a")
(run m3 "0")
(same "kill and yank" e3 m3)
(run e3 "C-/ C-/ C-?")
(run m3 "u u C-r")
(same "undo and redo" e3 m3)
(check "after undo and redo" " world" (text e3))

;; Vertical motion keeps its column across a short line.
(define e4 (run (emacs "abc\nde\nfghij") "C-f C-f C-n C-n"))
(define m4 (run (modal "abc\nde\nfghij") "l l j j"))
(same "goal column" e4 m4)
(check "goal column position" '((9 9)) (sel e4))

;; Delete characters and to the end of the line. (The Vim cursor cannot stay
;; at the end of the line, so only the texts and units compare.)
(let ((e (run (emacs "abcdef\nxyz") "C-f C-d C-d C-k"))
      (m (run (modal "abcdef\nxyz") "l x x D")))
  (check "delete: text" "a\nxyz" (text m))
  (check "delete: same text" (text e) (text m))
  (check "delete: undo units" (undo-trail e) (undo-trail m)))

;;; Emacs

(define e (emacs "one two three"))
(type-text e "zero ")
(check "typing" "zero one two three" (text e))
(check "typing is one unit" '("one two three") (undo-trail e))

(define e (emacs ""))
(type-text e (make-string 25 #\a))
(check "units of 20 characters" 2 (length (undo-trail e)))

(define e (run (emacs "a\nb\nc") "C-k C-k C-k C-y"))
(check "consecutive kills join" "a\nb\nc" (text e))
(check "kill ring" '("a\nb" . #f) (car (kill-ring e)))

(define e (run (emacs "one two three") "M-f C-SPC M-f M-f C-w"))
(check "kill region" "one" (text e))
(check "region is the kill" " two three" (car (yank-text e)))
(check "no region afterwards" '((3 3)) (sel e))

(define e (run (emacs "one two three") "C-SPC M-f"))
(check "the mark anchors the region" '((0 3)) (sel e))
(run e "x")
(check "typing does not replace the region" "onex two three" (text e))

(define e (run (emacs "ab ab ab") "C-s a b C-s RET"))
(check "isearch again" '((5 5)) (sel e))
(run e "C-r C-r C-r RET")
(check "isearch backward, again with the last string" '((0 0)) (sel e))
(define e (run (emacs "abc") "C-s z"))
(check "failing search" "Failing search: z" (sget e 'message))

(define e (run (emacs "abc") "M-q"))
(check "undefined keys" "M-q is undefined" (sget e 'message))
(define e (run (emacs "") "C-/"))
(check "nothing to undo" "nothing to undo" (sget e 'message))

;; Another actor's edit before the caret moves it; undo leaves that edit.
(define e (emacs "hello"))
(run e "M-f")
(type-text e "!")
(let ((agent (make-view (session-document e) "agent")))
  (view-edit! agent '((0 0 ">> ")) "new"))
(check "the caret follows another actor's edit" '((9 9)) (sel e))
(run e "C-/")
(check "undo keeps the other actor's edit" ">> hello" (text e))

;;; Modal

(define m (run (modal "one two three four five") "d 2 w"))
(check "d2w" "three four five" (text m))
(check "2dw" "five" (text (run m "2 d w")))

(define m (run (modal "one two") "3 x"))
(check "3x" " two" (text m))

(define m (modal "one two three"))
(run m "c w")
(type-text m "1")
(run m "ESC")
(check "cw changes to the end of the word" "1 two three" (text m))
(check "cw and its text are one unit" '("one two three") (undo-trail m))
(run m "w .")
(check "dot repeats the change" "1 1 three" (text m))
(check "dot at the cursor" '((2 2)) (sel m))

(define m (run (modal "a\nb\nc") "d d p"))
(check "dd then p" "b\na\nc" (text m))
(check "p puts the cursor on the pasted line" '((2 2)) (sel m))
(define m (run (modal "a\nb\nc") "G d d"))
(check "dd on the last line takes the break before it" "a\nb" (text m))
(check "yy p on the last line" "a\nb\nb" (text (run (modal "a\nb") "G y y p")))

(define m (run (modal "one two  three") "w d a w"))
(check "daw" "one three" (text m))
(define m (run (modal "one two  three") "w c i w"))
(type-text m "2")
(run m "ESC")
(check "ciw" "one 2  three" (text m))

(define m (run (modal "foo bar\nbaz") "w d w"))
(check "dw stops at the end of the line" "foo \nbaz" (text m))
(check "the cursor stays on a character" '((3 3)) (sel m))

(define m (run (modal "one two three") "v e d"))
(check "visual delete is inclusive" " two three" (text m))
(define m (run (modal "one two three") "w v e y $ p"))
(check "visual yank" "one two threetwo" (text m))

(define m (run (modal "abc") "o"))
(type-text m "def")
(run m "ESC")
(check "o opens a line" "abc\ndef" (text m))
(check "o and its text are one unit" '("abc") (undo-trail m))
(run m "O")
(type-text m "x")
(run m "ESC")
(check "O opens a line above" "abc\nx\ndef" (text m))

(define m (run (modal "one two one two") "/ t w o RET"))
(check "search" '((4 4)) (sel m))
(run m "n")
(check "search again" '((12 12)) (sel m))

(define m (run (modal "abc") "q"))
(check "undefined keys" "q is undefined" (sget m 'message))

(test-failures)
