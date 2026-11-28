;;; Buffers, their modes and options (EDITOR.md, section 1, "Buffers,
;;; modes and options").
;;;
;;; A buffer is what can be switched to: a document, or a presentation of
;;; rows (a view, a lens, a REPL; techne-editor's presentation), with a name, a major mode, the mode's own state (a REPL, an
;;; inspector's stack) and the view it was last shown in. The buffer list
;;; holds them, most recently shown first; a document's buffer is found by
;;; the document's identity, so forgetting a buffer forgets all of it.
;;;
;;; A mode is a named bundle, owned by the package that defines it: keymaps
;;; (one for chords, the Emacs profile and modal insert state, one for
;;; modal normal and visual states), a layer, how to find the target at a
;;; position, and option settings. A major mode has a parent whose bundle
;;; it extends and the file names it is for; a buffer has one. A minor mode
;;; has no parent; it is on where the boolean option of its name is.
;;;
;;; An option is declared, with a default, a type and documentation, and
;;; set in cells: a buffer, a mode, or globally. The more specific cell
;;; wins: the buffer, then its major mode and that mode's parents, nearest
;;; first, then global, then the default. In a mode's cell, a setting made
;;; with set-option! beats the mode's own.
;;;
;;; Keys resolve here, once, for dispatch, which-key and M-x: the more
;;; specific keymap first (EDITOR.md, section 4).

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "operations.scm")
(require "project-settings-bridge.scm")
(require "../schema.scm")

(provide make-buffer buffer? buffer-document buffer-name set-buffer-name! buffer-mode set-buffer-mode!
         buffer-state set-buffer-state! buffer-view set-buffer-view!
         buffer-list document-buffer remember-buffer! forget-buffer! current-buffer session-buffer
         define-mode register-mode! define-minor-mode register-minor-mode!
         find-mode mode-names mode? mode-name mode-doc mode-parent mode-minor? mode-map
         mode-chain derived-mode? mode-for-file mode-on? toggle-mode! buffer-minor-modes
         register-mode-line-segment! mode-line-segments
         define-option register-option! advertise-package!
         find-option option-names option-name option-default option-doc option-type
         option-user-only? option-directory-safe option-merge
         set-option! unset-option! option option-json-overlay explain-option
         add-option-changed-hook! remove-option-changed-hook! project-settings-refresh-display!
         set-document-logical-path-hook! set-remote-path-hook! set-execution-site-inspector-hook!
         active-keymaps keymap-sources explain-key key-binding all-keymaps buffer-layers buffer-inlays buffer-projections buffer-folds buffer-target-at buffer-completion buffer-indent
         buffer-directory buffer-input buffer-display-options)

;;; Buffers

(define-record-type buffer
  (%make-buffer document name mode state view settings display-cache)
  buffer?
  ;; A document, or a presentation.
  (document buffer-document)
  (name buffer-name set-buffer-name!)
  ;; A mode's name: modes are found when used, so redefining one changes
  ;; the buffers that have it.
  (mode buffer-mode set-buffer-mode!)
  (state buffer-state set-buffer-state!)
  (view buffer-view set-buffer-view!)
  ;; Options set in this buffer: an alist of (name . value).
  (settings buffer-settings %set-buffer-settings!)
  (display-cache buffer-display-cache set-buffer-display-cache!))

(define (set-buffer-settings! b settings)
  (%set-buffer-settings! b settings)
  (set-buffer-display-cache! b #f))

(define (make-buffer document name mode #:state [state #f])
  "Return a new buffer of DOCUMENT called NAME, in the major mode MODE.
LENS is the lens whose document it is, if any; STATE is what its mode
keeps in it."
  (%make-buffer document name mode state #f '() #f))

(define %buffers '())

(define (buffer-list)
  "Return the buffers, the most recently shown first."
  %buffers)

(define (document-buffer d)
  "Return the buffer of the document D, or #f.
Its identity is the document's: the list holds the document, so the
address is not reused while it is there."
  (find (lambda (b) (document=? (buffer-document b) d)) %buffers))

(define (remember-buffer! b)
  "Put the buffer B first in the buffer list."
  (set! %buffers (cons b (remove (lambda (x) (eq? x b)) %buffers))))

(define (forget-buffer! b)
  "Take the buffer B off the buffer list."
  (set! %buffers (remove (lambda (x) (eq? x b)) %buffers)))

(define (current-buffer s)
  "Return the buffer of the focused pane of S, whatever has the keys."
  (document-buffer (view-document (pane-view s))))

(define (session-buffer s)
  "Return the buffer the commands of S act on.
That is the focused pane's, or #f while they edit the minibuffer's
input."
  (document-buffer (session-document s)))

;;; Modes

(define-record-type mode
  (%make-mode name doc parent minor keys normal layer target-at complete indent directory options files)
  mode?
  (name mode-name)
  (doc mode-doc)
  ;; A major mode's parent's name, or #f.
  (parent mode-parent)
  (minor mode-minor?)
  ;; Keymaps: for chords, and for modal normal state.
  (keys mode-keys)
  (normal mode-normal)
  ;; A procedure (document from to) -> list of (from to face), or #f.
  (layer mode-layer)
  ;; A procedure (buffer position) -> target or #f, or #f.
  (target-at mode-target-at)
  ;; What completes the text before a position: a procedure (buffer
  ;; position) -> (start . candidates) or #f, or #f.
  (complete mode-complete)
  ;; What indents lines: a procedure (document starts) -> edits (from to
  ;; text) indenting the lines at STARTS, or #f.
  (indent mode-indent)
  ;; The directory a buffer is in: a procedure (buffer) -> directory, with
  ;; its slash, or #f.
  (directory mode-directory)
  ;; Option settings the mode makes: an alist of (name . value).
  (options mode-options)
  ;; File name suffixes a major mode is for.
  (files mode-files))

;; The minor modes' names, sorted, found again once modes change: the
;; modes on in a buffer are asked for on every key.
(define %minor-names #f)
;; The six display options are requested together on every snapshot. Registry
;; changes (including scope teardown) invalidate inherited values; buffer-local
;; settings clear the buffer's cache, and the cache key includes its mode.
(define %display-generation 0)
(define (display-changed! key value) (set! %display-generation (+ 1 %display-generation)))
(define %modes (make-registry 'modes #:changed (lambda (key value)
                                                 (set! %minor-names #f)
                                                 (display-changed! key value)
                                                 (notify-option-changed! 'lsp-settings))))

(define (find-mode name)
  "Return the mode NAME, or #f if there is none."
  (registry-ref %modes name))
(define (mode-names)
  "Return the names of the modes defined."
  (registry-keys %modes))

(define (mode-map name #:state [state 'chord])
  "Return the keymap of the mode NAME for STATE, to bind keys in.
STATE is `chord` (the default) or `normal`."
  (let ((m (or (find-mode name) (error "No such mode" name))))
    (if (eq? state 'normal) (mode-normal m) (mode-keys m))))

;; A keymap with BINDINGS, (key-description command) lists; a string in
;; place of the command names the prefix. A mode defined again keeps its
;; keymaps, so keys bound in them since stay.
(define (mode-keymap old bindings)
  (let ((km (or old (make-keymap))) (named? (lambda (b) (string? (cadr b)))))
    (for-each (lambda (b) (define-key! km (car b) (cadr b))) (remove named? bindings))
    (for-each (lambda (b) (name-prefix! km (car b) (cadr b))) (filter named? bindings))
    km))

;; The mode's procedures run in the scope defining it, as commands do.
(define (add-mode! name doc parent minor keys normal layer target-at complete indent directory options files)
  (let ((old (find-mode name)) (owned (lambda (p) (and p (scope-procedure p)))))
    (registry-add! %modes name
                   (%make-mode name doc parent minor
                               (mode-keymap (and old (mode-keys old)) keys)
                               (mode-keymap (and old (mode-normal old)) normal)
                               (owned layer) (owned target-at) (owned complete) (owned indent) (owned directory)
                               options files))))

(define %mode-inputs (make-registry 'mode-inputs))

(define (buffer-input b)
  "Return the direct key handler for buffer B, or #f."
  (and b (any (lambda (m) (registry-ref %mode-inputs (mode-name m))) (mode-chain (buffer-mode b)))))

(define (register-mode! name doc #:parent [parent 'fundamental-mode] #:files [files '()]
                        #:keys [keys '()] #:normal [normal '()] #:layer [layer #f] #:target-at [target-at #f]
                        #:complete [complete #f] #:indent [indent #f] #:directory [directory #f] #:options [options '()]
                        #:projections [projections #f] #:folds [folds #f] #:input [input #f])
  "Define the major mode NAME, documented by DOC, extending PARENT.
It has PARENT's keys, layers and options, and its own: KEYS and NORMAL
are (key-description command) bindings for chords and for modal normal
state, a string in place of the command naming a prefix, LAYER a
procedure (document from to) giving highlights (from to face),
TARGET-AT a procedure (buffer position) giving the target there,
COMPLETE a procedure (buffer position) giving what completes the text
before it, (start . candidates), INDENT a procedure (document starts)
giving the edits (from to text) that indent the lines at STARTS,
DIRECTORY a procedure (buffer) giving the directory the buffer is in,
with its slash, for a buffer of no file (a directory's listing); OPTIONS
an alist of (option . value) set in the mode, FILES the file name
suffixes it is for, PROJECTIONS a procedure (document from to) giving
how the text is laid out (`buffer-projections'), FOLDS a procedure
(view) returning hidden line ranges (from to) (`buffer-folds'). It
defines the command NAME, which gives the focused buffer this mode.
INPUT is a procedure (session key) returning true if it handled the key
before the profile."
  (add-mode! name doc (and (not (eq? name 'fundamental-mode)) parent) #f keys normal layer target-at complete indent directory
             options files)
  (projections! name projections)
  (folds! name folds)
  ;; INPUT gets (session key) and returns true if it consumed the key.
  (registry-add! %mode-inputs name (and input (scope-procedure input)))
  (register-command! name doc (lambda (s n)
                                (set-buffer-mode! (or (current-buffer s) (error "No buffer")) name)
                                (message! s (symbol->string name))))
  name)

(define-syntax define-mode
  (syntax-rules ()
    "Define the major mode NAME, documented by DOC.
The other arguments are keywords, as `register-mode!` takes them."
    ((_ name doc arg ...) (register-mode! 'name doc arg ...))))

(define (register-minor-mode! name doc #:keys [keys '()] #:normal [normal '()] #:layer [layer #f] #:inlays [inlays #f]
                              #:projections [projections #f] #:folds [folds #f])
  "Define the minor mode NAME, documented by DOC.
It is on where the boolean option NAME is. KEYS and NORMAL are
(key-description command) bindings for chords and for modal normal
state, LAYER a procedure (document from to) giving highlights (from to
face), INLAYS one (document from to) giving text shown in the line and
not in the document (position text face): a type after a name,
PROJECTIONS one (document from to) giving how the text is laid out
(`buffer-projections'), FOLDS one (view) returning hidden line ranges
(from to) (`buffer-folds'). It defines the command NAME, which turns the
mode on and off in the focused buffer."
  (add-mode! name doc #f #t keys normal layer #f #f #f #f '() '())
  (when inlays (registry-add! %inlays name (scope-procedure inlays)))
  (projections! name projections)
  (folds! name folds)
  (register-option! name #f doc #:type 'boolean)
  (register-command! name doc (lambda (s n) (toggle-mode! s name)))
  name)

(define-syntax define-minor-mode
  (syntax-rules ()
    "Define the minor mode NAME, documented by DOC.
The other arguments are keywords, as `register-minor-mode!` takes them."
    ((_ name doc arg ...) (register-minor-mode! 'name doc arg ...))))

(define (mode-chain name)
  "Return the mode NAME and its parents, nearest first.
Modes no longer defined end the chain."
  (let loop ((name name) (acc '()))
    (let ((m (and name (not (memq name (map mode-name acc))) (find-mode name))))
      (if m (loop (mode-parent m) (cons m acc)) (reverse acc)))))

(define (derived-mode? name ancestor)
  "Return #t if the mode NAME is ANCESTOR or extends it."
  (and (memq ancestor (map mode-name (mode-chain name))) #t))

(define (mode-for-file path)
  "Return the major mode for a file at PATH.
That is the one with the longest suffix of PATH, else
`fundamental-mode`."
  (let ((best (fold (lambda (name best)
                      (let ((m (find-mode name)))
                        (fold (lambda (suffix best)
                                (if (and (string-suffix? suffix path) (or (not best) (> (string-length suffix) (cdr best))))
                                    (cons name (string-length suffix))
                                    best))
                              best (if (mode-minor? m) '() (mode-files m)))))
                    #f (mode-names))))
    (if best (car best) 'fundamental-mode)))

;;; Options

(define-record-type option-declaration
  (%make-option name default doc type user-only safe merge)
  option-declaration?
  (name option-name)
  (default option-default)
  (doc option-doc)
  (type option-type)
  (user-only option-user-only?)
  (safe option-directory-safe)
  (merge option-merge))

(define %option-changed-hooks (make-registry 'option-changed-hooks))

(define (add-option-changed-hook! name proc)
  "Register PROC under NAME to run when an option changes.
PROC is called as `(PROC option-name)` whenever an option is set, unset,
or its defining scope shuts down."
  (registry-add! %option-changed-hooks name (scope-procedure proc))
  name)

(define (remove-option-changed-hook! name)
  "Remove the option-changed hook registered under NAME."
  (registry-remove! %option-changed-hooks name))

(define (notify-option-changed! opt-name)
  (for-each (lambda (k)
              (let ((p (registry-ref %option-changed-hooks k)))
                (when p (guard (e (#t #f)) (p opt-name)))))
            (registry-keys %option-changed-hooks)))

(define (project-settings-refresh-display!)
  "Invalidate cached buffer display options after settings changes."
  (set! %display-generation (+ 1 %display-generation)))

(define (settings-registry-changed! key value)
  (display-changed! key value)
  (when (pair? key) (notify-option-changed! (car key))))

(define %options (make-registry 'options #:changed display-changed!))

;; Settings made with set-option!, by (name mode) or (name): the value and
;; who made it, each owned by the scope it was made in.
(define %settings (make-registry 'settings #:changed settings-registry-changed!))

;; User per-directory settings made with set-option! #:directory, keyed by
;; (name dir mode-or-#f).
(define %has-dir-settings? #f)
(define (dir-settings-changed! key value)
  (set! %has-dir-settings? (pair? (registry-keys %dir-settings)))
  (settings-registry-changed! key value))
(define %dir-settings (make-registry 'dir-settings #:changed dir-settings-changed!))

(define %document-logical-path-hook (lambda (d) (document-path d)))
(define %remote-path-hook (lambda (s) #f))
(define %execution-site-inspector-hook (lambda (x) #f))

(define (set-document-logical-path-hook! proc)
  "Register PROC mapping a document to its logical path."
  (unless (procedure? proc)
    (error "set-document-logical-path-hook!: expected procedure" proc))
  (set! %document-logical-path-hook proc))

(define (set-remote-path-hook! proc)
  "Register PROC testing whether a path string is remote."
  (unless (procedure? proc)
    (error "set-remote-path-hook!: expected procedure" proc))
  (set! %remote-path-hook proc))

(define (set-execution-site-inspector-hook! proc)
  "Register PROC inspecting an `execution-site`."
  (unless (procedure? proc)
    (error "set-execution-site-inspector-hook!: expected procedure" proc))
  (set! %execution-site-inspector-hook proc))

(define (local-path-directory path)
  (let ((n (string-length path)))
    (let trim ((end n))
      (if (and (> end 1) (char=? (string-ref path (- end 1)) #\/))
          (trim (- end 1))
          (let find-slash ((i (- end 1)))
            (cond
              ((< i 0) (if (and (= end 1) (char=? (string-ref path 0) #\/)) "/" "."))
              ((char=? (string-ref path i) #\/)
               (let strip ((j i))
                 (cond ((zero? j) "/")
                       ((char=? (string-ref path (- j 1)) #\/) (strip (- j 1)))
                       (else (substring path 0 j)))))
              (else (find-slash (- i 1)))))))))

(define (expand-tilde dir)
  (cond
    ((string=? dir "~") (or (get-environment-variable "HOME") "/"))
    ((string-prefix? "~/" dir)
     (string-append (or (get-environment-variable "HOME") "")
                    (substring dir 1 (string-length dir))))
    (else dir)))

(define (option-path-has-parent-component? p)
  (and (string? p)
       (any (lambda (seg) (string=? seg "..")) (string-split p "/"))))

(define (normalize-option-dir dir)
  ;; Pure, zero-I/O lexical normalization for local directories; returns #f for
  ;; remote paths, non-strings, or paths containing `..` parent components.
  (cond
    ((or (not (string? dir)) (%remote-path-hook dir) (option-path-has-parent-component? dir)) #f)
    ((string=? dir "") "/")
    (else
     (let* ((abs? (string-prefix? "/" dir))
            (parts (string-split dir "/"))
            (stack (let loop ((xs parts) (acc '()))
                     (if (null? xs)
                         (reverse acc)
                         (let ((seg (car xs)))
                           (cond ((or (string=? seg "") (string=? seg "."))
                                  (loop (cdr xs) acc))
                                 (else (loop (cdr xs) (cons seg acc)))))))))
       (if abs?
           (if (null? stack) "/" (string-append "/" (string-join stack "/") "/"))
           (if (null? stack) "/" (string-append (string-join stack "/") "/")))))))

(define (normalize-option-dir-write who dir)
  (unless (string? dir)
    (error (string-append who ": #:directory must be a string") dir))
  (when (%remote-path-hook dir)
    (error (string-append who ": #:directory cannot be a remote path") dir))
  (let ((expanded (expand-tilde dir)))
    (unless (string-prefix? "/" expanded)
      (error (string-append who ": #:directory must be an absolute local path") dir))
    (project-settings-canonical-dir expanded)))

(define (find-option name)
  "Return the declaration of the option NAME, or #f."
  (registry-ref %options name))
(define (option-names)
  "Return the names of the options declared."
  (registry-keys %options))

(define (register-option! name default doc #:type [type 'any] #:user-only [user-only #f]
                          #:directory-safe [directory-safe #f] #:merge [merge 'replace])
  "Declare the option NAME, its DEFAULT and its documentation DOC.
TYPE is the schema of its values, as `valid?` takes it: boolean, natural,
(one-of value ...), (or schema ...) and so on. When USER-ONLY is true,
NAME may only be set globally by the user (not per-buffer, per-mode, from
project settings, or by external agent principals). DIRECTORY-SAFE controls
whether project `.techne.scm` files may set NAME without explicit user
trust: `#f` (the default) requires value-based trust, `#t` allows any
valid value, or a predicate `(VALUE) -> boolean` checks safety per value.
MERGE is `'replace` (the default) or `'json-object` for deep dict
overlays."
  (unless (schema? type) (error "Not a schema:" type))
  (unless (or (boolean? directory-safe) (procedure? directory-safe))
    (error "directory-safe must be a boolean or procedure:" directory-safe))
  (unless (memq merge '(replace json-object))
    (error "merge must be replace or json-object:" merge))
  (when (and user-only directory-safe)
    (error "Option cannot be both user-only and directory-safe:" name))
  (let ((existing (find-option name)))
    (when (and existing (option-user-only? existing) (not user-only))
      (error "Cannot weaken #:user-only option:" name)))
  (registry-add! %options name
                 (%make-option name default doc type (and user-only #t)
                               (if (procedure? directory-safe) (scope-procedure directory-safe) (and directory-safe #t))
                               merge))
  name)

(define-syntax define-option
  (syntax-rules ()
    "Declare the option NAME, its DEFAULT and documentation DOC.
The other arguments are keywords, as `register-option!` takes them."
    ((_ name default doc arg ...) (register-option! 'name default doc arg ...))))

(define (advertise-package! name entry commands options)
  "Advertise the package NAME without loading the file ENTRY; return NAME.
What a package's manifest declares for the editor (doc/packages.md) is
there from now on: OPTIONS, each (name default schema documentation),
are declared, and COMMANDS, each (name documentation), offered, the
first use of one loading ENTRY as the package
(`register-command-package!`). A scope of the package's name owns the
declarations: if one of them fails, none is left."
  (let ((scope (make-scope name #:parent %root-scope)))
    (guard (e (#t (scope-shutdown! scope) (raise e)))
      (with-scope scope
        (for-each (lambda (o) (register-option! (car o) (cadr o) (cadddr o) #:type (caddr o))) options)
        (register-command-package! name entry commands)))
    name))

;; Raise an error unless VALUE is a value of the option NAME.
(define (check-option name value)
  (let ((o (find-option name)))
    (when o
      (let ((checked (if (and (eq? (option-merge o) 'json-object) (dict? value))
                         (json-object-strip-unset value)
                         value)))
        (unless (valid? (option-type o) checked)
          (error (string-append "Not a value of " (symbol->string name) ": expected " (schema-description (option-type o)) ", got")
                 value))))))

(define (check-user-only-write! name b mode dir)
  (let ((o (find-option name)))
    (when (and o (option-user-only? o))
      (when (or b mode dir)
        (error (string-append "Option " (symbol->string name) " is user-only and cannot be set per-buffer, per-mode, or per-directory")))
      (let ((who (current-principal)))
        (unless (equal? who "user")
          (error (string-append "Option " (symbol->string name) " is user-only and cannot be set by principal "
                                (if (string? who) who "non-user"))))))))

(define (set-option! name value #:buffer [b #f] #:mode [mode #f] #:directory [dir #f])
  "Set the option NAME to VALUE in buffer B, directory DIR, or mode MODE.
Without any target, it is set globally. B cannot be combined with MODE or
DIR; DIR and MODE together set a user directory override for that mode."
  (when (and b (or mode dir))
    (error "set-option!: #:buffer cannot be combined with #:mode or #:directory"))
  (check-user-only-write! name b mode dir)
  (check-option name value)
  (note-effect! 'settings)
  (cond
    (b
     (set-buffer-settings! b (cons (cons name value) (remove (lambda (e) (eq? (car e) name)) (buffer-settings b))))
     (notify-option-changed! name))
    (dir
     (let ((nd (normalize-option-dir-write "set-option!" dir)))
       (registry-add! %dir-settings (list name nd mode) (cons value (scope-name (current-scope))))))
    (else
     (registry-add! %settings (if mode (list name mode) (list name)) (cons value (scope-name (current-scope))))))
  value)

(define (unset-option! name #:buffer [b #f] #:mode [mode #f] #:directory [dir #f])
  "Take back the setting of NAME in buffer B, directory DIR, or mode MODE.
Without any target, the global setting is taken back."
  (when (and b (or mode dir))
    (error "unset-option!: #:buffer cannot be combined with #:mode or #:directory"))
  (check-user-only-write! name b mode dir)
  (note-effect! 'settings)
  (cond
    (b
     (set-buffer-settings! b (remove (lambda (e) (eq? (car e) name)) (buffer-settings b)))
     (notify-option-changed! name))
    (dir
     (let ((nd (normalize-option-dir-write "unset-option!" dir)))
       (registry-remove! %dir-settings (list name nd mode))))
    (else
     (registry-remove! %settings (if mode (list name mode) (list name))))))

;; Extract (values buf dir path modes snap) from WHERE without I/O.
(define (resolve-where where)
  (cond
    ((not where) (values #f #f #f '() #f))
    ((buffer? where)
     (let* ((d (buffer-document where))
            (raw-path (and d (document-path d)))
            (modes (map mode-name (mode-chain (buffer-mode where)))))
       (if (and raw-path (%remote-path-hook raw-path))
           (values where #f #f modes #f)
           (let* ((logical (and d (guard (e (#t raw-path)) (%document-logical-path-hook d))))
                  (raw-dir (cond (raw-path (local-path-directory raw-path))
                                 ((buffer-directory where) => values)
                                 (else #f)))
                  (parent-comp? (or (option-path-has-parent-component? raw-path)
                                    (option-path-has-parent-component? raw-dir)
                                    (option-path-has-parent-component? logical)))
                  (dir (and (not parent-comp?) (normalize-option-dir raw-dir)))
                  (snap (and dir (project-settings-cached-snapshot dir)))
                  (eff-dir (if snap (project-settings-snapshot-directory snap) dir)))
             (values where eff-dir (and dir logical) modes snap)))))
    ((settings-context? where)
     (let* ((raw-dir (settings-context-directory where))
            (raw-path (settings-context-path where))
            (modes (settings-context-modes where)))
       (if (or (and raw-dir (%remote-path-hook raw-dir))
               (and raw-path (%remote-path-hook raw-path))
               (option-path-has-parent-component? raw-dir)
               (option-path-has-parent-component? raw-path))
           (values #f #f #f modes #f)
           (let* ((dir (normalize-option-dir raw-dir))
                  (snap (or (and dir (project-settings-cached-snapshot dir))
                            (settings-context-snapshot where)))
                  (eff-dir (if snap (project-settings-snapshot-directory snap) dir)))
             (values #f eff-dir raw-path modes snap)))))
    ((%execution-site-inspector-hook where)
     => (lambda (info)
          (let ((remote? (car info))
                (raw-dir (cdr info)))
            (if (or remote? (option-path-has-parent-component? raw-dir))
                (values #f #f #f '() #f)
                (let* ((dir (normalize-option-dir raw-dir))
                       (snap (and dir (project-settings-cached-snapshot dir)))
                       (eff-dir (if snap (project-settings-snapshot-directory snap) dir)))
                  (values #f eff-dir #f '() snap))))))
    ((string? where)
     (if (or (%remote-path-hook where) (option-path-has-parent-component? where))
         (values #f #f #f '() #f)
         (let* ((dir (normalize-option-dir where))
                (snap (and dir (project-settings-cached-snapshot dir)))
                (eff-dir (if snap (project-settings-snapshot-directory snap) dir)))
           (values #f eff-dir #f '() snap))))
    (else
     (error "option: expected buffer, settings-context, execution-site, directory string, or #f" where))))

(define (matching-user-dir-entries name dir user-only?)
  (if (or user-only? (not dir) (not %has-dir-settings?))
      '()
      (let ((keys (registry-keys %dir-settings)))
        (if (null? keys)
            '()
            (append-map
             (lambda (k)
               (if (and (pair? k) (eq? (car k) name))
                   (let ((entry-dir (cadr k))
                         (entry-mode (caddr k)))
                     (if (or (string=? entry-dir dir) (string-prefix? entry-dir dir))
                         (map (lambda (e)
                                (list entry-dir entry-mode (car (car e)) (cdr (car e))))
                              (reverse (registry-entries %dir-settings k)))
                         '()))
                   '()))
             keys)))))

(define (resolve-dir-layers o name dir path modes snap)
  (let* ((user-only? (and o (option-user-only? o)))
         (user-entries (matching-user-dir-entries name dir user-only?)))
    (if (and (not snap) (null? user-entries))
        '()
        (let-values (((applied pending rejected)
                      (project-settings-resolve-option
                       snap path modes user-entries name
                       #:known? (and o #t)
                       #:user-only? user-only?
                       #:safe (if o (option-directory-safe o) #f)
                       #:valid? (if o (lambda (v) (valid? (option-type o) v)) (lambda (v) #t))
                       #:merge (if o (option-merge o) 'replace))))
          applied))))

(define (format-dir-source entry)
  (let ((dir (list-ref entry 0))
        (kind (list-ref entry 1))
        (label (list-ref entry 2)))
    (cond
      ((eq? kind 'user)
       (string-append (if (symbol? label)
                          (symbol->string label)
                          (call-with-output-string (lambda (p) (display label p))))
                      " (" dir ")"))
      ((string? label) label)
      (else (symbol->string kind)))))

(define (mode-objects-from-names names)
  (filter-map find-mode names))

(define (explain-option where name)
  "Return the settings of the option NAME that apply at WHERE.
WHERE may be a buffer, a `settings-context`, an `execution-site`, a
directory string, or `#f` for global settings only. Performs no I/O. Each
row is `(cell value by)`, the highest-precedence setting first: CELL is
`buffer`, `directory`, a mode's name, `global` or `default`; BY is who
made it (a package, `root`, a mode, or a `.techne.scm` / `.editorconfig`
file path)."
  (let* ((o (find-option name))
         (user-only? (and o (option-user-only? o)))
         (settings (lambda (cell key)
                     (map (lambda (e) (list cell (car (car e)) (cdr (car e))))
                          (registry-entries %settings key)))))
    (if (and (buffer? where) (not %has-dir-settings?) (not (project-settings-loaded?)))
        (let* ((in-buffer (and (not user-only?) (assq name (buffer-settings where))))
               (in-modes (if (not user-only?)
                             (append-map (lambda (m)
                                           (let ((own (assq name (mode-options m))))
                                             (append (settings (mode-name m) (list name (mode-name m)))
                                                     (if own (list (list (mode-name m) (cdr own) (mode-name m))) '()))))
                                         (mode-chain (buffer-mode where)))
                             '())))
          (append (if in-buffer (list (list 'buffer (cdr in-buffer) 'buffer)) '())
                  in-modes
                  (settings 'global (list name))
                  (list (list 'default (and o (option-default o)) 'default))))
        (let-values (((b dir path modes snap) (resolve-where where)))
          (let* ((in-buffer (and (not user-only?) b (assq name (buffer-settings b))))
                 (in-dirs (if user-only?
                              '()
                              (map (lambda (e)
                                     (list 'directory (list-ref e 3) (format-dir-source e)))
                                   (resolve-dir-layers o name dir path modes snap))))
                 (in-modes (if (and (not user-only?) (pair? modes))
                               (append-map (lambda (m)
                                             (let ((own (assq name (mode-options m))))
                                               (append (settings (mode-name m) (list name (mode-name m)))
                                                       (if own (list (list (mode-name m) (cdr own) (mode-name m))) '()))))
                                           (mode-objects-from-names modes))
                               '())))
            (append (if in-buffer (list (list 'buffer (cdr in-buffer) 'buffer)) '())
                    in-dirs
                    in-modes
                    (settings 'global (list name))
                    (list (list 'default (and o (option-default o)) 'default))))))))

(define (buffer-display-options b)
  "Return a vector of the resolved display options of buffer B.
In order: line numbers, end marker, syntax highlighting, word wrapping,
truncation and pointer reporting. With B false, return global values."
  (let ((cached (and b (buffer-display-cache b)))
        (mode (and b (buffer-mode b)))
        (ps-gen (project-settings-generation)))
    (if (and cached
             (= %display-generation (vector-ref cached 0))
             (= ps-gen (vector-ref cached 1))
             (eq? mode (vector-ref cached 2)))
        (vector-copy (vector-ref cached 3))
        (let ((values (vector (option b 'line-numbers) (option b 'eob-marker) (option b 'syntax-highlight)
                              (option b 'word-wrap) (option b 'truncate-lines) (option b 'report-pointer))))
          (when b (set-buffer-display-cache! b (vector %display-generation ps-gen mode (vector-copy values))))
          values))))

(define (collect-json-overlay-layers where name o)
  (let-values (((b dir path modes snap) (resolve-where where)))
    (let* ((user-only? (and o (option-user-only? o)))
           (layers '())
           (push! (lambda (v) (when (dict? v) (set! layers (cons v layers))))))
      ;; Lowest to highest: default -> global -> modes (far to near) -> dirs (far to near) -> buffer
      (when (and o (dict? (option-default o)) (pair? (dict-keys (option-default o))))
        (push! (option-default o)))
      (let ((entry (registry-ref %settings (list name))))
        (when entry (push! (car entry))))
      (unless user-only?
        (for-each (lambda (m)
                    (let ((own (assq name (mode-options m))))
                      (when own (push! (cdr own))))
                    (let ((entry (registry-ref %settings (list name (mode-name m)))))
                      (when entry (push! (car entry)))))
                  (reverse (mode-objects-from-names modes)))
        (for-each (lambda (e) (push! (list-ref e 3)))
                  (reverse (resolve-dir-layers o name dir path modes snap)))
        (let ((local (and b (assq name (buffer-settings b)))))
          (when local (push! (cdr local)))))
      (reverse layers))))

(define (option-json-overlay where name)
  "Return the composed JSON-object overlay for option NAME at WHERE.
Preserves `json-unset` tombstones so callers such as LSP configuration can
overlay the result onto a server base dict via `json-object-overlay`.
Performs no I/O."
  (let* ((o (find-option name))
         (layers (collect-json-overlay-layers where name o)))
    (fold (lambda (layer acc) (json-object-compose-overlays acc layer))
          (dict)
          layers)))

(define (option-global-value o name)
  (let ((entry (registry-ref %settings (list name))))
    (if entry (car entry) (and o (option-default o)))))

(define (option-from-where o where name)
  (let-values (((b dir path modes snap) (resolve-where where)))
    (let ((dirs (resolve-dir-layers o name dir path modes snap)))
      (if (pair? dirs)
          (list-ref (car dirs) 3)
          (let loop ((ms modes))
            (if (null? ms)
                (option-global-value o name)
                (let* ((mname (car ms))
                       (m (find-mode mname)))
                  (if (not m)
                      (loop (cdr ms))
                      (let ((entry (registry-ref %settings (list name mname))))
                        (if entry
                            (car entry)
                            (let ((own (assq name (mode-options m))))
                              (if own (cdr own) (loop (cdr ms))))))))))))))

;; The value of option NAME at WHERE (#f: global only).
(define (option where name)
  "Return the value of the option NAME at WHERE, performing no I/O.
WHERE may be a buffer, a `settings-context`, an `execution-site`, a
directory string, or `#f` for global settings only."
  ;; Same precedence as explain-option, without building provenance rows or
  ;; looking up settings that cannot win. Entries are pairs even for #f.
  (let* ((o (find-option name))
         (user-only? (and o (option-user-only? o))))
    (cond
      ((and o (eq? (option-merge o) 'json-object))
       (json-object-overlay (dict) (option-json-overlay where name)))
      ((or user-only? (not where))
       (option-global-value o name))
      ((buffer? where)
       (let ((local (assq name (buffer-settings where))))
         (if local
             (cdr local)
             (if (and (not %has-dir-settings?) (not (project-settings-loaded?)))
                 (let loop ((mode (buffer-mode where)) (seen '()))
                   (let ((m (and mode (not (memq mode seen)) (find-mode mode))))
                     (if (not m) (option-global-value o name)
                         (let ((entry (registry-ref %settings (list name mode))))
                           (if entry (car entry)
                               (let ((own (assq name (mode-options m))))
                                 (if own (cdr own) (loop (mode-parent m) (cons mode seen)))))))))
                 (option-from-where o where name)))))
      (else (option-from-where o where name)))))

;;; Mode line segments: what a package says of a buffer in its mode
;;; line (a language server's state, its diagnostics' count), after the
;;; modes. Scope-owned, so a package's segment goes with it.

(define %mode-line-segment-names #f)
(define %mode-line-segments
  (make-registry 'mode-line-segments #:changed (lambda (k v) (set! %mode-line-segment-names #f))))

(define (register-mode-line-segment! name proc)
  "Show (PROC buffer) in the mode line of the buffer's panes, as NAME.
PROC returns a string, or #f for nothing. Segments are shown in the
order of their names; one that fails shows nothing."
  (registry-add! %mode-line-segments name (scope-procedure proc))
  name)

(define (mode-line-segments b)
  "Return what the mode line segments say of buffer B, a list of strings."
  (filter-map (lambda (name)
                (let ((text (guard (e (#t #f)) ((registry-ref %mode-line-segments name) b))))
                  (and (string? text) (not (string=? text "")) text)))
              (or %mode-line-segment-names
                  (begin
                    (set! %mode-line-segment-names
                          (sort (registry-keys %mode-line-segments)
                                (lambda (a b) (string<? (symbol->string a) (symbol->string b)))))
                    %mode-line-segment-names))))

;;; Minor modes

(define (buffer-minor-modes b)
  "Return the minor modes on in buffer B, as (mode . local?), by name.
LOCAL? is true when the buffer or a mode turned it on, not a global
setting."
  ;; Sorted after the major modes are left out, few as minor ones are:
  ;; this runs on every key.
  (filter-map (lambda (name)
                (and (option b name)
                     (let ((winner (car (explain-option b name))))
                       (and (cadr winner) (cons (find-mode name) (not (memq (car winner) '(global default))))))))
              (or %minor-names
                  (begin
                    (set! %minor-names
                          (sort (filter (lambda (name) (mode-minor? (find-mode name))) (mode-names))
                                (lambda (a b) (string<? (symbol->string a) (symbol->string b)))))
                    %minor-names))))

(define (mode-on? s name)
  "Return #t if the minor mode NAME is on in the focused buffer of S."
  (let ((m (find-mode name)))
    (and m (mode-minor? m) (option (current-buffer s) name) #t)))

(define (toggle-mode! s name)
  "Turn the minor mode NAME on or off in the focused buffer of S."
  (let ((on (not (mode-on? s name))))
    (set-option! name on #:buffer (or (current-buffer s) (error "No buffer")))
    (message! s (string-append (symbol->string name) (if on " on" " off")))))

;;; Keys

;; The modes whose keymaps apply in buffer B, the more specific first: the
;; minor modes the buffer or a mode turned on, its major mode and that
;; mode's parents, then the minor modes on globally.
(define (buffer-modes b)
  (if b
      (let ((minors (buffer-minor-modes b)))
        (append (map car (filter cdr minors)) (mode-chain (buffer-mode b)) (map car (remove cdr minors))))
      '()))

;; The keymaps keys are looked up in, in input STATE (`chord` or `normal`),
;; the first binding winning, each with where it comes from: a list of
;; (source . keymap). The minibuffer's while it is open, which takes every
;; key; else an overriding one's (an incremental search's, as Emacs's
;; overriding-terminal-local-map), an overlay's (the completion popup's),
;; the focused buffer's modes' (see `buffer-modes`), then the profile's
;; own. A source is `prompt` (a prompt's own keymap, over the
;; minibuffer's), `minibuffer`, the overriding one's name, `overlay`, a
;; mode's name or the profile's.
(define (keymap-sources s state)
  "Return the keymaps keys are looked up in, in session S and STATE.
STATE is `chord` or `normal`; see the comment above for the order."
  "Return the keymaps keys are looked up in, in session S and STATE.
STATE is `chord` or `normal`; see the comment above for the order."
  (or (sget s 'transient-map)
      (append (let ((o (sget s 'overriding-map))) (if o (list o) '()))
              (let ((o (sget s 'overlay-map))) (if o (list (cons 'overlay o)) '()))
              (map (lambda (m) (cons (mode-name m) (if (eq? state 'normal) (mode-normal m) (mode-keys m))))
                   (buffer-modes (current-buffer s)))
              (let* ((p (sget s 'profile)) (km (profile-keymap p state)))
                (if km (list (cons (profile-name p) km)) '())))))

(define (active-keymaps s state)
  "Return the keymaps keys are looked up in, in session S and STATE.
They are in order, the first binding winning; see `keymap-sources`."
  (map cdr (keymap-sources s state)))

(define (explain-key s keys state)
  "Return what binds KEYS in session S and input STATE, the winner first.
KEYS is a list of keys. Each is (source binding): BINDING a command's
name or a keymap, a prefix; SOURCE where it is bound, as
`keymap-sources` names it. The first is what KEYS do; the others are
shadowed by it."
  (filter-map (lambda (src)
                (let ((b (lookup-key (cdr src) keys)))
                  (and b (list (car src) b))))
              (keymap-sources s state)))

(define (key-binding maps keys)
  "Return the binding of KEYS, a list of keys, in MAPS.
The first map binding them wins: a command's name, a keymap (a prefix)
or #f."
  (let loop ((maps maps))
    (cond ((null? maps) #f)
          ((lookup-key (car maps) keys) => (lambda (b) b))
          (else (loop (cdr maps))))))

(define (all-keymaps s transients)
  "Return every keymap a key may be looked up in, in session S.
Those are the profile's, every mode's for each state, and TRANSIENTS
(the minibuffer's)."
  (let ((p (sget s 'profile)))
    (append transients
            (filter (lambda (km) km) (list (profile-keymap p 'chord) (profile-keymap p 'normal)))
            (append-map (lambda (name) (let ((m (find-mode name))) (list (mode-keys m) (mode-normal m)))) (mode-names)))))

;;; Layers and targets

(define (buffer-layers doc from to)
  "Return the highlights of DOC from FROM to TO, in order.
They come from the layers of the modes of DOC's buffer, the modes
least specific first (`buffer-modes' the other way), which is their
precedence where they overlap."
  (append-map (lambda (m) (let ((layer (mode-layer m))) (if layer (layer doc from to) '())))
              (reverse (buffer-modes (document-buffer doc)))))

(define %inlays (make-registry 'inlays))

(define (buffer-inlays doc from to)
  "Return the inlays of DOC from FROM to TO: each (position text face).
They come from the minor modes on in DOC's buffer."
  (let ((b (document-buffer doc)))
    (if (not b)
        '()
        (append-map (lambda (m)
                      (let ((inlays (registry-ref %inlays (mode-name (car m)))))
                        (if inlays (inlays doc from to) '())))
                    (buffer-minor-modes b)))))

;; Projections (EDITOR.md, section 3): layout, as layers are looks.
(define %projections (make-registry 'projections))

(define (projections! name projections)
  (when projections (registry-add! %projections name (scope-procedure projections))))

(define (buffer-projections doc from to)
  "Return how the modes of DOC's buffer lay out its text from FROM to TO.
Each is (hang LINE AT), the line starting at LINE wrapping under the
character at AT, its wrapped lines starting where that character does
(a list item's under its text, not its bullet); or (block AT IMAGE),
IMAGE (`image-load') shown as a block of its own above the line AT is
on, fitted to the pane's width. A frontend that cannot draw one
ignores it: a terminal shows no images."
  ;; Asked for every pane on every snapshot: only the few modes that lay
  ;; text out are looked at, not every mode of the buffer.
  (let ((names (registry-keys %projections)) (b (document-buffer doc)))
    (if (or (null? names) (not b))
        '()
        (append-map (lambda (name)
                      (let ((m (find-mode name)))
                        (if (and m (if (mode-minor? m) (option b name) (extends? (buffer-mode b) name)))
                            ((registry-ref %projections name) doc from to)
                            '())))
                    names))))

(define %folds (make-registry 'folds))

(define (folds! name folds)
  (when folds (registry-add! %folds name (scope-procedure folds))))

(define (buffer-folds view)
  "Return the hidden line ranges (from to) folded in VIEW.
Folds are per view so two panes showing one buffer fold independently."
  (let* ((names (registry-keys %folds))
         (b (document-buffer (view-document view))))
    (if (or (null? names) (not b))
        '()
        (append-map (lambda (name)
                      (let ((m (find-mode name)))
                        (if (and m (if (mode-minor? m) (option b name) (extends? (buffer-mode b) name)))
                            ((registry-ref %folds name) view)
                            '())))
                    names))))

;; Whether the major mode NAME is ANCESTOR or extends it, without a list.
(define (extends? name ancestor)
  (let loop ((n name))
    (cond ((not n) #f)
          ((eq? n ancestor) #t)
          (else (let ((m (find-mode n))) (and m (loop (mode-parent m))))))))

(define (buffer-target-at b pos)
  "Return the target at POS in buffer B, or #f.
The nearest mode of B with a way to find one says."
  (let ((m (find (lambda (m) (mode-target-at m)) (mode-chain (buffer-mode b)))))
    (and m ((mode-target-at m) b pos))))

(define (buffer-indent b)
  "Return what indents lines in buffer B, or #f.
The nearest mode of B that indents says: a procedure (document starts)
giving edits (from to text)."
  (let ((m (find (lambda (m) (mode-indent m)) (mode-chain (buffer-mode b)))))
    (and m (mode-indent m))))

(define (buffer-directory b)
  "Return the directory buffer B is in, with its slash, or #f.
The nearest mode of B that knows says; a file's buffer is in its file's
directory, which `default-directory` finds itself."
  (let ((m (find (lambda (m) (mode-directory m)) (mode-chain (buffer-mode b)))))
    (and m ((mode-directory m) b))))

(define (buffer-completion b pos)
  "Return what completes the text before POS in buffer B, or #f.
The nearest mode of B that completes says: (start . candidates)."
  (let ((m (find (lambda (m) (mode-complete m)) (mode-chain (buffer-mode b)))))
    (and m ((mode-complete m) b pos))))

;;; Display options: the frontend draws them beside the text.

(define-option line-numbers #f
  "Line numbers beside the text: absolute, relative, or none.
Relative numbers count from the caret's line, which shows its own."
  #:type '(one-of #f absolute relative)
  #:directory-safe #t)

(define-option word-wrap #f
  "Whether terminal visual lines wrap at spaces rather than within words.
Oversized words still wrap by grapheme. This changes layout, never source
text, caret positions or logical rows. Rich prose enables it."
  #:type 'boolean
  #:directory-safe #t)

(define-option report-pointer #f
  "Whether the frontend delivers pointer events in text cells."
  #:type 'boolean
  #:directory-safe #t)

(define-option truncate-lines #f
  "Whether long lines are clipped instead of wrapped."
  #:type 'boolean
  #:directory-safe #t)

(define-option eob-marker #f
  "What is drawn on the lines past the end of the text, or nothing.
Vim draws a ~."
  #:type '(or (one-of #f) string)
  #:directory-safe #t)

(define-option syntax-highlight #t
  "Whether the text is coloured by its language's syntax.
The runtime's tree-sitter grammars name faces under the buffer's own
layers; which language is the buffer's is `syntax-language'."
  #:type 'boolean
  #:directory-safe #t)

(define-option syntax-language #f
  "The language of the buffer's syntax: rust, scheme, markdown, ...
#f asks the file's name, as the languages' file suffixes say
(`define-language'). A mode for one language sets this, as
scheme-mode does, so buffers without a file are coloured too."
  #:type '(or (one-of #f) symbol)
  #:directory-safe #t)

;;; Formatting options: safe for `.editorconfig` and `.techne.scm`.

(define-option indent-width 4
  "Width in columns of one indentation step.
Mapped from EditorConfig `indent_size` and safe in `.techne.scm`."
  #:type '(integer #:min 1 #:max 32)
  #:directory-safe #t)

(define-option tab-width 8
  "Width in columns of a literal tab character.
Mapped from EditorConfig `tab_width` and safe in `.techne.scm`."
  #:type '(integer #:min 1 #:max 32)
  #:directory-safe #t)

(define-option indent-tabs #f
  "Whether indentation uses tab characters instead of spaces.
Mapped from EditorConfig `indent_style` and safe in `.techne.scm`."
  #:type 'boolean
  #:directory-safe #t)

(define-option fill-column 80
  "Preferred line-length limit in columns, or `#f` for none.
Mapped from EditorConfig `max_line_length` and safe in `.techne.scm`."
  #:type '(or (one-of #f) (integer #:min 1 #:max 1000))
  #:directory-safe #t)

(define-option insert-final-newline 'preserve
  "Whether saving ensures a trailing newline: `'preserve`, `#t`, or `#f`.
When `#t`, saving an editable non-empty buffer appends a newline if
missing as an undoable edit. Mapped from EditorConfig
`insert_final_newline`."
  #:type '(one-of preserve #t #f)
  #:directory-safe #t)

(define-option trim-trailing-whitespace #f
  "Whether saving removes trailing spaces and tabs on lines.
When `#t`, saving an editable buffer trims trailing horizontal
whitespace as an undoable edit. Mapped from EditorConfig
`trim_trailing_whitespace`."
  #:type 'boolean
  #:directory-safe #t)

(define-option end-of-line 'preserve
  "Line-ending convention: `'preserve`, `'lf`, `'crlf`, or `'cr`.
Mapped from EditorConfig `end_of_line` and safe in `.techne.scm`."
  #:type '(one-of preserve lf crlf cr)
  #:directory-safe #t)

;;; The modes others build on, as Emacs has them. Until a language has a
;;; mode of its own, its files get prog-mode. Line numbers and ~ are where
;;; Arthur's Doom shows them: numbers in programming, prose and
;;; configuration buffers, ~ in the first two.

(define-mode fundamental-mode "The mode every other extends.")
(define-mode text-mode
  "Prose."
  #:files '(".txt" ".md" ".yaml" ".yml")
  #:options '((line-numbers . absolute) (eob-marker . "~")))
(define-mode conf-mode "Configuration files." #:files '(".conf" ".cfg" ".ini" ".toml") #:options '((line-numbers . absolute)))
(define-mode prog-mode
  "Programming languages.
\\[indent-for-tab-command] indents the line, where the language's mode indents."
  #:keys '(("TAB" indent-for-tab-command))
  #:options '((line-numbers . absolute) (eob-marker . "~"))
  #:files '(".rs" ".c" ".h" ".cc" ".cpp" ".hpp" ".py" ".sh" ".bash" ".nix" ".el" ".js" ".ts" ".json" ".go"
            ".java" ".lua" ".zig" ".hs" ".ml" ".rb" ".pl"))
(define-option read-only #f "Whether the buffer's text can be edited from its views." #:type 'boolean)
(define-option cursor-visible #t
  "Whether the buffer shows a text caret."
  #:type 'boolean
  #:directory-safe #t)
(define-option inherit-on-attach #t
  "Whether a frontend attaching without a file may inherit this buffer."
  #:type 'boolean)

(define-mode special-mode
  "Buffers the editor writes, not you.
\\[quit-window] leaves one."
  #:options '((read-only . #t))
  #:keys '(("q" quit-window))
  #:normal '(("q" quit-window)))
(define-mode log-mode "Output as it comes: *Messages*, a shell command's." #:parent 'special-mode)
