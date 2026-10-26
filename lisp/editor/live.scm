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
(require "files.scm")
(require "inspect.scm")
(require "completion.scm")
(require "indent.scm")

(provide eval-region! eval-expression eval-defun eval-last-sexp eval-buffer eval-buffer-or-region inspect-at-point indent-sexp
         find-definition pop-definition describe-at-point symbol-at-point)

(define (written v) (call-with-output-string (lambda (p) (write v p))))

(define (user-value s thunk)
  "Return (VALUE), VALUE what THUNK returns, or #f after showing in
session S what THUNK raised. THUNK runs the user's code: what that raises
is the user's to fix, not a failure of the command running it, nor a
violation of the editor's (`take-violations!`). Its time is not the
keystroke budget's: `ran-user-code` says that it ran."
  (sset! s 'ran-user-code #t)
  (guard (e (#t (message! s (error-text e)) #f))
    (list (thunk))))

(define (show-result! s found #:source [source #f])
  "Keep and show the value in FOUND, from `user-value`, if there is one.
A definition gives no value: if SOURCE, the code evaluated, ends with
one, the name it defines is shown instead, as Emacs shows it."
  (when found
    (sset! s 'last-result (car found))
    (message! s (let ((name (and source (eq? (car found) (if #f #f)) (defined-name source))))
                  (if name (symbol->string name) (written (car found)))))))

;; The name the last form of SOURCE defines, if it is a definition.
(define (defined-name source)
  (guard (e (#t #f))
    (let ((p (open-input-string source)))
      (let loop ((last #f))
        (let ((form (read p)))
          (if (eof-object? form)
              (and (pair? last) (symbol? (car last)) (string-prefix? "define" (symbol->string (car last)))
                   (pair? (cdr last))
                   (let name ((target (cadr last)))
                     (cond ((symbol? target) target) ((pair? target) (name (car target))) (else #f))))
              (loop form)))))))

;; The module of document D's code, loaded from D's text the first time
;; code of it is evaluated: unsaved edits and all, not its file as it was
;; saved; up to a form not complete yet, as it is typed. A load that fails
;; says why in S's echo area (and *Messages*), and leaves the module as
;; far as it got, for what was asked to go on in. Return (VALUE), VALUE
;; what the text's last form gave, if it loaded it now; else #t.
(define (load-module! s d)
  (let ((m (document-module d)))
    (or (string=? m "user")
        (let* ((forms (document-forms d))
               (text (document-substring d 0 (if (null? forms) 0 (cadr (last forms)))))
               (found (user-value s (lambda () (load-module-from m text)))))
          (cond ((not found)
                 (message! s (string-append "Loading " (file-name m) ": " (sget s 'message)))
                 #t)
                ((car found))
                (else #t))))))

(define-command (eval-expression s n)
  "Read an expression in the minibuffer and evaluate it; show the result.
It is evaluated in the focused file's module."
  (let ((module (document-module (doc s))))
    (load-module! s (doc s))
    (completing-read s "Eval: " '()
                     #:require-match #f
                     #:history 'eval-expression
                     #:accept (lambda (s c)
                                (show-result! s (user-value s (lambda () (eval-source (candidate-text c) module "*eval*"))))))))

;;; The live loop: evaluate code in the module of its file, see the result,
;;; jump to definitions and back.

(define (eval-region! s from to)
  "Evaluate the text from FROM to TO of S's focused document; show it.
It is evaluated in the document's module, as part of its file; the
first time, that module is loaded from the document's text."
  (let ((d (doc s)))
    (load-module! s d)
    (let* ((line (line-number d from))
           (column (+ 1 (- from (line-start d from))))
           (text (document-substring d from to))
           (padded (string-append (make-string (- line 1) #\newline) (make-string (- column 1) #\space) text)))
      (show-result! s (user-value s (lambda () (eval-source padded (document-module d) (or (document-path d) "*scratch*"))))
                    #:source text))))

;; The top-level form around point, else the last one before it.
(define (form-at s)
  (let ((p (point s)) (forms (document-forms (doc s))))
    (or (find (lambda (f) (and (<= (car f) p) (< p (cadr f)))) forms)
        (let ((before (filter (lambda (f) (<= (cadr f) p)) forms)))
          (and (pair? before) (last before)))
        (error "No form here"))))

(define-command (eval-defun s n)
  "Evaluate the top-level form around point in its file's module."
  ;; Reading the file to find the form is evaluating it, as long as it is
  ;; long: only a reader that knows where forms are without reading all
  ;; before would be quicker.
  (sset! s 'ran-user-code #t)
  (let ((f (form-at s))) (eval-region! s (car f) (cadr f))))

(define-command (indent-sexp s n)
  "Indent the lines of the top-level form around point."
  ;; Its lines found as indenting finds them, not by reading the text.
  (let ((f (or (lisp-form-around (doc s) (point s)) (error "No form here"))))
    (indent-lines! s (line-starts (doc s) (car f) (cadr f)) 'new)))

(define-command (eval-last-sexp s n)
  "Evaluate the expression before point in its file's module."
  (sset! s 'ran-user-code #t)
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
  (load-module! s (doc s))
  (let* ((name (symbol-at-point s))
         (found (user-value s (lambda ()
                                (guard (e ((memq name (command-names)) (command name)))
                                  (eval name (document-module (doc s))))))))
    (when found (inspect! s (car found) #:name name))))

(define-command (eval-buffer s n)
  "Evaluate every top-level form of the document in its module."
  ;; Reading the whole document is evaluating it, as long as it is long.
  (sset! s 'ran-user-code #t)
  (let* ((d (doc s)) (loaded (load-module! s d)))
    (cond ((pair? loaded)
           ;; Loaded from the text now: that evaluated it.
           (show-result! s loaded #:source (document-substring d 0 (document-length d))))
          (else
           (let ((forms (document-forms d)))
             (unless (null? forms) (eval-region! s (car (car forms)) (cadr (last forms)))))))))

(define (symbol-at-point s)
  "Return the identifier around point in session S, as a symbol."
  (let ((span (identifier-span (doc s) (point s))))
    (if (= (car span) (cadr span))
        (error "No identifier at point")
        (string->symbol (document-substring (doc s) (car span) (cadr span))))))

(define-command (find-definition s n)
  "Go to the definition of the procedure named at point."
  (load-module! s (doc s))
  (let* ((name (symbol-at-point s))
         (found (user-value s (lambda () (eval name (document-module (doc s))))))
         (where (and found (procedure? (car found)) (procedure-location (car found)))))
    (cond ((not found))
          ((and where (file-exists? (car where))) (visit! s (car where) (cadr where) (caddr where)))
          (else (message! s (string-append "No source for " (symbol->string name)))))))

(define-command (pop-definition s n)
  "Go back to where find-definition was used."
  (let ((visited (sget s 'visited)))
    (if (null? (or visited '()))
        (message! s "Nothing to go back to")
        (let ((b (document-buffer (view-document (car visited)))))
          (sset! s 'visited (cdr visited))
          (set-pane-view! s (car visited))
          ;; Back where it was, it is the buffer shown last, for C-x b.
          (when b (remember-buffer! b))))))

(define-command (describe-at-point s n)
  "Show the signature, location and documentation of the name at point."
  (let ((name (symbol-at-point s)))
    (load-module! s (doc s))
    (message! s (eval `(%describe ',name) (document-module (doc s))))))

;; Scheme buffers, with Geiser's keys.
(define-mode scheme-mode
  "Techne Lisp and Scheme: code evaluates in its file's module."
  #:parent 'prog-mode
  #:files '(".scm" ".sld" ".sls" ".ss")
  #:complete (lambda (b pos) (scheme-completion (buffer-document b) pos (document-module (buffer-document b))))
  #:indent lisp-indent
  #:normal '(("SPC m e d" eval-defun) ("SPC m e e" eval-last-sexp) ("SPC m e b" eval-buffer))
  ;; A terminal without the kitty protocol sends C-M-i as M-TAB, as
  ;; Emacs binds both.
  #:keys '(("C-M-q" indent-sexp) ("C-M-x" eval-defun) ("C-c C-k" eval-buffer) ("C-M-i" completion-at-point) ("M-TAB" completion-at-point)
           ;; Geiser's documentation at point.
           ("C-c C-d C-d" inspect-at-point) ("C-c C-d d" inspect-at-point)
           ;; As CIDER's inspector.
           ("C-c M-i" inspect-last-result)))
