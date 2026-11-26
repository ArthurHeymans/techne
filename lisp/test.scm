;;; Techne's Scheme test framework: `(techne test)`.
;;;
;;; Write unit tests with `define-test` and `define-suite`, assert with
;;; `assert-equal`, `assert-true`, `assert-false`, `assert-raises`,
;;; `assert-match`, `assert-eventually`, and `assert-golden`, or use the
;;; soft checks `check` and `check-true`. Run with `techne test` (or
;;; standalone with `techne run`).

(provide
  test-file
  define-test define-suite
  assert-equal assert-true assert-false assert-raises assert-match
  assert-eventually assert-golden
  skip
  call-with-temporary-directory test-file-directory test-executable
  format-diff
  check check-true test-failures test-summary
  ;; Runner protocol hooks used by `techne run --test-worker`.
  %test-runner-active? %test-registered %test-run-worker!
  %test-worker-begin! %test-worker-set-load-error! %test-worker-finish!
  %test-count %test-emit)

;; Also the library (techne test) (lisp/techne/test.sld).
(%name-library '(techne test))

;;; ---------------------------------------------------------------------------
;;; Environment helpers (safe even without Environment / Clock / Entropy grants)
;;; ---------------------------------------------------------------------------

(define (%safe-env name)
  (guard (e (#t #f)) (get-environment-variable name)))

(define (%safe-jiffy)
  (guard (e (#t 0)) (current-jiffy)))

(define (%safe-jps)
  (guard (e (#t 1000000000))
    (let ((j (jiffies-per-second)))
      (if (and (number? j) (> j 0)) j 1000000000))))

(define (%elapsed-ms start)
  (let ((now (%safe-jiffy)) (jps (%safe-jps)))
    (if (<= now start) 0 (quotient (* (- now start) 1000) jps))))

(define (%written v)
  (call-with-output-string (lambda (p) (write v p))))

;; Worker state initialized by the Rust host (`main_test_worker`), never
;; inherited through environment variables.
(define %worker-nonce #f)
(define %worker-file #f)
(define %worker-filters '())
(define %worker-timeout #f)
(define %worker-capture? #t)
(define %worker-list-only? #f)
(define %worker-world #f)
(define %worker-without '())
(define %worker-executable #f)
(define %worker-load-error #f)

(define (%test-worker-begin! nonce path filters timeout capture? list-only? world without executable)
  (set! %worker-nonce (if (and (string? nonce) (not (string=? nonce ""))) nonce #f))
  (set! %worker-file path)
  (set! %worker-filters (if (list? filters) filters '()))
  (set! %worker-timeout (and (exact-integer? timeout) (> timeout 0) timeout))
  (set! %worker-capture? (and capture? #t))
  (set! %worker-list-only? (and list-only? #t))
  (set! %worker-world world)
  (set! %worker-without (if (list? without) without '()))
  (set! %worker-executable (and (string? executable) executable))
  (set! %worker-load-error #f))

(define (%test-worker-set-load-error! err)
  (set! %worker-load-error (and (string? err) err)))

(define (%test-runner-active?) (and (string? %worker-nonce) #t))

(define (%path-dirname path)
  (let ((n (string-length path)))
    (let loop ((i (- n 1)))
      (cond ((< i 0) ".")
            ((char=? (string-ref path i) #\/)
             (if (= i 0) "/" (substring path 0 i)))
            (else (loop (- i 1)))))))

(define (test-file-directory)
  "Return the directory containing the currently running test file."
  (let ((f (or %worker-file
               (%safe-env "TECHNE_TEST_FILE")
               (guard (e (#t #f))
                 (let ((cl (command-line)))
                   (and (pair? cl) (car cl)))))))
    (if (string? f) (%path-dirname f) ".")))

(define (test-executable)
  "Return the path to the `techne` executable running these tests."
  (or %worker-executable
      (%safe-env "TECHNE_BIN")
      (error "test-executable: TECHNE_BIN is not set (run via `techne test` or set TECHNE_BIN)")))

(define %temp-counter 0)

(define (call-with-temporary-directory proc)
  "Create a short-lived temporary directory, call (PROC dir), and delete it
when PROC returns, fails, or unwinds."
  (let* ((env-tmp (%safe-env "TMPDIR"))
         (base (if (and env-tmp (< (string-length env-tmp) 48)) env-tmp "/tmp")))
    (let make ((tries 32))
      (set! %temp-counter (+ %temp-counter 1))
      (let* ((tag (string-append (number->string (modulo (%safe-jiffy) 1000000))
                                 "-" (number->string %temp-counter)))
             (dir (string-append base "/tt-" tag))
             (err #f)
             (ok (guard (e (#t (set! err e) #f)) (create-directory dir #o700) #t)))
        (cond
          (ok
           (dynamic-wind
             (lambda () #f)
             (lambda () (proc dir))
             (lambda () (guard (e (#t #f)) (delete-path dir #t)))))
          ((and (> tries 1)
                (let ((msg (condition/report-string err)))
                  (or (string-contains msg "File exists")
                      (string-contains msg "already exists"))))
           (make (- tries 1)))
          (else (raise err)))))))

;;; ---------------------------------------------------------------------------
;;; File declaration (read as data by `techne test`; verified at runtime)
;;; ---------------------------------------------------------------------------

(define (%canonical-cap-name c)
  (let ((s (if (symbol? c) (symbol->string c) c)))
    (if (string=? s "host_control") "host-control" s)))

(define (%same-string-set? a b)
  (and (every (lambda (x) (member x b)) a)
       (every (lambda (x) (member x a)) b)))

(define (test-file #:environment [environment 'vm]
                   #:without [without '()]
                   #:timeout [timeout #f]
                   #:serial [serial #f]
                   #:gc-stress [gc-stress 'default]
                   #:requires [requires '()]
                   #:cwd [cwd #f]
                   #:ignore [ignore #f])
  "Declare file-level test metadata. Must appear near the top of the file
(before executable forms, after any import/require/provide). Under `techne test`,
verifies that the runner started the worker with matching world and grants."
  (unless (memq environment '(vm host editor))
    (error "test-file: #:environment must be vm, host, or editor" environment))
  (unless (list? without)
    (error "test-file: #:without must be a list of capability symbols" without))
  (when (and timeout (not (and (exact-integer? timeout) (> timeout 0))))
    (error "test-file: #:timeout must be a positive integer (seconds)" timeout))
  (when (and (%test-runner-active?) (not ignore))
    (let ((want-env (symbol->string environment))
          (want-without (map %canonical-cap-name without)))
      (unless (and (string? %worker-world) (string=? want-env %worker-world))
        (error "test-file: declaration was not active when worker started (move test-file to the top of the file after import/require)"
               (list 'declared environment 'actual %worker-world)))
      (unless (%same-string-set? want-without %worker-without)
        (error "test-file: #:without declaration was not active when worker started (move test-file to the top of the file after import/require)"
               (list 'declared want-without 'actual %worker-without)))))
  (if #f #f))

;;; ---------------------------------------------------------------------------
;;; Structural and line diffs
;;; ---------------------------------------------------------------------------

(define (%multiline? s)
  (and (string? s) (string-contains s "\n") #t))

(define (%format-line-diff expected actual)
  (let* ((el (string-split expected "\n"))
         (al (string-split actual "\n"))
         (ne (length el))
         (na (length al))
         (n (max ne na))
         (out (open-output-string))
         (shown 0)
         (limit 40))
    (let loop ((i 0) (es el) (as al))
      (when (and (< i n) (< shown limit))
        (let ((e (and (pair? es) (car es)))
              (a (and (pair? as) (car as))))
          (unless (equal? e a)
            (when e
              (display "  - " out) (display (+ i 1) out) (display ": " out) (write e out) (newline out)
              (set! shown (+ shown 1)))
            (when a
              (display "  + " out) (display (+ i 1) out) (display ": " out) (write a out) (newline out)
              (set! shown (+ shown 1))))
          (loop (+ i 1) (if (pair? es) (cdr es) '()) (if (pair? as) (cdr as) '())))))
    (when (>= shown limit)
      (display "  ... (diff truncated)\n" out))
    (get-output-string out)))

(define (%first-diff-path expected actual path)
  (cond
    ((equal? expected actual) #f)
    ((and (list? expected) (list? actual))
     (if (not (= (length expected) (length actual)))
         (list (reverse path)
               (string-append "length " (number->string (length expected)))
               (string-append "length " (number->string (length actual))))
         (let loop ((i 0) (es expected) (as actual))
           (if (null? es) #f
               (or (%first-diff-path (car es) (car as) (cons i path))
                   (loop (+ i 1) (cdr es) (cdr as)))))))
    ((and (vector? expected) (vector? actual))
     (%first-diff-path (vector->list expected) (vector->list actual) path))
    ((and (pair? expected) (pair? actual))
     (or (%first-diff-path (car expected) (car actual) (cons 'car path))
         (%first-diff-path (cdr expected) (cdr actual) (cons 'cdr path))))
    (else (list (reverse path) expected actual))))

(define (format-diff expected actual)
  "Format a human-readable diff between EXPECTED and ACTUAL."
  (cond
    ((and (%multiline? expected) (%multiline? actual))
     (string-append "diff (- expected, + actual):\n" (%format-line-diff expected actual)))
    ((or (and (pair? expected) (pair? actual))
         (and (vector? expected) (vector? actual)))
     (let ((d (%first-diff-path expected actual '())))
       (string-append
         "  expected: " (%written expected) "\n"
         "  actual:   " (%written actual)
         (if (and d (pair? (car d)))
             (string-append "\n  first diff at " (%written (car d))
                            ": expected " (%written (cadr d))
                            ", actual " (%written (caddr d)))
             ""))))
    (else
     (string-append
       "  expected: " (%written expected) "\n"
       "  actual:   " (%written actual)))))

;;; ---------------------------------------------------------------------------
;;; Failure / skip conditions & source locations
;;; ---------------------------------------------------------------------------

(define-record-type %test-failure
  (%make-test-failure location message detail)
  %test-failure?
  (location %test-failure-location)
  (message %test-failure-message)
  (detail %test-failure-detail))

(define-record-type %test-skip
  (%make-test-skip reason)
  %test-skip?
  (reason %test-skip-reason))

(define (%format-location loc)
  (if (and (list? loc) (= (length loc) 3) (string? (car loc)))
      (string-append (car loc) ":" (number->string (cadr loc)) ":" (number->string (caddr loc)))
      #f))

(define (skip reason)
  "Skip the currently running test with REASON."
  (raise (%make-test-skip (if (string? reason) reason (%written reason)))))

;;; ---------------------------------------------------------------------------
;;; Assertions (abort current test on failure)
;;; ---------------------------------------------------------------------------

(define (%assert-equal loc expected actual msg)
  (set! %count (+ %count 1))
  (unless (equal? expected actual)
    (raise (%make-test-failure loc (or msg "values are not equal") (format-diff expected actual)))))

(define (%assert-true loc val msg)
  (set! %count (+ %count 1))
  (unless val
    (raise (%make-test-failure loc (or msg "expected truthy value")
                               (string-append "  actual: " (%written val))))))

(define (%assert-false loc val msg)
  (set! %count (+ %count 1))
  (when val
    (raise (%make-test-failure loc (or msg "expected #f")
                               (string-append "  actual: " (%written val))))))

(define (%assert-match loc pattern actual msg)
  (set! %count (+ %count 1))
  (unless (string? actual)
    (raise (%make-test-failure loc (or msg "assert-match expected a string")
                               (string-append "  pattern: " (%written pattern)
                                              "\n  actual:  " (%written actual)))))
  (unless (string-contains actual pattern)
    (raise (%make-test-failure loc (or msg "string did not contain pattern")
                               (string-append "  pattern: " (%written pattern)
                                              "\n  actual:  " (%written actual))))))

(define (%assert-raises loc pred thunk)
  (set! %count (+ %count 1))
  (let ((raised #f) (cond-val #f))
    (guard (e (#t (set! raised #t) (set! cond-val e)))
      (thunk))
    (unless raised
      (raise (%make-test-failure loc "expected an error to be raised, but expression returned normally" "")))
    (cond
      ((not pred) cond-val)
      ((string? pred)
       (let ((report (condition/report-string cond-val)))
         (unless (string-contains report pred)
           (raise (%make-test-failure loc "raised error message did not match"
                                      (string-append "  expected substring: " (%written pred)
                                                     "\n  actual error:       " (%written report)))))
         cond-val))
      ((procedure? pred)
       (unless (pred cond-val)
         (raise (%make-test-failure loc "raised condition failed predicate"
                                    (string-append "  condition: " (condition/report-string cond-val)))))
       cond-val)
      (else (error "assert-raises: predicate must be a string or procedure" pred)))))

(define-syntax assert-equal
  (syntax-rules ()
    ((_ expected actual)
     (%assert-equal (procedure-location (lambda () #f)) expected actual #f))
    ((_ expected actual msg)
     (%assert-equal (procedure-location (lambda () #f)) expected actual msg))))

(define-syntax assert-true
  (syntax-rules ()
    ((_ expr)
     (%assert-true (procedure-location (lambda () #f)) expr #f))
    ((_ expr msg)
     (%assert-true (procedure-location (lambda () #f)) expr msg))))

(define-syntax assert-false
  (syntax-rules ()
    ((_ expr)
     (%assert-false (procedure-location (lambda () #f)) expr #f))
    ((_ expr msg)
     (%assert-false (procedure-location (lambda () #f)) expr msg))))

(define-syntax assert-match
  (syntax-rules ()
    ((_ pattern actual)
     (%assert-match (procedure-location (lambda () #f)) pattern actual #f))
    ((_ pattern actual msg)
     (%assert-match (procedure-location (lambda () #f)) pattern actual msg))))

(define-syntax assert-raises
  (syntax-rules ()
    ((_ expr)
     (%assert-raises (procedure-location (lambda () #f)) #f (lambda () expr)))
    ((_ pred expr)
     (%assert-raises (procedure-location (lambda () #f)) pred (lambda () expr)))))

(define (assert-eventually thunk #:timeout [timeout 5000] #:interval [interval 25] #:message [msg #f])
  "Poll THUNK every INTERVAL ms until it returns truthy or TIMEOUT ms elapses."
  (set! %count (+ %count 1))
  (let ((start (%safe-jiffy)))
    (let loop ()
      (let ((v (thunk)))
        (cond
          (v v)
          ((>= (%elapsed-ms start) timeout)
           (raise (%make-test-failure
                    (procedure-location thunk)
                    (or msg (string-append "condition not met within " (number->string timeout) "ms"))
                    "")))
          (else (sleep interval) (loop)))))))

(define (assert-golden path actual)
  "Assert that ACTUAL equals the contents of golden file PATH.
When TECHNE_BLESS=1 is set, writes ACTUAL to PATH and passes."
  (set! %count (+ %count 1))
  (cond
    ((equal? (%safe-env "TECHNE_BLESS") "1")
     (call-with-output-file path (lambda (p) (display actual p))))
    ((not (file-exists? path))
     (raise (%make-test-failure #f (string-append "golden file does not exist: " path)
                                "  run with TECHNE_BLESS=1 to create it")))
    (else
     (let ((expected (file->string path)))
       (unless (equal? expected actual)
         (raise (%make-test-failure #f (string-append "golden mismatch: " path)
                                    (format-diff expected actual))))))))

;;; ---------------------------------------------------------------------------
;;; Soft checks (backwards-compatible with all existing suites)
;;; ---------------------------------------------------------------------------

(define %failures 0)
(define %count 0)
(define %active-soft-failures (make-parameter #f))

(define (%test-count) %count)

(define (check name expected actual)
  "Count a soft check called NAME: ACTUAL must be `equal?` to EXPECTED.
A failure is recorded and printed with both without aborting the test."
  (set! %count (+ %count 1))
  (unless (equal? expected actual)
    (set! %failures (+ %failures 1))
    (let ((box (%active-soft-failures)))
      (if box
          (set-car! box (cons (list name (format-diff expected actual)) (car box)))
          (begin
            (display "FAIL ") (displayln name)
            (displayln (format-diff expected actual)))))))

(define (check-true name v)
  "Count a soft check called NAME: V must be true."
  (check name #t (and v #t)))

(define (test-failures)
  "Return how many checks failed."
  %failures)

(define (test-summary)
  "Print how many checks passed, then exit with failure if any failed.
When running under `techne test`, this is a no-op so the runner collects results."
  (unless (%test-runner-active?)
    (displayln (string-append (number->string (- %count %failures)) "/" (number->string %count) " passed"))
    (exit (if (= %failures 0) 0 1))))

;;; ---------------------------------------------------------------------------
;;; Test registration & execution
;;; ---------------------------------------------------------------------------

(define %suite-prefix (make-parameter '()))
(define %registered '()) ; reversed list of (full-name thunk location options)

(define (%test-registered) (reverse %registered))

(define (%join-suite-name name)
  (let ((prefix (%suite-prefix)))
    (if (null? prefix)
        name
        (string-join (append prefix (list name)) " / "))))

(define (%parse-test-options opts)
  (let loop ((xs opts) (acc '()))
    (cond
      ((null? xs) acc)
      ((and (keyword? (car xs)) (pair? (cdr xs)))
       (loop (cddr xs) (cons (cons (car xs) (cadr xs)) acc)))
      (else (error "define-test: expected keyword-value option pairs" opts)))))

(define (%opt-ref alist key default)
  (let ((p (assq key alist)))
    (if p (cdr p) default)))

(define (%capture-and-run thunk timeout-sec capture?)
  (let ((out-port (and capture? (open-output-string)))
        (err-port (and capture? (open-output-string)))
        (soft-box (list '()))
        (bt '())
        (status 'pass)
        (detail "")
        (location #f))
    (define (invoke)
      (parameterize ((%active-soft-failures soft-box)
                     (current-output-port (or out-port (current-output-port)))
                     (current-error-port (or err-port (current-error-port))))
        (guard (e
                 ((%test-skip? e)
                  (set! status 'skip)
                  (set! detail (%test-skip-reason e)))
                 ((%test-failure? e)
                  (set! status 'fail)
                  (set! location (%format-location (%test-failure-location e)))
                  (set! detail (string-append (%test-failure-message e)
                                              (if (string=? (%test-failure-detail e) "")
                                                  ""
                                                  (string-append "\n" (%test-failure-detail e))))))
                 (#t
                  (set! status 'fail)
                  (set! detail (string-append "uncaught error: " (condition/report-string e)
                                              (if (pair? bt)
                                                  (string-append "\n  backtrace:\n    "
                                                                 (string-join bt "\n    "))
                                                  "")))))
          (with-exception-handler
            (lambda (e)
              (when (and (null? bt) (not (%test-failure? e)) (not (%test-skip? e)))
                (set! bt (guard (x (#t '())) (current-backtrace))))
              (raise e))
            thunk))))
    (if (and timeout-sec (> timeout-sec 0))
        (let* ((timed-out #f)
               (t (spawn invoke))
               (watch (spawn (lambda ()
                               (sleep (* timeout-sec 1000))
                               (set! timed-out #t)
                               ;; Cancel cooperatively first so dynamic-wind cleanups run.
                               (guard (e (#t #f)) (task-cancel t))
                               (sleep 500)
                               (guard (e (#t #f)) (task-kill t))))))
          (guard (e
                   (timed-out
                    (set! status 'fail)
                    (set! detail (string-append "timed out after " (number->string timeout-sec) "s")))
                   (#t
                    (set! status 'fail)
                    (set! detail (string-append "task error: " (condition/report-string e)))))
            (task-join t))
          (guard (e (#t #f)) (task-kill watch))
          (when timed-out
            (set! status 'fail)
            (set! detail (string-append "timed out after " (number->string timeout-sec) "s"))))
        (invoke))
    ;; Attach any soft check failures if the test did not already abort.
    (when (and (eq? status 'pass) (pair? (car soft-box)))
      (set! status 'fail)
      (set! detail
        (string-join
          (map (lambda (f) (string-append "check " (car f) ":\n" (cadr f)))
               (reverse (car soft-box)))
          "\n")))
    (list status location detail
          (if out-port (get-output-string out-port) "")
          (if err-port (get-output-string err-port) ""))))

(define (%register-test! name thunk raw-opts)
  (let* ((full-name (%join-suite-name name))
         (loc (procedure-location thunk))
         (opts (%parse-test-options raw-opts)))
    (if (%test-runner-active?)
        (set! %registered (cons (list full-name thunk loc opts) %registered))
        ;; Standalone `techne run`: run immediately and report.
        (let* ((skip-reason (%opt-ref opts #:skip #f))
               (timeout (%opt-ref opts #:timeout #f))
               (start (%safe-jiffy)))
          (set! %count (+ %count 1))
          (if skip-reason
              (begin (display "SKIP ") (display full-name)
                     (display " (") (display skip-reason) (displayln ")"))
              (let* ((res (%capture-and-run thunk timeout #f))
                     (st (car res))
                     (floc (cadr res))
                     (det (caddr res)))
                (cond
                  ((eq? st 'pass)
                   (display "PASS ") (displayln full-name))
                  ((eq? st 'skip)
                   (display "SKIP ") (display full-name)
                   (display " (") (display det) (displayln ")"))
                  (else
                   (set! %failures (+ %failures 1))
                   (display "FAIL ") (display full-name)
                   (when (or floc (%format-location loc))
                     (display " [") (display (or floc (%format-location loc))) (display "]"))
                   (newline)
                   (unless (string=? det "") (displayln det))))))))))

(define-syntax define-suite
  (syntax-rules ()
    ((_ name body ...)
     (parameterize ((%suite-prefix (append (%suite-prefix) (list name))))
       body ...))))

(define-syntax define-test
  (syntax-rules ()
    ((_ (name opts ...) body ...)
     (%register-test! name (lambda () body ...) (list opts ...)))
    ((_ name body ...)
     (%register-test! name (lambda () body ...) '()))))

;;; ---------------------------------------------------------------------------
;;; Worker protocol (used by lisp/test-worker.scm)
;;; ---------------------------------------------------------------------------

(define (%test-emit event)
  (when (string? %worker-nonce)
    (let ((p (current-output-port)))
      ;; Leading newline ensures partial test output cannot concatenate onto the event line.
      (newline p)
      (display %worker-nonce p)
      (display " " p)
      (write event p)
      (newline p)
      (flush-output-port p))))

(define (%matches-filter? name filters)
  (or (null? filters)
      (any (lambda (f) (and (string-contains name f) #t)) filters)))

(define (%test-run-worker! filters default-timeout capture? list-only? raw-port)
  (let ((tests (%test-registered))
        (passed 0) (failed 0) (skipped 0))
    (for-each
      (lambda (entry)
        (let* ((name (car entry))
               (thunk (cadr entry))
               (loc (caddr entry))
               (opts (cadddr entry)))
          (when (%matches-filter? name filters)
            (if list-only?
                (parameterize ((current-output-port raw-port))
                  (%test-emit `(test-list ,name ,(%format-location loc))))
                (let* ((skip-reason (%opt-ref opts #:skip #f))
                       (timeout (%opt-ref opts #:timeout default-timeout))
                       (start (%safe-jiffy)))
                  (if skip-reason
                      (begin
                        (set! skipped (+ skipped 1))
                        (parameterize ((current-output-port raw-port))
                          (%test-emit `(test-result ,name skip 0 ,(%format-location loc) ,skip-reason "" ""))))
                      (let* ((res (%capture-and-run thunk timeout capture?))
                             (st (car res))
                             (floc (or (cadr res) (%format-location loc)))
                             (det (caddr res))
                             (out (cadddr res))
                             (err (car (cddddr res)))
                             (ms (%elapsed-ms start)))
                        (cond ((eq? st 'pass) (set! passed (+ passed 1)))
                              ((eq? st 'skip) (set! skipped (+ skipped 1)))
                              (else (set! failed (+ failed 1))))
                        (parameterize ((current-output-port raw-port))
                          (%test-emit `(test-result ,name ,st ,ms ,floc ,det ,out ,err))))))))))
      tests)
    (list passed failed skipped)))

(define (%test-worker-finish!)
  "Finalize a `--test-worker` run: run registered `define-test`s or report
top-level checks/load errors as structured S-expression events."
  (let* ((test-file %worker-file)
         (load-err %worker-load-error)
         (filters %worker-filters)
         (timeout %worker-timeout)
         (capture? %worker-capture?)
         (list-only? %worker-list-only?)
         (raw-port (current-output-port))
         (top-checks %count)
         (top-fails %failures)
         (reg (%test-registered)))
    (cond
      (load-err
       (%test-emit `(test-result "<load>" fail 0 ,test-file ,load-err "" ""))
       (%test-emit '(test-done 0 1 0)))
      ((pair? reg)
       (let* ((counts (%test-run-worker! filters timeout capture? list-only? raw-port))
              (p (car counts))
              (f (cadr counts))
              (s (caddr counts)))
         (when (and (not list-only?) (> top-checks 0) (null? filters))
           (let ((st (if (= top-fails 0) 'pass 'fail))
                 (det (if (= top-fails 0) ""
                          (string-append (number->string top-fails) " of "
                                         (number->string top-checks) " top-level checks failed"))))
             (if (eq? st 'pass) (set! p (+ p 1)) (set! f (+ f 1)))
             (%test-emit `(test-result "<top-level>" ,st 0 ,test-file ,det "" ""))))
         (%test-emit `(test-done ,p ,f ,s))))
      (list-only?
       (when (null? filters)
         (%test-emit `(test-list "<top-level>" ,test-file)))
       (%test-emit '(test-done 0 0 0)))
      ((> top-checks 0)
       (if (pair? filters)
           ;; Check-only files have no individual test names to match a filter.
           (%test-emit '(test-done 0 0 0))
           (let ((st (if (= top-fails 0) 'pass 'fail))
                 (det (if (= top-fails 0) ""
                          (string-append (number->string top-fails) " of "
                                         (number->string top-checks) " checks failed"))))
             (%test-emit `(test-result "<top-level>" ,st 0 ,test-file ,det "" ""))
             (%test-emit `(test-done ,(if (= top-fails 0) 1 0) ,(if (= top-fails 0) 0 1) 0)))))
      (else
       (%test-emit `(test-result "<top-level>" fail 0 ,test-file
                                 "file defined no tests and ran no checks" "" ""))
       (%test-emit '(test-done 0 1 0))))))
