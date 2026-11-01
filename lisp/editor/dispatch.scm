;;; Dispatch: keys to the profile, commands by name, hooks, and the
;;; messages commands show. `press` hands a key to the profile, which
;;; decides what it means.
;;;
;;; Commands are named procedures (session count) in one table; profiles bind
;;; keys to their names. Each run is an operation (operations.scm). Errors
;;; a command raises become the session's message, as in Emacs. While the minibuffer is open it takes the keys
;;; (`transient`). An error that is a violation, a mistake in the editor's
;;; code rather than a failure a command reports, is kept for
;;; `take-violations!` too.
;;;
;;; Hooks are named events packages react to, never a way to configure a
;;; buffer (that is what modes and options are for).

(require "session.scm")
(require "keymaps.scm")
(require "operations.scm")
(require "../schema.scm")

(provide press press-keys type-text command-doc
         define-command register-command! command command-names run-command command-joins! message!
         invoke make-contract command-contract command-code contract? contract-params contract-entries contract-read
         contract-effects contract-expose contract-available contract-result contract-hash
         contract-argument-schema contract-sensitive set-command-acquirer! contract-conform-argument ->data error-text messages-document message-log-max
         log-history
         failed! take-violations!
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
;; The procedures of each hook, in the order added, found when the hook
;; first runs after they change: a hook runs on every key.
(define %hook-order (make-hash-table))
(define %hook-procedures
  (make-registry 'hook-procedures #:changed (lambda (key value) (hash-table-delete! %hook-order (car key)))))
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
error in S, and the others still run. They run in no operation: what
they change is no command's doing."
  (let ((ps (hook-procedures hook)))
    ;; Every key runs two: none added is the common case.
    (unless (null? ps)
      (without-operation (lambda () (run-hook* s hook ps args))))))

(define (hook-procedures hook)
  (or (hash-table-ref/default %hook-order hook #f)
      (let ((ps (sort (filter-map (lambda (k) (and (eq? (car k) hook) (registry-ref %hook-procedures k)))
                                  (registry-keys %hook-procedures))
                      (lambda (a b) (< (car a) (car b))))))
        (hash-table-set! %hook-order hook ps)
        ps)))

(define (run-hook* s hook ps args)
  (for-each (lambda (p)
              (guard (e (#t (failed! s hook e)))
                (apply (cdr p) s args)))
            ps))

(define-hook after-key "After the session handles a key, whatever the key did: (session key).")
(define-hook operation-ended "When an operation ends: (session operation).
It ends done, failed, cancelled or partial (`operation-outcome`); one
that runs of its command join (typing) ends again with each run.")

(set-operation-ended-handler!
 (lambda (s op r)
   ;; An effect a contract did not declare is a mistake in the command, as
   ;; a violation is: kept for `take-violations!`, which tests and keyfuzz read.
   (when (record-violation r)
     (note-violation! s (string-append (symbol->string (record-command r)) ": " (record-violation r))))
   (run-hook! s 'operation-ended op)))

;;; Commands

;; Commands live in a registry: the scope a command is defined in owns it,
;; so shutting that scope (unloading a mode) removes it, and a command it
;; overrode is in effect again. A command runs in that scope too: the
;; tasks and processes it starts belong to its package, and go with it.
(define %commands (make-registry 'commands))

(define (register-command! name doc proc #:body [body #f] #:contract [contract #f])
  "Make PROC, a procedure of a session and a count, the command NAME.
DOC documents it. The current scope owns the command. BODY, if given,
is a thunk returning the procedure PROC runs now (a global binding's,
which redefining changes), whose identity records of its runs keep.
With CONTRACT (`make-contract`), PROC is a procedure of a session and
the contract's arguments, in order, and the command is run with them:
read from the user (keys, \\[execute-extended-command]), or given as
data (`invoke`)."
  ;; Last: (procedure . its code-id), the code BODY gave last, numbered.
  (registry-add! %commands name (list doc proc (scope-procedure proc) (or body (lambda () proc)) contract (cons #f #f))))

(define (command-code name)
  "Return the number of the code the command NAME runs now, or #f.
Records of its runs keep the number of theirs (`record-code`)."
  (let ((c (registry-ref %commands name)))
    (and c (command-code-id c))))

;; The number of the code command entry C runs now: the procedure its
;; binding holds, numbered once while it holds the same.
(define (command-code-id c)
  (let ((code ((cadddr c))) (cached (list-ref c 5)))
    (unless (eq? (car cached) code)
      (set-car! cached code)
      (set-cdr! cached (code-id code)))
    (cdr cached)))

(define (command-contract name)
  "Return the contract of the command NAME, or #f: run by a count only."
  (let ((c (registry-ref %commands name)))
    (and c (list-ref c 4))))

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
invoked. With #:arguments after DOC, it is a command with a contract,
a procedure of the session and its arguments, in order: (define-command
(name s arg ...) doc #:arguments '((arg schema option ...) ...) ...
body ...), with #:read, #:effects, #:expose, #:available and #:result
after #:arguments as `make-contract` takes them."
    ((_ (name s param ...) doc #:arguments arguments rest ...)
     (%command-options (name s param ...) doc (arguments) rest ...))
    ((_ (name s n) doc body ...)
     (begin
       (define (name s n) doc body ...)
       (register-command! 'name doc (lambda (s2 n2) (name s2 n2)) #:body (lambda () name))
       'name))))

;; The options of a command with a contract, after its #:arguments, then
;; its body.
(define-syntax %command-options
  (syntax-rules ()
    ((_ head doc (opt ...) #:read v rest ...) (%command-options head doc (opt ... #:read v) rest ...))
    ((_ head doc (opt ...) #:effects v rest ...) (%command-options head doc (opt ... #:effects v) rest ...))
    ((_ head doc (opt ...) #:expose v rest ...) (%command-options head doc (opt ... #:expose v) rest ...))
    ((_ head doc (opt ...) #:available v rest ...) (%command-options head doc (opt ... #:available v) rest ...))
    ((_ head doc (opt ...) #:result v rest ...) (%command-options head doc (opt ... #:result v) rest ...))
    ((_ (name s param ...) doc (opt ...) body ...)
     (begin
       (define (name s param ...) doc body ...)
       (let ((contract (make-contract opt ...)))
         (unless (equal? (contract-params contract) '(param ...))
           (error "define-command: the arguments are not the procedure's" 'name (contract-params contract)))
         (register-command! 'name doc (lambda (s2 . args) (apply name s2 args)) #:body (lambda () name) #:contract contract))
       'name))))

;;; Contracts (doc/operations.md, "Contracts"): the arguments a command
;;; takes, as data, and what it may do.

(define-record-type contract
  (%make-contract params entries read effects expose available result hash)
  contract?
  ;; The arguments' names, in order.
  (params contract-params)
  ;; ((name schema option ...) ...): SCHEMA a schema, or a procedure of
  ;; the arguments before it, as a dict, giving one; options #:doc,
  ;; #:default, #:optional, #:sensitive.
  (entries contract-entries)
  ;; ((name how option ...) ...): how to read each from the user.
  (read contract-read)
  ;; What it may do, a list of `read`, `query`, `edit`, `settings`,
  ;; `external`, or a procedure of the arguments giving one.
  (effects contract-effects)
  ;; Who besides the user may invoke it: `agents`.
  (expose contract-expose)
  ;; (session) -> #t, or why it cannot run there; or #f.
  (available contract-available)
  (result contract-result)
  ;; Its identity: a changed contract makes old arguments suspect.
  (hash contract-hash))

(define (make-contract entries #:read [read '()] #:effects [effects '()] #:expose [expose '()] #:available [available #f] #:result [result #f])
  "Return the contract of a command taking the arguments ENTRIES.
ENTRIES are (name schema option ...), as a dict schema's, SCHEMA a
procedure of the arguments before when it depends on them; options
#:doc, #:default, #:optional and #:sensitive (redacted wherever it would
be recorded or written). READ says how to ask the user for each: (name
how option ...). EFFECTS is what it may do, a list of `read`, `query`,
`edit`, `settings`, `external`, or a procedure of the arguments giving
it. EXPOSE lists who else may invoke it (`agents`). AVAILABLE is a
procedure of the session giving #t or why not. RESULT is the schema of
what it returns."
  (for-each (lambda (e)
              ;; Its schema is checked when invoked: kinds it names may
              ;; be defined after it (contracts.scm).
              (unless (and (pair? e) (symbol? (car e)) (pair? (cdr e)) (or (procedure? (cadr e)) (symbol? (cadr e)) (pair? (cadr e))))
                (error "make-contract: not an argument (name schema option ...):" e)))
            entries)
  (%make-contract (map car entries) entries read effects expose available result
                  (contract-hash* entries effects expose result)))

;; A number for what a contract says: written, procedures as such.
(define (contract-hash* . parts)
  (let ((text (call-with-output-string (lambda (p) (write parts p)))))
    (fold (lambda (c h) (modulo (+ (* h 31) (char->integer c)) 1000000007)) 7 (string->list text))))

(define (entry-option e key default)
  (let loop ((opts (cddr e)))
    (cond ((null? opts) default)
          ;; Flags take no value.
          ((memq (car opts) '(#:optional #:sensitive)) (loop (cdr opts)))
          ((null? (cdr opts)) default)
          ((eq? (car opts) key) (cadr opts))
          (else (loop (cddr opts))))))

(define (entry-flag? e key) (and (memq key (cddr e)) #t))

(define (contract-sensitive contract)
  "Return the names of CONTRACT's sensitive arguments."
  (map car (filter (lambda (e) (entry-flag? e #:sensitive)) (contract-entries contract))))

;; An argument's schema, given the arguments before it.
(define (entry-schema e args)
  (if (procedure? (cadr e)) ((cadr e) args) (cadr e)))

(define (contract-argument-schema contract)
  "Return CONTRACT's arguments as a dict schema, for agents.
An argument whose schema depends on others is `any`; sensitive ones are
marked so in their documentation."
  `(dict ,@(map (lambda (e)
                  (let ((doc (entry-option e #:doc #f)))
                    `(,(car e) ,(if (procedure? (cadr e)) 'any (cadr e))
                      ,@(if (entry-flag? e #:optional) '(#:optional) '())
                      ,@(let ((d (memq #:default (cddr e)))) (if d (list #:default (cadr d)) '()))
                      ,@(if doc (list #:doc (if (entry-flag? e #:sensitive) (string-append doc " (sensitive)") doc)) '()))))
                (contract-entries contract))))

;; ARGS, a dict (keys symbols, or strings as JSON has them), made the
;; values of CONTRACT's arguments, in order; else an error saying why.
;; Return (values arguments-dict).
(define (contract-conform contract args name)
  (let* ((given (lambda (k)
                  (cond ((dict-contains? args k) (list (dict-ref args k)))
                        ((dict-contains? args (symbol->string k)) (list (dict-ref args (symbol->string k))))
                        (else '()))))
         (known (append (contract-params contract) (map symbol->string (contract-params contract))))
         (unknown (remove (lambda (k) (member k known)) (dict-keys args))))
    (let loop ((entries (contract-entries contract)) (done (dict)) (problems '()))
      (if (null? entries)
          (let ((problems (append problems (map (lambda (k) (dict 'path (list k) 'message "not an argument")) unknown))))
            (if (null? problems)
                done
                (error (string-append "Invalid arguments for " (symbol->string name) ":\n" (problems->string problems)))))
          (let* ((e (car entries)) (k (car e)) (d (memq #:default (cddr e)))
                 ;; A default goes the way of a value given.
                 (v (let ((v (given k))) (if (and (null? v) d) (list (cadr d)) v))))
            (if (null? v)
                (if (entry-flag? e #:optional)
                    (loop (cdr entries) done problems)
                    (loop (cdr entries) done (append problems (list (dict 'path (list k) 'message "missing")))))
                (let ((r (conform-argument e (car v) done)))
                  (if (null? (cdr r))
                      (loop (cdr entries) (dict-set done k (car r)) problems)
                      (loop (cdr entries) done (append problems (cdr r)))))))))))

;; Argument E's VALUE conformed, given the arguments DONE before it:
;; (value . problems), the problems' paths starting with its name.
(define (conform-argument e value done)
  (let ((schema (guard (x ((error-object? x) #f)) (entry-schema e done))))
    (if (not schema)
        (cons #f (list (dict 'path (list (car e)) 'message "its type depends on arguments that are not valid")))
        (parameterize ((schema-redacted (entry-flag? e #:sensitive)))
          (let* ((ps '())
                 (conformed (conform schema value #:else (lambda (found) (set! ps found) #f))))
            (cons conformed (map (lambda (p) (dict-set p 'path (cons (car e) (dict-ref p 'path)))) ps)))))))

(define (contract-conform-argument name key value done)
  "Return VALUE made a value of the argument KEY of command NAME.
DONE is a dict of the arguments before it. Raise an error saying why
not, as `invoke` would."
  (let* ((contract (command-contract name))
         (e (or (assq key (contract-entries contract)) (error "Not an argument:" key)))
         (r (conform-argument e value done)))
    (if (null? (cdr r))
        (car r)
        (error (string-append "Invalid arguments for " (symbol->string name) ":\n" (problems->string (cdr r)))))))

;; What a record keeps of a value: data, walked down, bounded (strings
;; and lists cut short, saying so), a kind's value as its data, what is
;; not data written (a procedure as #<procedure>): no live object.
(define (->data v [depth 0])
  "Return what a record keeps of V: data, bounded, no live object.
Long strings and lists are cut short, saying so; a kind's value is its
data; what is not data is written. DEPTH is how deep V is in what is
kept, four at most."
  (let ((kind (and (not (string? v)) (schema-kind-of v))))
    (cond (kind
           (if (< depth 4) (->data (schema-kind-data kind v) (+ depth 1)) "…"))
          ((string? v)
           (if (> (string-length v) 200)
               (string-append (substring v 0 200) "… (" (number->string (string-length v)) " characters)")
               v))
          ((or (boolean? v) (symbol? v) (keyword? v) (char? v) (number? v) (null? v)) v)
          ((>= depth 4) "…")
          ((list? v)
           (let ((items (map (lambda (x) (->data x (+ depth 1))) (if (> (length v) 32) (take v 32) v))))
             (if (> (length v) 32) (append items (list (string-append "… (" (number->string (length v)) " in all)"))) items)))
          ((vector? v) (list->vector (->data (vector->list v) depth)))
          ((dict? v)
           (fold (lambda (k d) (dict-set d (->data k (+ depth 1)) (->data (dict-ref v k) (+ depth 1))))
                 (dict) (let ((ks (dict-keys v))) (if (> (length ks) 32) (take ks 32) ks))))
          (else (let ((w (call-with-output-string (lambda (p) (write v p)))))
                  (if (> (string-length w) 80) (string-append (substring w 0 80) "…") w))))))

;; An argument's value as data, for a record.
(define (argument-data e value)
  (if (entry-flag? e #:sensitive) "<redacted>" (->data value)))

;; How acquisition reads a command's arguments from the user, then
;; invokes it (contracts.scm): (session name count).
(define %acquire (lambda (s name n) (error "No reader of arguments yet" name)))

(define (set-command-acquirer! proc)
  "Read a contract command's arguments with (PROC session name count).
PROC ends by invoking the command (`invoke`), or by nothing when the
user aborts."
  (set! %acquire proc))

(define (invoke s name args #:principal [principal "user"] #:mode [mode 'apply])
  "Run the command NAME in session S with ARGS, a dict of its arguments.
Keys are the arguments' names, symbols or strings (JSON's). The command
must have a contract; it checks that PRINCIPAL (\"user\", or an agent's
name) may invoke it, that it is available, and conforms and validates
ARGS, refusing with what is wrong, values of sensitive arguments left
out. It runs as an operation, a child of the current one, recording the
arguments as data. MODE `check` stops there and returns #t; `apply`
runs it and returns what it returns. Errors are raised to the caller,
irritants that are sensitive values redacted."
  (let* ((c (or (registry-ref %commands name) (error "No such command" name)))
         (contract (or (list-ref c 4) (error "This command takes no arguments as data: it has no contract" name))))
    (unless (or (equal? principal "user") (memq 'agents (contract-expose contract)))
      (error "This command is not exposed to agents" name))
    ;; Only #t admits it: #f says it is not available, without saying why.
    (let ((why (if (contract-available contract) ((contract-available contract) s) #t)))
      (unless (eq? why #t)
        (error (string-append (symbol->string name) " is not available: " (if (string? why) why "not here")))))
    (let* ((done (contract-conform contract args name))
           (vals (map (lambda (k) (dict-ref done k #f)) (contract-params contract)))
           (effects (let ((e (contract-effects contract))) (if (procedure? e) (apply e vals) e)))
           (secrets (filter-map (lambda (ev) (and (entry-flag? (car ev) #:sensitive) (cdr ev))) (map cons (contract-entries contract) vals)))
           (data (map (lambda (e v) (cons (car e) (argument-data e v))) (contract-entries contract) vals)))
      (case mode
        ((check) #t)
        ((stage) (error "Staging is not built yet (doc/operations.md, \"Order of work\", step 6)"))
        ((apply)
         (let* ((d (view-document (session-view s)))
                (r (start-invocation! name s (command-code-id c) (document-id d) (document-revision d)
                                      data principal effects (contract-hash contract) secrets))
                (result (guard (e ((and (pair? secrets) (error-object? e))
                                   ;; Raised again without the secrets.
                                   (apply error (error-object-message e)
                                          (map (lambda (x) (if (member x secrets) "<redacted>" x)) (error-object-irritants e)))))
                          (call-in-operation
                           (record-operation r)
                           (lambda ()
                             (let ((result (apply (caddr c) s vals)))
                               ;; Checked in the operation, which fails with it,
                               ;; and redacted where it may hold a secret.
                               (when (and (contract-result contract) (not (equal? principal "user")))
                                 (let ((ps (parameterize ((schema-redacted (pair? secrets)))
                                             (explain (contract-result contract) result))))
                                   (unless (null? ps)
                                     (error (string-append "The result of " (symbol->string name) " is not what its contract says:\n"
                                                           (problems->string ps))))))
                               result))))))
           result))
        (else (error "Not a mode of invocation:" mode))))))

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

(define log-history 128
  "The most revisions the history of a log, as *Messages*, keeps.
A log's text is written by the editor, not edited: what it no longer
shows is not kept for undo either.")

(define %messages #f)

(define (messages-document)
  "Return the document of *Messages*, made the first time."
  (unless %messages
    (let ((d (make-document "")))
      (document-limit-history! d log-history)
      (set! %messages (list d (make-view d "messages") #f 0))))
  (car %messages))

(define (log-message! text)
  (without-operation (lambda () (log-message* text))))

(define (log-message* text)
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

;; The violations kept, at most.
(define %violations-max 20)

(define (failed! s who e)
  "Show the error E that WHO, a command or hook, raised in session S.
A violation is kept for `take-violations!` too, the latest
`%violations-max`."
  (message! s (error-text e))
  (when (violation? e)
    (let ((kept (or (sget s 'violations) '())))
      (sset! s 'violations (cons (string-append (symbol->string who) ": " (error-text e))
                                 (if (< (length kept) %violations-max) kept (take kept (- %violations-max 1))))))))

(define (note-violation! s text)
  (let ((kept (or (sget s 'violations) '())))
    (sset! s 'violations (cons text (if (< (length kept) %violations-max) kept (take kept (- %violations-max 1)))))))

(define (take-violations! s)
  "Return and forget the violations raised in session S since last asked.
They are those of commands and hooks, oldest first, each as \"who: what
it says\"."
  (let ((kept (or (sget s 'violations) '())))
    (sset! s 'violations '())
    (reverse kept)))

;;; Runs that join: a command run again, from a key, right after a run of
;;; it that ended done joins that run's operation, when its join says so
;;; (typing joins as its undo unit does). Commands run by commands never
;;; join.

(define %joins (make-hash-table))

(define (command-joins! name join)
  "Let runs of the command NAME join one operation when JOIN says so.
JOIN is #t, or a procedure (session record) saying whether this run
joins the last, whose record is RECORD."
  (hash-table-set! %joins name join))

;; The operation to run the command NAME, its registry entry C, in, in
;; session S: the last one, which this run joins, or a new one.
(define (command-operation! s name c)
  (let* ((code (command-code-id c))
         (top (not (current-operation)))
         (last (and top (sget s 'last-record)))
         (join (and last (hash-table-ref/default %joins name #f)))
         (r (or (and join
                     (eq? (record-command last) name)
                     (eqv? (record-code last) code)
                     (or (eq? join #t) (join s last))
                     (continue-operation! last s)
                     last)
                (let ((d (view-document (session-view s))))
                  (%start-operation! name s code (document-id d) (document-revision d) (sget s 'current-prefix))))))
    (when top (sset! s 'last-record r))
    (record-operation r)))

(define (run-command s name n #:operation [operation #t])
  "Run the command NAME in session S with the count N.
The run is an operation, a child of the current one, unless OPERATION
is #f: a key of the minibuffer, which only reads, runs in none.
What commands record for the next one (a kill to append to, a goal
column) moves from now to last here; an error becomes the message.
Return #t when the command finished, #f when it raised an error."
  (sset! s 'this-command name)
  (sset! s 'kill-now #f)
  (sset! s 'goal-now #f)
  (message! s #f)
  (let ((ok (guard (e (#t (failed! s name e) #f))
              (let ((c (or (registry-ref %commands name) (error "no such command" name))))
                (cond ((list-ref c 4)
                       ;; Its arguments are read, then it is invoked with them,
                       ;; an operation of its own.
                       (%acquire s name n))
                      (operation
                       (call-in-operation (command-operation! s name c) (lambda () ((caddr c) s n))))
                      (else (without-operation (lambda () ((caddr c) s n))))))
              #t)))
    (sset! s 'last-kill (sget s 'kill-now))
    (unless (sget s 'goal-now) (sset! s 'goal #f))
    (sset! s 'last-command name)
    ok))
