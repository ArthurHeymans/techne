;;; Help, as Emacs's: what a name is (a procedure, a macro, a variable, a
;;; command, an option, a mode, a hook, often several at once), what a key
;;; does and why, which keys run a command, the modes of a buffer and their
;;; keys, every name matching a pattern, the manual. Help is shown in the
;;; *Help* view: every name, key and definition in it is a target, so RET
;;; describes a name or goes to a definition and C-; offers the rest; l
;;; goes back to what was shown before.
;;;
;;; Docstrings write keys as \\[command]; help shows the key that runs the
;;; command in the user's profile instead (`substitute-command-keys`).

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "emacs.scm")
(require "modal.scm")
(require "targets.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "files.scm")
(require "views.scm")
(require "options.scm")

(provide substitute-command-keys command-key-descriptions describe-name! effective-binding name-module
         describe-symbol describe-function describe-variable describe-command describe-key
         describe-mode describe-bindings where-is apropos help-back view-manual manual-file)

;;; Keys

;; The input states keys are looked up in, in order, in the session's
;; profile and state: Emacs chords; in modal insert state chords, else
;; normal state's keymaps and the chords Vim leaves unused.
(define (lookup-states s)
  (cond ((not (eq? (profile-name (sget s 'profile)) 'modal)) '(chord))
        ((eq? (sget s 'mode) 'insert) '(chord))
        (else '(normal chord))))

(define (effective-binding s keys)
  "Return what the key sequence KEYS does in session S.
That is what pressing it would do. KEYS is a list of keys; the result
is a command's name, a keymap (KEYS is a prefix) or #f."
  (if (equal? (lookup-states s) '(chord))
      (key-binding (active-keymaps s 'chord) keys)
      (key-binding (modal-keymaps s keys) keys)))

;; A key sequence is shown as `kbd` reads it; plain chords come before
;; named keys (<f1>), then fewer keys before more, then shorter ones, as
;; Marginalia shows them: C-x C-f before C-c f f.
(define (key<? a b)
  (let ((named? (lambda (k) (string-contains k "<")))
        (keys (lambda (k) (length (kbd k)))))
    (cond ((not (eq? (not (named? a)) (not (named? b)))) (not (named? a)))
          ((not (= (keys a) (keys b))) (< (keys a) (keys b)))
          (else (< (string-length a) (string-length b))))))

(define (command-key-descriptions s name)
  "Return the key sequences that run the command NAME in session S.
Each is a string as `kbd` reads it, the shortest plain one first; a
sequence shadowed by another binding is not one of them."
  (sort (delete-duplicates
         (filter-map (lambda (keys) (and (eq? (effective-binding s keys) name) (string-join keys " ")))
                     (append-map (lambda (state) (append-map (lambda (src) (keymap-sequences-to (cdr src) name)) (keymap-sources s state)))
                                 (lookup-states s))))
        key<?))

;; How the user runs command NAME: its key, else through M-x.
(define (command-key s name)
  (let ((keys (command-key-descriptions s name)))
    (if (pair? keys)
        (car keys)
        (let ((mx (command-key-descriptions s 'execute-extended-command)))
          (string-append (if (pair? mx) (car mx) "M-x") " " (symbol->string name))))))

(define (substitute-command-keys s text)
  "Return TEXT with each `\\[command]` replaced by the key that runs it.
The key is the one of session S's profile and state, or the way to run
the command by name when no key does. Text in backquotes is left as it
is, so that a docstring can show `\\[command]` itself; `\\=` keeps the
character after it as it is, and goes, as in Emacs."
  (let loop ((cs (string->list text)) (quoted #f) (acc '()))
    (cond ((null? cs) (list->string (reverse acc)))
          ((and (not quoted) (char=? (car cs) #\\) (pair? (cdr cs)) (char=? (cadr cs) #\=) (pair? (cddr cs)))
           (loop (cdddr cs) quoted (cons (caddr cs) acc)))
          ((char=? (car cs) #\`) (loop (cdr cs) (not quoted) (cons (car cs) acc)))
          ((and (not quoted) (char=? (car cs) #\\) (pair? (cdr cs)) (char=? (cadr cs) #\[) (memv #\] (cddr cs)))
           (let* ((rest (cddr cs))
                  (after (cdr (memv #\] rest)))
                  (name (list->string (take rest (- (length rest) (length after) 1)))))
             (loop after quoted (append (reverse (string->list (command-key s (string->symbol name)))) acc))))
          (else (loop (cdr cs) quoted (cons (car cs) acc))))))

;;; Names

(define (field alist key) (let ((e (and alist (assq key alist)))) (and e (cdr e))))

;; Names the editor's registries know, by kind.
(define (registered s)
  (list (cons 'command (command-names)) (cons 'option (option-names)) (cons 'mode (mode-names)) (cons 'hook (hook-names))))

(define (name-module s name)
  "Return the module NAME is bound in, as session S sees it, or #f.
That is the focused file's module if it sees NAME, else the first
module loaded that defines it."
  (binding-module name (document-module (doc s))))

;; Every name help knows, by name, but those starting with %: bound in a
;; module loaded, seen by the focused file's module, a special form, or in
;; a registry. Each is (name
;; module kind kinds): the module it is bound in (`name-module`) and its
;; kind there (as `binding-description` says), both #f if only registries
;; know it, and the registries'.
(define (known-names s)
  (let ((registries (make-hash-table)))
    (for-each (lambda (r)
                (for-each (lambda (n) (hash-table-update!/default registries n (lambda (ks) (append ks (list (car r)))) '()))
                          (cdr r)))
              (registered s))
    (let* ((bound (map (lambda (e)
                         (let ((ks (hash-table-ref/default registries (car e) #f)))
                           (when ks (hash-table-set! registries (car e) 'bound))
                           (list (car e) (cadr e) (caddr e) (or ks '()))))
                       (defined-names (document-module (doc s)))))
           (only (sort (filter-map (lambda (p)
                                     (and (pair? (cdr p)) (not (string-prefix? "%" (symbol->string (car p))))
                                          (list (car p) #f #f (cdr p))))
                                   (hash-table->alist registries))
                       name<?)))
      (merge-sorted bound only name<?))))

(define (name<? a b) (string<? (symbol->string (car a)) (symbol->string (car b))))

;; The sorted lists A and B as one sorted list, by LESS?.
(define (merge-sorted a b less?)
  (let loop ((a a) (b b) (acc '()))
    (cond ((null? a) (append (reverse acc) b))
          ((null? b) (append (reverse acc) a))
          ((less? (car b) (car a)) (loop a (cdr b) (cons (car b) acc)))
          (else (loop (cdr a) b (cons (car a) acc))))))

(define (first-line text)
  (let ((i (string-index text #\newline))) (if i (substring text 0 i) text)))

;; What a name is, in a word or two, and the first line of its
;; documentation; E is its entry of `known-names`.
(define (summary s e)
  (let* ((name (car e)) (kinds (cadddr e))
         (doc (or (field (and (cadr e) (binding-description name (cadr e))) 'doc)
                  (and (memq 'command kinds) (command-doc name))
                  (and (memq 'option kinds) (let ((o (find-option name))) (and o (option-doc o))))
                  (and (memq 'mode kinds) (let ((m (find-mode name))) (and m (mode-doc m))))
                  (and (memq 'hook kinds) (hook-doc name)))))
    (string-append (string-join (delete-duplicates (map symbol->string (if (caddr e) (cons (caddr e) kinds) kinds))) ", ")
                   (if (string? doc) (string-append "  " (substitute-command-keys s (first-line doc))) ""))))

;; Read a name with PROMPT among those KEEP? accepts, then describe it.
;; KEEP? is called with the name's kind, #f if it is not bound, and the
;; registries that know it (`known-names`).
(define (read-name s prompt keep?)
  (completing-read s prompt (name-candidates s keep?)
                   #:accept (lambda (s c) (describe-name! s (target-value (candidate-target c))))))

;; The candidates of `read-name`: there are thousands, so they are matched
;; natively, each made, with its summary, when it is first shown.
(define (name-candidates s keep?)
  (let ((names (list->vector (filter (lambda (e) (keep? (caddr e) (cadddr e))) (known-names s)))))
    (candidate-table (make-matcher (map (lambda (e) (symbol->string (car e))) (vector->list names)))
                     (lambda (i)
                       (let ((e (vector-ref names i)))
                         (candidate (symbol->string (car e)) #:annotation (summary s e) #:target (target 'symbol (car e))))))))

(define (function-kind? kind)
  (memq kind (list 'procedure 'macro (string->symbol "built-in procedure") (string->symbol "special form"))))

;;; The *Help* view

(define-mode help-mode
  "Help: what a name is, what a key does, the modes of a buffer.
RET on a name describes it and on a definition goes there; l goes back
to what was shown before."
  #:parent 'rows-mode
  #:keys '(("l" help-back)))

;; Rows of documentation TEXT, keys shown as session S's: one a line, the
;; first labelled LABEL; a line naming something in backquotes stands for
;; the first such name help knows.
(define (doc-rows s text #:label [label "doc"])
  (let loop ((lines (string-split (substitute-command-keys s text) "\n")) (label label) (acc '()))
    (if (null? lines)
        (reverse acc)
        (loop (cdr lines) "" (cons (row label (car lines) #:target (quoted-target s (car lines))) acc)))))

;; The first name in backquotes in LINE that help knows, as a target.
(define (quoted-target s line)
  (let ((parts (string-split line "`")))
    (let loop ((i 1))
      (and (< i (length parts))
           (let ((name (string->symbol (list-ref parts i))))
             (if (or (name-module s name) (any (lambda (r) (memq name (cdr r))) (registered s)))
                 (target 'symbol name)
                 (loop (+ i 2))))))))

(define (location-row where)
  (row "defined" (string-append (car where) ":" (number->string (cadr where)))
       #:target (target 'location (file-location (car where) (cadr where)))))

;; Rows about NAME as a binding of a module.
(define (binding-rows s name)
  (let* ((m (name-module s name)) (d (and m (binding-description name m))))
    (if (not d)
        '()
        (append (list (row (symbol->string (field d 'kind)) (or (field d 'signature) (symbol->string name))))
                (if (field d 'location) (list (location-row (field d 'location))) '())
                (if (field d 'doc) (doc-rows s (field d 'doc)) '())))))

;; Rows about NAME as a command: its keys, and its documentation unless
;; its procedure's was shown.
(define (command-rows s name shown-doc)
  (if (memq name (command-names))
      (let ((keys (command-key-descriptions s name)) (doc (command-doc name)))
        (append (list (row "command" (symbol->string name) #:target (target 'command name))
                      (row "keys" (if (null? keys) (command-key s name) (string-join keys ", "))))
                (if (and (string? doc) (not (equal? doc shown-doc))) (doc-rows s doc) '())))
      '()))

(define (mode-rows s name)
  (let ((m (find-mode name)))
    (if (not m)
        '()
        (append (list (row (if (mode-minor? m) "minor mode" "major mode") (symbol->string name)))
                (if (mode-parent m) (list (row "parent" (symbol->string (mode-parent m)) #:target (target 'symbol (mode-parent m)))) '())
                (doc-rows s (mode-doc m))
                (keymap-rows (mode-map name))
                (keymap-rows (mode-map name #:state 'normal) #:label "normal")))))

;; A row for each key bound in keymap KM, the first labelled LABEL.
(define (keymap-rows km #:label [label "keys"])
  (let loop ((seqs (sort (keymap-sequences km) string<?)) (label label) (acc '()))
    (if (null? seqs)
        (reverse acc)
        (let ((b (lookup-key km (kbd (car seqs)))))
          (loop (cdr seqs) ""
                (cons (row label (string-append (car seqs) "  " (if (symbol? b) (symbol->string b) "")) #:target (and (symbol? b) (target 'symbol b)))
                      acc))))))

(define (hook-rows s name)
  (let ((doc (hook-doc name)))
    (if doc (cons (row "hook" (symbol->string name)) (doc-rows s doc)) '())))

;; Everything help knows about NAME.
(define (name-rows s name)
  (let* ((m (name-module s name))
         (d (and m (binding-description name m)))
         (rows (append (binding-rows s name)
                       (command-rows s name (field d 'doc))
                       (if (find-option name) (option-rows (current-buffer s) name) '())
                       (mode-rows s name)
                       (hook-rows s name))))
    (if (null? rows) (list (row (symbol->string name) "is not defined")) rows)))

;; Show the help page MAKE-ROWS (session -> rows), keeping what was shown
;; before for `help-back`.
(define (show-help! s make-rows)
  (let ((old (buffer-named "*Help*")))
    (show-help-stack! s (cons make-rows (or (and old (eq? (buffer-mode old) 'help-mode) (view-data old)) '())))))

(define (show-help-stack! s stack)
  (show-view! s "*Help*" (car stack) #:mode 'help-mode #:data stack))

(define (describe-name! s name)
  "Show in *Help* everything NAME, a symbol, is in session S."
  (show-help! s (lambda (s) (name-rows s name))))

(define-command (help-back s n)
  "Show the help shown before this."
  (let ((stack (or (view-data (session-buffer s)) '())))
    (if (or (null? stack) (null? (cdr stack)))
        (message! s "Nothing shown before")
        (show-help-stack! s (cdr stack)))))

(define-action symbol (describe-symbol-target s name) "Describe what the name is." (describe-name! s name))
(define-action symbol (find-symbol-definition s name)
  "Go to the name's definition."
  (let* ((m (name-module s name)) (where (and m (field (binding-description name m) 'location))))
    (if where (visit! s (car where) (cadr where) (caddr where)) (error "No definition known for" name))))

;;; Commands

(define-command (describe-symbol s n)
  "Show what a name is: a procedure, variable, command, option, mode..."
  (read-name s "Describe symbol: " (lambda (kind kinds) #t)))

(define-command (describe-function s n)
  "Show the documentation of a procedure, macro or command."
  (read-name s "Describe function: "
             (lambda (kind kinds) (or (memq 'command kinds) (function-kind? kind)))))

(define-command (describe-variable s n)
  "Show the documentation and value of a variable or option."
  (read-name s "Describe variable: "
             (lambda (kind kinds) (or (memq 'option kinds) (eq? kind 'variable)))))

(define-command (describe-command s n)
  "Show the documentation and keys of a command."
  (read-name s "Describe command: " (lambda (kind kinds) (memq 'command kinds))))

(define-command (where-is s n)
  "Say which keys run a command."
  (completing-read s "Where is command: " (map symbol->string (sort (command-names) (lambda (a b) (string<? (symbol->string a) (symbol->string b)))))
                   #:accept (lambda (s c)
                              (let* ((name (string->symbol (candidate-text c))) (keys (command-key-descriptions s name)))
                                (message! s (if (null? keys)
                                                (string-append (candidate-text c) " is not on any key; " (command-key s name))
                                                (string-append (candidate-text c) " is on " (string-join keys ", "))))))))

(define-command (apropos s n)
  "Show every name containing all the words typed, with what it is.
The first `apropos-rows-max` are shown, then how many more there are."
  (completing-read s "Apropos: " '()
                   #:require-match #f
                   #:accept (lambda (s c)
                              (let ((words (string-split (string-downcase (candidate-text c)))))
                                (when (null? words) (error "Apropos needs a word to look for"))
                                (show-help! s (lambda (s) (apropos-rows s words)))))))

;; The most names apropos shows: each row takes some 100,000 instructions,
;; and a view is made all at once.
(define apropos-rows-max 100)

(define (apropos-rows s words)
  (let* ((found (filter (lambda (e) (let ((name (symbol->string (car e)))) (every (lambda (w) (string-contains name w)) words)))
                        (known-names s)))
         (more (- (length found) apropos-rows-max)))
    (append (map (lambda (e) (row (symbol->string (car e)) (summary s e) #:target (target 'symbol (car e))))
                 (if (> more 0) (take found apropos-rows-max) found))
            (if (> more 0) (list (row "..." (string-append (number->string more) " more: type more words to see them"))) '()))))

;; What KEYS do, why, and the command they run.
(define (key-rows s keys)
  (let ((b (effective-binding s keys))
        (seq (string-join keys " ")))
    (append (list (row "key" seq)
                  (row "runs" (cond ((symbol? b) (symbol->string b)) ((keymap? b) "a prefix") (else "nothing"))
                       #:target (and (symbol? b) (target 'symbol b))))
            (let loop ((why (append-map (lambda (state) (explain-key s keys state)) (lookup-states s))) (label "bound in") (acc '()))
              (if (null? why)
                  (reverse acc)
                  (let ((w (car why)))
                    (loop (cdr why) "shadows"
                          (cons (row label (string-append (symbol->string (car w)) ": "
                                                          (if (symbol? (cadr w)) (symbol->string (cadr w)) "a prefix")))
                                acc)))))
            (if (symbol? b) (cons (row "" "") (name-rows s b)) '()))))

(define-command (describe-key s n)
  "Read a key sequence, then show what it runs and where it is bound."
  (sset! s 'describe-keys '())
  (message! s "Describe key: ")
  (sset! s 'transient
         (lambda (s key)
           (let* ((keys (append (sget s 'describe-keys) (list key))) (b (effective-binding s keys)))
             (if (keymap? b)
                 (begin (sset! s 'describe-keys keys)
                        (message! s (string-append "Describe key: " (string-join keys " ") "-")))
                 (begin (sset! s 'transient #f)
                        (show-help! s (lambda (s) (key-rows s keys)))))))))

(define-command (describe-mode s n)
  "Show the modes of this buffer, what they are for and their keys."
  (let ((b (or (current-buffer s) (error "No buffer"))))
    (show-help! s (lambda (s)
                    (append (append-map (lambda (m) (mode-rows s (mode-name m))) (mode-chain (buffer-mode b)))
                            (append-map (lambda (m) (mode-rows s (mode-name (car m)))) (buffer-minor-modes b)))))))

(define-command (describe-bindings s n)
  "Show every key bound here, by where it is bound, the first winning."
  (show-help! s (lambda (s)
                  (append-map (lambda (state)
                                (append-map (lambda (src) (keymap-rows (cdr src) #:label (symbol->string (car src))))
                                            (keymap-sources s state)))
                              (lookup-states s)))))

;;; The manual

(define (manual-file)
  "Return the file of the manual, doc/manual.org beside Techne's Lisp."
  (string-append (directory-of (car (procedure-location manual-file))) "../../doc/manual.org"))

(define-mode manual-mode
  "The manual, with keys shown as your profile binds them."
  #:parent 'special-mode)

(define-command (view-manual s n)
  "Show the manual, keys in it shown as your profile binds them."
  (let ((text (call-with-input-file (manual-file) read-string-all)))
    (show-buffer! s (make-generated-buffer! "*Manual*" (make-document (substitute-command-keys s text)) 'manual-mode))))
