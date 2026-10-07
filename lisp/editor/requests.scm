;;; Requests: work whose result is wanted only while nothing newer is (a
;;; view's rows from a process, candidates from a service). A request slot
;;; holds at most one request in flight: making another cancels the one
;;; before, so a result never lands after a newer one, and a slow source
;;; never queues up work. The request runs in a task owned by the scope it
;;; is made in, so unloading the package that made it cancels it too.
;;;
;;; What is delivered is decided when the result is there, not when it was
;;; asked for: a delivery whose request is no longer the latest is dropped.

(provide make-request-slot request! request-pending? cancel-request!)

(define-record-type request-slot
  (%make-request-slot task serial)
  request-slot?
  ;; The task of the request in flight, or #f.
  (task slot-task set-slot-task!)
  ;; Counts requests made, so a late delivery can tell it is stale.
  (serial slot-serial set-slot-serial!))

(define (make-request-slot)
  "Return a new request slot, with no request in flight."
  (%make-request-slot #f 0))

;; A cancelled task goes on ending: its cancellation is not a failure.
(define (cancelled? e)
  (and (error-object? e) (equal? (error-object-message e) "task cancelled")))

(define (request-pending? slot)
  "Return #t if SLOT has a request in flight."
  (and (slot-task slot) #t))

(define (cancel-request! slot)
  "Cancel the request in flight in SLOT, if any.
Its result is never delivered."
  (let ((t (slot-task slot)))
    (set-slot-serial! slot (+ 1 (slot-serial slot)))
    (set-slot-task! slot #f)
    (when (and t (not (task-done? t))) (task-cancel t))))

(define (request! slot produce deliver #:fail [fail #f])
  "Run (PRODUCE) in a task, cancelling the request SLOT had in flight.
When it returns, call (DELIVER result), unless a newer request was made
in SLOT since. When PRODUCE raises, call (FAIL condition) instead, if
given. The task belongs to the current scope."
  (cancel-request! slot)
  (let ((n (slot-serial slot)))
    (set-slot-task! slot
                    (spawn (lambda ()
                             (let ((outcome (guard (e ((not (cancelled? e)) (cons 'failed e))) (cons 'done (produce)))))
                               (when (= n (slot-serial slot))
                                 (set-slot-task! slot #f)
                                 (cond ((eq? (car outcome) 'done) (deliver (cdr outcome)))
                                       (fail (fail (cdr outcome)))))))))
    slot))
