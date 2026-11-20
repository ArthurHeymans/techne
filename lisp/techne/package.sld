;;; A package's directory: its manifest, `techne-package.scm`, one datum
;;; read and never evaluated; the libraries it provides, under `lib/`; and
;;; the packages it depends on. Nothing here needs the editor: a program
;;; run from a package's directory (`techne run`) and the editor's
;;; packages resolve their libraries the same way.
;;;
;;;   (package mail
;;;     (depends (authinfo (path "../authinfo")))
;;;     (editor "editor.scm"
;;;       (commands (mail "Open the mail dashboard."))
;;;       (options (mail-prefer-html #t boolean "Whether to show HTML."))))
;;;
;;; The package NAME owns the libraries whose names begin with it: `(mail
;;; imap)` is `lib/mail/imap.sld` in its directory and nowhere else. They
;;; are the world's, one instance whoever imports them. `depends` names the
;;; packages whose libraries this one imports, each by the directory it is
;;; in, relative to this one. `editor` is what the editor loads on first
;;; use (doc/packages.md): the entry file, the commands advertised before
;;; that and the options declared early, as (name default schema
;;; documentation).

(define-library (techne package)
  (export package-manifest-file read-package-manifest package-manifest? manifest-name manifest-directory
          manifest-dependencies manifest-editor use-package-directory! package-closure package-directories)
  (import (techne) (techne path) (techne schema))
  (begin

    (define package-manifest-file "techne-package.scm"
      "The name of the file in a package's directory holding its manifest.")

    (define-record-type manifest
      (%manifest name directory dependencies editor)
      package-manifest?
      (name manifest-name)
      (directory manifest-directory)
      (dependencies manifest-dependencies)
      (editor manifest-editor))

    (define (bad what . irritants)
      (apply error (string-append package-manifest-file ": " what) irritants))

    ;; A name that is one path component and nothing a shell or a path
    ;; could take otherwise.
    (define (component? s)
      (and (string? s) (positive? (string-length s)) (not (member s '("." "..")))
           (every (lambda (c) (or (char-alphabetic? c) (char-numeric? c) (memv c '(#\- #\_ #\.)))) (string->list s))))

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

    (define (dependencies root name clause)
      (map (lambda (d)
             (unless (and (list? d) (= 2 (length d)) (symbol? (car d)) (component? (symbol->string (car d)))
                          (list? (cadr d)) (= 2 (length (cadr d))) (eq? 'path (car (cadr d)))
                          (string? (cadr (cadr d))) (not (path-absolute? (cadr (cadr d)))))
               (bad "expected a dependency (NAME (path \"relative/directory\"))" d))
             (when (eq? (car d) name) (bad "a package cannot depend on itself" name))
             (cons (car d) (path-join root (cadr (cadr d)))))
           clause))

    (define (editor root clause)
      (unless (and (pair? clause) (string? (car clause))) (bad "expected (editor \"entry.scm\" (commands ...) [(options ...)])"))
      (let* ((parts (cdr clause))
             (part (lambda (head)
                     (let ((found (filter (lambda (p) (and (pair? p) (eq? (car p) head))) parts)))
                       (when (pair? (if (pair? found) (cdr found) '())) (bad "given twice" head))
                       (and (pair? found) (cdar found)))))
             (commands (part 'commands))
             (options (or (part 'options) '())))
        (unless (every (lambda (p) (and (pair? p) (memq (car p) '(commands options)))) parts)
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

    (define (read-package-manifest directory)
      "Return the manifest of the package in DIRECTORY, read as data.
Nothing in it is evaluated and no other file is read: its entry must
exist inside the package, its dependencies need not be there yet. It
is an error if the manifest is missing or is not one."
      (let* ((found (or (%package-manifest directory) (bad "none to read in" directory)))
             (root (car found))
             (d (one-datum (cadr found))))
        (unless (and (list? d) (pair? d) (pair? (cdr d)) (eq? (car d) 'package) (symbol? (cadr d))
                     (component? (symbol->string (cadr d))))
          (bad "expected (package NAME clause ...)"))
        (let* ((name (cadr d))
               ;; As first written: (package NAME ENTRY COMMANDS OPTIONS).
               (clauses (if (and (pair? (cddr d)) (string? (caddr d)))
                            (if (and (= 5 (length d)) (list? (cadddr d)) (list? (list-ref d 4)))
                                `((editor ,(caddr d) (commands ,@(cadddr d)) (options ,@(list-ref d 4))))
                                (bad "expected (package NAME \"entry.scm\" (COMMAND ...) (OPTION ...))"))
                            (cddr d)))
               (clause (lambda (head)
                         (let ((found (assq head clauses))) (and found (cdr found))))))
          (unless (every (lambda (c) (and (pair? c) (list? c) (memq (car c) '(depends editor)))) clauses)
            (bad "expected (depends ...) and (editor ...) clauses" (map (lambda (c) (if (pair? c) (car c) c)) clauses)))
          (distinct! "clause given twice" (map car clauses))
          (let ((depends (dependencies root name (or (clause 'depends) '()))))
            (distinct! "dependency declared twice" (map car depends))
            (%manifest name root depends (and (clause 'editor) (editor root (clause 'editor))))))))

    ;; The manifest of each directory used.
    (define used (make-hash-table equal?))
    ;; The directory each package name was used from.
    (define names (make-hash-table eq?))

    ;; The manifests of the package in ROOT and of those it depends on
    ;; that are not used yet, each after the ones it depends on. Everything
    ;; is checked here and nothing changed: a package set that cannot be
    ;; used leaves no part of itself behind. With WITHIN, a directory, none
    ;; of them may be outside it.
    (define (unused-closure root within)
      (let ((found '()))
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
                 (same (filter (lambda (entry) (eq? name (manifest-name (cdr entry)))) found))
                 (visiting (filter (lambda (entry) (eq? name (cdr entry))) above))
                 (other (or (hash-table-ref/default names name #f) (and (pair? same) (caar same))
                            (and (pair? visiting) (caar visiting))))
                 ;; Asked now: refused when the libraries are provided, a
                 ;; set of packages would be left half made known.
                 (conflict (%library-conflict (symbol->string name) (library-directory m))))
            (when (memq name '(scheme srfi techne))
              (error "A package cannot take the name of Techne's own libraries" name))
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
            (when (and known within)
              (for-each (lambda (dependency)
                          (let ((dir (%loadable-path (cdr dependency))))
                            (when dir (visit dir (cons (cons root (manifest-name known)) above)))))
                        (manifest-dependencies known))))
          (or (manifest-at root)
              (let ((m (read-package-manifest root)))
                (check-name! m above)
                (for-each (lambda (dependency)
                            (unless (%loadable-path (path-join (cdr dependency) package-manifest-file))
                              (error "No package where a dependency says" (car dependency) (cdr dependency)))
                            (let ((name (manifest-name (visit (%loadable-path (cdr dependency))
                                                              (cons (cons root (manifest-name m)) above)))))
                              (unless (eq? name (car dependency))
                                (error "A dependency names another package than its directory holds" (car dependency) name))))
                          (manifest-dependencies m))
                (set! found (cons (cons root m) found))
                m)))
        (visit root '())
        (reverse (map cdr found))))

    ;; Where the libraries of the package of manifest M are, or would be.
    (define (library-directory m)
      (let ((lib (path-join (manifest-directory m) "lib")))
        (or (%loadable-path lib) lib)))

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
its own, are found there from now on and nowhere else, and so are those
of the packages it depends on, and of theirs. Its files import, of the
libraries packages provide, only its own and those of the packages it
depends on itself: what it needs is what its manifest says. No code is
loaded. It is an error, and nothing changes, if another directory holds
a package of the same name, if a dependency is missing or is another
package than the one named, if packages depend on each other in a
circle, or, with WITHIN, a directory, if one of them is outside it (a
repository's packages depend on each other, not on what is beside it)."
      (let ((root (resolved directory)))
        ;; Nothing to make known of a package known already, which is
        ;; held to WITHIN all the same.
        (for-each (lambda (m)
                    (let ((dir (manifest-directory m))
                          (name (manifest-name m)))
                      (%provide-libraries! (symbol->string name) (library-directory m))
                      (%package-imports! dir (symbol->string name)
                                         (map symbol->string (cons name (map car (manifest-dependencies m)))))
                      (hash-table-set! names name dir)
                      (hash-table-set! used dir m)))
                  (package-closure root #:within within))
        (hash-table-ref/default used root #f)))

    (define (package-directories root)
      "Return the directories under ROOT holding a package, relative to it.
ROOT's own is \"\", and then the only one: a package's directory is not
searched for others. Directories whose names begin with a dot are not
searched, nor those more than four below ROOT."
      (let walk ((relative "") (depth 0))
        (let ((dir (path-join root relative)))
          (cond ((file-exists? (path-join dir package-manifest-file)) (list relative))
                ((= depth 4) '())
                (else
                 (append-map (lambda (entry)
                               (if (and (string-suffix? "/" entry) (not (string-prefix? "." entry)))
                                   (walk (path-join relative (substring entry 0 (- (string-length entry) 1))) (+ depth 1))
                                   '()))
                             (directory-list dir)))))))))
