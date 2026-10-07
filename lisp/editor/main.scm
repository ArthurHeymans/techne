;;; The editor application: a session over a view the host made, commands
;;; for files, panes and the live loop, and what the host asks of Lisp to
;;; present the session (panes, mode lines, the echo area, highlights, the
;;; cursor shape). The runtime (techne-editor) calls the procedures
;;; provided here.

(require "session.scm")
(require "commands.scm")
(require "emacs.scm")
(require "modal.scm")

(provide start-session editor-press editor-click editor-message! session-quit?
         editor-panes editor-focus pane-status echo-line pane-layers cursor-shape
         bound-keys editor-unsendable! find-file current-session eval-region!)

(define (start-session view profile-name)
  (let ((d (view-document view)))
    (when (document-path d) (hash-table-set! %documents (document-path d) d)))
  (set! %session (make-session-for-view view (if (equal? profile-name "modal") modal-profile emacs-profile)))
  %session)

;; The session last started: the one code evaluated from the editor acts on.
(define %session #f)
(define (current-session) %session)

;; Documents by file, so a file opened twice is one document.
(define %documents (make-hash-table))

(define (find-file path)
  (or (hash-table-ref/default %documents path #f)
      (let ((d (open-file path)))
        (hash-table-set! %documents path d)
        d)))

(define (editor-press s key) (press s key))
(define (editor-click s view pos extend)
  (focus-view! s view)
  ((profile-click (sget s 'profile)) s pos extend))
(define (editor-message! s text) (message! s text))
(define (session-quit? s) (sget s 'quit))
;; The profile's keymap. The modal profile has none: its keys are plain
;; characters and a few control keys every terminal sends.
(define (session-keymap s)
  (and (eq? (profile-name (sget s 'profile)) 'emacs) emacs-map))

;; The key sequences bound, for the frontend to check which it can send.
(define (bound-keys s)
  (let ((km (session-keymap s)))
    (if km (keymap-sequences km) '())))

;; Bound keys the frontend cannot send: say which commands they leave out
;; of reach, and remember them.
;; Note the keys the terminal cannot send, unless a message (such as the
;; journal's recovery) is showing.
(define (editor-unsendable! s keys)
  (sset! s 'unsendable keys)
  (unless (or (null? keys) (sget s 'message))
    (message! s (string-append
                 "Keys this terminal cannot send: "
                 (string-join (map (lambda (k)
                                     (let ((b (lookup-key (session-keymap s) (kbd k))))
                                       (if (symbol? b) (string-append k " (" (symbol->string b) ")") k)))
                                   keys)
                              ", ")))))

(define-command (save-buffer s n)
  "Write the document to its file."
  (let ((d (doc s)))
    (document-save! d)
    (message! s (string-append "Wrote " (document-path d)))))

(define-command (quit s n)
  "End the session. Unsaved edits stay in the journal for the next start."
  (sset! s 'quit #t))

(define-key! emacs-map "C-x C-s" 'save-buffer)
(define-key! emacs-map "C-x C-c" 'quit)

(define (state-name s)
  (case (sget s 'mode)
    ((insert) "INSERT")
    ((visual) "VISUAL")
    ((normal) "NORMAL")
    (else #f)))

;;; What the frontend shows: panes, each with its mode line and the
;;; layers' highlights, and the echo area.

(define (editor-panes s) (session-panes s))
(define (editor-focus s) (session-focus s))

(define (view-point v) (cadr (list-ref (view-ranges v) (view-primary v))))

;; File, modified mark, line, the modes on; in the focused pane, the modal
;; state too.
(define (pane-status s view)
  (let* ((d (view-document view))
         (focused (view=? view (session-view s)))
         (modes (map symbol->string (filter (lambda (m) (mode-on? s m)) (sget s 'modes))))
         (parts (list (or (document-path d) "*scratch*")
                      (if (document-dirty? d) "[+]" #f)
                      (string-append "L" (number->string (line-number d (view-point view))))
                      (and focused (state-name s))
                      (and (pair? modes) (string-append "(" (string-join modes " ") ")")))))
    (string-join (filter (lambda (x) x) parts) "  ")))

;; Keys waiting for the rest of their sequence, the search being typed,
;; and the message or open prompt.
(define (echo-line s)
  (let* ((prompt (and (eq? (profile-name (sget s 'profile)) 'modal) (modal-prompt s)))
         (pending (append (or (sget s 'pending) '()) (or (sget s 'mode-pending) '())))
         (parts (list (and (pair? pending) (string-append (string-join pending " ") "-"))
                      (and (sget s 'isearch) (string-append "I-search: " (cadr (sget s 'isearch))))
                      (or prompt (sget s 'message)))))
    (string-join (filter (lambda (x) x) parts) "  ")))

(define (pane-layers s view from to) (session-layers s (view-document view) from to))

(define (cursor-shape s view)
  (if (and (view=? view (session-view s)) (memq (sget s 'mode) '(normal visual))) 'block 'bar))

;;; Panes.

(define-command (split-window-below s n)
  "Show the focused view's document in a second pane below it."
  (let* ((panes (session-panes s)) (i (session-focus s)) (new (view-split (list-ref panes i))))
    (set-session-panes! s (append (take panes (+ i 1)) (list new) (drop panes (+ i 1))) i)))

(define-command (other-window s n)
  "Focus the next pane."
  (sset! s 'focus (modulo (+ (session-focus s) n) (length (session-panes s)))))

(define-command (delete-window s n)
  "Close the focused pane."
  (let ((panes (session-panes s)) (i (session-focus s)))
    (if (= (length panes) 1)
        (message! s "The only pane")
        (set-session-panes! s (append (take panes i) (drop panes (+ i 1))) (min i (- (length panes) 2))))))

(define-command (delete-other-windows s n)
  "Close every pane but the focused one."
  (set-session-panes! s (list (session-view s)) 0))

;;; The live loop: evaluate code in the module of its file, see the result,
;;; jump to definitions and back.

;; The module code of DOC evaluates in: its file's, else the user module.
(define (document-module d)
  (let ((path (document-path d)))
    (if (and path (string-suffix? ".scm" path)) path "user")))

;; Evaluate the text from FROM to TO of the focused document in its
;; module, as part of its file; show the result.
(define (eval-region! s from to)
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

(define-command (eval-buffer s n)
  "Evaluate every top-level form of the document in its module."
  (let ((forms (document-forms (doc s))))
    (unless (null? forms) (eval-region! s (car (car forms)) (cadr (last forms))))))

;; The identifier around point, as a symbol.
(define (symbol-at-point s)
  (let* ((d (doc s)) (p (point s))
         (start (line-start d p)) (end (line-end d p))
         (line (document-substring d start end))
         (col (string-length (document-substring d start p)))
         (delimiter? (lambda (c) (or (char-whitespace? c) (memv c '(#\( #\) #\[ #\] #\" #\; #\' #\` #\,)))))
         (chars (string->list line))
         (from (let loop ((i col)) (if (and (> i 0) (not (delimiter? (list-ref chars (- i 1))))) (loop (- i 1)) i)))
         (to (let loop ((i col)) (if (and (< i (length chars)) (not (delimiter? (list-ref chars i)))) (loop (+ i 1)) i))))
    (if (= from to) (error "No identifier at point") (string->symbol (substring line from to)))))

;; Show the line LINE (1-based), column COLUMN, of the file PATH in the
;; focused pane, remembering where it was.
(define (visit! s path line column)
  (let* ((d (find-file path))
         (v (make-view d "user"))
         (p (line-down d 0 (- line 1) (- column 1)))
         (panes (session-panes s)) (i (session-focus s)))
    (sset! s 'visited (cons (session-view s) (or (sget s 'visited) '())))
    (view-set-ranges! v (list (list p p)) 0)
    (view-set-scroll! v (line-start d p))
    (set-session-panes! s (append (take panes i) (list v) (drop panes (+ i 1))) i)))

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
  (let ((visited (sget s 'visited)) (panes (session-panes s)) (i (session-focus s)))
    (if (null? (or visited '()))
        (message! s "Nothing to go back to")
        (begin (sset! s 'visited (cdr visited))
               (set-session-panes! s (append (take panes i) (list (car visited)) (drop panes (+ i 1))) i)))))

(define-command (describe-at-point s n)
  "Show the signature, location and documentation of the name at point."
  (let ((name (symbol-at-point s)))
    (message! s (eval `(%describe ',name) (document-module (doc s))))))

(for-each (lambda (b) (define-key! emacs-map (car b) (cadr b)))
          '(("C-x 2" split-window-below) ("C-x o" other-window) ("C-x 0" delete-window) ("C-x 1" delete-other-windows)
            ("C-M-x" eval-defun) ("C-x C-e" eval-defun) ("C-c C-k" eval-buffer)
            ("M-." find-definition) ("M-," pop-definition) ("C-h ." describe-at-point)))
