;;; Lens buffers and structured views (EDITOR.md, sections 1 and 11).
;;;
;;; A lens buffer shows excerpts of other buffers between generated labels;
;;; editing an excerpt edits its source (techne-editor's lens), and an edit
;;; the lens cannot make (its source changed since, it touches a label) is
;;; refused with the reason. Excerpts whose source changed are marked; C-c
;;; C-r shows the sources as they are now. The search lens shows the lines
;;; containing a string in every buffer; the minibuffer's candidates that
;;; are locations become one with C-c C-e, as Embark exports them.
;;;
;;; A structured view is a lens of generated rows only, so it is read-only,
;;; each row with a target: RET does its default action, C-. offers all of
;;; them. `define-view` makes a command that shows one.

(require "session.scm")
(require "commands.scm")
(require "targets.scm")
(require "minibuffer.scm")
(require "buffers.scm")

(provide show-lens! lens-map lens-search lens-refresh lens-save minibuffer-export
         row row? row-columns row-target define-view show-view! view-map)

;;; Lens buffers

(define lens-map (make-keymap))

;; Show a lens of ITEMS (strings, and excerpts (document from to)) in the
;; focused pane as the buffer NAME, replacing a buffer of that name.
(define (show-lens! s name items #:keymap [keymap lens-map] #:target-at [target-at #f] #:refresh [refresh #f])
  (let* ((lens (make-lens items))
         (d (lens-document lens))
         (v (lens-view lens "user")))
    (for-each (lambda (b) (when (equal? (buffer-name b) name) (forget-buffer! b))) (buffer-list))
    (set-doc-prop! d 'name name)
    (set-doc-prop! d 'view v)
    (set-doc-prop! d 'lens lens)
    (set-doc-prop! d 'sources (fold (lambda (i acc)
                                      (if (and (pair? i) (not (any (lambda (x) (document=? x (car i))) acc)))
                                          (cons (car i) acc)
                                          acc))
                                    '() items))
    (set-doc-prop! d 'keymap keymap)
    (set-doc-prop! d 'refresh refresh)
    (set-doc-prop! d 'layers (list (lambda (d from to) (lens-highlights lens from to))))
    (set-doc-prop! d 'target-at (or target-at (lambda (pos) (excerpt-target lens d pos))))
    (show-document! s d)))

;; Labels are drawn as comments, excerpts whose source changed as warnings.
(define (lens-highlights lens from to)
  (let* ((ranges (lens-excerpt-ranges lens))
         (len (document-length (lens-document lens)))
         (gaps (let loop ((at 0) (rs ranges) (acc '()))
                 (cond ((null? rs) (reverse (if (< at len) (cons (list at len 'comment) acc) acc)))
                       (else (loop (cadar rs) (cdr rs)
                                   (if (< at (caar rs)) (cons (list at (caar rs) 'comment) acc) acc))))))
         (stale (map (lambda (i) (append (list-ref ranges i) '(warning))) (lens-stale lens))))
    (filter (lambda (h) (and (< (car h) to) (> (cadr h) from))) (append gaps stale))))

;; The location an excerpt shows at POS, or at the end of POS's line (for a
;; position in its label).
(define (excerpt-target lens d pos)
  (let ((i (or (lens-excerpt-at lens pos) (lens-excerpt-at lens (line-end d pos)))))
    (and i
         (let* ((src (lens-source lens i))
                (start (car (list-ref (lens-excerpt-ranges lens) i)))
                (offset (max 0 (- pos start))))
           (target 'location (location (car src) (min (+ (cadr src) offset) (caddr src))))))))

(define (lens-of s)
  (or (doc-prop (doc s) 'lens) (error "Not a lens")))

(define-command (lens-refresh s n)
  "Show the sources of this lens as they are now; a view is made again."
  (let ((refresh (doc-prop (doc s) 'refresh)))
    (if refresh (refresh s) (lens-refresh! (lens-of s)))))

(define-command (lens-save s n)
  "Write the files edited through this lens."
  (let ((dirty (filter (lambda (d) (and (document-dirty? d) (document-path d))) (or (doc-prop (doc s) 'sources) '()))))
    (for-each document-save! dirty)
    (message! s (if (null? dirty) "No changes to write" (string-append "Wrote " (string-join (map document-path dirty) ", "))))))

(for-each (lambda (b) (define-key! lens-map (car b) (cadr b)))
          '(("C-c C-o" act-default-at-point) ("C-c C-r" lens-refresh) ("C-x C-s" lens-save)))

;;; Lenses of lines

;; A lens item list for the lines at POSITIONS (document . position),
;; labelled by buffer and line.
(define (line-items places)
  (append-map (lambda (p)
                (let* ((d (car p)) (start (line-start d (cdr p))))
                  (list (string-append (buffer-name d) ":" (number->string (line-number d start)) ": ")
                        (list d start (line-end d start))
                        "\n")))
              places))

;; The buffers with text of their own.
(define (text-buffers) (filter (lambda (d) (not (doc-prop d 'lens))) (buffer-list)))

(define (show-search-lens! s needle)
  (let ((places (append-map (lambda (d)
                              (map (lambda (start) (cons d start))
                                   (delete-duplicates
                                    (map (lambda (m) (line-start d (car m))) (search-all d needle 0 (document-length d))))))
                            (text-buffers))))
    (if (null? places)
        (message! s (string-append "No buffer contains " needle))
        (show-lens! s (string-append "*lens " needle "*") (line-items places)))))

(define-command (lens-search s n)
  "Show the lines of every buffer containing a string as a lens: editing
them edits the buffers."
  (completing-read s "Lens of lines containing: " '()
                   #:require-match #f
                   #:accept (lambda (s c) (show-search-lens! s (candidate-text c)))))

(define-command (minibuffer-export s n)
  "Show the candidates matching as a lens of their lines, when they are
locations."
  (let ((targets (map candidate-target (minibuffer-candidates s))) (input (minibuffer-input s)))
    (if (and (pair? targets) (every (lambda (t) (and (target? t) (eq? (target-kind t) 'location))) targets))
        (begin
          (abort-minibuffer! s)
          (show-lens! s (string-append "*lens " input "*")
                      (line-items (map (lambda (t) (let ((l (target-value t))) (cons (location-document l) (location-position l))))
                                       targets))))
        (message! s "Only locations can be shown as a lens"))))

(define-key! minibuffer-map "C-c C-e" 'minibuffer-export)

;;; Structured views

(define-record-type row
  (%row columns target)
  row?
  (columns row-columns)
  (target row-target))

;; A row of a view: its columns, strings, and the target it stands for.
(define (row #:target [target #f] . columns) (%row columns target))

(define view-map (make-keymap))

;; The rows' text: columns padded to the widest of each, one row a line.
(define (rows-text rows)
  (let* ((n (fold (lambda (r m) (max m (length (row-columns r)))) 0 rows))
         (widths (map (lambda (i)
                        (fold (lambda (r m) (if (< i (length (row-columns r))) (max m (string-length (list-ref (row-columns r) i))) m))
                              0 rows))
                      (iota n))))
    (apply string-append
           (map (lambda (r)
                  (let loop ((cols (row-columns r)) (ws widths) (acc ""))
                    (cond ((null? cols) (string-append (string-trim-right acc) "\n"))
                          ((null? (cdr cols)) (loop '() '() (string-append acc (car cols))))
                          (else (loop (cdr cols) (cdr ws)
                                      (string-append acc (car cols) (make-string (+ 2 (- (car ws) (string-length (car cols)))) #\space)))))))
                rows))))

;; Show the view NAME, whose rows (MAKE-ROWS session) gives.
(define (show-view! s name make-rows)
  (let* ((rows (make-rows s))
         (targets (list->vector (map row-target rows)))
         (refresh (lambda (s) (show-view! s name make-rows))))
    (show-lens! s name (list (rows-text rows))
                #:keymap view-map
                #:refresh refresh
                #:target-at (lambda (pos)
                              (let ((i (- (line-number (doc s) pos) 1)))
                                (and (< i (vector-length targets)) (vector-ref targets i)))))
    (when (null? rows) (message! s "Nothing to show"))))

;; (define-view (name s) doc body ...): the command NAME shows the rows
;; BODY gives in a buffer of their own, RET doing the default action on a
;; row's target and C-c C-r showing them again.
(define-syntax define-view
  (syntax-rules ()
    ((_ (name s) doc body ...)
     (define-command (name s n) doc
       (show-view! s (string-append "*" (symbol->string 'name) "*") (lambda (s) body ...))))))

(for-each (lambda (b) (define-key! view-map (car b) (cadr b)))
          '(("RET" act-default-at-point) ("C-c C-o" act-default-at-point) ("C-c C-r" lens-refresh)))
