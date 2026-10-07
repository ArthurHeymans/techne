;;; The editor application: a session over a view the host made, commands
;;; for files, panes and the live loop, and what the host asks of Lisp to
;;; present the session (panes, mode lines, the echo area, highlights, the
;;; cursor shape). The runtime (techne-editor) calls the procedures
;;; provided here.

(require "session.scm")
(require "commands.scm")
(require "emacs.scm")
(require "modal.scm")
(require "targets.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "lens.scm")
(require "inspect.scm")
(require "shell.scm")
(require "repl.scm")

(provide start-session editor-press editor-click editor-message! session-quit?
         editor-panes editor-focus pane-status echo-line pane-layers cursor-shape editor-minibuffer
         editor-session-state editor-restore! editor-clipboard! editor-clipboard-out
         bound-keys editor-unsendable! current-session eval-region!)

(define (start-session view profile-name)
  (add-buffer! (view-document view))
  (set! %session (make-session-for-view view (if (equal? profile-name "modal") modal-profile emacs-profile)))
  %session)

;; The session last started: the one code evaluated from the editor acts on.
(define %session #f)
(define (current-session) %session)

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

;; The key each command is bound to in a keymap, as Marginalia shows one:
;; (command . key), a plain chord before a named key, the shortest first.
(define (command-keys km)
  (let ((by-command (make-hash-table)) (named? (lambda (k) (string-contains k "<"))))
    (for-each (lambda (seq)
                (let ((b (lookup-key km (kbd seq))))
                  (when (symbol? b)
                    (hash-table-update!/default by-command b (lambda (l) (cons seq l)) '()))))
              (keymap-sequences km))
    (map (lambda (b)
           (cons b (car (sort (hash-table-ref/default by-command b '())
                              (lambda (x y) (if (eq? (not (named? x)) (not (named? y)))
                                                (< (string-length x) (string-length y))
                                                (not (named? x))))))))
         (hash-table-keys by-command))))

(define (first-line text)
  (let ((i (string-index text #\newline))) (if i (substring text 0 i) text)))

(define-command (execute-extended-command s n)
  "Run a command by its name."
  (let* ((km (if (eq? (profile-name (sget s 'profile)) 'emacs) emacs-map modal-map))
         (keys (command-keys km))
         (names (sort (command-names) (lambda (a b) (string<? (symbol->string a) (symbol->string b))))))
    (completing-read s "M-x "
                     (map (lambda (name)
                            (let ((key (assq name keys)) (doc (command-doc name)))
                              (candidate (symbol->string name)
                                         #:suffix (and key (string-append "(" (cdr key) ")"))
                                         #:annotation (if (string? doc) (first-line doc) "")
                                         #:target (target 'command name))))
                          names))))

;; The system clipboard, through the frontend: what another program put
;; there comes in as a kill; what is killed goes out.
(define (editor-clipboard! s text) (clipboard-in! s text))
(define (editor-clipboard-out s) (take-clipboard-out! s))

(define-command (view-echo-area-messages s n)
  "Show *Messages*, the messages shown so far, at its end."
  (let* ((d (messages-document)) (v (show-document! s d)) (end (document-length d)))
    (view-set-ranges! v (list (list end end)) 0)))

(define (written v) (call-with-output-string (lambda (p) (write v p))))

(define-command (eval-expression s n)
  "Read an expression in the minibuffer and evaluate it in the focused
file's module; show the result."
  (let ((module (document-module (doc s))))
    (completing-read s "Eval: " '()
                     #:require-match #f
                     #:accept (lambda (s c)
                                (let ((result (eval-source (candidate-text c) module "*eval*")))
                                  (sset! s 'last-result result)
                                  (message! s (written result)))))))

;;; M-y, as consult-yank-pop: a kill chosen in the minibuffer, previewed
;;; where it goes; after C-y it replaces the text yanked. C-g puts back
;;; what was there.

(define (one-line text)
  (string-join (string-split text "\n") "⏎"))

(define-command (yank-pop s n)
  "Choose a kill to insert, previewing it in place; after a yank, it
replaces the text yanked."
  (when (null? (kill-ring s)) (error "the kill ring is empty"))
  (let* ((v (pane-view s))
         (after-yank (and (memq (sget s 'last-command) '(yank yank-pop)) (sget s 'last-yank)))
         (start (if after-yank (car after-yank) (point s)))
         (span (list start (if after-yank (cadr after-yank) start)))
         (original (document-substring (view-document v) (car span) (cadr span)))
         ;; After a yank the replacement joins its undo unit.
         (group (if after-yank "extend" "new"))
         (put! (lambda (s text)
                 (view-edit! v (list (list (car span) (cadr span) text)) group)
                 (set! group "extend")
                 (set! span (list (car span) (cadr (list-ref (view-ranges v) (view-primary v))))))))
    (completing-read s "Yank from kill ring: "
                     (map (lambda (k) (candidate (one-line (car k)) #:target (target 'kill (car k)))) (kill-ring s))
                     #:preview (lambda (s c) (put! s (target-value (candidate-target c))))
                     #:accept (lambda (s c)
                                (put! s (target-value (candidate-target c)))
                                (sset! s 'last-yank span))
                     #:abort (lambda (s) (put! s original)))))

(define-action kill (insert-kill s text) "Insert the text." (insert-text! s text 'new))

;; Commands are targets too.
(define-action command (run-named-command s name) "Run the command." (run-command s name 1))
(define-action command (describe-command s name)
  "Show the command's documentation and where it is defined."
  (let ((where (procedure-location (command name))))
    (message! s (string-append (symbol->string name) ": " (or (command-doc name) "")
                               (if where (string-append "  (" (car where) ":" (number->string (cadr where)) ")") "")))))
(define-action command (find-command-definition s name)
  "Go to the command's definition."
  (let ((where (procedure-location (command name))))
    (if where (visit! s (car where) (cadr where) (caddr where)) (error "No source for" name))))

(for-each (lambda (b) (define-key! emacs-map (car b) (cadr b)))
          '(("M-x" execute-extended-command) ("C-x C-f" find-file) ("C-x b" switch-to-buffer) ("C-x k" kill-buffer)
            ("C-;" act-at-point) ("M-s o" lens-search) ("M-y" yank-pop) ("C-h e" view-echo-area-messages) ("M-:" eval-expression)
            ("M-!" shell-command) ("M-&" async-shell-command) ("M-|" shell-command-on-region)
            ;; Doom's leader key without evil: C-c.
            ("C-c a" act-at-point) ("C-c f f" find-file)
            ("C-c s s" search-lines) ("C-c s b" search-lines) ("C-c s B" search-all-buffers)))

;; The modal profile's leader key, as in Doom.
(for-each (lambda (b) (define-key! modal-map (car b) (cadr b)))
          '(("SPC :" execute-extended-command) ("SPC f f" find-file) ("SPC ." find-file)
            ("SPC b b" switch-to-buffer) ("SPC ," switch-to-buffer) ("SPC b k" kill-buffer)
            ("SPC a" act-at-point) ("SPC s s" search-lines) ("SPC s b" search-lines) ("SPC s B" search-all-buffers)
            ("SPC w s" split-window-below) ("SPC w w" other-window) ("SPC w d" delete-window)
            ("SPC c e" eval-buffer-or-region) ("SPC c d" find-definition) ("SPC c k" inspect-at-point)))

(define (state-name s)
  (case (sget s 'mode)
    ((insert) "INSERT")
    ((visual) "VISUAL")
    ((normal) "NORMAL")
    (else #f)))

;;; Coming back after a crash: the host keeps what `editor-session-state`
;;; last gave and hands it to the next runtime's `editor-restore!`. Files
;;; are opened again with their journals, so with their unsaved edits;
;;; generated buffers (lenses, views) are not kept.

(define (file-of d)
  (and (document-path d) (not (doc-prop d 'lens)) (absolute-path (document-path d))))

(define (editor-session-state s)
  (let* ((kept (filter (lambda (v) (file-of (view-document v))) (session-panes s)))
         (focus (or (list-index (lambda (v) (view=? v (pane-view s))) kept) 0))
         (pane (lambda (v)
                 (let ((r (list-ref (view-ranges v) (view-primary v))))
                   (list (file-of (view-document v)) (car r) (cadr r) (view-scroll v))))))
    (call-with-output-string
     (lambda (p)
       (write `((buffers ,@(filter-map file-of (buffer-list))) (panes ,@(map pane kept)) (focus ,focus)) p)))))

(define (editor-restore! s text)
  (let* ((state (read (open-input-string text)))
         (field (lambda (k) (cdr (assq k state))))
         (open (lambda (path) (guard (e (#t #f)) (file-document path))))
         (view-at (lambda (d anchor head scroll)
                    (let ((v (make-view d "user")) (len (document-length d)))
                      (guard (e (#t #f)) (view-set-ranges! v (list (list (min anchor len) (min head len))) 0))
                      (guard (e (#t #f)) (view-set-scroll! v (min scroll len)))
                      (set-doc-prop! d 'view v)
                      v)))
         (views (filter-map (lambda (p)
                              (let ((d (open (car p))))
                                (and d (view-at d (cadr p) (caddr p) (cadddr p)))))
                            (field 'panes))))
    (for-each (lambda (path) (let ((d (open path))) (when d (add-buffer! d)))) (reverse (field 'buffers)))
    (unless (null? views)
      (set-session-panes! s views (min (car (field 'focus)) (- (length views) 1))))))

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
         (parts (list (or (document-path d) (buffer-name d) "*scratch*")
                      (if (and (document-dirty? d) (not (doc-prop d 'lens)) (not (doc-prop d 'read-only))) "[+]" #f)
                      (string-append "L" (number->string (line-number d (view-point view))))
                      (and focused (state-name s))
                      (and (pair? modes) (string-append "(" (string-join modes " ") ")")))))
    (string-join (filter (lambda (x) x) parts) "  ")))

;; Keys waiting for the rest of their sequence, the search being typed,
;; and the message or open prompt.
(define (echo-line s)
  (let* ((prompt (and (eq? (profile-name (sget s 'profile)) 'modal) (modal-prompt s)))
         (pending (append (or (sget s 'prefix-keys) '()) (or (sget s 'pending) '()) (or (sget s 'mode-pending) '())))
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
  (set-session-panes! s (list (pane-view s)) 0))

;;; The live loop: evaluate code in the module of its file, see the result,
;;; jump to definitions and back.

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

(for-each (lambda (b) (define-key! emacs-map (car b) (cadr b)))
          '(("C-x 2" split-window-below) ("C-x o" other-window) ("C-x 0" delete-window) ("C-x 1" delete-other-windows)
            ("C-M-x" eval-defun) ("C-x C-e" eval-last-sexp) ("C-c C-k" eval-buffer)
            ("M-." find-definition) ("M-," pop-definition) ("C-h ." describe-at-point)
            ;; Doom's code prefix, C-c c.
            ("C-c c e" eval-buffer-or-region) ("C-c c d" find-definition) ("C-c c k" inspect-at-point)
            ;; Geiser's documentation at point.
            ("C-c C-d C-d" inspect-at-point) ("C-c C-d d" inspect-at-point)
            ;; As CIDER's inspector.
            ("C-c M-i" inspect-last-result)))
