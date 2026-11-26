;;; Packages and generations: staged load, atomic publish, the previous
;;; generation kept on failure and shut after success.

(test-begin "packages")

;; What packages share with this file lives in the root module.
(eval '(define demo-commands (make-registry 'demo)) "root")
(eval '(define keeper (make-scope 'keeper)) "root")
(eval '(define kept-result (box #f)) "root")

;; Beside this program, whose name differs between the runs in each mode.
(define path (string-append (car (command-line)) ".package.scm"))
(define (write-package! . forms)
  (call-with-output-file path (lambda (port) (for-each (lambda (f) (write f port) (newline port)) forms))))
(define (command name) (let ((c (registry-ref demo-commands name))) (and c (c))))
(define base-tasks (%live-task-count))

(write-package!
 '(define-record-type point (make-point x) point? (x point-x))
 '(define (version) 'v1)
 '(registry-add! demo-commands 'hello (lambda () (version)))
 '(registry-add! demo-commands 'only-v1 (lambda () 'only-v1))
 '(registry-add! demo-commands 'make-point (lambda () (make-point 1)))
 '(registry-add! demo-commands 'point? (lambda () point?))
 ;; A timer, cancelled with its generation.
 '(spawn (lambda () (let loop () (sleep 1) (loop))))
 ;; A task that must outlive a reload: moved to a longer-lived scope, it
 ;; finishes on its own generation's code.
 '(scope-transfer! (spawn (lambda () (sleep 30) (set-box! kept-result (version)))) keeper))

