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

(provide current-operation start-operation! call-in-operation operation-procedure without-operation
         operation-hold! operation-release! operation-failed! operation-cancelled!
         operation? operation-serial operation-runtime operation-parent-serial operation-command
         operation-committed? operation-phase operation-outcome operation-pending?
         set-operation-ended-handler! cancelled-condition?)

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
  (make-pending op session holds condition cancelled last-cancelled)
  pending?
  (op pending-op)
  (session pending-session)
  ;; (token . cancel), cancel a thunk stopping that work, or #f.
  (holds pending-holds set-pending-holds!)
  ;; (condition), the first it failed with, or #f: #f can be raised too.
  (condition pending-condition set-pending-condition!)
  (cancelled pending-cancelled set-pending-cancelled!)
  ;; Whether the work released last was cancelled.
  (last-cancelled pending-last-cancelled set-pending-last-cancelled!))

(define %pending (make-hash-table))

(define (pending-of op) (hash-table-ref/default %pending (operation-serial op) #f))

(define (operation-pending? op)
  "Return #t if OP has not ended."
  (and (pending-of op) #t))

;; Called with the session and the operation when one ends.
(define %ended (lambda (s op) #f))

(define (set-operation-ended-handler! proc)
  "Call (PROC session operation) when an operation ends."
  (set! %ended proc))

(define (start-operation! command s)
  "Start an operation of COMMAND, a symbol, in session S, and return it.
It is a child of the current operation. It ends when what holds it is
released, the first hold `call-in-operation`'s."
  (let ((op (%make-operation (symbol->string command) (current-operation))))
    (hash-table-set! %pending (operation-serial op) (make-pending op s '() #f #f #f))
    op))

(define (operation-hold! op #:cancel [cancel #f])
  "Keep OP from ending until the token returned is released.
CANCEL, if given, is a thunk stopping the work holding it, called when
OP fails or is cancelled. Return #f, holding nothing, if OP has ended."
  (let ((p (pending-of op)))
    (and p
         (let ((token (list 'hold)))
           (set-pending-holds! p (cons (cons token cancel) (pending-holds p)))
           token))))

(define (operation-release! op token #:cancelled [cancelled #f])
  "Release the hold TOKEN on OP; it ends when nothing holds it.
CANCELLED says the work holding it was cancelled rather than done."
  (let ((p (pending-of op)))
    (when (and p token (assq token (pending-holds p)))
      (set-pending-holds! p (filter (lambda (h) (not (eq? (car h) token))) (pending-holds p)))
      (set-pending-last-cancelled! p cancelled)
      (when (null? (pending-holds p)) (end! p)))))

(define (end! p)
  (let* ((op (pending-op p))
         (outcome (cond ((not (or (pending-condition p) (pending-cancelled p) (pending-last-cancelled p))) 'done)
                        ((operation-committed? op) 'partial)
                        ((pending-condition p) 'failed)
                        (else 'cancelled))))
    (hash-table-delete! %pending (operation-serial op))
    (%operation-end! op (symbol->string outcome))
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
  (let ((token (operation-hold! op)) (old (current-operation)))
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
       (operation-release! op token)))))

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
