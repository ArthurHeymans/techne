;;; End-to-end tests for the `techne test` CLI runner.
(import (techne) (techne test) (techne path))
(test-file #:environment 'host)

(define (run-techne-test dir . args)
  (call-with-process (test-executable) (cons "test" args)
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

(define-suite "techne test CLI"
  (define-test "discovers package tests, runs passing/skipped tests, and formats sexp"
    (call-with-temporary-directory
      (lambda (root)
        (let ((pkg (path-join root "demo")))
          (make-directory (path-join pkg "lib/demo"))
          (make-directory (path-join pkg "tests"))
          (write-file (path-join pkg "techne-package.scm") "(package demo)\n")
          (write-file (path-join pkg "lib/demo/math.sld")
            "(define-library (demo math)\n  (export double)\n  (import (scheme base))\n  (begin (define (double x) (* 2 x))))\n")
          (write-file (path-join pkg "tests/math-test.scm")
            (string-append
              "(import (techne test) (demo math))\n"
              "(define-test \"doubles integers\"\n"
              "  (assert-equal 8 (double 4)))\n"
              "(define-test (\"skipped case\" #:skip \"not yet\")\n"
              "  (assert-true #f))\n"))
          ;; --list discovers the package test with package-prefixed id
          (let ((listed (run-techne-test pkg "--list")))
            (assert-equal 0 (car listed))
            (assert-match "demo:tests/math-test.scm" (cadr listed)))
          ;; --format sexp runs and reports counts
          (let* ((res (run-techne-test pkg "--format" "sexp"))
                 (data (read (open-input-string (cadr res)))))
            (assert-equal 0 (car res))
            (assert-equal 'ok (cadr (assq 'status data)))
            (assert-equal 1 (cadr (assq 'passed data)))
            (assert-equal 0 (cadr (assq 'failed data)))
            (assert-equal 1 (cadr (assq 'skipped data))))))))

  (define-test "isolates failing tests, reports diffs and backtraces, and supports filtering"
    (call-with-temporary-directory
      (lambda (root)
        (let ((pkg (path-join root "sample")))
          (make-directory (path-join pkg "tests"))
          (write-file (path-join pkg "techne-package.scm") "(package sample)\n")
          (write-file (path-join pkg "tests/cases-test.scm")
            (string-append
              "(import (techne test))\n"
              "(define-test \"alpha passes\"\n"
              "  (assert-equal 1 1))\n"
              "(define-test \"beta fails with diff\"\n"
              "  (assert-equal '(1 2 3) '(1 99 3)))\n"
              "(define-test \"gamma raises uncaught\"\n"
              "  (define (helper) (error \"kaboom\"))\n"
              "  (helper))\n"))
          ;; Running all tests reports 1 passed, 2 failed, with diff + backtrace
          (let ((res (run-techne-test pkg)))
            (assert-equal 1 (car res))
            (assert-match "1 passed, 2 failed" (cadr res))
            (assert-match "first diff at (1)" (cadr res))
            (assert-match "kaboom" (cadr res))
            (assert-match "helper" (cadr res)))
          ;; Filtering by test name runs only the matching test and passes
          (let ((res (run-techne-test pkg "alpha")))
            (assert-equal 0 (car res))
            (assert-match "1 passed, 0 failed" (cadr res)))
          ;; Filtering with no match fails cleanly
          (let ((res (run-techne-test pkg "does-not-exist")))
            (assert-equal 1 (car res))
            (assert-match "filter matched 0 tests" (cadr res)))))))

  (define-test "works with #:without '(environment) and #:without '(loading)"
    (call-with-temporary-directory
      (lambda (root)
        (let ((pkg (path-join root "caps")))
          (make-directory (path-join pkg "tests"))
          (write-file (path-join pkg "techne-package.scm") "(package caps)\n")
          (write-file (path-join pkg "tests/no-env-test.scm")
            (string-append
              "(import (techne test))\n"
              "(test-file #:environment 'vm #:without '(environment))\n"
              "(define-test \"env capability is absent\"\n"
              "  (assert-raises (get-environment-variable \"PATH\")))\n"))
          (write-file (path-join pkg "tests/no-load-test.scm")
            (string-append
              "(import (techne test))\n"
              "(test-file #:environment 'vm #:without '(loading))\n"
              "(define-test \"loading capability is absent\"\n"
              "  (assert-equal 4 (+ 2 2)))\n"))
          (let ((res (run-techne-test pkg)))
            (assert-equal 0 (car res))
            (assert-match "2 passed, 0 failed" (cadr res)))))))

  (define-test "fails closed on misplaced or invalid test-file declarations"
    (call-with-temporary-directory
      (lambda (root)
        (let ((pkg (path-join root "badmeta")))
          (make-directory (path-join pkg "tests"))
          (write-file (path-join pkg "techne-package.scm") "(package badmeta)\n")
          (write-file (path-join pkg "tests/misplaced-test.scm")
            (string-append
              "(import (techne test))\n"
              "(define x 1)\n"
              "(test-file #:environment 'host #:without '(files))\n"
              "(define-test \"should fail\" (assert-equal 1 x))\n"))
          (write-file (path-join pkg "tests/invalid-test.scm")
            (string-append
              "(import (techne test))\n"
              "(test-file #:environment 'bogus)\n"
              "(define-test \"never\" (assert-true #t))\n"))
          (write-file (path-join pkg "tests/misplaced-serial-test.scm")
            (string-append
              "(import (techne test))\n"
              "(define y 2)\n"
              "(test-file #:serial #t)\n"
              "(define-test \"should fail\" (assert-equal 2 y))\n"))
          (let ((res (run-techne-test pkg)))
            (assert-equal 1 (car res))
            (assert-match "must appear at the top of the file" (cadr res))
            (assert-match "invalid test-file declaration" (cadr res)))))))

  (define-test "assert-raises propagates skip/failure, soft check beats later skip, and -p outside workspace works"
    (call-with-temporary-directory
      (lambda (root)
        (let ((pkg (path-join root "solo")))
          (make-directory (path-join pkg "tests"))
          (write-file (path-join pkg "techne-package.scm") "(package solo)\n")
          (write-file (path-join pkg "tests/edge-test.scm")
            (string-append
              "(import (techne test))\n"
              "(define-test \"assert-raises propagates skip\"\n"
              "  (assert-raises (skip \"unavailable\")))\n"
              "(define-test \"assert-raises propagates inner assert-equal failure\"\n"
              "  (assert-raises (assert-equal 1 2)))\n"
              "(define-test \"soft check failure beats later skip\"\n"
              "  (check \"mismatch\" 1 2)\n"
              "  (skip \"later skip\"))\n"))
          ;; -p with non-matching name outside a workspace must fail
          (let ((other (run-techne-test pkg "-p" "other")))
            (assert-equal 1 (car other))
            (assert-match "no test files found" (caddr other)))
          ;; -p matching the standalone package runs its tests
          (let* ((res (run-techne-test pkg "-p" "solo" "--format" "sexp"))
                 (data (read (open-input-string (cadr res)))))
            (assert-equal 1 (car res))
            (assert-equal 0 (cadr (assq 'passed data)))
            (assert-equal 2 (cadr (assq 'failed data)))
            (assert-equal 1 (cadr (assq 'skipped data))))))))

  (define-test "handles ESC in load errors, partial stdout before events, and nested techne run"
    (call-with-temporary-directory
      (lambda (root)
        (let ((pkg (path-join root "edge")))
          (make-directory (path-join pkg "tests"))
          (write-file (path-join pkg "techne-package.scm") "(package edge)\n")
          ;; Load error containing ESC byte (\x1b;) must not crash worker formatting
          (write-file (path-join pkg "tests/esc-err-test.scm")
            "(import (techne test))\n(error (string-append \"esc \" (string (integer->char 27)) \" boom\"))\n")
          ;; Top-level display without newline must not corrupt protocol framing
          (write-file (path-join pkg "tests/partial-out-test.scm")
            "(import (techne test))\n(display \"no-newline\")\n(define-test \"still passes\" (assert-equal 2 (+ 1 1)))\n")
          ;; Nested `techne run` must NOT inherit runner mode
          (write-file (path-join pkg "child.scm")
            "(import (techne test))\n(define-test \"child fails\" (assert-equal 1 2))\n(test-summary)\n")
          (write-file (path-join pkg "tests/nested-run-test.scm")
            (string-append
              "(import (techne) (techne test))\n"
              "(test-file #:environment 'host)\n"
              "(define-test \"nested techne run exits nonzero on failure\"\n"
              "  (let ((code (call-with-process (test-executable) (list \"run\" "
              (call-with-output-string (lambda (p) (write (path-join pkg "child.scm") p)))
              ") process-wait #:stdin 'null #:stdout 'null #:stderr 'null)))\n"
              "    (assert-equal 1 code)))\n"))
          (let ((r1 (run-techne-test pkg (path-join pkg "tests/esc-err-test.scm"))))
            (assert-equal 1 (car r1))
            (assert-match "esc " (cadr r1))
            (assert-false (string-contains (cadr r1) "unknown escape")))
          (let ((r2 (run-techne-test pkg (path-join pkg "tests/partial-out-test.scm"))))
            (assert-equal 0 (car r2))
            (assert-match "1 passed, 0 failed" (cadr r2)))
          (let ((r3 (run-techne-test pkg (path-join pkg "tests/nested-run-test.scm"))))
            (assert-equal 0 (car r3))
            (assert-match "1 passed, 0 failed" (cadr r3)))))))

  (define-test "timeout runs dynamic-wind cleanup, directory targets scope discovery, and missing paths fail"
    (call-with-temporary-directory
      (lambda (root)
        (let ((pkg (path-join root "scope"))
              (marker (path-join root "cleaned.txt")))
          (make-directory (path-join pkg "tests/sub"))
          (write-file (path-join pkg "techne-package.scm") "(package scope)\n")
          (write-file (path-join pkg "tests/root-test.scm")
            "(import (techne test))\n(define-test \"root\" (assert-true #t))\n")
          (write-file (path-join pkg "tests/sub/timeout-test.scm")
            (string-append
              "(import (techne test))\n"
              "(define-test (\"times out with cleanup\" #:timeout 1)\n"
              "  (dynamic-wind\n"
              "    (lambda () #f)\n"
              "    (lambda () (sleep 10000))\n"
              "    (lambda () (call-with-output-file "
              (call-with-output-string (lambda (p) (write marker p)))
              " (lambda (p) (display \"cleaned\" p))))))\n"))
          ;; Directory target scopes --list to tests/sub only
          (let ((listed (run-techne-test pkg "--list" (path-join pkg "tests/sub"))))
            (assert-equal 0 (car listed))
            (assert-match "timeout-test.scm" (cadr listed))
            (assert-false (string-contains (cadr listed) "root-test.scm")))
          ;; Missing path fails with exit 2
          (let ((missing (run-techne-test pkg "tests/does-not-exist.scm")))
            (assert-equal 2 (car missing))
            (assert-match "path not found" (caddr missing)))
          ;; Per-test timeout fails AND runs dynamic-wind cleanup
          (let ((timed (run-techne-test pkg (path-join pkg "tests/sub/timeout-test.scm"))))
            (assert-equal 1 (car timed))
            (assert-match "timed out after 1s" (cadr timed))
            (assert-true (file-exists? marker))
            (assert-equal "cleaned" (file->string marker)))
          ;; Soft check failure inside define-test fails the test
          (write-file (path-join pkg "tests/soft-test.scm")
            "(import (techne test))\n(define-test \"soft check\" (check \"mismatch\" 1 2))\n")
          (let ((soft (run-techne-test pkg (path-join pkg "tests/soft-test.scm"))))
            (assert-equal 1 (car soft))
            (assert-match "check mismatch" (cadr soft)))
          ;; --list-tests surfaces load errors and exits non-zero
          (write-file (path-join pkg "tests/broken-test.scm")
            "(import (techne test))\n(error \"broken load\")\n")
          (let ((lt (run-techne-test pkg "--list-tests" (path-join pkg "tests/broken-test.scm"))))
            (assert-equal 1 (car lt))
            (assert-match "broken load" (caddr lt))))))))

(define-test "script selection is static and requirements do not select files"
  (call-with-temporary-directory
    (lambda (root)
      (let ((script (path-join root "tests/script.scm"))
            (marker (path-join root "script-ran")))
        (make-directory (path-join root "tests"))
        (write-file (path-join root "techne-package.scm") "(package selection)")
        (write-file script
          (string-append "(import (techne) (techne test))\n(test-file #:script #t)\n"
                         "(call-with-output-file \""
                         marker "\" (lambda (p) (display \"ran\" p)))\n(check \"legacy\" 1 1)\n"))
        (write-file (path-join root "tests/requires.scm")
          "(import (techne test))\n(test-file #:requires '(program \"techne-test-program-that-does-not-exist\"))\n(define-test \"gated body\" (error \"must not run\"))\n")
        (let ((listed (run-techne-test root "--list-tests")))
          (assert-equal 0 (car listed))
          (assert-match "selection:tests/script.scm" (cadr listed))
          (assert-match "gated body" (cadr listed)))
        (assert-false (file-exists? marker))
        (let* ((no-match (run-techne-test root "-k" "zzz-nomatch" "--format" "sexp"))
               (data (read (open-input-string (cadr no-match)))))
          (assert-equal 1 (car no-match))
          (assert-equal 0 (cadr (assq 'files data)))
          (assert-equal 0 (cadr (assq 'skipped data))))
        (assert-false (file-exists? marker))
        (let ((gated (run-techne-test root "-k" "gated body")))
          (assert-equal 0 (car gated))
          (assert-match "1 skipped" (cadr gated)))
        (let ((selected (run-techne-test root "-k" "script.scm")))
          (assert-equal 0 (car selected))
          (assert-match "1 file, 1 passed" (cadr selected)))
        (assert-equal "ran" (file->string marker))))))

(define-test "checks called by helpers at registered-file load are errors"
  (call-with-temporary-directory
    (lambda (root)
      (write-file (path-join root "broken.scm")
        "(import (techne test))\n(define (helper) (check-true \"bad\" #t))\n(helper)\n")
      (let ((result (run-techne-test root "--list-tests" "broken.scm")))
        (assert-equal 1 (car result))
        (assert-match "wrap the check in a test or declare (test-file #:script #t)" (caddr result))))))
