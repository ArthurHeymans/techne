;;; Mail as a package: its files import its own libraries and Techne's,
;;; and no other package's, which its manifest depends on none of.
;;; Run: techne run packages/mail/tests/mail-package-tests.scm
(import (techne) (techne path) (techne uuid) (techne test))
(test-file #:script #t)
(import (mail date))

(define dir (path-join (or (get-environment-variable "TMPDIR") "/tmp") (string-append "techne-mail-package-" (uuid-v4))))
(make-directory (path-join dir "lib" "stranger"))
(call-with-output-file (path-join dir "lib" "stranger" "x.sld")
  (lambda (p) (write '(define-library (stranger x) (export x) (import (scheme base)) (begin (define x 1))) p)))
;; Another package's libraries, provided as its manifest would.
(%provide-libraries! "stranger" (path-join dir "lib"))

(define (why thunk) (guard (e (#t (condition/report-string e))) (thunk) #f))
(define here (path-directory (car (procedure-location why))))
(check "mail's own libraries" #t (procedure? mail-date-label))
(check "not another package's, from a file of mail's" #t
       (and (string-contains (or (why (lambda () (load-package 'mail-probe (path-join here "fixtures/imports-another-package.scm")))) "")
                             "the package mail does not depend on stranger")
            #t))
(check "which is not loaded for the attempt" #f (and (member "(stranger x)" (loaded-modules)) #t))
;; What comes from no file is no package's, wherever the program runs:
;; these suites run in mail's directory.
(check "an environment imports any" 1 (eval 'x (environment '(stranger x))))
(check "and so does an evaluation" 1 (eval '(begin (import (stranger x)) x)))
(delete-path dir #t)
(test-summary)
