;;; Exception handlers run as frames of the raising task: they can wait,
;;; and a handler that waits still has the live restarts of the raise.

(test-begin "handlers")

(define (wait-then v) (sleep 1) v)

;; raise-continuable: the handler's value is the raise's, after a wait.
(test 43 (task-join (spawn (lambda ()
  (with-exception-handler (lambda (c) (wait-then 42))
    (lambda () (+ 1 (raise-continuable 'x))))))))

;; A VM error and a raise that cannot continue: the handler waits, then
;; invokes a restart of the raise.
(define (risky x)
  (restart-case (+ 1 (car x))
    (use-value (v) v)))
(test 7 (task-join (spawn (lambda ()
  (with-exception-handler (lambda (c) (sleep 1) (invoke-restart 'use-value 7))
    (lambda () (risky 5)))))))
(test 'from-handler (task-join (spawn (lambda ()
  (handler-bind ((symbol? (lambda (c) (sleep 1) (invoke-restart 'use-value 'from-handler))))
    (restart-case (raise 'oops) (use-value (v) v)))))))

;; Two tasks wait in their handlers for a choice while a third finishes;
;; a redefinition made meanwhile is what `retry` runs.
(define (compute n) (if (> n 1) (error "too big" n) n))
(define (attempt n)
  (let loop ()
    (restart-case (compute n)
      (retry () (loop))
      (use-value (v) v))))
(define waiting (make-channel 2))
(define (debugged n choices)
  (spawn (lambda ()
    (with-exception-handler
     (lambda (c)
       (channel-send waiting n)
       (apply invoke-restart (channel-recv choices)))
     (lambda () (attempt n))))))
(define a-choice (make-channel 1))
(define b-choice (make-channel 1))
(define a (debugged 2 a-choice))
(define b (debugged 3 b-choice))
(define c (spawn (lambda () (wait-then 'third))))
(test '(2 3) (list-sort < (list (channel-recv waiting) (channel-recv waiting))))
(test 'third (task-join c))
(set! compute (lambda (n) (* n 10)))
(channel-send a-choice '(retry))
(channel-send b-choice '(use-value -1))
(test '(20 -1) (list (task-join a) (task-join b)))

;; A handler returning from a raise that cannot continue: the handlers
;; outside it hear of it, and the handler itself is not called again.
(define calls 0)
(test "exception handler returned from non-continuable raise: uncaught exception: inner"
      (task-join (spawn (lambda ()
        (guard (e (#t (condition/report-string e)))
          (with-exception-handler (lambda (c) (set! calls (+ calls 1)) (sleep 1) 'ignored)
            (lambda () (raise 'inner))))))))
(test 1 calls)

;; What a handler raises goes to the handlers outside it, which also run
;; as frames and can wait.
(test '(outer inner) (task-join (spawn (lambda ()
  (with-exception-handler
   (lambda (c) (sleep 1) (list 'outer c))
   (lambda ()
     (with-exception-handler
      (lambda (c) (raise-continuable 'inner))
      (lambda () (raise-continuable 'first)))))))))

;; A handler that escapes leaves its handler entries visible again.
(test '(escaped again)
      (with-exception-handler
       (lambda (c) (if (eq? c 'second) 'again 'unexpected))
       (lambda ()
         (list (call/cc (lambda (k)
                 (with-exception-handler (lambda (c) (k 'escaped))
                   (lambda () (raise 'first)))))
               (raise-continuable 'second)))))

;; A task waiting in its handler is cancelled: the handler's cleanup runs.
(define cleaned #f)
(define stuck (spawn (lambda ()
  (with-exception-handler
   (lambda (c) (dynamic-wind (lambda () #f) (lambda () (sleep 100000)) (lambda () (set! cleaned #t))))
   (lambda () (raise 'stuck))))))
(sleep 1)
(task-cancel stuck)
(test-error (task-join stuck))
(test #t cleaned)

;; How the VM calls handlers is set once, by the prelude; the masks a
;; handler runs under name a handler procedure.
(test-error (%set-raise-handler! (lambda (handler condition index) 'returned)))
(test-error (%push-mask 0.0))
(test-error (%push-mask 100000))
(test 'still-caught (guard (e (#t 'still-caught)) (raise 'x)))

;; Errors raised in handlers called outside tasks still work as before.
(test 'outside (with-exception-handler (lambda (c) 'outside) (lambda () (raise-continuable 'x))))
(test "boom" (guard (e (#t (error-object-message e)))
  (with-exception-handler (lambda (c) (raise c)) (lambda () (error "boom")))))

(test-end)
