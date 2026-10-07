;;; The editor application: a session over a view the host made, commands
;;; for files, panes and the live loop, and what the host asks of Lisp to
;;; present the session (panes, mode lines, the echo area, highlights, the
;;; cursor shape). The runtime (techne-editor) calls the procedures
;;; provided here.

(require "session.scm")
(require "modes.scm")
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
(require "which-key.scm")
(require "options.scm")

(provide start-session editor-press editor-click editor-message! session-quit?
         editor-panes editor-focus pane-status echo-line pane-layers cursor-shape editor-minibuffer
         editor-session-state editor-restore! editor-pane-places editor-key-hints editor-take-request! editor-paged! editor-clipboard! editor-clipboard-out
         bound-keys editor-unsendable! current-session eval-region!)

(define (start-session view profile-name)
  ;; *Messages* is a buffer from the start, as in Emacs.
  (make-generated-buffer! "*Messages*" (messages-document) 'log-mode)
  (add-buffer! (view-document view))
  (set! %session (make-session-for-view view (if (equal? profile-name "modal") modal-profile emacs-profile)))
  (sset! %session 'after-key which-key-after-key)
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
;; Every keymap a key may be looked up in, those of the focused buffer
;; first.
(define (keymaps-to-send s)
  (reverse (fold (lambda (km acc) (if (memq km acc) acc (cons km acc)))
                 '()
                 (append (active-keymaps s 'chord) (active-keymaps s 'normal) (all-keymaps s (list minibuffer-map))))))

;; The key sequences bound in them, for the frontend to check which it can
;; send.
(define (bound-keys s)
  (delete-duplicates (append-map keymap-sequences (keymaps-to-send s))))

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
                                     (let ((b (key-binding (keymaps-to-send s) (kbd k))))
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
  "Run a command by its name, with the prefix argument given to M-x."
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

;;; Paging and recentering are visual: the command asks the frontend,
;;; which knows the screen; it answers with where it scrolled and where
;;; the caret goes (editor-paged!), or that there is nothing further.
;;; As Arthur's Emacs: two lines of context, the caret keeping its place
;;; on the screen, an error at either end.

(define next-screen-context-lines 2)

;; Requests of keys handled before the frontend answers add up: two pages
;; down are one of two screens.
(define (request-view! s request)
  (let ((old (sget s 'view-request)) (id (view-id (pane-view s))))
    (sset! s 'view-request
           (if (and old (= (car old) id) (eq? (cadr old) 'page) (eq? (car request) 'page))
               (list id 'page (+ (caddr old) (cadr request)) (caddr request))
               (cons id request)))))

(define (editor-take-request! s view)
  (let ((r (sget s 'view-request)))
    (and r (= (car r) (view-id view))
         (begin (sset! s 'view-request #f) (cdr r)))))

(define (editor-paged! s view pos)
  (editor-click s view pos (or (sget s 'extend) (eq? (sget s 'mode) 'visual))))

(define-command (scroll-up-command s n)
  "Show the next screen of text; the caret keeps its place on the screen."
  (request-view! s (list 'page (if (< n 0) -1.0 1.0) next-screen-context-lines)))

(define-command (scroll-down-command s n)
  "Show the previous screen of text; the caret keeps its place on the screen."
  (request-view! s (list 'page (if (< n 0) 1.0 -1.0) next-screen-context-lines)))

(define-command (scroll-half-down s n) "Show the next half screen." (request-view! s (list 'page 0.5 0)))
(define-command (scroll-half-up s n) "Show the previous half screen." (request-view! s (list 'page -0.5 0)))

(define-command (recenter-top-bottom s n)
  "Scroll the caret's line to the middle; again, to the top, then the bottom."
  (let ((at (if (eq? (sget s 'last-command) 'recenter-top-bottom)
                (case (sget s 'recentered) ((middle) 'top) ((top) 'bottom) (else 'middle))
                'middle)))
    (sset! s 'recentered at)
    (request-view! s (list 'recenter at))))

(define-command (recenter-middle s n) "Scroll the caret's line to the middle." (request-view! s (list 'recenter 'middle)))
(define-command (recenter-top s n) "Scroll the caret's line to the top." (request-view! s (list 'recenter 'top)))
(define-command (recenter-bottom s n) "Scroll the caret's line to the bottom." (request-view! s (list 'recenter 'bottom)))

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
            ("C-;" act-at-point) ("M-s o" lens-search) ("M-y" yank-pop)
            ("C-v" scroll-up-command) ("<next>" scroll-up-command) ("M-v" scroll-down-command) ("<prior>" scroll-down-command)
            ("C-l" recenter-top-bottom) ("C-h e" view-echo-area-messages) ("M-:" eval-expression)
            ("M-!" shell-command) ("M-&" async-shell-command) ("M-|" shell-command-on-region)
            ;; Doom's leader key without evil: C-c.
            ("C-c a" act-at-point) ("C-c f f" find-file)
            ("C-c s s" search-lines) ("C-c s b" search-lines) ("C-c s B" search-all-buffers)))

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
  (let ((b (document-buffer d)))
    (and (document-path d) (not (and b (buffer-lens b))) (absolute-path (document-path d)))))

(define (editor-session-state s)
  (let* ((kept (filter (lambda (v) (file-of (view-document v))) (session-panes s)))
         (focus (or (list-index (lambda (v) (view=? v (pane-view s))) kept) 0))
         (pane (lambda (v)
                 (let ((r (list-ref (view-ranges v) (view-primary v))))
                   (list (file-of (view-document v)) (car r) (cadr r) (view-scroll v))))))
    (call-with-output-string
     (lambda (p)
       (write `((buffers ,@(filter-map (lambda (b) (file-of (buffer-document b))) (buffer-list))) (panes ,@(map pane kept)) (focus ,focus)
                ;; The tiling, when every pane is kept.
                (tree ,(and (= (length kept) (length (session-panes s))) (session-tree s))))
              p)))))

(define (editor-restore! s text)
  (let* ((state (read (open-input-string text)))
         (field (lambda (k) (cdr (assq k state))))
         (open (lambda (path) (guard (e (#t #f)) (file-document path))))
         (view-at (lambda (d anchor head scroll)
                    (let ((v (make-view d "user")) (len (document-length d)))
                      (guard (e (#t #f)) (view-set-ranges! v (list (list (min anchor len) (min head len))) 0))
                      (guard (e (#t #f)) (view-set-scroll! v (min scroll len)))
                      (set-buffer-view! (add-buffer! d) v)
                      v)))
         (views (filter-map (lambda (p)
                              (let ((d (open (car p))))
                                (and d (view-at d (cadr p) (caddr p) (cadddr p)))))
                            (field 'panes))))
    (for-each (lambda (path) (let ((d (open path))) (when d (add-buffer! d)))) (reverse (field 'buffers)))
    (unless (null? views)
      (set-session-panes! s views (min (car (field 'focus)) (- (length views) 1)))
      (let ((tree (let ((t (assq 'tree state))) (and t (cadr t)))))
        (when (and tree (= (length (tree-leaves tree)) (length views)))
          (set-session-tree! s tree))))))

;;; What the frontend shows: panes, each with its mode line and the
;;; layers' highlights, and the echo area.

;; The views shown, each read-only as its buffer's option says now.
(define (editor-panes s)
  (for-each (lambda (v)
              (let ((b (document-buffer (view-document v))))
                (when b (set-view-read-only! v (option b 'read-only)))))
            (session-panes s))
  (session-panes s))
(define (editor-pane-places s) (pane-places s))
(define (editor-focus s) (session-focus s))

(define (view-point v) (cadr (list-ref (view-ranges v) (view-primary v))))

;; File, modified mark, line, the modes on; in the focused pane, the modal
;; state too.
;; A mode's name as the mode line shows it: without "-mode".
(define (mode-label name)
  (let ((n (symbol->string name)))
    (if (string-suffix? "-mode" n) (substring n 0 (- (string-length n) 5)) n)))

;; File or buffer, modified mark, line, the major mode and the minor modes
;; on; in the focused pane, the modal state too.
(define (pane-status s view)
  (let* ((d (view-document view))
         (b (document-buffer d))
         (focused (view=? view (session-view s)))
         (modes (if b (cons (buffer-mode b) (map (lambda (m) (mode-name (car m))) (buffer-minor-modes b))) '()))
         (parts (list (or (document-path d) (and b (buffer-name b)) "*scratch*")
                      (if (and (document-dirty? d) (not (and b (or (buffer-lens b) (option b 'read-only))))) "[+]" #f)
                      (string-append "L" (number->string (line-number d (view-point view))))
                      (and focused (state-name s))
                      (and (pair? modes) (string-append "(" (string-join (map mode-label modes) " ") ")")))))
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

;; Highlights: the session's layers, and the matches of a search being
;; typed in the focused pane, as Emacs's isearch and lazy-highlight.
(define (pane-layers s view from to)
  (sort (append (buffer-layers (view-document view) from to) (search-highlights s view from to))
        (lambda (a b) (< (car a) (car b)))))

(define (search-highlights s view from to)
  (let ((needle (cond ((sget s 'isearch) (cadr (sget s 'isearch)))
                      ((eq? (sget s 'mode) 'search) (sget s 'search-input))
                      (else #f))))
    (if (and needle (not (string=? needle "")) (view=? view (pane-view s)))
        (let ((current (sget s 'isearch-match)))
          (map (lambda (m) (list (car m) (cadr m) (if (equal? m current) 'isearch 'lazy-highlight)))
               (search-all (view-document view) needle from to)))
        '())))

(define (cursor-shape s view)
  (if (and (view=? view (session-view s)) (memq (sget s 'mode) '(normal visual))) 'block 'bar))

;;; Panes.

(define-command (split-window-below s n)
  "Split the focused pane in two, one above the other, both showing its
buffer; the focus stays in the upper one."
  (split-pane! s 'below (view-split (pane-view s))))

(define-command (split-window-right s n)
  "Split the focused pane in two, side by side, both showing its buffer;
the focus stays in the left one."
  (split-pane! s 'right (view-split (pane-view s))))

(define-command (other-window s n)
  "Focus the next pane."
  (sset! s 'focus (modulo (+ (session-focus s) n) (length (session-panes s)))))

(define-command (delete-window s n)
  "Close the focused pane; its space goes to its neighbour, which gets the
focus."
  (let ((panes (session-panes s)) (i (session-focus s)))
    (if (= (length panes) 1)
        (message! s "Attempt to delete the sole window")
        (delete-pane! s i))))

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
          '(("C-x 2" split-window-below) ("C-x 3" split-window-right) ("C-x o" other-window) ("C-x 0" delete-window) ("C-x 1" delete-other-windows)
            ;; Global in Arthur's Emacs (eros).
            ("C-x C-e" eval-last-sexp)
            ("M-." find-definition) ("M-," pop-definition) ("C-h ." describe-at-point)
            ;; Doom's code prefix, C-c c.
            ("C-c c e" eval-buffer-or-region) ("C-c c d" find-definition) ("C-c c k" inspect-at-point)))

;; Scheme buffers, with Geiser's keys.
(define-mode scheme-mode
  "Techne Lisp and Scheme: code evaluates in its file's module."
  #:parent 'prog-mode
  #:files '(".scm" ".sld" ".sls" ".ss")
  #:keys '(("C-M-x" eval-defun) ("C-c C-k" eval-buffer)
           ;; Geiser's documentation at point.
           ("C-c C-d C-d" inspect-at-point) ("C-c C-d d" inspect-at-point)
           ;; As CIDER's inspector.
           ("C-c M-i" inspect-last-result)))

;; Prefix names, as which-key shows them (Doom's for its leader keys).
(for-each (lambda (n) (name-prefix! emacs-map (car n) (cadr n)))
          '(("C-x" "C-x") ("C-c" "leader") ("C-c c" "code") ("C-c f" "file") ("C-c s" "search")
            ("C-h" "help") ("M-s" "search")))
(name-prefix! (mode-map 'scheme-mode) "C-c C-d" "documentation")
(for-each (lambda (n) (name-prefix! modal-map (car n) (cadr n)))
          '(("SPC" "leader") ("SPC b" "buffer") ("SPC c" "code") ("SPC f" "file") ("SPC s" "search") ("SPC w" "window")))
