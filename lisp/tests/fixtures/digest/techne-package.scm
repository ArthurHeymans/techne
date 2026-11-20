;;; A package without the editor that depends on another: mail, for its
;;; MIME library (lisp/tests/shipped-packages-tests.scm).
(package digest
  (depends (mail (path "../../../../packages/mail"))))
