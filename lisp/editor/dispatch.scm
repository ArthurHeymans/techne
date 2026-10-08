;;; Dispatch: keys to the profile, commands by name, hooks, and the
;;; messages commands show. `press` hands a key to the profile, which
;;; decides what it means.
;;;
;;; Commands are named procedures (session count) in one table; profiles bind
;;; keys to their names. Errors a command raises become the session's
;;; message, as in Emacs. While the minibuffer is open it takes the keys
;;; (`transient`).
;;;
;;; Hooks are named events packages react to, never a way to configure a
;;; buffer (that is what modes and options are for).

(require "session.scm")
(require "keymaps.scm")

(provide press press-keys type-text command-doc
         define-command register-command! command command-names run-command message! error-text messages-document message-log-max
         define-hook register-hook! add-hook! remove-hook! run-hook! hook-names hook-doc)

(define (press s key)
  "Handle the key KEY in session S, then run the `after-key` hook.
The key goes to the minibuffer while it is open, else to the profile.
As in Emacs, a key clears the echo area's message first."
  (message! s #f)
  ((or (sget s 'transient) (profile-key (sget s 'profile))) s key)
  (run-hook! s 'after-key key))

(define (press-keys s keys)
  "Press each key of KEYS, a key description, in session S."
  (for-each (lambda (k) (press s k)) (kbd keys)))

(define (type-text s text)
  "Press in session S the key of each character of TEXT, as typing it."
  (for-each (lambda (c) (press s (key-for-char c))) (string->list text)))

;;; Hooks: named events, each with documentation saying when it runs and
;;; with what. A procedure is added to one under a name, owned by the scope
;;; that adds it, and runs in that scope: adding it again under that name
;;; replaces it, keeping its place, and unloading its package removes it.
;;; They run in the order added; one that fails shows its error and the
;;; others still run.

(define %hooks (make-registry 'hooks))
(define %hook-procedures (make-registry 'hook-procedures))
(define %hook-count 0)

(define (register-hook! name doc)
  "Declare the hook NAME, documented by DOC; return NAME."
  (registry-add! %hooks name doc)
  name)

(define-syntax define-hook
  (syntax-rules ()
    "Declare the hook NAME, documented by DOC.
DOC says when it runs and what its procedures are called with."
    ((_ name doc) (register-hook! 'name doc))))

(define (hook-names)
  "Return the names of the hooks declared."
  (registry-keys %hooks))
(define (hook-doc name)
  "Return the documentation of the hook NAME, or #f if there is none."
  (registry-ref %hooks name))

(define (add-hook! hook name proc)
  "Run (PROC session args ...) at each HOOK, as NAME."
  (unless (registry-ref %hooks hook) (error "No such hook" hook))
  (let ((old (registry-ref %hook-procedures (cons hook name))))
    (set! %hook-count (+ %hook-count 1))
    (registry-add! %hook-procedures (cons hook name) (cons (if old (car old) %hook-count) (scope-procedure proc)))
    name))

(define (remove-hook! hook name)
  "Remove the procedure added to HOOK as NAME."
  (registry-remove! %hook-procedures (cons hook name)))

(define (run-hook! s hook . args)
  "Call each procedure added to HOOK, in the order added.
Each is called with the session S and ARGS; one that fails shows its
error in S, and the others still run."
  (for-each (lambda (p)
              (guard (e (#t (message! s (error-text e))))
                (apply (cdr p) s args)))
            (sort (filter-map (lambda (k) (and (eq? (car k) hook) (registry-ref %hook-procedures k)))
                              (registry-keys %hook-procedures))
                  (lambda (a b) (< (car a) (car b))))))

(define-hook after-key "After the session handles a key, whatever the key did: (session key).")

;;; Commands

;; Commands live in a registry: the scope a command is defined in owns it,
;; so shutting that scope (unloading a mode) removes it, and a command it
;; overrode is in effect again. A command runs in that scope too: the
;; tasks and processes it starts belong to its package, and go with it.
(define %commands (make-registry 'commands))

(define (register-command! name doc proc)
  "Make PROC, a procedure of a session and a count, the command NAME.
DOC documents it. The current scope owns the command."
  (registry-add! %commands name (list doc proc (scope-procedure proc))))

;; The command's procedure, as defined (for its documentation and source).
(define (command name)
  "Return the procedure of the command NAME; it is an error if none."
  (let ((c (registry-ref %commands name)))
    (if c (cadr c) (error "no such command" name))))

(define (command-names)
  "Return the names of the commands defined."
  (registry-keys %commands))

(define (command-doc name)
  "Return the documentation of the command NAME, or #f."
  (let ((c (registry-ref %commands name)))
    (and c (car c))))

;; Call command NAME in its package's scope.
(define (invoke-command name s n)
  (let ((c (or (registry-ref %commands name) (error "no such command" name))))
    ((caddr c) s n)))

;; The table calls the global binding, so redefining a command takes effect
;; at its next invocation.
(define-syntax define-command
  (syntax-rules ()
    "Define the command NAME, a procedure of the session S and the count N.
DOC documents both the procedure and the command; BODY runs when it is
invoked."
    ((_ (name s n) doc body ...)
     (begin
       (define (name s n) doc body ...)
       (register-command! 'name doc (lambda (s2 n2) (name s2 n2)))
       'name))))

(define (message! s text)
  "Show TEXT in the echo area of session S; #f clears it.
The message is kept in *Messages*."
  (sset! s 'message text)
  (when text (log-message! text)))

;;; *Messages*: every message shown, as Emacs keeps them, the same one
;;; repeated counted on one line; at most `message-log-max` lines. Its
;;; views are read-only; a view of its own writes it.

(define message-log-max 1000
  "The most lines *Messages* keeps.")
(define %messages #f)

(define (messages-document)
  "Return the document of *Messages*, made the first time."
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

(define (error-text e)
  "Return what the error E says, for the echo area.
The VM's prefix naming a view native (view-edit! ...) is left out, as
Emacs says \"Text is read-only\"."
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

(define (run-command s name n)
  "Run the command NAME in session S with the count N.
What commands record for the next one (a kill to append to, a goal
column) moves from now to last here; an error becomes the message."
  (sset! s 'this-command name)
  (sset! s 'kill-now #f)
  (sset! s 'goal-now #f)
  (message! s #f)
  (guard (e (#t (message! s (error-text e))))
    (invoke-command name s n))
  (sset! s 'last-kill (sget s 'kill-now))
  (unless (sget s 'goal-now) (sset! s 'goal #f))
  (sset! s 'last-command name))
