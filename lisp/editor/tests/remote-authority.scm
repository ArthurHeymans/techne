;;; Tests for privileged lazy initialization and authority enforcement in
;;; `lisp/editor/remote.scm`, `(techne endpoint)`, and `(techne execution-site)`.
(require "../../test.scm")
(require "../main.scm")
(require "../modes.scm")
(require "../remote.scm")

(define (raises? thunk)
  (guard (e (#t #t))
    (thunk)
    #f))

;; Neither endpoint nor execution-site is loaded yet in this fresh editor VM.
(check "execution-site starts unloaded before first lazy init" #f (execution-site-loaded?))

;; First legitimate lazy initialization originating from a child scope must succeed
;; via boot-captured root capabilities, while subsequent child attempts to replace
;; hooks or options remain rejected.
(let ((pkg (make-scope 'untrusted-pkg)))
  (with-scope pkg
    (let ((es-env (ensure-execution-site!)))
      (check "child-triggered first lazy init loads execution-site" #t (execution-site-loaded?))
      (check "child-triggered first lazy init returns environment" #t (and es-env #t)))
    (let ((ep-env (environment '(techne endpoint)))
          (es-env (environment '(techne execution-site)))
          (real-root (scope-parent (current-scope))))
      (check "child cannot replace remote-node-command hook" #t
             (raises? (lambda ()
                        ((eval 'set-remote-node-command-hook! ep-env) (lambda () '("evil"))))))
      (check "child cannot escalate via with-scope real-root to replace remote-node-command hook" #t
             (raises? (lambda ()
                        (with-scope real-root
                          ((eval 'set-remote-node-command-hook! ep-env) (lambda () '("evil")))))))
      (check "child cannot escalate via parameterize current-scope to replace remote-node-command hook" #t
             (raises? (lambda ()
                        (parameterize ((current-scope real-root))
                          ((eval 'set-remote-node-command-hook! ep-env) (lambda () '("evil")))))))
      (check "child cannot rebind default-remote-node-command" #t
             (raises? (lambda ()
                        (parameterize (((eval 'default-remote-node-command ep-env) '("evil")))
                          #t))))
      (check "child cannot set remote-node-command editor option" #t
             (raises? (lambda ()
                        (set-option! 'remote-node-command '("evil")))))
      (check "child cannot replace direnv program hook" #t
             (raises? (lambda ()
                        ((eval 'set-direnv-program-hook! es-env) (lambda () "/tmp/evil")))))))
  (scope-shutdown! pkg))

;; Root authority can still configure remote-node-command and read an isolated copy.
(let ((cmd (list (string-copy "custom-techne-node"))))
  (set-option! 'remote-node-command cmd)
  (string-fill! (car cmd) #\X)
  (check "root remote-node-command option value is isolated"
         '("custom-techne-node")
         (option #f 'remote-node-command)))

;; Protected parameters resist raw %task-local-* tampering, record-fields
;; inspection, and inherited-value mutation from child tasks.
(let* ((ep-env (environment '(techne endpoint)))
       (es-env (environment '(techne execution-site)))
       (p-cmd (eval 'default-remote-node-command ep-env))
       (p-prog (eval 'default-direnv-program es-env))
       (p-data (eval 'default-direnv-data-dir es-env))
       (p-timeout (eval 'default-direnv-timeout es-env))
       (p-provs (eval 'default-env-providers es-env))
       (pkg (make-scope 'r19-param-attacker)))
  (parameterize ((p-cmd (list (string-copy "/usr/bin/techne-node")))
                 (p-prog (string-copy "/usr/bin/direnv"))
                 (p-data (string-copy "/tmp/direnv-data")))
    (with-scope pkg
      (check "child cannot inspect protected parameter via record-fields"
             '() (record-fields p-prog))
      (check "child cannot inspect protected parameter via %record-ref" #t
             (raises? (lambda () (%record-ref p-prog %parameter-type 0))))
      (check "child cannot replace protected parameter reader via %record-set!" #t
             (raises? (lambda () (%record-set! p-prog %parameter-type 0 (lambda () "/tmp/evil")))))
      (check "child cannot bypass via %protected-param-set!" #t
             (raises? (lambda () (%protected-param-set! p-prog "/tmp/evil"))))
      (check "child cannot bypass via %protected-param-save" #t
             (raises? (lambda () (%protected-param-save p-prog))))
      (check "consumer still observes original default-direnv-program after tampering attempts"
             "/usr/bin/direnv" (p-prog))
      (check "child cannot rebind default-direnv-program" #t
             (raises? (lambda () (parameterize ((p-prog "/tmp/evil")) #t))))
      (check "child cannot rebind default-direnv-timeout" #t
             (raises? (lambda () (parameterize ((p-timeout 1)) #t))))
      (check "child cannot rebind default-env-providers" #t
             (raises? (lambda () (parameterize ((p-provs '())) #t))))
      (let ((t (spawn (lambda ()
                        (let ((s (p-prog)) (c (p-cmd)) (d (p-data)))
                          (string-set! s 0 #\X)
                          (string-set! (car c) 0 #\X)
                          (string-set! d 0 #\X)
                          (list (p-prog) (p-cmd) (p-data)))))))
        (check "child task mutation does not alias inherited protected parameters"
               '("/usr/bin/direnv" ("/usr/bin/techne-node") "/tmp/direnv-data")
               (task-join t))))
    (check "parent retains unmutated protected parameter values"
           '("/usr/bin/direnv" ("/usr/bin/techne-node") "/tmp/direnv-data")
           (list (p-prog) (p-cmd) (p-data))))
  (scope-shutdown! pkg))

(test-failures)
