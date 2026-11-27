;;; `techne check`: package and documentation checker for Techne Scheme.
;;;
;;; Invoked by `crates/techne-cli/src/main.rs` as:
;;;   (command-line) => (runner-path executable arg1 arg2 ...)

(import (techne) (techne package) (techne path) (techne crypto))

(define usage-text
  (string-append
    "usage: techne check [OPTIONS] [PATH...]\n"
    "\n"
    "Check Scheme packages, libraries, and docstrings in the current package,\n"
    "workspace, or given `.scm`/`.sld` files.\n"
    "Note: `techne check` loads and evaluates trusted package/module code.\n"
    "\n"
    "Options:\n"
    "  -p, --package NAME      Check only package NAME (repeatable)\n"
    "      --all, --workspace  Check all workspace packages and core libraries\n"
    "      --editor            Use the editor world when checking explicit files\n"
    "      --no-editor         Skip editor-world checks\n"
    "      --timeout SECONDS   Per-worker timeout in seconds (default: 120)\n"
    "      --format FORMAT     Output format: human (default) or sexp\n"
    "  -h, --help              Show this help\n"))

(define (string-suffix? suffix s)
  (let ((slen (string-length s)) (suflen (string-length suffix)))
    (and (>= slen suflen)
         (string=? (substring s (- slen suflen) slen) suffix))))

