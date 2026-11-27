;;; `techne test`: package-aware Scheme test runner.
;;;
;;; Invoked by `crates/techne-cli/src/main.rs` as:
;;;   (command-line) => (runner-path executable arg1 arg2 ...)

(import (techne) (techne package) (techne path) (techne crypto))

(define usage-text
  (string-append
    "usage: techne test [OPTIONS] [PATH-OR-FILTER...]\n"
    "\n"
    "Discover and run Scheme tests in the current package, workspace, or given paths.\n"
    "Positional arguments that are not existing files or directories act as name filters.\n"
    "\n"
    "Options:\n"
    "  -p, --package NAME      Run tests only for package NAME (repeatable)\n"
    "      --all, --workspace  Run all workspace packages and test directories\n"
    "  -k, --filter PATTERN    Filter by file path or test name substring (repeatable)\n"
    "      --mode MODES        Comma-separated VM modes: default,interp,jit,gc,no-ir-interp,no-ir-jit\n"
    "      --vm-only           Run only `vm`-world test files\n"
    "  -j, --jobs N            Max parallel test workers (default: CPU count)\n"
    "      --timeout SECONDS   Per-file timeout in seconds (default: 120)\n"
    "  -x, --fail-fast         Stop launching new files after the first failure\n"
    "  -v, --verbose           Show individual test results and captured output\n"
    "      --no-capture        Do not capture test stdout/stderr\n"
    "      --list              List discovered test files and exit\n"
    "      --list-tests        List individual tests in discovered files and exit\n"
    "      --format FORMAT     Output format: human (default) or sexp\n"
    "  -h, --help              Show this help\n"))

;;; ---------------------------------------------------------------------------
;;; Small utilities
;;; ---------------------------------------------------------------------------

(define (string-suffix? suffix s)
  (let ((slen (string-length s)) (suflen (string-length suffix)))
    (and (>= slen suflen)
         (string=? (substring s (- slen suflen) slen) suffix))))

