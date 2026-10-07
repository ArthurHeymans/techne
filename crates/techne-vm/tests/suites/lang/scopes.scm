;;; Owned scopes: shutting a scope removes what it owns.

(test-begin "scopes")

(define commands (make-registry 'commands))
(define callbacks '())
(define writes '())

;; A sample mode: a command, a long-running task (a timer), a task that
;; writes later, a channel, a child scope, and a callback handed to code
;; that outlives the mode.
(define (load-mode version)
  (let ((s (make-scope 'mode)))
    (with-scope s
      (registry-add! commands 'greet (lambda () version))
      (spawn (lambda () (let loop () (sleep 1) (loop))))
      (spawn (lambda () (sleep 20) (set! writes (cons version writes))))
      (make-channel 1)
      (with-scope (make-scope 'child)
        (registry-add! commands (string->symbol (string-append "child-" (number->string version))) version))
      (set! callbacks (cons (scope-procedure (lambda () (set! writes (cons version writes)) version)) callbacks)))
    s))

(define base-tasks (%live-task-count))

;; Load and unload a hundred times: nothing is left behind.
(do ((i 0 (+ i 1))) ((= i 100))
  (scope-shutdown! (load-mode i)))
(run-tasks)
(test '() (registry-keys commands))
(test base-tasks (%live-task-count))
(test '() (scope-children %root-scope))
(test '() writes)
;; Late callbacks of unloaded modes do nothing.
(test '(#f) (delete-duplicates (map (lambda (cb) (cb)) callbacks)))
(test '() writes)

;; A replacement loaded before the old mode is shut keeps its entry.
(define old (load-mode 1000))
(define new (load-mode 1001))
(scope-shutdown! old)
(test 1001 ((registry-ref commands 'greet)))
(test new (registry-owner commands 'greet))
(scope-shutdown! new)
(test #f (registry-ref commands 'greet))

;; Something that must outlive its scope moves to a longer-lived one.
(define keeper (make-scope 'keeper))
(define doc (list 'document))
(define closed #f)
(define temp (make-scope 'temp))
(with-scope temp (scope-own! doc (lambda (d) (set! closed #t))))
(scope-transfer! doc keeper)
(scope-shutdown! temp)
(test #f closed)
(test keeper (scope-of doc))
(scope-shutdown! keeper)
(test #t closed)

;; Cleanups run newest first; a failing one does not stop the others.
(define order '())
(define s (make-scope))
(with-scope s
  (scope-own! 'a (lambda (r) (set! order (cons r order))))
  (scope-own! 'b (lambda (r) (error "cleanup failed")))
  (scope-own! 'c (lambda (r) (set! order (cons r order)))))
(test "cleanup failed" (guard (e (#t (error-object-message e))) (scope-shutdown! s) "no failure"))
(test '(a c) order)
(test #f (scope-live? s))
(test-error (with-scope s (make-channel)))

(test-end)
