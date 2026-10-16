;;; `eval` and `eval-source` compile a form at a time and call it from
;;; Scheme: what they evaluate can wait in a task.

(test-begin "eval")

(define (waits v) (sleep 1) v)

(test 3 (eval '(+ 1 2)))
(test 8 (task-join (spawn (lambda () (eval '(+ 1 (waits 7)))))))
(test 42 (task-join (spawn (lambda () (eval-source "(define w (waits 7)) (* w 6)" "user" "w.scm")))))

;; A form may use what the forms before it defined, macros included.
(test 12 (eval-source "(define x 10) (define-syntax twice (syntax-rules () ((_ e) (begin e e)))) (twice (set! x (+ x 1))) x"
                      "user" "macros.scm"))

;; The module evaluated in is current while it runs, and only then.
(test "root" (eval '(current-module) "root"))
(test "user" (current-module))
(test "user" (begin (eval '(in-module "root")) (current-module)))
(test "root" (eval-source "(in-module \"user\") (in-module \"root\") (current-module)" "user" "m.scm"))
(test "user" (current-module))

;; The current module is the task's own.
(test '("root" "user")
      (let ((t (spawn (lambda () (in-module "root") (waits (current-module))))))
        (list (task-join t) (current-module))))

;; Errors name the file and position.
(test "car: expected pair, got 1" (guard (e (#t (condition/report-string e))) (eval-source "(car 1)" "user" "e.scm")))
(test "e.scm:1:1: unexpected end of input: the list here is not closed"
      (guard (e (#t (condition/report-string e))) (eval-source "(car" "user" "e.scm")))

;; A handler of what the evaluated code raises runs where it was raised,
;; and can wait there.
(test 'handled (task-join (spawn (lambda ()
  (with-exception-handler (lambda (c) (waits 'handled))
    (lambda () (eval-source "(raise-continuable 'x)" "user" "h.scm")))))))

(test-end)