(define (dir-exists? p)
  (let ((a (guard (e (#t #f)) (file-attributes p))))
    (and a (eq? (car a) 'directory))))

(define (regular-file? p)
  (let ((a (guard (e (#t #f)) (file-attributes p))))
    (and a (eq? (car a) 'file))))

(define (parent-dir p)
  (let ((n (string-length p)))
    (if (<= n 1) #f
        (let loop ((i (- n 1)))
          (cond ((< i 0) #f)
                ((char=? (string-ref p i) #\/)
                 (if (= i 0) "/" (substring p 0 i)))
                (else (loop (- i 1))))))))

(define (relative-to base path)
  (let ((prefix (if (string-suffix? "/" base) base (string-append base "/"))))
    (if (string-prefix? prefix path)
        (substring path (string-length prefix) (string-length path))
        path)))

(define (elapsed-ms start)
  (let ((now (current-jiffy)) (jps (jiffies-per-second)))
    (if (<= now start) 0 (quotient (* (- now start) 1000) jps))))

(define (format-seconds ms)
  (let* ((tenths (quotient ms 10))
         (whole (quotient tenths 100))
         (frac (modulo tenths 100)))
    (string-append (number->string whole) "."
                   (if (< frac 10) "0" "")
                   (number->string frac) "s")))

(define (find-upward start filename)
  (let loop ((dir (file-truename start)))
    (let ((candidate (path-join dir filename)))
      (cond ((regular-file? candidate) dir)
            (else
             (let ((up (parent-dir dir)))
               (if (or (not up) (string=? up dir)) #f (loop up))))))))

(define (program-in-path? name)
  (define (usable-file? p)
    (and (file-exists? p)
         (let ((real (guard (e (#t #f)) (file-truename p))))
           (and real (regular-file? real)))))
  (if (string-contains name "/")
      (usable-file? name)
      (let ((path-env (or (get-environment-variable "PATH") "")))
        (any (lambda (dir)
               (and (not (string=? dir ""))
                    (usable-file? (path-join dir name))))
             (string-split path-env ":")))))

;;; ---------------------------------------------------------------------------
;;; Modes
;;; ---------------------------------------------------------------------------

(define mode-envs
  '(("default"      . ())
    ("interp"       . (("TECHNE_JIT" . "0")))
    ("jit"          . (("TECHNE_JIT" . "1")))
    ("gc"           . (("TECHNE_GC_STRESS" . "1")))
    ("no-ir-interp" . (("TECHNE_IR" . "0") ("TECHNE_JIT" . "0")))
    ("no-ir-jit"    . (("TECHNE_IR" . "0") ("TECHNE_JIT" . "1")))))

(define (valid-mode? m) (and (assoc m mode-envs) #t))

;;; ---------------------------------------------------------------------------
;;; Reading `(test-file ...)` metadata without evaluating
;;; ---------------------------------------------------------------------------

(define (default-meta)
  (list (cons 'environment 'vm)
        (cons 'without '())
        (cons 'timeout #f)
        (cons 'serial #f)
        (cons 'gc-stress 'default)
        (cons 'requires '())
        (cons 'cwd #f)
        (cons 'ignore #f)))

(define (meta-ref meta key)
  (let ((p (assq key meta))) (if p (cdr p) #f)))

(define (strip-quote datum)
  (if (and (pair? datum) (eq? (car datum) 'quote) (pair? (cdr datum)) (null? (cddr datum)))
      (cadr datum)
      datum))

(define valid-capabilities
  '(files environment processes network loading host-control host_control console clock entropy))

(define (valid-requires? reqs)
  (define (one? r)
    (and (list? r) (= (length r) 2)
         (memq (car r) '(program env))
         (string? (cadr r))))
  (or (null? reqs)
      (one? reqs)
      (and (list? reqs) (every one? reqs))))

(define (read-test-file-meta path)
  (let ((meta (default-meta)))
    (guard (e (#t (cons (cons 'meta-error (condition/report-string e)) meta)))
      (let ((decl (call-with-input-file path
                    (lambda (p)
                      (let scan ((in-header? #t) (found #f))
                        (let ((d (read p)))
                          (cond
                            ((eof-object? d) found)
                            ((and (pair? d) (eq? (car d) 'test-file))
                             (when found
                               (error "test-file: duplicate declaration"))
                             (unless in-header?
                               (error "test-file: declaration must appear at the top of the file before executable forms"))
                             (scan #t d))
                            ((and in-header? (pair? d) (memq (car d) '(import require provide)))
                             (scan #t found))
                            (else
                             (scan #f found)))))))))
        (if (not decl)
            meta
            (let loop ((xs (cdr decl)) (acc meta))
              (cond
                ((null? xs) acc)
                ((and (keyword? (car xs)) (pair? (cdr xs)))
                 (let ((k (car xs))
                       (v (strip-quote (cadr xs))))
                   (cond
                     ((eq? k #:environment)
                      (unless (memq v '(vm host editor))
                        (error "test-file: #:environment must be vm, host, or editor" v))
                      (loop (cddr xs) (cons (cons 'environment v) acc)))
                     ((eq? k #:without)
                      (unless (and (list? v) (every (lambda (c) (and (symbol? c) (memq c valid-capabilities))) v))
                        (error "test-file: #:without must be a list of valid capability symbols" v))
                      (loop (cddr xs) (cons (cons 'without v) acc)))
                     ((eq? k #:timeout)
                      (unless (or (not v) (and (exact-integer? v) (> v 0)))
                        (error "test-file: #:timeout must be a positive integer" v))
                      (loop (cddr xs) (cons (cons 'timeout v) acc)))
                     ((eq? k #:serial)
                      (unless (boolean? v)
                        (error "test-file: #:serial must be a boolean" v))
                      (loop (cddr xs) (cons (cons 'serial v) acc)))
                     ((eq? k #:gc-stress)
                      (unless (or (boolean? v) (eq? v 'default))
                        (error "test-file: #:gc-stress must be #t, #f, or default" v))
                      (loop (cddr xs) (cons (cons 'gc-stress v) acc)))
                     ((eq? k #:requires)
                      (unless (valid-requires? v)
                        (error "test-file: #:requires must be (program NAME) / (env NAME) or a list of them" v))
                      (loop (cddr xs) (cons (cons 'requires v) acc)))
                     ((eq? k #:cwd)
                      (unless (memq v '(#f file package workspace lisp))
                        (error "test-file: #:cwd must be file, package, workspace, or lisp" v))
                      (loop (cddr xs) (cons (cons 'cwd v) acc)))
                     ((eq? k #:ignore)
                      (unless (boolean? v)
                        (error "test-file: #:ignore must be a boolean" v))
                      (loop (cddr xs) (cons (cons 'ignore v) acc)))
                     (else
                      (error "test-file: unknown option" k)))))
                (else
                 (error "test-file: expected keyword-value option pairs" xs)))))))))

(define (check-requires requires)
  (cond
    ((null? requires) #f)
    ((and (pair? requires) (symbol? (car requires))
          (memq (car requires) '(program env)))
     ;; Single requirement form like '(program "tmux")
     (check-requires (list requires)))
    (else
     (let loop ((reqs requires))
       (if (null? reqs) #f
           (let ((r (car reqs)))
             (cond
               ((and (pair? r) (eq? (car r) 'program) (pair? (cdr r)))
                (if (program-in-path? (cadr r))
                    (loop (cdr reqs))
                    (string-append "requires program " (cadr r))))
               ((and (pair? r) (eq? (car r) 'env) (pair? (cdr r)))
                (let ((v (get-environment-variable (cadr r))))
                  (if (and v (not (string=? v "")))
                      (loop (cdr reqs))
                      (string-append "requires env " (cadr r)))))
               (else (loop (cdr reqs))))))))))

;;; ---------------------------------------------------------------------------
;;; Discovery: packages, workspaces, and test files
;;; ---------------------------------------------------------------------------

(define-record-type test-spec
  (make-test-spec id path pkg-name pkg-root ws-root meta)
  test-spec?
  (id spec-id)
  (path spec-path)
  (pkg-name spec-pkg-name)
  (pkg-root spec-pkg-root)
  (ws-root spec-ws-root)
  (meta spec-meta))

(define (collect-scm-files root)
  (if (not (dir-exists? root))
      '()
      (let walk ((dir root))
        (let ((entries (guard (e (#t '())) (directory-list dir))))
          (apply append
            (map (lambda (entry)
                   (cond
                     ((string-suffix? "/" entry)
                      (let ((name (substring entry 0 (- (string-length entry) 1))))
                        (if (or (string=? name "fixtures")
                                (string=? name "support")
                                (string-prefix? "." name))
                            '()
                            (walk (path-join dir name)))))
                     ((string-suffix? ".scm" entry)
                      (list (path-join dir entry)))
                     (else '())))
                 entries))))))

(define (discover-package pkg-dir ws-root)
  (let* ((canon (file-truename pkg-dir))
         (m (read-package-manifest canon))
         (name (symbol->string (manifest-name m)))
         (tests-dir (path-join canon "tests"))
         (files (collect-scm-files tests-dir)))
    (filter (lambda (s) (not (meta-ref (spec-meta s) 'ignore)))
      (map (lambda (f)
             (make-test-spec
               (string-append name ":" (relative-to canon f))
               f name canon ws-root (read-test-file-meta f)))
           files))))

(define (read-workspace-manifest ws-dir)
  (let* ((file (path-join ws-dir "techne-workspace.scm"))
         (datum (call-with-input-file file (lambda (p) (read p)))))
    (unless (and (pair? datum) (eq? (car datum) 'workspace))
      (error "techne-workspace.scm: expected (workspace ...)" file))
    (let ((pkgs '()) (tests '()))
      (for-each
        (lambda (clause)
          (cond
            ((and (pair? clause) (eq? (car clause) 'packages))
             (set! pkgs (append pkgs (cdr clause))))
            ((and (pair? clause) (eq? (car clause) 'tests))
             (set! tests (append tests (cdr clause))))))
        (cdr datum))
      (cons pkgs tests))))

(define (discover-workspace ws-dir selected-packages)
  (let* ((canon (file-truename ws-dir))
         (parsed (read-workspace-manifest canon))
         (pkg-roots (car parsed))
         (test-dirs (cdr parsed))
         (pkg-specs
           (apply append
             (map (lambda (rel)
                    (let* ((base (path-join canon rel))
                           (entries (if (dir-exists? base) (directory-list base) '())))
                      (apply append
                        (map (lambda (entry)
                               (if (string-suffix? "/" entry)
                                   (let ((d (path-join base (substring entry 0 (- (string-length entry) 1)))))
                                     (if (regular-file? (path-join d "techne-package.scm"))
                                         (discover-package d canon)
                                         '()))
                                   '()))
                             entries))))
                  pkg-roots)))
         (ws-test-specs
           (if (pair? selected-packages)
               '()
               (apply append
                 (map (lambda (rel)
                        (let* ((dir (path-join canon rel))
                               (files (collect-scm-files dir)))
                          (filter (lambda (s) (not (meta-ref (spec-meta s) 'ignore)))
                            (map (lambda (f)
                                   (make-test-spec
                                     (relative-to canon f)
                                     f #f #f canon (read-test-file-meta f)))
                                 files))))
                      test-dirs)))))
    (if (pair? selected-packages)
        (filter (lambda (s) (member (spec-pkg-name s) selected-packages)) pkg-specs)
        (append ws-test-specs pkg-specs))))

(define (discover-path target all? selected-packages)
  (let ((canon (file-truename target)))
    (cond
      ((regular-file? canon)
       (let* ((pkg-dir (find-upward (parent-dir canon) "techne-package.scm"))
              (ws-dir (find-upward (parent-dir canon) "techne-workspace.scm"))
              (pkg-name (and pkg-dir (symbol->string (manifest-name (read-package-manifest pkg-dir)))))
              (id (cond
                    (pkg-dir (string-append pkg-name ":" (relative-to pkg-dir canon)))
                    (ws-dir (relative-to ws-dir canon))
                    (else target))))
         (list (make-test-spec id canon pkg-name pkg-dir ws-dir (read-test-file-meta canon)))))
      ((dir-exists? canon)
       (let* ((ws-dir (find-upward canon "techne-workspace.scm"))
              (enclosing-pkg (and (not all?) (find-upward canon "techne-package.scm")))
              (pkg-dir (if (and (pair? selected-packages) ws-dir) #f enclosing-pkg))
              (prefix (if (string-suffix? "/" canon) canon (string-append canon "/")))
              (under-canon? (lambda (s)
                              (or (string=? (spec-path s) canon)
                                  (string-prefix? prefix (spec-path s)))))
              (specs
                (cond
                  (pkg-dir
                   (let ((pkg-specs (discover-package pkg-dir ws-dir)))
                     (if (pair? selected-packages)
                         (filter (lambda (s) (member (spec-pkg-name s) selected-packages)) pkg-specs)
                         pkg-specs)))
                  (ws-dir (discover-workspace ws-dir selected-packages))
                  (else
                   (if (pair? selected-packages)
                       '()
                       (let ((files (collect-scm-files canon)))
                         (filter (lambda (s) (not (meta-ref (spec-meta s) 'ignore)))
                           (map (lambda (f)
                                  (make-test-spec (relative-to canon f) f #f #f canon (read-test-file-meta f)))
                                files))))))))
         (if (or all? (pair? selected-packages)
                 (and pkg-dir (string=? canon pkg-dir))
                 (and ws-dir (string=? canon ws-dir)))
             specs
             (filter under-canon? specs))))
      (else '()))))

;;; ---------------------------------------------------------------------------
;;; Worker execution
;;; ---------------------------------------------------------------------------

(define (make-short-temp-dir)
  (let* ((env-tmp (get-environment-variable "TMPDIR"))
         (base (if (and env-tmp (< (string-length env-tmp) 48)) env-tmp "/tmp")))
    (let make ((tries 32))
      (let ((dir (string-append base "/tr-" (hex-encode (random-bytes 5)))))
        (if (guard (e (#t #f)) (create-directory dir #o700) #t)
            dir
            (if (<= tries 1)
                (error "test-runner: could not create temp dir" dir)
                (make (- tries 1))))))))

(define (resolve-cwd spec)
  (let ((cwd (meta-ref (spec-meta spec) 'cwd)))
    (cond
      ((eq? cwd 'file) (parent-dir (spec-path spec)))
      ((eq? cwd 'package) (or (spec-pkg-root spec) (spec-ws-root spec) (parent-dir (spec-path spec))))
      ((eq? cwd 'workspace) (or (spec-ws-root spec) (spec-pkg-root spec) (parent-dir (spec-path spec))))
      ((eq? cwd 'lisp) (if (spec-ws-root spec) (path-join (spec-ws-root spec) "lisp") (%lisp-directory)))
      ((spec-pkg-root spec) (spec-pkg-root spec))
      ((and (spec-ws-root spec)
            (string-prefix? (string-append (spec-ws-root spec) "/lisp/tests/") (spec-path spec)))
       (path-join (spec-ws-root spec) "lisp"))
      ((spec-ws-root spec) (spec-ws-root spec))
      (else (parent-dir (spec-path spec))))))

(define (parse-worker-events stdout nonce)
  (let ((prefix (string-append nonce " "))
        (events '())
        (stray '()))
    (for-each
      (lambda (line)
        (if (string-prefix? prefix line)
            (let ((body (substring line (string-length prefix) (string-length line))))
              (guard (e (#t (set! stray (cons line stray))))
                (set! events (cons (read (open-input-string body)) events))))
            (unless (string=? line "")
              (set! stray (cons line stray)))))
      (string-split stdout "\n"))
    (cons (reverse events) (string-join (reverse stray) "\n"))))

(define (run-worker executable spec mode filters timeout capture? list-only?)
  (let* ((meta (spec-meta spec))
         (meta-err (meta-ref meta 'meta-error))
         (req-skip (and (not meta-err) (check-requires (meta-ref meta 'requires))))
         (start (current-jiffy)))
    (cond
      (meta-err
       (if list-only?
           (list 'file-list (spec-id spec) mode '() 1 meta-err)
           (list 'file-result (spec-id spec) mode 'fail 0
                 (list (list 'test-result "<metadata>" 'fail 0 (spec-path spec)
                             (string-append "invalid test-file declaration: " meta-err) "" ""))
                 0 1 0 "" "")))
      ((and req-skip (not list-only?))
       (list 'file-result (spec-id spec) mode 'skip 0
             (list (list 'test-result "<file>" 'skip 0 (spec-path spec) req-skip "" ""))
             0 0 1 "" ""))
      (else
        (let* ((env-kind (meta-ref meta 'environment))
               (without (meta-ref meta 'without))
               (base-timeout (or (meta-ref meta 'timeout) timeout))
               (file-timeout (if (string=? mode "gc") (max base-timeout 300) base-timeout))
               (nonce (string-append ";;T" (hex-encode (random-bytes 6))))
               (id-matches? (and (pair? filters)
                                 (any (lambda (f) (and (string-contains (spec-id spec) f) #t)) filters)))
               (worker-filters (if id-matches? '() filters))
               (world-args (append
                             (list "run" "--test-worker" "--world" (symbol->string env-kind))
                             (if (pair? without)
                                 (list "--without" (string-join (map symbol->string without) ","))
                                 '())
                             (list (spec-path spec))))
               (tmp (make-short-temp-dir)))
          (dynamic-wind
            (lambda () #f)
            (lambda ()
              (let* ((xdg-cfg (path-join tmp "cfg"))
                     (xdg-data (path-join tmp "data"))
                     (xdg-state (path-join tmp "state"))
                     (node-dir (path-join tmp "node")))
                (create-directory xdg-cfg #o700)
                (create-directory xdg-data #o700)
                (create-directory xdg-state #o700)
                (create-directory node-dir #o700)
                (let* ((cfg-datum
                         (list (cons 'nonce nonce)
                               (cons 'file (spec-path spec))
                               (cons 'filters worker-filters)
                               (cons 'timeout file-timeout)
                               (cons 'capture capture?)
                               (cons 'list-only list-only?)
                               (cons 'world (symbol->string env-kind))
                               (cons 'without (map symbol->string without))
                               (cons 'executable executable)))
                       (cfg-str (call-with-output-string (lambda (p) (write cfg-datum p))))
                       (extra-env
                         (append
                           (list (cons "HOME" tmp)
                                 (cons "TMPDIR" tmp)
                                 (cons "XDG_CONFIG_HOME" xdg-cfg)
                                 (cons "XDG_DATA_HOME" xdg-data)
                                 (cons "XDG_STATE_HOME" xdg-state)
                                 (cons "TECHNE_NODE_DIR" node-dir)
                                 (cons "TECHNE_BIN" executable)
                                 (cons "TECHNE_TEST_CONFIG" cfg-str))
                           (cdr (assoc mode mode-envs))))
                       (timed-out #f))
                  (call-with-process executable world-args
                    (lambda (p)
                      (let* ((watch (spawn (lambda ()
                                             (sleep (* file-timeout 1000))
                                             (set! timed-out #t)
                                             (process-kill p))))
                             (err-task (spawn (lambda () (process-read-all p 'stderr))))
                             (stdout (process-read-all p 'stdout))
                             (stderr (task-join err-task))
                             (code (process-wait p)))
                        (guard (e (#t #f)) (task-kill watch))
                        (let* ((ms (elapsed-ms start))
                               (parsed (parse-worker-events stdout nonce))
                               (events (car parsed))
                               (stray (cdr parsed))
                               (results (filter (lambda (ev) (and (pair? ev) (eq? (car ev) 'test-result))) events))
                               (listed (filter (lambda (ev) (and (pair? ev) (eq? (car ev) 'test-list))) events))
                               (done-ev (find (lambda (ev) (and (pair? ev) (eq? (car ev) 'test-done))) events)))
                          (cond
                            (list-only?
                             (let ((err-msg
                                     (cond
                                       (timed-out (string-append "timed out after " (number->string file-timeout) "s"))
                                       ((not done-ev) (string-append "worker exited without finishing (status " (number->string (or code -1)) ")\n" stderr))
                                       ((> (caddr done-ev) 0)
                                        (let ((fev (find (lambda (ev) (eq? (list-ref ev 2) 'fail)) results)))
                                          (if fev (list-ref fev 5) "failed while loading tests")))
                                       (else #f))))
                               (list 'file-list (spec-id spec) mode listed (if err-msg 1 0) (or err-msg ""))))
                            (timed-out
                             (list 'file-result (spec-id spec) mode 'fail ms
                                   (append results
                                     (list (list 'test-result "<timeout>" 'fail ms (spec-path spec)
                                                 (string-append "file timed out after " (number->string file-timeout) "s")
                                                 stray stderr)))
                                   0 1 0 stray stderr))
                            ((not done-ev)
                             (list 'file-result (spec-id spec) mode 'fail ms
                                   (append results
                                     (list (list 'test-result "<crash>" 'fail ms (spec-path spec)
                                                 (string-append "worker exited without finishing (status "
                                                                (number->string (or code -1)) ")")
                                                 stray stderr)))
                                   0 1 0 stray stderr))
                            (else
                             (let ((p-cnt (cadr done-ev))
                                   (f-cnt (caddr done-ev))
                                   (s-cnt (cadddr done-ev)))
                               (if (and (not (eqv? code 0)) (= f-cnt 0))
                                   (list 'file-result (spec-id spec) mode 'fail ms
                                         (append results
                                           (list (list 'test-result "<exit>" 'fail ms (spec-path spec)
                                                       (string-append "worker exited with status " (number->string code))
                                                       stray stderr)))
                                         p-cnt 1 s-cnt stray stderr)
                                   (list 'file-result (spec-id spec) mode
                                         (cond ((> f-cnt 0) 'fail)
                                               ((and (= p-cnt 0) (> s-cnt 0)) 'skip)
                                               (else 'pass))
                                         ms results p-cnt f-cnt s-cnt stray stderr))))))))
                    #:directory (resolve-cwd spec)
                    #:environment extra-env
                    #:stdin 'null))))
            (lambda ()
              (guard (e (#t #f)) (delete-path tmp #t)))))))))

;;; ---------------------------------------------------------------------------
;;; CLI parsing
;;; ---------------------------------------------------------------------------

(define (parse-cli argv)
  (let ((packages '())
        (filters '())
        (paths '())
        (modes '("default"))
        (jobs (available-parallelism))
        (timeout 120)
        (all? #f)
        (vm-only? #f)
        (fail-fast? #f)
        (verbose? #f)
        (capture? #t)
        (list-files? #f)
        (list-tests? #f)
        (format-kind 'human)
        (help? #f))
    (define (bad msg)
      (display (string-append "techne test: " msg "\n") (current-error-port))
      (exit 2))
    (let loop ((xs argv))
      (unless (null? xs)
        (let ((a (car xs)))
          (cond
            ((or (string=? a "-h") (string=? a "--help"))
             (set! help? #t) (loop (cdr xs)))
            ((or (string=? a "--all") (string=? a "--workspace"))
             (set! all? #t) (loop (cdr xs)))
            ((string=? a "--vm-only")
             (set! vm-only? #t) (loop (cdr xs)))
            ((or (string=? a "-x") (string=? a "--fail-fast"))
             (set! fail-fast? #t) (loop (cdr xs)))
            ((or (string=? a "-v") (string=? a "--verbose"))
             (set! verbose? #t) (loop (cdr xs)))
            ((string=? a "--no-capture")
             (set! capture? #f) (loop (cdr xs)))
            ((string=? a "--list")
             (set! list-files? #t) (loop (cdr xs)))
            ((string=? a "--list-tests")
             (set! list-tests? #t) (loop (cdr xs)))
            ((or (string=? a "-p") (string=? a "--package"))
             (when (null? (cdr xs)) (bad "--package expects a package name"))
             (set! packages (append packages (list (cadr xs))))
             (loop (cddr xs)))
            ((or (string=? a "-k") (string=? a "--filter"))
             (when (null? (cdr xs)) (bad "--filter expects a pattern"))
             (set! filters (append filters (list (cadr xs))))
             (loop (cddr xs)))
            ((string=? a "--mode")
             (when (null? (cdr xs)) (bad "--mode expects a mode name"))
             (let ((ms (filter (lambda (s) (not (string=? s ""))) (string-split (cadr xs) ","))))
               (for-each (lambda (m) (unless (valid-mode? m) (bad (string-append "unknown mode: " m)))) ms)
               (when (null? ms) (bad "--mode expects at least one mode"))
               (set! modes ms))
             (loop (cddr xs)))
            ((or (string=? a "-j") (string=? a "--jobs"))
             (when (null? (cdr xs)) (bad "--jobs expects a positive integer"))
             (let ((n (string->number (cadr xs))))
               (unless (and (exact-integer? n) (> n 0)) (bad "--jobs expects a positive integer"))
               (set! jobs n))
             (loop (cddr xs)))
            ((string=? a "--timeout")
             (when (null? (cdr xs)) (bad "--timeout expects a positive integer"))
             (let ((n (string->number (cadr xs))))
               (unless (and (exact-integer? n) (> n 0)) (bad "--timeout expects a positive integer"))
               (set! timeout n))
             (loop (cddr xs)))
            ((string=? a "--format")
             (when (null? (cdr xs)) (bad "--format expects human or sexp"))
             (cond ((string=? (cadr xs) "human") (set! format-kind 'human))
                   ((string=? (cadr xs) "sexp") (set! format-kind 'sexp))
                   (else (bad (string-append "unknown format: " (cadr xs)))))
             (loop (cddr xs)))
            ((string=? a "--")
             (for-each (lambda (rest) (set! paths (append paths (list rest)))) (cdr xs)))
            ((string-prefix? "-" a)
             (bad (string-append "unknown option: " a)))
            (else
             (cond
               ((guard (e (#t #f)) (file-attributes a))
                (set! paths (append paths (list a))))
               ((or (string-contains a "/") (string-suffix? ".scm" a))
                (bad (string-append "path not found: " a)))
               (else
                (set! filters (append filters (list a)))))
             (loop (cdr xs)))))))
    (list (cons 'packages packages)
          (cons 'filters filters)
          (cons 'paths paths)
          (cons 'modes modes)
          (cons 'jobs jobs)
          (cons 'timeout timeout)
          (cons 'all all?)
          (cons 'vm-only vm-only?)
          (cons 'fail-fast fail-fast?)
          (cons 'verbose verbose?)
          (cons 'capture capture?)
          (cons 'list-files list-files?)
          (cons 'list-tests list-tests?)
          (cons 'format format-kind)
          (cons 'help help?))))

;;; ---------------------------------------------------------------------------
;;; Reporting
;;; ---------------------------------------------------------------------------

(define (print-file-line res multi-mode? verbose? show-output?)
  (let* ((id (list-ref res 1))
         (mode (list-ref res 2))
         (status (list-ref res 3))
         (ms (list-ref res 4))
         (events (list-ref res 5))
         (p (list-ref res 6))
         (f (list-ref res 7))
         (s (list-ref res 8))
         (tag (cond ((eq? status 'pass) "PASS")
                    ((eq? status 'skip) "SKIP")
                    (else "FAIL")))
         (label (if multi-mode? (string-append id " [" mode "]") id)))
    (when (or verbose? (> (+ p f s) 0))
      (display "  ") (display tag) (display " [") (display (format-seconds ms)) (display "] ")
      (display label)
      (cond
        ((eq? status 'skip)
         (let ((reason (and (pair? events) (list-ref (car events) 5))))
           (when (and (string? reason) (not (string=? reason "")))
             (display " (") (display reason) (display ")"))))
        ((> f 0)
         (display " (") (display f) (display " failed, ") (display p) (display " passed")
         (when (> s 0) (display ", ") (display s) (display " skipped"))
         (display ")"))
        (else
         (display " (") (display p) (display (if (= p 1) " test" " tests"))
         (when (> s 0) (display ", ") (display s) (display " skipped"))
         (display ")")))
      (newline)
      (flush-output-port (current-output-port))
      (when (or verbose? show-output?)
        (for-each
          (lambda (ev)
            (let ((tname (list-ref ev 1))
                  (tst (list-ref ev 2))
                  (tms (list-ref ev 3))
                  (out (list-ref ev 6))
                  (err (list-ref ev 7)))
              (when verbose?
                (display "    ")
                (display (cond ((eq? tst 'pass) "ok  ") ((eq? tst 'skip) "skip") (else "FAIL")))
                (display " [") (display (format-seconds tms)) (display "] ")
                (displayln tname))
              (when (and (or verbose? show-output?) (not (eq? tst 'fail)))
                (unless (string=? out "") (displayln out))
                (unless (string=? err "") (displayln err)))))
          events)
        (when (and show-output? (not (eq? status 'fail)))
          (let ((stray (list-ref res 9)) (stderr (list-ref res 10)))
            (unless (string=? stray "") (displayln stray))
            (unless (string=? stderr "") (displayln stderr))))))))

(define (print-failures all-results multi-mode?)
  (let ((failed-files (filter (lambda (r) (eq? (list-ref r 3) 'fail)) all-results)))
    (when (pair? failed-files)
      (display "\nFailures:\n")
      (for-each
        (lambda (r)
          (let* ((id (list-ref r 1))
                 (mode (list-ref r 2))
                 (events (list-ref r 5))
                 (stray (list-ref r 9))
                 (stderr (list-ref r 10))
                 (label (if multi-mode? (string-append id " [" mode "]") id)))
            (for-each
              (lambda (ev)
                (when (eq? (list-ref ev 2) 'fail)
                  (let ((tname (list-ref ev 1))
                        (loc (list-ref ev 4))
                        (detail (list-ref ev 5))
                        (out (list-ref ev 6))
                        (err (list-ref ev 7)))
                    (display "\n---- ") (display label)
                    (unless (string=? tname "<top-level>")
                      (display "::") (display tname))
                    (when (string? loc) (display " [") (display loc) (display "]"))
                    (displayln " ----")
                    (unless (string=? detail "") (displayln detail))
                    (unless (string=? out "")
                      (display "  stdout:\n") (displayln out))
                    (unless (string=? err "")
                      (display "  stderr:\n") (displayln err)))))
              events)
            (unless (string=? stray "")
              (display "  worker stdout:\n") (displayln stray))
            (unless (string=? stderr "")
              (display "  worker stderr:\n") (displayln stderr))))
        failed-files))))

;;; ---------------------------------------------------------------------------
;;; Parallel scheduler
;;; ---------------------------------------------------------------------------

(define (run-tasks-pool tasks jobs fail-fast? on-result)
  (let ((total (length tasks))
        (queue tasks)
        (results '())
        (failed? #f)
        (done-ch (make-channel))
        (in-flight 0))
    (define (launch-more!)
      (let loop ()
        (when (and (< in-flight jobs)
                   (pair? queue)
                   (not (and fail-fast? failed?)))
          (let ((task (car queue)))
            (set! queue (cdr queue))
            (set! in-flight (+ in-flight 1))
            (spawn (lambda ()
                     (let ((res (guard (e (#t (list 'worker-error (condition/report-string e))))
                                  (task))))
                       (channel-send done-ch res))))
            (loop)))))
    (launch-more!)
    (let loop ()
      (when (> in-flight 0)
        (let ((res (channel-recv done-ch)))
          (set! in-flight (- in-flight 1))
          (set! results (cons res results))
          (when (and (eq? (car res) 'file-result) (eq? (list-ref res 3) 'fail))
            (set! failed? #t))
          (when on-result (on-result res))
          (launch-more!)
          (loop))))
    (reverse results)))

;;; ---------------------------------------------------------------------------
;;; Main entry point
;;; ---------------------------------------------------------------------------

(let* ((cl (command-line))
       (executable (cadr cl))
       (argv (cddr cl))
       (opts (parse-cli argv))
       (opt (lambda (k) (cdr (assq k opts)))))
  (when (opt 'help)
    (display usage-text)
    (exit 0))
  (let* ((start (current-jiffy))
         (targets (if (null? (opt 'paths)) (list ".") (opt 'paths)))
         (specs (apply append
                  (map (lambda (t) (discover-path t (opt 'all) (opt 'packages))) targets)))
         ;; Deduplicate specs by canonical path
         (specs (let loop ((xs specs) (seen '()) (acc '()))
                  (cond ((null? xs) (reverse acc))
                        ((member (spec-path (car xs)) seen) (loop (cdr xs) seen acc))
                        (else (loop (cdr xs) (cons (spec-path (car xs)) seen) (cons (car xs) acc))))))
         (specs (if (opt 'vm-only)
                    (filter (lambda (s) (eq? (meta-ref (spec-meta s) 'environment) 'vm)) specs)
                    specs))
         (filters (opt 'filters))
         (modes (opt 'modes))
         (multi-mode? (> (length modes) 1)))
    (when (null? specs)
      (display "techne test: no test files found\n" (current-error-port))
      (exit 1))
    ;; Build (spec . mode) helper
    (define (gc-applies? spec)
      (let ((gs (meta-ref (spec-meta spec) 'gc-stress))
            (env (meta-ref (spec-meta spec) 'environment)))
        (cond ((eq? gs #t) #t)
              ((eq? gs #f) #f)
              (else (eq? env 'vm)))))
    (define (mode-applies? spec mode)
      (if (string=? mode "gc") (gc-applies? spec) #t))
    ;; --list: fast file listing without spawning workers
    (when (opt 'list-files)
      (let ((shown (filter (lambda (s)
                             (or (null? filters)
                                 (any (lambda (f) (and (string-contains (spec-id s) f) #t)) filters)))
                           specs)))
        (cond
          ((eq? (opt 'format) 'sexp)
           (write (map (lambda (s)
                         (list (spec-id s)
                               (cons 'path (spec-path s))
                               (cons 'environment (meta-ref (spec-meta s) 'environment))
                               (cons 'gc-stress (gc-applies? s))
                               (cons 'serial (and (meta-ref (spec-meta s) 'serial) #t))))
                       shown))
           (newline))
          (else
           (for-each (lambda (s) (displayln (spec-id s))) shown)))
        (exit 0)))
    (let* ((items (apply append
                    (map (lambda (s)
                           (filter-map (lambda (m) (and (mode-applies? s m) (cons s m))) modes))
                         specs))))
      (when (null? items)
        (display "techne test: no applicable test files for the selected mode(s)\n" (current-error-port))
        (exit 1))
      (let* ((serial-items (filter (lambda (it) (meta-ref (spec-meta (car it)) 'serial)) items))
             (par-items (filter (lambda (it) (not (meta-ref (spec-meta (car it)) 'serial))) items))
             (make-job (lambda (it)
                         (lambda ()
                           (guard (e (#t
                                      (let ((s (car it)) (m (cdr it)))
                                        (list 'file-result (spec-id s) m 'fail 0
                                              (list (list 'test-result "<runner>" 'fail 0 (spec-path s)
                                                          (string-append "runner error: " (condition/report-string e))
                                                          "" ""))
                                              0 1 0 "" ""))))
                             (run-worker executable (car it) (cdr it)
                                         filters (opt 'timeout) (opt 'capture) (opt 'list-tests))))))
             (on-res (lambda (res)
                       (when (and (eq? (opt 'format) 'human)
                                  (not (opt 'list-tests))
                                  (eq? (car res) 'file-result))
                         (print-file-line res multi-mode? (opt 'verbose) (not (opt 'capture))))))
             (par-results (run-tasks-pool (map make-job par-items) (opt 'jobs) (opt 'fail-fast) on-res))
             (par-failed? (any (lambda (r) (and (eq? (car r) 'file-result) (eq? (list-ref r 3) 'fail))) par-results))
             (serial-results (if (and (opt 'fail-fast) par-failed?)
                                 '()
                                 (run-tasks-pool (map make-job serial-items) 1 (opt 'fail-fast) on-res)))
             (all-results (append par-results serial-results))
             (total-ms (elapsed-ms start)))
      (when (opt 'list-tests)
        (let ((list-errors 0))
          (for-each
            (lambda (r)
              (let ((id (list-ref r 1))
                    (listed (list-ref r 3))
                    (code (list-ref r 4))
                    (err (list-ref r 5)))
                (if (not (eqv? code 0))
                    (begin
                      (set! list-errors (+ list-errors 1))
                      (display (string-append "error listing " id ": " err "\n") (current-error-port)))
                    (for-each
                      (lambda (ev)
                        (display id) (display "::") (displayln (cadr ev)))
                      listed))))
            all-results)
          (exit (if (= list-errors 0) 0 1))))
      (let* ((total-passed (apply + (map (lambda (r) (list-ref r 6)) all-results)))
             (total-failed (apply + (map (lambda (r) (list-ref r 7)) all-results)))
             (total-skipped (apply + (map (lambda (r) (list-ref r 8)) all-results)))
             (files-ran (length all-results))
             (matched-nothing? (and (pair? filters) (= (+ total-passed total-failed total-skipped) 0)))
             (ok? (and (= total-failed 0) (> files-ran 0) (not matched-nothing?))))
        (if (eq? (opt 'format) 'sexp)
            (begin
              (write `((status ,(if ok? 'ok 'failed))
                       (files ,files-ran)
                       (passed ,total-passed)
                       (failed ,total-failed)
                       (skipped ,total-skipped)
                       (elapsed-ms ,total-ms)
                       (failures
                         ,(filter-map
                            (lambda (r)
                              (and (eq? (list-ref r 3) 'fail)
                                   (list (list-ref r 1)
                                         (list-ref r 2)
                                         (filter (lambda (ev) (eq? (list-ref ev 2) 'fail))
                                                 (list-ref r 5)))))
                            all-results))))
              (newline))
            (begin
              (print-failures all-results multi-mode?)
              (newline)
              (when matched-nothing?
                (displayln "error: filter matched 0 tests"))
              (display "test result: ") (display (if ok? "ok" "FAILED")) (display ". ")
              (display files-ran) (display (if (= files-ran 1) " file, " " files, "))
              (display total-passed) (display " passed, ")
              (display total-failed) (display " failed, ")
              (display total-skipped) (display " skipped (")
              (display (format-seconds total-ms)) (displayln ")")))
        (exit (if ok? 0 1)))))))
