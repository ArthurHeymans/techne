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

(provide current-operation start-operation! %start-operation! continue-operation! call-in-operation operation-procedure without-operation
         operation-hold! operation-release! operation-failed! operation-cancelled!
         operation? operation-serial operation-runtime operation-parent-serial operation-command
         operation-committed? operation-phase operation-outcome operation-pending?
         set-operation-ended-handler! cancelled-condition?
         operation-record-of operation-record? record-operation record-command record-code code-id record-document record-revision
         record-prefix record-runs record-condition record-pending?
         find-operation-record kept-operation-records pending-operation-records traced-operation-records operation-logs-bytes
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
  (make-pending op session record count holds condition cancelled last-cancelled)
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
  (last-cancelled pending-last-cancelled set-pending-last-cancelled!))

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
(define %ended (lambda (s op) #f))

(define (set-operation-ended-handler! proc)
  "Call (PROC session operation) when an operation ends."
  (set! %ended proc))

(define (start-operation! command s #:code [code #f] #:document [document #f] #:revision [revision #f] #:prefix [prefix #f])
  "Start an operation of COMMAND, a symbol, in session S, and return it.
It is a child of the current operation. It ends when what holds it is
released, the first hold `call-in-operation`'s. CODE is the procedure
that runs, DOCUMENT the id of the document it starts in, at REVISION,
and PREFIX its prefix argument, for its record."
  (record-operation (%start-operation! command s code document revision prefix)))

;; As `start-operation!`, without keywords, which cost: every key runs it.
;; Return the operation's record.
(define (%start-operation! command s code document revision prefix)
  (let* ((op (%make-operation (symbol->string command) (current-operation)))
         (r (make-record op command (code-id code) document revision prefix 1 #f #f #f)))
    (hash-table-set! %pending (operation-serial op) (make-pending op s r 0 '() #f #f #f))
    (ring-push! %trace r 96)
    r))

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
         (set-record-operation! r op)
         (set-record-outcome! r #f)
         (set-record-runs! r (+ 1 (record-runs r)))
         (hash-table-set! %pending (operation-serial op) (make-pending op s r 0 '() #f #f #f))
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
      (when (pending-condition p) (set-record-condition! r (condition-text (car (pending-condition p)))))
      (cond ((record-kept r)
             ;; Kept already, by an earlier run joined: what it holds now.
             (ring-recharge! %kept (record-kept r) (record-bytes r)))
            ((or (operation-committed? op) (pending-condition p))
             (set-record-kept! r (ring-push! %kept r (record-bytes r))))))
    (%ended (pending-session p) op)))

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

;;; Records

(define-record-type operation-record
  (make-record op command code document revision prefix runs condition kept outcome)
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
  (outcome record-outcome set-record-outcome!))

(define (record-pending? r)
  "Return #t if the operation of record R has not ended."
  (operation-pending? (record-operation r)))

;; What a failure says, written briefly: no live condition is kept.
(define (condition-text e)
  (let ((s (cond ((error-object? e)
                  (call-with-output-string
                   (lambda (p)
                     (display (error-object-message e) p)
                     (for-each (lambda (x) (display " " p) (write x p)) (error-object-irritants e)))))
                 ((string? e) e)
                 (else (call-with-output-string (lambda (p) (write e p)))))))
    (if (> (string-length s) 300) (string-append (substring s 0 297) "...") s)))

;; About what a kept record holds, in bytes: at most some 16 KiB, as what
;; an operation records is bounded (`operation.rs`).
(define (record-bytes r)
  (+ 160
     (if (record-condition r) (string-length (record-condition r)) 0)
     (%operation-recorded-bytes (record-operation r))))

;;; A ring: the newest items, at most (MAX) of them and (MAX-BYTES) of
;;; what each is said to hold, the oldest forgotten first: a vector used as
;;; a queue, growing to its bound.

(define-record-type ring
  (%make-ring items head count bytes max max-bytes evicted)
  ring?
  ;; (item . bytes), oldest at HEAD.
  (items ring-items set-ring-items!)
  (head ring-head set-ring-head!)
  (count ring-count set-ring-count!)
  (bytes ring-bytes set-ring-bytes!)
  (max ring-max)
  (max-bytes ring-max-bytes)
  ;; (item) -> what forgetting an item does.
  (evicted ring-evicted))

(define (make-ring max max-bytes evicted) (%make-ring (make-vector 64 #f) 0 0 0 max max-bytes evicted))

(define operation-trace-max 4096
  "The most operations the trace keeps: every command run, newest kept.")
(define operation-trace-bytes (* 512 1024)
  "About the most bytes the trace's records hold.")
(define operations-kept-max 1000
  "The most operations kept once they changed something or failed.")
(define operations-kept-bytes (* 4 1024 1024)
  "About the most bytes the kept operations' records hold.")

(define %trace (make-ring (lambda () operation-trace-max) (lambda () operation-trace-bytes) (lambda (r) #f)))
;; A record no longer kept forgets what its operation changed: only the
;; trace, which keeps no more than the record, may still list it.
(define %kept
  (make-ring (lambda () operations-kept-max) (lambda () operations-kept-bytes)
             (lambda (r)
               (set-record-kept! r #f)
               (%operation-forget-effects! (record-operation r)))))

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

;; Forget the oldest items of RING while it holds more than its bounds.
(define (ring-evict! ring)
  (let ((v (ring-items ring)) (max ((ring-max ring))) (max-bytes ((ring-max-bytes ring))))
    (let loop ()
      (when (and (> (ring-count ring) 0) (or (> (ring-count ring) max) (> (ring-bytes ring) max-bytes)))
        (let ((oldest (vector-ref v (ring-head ring))))
          (vector-set! v (ring-head ring) #f)
          (set-ring-head! ring (modulo (+ 1 (ring-head ring)) (vector-length v)))
          (set-ring-count! ring (- (ring-count ring) 1))
          (set-ring-bytes! ring (- (ring-bytes ring) (cdr oldest)))
          ;; An entry forgotten says so, for whoever holds it.
          (set-cdr! oldest #f)
          ((ring-evicted ring) (car oldest))
          (loop))))))

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
