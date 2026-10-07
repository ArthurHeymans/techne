;;; Editing commands shared by both key profiles (EDITOR.md, section 4).
;;;
;;; Motions give a destination; an operator's extent comes from a motion by
;;; the motion's kind (exclusive, inclusive, linewise); each operator makes one
;;; transaction. Commands act on every range of the selection. With the
;;; session's `extend` set (an active mark, visual mode) motions move the
;;; head and keep the anchor; otherwise ranges collapse to carets.

(require "session.scm")

(provide make-motion motion? motion-move motion-kind
         char-forward char-backward emacs-word-forward emacs-word-backward
         vim-word-forward vim-word-backward vim-word-end
         line-beginning line-ending line-next line-previous buffer-beginning buffer-ending
         doc ranges point move! motion-extent edit! insert-text! delete-extents!
         kill-save! kill-ring yank-text undo! redo! search!
         region-text replace-region! search-all goto-next!)

(define (doc s) (session-document s))
(define (ranges s) (view-ranges (session-view s)))
(define (point s) (cadr (list-ref (ranges s) (view-primary (session-view s)))))

;; Replace every range by (f anchor head) -> (anchor head).
(define (set-ranges! s f)
  (let ((v (session-view s)))
    (view-set-ranges! v (map (lambda (r) (f (car r) (cadr r))) (view-ranges v)) (view-primary v))))

