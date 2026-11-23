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
  "Validate DATUM and return a package source, without accessing a repository."
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
  "Register a catalog entry DATUM, (recipe (owner pkg) source), without fetching it."
  (unless (and (list? datum) (= 3 (length datum)) (eq? 'recipe (car datum)) (package-name? (cadr datum)))
    (error "Invalid recipe" datum))
  (let ((wanted (package-source-package (parse-package-source (caddr datum)))))
    (when (and wanted (not (equal? wanted (cadr datum)))) (error "Recipe names another package than its source" (cadr datum) wanted)))
  (hash-table-set! catalog (cadr datum) (%recipe (cadr datum) (caddr datum))))

(define (store)
  (let ((xdg (get-environment-variable "XDG_DATA_HOME")))
    (path-join (if (and xdg (path-absolute? xdg)) xdg (path-join (get-environment-variable "HOME") ".local/share"))
               "techne/packages")))
(define (one-datum file)
  (call-with-input-file file (lambda (p)
    (let ((d (read p))) (unless (and (not (eof-object? d)) (eof-object? (read p))) (error "Expected one datum" file)) d))))
(define (git dir protocol . args)
  (call-with-process "git" (append (list "-c" "core.hooksPath=/dev/null" "-c" "protocol.allow=never"
                                         "-c" (string-append "protocol." protocol ".allow=always")) args)
    (lambda (p) (let ((out (process-read-all p 'stdout)))
                  (unless (eqv? 0 (process-wait p)) (error "Package Git operation failed" (car args)))
                  (string-trim out))) #:directory dir #:stdin 'null #:stderr 'inherit
    #:environment '(("GIT_TERMINAL_PROMPT" . "0"))))
;; A store entry is one checkout of a repository at a commit, shared by the
;; packages installed from it: what they depend on beside them is one
;; directory for all. Its pin is (pin SOURCE COMMIT DIRECTORY ...): the
;; source without the package wanted, and the directories in the checkout
;; of the packages installed, relative; none, for a checkout that is itself
;; the one package.
(define (pin path)
  (let ((d (one-datum (path-join path "pin"))))
    (unless (and (list? d) (>= (length d) 3) (eq? (car d) 'pin) (oid? (caddr d))
                 (every (lambda (sub) (and (string? sub) (every component? (string-split sub "/")))) (cdddr d)))
      (error "Invalid package pin"))
    d))
(define (pin-directories d) (if (null? (cdddr d)) '("") (cdddr d)))
;; SOURCE, a source datum, without the package it wants: what is fetched.
(define (checkout-datum datum)
  (remove (lambda (e) (and (pair? e) (eq? (car e) 'package))) datum))
;; An installation: (ENTRY . DIRECTORY), a store entry and the directory in
;; its checkout of a package installed from it.
(define (installations path) (map (lambda (sub) (cons path sub)) (pin-directories (pin path))))
(define (installation-checkout i) (path-join (car i) "tree"))
(define (installation-tree i)
  (if (string=? (cdr i) "") (installation-checkout i) (path-join (installation-checkout i) (cdr i))))
(define (all-installations dir) (append-map installations (installed-paths dir)))
;; Where the package wanted is in the checkout TREE, relative: TREE itself
;; if it is a package, else the directory under it of the package WANTED,
;; or of the only one. A manifest that does not read is no candidate, nor
;; a directory a pin would not hold (`pin`): a name with a space, say.
(define (package-subdirectory tree wanted)
  (let ((found (filter (lambda (dir) (or (string=? dir "") (every component? (string-split dir "/"))))
                       (package-directories tree))))
    (cond ((null? found) (error "Repository holds no package: no techne-package.scm"))
          ((equal? found '("")) "")
          (else
           (let* ((named (filter-map (lambda (dir)
                                       (guard (e (#t #f)) (cons (manifest-name (read-package-manifest (path-join tree dir))) dir)))
                                     found))
                  (hit (and wanted (assoc wanted named))))
             (cond (hit (cdr hit))
                   (wanted (error "Repository holds no package of this name" wanted (map car named)))
                   ((and (pair? named) (null? (cdr named))) (cdar named))
                   (else (error "Repository holds several packages: say which with (package (owner pkg))" (map car named)))))))))
;; The package in DIRECTORY and what it depends on stay in the checkout
;; TREE, links resolved, and can be made known here: checked before a
;; checkout is published, and changing nothing.
(define (check-contained! tree directory)
  (let ((root (file-truename tree)) (dir (file-truename directory)))
    (unless (or (equal? root dir) (string-prefix? (string-append root "/") dir)) (error "Package leaves its repository" directory))
    (package-closure dir #:within root)))
;; The manifest of the package in ROOT, as `(techne package)` reads it: what
;; is installed here is for the editor.
(define (manifest root)
  (let ((m (read-package-manifest root)))
    (unless (manifest-editor m) (error "Package declares nothing for the editor" (manifest-name m)))
    m))
(define (commands m) (cadr (manifest-editor m)))
(define (options m) (caddr (manifest-editor m)))

;; One immutable checkout per source. Publish pin + tree by renaming the
;; temporary directory. Never overwrite another install or fetch on restore.
;; DATUM is what is fetched (`checkout-datum`), WANTED the package to
;; install from it, or #f. Return whether this published DEST: not if
;; another editor did meanwhile.
(define (fetch! dir dest datum source wanted)
  (let* ((tmp (path-join dir (string-append ".install-" (hex-encode (random-bytes 12)))))
         (tree (path-join tmp "tree")) (ref (package-source-ref source))
         (protocol (if (eq? (package-source-kind source) 'local) "file" "https")))
    (create-directory tmp #o700)
    (dynamic-wind (lambda () #f)
      (lambda ()
        (create-directory tree #o700)
        (git tree protocol "init" "--quiet" "--template=")
        (git tree protocol "fetch" "--quiet" "--depth=1" "--no-tags" "--" (package-source-url source)
             (if ref (case (git-ref-kind ref) ((branch) (string-append "refs/heads/" (git-ref-value ref)))
                       ((tag) (string-append "refs/tags/" (git-ref-value ref))) (else (git-ref-value ref))) "HEAD"))
        (let ((oid (git tree protocol "rev-parse" "--verify" "FETCH_HEAD^{commit}")))
          (unless (oid? oid) (error "Git returned an invalid commit"))
          (git tree protocol "checkout" "--quiet" "--detach" oid)
          (let* ((sub (package-subdirectory tree wanted))
                 (d (manifest (path-join tree sub))))
            (call-with-output-file (path-join tmp "pin")
              (lambda (p) (write (if (string=? sub "") (list 'pin datum oid) (list 'pin datum oid sub)) p)))
            ;; Two editors may have fetched different sources at the same time.
            ;; Serialize only the local preflight and atomic rename, not fetch.
            (publishing dir
              (lambda ()
                (and (not (file-exists? dest))
                     (begin
                       (check-declarations! d wanted)
                       (check-contained! tree (path-join tree sub))
                       (check-disjoint! (cons d (map stored-manifest (all-installations dir))))
                       (rename-file tmp dest)
                       #t)))))))
      (lambda () (when (file-exists? tmp) (delete-path tmp #t))))))

;; Call THUNK as the store DIR's only publisher (doc/packages.md).
(define (publishing dir thunk)
  (let ((lock (path-join dir ".publishing")))
    (dynamic-wind (lambda () (create-directory lock #o700)) thunk (lambda () (delete-path lock #f)))))
(define (write-pin! path d)
  (replace-file (path-join path "pin") (call-with-output-string (lambda (p) (write d p)))))
;; Install the package in the directory SUB of the checkout already at
;; DEST, beside those installed from it: checked as a fetched one is, then
;; added to the pin. Return whether it was added: not if it is there.
(define (add-installation! dir dest sub wanted)
  (publishing dir
    (lambda ()
      (let ((p (pin dest)) (i (cons dest sub)))
        (and (not (member sub (pin-directories p)))
             (let ((d (stored-manifest i)))
               (check-declarations! d wanted)
               (check-contained! (installation-checkout i) (installation-tree i))
               (check-disjoint! (cons d (map stored-manifest (all-installations dir))))
               (write-pin! dest (append p (list sub)))
               #t))))))
;; Take the installation of SUB put in DEST back: it could not be made
;; known here. The others installed from the checkout stay, another editor's
;; added meanwhile too; the checkout goes with the last.
(define (remove-installation! dir dest sub)
  (publishing dir
    (lambda ()
      (let* ((p (pin dest)) (others (delete sub (pin-directories p))))
        (if (null? others)
            (delete-path dest #t)
            (write-pin! dest (append (list 'pin (cadr p) (caddr p)) others)))))))

(define (installed-paths dir)
  (map (lambda (key) (path-join dir key))
       (filter oid? (map path-file-name (filter (lambda (entry) (string-suffix? "/" entry)) (directory-list dir))))))
(define (stored-manifest i) (manifest (installation-tree i)))
(define (check-disjoint! manifests)
  (let ((unique! (lambda (names)
                   (unless (= (length names) (length (delete-duplicates names))) (error "Conflicting package declarations" names)))))
    (unique! (map manifest-name manifests))
    (unique! (append-map (lambda (d) (map car (commands d))) manifests))
    (unique! (append-map (lambda (d) (map car (options d))) manifests))))
(define indexed (make-hash-table equal?))
;; Run the same checks before rename and before restoring any declarations.
(define (check-declarations! d expected)
  (let ((name (manifest-name d)))
    (when (and expected (not (equal? expected name))) (error "Recipe and manifest disagree" expected name))
    (when (or (find-package name) (package-declared? name)) (error "Package name already installed" name))
    (for-each (lambda (c) (when (memq (car c) (command-names)) (error "Command already registered" (car c)))) (commands d))
    (for-each (lambda (o) (when (find-option (car o)) (error "Option already declared" (car o)))) (options d))))
;; Advertise the installation I; EXPECTED is the name it must have, or #f.
(define (announce! i expected)
  (let* ((tree (installation-tree i)) (d (manifest tree)) (name (manifest-name d)) (old (hash-table-ref/default indexed name #f)))
    (when (and expected (not (equal? expected name))) (error "Package name mismatch" name expected))
    (cond (old (unless (equal? old i) (error "Package name already installed" name)))
          (else
           (check-declarations! d expected)
           (check-contained! (installation-checkout i) tree)
           ;; As a package directory Techne ships is advertised, its
           ;; libraries and those of what it depends on found first: the
           ;; packages beside it in its checkout, at the commit pinned.
           (use-package-directory! tree #:within (installation-checkout i))
           (advertise-package! name (car (manifest-editor d)) (commands d) (options d))
           (hash-table-set! indexed name i))) name))
(define (package-install! spec #:directory [directory (store)])
  "Install SPEC (a source datum or catalog `(owner pkg)`) and advertise it.
DIRECTORY is the package store. First install pins a commit; repeats are offline.
Of a repository holding several packages, the one installed is the one
the source names with (package (owner pkg)), else the one of the catalog
name. The packages installed from one source share its checkout, and so
the packages beside them that they depend on."
  (let* ((recipe (and (package-name? spec) (or (hash-table-ref/default catalog spec #f) (error "Unknown recipe" spec))))
         (datum (if recipe (recipe-source recipe) spec)) (source (parse-package-source datum)) (dir directory)
         (wanted (or (package-source-package source) (and recipe (recipe-name recipe))))
         ;; The package wanted is no part of what is fetched, nor of its key.
         (checkout (checkout-datum datum))
         (key (hex-encode (sha-256 (string->utf8 (call-with-output-string (lambda (p) (write checkout p)))))))
         (dest (path-join dir key)))
    (make-directory dir)
    (let ((published (and (not (file-exists? dest)) (fetch! dir dest checkout source wanted))))
      (unless (equal? (cadr (pin dest)) checkout) (error "Pin does not match recipe"))
      (let* ((sub (package-subdirectory (path-join dest "tree") wanted))
             ;; A repeat takes no part in publishing.
             (added (and (not (member sub (pin-directories (pin dest)))) (add-installation! dir dest sub wanted))))
        ;; What was just put in the store and cannot be made known here
        ;; after all is taken back: left there, it would fail every later
        ;; install and restore.
        (guard (e ((or published added) (remove-installation! dir dest sub) (raise e)))
          (announce! (cons dest sub) wanted))))))
(define (package-restore! #:directory [directory (store)])
  "Advertise all packages installed in DIRECTORY without fetching or loading."
  (let ((dir directory))
    (if (file-exists? dir)
        (let* ((all (all-installations dir)) (manifests (map stored-manifest all)))
          (check-disjoint! manifests)
          (for-each (lambda (i d)
                      (let ((old (hash-table-ref/default indexed (manifest-name d) #f)))
                        (if old (unless (equal? old i) (error "Package name already installed" (manifest-name d)))
                            (check-declarations! d #f)))) all manifests)
          (map (lambda (i) (announce! i #f)) all)) '())))
