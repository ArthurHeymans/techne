;;; Hold N idle connections to 127.0.0.1:PORT open until killed:
;;;
;;;   techne run idle.scm PORT N
;;;
;;; Says "ready" once they are all open.

(import (techne))

(define args (cdr (command-line)))
(define port (string->number (car args)))
(define held
  (map (lambda (_) (tcp-connect "127.0.0.1" port)) (iota (string->number (cadr args)))))
(display "ready\n")
(flush-output-port)
(let wait () (sleep 3600000) (wait))