;; Move every head by `to` (pos -> pos).
(define (move! s to)
  (set-ranges! s (lambda (a h) (let ((h2 (to h))) (list (if (sget s 'extend) a h2) h2)))))

(define (edit! s edits group) (view-edit! (session-view s) edits group))

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

(define char-forward (stepping next-grapheme 'exclusive))
(define char-backward (stepping prev-grapheme 'exclusive))
(define emacs-word-forward (words word-end 'emacs 'exclusive))
(define emacs-word-backward (words word-start 'emacs 'exclusive))
(define vim-word-forward (words next-word-start 'vim 'exclusive))
(define vim-word-backward (words word-start 'vim 'exclusive))
;; Vim `e`: the last character of this word, or of the next one when
;; already on it.
(define vim-word-end
  (stepping (lambda (d p) (prev-grapheme d (word-end d (next-grapheme d p) 'vim))) 'inclusive))

(define line-beginning (make-motion (lambda (s p n) (line-start (doc s) p)) 'exclusive))
(define line-ending
  (make-motion (lambda (s p n) (line-end (doc s) (line-down (doc s) p (- n 1) 0))) 'exclusive))
(define buffer-beginning (make-motion (lambda (s p n) 0) 'exclusive))
(define buffer-ending (make-motion (lambda (s p n) (document-length (doc s))) 'exclusive))

;; Vertical motion keeps the column it started from while it repeats.
(define (goal-column s p)
  (sset! s 'goal-now #t)
  (or (sget s 'goal)
      (let ((c (column (doc s) p)))
        (sset! s 'goal c)
        c)))

(define line-next (make-motion (lambda (s p n) (line-down (doc s) p n (goal-column s p))) 'linewise))
(define line-previous (make-motion (lambda (s p n) (line-down (doc s) p (- n) (goal-column s p))) 'linewise))

;; The extent an operator takes from a motion at `pos`: (from to linewise?).
(define (motion-extent s m pos n)
  (let* ((d (doc s))
         (to ((motion-move m) s pos n))
         (a (min pos to))
         (b (max pos to)))
    (case (motion-kind m)
      ((exclusive) (list a b #f))
      ((inclusive) (list a (next-grapheme d b) #f))
      ((linewise) (list (car (line-span d a)) (cadr (line-span d b)) #t)))))

;;; Operators

;; Insert text at every range, replacing what is selected.
(define (insert-text! s text group)
  (edit! s (map (lambda (r) (list (min (car r) (cadr r)) (max (car r) (cadr r)) text)) (ranges s)) group))

;; A linewise extent on a last line without a line break takes the break
;; before it instead, so no empty line is left behind.
(define (linewise-fix d e)
  (let ((from (car e)) (to (cadr e)) (len (document-length d)))
    (if (and (caddr e) (= to len) (> from 0)
             (not (string=? (document-substring d (prev-grapheme d to) to) "\n")))
        (list (prev-grapheme d from) to #t)
        e)))

;; Delete extents (sorted, disjoint) in one transaction; with `kill`, save the
;; first one's text. Empty extents are left out. Returns the extents deleted.
(define (delete-extents! s extents kill group)
  (let* ((d (doc s))
         (extents (filter (lambda (e) (< (car e) (cadr e))) (map (lambda (e) (linewise-fix d e)) extents))))
    (when (and kill (pair? extents))
      (let ((e (car extents)))
        (kill-save! s (document-substring d (car e) (cadr e)) (caddr e) (eq? kill 'backward))))
    (edit! s (map (lambda (e) (list (car e) (cadr e) "")) extents) group)
    extents))

;;; Kill ring: a list of (text . linewise?), newest first. Consecutive kills
;;; join into one entry, as Emacs does.

(define (kill-ring s) (or (sget s 'kill-ring) '()))

;; Whole lines are kept with one line break at their end, whichever break
;; was deleted with them.
(define (as-lines text)
  (cond ((and (> (string-length text) 0) (char=? (string-ref text 0) #\newline))
         (string-append (substring text 1 (string-length text)) "\n"))
        ((and (> (string-length text) 0) (char=? (string-ref text (- (string-length text) 1)) #\newline)) text)
        (else (string-append text "\n"))))

(define (kill-save! s text linewise backward)
  (let ((ring (kill-ring s))
        (text (if linewise (as-lines text) text)))
    (sset! s 'kill-ring
           (if (and (sget s 'last-kill) (pair? ring))
               (cons (cons (if backward (string-append text (caar ring)) (string-append (caar ring) text))
                           (cdar ring))
                     (cdr ring))
               (cons (cons text linewise) ring)))
    (sset! s 'kill-now #t)))

(define (yank-text s)
  (let ((ring (kill-ring s)))
    (if (null? ring) (error "the kill ring is empty") (car ring))))

(define (undo! s) (view-undo! (session-view s)))
(define (redo! s) (view-redo! (session-view s)))

;; Search for text from `from`; returns (start end) or raises.
(define (search! s needle from forward)
  (or (search-text (doc s) from needle forward)
      (error "search failed" needle)))

;;; For extensions: the region (the primary range) as text, replacing every
;;; range's text, finding text.

(define (region-text s)
  (let ((r (list-ref (ranges s) (view-primary (session-view s)))))
    (document-substring (doc s) (min (car r) (cadr r)) (max (car r) (cadr r)))))

;; Replace the text of every range by (F text), as one undo unit.
(define (replace-region! s f)
  (let ((d (doc s)))
    (edit! s (map (lambda (r)
                    (let ((from (min (car r) (cadr r))) (to (max (car r) (cadr r))))
                      (list from to (f (document-substring d from to)))))
                  (ranges s))
           "new")))

;; The spans (start end) of NEEDLE in DOC that start between FROM and TO.
(define (search-all doc needle from to)
  (let loop ((p from) (acc '()))
    (let ((m (and (< p to) (search-text doc p needle #t))))
      (if (and m (< (car m) to))
          (loop (cadr m) (cons m acc))
          (reverse acc)))))

;; Move point to the next NEEDLE after it.
(define (goto-next! s needle)
  (let ((m (search! s needle (point s) #t)))
    (move! s (lambda (p) (car m)))))

;;; Commands with Emacs's names; both profiles bind them.

(define-command (forward-char s n) "Move forward by characters." (move! s (lambda (p) ((motion-move char-forward) s p n))))
(define-command (backward-char s n) "Move backward by characters." (move! s (lambda (p) ((motion-move char-backward) s p n))))
(define-command (forward-word s n) "Move to the end of the next word." (move! s (lambda (p) ((motion-move emacs-word-forward) s p n))))
(define-command (backward-word s n) "Move to the start of this or the previous word." (move! s (lambda (p) ((motion-move emacs-word-backward) s p n))))
(define-command (next-line s n) "Move down by lines, keeping the column." (move! s (lambda (p) ((motion-move line-next) s p n))))
(define-command (previous-line s n) "Move up by lines, keeping the column." (move! s (lambda (p) ((motion-move line-previous) s p n))))
(define-command (beginning-of-line s n) "Move to the start of the line." (move! s (lambda (p) (line-start (doc s) p))))
(define-command (end-of-line s n) "Move to the end of the line." (move! s (lambda (p) ((motion-move line-ending) s p n))))
(define-command (beginning-of-buffer s n) "Move to the start of the document." (move! s (lambda (p) 0)))
(define-command (end-of-buffer s n) "Move to the end of the document." (move! s (lambda (p) (document-length (doc s)))))

(define (forward-extents s m n)
  (map (lambda (r) (motion-extent s m (cadr r) n)) (ranges s)))

(define-command (delete-char s n) "Delete the next characters." (delete-extents! s (forward-extents s char-forward n) #f 'new))
(define-command (delete-backward-char s n) "Delete the previous characters." (delete-extents! s (forward-extents s char-backward n) #f 'new))
(define-command (kill-word s n) "Kill to the end of the next word." (delete-extents! s (forward-extents s emacs-word-forward n) 'forward 'new))
(define-command (backward-kill-word s n) "Kill to the start of the previous word." (delete-extents! s (forward-extents s emacs-word-backward n) 'backward 'new))

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

(define-command (yank s n) "Insert the last kill." (insert-text! s (car (yank-text s)) 'new))
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
