;;; Structured views (EDITOR.md, sections 1, 2 and 11): buffers of rows a
;;; procedure makes, each row standing for a target. They are presentations
;;; (techne-editor's presentation): text to move, search and copy in as in
;;; any buffer, read-only, made of rows with keys, so making the rows again
;;; keeps the caret on the row it was on wherever that row went.
;;;
;;; A row has columns: a string, or a list of runs, each a string, (text
;;; face) or an excerpt of a document (`excerpt`, edited through; lens.scm).
;;; Its key is what identifies it across refreshes; without one, its first
;;; column's text is (numbered when several rows share it). RET does the
;;; default action on a row's target, C-; offers all of them. `define-view`
;;; makes a command that shows one.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "minibuffer.scm")
(require "buffers.scm")

(provide row row? row-columns row-target excerpt present! row-at
         define-view show-view! view-data view-refresh)

(define-record-type row
  (%row key columns target)
  row?
  (key %row-key)
  (columns row-columns)
  (target row-target))

(define (row #:key [key #f] #:target [target #f] . columns)
  "Return a row of a view of COLUMNS standing for TARGET.
A column is a string, or a list of runs, each a string, (text face) or
an `excerpt`. KEY identifies the row across refreshes; without it, its
first column's text does."
  (%row key columns target))

(define (excerpt d from to #:face [face #f])
  "Return a run showing FROM to TO of document D, edited through to it.
FACE is how it is drawn, or #f."
  (list d from to face))

(define (written v) (if (string? v) v (call-with-output-string (lambda (p) (write v p)))))

;; A column's text, for a key: excerpts as what they show.
(define (column-text c)
  (if (string? c)
      c
      (apply string-append (map (lambda (r)
                                  (cond ((string? r) r)
                                        ((string? (car r)) (car r))
                                        (else (document-substring (car r) (cadr r) (caddr r)))))
                                c))))

;; The rows' keys: their own, else their first column's text, numbered
;; from the second row with the same.
(define (row-keys rows)
  (let ((seen (make-hash-table)))
    (map (lambda (r)
           (if (%row-key r)
               (written (%row-key r))
               (let* ((base (if (null? (row-columns r)) "" (column-text (car (row-columns r)))))
                      (n (hash-table-ref/default seen base 0)))
                 (hash-table-set! seen base (+ n 1))
                 (if (= n 0) base (string-append base "#" (number->string (+ n 1)))))))
         rows)))

(define (present! p rows)
  "Show ROWS in the presentation P; return a table of the rows by key.
Only what differs from what P shows changes."
  (let ((keys (row-keys rows)) (table (make-hash-table)))
    (for-each (lambda (k r) (hash-table-set! table k r)) keys rows)
    (presentation-set-rows! p (map (lambda (k r) (cons k (map (lambda (c) (if (string? c) (list c) c)) (row-columns r))))
                                   keys rows))
    table))

(define (row-at p table pos)
  "Return the row at POS of presentation P, or #f.
TABLE is the table of rows `present!` gave."
  (let ((k (presentation-key-at p pos)))
    (and k (hash-table-ref/default table k #f))))

;;; Views: a buffer of rows from a procedure.

;; What a view's buffer keeps: how to make its rows, the rows shown by
;; key, and what the mode extending rows-mode keeps (the inspector's
;; stack).
(define-record-type rows-view
  (make-rows-view make-rows table data)
  rows-view?
  (make-rows rows-view-make-rows)
  (table rows-view-table)
  (data rows-view-data))

(define-mode rows-mode
  "A structured view: rows of generated text, each standing for a target.
\\[act-default-at-point] does the default action on the row's;
\\[view-refresh] makes the rows again."
  #:parent 'special-mode
  #:keys '(("RET" act-default-at-point) ("C-c C-o" act-default-at-point) ("C-c C-r" view-refresh))
  #:normal '(("RET" act-default-at-point))
  #:target-at (lambda (b pos)
                (let ((v (buffer-state b)))
                  (and (rows-view? v)
                       (let ((r (row-at (buffer-document b) (rows-view-table v) pos)))
                         (and r (row-target r)))))))

(define (view-data b)
  "Return what the mode extending `rows-mode` keeps in view buffer B."
  (and b (rows-view? (buffer-state b)) (rows-view-data (buffer-state b))))

;; A view's first column is its labels, drawn as comments.
(define (labelled rows)
  (map (lambda (r)
         (let ((cs (row-columns r)))
           (if (and (pair? cs) (string? (car cs)))
               (%row (%row-key r) (cons (list (list (car cs) 'comment)) (cdr cs)) (row-target r))
               r)))
       rows))

(define (show-view! s name make-rows #:mode [mode 'rows-mode] #:data [data #f])
  "Show in S the view NAME, whose rows (MAKE-ROWS session) gives.
Its mode is MODE, `rows-mode` or one extending it, keeping DATA."
  (%show-view! s name (scope-procedure make-rows) mode data))

(define (%show-view! s name make-rows mode data)
  (let* ((old (buffer-named name))
         (again (and old (rows-view? (buffer-state old)) (eq? (buffer-mode old) mode)))
         (p (if again (buffer-document old) (make-presentation)))
         (rows (make-rows s))
         (view (make-rows-view make-rows (present! p (labelled rows)) data)))
    (if again
        (begin (set-buffer-state! old view) (show-buffer! s old))
        (show-buffer! s (make-generated-buffer! name p mode #:state view)))
    (when (null? rows) (message! s "Nothing to show"))))

(define-command (view-refresh s n)
  "Make this view's rows again."
  (let* ((b (or (session-buffer s) (error "Not a view"))) (v (buffer-state b)))
    (unless (rows-view? v) (error "Not a view"))
    (%show-view! s (buffer-name b) (rows-view-make-rows v) (buffer-mode b) (rows-view-data v))))

;; (define-view (name s) doc body ...): the command NAME shows the rows
;; BODY gives in a buffer of their own, RET doing the default action on a
;; row's target and C-c C-r showing them again.
(define-syntax define-view
  (syntax-rules ()
    "Define the command NAME showing a view of the rows BODY gives.
DOC documents it. RET does the default action on a row's target and
\\[view-refresh] shows the rows again."
    ((_ (name s) doc body ...)
     (define-command (name s n) doc
       (show-view! s (string-append "*" (symbol->string 'name) "*") (lambda (s) body ...))))))
