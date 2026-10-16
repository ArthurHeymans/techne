;;; Modules a top-level `require` loads, and packages, are evaluated from
;;; Scheme a form at a time: what they run can wait in a task.

(test-begin "loading")

;; Files beside this program, whose name differs between the runs in each
;; mode; required by their names relative to it.
(define base (car (command-line)))
(define (file name) (string-append base "." name ".scm"))
(define (relative name) (let ((p (file name))) (substring p (+ 1 (%last-slash p)) (string-length p))))
(define (%last-slash s)
  (let loop ((i (- (string-length s) 1))) (if (char=? (string-ref s i) #\/) i (loop (- i 1)))))
(define (write-file! name . forms)
  (call-with-output-file (file name) (lambda (port) (for-each (lambda (f) (write f port) (newline port)) forms))))
(define (in-task thunk) (task-join (spawn thunk)))

;; The module evaluated in: a file of its own, beside the others.
(write-file! "main" '(define main-module #t))
(define main (file "main"))

;; A module whose top level waits, required by one required in a task.
(write-file! "waits" '(provide twice waited) '(define (twice x) (* 2 x)) '(define waited (begin (sleep 1) 'yes)))
(write-file! "uses" `(require ,(relative "waits")) '(define uses-value (list (twice 21) waited)))
(test '(42 yes)
      (in-task (lambda () (eval-source (string-append "(require \"" (relative "uses") "\") uses-value") main main))))
;; Loaded once: requiring it again runs nothing.
(test '(42 yes) (in-task (lambda () (eval-source (string-append "(require \"" (relative "uses") "\") uses-value") main main))))

;; A handler of what a module's top level raises runs there and can wait.
(write-file! "raises" '(define asked (raise-continuable 'which?)))
(test 'answer
      (in-task (lambda ()
        (with-exception-handler (lambda (c) (sleep 1) 'answer)
          (lambda () (eval-source (string-append "(require \"" (relative "raises") "\") asked") main main))))))

;; A module that fails to load fails the require; one requiring itself is
;; told so.
(write-file! "fails" '(define before 1) '(car 1))
(test-error (in-task (lambda () (eval-source (string-append "(require \"" (relative "fails") "\")") main main))))
(write-file! "circular" `(require ,(relative "circular")))
(test "circular module dependency"
      (in-task (lambda ()
        (guard (e (#t (condition/report-string e)))
          (eval-source (string-append "(require \"" (relative "circular") "\")") main main)))))

;; A module whose loading task was killed is not loading for ever: it is
;; as far as it got, as a module that failed to load.
(write-file! "stuck" '(define stuck-before 'here) '(sleep 100000) '(define stuck-after 'never))
(define stuck (spawn (lambda () (eval-source (string-append "(require \"" (relative "stuck") "\")") main main))))
(sleep 5)
(task-kill stuck)
(test 'here (in-task (lambda () (eval-source (string-append "(require \"" (relative "stuck") "\") stuck-before") main main))))

;; A module whose own `require` is a procedure: it is called, nothing loads.
(write-file! "never" '(car 'loaded))
(write-file! "shadows" '(provide got) '(define got #f) '(define (require . files) (set! got files)) `(require ,(relative "never")))
(test (list (relative "never"))
      (in-task (lambda () (eval-source (string-append "(require \"" (relative "shadows") "\") got") main main))))

;; An operand the compiler cannot load stops the files after it loading.
(eval '(define side-loads 0) "root")
(write-file! "side" '(set! side-loads (+ side-loads 1)))
(test-error (in-task (lambda () (eval-source (string-append "(require \"" (relative "missing") "\" \"" (relative "side") "\")") main main))))
(test-error (in-task (lambda () (eval-source (string-append "(require 123 \"" (relative "side") "\")") main main))))
(write-file! "bad" '(provide absent))
(in-task (lambda () (guard (e (#t #f)) (eval-source (string-append "(require \"" (relative "bad") "\")") main main))))
(test-error (in-task (lambda () (eval-source (string-append "(require \"" (relative "bad") "\" \"" (relative "side") "\")") main main))))
(test 0 side-loads)

;; Modules are entered by their forms only.
(test-error (%enter-module -1))
(test-error (%enter-module (open-input-string "")))

;; A module another task is loading is told so, not taken for a cycle.
(eval '(define slow-go (make-channel)) "root")
(write-file! "slow" '(channel-recv slow-go) '(define slow-done #t))
(define slow-loader (spawn (lambda () (eval-source (string-append "(require \"" (relative "slow") "\")") main main))))
(sleep 5)
(test "module being loaded by another task"
      (in-task (lambda ()
        (guard (e (#t (condition/report-string e)))
          (eval-source (string-append "(require \"" (relative "slow") "\")") main main)))))
(channel-send slow-go #t)
(task-join slow-loader)

;; While a task stages a package, another task's require of a file in its
;; directory loads the published module, not one of the package's
;; generation: retired with it when the package fails, it would load again.
(eval '(define dep-loads 0) "root")
(eval '(define staging-go (make-channel)) "root")
(write-file! "dep" '(set! dep-loads (+ dep-loads 1)))
(write-file! "staged" '(channel-recv staging-go) '(car 1))
(define stager (spawn (lambda () (guard (e (#t 'failed)) (load-package 'staged (file "staged"))))))
(sleep 5)
(in-task (lambda () (eval-source (string-append "(require \"" (relative "dep") "\")") main main)))
(channel-send staging-go #t)
(test 'failed (task-join stager))
(in-task (lambda () (eval-source (string-append "(require \"" (relative "dep") "\")") main main)))
(test 1 dep-loads)

;; A package whose top level waits loads in a task.
(write-file! "package" '(define (package-hello) 'hello) '(define package-waited (begin (sleep 1) 'waited)))
(test 1 (in-task (lambda () (load-package 'loading-package (file "package")))))
(test 'waited (eval 'package-waited (package-module (find-package 'loading-package))))
(unload-package 'loading-package)

(test-end)
