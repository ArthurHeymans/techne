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
(define keys (%make-registry 'keys (make-hash-table) (lambda (k v) (set! changes (cons (list k v) changes))) #f))
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
(define shut-scope (make-scope 'shut))
(scope-shutdown! shut-scope)
(test-error (scope-transfer! doc shut-scope))
(test temp (scope-of doc))
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

;; A long-lived scope forgets what ended by itself.
(define busy (make-scope 'busy))
(with-scope busy (do ((i 0 (+ i 1))) ((= i 1000)) (task-join (spawn (lambda () i)))))
(test #t (< (length (scope-resources busy)) 200))
(scope-shutdown! busy)

;; A cleanup that never ends is stopped past the budget; what has a native
;; cleanup is released first, whatever it was made before.
(define stuck (make-scope 'stuck))
(define stuck-task (with-scope stuck (spawn (lambda () (sleep 100000)))))
(define stuck-channel (with-scope stuck (make-channel 1)))
(with-scope stuck (scope-own! 'loop (lambda (r) (let forever () (forever)))))
(test "a scope's cleanups ran out of time"
      (parameterize ((scope-cleanup-budget 50))
        (guard (e (#t (error-object-message e))) (scope-shutdown! stuck) "no failure")))
(test '(#t #t) (list (task-done? stuck-task) (channel-closed? stuck-channel)))

;; A task shutting its own scope: cancelling it fails, the Lisp cleanups
;; run all the same.
(define self-shut (make-scope 'self))
(define self-cleaned #f)
(with-scope self-shut (scope-own! 'x (lambda (r) (set! self-cleaned #t))))
(define shutter (with-scope self-shut (spawn (lambda () (scope-shutdown! self-shut)))))
(guard (e (#t #f)) (task-join shutter))
(test #t self-cleaned)

;; Code that cannot wait (eval in a task) runs the Lisp cleanups itself.
(define inline (make-scope 'inline))
(define inline-cleaned #f)
(with-scope inline (scope-own! 'x (lambda (r) (set! inline-cleaned #t))))
(test #t (task-join (spawn (lambda () (eval '(begin (scope-shutdown! inline) inline-cleaned))))))

;; Scope authority narrows monotonically across `with-scope`, cannot be widened
;; back to ancestors or siblings (via `with-scope`, `parameterize`, or
;; `%task-local-set!`), and can only be crossed via a `scope-procedure`
;; explicitly captured under the target authority.
(test #t (root-authority?))
(define auth-root (current-scope))
(define auth-pkg-a (make-scope 'pkg-a))
(define auth-pkg-b (make-scope 'pkg-b))
(define auth-root-helper
  (scope-procedure (lambda () (list (root-authority?) (eq? (current-scope) auth-root)))))

(test '(#f #t) (with-scope auth-pkg-a (list (root-authority?) (eq? (authority-scope) auth-pkg-a))))
;; Cannot enter ancestor (root) or sibling (pkg-b) via with-scope.
(test-error (with-scope auth-pkg-a (with-scope (scope-parent (current-scope)) (root-authority?))))
(test-error (with-scope auth-pkg-a (with-scope auth-root (root-authority?))))
(test-error (with-scope auth-pkg-a (with-scope auth-pkg-b (current-scope))))
;; Cannot rebind current-scope via parameterize or %task-local-set!.
(test-error (with-scope auth-pkg-a (parameterize ((current-scope auth-root)) (root-authority?))))
(test-error (with-scope auth-pkg-a (%task-local-set! (%current-scope-key) auth-root)))
(test-error (with-scope auth-pkg-a (%task-local-set! -5 auth-root)))
;; Cannot create a scope under root or sibling, or shut down root/sibling.
(define (try-make-under p) (with-scope auth-pkg-a (make-scope 'escape (string->keyword "parent") p)))
(test-error (try-make-under auth-root))
(test-error (try-make-under auth-pkg-b))
(test-error (with-scope auth-pkg-a (%make-scope 'escape auth-root)))
(test-error (with-scope auth-pkg-a (scope-shutdown! auth-root)))
(test-error (with-scope auth-pkg-a (scope-shutdown! auth-pkg-b)))
;; Can narrow to own child scope and restore back to pkg-a, but child cannot climb back to pkg-a.
(test '(#f #t #t #t)
      (with-scope auth-pkg-a
        (let* ((sub (make-scope 'sub))
               (inner (with-scope sub
                        (list (root-authority?)
                              (eq? (authority-scope) sub)
                              (guard (e (#t #t)) (with-scope auth-pkg-a #f))))))
          (scope-shutdown! sub)
          (append inner (list (eq? (authority-scope) auth-pkg-a))))))
;; A root-captured scope-procedure runs with root authority and restores pkg-a on return.
(test '((#t #t) #f #t)
      (with-scope auth-pkg-a
        (let ((res (auth-root-helper)))
          (list res (root-authority?) (eq? (authority-scope) auth-pkg-a)))))
(test #t (root-authority?))
(scope-shutdown! auth-pkg-a)
(scope-shutdown! auth-pkg-b)

;; Round 19: cleanup and done? run with registration authority, never the
;; shutdown caller's root authority (even when unloaded by root or transferred).
(define r19-root-param (make-root-parameter (list (string-copy "init"))))
(define r19-cleanup-log '())
(define r19-pkg (make-scope 'r19-pkg))
(define r19-res (list 'r19-res))
(with-scope r19-pkg
  (scope-own! r19-res
    (lambda (r)
      (let ((can-write (guard (e (#t #f)) (parameterize ((r19-root-param '("pwn"))) #t))))
        (set! r19-cleanup-log (cons (list 'cleanup (root-authority?) can-write) r19-cleanup-log))))
    (lambda (r)
      (let ((can-write (guard (e (#t #f)) (parameterize ((r19-root-param '("pwn"))) #t))))
        (set! r19-cleanup-log (cons (list 'done (root-authority?) can-write) r19-cleanup-log)))
      #f)))
(scope-shutdown! r19-pkg)
(test '((cleanup #f #f) (done #f #f)) r19-cleanup-log)
(test '("init") (r19-root-param))

;; Transferring a package resource to a root-owned scope preserves its
;; non-root registration authority for cleanup.
(define r19-transfer-log '())
(define r19-src (make-scope 'r19-src))
(define r19-dst (make-scope 'r19-dst))
(define r19-tres (list 'r19-tres))
(with-scope r19-src
  (scope-own! r19-tres
    (lambda (r) (set! r19-transfer-log (cons (root-authority?) r19-transfer-log)))
    (lambda (r) (set! r19-transfer-log (cons (root-authority?) r19-transfer-log)) #f)))
(scope-transfer! r19-tres r19-dst)
(scope-shutdown! r19-src)
(test '() r19-transfer-log)
(scope-shutdown! r19-dst)
(test '(#f #f) r19-transfer-log)

;; Round 19: protected parameters enforce root authority & deep copy isolation
;; at the VM level, blocking raw %task-local-*, record-fields, and aliasing.
(define r19-orig (list (string-copy "secret")))
(define r19-param (make-root-parameter r19-orig))
(string-set! (car r19-orig) 0 #\X)
(test '("secret") (r19-param))
(let ((got (r19-param)))
  (string-set! (car got) 0 #\Y))
(test '("secret") (r19-param))
(test #f (parameter-bound? r19-param))
(test '() (record-fields (%scope-capture)))
(test-error (%task-local-ref -100))
(test-error (%task-local-set! -100 "bad"))
(test-error (%task-local-delete! -100))
(test-error (%task-local-set! -5 %root-scope))
(test-error (%task-local-delete! -5))
(define r19-p-scope (make-scope 'r19-p-scope))
(test-error (with-scope r19-p-scope (make-root-parameter "nope")))
(test-error (with-scope r19-p-scope (parameterize ((r19-param '("nope"))) (r19-param))))
(test '("secret") (r19-param))
;; Child task inherits a protected parameter, and mutating its read copy does
;; not alias the parent or sibling tasks.
(parameterize ((r19-param (list (string-copy "bound"))))
  (test #t (parameter-bound? r19-param))
  (let ((child (with-scope r19-p-scope
                 (spawn (lambda ()
                          (let ((v (r19-param)))
                            (string-set! (car v) 0 #\Z)
                            (list v (r19-param))))))))
    (test '(("Zound") ("bound")) (task-join child)))
  (test '("bound") (r19-param)))
(test #f (parameter-bound? r19-param))
(scope-shutdown! r19-p-scope)

;; Round 19: cross-scope resource ownership mutation is rejected and atomic.
(define r19-a (make-scope 'r19-a))
(define r19-b (make-scope 'r19-b))
(define r19-root-res (list 'r19-root-res))
(define r19-a-res (list 'r19-a-res))
(define r19-a-cleaned #f)
(scope-own! r19-root-res (lambda (r) #f))
(with-scope r19-a
  (scope-own! r19-a-res (lambda (r) (set! r19-a-cleaned #t))))
(test-error (with-scope r19-a (scope-disown! r19-root-res)))
(test %root-scope (scope-of r19-root-res))
(test-error (with-scope r19-b (scope-disown! r19-a-res)))
(test r19-a (scope-of r19-a-res))
;; Failed transfer to unauthorized destination must leave source ownership intact.
(test-error (with-scope r19-a (scope-transfer! r19-a-res r19-b)))
(test r19-a (scope-of r19-a-res))
(test-error (with-scope r19-a (scope-transfer! r19-a-res %root-scope)))
(test r19-a (scope-of r19-a-res))
;; Cannot steal ownership with scope-own!.
(test-error (with-scope r19-b (scope-own! r19-a-res (lambda (r) #f))))
(test r19-a (scope-of r19-a-res))
;; Cannot forge predecessor across scopes.
(test-error (with-scope r19-a (%set-scope-predecessor! r19-a r19-b)))
;; Legitimate cross-scope release works via a resource-specific capability.
(define r19-cap-res (list 'r19-cap-res))
(define r19-release-cap
  (with-scope r19-a
    (scope-own! r19-cap-res (lambda (r) #f))
    (scope-procedure (lambda () (scope-disown! r19-cap-res)))))
(with-scope r19-b (r19-release-cap))
(test #f (scope-of r19-cap-res))
(scope-shutdown! r19-a)
(test #t r19-a-cleaned)
(scope-shutdown! r19-b)
(scope-disown! r19-root-res)

;; Round 19: adopting a temporary scope keeps its scope-procedure callbacks
;; live under the adopting owner, while shutting it down disables them.
(define r19-adopt-tmp (make-scope 'r19-adopt-tmp))
(define r19-adopt-cb
  (with-scope r19-adopt-tmp
    (scope-procedure (lambda () (list (scope-name (current-scope)) (spawn (lambda () 'adopted-ok)))))))
(%scope-adopt! r19-adopt-tmp %root-scope)
(let ((res (r19-adopt-cb)))
  (test 'root (car res))
  (test 'adopted-ok (task-join (cadr res))))
(define r19-fail-tmp (make-scope 'r19-fail-tmp))
(define r19-fail-cb
  (with-scope r19-fail-tmp
    (scope-procedure (lambda () 'should-not-run))))
(scope-shutdown! r19-fail-tmp)
(test #f (r19-fail-cb))

;; Round 20 (Finding 1): privileged VM boot primitives are sealed out of
;; ROOT_MODULE after boot, and outer exception handlers around escalating
;; scope-procedure calls never receive the callee's higher authority.
(test-error (eval '%world-load-enter!))
(test-error (eval '%staging-load-enter!))
(test-error (eval '%cleanup-action-take!))
(test-error (eval '%scope-restore!))
(test-error (eval '%make-world-scope))
(test-error (eval '%seal-vm-boot!))
(define r20-root-failing (scope-procedure (lambda () (error "root boom"))))
(define r20-exc-scope (make-scope 'r20-exc-scope))
(define r20-handler-saw-root 'unset)
(test 'caught
      (with-scope r20-exc-scope
        (call/cc
         (lambda (k)
           (with-exception-handler
            (lambda (e)
              (set! r20-handler-saw-root
                    (list (root-authority?)
                          (guard (x (#t #f)) (parameterize ((r19-param '("pwn"))) #t))))
              (k 'caught))
            (lambda () (r20-root-failing)))))))
(test '(#f #f) r20-handler-saw-root)
(test '("secret") (r19-param))
(scope-shutdown! r20-exc-scope)

;; Round 20 (Finding 2): protected parameter objects and normal parameters
;; cannot be mutated via generic %record-set!, and policy consumers keep
;; observing the unmutated protected value.
(define (r20-policy-consumer) (r19-param))
(test '() (record-fields r19-param))
(test-error (%record-ref r19-param %parameter-type 0))
(test-error (%record-set! r19-param %parameter-type 0 (lambda () '("forged"))))
(test-error (%record-set! current-scope %parameter-type 0 (lambda () %root-scope)))
(test '("secret") (r20-policy-consumer))

;; Round 20 (Finding 3): shutdown returns opaque one-shot cleanup actions
;; without leaking registration capabilities (root -> child transfer direction).
(define r20-owner-child (make-scope 'r20-owner-child))
(define r20-root-reg-res (list 'r20-root-reg-res))
(define r20-root-reg-log '())
(scope-own! r20-root-reg-res
  (lambda (r) (set! r20-root-reg-log (cons (list 'cleanup (root-authority?)) r20-root-reg-log)))
  (lambda (r) (set! r20-root-reg-log (cons (list 'done (root-authority?)) r20-root-reg-log)) #f))
(scope-transfer! r20-root-reg-res r20-owner-child)
(let ((res
       (with-scope r20-owner-child
         (let* ((shut (%scope-shutdown! r20-owner-child))
                (actions (caddr shut))
                (act (car actions))
                (fields (record-fields act))
                (auth-after-shut (root-authority?)))
           (%run-cleanup-action act)
           ;; Second invocation must be a no-op (one-shot).
           (%run-cleanup-action act)
           (list fields auth-after-shut (root-authority?))))))
  (test '(() #f #f) res))
(test '((cleanup #t) (done #t)) r20-root-reg-log)
;; Failing root-registered cleanup triggered by child must not run child's
;; exception handler under root authority.
(define r20-owner-child2 (make-scope 'r20-owner-child2))
(define r20-fail-reg-res (list 'r20-fail-reg-res))
(define r20-cleanup-handler-root 'unset)
(scope-own! r20-fail-reg-res (lambda (r) (error "root cleanup boom")))
(scope-transfer! r20-fail-reg-res r20-owner-child2)
(test 'caught-cleanup
      (with-scope r20-owner-child2
        (call/cc
         (lambda (k)
           (with-exception-handler
            (lambda (e)
              (set! r20-cleanup-handler-root (root-authority?))
              (k 'caught-cleanup))
            (lambda () (scope-shutdown! r20-owner-child2)))))))
(test #f r20-cleanup-handler-root)

;; Round 20 (Finding 4): adoption transfers lifetime ownership to the adopter
;; without widening a non-root callback's captured privilege ceiling.
(define r20-adopt-existing (list 'r20-adopt-existing))
(scope-own! r20-adopt-existing (lambda (r) #f))
(define r20-adopt-nonroot-tmp (make-scope 'r20-adopt-nonroot-tmp))
(define r20-adopted-fresh (list 'r20-adopted-fresh))
(define r20-adopt-nonroot-cb
  (with-scope r20-adopt-nonroot-tmp
    (scope-procedure
     (lambda ()
       (let* ((owner-ok (and (eq? (current-scope) %root-scope) (scope-live? (current-scope))))
              (is-root (root-authority?))
              (param-blocked (guard (e (#t #t)) (parameterize ((r19-param '("pwn"))) #f)))
              (shut-blocked (guard (e (#t #t)) (scope-shutdown! %root-scope) #f))
              (disown-blocked (guard (e (#t #t)) (scope-disown! r20-adopt-existing) #f))
              (hijack-blocked (guard (e (#t #t)) (%scope-own! %root-scope r20-adopt-existing (lambda (_) #f) (lambda (_) #f)) #f))
              (fresh-owned (begin (scope-own! r20-adopted-fresh (lambda (_) #f)) (eq? (scope-of r20-adopted-fresh) %root-scope)))
              (spawn-root (task-join (spawn (lambda () (root-authority?))))))
         (list owner-ok is-root param-blocked shut-blocked disown-blocked hijack-blocked fresh-owned spawn-root))))))
(%scope-adopt! r20-adopt-nonroot-tmp %root-scope)
(test '(#t #f #t #t #t #t #t #f) (r20-adopt-nonroot-cb))
(scope-disown! r20-adopted-fresh)
(scope-disown! r20-adopt-existing)

;; Round 20 (Finding 5): failed parameterize leaves earlier bindings untouched.
(define r20-p1 (make-parameter 'initial))
(define r20-p2 (make-root-parameter 'ok))
(define r20-body-ran #f)
(test-error
 (parameterize ((r20-p1 'temporary)
                (r20-p2 (make-hash-table)))
   (set! r20-body-ran #t)))
(test #f r20-body-ran)
(test 'initial (r20-p1))
(test #f (parameter-bound? r20-p1))
(define r20-p-child (make-scope 'r20-p-child))
(test-error
 (with-scope r20-p-child
   (parameterize ((r20-p1 'temporary-child)
                  (r20-p2 'denied))
     (set! r20-body-ran #t))))
(test #f r20-body-ran)
(test 'initial (r20-p1))
(test #f (parameter-bound? r20-p1))
(scope-shutdown! r20-p-child)

;; Round 20 (Finding 6): a task spawned before adoption resolves current-scope
;; to the adopter after adoption and can register resources without gaining root.
(define r20-task-tmp (make-scope 'r20-task-tmp))
(define r20-task-go (make-channel))
(define r20-task-res (list 'r20-task-res))
(define r20-adopted-task
  (with-scope r20-task-tmp
    (spawn
     (lambda ()
       (channel-recv r20-task-go)
       (let ((live (scope-live? (current-scope)))
             (is-adopter (eq? (current-scope) %root-scope))
             (is-root (root-authority?)))
         (scope-own! r20-task-res (lambda (_) #f))
         (list live is-adopter is-root (eq? (scope-of r20-task-res) %root-scope)))))))
(%scope-adopt! r20-task-tmp %root-scope)
(channel-send r20-task-go #t)
(test '(#t #t #f #t) (task-join r20-adopted-task))
(scope-disown! r20-task-res)

;; Round 21 (Finding 2): inline scope pruning must not panic when a native
;; done? predicate mutates or shuts down the scope mid-prune.
(define r21-prune-s (make-scope 'r21-prune-s))
(with-scope r21-prune-s
  (%scope-own! r21-prune-s r21-prune-s (lambda (_) #f) %scope-shutdown!)
  (let loop ((i 0))
    (when (< i 128)
      (guard (e (#t #f))
        (%scope-own! r21-prune-s (list 'r21-item i) (lambda (_) #f) (lambda (_) #f)))
      (loop (+ i 1)))))
(test #f (scope-live? r21-prune-s))

;; Round 21 (Finding 3a): adoption must preserve the scope tree and reject
;; adopting a scope into its own descendant or adopting root/world scopes.
(define r21-anc (make-scope 'r21-anc))
(define r21-mid (with-scope r21-anc (make-scope 'r21-mid)))
(define r21-desc (with-scope r21-mid (make-scope 'r21-desc)))
(test-error (%scope-adopt! r21-anc r21-mid))
(test-error (%scope-adopt! r21-anc r21-desc))
(test-error (%scope-adopt! %root-scope r21-anc))
(test '(#t #t #t) (list (scope-live? r21-anc) (scope-live? r21-mid) (scope-live? r21-desc)))
(test #t (eq? (scope-parent r21-mid) r21-anc))
(test #t (eq? (scope-parent r21-desc) r21-mid))
(scope-shutdown! r21-anc)
(test '(#f #f #f) (list (scope-live? r21-anc) (scope-live? r21-mid) (scope-live? r21-desc)))

;; Round 21 (Finding 3b): long adoption chains with slot reuse remain
;; resolvable for captured callbacks and already-spawned tasks.
(define r21-chain-start (make-scope 'r21-chain-0))
(define r21-chain-go (make-channel))
(define r21-chain-cb
  (with-scope r21-chain-start
    (scope-procedure
     (lambda ()
       (list (scope-live? (current-scope)) (current-scope) (root-authority?))))))
(define r21-chain-task
  (with-scope r21-chain-start
    (spawn
     (lambda ()
       (channel-recv r21-chain-go)
       (list (scope-live? (current-scope)) (current-scope) (root-authority?))))))
(define r21-chain-end
  (let loop ((i 0) (cur r21-chain-start))
    (if (= i 40)
        cur
        (let ((nxt (make-scope 'r21-chain-step)))
          (%scope-adopt! cur nxt)
          (loop (+ i 1) nxt)))))
(let ((cb-res (r21-chain-cb)))
  (test #t (car cb-res))
  (test #t (eq? (cadr cb-res) r21-chain-end))
  (test #f (caddr cb-res)))
(channel-send r21-chain-go #t)
(let ((t-res (task-join r21-chain-task)))
  (test #t (car t-res))
  (test #t (eq? (cadr t-res) r21-chain-end))
  (test #f (caddr t-res)))
(scope-shutdown! r21-chain-end)
(test #f (r21-chain-cb))

;; Round 21 (Finding 4): protected-parameter cloning bounds total nodes and
;; bytes so shared acyclic DAGs and huge allocations fail cleanly without
;; mutating existing bindings.
(define r21-clone-param (make-root-parameter "safe-orig"))
(define r21-dag-bomb
  (let loop ((i 0) (x "leaf"))
    (if (= i 30) x (loop (+ i 1) (cons x x)))))
(test-error (parameterize ((r21-clone-param r21-dag-bomb)) #t))
(test "safe-orig" (r21-clone-param))
(test-error (%protected-param-set! r21-clone-param r21-dag-bomb))
(test "safe-orig" (r21-clone-param))
(test-error (make-root-parameter r21-dag-bomb))
(test-error (parameterize ((r21-clone-param (make-vector 20000 #f))) #t))
(test "safe-orig" (r21-clone-param))
(test-error (parameterize ((r21-clone-param (make-string 1200000 #\a))) #t))
(test "safe-orig" (r21-clone-param))

;; Round 22 (Finding 3): low-level handler-stack and forms-module primitives
;; are sealed out of ROOT_MODULE after boot so Scheme code cannot bypass VM
;; authority barriers.
(for-each (lambda (sym) (test-error (eval sym)))
          '(%enter-forms-module
            %set-staging-scope!
            %package-begin
            %package-end
            %package-publish
            %package-discard
            %retire-generation
            %pop-handler
            %push-wind
            %push-handler
            %resume-unwind
            %push-mask
            %handler-at
            %continuable-handler
            %handler-returned))

;; Round 22 (Finding 4): shutting down a deeply nested scope tree runs
;; iteratively without overflowing the Rust call stack and preserves
;; child-first cleanup order.
(define r22-deep-top (make-scope 'r22-deep-0))
(define r22-deep-order '())
(define r22-deep-leaf
  (let loop ((i 0) (cur r22-deep-top))
    (when (or (= i 0) (= i 1500) (= i 3000))
      (let ((tag i))
        (with-scope cur
          (scope-own! (list 'deep-res tag)
                      (lambda (_) (set! r22-deep-order (cons tag r22-deep-order)))))))
    (if (= i 3000)
        cur
        (loop (+ i 1) (with-scope cur (make-scope 'r22-deep-child))))))
(scope-shutdown! r22-deep-top)
(test #f (scope-live? r22-deep-top))
(test #f (scope-live? r22-deep-leaf))
(test '(0 1500 3000) r22-deep-order)

(test-end)