(test 1 (load-package 'demo path))
(test 'v1 (command 'hello))
(define old-point (command 'make-point))
(test (+ base-tasks 2) (%live-task-count))

;; A reload that fails changes nothing visible.
(write-package!
 '(registry-add! demo-commands 'hello (lambda () 'broken))
 '(registry-add! demo-commands 'extra (lambda () 'extra))
 '(spawn (lambda () (let loop () (sleep 1) (loop))))
 '(car '()))
(test-error (load-package 'demo path))
(test 'v1 (command 'hello))
(test #f (command 'extra))
(test 1 (package-generation (find-package 'demo)))
(test (+ base-tasks 2) (begin (sleep 5) (%live-task-count)))

;; A successful one switches the commands; the old generation is shut.
(write-package!
 '(define-record-type point (make-point x) point? (x point-x))
 '(define (version) 'v3)
 '(registry-add! demo-commands 'hello (lambda () (version)))
 '(registry-add! demo-commands 'point? (lambda () point?)))
(test 2 (load-package 'demo path))
(test 'v3 (command 'hello))
(test #f (command 'only-v1))
;; Records of a new generation are a new type.
(test #f ((command 'point?) old-point))
;; The moved task finishes with the code of its own generation.
(sleep 60)
(test 'v1 (unbox kept-result))
(run-tasks)
(test base-tasks (%live-task-count))

;; An override made since stays in effect over the next generation, and
;; unloading the package uncovers what its entries shadowed.
(registry-add! demo-commands 'point? (lambda () 'overridden))
(registry-add! demo-commands 'before (lambda () 'mine))
(write-package!
 '(define (version) 'v4)
 '(registry-add! demo-commands 'hello (lambda () (version)))
 '(registry-add! demo-commands 'point? (lambda () 'v4))
 ;; Registered twice, it still keeps its predecessor's place.
 '(registry-add! demo-commands 'point? (lambda () 'v4-again))
 '(registry-add! demo-commands 'before (lambda () 'package)))
(test 3 (load-package 'demo path))
(test 'v4 (command 'hello))
(test 'overridden (command 'point?))
(test 'package (command 'before))
(test 4 (load-package 'demo path))
(test '(package mine) (map (lambda (e) ((car e))) (registry-entries demo-commands 'before)))
(test '(overridden v4-again) (map (lambda (e) ((car e))) (registry-entries demo-commands 'point?)))

;; Unloading removes everything the package owns.
(unload-package 'demo)
(test 'mine (command 'before))
(registry-remove! demo-commands 'before)
(registry-remove! demo-commands 'point?)
(test '() (registry-keys demo-commands))
(test #f (find-package 'demo))
(delete-file path)

;; Packages in directories of their own, beside this program.
(define (package-dir name) (string-append path "." (symbol->string name) "/"))
(define (package-main name) (string-append (package-dir name) "main.scm"))
(define (write-forms! file . forms)
  (call-with-output-file file (lambda (port) (for-each (lambda (f) (write f port) (newline port)) forms))))

;; What a package loads from outside its directory (a file it requires, a
;; library found among the shipped ones) is the world's, not the package's
;; that happened to load it first: it loads once, whatever the order, and
;; stays when that package is reloaded or unloaded.
(define shared (string-append path ".shared.scm"))
(write-forms! shared '(provide token) '(define token (list 'instance)))
(define (write-user! name)
  (make-directory (package-dir name))
  (write-forms! (package-main name)
                `(require ,shared)
                '(import (techne path))
                `(registry-add! demo-commands ',name (lambda () (cons token path-join)))))
(define (same? a b)
  (let ((x (command a)) (y (command b)))
    (and (eq? (car x) (car y)) (eq? (cdr x) (cdr y)))))
(for-each write-user! '(first second third))
(load-package 'first (package-main 'first))
(load-package 'second (package-main 'second))
(test #t (same? 'first 'second))
;; The one that loaded them first is reloaded: the others keep them, and
;; so does its next generation, and one loaded after.
(load-package 'first (package-main 'first))
(test #t (same? 'first 'second))
(load-package 'third (package-main 'third))
(test #t (same? 'third 'second))
;; It is unloaded: nothing changes for the others, reloaded or not.
(unload-package 'first)
(load-package 'second (package-main 'second))
(test #t (same? 'third 'second))
(for-each unload-package '(second third))
(for-each (lambda (name) (delete-file (package-main name))) '(first second third))
(delete-file shared)

;; The libraries a package's files define are its own: they load afresh
;; with each generation, as its files do.
(define own-library (string-append (package-dir 'own) "own/lib.sld"))
(define (write-own! version . forms)
  (write-forms! own-library
                `(define-library (own lib) (export version) (import (scheme base)) (begin (define (version) ',version))))
  (apply write-forms! (package-main 'own) '(import (own lib)) forms))
(define own-command '(registry-add! demo-commands 'own (lambda () (version))))
(make-directory (string-append (package-dir 'own) "own"))
(write-own! 'v1 own-command)
(load-package 'own (package-main 'own))
(test 'v1 (command 'own))
(write-own! 'v2 own-command)
(load-package 'own (package-main 'own))
(test 'v2 (command 'own))
(write-own! 'v3 own-command)
(load-package 'own (package-main 'own))
(test 'v3 (command 'own))
(test 'v3 (eval '(version) "(own lib)"))
;; While the next generation loads, its library is its own still: another
;; task finds the published one by its name.
(eval '(define own-go (make-channel)) "root")
(write-own! 'v4 '(channel-recv own-go) own-command)
(define own-loader (spawn (lambda () (load-package 'own (package-main 'own)))))
(sleep 5)
(test 'v3 (task-join (spawn (lambda () (eval '(version) "(own lib)")))))
(channel-send own-go #t)
(task-join own-loader)
(test 'v4 (eval '(version) "(own lib)"))
(test 'v4 (command 'own))
;; A generation that fails takes its library with it.
(write-own! 'v5 '(car '()))
(test-error (load-package 'own (package-main 'own)))
(test 'v4 (eval '(version) "(own lib)"))
(test 'v4 (command 'own))
;; Unloaded, the library goes with the package.
(unload-package 'own)
(test-error (eval '(version) "(own lib)"))
(test '() (registry-keys demo-commands))
(delete-file own-library)
(delete-file (package-main 'own))

;; What a file from elsewhere starts and registers as it loads is the
;; world's too, as the file is: not the package's that happened to load it
;; first, whose unloading would stop it for the others.
(define worker (string-append path ".worker.scm"))
(write-forms! worker
              '(provide worker-task)
              '(define worker-task (spawn (lambda () (let loop () (sleep 1) (loop)))))
              '(registry-add! demo-commands 'from-elsewhere (lambda () 'elsewhere)))
(define (write-worker-user! name)
  (make-directory (package-dir name))
  (write-forms! (package-main name) `(require ,worker) `(registry-add! demo-commands ',name (lambda () worker-task))))
(for-each write-worker-user! '(starts joins))
(load-package 'starts (package-main 'starts))
(load-package 'joins (package-main 'joins))
(define the-worker (command 'starts))
(test #t (eq? the-worker (command 'joins)))
(unload-package 'starts)
(sleep 5)
(test #f (task-done? the-worker))
(test 'elsewhere (command 'from-elsewhere))
(test #f (command 'starts))
(unload-package 'joins)
(task-kill the-worker)
(registry-remove! demo-commands 'from-elsewhere)
(test '() (registry-keys demo-commands))
(for-each (lambda (name) (delete-file (package-main name))) '(starts joins))
(delete-file worker)

;; A world module that fails while a package stages it leaves neither its
;; tasks nor its registrations behind in the root scope.
(define failing-world (string-append path ".failing-world.scm"))
(eval '(define leaked-task #f) "root")
(write-forms! failing-world
              '(eval '(set! leaked-task (spawn (lambda () (let loop () (sleep 1) (loop))))) "root")
              '(registry-add! demo-commands 'leaked-reg (lambda () 'bad))
              '(error "world init failed"))
(make-directory (package-dir 'fails-world))
(write-forms! (package-main 'fails-world) `(require ,failing-world))
(test-error (load-package 'fails-world (package-main 'fails-world)))
(sleep 5)
(test #t (task-done? (eval 'leaked-task "root")))
(test #f (command 'leaked-reg))
(delete-file (package-main 'fails-world))
(delete-file failing-world)

;; A package-owned file required by a world module still loads in the
;; staged package's scope so unloading the package cleans it up.
(define world-bridge (string-append path ".world-bridge.scm"))
(make-directory (package-dir 'bridged))
(define bridged-helper (string-append (package-dir 'bridged) "helper.scm"))
(write-forms! bridged-helper
              '(provide bridged-task)
              '(define bridged-task (spawn (lambda () (let loop () (sleep 1) (loop)))))
              '(registry-add! demo-commands 'bridged-helper (lambda () bridged-task)))
(write-forms! world-bridge `(require ,bridged-helper))
(write-forms! (package-main 'bridged) `(require ,world-bridge))
(load-package 'bridged (package-main 'bridged))
(define b-task (command 'bridged-helper))
(test #f (task-done? b-task))
(unload-package 'bridged)
(sleep 5)
(test #t (task-done? b-task))
(test #f (command 'bridged-helper))
(delete-file bridged-helper)
(delete-file (package-main 'bridged))
(delete-file world-bridge)

;; A shared library loaded via the synchronous compiler fallback (e.g. eval)
;; while a package is staged still owns its tasks/registrations in root scope.
(define sync-lib-dir (string-append path ".sync-lib"))
(make-directory sync-lib-dir)
(make-directory (string-append sync-lib-dir "/synclib"))
(define sync-lib-file (string-append sync-lib-dir "/synclib/shared.sld"))
(write-forms! sync-lib-file
              '(define-library (synclib shared)
                 (export sync-worker)
                 (import (techne))
                 (begin
                   (define sync-worker (spawn (lambda () (let loop () (sleep 1) (loop)))))
                   (registry-add! (eval 'demo-commands "root") 'from-sync-lib (lambda () sync-worker)))))
(%provide-libraries! "synclib" sync-lib-dir)
(make-directory (package-dir 'sync-pkg))
(write-forms! (package-main 'sync-pkg)
              '(eval '(import (synclib shared)))
              '(registry-add! demo-commands 'sync-pkg (lambda () 'ok)))
(load-package 'sync-pkg (package-main 'sync-pkg))
(define sw (command 'from-sync-lib))
(test #f (task-done? sw))
(unload-package 'sync-pkg)
(sleep 5)
(test #f (task-done? sw))
(test #t (eq? sw (command 'from-sync-lib)))
(task-kill sw)
(registry-remove! demo-commands 'from-sync-lib)
(delete-file (package-main 'sync-pkg))
(delete-file sync-lib-file)

;; A `scope-procedure` registered by a world module stays live after the
;; temporary world-load scope is adopted into `%root-scope`, and runs in
;; `%root-scope` even after the first importer unloads. A `scope-procedure`
;; captured by a world module that fails mid-load becomes dead.
(define cb-world (string-append path ".cb-world.scm"))
(write-forms! cb-world
              '(provide cb-proc)
              '(define cb-proc (scope-procedure (lambda () (cons 'alive (eq? (current-scope) %root-scope)))))
              '(registry-add! demo-commands 'world-cb cb-proc))
(make-directory (package-dir 'cb-pkg))
(write-forms! (package-main 'cb-pkg) `(require ,cb-world))
(load-package 'cb-pkg (package-main 'cb-pkg))
(test '(alive . #t) (command 'world-cb))
(unload-package 'cb-pkg)
(test '(alive . #t) (command 'world-cb))
(registry-remove! demo-commands 'world-cb)
(delete-file (package-main 'cb-pkg))
(delete-file cb-world)

(define fail-cb-world (string-append path ".fail-cb-world.scm"))
(eval '(define dead-cb (box #f)) "root")
(write-forms! fail-cb-world
              '(set-box! (eval 'dead-cb "root") (scope-procedure (lambda () 'should-not-run)))
              '(error "fail after capture"))
(make-directory (package-dir 'fail-cb-pkg))
(write-forms! (package-main 'fail-cb-pkg) `(require ,fail-cb-world))
(test-error (load-package 'fail-cb-pkg (package-main 'fail-cb-pkg)))
(test #f ((unbox (eval 'dead-cb "root"))))
(delete-file (package-main 'fail-cb-pkg))
(delete-file fail-cb-world)

;; A task spawned during staging keeps its inherited `%staging-scope`. If it
;; later loads a world module after staging ends (whether in its inherited
;; package scope or in `%root-scope`), that module keeps world ownership and
;; survives unloading the package.
(define late-world (string-append path ".late-world.scm"))
(write-forms! late-world
              '(provide late-task)
              '(define late-task (spawn (lambda () (let loop () (sleep 1) (loop)))))
              '(registry-add! demo-commands 'from-late-world (lambda () late-task)))
(define late-world-root (string-append path ".late-world-root.scm"))
(write-forms! late-world-root
              '(provide late-root-task)
              '(define late-root-task (spawn (lambda () (let loop () (sleep 1) (loop)))))
              '(registry-add! demo-commands 'from-late-world-root (lambda () late-root-task)))
(eval '(define late-go (make-channel)) "root")
(eval '(define late-done (make-channel)) "root")
(make-directory (package-dir 'late-pkg))
(write-forms! (package-main 'late-pkg)
              `(scope-transfer!
                (spawn (lambda ()
                         (channel-recv late-go)
                         (eval '(require ,late-world))
                         (with-scope %root-scope (eval '(require ,late-world-root)))
                         (channel-send late-done #t)))
                keeper))
(load-package 'late-pkg (package-main 'late-pkg))
(channel-send late-go #t)
(channel-recv late-done)
(define lt (command 'from-late-world))
(define ltr (command 'from-late-world-root))
(test #f (task-done? lt))
(test #f (task-done? ltr))
(unload-package 'late-pkg)
(sleep 5)
(test #f (task-done? lt))
(test #f (task-done? ltr))
(test #t (eq? lt (command 'from-late-world)))
(test #t (eq? ltr (command 'from-late-world-root)))
(task-kill lt)
(task-kill ltr)
(registry-remove! demo-commands 'from-late-world)
(registry-remove! demo-commands 'from-late-world-root)
(delete-file (package-main 'late-pkg))
(delete-file late-world)
(delete-file late-world-root)

;; A synchronous library load (via `eval`) that fails mid-load rolls back the
;; tasks and registrations it created in the root scope, and a retry succeeds.
(define fail-sync-file (string-append sync-lib-dir "/synclib/flaky.sld"))
(eval '(define flaky-task (box #f)) "root")
(write-forms! fail-sync-file
              '(define-library (synclib flaky)
                 (export flaky-worker)
                 (import (techne))
                 (begin
                   (define flaky-worker (spawn (lambda () (let loop () (sleep 1) (loop)))))
                   (set-box! (eval 'flaky-task "root") flaky-worker)
                   (registry-add! (eval 'demo-commands "root") 'from-flaky-lib (lambda () flaky-worker))
                   (error "sync library init failed"))))
(make-directory (package-dir 'flaky-pkg))
(write-forms! (package-main 'flaky-pkg) '(eval '(import (synclib flaky))))
(test-error (load-package 'flaky-pkg (package-main 'flaky-pkg)))
(sleep 5)
(test #t (task-done? (unbox (eval 'flaky-task "root"))))
(test #f (command 'from-flaky-lib))
;; Once fixed, the library loads cleanly on retry and outlives the package.
(write-forms! fail-sync-file
              '(define-library (synclib flaky)
                 (export flaky-worker)
                 (import (techne))
                 (begin
                   (define flaky-worker (spawn (lambda () (let loop () (sleep 1) (loop)))))
                   (registry-add! (eval 'demo-commands "root") 'from-flaky-lib (lambda () flaky-worker)))))
(load-package 'flaky-pkg (package-main 'flaky-pkg))
(define fw (command 'from-flaky-lib))
(test #f (task-done? fw))
(unload-package 'flaky-pkg)
(sleep 5)
(test #f (task-done? fw))
(test #t (eq? fw (command 'from-flaky-lib)))
(task-kill fw)
(registry-remove! demo-commands 'from-flaky-lib)
(delete-file (package-main 'flaky-pkg))
(delete-file fail-sync-file)

;; A world module shadowing an existing root-scope registry entry restores the
;; old entry on failure, retires the shadowed root entry on success (keeping one
;; root entry per key), and lets tasks spawned during the load replace/remove
;; root entries without duplicating them. A resource transferred away during
;; world load keeps its transferred owner on commit.
(define shadow-fail-world (string-append path ".shadow-fail-world.scm"))
(define shadow-ok-world (string-append path ".shadow-ok-world.scm"))
(eval '(define shadow-go (make-channel)) "root")
(eval '(define shadow-done (make-channel)) "root")
(eval '(define shadow-transferred (box #f)) "root")
(registry-add! demo-commands 'shadowed-root (lambda () 'root-v1))
;; Failing shadow restores root-v1.
(write-forms! shadow-fail-world
              '(registry-add! demo-commands 'shadowed-root (lambda () 'world-fail))
              '(error "shadow load failed"))
(make-directory (package-dir 'shadow-fail-pkg))
(write-forms! (package-main 'shadow-fail-pkg) `(require ,shadow-fail-world))
(test-error (load-package 'shadow-fail-pkg (package-main 'shadow-fail-pkg)))
(test 'root-v1 (command 'shadowed-root))
;; Succeeding shadow retires root-v1; a task spawned during load inherits the
;; forwarded temp scope and still replaces/removes the root entry cleanly; a
;; resource transferred to `keeper` during load stays owned by `keeper`.
(write-forms! shadow-ok-world
              '(registry-add! demo-commands 'shadowed-root (lambda () 'world-v2))
              '(let ((r (list 'xfer)))
                 (scope-own! r (lambda (_) #f))
                 (scope-transfer! r keeper)
                 (set-box! (eval 'shadow-transferred "root") r))
              '(spawn (lambda ()
                        (channel-recv shadow-go)
                        (registry-add! demo-commands 'shadowed-root (lambda () 'world-v3))
                        (channel-send shadow-done #t))))
(make-directory (package-dir 'shadow-pkg))
(write-forms! (package-main 'shadow-pkg) `(require ,shadow-ok-world))
(load-package 'shadow-pkg (package-main 'shadow-pkg))
(test 'world-v2 (command 'shadowed-root))
(test #t (eq? keeper (scope-of (unbox (eval 'shadow-transferred "root")))))
(scope-disown! (unbox (eval 'shadow-transferred "root")))
;; Task spawned during world load updates the root entry via its forwarded scope.
(channel-send shadow-go #t)
(channel-recv shadow-done)
(test 'world-v3 (command 'shadowed-root))
;; Only one root entry exists: a single registry-remove! clears it completely.
(registry-remove! demo-commands 'shadowed-root)
(test #f (command 'shadowed-root))
(unload-package 'shadow-pkg)
(delete-file (package-main 'shadow-fail-pkg))
(delete-file (package-main 'shadow-pkg))
(delete-file shadow-fail-world)
(delete-file shadow-ok-world)

;; Primitives are sealed: shadowed by a definition, never changed.
(test-error (eval '(set! car cdr)))
(test 'shadowed (eval '(begin (define (car x) 'shadowed) (car '(1)))))

(test-end)
