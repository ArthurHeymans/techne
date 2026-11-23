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

(define (component? s)
  (and (string? s) (positive? (string-length s)) (not (member s '("." "..")))
       (every (lambda (c) (or (char-alphabetic? c) (char-numeric? c) (memv c '(#\- #\_ #\.)))) (string->list s))))

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
                      (subdirs (filter (lambda (e) (and (pair? e) (eq? (car e) 'subdir))) rest)))
                 (and (= 1 (length commits))
                      (<= (length subdirs) 1)
                      (= (length rest) (+ (length commits) (length subdirs)))
                      (let ((c (car commits)))
                        (and (list? c) (= 2 (length c)) (oid? (cadr c))))
                      (every (lambda (sd) (and (list? sd) (= 2 (length sd)) (subdir-string? (cadr sd)))) subdirs)))))
         (else #f))))

(define (locked-source-kind src) (car src))
(define (locked-source-location src) (cadr src))
(define (locked-source-commit src)
  (let ((c (assq 'commit (cddr src)))) (and c (cadr c))))
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
    #:environment '(("GIT_TERMINAL_PROMPT" . "0"))))

(define (source-git-url src)
  (case (car src)
    ((github) (string-append "https://github.com/" (cadr src) ".git"))
    ((git) (cadr src))
    ((local) (cadr src))
    (else (error "Not a Git package source" src))))

(define (source-git-protocol src)
  (if (eq? (car src) 'local) "file" "https"))

(define (source-git-ref src)
  (let ((found (find (lambda (e) (and (pair? e) (memq (car e) '(branch tag commit)))) (cddr src))))
    found))

(define (source-git-package src)
  (let ((found (assq 'package (cddr src))))
    (and found (cadr found))))

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
           (guard (e ((and (eq? (car src) 'local) ref (eq? (car ref) 'commit))
                      ;; Fallback for older git when fetching a non-tip commit from a local repo.
                      (git-run dir protocol "fetch" "--quiet" "--no-tags" "--" url)
                      (git-run dir protocol "rev-parse" "--verify" (string-append (cadr ref) "^{commit}"))))
             (git-run dir protocol "fetch" "--quiet" "--depth=1" "--no-tags" "--" url refspec)
             (git-run dir protocol "rev-parse" "--verify" "FETCH_HEAD^{commit}"))))
      (unless (oid? oid)
        (error "Git returned an invalid commit" oid))
      (git-run dir protocol "checkout" "--quiet" "--detach" oid)
      (cons oid dir))))

(define (find-package-subdir checkout-dir wanted)
  (let ((found (filter (lambda (dir) (or (string=? dir "") (every component? (string-split dir "/"))))
                       (package-directories checkout-dir))))
    (cond ((null? found)
           (error "Repository holds no package: no techne-package.scm"))
          ((equal? found '(""))
           (when wanted
             (let ((actual (manifest-name (read-package-manifest checkout-dir))))
               (unless (equal? actual wanted)
                 (error "Repository holds no package of this name" wanted (list actual)))))
           "")
          (else
           (let* ((named (filter-map (lambda (dir)
                                       (guard (e (#t #f))
                                         (cons (manifest-name (read-package-manifest (path-join checkout-dir dir))) dir)))
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
                    (error "Repository holds several packages: say which with (package (owner pkg))" (map car named)))))))))

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

(define (entries-compatible? a b)
  (let ((sa (resolved-source a))
        (sb (resolved-source b)))
    (cond ((and (eq? 'path (car sa)) (eq? 'path (car sb)))
           (and (equal? (resolved-canonical-dir a) (resolved-canonical-dir b))
                (string=? (resolved-tree-sha256 a) (resolved-tree-sha256 b))))
          ((and (not (eq? 'path (car sa))) (not (eq? 'path (car sb))))
           (and (equal? (locked-source-commit sa) (locked-source-commit sb))
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

;; Build a locked source datum for a git checkout + optional subdir.
(define (make-git-locked-source kind location oid sub)
  (if (and sub (positive? (string-length sub)))
      (list kind location (list 'commit oid) (list 'subdir sub))
      (list kind location (list 'commit oid))))

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

(define (resolve-project directory #:store [store (package-store-dir)])
  "Resolve all dependencies of the root package in DIRECTORY.
Applies root `(override ...)` entries, fetches and stages Git
dependencies, enforces one version per canonical `(owner pkg)` (failing
loudly on diamond conflicts naming both full dependency chains), detects
cycles, publishes materialized trees into STORE, and returns a lockfile."
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
          (let ((checkouts (make-hash-table equal?))
                (resolved (make-hash-table equal?)))
            (define (get-checkout src)
              (let ((key (list (car src) (cadr src) (source-git-ref src))))
                (or (hash-table-ref/default checkouts key #f)
                    (let ((fetched (fetch-git-checkout! tmp-root src)))
                      (hash-table-set! checkouts key fetched)
                      fetched))))

            ;; Resolve dependency edge DEP-NAME with declared source RAW-SRC.
            ;; CONTEXT is either `(root . root-dir)` for root/path packages or
            ;; `(git kind location oid checkout-dir pkg-dir)` for packages inside
            ;; a git checkout.
            (define (resolve-edge dep-name raw-src context visiting)
              (let* ((chain (reverse (cons dep-name visiting))))
                (when (member dep-name visiting)
                  (error (string-append "Dependency cycle detected: " (format-chain chain))))
                (let* ((override-hit (assoc dep-name overrides))
                       (src (if override-hit (cdr override-hit) raw-src))
                       (eff-context (if override-hit (cons 'root root-dir) context)))
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
                               ;; non-root manifest overrides are ignored).
                               (for-each
                                 (lambda (sub-dep)
                                   (resolve-edge (car sub-dep) (cdr sub-dep) next-context (cons dep-name visiting)))
                                 (manifest-dependency-sources dep-manifest))))))))))

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
                        ;; Intra-repository (path ...) dependency inside a git checkout.
                        (let* ((kind (list-ref context 1))
                               (location (list-ref context 2))
                               (oid (list-ref context 3))
                               (checkout-dir (list-ref context 4))
                               (parent-dir (list-ref context 5))
                               (checkout-true (file-truename checkout-dir))
                               (target (path-join parent-dir rel)))
                          (unless (file-exists? (path-join target package-manifest-file))
                            (error "No package where a dependency says" dep-name target))
                          (let ((target-true (file-truename target)))
                            (unless (or (equal? target-true checkout-true)
                                        (string-prefix? (string-append checkout-true "/") target-true))
                              (error "A package depends on one outside its repository" dep-name rel))
                            (let* ((sub (if (equal? target-true checkout-true)
                                            ""
                                            (substring target-true (+ 1 (string-length checkout-true))
                                                       (string-length target-true))))
                                   (staged (stage-clean-package! tmp-root target-true))
                                   (hash (car staged))
                                   (staged-dir (cdr staged))
                                   (locked-src (make-git-locked-source kind location oid sub)))
                              (values target-true locked-src
                                      (list 'git kind location oid checkout-true target-true)
                                      staged-dir hash))))))
                  ;; Git source: github, git, or local.
                  (let* ((fetched (get-checkout src))
                         (oid (car fetched))
                         (checkout-dir (file-truename (cdr fetched)))
                         (wanted (source-git-package src))
                         (sub (find-package-subdir checkout-dir wanted))
                         (pkg-dir (if (string=? sub "") checkout-dir (path-join checkout-dir sub)))
                         (staged (stage-clean-package! tmp-root pkg-dir))
                         (hash (car staged))
                         (staged-dir (cdr staged))
                         (locked-src (make-git-locked-source (car src) (cadr src) oid sub)))
                    (values pkg-dir locked-src
                            (list 'git (car src) (cadr src) oid checkout-dir pkg-dir)
                            staged-dir hash))))

            ;; Walk all direct dependencies of root.
            (for-each
              (lambda (dep)
                (resolve-edge (car dep) (cdr dep) (cons 'root root-dir) (list root-name)))
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
  (let ((staging-parent (or stage-root store)))
    (make-directory staging-parent)
    (let ((tmp-root (path-join staging-parent (string-append ".resolve-" (hex-encode (random-bytes 12))))))
      (create-directory tmp-root #o700)
      (dynamic-wind
        (lambda () #f)
        (lambda ()
          (let ((checkouts (make-hash-table equal?))
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
            (define (get-checkout src)
              (let ((key (list (car src) (cadr src) (source-git-ref src))))
                (or (hash-table-ref/default checkouts key #f)
                    (let ((fetched (fetch-git-checkout! tmp-root src)))
                      (hash-table-set! checkouts key fetched)
                      fetched))))

            (define (check-against-existing! name entry)
              (let ((old (hash-table-ref/default existing-map name #f)))
                (when (and old (not (entries-compatible? old entry)))
                  (diamond-conflict-error name old entry))))

            (define (resolve-node expected-name src context visiting)
              (let* ((fetched-info
                      (if (eq? 'path (car src))
                          (let* ((rel (cadr src))
                                 (kind (list-ref context 1))
                                 (location (list-ref context 2))
                                 (oid (list-ref context 3))
                                 (checkout-dir (list-ref context 4))
                                 (parent-dir (list-ref context 5))
                                 (checkout-true (file-truename checkout-dir))
                                 (target (path-join parent-dir rel)))
                            (unless (file-exists? (path-join target package-manifest-file))
                              (error "No package where a dependency says" expected-name target))
                            (let ((target-true (file-truename target)))
                              (unless (or (equal? target-true checkout-true)
                                          (string-prefix? (string-append checkout-true "/") target-true))
                                (error "A package depends on one outside its repository" expected-name rel))
                              (let* ((sub (if (equal? target-true checkout-true)
                                              ""
                                              (substring target-true (+ 1 (string-length checkout-true))
                                                         (string-length target-true))))
                                     (staged (stage-clean-package! tmp-root target-true)))
                                (list target-true
                                      (make-git-locked-source kind location oid sub)
                                      (list 'git kind location oid checkout-true target-true)
                                      (car staged) (cdr staged)))))
                          (let* ((fetched (get-checkout src))
                                 (oid (car fetched))
                                 (checkout-dir (file-truename (cdr fetched)))
                                 (sel (or (source-git-package src) (and (null? visiting) expected-name)))
                                 (sub (find-package-subdir checkout-dir sel))
                                 (pkg-dir (if (string=? sub "") checkout-dir (path-join checkout-dir sub)))
                                 (staged (stage-clean-package! tmp-root pkg-dir)))
                            (list pkg-dir
                                  (make-git-locked-source (car src) (cadr src) oid sub)
                                  (list 'git (car src) (cadr src) oid checkout-dir pkg-dir)
                                  (car staged) (cdr staged)))))
                     (pkg-dir (list-ref fetched-info 0))
                     (locked-src (list-ref fetched-info 1))
                     (next-context (list-ref fetched-info 2))
                     (hash (list-ref fetched-info 3))
                     (staged-dir (list-ref fetched-info 4))
                     (m (read-package-manifest pkg-dir))
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
                           (for-each
                             (lambda (dep)
                               (resolve-node (car dep) (cdr dep) next-context (cons actual-name visiting)))
                             (manifest-dependency-sources m))))
                    actual-name))))

            (let* ((root-name (resolve-node wanted source-datum #f '()))
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

(define (lock-project! directory #:store [store (package-store-dir)])
  "Resolve DIRECTORY's manifest into STORE and write `techne.lock`.
Reads `techne-package.scm`, materializes Git dependencies into STORE,
and writes `techne.lock` beside the manifest."
  (let* ((root-dir (manifest-directory (read-package-manifest directory)))
         (lf (resolve-project root-dir #:store store)))
    (write-lockfile (path-join root-dir lockfile-name) lf)))

;; --- Strictly offline verification & VM wiring ---

(define (locked-package-directory pkg root-dir store)
  (let ((src (locked-package-source pkg)))
    (if (eq? 'path (car src))
        (path-join root-dir (cadr src))
        (path-join store (locked-package-tree-sha256 pkg)))))

(define (verify-package-entry pkg root-dir store)
  (let* ((name (locked-package-name pkg))
         (expected-hash (locked-package-tree-sha256 pkg))
         (dir (locked-package-directory pkg root-dir store)))
    (unless (file-exists? dir)
      (error (string-append "Locked package " (format-pkg-name name)
                            " is not materialized at " dir " (run `techne lock`)")))
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
                (let ((expected-dir (file-truename (path-join parent-dir rel)))
                      (locked-dir (file-truename (path-join root-dir (cadr locked-src)))))
                  (unless (equal? expected-dir locked-dir)
                    (error (string-append "techne.lock is stale: " (format-pkg-name dep-name)
                                          " path changed from " (cadr locked-src) " to " rel)))))
              ;; Intra-repo path dependency of a locked git package.
              (let* ((psrc (locked-package-source parent-locked))
                     (expected-sub (resolve-subdir-path (locked-source-subdir psrc) rel)))
                (unless (and (eq? (car locked-src) (car psrc))
                             (equal? (cadr locked-src) (cadr psrc))
                             (equal? (locked-source-commit locked-src) (locked-source-commit psrc))
                             (equal? (locked-source-subdir locked-src) expected-sub))
                  (error (string-append "techne.lock is stale: intra-repository dependency "
                                        (format-pkg-name dep-name) " does not match parent checkout"))))))
        ;; Git source: github, git, or local.
        (begin
          (unless (and (eq? (car locked-src) (car declared-src))
                       (equal? (cadr locked-src) (cadr declared-src)))
            (error (string-append "techne.lock is stale: " (format-pkg-name dep-name)
                                  " source does not match techne-package.scm")))
          (let ((ref (source-git-ref declared-src)))
            (when (and ref (eq? 'commit (car ref))
                       (not (equal? (cadr ref) (locked-source-commit locked-src))))
              (error (string-append "techne.lock is stale: " (format-pkg-name dep-name)
                                    " commit changed in techne-package.scm"))))))))

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
         (locked-map (make-hash-table equal?))
         (manifests-map (make-hash-table equal?))
         (reachable (make-hash-table equal?)))
    (for-each (lambda (p) (hash-table-set! locked-map (locked-package-name p) p))
              (lockfile-packages lf))
    ;; Verify every locked package exists on disk and matches its tree-sha256 & manifest.
    (for-each (lambda (p)
                (hash-table-set! manifests-map (locked-package-name p)
                                 (verify-package-entry p root-dir store)))
              (lockfile-packages lf))
    ;; Walk graph from root strictly offline, checking every edge and reachability.
    (define (walk-edges parent-locked parent-dir dep-sources visiting)
      (for-each
        (lambda (dep)
          (let* ((dep-name (car dep))
                 (override-hit (assoc dep-name overrides))
                 (eff-src (if override-hit (cdr override-hit) (cdr dep)))
                 (eff-parent-locked (if override-hit #f parent-locked))
                 (eff-parent-dir (if override-hit root-dir parent-dir))
                 (chain (reverse (cons dep-name visiting))))
            (when (member dep-name visiting)
              (error (string-append "Dependency cycle detected: " (format-chain chain))))
            (let ((locked-pkg (hash-table-ref/default locked-map dep-name #f)))
              (unless locked-pkg
                (error (string-append "techne.lock is stale: missing dependency " (format-pkg-name dep-name))))
              (check-edge-source! dep-name eff-src eff-parent-locked eff-parent-dir root-dir locked-pkg)
              (unless (hash-table-ref/default reachable dep-name #f)
                (hash-table-set! reachable dep-name #t)
                (let ((dep-manifest (hash-table-ref/default manifests-map dep-name #f)))
                  (walk-edges locked-pkg (manifest-directory dep-manifest)
                              (manifest-dependency-sources dep-manifest)
                              (cons dep-name visiting)))))))
        dep-sources))
    (walk-edges #f root-dir (manifest-dependency-sources root-manifest) (list root-name))
    ;; Reject stale leftover entries in techne.lock that are no longer reachable.
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

(define (verify-world-lockfile lf #:store [store (package-store-dir)])
  "Verify every package in world lockfile LF against STORE.
Checks each entry's `tree-sha256` and returns an alist of
`((owner pkg) . manifest)` for all packages in LF."
  (map (lambda (p)
         (when (eq? 'path (car (locked-package-source p)))
           (error "World lockfile cannot contain local path entries" (locked-package-name p)))
         (cons (locked-package-name p) (verify-package-entry p "" store)))
       (lockfile-packages lf)))
