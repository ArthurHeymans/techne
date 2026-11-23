;;; A package's directory: its manifest, `techne-package.scm`, one datum
;;; read and never evaluated; the libraries it provides, under `lib/`; and
;;; the packages it depends on. Nothing here needs the editor: a program
;;; run from a package's directory (`techne run`) and the editor's
;;; packages resolve their libraries the same way.
;;;
;;;   (package (techne mail)
;;;     (version "0.1.0")
;;;     (techne "0.4")
;;;     (description "Native Scheme mail client.")
;;;     (depends
;;;       ((alice authinfo) (path "../authinfo"))
;;;       ((bob util) (github "bob/util" (tag "v1.0"))))
;;;     (override
;;;       ((bob util) (path "../local-util")))
;;;     (editor "editor.scm"
;;;       (commands (mail "Open the mail dashboard."))
;;;       (options (mail-prefer-html #t boolean "Whether to show HTML.")))
;;;     (cloud "app.scm"
;;;       (bindings (cache kv) (api-key secret))))
;;;
;;; The package `(owner pkg)` owns the libraries whose names begin with
;;; `(owner pkg)`: `(techne mail)` is `lib/techne/mail.sld` and `(techne
;;; mail imap)` is `lib/techne/mail/imap.sld` in its directory and nowhere
;;; else. They are the world's, one instance whoever imports them.
;;; `depends` names the packages whose libraries this one imports, by
;;; local relative path or by Git repository (`github`, `git`, `local`).
;;; `override` is a root-only clause that replaces the source of any
;;; direct or transitive dependency. `editor` is what the editor loads on
;;; first use (doc/packages.md): the entry file, the commands advertised
;;; before that and the options declared early, as
;;; (name default schema documentation). `cloud` declares a cloud worker
;;; entry file and its binding interface (name and kind: kv, secret, text).

(define-library (techne package)
  (export package-manifest-file package-name? package-name<? package-source-datum?
          read-package-manifest package-manifest?
          manifest-name manifest-directory manifest-version manifest-techne manifest-description
          manifest-dependencies manifest-dependency-sources manifest-overrides
          manifest-editor manifest-cloud
          directory-tree-sha256 package-tree-sha256
          register-package-world! use-package-directory! package-closure package-directories)
  (import (techne) (techne crypto) (techne path) (techne schema))
  (begin

    (define package-manifest-file "techne-package.scm"
      "The name of the file in a package's directory holding its manifest.")

    (define-record-type manifest
      (%manifest name directory version techne description dependencies dependency-sources overrides editor cloud)
      package-manifest?
      (name manifest-name)
      (directory manifest-directory)
      (version manifest-version)
      (techne manifest-techne)
      (description manifest-description)
      (dependencies manifest-dependencies)
      (dependency-sources manifest-dependency-sources)
      (overrides manifest-overrides)
      (editor manifest-editor)
      (cloud manifest-cloud))

    (define (bad what . irritants)
      (apply error (string-append package-manifest-file ": " what) irritants))

    (define (ascii-lower-or-digit? c)
      (let ((n (char->integer c)))
        (or (and (>= n 97) (<= n 122))
            (and (>= n 48) (<= n 57)))))

    (define (ascii-alnum? c)
      (let ((n (char->integer c)))
        (or (and (>= n 97) (<= n 122))
            (and (>= n 65) (<= n 90))
            (and (>= n 48) (<= n 57)))))

    ;; A name that is one ASCII path component starting with an ASCII letter or
    ;; digit, never `.` or `-`, and nothing a shell or a path could take otherwise.
    (define (component? s)
      (and (string? s) (positive? (string-length s))
           (ascii-alnum? (string-ref s 0))
           (every (lambda (c) (or (ascii-alnum? c) (memv c '(#\- #\_ #\.)))) (string->list s))))

    (define (package-part? sym)
      (and (symbol? sym)
           (let ((s (symbol->string sym)))
             (and (positive? (string-length s))
                  (ascii-lower-or-digit? (string-ref s 0))
                  (every (lambda (c) (or (ascii-lower-or-digit? c) (memv c '(#\- #\_ #\.)))) (string->list s))))))

    (define (package-name? x)
      "Return whether X is a canonical `(owner pkg)` package identity."
      (and (list? x) (= 2 (length x)) (package-part? (car x)) (package-part? (cadr x))))

    (define (package-name<? a b)
      "Return whether package identity A sorts lexicographically before B."
      (let ((oa (symbol->string (car a)))
            (ob (symbol->string (car b))))
        (or (string<? oa ob)
            (and (string=? oa ob)
                 (string<? (symbol->string (cadr a)) (symbol->string (cadr b)))))))

    (define (reserved-namespace? name)
      (memq (car name) '(scheme srfi)))

    (define (version-string? s)
      (and (string? s) (positive? (string-length s))
           (char-numeric? (string-ref s 0))
           (every (lambda (c) (or (char-alphabetic? c) (char-numeric? c) (memv c '(#\. #\- #\+))))
                  (string->list s))))

    (define (oid? s)
      (and (string? s) (memv (string-length s) '(40 64))
           (every (lambda (c) (and (string-index "0123456789abcdef" c) #t)) (string->list s))))

    (define (https-url? s)
      (and (string? s) (string-prefix? "https://" s) (> (string-length s) 8)
           (not (any (lambda (c) (or (char-whitespace? c) (< (char->integer c) 32))) (string->list s)))))

    (define (git-ref-clause? x)
      (and (list? x) (= 2 (length x)) (string? (cadr x)) (positive? (string-length (cadr x)))
           (case (car x)
             ((branch tag) (every component? (string-split (cadr x) "/")))
             ((commit) (oid? (cadr x)))
             (else #f))))

    (define (package-selector-clause? x)
      (and (list? x) (= 2 (length x)) (eq? (car x) 'package)
           (package-name? (cadr x)) (not (reserved-namespace? (cadr x)))))

    (define (git-source-valid? src)
      (and (list? src) (memv (length src) '(2 3 4)) (string? (cadr src))
           (case (car src)
             ((github) (let ((parts (string-split (cadr src) "/")))
                         (and (= 2 (length parts)) (every component? parts))))
             ((git) (https-url? (cadr src)))
             ((local) (and (positive? (string-length (cadr src))) (path-absolute? (cadr src))))
             (else #f))
           (let* ((rest (cddr src))
                  (pkgs (filter (lambda (e) (and (pair? e) (eq? (car e) 'package))) rest))
                  (refs (remove (lambda (e) (and (pair? e) (eq? (car e) 'package))) rest)))
             (and (<= (length pkgs) 1)
                  (<= (length refs) 1)
                  (every package-selector-clause? pkgs)
                  (every git-ref-clause? refs)))))

    (define (path-source-valid? src)
      (and (list? src) (= 2 (length src)) (eq? 'path (car src))
           (string? (cadr src)) (positive? (string-length (cadr src)))
           (not (path-absolute? (cadr src)))))

    (define (package-source-datum? src)
      "Return whether SRC is a valid dependency or override source datum."
      (or (path-source-valid? src) (git-source-valid? src)))

    (define (source-selected-package src)
      (and (pair? src) (not (eq? (car src) 'path))
           (let ((found (assq 'package (cddr src))))
             (and found (cadr found)))))

    (define (relative? path)
      (and (string? path) (every component? (string-split path "/"))))

    ;; Files are reached by the authority to load (`%package-manifest`,
    ;; `%loadable-path`), which a world deciding what it loads has: one
    ;; without authority over files uses packages too.

    ;; RELATIVE under ROOT, links resolved: it is an error if nothing is
    ;; there, or if it leaves ROOT.
    (define (contained root relative)
      (unless (relative? relative) (bad "expected a path inside the package" relative))
      (let ((p (or (%loadable-path (path-join root relative)) (bad "nothing at" relative))))
        (unless (string-prefix? (string-append root "/") p) (bad "path leaves the package" relative))
        p))

    (define (one-datum text)
      (let* ((port (open-input-string text))
             (d (read port)))
        (unless (and (not (eof-object? d)) (eof-object? (read port))) (bad "expected one datum"))
        d))

    (define (distinct! what names)
      (unless (= (length names) (length (delete-duplicates names))) (bad what names)))

    (define (version-clause what clause)
      (unless (and (list? clause) (= 1 (length clause)) (version-string? (car clause)))
        (bad (string-append "expected (" what " \"version\")") clause))
      (car clause))

    (define (description-clause clause)
      (unless (and (list? clause) (= 1 (length clause)) (string? (car clause)) (positive? (string-length (car clause))))
        (bad "expected (description \"...\")" clause))
      (car clause))

    (define (source-entries what name clause)
      (let ((entries
             (map (lambda (d)
                    (unless (and (list? d) (= 2 (length d)) (package-name? (car d))
                                 (not (reserved-namespace? (car d)))
                                 (package-source-datum? (cadr d)))
                      (bad (string-append "expected a " what " ((owner pkg) <source>)") d))
                    (when (equal? (car d) name)
                      (bad (string-append "a package cannot " what " itself") name))
                    (let ((selected (source-selected-package (cadr d))))
                      (when (and selected (not (equal? selected (car d))))
                        (bad (string-append what " source selects another package than named") (car d) selected)))
                    (cons (car d) (cadr d)))
                  clause)))
        (distinct! (string-append what " declared twice") (map car entries))
        entries))

    (define (dependencies-from-sources root sources)
      (map (lambda (entry)
             (let ((dep (car entry))
                   (src (cdr entry)))
               (if (eq? 'path (car src))
                   (cons dep (path-join root (cadr src)))
                   (cons dep src))))
           sources))

    (define (editor root clause)
      (unless (and (pair? clause) (string? (car clause))) (bad "expected (editor \"entry.scm\" (commands ...) [(options ...)])"))
      (let* ((parts (cdr clause))
             (part (lambda (head)
                     (let ((found (filter (lambda (p) (and (pair? p) (eq? (car p) head))) parts)))
                       (when (pair? (if (pair? found) (cdr found) '())) (bad "given twice" head))
                       (and (pair? found) (cdar found)))))
             (commands (part 'commands))
             (options (or (part 'options) '())))
        (unless (every (lambda (p) (and (pair? p) (list? p) (memq (car p) '(commands options)))) parts)
          (bad "expected (commands ...) and (options ...) after the editor's entry"))
        (unless (and (list? commands) (pair? commands)
                     (every (lambda (c) (and (list? c) (= 2 (length c)) (symbol? (car c)) (string? (cadr c)))) commands))
          (bad "expected commands, each (NAME \"documentation\"), at least one"))
        (unless (and (list? options)
                     (every (lambda (o) (and (list? o) (= 4 (length o)) (symbol? (car o)) (string? (cadddr o))
                                             (schema? (caddr o)) (valid? (caddr o) (cadr o))))
                            options))
          (bad "expected options, each (NAME DEFAULT SCHEMA \"documentation\")"))
        (distinct! "command declared twice" (map car commands))
        (distinct! "option declared twice" (map car options))
        (list (contained root (car clause)) commands options)))

    (define (cloud root clause)
      (unless (and (pair? clause) (string? (car clause)))
        (bad "expected (cloud \"entry.scm\" [(bindings (name kind) ...)])"))
      (let* ((parts (cdr clause))
             (found (filter (lambda (p) (and (pair? p) (eq? (car p) 'bindings))) parts)))
        (unless (every (lambda (p) (and (pair? p) (list? p) (eq? (car p) 'bindings))) parts)
          (bad "expected (bindings ...) after the cloud entry"))
        (when (and (pair? found) (pair? (cdr found))) (bad "given twice" 'bindings))
        (let ((bindings (if (pair? found) (cdar found) '())))
          (unless (every (lambda (b)
                           (and (list? b) (= 2 (length b)) (symbol? (car b))
                                (symbol? (cadr b)) (memq (cadr b) '(kv secret text))))
                         bindings)
            (bad "expected bindings, each (NAME kv|secret|text)"))
          (distinct! "binding declared twice" (map car bindings))
          (list (contained root (car clause)) bindings))))

    (define (read-package-manifest directory)
      "Return the manifest of the package in DIRECTORY, read as data.
Nothing in it is evaluated and no other file is read: its entries must
exist inside the package, its dependencies need not be there yet. It is
an error if the manifest is missing or is not one."
      (let* ((found (or (%package-manifest directory) (bad "none to read in" directory)))
             (root (car found))
             (d (one-datum (cadr found))))
        (unless (and (list? d) (pair? d) (pair? (cdr d)) (eq? (car d) 'package) (package-name? (cadr d)))
          (bad "expected (package (owner pkg) clause ...)"))
        (let* ((name (cadr d))
               (_ (when (reserved-namespace? name)
                    (bad "a package cannot take a reserved library namespace" name)))
               ;; Shorthand form: (package (owner pkg) ENTRY COMMANDS OPTIONS).
               (clauses (if (and (pair? (cddr d)) (string? (caddr d)))
                            (if (and (= 5 (length d)) (list? (cadddr d)) (list? (list-ref d 4)))
                                `((editor ,(caddr d) (commands ,@(cadddr d)) (options ,@(list-ref d 4))))
                                (bad "expected (package (owner pkg) \"entry.scm\" (COMMAND ...) (OPTION ...))"))
                            (cddr d)))
               (clause (lambda (head)
                         (let ((found (assq head clauses))) (and found (cdr found))))))
          (unless (every (lambda (c) (and (pair? c) (list? c) (memq (car c) '(version techne description depends override editor cloud)))) clauses)
            (bad "unknown manifest clause" (map (lambda (c) (if (pair? c) (car c) c)) clauses)))
          (distinct! "clause given twice" (map car clauses))
          (let* ((dep-sources (source-entries "dependency" name (or (clause 'depends) '())))
                 (deps (dependencies-from-sources root dep-sources))
                 (overrides (source-entries "override" name (or (clause 'override) '()))))
            (%manifest name root
                       (and (clause 'version) (version-clause "version" (clause 'version)))
                       (and (clause 'techne) (version-clause "techne" (clause 'techne)))
                       (and (clause 'description) (description-clause (clause 'description)))
                       deps
                       dep-sources
                       overrides
                       (and (clause 'editor) (editor root (clause 'editor)))
                       (and (clause 'cloud) (cloud root (clause 'cloud))))))))

    ;; Read the raw bytes of PATH into a single bytevector.
    (define (read-file-bytes path)
      (let ((port (open-binary-input-file path)))
        (dynamic-wind
          (lambda () #f)
          (lambda ()
            (let ((out (open-output-bytevector)))
              (let loop ()
                (let ((chunk (read-bytevector 65536 port)))
                  (unless (eof-object? chunk)
                    (write-bytevector chunk out)
                    (loop))))
              (get-output-bytevector out)))
          (lambda () (close-input-port port)))))

    (define (directory-tree-sha256 directory)
      "Return the canonical 64-hex SHA-256 tree digest of DIRECTORY.
Regular files are walked recursively in sorted UTF-8 relative POSIX path
order, excluding hidden files and directories whose path component begins
with `.` (including `.git`, `.jj`, `.secrets.scm`, and `.DS_Store`).
Symlinks and non-regular files are rejected. Each regular file is hashed
as \"F <relpath>\\0<byte-len>\\0<file-bytes>\\n\"."
      (let* ((root (file-truename directory))
             (attrs (file-attributes root)))
        (unless (and attrs (eq? 'directory (car attrs)))
          (error "Not a directory" directory))
        (let ((files '()))
          (let walk ((dir root) (prefix ""))
            (for-each
              (lambda (raw)
                (let ((name (if (string-suffix? "/" raw)
                                (substring raw 0 (- (string-length raw) 1))
                                raw)))
                  (unless (string-prefix? "." name)
                    (let* ((rel (if (string=? prefix "") name (string-append prefix "/" name)))
                           (full (path-join dir name))
                           (a (file-attributes full))
                           (kind (and a (car a))))
                      (cond ((eq? kind 'symlink)
                             (error "Symlink not allowed in package tree" rel))
                            ((eq? kind 'directory)
                             (walk full rel))
                            ((eq? kind 'file)
                             (set! files (cons (cons rel full) files)))
                            (else
                             (error "Unsupported file type in package tree" rel)))))))
              (directory-list dir)))
          (let ((sorted (sort files (lambda (a b) (string<? (car a) (car b)))))
                (out (open-output-bytevector)))
            (for-each
              (lambda (entry)
                (let* ((rel (car entry))
                       (bytes (read-file-bytes (cdr entry))))
                  (write-bytevector (string->utf8 "F ") out)
                  (write-bytevector (string->utf8 rel) out)
                  (write-u8 0 out)
                  (write-bytevector (string->utf8 (number->string (bytevector-length bytes))) out)
                  (write-u8 0 out)
                  (write-bytevector bytes out)
                  (write-u8 10 out)))
              sorted)
            (hex-encode (sha-256 (get-output-bytevector out)))))))

    (define (package-tree-sha256 directory)
      "Return the canonical `tree-sha256` digest of the package in DIRECTORY.
This is an alias for `directory-tree-sha256`."
      (directory-tree-sha256 directory))

    ;; The manifest of each directory used.
    (define used (make-hash-table equal?))
    ;; The directory each package name was used from.
    (define names (make-hash-table equal?))

    ;; Where the libraries of the package of manifest M are, or would be.
    (define (library-directory m)
      (let ((lib (path-join (manifest-directory m) "lib")))
        (or (%loadable-path lib) lib)))

    (define (register-package-world! manifests)
      "Register MANIFESTS (a list of resolved package manifests) atomically.
All conflicts are checked before anything changes: reserved namespaces,
duplicate package identities pointing to different directories, and VM
library provider collisions. Each manifest's libraries and declared
direct imports are then wired into the VM."
      (let ((batch (make-hash-table equal?)))
        (for-each
          (lambda (m)
            (let* ((name (manifest-name m))
                   (dir (manifest-directory m))
                   (existing (or (hash-table-ref/default names name #f)
                                 (hash-table-ref/default batch name #f)))
                   (conflict (%library-conflict name (library-directory m))))
              (when (reserved-namespace? name)
                (error "A package cannot take a reserved library namespace" name))
              (when (and existing (not (equal? existing dir)))
                (error "Another directory holds a package of this name" name existing dir))
              (when conflict
                (error (string-append "A package's libraries cannot be found in its directory: " conflict) name dir))
              (hash-table-set! batch name dir)))
          manifests)
        (for-each
          (lambda (m)
            (let ((dir (manifest-directory m))
                  (name (manifest-name m)))
              (%provide-libraries! name (library-directory m))
              (%package-imports! dir name (cons name (map car (manifest-dependency-sources m))))
              (hash-table-set! names name dir)
              (hash-table-set! used dir m)))
          manifests)
        manifests))

    ;; The manifests of the package in ROOT and of those it depends on
    ;; that are not used yet, each after the ones it depends on. Everything
    ;; is checked here and nothing changed: a package set that cannot be
    ;; used leaves no part of itself behind. With WITHIN, a directory, none
    ;; of them may be outside it.
    (define (unused-closure root within)
      (let ((found '())
            ;; The packages known already whose dependencies were looked
            ;; at: once each, however many depend on them.
            (walked (make-hash-table equal?)))
        (define (check-within! root)
          (unless (or (not within) (equal? root within) (string-prefix? (string-append within "/") root))
            (error "A package depends on one outside its repository" root within)))
        (define (manifest-at root)
          (or (hash-table-ref/default used root #f)
              (let ((new (assoc root found))) (and new (cdr new)))))
        ;; ABOVE: the packages depending on this one that are being
        ;; visited, each (directory . name), not in FOUND yet.
        (define (check-name! m above)
          (let* ((name (manifest-name m))
                 (dir (manifest-directory m))
                 (same (filter (lambda (entry) (equal? name (manifest-name (cdr entry)))) found))
                 (visiting (filter (lambda (entry) (equal? name (cdr entry))) above))
                 (other (or (hash-table-ref/default names name #f) (and (pair? same) (caar same))
                            (and (pair? visiting) (caar visiting))))
                 ;; Asked now: refused when the libraries are provided, a
                 ;; set of packages would be left half made known.
                 (conflict (%library-conflict name (library-directory m))))
            (when (reserved-namespace? name)
              (error "A package cannot take a reserved library namespace" name))
            (when (and other (not (equal? other dir)))
              (error "Another directory holds a package of this name" name other dir))
            (when conflict
              (error (string-append "A package's libraries cannot be found in its directory: " conflict) name dir))))
        (define (visit root above)
          (when (assoc root above)
            (error "Packages depend on each other in a circle" (reverse (cons root (map car above)))))
          (check-within! root)
          ;; One known already was not held to WITHIN when it was made
          ;; known: what it depends on is looked at now.
          (let ((known (hash-table-ref/default used root #f)))
            (when (and known within (not (hash-table-ref/default walked root #f)))
              (for-each (lambda (dependency)
                          (when (string? (cdr dependency))
                            (let ((dir (%loadable-path (cdr dependency))))
                              (when dir (visit dir (cons (cons root (manifest-name known)) above))))))
                        (manifest-dependencies known))
              (hash-table-set! walked root #t)))
          (or (manifest-at root)
              (let ((m (read-package-manifest root)))
                (check-name! m above)
                (for-each (lambda (dependency)
                            (unless (string? (cdr dependency))
                              (error "Git dependency requires techne.lock (run `techne lock` and `techne run --locked`)"
                                     (car dependency) (cdr dependency)))
                            (unless (%loadable-path (path-join (cdr dependency) package-manifest-file))
                              (error "No package where a dependency says" (car dependency) (cdr dependency)))
                            (let ((name (manifest-name (visit (%loadable-path (cdr dependency))
                                                              (cons (cons root (manifest-name m)) above)))))
                              (unless (equal? name (car dependency))
                                (error "A dependency names another package than its directory holds" (car dependency) name))))
                          (manifest-dependencies m))
                (set! found (cons (cons root m) found))
                m)))
        (visit root '())
        (reverse (map cdr found))))

    (define (resolved directory)
      (or (%loadable-path directory) (error "No package directory at" directory)))

    (define (package-closure directory #:within [within #f])
      "Return the manifests `use-package-directory!` would make known.
They are those of the package in DIRECTORY and of the packages it
depends on that are not known yet, each after the ones it depends on:
none, if DIRECTORY's is known. Everything that procedure checks is
checked, WITHIN as it takes it, and nothing is changed."
      (let ((root (resolved directory)))
        (if (and (hash-table-ref/default used root #f) (not within))
            '()
            (unused-closure root (and within (resolved within))))))

    (define (use-package-directory! directory #:within [within #f])
      "Make the package in DIRECTORY known here; return its manifest.
Its libraries, those under its `lib` directory, whose names begin with
its own `(owner pkg)`, are found there from now on and nowhere else,
and so are those of the packages it depends on, and of theirs. Its files
import, of the libraries packages provide, only its own and those of the
packages it depends on itself: what it needs is what its manifest says.
No code is loaded. It is an error, and nothing changes, if another
directory holds a package of the same name, if a dependency is missing
or is another package than the one named, if packages depend on each
other in a circle, or, with WITHIN, a directory, if one of them is
outside it (a repository's packages depend on each other, not on what is
beside it)."
      (let ((root (resolved directory)))
        ;; Nothing to make known of a package known already, which is
        ;; held to WITHIN all the same.
        (register-package-world! (package-closure root #:within within))
        (hash-table-ref/default used root #f)))

    (define (package-directories root)
      "Return the directories under ROOT holding a package, relative to it.
ROOT's own is \"\", and then the only one: a package's directory is not
searched for others. Directories whose names begin with a dot and
symbolic links are not searched, nor those more than four below ROOT."
      (let walk ((relative "") (depth 0))
        (let* ((dir (path-join root relative))
               (manifest-attrs (file-attributes (path-join dir package-manifest-file))))
          (cond ((and manifest-attrs (eq? (car manifest-attrs) 'file)) (list relative))
                ((= depth 4) '())
                (else
                 (append-map (lambda (entry)
                               (let ((name (if (string-suffix? "/" entry)
                                               (substring entry 0 (- (string-length entry) 1))
                                               entry)))
                                 (if (not (string-prefix? "." name))
                                     (let ((attrs (file-attributes (path-join dir name))))
                                       (if (and attrs (eq? (car attrs) 'directory))
                                           (walk (path-join relative name) (+ depth 1))
                                           '()))
                                     '())))
                             (directory-list dir)))))))))
