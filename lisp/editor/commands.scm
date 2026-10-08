;;; Editing commands shared by both key profiles (EDITOR.md, section 4).
;;;
;;; Motions give a destination; an operator's extent comes from a motion by
;;; the motion's kind (exclusive, inclusive, linewise); each operator makes one
;;; transaction. Commands act on every range of the selection. With the
;;; session's `extend` set (an active mark, visual mode) motions move the
;;; head and keep the anchor; otherwise ranges collapse to carets.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")

(provide make-motion motion? motion-move motion-kind
         char-forward char-backward emacs-word-forward emacs-word-backward
         vim-word-forward vim-word-backward vim-word-end
         line-beginning line-ending line-next line-previous buffer-beginning buffer-ending
         doc ranges point move! motion-extent edit! insert-text! delete-extents!
         kill-save! kill-ring kill-ring-max yank-text clipboard-in! take-clipboard-out! current-prefix
         undo! redo! search!
         region-text replace-region! search-all goto-next! fold-case-for)

(define (doc s)
  "Return the document the commands of session S act on."
  (session-document s))
(define (ranges s)
  "Return the ranges of the selection of session S, each (anchor head)."
  (view-ranges (session-view s)))
(define (point s)
  "Return the head of the primary range of session S: point."
  (cadr (list-ref (ranges s) (view-primary (session-view s)))))

;; Replace every range by (f anchor head) -> (anchor head).
(define (set-ranges! s f)
  (let ((v (session-view s)))
    (view-set-ranges! v (map (lambda (r) (f (car r) (cadr r))) (view-ranges v)) (view-primary v))))

