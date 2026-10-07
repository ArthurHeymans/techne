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

(provide make-buffer buffer? buffer-document buffer-name set-buffer-name! buffer-mode set-buffer-mode!
         buffer-state set-buffer-state! buffer-view set-buffer-view!
         buffer-list document-buffer remember-buffer! forget-buffer! current-buffer session-buffer
         define-mode register-mode! define-minor-mode register-minor-mode!
         find-mode mode-names mode? mode-name mode-doc mode-parent mode-minor? mode-map
         mode-chain derived-mode? mode-for-file mode-on? toggle-mode! buffer-minor-modes
         define-option register-option! find-option option-names option-name option-default option-doc option-type
         set-option! unset-option! option explain-option
         active-keymaps keymap-sources explain-key key-binding all-keymaps buffer-layers buffer-target-at buffer-completion)

;;; Buffers

(define-record-type buffer
  (%make-buffer document name mode state view settings)
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
  (settings buffer-settings set-buffer-settings!))

(define (make-buffer document name mode #:state [state #f])
  "Return a new buffer of DOCUMENT called NAME, in the major mode MODE.
LENS is the lens whose document it is, if any; STATE is what its mode
keeps in it."
  (%make-buffer document name mode state #f '()))

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
  (%make-mode name doc parent minor keys normal layer target-at complete options files)
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
  ;; Option settings the mode makes: an alist of (name . value).
  (options mode-options)
  ;; File name suffixes a major mode is for.
  (files mode-files))

(define %modes (make-registry 'modes))

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

;; A keymap with BINDINGS, (key-description command) lists. A mode defined
;; again keeps its keymaps, so keys bound in them since stay.
(define (mode-keymap old bindings)
  (let ((km (or old (make-keymap))))
    (for-each (lambda (b) (define-key! km (car b) (cadr b))) bindings)
    km))

;; The mode's procedures run in the scope defining it, as commands do.
(define (add-mode! name doc parent minor keys normal layer target-at complete options files)
  (let ((old (find-mode name)) (owned (lambda (p) (and p (scope-procedure p)))))
    (registry-add! %modes name
                   (%make-mode name doc parent minor
                               (mode-keymap (and old (mode-keys old)) keys)
                               (mode-keymap (and old (mode-normal old)) normal)
                               (owned layer) (owned target-at) (owned complete) options files))))

(define (register-mode! name doc #:parent [parent 'fundamental-mode] #:files [files '()]
                        #:keys [keys '()] #:normal [normal '()] #:layer [layer #f] #:target-at [target-at #f]
                        #:complete [complete #f] #:options [options '()])
  "Define the major mode NAME, documented by DOC, extending PARENT.
It has PARENT's keys, layers and options, and its own: KEYS and NORMAL
are (key-description command) bindings for chords and for modal normal
state, LAYER a procedure (document from to) giving highlights (from to
face), TARGET-AT a procedure (buffer position) giving the target there,
COMPLETE a procedure (buffer position) giving what completes the text
before it, (start . candidates), OPTIONS an alist of (option . value)
set in the mode, FILES the file name suffixes it is for. It defines the
command NAME, which gives the focused buffer this mode."
  (add-mode! name doc (and (not (eq? name 'fundamental-mode)) parent) #f keys normal layer target-at complete options files)
  (register-command! name doc (lambda (s n)
                                (set-buffer-mode! (or (current-buffer s) (error "No buffer")) name)
                                (message! s (symbol->string name))))
  name)

(define-syntax define-mode
  (syntax-rules ()
    "Define the major mode NAME, documented by DOC.
The other arguments are keywords, as `register-mode!` takes them."
    ((_ name doc arg ...) (register-mode! 'name doc arg ...))))

(define (register-minor-mode! name doc #:keys [keys '()] #:normal [normal '()] #:layer [layer #f])
  "Define the minor mode NAME, documented by DOC.
It is on where the boolean option NAME is. KEYS and NORMAL are
(key-description command) bindings for chords and for modal normal
state, LAYER a procedure (document from to) giving highlights (from to
face). It defines the command NAME, which turns the mode on and off in
the focused buffer."
  (add-mode! name doc #f #t keys normal layer #f #f '() '())
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
  (%make-option name default doc type)
  option-declaration?
  (name option-name)
  (default option-default)
  (doc option-doc)
  (type option-type))

(define %options (make-registry 'options))

;; Settings made with set-option!, by (name mode) or (name): the value and
;; who made it, each owned by the scope it was made in.
(define %settings (make-registry 'settings))

(define (find-option name)
  "Return the declaration of the option NAME, or #f."
  (registry-ref %options name))
(define (option-names)
  "Return the names of the options declared."
  (registry-keys %options))

(define (register-option! name default doc #:type [type 'any])
  "Declare the option NAME, its DEFAULT and its documentation DOC.
TYPE describes its values: boolean, string, natural, symbol, any, (one-of
value ...) or (or type ...)."
  (registry-add! %options name (%make-option name default doc type))
  name)

(define-syntax define-option
  (syntax-rules ()
    "Declare the option NAME, its DEFAULT and documentation DOC.
The other arguments are keywords, as `register-option!` takes them."
    ((_ name default doc arg ...) (register-option! 'name default doc arg ...))))

;; Whether V is a value of TYPE.
(define (of-type? v type)
  (cond ((eq? type 'any) #t)
        ((eq? type 'boolean) (boolean? v))
        ((eq? type 'string) (string? v))
        ((eq? type 'natural) (and (exact-integer? v) (>= v 0)))
        ((eq? type 'symbol) (symbol? v))
        ((and (pair? type) (eq? (car type) 'one-of)) (and (member v (cdr type)) #t))
        ((and (pair? type) (eq? (car type) 'or)) (any (lambda (t) (of-type? v t)) (cdr type)))
        (else (error "Not a type" type))))

(define (check-option name value)
  (let ((o (find-option name)))
    (when (and o (not (of-type? value (option-type o))))
      (error (string-append "Not a value of " (symbol->string name) ":") value (option-type o)))))

(define (set-option! name value #:buffer [b #f] #:mode [mode #f])
  "Set the option NAME to VALUE in buffer B or the major mode MODE.
Without either, it is set globally."
  (check-option name value)
  (if b
      (set-buffer-settings! b (cons (cons name value) (remove (lambda (e) (eq? (car e) name)) (buffer-settings b))))
      (registry-add! %settings (if mode (list name mode) (list name)) (cons value (scope-name (current-scope)))))
  value)

(define (unset-option! name #:buffer [b #f] #:mode [mode #f])
  "Take back the setting of the option NAME in buffer B or mode MODE.
Without either, the global setting is taken back."
  (if b
      (set-buffer-settings! b (remove (lambda (e) (eq? (car e) name)) (buffer-settings b)))
      (registry-remove! %settings (if mode (list name mode) (list name)))))

(define (explain-option b name)
  "Return the settings of the option NAME that apply in buffer B.
B #f means no buffer. Each is (cell value by), the one that wins first:
CELL is `buffer`, a mode's name, `global` or `default`; BY is who made
it (a package, `root` for the editor's own code and what is evaluated
in it, a mode for its own options)."
  (let* ((settings (lambda (cell key)
                     (map (lambda (e) (list cell (car (car e)) (cdr (car e)))) (registry-entries %settings key))))
         (in-buffer (and b (assq name (buffer-settings b))))
         (in-modes (if b
                       (append-map (lambda (m)
                                     (let ((own (assq name (mode-options m))))
                                       (append (settings (mode-name m) (list name (mode-name m)))
                                               (if own (list (list (mode-name m) (cdr own) (mode-name m))) '()))))
                                   (mode-chain (buffer-mode b)))
                       '()))
         (o (find-option name)))
    (append (if in-buffer (list (list 'buffer (cdr in-buffer) 'buffer)) '())
            in-modes
            (settings 'global (list name))
            (list (list 'default (and o (option-default o)) 'default)))))

;; The value of option NAME in buffer B (#f: what applies to no buffer).
(define (option b name)
  "Return the value of the option NAME in buffer B, #f meaning no buffer."
  (cadr (car (explain-option b name))))

;;; Minor modes

(define (buffer-minor-modes b)
  "Return the minor modes on in buffer B, as (mode . local?), by name.
LOCAL? is true when the buffer or a mode turned it on, not a global
setting."
  (filter-map (lambda (name)
                (let ((m (find-mode name)))
                  (and (mode-minor? m)
                       (let ((winner (car (explain-option b name))))
                         (and (cadr winner) (cons m (not (memq (car winner) '(global default)))))))))
              (sort (mode-names) (lambda (a b) (string<? (symbol->string a) (symbol->string b))))))

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
;; key; else an overlay's (the completion popup's) over the focused
;; buffer's modes' (see `buffer-modes`), then the profile's own. A source
;; is `minibuffer`, `overlay`, a mode's name or the profile's.
(define (keymap-sources s state)
  "Return the keymaps keys are looked up in, in session S and STATE.
STATE is `chord` or `normal`; see the comment above for the order."
  "Return the keymaps keys are looked up in, in session S and STATE.
STATE is `chord` or `normal`; see the comment above for the order."
  (or (let ((t (sget s 'transient-map))) (and t (list (cons 'minibuffer t))))
      (append (let ((o (sget s 'overlay-map))) (if o (list (cons 'overlay o)) '()))
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
They come from the layers of the modes of DOC's buffer."
  (sort (append-map (lambda (m) (let ((layer (mode-layer m))) (if layer (layer doc from to) '())))
                    (buffer-modes (document-buffer doc)))
        (lambda (a b) (< (car a) (car b)))))

(define (buffer-target-at b pos)
  "Return the target at POS in buffer B, or #f.
The nearest mode of B with a way to find one says."
  (let ((m (find (lambda (m) (mode-target-at m)) (mode-chain (buffer-mode b)))))
    (and m ((mode-target-at m) b pos))))

(define (buffer-completion b pos)
  "Return what completes the text before POS in buffer B, or #f.
The nearest mode of B that completes says: (start . candidates)."
  (let ((m (find (lambda (m) (mode-complete m)) (mode-chain (buffer-mode b)))))
    (and m ((mode-complete m) b pos))))

;;; Display options: the frontend draws them beside the text.

(define-option line-numbers #f
  "Line numbers beside the text: absolute, relative, or none.
Relative numbers count from the caret's line, which shows its own."
  #:type '(one-of #f absolute relative))

(define-option eob-marker #f
  "What is drawn on the lines past the end of the text, or nothing.
Vim draws a ~."
  #:type '(or (one-of #f) string))

;;; The modes others build on, as Emacs has them. Until a language has a
;;; mode of its own, its files get prog-mode. Line numbers and ~ are where
;;; Arthur's Doom shows them: numbers in programming, prose and
;;; configuration buffers, ~ in the first two.

(define-mode fundamental-mode "The mode every other extends.")
(define-mode text-mode
  "Prose."
  #:files '(".txt" ".md" ".org" ".yaml" ".yml")
  #:options '((line-numbers . absolute) (eob-marker . "~")))
(define-mode conf-mode "Configuration files." #:files '(".conf" ".cfg" ".ini" ".toml") #:options '((line-numbers . absolute)))
(define-mode prog-mode
  "Programming languages."
  #:options '((line-numbers . absolute) (eob-marker . "~"))
  #:files '(".rs" ".c" ".h" ".cc" ".cpp" ".hpp" ".py" ".sh" ".bash" ".nix" ".el" ".js" ".ts" ".json" ".go"
            ".java" ".lua" ".zig" ".hs" ".ml" ".rb" ".pl"))
(define-option read-only #f "Whether the buffer's text can be edited from its views." #:type 'boolean)

(define-mode special-mode "Buffers the editor writes, not you." #:options '((read-only . #t)))
(define-mode log-mode "Output as it comes: *Messages*, a shell command's." #:parent 'special-mode)
