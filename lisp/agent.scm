;;; `techne agent ROOT FORM [SESSION]`: an ordinary node client, with the
;;; same library available from `run` and nREPL. No new wire protocol.
(import (techne))
(define args (cdr (command-line)))
(define executable (list-ref args 0))
(define root (list-ref args 1))
(define source (list-ref args 2))
(define session (list-ref args 3))
(define (written obj)
  (let ((p (open-output-string))) (write obj p) (get-output-string p)))
;; Exactly one form is nested as syntax; never interpolate path strings.
(define form (read (open-input-string source)))
(when (eof-object? form) (error "techne agent: expected a Scheme form"))
(define check (open-input-string source))
(read check)
(unless (eof-object? (read check)) (error "techne agent: expected one form; use begin or list to batch"))
(define node (node-connect (list executable "node" "--session" session)))
(dynamic-wind
  (lambda () #f)
  (lambda ()
    (let ((result (node-eval node
                  (string-append "(import (techne agent))\n"
                    (written `(let ((project (agent-session-project ,root))) ,form))))))
      (write result) (newline)))
  (lambda () (node-close node)))
