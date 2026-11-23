;;; Packages and generations: staged load, atomic publish, the previous
;;; generation kept on failure and shut after success.

(test-begin "packages")

;; What packages share with this file lives in the root module.
(eval '(define demo-commands (make-registry 'demo)) "root")
(eval '(define keeper (make-scope 'keeper)) "root")
(eval '(define keep-resource! (scope-procedure (lambda (r) (scope-transfer! r keeper)))) "root")
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
 ;; A task that must outlive a reload: moved to a longer-lived scope via
 ;; an explicit root-authorized capability, it finishes on its own
 ;; generation's code.
 '(keep-resource! (spawn (lambda () (sleep 30) (set-box! kept-result (version))))))

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
(eval '(define leaked-task (box #f)) "root")
(write-forms! failing-world
              '(set-box! leaked-task (spawn (lambda () (let loop () (sleep 1) (loop)))))
              '(registry-add! demo-commands 'leaked-reg (lambda () 'bad))
              '(error "world init failed"))
(make-directory (package-dir 'fails-world))
(write-forms! (package-main 'fails-world) `(require ,failing-world))
(test-error (load-package 'fails-world (package-main 'fails-world)))
(sleep 5)
(test #t (task-done? (unbox (eval 'leaked-task "root"))))
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
(define sync-host-file (string-append sync-lib-dir "/synclib/host.sld"))
(write-forms! sync-host-file
              '(define-library (synclib host)
                 (export sync-registry set-sync-registry!)
                 (import (techne))
                 (begin
                   (define sync-registry (box #f))
                   (define (set-sync-registry! r) (set-box! sync-registry r)))))
(define sync-lib-file (string-append sync-lib-dir "/synclib/shared.sld"))
(write-forms! sync-lib-file
              '(define-library (synclib shared)
                 (export sync-worker)
                 (import (techne) (synclib host))
                 (begin
                   (define sync-worker (spawn (lambda () (let loop () (sleep 1) (loop)))))
                   (registry-add! (unbox sync-registry) 'from-sync-lib (lambda () sync-worker)))))
(%provide-libraries! "synclib" sync-lib-dir)
(eval '(begin (import (synclib host)) (set-sync-registry! demo-commands)) "root")
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
(delete-file sync-host-file)

;; Primitives are sealed: shadowed by a definition, never changed.
(test-error (eval '(set! car cdr)))
(test 'shadowed (eval '(begin (define (car x) 'shadowed) (car '(1)))))

;; Round 19: untrusted external files, provide_libraries dependencies, and
;; load-module-from text never receive root authority during package staging,
;; while package cleanups on unload also run without root authority.
(eval '(define r19-pkg-param (make-root-parameter "safe")) "root")
(eval '(define r19-auth-log (box '())) "root")
(define untrusted-ext (string-append path ".untrusted-ext.scm"))
(write-forms! untrusted-ext
              '(provide ext-auth)
              '(define ext-auth (list (root-authority?) (guard (e (#t #f)) (parameterize ((r19-pkg-param "pwn")) #t)))))
(define untrusted-lib-dir (string-append path ".untrusted-lib"))
(make-directory untrusted-lib-dir)
(make-directory (string-append untrusted-lib-dir "/untrustedlib"))
(define untrusted-host-file (string-append untrusted-lib-dir "/untrustedlib/host.sld"))
(write-forms! untrusted-host-file
              '(define-library (untrustedlib host)
                 (export untrusted-param-box set-untrusted-param!)
                 (import (techne))
                 (begin
                   (define untrusted-param-box (box #f))
                   (define (set-untrusted-param! p) (set-box! untrusted-param-box p)))))
(define untrusted-lib-file (string-append untrusted-lib-dir "/untrustedlib/dep.sld"))
(write-forms! untrusted-lib-file
              '(define-library (untrustedlib dep)
                 (export lib-auth)
                 (import (techne) (untrustedlib host))
                 (begin
                   (define lib-auth (list (root-authority?) (guard (e (#t #f)) (parameterize (((unbox untrusted-param-box) "pwn")) #t)))))))
(%provide-libraries! "untrustedlib" untrusted-lib-dir)
(eval '(begin (import (untrustedlib host)) (set-untrusted-param! r19-pkg-param)) "root")
(define fake-text-path (string-append path ".fake-text.scm"))
(write-forms! fake-text-path '42)
(make-directory (package-dir 'r19-sec))
(write-forms! (package-main 'r19-sec)
              `(require ,untrusted-ext)
              '(eval '(import (untrustedlib dep)))
              `(let ((text-auth (load-module-from ,fake-text-path "(root-authority?)")))
                 (set-box! r19-auth-log (list ext-auth lib-auth text-auth)))
              '(scope-own! (list 'pkg-res)
                 (lambda (r) (set-box! r19-auth-log (cons (list 'cleanup (root-authority?)) (unbox r19-auth-log))))
                 (lambda (r) (set-box! r19-auth-log (cons (list 'done (root-authority?)) (unbox r19-auth-log))) #f)))
(load-package 'r19-sec (package-main 'r19-sec))
(test '((#f #f) (#f #f) (#f)) (unbox (eval 'r19-auth-log "root")))
(unload-package 'r19-sec)
(test '((cleanup #f) (done #f) (#f #f) (#f #f) (#f)) (unbox (eval 'r19-auth-log "root")))
(test "safe" ((eval 'r19-pkg-param "root")))
(delete-file (package-main 'r19-sec))
(delete-file untrusted-ext)
(delete-file untrusted-lib-file)
(delete-file untrusted-host-file)
(delete-file fake-text-path)

;; Round 20 (Finding 1): possessing dependency forms for a trusted core
;; library (via %next-form) does not grant ambient root authority to the
;; caller; %world-load-enter! is sealed, and %load-forms executes only the
;; verified forms and restores caller authority before returning.
(define r20-dep-scope (make-scope 'r20-dep-scope))
(let* ((r20-param (eval 'r19-pkg-param "root"))
       (res
        (with-scope r20-dep-scope
          (let* ((f (%read-forms "(import (techne authinfo))" "user" "r20-probe.scm"))
                 (dep (%next-form f))
                 (enter-unbound (guard (e (#t #t)) (eval '%world-load-enter!) #f))
                 (auth-before (root-authority?))
                 (dep-loaded (and (not (eof-object? dep)) (not (procedure? dep)))))
            (when dep-loaded
              (%load-forms dep))
            (%drop-forms f)
            (list dep-loaded
                  enter-unbound
                  auth-before
                  (root-authority?)
                  (guard (e (#t #f)) (parameterize ((r20-param "pwn")) #t)))))))
  (test '(#t #t #f #f #f) res))
(scope-shutdown! r20-dep-scope)

;; Round 21 (Finding 1): a failing staged dependency load invoked from a
;; narrower scope inside a staged package must restore the narrower caller's
;; scope and authority BEFORE running the caller's exception handler.
(eval '(define r21-stage-handler-res (box 'unset)) "root")
(eval '(define r21-stage-forms-res (box 'unset)) "root")
(define r21-stage-dir (package-dir 'r21-stage-pkg))
(make-directory r21-stage-dir)
(define r21-stage-dep (string-append r21-stage-dir "/dep-boom.scm"))
(define r21-stage-dep2 (string-append r21-stage-dir "/dep-forms-boom.scm"))
(define r21-stage-main (package-main 'r21-stage-pkg))
(write-forms! r21-stage-dep '(error "r21 staged dep boom"))
(write-forms! r21-stage-main
              `(let* ((pkg-scope (current-scope))
                      (narrow (make-scope 'r21-narrow-child))
                      (check-handler
                       (lambda (slot thunk)
                         (with-scope narrow
                           (call/cc
                            (lambda (k)
                              (with-exception-handler
                               (lambda (e)
                                 (let ((cur-ok (eq? (current-scope) narrow))
                                       (auth-ok (eq? (authority-scope) narrow))
                                       (own-blocked (guard (err (#t #t))
                                                      (%scope-own! pkg-scope (list 'probe) (lambda (_) #f) (lambda (_) #f))
                                                      #f))
                                       (child-blocked (guard (err (#t #t))
                                                        (with-scope pkg-scope (make-scope 'r21-escalated))
                                                        #f)))
                                   (set-box! slot (list #t cur-ok auth-ok own-blocked child-blocked))
                                   (k 'handled)))
                               thunk)))))))
                 (check-handler r21-stage-handler-res
                                (lambda () (eval (list 'require ,r21-stage-dep))))
                 (check-handler r21-stage-forms-res
                                (lambda () (load-module-from ,r21-stage-dep2 "(error \"r21 staged forms boom\")")))))
(test 1 (load-package 'r21-stage-pkg r21-stage-main))
(test '(#t #t #t #t #t) (unbox (eval 'r21-stage-handler-res "root")))
(test '(#t #t #t #t #t) (unbox (eval 'r21-stage-forms-res "root")))
(unload-package 'r21-stage-pkg)
(delete-file r21-stage-main)
(delete-file r21-stage-dep)

;; Round 22 (Finding 1): staging authority verifies provenance of the forms,
;; not just the destination module. Caller-supplied %read-forms, load-module-from,
;; and inline (eval '(define-library ...)) from a narrower child scope must stay
;; in the narrower scope, while a real disk-backed staged dependency runs with
;; package scope and restores the narrower scope on return.
(eval '(define r22-prov-res (box 'unset)) "root")
(eval '(define r22-prov-disk-res (box 'unset)) "root")
(eval '(define r22-prov-pkg-scope (box #f)) "root")
(eval '(define r22-prov-narrow-scope (box #f)) "root")
(define r22-prov-dir (package-dir 'r22-stage-prov))
(make-directory r22-prov-dir)
(define r22-prov-dep (string-append r22-prov-dir "/disk-dep.scm"))
(define r22-prov-text-path (string-append r22-prov-dir "/inline-text.scm"))
(write-forms! r22-prov-text-path '42)
(define r22-prov-main (package-main 'r22-stage-prov))
(write-forms! r22-prov-dep
              '(set-box! r22-prov-disk-res
                         (list (eq? (current-scope) (unbox r22-prov-pkg-scope))
                               (eq? (authority-scope) (unbox r22-prov-pkg-scope)))))
(write-forms! r22-prov-main
              `(let* ((pkg-scope (current-scope))
                      (narrow (make-scope 'r22-narrow)))
                 (set-box! r22-prov-pkg-scope pkg-scope)
                 (set-box! r22-prov-narrow-scope narrow)
                 (with-scope narrow
                   (let* ((f (%read-forms
                              "(list (eq? (current-scope) (unbox r22-prov-narrow-scope))
                                     (eq? (authority-scope) (unbox r22-prov-narrow-scope))
                                     (guard (e (#t #t))
                                       (%scope-own! (unbox r22-prov-pkg-scope) (list 'r22-rf) (lambda (_) #f) (lambda (_) #f))
                                       #f))"
                              (current-module)
                              "r22-caller-forms.scm"))
                          (rf-res (%load-forms f))
                          (lmf-res (load-module-from
                                    ,r22-prov-text-path
                                    "(list (eq? (current-scope) (unbox r22-prov-narrow-scope))
                                           (eq? (authority-scope) (unbox r22-prov-narrow-scope))
                                           (guard (e (#t #t))
                                             (%scope-own! (unbox r22-prov-pkg-scope) (list 'r22-lmf) (lambda (_) #f) (lambda (_) #f))
                                             #f))"))
                          (dl-res (begin
                                    (eval '(define-library (r22-inline-lib probe)
                                             (export dl-cur dl-auth)
                                             (import (techne))
                                             (begin
                                               (define dl-cur (current-scope))
                                               (define dl-auth (authority-scope)))))
                                    (eval '(import (r22-inline-lib probe)))
                                    (list (eq? (eval 'dl-cur) narrow)
                                          (eq? (eval 'dl-auth) narrow))))
                          (after-disk (begin
                                        (eval (list 'require ,r22-prov-dep))
                                        (list (eq? (current-scope) narrow)
                                              (eq? (authority-scope) narrow)))))
                     (set-box! r22-prov-res (list rf-res lmf-res dl-res after-disk))))))
(test 1 (load-package 'r22-stage-prov r22-prov-main))
(let ((res (unbox (eval 'r22-prov-res "root"))))
  (test '(#t #t #t) (list-ref res 0))
  (test '((#t #t #t)) (list-ref res 1))
  (test '(#t #t) (list-ref res 2))
  (test '(#t #t) (list-ref res 3)))
(test '(#t #t) (unbox (eval 'r22-prov-disk-res "root")))
(unload-package 'r22-stage-prov)
(delete-file r22-prov-main)
(delete-file r22-prov-dep)
(delete-file r22-prov-text-path)

;; Round 22 (Finding 2): non-root scopes cannot evaluate into or enter other
;; existing modules (root or libraries), nor call %package-begin/%package-end,
;; while current-module eval and explicit (environment ...) eval still work.
(define r22-mod-scope (make-scope 'r22-mod-scope))
(define r22-mod-dir (package-dir 'r22-forged-pkg))
(test-error (with-scope r22-mod-scope (eval '(+ 1 2) "root")))
(test-error (with-scope r22-mod-scope (eval-source "(+ 1 2)" "root" "r22-probe.scm")))
(test-error (with-scope r22-mod-scope (%read-forms "(+ 1 2)" "root" "r22-probe.scm")))
(test-error (with-scope r22-mod-scope (in-module "root")))
(test-error (with-scope r22-mod-scope (%enter-module "root")))
(test-error (with-scope r22-mod-scope (eval 'parse-authinfo "(techne authinfo)")))
(test-error (with-scope r22-mod-scope (%package-begin 'r22-forged-pkg r22-mod-dir)))
(test-error (with-scope r22-mod-scope (%package-end #f)))
(test 3 (with-scope r22-mod-scope (eval '(+ 1 2))))
(test 3 (with-scope r22-mod-scope (eval '(+ 1 2) (environment '(scheme base)))))
(test-error (with-scope r22-mod-scope (eval 'r19-pkg-param (environment '(scheme base)))))
(scope-shutdown! r22-mod-scope)

(test-end)
