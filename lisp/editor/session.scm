;;; Editing sessions: views in panes, driven by one key profile.
;;;
;;; A session is a property table, so profiles and packages keep their own
;;; state on it. It holds its panes (views, techne-editor), the focused one
;;; and a profile. Keys are strings in Emacs notation: "a", "C-x", "M-f",
;;; "C-M-_", and the named keys "RET", "ESC", "DEL", "SPC", "TAB". `press`
;;; hands a key to the profile, which decides what it means.
;;;
;;; Commands are named procedures (session count) in one table; profiles bind
;;; keys to their names. Errors a command raises become the session's
;;; message, as in Emacs.
;;;
;;; While the minibuffer is open it takes the keys (`transient`), and
;;; commands edit its input (`session-view`).
;;;
;;; Hooks are named events packages react to, never a way to configure a
;;; buffer (that is what modes and options are for).

(provide make-session make-session-for-view sget sset! press press-keys type-text kbd
         session-view session-document session-panes session-focus set-session-panes! focus-view! view=?
         session-tree set-session-tree! tree-leaves split-pane! delete-pane! pane-places
         pane-view set-pane-view! document=? command-doc
         define-command register-command! command command-names run-command message! error-text messages-document message-log-max
         define-hook register-hook! add-hook! remove-hook! run-hook! hook-names hook-doc
         make-keymap keymap? define-key! lookup-key keymap-sequences keymap-name name-prefix! prefix-bindings
         printable-key? key-char key-for-char
         make-profile profile? profile-name profile-click profile-keymap)

(define-record-type profile
  (make-profile name init key click keymaps)
  profile?
  (name profile-name)
  ;; (session) -> sets up the profile's state
  (init profile-init)
  ;; (session key) -> handles one key
  (key profile-key)
  ;; (session position extend?) -> handles a click the frontend resolved
  (click profile-click)
  ;; Its own keymap for each input state it has: an alist of (state .
  ;; keymap), the states `chord` and `normal`.
  (keymaps profile-keymaps))

(define (profile-keymap p state)
  (let ((km (assq state (profile-keymaps p)))) (and km (cdr km))))

