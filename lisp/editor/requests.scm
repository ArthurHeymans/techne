;;; Requests: work whose result is wanted only while nothing newer is (a
;;; view's rows from a process, candidates from a service). A request slot
;;; holds at most one request in flight: making another cancels the one
;;; before, so a result never lands after a newer one, and a slow source
;;; never queues up work. The request runs in a task owned by the scope it
;;; is made in, so unloading the package that made it cancels it too.
;;;
;;; What is delivered is decided when the result is there, not when it was
;;; asked for: a delivery whose request is no longer the latest is dropped.
;;;
;;; A request holds the operation it is made in (operations.scm) until its
;;; task ends: a request that fails fails it, and it ending (another of its
;;; work failing) cancels the request. A request cancelled (superseded, its
;;; scope shut) is work of it cancelled: the operation ends cancelled when
;;; that was the last of its work to end.

(require "operations.scm")

(provide make-request-slot request! request-pending? cancel-request!)

(define-record-type request-slot
  (%make-request-slot task serial release)
  request-slot?
  ;; The task of the request in flight, or #f.
  (task slot-task set-slot-task!)
  ;; Counts requests made, so a late delivery can tell it is stale.
  (serial slot-serial set-slot-serial!)
  ;; (thunk) stopping the request in flight and releasing the operation
  ;; it holds, or #f.
  (stop slot-stop set-slot-stop!))

(define (make-request-slot)
  "Return a new request slot, with no request in flight."
  (%make-request-slot #f 0 #f))

;; A cancelled task goes on ending: its cancellation is not a failure.
(define cancelled? cancelled-condition?)

(define (request-pending? slot)
  "Return #t if SLOT has a request in flight.
A request whose task ended without delivering, cancelled with the scope
it belongs to, say, is not."
  (let ((t (slot-task slot))) (and t (not (task-done? t)))))

(define (cancel-request! slot)
  "Cancel the request in flight in SLOT, if any.
Its result is never delivered."
  (let ((t (slot-task slot)) (stop (slot-stop slot)))
    (set-slot-serial! slot (+ 1 (slot-serial slot)))
    (set-slot-task! slot #f)
    (set-slot-stop! slot #f)
    (if stop
        (stop)
        (when (and t (not (task-done? t))) (task-cancel t)))))

;; A hold of OP for the request whose task (TASK) returns, and two
;; procedures: (release cancelled?), which the task calls as it ends, and
;; (stop), which cancels the task. The hold is released once: by the task
;; as it ends, its cleanups run, or, for a task cancelled before it ran
;; (which ends running nothing), by whoever stopped it: the request
;; cancelled, OP ending, or the scope owning the task shutting.
(define (hold-for-request! op task)
  (let* ((owned (list 'request))
         (hold #f)
         (released #f)
         (release (lambda (cancelled shut)
                    (unless released
                      (set! released #t)
                      (unless shut (scope-disown! owned))
                      (operation-release! op hold #:cancelled cancelled))))
         ;; SHUT: the scope shuts, and cancelled the task already.
         (stop (lambda (shut)
                 (let ((t (task)))
                   (when (and t (not (task-done? t)))
                     ;; The task itself, ending its own operation, cannot
                     ;; be cancelled: it releases as it ends.
                     (guard (e (#t #f)) (task-cancel t)))
                   (when (or (not t) (task-done? t)) (release #t shut))))))
    (set! hold (operation-hold! op #:cancel (lambda () (stop #f))))
    (scope-own! owned (lambda (r) (stop #t)))
    (values (lambda (cancelled) (release cancelled #f)) (lambda () (stop #f)))))

(define (request! slot produce deliver #:fail [fail #f])
  "Run (PRODUCE) in a task, cancelling the request SLOT had in flight.
When it returns, call (DELIVER result), unless a newer request was made
in SLOT since. When PRODUCE raises, call (FAIL condition) instead, if
given. The task belongs to the current scope."
  (cancel-request! slot)
  (let*-values (((n) (slot-serial slot))
                ((op) (current-operation))
                ((task) #f)
                ((release stop) (if op (hold-for-request! op (lambda () task)) (values #f #f))))
    (set! task
          (spawn (lambda ()
                   (let ((finished #f))
                     (dynamic-wind
                      (lambda () #f)
                      (lambda ()
                        (guard (e ((and op (not (cancelled? e))) (operation-failed! op e) (raise e)))
                          (let ((outcome (guard (e ((not (cancelled? e)) (cons 'failed e))) (cons 'done (produce)))))
                            (when (and op (eq? (car outcome) 'failed)) (operation-failed! op (cdr outcome)))
                            (when (= n (slot-serial slot))
                              (set-slot-task! slot #f)
                              (set-slot-stop! slot #f)
                              (cond ((eq? (car outcome) 'done) (deliver (cdr outcome)))
                                    (fail (fail (cdr outcome)))))
                            (set! finished #t))))
                      (lambda () (when release (release (not finished)))))))))
    (set-slot-task! slot task)
    (set-slot-stop! slot stop)
    slot))