(define (move! s to)
  "Move the head of every range of session S to (TO head).
With the session's `extend` set (an active mark, visual mode) the
anchors stay; otherwise ranges collapse to carets."
  (set-ranges! s (lambda (a h) (let ((h2 (to h))) (list (if (sget s 'extend) a h2) h2)))))

(define (edit! s edits group)
  "Make EDITS, each (from to text), in session S as one change.
GROUP is `new` or `extend`, as `view-edit!` takes it."
  (view-edit! (session-view s) edits group))

;;; Motions: (session position count) -> position.

(define-record-type motion
  (make-motion move kind)
  motion?
  (move motion-move)
  (kind motion-kind))

(define (times n f x) (if (<= n 0) x (times (- n 1) f (f x))))

(define (stepping step kind)
  (make-motion (lambda (s p n) (times n (lambda (p) (step (doc s) p)) p)) kind))

(define (words step style kind)
  (stepping (lambda (d p) (step d p style)) kind))

(define char-forward
  (stepping next-grapheme 'exclusive)
  "The motion to the next character.")
(define char-backward
  (stepping prev-grapheme 'exclusive)
  "The motion to the previous character.")
(define emacs-word-forward
  (words word-end 'emacs 'exclusive)
  "The motion to the end of the word, as Emacs's `forward-word`.")
(define emacs-word-backward
  (words word-start 'emacs 'exclusive)
  "The motion to the start of the word, as Emacs's `backward-word`.")
(define vim-word-forward
  (words next-word-start 'vim 'exclusive)
  "The motion to the start of the next word, as Vim's w.")
(define vim-word-backward
  (words word-start 'vim 'exclusive)
  "The motion to the start of the word, as Vim's b.")
;; Vim `e`: the last character of this word, or of the next one when
;; already on it.
(define vim-word-end
  (stepping (lambda (d p) (prev-grapheme d (word-end d (next-grapheme d p) 'vim))) 'inclusive)
  "The motion to the end of the word, as Vim's e.")

(define line-beginning
  (make-motion (lambda (s p n) (line-start (doc s) p)) 'exclusive)
  "The motion to the start of the line.")
(define line-ending
  (make-motion (lambda (s p n) (line-end (doc s) (line-down (doc s) p (- n 1) 0))) 'exclusive)
  "The motion to the end of the line, N - 1 lines down.")
(define buffer-beginning
  (make-motion (lambda (s p n) 0) 'exclusive)
  "The motion to the start of the buffer.")
(define buffer-ending
  (make-motion (lambda (s p n) (document-length (doc s))) 'exclusive)
  "The motion to the end of the buffer.")

;; Vertical motion keeps the column it started from while it repeats.
(define (goal-column s p)
  (sset! s 'goal-now #t)
  (or (sget s 'goal)
      (let ((c (column (doc s) p)))
        (sset! s 'goal c)
        c)))

(define line-next (make-motion (lambda (s p n) (line-down (doc s) p n (goal-column s p))) 'linewise)
  "The motion to the next line, keeping the goal column.")
(define line-previous (make-motion (lambda (s p n) (line-down (doc s) p (- n) (goal-column s p))) 'linewise)
  "The motion to the previous line, keeping the goal column.")

(define (motion-extent s m pos n)
  "Return the extent an operator takes from motion M at POS in S.
The motion is made N times; the result is (from to linewise?), by the
motion's kind: exclusive, inclusive or linewise."
  (let* ((d (doc s))
         (to ((motion-move m) s pos n))
         (a (min pos to))
         (b (max pos to)))
    (case (motion-kind m)
      ((exclusive) (list a b #f))
      ((inclusive) (list a (next-grapheme d b) #f))
      ((linewise) (list (car (line-span d a)) (cadr (line-span d b)) #t)))))

;;; Operators

(define (insert-text! s text group)
  "Insert TEXT at every range of session S, replacing what it selects.
GROUP is `new` or `extend`, as `view-edit!` takes it."
  (edit! s (map (lambda (r) (list (min (car r) (cadr r)) (max (car r) (cadr r)) text)) (ranges s)) group))

;; A linewise extent on a last line without a line break takes the break
;; before it instead, so no empty line is left behind.
(define (linewise-fix d e)
  (let ((from (car e)) (to (cadr e)) (len (document-length d)))
    (if (and (caddr e) (= to len) (> from 0)
             (not (string=? (document-substring d (prev-grapheme d to) to) "\n")))
        (list (prev-grapheme d from) to #t)
        e)))

(define (delete-extents! s extents kill group)
  "Delete EXTENTS from the document of session S as one change.
EXTENTS are sorted and disjoint, each (from to linewise?); empty ones
are left out. With KILL, the first one's text is saved as a kill, at
the front if KILL is `backward`. GROUP is as `edit!` takes it. Return
the extents deleted."
  (let* ((d (doc s))
         (extents (filter (lambda (e) (< (car e) (cadr e))) (map (lambda (e) (linewise-fix d e)) extents))))
    (when (and kill (pair? extents))
      (let ((e (car extents)))
        (kill-save! s (document-substring d (car e) (cadr e)) (caddr e) (eq? kill 'backward))))
    (edit! s (map (lambda (e) (list (car e) (cadr e) "")) extents) group)
    extents))

;;; Kill ring: a list of (text . linewise?), newest first, at most
;;; `kill-ring-max` long. Consecutive kills join into one entry, as Emacs
;;; does. There is one, which every session shares, as Emacs has one for
;;; all its frames. What is killed goes to the system clipboard of the
;;; session's frontend too (it takes it, `take-clipboard-out!`), and what
;;; another program put there comes in as the newest kill
;;; (`clipboard-in!`).

(define kill-ring-max 120
  "The most kills the kill ring keeps.")

(define %kill-ring '())
;; The session that changed the kill ring last: only its kills join the
;; newest one.
(define %kill-ring-by #f)

(define (kill-ring s)
  "Return the kill ring, which S shares: (text . linewise?), newest first."
  %kill-ring)

(define (set-kill-ring! s ring)
  (set! %kill-ring (if (> (length ring) kill-ring-max) (take ring kill-ring-max) ring))
  (set! %kill-ring-by s))

(define (clipboard-in! s text)
  "Take TEXT, what the system clipboard has, as a kill in session S.
It is left out when it is what was killed last, or what was last given
to or taken from the clipboard."
  (let ((ring (kill-ring s)))
    (unless (or (string=? text "")
                (and (pair? ring) (string=? text (caar ring)))
                (equal? text (sget s 'clipboard-seen)))
      (set-kill-ring! s (cons (cons text #f) ring)))
    (sset! s 'clipboard-seen text)))

(define (take-clipboard-out! s)
  "Return the newest kill of session S if not given to the clipboard yet.
Otherwise return #f."
  (let ((text (sget s 'clipboard-out)))
    (sset! s 'clipboard-out #f)
    (when text (sset! s 'clipboard-seen text))
    text))

;; Whole lines are kept with one line break at their end, whichever break
;; was deleted with them.
(define (as-lines text)
  (cond ((and (> (string-length text) 0) (char=? (string-ref text 0) #\newline))
         (string-append (substring text 1 (string-length text)) "\n"))
        ((and (> (string-length text) 0) (char=? (string-ref text (- (string-length text) 1)) #\newline)) text)
        (else (string-append text "\n"))))

(define (kill-save! s text linewise backward)
  "Save TEXT as a kill in session S, joining it to a kill just made.
With LINEWISE it is whole lines; with BACKWARD it joins at the front."
  (let ((ring (kill-ring s))
        (text (if linewise (as-lines text) text)))
    (set-kill-ring! s
                    (if (and (sget s 'last-kill) (pair? ring) (eq? %kill-ring-by s))
                        (cons (cons (if backward (string-append text (caar ring)) (string-append (caar ring) text))
                                    (cdar ring))
                              (cdr ring))
                        (cons (cons text linewise) ring)))
    (sset! s 'clipboard-out (caar (kill-ring s)))
    (sset! s 'kill-now #t)))

(define (current-prefix s)
  "Return the prefix argument the running command of S was given.
As Emacs has it: #f, (4) for `C-u` (16 for `C-u C-u`...), a number,
or - for a bare minus."
  (sget s 'current-prefix))

(define (yank-text s)
  "Return the newest kill of session S; it is an error if there is none."
  (let ((ring (kill-ring s)))
    (if (null? ring) (error "the kill ring is empty") (car ring))))

(define (undo! s)
  "Undo the last change of the document of session S."
  (view-undo! (session-view s)))
(define (redo! s)
  "Redo the last change undone in the document of session S."
  (view-redo! (session-view s)))

(define (fold-case-for needle)
  "Return #t if a search for NEEDLE should ignore case.
It does unless NEEDLE has an upper-case letter, as Emacs's
search-upper-case."
  (not (any char-upper-case? (string->list needle))))

(define (search! s needle from forward)
  "Return the span (start end) of the next NEEDLE from FROM in S.
It searches forward, or with FORWARD #f backward, and raises an error
if there is none. Case is ignored as `fold-case-for` says."
  (or (search-text (doc s) from needle forward (fold-case-for needle))
      (error "search failed" needle)))

;;; For extensions: the region (the primary range) as text, replacing every
;;; range's text, finding text.

(define (region-text s)
  "Return the text of the region of session S, its primary range."
  (let ((r (list-ref (ranges s) (view-primary (session-view s)))))
    (document-substring (doc s) (min (car r) (cadr r)) (max (car r) (cadr r)))))

(define (replace-region! s f)
  "Replace the text of every range of session S by (F text).
It is one undo unit."
  (let ((d (doc s)))
    (edit! s (map (lambda (r)
                    (let ((from (min (car r) (cadr r))) (to (max (car r) (cadr r))))
                      (list from to (f (document-substring d from to)))))
                  (ranges s))
           "new")))

(define (search-all doc needle from to)
  "Return the spans (start end) of NEEDLE in DOC starting from FROM to TO.
Case is ignored as `fold-case-for` says."
  (search-text-all doc needle from to (fold-case-for needle)))

(define (goto-next! s needle)
  "Move point in session S to the next NEEDLE after it."
  (let ((m (search! s needle (point s) #t)))
    (move! s (lambda (p) (car m)))))

;;; Commands with Emacs's names; both profiles bind them.

;; A motion N times, or the opposite one -N times when N is negative.
(define (directed m opposite n) (if (< n 0) (values opposite (- n)) (values m n)))
(define (move-by! s m opposite n)
  (call-with-values (lambda () (directed m opposite n))
    (lambda (m n) (move! s (lambda (p) ((motion-move m) s p n))))))

(define-command (forward-char s n) "Move forward by characters." (move-by! s char-forward char-backward n))
(define-command (backward-char s n) "Move backward by characters." (move-by! s char-backward char-forward n))
(define-command (forward-word s n) "Move to the end of the next word." (move-by! s emacs-word-forward emacs-word-backward n))
(define-command (backward-word s n) "Move to the start of this or the previous word." (move-by! s emacs-word-backward emacs-word-forward n))
(define-command (next-line s n) "Move down by lines, keeping the column." (move! s (lambda (p) ((motion-move line-next) s p n))))
(define-command (previous-line s n) "Move up by lines, keeping the column." (move! s (lambda (p) ((motion-move line-previous) s p n))))
(define-command (beginning-of-line s n) "Move to the start of the line." (move! s (lambda (p) (line-start (doc s) p))))
(define-command (end-of-line s n) "Move to the end of the line." (move! s (lambda (p) ((motion-move line-ending) s p n))))
(define-command (beginning-of-buffer s n) "Move to the start of the document." (move! s (lambda (p) 0)))
(define-command (end-of-buffer s n) "Move to the end of the document." (move! s (lambda (p) (document-length (doc s)))))

(define (forward-extents s m n)
  (map (lambda (r) (motion-extent s m (cadr r) n)) (ranges s)))

;; Delete (and with KILL save) what motion M covers N times, the opposite
;; motion's when N is negative.
(define (delete-by! s m opposite n kill)
  (call-with-values (lambda () (directed m opposite n))
    (lambda (m2 n2)
      (delete-extents! s (forward-extents s m2 n2) (and kill (if (eq? m2 m) kill (if (eq? kill 'forward) 'backward 'forward))) 'new))))

(define-command (delete-char s n) "Delete the next characters." (delete-by! s char-forward char-backward n #f))
(define-command (delete-backward-char s n) "Delete the previous characters." (delete-by! s char-backward char-forward n #f))
(define-command (kill-word s n) "Kill to the end of the next word." (delete-by! s emacs-word-forward emacs-word-backward n 'forward))
(define-command (backward-kill-word s n) "Kill to the start of the previous word." (delete-by! s emacs-word-backward emacs-word-forward n 'backward))

(define-command (kill-line s n)
  "Kill to the end of the line, or the line break when at the end."
  (let ((d (doc s)))
    (delete-extents! s (map (lambda (r)
                              (let* ((p (cadr r)) (end (line-end d p)))
                                (list p (if (= p end) (cadr (line-span d p)) end) #f)))
                            (ranges s))
                     'forward 'new)))

(define (region s)
  (unless (sget s 'extend) (error "the mark is not active, so there is no region"))
  (map (lambda (r) (list (min (car r) (cadr r)) (max (car r) (cadr r)) #f)) (ranges s)))

(define-command (kill-region s n) "Kill the region." (let ((r (region s))) (sset! s 'extend #f) (delete-extents! s r 'forward 'new)))
(define-command (copy-region-as-kill s n)
  "Save the region as a kill without deleting it."
  (let ((e (car (region s))))
    (kill-save! s (document-substring (doc s) (car e) (cadr e)) #f #f)
    (sset! s 'extend #f)
    (set-ranges! s (lambda (a h) (list h h)))))

(define (kill-at s i)
  (let ((ring (kill-ring s)))
    (cond ((null? ring) (error "the kill ring is empty"))
          ((or (< i 0) (>= i (length ring))) (error "the kill ring has no such entry" (+ i 1)))
          (else (list-ref ring i)))))

(define-command (yank s n)
  "Insert the last kill.
With a prefix argument, leave point before it; with a number N,
insert the Nth most recent kill instead."
  (let* ((raw (current-prefix s))
         (k (if (integer? raw) (kill-at s (- raw 1)) (yank-text s)))
         (one (= (length (ranges s)) 1))
         (start (let ((r (car (ranges s)))) (min (car r) (cadr r)))))
    (insert-text! s (car k) 'new)
    ;; Where the yanked text is, for M-y to replace it: (start end revision
    ;; text).
    (sset! s 'last-yank (and one (list start (point s) (document-revision (doc s)) (car k))))
    (when (and one (pair? raw))
      (view-set-ranges! (session-view s) (list (list start start)) 0))))

(define-command (exchange-point-and-mark s n)
  "Put point where the mark is and the mark where point was.
The region is active."
  (if (sget s 'extend)
      (set-ranges! s (lambda (a h) (list h a)))
      (message! s "The mark is not set")))
(define-command (newline s n) "Insert a line break." (insert-text! s "\n" 'new))
(define-command (undo s n) "Undo your last change." (times n (lambda (_) (undo! s)) #f))
(define-command (redo s n) "Redo what you undid." (times n (lambda (_) (redo! s)) #f))

(define-command (set-mark s n)
  "Start a region at point: motions now extend it."
  (sset! s 'extend #t)
  (set-ranges! s (lambda (a h) (list h h))))

(define-command (keyboard-quit s n)
  "Cancel: deactivate the region."
  (sset! s 'extend #f)
  (set-ranges! s (lambda (a h) (list h h)))
  (message! s "Quit"))
