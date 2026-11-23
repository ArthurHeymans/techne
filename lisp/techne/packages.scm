(define-record-type git-ref (%ref kind value) git-ref?
  (kind git-ref-kind) (value git-ref-value))
(define-record-type package-source (%source kind url ref package) package-source?
  (kind package-source-kind) (url package-source-url) (ref package-source-ref)
  ;; The package wanted of a repository holding several, (owner pkg), or #f.
  (package package-source-package))
(define-record-type recipe (%recipe name source) recipe?
  (name recipe-name) (source recipe-source))
(define catalog (make-hash-table equal?))

(define (ascii-alnum? c)
  (let ((n (char->integer c)))
    (or (and (>= n 97) (<= n 122))
        (and (>= n 65) (<= n 90))
        (and (>= n 48) (<= n 57)))))

(define (component? s)
  (and (string? s) (positive? (string-length s))
       (ascii-alnum? (string-ref s 0))
       (every (lambda (c) (or (ascii-alnum? c) (memv c '(#\- #\_ #\.)))) (string->list s))))
(define (oid? s)
  (and (string? s) (memv (string-length s) '(40 64))
       (every (lambda (c) (string-index "0123456789abcdef" c)) (string->list s))))
(define (parse-ref ref)
  (unless (and (list? ref) (= 2 (length ref)) (string? (cadr ref))
               (case (car ref)
                 ((branch tag) (every component? (string-split (cadr ref) "/")))
                 ((commit) (oid? (cadr ref))) (else #f)))
    (error "Expected (branch name), (tag name) or (commit full-object-id)" ref))
  (%ref (car ref) (cadr ref)))
(define (parse-package-source datum)
  "Validate DATUM and return a package source.
Does not access any repository."
  (unless (and (list? datum) (memv (length datum) '(2 3 4)) (string? (cadr datum)))
    (error "Expected (github owner/repo [ref] [(package (owner pkg))]), (git https-url ...) or (local absolute-repo ...)" datum))
  (let* ((kind (car datum)) (s (cadr datum))
         (url (case kind
                ((github) (unless (let ((parts (string-split s "/"))) (and (= 2 (length parts)) (every component? parts)))
                            (error "Expected a GitHub owner/repo" s))
                          (string-append "https://github.com/" s ".git"))
                ((git) (unless (and (string-prefix? "https://" s) (> (string-length s) 8)) (error "Expected HTTPS Git URL")) s)
                ((local) (unless (path-absolute? s) (error "Expected absolute repository path" s)) s)
                (else (error "Unknown package source" kind)))))
    ;; After the place: a revision, and the package wanted, in either order.
    (let* ((package? (lambda (e) (and (pair? e) (eq? (car e) 'package))))
           (packages (filter package? (cddr datum)))
           (refs (remove package? (cddr datum))))
      (when (or (pair? (if (pair? packages) (cdr packages) '())) (pair? (if (pair? refs) (cdr refs) '())))
        (error "Expected at most one revision and one (package (owner pkg))" datum))
      (for-each (lambda (p)
                  (unless (and (list? p) (= 2 (length p)) (package-name? (cadr p)))
                    (error "Expected (package (owner pkg))" p)))
                packages)
      (%source kind url (and (pair? refs) (parse-ref (car refs))) (and (pair? packages) (cadr (car packages)))))))
(define (register-package-recipe! datum)
  "Register a catalog entry DATUM without fetching it.
DATUM has the form `(recipe (owner pkg) source)`."
  (unless (and (list? datum) (= 3 (length datum)) (eq? 'recipe (car datum)) (package-name? (cadr datum)))
    (error "Invalid recipe" datum))
  (let ((wanted (package-source-package (parse-package-source (caddr datum)))))
    (when (and wanted (not (equal? wanted (cadr datum)))) (error "Recipe names another package than its source" (cadr datum) wanted)))
  (hash-table-set! catalog (cadr datum) (%recipe (cadr datum) (caddr datum))))

;; --- Content-addressed store, world.lock & installed roots ---

;; Resolve metadata directory (holding installed.scm, world.lock, .publishing)
;; and content-addressed store directory. When DIRECTORY is omitted, honors
;; $TECHNE_PACKAGE_STORE via (package-store-dir); when an explicit DIRECTORY is
;; passed without STORE, uses <DIRECTORY>/store for isolated environments/tests.
(define (resolve-install-dirs directory store)
  (let* ((dir (or directory (packages-data-dir)))
         (sdir (or store
                   (if directory
                       (path-join directory "store")
                       (package-store-dir)))))
    (values dir sdir)))

(define (world-lock-path dir) (path-join dir "world.lock"))
(define (installed-roots-path dir) (path-join dir "installed.scm"))

(define (one-datum file)
  (call-with-input-file file
    (lambda (p)
      (let ((d (read p)))
        (unless (and (not (eof-object? d)) (eof-object? (read p)))
          (error "Expected one datum" file))
        d))))

(define (strip-package-clause datum)
  (remove (lambda (e) (and (pair? e) (eq? (car e) 'package))) datum))

(define (with-package-clause datum name)
  (if (any (lambda (e) (and (pair? e) (eq? (car e) 'package))) (cddr datum))
      datum
      (append datum (list (list 'package name)))))

(define (read-installed-roots dir)
  (let ((path (installed-roots-path dir)))
    (if (not (file-exists? path))
        '()
        (let ((d (one-datum path)))
          (unless (and (list? d) (pair? d) (eq? 'installed (car d)))
            (error "Invalid installed.scm" path))
          (map (lambda (entry)
                 (unless (and (list? entry) (= 3 (length entry)) (eq? 'root (car entry))
                              (package-name? (cadr entry)) (package-source-datum? (caddr entry)))
                   (error "Invalid root entry in installed.scm" entry))
                 (cons (cadr entry) (caddr entry)))
               (cdr d))))))

(define (write-installed-roots! dir roots)
  (let ((sorted (sort roots (lambda (a b) (package-name<? (car a) (car b))))))
    (replace-file (installed-roots-path dir)
                  (call-with-output-string
                    (lambda (port)
                      (display "(installed" port)
                      (for-each (lambda (r)
                                  (display "\n  (root " port)
                                  (write (car r) port)
                                  (display " " port)
                                  (write (cdr r) port)
                                  (display ")" port))
                                sorted)
                      (display ")\n" port))))))

(define (read-world-lock dir)
  (let ((path (world-lock-path dir)))
    (and (file-exists? path) (read-lockfile path))))

;; Compute the subset of LF reachable from all installed ROOTS except DROP-NAME.
;; Used when upgrading DROP-NAME so its old version and exclusive dependencies do
;; not conflict with the upgrade, while dependencies still shared with other
;; installed roots remain locked and conflict-checked.
(define (lockfile-without-root lf roots drop-name)
  (if (not lf)
      #f
      (let ((by-name (make-hash-table equal?))
            (keep (make-hash-table equal?)))
        (for-each (lambda (p) (hash-table-set! by-name (locked-package-name p) p))
                  (lockfile-packages lf))
        (define (mark! name)
          (unless (hash-table-ref/default keep name #f)
            (let ((p (hash-table-ref/default by-name name #f)))
              (when p
                (hash-table-set! keep name p)
                (for-each mark! (locked-package-depends p))))))
        (for-each (lambda (r)
                    (unless (equal? (car r) drop-name)
                      (mark! (car r))))
                  roots)
        (if (zero? (hash-table-size keep))
            #f
            (make-lockfile (lockfile-techne lf) (hash-table-values keep))))))

(define (find-installed-root roots datum wanted)
  (let ((base (strip-package-clause datum)))
    (if wanted
        (find (lambda (r)
                (and (equal? (car r) wanted)
                     (equal? (strip-package-clause (cdr r)) base)))
              roots)
        (let ((exact (filter (lambda (r) (equal? (cdr r) datum)) roots)))
          (if (= 1 (length exact))
              (car exact)
              (let ((norm (filter (lambda (r) (equal? (strip-package-clause (cdr r)) base)) roots)))
                (and (= 1 (length norm)) (car norm))))))))

(define (upsert-root roots name datum)
  (cons (cons name datum)
        (remove (lambda (r) (equal? (car r) name)) roots)))

;; Call THUNK as the store DIR's only publisher (doc/packages.md).
(define (publishing dir thunk)
  (let ((lock (path-join dir ".publishing")))
    (dynamic-wind (lambda () (create-directory lock #o700)) thunk (lambda () (delete-path lock #f)))))

;; --- Manifest inspection (editor clause is optional) ---

(define (commands m) (if (manifest-editor m) (cadr (manifest-editor m)) '()))
(define (options m) (if (manifest-editor m) (caddr (manifest-editor m)) '()))

(define (check-disjoint! manifests)
  (let ((unique! (lambda (names)
                   (unless (= (length names) (length (delete-duplicates names)))
                     (error "Conflicting package declarations" names)))))
    (unique! (map manifest-name manifests))
    (unique! (append-map (lambda (d) (map car (commands d))) manifests))
    (unique! (append-map (lambda (d) (map car (options d))) manifests))))

;; Names announced in this editor VM, mapped to their verified tree-sha256.
(define indexed (make-hash-table equal?))

;; Run the same checks before publishing and before restoring any declarations.
(define (check-declarations! d expected)
  (let ((name (manifest-name d)))
    (when (and expected (not (equal? expected name))) (error "Recipe and manifest disagree" expected name))
    (when (or (find-package name) (package-declared? name)) (error "Package name already installed" name))
    (for-each (lambda (c) (when (memq (car c) (command-names)) (error "Command already registered" (car c)))) (commands d))
    (for-each (lambda (o) (when (find-option (car o)) (error "Option already declared" (car o)))) (options d))))

;; Collect the manifests of ROOT-NAME and all its transitive dependencies in LF,
;; using MANIFEST-ALIST `((owner pkg) . manifest)`.
(define (closure-manifests lf manifest-alist root-name)
  (let ((by-name (make-hash-table equal?))
        (seen (make-hash-table equal?))
        (result '()))
    (for-each (lambda (p) (hash-table-set! by-name (locked-package-name p) p))
              (lockfile-packages lf))
    (define (visit name)
      (unless (hash-table-ref/default seen name #f)
        (hash-table-set! seen name #t)
        (let ((p (or (hash-table-ref/default by-name name #f)
                     (error "Missing package in world.lock" name)))
              (pair (or (assoc name manifest-alist)
                        (error "Missing verified manifest" name))))
          (for-each visit (locked-package-depends p))
          (set! result (cons (cdr pair) result)))))
    (visit root-name)
    (reverse result)))

(define (locked-package-hash lf name)
  (let ((p (find (lambda (e) (equal? (locked-package-name e) name)) (lockfile-packages lf))))
    (unless p (error "Missing package in world.lock" name))
    (locked-package-tree-sha256 p)))

(define (locked-package-subdir-of lf name)
  (let* ((p (find (lambda (e) (equal? (locked-package-name e) name)) (lockfile-packages lf)))
         (src (and p (locked-package-source p)))
         (sub (and src (assq 'subdir (cddr src)))))
    (and sub (cadr sub))))

;; Wire libraries and (when declared) editor commands/options for ROOT-NAME
;; from a verified world lockfile LF and MANIFEST-ALIST.
(define (announce-root! lf manifest-alist root-name expected upgrade?)
  (let* ((pair (or (assoc root-name manifest-alist)
                   (error "Missing verified manifest" root-name)))
         (d (cdr pair))
         (hash (locked-package-hash lf root-name))
         (old (hash-table-ref/default indexed root-name #f)))
    (when (and expected (not (equal? expected root-name)))
      (error "Package name mismatch" root-name expected))
    (cond ((and old (string=? old hash)) root-name)
          ((and old upgrade?)
           ;; Upgraded checkout is stored and locked for subsequent sessions;
           ;; keep the running VM's existing library bindings intact while
           ;; updating indexed so repeat installs/restores see the new hash.
           (hash-table-set! indexed root-name hash)
           root-name)
          (old (error "Package name already installed" root-name))
          (else
           (check-declarations! d expected)
           (let ((closure (closure-manifests lf manifest-alist root-name)))
             (register-package-world! closure)
             (when (manifest-editor d)
               (advertise-package! root-name (car (manifest-editor d)) (commands d) (options d)))
             (hash-table-set! indexed root-name hash)
             root-name)))))

;; Validate that staged manifests for a candidate install are compatible with
;; already installed roots and current editor declarations.
(define (validate-candidate! sdir root-name staged-manifests existing-lock existing-roots wanted upgrade?)
  (let* ((root-pair (or (assoc root-name staged-manifests)
                        (error "Resolved package missing from staged manifests" root-name)))
         (root-manifest (cdr root-pair))
         (old-in-vm (hash-table-ref/default indexed root-name #f)))
    (when (and (not upgrade?) (assoc root-name existing-roots))
      (error "Package name already installed" root-name))
    (unless (and old-in-vm upgrade?)
      (check-declarations! root-manifest wanted))
    (let ((other-manifests
           (if existing-lock
               (filter-map (lambda (r)
                             (and (not (and upgrade? (equal? (car r) root-name)))
                                  (let ((p (find (lambda (e) (equal? (locked-package-name e) (car r)))
                                                 (lockfile-packages existing-lock))))
                                    (and p (read-package-manifest (path-join sdir (locked-package-tree-sha256 p)))))))
                           existing-roots)
               '())))
      (check-disjoint! (cons root-manifest other-manifests)))))

;; Roll back metadata files if post-publish verification or announcement fails.
;; Must be called while already holding `.publishing`. Never deletes
;; content-addressed `store/<tree-sha256>` entries because they are immutable
;; and may be shared by other installations or worlds.
(define (rollback-metadata! dir old-lock old-roots)
  (if old-lock
      (write-lockfile (world-lock-path dir) old-lock)
      (when (file-exists? (world-lock-path dir))
        (delete-file (world-lock-path dir))))
  (if (pair? old-roots)
      (write-installed-roots! dir old-roots)
      (when (file-exists? (installed-roots-path dir))
        (delete-file (installed-roots-path dir)))))

(define (package-install! spec #:directory [directory #f] #:store [store #f] #:upgrade [upgrade #f])
  "Install SPEC (a source datum or catalog `(owner pkg)`) and advertise it.
DIRECTORY holds `world.lock` and `installed.scm` (defaulting to
`(packages-data-dir)`). STORE overrides the content-addressed store
directory (defaulting to `(package-store-dir)`, which honors
`$TECHNE_PACKAGE_STORE`, or `<DIRECTORY>/store` when DIRECTORY is given).
First install pins commits and content hashes; repeats are offline unless
UPGRADE is `#t`. Packages without an `(editor ...)` clause are stored,
verified, and registered cleanly."
  (let* ((recipe (and (package-name? spec) (or (hash-table-ref/default catalog spec #f) (error "Unknown recipe" spec))))
         (datum (if recipe (recipe-source recipe) spec))
         (source (parse-package-source datum))
         (wanted (or (package-source-package source) (and recipe (recipe-name recipe)))))
    (call-with-values (lambda () (resolve-install-dirs directory store))
      (lambda (dir sdir)
        (make-directory dir)
        (define (offline-repeat hit)
          (let* ((lf (or (read-world-lock dir) (error "Missing world.lock for installed package" (car hit))))
                 (manifests (verify-world-closure lf (list (car hit)) #:store sdir #:root-recipes (list hit))))
            (announce-root! lf manifests (car hit) wanted #f)))
        ;; Lock-free fast path for offline repeats when not upgrading.
        (let* ((initial-roots (read-installed-roots dir))
               (initial-hit (and (not upgrade) (find-installed-root initial-roots datum wanted))))
          (if initial-hit
              (offline-repeat initial-hit)
              ;; Acquire `.publishing` BEFORE reading `installed.scm` and `world.lock`
              ;; so concurrent installs cannot lose updates or clobber `world.lock`.
              (publishing dir
                (lambda ()
                  (let* ((cur-roots (read-installed-roots dir))
                         (cur-hit (and (not upgrade) (find-installed-root cur-roots datum wanted))))
                    (if cur-hit
                        (offline-repeat cur-hit)
                        (let ((cur-lock (read-world-lock dir)))
                          (when (and (pair? cur-roots) (not cur-lock))
                            (error "Missing world.lock for installed packages" dir))
                          (let* ((drop-target
                                  (and upgrade
                                       (or wanted
                                           (let ((existing (find-installed-root cur-roots datum #f)))
                                             (and existing (car existing)))
                                           (let ((same-repo
                                                  (filter (lambda (r)
                                                            (and (eq? (car (cdr r)) (car datum))
                                                                 (equal? (cadr (cdr r)) (cadr datum))))
                                                          cur-roots)))
                                             (and (= 1 (length same-repo)) (caar same-repo))))))
                                 (base-lock (if (and upgrade drop-target)
                                                (lockfile-without-root cur-lock cur-roots drop-target)
                                                cur-lock))
                                 (recorded-entry #f))
                            (call-with-values
                              (lambda ()
                                (resolve-package-source
                                  datum
                                  #:wanted wanted
                                  #:store sdir
                                  #:existing-lock base-lock
                                  #:stage-root dir
                                  #:wrap-publish
                                  (lambda (root-name staged-manifests merged-lock publish!)
                                    (validate-candidate! sdir root-name staged-manifests cur-lock cur-roots wanted upgrade)
                                    (let* ((published (publish!))
                                           (sub (locked-package-subdir-of merged-lock root-name))
                                           (root-datum (if sub (with-package-clause datum root-name) datum))
                                           (new-roots (upsert-root cur-roots root-name root-datum)))
                                      (set! recorded-entry (cons root-name root-datum))
                                      (write-lockfile (world-lock-path dir) merged-lock)
                                      (write-installed-roots! dir new-roots)
                                      published))))
                              (lambda (root-name merged-lock _newly-published)
                                (guard (e (#t
                                           (rollback-metadata! dir cur-lock cur-roots)
                                           (raise e)))
                                  (let ((manifests (verify-world-closure merged-lock (list root-name)
                                                                         #:store sdir
                                                                         #:root-recipes (list recorded-entry))))
                                    (announce-root! merged-lock manifests root-name wanted upgrade)))))))))))))))))

(define (package-restore! #:directory [directory #f] #:store [store #f])
  "Verify and restore all packages installed in DIRECTORY offline.
Reads `world.lock` and `installed.scm` from DIRECTORY, checks the exact
transitive closure of installed roots against STORE (defaulting to
`(package-store-dir)`, or `<DIRECTORY>/store` when DIRECTORY is given),
verifies every `tree-sha256`, registers library providers and import
boundaries, and advertises editor declarations when present."
  (call-with-values (lambda () (resolve-install-dirs directory store))
    (lambda (dir sdir)
      (if (not (file-exists? dir))
          '()
          (let* ((roots (read-installed-roots dir))
                 (lock-exists? (file-exists? (world-lock-path dir))))
            (cond
              ((and (null? roots) (not lock-exists?)) '())
              ((and (pair? roots) (not lock-exists?))
               (error "Missing world.lock for installed packages" dir))
              (else
               (let* ((lf (read-world-lock dir))
                      (root-names (map car roots))
                      (manifest-alist (verify-world-closure lf root-names
                                                            #:store sdir
                                                            #:exact #t
                                                            #:root-recipes roots))
                      (root-manifests (map (lambda (name)
                                             (let ((p (assoc name manifest-alist)))
                                               (unless p (error "Installed root missing from world.lock" name))
                                               (cdr p)))
                                           root-names)))
                 ;; Validate all declarations and disjointness before mutating editor state.
                 (check-disjoint! root-manifests)
                 (for-each (lambda (name m)
                             (let ((hash (locked-package-hash lf name))
                                   (old (hash-table-ref/default indexed name #f)))
                               (if old
                                   (unless (string=? old hash)
                                     (error "Package name already installed" name))
                                   (check-declarations! m #f))))
                           root-names root-manifests)
                 ;; Register all verified package libraries atomically, then advertise editor packages.
                 (register-package-world! (map cdr manifest-alist))
                 (map (lambda (name m)
                        (let ((hash (locked-package-hash lf name))
                              (old (hash-table-ref/default indexed name #f)))
                          (unless old
                            (when (manifest-editor m)
                              (advertise-package! name (car (manifest-editor m)) (commands m) (options m)))
                            (hash-table-set! indexed name hash))
                          name))
                      root-names root-manifests)))))))))
