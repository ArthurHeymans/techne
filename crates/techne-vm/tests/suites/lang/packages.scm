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

;; Primitives are sealed: shadowed by a definition, never changed.
(test-error (eval '(set! car cdr)))
(test 'shadowed (eval '(begin (define (car x) 'shadowed) (car '(1)))))

(test-end)
