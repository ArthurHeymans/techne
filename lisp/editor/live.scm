;;; The live loop: evaluate code in the module of its file, see the
;;; result, inspect it, jump to definitions and back. Scheme buffers have
;;; Geiser's keys.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "inspect.scm")
(require "completion.scm")

(provide eval-region! eval-expression eval-defun eval-last-sexp eval-buffer eval-buffer-or-region inspect-at-point
         find-definition pop-definition describe-at-point symbol-at-point)

(define (written v) (call-with-output-string (lambda (p) (write v p))))

(define-command (eval-expression s n)
  "Read an expression in the minibuffer and evaluate it; show the result.
It is evaluated in the focused file's module."
  (let ((module (document-module (doc s))))
    (completing-read s "Eval: " '()
                     #:require-match #f
                     #:accept (lambda (s c)
                                (let ((result (eval-source (candidate-text c) module "*eval*")))
                                  (sset! s 'last-result result)
                                  (message! s (written result)))))))

;;; The live loop: evaluate code in the module of its file, see the result,
;;; jump to definitions and back.

(define (eval-region! s from to)
  "Evaluate the text from FROM to TO of S's focused document; show it.
It is evaluated in the document's module, as part of its file."
  (let* ((d (doc s))
         (line (line-number d from))
         (column (+ 1 (- from (line-start d from))))
         (padded (string-append (make-string (- line 1) #\newline) (make-string (- column 1) #\space)
                                (document-substring d from to)))
         (result (eval-source padded (document-module d) (or (document-path d) "*scratch*"))))
    (sset! s 'last-result result)
    (message! s (call-with-output-string (lambda (p) (write result p))))))

;; The top-level form around point, else the last one before it.
(define (form-at s)
  (let ((p (point s)) (forms (document-forms (doc s))))
    (or (find (lambda (f) (and (<= (car f) p) (< p (cadr f)))) forms)
        (let ((before (filter (lambda (f) (<= (cadr f) p)) forms)))
          (and (pair? before) (last before)))
        (error "No form here"))))

(define-command (eval-defun s n)
  "Evaluate the top-level form around point in its file's module."
  (let ((f (form-at s))) (eval-region! s (car f) (cadr f))))

(define-command (eval-last-sexp s n)
  "Evaluate the expression before point in its file's module."
  (let ((span (document-datum-before (doc s) (point s))))
    (if span (eval-region! s (car span) (cadr span)) (message! s "No expression before point"))))

(define-command (eval-buffer-or-region s n)
  "Evaluate the region if it is active, else the whole document."
  (if (sget s 'extend)
      (let ((r (list-ref (ranges s) (view-primary (session-view s)))))
        (sset! s 'extend #f)
        (eval-region! s (min (car r) (cadr r)) (max (car r) (cadr r))))
      (eval-buffer s n)))

(define-command (inspect-at-point s n)
  "Inspect the value of the name at point, in the file's module."
  (let* ((name (symbol-at-point s))
         (value (guard (e ((memq name (command-names)) (command name)))
                  (eval name (document-module (doc s))))))
    (inspect! s value #:name name)))

(define-command (eval-buffer s n)
  "Evaluate every top-level form of the document in its module."
  (let ((forms (document-forms (doc s))))
    (unless (null? forms) (eval-region! s (car (car forms)) (cadr (last forms))))))

(define (symbol-at-point s)
  "Return the identifier around point in session S, as a symbol."
  (let ((span (identifier-span (doc s) (point s))))
    (if (= (car span) (cadr span))
        (error "No identifier at point")
        (string->symbol (document-substring (doc s) (car span) (cadr span))))))

(define-command (find-definition s n)
  "Go to the definition of the procedure named at point."
  (let* ((name (symbol-at-point s))
         (value (eval name (document-module (doc s))))
         (where (and (procedure? value) (procedure-location value))))
    (if (and where (file-exists? (car where)))
        (visit! s (car where) (cadr where) (caddr where))
        (message! s (string-append "No source for " (symbol->string name))))))

(define-command (pop-definition s n)
  "Go back to where find-definition was used."
  (let ((visited (sget s 'visited)))
    (if (null? (or visited '()))
        (message! s "Nothing to go back to")
        (begin (sset! s 'visited (cdr visited))
               (set-pane-view! s (car visited))))))

(define-command (describe-at-point s n)
  "Show the signature, location and documentation of the name at point."
  (let ((name (symbol-at-point s)))
    (message! s (eval `(%describe ',name) (document-module (doc s))))))

;; Scheme buffers, with Geiser's keys.
(define-mode scheme-mode
  "Techne Lisp and Scheme: code evaluates in its file's module."
  #:parent 'prog-mode
  #:files '(".scm" ".sld" ".sls" ".ss")
  #:complete (lambda (b pos) (scheme-completion (buffer-document b) pos (document-module (buffer-document b))))
  #:keys '(("C-M-x" eval-defun) ("C-c C-k" eval-buffer) ("C-M-i" completion-at-point)
           ;; Geiser's documentation at point.
           ("C-c C-d C-d" inspect-at-point) ("C-c C-d d" inspect-at-point)
           ;; As CIDER's inspector.
           ("C-c M-i" inspect-last-result)))
