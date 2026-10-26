;;; Listings: views of rows that are marked, then acted on together, as
;;; Emacs's dired and ibuffer are. A listing is a buffer of rows (views.scm)
;;; whose state is a table: how to make its rows, the rows shown by key, and
;;; a mark by row key, a character (* to act on, D to delete). A package
;;; keeps its own entries in the table beside those (the directory listed,
;;; how it is sorted), plain data, so its next generation reads them.
;;;
;;; Rows marked are made with `listing-row`, whose first column shows the
;;; row's mark; other rows (headings, a directory's parent) never are.
;;; The commands here mark and unmark (bound in `listing-mode`, which the
;;; packages' modes extend); `listing-targets` gives what the package's
;;; own commands act on: the marked rows' targets, else the one at point.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "views.scm")

(provide make-listing-buffer! listing-redisplay! listing-row listing-mark listing-marked listing-targets
         listing-key-at listing-goto! listing-get listing-set! listing?
         listing-mark-row listing-unmark-row listing-flag-row listing-unmark-backward listing-unmark-all
         listing-toggle-marks listing-refresh)

(define (listing? b)
  "Return #t if B is a buffer whose state is a listing's."
  (and b (hash-table? (buffer-state b)) (hash-table-ref/default (buffer-state b) 'listing #f) #t))

(define (listing-get b key)
  "Return the entry KEY of the state of listing B, or #f."
  (hash-table-ref/default (buffer-state b) key #f))

(define (listing-set! b key value)
  "Make VALUE the entry KEY of the state of listing B."
  (hash-table-set! (buffer-state b) key value))

(define-mode listing-mode
  "Rows to mark and act on together.
\\[listing-mark-row] marks a row, \\[listing-flag-row] flags it for
deletion, \\[listing-unmark-row] unmarks it, \\[listing-unmark-all]
unmarks every row and \\[listing-toggle-marks] swaps the marked and
unmarked ones. \\[act-default-at-point] does the default action on the
row's target, \\[listing-refresh] makes the rows again."
  #:parent 'special-mode
  #:keys '(("RET" act-default-at-point) ("m" listing-mark-row) ("u" listing-unmark-row) ("d" listing-flag-row)
           ("DEL" listing-unmark-backward) ("U" listing-unmark-all) ("t" listing-toggle-marks)
           ("g" listing-refresh) ("n" next-line) ("p" previous-line) ("SPC" next-line))
  #:normal '(("RET" act-default-at-point) ("m" listing-mark-row) ("u" listing-unmark-row) ("d" listing-flag-row)
             ("U" listing-unmark-all) ("t" listing-toggle-marks) ("g r" listing-refresh))
  #:target-at (lambda (b pos)
                (and (listing? b)
                     (let ((r (row-at (buffer-document b) (listing-get b 'rows) pos)))
                       (and r (row-target r))))))

(define (make-listing-buffer! name mode make-rows #:state [state '()])
  "Return the listing NAME in MODE, made if there is none, made anew.
Its rows are (MAKE-ROWS buffer), made with `listing-row`. STATE is an
alist of the entries the package keeps in it, set each time; the marks
of a listing already there, in MODE or one extending it, stay."
  (let* ((old (buffer-named name))
         (b (if (and (listing? old) (derived-mode? (buffer-mode old) mode))
                old
                (let ((st (make-hash-table)))
                  (hash-table-set! st 'listing #t)
                  (hash-table-set! st 'marks (make-hash-table))
                  (make-generated-buffer! name (make-presentation) mode #:state st)))))
    (listing-set! b 'make (scope-procedure make-rows))
    (for-each (lambda (e) (listing-set! b (car e) (cdr e))) state)
    (listing-redisplay! b)
    b))

(define (listing-redisplay! b)
  "Make the rows of listing B again, from what its state holds now.
Marks of rows no longer there are forgotten. Once the package that made
it is gone, the rows stay as they are."
  (listing-set! b 'markable (make-hash-table))
  (let ((rows ((listing-get b 'make) b)))
    (when rows
      (let ((table (present! (buffer-document b) rows)) (marks (listing-get b 'marks)))
        (for-each (lambda (k) (unless (markable? b k) (hash-table-delete! marks k)))
                  (hash-table-keys marks))
        (listing-set! b 'rows table)))))

;; Whether the row KEY of B can be marked: `listing-row` made it.
(define (markable? b key)
  (hash-table-ref/default (listing-get b 'markable) key #f))

(define (listing-mark b key)
  "Return the mark of the row KEY of listing B, a character, or #f."
  (hash-table-ref/default (listing-get b 'marks) key #f))

(define (mark-face c) (if (eqv? c #\D) 'error 'warning))

(define (listing-row b key target . columns)
  "Return the row KEY, a string, of listing B, standing for TARGET.
Its first column is its mark, COLUMNS follow. Only such rows are
marked; others, as headings, are passed over."
  (let ((c (listing-mark b key)) (markable (listing-get b 'markable)))
    (when markable (hash-table-set! markable key #t))
    (apply row #:key key #:target target
           (if c (list (list (string c) (mark-face c))) " ")
           columns)))

(define (listing-key-at b pos)
  "Return the key of the row at POS of listing B, or #f."
  (presentation-key-at (buffer-document b) pos))

(define (listing-goto! s b key #:offset [offset 0])
  "Put the caret of S's focused pane at the row KEY of listing B.
It goes OFFSET bytes into the row; nothing moves when there is no
such row."
  (let ((span (presentation-row-span (buffer-document b) key)))
    (when span
      (let ((p (min (+ (car span) offset) (cadr span))))
        (view-set-ranges! (pane-view s) (list (list p p)) 0)))))

(define (listing-marked b c)
  "Return the targets of the rows of listing B marked with C, in order."
  (let ((p (buffer-document b)) (table (listing-get b 'rows)))
    (filter-map (lambda (l)
                  (let ((k (presentation-key-at p (car l))))
                    (and k (eqv? (listing-mark b k) c)
                         (let ((r (hash-table-ref/default table k #f))) (and r (row-target r))))))
                (document-lines p))))

(define (listing-targets s #:mark [c #\*])
  "Return the targets the focused listing's command in S acts on.
They are those of the rows marked with C, else the row's at point; it
is an error if there are none."
  (let* ((b (listing-of s)) (marked (listing-marked b c)))
    (if (pair? marked)
        marked
        (let ((t (target-at (doc s) (point s))))
          (if t (list t) (error "Nothing here to act on"))))))

(define (listing-of s)
  (let ((b (session-buffer s)))
    (if (listing? b) b (error "Not a listing"))))

;; Set the mark of N rows from point to C (#f to unmark), moving down past
;; them; rows without a target are passed over.
(define (mark-rows! s n c)
  (let ((b (listing-of s)))
    (let loop ((n n))
      (when (> n 0)
        (let* ((pos (point s)) (k (listing-key-at b pos)))
          (when (and k (markable? b k))
            (if c (hash-table-set! (listing-get b 'marks) k c) (hash-table-delete! (listing-get b 'marks) k)))
          (move! s (lambda (p) ((motion-move line-next) s p 1)))
          (unless (= pos (point s)) (loop (- n 1))))))
    (listing-redisplay! b)))

(define-command (listing-mark-row s n)
  "Mark the row at point and move down; with a count, that many rows."
  (mark-rows! s n #\*))

(define-command (listing-flag-row s n)
  "Flag the row at point for deletion and move down."
  (mark-rows! s n #\D))

(define-command (listing-unmark-row s n)
  "Unmark the row at point and move down."
  (mark-rows! s n #f))

(define-command (listing-unmark-backward s n)
  "Move up a row and unmark it."
  (let ((b (listing-of s)))
    (move! s (lambda (p) ((motion-move line-previous) s p n)))
    (let ((k (listing-key-at b (point s))))
      (when k (hash-table-delete! (listing-get b 'marks) k)))
    (listing-redisplay! b)))

(define-command (listing-unmark-all s n)
  "Unmark every row."
  (let ((b (listing-of s)))
    (let ((marks (listing-get b 'marks)))
      (for-each (lambda (k) (hash-table-delete! marks k)) (hash-table-keys marks)))
    (listing-redisplay! b)))

(define-command (listing-toggle-marks s n)
  "Mark the unmarked rows and unmark the marked ones.
Rows flagged for deletion stay so."
  (let* ((b (listing-of s)) (marks (listing-get b 'marks)) (p (buffer-document b)))
    (for-each (lambda (l)
                (let ((k (presentation-key-at p (car l))))
                  (when (and k (markable? b k))
                    (case (listing-mark b k)
                      ((#\*) (hash-table-delete! marks k))
                      ((#f) (hash-table-set! marks k #\*))))))
              (document-lines p))
    (listing-redisplay! b)))

(define-command (listing-refresh s n)
  "Make this listing's rows again, keeping the marks of rows still there."
  (listing-redisplay! (listing-of s)))
