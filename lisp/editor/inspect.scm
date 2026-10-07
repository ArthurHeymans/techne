;;; The inspector (PLAN.md, "The object microscope"): a structured view of
;;; a value. Its rows say what the value is and where it comes from: its
;;; type, documentation and definition, the keys bound to it when it is a
;;; command, and its parts (elements, fields, entries), each a target that
;;; RET inspects in turn; l goes back. Definitions are locations, so RET on
;;; one goes to the source.

(require "session.scm")
(require "commands.scm")
(require "targets.scm")
(require "lens.scm")

(provide inspect! inspect-last-result inspector-map)

(define inspector-map (make-keymap))

;; How a value is written, cut to one line of at most N characters.
(define (short v #:max [n 120])
  (let* ((s (call-with-output-string (lambda (p) (write v p))))
         (line (let ((i (string-index s #\newline))) (if i (substring s 0 i) s))))
    (if (> (string-length line) n) (string-append (substring line 0 (- n 1)) "…") line)))

;; At most this many parts are shown.
(define max-parts 200)

(define (part label v) (row label (short v) #:target (target 'value v)))

(define (parts v)
  (let ((rows (cond ((pair? v)
                     (let loop ((x v) (i 0) (acc '()))
                       (cond ((pair? x) (loop (cdr x) (+ i 1) (cons (part (number->string i) (car x)) acc)))
                             ((null? x) (reverse acc))
                             (else (reverse (cons (part "tail" x) acc))))))
                    ((vector? v) (map (lambda (i) (part (number->string i) (vector-ref v i))) (iota (vector-length v))))
                    ((record? v) (map (lambda (f) (part (symbol->string (car f)) (cdr f))) (record-fields v)))
                    ((hash-table? v) (map (lambda (e) (part (short (car e) #:max 40) (cdr e))) (hash-table->alist v)))
                    (else '()))))
    (if (> (length rows) max-parts)
        (append (take rows max-parts) (list (row "…" (string-append (number->string (- (length rows) max-parts)) " more"))))
        rows)))

;; What a procedure's documentation and definition say.
(define (about v)
  (let ((doc (and (procedure? v) (documentation v)))
        (where (and (procedure? v) (procedure-location v))))
    (append (if (string? doc) (map (lambda (l) (row "doc" l)) (string-split doc "\n")) '())
            (if where
                (list (row "defined" (string-append (car where) ":" (number->string (cadr where)))
                           #:target (target 'location (file-location (car where) (cadr where)))))
                '()))))

;; As a command: its documentation and keys, when NAME is one.
(define (as-command s name)
  (if (and name (memq name (command-names)))
      (list (row "command" (or (command-doc name) "") #:target (target 'command name)))
      '()))

(define (inspector-rows s v name)
  (append (list (row "value" (short v))
                (row "type" (short (type-of v))))
          (if name (list (row "name" (symbol->string name))) '())
          (as-command s name)
          (about v)
          (parts v)))

;; Show V (named NAME, a symbol, if it is a binding's value) in the
;; inspector; what it showed before is kept for l.
(define (inspect! s v #:name [name #f])
  (sset! s 'inspected (cons (cons v name) (or (sget s 'inspected) '())))
  (show-inspected! s))

(define (show-inspected! s)
  (let ((top (car (sget s 'inspected))))
    (show-view! s "*inspect*" (lambda (s) (inspector-rows s (car top) (cdr top))) #:keymap inspector-map)))

(define-command (inspector-back s n)
  "Inspect what was inspected before."
  (let ((stack (or (sget s 'inspected) '())))
    (if (or (null? stack) (null? (cdr stack)))
        (message! s "Nothing inspected before")
        (begin (sset! s 'inspected (cdr stack)) (show-inspected! s)))))

(define-command (inspect-last-result s n)
  "Inspect the value the last evaluation gave."
  (inspect! s (sget s 'last-result)))

(define-action value (inspect-value s v) "Inspect the value." (inspect! s v))
(define-action value (copy-value s v) "Save the value, written, as a kill." (kill-save! s (short v #:max 100000) #f #f))

(for-each (lambda (b) (define-key! inspector-map (car b) (cadr b)))
          '(("RET" act-default-at-point) ("l" inspector-back) ("C-c C-r" lens-refresh)))
