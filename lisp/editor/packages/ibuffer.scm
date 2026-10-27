;;; The buffer list, as Emacs's ibuffer shows it: a row a buffer, under a
;;; heading for its project (the version-control root of its file) or for
;;; buffers of no file. Rows are marked (m), flagged (d), and the marked
;;; ones killed (D, x for the flagged) or saved (S) together; RET shows a
;;; buffer. Sorting (s) and filtering (/) are kept in the listing.

(import (techne editor))

(define-option ibuffer-sort 'recency
  "How the buffer list is sorted within each group.
By `recency` (the most recently shown first), `name`, `size` (the
largest first) or `mode`."
  #:type '(one-of recency name size mode))

(define-option ibuffer-group-by-project #t
  "Whether the buffer list groups file buffers by project."
  #:type 'boolean)

;;; The listing's state: `sort`, and `filters`, an alist of (kind . text)
;;; each buffer shown must pass: KIND `name`, `mode` or `file`.

(define (ibuffer? b) (and b (derived-mode? (buffer-mode b) 'ibuffer-mode) (listing? b)))

(define (current s)
  (let ((b (session-buffer s)))
    (if (ibuffer? b) b (error "Not the buffer list"))))

(define (sort-by b) (or (listing-get b 'sort) (option b 'ibuffer-sort)))
(define (filters b) (or (listing-get b 'filters) '()))

(define (contains? text part) (and (string-contains (string-downcase text) (string-downcase part)) #t))

(define (passes? x f)
  (case (car f)
    ((name) (contains? (buffer-name x) (cdr f)))
    ((mode) (string=? (symbol->string (buffer-mode x)) (cdr f)))
    ((file) (let ((p (document-path (buffer-document x)))) (and p (contains? p (cdr f)))))
    (else #t)))

(define (size x) (document-length (buffer-document x)))

(define (sorted xs by)
  (case by
    ((name) (sort xs (lambda (a b) (string-ci<? (buffer-name a) (buffer-name b)))))
    ((size) (sort xs (lambda (a b) (> (size a) (size b)))))
    ((mode) (sort xs (lambda (a b) (string<? (symbol->string (buffer-mode a)) (symbol->string (buffer-mode b))))))
    (else xs)))

;; The group of buffer X: its project's root, or #f for a remote buffer
;; or one of no file. Redisplay must not connect to another machine.
(define (group x)
  (let ((p (document-path (buffer-document x))))
    (and p (not (remote-path? p)) (guard (e (#t #f)) (project-root (absolute-directory p))))))

(define (absolute-directory path)
  (let loop ((i (- (string-length path) 1)))
    (cond ((< i 0) "/") ((char=? (string-ref path i) #\/) (substring path 0 (+ i 1))) (else (loop (- i 1))))))

(define (mode-label m)
  (let ((s (symbol->string m))) (if (string-suffix? "-mode" s) (substring s 0 (- (string-length s) 5)) s)))

(define (flags x)
  (let ((d (buffer-document x)))
    (string (if (and (document-path d) (document-dirty? d)) #\* #\space)
            (if (option x 'read-only) #\% #\space))))

(define (buffer-row b x)
  (let ((mark (listing-mark b (buffer-name x))) (path (document-path (buffer-document x))))
    (listing-row b (buffer-name x) (target 'buffer x)
                 (list (list (flags x) 'warning))
                 (list (list (buffer-name x) (cond ((eqv? mark #\D) 'error) (mark 'warning) ((not path) 'comment) (else #f))))
                 (number->string (size x))
                 (list (list (mode-label (buffer-mode x)) 'keyword))
                 (list (list (if path (abbreviate-path path) "") 'comment)))))

(define (heading key text) (row #:key key (list (list text 'heading.ibuffer))))

(define (ibuffer-rows b)
  (let* ((shown (sorted (filter (lambda (x) (and (not (eq? x b)) (every (lambda (f) (passes? x f)) (filters b))))
                                (buffer-list))
                        (sort-by b)))
         (title (row #:key "" (list (list (string-append "Buffers by " (symbol->string (sort-by b))
                                                         (apply string-append
                                                                (map (lambda (f) (string-append ", " (symbol->string (car f)) " ~ " (cdr f)))
                                                                     (filters b))))
                                          'comment)))))
    (cons title
          (if (option b 'ibuffer-group-by-project)
              ;; Each buffer's group found once: it looks at the disk.
              (let* ((grouped (map (lambda (x) (cons x (group x))) shown))
                     (roots (delete-duplicates (filter-map cdr grouped)))
                     (in (lambda (r) (map car (filter (lambda (g) (equal? (cdr g) r)) grouped))))
                     (part (lambda (key text xs)
                             (if (null? xs) '() (cons (heading key text) (map (lambda (x) (buffer-row b x)) xs))))))
                (append (append-map (lambda (r)
                                      (part (string-append "[project " r) (string-append "[ " (abbreviate-path r) " ]") (in r)))
                                    roots)
                        (part "[other" "[ Other ]" (in #f))))
              (map (lambda (x) (buffer-row b x)) shown)))))

(define-mode ibuffer-mode
  "The buffers, grouped by project.
\\[ibuffer-visit] shows the buffer at point, \\[ibuffer-visit-other-pane]
in a new pane. The buffers marked with \\[listing-mark-row], or the one
at point, are killed with \\[ibuffer-do-kill] and saved with
\\[ibuffer-do-save]; \\[ibuffer-do-kill-flagged] kills those flagged
with \\[listing-flag-row]. \\[ibuffer-mark-unsaved] marks the unsaved
ones. \\[ibuffer-cycle-sort] sorts, \\[ibuffer-filter-by-name] and
\\[ibuffer-filter-by-mode] filter, \\[ibuffer-filter-clear] shows all."
  #:parent 'listing-mode
  #:keys '(("RET" ibuffer-visit) ("o" ibuffer-visit-other-pane)
           ("D" ibuffer-do-kill) ("x" ibuffer-do-kill-flagged) ("S" ibuffer-do-save)
           ("* u" ibuffer-mark-unsaved) ("* s" ibuffer-mark-special)
           ("s" ibuffer-cycle-sort) ("," ibuffer-cycle-sort)
           ("/ n" ibuffer-filter-by-name) ("/ m" ibuffer-filter-by-mode) ("/ f" ibuffer-filter-by-file)
           ("/ /" ibuffer-filter-clear) ("*" "mark") ("/" "filter"))
  #:normal '(("RET" ibuffer-visit) ("o" ibuffer-visit-other-pane)
             ("D" ibuffer-do-kill) ("x" ibuffer-do-kill-flagged) ("S" ibuffer-do-save)
             ("* u" ibuffer-mark-unsaved) ("* s" ibuffer-mark-special)
             ("s" ibuffer-cycle-sort)
             ("/ n" ibuffer-filter-by-name) ("/ m" ibuffer-filter-by-mode) ("/ f" ibuffer-filter-by-file)
             ("/ /" ibuffer-filter-clear) ("*" "mark") ("/" "filter")))

(define (redisplay! s b)
  (let ((k (listing-key-at b (point s))))
    (listing-redisplay! b)
    (when k (listing-goto! s b k))))

(define-command (ibuffer s n)
  "Show the buffers, grouped by project, to act on.
The caret goes to the buffer shown before."
  (let* ((before (current-buffer s))
         (b (make-listing-buffer! "*Ibuffer*" 'ibuffer-mode ibuffer-rows)))
    (show-buffer! s b)
    (when before (listing-goto! s b (buffer-name before)))))

(define (buffer-here s)
  (let ((t (target-at (doc s) (point s))))
    (if (and t (eq? (target-kind t) 'buffer)) (target-value t) (error "No buffer here"))))

(define-command (ibuffer-visit s n)
  "Show the buffer at point."
  (show-buffer! s (buffer-here s)))

(define-command (ibuffer-visit-other-pane s n)
  "Show the buffer at point in a new pane below."
  (show-in-other-pane! s (buffer-document (buffer-here s))))

(define (buffers s mark) (map target-value (listing-targets s #:mark mark)))

(define (kill! s xs)
  (let ((b (current s)))
    (for-each (lambda (x) (drop-buffer! s x)) xs)
    (show-buffer! s b)
    (redisplay! s b)
    (message! s (string-append "Killed " (number->string (length xs)) " buffers"))))

(define-command (ibuffer-do-kill s n)
  "Kill the marked buffers, or the one at point.
Their files' unsaved edits stay in their journals."
  (kill! s (buffers s #\*)))

(define-command (ibuffer-do-kill-flagged s n)
  "Kill the buffers flagged for deletion."
  (let ((xs (map target-value (listing-marked (current s) #\D))))
    (if (null? xs) (message! s "No buffers flagged") (kill! s xs))))

(define-command (ibuffer-do-save s n)
  "Save the marked buffers' files, or the one at point's."
  (let* ((b (current s))
         (files (filter (lambda (x) (document-path (buffer-document x))) (buffers s #\*))))
    (for-each (lambda (x) (save-document! (buffer-document x))) files)
    (redisplay! s b)
    (message! s (string-append "Saved " (number->string (length files)) " buffers"))))

(define (mark-where! s keep?)
  (let ((b (current s)))
    (for-each (lambda (x)
                (when (and (not (eq? x b)) (keep? x))
                  (hash-table-set! (listing-get b 'marks) (buffer-name x) #\*)))
              (buffer-list))
    (redisplay! s b)))

(define-command (ibuffer-mark-unsaved s n)
  "Mark the buffers whose files have unsaved edits."
  (mark-where! s (lambda (x) (let ((d (buffer-document x))) (and (document-path d) (document-dirty? d))))))

(define-command (ibuffer-mark-special s n)
  "Mark the buffers of no file, as *Messages*."
  (mark-where! s (lambda (x) (not (document-path (buffer-document x))))))

(define-command (ibuffer-cycle-sort s n)
  "Sort by the next of recency, name, size and mode."
  (let ((b (current s)))
    (listing-set! b 'sort (case (sort-by b) ((recency) 'name) ((name) 'size) ((size) 'mode) (else 'recency)))
    (redisplay! s b)))

(define (add-filter! s kind prompt candidates)
  (let ((b (current s)))
    (completing-read s prompt candidates
                     #:require-match #f
                     #:accept (lambda (s c)
                                (listing-set! b 'filters (cons (cons kind (candidate-text c)) (filters b)))
                                (redisplay! s b)))))

(define-command (ibuffer-filter-by-name s n)
  "Show only the buffers whose names contain what is read."
  (add-filter! s 'name "Filter by name: " '()))

(define-command (ibuffer-filter-by-file s n)
  "Show only the buffers whose files' names contain what is read."
  (add-filter! s 'file "Filter by file name: " '()))

(define-command (ibuffer-filter-by-mode s n)
  "Show only the buffers in the major mode read."
  (add-filter! s 'mode "Filter by mode: "
               (delete-duplicates (map (lambda (x) (symbol->string (buffer-mode x))) (buffer-list)))))

(define-command (ibuffer-filter-clear s n)
  "Show every buffer again."
  (let ((b (current s)))
    (listing-set! b 'filters '())
    (redisplay! s b)))

;; A new generation takes over the buffer list shown.
(when (ibuffer? (buffer-named "*Ibuffer*"))
  (make-listing-buffer! "*Ibuffer*" 'ibuffer-mode ibuffer-rows))

(define-key! emacs-map "C-x C-b" 'ibuffer)
(define-key! modal-map "SPC b i" 'ibuffer)
