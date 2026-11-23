;;; Mail as a package: its files import its own libraries and Techne's,
;;; and no other package's, which its manifest depends on none of.
;;; Run: techne run packages/mail/tests/mail-package-tests.scm
(import (techne) (techne path) (techne uuid) (techne test))
(import (techne mail date))

(define dir (path-join (or (get-environment-variable "TMPDIR") "/tmp") (string-append "techne-mail-package-" (uuid-v4))))
(make-directory (path-join dir "lib" "other" "stranger"))
(call-with-output-file (path-join dir "lib" "other" "stranger" "x.sld")
  (lambda (p) (write '(define-library (other stranger x) (export x) (import (scheme base)) (begin (define x 1))) p)))
;; Another package's libraries, provided as its manifest would.
(%provide-libraries! '(other stranger) (path-join dir "lib"))

(define (why thunk) (guard (e (#t (condition/report-string e))) (thunk) #f))
(define here (path-directory (car (procedure-location why))))
(check "mail's own libraries" #t (procedure? mail-date-label))
(check "not another package's, from a file of mail's" #t
       (and (string-contains (or (why (lambda () (load-package 'mail-probe (path-join here "fixtures/imports-another-package.scm")))) "")
                             "the package (techne mail) does not depend on (other stranger)")
            #t))
(check "which is not loaded for the attempt" #f (and (member "(other stranger x)" (loaded-modules)) #t))
;; What comes from no file is no package's, wherever the program runs:
;; these suites run in mail's directory.
(check "an environment imports any" 1 (eval 'x (environment '(other stranger x))))
(check "and so does an evaluation" 1 (eval '(begin (import (other stranger x)) x)))
(delete-path dir #t)
(test-summary)
