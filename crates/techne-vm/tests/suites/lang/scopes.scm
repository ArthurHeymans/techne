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

;; Entries stack: an override shadows what it overrides until its scope
;; is shut; a scope adding again replaces its own entry, on top.
(define changes '())
;; (make-registry 'keys #:changed ...); this suite's forms are read, and
;; the reader's keywords are not data eval takes.
(define keys (%make-registry 'keys (make-hash-table) (lambda (k v) (set! changes (cons (list k v) changes)))))
(registry-add! keys 'k 'base)
(define user (make-scope 'user))
(define pkg (make-scope 'pkg))
(with-scope pkg (registry-add! keys 'k 'pkg))
(test 'pkg (registry-ref keys 'k))
(test (list (cons 'pkg pkg) (cons 'base %root-scope)) (registry-entries keys 'k))
(with-scope user (registry-add! keys 'k 'user))
(with-scope pkg (registry-add! keys 'k 'pkg2))
(test '(pkg2 user base) (map car (registry-entries keys 'k)))
(scope-shutdown! pkg)
(test 'user (registry-ref keys 'k))
;; Taking back an entry takes back one's own, else the one in effect.
(registry-remove! keys 'k)
(test '(user) (map car (registry-entries keys 'k)))
(scope-shutdown! user)
(test #f (registry-ref keys 'k))
(test '((k #f) (k user) (k pkg2) (k user) (k pkg) (k base)) changes)

;; A procedure handed over runs in the scope it was made in: what it
;; starts is owned there, whoever calls it.
(define owner (make-scope 'owner))
(define started (with-scope owner (scope-procedure (lambda () (spawn (lambda () (sleep 100)))))))
(define task (started))
(test owner (scope-of task))
(scope-shutdown! owner)
(test #t (task-done? task))
(test #f (started))

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
