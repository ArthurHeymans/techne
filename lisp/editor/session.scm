;;; Editing sessions: views in panes, driven by one key profile.
;;;
;;; A session is a property table, so profiles and packages keep their own
;;; state on it. It holds its panes (views, techne-editor), the focused one,
;;; a profile and the minor modes it has on. Keys are
;;; strings in Emacs notation: "a", "C-x", "M-f", "C-M-_", and the named keys
;;; "RET", "ESC", "DEL", "SPC", "TAB". `press` hands a key to the profile,
;;; which decides what it means.
;;;
;;; Commands are named procedures (session count) in one table; profiles bind
;;; keys to their names. Errors a command raises become the session's
;;; message, as in Emacs.
;;;
;;; While the minibuffer is open it takes the keys (`transient`), and
;;; commands edit its input (`session-view`).

(provide make-session make-session-for-view sget sset! press press-keys type-text kbd
         session-view session-document session-panes session-focus set-session-panes! focus-view! view=?
         pane-view set-pane-view! document=? doc-prop set-doc-prop! command-doc
         define-mode register-mode! find-mode mode-names mode-on? toggle-mode! session-layers mode-binding
         define-command register-command! command command-names run-command message!
         make-keymap keymap? define-key! lookup-key keymap-sequences
         printable-key? key-char key-for-char
         make-profile profile? profile-name profile-click)

(define-record-type profile
  (make-profile name init key click)
  profile?
  (name profile-name)
  ;; (session) -> sets up the profile's state
  (init profile-init)
  ;; (session key) -> handles one key
  (key profile-key)
  ;; (session position extend?) -> handles a click the frontend resolved
  (click profile-click))

