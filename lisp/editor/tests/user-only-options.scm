;;; Negative tests for user-only option authority (scope identity vs scope name).
(require "../../test.scm")
(require "../main.scm")
(require "../modes.scm")
(require "../session.scm")
(import (techne trusted-process))

(define s (start-session (make-view (make-document "") "user") "emacs"))
(define b (current-buffer s))

(define (raises? thunk)
  (guard (e (#t #t))
    (thunk)
    #f))

;; Real root scope declares and sets a user-only option.
(register-option! 'test-user-only-secret "safe-default" "Test user-only option." #:user-only #t)
(check "root sets user-only option" "root-val" (set-option! 'test-user-only-secret "root-val"))
(check "option returns root value" "root-val" (option b 'test-user-only-secret))

;; A child scope merely named 'root must NOT pass root authority checks.
(let ((forged-root (make-scope 'root)))
  (with-scope forged-root
    (check "forged 'root scope cannot shadow user-only option" #t
           (raises? (lambda ()
                      (register-option! 'test-user-only-secret "evil" "Forged." #:user-only #f))))
    (check "forged 'root scope cannot re-declare user-only option even as user-only" #t
           (raises? (lambda ()
                      (register-option! 'test-user-only-secret "evil" "Forged." #:user-only #t))))
    (check "forged 'root scope cannot set user-only option" #t
           (raises? (lambda ()
                      (set-option! 'test-user-only-secret "evil"))))
    (check "forged 'root scope cannot unset user-only option" #t
           (raises? (lambda ()
                      (unset-option! 'test-user-only-secret))))
    (check "forged 'root scope cannot replace trusted direnv hook" #t
           (raises? (lambda ()
                      (set-trusted-direnv-program-hook! (lambda () "/tmp/evil-direnv"))))))
  (scope-shutdown! forged-root))

;; Value remains untouched after forged attempts.
(check "user-only option preserved after forged 'root attempts" "root-val" (option b 'test-user-only-secret))

;; Even if a forged 'root scope wrote a setting before an option became user-only,
;; option and explain-option must ignore non-root scope entries.
(let ((forged-root (make-scope 'root)))
  (with-scope forged-root
    (set-option! 'test-late-user-only "forged-early"))
  (register-option! 'test-late-user-only "clean-default" "Late user-only option." #:user-only #t)
  (check "option ignores pre-existing setting from forged 'root scope"
         "clean-default" (option b 'test-late-user-only))
  (check "explain-option excludes pre-existing setting from forged 'root scope"
         '((default "clean-default" default))
         (explain-option b 'test-late-user-only))
  (scope-shutdown! forged-root))

;; Mutable values (lists/strings) in user-only options must be isolated on storage
;; and on all read APIs (option, explain-option, option-default, set-option! return).
(let* ((def-item (string-copy "default-key"))
       (def-list (list def-item))
       (val-item (string-copy "trusted-recipient"))
       (val-list (list val-item)))
  (register-option! 'test-mutable-user-only def-list "Mutable list user-only option."
                    #:type '(list-of string) #:user-only #t)
  ;; Mutate caller's default container after registration.
  (string-fill! def-item #\D)
  (set-car! def-list "mutated-default")
  (check "option default unaffected by mutating registration argument"
         '("default-key") (option #f 'test-mutable-user-only))
  (let ((ret (set-option! 'test-mutable-user-only val-list)))
    ;; Mutate both setter argument and setter return value.
    (string-fill! val-item #\A)
    (set-car! val-list "mutated-arg")
    (string-fill! (car ret) #\R)
    (set-car! ret "mutated-ret"))
  ;; Child scope reads via option, explain-option, and option-default and mutates everything.
  (let ((pkg (make-scope 'mutator-pkg)))
    (with-scope pkg
      (let ((got (option #f 'test-mutable-user-only)))
        (string-fill! (car got) #\O)
        (set-car! got "mutated-option"))
      (let ((rows (explain-option #f 'test-mutable-user-only)))
        (for-each (lambda (row)
                    (let ((v (cadr row)))
                      (when (and (pair? v) (string? (car v)))
                        (string-fill! (car v) #\E)
                        (set-car! v "mutated-explain"))))
                  rows))
      (let ((d (option-default (find-option 'test-mutable-user-only))))
        (string-fill! (car d) #\F)
        (set-car! d "mutated-find-default")))
    (scope-shutdown! pkg))
  (check "user-only option value preserved after external and read-API mutations"
         '("trusted-recipient") (option #f 'test-mutable-user-only))
  (check "user-only option default preserved after read-API mutations"
         '("default-key") (option-default (find-option 'test-mutable-user-only)))
  (check "explain-option rows preserved after mutations"
         '((global ("trusted-recipient") root) (default ("default-key") default))
         (explain-option #f 'test-mutable-user-only)))

;; A child scope cannot escalate to the real root scope via `with-scope (scope-parent ...)`,
;; `parameterize`, or `%task-local-set!` to bypass user-only option or hook guards.
(let ((child (make-scope 'evil-child)))
  (with-scope child
    (check "child cannot rebind real parent via with-scope to set user-only option" #t
           (raises? (lambda ()
                      (with-scope (scope-parent (current-scope))
                        (set-option! 'test-user-only-secret "escalated")))))
    (check "child cannot rebind real parent via parameterize to set user-only option" #t
           (raises? (lambda ()
                      (parameterize ((current-scope (scope-parent (current-scope))))
                        (set-option! 'test-user-only-secret "escalated")))))
    (check "child cannot rebind real parent via with-scope to replace trusted direnv hook" #t
           (raises? (lambda ()
                      (with-scope (scope-parent (current-scope))
                        (set-trusted-direnv-program-hook! (lambda () "/tmp/evil-direnv")))))))
  (scope-shutdown! child))
(check "user-only option preserved after real-parent escalation attempts"
       "root-val" (option b 'test-user-only-secret))

;; Generic #:user-only options (#:type 'any) reject unsupported mutable objects (like
;; Scheme records) directly or nested in containers, unless a #:snapshot procedure is supplied.
(define-record-type mutable-box
  (make-mutable-box value)
  mutable-box?
  (value mutable-box-value set-mutable-box-value!))

(check "user-only option rejects mutable record as default without #:snapshot" #t
       (raises? (lambda ()
                  (register-option! 'test-rec-default-reject (make-mutable-box "a")
                                    "Bad default." #:user-only #t))))
(check "user-only option rejects nested mutable record as default without #:snapshot" #t
       (raises? (lambda ()
                  (register-option! 'test-rec-nested-default-reject (list (make-mutable-box "a"))
                                    "Bad nested default." #:user-only #t))))
(register-option! 'test-any-user-only '() "Generic user-only option." #:type 'any #:user-only #t)
(check "user-only option rejects mutable record in set-option! without #:snapshot" #t
       (raises? (lambda () (set-option! 'test-any-user-only (make-mutable-box "x")))))
(check "user-only option rejects nested mutable record in set-option! without #:snapshot" #t
       (raises? (lambda () (set-option! 'test-any-user-only (list (make-mutable-box "x"))))))

;; With an explicit #:snapshot procedure, custom mutable records ARE isolated on write & read.
(let ((snap (lambda (box)
              (unless (mutable-box? box) (error "expected mutable-box" box))
              (make-mutable-box (string-copy (mutable-box-value box)))))
      (init (make-mutable-box (string-copy "init")))
      (next (make-mutable-box (string-copy "trusted"))))
  (register-option! 'test-snap-user-only init "Snapshotted user-only option."
                    #:type 'any #:user-only #t #:snapshot snap)
  (set-mutable-box-value! init "corrupted-init")
  (check "snapshotted user-only default isolated from caller mutation"
         "init" (mutable-box-value (option #f 'test-snap-user-only)))
  (let ((ret (set-option! 'test-snap-user-only next)))
    (set-mutable-box-value! next "corrupted-arg")
    (set-mutable-box-value! ret "corrupted-ret"))
  (let ((got (option #f 'test-snap-user-only)))
    (set-mutable-box-value! got "corrupted-read"))
  (check "snapshotted user-only value isolated across write and read"
         "trusted" (mutable-box-value (option #f 'test-snap-user-only))))

(test-failures)
