;;; Operations (doc/operations.md): every command run is one, with the
;;; work it holds, until that work has settled. `current-operation` is the
;;; one running code is part of, a parameter: tasks inherit it, and the
;;; callbacks a command leaves (a minibuffer's accept and preview) run in
;;; it again. The natives that commit read it (`operation.rs`), so every
;;; change of a document names the operation it was made in.
;;;
;;; An operation is pending while its procedure runs and while anything it
;;; holds is unfinished: a request made in it, a minibuffer it opened.
;;; Work started for longer (a language server's session) does not hold
;;; it. The first failure, or a cancellation, makes it ending: what it
;;; still holds is cancelled, nothing commits in it, and it ends when that
;;; work has ended. It ends failed, or cancelled; or cancelled too when the
;;; last of its work to end was cancelled (a request superseded); else
;;; done. Failed or cancelled, it is partial when it had made changes.
;;;
;;; Each is recorded: in the trace, every operation, and, once it made a
;;; change or failed, in the kept operations; both bounded, the oldest
;;; forgotten first. Runs of a command that join (typing a word, moving)
;;; are one operation, counting its runs.

(provide current-operation start-operation! %start-operation! start-invocation! current-principal note-effect! continue-operation! call-in-operation operation-procedure without-operation
         operation-hold! operation-release! operation-failed! operation-cancelled!
         operation? operation-serial operation-runtime operation-parent-serial operation-command
         operation-committed? operation-staged? operation-proposals operation-phase operation-outcome operation-pending?
         set-operation-ended-handler! cancelled-condition?
         operation-record-of operation-record? record-operation record-command record-code code-id record-document record-revision
         record-arguments record-unavailable-arguments record-principal record-effects record-contract record-violation
         record-prefix record-runs record-condition record-pending? record-choices record-settled set-record-settled!
         make-choice recovery-choice? choice-label choice-doc choice-command choice-arguments choice-settles?
         condition-choices with-choices error-with-choices
         find-operation-record forget-operation! kept-operation-records pending-operation-records traced-operation-records operation-logs-bytes
         operation-trace-max operation-trace-bytes operations-kept-max operations-kept-bytes)

(define current-operation (make-parameter #f)
  "Return the operation the running code is part of, or #f: a parameter.")
(define %key (%parameter-key current-operation))
(%set-operation-key! %key)

(define (operation? x)
  "Return #t if X is an operation."
  (guard (e (#t #f)) (operation-serial x) #t))

(define (operation-command op)
  "Return the name of the command OP is a run of."
  (string->symbol (%operation-command op)))

(define (operation-phase op)
  "Return whether OP is `open`, `ending` or `ended`.
Ending, it failed or was cancelled, and the work it holds is ending."
  (string->symbol (%operation-phase op)))

(define (operation-outcome op)
  "Return how OP ended: `done`, `failed`, `cancelled` or `partial`.
Return #f while it has not."
  (let ((o (%operation-outcome op))) (and o (string->symbol o))))

;;; The operations not ended, by serial: what holds each, and how it is
;;; ending.

(define-record-type pending
  (make-pending op session record count holds condition cancelled last-cancelled secrets observed)
  pending?
  (op pending-op)
  (session pending-session)
  (record pending-record)
  ;; How many hold it: those of `call-in-operation`, which every key
  ;; takes, are counted only; the others are in HOLDS too.
  (count pending-count set-pending-count!)
  ;; (token . cancel), cancel a thunk stopping that work, or #f.
  (holds pending-holds set-pending-holds!)
  ;; (condition), the first it failed with, or #f: #f can be raised too.
  (condition pending-condition set-pending-condition!)
  (cancelled pending-cancelled set-pending-cancelled!)
  ;; Whether the work released last was cancelled.
  (last-cancelled pending-last-cancelled set-pending-last-cancelled!)
  ;; The values of its sensitive arguments, while it runs: what it fails
  ;; with is written without them.
  (secrets pending-secrets)
  ;; Effects noted as they happened (`note-effect!`): `settings`,
  ;; `external` (a process started).
  (observed pending-observed set-pending-observed!))

(define %pending (make-hash-table))

;; The pending state of OP, this run of it: runs joining one operation
;; share its serial, each an object of its own.
(define (pending-of op)
  (let ((p (hash-table-ref/default %pending (operation-serial op) #f)))
    (and p (eq? (pending-op p) op) p)))

(define (operation-pending? op)
  "Return #t if OP has not ended."
  (and (pending-of op) #t))

;; Called with the session and the operation when one ends.
(define %ended (lambda (s op r) #f))

(define (set-operation-ended-handler! proc)
  "Call (PROC session operation record) when an operation ends."
  (set! %ended proc))

(define (start-operation! command s #:code [code #f] #:document [document #f] #:revision [revision #f] #:prefix [prefix #f]
                          #:parent [parent #f] #:principal [principal "user"])
  "Start an operation of COMMAND, a symbol, in session S, and return it.
It is a child of PARENT, an operation, or else of the current one, run
for PRINCIPAL. It ends when what holds it is released, the first hold
`call-in-operation`'s. CODE is the procedure that runs, DOCUMENT the id
of the document it starts in, at REVISION, and PREFIX its prefix
argument, for its record."
  (let ((start (lambda () (record-operation (%begin-operation! command s (code-id code) document revision prefix principal)))))
    (if parent
        (let ((old (current-operation)))
          (dynamic-wind (lambda () (%task-local-set! %key parent)) start (lambda () (%task-local-set! %key old))))
        (start))))

;; Who each operation runs for, by its object, held as long as it is:
;; work it leaves (a task, a callback) keeps its principal after it ends.
(define %principals (make-weak-hash-table))

;; As `start-operation!`, without keywords, which cost: every key runs it.
;; CODE is the code's number (`code-id`). Return the operation's record.
(define (%start-operation! command s code document revision prefix)
  (%begin-operation! command s code document revision prefix "user"))
(define (%begin-operation! command s code document revision prefix principal)
  (let* ((parent (current-operation))
         (op (%make-operation (symbol->string command) parent))
         (r (make-record op command code document revision prefix 1 #f #f #f #f principal #f #f #f '() #f #f '())))
    ;; A task left after its operation ended still cannot shed staging by
    ;; running an opaque command. A provenance parent alone (acceptance)
    ;; is different from the operation current while code executes.
    (when (and parent (operation-staged? parent)) (%operation-stage! op))
    (hash-table-set! %principals op principal)
    (hash-table-set! %pending (operation-serial op) (make-pending op s r 0 '() #f #f #f '() '()))
    (set-record-traced! r (ring-push! %trace r 96))
    r))

(define (note-effect! effect)
  "Note that the current operation has EFFECT: `settings`, `external`.
What changes text and saves files is noted where it commits; this is
for the rest: a setting changed, a process started. An invocation that
did not declare it has it as a violation."
  (let* ((op (current-operation)) (p (and op (pending-of op))))
    (when (and op (operation-staged? op) (memq effect '(settings external)))
      (error "Refused in stage mode:" effect))
    (when p
      (unless (memq effect (pending-observed p))
        (set-pending-observed! p (cons effect (pending-observed p))))
      (%operation-had-effect! op))))

;; An invocation (`invoke`): an operation of COMMAND with ARGUMENTS as
;; data, by PRINCIPAL, declaring EFFECTS, under the contract of hash
;; CONTRACT, whose sensitive argument values are SECRETS. Return its record.
(define (start-invocation! command s code document revision arguments principal effects contract secrets #:parent [parent #f]
                           #:mode [mode 'apply] #:unavailable [unavailable '()])
  "Start an operation of COMMAND invoked with ARGUMENTS; return its record.
It runs in session S, CODE the number of what runs (`code-id`); DOCUMENT is the
id of the document it starts in, at REVISION. ARGUMENTS are data,
((name . data) ...), sensitive ones redacted; PRINCIPAL invoked it,
declaring EFFECTS, under the contract of hash CONTRACT. SECRETS are the
values of its sensitive arguments, kept out of what its failure says.
It is a child of PARENT, an operation, or else of the current one.
MODE `stage` stages changes; nested operations inherit staging.
UNAVAILABLE names arguments redacted or shortened in the record, which
must be acquired again rather than replayed from incomplete data."
  (let* ((op (%make-operation (symbol->string command) (or parent (current-operation))))
         (r (make-record op command code document revision #f 1 #f #f #f arguments principal effects contract #f '() #f #f unavailable)))
    (when (or (eq? mode 'stage) (and (current-operation) (operation-staged? (current-operation))))
      (%operation-stage! op))
    (hash-table-set! %principals op principal)
    (hash-table-set! %pending (operation-serial op) (make-pending op s r 0 '() #f #f #f secrets '()))
    (set-record-traced! r (ring-push! %trace r (trace-bytes r)))
    r))

(define (current-principal)
  "Return who the current operation runs for: \"user\", an agent's name.
Ended, it runs for whom it ran for: work it left (a task, a callback)
keeps that principal. \"user\" without an operation."
  (let ((op (current-operation)))
    (if op (hash-table-ref/default %principals op "user") "user")))

;; Procedures by a number each, held weakly: a record names the code that
;; ran without keeping it, or what it holds, alive.
(define %code-ids (make-weak-hash-table))
(define %code-count 0)

(define (code-id code)
  "Return the number naming the procedure CODE in records, or #f.
A command redefined runs another procedure, under another number."
  (and code
       (or (hash-table-ref/default %code-ids code #f)
           (begin (set! %code-count (+ 1 %code-count))
                  (hash-table-set! %code-ids code %code-count)
                  %code-count))))

(define (operation-record-of op)
  "Return the record of OP while it is pending, else #f."
  (let ((p (pending-of op))) (and p (pending-record p))))

(define (continue-operation! r s)
  "Run again in the operation of record R, which ended done.
Another run of its command joins it, in session S. Return the
operation, or #f if it did not end done."
  (and (eq? (record-outcome r) 'done)
       ;; A new object of the same operation: what still holds the run
       ;; before sees that run ended.
       (let ((op (%operation-continue (record-operation r))))
         (hash-table-set! %principals op (record-principal r))
         (set-record-operation! r op)
         (set-record-outcome! r #f)
         (set-record-runs! r (+ 1 (record-runs r)))
         (hash-table-set! %pending (operation-serial op) (make-pending op s r 0 '() #f #f #f '() '()))
         op)))

(define (operation-hold! op #:cancel [cancel #f])
  "Keep OP from ending until the token returned is released.
CANCEL, if given, is a thunk stopping the work holding it, called when
OP fails or is cancelled. Return #f, holding nothing, if OP has ended."
  (let ((p (pending-of op)))
    (and p
         (let ((token (list 'hold)))
           (set-pending-holds! p (cons (cons token cancel) (pending-holds p)))
           (set-pending-count! p (+ 1 (pending-count p)))
           token))))

(define (operation-release! op token #:cancelled [cancelled #f])
  "Release the hold TOKEN on OP; it ends when nothing holds it.
CANCELLED says the work holding it was cancelled rather than done."
  (let ((p (pending-of op)))
    (when (and p token (assq token (pending-holds p)))
      (set-pending-holds! p (filter (lambda (h) (not (eq? (car h) token))) (pending-holds p)))
      (release! p cancelled))))

;; One hold of P less: it ends when none is left.
(define (release! p cancelled)
  (set-pending-count! p (- (pending-count p) 1))
  (set-pending-last-cancelled! p cancelled)
  (when (= (pending-count p) 0) (end! p)))

(define (end! p)
  (let* ((op (pending-op p))
         (outcome (cond ((not (or (pending-condition p) (pending-cancelled p) (pending-last-cancelled p))) 'done)
                        ((operation-committed? op) 'partial)
                        ((pending-condition p) 'failed)
                        (else 'cancelled))))
    (hash-table-delete! %pending (operation-serial op))
    (%operation-end! op (case outcome ((done) "done") ((partial) "partial") ((failed) "failed") (else "cancelled")))
    (let ((r (pending-record p)))
      (set-record-outcome! r outcome)
      (when (pending-condition p)
        (set-record-condition! r (condition-text (car (pending-condition p)) (pending-secrets p)))
        (set-record-choices! r (kept-choices (condition-choices (car (pending-condition p))) (pending-secrets p)))
        (when (record-traced r) (ring-recharge! %trace (record-traced r) (trace-bytes r))))
      (let ((v (undeclared r op (pending-observed p))))
        (when v (set-record-violation! r v)))
      (cond ((record-kept r)
             ;; Kept already, by an earlier run joined: what it holds now.
             (ring-recharge! %kept (record-kept r) (record-bytes r)))
            ;; It changed something, failed, or was invoked with arguments.
            ((or (operation-committed? op) (pending-condition p) (record-arguments r))
             (set-record-kept! r (ring-push! %kept r (record-bytes r))))))
    (%ended (pending-session p) op (pending-record p))))

;; Make P ending: nothing commits in it, and what it holds is stopped.
(define (ending! p)
  (%operation-ending! (pending-op p))
  (for-each (lambda (h) (when (cdr h) (guard (e (#t #f)) ((cdr h)))))
            (pending-holds p)))

(define (operation-failed! op condition)
  "Say that OP failed with CONDITION; the first failure is the one kept.
OP is ending from then: what it holds is cancelled."
  (let ((p (pending-of op)))
    (when (and p (not (pending-condition p)) (not (pending-cancelled p)))
      (set-pending-condition! p (list condition))
      (ending! p))))

(define (operation-cancelled! op)
  "Cancel OP, unless it failed already: what it holds is cancelled."
  (let ((p (pending-of op)))
    (when (and p (not (pending-condition p)) (not (pending-cancelled p)))
      (set-pending-cancelled! p #t)
      (ending! p))))

(define (cancelled-condition? e)
  "Return #t if E is the condition a cancelled task ends with."
  (and (error-object? e) (equal? (error-object-message e) "task cancelled")))

(define (call-in-operation op thunk #:fail [fail #t])
  "Call THUNK with OP current, holding OP while it runs.
A condition THUNK raises fails OP (a task's cancellation cancels it),
unless FAIL is #f, and is raised again: a preview failing, say, is
shown, and the reading goes on."
  ;; As (parameterize ((current-operation op)) ...), cheaper: every key
  ;; runs it.
  (let ((p (pending-of op)) (old (current-operation)))
    ;; Counted only: released once, below.
    (when p (set-pending-count! p (+ 1 (pending-count p))))
    (dynamic-wind
     (lambda () (%task-local-set! %key op))
     (lambda ()
       (if fail
           (guard (e (#t (if (cancelled-condition? e) (operation-cancelled! op) (operation-failed! op e))
                         (raise e)))
             (thunk))
           (thunk)))
     (lambda ()
       (%task-local-set! %key old)
       (when p (release! p #f))))))

(define (operation-procedure proc #:fail [fail #t])
  "Return PROC made to run in the operation current now.
Return PROC itself when there is none, or PROC is #f. FAIL is as
`call-in-operation` takes it."
  (let ((op (current-operation)))
    (if (and op proc)
        (lambda args (call-in-operation op (lambda () (apply proc args)) #:fail fail))
        proc)))

(define (without-operation thunk)
  "Call THUNK in no operation: what it commits names none.
For the editor's own text (*Messages*), which no command changes."
  (let ((old (current-operation)))
    (if old
        (dynamic-wind (lambda () (%task-local-set! %key #f)) thunk (lambda () (%task-local-set! %key old)))
        (thunk))))

;;; Recovery choices (doc/operations.md, "Recovery"): what a failure
;;; offers to do next, as data, kept on the operation that failed once it
;;; has ended. Choosing one invokes a command with arguments, checked as
;;; any invocation is then (recovery.scm).

(define-record-type recovery-choice
  (%make-choice label doc command arguments settles)
  recovery-choice?
  (label choice-label)
  (doc choice-doc)
  (command choice-command)
  ;; A dict of data, as `invoke` takes arguments.
  (arguments choice-arguments)
  (settles choice-settles?))

(define (make-choice label command arguments #:doc [doc ""] #:settles [settles #t])
  "Return a recovery choice, LABEL: invoking COMMAND with ARGUMENTS.
ARGUMENTS is a dict of data, as `invoke` takes them, which say what the
choice expects (a document as of a revision): invoked, they are checked
against the state then, and refused if stale. DOC says what it does. A
choice that only shows something (comparing) does not settle the
failure: SETTLES #f."
  (%make-choice label doc command (->data* arguments) settles))

;; ARGUMENTS as data: a dict of strings, numbers, symbols, dicts, lists.
(define (->data* x)
  (cond ((dict? x) (fold (lambda (k d) (dict-set d k (->data* (dict-ref x k)))) (dict) (dict-keys x)))
        ((list? x) (map ->data* x))
        ((or (string? x) (number? x) (symbol? x) (boolean? x) (null? x)) x)
        (else (error "make-choice: not data, a choice's arguments must be:" x))))

;; Conditions' choices, held as long as the condition is.
(define %choices (make-weak-hash-table))

(define (condition-choices e)
  "Return the recovery choices the condition E offers, a list."
  (hash-table-ref/default %choices e '()))

(define (with-choices e choices)
  "Make the condition E offer CHOICES, recovery choices; return E."
  (unless (null? choices) (hash-table-set! %choices e choices))
  e)

(define (error-with-choices choices message . irritants)
  "Raise an error of MESSAGE and IRRITANTS offering CHOICES.
CHOICES are recovery choices (`make-choice`): the operation it fails
keeps them, for the user, or who it ran for, to choose from."
  (raise (with-choices (guard (e (#t e)) (apply error message irritants)) choices)))

;;; Records

(define-record-type operation-record
  (make-record op command code document revision prefix runs condition kept outcome arguments principal effects contract violation
               choices settled traced unavailable)
  operation-record?
  ;; The operation, the object of its last run when runs joined it.
  (op record-operation set-record-operation!)
  ;; Its command's name, as `operation-command` has it.
  (command record-command)
  ;; The number of the procedure that ran (`code-id`): a command
  ;; redefined since runs another.
  (code record-code)
  ;; The id of the document it started in (`document-id`), and its
  ;; revision then.
  (document record-document)
  (revision record-revision)
  (prefix record-prefix)
  ;; How many runs of its command it is (typing a word: its letters).
  (runs record-runs set-record-runs!)
  ;; What it failed with, written, or #f.
  (condition record-condition set-record-condition!)
  ;; Its entry in the kept operations, or #f.
  (kept record-kept set-record-kept!)
  ;; How it ended, as `operation-outcome` has it, once it has.
  (outcome record-outcome set-record-outcome!)
  ;; Invoked with a contract (doc/operations.md, "Contracts"): its
  ;; arguments as data, ((name . data) ...), sensitive ones redacted; who
  ;; invoked it; the effects it declared for them; its contract's hash;
  ;; else #f, "user", #f, #f.
  (arguments record-arguments)
  (principal record-principal)
  (effects record-effects)
  (contract record-contract)
  ;; Effects it had that it did not declare, said, or #f.
  (violation record-violation set-record-violation!)
  ;; The recovery choices its failure offers.
  ;; Who settled it.
  ;; #f, the serial of the operation choosing (running or done), or
  ;; `dismissed`.
  (choices record-choices set-record-choices!)
  (settled record-settled %set-record-settled!)
  ;; Its entry in the trace.
  (traced record-traced set-record-traced!)
  ;; Redacted, shortened, non-data or absent: never replay these values.
  (unavailable record-unavailable-arguments))

(define (set-record-settled! r by)
  "Say who settled the failure of R's operation: BY.
BY is the serial of the operation choosing (running or done),
`dismissed`, a claim (a pair) while a choice of it is checked, or #f:
undecided again."
  (%set-record-settled! r by))

(define (record-pending? r)
  "Return #t if the operation of record R has not ended."
  (operation-pending? (record-operation r)))

;; What a failure says, written briefly: no live condition is kept, and
;; none of SECRETS, values of sensitive arguments, among its irritants.
(define (condition-text e [secrets '()])
  (let ((s (cond ((error-object? e)
                  (call-with-output-string
                   (lambda (p)
                     (display (error-object-message e) p)
                     (for-each (lambda (x) (display " " p) (if (member x secrets) (display "<redacted>" p) (write x p)))
                               (error-object-irritants e)))))
                 ((string? e) e)
                 (else (call-with-output-string (lambda (p) (write e p)))))))
    (if (> (string-length s) 300) (string-append (substring s 0 297) "...") s)))

;; What record R's operation did that it did not declare, said, or #f:
;; only of an invocation, whose contract declares its effects.
(define (undeclared r op observed)
  (let ((declared (record-effects r)))
    (and (list? declared)
         (let ((did (delete-duplicates
                     (append (if (or (> (operation-change-total op) 0) (pair? (operation-proposals op))) '(edit) '())
                             (if (pair? (operation-saves op)) '(external) '())
                             observed))))
           (let ((missing (remove (lambda (e) (memq e declared)) did)))
             (and (pair? missing)
                  (string-append "it had effects it does not declare: "
                                 (string-join (map symbol->string missing) ", "))))))))

;; What the trace is charged for record R: its arguments and choices,
;; which it holds as long as the kept operations do.
(define (trace-bytes r)
  (+ 96 (if (record-arguments r) (data-bytes (record-arguments r)) 0)
     (if (pair? (record-unavailable-arguments r)) (data-bytes (record-unavailable-arguments r)) 0)
     (choices-bytes (record-choices r))))

(define (choice-bytes c)
  (+ 64 (string-length (choice-label c)) (string-length (choice-doc c)) (data-bytes (choice-arguments c))))

(define (choices-bytes cs) (fold + 0 (map choice-bytes cs)))

;; The most choices a record keeps, and the most bytes one may hold.
(define %choices-max 8)
(define %choice-bytes-max 2048)

;; Of CHOICES, those a record keeps: at most `%choices-max`, each under
;; `%choice-bytes-max`, none holding one of SECRETS, values of the
;; operation's sensitive arguments, which no record holds. Such a choice
;; is not offered: to run it again, those are asked for again.
(define (kept-choices choices secrets)
  (let ((ok (filter (lambda (c)
                      (and (<= (choice-bytes c) %choice-bytes-max)
                           (not (holds-any? (choice-arguments c) secrets))))
                    choices)))
    (if (> (length ok) %choices-max) (take ok %choices-max) ok)))

;; Whether the data X holds one of VALUES, anywhere in it.
(define (holds-any? x values)
  (and (pair? values)
       (let walk ((x x))
         (cond ((member x values) #t)
               ((dict? x) (any (lambda (k) (or (walk k) (walk (dict-ref x k)))) (dict-keys x)))
               ((pair? x) (or (walk (car x)) (walk (cdr x))))
               ((vector? x) (any walk (vector->list x)))
               (else #f)))))

;; About the bytes data holds: what writing it takes. A record's
;; arguments are bounded data (`->data`).
(define (data-bytes x)
  (string-length (call-with-output-string (lambda (p) (write x p)))))

;; About what a kept record holds, in bytes: at most some 16 KiB, as what
;; an operation records is bounded (`operation.rs`).
(define (record-bytes r)
  (+ 160
     (if (pair? (record-unavailable-arguments r)) (data-bytes (record-unavailable-arguments r)) 0)
     (if (record-condition r) (string-length (record-condition r)) 0)
     (if (record-arguments r) (data-bytes (record-arguments r)) 0)
     (choices-bytes (record-choices r))
     (%operation-recorded-bytes (record-operation r))))

;;; A ring: the newest items, at most (MAX) of them and (MAX-BYTES) of
;;; what each is said to hold, the oldest forgotten first: a vector used as
;;; a queue, growing to its bound.

(define-record-type ring
  (%make-ring items head count bytes max max-bytes evicted keep)
  ring?
  ;; (item . bytes), oldest at HEAD.
  (items ring-items set-ring-items!)
  (head ring-head set-ring-head!)
  (count ring-count set-ring-count!)
  (bytes ring-bytes set-ring-bytes!)
  (max ring-max)
  (max-bytes ring-max-bytes)
  ;; (item) -> what forgetting an item does.
  (evicted ring-evicted)
  ;; (item) -> whether eviction must spare it for now: an item it holds
  ;; stays past the bounds until whatever holds it lets go.
  (keep ring-keep))

(define (make-ring max max-bytes evicted #:keep [keep (lambda (item) #f)])
  (%make-ring (make-vector 64 #f) 0 0 0 max max-bytes evicted keep))

(define operation-trace-max 4096
  "The most operations the trace keeps: every command run, newest kept.")
(define operation-trace-bytes (* 512 1024)
  "About the most bytes the trace's records hold.")
(define operations-kept-max 1000
  "The most operations kept once they changed something or failed.
A record whose staged proposals are still pending is spared, past this
bound; forgetting such a record rejects the proposals.")
(define operations-kept-bytes (* 4 1024 1024)
  "About the most bytes the kept operations' records hold.
A record whose staged proposals are still pending is spared, past this
bound; forgetting such a record rejects the proposals.")

(define %trace (make-ring (lambda () operation-trace-max) (lambda () operation-trace-bytes) (lambda (r) #f)))

(define (record-has-pending-proposals? r)
  "Return #t while a proposal of record R's operation waits undecided.
The record is those proposals' review path, so eviction spares it:
unrelated operations cannot silently discard staged work. How many can
be spared is bounded by the proposal table's own bound. Forgetting the
record (`forget-operation!`) remains the way to reject them."
  (any (lambda (id) (guard (e (#t #f)) (equal? (proposal-state id) "pending")))
       (operation-proposals (record-operation r))))

;; A record no longer kept forgets what its operation changed: only the
;; trace, which keeps no more than the record, may still list it. A
;; record whose proposals were decided or forgotten has none pending;
;; one forgotten on purpose has its pending proposals rejected before
;; their identities go, not orphaned in the proposal table where they
;; would consume capacity without a review path.
(define %kept
  (make-ring (lambda () operations-kept-max) (lambda () operations-kept-bytes)
             (lambda (r)
               (set-record-kept! r #f)
               (for-each (lambda (id)
                           ;; Applied, rejected or long forgotten: nothing
                           ;; left to discard. Cleanup is idempotent.
                           (guard (e (#t #f)) (proposal-reject! id)))
                         (operation-proposals (record-operation r)))
               (%operation-forget-effects! (record-operation r)))
             #:keep record-has-pending-proposals?))

(define (ring-ref ring i)
  (let ((v (ring-items ring)))
    (vector-ref v (modulo (+ (ring-head ring) i) (vector-length v)))))

(define (ring-push! ring item bytes)
  (when (= (ring-count ring) (vector-length (ring-items ring)))
    ;; Grow, keeping the order.
    (let ((w (make-vector (* 2 (ring-count ring)) #f)))
      (do ((i 0 (+ i 1))) ((= i (ring-count ring))) (vector-set! w i (ring-ref ring i)))
      (set-ring-items! ring w)
      (set-ring-head! ring 0)))
  (let ((v (ring-items ring)) (entry (cons item bytes)))
    (vector-set! v (modulo (+ (ring-head ring) (ring-count ring)) (vector-length v)) entry)
    (set-ring-count! ring (+ 1 (ring-count ring)))
    (set-ring-bytes! ring (+ (ring-bytes ring) bytes))
    (ring-evict! ring)
    ;; #f when it was forgotten at once (a bound of 0).
    (and (cdr entry) entry)))

;; Forget RING's I'th oldest item, 0 the oldest: the common case, 0,
;; moves only the head; a later one (the oldest spared) shifts the rest.
(define (ring-forget-at! ring i)
  (let* ((v (ring-items ring)) (n (vector-length v))
         (slot (lambda (j) (modulo (+ (ring-head ring) j) n)))
         (entry (vector-ref v (slot i))))
    (if (= i 0)
        (begin (vector-set! v (ring-head ring) #f)
               (set-ring-head! ring (modulo (+ 1 (ring-head ring)) n)))
        (begin
          (do ((j i (+ j 1))) ((= j (- (ring-count ring) 1)))
            (vector-set! v (slot j) (vector-ref v (slot (+ j 1)))))
          (vector-set! v (slot (- (ring-count ring) 1)) #f)))
    (set-ring-count! ring (- (ring-count ring) 1))
    (set-ring-bytes! ring (- (ring-bytes ring) (cdr entry)))
    ;; An entry forgotten says so, for whoever holds it.
    (set-cdr! entry #f)
    ((ring-evicted ring) (car entry))))

;; Forget the oldest items of RING while it holds more than its bounds,
;; sparing any its keep holds: with everything held it stays past them.
(define (ring-evict! ring)
  (let ((max ((ring-max ring))) (max-bytes ((ring-max-bytes ring))))
    (let loop ()
      (when (and (> (ring-count ring) 0) (or (> (ring-count ring) max) (> (ring-bytes ring) max-bytes)))
        (let seek ((i 0))
          (cond ((= i (ring-count ring)) #f)
                (((ring-keep ring) (car (ring-ref ring i))) (seek (+ i 1)))
                (else (ring-forget-at! ring i) (loop))))))))

;; Charge ENTRY of RING with BYTES now, its item having grown.
(define (ring-recharge! ring entry bytes)
  (when (cdr entry)
    (set-ring-bytes! ring (+ (ring-bytes ring) (- bytes (cdr entry))))
    (set-cdr! entry bytes)
    (ring-evict! ring)))

;; The items of RING, newest first.
(define (ring-items-newest ring)
  (let loop ((i 0) (acc '()))
    (if (= i (ring-count ring)) acc (loop (+ i 1) (cons (car (ring-ref ring i)) acc)))))

(define (kept-operation-records)
  "Return the records of the operations kept, newest first.
They are those that made a change or failed, as far back as their
bounds (`operations-kept-max`) let them be kept."
  (ring-items-newest %kept))

(define (traced-operation-records)
  "Return the records of the operations in the trace, newest first."
  (ring-items-newest %trace))

(define (pending-operation-records)
  "Return the records of the operations not ended, newest first.
They are listed from the work holding them, kept in the logs or not."
  (sort (map pending-record (hash-table-values %pending))
        (lambda (a b) (> (operation-serial (record-operation a)) (operation-serial (record-operation b))))))

;; Forget the items of RING that PRED is true of, as eviction forgets
;; them; return how many.
(define (ring-remove! ring pred)
  (let* ((entries (let loop ((i (- (ring-count ring) 1)) (acc '()))
                    (if (< i 0) acc (loop (- i 1) (cons (ring-ref ring i) acc)))))
         (gone (filter (lambda (e) (pred (car e))) entries))
         (staying (remove (lambda (e) (pred (car e))) entries))
         (v (make-vector (vector-length (ring-items ring)) #f)))
    (do ((rest staying (cdr rest)) (i 0 (+ i 1))) ((null? rest)) (vector-set! v i (car rest)))
    (set-ring-items! ring v)
    (set-ring-head! ring 0)
    (set-ring-count! ring (length staying))
    (for-each (lambda (e)
                (set-ring-bytes! ring (- (ring-bytes ring) (cdr e)))
                (set-cdr! e #f)
                ((ring-evicted ring) (car e)))
              gone)
    (length gone)))

(define (forget-operation! r)
  "Forget the operation of record R: its record leaves the logs.
Its changes still name it, by number; asked for, it is forgotten.
Any pending proposals it owns are rejected, without editing their
sources. A pending operation is not forgotten: it is an error."
  (when (record-pending? r) (error "forget-operation: it is still pending" (operation-serial (record-operation r))))
  (let ((it? (lambda (x) (eq? x r))))
    (> (+ (ring-remove! %kept it?) (ring-remove! %trace it?)) 0)))

(define (find-operation-record serial)
  "Return the record of the operation SERIAL of this runtime, or #f.
#f when it is forgotten: past the bounds of the logs, ended."
  (let ((p (hash-table-ref/default %pending serial #f)))
    (if p
        (pending-record p)
        (let ((found (lambda (r) (= (operation-serial (record-operation r)) serial))))
          (or (find found (kept-operation-records)) (find found (traced-operation-records)))))))

(define (operation-logs-bytes)
  "Return about how many bytes the logs' records hold: (trace kept)."
  (list (ring-bytes %trace) (ring-bytes %kept)))
