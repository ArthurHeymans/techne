;;; End-to-end tests for `techne check` on standalone packages.
(import (techne) (techne test) (techne path))
(test-file #:environment 'host)

(define (run-techne-check dir . args)
  (call-with-process (test-executable) (cons "check" args)
    (lambda (p)
      (let* ((err-task (spawn (lambda () (process-read-all p 'stderr))))
             (out (process-read-all p 'stdout))
             (err (task-join err-task))
             (code (process-wait p)))
        (list code out err)))
    #:directory dir
    #:stdin 'null))

(define (write-file path text)
  (call-with-output-file path (lambda (p) (display text p))))

(define-suite "techne check CLI"
  (define-test "passes on a clean standalone package and fails on missing docstrings"
    (call-with-temporary-directory
      (lambda (root)
        (let ((pkg (path-join root "greeter")))
          (make-directory (path-join pkg "lib/greeter"))
          (write-file (path-join pkg "techne-package.scm") "(package greeter)\n")
          (write-file (path-join pkg "lib/greeter/core.sld")
            (string-append
              "(define-library (greeter core)\n"
              "  (export greet)\n"
              "  (import (scheme base))\n"
              "  (begin\n"
              "    (define (greet name)\n"
              "      \"Return a greeting for NAME.\"\n"
              "      (string-append \"hello \" name))))\n"))
          (let* ((ok (run-techne-check pkg "--format" "sexp"))
                 (data (read (open-input-string (cadr ok)))))
            (assert-equal 0 (car ok))
            (assert-equal 'ok (cadr (assq 'status data)))
            (assert-equal 0 (cadr (assq 'problems data))))
          ;; Non-matching -p fails cleanly (even alongside a matching -p)
          (let ((other (run-techne-check pkg "-p" "greeter" "-p" "nope")))
            (assert-equal 1 (car other))
            (assert-contains "package not found: nope" (cadr other)))
          ;; Explicit .sld file check works using declared library name
          (let ((one (run-techne-check pkg (path-join pkg "lib/greeter/core.sld"))))
            (assert-equal 0 (car one)))
          ;; Unsupported extension fails with exit 2
          (write-file (path-join pkg "README.txt") "not scheme\n")
          (let ((unsup (run-techne-check pkg (path-join pkg "README.txt"))))
            (assert-equal 2 (car unsup))
            (assert-contains "unsupported file extension" (caddr unsup)))
          ;; Break the docstring and verify `techne check` catches it
          (write-file (path-join pkg "lib/greeter/core.sld")
            (string-append
              "(define-library (greeter core)\n"
              "  (export greet)\n"
              "  (import (scheme base))\n"
              "  (begin\n"
              "    (define (greet name) (string-append \"hello \" name))))\n"))
          (let ((bad (run-techne-check pkg)))
            (assert-equal 1 (car bad))
            (assert-contains "Document it with a docstring." (cadr bad)))))))

  (define-test "checks generic user workspaces and non-built-in editor packages"
    (call-with-temporary-directory
      (lambda (root)
        (let ((pkg (path-join root "packages/myed")))
          (make-directory (path-join pkg "lib/myed"))
          (write-file (path-join root "techne-workspace.scm") "(workspace (packages \"packages\"))\n")
          (write-file (path-join pkg "techne-package.scm")
            "(package myed (editor \"editor.scm\" (commands (myed-hello \"Say hello.\"))))\n")
          (write-file (path-join pkg "lib/myed/util.sld")
            (string-append
              "(define-library (myed util)\n"
              "  (export greeting)\n"
              "  (import (scheme base))\n"
              "  (begin (define (greeting) \"Return a greeting.\" \"hi\")))\n"))
          (write-file (path-join pkg "editor.scm")
            (string-append
              "(import (techne editor) (myed util))\n"
              "(define-command (myed-hello s n) \"Say hello.\" (greeting))\n"))
          ;; Clean workspace + non-built-in editor package passes
          (let ((ok (run-techne-check root "--all")))
            (assert-equal 0 (car ok))
            (assert-contains "OK   package:myed (editor)" (cadr ok)))
          ;; Missing advertised command fails editor check
          (write-file (path-join pkg "editor.scm")
            "(import (techne editor) (myed util))\n")
          (let ((missing (run-techne-check root "--all")))
            (assert-equal 1 (car missing))
            (assert-contains "advertised in techne-package.scm" (cadr missing))))))))
