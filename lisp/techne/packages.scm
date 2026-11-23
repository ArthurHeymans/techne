(define-record-type git-ref (%ref kind value) git-ref?
  (kind git-ref-kind) (value git-ref-value))
(define-record-type package-source (%source kind url ref package) package-source?
  (kind package-source-kind) (url package-source-url) (ref package-source-ref)
  ;; The package wanted of a repository holding several, (owner pkg), or #f.
  (package package-source-package))
(define-record-type recipe (%recipe name source) recipe?
  (name recipe-name) (source recipe-source))
(define catalog (make-hash-table equal?))

(define (component? s)
  (and (string? s) (positive? (string-length s)) (not (member s '("." "..")))
       (every (lambda (c) (or (char-alphabetic? c) (char-numeric? c) (memv c '(#\- #\_ #\.)))) (string->list s))))
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

(define (store) (packages-data-dir))
(define (store-dir dir) (path-join dir "store"))
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
  (if wanted
      (find (lambda (r)
              (and (equal? (car r) wanted)
                   (equal? (strip-package-clause (cdr r)) (strip-package-clause datum))))
            roots)
      (let ((hits (filter (lambda (r) (equal? (cdr r) datum)) roots)))
        (and (= 1 (length hits)) (car hits)))))

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
           ;; keep the running VM's existing library bindings intact.
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
(define (validate-candidate! dir root-name staged-manifests existing-lock existing-roots wanted upgrade?)
  (let* ((root-pair (or (assoc root-name staged-manifests)
                        (error "Resolved package missing from staged manifests" root-name)))
         (root-manifest (cdr root-pair))
         (old-in-vm (hash-table-ref/default indexed root-name #f)))
    (when (and (not upgrade?) (assoc root-name existing-roots))
      (error "Package name already installed" root-name))
    (unless (and old-in-vm upgrade?)
      (check-declarations! root-manifest wanted))
    (let* ((sdir (store-dir dir))
           (other-manifests
            (if existing-lock
                (filter-map (lambda (r)
                              (and (not (and upgrade? (equal? (car r) root-name)))
                                   (let ((p (find (lambda (e) (equal? (locked-package-name e) (car r)))
                                                  (lockfile-packages existing-lock))))
                                     (and p (read-package-manifest (path-join sdir (locked-package-tree-sha256 p)))))))
                            existing-roots)
                '())))
      (check-disjoint! (cons root-manifest other-manifests)))))

;; Roll back disk state if announcing fails after publishing.
(define (rollback-install! dir old-lock old-roots newly-published)
  (publishing dir
    (lambda ()
      (if old-lock
          (write-lockfile (world-lock-path dir) old-lock)
          (when (file-exists? (world-lock-path dir))
            (delete-file (world-lock-path dir))))
      (if (pair? old-roots)
          (write-installed-roots! dir old-roots)
          (when (file-exists? (installed-roots-path dir))
            (delete-file (installed-roots-path dir))))
      (for-each (lambda (entry)
                  (when (file-exists? (cdr entry))
                    (delete-path (cdr entry) #t)))
                newly-published)
      (let ((sdir (store-dir dir)))
        (when (and (not old-lock) (file-exists? sdir) (null? (directory-list sdir)))
          (delete-path sdir #f))))))

(define (package-install! spec #:directory [directory (store)] #:upgrade [upgrade #f])
  "Install SPEC (a source datum or catalog `(owner pkg)`) and advertise it.
DIRECTORY is the packages data directory holding `store/<tree-sha256>/`,
`world.lock`, and `installed.scm`. First install pins commits and content
hashes; repeats are offline unless UPGRADE is `#t`. Packages without an
`(editor ...)` clause (pure library or `(cloud ...)` packages) are stored,
verified, and registered cleanly without requiring editor commands or
options."
  (let* ((recipe (and (package-name? spec) (or (hash-table-ref/default catalog spec #f) (error "Unknown recipe" spec))))
         (datum (if recipe (recipe-source recipe) spec))
         (source (parse-package-source datum))
         (dir directory)
         (sdir (store-dir dir))
         (wanted (or (package-source-package source) (and recipe (recipe-name recipe)))))
    (make-directory dir)
    (let* ((initial-roots (read-installed-roots dir))
           (hit (and (not upgrade) (find-installed-root initial-roots datum wanted))))
      (if hit
          ;; Offline repeat: verify world.lock + store tree-sha256 without touching Git.
          (let* ((lf (or (read-world-lock dir) (error "Missing world.lock for installed package" (car hit))))
                 (manifests (verify-world-lockfile lf #:store sdir)))
            (announce-root! lf manifests (car hit) wanted #f))
          ;; Fetch, stage, resolve transitive dependencies + diamond conflicts, then publish atomically.
          (let* ((pre-lock (read-world-lock dir))
                 (pre-roots initial-roots)
                 (drop-target (and upgrade
                                   (or wanted
                                       (let ((existing (find-installed-root pre-roots datum #f)))
                                         (and existing (car existing)))
                                       (let ((same-repo
                                              (filter (lambda (r)
                                                        (and (eq? (car (cdr r)) (car datum))
                                                             (equal? (cadr (cdr r)) (cadr datum))))
                                                      pre-roots)))
                                         (and (= 1 (length same-repo)) (caar same-repo))))))
                 (base-lock (if (and upgrade drop-target)
                                (lockfile-without-root pre-lock pre-roots drop-target)
                                pre-lock))
                 (saved-old-lock #f)
                 (saved-old-roots '())
                 (recorded-datum #f))
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
                    (publishing dir
                      (lambda ()
                        (let ((cur-lock (read-world-lock dir))
                              (cur-roots (read-installed-roots dir)))
                          (set! saved-old-lock cur-lock)
                          (set! saved-old-roots cur-roots)
                          (validate-candidate! dir root-name staged-manifests cur-lock cur-roots wanted upgrade)
                          (let* ((published (publish!))
                                 (sub (locked-package-subdir-of merged-lock root-name))
                                 (root-datum (if sub (with-package-clause datum root-name) datum))
                                 (new-roots (upsert-root cur-roots root-name root-datum)))
                            (set! recorded-datum root-datum)
                            (write-lockfile (world-lock-path dir) merged-lock)
                            (write-installed-roots! dir new-roots)
                            published)))))))
              (lambda (root-name merged-lock newly-published)
                (guard (e (#t
                           (rollback-install! dir saved-old-lock saved-old-roots newly-published)
                           (raise e)))
                  (let ((manifests (verify-world-lockfile merged-lock #:store sdir)))
                    (announce-root! merged-lock manifests root-name wanted upgrade))))))))))

(define (package-restore! #:directory [directory (store)])
  "Verify and restore all packages installed in DIRECTORY strictly offline.
Reads `world.lock` and `installed.scm`, checks every materialized package's
`tree-sha256` against `<DIRECTORY>/store/<tree-sha256>/`, registers library
providers and import boundaries, and advertises editor declarations when
present."
  (let ((dir directory))
    (if (not (and (file-exists? dir) (file-exists? (world-lock-path dir))))
        '()
        (let* ((sdir (store-dir dir))
               (lf (read-world-lock dir))
               (roots (read-installed-roots dir))
               (root-names (map car roots))
               (manifest-alist (verify-world-lockfile lf #:store sdir))
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
               root-names root-manifests)))))