(define (dir-exists? p)
  (let ((real (guard (e (#t #f)) (file-truename p))))
    (and real
         (let ((a (guard (e (#t #f)) (file-attributes real))))
           (and a (eq? (car a) 'directory))))))

(define (regular-file? p)
  (let ((real (guard (e (#t #f)) (file-truename p))))
    (and real
         (let ((a (guard (e (#t #f)) (file-attributes real))))
           (and a (eq? (car a) 'file))))))

(define (parent-dir p)
  (let ((n (string-length p)))
    (if (<= n 1) #f
        (let loop ((i (- n 1)))
          (cond ((< i 0) #f)
                ((char=? (string-ref p i) #\/)
                 (if (= i 0) "/" (substring p 0 i)))
                (else (loop (- i 1))))))))

(define (nth-parent dir n)
  (let loop ((d dir) (k n))
    (if (or (<= k 0) (not d))
        d
        (loop (parent-dir d) (- k 1)))))

(define (relative-to base path)
  (let ((prefix (if (string-suffix? "/" base) base (string-append base "/"))))
    (if (string-prefix? prefix path)
        (substring path (string-length prefix) (string-length path))
        path)))

(define (find-upward start filename)
  (let loop ((dir (file-truename start)))
    (let ((candidate (path-join dir filename)))
      (cond ((regular-file? candidate) dir)
            (else
             (let ((up (parent-dir dir)))
               (if (or (not up) (string=? up dir)) #f (loop up))))))))

(define (collect-files root ext)
  (if (not (dir-exists? root))
      '()
      (let walk ((dir root))
        (let ((entries (directory-list dir)))
          (apply append
            (map (lambda (entry)
                   (cond
                     ((string-suffix? "/" entry)
                      (let ((name (substring entry 0 (- (string-length entry) 1))))
                        (if (or (string=? name "fixtures")
                                (string=? name "support")
                                (string=? name "tests")
                                (string-prefix? "." name))
                            '()
                            (walk (path-join dir name)))))
                     ((string-suffix? ext entry)
                      (list (path-join dir entry)))
                     (else '())))
                 entries))))))

(define (sld-path->library-name lib-root sld-path)
  (let* ((rel (relative-to lib-root sld-path))
         (stem (substring rel 0 (- (string-length rel) 4))))
    (map string->symbol (string-split stem "/"))))

;; Inspect a `.sld` or `.scm` file as data to detect `(define-library NAME ...)`
;; and whether it imports `(techne editor)`.
(define (inspect-library-file path)
  (guard (e (#t (list #f #f)))
    (call-with-input-file path
      (lambda (p)
        (let ((d (read p)))
          (if (and (pair? d) (eq? (car d) 'define-library) (pair? (cdr d)) (list? (cadr d)))
              (let* ((lib-name (cadr d))
                     (imports-editor?
                       (any (lambda (clause)
                              (and (pair? clause) (eq? (car clause) 'import)
                                   (let has-ed? ((x (cdr clause)))
                                     (cond ((equal? x '(techne editor)) #t)
                                           ((pair? x) (or (has-ed? (car x)) (has-ed? (cdr x))))
                                           (else #f)))))
                            (cddr d))))
                (list lib-name imports-editor?))
              (let scan ((form d))
                (cond
                  ((eof-object? form) (list #f #f))
                  ((and (pair? form) (eq? (car form) 'import)
                        (let has-ed? ((x (cdr form)))
                          (cond ((equal? x '(techne editor)) #t)
                                ((pair? x) (or (has-ed? (car x)) (has-ed? (cdr x))))
                                (else #f))))
                   (list #f #t))
                  (else (scan (read p)))))))))))

(define (make-short-temp-dir)
  (let* ((env-tmp (get-environment-variable "TMPDIR"))
         (base (if (and env-tmp (< (string-length env-tmp) 48)) env-tmp "/tmp")))
    (let make ((tries 32))
      (let ((dir (string-append base "/tc-" (hex-encode (random-bytes 5)))))
        (if (guard (e (#t #f)) (create-directory dir #o700) #t)
            dir
            (if (<= tries 1)
                (error "techne check: could not create temp dir" dir)
                (make (- tries 1))))))))

;; Run a Scheme check snippet in a fresh isolated worker (`host` or `editor`)
;; with a bounded timeout. Returns (ok? . problem-strings).
(define (run-check-script executable world cwd timeout extra-env forms)
  (let ((tmp (make-short-temp-dir))
        (nonce (string-append ";;C" (hex-encode (random-bytes 6)))))
    (dynamic-wind
      (lambda () #f)
      (lambda ()
        (let* ((script (path-join tmp "check.scm"))
               (xdg-cfg (path-join tmp "cfg"))
               (xdg-data (path-join tmp "data"))
               (xdg-state (path-join tmp "state"))
               (node-dir (path-join tmp "node"))
               (timed-out #f))
          (create-directory xdg-cfg #o700)
          (create-directory xdg-data #o700)
          (create-directory xdg-state #o700)
          (create-directory node-dir #o700)
          (call-with-output-file script
            (lambda (p)
              (for-each (lambda (f) (write f p) (newline p)) forms)
              (write `(begin
                        (newline)
                        (display ,nonce)
                        (display " ")
                        (write (%check-problems))
                        (newline)
                        (flush-output-port (current-output-port)))
                     p)
              (newline p)))
          (call-with-process executable (list "run" "--world" world script)
            (lambda (p)
              (let* ((watch (spawn (lambda ()
                                     (sleep (* timeout 1000))
                                     (set! timed-out #t)
                                     (process-kill p))))
                     (err-task (spawn (lambda () (process-read-all p 'stderr))))
                     (out (process-read-all p 'stdout))
                     (err (task-join err-task))
                     (code (process-wait p))
                     (prefix (string-append nonce " "))
                     (line (find (lambda (l) (string-prefix? prefix l)) (string-split out "\n"))))
                (guard (e (#t #f)) (task-kill watch))
                (cond
                  (timed-out
                   (cons #f (list (string-append "check worker timed out after " (number->string timeout) "s"))))
                  ((not (eqv? code 0))
                   (cons #f (list (string-append "check worker failed (exit " (number->string code) "): "
                                                 (string-trim (if (string=? err "") out err))))))
                  ((not line)
                   (cons #f (list (string-append "check worker produced no result: " (string-trim err)))))
                  (else
                   (let ((probs (read (open-input-string (substring line (string-length prefix) (string-length line))))))
                     (cons (null? probs) probs))))))
            #:directory cwd
            #:environment (append
                            (list (cons "HOME" tmp)
                                  (cons "TMPDIR" tmp)
                                  (cons "XDG_CONFIG_HOME" xdg-cfg)
                                  (cons "XDG_DATA_HOME" xdg-data)
                                  (cons "XDG_STATE_HOME" xdg-state)
                                  (cons "TECHNE_NODE_DIR" node-dir))
                            extra-env)
            #:stdin 'null)))
      (lambda ()
        (guard (e (#t #f)) (delete-path tmp #t))))))

(define (check-package executable pkg-dir no-editor? timeout)
  (let ((canon (file-truename pkg-dir))
        (err #f)
        (manifest #f))
    (guard (e (#t (set! err (condition/report-string e))))
      (set! manifest (read-package-manifest canon)))
    (if err
        (list (list (string-append "package:" canon) #f (list (string-append "manifest error: " err))))
        (let* ((name (symbol->string (manifest-name manifest)))
               (lib-dir (path-join canon "lib"))
               (slds (guard (e (#t (set! err (condition/report-string e)) '()))
                       (collect-files lib-dir ".sld")))
               (libs (map (lambda (f) (sld-path->library-name lib-dir f)) slds))
               (strip (string-append "\"" canon "/"))
               (results '()))
          (if err
              (list (list (string-append "package:" name) #f (list (string-append "library discovery error: " err))))
              (begin
                ;; 1. Check package libraries in a standalone host world (no editor needed).
                (let ((res
                        (if (null? libs)
                            (cons #t '())
                            (run-check-script executable "host" canon timeout '()
                              (append
                                (list '(import (techne) (techne package) (techne checkdoc))
                                      `(use-package-directory! ,canon))
                                (map (lambda (lib) `(import ,lib)) libs)
                                (list `(define (%check-problems)
                                         (map (lambda (p) (format-problem p #:strip-prefix ,strip))
                                              (documentation-problems ',libs)))))))))
                  (set! results (cons (list (string-append "package:" name) (car res) (cdr res)) results)))
                ;; 2. If the package has an editor entry, register its libraries and verify commands & docs.
                (let ((ed (manifest-editor manifest)))
                  (when (and ed (not no-editor?))
                    (let* ((entry (car ed))
                           (advertised-cmds (map car (cadr ed)))
                           (ed-res
                             (run-check-script executable "editor" canon timeout '()
                               (list '(import (techne package))
                                     `(require ,(path-join (%lisp-directory) "editor/main.scm"))
                                     `(require ,(path-join (%lisp-directory) "editor/api.scm"))
                                     `(require ,(path-join (%lisp-directory) "editor/builtins.scm"))
                                     `(require ,(path-join (%lisp-directory) "editor/checkdoc.scm"))
                                     `(define (%fmt p)
                                        (string-append (repr (problem-module p)) " "
                                                       (symbol->string (problem-subject p)) " "
                                                       (symbol->string (problem-name p)) ": "
                                                       (problem-text p)))
                                     `(define %before-mods (loaded-modules))
                                     `(define %before-ed (map %fmt (editor-documentation-problems)))
                                     `(use-package-directory! ,canon)
                                     `(load-package ',(manifest-name manifest) ,entry)
                                     `(define (%check-problems)
                                        (let* ((added (filter (lambda (m) (not (member m %before-mods))) (loaded-modules)))
                                               (mod-probs (map %fmt (append-map module-documentation-problems added)))
                                               (ed-probs (filter (lambda (s) (not (member s %before-ed)))
                                                                 (map %fmt (editor-documentation-problems))))
                                               (missing-cmds
                                                 (filter-map
                                                   (lambda (c)
                                                     (and (not (memq c (command-names)))
                                                          (string-append "manifest command " (symbol->string c)
                                                                         ": advertised in techne-package.scm but not defined by " ,entry ".")))
                                                   ',advertised-cmds)))
                                          (append missing-cmds mod-probs ed-probs)))))))
                      (set! results (cons (list (string-append "package:" name " (editor)") (car ed-res) (cdr ed-res)) results)))))
                (reverse results)))))))

(define (read-workspace-packages ws-dir)
  (let* ((file (path-join ws-dir "techne-workspace.scm"))
         (datum (call-with-input-file file (lambda (p) (read p)))))
    (unless (and (pair? datum) (eq? (car datum) 'workspace))
      (error "techne-workspace.scm: expected (workspace ...)" file))
    (apply append
      (filter-map (lambda (c) (and (pair? c) (eq? (car c) 'packages) (cdr c)))
                  (cdr datum)))))

(define (workspace-package-dirs ws-dir)
  (apply append
    (map (lambda (rel)
           (let* ((base (path-join ws-dir rel))
                  (entries (if (dir-exists? base) (directory-list base) '())))
             (filter-map
               (lambda (entry)
                 (and (string-suffix? "/" entry)
                      (let ((d (path-join base (substring entry 0 (- (string-length entry) 1)))))
                        (and (regular-file? (path-join d "techne-package.scm")) d))))
               entries)))
         (read-workspace-packages ws-dir))))

(define (safe-manifest-name pdir)
  (guard (e (#t #f))
    (symbol->string (manifest-name (read-package-manifest pdir)))))

(define (techne-source-workspace? ws-dir)
  (and (regular-file? (path-join ws-dir "lisp/editor/main.scm"))
       (dir-exists? (path-join ws-dir "lisp/techne"))))

(define (check-workspace-core executable ws-dir no-editor? timeout)
  (let* ((canon (file-truename ws-dir))
         (lisp-dir (path-join canon "lisp"))
         (strip (string-append "\"" canon "/"))
         (techne-slds (collect-files (path-join lisp-dir "techne") ".sld"))
         (techne-libs
           (filter-map
             (lambda (f)
               (let ((info (inspect-library-file f)))
                 (if (and no-editor? (cadr info))
                     #f
                     (sld-path->library-name lisp-dir f))))
             techne-slds))
         (core-files
           (filter regular-file?
             (append
               (list (path-join lisp-dir "test.scm")
                     (path-join lisp-dir "schema.scm"))
               (collect-files (path-join lisp-dir "org") ".scm")
               (collect-files (path-join lisp-dir "http") ".scm")
               (collect-files (path-join lisp-dir "cloud") ".scm"))))
         (runtime-pkgs
           (filter (lambda (pair) (regular-file? (cdr pair)))
             (list (cons 'compile-runtime (path-join lisp-dir "editor/compile/runtime.scm"))
                   (cons 'vc-runtime (path-join lisp-dir "editor/packages/vc/runtime.scm"))
                   (cons 'direnv-runtime (path-join lisp-dir "editor/direnv/runtime.scm"))
                   (cons 'dired-runtime (path-join lisp-dir "editor/packages/dired/runtime.scm"))
                   (cons 'project-settings-runtime (path-join lisp-dir "editor/project-settings/runtime.scm")))))
         (pkg-ed-entries
           (filter-map
             (lambda (pdir)
               (guard (e (#t #f))
                 (let* ((m (read-package-manifest pdir))
                        (ed (manifest-editor m)))
                   (and ed (cons (manifest-name m) (car ed))))))
             (workspace-package-dirs canon))))
    (if no-editor?
        (let ((res
                (run-check-script executable "host" canon timeout '()
                  (append
                    (list '(import (techne) (techne checkdoc)))
                    (map (lambda (f) `(require ,f)) core-files)
                    (map (lambda (lib) `(import ,lib)) techne-libs)
                    (list `(define (%check-problems)
                             (map (lambda (p) (format-problem p #:strip-prefix ,strip))
                                  (documentation-problems))))))))
          (list (list "workspace:core" (car res) (cdr res))))
        (let ((res
                (run-check-script executable "editor" canon timeout '()
                  (append
                    (list `(require ,(path-join lisp-dir "editor/main.scm"))
                          `(require ,(path-join lisp-dir "editor/api.scm"))
                          `(require ,(path-join lisp-dir "editor/builtins.scm")))
                    (map (lambda (f) `(require ,f)) core-files)
                    (map (lambda (p) `(load-package ',(car p) ,(cdr p))) (append pkg-ed-entries runtime-pkgs))
                    (map (lambda (lib) `(import ,lib)) techne-libs)
                    (list `(require ,(path-join lisp-dir "editor/checkdoc.scm")))
                    (list `(define (%check-problems)
                             (map (lambda (p)
                                    (let* ((mod (repr (problem-module p)))
                                           (mod (if (string-prefix? ,strip mod)
                                                    (string-append "\"" (substring mod (string-length ,strip) (string-length mod)))
                                                    mod)))
                                      (string-append mod " "
                                                     (symbol->string (problem-subject p)) " "
                                                     (symbol->string (problem-name p)) ": "
                                                     (problem-text p))))
                                  (documentation-problems))))))))
          (list (list "workspace:core+editor" (car res) (cdr res)))))))

(define (check-single-file executable file-path force-editor? no-editor? timeout)
  (let* ((canon (file-truename file-path))
         (sld? (string-suffix? ".sld" canon))
         (scm? (string-suffix? ".scm" canon)))
    (unless (or sld? scm?)
      (display (string-append "techne check: unsupported file extension (expected .scm or .sld): " file-path "\n")
               (current-error-port))
      (exit 2))
    (let* ((info (inspect-library-file canon))
           (declared-lib (car info))
           (needs-editor? (and (not no-editor?)
                               (or force-editor?
                                   (cadr info)
                                   (and (string-contains canon "/lisp/editor/") #t))))
           (pkg-dir (find-upward (parent-dir canon) "techne-package.scm"))
           (cwd (or pkg-dir (parent-dir canon)))
           (strip (string-append "\"" cwd "/"))
           (lib-root (if (and sld? declared-lib)
                         (nth-parent (parent-dir canon) (- (length declared-lib) 1))
                         #f))
           (extra-env (if (and lib-root (not pkg-dir))
                          (list (cons "TECHNE_LIBRARY_PATH" lib-root))
                          '()))
           (world (if needs-editor? "editor" "host"))
           (editor-preamble
             (if needs-editor?
                 (list `(require ,(path-join (%lisp-directory) "editor/main.scm"))
                       `(require ,(path-join (%lisp-directory) "editor/api.scm"))
                       `(require ,(path-join (%lisp-directory) "editor/builtins.scm")))
                 '()))
           (forms
             (cond
               (sld?
                (unless declared-lib
                  (error "expected a (define-library NAME ...) form in .sld file" file-path))
                (append
                  editor-preamble
                  (list '(import (techne) (techne package))
                        '(import (prefix (techne checkdoc) core-)))
                  (if pkg-dir (list `(use-package-directory! ,pkg-dir)) '())
                  (list `(import ,declared-lib))
                  (if needs-editor?
                      (list `(require ,(path-join (%lisp-directory) "editor/checkdoc.scm"))
                            `(define (%check-problems)
                               (map (lambda (p) (core-format-problem p #:strip-prefix ,strip))
                                    (module-documentation-problems ',declared-lib))))
                      (list `(define (%check-problems)
                               (map (lambda (p) (core-format-problem p #:strip-prefix ,strip))
                                    (core-module-documentation-problems ',declared-lib)))))))
               (else
                (append
                  editor-preamble
                  (list '(import (techne) (techne package))
                        '(import (prefix (techne checkdoc) core-)))
                  (if pkg-dir (list `(use-package-directory! ,pkg-dir)) '())
                  (list `(require ,canon))
                  (if needs-editor?
                      (list `(require ,(path-join (%lisp-directory) "editor/checkdoc.scm"))
                            `(define (%check-problems)
                               (map (lambda (p) (core-format-problem p #:strip-prefix ,strip))
                                    (module-documentation-problems ,canon))))
                      (list `(define (%check-problems)
                               (map (lambda (p) (core-format-problem p #:strip-prefix ,strip))
                                    (core-module-documentation-problems ,canon)))))))))
           (res (run-check-script executable world cwd timeout extra-env forms)))
      (list (list file-path (car res) (cdr res))))))

(define (parse-cli argv)
  (let ((packages '())
        (paths '())
        (all? #f)
        (force-editor? #f)
        (no-editor? #f)
        (timeout 120)
        (format-kind 'human)
        (help? #f))
    (define (bad msg)
      (display (string-append "techne check: " msg "\n") (current-error-port))
      (exit 2))
    (let loop ((xs argv))
      (unless (null? xs)
        (let ((a (car xs)))
          (cond
            ((or (string=? a "-h") (string=? a "--help"))
             (set! help? #t) (loop (cdr xs)))
            ((or (string=? a "--all") (string=? a "--workspace"))
             (set! all? #t) (loop (cdr xs)))
            ((string=? a "--editor")
             (set! force-editor? #t) (loop (cdr xs)))
            ((string=? a "--no-editor")
             (set! no-editor? #t) (loop (cdr xs)))
            ((string=? a "--timeout")
             (when (null? (cdr xs)) (bad "--timeout expects a positive integer"))
             (let ((n (string->number (cadr xs))))
               (unless (and (exact-integer? n) (> n 0)) (bad "--timeout expects a positive integer"))
               (set! timeout n))
             (loop (cddr xs)))
            ((or (string=? a "-p") (string=? a "--package"))
             (when (null? (cdr xs)) (bad "--package expects a package name"))
             (set! packages (append packages (list (cadr xs))))
             (loop (cddr xs)))
            ((string=? a "--format")
             (when (null? (cdr xs)) (bad "--format expects human or sexp"))
             (cond ((string=? (cadr xs) "human") (set! format-kind 'human))
                   ((string=? (cadr xs) "sexp") (set! format-kind 'sexp))
                   (else (bad (string-append "unknown format: " (cadr xs)))))
             (loop (cddr xs)))
            ((string-prefix? "-" a)
             (bad (string-append "unknown option: " a)))
            (else
             (unless (guard (e (#t #f)) (file-attributes a))
               (bad (string-append "path not found: " a)))
             (set! paths (append paths (list a)))
             (loop (cdr xs)))))))
    (list (cons 'packages packages)
          (cons 'paths paths)
          (cons 'all all?)
          (cons 'force-editor force-editor?)
          (cons 'no-editor no-editor?)
          (cons 'timeout timeout)
          (cons 'format format-kind)
          (cons 'help help?))))

(let* ((cl (command-line))
       (executable (cadr cl))
       (argv (cddr cl))
       (opts (parse-cli argv))
       (opt (lambda (k) (cdr (assq k opts)))))
  (when (opt 'help)
    (display usage-text)
    (exit 0))
  (let* ((targets (if (null? (opt 'paths)) (list ".") (opt 'paths)))
         (selected (opt 'packages))
         (all? (opt 'all))
         (force-editor? (opt 'force-editor))
         (no-editor? (opt 'no-editor))
         (timeout (opt 'timeout))
         (matched-packages '())
         (results
           (apply append
             (map (lambda (t)
                    (let ((canon (file-truename t)))
                      (cond
                        ((regular-file? canon)
                         (check-single-file executable t force-editor? no-editor? timeout))
                        ((dir-exists? canon)
                         (let* ((ws-dir (find-upward canon "techne-workspace.scm"))
                                (enclosing-pkg (and (not all?) (find-upward canon "techne-package.scm")))
                                (pkg-dir (if (and (pair? selected) ws-dir) #f enclosing-pkg)))
                           (cond
                             (pkg-dir
                              (let ((pname (safe-manifest-name pkg-dir)))
                                (cond
                                  ((not pname)
                                   ;; Malformed manifest: let check-package report it cleanly.
                                   (check-package executable pkg-dir no-editor? timeout))
                                  ((or (null? selected) (member pname selected))
                                   (set! matched-packages (cons pname matched-packages))
                                   (check-package executable pkg-dir no-editor? timeout))
                                  (else '()))))
                             (ws-dir
                              (let* ((pdirs (guard (e (#t '())) (workspace-package-dirs ws-dir)))
                                     (pdirs (if (pair? selected)
                                                (filter (lambda (d)
                                                          (let ((pname (safe-manifest-name d)))
                                                            (and pname (member pname selected)
                                                                 (begin (set! matched-packages (cons pname matched-packages)) #t))))
                                                        pdirs)
                                                pdirs))
                                     (pkg-res (apply append (map (lambda (d) (check-package executable d no-editor? timeout)) pdirs)))
                                     (core-res (if (and (null? selected) (techne-source-workspace? ws-dir))
                                                   (check-workspace-core executable ws-dir no-editor? timeout)
                                                   '())))
                                (append pkg-res core-res)))
                             (else
                              (list (list t #f (list "no techne-package.scm or techne-workspace.scm found")))))))
                        (else '()))))
                  targets)))
         (missing-pkgs
           (filter (lambda (p) (not (member p matched-packages))) selected))
         (results
           (append results
             (map (lambda (p) (list (string-append "package:" p) #f (list (string-append "package not found: " p))))
                  missing-pkgs))))
    (when (null? results)
      (display "techne check: nothing found to check\n" (current-error-port))
      (exit 1))
    (let* ((bad (filter (lambda (r) (not (cadr r))) results))
           (total-problems (apply + (map (lambda (r) (length (caddr r))) results)))
           (ok? (null? bad)))
      (if (eq? (opt 'format) 'sexp)
          (begin
            (write `((status ,(if ok? 'ok 'failed))
                     (checked ,(length results))
                     (problems ,total-problems)
                     (results ,results)))
            (newline))
          (begin
            (for-each
              (lambda (r)
                (display (if (cadr r) "  OK   " "  FAIL "))
                (display (car r))
                (unless (cadr r)
                  (display " (") (display (length (caddr r))) (display " problems)"))
                (newline)
                (for-each (lambda (msg) (display "    ") (displayln msg)) (caddr r)))
              results)
            (newline)
            (display "check result: ") (display (if ok? "ok" "FAILED")) (display ". ")
            (display (length results)) (display " checked, ")
            (display total-problems) (displayln " problems")))
      (exit (if ok? 0 1)))))