(define (sget s k) (hash-table-ref/default s k #f))
(define (sset! s k v) (hash-table-set! s k v))

(define (make-session doc actor profile)
  (make-session-for-view (make-view doc actor) profile))

(define (make-session-for-view view profile)
  (let ((s (make-hash-table)))
    (set-session-panes! s (list view) 0)
    (sset! s 'profile profile)
    (sset! s 'modes '())
    ((profile-init profile) s)
    s))

;; The panes: views shown one above the other, and the focused one.
(define (session-panes s) (sget s 'panes))
(define (session-focus s) (sget s 'focus))
(define (set-session-panes! s views focus)
  (sset! s 'panes views)
  (sset! s 'focus focus))
;; Views are the same when their ids are (the host may hand Lisp a new
;; handle to a view it already has).
(define (view=? a b) (= (view-id a) (view-id b)))

(define (focus-view! s view)
  (let loop ((vs (session-panes s)) (i 0))
    (cond ((null? vs) #f)
          ((view=? (car vs) view) (sset! s 'focus i) #t)
          (else (loop (cdr vs) (+ i 1))))))

;; Commands act on the focused pane's view and its document, or on the
;; minibuffer's input while it is open.
(define (session-view s) (or (sget s 'input-view) (pane-view s)))
(define (session-document s) (view-document (session-view s)))

;; The focused pane's view, and putting another in its place.
(define (pane-view s) (list-ref (session-panes s) (session-focus s)))
(define (set-pane-view! s view)
  (let ((panes (session-panes s)) (i (session-focus s)))
    (set-session-panes! s (append (take panes i) (list view) (drop panes (+ i 1))) i)))

;; Keys go to the transient handler if there is one (the minibuffer's),
;; else to the profile.
(define (press s key)
  ((or (sget s 'transient) (profile-key (sget s 'profile))) s key))

;;; Document properties: what Lisp keeps about a document (a buffer's name,
;;; its own keymap and layers), by its identity.

(define (document=? a b) (= (document-id a) (document-id b)))

(define %doc-props (make-hash-table))

(define (doc-prop d key)
  (let ((props (hash-table-ref/default %doc-props (document-id d) #f)))
    (and props (hash-table-ref/default props key #f))))

(define (set-doc-prop! d key value)
  (let ((props (or (hash-table-ref/default %doc-props (document-id d) #f)
                   (let ((t (make-hash-table))) (hash-table-set! %doc-props (document-id d) t) t))))
    (hash-table-set! props key value)))

(define (kbd keys) (string-split keys " "))

;; Press each key of a space-separated key description.
(define (press-keys s keys) (for-each (lambda (k) (press s k)) (kbd keys)))

;; Press the key of each character, as typing it would.
(define (type-text s text)
  (for-each (lambda (c) (press s (key-for-char c))) (string->list text)))

(define (key-for-char c)
  (case c
    ((#\space) "SPC")
    ((#\newline) "RET")
    ((#\tab) "TAB")
    (else (string c))))

(define (printable-key? key)
  (or (= (string-length key) 1) (member key '("SPC" "TAB"))))

(define (key-char key)
  (cond ((string=? key "SPC") #\space)
        ((string=? key "TAB") #\tab)
        (else (string-ref key 0))))

;;; Commands

;; Commands live in a registry: the scope a command is defined in owns it,
;; so shutting that scope (unloading a mode) removes it.
(define %commands (make-registry 'commands))

(define (register-command! name doc proc)
  (registry-add! %commands name (cons doc proc)))

(define (command name)
  (let ((c (registry-ref %commands name)))
    (if c (cdr c) (error "no such command" name))))

(define (command-names) (registry-keys %commands))

(define (command-doc name)
  (let ((c (registry-ref %commands name)))
    (and c (car c))))

;; The table calls the global binding, so redefining a command takes effect
;; at its next invocation.
(define-syntax define-command
  (syntax-rules ()
    ((_ (name s n) doc body ...)
     (begin
       (define (name s n) body ...)
       (register-command! 'name doc (lambda (s2 n2) (name s2 n2)))
       'name))))

(define (message! s text) (sset! s 'message text))

(define (error-text e)
  (cond ((error-object? e)
         (let ((irritants (error-object-irritants e)))
           (if (null? irritants)
               (error-object-message e)
               (string-append (error-object-message e) ": "
                              (string-join (map (lambda (x) (call-with-output-string (lambda (p) (display x p)))) irritants) " ")))))
        ((string? e) e)
        (else (call-with-output-string (lambda (p) (write e p))))))

;; Run a command with a count. What commands record for the next one (a
;; kill to append to, a goal column to keep) is moved from "now" to "last"
;; here.
(define (run-command s name n)
  (sset! s 'this-command name)
  (sset! s 'kill-now #f)
  (sset! s 'goal-now #f)
  (message! s #f)
  (guard (e (#t (message! s (error-text e))))
    ((command name) s n))
  (sset! s 'last-kill (sget s 'kill-now))
  (unless (sget s 'goal-now) (sset! s 'goal #f))
  (sset! s 'last-command name))

;;; Minor modes: a keymap whose bindings come before the profile's, and
;;; layers that highlight text, turned on and off per session. A mode lives
;;; in a registry, owned by the scope (the package) that defined it; when
;;; that is shut, the mode is gone from every session that had it on.

(define-record-type mode
  (make-mode name doc keymap layers)
  mode?
  (name mode-name)
  (doc mode-doc)
  (keymap mode-keymap)
  ;; Procedures (document from to) -> list of (from to face).
  (layers mode-layers))

(define %modes (make-registry 'modes))

(define (register-mode! name doc #:keys [keys '()] #:layer [layer #f])
  "Define the minor mode NAME: KEYS are (key-description command) bindings,
LAYER a procedure (document from to) giving highlights (from to face).
Defines the command NAME, which turns the mode on and off."
  (let ((km (make-keymap)))
    (for-each (lambda (b) (%define-key! km (car b) (cadr b))) keys)
    (registry-add! %modes name (make-mode name doc km (if layer (list layer) '())))
    (register-command! name doc (lambda (s n) (toggle-mode! s name)))
    name))

(define-syntax define-mode
  (syntax-rules ()
    ((_ name doc arg ...) (register-mode! 'name doc arg ...))))

(define (find-mode name) (registry-ref %modes name))
(define (mode-names) (registry-keys %modes))

;; The modes on in a session that still exist.
(define (session-modes s) (filter (lambda (m) m) (map find-mode (sget s 'modes))))

(define (mode-on? s name) (and (memq name (sget s 'modes)) (find-mode name) #t))

(define (toggle-mode! s name)
  (if (mode-on? s name)
      (begin (sset! s 'modes (remove (lambda (m) (eq? m name)) (sget s 'modes)))
             (message! s (string-append (symbol->string name) " off")))
      (begin (sset! s 'modes (cons name (sget s 'modes)))
             (message! s (string-append (symbol->string name) " on")))))

;; The binding of a key sequence in the modes on, newest first, then in the
;; focused document's own keymap: a command name, a keymap (a prefix) or #f.
(define (mode-binding s keys)
  (let loop ((maps (append (map mode-keymap (session-modes s))
                           (let ((km (doc-prop (session-document s) 'keymap))) (if km (list km) '())))))
    (cond ((null? maps) #f)
          ((lookup-key (car maps) keys) => (lambda (b) b))
          (else (loop (cdr maps))))))

;; Highlights of DOC between FROM and TO from the modes' layers and the
;; document's own, in order.
(define (session-layers s doc from to)
  (sort (append-map (lambda (layer) (layer doc from to))
                    (append (append-map mode-layers (session-modes s)) (or (doc-prop doc 'layers) '())))
        (lambda (a b) (< (car a) (car b)))))

;;; Keymaps: key -> command name or keymap.

(define-record-type keymap
  (%make-keymap table)
  keymap?
  (table keymap-table))

(define (make-keymap) (%make-keymap (make-hash-table)))

;; Bind KEYS (a key description) to BINDING in MAP. A binding made in a
;; scope other than the root is owned by it: shutting the scope removes the
;; binding, unless it has been rebound since. While a package loads, the
;; binding waits until the package is published.
(define (define-key! map keys binding)
  (let ((s (current-scope)))
    (cond ((%scope-pending s)
           (%set-scope-pending! s (cons (lambda () (define-key! map keys binding)) (%scope-pending s))))
          (else
           (%define-key! map keys binding)
           (let ((token (list keys)) (place (cons map keys)))
             (hash-table-set! %key-owners place token)
             (unless (eq? s %root-scope)
               (scope-own! token
                           (lambda (t)
                             (when (eq? (hash-table-ref/default %key-owners place #f) t)
                               (hash-table-delete! %key-owners place)
                               (%undefine-key! map (kbd keys)))))))))))

;; The latest binding of each (keymap . keys): only its owner removes it.
(define %key-owners (make-hash-table))

(define (%undefine-key! map keys)
  (let ((m (lookup-key map (reverse (cdr (reverse keys))))))
    (when (keymap? m) (hash-table-delete! (keymap-table m) (last keys)))))

(define (%define-key! map keys binding)
  (let loop ((map map) (keys (kbd keys)))
    (if (null? (cdr keys))
        (hash-table-set! (keymap-table map) (car keys) binding)
        (let ((next (hash-table-ref/default (keymap-table map) (car keys) #f)))
          (if (keymap? next)
              (loop next (cdr keys))
              (let ((m (make-keymap)))
                (hash-table-set! (keymap-table map) (car keys) m)
                (loop m (cdr keys))))))))

;; Every key sequence bound in a keymap, each a string as `kbd` reads it.
(define (keymap-sequences km)
  (append-map (lambda (key)
                (let ((b (hash-table-ref/default (keymap-table km) key #f)))
                  (if (keymap? b)
                      (map (lambda (rest) (string-append key " " rest)) (keymap-sequences b))
                      (list key))))
              (hash-table-keys (keymap-table km))))

;; The binding of a key sequence (a list of keys): a command name, a keymap
;; (a prefix), or #f.
(define (lookup-key map keys)
  (cond ((null? keys) map)
        ((not (keymap? map)) #f)
        (else (lookup-key (hash-table-ref/default (keymap-table map) (car keys) #f) (cdr keys)))))
