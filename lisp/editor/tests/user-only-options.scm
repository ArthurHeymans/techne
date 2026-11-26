;;; Tests for #:user-only option metadata and principal/cell guards.
(import (techne test))
(test-file #:environment (quote editor))
(require "../main.scm")
(require "../modes.scm")
(require "../operations.scm")
(require "../session.scm")

(define s (start-session (make-view (make-document "") "user") "emacs"))
(define b (current-buffer s))

(define (raises? thunk)
  (guard (e (#t #t))
    (thunk)
    #f))

;; User declares and sets a user-only option globally.
(register-option! 'test-user-only-secret "safe-default" "Test user-only option." #:user-only #t)
(check "option-user-only? is true" #t (option-user-only? (find-option 'test-user-only-secret)))
(check "user sets user-only option globally" "user-val" (set-option! 'test-user-only-secret "user-val"))
(check "option returns global value" "user-val" (option b 'test-user-only-secret))

;; Per-buffer and per-mode overrides are rejected.
(check "set-option! rejects #:buffer on user-only option" #t
       (raises? (lambda () (set-option! 'test-user-only-secret "buf-val" #:buffer b))))
(check "set-option! rejects #:mode on user-only option" #t
       (raises? (lambda () (set-option! 'test-user-only-secret "mode-val" #:mode 'text-mode))))
(check "unset-option! rejects #:buffer on user-only option" #t
       (raises? (lambda () (unset-option! 'test-user-only-secret #:buffer b))))
(check "unset-option! rejects #:mode on user-only option" #t
       (raises? (lambda () (unset-option! 'test-user-only-secret #:mode 'text-mode))))

;; External agent principals cannot modify user-only options.
(let ((agent-op (start-operation! 'agent-test s #:principal "agent")))
  (call-in-operation agent-op
    (lambda ()
      (check "agent principal cannot set user-only option" #t
             (raises? (lambda () (set-option! 'test-user-only-secret "agent-evil"))))
      (check "agent principal cannot unset user-only option" #t
             (raises? (lambda () (unset-option! 'test-user-only-secret))))
      (check "agent principal cannot set user-only option via set-option command" #t
             (raises? (lambda () (invoke s 'set-option 'test-user-only-secret "agent-evil" #t #f)))))))
(check "user-only option preserved after rejected writes" "user-val" (option b 'test-user-only-secret))

;; explain-option and option ignore pre-existing buffer settings once marked user-only.
(set-option! 'test-late-user-only "buf-early" #:buffer b)
(register-option! 'test-late-user-only "clean-default" "Late user-only option." #:user-only #t)
(check "option ignores buffer setting for user-only option" "clean-default" (option b 'test-late-user-only))
(check "explain-option omits buffer cell for user-only option"
       '((default "clean-default" default))
       (explain-option b 'test-late-user-only))

;; User can unset the global value back to default.
(unset-option! 'test-user-only-secret)
(check "unset-option! restores default" "safe-default" (option b 'test-user-only-secret))

(test-failures)
