;;; Implementation of `(techne lock)`.

(define lockfile-name "techne.lock"
  "The name of the lockfile written beside `techne-package.scm`.")

(define-record-type lockfile
  (%make-lockfile techne packages)
  lockfile?
  (techne lockfile-techne)
  (packages lockfile-packages))

(define-record-type locked-package
  (%make-locked-package name version source tree-sha256 depends)
  locked-package?
  (name locked-package-name)
  (version locked-package-version)
  (source locked-package-source)
  (tree-sha256 locked-package-tree-sha256)
  (depends locked-package-depends))

(define (make-lockfile techne packages)
  "Construct a validated lockfile record with TECHNE version and PACKAGES."
  (unless (version-string? techne)
    (error "Invalid techne version in lockfile" techne))
  (unless (and (list? packages) (every locked-package? packages))
    (error "Expected a list of locked packages" packages))
  (%make-lockfile techne (sort packages (lambda (a b) (package-name<? (locked-package-name a) (locked-package-name b))))))

(define (make-locked-package name version source tree-sha256 depends)
  "Construct a validated locked package record for NAME.
VERSION is a version string, SOURCE is a pinned locked source datum,
TREE-SHA256 is a 64-character lowercase hex digest, and DEPENDS is a
list of canonical `(owner pkg)` dependency names."
  (unless (and (package-name? name) (not (reserved-namespace? name)))
    (error "Invalid locked package name" name))
  (unless (version-string? version)
    (error "Invalid locked package version" version))
  (unless (locked-source-valid? source)
    (error "Invalid locked package source" source))
  (unless (sha256-hex? tree-sha256)
    (error "Invalid tree-sha256 digest" tree-sha256))
  (unless (and (list? depends)
               (every (lambda (d) (and (package-name? d) (not (reserved-namespace? d)))) depends))
    (error "Invalid locked package depends" depends))
  (when (member name depends)
    (error "Locked package cannot depend on itself" name))
  (%make-locked-package name version source tree-sha256 (sort (delete-duplicates depends) package-name<?)))

;; --- Validation helpers ---

(define (ascii-alnum? c)
  (let ((n (char->integer c)))
    (or (and (>= n 97) (<= n 122))
        (and (>= n 65) (<= n 90))
        (and (>= n 48) (<= n 57)))))

