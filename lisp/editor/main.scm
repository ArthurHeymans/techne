;;; The editor application: a session over a view the host made, the
;;; built-in features, and the keys of both profiles. The runtime
;;; (techne-editor) calls what host.scm provides, through here.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "emacs.scm")
(require "modal.scm")
(require "targets.scm")
(require "files.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "views.scm")
(require "lens.scm")
(require "inspect.scm")
(require "shell.scm")
(require "repl.scm")
(require "which-key.scm")
(require "options.scm")
(require "completion.scm")
(require "windows.scm")
(require "live.scm")
(require "host.scm")
(require "checkdoc.scm")
(require "help.scm")
(require "server.scm")

(provide start-session editor-detach! editor-select! editor-open! editor-take-done! editor-forget! editor-press editor-click editor-message! session-quit?
         editor-panes editor-focus pane-status pane-display echo-line editor-completion pane-layers cursor-shape editor-minibuffer
         editor-session-state editor-restore! editor-pane-places editor-key-hints editor-take-request! editor-paged! editor-clipboard! editor-clipboard-out
         bound-keys editor-unsendable!)

(define (start-session view profile-name)
  "Start a session for a frontend attaching, keys read by PROFILE-NAME.
It shows VIEW; with VIEW #f, it shows what the current session shows,
or else an empty *scratch*. PROFILE-NAME is \"emacs\" or \"modal\".
Return the session, now the current one."
  ;; *Messages* is a buffer from the start, as in Emacs.
  (unless (buffer-named "*Messages*")
    (make-generated-buffer! "*Messages*" (messages-document) 'log-mode))
  (let* ((view (cond (view view)
                     ((current-session) (view-split (pane-view (current-session))))
                     (else (make-view (make-document "") "user"))))
         (s (make-session-for-view view (if (equal? profile-name "modal") modal-profile emacs-profile))))
    (add-buffer! (view-document view))
    (attach-session! s)
    s))

(define (editor-detach! s)
  "End the session S of a frontend detaching.
The buffers it showed stay, with their unsaved edits."
  (detach-session! s))

(define (editor-select! s)
  "Make S, whose frontend's input comes next, the current session."
  (set-current-session! s))

(define-command (save-buffer s n)
  "Write the document to its file."
  (let ((d (doc s)))
    (document-save! d)
    (message! s (string-append "Wrote " (document-path d)))))

(define-command (quit s n)
  "End the session. Unsaved edits stay in the journal for the next start.
In the buffer of a file a program waits for, be done with the file
instead, without saving it, and kill its buffer."
  (if (waited? (doc s))
      (drop-buffer! s (current-buffer s))
      (sset! s 'quit #t)))

(define-key! emacs-map "C-x C-s" 'save-buffer)
(define-key! emacs-map "C-x C-c" 'quit)

;; The key each command is bound to in keymaps MAPS, the first binding of
;; a key winning, as Marginalia shows one: (command . key), a plain chord
;; before a named key, the shortest first.
(define (command-keys maps)
  (let ((by-command (make-hash-table)) (named? (lambda (k) (string-contains k "<"))))
    (for-each (lambda (seq)
                (let ((b (key-binding maps (kbd seq))))
                  (when (symbol? b)
                    (hash-table-update!/default by-command b (lambda (l) (cons seq l)) '()))))
              (delete-duplicates (append-map keymap-sequences maps)))
    (map (lambda (b)
           (cons b (car (sort (hash-table-ref/default by-command b '())
                              (lambda (x y) (if (eq? (not (named? x)) (not (named? y)))
                                                (< (string-length x) (string-length y))
                                                (not (named? x))))))))
         (hash-table-keys by-command))))

(define (first-line text)
  (let ((i (string-index text #\newline))) (if i (substring text 0 i) text)))

(define-command (execute-extended-command s n)
  "Run a command by its name, with the prefix argument given before."
  (let* ((arg (current-prefix s))
         (keys (command-keys (if (eq? (profile-name (sget s 'profile)) 'emacs)
                                (active-keymaps s 'chord)
                                (append (active-keymaps s 'normal) (active-keymaps s 'chord)))))
         (names (sort (command-names) (lambda (a b) (string<? (symbol->string a) (symbol->string b))))))
    (completing-read s "M-x "
                     (map (lambda (name)
                            (let ((key (assq name keys)) (doc (command-doc name)))
                              (candidate (symbol->string name)
                                         #:suffix (and key (string-append "(" (cdr key) ")"))
                                         #:annotation (if (string? doc) (first-line doc) "")
                                         #:target (target 'command name))))
                          names)
                     #:accept (lambda (s c)
                                (sset! s 'current-prefix arg)
                                (run-command s (target-value (candidate-target c)) (prefix-count arg))
                                (sset! s 'current-prefix #f)))))

(define-command (view-echo-area-messages s n)
  "Show *Messages*, the messages shown so far, at its end."
  (let* ((d (messages-document)) (v (show-document! s d)) (end (document-length d)))
    (view-set-ranges! v (list (list end end)) 0)))

;;; M-y, as consult-yank-pop: a kill chosen in the minibuffer, previewed
;;; where it goes; after C-y it replaces the text yanked. C-g puts back
;;; what was there.

(define (one-line text)
  (string-join (string-split text "\n") "⏎"))

(define-command (yank-pop s n)
  "Choose a kill to insert, previewing it in place.
After a yank, it replaces the text yanked."
  (when (null? (kill-ring s)) (error "the kill ring is empty"))
  (let* ((v (pane-view s))
         (d (view-document v))
         (after-yank (and (memq (sget s 'last-command) '(yank yank-pop)) (sget s 'last-yank)))
         ;; The span replaced and what it holds, (start end revision text),
         ;; followed through others' edits when it is used.
         (span (or after-yank (list (point s) (point s) (document-revision d) "")))
         (now (lambda ()
                (let ((from (document-map-position d (car span) (caddr span)))
                      (to (document-map-position d (cadr span) (caddr span))))
                  (and from to (string=? (document-substring d from to) (cadddr span)) (list from to)))))
         (original (cadddr span))
         ;; After a yank the replacement joins its undo unit.
         (group (if after-yank "extend" "new"))
         ;; Put TEXT in place of the span; refused when another actor has
         ;; changed it.
         (put! (lambda (s text)
                 (let ((r (or (now) (error "The text yanked over has changed"))))
                   (view-edit! v (list (list (car r) (cadr r) text)) group)
                   (set! group "extend")
                   (set! span (list (car r) (cadr (list-ref (view-ranges v) (view-primary v))) (document-revision d) text))))))
    (unless (now) (error "The text yanked has changed"))
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
(define-action command (describe-named-command s name)
  "Show the command's documentation, keys and definition."
  (describe-name! s name))
(define-action command (find-command-definition s name)
  "Go to the command's definition."
  (let ((where (procedure-location (command name))))
    (if where (visit! s (car where) (cadr where) (caddr where)) (error "No source for" name))))

(for-each (lambda (b) (define-key! emacs-map (car b) (cadr b)))
          '(("M-x" execute-extended-command) ("C-x C-f" find-file) ("C-x b" switch-to-buffer) ("C-x k" kill-buffer)
            ("C-;" act-at-point) ("M-s o" lens-search) ("M-y" yank-pop)
            ("C-v" scroll-up-command) ("<next>" scroll-up-command) ("M-v" scroll-down-command) ("<prior>" scroll-down-command)
            ("C-l" recenter-top-bottom) ("M-:" eval-expression)
            ("M-!" shell-command) ("M-&" async-shell-command) ("M-|" shell-command-on-region)
            ;; Doom's leader key without evil: C-c.
            ("C-c a" act-at-point) ("C-c f f" find-file)
            ("C-c s s" search-lines) ("C-c s b" search-lines) ("C-c s B" search-all-buffers)))

;; Help, under C-h as in Emacs and SPC h as in Doom.
(define help-keys
  '(("f" describe-function) ("v" describe-variable) ("o" describe-symbol) ("x" describe-command)
    ("k" describe-key) ("m" describe-mode) ("b" describe-bindings) ("w" where-is) ("a" apropos)
    ("e" view-echo-area-messages) ("r" view-manual) ("." describe-at-point))
  "The help commands, by the key that follows the help prefix.")

(for-each (lambda (b)
            (define-key! emacs-map (string-append "C-h " (car b)) (cadr b))
            (define-key! modal-map (string-append "SPC h " (car b)) (cadr b)))
          help-keys)

;; The modal profile's leader key, as in Doom.
(for-each (lambda (b) (define-key! modal-map (car b) (cadr b)))
          '(("SPC :" execute-extended-command) ("SPC f f" find-file) ("SPC ." find-file)
            ("SPC b b" switch-to-buffer) ("SPC ," switch-to-buffer) ("SPC b k" kill-buffer)
            ("SPC a" act-at-point) ("SPC s s" search-lines) ("SPC s b" search-lines) ("SPC s B" search-all-buffers)
            ("SPC w s" split-window-below) ("SPC w v" split-window-right) ("SPC w w" other-window) ("SPC w d" delete-window)
            ;; evil's paging and z keys.
            ("C-f" scroll-up-command) ("C-b" scroll-down-command) ("<next>" scroll-up-command) ("<prior>" scroll-down-command)
            ("C-d" scroll-half-down) ("C-u" scroll-half-up)
            ("z z" recenter-middle) ("z t" recenter-top) ("z b" recenter-bottom)
            ("SPC c e" eval-buffer-or-region) ("SPC c d" find-definition) ("SPC c k" inspect-at-point)))

(for-each (lambda (b) (define-key! emacs-map (car b) (cadr b)))
          '(("C-x #" server-edit) ("C-x 2" split-window-below) ("C-x 3" split-window-right) ("C-x o" other-window) ("C-x 0" delete-window) ("C-x 1" delete-other-windows)
            ;; Global in Arthur's Emacs (eros).
            ("C-x C-e" eval-last-sexp)
            ("M-." find-definition) ("M-," pop-definition)
            ;; Doom's code prefix, C-c c.
            ("C-c c e" eval-buffer-or-region) ("C-c c d" find-definition) ("C-c c k" inspect-at-point)))

;; Prefix names, as which-key shows them (Doom's for its leader keys).
(for-each (lambda (n) (name-prefix! emacs-map (car n) (cadr n)))
          '(("C-x" "C-x") ("C-c" "leader") ("C-c c" "code") ("C-c f" "file") ("C-c s" "search")
            ("C-h" "help") ("M-s" "search")))
(name-prefix! (mode-map 'scheme-mode) "C-c C-d" "documentation")
(for-each (lambda (n) (name-prefix! modal-map (car n) (cadr n)))
          '(("SPC" "leader") ("SPC b" "buffer") ("SPC c" "code") ("SPC h" "help") ("z" "scroll") ("SPC f" "file") ("SPC s" "search") ("SPC w" "window")))
