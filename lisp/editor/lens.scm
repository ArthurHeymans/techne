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
;;; each row with a target: RET does its default action, C-; offers all of
;;; them. `define-view` makes a command that shows one.

(require "session.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "minibuffer.scm")
(require "buffers.scm")

(provide show-lens! lens-search lens-refresh lens-save minibuffer-export
         row row? row-columns row-target define-view show-view! view-data)

;;; Lens buffers

(define-mode lens-mode
  "Excerpts of other buffers between labels: editing an excerpt edits its
source. C-c C-r shows the sources as they are now; C-x C-s writes them."
  #:keys '(("C-c C-o" act-default-at-point) ("C-c C-r" lens-refresh) ("C-x C-s" lens-save))
  #:layer (lambda (d from to)
            (let ((b (document-buffer d)))
              (if (and b (buffer-lens b)) (lens-highlights (buffer-lens b) from to) '())))
  #:target-at (lambda (b pos) (excerpt-target (buffer-lens b) (buffer-document b) pos)))

;; Show a lens of ITEMS (strings, and excerpts (document from to)) in the
;; focused pane as the buffer NAME in MODE, with STATE, replacing a buffer
;; of that name. Returns the view.
(define (show-lens! s name items #:mode [mode 'lens-mode] #:state [state #f])
  (let ((lens (make-lens items)))
    (show-buffer! s (make-generated-buffer! name (lens-document lens) mode #:lens lens #:state state))))

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
  (let ((b (session-buffer s)))
    (or (and b (buffer-lens b)) (error "Not a lens"))))

;; The documents a lens shows excerpts of.
(define (lens-sources lens)
  (fold (lambda (i acc)
          (let ((d (car (lens-source lens i))))
            (if (any (lambda (x) (document=? x d)) acc) acc (cons d acc))))
        '() (iota (length (lens-excerpt-ranges lens)))))

(define-command (lens-refresh s n)
  "Show the sources of this lens as they are now."
  (lens-refresh! (lens-of s)))

(define-command (lens-save s n)
  "Write the files edited through this lens."
  (let ((dirty (filter (lambda (d) (and (document-dirty? d) (document-path d))) (lens-sources (lens-of s)))))
    (for-each document-save! dirty)
    (message! s (if (null? dirty) "No changes to write" (string-append "Wrote " (string-join (map document-path dirty) ", "))))))

;;; Lenses of lines

;; A lens item list for the lines at POSITIONS (document . position),
;; labelled by buffer and line.
(define (line-items places)
  (append-map (lambda (p)
                (let* ((d (car p)) (start (line-start d (cdr p))))
                  (list (string-append (document-label d) ":" (number->string (line-number d start)) ": ")
                        (list d start (line-end d start))
                        "\n")))
              places))

;; A document's buffer's name, else its file's.
(define (document-label d)
  (let ((b (document-buffer d)))
    (cond (b (buffer-name b)) ((document-path d) => file-name) (else "?"))))

;; The documents of buffers with text of their own.
(define (text-documents) (map buffer-document (filter (lambda (b) (not (buffer-lens b))) (buffer-list))))

(define (show-search-lens! s needle)
  (let ((places (append-map (lambda (d)
                              (map (lambda (start) (cons d start))
                                   (delete-duplicates
                                    (map (lambda (m) (line-start d (car m))) (search-all d needle 0 (document-length d))))))
                            (text-documents))))
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
(define-key! minibuffer-map "C-c C-;" 'minibuffer-export)

;;; Structured views

(define-record-type row
  (%row columns target)
  row?
  (columns row-columns)
  (target row-target))

;; A row of a view: its columns, strings, and the target it stands for.
(define (row #:target [target #f] . columns) (%row columns target))

;; The widest text of each column.
(define (column-widths rows)
  (let ((n (fold (lambda (r m) (max m (length (row-columns r)))) 0 rows)))
    (map (lambda (i)
           (fold (lambda (r m) (if (< i (length (row-columns r))) (max m (string-length (list-ref (row-columns r) i))) m))
                 0 rows))
         (iota n))))

;; The rows' text: columns padded to the widest of each, one row a line.
(define (rows-text rows)
  (let ((widths (column-widths rows)))
    (apply string-append
           (map (lambda (r)
                  (let loop ((cols (row-columns r)) (ws widths) (acc ""))
                    (cond ((null? cols) (string-append (string-trim-right acc) "\n"))
                          ((null? (cdr cols)) (loop '() '() (string-append acc (car cols))))
                          (else (loop (cdr cols) (cdr ws)
                                      (string-append acc (car cols) (make-string (+ 2 (- (car ws) (string-length (car cols)))) #\space)))))))
                rows))))

;; A view's first column, its labels, is drawn as comments.
(define (first-column-layer width)
  (lambda (d from to)
    (let loop ((p (line-start d from)) (acc '()))
      (if (or (>= p to) (>= p (document-length d)))
          (reverse acc)
          (loop (+ (line-end d p) 1)
                (cons (list p (let step ((q p) (n width))
                                (if (or (= n 0) (>= q (line-end d p))) q (step (next-grapheme d q) (- n 1))))
                            'comment)
                      acc))))))

;; What a view's buffer keeps: how to make its rows, the target of each,
;; its first column's width, and what the mode extending rows-mode keeps
;; (the inspector's stack).
(define-record-type rows-view
  (make-rows-view make-rows targets width data)
  rows-view?
  (make-rows rows-view-make-rows)
  (targets rows-view-targets)
  (width rows-view-width)
  (data rows-view-data))

(define-mode rows-mode
  "A structured view: rows of generated text, each standing for a target.
RET does the default action on the row's; C-c C-r makes the rows again."
  #:parent 'special-mode
  #:keys '(("RET" act-default-at-point) ("C-c C-o" act-default-at-point) ("C-c C-r" view-refresh))
  #:normal '(("RET" act-default-at-point))
  #:layer (lambda (d from to)
            (let ((b (document-buffer d)))
              (if (and b (rows-view? (buffer-state b))) ((first-column-layer (rows-view-width (buffer-state b))) d from to) '())))
  #:target-at (lambda (b pos)
                (let ((targets (rows-view-targets (buffer-state b)))
                      (i (- (line-number (buffer-document b) pos) 1)))
                  (and (< i (vector-length targets)) (vector-ref targets i)))))

;; What the mode extending rows-mode keeps in view buffer B.
(define (view-data b)
  (and b (rows-view? (buffer-state b)) (rows-view-data (buffer-state b))))

;; Show the view NAME, whose rows (MAKE-ROWS session) gives, in MODE
;; (rows-mode or one extending it), keeping DATA for it.
(define (show-view! s name make-rows #:mode [mode 'rows-mode] #:data [data #f])
  (let ((rows (make-rows s)))
    (show-lens! s name (list (rows-text rows))
                #:mode mode
                #:state (make-rows-view make-rows (list->vector (map row-target rows))
                                        (if (null? rows) 0 (car (column-widths rows))) data))
    (when (null? rows) (message! s "Nothing to show"))))

(define-command (view-refresh s n)
  "Make this view's rows again."
  (let* ((b (or (session-buffer s) (error "Not a view"))) (v (buffer-state b)))
    (unless (rows-view? v) (error "Not a view"))
    (show-view! s (buffer-name b) (rows-view-make-rows v) #:mode (buffer-mode b) #:data (rows-view-data v))))

;; (define-view (name s) doc body ...): the command NAME shows the rows
;; BODY gives in a buffer of their own, RET doing the default action on a
;; row's target and C-c C-r showing them again.
(define-syntax define-view
  (syntax-rules ()
    ((_ (name s) doc body ...)
     (define-command (name s n) doc
       (show-view! s (string-append "*" (symbol->string 'name) "*") (lambda (s) body ...))))))
