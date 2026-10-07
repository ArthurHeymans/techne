;;; Buffers and their modes (EDITOR.md, section 1, "Buffers, modes and
;;; options").
;;;
;;; A buffer is what can be switched to: a document, or a lens and its
;;; document, with a name, a major mode, the mode's own state (a REPL, an
;;; inspector's stack) and the view it was last shown in. The buffer list
;;; holds them, most recently shown first; a document's buffer is found by
;;; the document's identity, so forgetting a buffer forgets all of it.
;;;
;;; A mode is a named bundle, owned by the package that defines it: keymaps
;;; (one for chords, the Emacs profile and modal insert state, one for
;;; modal normal and visual states), a layer, how to find the target at a
;;; position, and settings (`read-only`). A major mode has a parent whose
;;; bundle it extends and the file names it is for; a buffer has one. A
;;; minor mode has no parent and is turned on and off by itself.
;;;
;;; Keys resolve here, once, for dispatch, which-key and M-x: the more
;;; specific keymap first (EDITOR.md, section 4).

(require "session.scm")

(provide make-buffer buffer? buffer-document buffer-lens buffer-name set-buffer-name! buffer-mode set-buffer-mode!
         buffer-state set-buffer-state! buffer-view set-buffer-view!
         buffer-list document-buffer remember-buffer! forget-buffer! current-buffer session-buffer
         define-mode register-mode! define-minor-mode register-minor-mode!
         find-mode mode-names mode? mode-name mode-doc mode-parent mode-minor? mode-map
         mode-chain derived-mode? mode-setting buffer-setting mode-for-file
         mode-on? toggle-mode! session-modes
         active-keymaps key-binding all-keymaps buffer-layers buffer-target-at)

;;; Buffers

(define-record-type buffer
  (%make-buffer document lens name mode state view)
  buffer?
  (document buffer-document)
  ;; The lens whose document this is, or #f.
  (lens buffer-lens)
  (name buffer-name set-buffer-name!)
  ;; A mode's name: modes are found when used, so redefining one changes
  ;; the buffers that have it.
  (mode buffer-mode set-buffer-mode!)
  (state buffer-state set-buffer-state!)
  (view buffer-view set-buffer-view!))

(define (make-buffer document name mode #:lens [lens #f] #:state [state #f])
  (%make-buffer document lens name mode state #f))

(define %buffers '())

(define (buffer-list) %buffers)

;; The buffer of document D, or #f. Its identity is the document's
;; address; the list holds the document, so the address is not reused
;; while it is there.
(define (document-buffer d)
  (find (lambda (b) (document=? (buffer-document b) d)) %buffers))

;; Put B first in the list.
(define (remember-buffer! b)
  (set! %buffers (cons b (remove (lambda (x) (eq? x b)) %buffers))))

(define (forget-buffer! b)
  (set! %buffers (remove (lambda (x) (eq? x b)) %buffers)))

;; The buffer of the focused pane, whatever has the keys.
(define (current-buffer s) (document-buffer (view-document (pane-view s))))

;; The buffer commands act on: the focused pane's, or #f while they edit
;; the minibuffer's input.
(define (session-buffer s) (document-buffer (session-document s)))

;;; Modes

(define-record-type mode
  (%make-mode name doc parent minor keys normal layer target-at settings files)
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
  ;; An alist of (name . value): `read-only`.
  (settings mode-settings)
  ;; File name suffixes a major mode is for.
  (files mode-files))

(define %modes (make-registry 'modes))

(define (find-mode name) (registry-ref %modes name))
(define (mode-names) (registry-keys %modes))

;; The keymap of mode NAME for STATE (`chord` or `normal`), to bind keys in.
(define (mode-map name #:state [state 'chord])
  (let ((m (or (find-mode name) (error "No such mode" name))))
    (if (eq? state 'normal) (mode-normal m) (mode-keys m))))

;; A keymap with BINDINGS, (key-description command) lists. A mode defined
;; again keeps its keymaps, so keys bound in them since stay.
(define (mode-keymap old bindings)
  (let ((km (or old (make-keymap))))
    (for-each (lambda (b) (define-key! km (car b) (cadr b))) bindings)
    km))

(define (add-mode! name doc parent minor keys normal layer target-at settings files)
  (let ((old (find-mode name)))
    (registry-add! %modes name
                   (%make-mode name doc parent minor
                               (mode-keymap (and old (mode-keys old)) keys)
                               (mode-keymap (and old (mode-normal old)) normal)
                               layer target-at settings files))))

(define (register-mode! name doc #:parent [parent 'fundamental-mode] #:files [files '()]
                        #:keys [keys '()] #:normal [normal '()] #:layer [layer #f] #:target-at [target-at #f]
                        #:settings [settings '()])
  "Define the major mode NAME, extending PARENT's keys, layers and settings:
KEYS and NORMAL are (key-description command) bindings for chords and for
modal normal state, LAYER a procedure (document from to) giving highlights
(from to face), TARGET-AT a procedure (buffer position) giving the target
there, SETTINGS an alist of (setting . value), FILES the file name suffixes
it is for. Defines the command NAME, which gives the focused buffer this
mode."
  (add-mode! name doc (and (not (eq? name 'fundamental-mode)) parent) #f keys normal layer target-at settings files)
  (register-command! name doc (lambda (s n)
                                (set-buffer-mode! (or (current-buffer s) (error "No buffer")) name)
                                (message! s (symbol->string name))))
  name)

(define-syntax define-mode
  (syntax-rules ()
    ((_ name doc arg ...) (register-mode! 'name doc arg ...))))

(define (register-minor-mode! name doc #:keys [keys '()] #:normal [normal '()] #:layer [layer #f])
  "Define the minor mode NAME: KEYS and NORMAL are (key-description
command) bindings for chords and for modal normal state, LAYER a procedure
(document from to) giving highlights (from to face). Defines the command
NAME, which turns the mode on and off."
  (add-mode! name doc #f #t keys normal layer #f '() '())
  (register-command! name doc (lambda (s n) (toggle-mode! s name)))
  name)

(define-syntax define-minor-mode
  (syntax-rules ()
    ((_ name doc arg ...) (register-minor-mode! 'name doc arg ...))))

;; Mode NAME and its parents, nearest first; modes no longer defined end
;; the chain.
(define (mode-chain name)
  (let loop ((name name) (acc '()))
    (let ((m (and name (not (memq name (map mode-name acc))) (find-mode name))))
      (if m (loop (mode-parent m) (cons m acc)) (reverse acc)))))

;; Whether mode NAME is ANCESTOR or extends it.
(define (derived-mode? name ancestor)
  (and (memq ancestor (map mode-name (mode-chain name))) #t))

;; Setting KEY of mode NAME: its own, else its nearest parent's; #f if none
;; has one.
(define (mode-setting name key)
  (let loop ((chain (mode-chain name)))
    (cond ((null? chain) #f)
          ((assq key (mode-settings (car chain))) => cdr)
          (else (loop (cdr chain))))))

(define (buffer-setting b key) (and b (mode-setting (buffer-mode b) key)))

;; The major mode for a file named PATH: the one with the longest suffix
;; of it, else fundamental-mode.
(define (mode-for-file path)
  (let ((best (fold (lambda (name best)
                      (let ((m (find-mode name)))
                        (fold (lambda (suffix best)
                                (if (and (string-suffix? suffix path) (or (not best) (> (string-length suffix) (cdr best))))
                                    (cons name (string-length suffix))
                                    best))
                              best (if (mode-minor? m) '() (mode-files m)))))
                    #f (mode-names))))
    (if best (car best) 'fundamental-mode)))

;;; Minor modes are on per session for now.

;; The minor modes on in a session that still exist, newest first.
(define (session-modes s) (filter (lambda (m) m) (map find-mode (or (sget s 'modes) '()))))

(define (mode-on? s name) (and (memq name (or (sget s 'modes) '())) (find-mode name) #t))

(define (toggle-mode! s name)
  (if (mode-on? s name)
      (begin (sset! s 'modes (remove (lambda (m) (eq? m name)) (sget s 'modes)))
             (message! s (string-append (symbol->string name) " off")))
      (begin (sset! s 'modes (cons name (or (sget s 'modes) '())))
             (message! s (string-append (symbol->string name) " on")))))

;;; Keys

;; The modes whose keymaps apply in the focused buffer, the more specific
;; first: its major mode and that mode's parents, then the minor modes on.
(define (buffer-modes s)
  (let ((b (session-buffer s)))
    (append (if b (mode-chain (buffer-mode b)) '()) (session-modes s))))

;; The keymaps keys are looked up in, in input STATE (`chord` or `normal`),
;; the first binding winning: the minibuffer's while it is open; else the
;; modes' (see `buffer-modes`), then the profile's own.
(define (active-keymaps s state)
  (or (let ((t (sget s 'transient-map))) (and t (list t)))
      (append (map (lambda (m) (if (eq? state 'normal) (mode-normal m) (mode-keys m))) (buffer-modes s))
              (let ((km (profile-keymap (sget s 'profile) state))) (if km (list km) '())))))

;; The binding of KEYS (a list of keys) in MAPS: a command name, a keymap
;; (a prefix) or #f.
(define (key-binding maps keys)
  (let loop ((maps maps))
    (cond ((null? maps) #f)
          ((lookup-key (car maps) keys) => (lambda (b) b))
          (else (loop (cdr maps))))))

;; Every keymap some key may be looked up in with the session's profile:
;; the profile's, every mode's for each state, and TRANSIENTS (the
;; minibuffer's).
(define (all-keymaps s transients)
  (let ((p (sget s 'profile)))
    (append transients
            (filter (lambda (km) km) (list (profile-keymap p 'chord) (profile-keymap p 'normal)))
            (append-map (lambda (name) (let ((m (find-mode name))) (list (mode-keys m) (mode-normal m)))) (mode-names)))))

;;; Layers and targets

;; Highlights of DOC between FROM and TO from the layers of its buffer's
;; modes and the minor modes on, in order.
(define (buffer-layers s doc from to)
  (let* ((b (document-buffer doc))
         (modes (append (if b (mode-chain (buffer-mode b)) '()) (session-modes s))))
    (sort (append-map (lambda (m) (let ((layer (mode-layer m))) (if layer (layer doc from to) '()))) modes)
          (lambda (a b) (< (car a) (car b))))))

;; The target at POS in buffer B, as its nearest mode with a way to find
;; one says.
(define (buffer-target-at b pos)
  (let ((m (find (lambda (m) (mode-target-at m)) (mode-chain (buffer-mode b)))))
    (and m ((mode-target-at m) b pos))))

;;; The modes others build on, as Emacs has them. Until a language has a
;;; mode of its own, its files get prog-mode.

(define-mode fundamental-mode "The mode every other extends.")
(define-mode text-mode "Prose." #:files '(".txt" ".md" ".org" ".yaml" ".yml"))
(define-mode conf-mode "Configuration files." #:files '(".conf" ".cfg" ".ini" ".toml"))
(define-mode prog-mode
  "Programming languages."
  #:files '(".rs" ".c" ".h" ".cc" ".cpp" ".hpp" ".py" ".sh" ".bash" ".nix" ".el" ".js" ".ts" ".json" ".go"
            ".java" ".lua" ".zig" ".hs" ".ml" ".rb" ".pl"))
(define-mode special-mode "Buffers the editor writes, not you." #:settings '((read-only . #t)))
(define-mode log-mode "Output as it comes: *Messages*, a shell command's." #:parent 'special-mode)