(define (component? s)
  (and (string? s) (positive? (string-length s))
       (ascii-alnum? (string-ref s 0))
       (every (lambda (c) (or (ascii-alnum? c) (memv c '(#\- #\_ #\.)))) (string->list s))))

(define (reserved-namespace? name)
  (memq (car name) '(scheme srfi)))

(define (version-string? s)
  (and (string? s) (positive? (string-length s))
       (char-numeric? (string-ref s 0))
       (every (lambda (c) (or (char-alphabetic? c) (char-numeric? c) (memv c '(#\. #\- #\+))))
              (string->list s))))

(define (hex-string? s)
  (and (string? s) (every (lambda (c) (and (string-index "0123456789abcdef" c) #t)) (string->list s))))

(define (oid? s)
  (and (hex-string? s) (memv (string-length s) '(40 64)) #t))

(define (sha256-hex? s)
  (and (hex-string? s) (= 64 (string-length s))))

(define (https-url? s)
  (and (string? s) (string-prefix? "https://" s) (> (string-length s) 8)
       (not (any (lambda (c) (or (char-whitespace? c) (< (char->integer c) 32))) (string->list s)))))

(define (subdir-string? s)
  (and (string? s) (positive? (string-length s))
       (every component? (string-split s "/"))))

(define (git-ref-clause? x)
  (and (list? x) (= 2 (length x)) (string? (cadr x)) (positive? (string-length (cadr x)))
       (case (car x)
         ((branch tag) (every component? (string-split (cadr x) "/")))
         ((commit) (oid? (cadr x)))
         (else #f))))

(define (locked-source-valid? src)
  (and (list? src) (>= (length src) 2) (string? (cadr src))
       (case (car src)
         ((path)
          (and (= 2 (length src)) (positive? (string-length (cadr src))) (not (path-absolute? (cadr src)))))
         ((github git local)
          (and (case (car src)
                 ((github) (let ((parts (string-split (cadr src) "/")))
                             (and (= 2 (length parts)) (every component? parts))))
                 ((git) (https-url? (cadr src)))
                 ((local) (and (positive? (string-length (cadr src))) (path-absolute? (cadr src)))))
               (let* ((rest (cddr src))
                      (commits (filter (lambda (e) (and (pair? e) (eq? (car e) 'commit))) rest))
                      (refs (filter (lambda (e) (and (pair? e) (eq? (car e) 'ref))) rest))
                      (subdirs (filter (lambda (e) (and (pair? e) (eq? (car e) 'subdir))) rest)))
                 (and (= 1 (length commits))
                      (<= (length refs) 1)
                      (<= (length subdirs) 1)
                      (= (length rest) (+ (length commits) (length refs) (length subdirs)))
                      (let ((c (car commits)))
                        (and (list? c) (= 2 (length c)) (oid? (cadr c))))
                      (every (lambda (r) (and (list? r) (= 2 (length r)) (git-ref-clause? (cadr r)))) refs)
                      (every (lambda (sd) (and (list? sd) (= 2 (length sd)) (subdir-string? (cadr sd)))) subdirs)))))
         (else #f))))

(define (locked-source-kind src) (car src))
(define (locked-source-location src) (cadr src))
(define (locked-source-commit src)
  (let ((c (assq 'commit (cddr src)))) (and c (cadr c))))
(define (locked-source-ref src)
  (let ((r (assq 'ref (cddr src)))) (and r (cadr r))))
(define (locked-source-subdir src)
  (let ((s (assq 'subdir (cddr src)))) (and s (cadr s))))

(define (format-pkg-name name)
  (string-append "(" (symbol->string (car name)) " " (symbol->string (cadr name)) ")"))

(define (format-chain chain)
  (string-join (map format-pkg-name chain) " -> "))

(define (distinct-names! what names)
  (unless (= (length names) (length (delete-duplicates names)))
    (error what names)))

;; --- Pure-data lockfile reader & writer ---

(define (bad-lock what . irritants)
  (apply error (string-append lockfile-name ": " what) irritants))

(define (parse-locked-package form)
  (unless (and (list? form) (>= (length form) 3) (eq? 'package (car form))
               (package-name? (cadr form)) (not (reserved-namespace? (cadr form)))
               (version-string? (caddr form)))
    (bad-lock "expected (package (owner pkg) \"version\" (source ...) (tree-sha256 ...) [(depends ...)])" form))
  (let* ((name (cadr form))
         (version (caddr form))
         (clauses (cdddr form)))
    (unless (every (lambda (c) (and (pair? c) (list? c) (memq (car c) '(source tree-sha256 depends)))) clauses)
      (bad-lock "unknown package clause in lockfile" form))
    (distinct-names! (string-append lockfile-name ": duplicate package clause") (map car clauses))
    (let ((src-clause (assq 'source clauses))
          (hash-clause (assq 'tree-sha256 clauses))
          (deps-clause (assq 'depends clauses)))
      (unless (and src-clause (= 2 (length src-clause)) (locked-source-valid? (cadr src-clause)))
        (bad-lock "invalid or missing (source ...) clause" name))
      (unless (and hash-clause (= 2 (length hash-clause)) (sha256-hex? (cadr hash-clause)))
        (bad-lock "invalid or missing (tree-sha256 \"<64-hex>\") clause" name))
      (let ((deps (if deps-clause (cdr deps-clause) '())))
        (unless (every (lambda (d) (and (package-name? d) (not (reserved-namespace? d)))) deps)
          (bad-lock "invalid dependency name in (depends ...)" name deps))
        (distinct-names! (string-append lockfile-name ": duplicate dependency in lockfile") deps)
        (when (member name deps)
          (bad-lock "package cannot depend on itself" name))
        (make-locked-package name version (cadr src-clause) (cadr hash-clause) deps)))))

(define (check-lockfile-graph! packages)
  (let ((by-name (make-hash-table equal?)))
    (for-each (lambda (p) (hash-table-set! by-name (locked-package-name p) p)) packages)
    ;; Check referential integrity: every listed dep exists in the lockfile.
    (for-each
      (lambda (p)
        (for-each
          (lambda (dep)
            (unless (hash-table-ref/default by-name dep #f)
              (bad-lock (string-append "package " (format-pkg-name (locked-package-name p))
                                       " depends on missing locked package " (format-pkg-name dep)))))
          (locked-package-depends p)))
      packages)
    ;; Check for cycles.
    (let ((visited (make-hash-table equal?)))
      (define (visit name stack)
        (when (member name stack)
          (let* ((cycle-tail (reverse (cons name stack))))
            (error (string-append "Dependency cycle detected: " (format-chain cycle-tail)))))
        (unless (hash-table-ref/default visited name #f)
          (let ((p (hash-table-ref/default by-name name #f)))
            (when p
              (for-each (lambda (dep) (visit dep (cons name stack))) (locked-package-depends p))
              (hash-table-set! visited name #t)))))
      (for-each (lambda (p) (visit (locked-package-name p) '())) packages))))

(define (parse-lockfile datum)
  "Validate pure-data DATUM and return a lockfile record.
Nothing is evaluated."
  (unless (and (list? datum) (>= (length datum) 2) (eq? 'lock (car datum)))
    (bad-lock "expected (lock (techne \"version\") (package ...) ...)"))
  (let* ((clauses (cdr datum))
         (technes (filter (lambda (c) (and (pair? c) (eq? 'techne (car c)))) clauses))
         (pkgs (filter (lambda (c) (and (pair? c) (eq? 'package (car c)))) clauses)))
    (unless (= (length clauses) (+ (length technes) (length pkgs)))
      (bad-lock "unknown top-level clause in lockfile"))
    (unless (and (= 1 (length technes))
                 (list? (car technes)) (= 2 (length (car technes)))
                 (version-string? (cadr (car technes))))
      (bad-lock "expected exactly one (techne \"version\") clause"))
    (let ((parsed (map parse-locked-package pkgs)))
      (distinct-names! (string-append lockfile-name ": duplicate package entry") (map locked-package-name parsed))
      (check-lockfile-graph! parsed)
      (make-lockfile (cadr (car technes)) parsed))))

(define (read-lockfile path)
  "Read and validate the pure-data lockfile at PATH without evaluating it."
  (unless (file-exists? path)
    (error (string-append "Missing lockfile: " path)))
  (let ((d (call-with-input-file path
             (lambda (p)
               (let ((first (read p)))
                 (unless (and (not (eof-object? first)) (eof-object? (read p)))
                   (bad-lock "expected exactly one datum" path))
                 first)))))
    (parse-lockfile d)))

(define (write-locked-source src port)
  (display "(" port)
  (display (symbol->string (car src)) port)
  (display " " port)
  (write (cadr src) port)
  (unless (eq? 'path (car src))
    (display " (commit " port)
    (write (locked-source-commit src) port)
    (display ")" port)
    (let ((ref (locked-source-ref src)))
      (when ref
        (display " (ref " port)
        (write ref port)
        (display ")" port)))
    (let ((sub (locked-source-subdir src)))
      (when sub
        (display " (subdir " port)
        (write sub port)
        (display ")" port))))
  (display ")" port))

(define (format-lockfile lf)
  "Serialize lockfile LF into canonical, deterministic S-expression text."
  (unless (lockfile? lf)
    (error "Expected a lockfile" lf))
  (let ((pkgs (sort (lockfile-packages lf)
                    (lambda (a b) (package-name<? (locked-package-name a) (locked-package-name b))))))
    (call-with-output-string
      (lambda (port)
        (display "(lock\n  (techne " port)
        (write (lockfile-techne lf) port)
        (display ")" port)
        (for-each
          (lambda (p)
            (display "\n  (package " port)
            (write (locked-package-name p) port)
            (display " " port)
            (write (locked-package-version p) port)
            (display "\n    (source " port)
            (write-locked-source (locked-package-source p) port)
            (display ")\n    (tree-sha256 " port)
            (write (locked-package-tree-sha256 p) port)
            (display ")\n    (depends" port)
            (for-each
              (lambda (d)
                (display " " port)
                (write d port))
              (sort (locked-package-depends p) package-name<?))
            (display "))" port))
          pkgs)
        (display ")\n" port)))))

(define (write-lockfile path lf)
  "Write lockfile LF atomically to PATH."
  (replace-file path (format-lockfile lf))
  lf)

;; --- Content-addressed store ---

(define (packages-data-dir)
  "Return the default packages data directory.
Defaults to `$XDG_DATA_HOME/techne/packages`."
  (let ((xdg (get-environment-variable "XDG_DATA_HOME")))
    (path-join (if (and xdg (path-absolute? xdg))
                   xdg
                   (path-join (or (get-environment-variable "HOME") ".") ".local/share"))
               "techne/packages")))

(define (package-store-dir #:data-dir [data-dir #f])
  "Return the content-addressed package store directory.
Uses DATA-DIR/store if DATA-DIR is given, else `TECHNE_PACKAGE_STORE` if
set to an absolute path, else `(path-join (packages-data-dir) \"store\")`."
  (cond (data-dir (path-join data-dir "store"))
        (else
         (let ((env (get-environment-variable "TECHNE_PACKAGE_STORE")))
           (if (and env (path-absolute? env))
               env
               (path-join (packages-data-dir) "store"))))))

;; Copy SOURCE-DIR into DEST-DIR cleanly: exclude hidden entries (starting
;; with `.`) and reject symlinks and non-regular files.
(define (copy-clean-tree! source-dir dest-dir)
  (let ((src-root (file-truename source-dir)))
    (let walk ((src src-root) (dst dest-dir) (prefix ""))
      (for-each
        (lambda (raw)
          (let ((name (if (string-suffix? "/" raw)
                          (substring raw 0 (- (string-length raw) 1))
                          raw)))
            (unless (string-prefix? "." name)
              (let* ((rel (if (string=? prefix "") name (string-append prefix "/" name)))
                     (src-path (path-join src name))
                     (dst-path (path-join dst name))
                     (attrs (file-attributes src-path))
                     (kind (and attrs (car attrs))))
                (cond ((eq? kind 'symlink)
                       (error "Symlink not allowed in package tree" rel))
                      ((eq? kind 'directory)
                       (create-directory dst-path #o755)
                       (walk src-path dst-path rel))
                      ((eq? kind 'file)
                       (copy-file src-path dst-path))
                      (else
                       (error "Unsupported file type in package tree" rel)))))))
        (directory-list src)))))

;; Atomically publish an already-staged directory STAGED-DIR with known HASH
;; into STORE-DIR/HASH. Idempotent if STORE-DIR/HASH already exists and
;; passes tree-sha256 verification.
(define (publish-staged-tree! staged-dir hash store-dir)
  (make-directory store-dir)
  (let ((dest (path-join store-dir hash)))
    (cond ((file-exists? dest)
           (let ((actual (package-tree-sha256 dest)))
             (unless (string=? actual hash)
               (error (string-append "Corrupted store entry at " dest ": expected " hash ", got " actual)))
             dest))
          (else
           (guard (e ((file-exists? dest)
                      (let ((actual (package-tree-sha256 dest)))
                        (unless (string=? actual hash)
                          (error (string-append "Corrupted store entry at " dest ": expected " hash ", got " actual)))
                        dest)))
             (rename-file staged-dir dest)
             dest)))))

(define (materialize-package-tree! source-dir #:store [store (package-store-dir)])
  "Materialize SOURCE-DIR into the content-addressed STORE directory.
Excludes hidden files and directories (such as `.git` and `.jj`), rejects
symlinks, computes canonical `tree-sha256`, stages into a temporary
directory, and atomically renames into `STORE/<tree-sha256>/`. Returns
`(cons tree-sha256 dest-path)`."
  (make-directory store)
  (let ((tmp (path-join store (string-append ".stage-" (hex-encode (random-bytes 12))))))
    (create-directory tmp #o700)
    (dynamic-wind
      (lambda () #f)
      (lambda ()
        (copy-clean-tree! source-dir tmp)
        (let* ((hash (package-tree-sha256 tmp))
               (dest (publish-staged-tree! tmp hash store)))
          (cons hash dest)))
      (lambda ()
        (when (file-exists? tmp)
          (delete-path tmp #t))))))

;; --- Path helpers ---

(define (split-abs-path path)
  (filter (lambda (p) (positive? (string-length p))) (string-split path "/")))

(define (path-relative-to base target)
  (let loop ((b (split-abs-path (file-truename base)))
             (t (split-abs-path (file-truename target))))
    (cond ((and (pair? b) (pair? t) (string=? (car b) (car t)))
           (loop (cdr b) (cdr t)))
          (else
           (let ((parts (append (make-list (length b) "..") t)))
             (if (null? parts) "." (string-join parts "/")))))))

;; Resolve logical POSIX relative path REL against optional base subdir SUB
;; (string or #f) inside a Git repository. Errors if REL escapes repo root.
(define (resolve-subdir-path base-sub rel)
  (let ((init (if base-sub (string-split base-sub "/") '())))
    (let loop ((parts (string-split rel "/")) (stack (reverse init)))
      (cond ((null? parts)
             (if (null? stack) #f (string-join (reverse stack) "/")))
            ((or (string=? (car parts) "") (string=? (car parts) "."))
             (loop (cdr parts) stack))
            ((string=? (car parts) "..")
             (when (null? stack)
               (error "A package depends on one outside its repository" rel))
             (loop (cdr parts) (cdr stack)))
            (else
             (unless (component? (car parts))
               (error "Invalid relative path component" (car parts) rel))
             (loop (cdr parts) (cons (car parts) stack)))))))

;; --- Git fetching & package discovery ---

(define (git-run dir protocol . args)
  (call-with-process "git"
    (append (list "-c" "core.hooksPath=/dev/null"
                  "-c" "core.symlinks=false"
                  "-c" "filter.lfs.smudge="
                  "-c" "filter.lfs.process="
                  "-c" "protocol.allow=never"
                  "-c" (string-append "protocol." protocol ".allow=always"))
            args)
    (lambda (p)
      (let ((out (process-read-all p 'stdout)))
        (unless (eqv? 0 (process-wait p))
          (error "Package Git operation failed" (car args)))
        (string-trim out)))
    #:directory dir
    #:stdin 'null
    #:stderr 'inherit
    #:environment '(("GIT_TERMINAL_PROMPT" . "0")
                    ("GIT_CONFIG_NOSYSTEM" . "1")
                    ("GIT_DIR" . #f)
                    ("GIT_WORK_TREE" . #f)
                    ("GIT_INDEX_FILE" . #f)
                    ("GIT_OBJECT_DIRECTORY" . #f)
                    ("GIT_ALTERNATE_OBJECT_DIRECTORIES" . #f)
                    ("GIT_CONFIG_PARAMETERS" . #f)
                    ("GIT_CONFIG_COUNT" . #f)
                    ("GIT_CONFIG_GLOBAL" . #f)
                    ("GIT_CONFIG_SYSTEM" . #f))))

(define (source-git-url src)
  (case (car src)
    ((github) (string-append "https://github.com/" (cadr src) ".git"))
    ((git) (cadr src))
    ((local) (cadr src))
    (else (error "Not a Git package source" src))))

(define (source-git-protocol src)
  (if (eq? (car src) 'local) "file" "https"))

(define (source-git-ref src)
  (find (lambda (e) (and (pair? e) (memq (car e) '(branch tag commit)))) (cddr src)))

(define (source-git-package src)
  (let ((found (assq 'package (cddr src))))
    (and found (cadr found))))

(define (commit-matches? declared-oid resolved-oid)
  (or (string=? declared-oid resolved-oid)
      (string-prefix? declared-oid resolved-oid)))

(define (verify-checkout-contained! checkout-dir pkg-dir)
  (let ((root-true (file-truename checkout-dir))
        (pkg-true (file-truename pkg-dir)))
    (unless (or (equal? pkg-true root-true)
                (string-prefix? (string-append root-true "/") pkg-true))
      (error "Package path escapes Git checkout" pkg-dir))))

;; Reject any non-hidden Git symlink entry (mode 120000) in OID under optional
;; SUB. Needed because `-c core.symlinks=false` checks out symlinks as plain
;; files so they cannot be followed on disk.
(define (check-git-tree-symlinks! checkout-dir protocol oid sub)
  (let* ((has-sub (and sub (positive? (string-length sub))))
         (args (if has-sub
                   (list "-c" "core.quotepath=false" "ls-tree" "-r" oid "--" sub)
                   (list "-c" "core.quotepath=false" "ls-tree" "-r" oid)))
         (out (apply git-run checkout-dir protocol args)))
    (unless (string=? out "")
      (for-each
        (lambda (line)
          (when (string-prefix? "120000 " line)
            (let ((tab (string-index line #\tab)))
              (when tab
                (let* ((path (substring line (+ tab 1) (string-length line)))
                       (rel (if (and has-sub (string-prefix? (string-append sub "/") path))
                                (substring path (+ (string-length sub) 1) (string-length path))
                                path))
                       (parts (string-split rel "/")))
                  (unless (any (lambda (p) (string-prefix? "." p)) parts)
                    (error "Symlink not allowed in package tree" rel)))))))
        (string-split out "\n")))))

(define (fetch-git-checkout! tmp-root src)
  (let* ((dir (path-join tmp-root (string-append "fetch-" (hex-encode (random-bytes 8)))))
         (protocol (source-git-protocol src))
         (url (source-git-url src))
         (ref (source-git-ref src))
         (refspec (if ref
                      (case (car ref)
                        ((branch) (string-append "refs/heads/" (cadr ref)))
                        ((tag) (string-append "refs/tags/" (cadr ref)))
                        ((commit) (cadr ref)))
                      "HEAD")))
    (create-directory dir #o700)
    (git-run dir protocol "init" "--quiet" "--template=")
    (let ((oid
           (guard (e ((and ref (eq? (car ref) 'commit))
                      ;; Fallback when a remote/local repo does not allow
                      ;; shallow direct fetch of a non-tip commit OID.
                      (git-run dir protocol "fetch" "--quiet" "--no-tags" "--" url)
                      (git-run dir protocol "rev-parse" "--verify" (string-append (cadr ref) "^{commit}"))))
             (git-run dir protocol "fetch" "--quiet" "--depth=1" "--no-tags" "--" url refspec)
             (git-run dir protocol "rev-parse" "--verify" "FETCH_HEAD^{commit}"))))
      (unless (oid? oid)
        (error "Git returned an invalid commit" oid))
      (git-run dir protocol "checkout" "--quiet" "--detach" oid)
      (cons oid dir))))

;; Scan CHECKOUT-DIR for package manifests using lstat (`file-attributes`) on
;; every entry so symlinks are never followed, and continue scanning nested
;; subdirectories even when the repository root itself holds a manifest.
(define (repo-package-directories checkout-dir)
  (let walk ((relative "") (depth 0))
    (let* ((dir (if (string=? relative "") checkout-dir (path-join checkout-dir relative)))
           (manifest-attrs (file-attributes (path-join dir package-manifest-file)))
           (here (if (and manifest-attrs (eq? (car manifest-attrs) 'file))
                     (list relative)
                     '()))
           (children
            (if (= depth 4)
                '()
                (append-map
                  (lambda (entry)
                    (let ((name (if (string-suffix? "/" entry)
                                    (substring entry 0 (- (string-length entry) 1))
                                    entry)))
                      (if (or (string-prefix? "." name) (not (component? name)))
                          '()
                          (let ((attrs (file-attributes (path-join dir name))))
                            (if (and attrs (eq? (car attrs) 'directory))
                                (walk (if (string=? relative "") name (string-append relative "/" name))
                                      (+ depth 1))
                                '())))))
                  (directory-list dir)))))
      (append here children))))

(define (find-package-subdir checkout-dir wanted)
  (let ((found (repo-package-directories checkout-dir)))
    (when (null? found)
      (error "Repository holds no package: no techne-package.scm"))
    (let ((named (map (lambda (dir)
                        (let ((full (if (string=? dir "") checkout-dir (path-join checkout-dir dir))))
                          (verify-checkout-contained! checkout-dir full)
                          (cons (manifest-name (read-package-manifest full)) dir)))
                      found)))
      (cond (wanted
             (let ((matches (filter (lambda (entry) (equal? (car entry) wanted)) named)))
               (cond ((null? matches)
                      (error "Repository holds no package of this name" wanted (map car named)))
                     ((pair? (cdr matches))
                      (error "Repository holds duplicate packages of this name" wanted (map cdr matches)))
                     (else (cdar matches)))))
            ((and (pair? named) (null? (cdr named)))
             (cdar named))
            (else
             (error "Repository holds several packages: say which with (package (owner pkg))" (map car named)))))))

;; --- Resolver (one-version diamond-conflict check + root overrides) ---

(define-record-type resolved-entry
  (%resolved-entry name version source tree-sha256 depends chain canonical-dir staged-dir)
  resolved-entry?
  (name resolved-name)
  (version resolved-version)
  (source resolved-source)
  (tree-sha256 resolved-tree-sha256)
  (depends resolved-depends)
  (chain resolved-chain)
  (canonical-dir resolved-canonical-dir)
  (staged-dir resolved-staged-dir))

(define (format-source-brief src)
  (call-with-output-string (lambda (p) (write-locked-source src p))))

;; Require full consistency with `check-edge-source!` so no generated lockfile
;; can ever fail its own `--check` (e.g. same commit from different mirrors or
;; incompatible requested refs).
(define (entries-compatible? a b)
  (let ((sa (resolved-source a))
        (sb (resolved-source b)))
    (cond ((and (eq? 'path (car sa)) (eq? 'path (car sb)))
           (and (equal? (resolved-canonical-dir a) (resolved-canonical-dir b))
                (string=? (resolved-tree-sha256 a) (resolved-tree-sha256 b))))
          ((and (not (eq? 'path (car sa))) (not (eq? 'path (car sb))))
           (and (eq? (car sa) (car sb))
                (equal? (cadr sa) (cadr sb))
                (equal? (locked-source-commit sa) (locked-source-commit sb))
                (equal? (locked-source-ref sa) (locked-source-ref sb))
                (equal? (locked-source-subdir sa) (locked-source-subdir sb))
                (string=? (resolved-tree-sha256 a) (resolved-tree-sha256 b))))
          (else #f))))

(define (diamond-conflict-error name existing incoming)
  (error (string-append
           "Diamond dependency conflict for " (format-pkg-name name) ":\n"
           "  " (format-chain (resolved-chain existing))
           " resolved to " (format-source-brief (resolved-source existing))
           " [tree-sha256: " (resolved-tree-sha256 existing) "]\n"
           "  " (format-chain (resolved-chain incoming))
           " resolved to " (format-source-brief (resolved-source incoming))
           " [tree-sha256: " (resolved-tree-sha256 incoming) "]\n"
           "Use (override (" (format-pkg-name name) " ...)) in the root techne-package.scm to select one version.")))

;; Build a canonical locked source datum for a git checkout + optional ref/subdir.
(define (make-git-locked-source kind location oid ref sub)
  (append (list kind location (list 'commit oid))
          (if ref (list (list 'ref ref)) '())
          (if (and sub (positive? (string-length sub)))
              (list (list 'subdir sub))
              '())))

;; Stage a clean copy of PACKAGE-DIR under TMP-ROOT/staged/<hash> and return
;; `(cons hash staged-path)`.
(define (stage-clean-package! tmp-root package-dir)
  (let* ((staged-root (path-join tmp-root "staged"))
         (work (path-join staged-root (string-append "work-" (hex-encode (random-bytes 8))))))
    (make-directory staged-root)
    (create-directory work #o700)
    (copy-clean-tree! package-dir work)
    (let* ((hash (package-tree-sha256 work))
           (target (path-join staged-root hash)))
      (if (file-exists? target)
          (begin (delete-path work #t) (cons hash target))
          (begin (rename-file work target) (cons hash target))))))

(define (make-checkout-cache tmp-root)
  (let ((checkouts (make-hash-table equal?)))
    (lambda (src)
      (let ((key (list (car src) (cadr src) (source-git-ref src))))
        (or (hash-table-ref/default checkouts key #f)
            (let ((fetched (fetch-git-checkout! tmp-root src)))
              (hash-table-set! checkouts key fetched)
              fetched))))))

;; Stage SUB inside CHECKOUT-DIR, enforcing containment and Git symlink checks.
(define (materialize-git-subdir! tmp-root checkout-dir kind location oid ref sub)
  (let* ((checkout-true (file-truename checkout-dir))
         (has-sub (and sub (positive? (string-length sub))))
         (pkg-dir (if has-sub (path-join checkout-true sub) checkout-true))
         (protocol (if (eq? kind 'local) "file" "https")))
    (unless (file-exists? (path-join pkg-dir package-manifest-file))
      (error "No package where a dependency says" (if has-sub sub ".") pkg-dir))
    (verify-checkout-contained! checkout-true pkg-dir)
    (check-git-tree-symlinks! checkout-true protocol oid (and has-sub sub))
    (let* ((staged (stage-clean-package! tmp-root pkg-dir))
           (hash (car staged))
           (staged-dir (cdr staged))
           (norm-sub (and has-sub sub))
           (locked-src (make-git-locked-source kind location oid ref norm-sub)))
      (values (file-truename pkg-dir) locked-src
              (list 'git kind location oid ref checkout-true norm-sub)
              staged-dir hash))))

;; Materialize a direct Git source SRC for EXPECTED-NAME (or #f for sole package).
(define (materialize-git-source! tmp-root get-checkout expected-name src)
  (let* ((fetched (get-checkout src))
         (oid (car fetched))
         (checkout-dir (file-truename (cdr fetched)))
         (ref (source-git-ref src))
         (sel (or (source-git-package src) expected-name))
         (sub (find-package-subdir checkout-dir sel)))
    (materialize-git-subdir! tmp-root checkout-dir (car src) (cadr src) oid ref
                             (if (string=? sub "") #f sub))))

;; Materialize an intra-repository `(path REL)` dependency inside a Git CONTEXT
;; `(git kind location oid ref checkout-dir sub)`. Lazily fetches pinned OID if
;; CONTEXT came from an existing store entry without a checkout directory.
(define (materialize-intra-repo-path! tmp-root get-checkout dep-name rel context)
  (let* ((kind (list-ref context 1))
         (location (list-ref context 2))
         (oid (list-ref context 3))
         (ref (list-ref context 4))
         (existing-checkout (list-ref context 5))
         (parent-sub (list-ref context 6))
         (target-sub (resolve-subdir-path parent-sub rel))
         (checkout-dir (or existing-checkout
                           (let ((fetched (get-checkout (list kind location (list 'commit oid)))))
                             (unless (string=? (car fetched) oid)
                               (error (string-append "Pinned commit mismatch while fetching " (format-pkg-name dep-name)
                                                     ": expected " oid ", got " (car fetched))))
                             (file-truename (cdr fetched))))))
    (materialize-git-subdir! tmp-root checkout-dir kind location oid ref target-sub)))

;; Whether a declared Git source SRC matches a pinned LOCKED-SRC without moving
;; the pin (same kind, location, requested ref, and compatible commit if given).
(define (git-source-matches-pinned? declared-src locked-src)
  (and (not (eq? 'path (car declared-src)))
       (not (eq? 'path (car locked-src)))
       (eq? (car declared-src) (car locked-src))
       (equal? (cadr declared-src) (cadr locked-src))
       (equal? (source-git-ref declared-src) (locked-source-ref locked-src))
       (let ((ref (source-git-ref declared-src)))
         (or (not (and ref (eq? 'commit (car ref))))
             (commit-matches? (cadr ref) (locked-source-commit locked-src))))))

;; Hydrate a single pinned Git LOCKED-PKG into STORE (or stage in TMP-ROOT if
;; not yet in STORE), asserting the computed `tree-sha256` equals the locked hash.
;; Returns `(values pkg-dir staged-dir checkout-dir)` where STAGED-DIR is #f if
;; already verified in STORE.
(define (hydrate-pinned-git-package! tmp-root get-checkout store locked-pkg)
  (let* ((name (locked-package-name locked-pkg))
         (src (locked-package-source locked-pkg))
         (expected-hash (locked-package-tree-sha256 locked-pkg))
         (dest (path-join store expected-hash)))
    (if (file-exists? dest)
        (let ((actual (package-tree-sha256 dest)))
          (unless (string=? actual expected-hash)
            (error (string-append "Corrupted store entry for " (format-pkg-name name)
                                  " at " dest ": expected " expected-hash ", got " actual)))
          (values dest #f #f))
        (let* ((kind (car src))
               (location (cadr src))
               (oid (locked-source-commit src))
               (ref (locked-source-ref src))
               (sub (locked-source-subdir src))
               (fetched (get-checkout (list kind location (list 'commit oid))))
               (actual-oid (car fetched))
               (checkout-dir (file-truename (cdr fetched))))
          (unless (string=? actual-oid oid)
            (error (string-append "Pinned commit mismatch for " (format-pkg-name name)
                                  ": lockfile has " oid ", Git returned " actual-oid)))
          (let-values (((pkg-dir _locked-src _next-ctx staged-dir actual-hash)
                        (materialize-git-subdir! tmp-root checkout-dir kind location oid ref sub)))
            (unless (string=? actual-hash expected-hash)
              (error (string-append "tree-sha256 mismatch while materializing " (format-pkg-name name)
                                    ": lockfile has " expected-hash ", pinned commit " oid
                                    " produced " actual-hash)))
            (values pkg-dir staged-dir checkout-dir))))))

(define (resolve-project directory #:store [store (package-store-dir)] #:existing-lock [existing-lock #f])
  "Resolve all dependencies of the root package in DIRECTORY.
Applies root `(override ...)` entries, fetches and stages Git
dependencies, enforces one version per canonical `(owner pkg)` (failing
loudly on diamond conflicts naming both full dependency chains), detects
cycles, publishes materialized trees into STORE, and returns a lockfile.
When EXISTING-LOCK is provided, preserves existing valid pins for
unchanged Git dependency edges while resolving newly added or changed
edges."
  (let* ((root-manifest (read-package-manifest directory))
         (root-name (manifest-name root-manifest))
         (root-dir (manifest-directory root-manifest))
         (overrides (manifest-overrides root-manifest))
         (techne-ver (or (manifest-techne root-manifest) "0.4")))
    (make-directory store)
    (let ((tmp-root (path-join store (string-append ".resolve-" (hex-encode (random-bytes 12))))))
      (create-directory tmp-root #o700)
      (dynamic-wind
        (lambda () #f)
        (lambda ()
          (let ((get-checkout (make-checkout-cache tmp-root))
                (resolved (make-hash-table equal?))
                (existing-map (make-hash-table equal?)))
            (when existing-lock
              (for-each (lambda (p) (hash-table-set! existing-map (locked-package-name p) p))
                        (lockfile-packages existing-lock)))

            (define (materialize-edge dep-name src context)
              (if (eq? 'path (car src))
                  (let ((rel (cadr src)))
                    (if (eq? 'root (car context))
                        (let* ((parent-dir (cdr context))
                               (target (path-join parent-dir rel)))
                          (unless (file-exists? (path-join target package-manifest-file))
                            (error "No package where a dependency says" dep-name target))
                          (let* ((canon (file-truename target))
                                 (hash (package-tree-sha256 canon))
                                 (rel-to-root (path-relative-to root-dir canon)))
                            (values canon (list 'path rel-to-root) (cons 'root canon) #f hash)))
                        ;; Intra-repository `(path ...)` dependency inside a Git checkout.
                        (let* ((kind (list-ref context 1))
                               (location (list-ref context 2))
                               (oid (list-ref context 3))
                               (ref (list-ref context 4))
                               (parent-sub (list-ref context 6))
                               (target-sub (resolve-subdir-path parent-sub rel))
                               (pinned (hash-table-ref/default existing-map dep-name #f)))
                          (if (and pinned
                                   (let ((psrc (locked-package-source pinned)))
                                     (and (eq? (car psrc) kind)
                                          (equal? (cadr psrc) location)
                                          (equal? (locked-source-commit psrc) oid)
                                          (equal? (locked-source-ref psrc) ref)
                                          (equal? (locked-source-subdir psrc) target-sub))))
                              (let-values (((pkg-dir staged-dir checkout-dir)
                                            (hydrate-pinned-git-package! tmp-root get-checkout store pinned)))
                                (values pkg-dir (locked-package-source pinned)
                                        (list 'git kind location oid ref checkout-dir target-sub)
                                        staged-dir (locked-package-tree-sha256 pinned)))
                              (materialize-intra-repo-path! tmp-root get-checkout dep-name rel context)))))
                  ;; Git source: github, git, or local. Preserve existing pin if unchanged.
                  (let ((pinned (hash-table-ref/default existing-map dep-name #f)))
                    (if (and pinned (git-source-matches-pinned? src (locked-package-source pinned)))
                        (let* ((psrc (locked-package-source pinned)))
                          (let-values (((pkg-dir staged-dir checkout-dir)
                                        (hydrate-pinned-git-package! tmp-root get-checkout store pinned)))
                            (values pkg-dir psrc
                                    (list 'git (car psrc) (cadr psrc) (locked-source-commit psrc)
                                          (locked-source-ref psrc) checkout-dir (locked-source-subdir psrc))
                                    staged-dir (locked-package-tree-sha256 pinned))))
                        (materialize-git-source! tmp-root get-checkout dep-name src)))))

            (define (resolve-edge dep-name raw-src context visiting allow-local?)
              (let* ((chain (reverse (cons dep-name visiting))))
                (when (member dep-name visiting)
                  (error (string-append "Dependency cycle detected: " (format-chain chain))))
                (let* ((override-hit (assoc dep-name overrides))
                       (src (if override-hit (cdr override-hit) raw-src))
                       (eff-context (if override-hit (cons 'root root-dir) context))
                       (local-ok? (or allow-local? (and override-hit #t))))
                  (when (and (eq? 'local (car src)) (not local-ok?))
                    (error (string-append "Non-root manifest cannot use (local ...) source for "
                                          (format-pkg-name dep-name) " (" (format-chain chain) ")")))
                  (let-values (((pkg-dir locked-src next-context staged-dir hash)
                                (materialize-edge dep-name src eff-context)))
                    (let* ((dep-manifest (read-package-manifest pkg-dir))
                           (actual-name (manifest-name dep-manifest)))
                      (unless (equal? actual-name dep-name)
                        (error (string-append "Dependency " (format-pkg-name dep-name)
                                              " resolved to package " (format-pkg-name actual-name)
                                              " in " pkg-dir)))
                      (let* ((version (or (manifest-version dep-manifest) "0.0.0"))
                             (dep-names (sort (map car (manifest-dependency-sources dep-manifest)) package-name<?))
                             (entry (%resolved-entry dep-name version locked-src hash dep-names chain
                                                     (if (eq? 'path (car locked-src)) (file-truename pkg-dir) #f)
                                                     staged-dir))
                             (existing (hash-table-ref/default resolved dep-name #f)))
                        (cond (existing
                               (unless (entries-compatible? existing entry)
                                 (diamond-conflict-error dep-name existing entry)))
                              (else
                               (hash-table-set! resolved dep-name entry)
                               ;; Walk transitive dependencies (root overrides remain active;
                               ;; non-root manifest overrides and (local ...) sources are rejected).
                               (for-each
                                 (lambda (sub-dep)
                                   (resolve-edge (car sub-dep) (cdr sub-dep) next-context
                                                 (cons dep-name visiting) #f))
                                 (manifest-dependency-sources dep-manifest))))))))))

            ;; Walk all direct dependencies of root.
            (for-each
              (lambda (dep)
                (resolve-edge (car dep) (cdr dep) (cons 'root root-dir) (list root-name) #t))
              (manifest-dependency-sources root-manifest))

            ;; Publish all staged trees atomically into STORE/<tree-sha256>/ now that
            ;; the entire graph resolved cleanly without conflicts or cycles.
            (let ((entries (hash-table-values resolved)))
              (for-each
                (lambda (e)
                  (when (resolved-staged-dir e)
                    (publish-staged-tree! (resolved-staged-dir e) (resolved-tree-sha256 e) store)))
                entries)
              (make-lockfile
                techne-ver
                (map (lambda (e)
                       (make-locked-package (resolved-name e) (resolved-version e)
                                            (resolved-source e) (resolved-tree-sha256 e)
                                            (resolved-depends e)))
                     entries)))))
        (lambda ()
          (when (file-exists? tmp-root)
            (delete-path tmp-root #t)))))))

(define (resolve-package-source source-datum #:wanted [wanted #f] #:store [store (package-store-dir)]
                                #:existing-lock [existing-lock #f] #:publish [publish #t]
                                #:stage-root [stage-root #f] #:wrap-publish [wrap-publish #f])
  "Resolve a standalone Git package SOURCE-DATUM and its dependencies.
WANTED optionally selects a package from a multi-package repository.
EXISTING-LOCK, if provided, is checked for diamond conflicts against
already locked packages. When PUBLISH is #f, staged directories are not
published into STORE. STAGE-ROOT overrides the parent directory for
temporary staging. WRAP-PUBLISH, if given, is called as
`(WRAP-PUBLISH root-name staged-manifests merged-lock publish!)` so
callers can serialize validation and store writes. Returns
`(values root-name merged-lockfile new-entries)` where NEW-ENTRIES is a
list of `(cons tree-sha256 store-dir)`."
  (unless (and (package-source-datum? source-datum) (not (eq? 'path (car source-datum))))
    (error "Expected a Git package source" source-datum))
  (let ((staging-parent (or stage-root store))
        (local-root? (eq? 'local (car source-datum))))
    (make-directory staging-parent)
    (let ((tmp-root (path-join staging-parent (string-append ".resolve-" (hex-encode (random-bytes 12))))))
      (create-directory tmp-root #o700)
      (dynamic-wind
        (lambda () #f)
        (lambda ()
          (let ((get-checkout (make-checkout-cache tmp-root))
                (resolved (make-hash-table equal?))
                (existing-map (make-hash-table equal?)))
            (when existing-lock
              (for-each (lambda (p)
                          (hash-table-set! existing-map (locked-package-name p)
                                           (%resolved-entry (locked-package-name p)
                                                            (locked-package-version p)
                                                            (locked-package-source p)
                                                            (locked-package-tree-sha256 p)
                                                            (locked-package-depends p)
                                                            (list (locked-package-name p))
                                                            #f #f)))
                        (lockfile-packages existing-lock)))

            (define (check-against-existing! name entry)
              (let ((old (hash-table-ref/default existing-map name #f)))
                (when (and old (not (entries-compatible? old entry)))
                  (diamond-conflict-error name old entry))))

            (define (resolve-node expected-name src context visiting allow-local?)
              (when (and (eq? 'local (car src)) (not allow-local?))
                (error (string-append "Non-root manifest cannot use (local ...) source for "
                                      (if expected-name (format-pkg-name expected-name) "package"))))
              (let-values (((pkg-dir locked-src next-context staged-dir hash)
                            (if (eq? 'path (car src))
                                (materialize-intra-repo-path! tmp-root get-checkout expected-name (cadr src) context)
                                (materialize-git-source! tmp-root get-checkout expected-name src))))
                (let* ((m (read-package-manifest pkg-dir))
                       (actual-name (manifest-name m)))
                  (when (and expected-name (not (equal? actual-name expected-name)))
                    (error (string-append "Dependency " (format-pkg-name expected-name)
                                          " resolved to package " (format-pkg-name actual-name))))
                  (let ((chain (reverse (cons actual-name visiting))))
                    (when (member actual-name visiting)
                      (error (string-append "Dependency cycle detected: " (format-chain chain))))
                    (let* ((version (or (manifest-version m) "0.0.0"))
                           (dep-names (sort (map car (manifest-dependency-sources m)) package-name<?))
                           (entry (%resolved-entry actual-name version locked-src hash dep-names chain #f staged-dir))
                           (seen (hash-table-ref/default resolved actual-name #f)))
                      (check-against-existing! actual-name entry)
                      (cond (seen
                             (unless (entries-compatible? seen entry)
                               (diamond-conflict-error actual-name seen entry)))
                            (else
                             (hash-table-set! resolved actual-name entry)
                             (let ((child-local-ok? (and local-root? (null? visiting))))
                               (for-each
                                 (lambda (dep)
                                   (resolve-node (car dep) (cdr dep) next-context
                                                 (cons actual-name visiting) child-local-ok?))
                                 (manifest-dependency-sources m)))))
                      actual-name)))))

            (let* ((root-name (resolve-node wanted source-datum #f '() #t))
                   (new-entries (hash-table-values resolved))
                   (staged-manifests (map (lambda (e) (cons (resolved-name e) (read-package-manifest (resolved-staged-dir e))))
                                          new-entries))
                   (merged (make-hash-table equal?)))
              (when existing-lock
                (for-each (lambda (p) (hash-table-set! merged (locked-package-name p) p))
                          (lockfile-packages existing-lock)))
              (for-each (lambda (e)
                          (hash-table-set! merged (resolved-name e)
                                           (make-locked-package (resolved-name e) (resolved-version e)
                                                                (resolved-source e) (resolved-tree-sha256 e)
                                                                (resolved-depends e))))
                        new-entries)
              (let* ((merged-lock (make-lockfile (if existing-lock (lockfile-techne existing-lock) "0.4")
                                                 (hash-table-values merged)))
                     (do-publish!
                      (lambda ()
                        (let ((published '()))
                          (when publish
                            (for-each
                              (lambda (e)
                                (let* ((hash (resolved-tree-sha256 e))
                                       (existed (file-exists? (path-join store hash)))
                                       (dest (publish-staged-tree! (resolved-staged-dir e) hash store)))
                                  (unless existed
                                    (set! published (cons (cons hash dest) published)))))
                              new-entries))
                          published)))
                     (published (if wrap-publish
                                    (wrap-publish root-name staged-manifests merged-lock do-publish!)
                                    (do-publish!))))
                (values root-name merged-lock published)))))
        (lambda ()
          (when (file-exists? tmp-root)
            (delete-path tmp-root #t)))))))

;; --- Strictly offline verification, fetch-by-lock & VM wiring ---

(define (locked-package-directory pkg root-dir store)
  (let ((src (locked-package-source pkg)))
    (if (eq? 'path (car src))
        (path-join root-dir (cadr src))
        (path-join store (locked-package-tree-sha256 pkg)))))

(define (verify-package-entry pkg root-dir store)
  (let* ((name (locked-package-name pkg))
         (src (locked-package-source pkg))
         (expected-hash (locked-package-tree-sha256 pkg))
         (dir (locked-package-directory pkg root-dir store)))
    (unless (file-exists? dir)
      (error (string-append "Locked package " (format-pkg-name name)
                            " is not materialized at " dir
                            (if (eq? 'path (car src))
                                " (missing local path)"
                                " (run `techne lock --frozen`)"))))
    (let ((actual-hash (package-tree-sha256 dir)))
      (unless (string=? actual-hash expected-hash)
        (error (string-append "tree-sha256 mismatch for " (format-pkg-name name)
                              " at " dir ": lockfile has " expected-hash ", directory has " actual-hash))))
    (let* ((m (read-package-manifest dir))
           (actual-name (manifest-name m))
           (actual-ver (or (manifest-version m) "0.0.0")))
      (unless (equal? actual-name name)
        (error (string-append "Manifest name mismatch for " (format-pkg-name name)
                              ": directory holds " (format-pkg-name actual-name))))
      (unless (string=? actual-ver (locked-package-version pkg))
        (error (string-append "Manifest version mismatch for " (format-pkg-name name)
                              ": lockfile has " (locked-package-version pkg) ", manifest has " actual-ver)))
      (let ((actual-deps (sort (map car (manifest-dependency-sources m)) package-name<?))
            (locked-deps (sort (locked-package-depends pkg) package-name<?)))
        (unless (equal? actual-deps locked-deps)
          (error (string-append "techne.lock is stale: dependencies of " (format-pkg-name name)
                                " do not match lockfile"))))
      m)))

;; Check that an edge's declared source (after applying root overrides) is
;; compatible with LOCKED-PKG's pinned source.
(define (check-edge-source! dep-name declared-src parent-locked parent-dir root-dir locked-pkg)
  (let ((locked-src (locked-package-source locked-pkg)))
    (if (eq? 'path (car declared-src))
        (let ((rel (cadr declared-src)))
          (if (or (not parent-locked) (eq? 'path (car (locked-package-source parent-locked))))
              (begin
                (unless (eq? 'path (car locked-src))
                  (error (string-append "techne.lock is stale: " (format-pkg-name dep-name)
                                        " source changed to local path " rel)))
                (let ((expected-path (path-join parent-dir rel))
                      (locked-path (path-join root-dir (cadr locked-src))))
                  (unless (and (file-exists? expected-path) (file-exists? locked-path)
                               (equal? (file-truename expected-path) (file-truename locked-path)))
                    (error (string-append "techne.lock is stale: " (format-pkg-name dep-name)
                                          " path changed from " (cadr locked-src) " to " rel)))))
              ;; Intra-repo path dependency of a locked git package.
              (let* ((psrc (locked-package-source parent-locked))
                     (expected-sub (resolve-subdir-path (locked-source-subdir psrc) rel)))
                (unless (and (eq? (car locked-src) (car psrc))
                             (equal? (cadr locked-src) (cadr psrc))
                             (equal? (locked-source-commit locked-src) (locked-source-commit psrc))
                             (equal? (locked-source-ref locked-src) (locked-source-ref psrc))
                             (equal? (locked-source-subdir locked-src) expected-sub))
                  (error (string-append "techne.lock is stale: intra-repository dependency "
                                        (format-pkg-name dep-name) " does not match parent checkout"))))))
        ;; Git source: github, git, or local.
        (begin
          (unless (and (eq? (car locked-src) (car declared-src))
                       (equal? (cadr locked-src) (cadr declared-src)))
            (error (string-append "techne.lock is stale: " (format-pkg-name dep-name)
                                  " source does not match techne-package.scm")))
          (let ((declared-ref (source-git-ref declared-src))
                (locked-ref (locked-source-ref locked-src)))
            (unless (equal? declared-ref locked-ref)
              (error (string-append "techne.lock is stale: " (format-pkg-name dep-name)
                                    " requested ref changed in techne-package.scm")))
            (when (and declared-ref (eq? 'commit (car declared-ref))
                       (not (commit-matches? (cadr declared-ref) (locked-source-commit locked-src))))
              (error (string-append "techne.lock is stale: " (format-pkg-name dep-name)
                                    " commit changed in techne-package.scm"))))))))

;; Preflight root manifest version, root edges, and lockfile reachability BEFORE
;; hashing or fetching any store entries.
(define (preflight-lockfile! root-manifest lf)
  (let* ((root-name (manifest-name root-manifest))
         (root-dir (manifest-directory root-manifest))
         (overrides (manifest-overrides root-manifest))
         (expected-techne (or (manifest-techne root-manifest) "0.4"))
         (locked-map (make-hash-table equal?))
         (reachable (make-hash-table equal?)))
    (unless (string=? (lockfile-techne lf) expected-techne)
      (error (string-append "techne.lock is stale: techne version changed from "
                            (lockfile-techne lf) " to " expected-techne)))
    (for-each (lambda (p) (hash-table-set! locked-map (locked-package-name p) p))
              (lockfile-packages lf))
    (define (mark-reachable! name visiting)
      (when (member name visiting)
        (error (string-append "Dependency cycle detected: " (format-chain (reverse (cons name visiting))))))
      (unless (hash-table-ref/default reachable name #f)
        (let ((p (hash-table-ref/default locked-map name #f)))
          (unless p
            (error (string-append "techne.lock is stale: missing dependency " (format-pkg-name name))))
          (hash-table-set! reachable name #t)
          (for-each (lambda (d) (mark-reachable! d (cons name visiting)))
                    (locked-package-depends p)))))
    ;; Verify every direct root edge and mark transitive reachability from lockfile.
    (for-each
      (lambda (dep)
        (let* ((dep-name (car dep))
               (override-hit (assoc dep-name overrides))
               (eff-src (if override-hit (cdr override-hit) (cdr dep)))
               (locked-pkg (hash-table-ref/default locked-map dep-name #f)))
          (unless locked-pkg
            (error (string-append "techne.lock is stale: missing dependency " (format-pkg-name dep-name))))
          (check-edge-source! dep-name eff-src #f root-dir root-dir locked-pkg)
          (mark-reachable! dep-name (list root-name))))
      (manifest-dependency-sources root-manifest))
    ;; Reject stale leftover entries before hashing/fetching anything.
    (for-each
      (lambda (p)
        (unless (hash-table-ref/default reachable (locked-package-name p) #f)
          (error (string-append "techne.lock is stale: unused locked package "
                                (format-pkg-name (locked-package-name p))))))
      (lockfile-packages lf))
    locked-map))

(define (materialize-lockfile! directory #:store [store (package-store-dir)])
  "Materialize missing STORE entries from DIRECTORY's `techne.lock`.
Walks the dependency graph top-down from `techne-package.scm` and root
overrides, authorizing every edge (including rejecting any non-root
`(local ...)` source) before fetching its pinned Git commit into STORE.
Verifies each `tree-sha256` matches the lockfile, checks freshness against
`techne-package.scm`, and never modifies `techne.lock`."
  (let* ((root-manifest (read-package-manifest directory))
         (root-name (manifest-name root-manifest))
         (root-dir (manifest-directory root-manifest))
         (overrides (manifest-overrides root-manifest))
         (lock-path (path-join root-dir lockfile-name))
         (lf (read-lockfile lock-path))
         (locked-map (preflight-lockfile! root-manifest lf)))
    (make-directory store)
    (let ((tmp-root (path-join store (string-append ".hydrate-" (hex-encode (random-bytes 12))))))
      (create-directory tmp-root #o700)
      (dynamic-wind
        (lambda () #f)
        (lambda ()
          (let ((get-checkout (make-checkout-cache tmp-root))
                (hydrated (make-hash-table equal?)))
            (define (hydrate-edges! parent-locked parent-dir dep-sources visiting allow-local?)
              (for-each
                (lambda (dep)
                  (let* ((dep-name (car dep))
                         (override-hit (assoc dep-name overrides))
                         (eff-src (if override-hit (cdr override-hit) (cdr dep)))
                         (eff-parent-locked (if override-hit #f parent-locked))
                         (eff-parent-dir (if override-hit root-dir parent-dir))
                         (local-ok? (or allow-local? (and override-hit #t)))
                         (chain (reverse (cons dep-name visiting))))
                    (when (member dep-name visiting)
                      (error (string-append "Dependency cycle detected: " (format-chain chain))))
                    (when (and (eq? 'local (car eff-src)) (not local-ok?))
                      (error (string-append "Non-root manifest cannot use (local ...) source for "
                                            (format-pkg-name dep-name) " (" (format-chain chain) ")")))
                    (let ((locked-pkg (hash-table-ref/default locked-map dep-name #f)))
                      (unless locked-pkg
                        (error (string-append "techne.lock is stale: missing dependency " (format-pkg-name dep-name))))
                      (check-edge-source! dep-name eff-src eff-parent-locked eff-parent-dir root-dir locked-pkg)
                      (unless (hash-table-ref/default hydrated dep-name #f)
                        (unless (eq? 'path (car (locked-package-source locked-pkg)))
                          (let-values (((_pkg-dir staged-dir _checkout-dir)
                                        (hydrate-pinned-git-package! tmp-root get-checkout store locked-pkg)))
                            (when staged-dir
                              (publish-staged-tree! staged-dir (locked-package-tree-sha256 locked-pkg) store))))
                        (let ((dep-manifest (verify-package-entry locked-pkg root-dir store)))
                          (hash-table-set! hydrated dep-name dep-manifest)
                          (hydrate-edges! locked-pkg (manifest-directory dep-manifest)
                                          (manifest-dependency-sources dep-manifest)
                                          (cons dep-name visiting) #f))))))
                dep-sources))
            (hydrate-edges! #f root-dir (manifest-dependency-sources root-manifest) (list root-name) #t)))
        (lambda ()
          (when (file-exists? tmp-root)
            (delete-path tmp-root #t)))))
    (check-lockfile root-dir #:store store)))

(define (lock-project! directory #:store [store (package-store-dir)]
                       #:update [update #f] #:frozen [frozen #f])
  "Resolve DIRECTORY's manifest into STORE and write `techne.lock`.
When FROZEN is `#t`, materializes missing STORE entries from the existing
`techne.lock` without modifying it. Otherwise, preserves existing valid
pins unless UPDATE is `#t`, resolves any newly added or changed edges,
and writes `techne.lock` beside the manifest."
  (when (and update frozen)
    (error "lock-project!: #:update and #:frozen cannot be used together"))
  (let* ((root-dir (manifest-directory (read-package-manifest directory)))
         (lock-path (path-join root-dir lockfile-name)))
    (if frozen
        (materialize-lockfile! root-dir #:store store)
        (let* ((existing (and (not update) (file-exists? lock-path) (read-lockfile lock-path)))
               (lf (resolve-project root-dir #:store store #:existing-lock existing)))
          (write-lockfile lock-path lf)))))

(define (verify-lockfile directory #:store [store (package-store-dir)])
  "Verify DIRECTORY's `techne.lock` strictly offline without running Git.
Checks that `techne.lock` exists, matches `techne-package.scm` and root
overrides, covers the exact reachable dependency graph without cycles,
and that every locked package's `tree-sha256` matches its directory on
disk or in STORE. Returns `(values lockfile root-manifest manifests)`."
  (let* ((root-manifest (read-package-manifest directory))
         (root-name (manifest-name root-manifest))
         (root-dir (manifest-directory root-manifest))
         (overrides (manifest-overrides root-manifest))
         (lock-path (path-join root-dir lockfile-name))
         (lf (read-lockfile lock-path))
         ;; Phase 1: check techne version, root edges, and graph reachability
         ;; BEFORE hashing any store or disk directories.
         (locked-map (preflight-lockfile! root-manifest lf))
         (manifests-map (make-hash-table equal?))
         (reachable (make-hash-table equal?)))
    ;; Phase 2: verify every reachable locked package exists and matches its
    ;; tree-sha256 and manifest metadata.
    (for-each (lambda (p)
                (hash-table-set! manifests-map (locked-package-name p)
                                 (verify-package-entry p root-dir store)))
              (lockfile-packages lf))
    ;; Phase 3: walk every edge of the verified manifests offline, rejecting
    ;; non-root `(local ...)` sources and checking every transitive edge.
    (define (walk-edges parent-locked parent-dir dep-sources visiting allow-local?)
      (for-each
        (lambda (dep)
          (let* ((dep-name (car dep))
                 (override-hit (assoc dep-name overrides))
                 (eff-src (if override-hit (cdr override-hit) (cdr dep)))
                 (eff-parent-locked (if override-hit #f parent-locked))
                 (eff-parent-dir (if override-hit root-dir parent-dir))
                 (local-ok? (or allow-local? (and override-hit #t)))
                 (chain (reverse (cons dep-name visiting))))
            (when (member dep-name visiting)
              (error (string-append "Dependency cycle detected: " (format-chain chain))))
            (when (and (eq? 'local (car eff-src)) (not local-ok?))
              (error (string-append "Non-root manifest cannot use (local ...) source for "
                                    (format-pkg-name dep-name) " (" (format-chain chain) ")")))
            (let ((locked-pkg (hash-table-ref/default locked-map dep-name #f)))
              (unless locked-pkg
                (error (string-append "techne.lock is stale: missing dependency " (format-pkg-name dep-name))))
              (check-edge-source! dep-name eff-src eff-parent-locked eff-parent-dir root-dir locked-pkg)
              (unless (hash-table-ref/default reachable dep-name #f)
                (hash-table-set! reachable dep-name #t)
                (let ((dep-manifest (hash-table-ref/default manifests-map dep-name #f)))
                  (walk-edges locked-pkg (manifest-directory dep-manifest)
                              (manifest-dependency-sources dep-manifest)
                              (cons dep-name visiting) #f))))))
        dep-sources))
    (walk-edges #f root-dir (manifest-dependency-sources root-manifest) (list root-name) #t)
    (for-each
      (lambda (p)
        (unless (hash-table-ref/default reachable (locked-package-name p) #f)
          (error (string-append "techne.lock is stale: unused locked package "
                                (format-pkg-name (locked-package-name p))))))
      (lockfile-packages lf))
    (values lf root-manifest
            (cons root-manifest
                  (map (lambda (p) (hash-table-ref/default manifests-map (locked-package-name p) #f))
                       (lockfile-packages lf))))))

(define (check-lockfile directory #:store [store (package-store-dir)])
  "Verify DIRECTORY's `techne.lock` against `techne-package.scm` and STORE.
Fails with an error if the lockfile is missing, stale, or any package's
`tree-sha256` does not match. Never modifies `techne.lock`."
  (call-with-values (lambda () (verify-lockfile directory #:store store))
    (lambda (lf _root _manifests) lf)))

(define (use-locked-package! directory #:store [store (package-store-dir)])
  "Verify DIRECTORY's `techne.lock` offline and register its packages.
Wires every `(owner pkg)` library provider and declared direct import
boundary from the lockfile and STORE without touching Git or the network.
Returns the root package manifest."
  (call-with-values (lambda () (verify-lockfile directory #:store store))
    (lambda (_lf root-manifest manifests)
      (register-package-world! manifests)
      root-manifest)))

(define (verify-world-closure lf root-names #:store [store (package-store-dir)]
                              #:exact [exact #f] #:root-recipes [root-recipes #f])
  "Verify ROOT-NAMES's transitive closure in LF against STORE.
Checks each reachable entry's `tree-sha256` and manifest metadata, rejecting
local path entries. When EXACT is `#t`, also fails if LF contains any
package outside the transitive closure of ROOT-NAMES. When ROOT-RECIPES is
given as an alist of `((owner pkg) . source)`, verifies each root recipe
and transitive dependency edge against LF. Returns an alist of
`((owner pkg) . manifest)` for the verified closure."
  (let ((by-name (make-hash-table equal?))
        (seen (make-hash-table equal?))
        (order '()))
    (for-each (lambda (p) (hash-table-set! by-name (locked-package-name p) p))
              (lockfile-packages lf))
    (define (visit name)
      (unless (hash-table-ref/default seen name #f)
        (let ((p (or (hash-table-ref/default by-name name #f)
                     (error "Missing package in world.lock" name))))
          (hash-table-set! seen name #t)
          (for-each visit (locked-package-depends p))
          (set! order (cons p order)))))
    (for-each visit root-names)
    (when exact
      (for-each
        (lambda (p)
          (unless (hash-table-ref/default seen (locked-package-name p) #f)
            (error (string-append "world.lock contains unrelated package not reachable from installed roots: "
                                  (format-pkg-name (locked-package-name p))))))
        (lockfile-packages lf)))
    (when root-recipes
      (for-each
        (lambda (r)
          (let ((p (or (hash-table-ref/default by-name (car r) #f)
                       (error "Missing package in world.lock" (car r)))))
            (check-edge-source! (car r) (cdr r) #f "" "" p)))
        root-recipes))
    (let ((verified
           (map (lambda (p)
                  (when (eq? 'path (car (locked-package-source p)))
                    (error "World lockfile cannot contain local path entries" (locked-package-name p)))
                  (cons (locked-package-name p) (verify-package-entry p "" store)))
                (reverse order))))
      (when root-recipes
        (for-each
          (lambda (pair)
            (let* ((pkg-name (car pair))
                   (m (cdr pair))
                   (parent-locked (hash-table-ref/default by-name pkg-name #f)))
              (for-each
                (lambda (dep)
                  (let ((child-locked (or (hash-table-ref/default by-name (car dep) #f)
                                          (error "Missing dependency in world.lock" (car dep)))))
                    (check-edge-source! (car dep) (cdr dep) parent-locked (manifest-directory m) "" child-locked)))
                (manifest-dependency-sources m))))
          verified))
      verified)))

(define (verify-world-lockfile lf #:store [store (package-store-dir)])
  "Verify every package in world lockfile LF against STORE.
Checks each entry's `tree-sha256` and returns an alist of
`((owner pkg) . manifest)` for all packages in LF."
  (map (lambda (p)
         (when (eq? 'path (car (locked-package-source p)))
           (error "World lockfile cannot contain local path entries" (locked-package-name p)))
         (cons (locked-package-name p) (verify-package-entry p "" store)))
       (lockfile-packages lf)))
