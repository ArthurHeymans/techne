;;; Lens buffers (EDITOR.md, sections 1 and 3): rows of excerpts of other
;;; buffers between generated labels. Editing an excerpt edits its source
;;; (techne-editor's lens), and an edit the lens cannot make (its source
;;; changed since, it touches a label) is refused with the reason. Excerpts
;;; whose source changed are marked; C-c C-r shows the sources as they are
;;; now. Undo in a lens undoes its edits in their sources, whose history
;;; they are. The search lens shows the lines containing a string in every
;;; buffer; the minibuffer's candidates that are locations become one with
;;; C-c C-e, as Embark exports them.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "files.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "views.scm")

(provide show-lens! lens-search lens-refresh lens-save minibuffer-export)

(define-mode lens-mode
  "Excerpts of other buffers between labels.
Editing an excerpt edits its source. \\[lens-refresh] shows the sources
as they are now; \\[lens-save] writes them."
  #:keys '(("C-c C-o" act-default-at-point) ("C-c C-r" lens-refresh) ("C-x C-s" lens-save))
  #:target-at (lambda (b pos)
                (let ((at (presentation-source-at (buffer-document b) pos)))
                  (and at (target 'location (location (car at) (cadr at)))))))

(define (show-lens! s name rows #:mode [mode 'lens-mode] #:state [state #f])
  "Show a lens of ROWS in S's focused pane as the buffer NAME.
Return its view. ROWS are a view's, with excerpts. The buffer is in
MODE, with STATE, and replaces a buffer of that name."
  (let ((p (make-presentation)))
    (present! p rows)
    (show-buffer! s (make-generated-buffer! name p mode #:state state))))

(define (lens-of s)
  (let ((d (session-document s)))
    (if (presentation? d) d (error "Not a lens"))))

(define-command (lens-refresh s n)
  "Show the sources of this lens as they are now."
  (presentation-refresh! (lens-of s)))

(define-command (lens-save s n)
  "Write the files edited through this lens."
  (let ((dirty (filter (lambda (d) (and (document-dirty? d) (document-path d))) (presentation-sources (lens-of s)))))
    (for-each document-save! dirty)
    (message! s (if (null? dirty) "No changes to write" (string-append "Wrote " (string-join (map document-path dirty) ", "))))))

;;; Lenses of lines

;; Rows of the lines at PLACES (document . position), each labelled by
;; buffer and line, which is its key.
(define (line-rows places)
  (map (lambda (p)
         (let* ((d (car p)) (start (line-start d (cdr p)))
                (label (string-append (document-label d) ":" (number->string (line-number d start)) ": ")))
           (row (list (list label 'comment) (excerpt d start (line-end d start))) #:key label)))
       places))

;; A document's buffer's name, else its file's.
(define (document-label d)
  (let ((b (document-buffer d)))
    (cond (b (buffer-name b)) ((document-path d) => file-name) (else "?"))))

;; The documents of buffers with text of their own.
(define (text-documents) (remove presentation? (map buffer-document (buffer-list))))

(define (show-search-lens! s needle)
  (let ((places (append-map (lambda (d)
                              (map (lambda (start) (cons d start))
                                   (delete-duplicates
                                    (map (lambda (m) (line-start d (car m))) (search-all d needle 0 (document-length d))))))
                            (text-documents))))
    (if (null? places)
        (message! s (string-append "No buffer contains " needle))
        (show-lens! s (string-append "*lens " needle "*") (line-rows places)))))

(define-command (lens-search s n)
  "Show the lines of every buffer containing a string as a lens.
Editing them edits the buffers."
  (completing-read s "Lens of lines containing: " '()
                   #:require-match #f
                   #:accept (lambda (s c) (show-search-lens! s (candidate-text c)))))

(define-command (minibuffer-export s n)
  "Show the candidates matching as a lens of their lines.
This is for candidates that are locations."
  (let ((targets (map candidate-target (minibuffer-candidates s))) (input (minibuffer-input s)))
    (if (and (pair? targets) (every (lambda (t) (and (target? t) (eq? (target-kind t) 'location))) targets))
        (begin
          (abort-minibuffer! s)
          (show-lens! s (string-append "*lens " input "*")
                      (line-rows (map (lambda (t) (let ((l (target-value t))) (cons (location-document l) (location-position l))))
                                      targets))))
        (message! s "Only locations can be shown as a lens"))))

(define-key! minibuffer-map "C-c C-e" 'minibuffer-export)
(define-key! minibuffer-map "C-c C-;" 'minibuffer-export)