(define (sget s k) (hash-table-ref/default s k #f))
(define (sset! s k v) (hash-table-set! s k v))

(define (make-session doc actor profile)
  (make-session-for-view (make-view doc actor) profile))

(define (make-session-for-view view profile)
  (let ((s (make-hash-table)))
    (set-session-panes! s (list view) 0)
    (sset! s 'profile profile)
    ((profile-init profile) s)
    s))

;;; Panes: the views shown, in order, and the focused one; and how they
;;; tile the frame, a tree as Emacs's windows (EDITOR.md, section 9). A
;;; leaf is a pane's index; a split is (split dir parts), DIR `below` or
;;; `right`, PARTS a list of (share . tree), the shares summing to 1.

(define (session-panes s) (sget s 'panes))
(define (session-focus s) (sget s 'focus))

;; Set the panes. With as many as the tree has, it stays; else they are
;; stacked evenly.
(define (set-session-panes! s views focus)
  (sset! s 'panes views)
  (sset! s 'focus focus)
  (unless (and (sget s 'tree) (= (length (tree-leaves (sget s 'tree))) (length views)))
    (sset! s 'tree (stacked (length views)))))

(define (session-tree s) (sget s 'tree))
(define (set-session-tree! s tree) (sset! s 'tree tree))

(define (stacked n)
  (if (= n 1) 0 (list 'split 'below (map (lambda (i) (cons (/ 1.0 n) i)) (iota n)))))

(define (tree-leaves t)
  (if (integer? t) (list t) (append-map (lambda (p) (tree-leaves (cdr p))) (caddr t))))

;; The tree with each leaf K replaced by (F K): a number or a tree.
(define (tree-map t f)
  (if (integer? t) (f t) (list 'split (cadr t) (map (lambda (p) (cons (car p) (tree-map (cdr p) f))) (caddr t)))))

;; Split pane I in two, in DIR, the new pane (I + 1) after it with VIEW;
;; the panes after it move up one. The focus stays.
(define (split-pane! s dir view)
  (let ((i (session-focus s)) (panes (session-panes s)))
    (sset! s 'tree (tree-map (session-tree s)
                             (lambda (k)
                               (cond ((< k i) k)
                                     ((> k i) (+ k 1))
                                     (else (list 'split dir (list (cons 0.5 i) (cons 0.5 (+ i 1)))))))))
    (sset! s 'panes (append (take panes (+ i 1)) (list view) (drop panes (+ i 1))))))

;; Delete pane I: its space goes to the part before it, else after it,
;; whose nearest pane gets the focus.
(define (delete-pane! s i)
  (define (without t)
    (if (integer? t)
        t
        (let* ((parts (caddr t))
               (k (list-index (lambda (p) (equal? (cdr p) i)) parts)))
          (if k
              (let* ((gone (car (list-ref parts k)))
                     (to (if (> k 0) (- k 1) 1))
                     (heir (cdr (list-ref parts to)))
                     (rest (filter-map (lambda (j)
                                         (let ((p (list-ref parts j)))
                                           (cond ((= j k) #f)
                                                 ((= j to) (cons (+ (car p) gone) (cdr p)))
                                                 (else p))))
                                       (iota (length parts)))))
                (sset! s 'heir (if (> k 0) (last (tree-leaves heir)) (car (tree-leaves heir))))
                (if (null? (cdr rest)) (cdr (car rest)) (list 'split (cadr t) rest)))
              (list 'split (cadr t) (map (lambda (p) (cons (car p) (without (cdr p)))) parts))))))
  (let* ((tree (without (session-tree s)))
         (heir (sget s 'heir))
         (panes (session-panes s)))
    (sset! s 'tree (tree-map tree (lambda (k) (if (> k i) (- k 1) k))))
    (sset! s 'panes (append (take panes i) (drop panes (+ i 1))))
    (sset! s 'focus (if (> heir i) (- heir 1) heir))))

;; Each pane's place in the frame, in its order: (x y w h), fractions.
(define (pane-places s)
  (let ((places (make-vector (length (session-panes s)) #f)))
    (let place ((t (session-tree s)) (x 0.0) (y 0.0) (w 1.0) (h 1.0))
      (if (integer? t)
          (vector-set! places t (list x y w h))
          (let loop ((parts (caddr t)) (at 0.0))
            (unless (null? parts)
              (let ((share (car (car parts))))
                (if (eq? (cadr t) 'right)
                    (place (cdr (car parts)) (+ x (* w at)) y (* w share) h)
                    (place (cdr (car parts)) x (+ y (* h at)) w (* h share)))
                (loop (cdr parts) (+ at share)))))))
    (vector->list places)))
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
;; else to the profile. As in Emacs, a key clears the echo area's message
;; first: a prefix key or one typed into the minibuffer runs no command
;; that would. Then the `after-key` hook runs.
(define (press s key)
  (message! s #f)
  ((or (sget s 'transient) (profile-key (sget s 'profile))) s key)
  (run-hook! s 'after-key))

;;; Hooks: named events, each with documentation saying when it runs and
;;; with what. A procedure is added to one under a name, owned by the scope
;;; that adds it: adding it again under that name replaces it, keeping its
;;; place, and unloading its package removes it. They run in the order
;;; added; one that fails shows its error and the others still run.

(define %hooks (make-registry 'hooks))
(define %hook-procedures (make-registry 'hook-procedures))
(define %hook-count 0)

(define (register-hook! name doc)
  (registry-add! %hooks name doc)
  name)

(define-syntax define-hook
  (syntax-rules ()
    ((_ name doc) (register-hook! 'name doc))))

(define (hook-names) (registry-keys %hooks))
(define (hook-doc name) (registry-ref %hooks name))

(define (add-hook! hook name proc)
  "Run (PROC session args ...) at each HOOK, as NAME."
  (unless (registry-ref %hooks hook) (error "No such hook" hook))
  (let ((old (registry-ref %hook-procedures (cons hook name))))
    (set! %hook-count (+ %hook-count 1))
    (registry-add! %hook-procedures (cons hook name) (cons (if old (car old) %hook-count) proc))
    name))

(define (remove-hook! hook name) (registry-remove! %hook-procedures (cons hook name)))

(define (run-hook! s hook . args)
  (for-each (lambda (p)
              (guard (e (#t (message! s (error-text e))))
                (apply (cdr p) s args)))
            (sort (filter-map (lambda (k) (and (eq? (car k) hook) (registry-ref %hook-procedures k)))
                              (registry-keys %hook-procedures))
                  (lambda (a b) (< (car a) (car b))))))

(define-hook after-key "After the session handles a key, whatever the key did: (session).")

;; Documents are the same when their ids are (each handle Lisp gets is a
;; new object).
(define (document=? a b) (= (document-id a) (document-id b)))

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

;; Show TEXT in the echo area (#f clears it); it is kept in *Messages*.
(define (message! s text)
  (sset! s 'message text)
  (when text (log-message! text)))

;;; *Messages*: every message shown, as Emacs keeps them, the same one
;;; repeated counted on one line; at most `message-log-max` lines. Its
;;; views are read-only; a view of its own writes it.

(define message-log-max 1000)
(define %messages #f)

(define (messages-document)
  (unless %messages
    (let ((d (make-document "")))
      (set! %messages (list d (make-view d "messages") #f 0))))
  (car %messages))

(define (log-message! text)
  (messages-document)
  (let* ((d (car %messages)) (w (cadr %messages)) (len (document-length d))
         (again (equal? text (caddr %messages)))
         (count (if again (+ 1 (cadddr %messages)) 1))
         (line (if again (string-append text " [" (number->string count) " times]") text)))
    (view-edit! w (list (list (if again (line-start d (prev-grapheme d len)) len) len (string-append line "\n"))) "new")
    (set! %messages (list d w text count))
    (let ((lines (- (line-number d (document-length d)) 1)))
      (when (> lines message-log-max)
        (view-edit! w (list (list 0 (line-down d 0 (- lines message-log-max) 0) "")) "new")))))

;; What an error says, for the echo area. The view's editing natives
;; (view-edit! ...) say why an edit is refused in words for the user; the
;; VM's prefix naming them is left out, as Emacs says "Text is read-only".
(define (error-text e)
  (cond ((error-object? e)
         (let ((irritants (error-object-irritants e))
               (message (let ((m (error-object-message e)))
                          (if (and (string-prefix? "view-" m) (string-contains m "!: "))
                              (substring m (+ (string-contains m "!: ") 3) (string-length m))
                              m))))
           (if (null? irritants)
               message
               (string-append message ": "
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

;;; Keymaps: key -> command name or keymap.

(define-record-type keymap
  (%make-keymap table name)
  keymap?
  (table keymap-table)
  ;; What a prefix map is for, as which-key shows it ("+file").
  (name keymap-name set-keymap-name!))

(define (make-keymap) (%make-keymap (make-hash-table) #f))

;; Name the prefix KEYS of MAP (a key description), for which-key.
(define (name-prefix! map keys name)
  (let ((m (lookup-key map (kbd keys))))
    (if (keymap? m) (set-keymap-name! m name) (error "not a prefix" keys))))

;; The bindings directly under the prefix KEYS in MAPS, the first map's
;; first: a list of (key . binding).
(define (prefix-bindings maps keys)
  (fold (lambda (map acc)
          (let ((m (lookup-key map keys)))
            (if (keymap? m)
                (append acc (filter-map (lambda (k)
                                          (and (not (assoc k acc))
                                               (cons k (hash-table-ref/default (keymap-table m) k #f))))
                                        (hash-table-keys (keymap-table m))))
                acc)))
        '()
        maps))

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
