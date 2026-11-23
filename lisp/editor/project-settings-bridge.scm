;;; Lazy bridge to `(techne project-settings)` and `(techne editorconfig)`.
;;; Loaded on first use so editor startup and keystrokes without project
;;; settings pay zero parse/load cost.

(import (techne project-settings params))
(require "session.scm")
(require "operations.scm")

(provide project-settings-loaded? ensure-project-settings! ps-ref
         default-trust-store-path
         json-unset json-unset?
         json-object-strip-unset json-object-overlay json-object-compose-overlays
         value-trust-digest
         make-settings-context settings-context?
         settings-context-directory settings-context-path
         settings-context-modes settings-context-snapshot
         project-settings-generation project-settings-canonical-dir
         project-settings-watched-path?
         project-settings-load! project-settings-cached-snapshot
         project-settings-invalidate! project-settings-invalidate-ancestors!
         project-settings-snapshot-directory
         project-settings-snapshot-diagnostics
         project-settings-resolve-option
         project-settings-collect-pending project-settings-collect-all-pending
         project-trust-verdict project-trust-entries project-trust-clear-memory!
         project-trust-approve! project-trust-deny! project-trust-revoke!)

(define %ps-env #f)
(define %tp-clean? #f)
(define %tp-env-var #f)
(define %tp-default-store-path #f)

(define (trim-trailing-slash base)
  (if (and (> (string-length base) 1) (string-suffix? "/" base))
      (trim-trailing-slash (substring base 0 (- (string-length base) 1)))
      base))

(define (ensure-trusted-process!)
  (unless %tp-clean?
    (let* ((tp (environment '(techne trusted-process)))
           (clean? (eval 'trusted-baseline-clean? tp))
           (env-var (eval 'trusted-environment-variable tp)))
      (set! %tp-clean? clean?)
      (set! %tp-env-var env-var)
      (set! %tp-default-store-path
            (if (not (clean?))
                'untrusted-baseline
                (let* ((xdg (env-var "XDG_STATE_HOME"))
                       (home (env-var "HOME"))
                       (base (cond ((and (string? xdg) (string-prefix? "/" xdg))
                                    (trim-trailing-slash xdg))
                                   ((and (string? home) (string-prefix? "/" home))
                                    (string-append (trim-trailing-slash home) "/.local/state"))
                                   (else #f))))
                  (if base
                      (string-append base "/techne/project-trust.scm")
                      'untrusted-baseline)))))))

(define (trusted-trust-store-path)
  ;; Derive the trust ledger from a verified clean host baseline.
  ;; Zero-I/O after initial baseline resolution in %init-ps-env!.
  ;; Returns 'untrusted-baseline when a startup direnv overlay could not be
  ;; cleanly unloaded or no trusted user state root can be established.
  (ensure-trusted-process!)
  (if (not (%tp-clean?))
      'untrusted-baseline
      %tp-default-store-path))

(define %init-ps-env!
  (scope-procedure
   (lambda ()
     (or %ps-env
         (let ((e (environment '(techne project-settings))))
           (ensure-trusted-process!)
           ((eval 'set-trust-store-path-hook! e) trusted-trust-store-path)
           ((eval 'set-trust-principal-hook! e)
            (lambda ()
              (guard (err (#t #f))
                (and (root-authority?)
                     (let ((who (current-principal)))
                       (and (equal? who "user") "user"))))))
           (let ((p (trusted-trust-store-path)))
             (when (string? p)
               (guard (err (#t #f)) ((eval 'project-trust-load! e) p))))
           (set! %ps-env e)
           e)))))

(define (project-settings-loaded?)
  "Return #t if `(techne project-settings)` is loaded in this runtime."
  (and %ps-env #t))

(define (ensure-project-settings!)
  "Load `(techne project-settings)` with editor trust hooks wired."
  (or %ps-env (%init-ps-env!)))

(define (ps-ref name)
  "Return binding NAME from the lazy `(techne project-settings)` env."
  (eval name (ensure-project-settings!)))

(define (path-basename path)
  (let ((n (string-length path)))
    (let loop ((i (- n 1)))
      (cond ((< i 0) path)
            ((char=? (string-ref path i) #\/) (substring path (+ i 1) n))
            (else (loop (- i 1)))))))

(define (project-settings-watched-path? path)
  "Return #t if PATH is a `.techne.scm` or `.editorconfig` file."
  (and (string? path)
       (let ((name (path-basename path)))
         (or (string=? name ".techne.scm")
             (string=? name ".editorconfig")))))

(define (project-settings-generation)
  "Return the monotonic project-settings cache generation number."
  (if %ps-env ((ps-ref 'project-settings-generation)) 0))

(define (project-settings-canonical-dir dir)
  "Return the canonical directory path for DIR with a trailing slash."
  ((ps-ref 'project-settings-canonical-dir) dir))

(define (project-settings-load! dir)
  "Load and cache the project-settings snapshot for directory DIR."
  ((ps-ref 'project-settings-load!) dir))

(define (project-settings-cached-snapshot dir)
  "Return the cached project-settings snapshot for DIR without I/O, or #f."
  (and %ps-env ((ps-ref 'project-settings-cached-snapshot) dir)))

(define (project-settings-invalidate! [dir #f])
  "Invalidate cached project-settings state for DIR, or all when `#f`."
  (when %ps-env ((ps-ref 'project-settings-invalidate!) dir)))

(define (project-settings-invalidate-ancestors! dir)
  "Invalidate cached project-settings state for DIR and its ancestors."
  (when %ps-env ((ps-ref 'project-settings-invalidate-ancestors!) dir)))

(define (make-settings-context dir #:path [path #f] #:modes [modes '()] #:load? [load? #t])
  "Return a `settings-context` for DIR with optional PATH, MODES, LOAD?."
  ((ps-ref 'make-settings-context) dir #:path path #:modes modes #:load? load?))

(define (settings-context? x)
  "Return #t if X is a `settings-context`."
  (and %ps-env ((ps-ref 'settings-context?) x)))

(define (settings-context-directory ctx)
  "Return the directory of settings context CTX."
  ((ps-ref 'settings-context-directory) ctx))

(define (settings-context-path ctx)
  "Return the optional file path of settings context CTX."
  ((ps-ref 'settings-context-path) ctx))

(define (settings-context-modes ctx)
  "Return the mode-chain list of settings context CTX."
  ((ps-ref 'settings-context-modes) ctx))

(define (settings-context-snapshot ctx)
  "Return the snapshot attached to settings context CTX, or #f."
  ((ps-ref 'settings-context-snapshot) ctx))

(define (project-settings-snapshot-directory snap)
  "Return the canonical directory of project-settings snapshot SNAP."
  ((ps-ref 'project-settings-snapshot-directory) snap))

(define (project-settings-snapshot-diagnostics snap)
  "Return the diagnostic records in snapshot SNAP."
  ((ps-ref 'project-settings-snapshot-diagnostics) snap))

(define (project-settings-resolve-option snap file-path mode-chain user-entries option-name
                                         #:known? [known? #t]
                                         #:user-only? [user-only? #f]
                                         #:safe [safe #f]
                                         #:valid? [valid? (lambda (v) #t)]
                                         #:merge [merge 'replace])
  "Resolve OPTION-NAME from SNAP, FILE-PATH, MODE-CHAIN, and USER-ENTRIES.
Honors KNOWN?, USER-ONLY?, SAFE, VALID?, and MERGE."
  ((ps-ref 'project-settings-resolve-option) snap file-path mode-chain user-entries option-name
                                             #:known? known?
                                             #:user-only? user-only?
                                             #:safe safe
                                             #:valid? valid?
                                             #:merge merge))

(define (project-settings-collect-pending snap file-path mode-chain lookup-meta)
  "Return pending untrusted settings for SNAP, FILE-PATH, MODE-CHAIN.
LOOKUP-META maps option names to metadata."
  ((ps-ref 'project-settings-collect-pending) snap file-path mode-chain lookup-meta))

(define (project-settings-collect-all-pending snap lookup-meta)
  "Return all pending untrusted settings across selectors in SNAP.
LOOKUP-META maps option names to metadata."
  ((ps-ref 'project-settings-collect-all-pending) snap lookup-meta))

(define (project-trust-verdict scope option-name effective-value)
  "Return trust verdict for OPTION-NAME and EFFECTIVE-VALUE in SCOPE."
  ((ps-ref 'project-trust-verdict) scope option-name effective-value))

(define (project-trust-entries)
  "Return all stored trust entries as `(SCOPE OPTION DIGEST VERDICT)`."
  ((ps-ref 'project-trust-entries)))

(define (project-trust-clear-memory!)
  "Clear in-memory trust verdicts and reset the loaded store path."
  (unless (root-authority?)
    (error "project-trust-clear-memory!: may only be called with root authority"
           (scope-name (authority-scope))))
  (when %ps-env ((ps-ref 'project-trust-clear-memory!))))

(define (project-trust-approve! scope option-name effective-value)
  "Approve EFFECTIVE-VALUE for OPTION-NAME in SCOPE (user principal only)."
  ((ps-ref 'project-trust-approve!) scope option-name effective-value))

(define (project-trust-deny! scope option-name effective-value)
  "Deny EFFECTIVE-VALUE for OPTION-NAME in SCOPE (user principal only)."
  ((ps-ref 'project-trust-deny!) scope option-name effective-value))

(define (value-trust-digest scope option-name effective-value)
  "Return the canonical SHA-256 trust digest for OPTION-NAME in SCOPE.
Hashes EFFECTIVE-VALUE with SCOPE and OPTION-NAME."
  ((ps-ref 'value-trust-digest) scope option-name effective-value))

(define (project-trust-revoke! [scope #f] [option-name #f])
  "Revoke stored trust verdicts matching SCOPE and OPTION-NAME."
  ((ps-ref 'project-trust-revoke!) scope option-name))
