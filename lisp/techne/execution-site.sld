;;; Execution sites and environment providers (`direnv` and extensions).
;;; Unifies where and under which launch policy (`project`,
;;; `endpoint-base`, or `user-trusted`) external processes execute.

(define-library (techne execution-site)
  (export make-execution-site execution-site? execution-site-for
          execution-site-location execution-site-route execution-site-directory
          execution-site-policy execution-site-environment execution-site-endpoint
          make-env-snapshot env-snapshot? env-snapshot-provider env-snapshot-status
          env-snapshot-root env-snapshot-rc-path env-snapshot-delta
          env-snapshot-message env-snapshot-loaded-at
          define-env-provider find-env-provider env-provider-names
          default-env-providers
          default-direnv-enabled set-direnv-enabled-hook!
          default-direnv-program set-direnv-program-hook!
          default-direnv-data-dir set-direnv-data-dir-hook!
          set-direnv-sync-dir-hook!
          default-direnv-timeout default-direnv-status-timeout
          execution-site-env-snapshot execution-site-cached-snapshot
          execution-site-resolved-environment execution-site-find-program
          execution-site-has-env-config? execution-site-needs-eval?
          execution-site-watched-path? execution-site-invalidate!
          execution-site-direnv-target execution-site-direnv-file-digest
          execution-site-direnv-allow! execution-site-direnv-deny!
          spawn-at-site call-with-process-at-site)
  (import (techne) (techne path) (techne location) (techne endpoint)
          (techne trusted-process) (techne task) (techne crypto))
  (begin
    (define %default-max-stdout 4194304)
    (define %max-stderr-kept 4096)

    (define %spawn-in-root-scope
      (scope-procedure (lambda (thunk) (spawn thunk))))

    ;;; Records

    (define-record-type execution-site
      (%make-execution-site location route directory policy environment)
      execution-site?
      (location execution-site-location)
      (route execution-site-route)
      (directory execution-site-directory)
      (policy execution-site-policy)
      (environment execution-site-environment))

    (define (valid-policy? policy)
      (and (memq policy '(project endpoint-base user-trusted)) #t))

    (define (make-execution-site location #:policy [policy 'project] #:environment [environment '()])
      "Construct an execution site for LOCATION under POLICY and ENVIRONMENT.
POLICY is `project` (project directory plus environment overlay),
`endpoint-base` (directory with clean host/endpoint base environment),
or `user-trusted` (local trusted working directory, no project overlay).
ENVIRONMENT is an optional list of `(NAME . VALUE-OR-#f)` overrides."
      (unless (valid-policy? policy)
        (error "make-execution-site: policy must be project, endpoint-base or user-trusted" policy))
      (unless (string? location)
        (error "make-execution-site: location must be a string" location))
      (%make-execution-site location
                            (if (eq? policy 'user-trusted) '() (location-route location))
                            (if (eq? policy 'user-trusted) (trusted-cwd) (location-path location))
                            policy
                            environment))

    (define (execution-site-for location #:policy [policy #f] #:environment [environment '()])
      "Return an `execution-site` for LOCATION under POLICY and ENVIRONMENT.
If LOCATION is already an `execution-site`, keep its policy unless
POLICY is explicitly provided; otherwise default POLICY to `project`."
      (if (execution-site? location)
          (let ((eff-policy (or policy (execution-site-policy location))))
            (if (and (eq? eff-policy (execution-site-policy location))
                     (null? environment))
                location
                (make-execution-site (execution-site-location location)
                                     #:policy eff-policy
                                     #:environment (append (execution-site-environment location) environment))))
          (make-execution-site location #:policy (or policy 'project) #:environment environment)))

    (define (execution-site-endpoint site)
      "Return the remote node endpoint for SITE, or #f for local/trusted sites."
      (and (not (eq? (execution-site-policy site) 'user-trusted))
           (route-endpoint (execution-site-route site))))

    (define (clone-str s)
      (if (string? s) (string-copy s) s))

    (define (clone-env-alist xs)
      (if (list? xs)
          (map (lambda (e)
                 (if (pair? e)
                     (cons (clone-str (car e)) (clone-str (cdr e)))
                     e))
               xs)
          xs))

    (define-record-type env-snapshot
      (%make-env-snapshot provider status root rc-path delta message loaded-at)
      env-snapshot?
      (provider env-snapshot-provider)
      (status env-snapshot-status)
      (root %env-snapshot-root)
      (rc-path %env-snapshot-rc-path)
      (delta %env-snapshot-delta)
      (message %env-snapshot-message)
      (loaded-at env-snapshot-loaded-at))

    (define (env-snapshot-root snap)
      "Return the project root directory of environment snapshot SNAP, or `#f`."
      (clone-str (%env-snapshot-root snap)))
    (define (env-snapshot-rc-path snap)
      "Return the config file path of environment snapshot SNAP, or `#f`."
      (clone-str (%env-snapshot-rc-path snap)))
    (define (env-snapshot-delta snap)
      "Return the environment variable delta alist of snapshot SNAP."
      (clone-env-alist (%env-snapshot-delta snap)))
    (define (env-snapshot-message snap)
      "Return the diagnostic message of environment snapshot SNAP, or `#f`."
      (clone-str (%env-snapshot-message snap)))

    (define (make-env-snapshot provider status #:root [root #f] #:rc-path [rc-path #f]
                               #:delta [delta '()] #:message [message #f] #:loaded-at [loaded-at #f])
      "Construct an `env-snapshot` for PROVIDER with STATUS.
STATUS is `active`, `none`, `blocked`, `denied`, `error`, `unavailable`
or `unsupported`. Optional fields are ROOT, RC-PATH, DELTA (list of
`(NAME . VALUE-OR-#f)`), MESSAGE and LOADED-AT (milliseconds)."
      (%make-env-snapshot provider status (clone-str root) (clone-str rc-path)
                          (clone-env-alist delta) (clone-str message)
                          (or loaded-at (current-milliseconds))))

    ;;; Configuration parameters & hooks

    (define %direnv-enabled-hook (lambda () #t))
    (define %direnv-program-hook (lambda () "direnv"))
    (define %direnv-data-dir-hook (lambda () #f))

    (define (set-direnv-enabled-hook! proc)
      "Register PROC as the fallback thunk returning whether direnv is enabled.
The editor calls this to read its `direnv-enabled` option."
      (unless (procedure? proc)
        (error "set-direnv-enabled-hook!: expected procedure" proc))
      (set! %direnv-enabled-hook proc))

    (define default-direnv-enabled
      (make-parameter 'inherit
        (lambda (v)
          (unless (or (boolean? v) (eq? v 'inherit))
            (error "default-direnv-enabled: expected boolean or 'inherit" v))
          v))
      "Parameter controlling whether local direnv loading is enabled.
Set to #t or #f to override; `inherit` consults the registered hook.")

    (define (set-direnv-program-hook! proc)
      "Register PROC as the fallback thunk returning the direnv executable.
The editor calls this to read its `direnv-program` option."
      (unless (procedure? proc)
        (error "set-direnv-program-hook!: expected procedure" proc))
      (set! %direnv-program-hook proc))

    (define default-direnv-program
      (make-parameter #f
        (lambda (v)
          (unless (or (not v) (string? v))
            (error "default-direnv-program: expected string or #f" v))
          v))
      "Parameter overriding the direnv executable name or path.
When #f, consults the registered direnv program hook.")

    (define (set-direnv-data-dir-hook! proc)
      "Register PROC as the fallback thunk returning the direnv data directory.
The editor calls this to read its `direnv-data-dir` option."
      (unless (procedure? proc)
        (error "set-direnv-data-dir-hook!: expected procedure" proc))
      (set! %direnv-data-dir-hook proc))

    (define default-direnv-data-dir
      (make-parameter #f
        (lambda (v)
          (unless (or (not v) (string? v))
            (error "default-direnv-data-dir: expected string or #f" v))
          v))
      "Parameter overriding the direnv data directory (`allow/` and `deny/`).
Overrides must be an absolute path ending in `/direnv` so subprocesses
receive matching `XDG_DATA_HOME`. When #f, consults the registered hook,
then `XDG_DATA_HOME/direnv` or `~/.local/share/direnv` from the clean
host baseline.")

    (define (%default-direnv-sync-dir _dir handle)
      (pinned-dir-sync! handle))

    (define %direnv-sync-dir-hook %default-direnv-sync-dir)

    (define (set-direnv-sync-dir-hook! proc)
      "Register PROC to fsync a direnv store directory.
PROC receives `(DIR HANDLE)` where HANDLE is the pinned directory descriptor.
When PROC is `#f`, restores `pinned-dir-sync!`."
      (unless (or (not proc) (procedure? proc))
        (error "set-direnv-sync-dir-hook!: expected procedure or #f" proc))
      (set! %direnv-sync-dir-hook (or proc %default-direnv-sync-dir)))

    (define default-direnv-status-timeout
      (make-parameter 5000
        (lambda (v)
          (unless (and (integer? v) (> v 0))
            (error "default-direnv-status-timeout: expected positive integer" v))
          v))
      "Timeout in milliseconds for `direnv status` and `show_dump` calls.")

    (define default-direnv-timeout
      (make-parameter 30000
        (lambda (v)
          (unless (and (integer? v) (> v 0))
            (error "default-direnv-timeout: expected positive integer" v))
          v))
      "Timeout in milliseconds for `direnv export`, `allow`, and `deny` calls.")

    (define (direnv-enabled?)
      (let ((v (default-direnv-enabled)))
        (if (eq? v 'inherit) (and (%direnv-enabled-hook) #t) (and v #t))))

    (define (direnv-program)
      (or (default-direnv-program) (%direnv-program-hook) "direnv"))

    (set-trusted-direnv-program-hook! direnv-program)

    ;;; Environment provider registry (built-ins survive scope shutdown)

    (define %builtin-providers (make-hash-table))
    (define %extension-providers (make-registry 'env-providers))

    (define (register-builtin-provider! name proc doc)
      (hash-table-set! %builtin-providers name (cons proc doc))
      name)

    (define (register-env-provider! name proc doc)
      (registry-add! %extension-providers name (cons (scope-procedure proc) doc))
      name)

    (define-syntax define-env-provider
      (syntax-rules ()
        "Define a scope-owned environment provider NAME with docstring DOC.
(define-env-provider (NAME SITE REFRESH?) DOC BODY ...) registers NAME
for the current scope; unloading the scope removes it."
        ((_ (name site refresh?) doc body1 body ...)
         (register-env-provider! 'name (lambda (site refresh?) body1 body ...) doc))))

    (define (find-env-provider name)
      "Return the procedure registered for environment provider NAME, or #f."
      (let ((entry (or (registry-ref %extension-providers name)
                       (hash-table-ref/default %builtin-providers name #f))))
        (and entry (car entry))))

    (define (env-provider-names)
      "Return the list of registered environment provider names."
      (let ((ext (registry-keys %extension-providers))
            (builtins (hash-table-keys %builtin-providers)))
        (let loop ((xs builtins) (acc ext))
          (cond ((null? xs) acc)
                ((memq (car xs) acc) (loop (cdr xs) acc))
                (else (loop (cdr xs) (cons (car xs) acc)))))))

    (define default-env-providers
      (make-parameter '(direnv)
        (lambda (v)
          (unless (and (list? v) (every symbol? v))
            (error "default-env-providers: expected list of symbols" v))
          v))
      "Parameter holding the ordered list of environment provider names.")

    ;;; Low-level process helpers (endpoint-base, clean host baseline, timeout)

    ;; Run PROGRAM with ARGS in DIR under the clean host baseline environment.
    ;; Resolves PROGRAM against trusted-baseline-path (rejecting relative paths
    ;; containing `/`), enforces TIMEOUT-MS, drains stderr, and bounds stdout.
    ;; When `default-direnv-data-dir` / `%direnv-data-dir-hook` is set, derives
    ;; `XDG_DATA_HOME` from its parent (requiring an absolute `/.../direnv` path)
    ;; so `direnv` subprocesses and Techne's approval store agree.
    ;; Returns:
    ;;   (ok STATUS STDOUT STDERR)
    ;;   (invalid-data-dir)
    ;;   (untrusted-baseline)
    ;;   (unavailable)
    ;;   (invalid-program)
    ;;   (timeout)
    ;;   (overflow)
    ;;   (error)
    (define (run-endpoint-base dir program args #:max-stdout [max-stdout %default-max-stdout]
                               #:timeout [timeout-ms #f] #:label [label "direnv"])
      (let ((xdg-ov (direnv-data-dir-override-xdg)))
        (cond
          ((eq? xdg-ov 'invalid)
           (list 'invalid-data-dir))
          ((not (trusted-baseline-clean? #:direnv-program program))
           (list 'untrusted-baseline))
          (else
           (let ((resolved (guard (e ((task-cancelled-condition? e) (raise e))
                                     (#t 'invalid))
                             (resolve-trusted-program program #:direnv-program program))))
             (cond
               ((eq? resolved 'invalid) (list 'invalid-program))
               ((not resolved) (list 'unavailable))
               (else
                (let* ((base-env (trusted-baseline-environment #:direnv-program program))
                       (eff-env (if (pair? xdg-ov)
                                    (append (filter (lambda (e) (not (and (pair? e) (string=? (car e) "XDG_DATA_HOME"))))
                                                    base-env)
                                            (list (cons "XDG_DATA_HOME" (cdr xdg-ov))))
                                    base-env))
                       (res (run-bounded-process resolved args
                                                 #:directory dir
                                                 #:environment eff-env
                                                 #:max-stdout max-stdout
                                                 #:max-stderr %max-stderr-kept
                                                 #:timeout timeout-ms)))
                  (case (car res)
                    ((ok) res)
                    ((timeout) (list 'timeout))
                    ((output-limit) (list 'overflow))
                    (else (list 'error)))))))))))

    ;;; Local path & filesystem fingerprint helpers

    (define (strip-trailing-slash path)
      (if (and (> (string-length path) 1) (string-suffix? "/" path))
          (strip-trailing-slash (substring path 0 (- (string-length path) 1)))
          path))

    (define (file-kind path)
      (let ((attrs (guard (e (#t #f)) (file-attributes path))))
        (and (pair? attrs) (car attrs))))

    ;; Resolve symlinks before statting so symlinked `.envrc` target edits are detected.
    ;; Captures `(real-path size mtime)` when present, or `#f` when absent.
    (define (file-fingerprint path)
      (and (string? path)
           (let* ((real (guard (e (#t #f)) (file-truename path)))
                  (attrs (and real (guard (e (#t #f)) (file-attributes real)))))
             (and (pair? attrs)
                  (>= (length attrs) 3)
                  (list real (list-ref attrs 1) (list-ref attrs 2))))))

    (define (dir-fingerprint path)
      (and (string? path)
           (let* ((real (guard (e (#t #f)) (file-truename path)))
                  (attrs (and real (guard (e (#t #f)) (file-attributes real)))))
             (and (pair? attrs)
                  (eq? (car attrs) 'directory)
                  (let ((entries (guard (e (#t '())) (sort (directory-list real) string<?))))
                    (list real (list-ref attrs 2) entries))))))

    (define (canonical-local-dir path)
      (let* ((real (guard (e (#t path)) (file-truename path)))
             (base (if (eq? (file-kind real) 'file) (path-directory real) real)))
        (strip-trailing-slash base)))

    (define (canonical-path path)
      (strip-trailing-slash (guard (e (#t path)) (file-truename path))))

    ;; Canonicalize the containing directory of ENTRY without resolving a
    ;; symlinked leaf `.envrc` itself, because `direnv` binds approval to
    ;; the `.envrc` entry's path under its canonical directory (plus its bytes).
    (define (canonical-entry-path path)
      (let* ((trimmed (strip-trailing-slash path))
             (parent (path-directory trimmed))
             (name (path-file-name trimmed)))
        (if (and (string? parent) (> (string-length parent) 0)
                 (string? name) (> (string-length name) 0))
            (path-join (canonical-local-dir parent) name)
            trimmed)))

    ;; Search upward for `.envrc` specifically (not bare `.env`, so an unrelated
    ;; child `.env` never masks an ancestor `.envrc`).
    (define (find-nearest-envrc dir)
      (let loop ((d (strip-trailing-slash dir)))
        (let ((envrc (path-join d ".envrc")))
          (cond ((file-exists? envrc) envrc)
                ((or (string=? d "/") (string=? d "") (string=? d ".")) #f)
                (else
                 (let ((parent (strip-trailing-slash (path-directory d))))
                   (if (string=? parent d) #f (loop parent))))))))

    (define (path-under-dir? path root)
      (let ((p (strip-trailing-slash path))
            (r (strip-trailing-slash root)))
        (or (string=? p r)
            (string=? r "/")
            (string-prefix? (string-append r "/") p))))

    ;; Return `'default` when no override is configured, `(ok . parent-xdg)`
    ;; when the override is an absolute path ending in `/direnv` with an
    ;; absolute parent directory, or `'invalid` otherwise.
    (define (direnv-data-dir-override-xdg)
      (let ((override (or (default-direnv-data-dir) (%direnv-data-dir-hook))))
        (cond
          ((not override) 'default)
          ((and (string? override) (path-absolute? override))
           (let* ((trimmed (strip-trailing-slash override))
                  (base (path-file-name trimmed))
                  (parent (strip-trailing-slash (path-directory trimmed))))
             (if (and (string? base) (string=? base "direnv")
                      (string? parent) (> (string-length parent) 0)
                      (path-absolute? parent))
                 (cons 'ok parent)
                 'invalid)))
          (else 'invalid))))

    (define (direnv-data-dir)
      (let ((xdg-ov (direnv-data-dir-override-xdg)))
        (cond
          ((eq? xdg-ov 'invalid) #f)
          ((pair? xdg-ov) (path-join (cdr xdg-ov) "direnv"))
          (else
           (let* ((prog (direnv-program))
                  (xdg (trusted-environment-variable "XDG_DATA_HOME" #:direnv-program prog)))
             (if (and (string? xdg) (string-prefix? "/" xdg))
                 (path-join xdg "direnv")
                 (let ((h (trusted-home)))
                   (and h (path-join h ".local/share/direnv")))))))))

    (define (trust-store-fingerprint)
      (let ((base (direnv-data-dir)))
        (and base
             (list base
                   (dir-fingerprint (path-join base "allow"))
                   (dir-fingerprint (path-join base "deny"))))))

    ;;; Built-in direnv provider (keyed by effective .envrc, watched-file aware,
    ;;; generation-guarded, single-flight deduplicated, 0-I/O mode-line lookup)

    (define-record-type direnv-cache-entry
      (make-direnv-cache-entry nearest-rc nearest-fp found-rc found-fp trust-fp watch-fps snapshot)
      direnv-cache-entry?
      (nearest-rc cache-entry-nearest-rc)
      (nearest-fp cache-entry-nearest-fp)
      (found-rc cache-entry-found-rc)
      (found-fp cache-entry-found-fp)
      (trust-fp cache-entry-trust-fp)
      (watch-fps cache-entry-watch-fps)
      (snapshot cache-entry-snapshot))

    (define-record-type direnv-inflight-slot
      (make-direnv-inflight-slot raw-dir canon-dir anchor-rc done box worker invalidated?)
      direnv-inflight-slot?
      (raw-dir inflight-slot-raw-dir)
      (canon-dir inflight-slot-canon-dir)
      (anchor-rc inflight-slot-anchor-rc)
      (done inflight-slot-done)
      (box inflight-slot-box)
      (worker inflight-slot-worker inflight-slot-worker-set!)
      (invalidated? inflight-slot-invalidated? inflight-slot-invalidated?-set!))

    ;; Keyed by: (string-append program "\x0;" rc-or-canon-dir) -> direnv-cache-entry
    (define %direnv-cache (make-hash-table))

    ;; Pure in-memory alias map for 0-I/O mode-line reads:
    ;;   (string-append program "\x0;" (strip-trailing-slash raw-dir)) -> cache-key
    (define %dir->cache-key (make-hash-table))

    ;; In-flight single-flight workers:
    ;;   cache-key -> direnv-inflight-slot
    (define %direnv-inflight (make-hash-table))

    ;; Monotonic generation bumped on every invalidation or explicit refresh.
    (define %cache-generation 0)

    (define (bump-generation!)
      (set! %cache-generation (+ %cache-generation 1))
      %cache-generation)

    (define (cache-key program anchor)
      (string-append program "\x0;" anchor))

    (define (dir-alias-key program raw-dir)
      (string-append program "\x0;" (strip-trailing-slash raw-dir)))

    (define (record-dir-alias! program raw-dir canon-dir key)
      (when (string? raw-dir)
        (hash-table-set! %dir->cache-key (dir-alias-key program raw-dir) key))
      (when (string? canon-dir)
        (hash-table-set! %dir->cache-key (dir-alias-key program canon-dir) key)))

    (define (parse-direnv-delta text)
      (let ((trimmed (string-trim (or text ""))))
        (if (string=? trimmed "")
            '()
            (let ((parsed (string->json trimmed)))
              (unless (dict? parsed)
                (error "direnv: expected JSON object"))
              (filter-map
               (lambda (k)
                 (let ((v (dict-ref parsed k 'missing)))
                   (cond ((string? v) (cons k v))
                         ((eq? v 'null) (cons k #f))
                         (else #f))))
               (dict-keys parsed))))))

    (define (delta-ref delta name)
      (let ((pair (assoc name delta)))
        (and pair (cdr pair))))

    ;; Decode `DIRENV_WATCHES` via `direnv show_dump` to get exact watched entries
    ;; `(path exists modtime)` (including `.envrc`, exact `allow/<hash>`,
    ;; `deny/<hash>`, `watch_file`, `source_up`, `flake.nix`, `flake.lock`).
    ;; Never leaks env values.
    (define (extract-watched-entries canon-dir program delta)
      (let ((watches (delta-ref delta "DIRENV_WATCHES")))
        (if (not (and (string? watches) (> (string-length watches) 0)))
            '()
            (let ((res (guard (e ((task-cancelled-condition? e) (raise e))
                                 (#t (list 'error)))
                         (run-endpoint-base canon-dir program (list "show_dump" watches)
                                            #:timeout (default-direnv-status-timeout)
                                            #:label "direnv"))))
              (if (and (eq? (car res) 'ok) (eqv? (cadr res) 0))
                  (let ((parsed (guard (e (#t #f)) (string->json (caddr res)))))
                    (if (vector? parsed)
                        (filter-map
                         (lambda (item)
                           (and (dict? item)
                                (let ((p (dict-ref item "path" #f)))
                                  (and (string? p)
                                       (list p
                                             (dict-ref item "exists" 'unknown)
                                             (dict-ref item "modtime" #f))))))
                         (vector->list parsed))
                        '()))
                  '())))))

    ;; Compute direnv-compatible watch mtime for PATH, or `#f` if absent
    ;; (including broken symlinks). Matches `direnv`'s `file_times.go`: for
    ;; symlinks whose target exists, uses `max(lstat.mtime, target.mtime)`.
    (define (watch-path-mtime path)
      (let ((lstat (guard (e (#t #f)) (file-attributes path))))
        (and (pair? lstat)
             (>= (length lstat) 3)
             (if (eq? (car lstat) 'symlink)
                 (let* ((real (guard (e (#t #f)) (file-truename path)))
                        (target (and real (guard (e (#t #f)) (file-attributes real)))))
                   (and (pair? target)
                        (>= (length target) 3)
                        (max (list-ref lstat 2) (list-ref target 2))))
                 (list-ref lstat 2)))))

    (define (watch-entry-matches-current? entry)
      (let ((exists (cadr entry))
            (modtime (caddr entry))
            (cur-mtime (watch-path-mtime (car entry))))
        (cond
          ((eq? exists #f) (not cur-mtime))
          ((eq? exists #t)
           (and cur-mtime
                (or (not (integer? modtime))
                    (= cur-mtime modtime))))
          (else #t))))

    (define (fingerprint-watch-list paths)
      (map (lambda (p) (cons p (file-fingerprint p))) paths))

    (define (watches-fresh? watch-fps)
      (every (lambda (entry)
               (equal? (cdr entry) (file-fingerprint (car entry))))
             watch-fps))

    (define (capture-pre-fp canon-dir)
      (let ((nearest (find-nearest-envrc canon-dir)))
        (list nearest (file-fingerprint nearest) (trust-store-fingerprint))))

    ;; Evaluate direnv for CANON-DIR with sanitized messages (never leaks stderr).
    ;; Returns `(list snapshot watch-fps watches-match?)`.
    (define (direnv-eval-uncached canon-dir program)
      (let ((nearest (find-nearest-envrc canon-dir)))
        (if (not nearest)
            (list (make-env-snapshot 'direnv 'none #:message "No .envrc") '() #t)
            (let ((status-res (run-endpoint-base canon-dir program '("status" "--json")
                                                 #:timeout (default-direnv-status-timeout)
                                                 #:label "direnv")))
              (case (car status-res)
                ((invalid-data-dir)
                 (list (make-env-snapshot 'direnv 'error #:rc-path nearest #:root (path-directory nearest)
                                          #:message "direnv-data-dir must be an absolute path ending in /direnv")
                       '() #t))
                ((invalid-program)
                 (list (make-env-snapshot 'direnv 'error #:message "direnv program path must be absolute or a bare name") '() #t))
                ((untrusted-baseline)
                 (list (make-env-snapshot 'direnv 'unavailable #:rc-path nearest #:root (path-directory nearest)
                                          #:message "direnv unavailable (startup direnv environment could not be cleanly unloaded)") '() #t))
                ((unavailable)
                 (list (make-env-snapshot 'direnv 'unavailable #:message "direnv unavailable") '() #t))
                ((timeout)
                 (list (make-env-snapshot 'direnv 'error #:rc-path nearest #:root (path-directory nearest)
                                          #:message "direnv status timed out") '() #t))
                ((overflow)
                 (list (make-env-snapshot 'direnv 'error #:rc-path nearest #:root (path-directory nearest)
                                          #:message "direnv status output exceeded size limit") '() #t))
                ((error)
                 (list (make-env-snapshot 'direnv 'error #:rc-path nearest #:root (path-directory nearest)
                                          #:message "direnv status failed") '() #t))
                ((ok)
                 (let ((code (cadr status-res))
                       (stdout (caddr status-res)))
                   (if (not (eqv? code 0))
                       (list (make-env-snapshot 'direnv 'error #:rc-path nearest #:root (path-directory nearest)
                                                #:message (string-append "direnv status failed (exit " (number->string code) ")"))
                             '() #t)
                       (let ((json (guard (e (#t 'bad-json)) (string->json stdout))))
                         (if (not (dict? json))
                             (list (make-env-snapshot 'direnv 'error #:rc-path nearest #:root (path-directory nearest)
                                                      #:message "direnv status returned invalid JSON")
                                   '() #t)
                             (let* ((state (dict-ref json "state" #f))
                                    (found (and (dict? state) (dict-ref state "foundRC" #f)))
                                    (found-path (and (dict? found) (dict-ref found "path" #f)))
                                    (allowed (and (dict? found) (dict-ref found "allowed" #f)))
                                    (root (and (string? found-path) (path-directory found-path))))
                               (cond
                                 ((not (string? found-path))
                                  (list (make-env-snapshot 'direnv 'none #:message "No .envrc") '() #t))
                                 ;; Blocked (.envrc not yet allowed) — never run export.
                                 ((eqv? allowed 1)
                                  (list (make-env-snapshot 'direnv 'blocked
                                                           #:root root #:rc-path found-path #:delta '()
                                                           #:message (string-append "Blocked: run M-x direnv-allow to approve " found-path))
                                        '() #t))
                                 ;; Explicitly denied.
                                 ((eqv? allowed 2)
                                  (list (make-env-snapshot 'direnv 'denied
                                                           #:root root #:rc-path found-path #:delta '()
                                                           #:message (string-append "Denied: " found-path))
                                        '() #t))
                                 ;; Allowed: run `direnv export json` against clean host baseline.
                                 ((eqv? allowed 0)
                                  (let ((exp (run-endpoint-base canon-dir program '("export" "json")
                                                                #:timeout (default-direnv-timeout)
                                                                #:label "direnv")))
                                    (case (car exp)
                                      ((invalid-data-dir)
                                       (list (make-env-snapshot 'direnv 'error #:root root #:rc-path found-path #:delta '()
                                                                #:message "direnv-data-dir must be an absolute path ending in /direnv") '() #t))
                                      ((untrusted-baseline)
                                       (list (make-env-snapshot 'direnv 'unavailable #:root root #:rc-path found-path
                                                                #:message "direnv unavailable (startup direnv environment could not be cleanly unloaded)") '() #t))
                                      ((unavailable)
                                       (list (make-env-snapshot 'direnv 'unavailable #:root root #:rc-path found-path
                                                                #:message "direnv unavailable") '() #t))
                                      ((timeout)
                                       (list (make-env-snapshot 'direnv 'error #:root root #:rc-path found-path #:delta '()
                                                                #:message "direnv export timed out") '() #t))
                                      ((overflow)
                                       (list (make-env-snapshot 'direnv 'error #:root root #:rc-path found-path #:delta '()
                                                                #:message "direnv export output exceeded size limit") '() #t))
                                      ((error invalid-program)
                                       (list (make-env-snapshot 'direnv 'error #:root root #:rc-path found-path #:delta '()
                                                                #:message "direnv export failed") '() #t))
                                      ((ok)
                                       (let ((ecode (cadr exp))
                                             (eout (caddr exp)))
                                         ;; Non-zero exit: NEVER publish partial stdout diff or raw stderr.
                                         (if (not (eqv? ecode 0))
                                             (list (make-env-snapshot 'direnv 'error #:root root #:rc-path found-path #:delta '()
                                                                      #:message (string-append "direnv export failed (exit "
                                                                                               (number->string ecode) ")"))
                                                   '() #t)
                                             (let ((delta (guard (e (#t 'parse-error)) (parse-direnv-delta eout))))
                                               (if (eq? delta 'parse-error)
                                                   (list (make-env-snapshot 'direnv 'error #:root root #:rc-path found-path #:delta '()
                                                                            #:message "direnv export returned invalid JSON")
                                                         '() #t)
                                                   (let* ((watch-entries (extract-watched-entries canon-dir program delta))
                                                          (watch-fps (fingerprint-watch-list (map car watch-entries)))
                                                          (watches-match? (every watch-entry-matches-current? watch-entries)))
                                                     (list (make-env-snapshot 'direnv 'active
                                                                              #:root root #:rc-path found-path #:delta delta
                                                                              #:message "Active")
                                                           watch-fps
                                                           watches-match?))))))))))
                                 (else
                                  (list (make-env-snapshot 'direnv 'error #:root root #:rc-path found-path
                                                           #:message "direnv status returned unexpected state")
                                        '() #t)))))))))
                (else
                 (list (make-env-snapshot 'direnv 'error #:message "direnv status failed") '() #t)))))))

    (define (direnv-cached-fresh? entry nearest-rc)
      (and (equal? (cache-entry-nearest-rc entry) nearest-rc)
           (equal? (cache-entry-nearest-fp entry) (file-fingerprint nearest-rc))
           (or (not (cache-entry-found-rc entry))
               (equal? (cache-entry-found-fp entry) (file-fingerprint (cache-entry-found-rc entry))))
           (equal? (cache-entry-trust-fp entry) (trust-store-fingerprint))
           (watches-fresh? (cache-entry-watch-fps entry))))

    ;; Key by canonical launch directory so subdirectories with their own
    ;; direnv-selected config (e.g. `load_dotenv` `.env`) never reuse a
    ;; parent directory's snapshot before `direnv` identifies their config.
    (define (cache-key-for-local program canon)
      (cache-key program canon))

    (define (cancel-inflight-slot! key slot)
      (inflight-slot-invalidated?-set! slot #t)
      (when (eq? (hash-table-ref/default %direnv-inflight key #f) slot)
        (hash-table-delete! %direnv-inflight key))
      (let ((worker (inflight-slot-worker slot)))
        (when worker
          (guard (e (#t #f)) (task-cancel worker)))))

    (define (await-inflight slot)
      (channel-recv (inflight-slot-done slot))
      (let ((res (car (inflight-slot-box slot))))
        (cond ((and (pair? res) (eq? (car res) 'ok)) res)
              ((and (pair? res) (eq? (car res) 'stale)) res)
              ((and (pair? res) (eq? (car res) 'err)) (raise (cdr res)))
              (else (cons 'ok (make-env-snapshot 'direnv 'error #:message "direnv evaluation failed"))))))

    (define (direnv-provider site refresh?)
      (cond
        ((not (direnv-enabled?))
         (make-env-snapshot 'direnv 'none #:message "direnv disabled"))
        ((pair? (execution-site-route site))
         (make-env-snapshot 'direnv 'unsupported #:message "Remote direnv is not enabled yet"))
        ((not (file-exists? (execution-site-directory site)))
         (make-env-snapshot 'direnv 'none #:message "Directory does not exist"))
        (else
         (let retry ((attempt 0) (do-refresh? refresh?))
           (let* ((program (direnv-program))
                  (raw-dir (execution-site-directory site))
                  (canon (canonical-local-dir raw-dir))
                  (nearest (find-nearest-envrc canon))
                  (key (cache-key-for-local program canon))
                  (cached (and (not do-refresh?) (hash-table-ref/default %direnv-cache key #f))))
             (if (and cached (direnv-cached-fresh? cached nearest))
                 (begin
                   (record-dir-alias! program raw-dir canon key)
                   (cache-entry-snapshot cached))
                 (let ((inflight (and (not do-refresh?)
                                      (let ((slot (hash-table-ref/default %direnv-inflight key #f)))
                                        (and slot
                                             (equal? (inflight-slot-anchor-rc slot) nearest)
                                             slot)))))
                   (if inflight
                       (let ((res (await-inflight inflight)))
                         (cond ((eq? (car res) 'ok)
                                (when (hash-table-contains? %direnv-cache key)
                                  (record-dir-alias! program raw-dir canon key))
                                (cdr res))
                               ((and (eq? (car res) 'stale) (< attempt 2))
                                (retry (+ attempt 1) #f))
                               (else
                                (make-env-snapshot 'direnv 'error
                                                   #:message "direnv environment changed during evaluation"))))
                       (let* ((start-gen (if do-refresh? (bump-generation!) %cache-generation))
                              (pre-fp (capture-pre-fp canon))
                              (done (make-channel 1))
                              (box (list #f))
                              (slot (make-direnv-inflight-slot raw-dir canon nearest done box #f #f)))
                         (when do-refresh?
                           (let ((prev (hash-table-ref/default %direnv-inflight key #f)))
                             (when prev (cancel-inflight-slot! key prev)))
                           (hash-table-delete! %direnv-cache key))
                         (hash-table-set! %direnv-inflight key slot)
                         (let ((spawn-ok
                                (guard (e (#t (cons 'spawn-fail e)))
                                  (let ((worker
                                         (%spawn-in-root-scope
                                          (lambda ()
                                            (dynamic-wind
                                                (lambda () #f)
                                                (lambda ()
                                                  (let ((outcome
                                                         (guard (e ((task-cancelled-condition? e)
                                                                    (if (or (inflight-slot-invalidated? slot)
                                                                            (not (= start-gen %cache-generation)))
                                                                        (list 'stale)
                                                                        (cons 'ok (make-env-snapshot 'direnv 'error
                                                                                                     #:message "direnv evaluation cancelled"))))
                                                                   (#t (cons 'err e)))
                                                           (let* ((eval-res (direnv-eval-uncached canon program))
                                                                  (snap (car eval-res))
                                                                  (watch-fps (cadr eval-res))
                                                                  (watches-match? (caddr eval-res))
                                                                  (post-fp (capture-pre-fp canon))
                                                                  (found-rc (env-snapshot-rc-path snap))
                                                                  (found-fp (file-fingerprint found-rc))
                                                                  (fresh? (and watches-match?
                                                                               (not (inflight-slot-invalidated? slot))
                                                                               (= start-gen %cache-generation)
                                                                               (equal? pre-fp post-fp)
                                                                               (watches-fresh? watch-fps))))
                                                             ;; Publish and deliver ONLY when neither invalidation nor input changes occurred during eval.
                                                             ;; Never cache transient `error` snapshots so subsequent calls can retry.
                                                             (if fresh?
                                                                 (if (eq? (env-snapshot-status snap) 'error)
                                                                     (begin
                                                                       (hash-table-delete! %direnv-cache key)
                                                                       (cons 'ok snap))
                                                                     (let ((entry (make-direnv-cache-entry
                                                                                   (car post-fp) (cadr post-fp)
                                                                                   found-rc found-fp (caddr post-fp)
                                                                                   watch-fps snap)))
                                                                       (hash-table-set! %direnv-cache key entry)
                                                                       (record-dir-alias! program raw-dir canon key)
                                                                       (cons 'ok snap)))
                                                                 (list 'stale))))))
                                                    (set-car! box outcome)))
                                                (lambda ()
                                                  (when (eq? (hash-table-ref/default %direnv-inflight key #f) slot)
                                                    (hash-table-delete! %direnv-inflight key))
                                                  (channel-close done)))))))
                                     (unless worker
                                       (error "direnv: failed to spawn worker task" worker))
                                     (inflight-slot-worker-set! slot worker)
                                     'ok))))
                           (when (and (pair? spawn-ok) (eq? (car spawn-ok) 'spawn-fail))
                             (when (eq? (hash-table-ref/default %direnv-inflight key #f) slot)
                               (hash-table-delete! %direnv-inflight key))
                             (channel-close done)
                             (raise (cdr spawn-ok)))
                           (let ((res (await-inflight slot)))
                             (cond ((eq? (car res) 'ok) (cdr res))
                                   ((and (eq? (car res) 'stale) (< attempt 2))
                                    (retry (+ attempt 1) #f))
                                   (else
                                    (make-env-snapshot 'direnv 'error
                                                       #:message "direnv environment changed during evaluation"))))))))))))))

    (register-builtin-provider!
     'direnv
     direnv-provider
     "Evaluate local project environments with `direnv export json`.")

    ;;; Public inspection, invalidation, allow/deny, and process launch API

    (define (execution-site-has-env-config? dir)
      "Return #t if local DIR has a `.envrc` or active startup `DIRENV_DIR`.
Does not spawn any subprocess."
      (and (direnv-enabled?)
           (string? dir)
           (not (remote-path? dir))
           (file-exists? dir)
           (or (and (get-environment-variable "DIRENV_DIR") #t)
               (and (find-nearest-envrc (canonical-local-dir dir)) #t))))

    (define (execution-site-needs-eval? dir)
      "Return #t if local DIR has a `.envrc` whose cache is missing or stale.
Does not spawn any subprocess; safe for deciding whether to defer work."
      (and (direnv-enabled?)
           (string? dir)
           (not (remote-path? dir))
           (file-exists? dir)
           (let* ((program (direnv-program))
                  (canon (canonical-local-dir dir))
                  (nearest (find-nearest-envrc canon)))
             (and nearest
                  (let* ((key (cache-key-for-local program canon))
                         (cached (hash-table-ref/default %direnv-cache key #f)))
                    (not (and cached (direnv-cached-fresh? cached nearest))))))))

    ;; Pure in-memory lookup: ZERO filesystem I/O, ZERO subprocesses.
    (define (execution-site-cached-snapshot dir)
      "Return the cached `env-snapshot` for DIR, or #f if none is cached.
Performs zero filesystem I/O and never spawns a subprocess; safe to call
while rendering the mode line."
      (and (string? dir)
           (not (remote-path? dir))
           (let* ((program (direnv-program))
                  (key (hash-table-ref/default %dir->cache-key
                                               (dir-alias-key program (strip-trailing-slash dir))
                                               #f))
                  (entry (and key (hash-table-ref/default %direnv-cache key #f))))
             (and entry (cache-entry-snapshot entry)))))

    (define (execution-site-watched-path? path)
      "Return #t if local PATH is watched by any cached direnv snapshot.
Matches `.envrc`, `.env`, and any file reported in `DIRENV_WATCHES`."
      (and (string? path)
           (not (remote-path? path))
           (let ((name (path-file-name path))
                 (canon (canonical-path path)))
             (or (member name '(".envrc" ".env"))
                 (any (lambda (entry)
                        (or (and (string? (cache-entry-nearest-rc entry))
                                 (string=? (canonical-path (cache-entry-nearest-rc entry)) canon))
                            (and (string? (cache-entry-found-rc entry))
                                 (string=? (canonical-path (cache-entry-found-rc entry)) canon))
                            (any (lambda (w) (string=? (canonical-path (car w)) canon))
                                 (cache-entry-watch-fps entry))))
                      (hash-table-values %direnv-cache))))))

    (define (inflight-matches-root? key slot root)
      (let* ((parts (string-split key "\x0;"))
             (anchor (if (pair? (cdr parts)) (cadr parts) "")))
        (or (path-under-dir? anchor root)
            (and (string? (inflight-slot-canon-dir slot))
                 (path-under-dir? (inflight-slot-canon-dir slot) root))
            (and (string? (inflight-slot-raw-dir slot))
                 (path-under-dir? (inflight-slot-raw-dir slot) root))
            (and (string? (inflight-slot-anchor-rc slot))
                 (or (path-under-dir? (inflight-slot-anchor-rc slot) root)
                     (path-under-dir? (canonical-path (inflight-slot-anchor-rc slot)) root))))))

    (define (execution-site-invalidate! [dir #f])
      "Clear cached environment snapshots under DIR, or all when DIR is #f.
No-op for remote locations. Cancels matching in-flight evaluations."
      (cond
        ((not dir)
         (bump-generation!)
         (for-each (lambda (k)
                     (let ((slot (hash-table-ref/default %direnv-inflight k #f)))
                       (when slot (cancel-inflight-slot! k slot))))
                   (hash-table-keys %direnv-inflight))
         (for-each (lambda (k) (hash-table-delete! %direnv-cache k))
                   (hash-table-keys %direnv-cache))
         (for-each (lambda (k) (hash-table-delete! %dir->cache-key k))
                   (hash-table-keys %dir->cache-key)))
        ((remote-path? dir) #f)
        (else
         (bump-generation!)
         (let ((root (canonical-local-dir dir)))
           ;; Cancel matching in-flight slots independently of cache presence
           ;; so cold/first evaluations cannot finish and deliver stale snapshots.
           (for-each
            (lambda (k)
              (let ((slot (hash-table-ref/default %direnv-inflight k #f)))
                (when (and slot (inflight-matches-root? k slot root))
                  (cancel-inflight-slot! k slot))))
            (hash-table-keys %direnv-inflight))
           (for-each
            (lambda (k)
              (let* ((parts (string-split k "\x0;"))
                     (cached-anchor (if (pair? (cdr parts)) (cadr parts) ""))
                     (entry (hash-table-ref/default %direnv-cache k #f)))
                (when (or (path-under-dir? cached-anchor root)
                          (and entry
                               (or (and (string? (cache-entry-nearest-rc entry))
                                        (or (path-under-dir? (cache-entry-nearest-rc entry) root)
                                            (path-under-dir? (canonical-path (cache-entry-nearest-rc entry)) root)))
                                   (and (string? (cache-entry-found-rc entry))
                                        (or (path-under-dir? (cache-entry-found-rc entry) root)
                                            (path-under-dir? (canonical-path (cache-entry-found-rc entry)) root)))
                                   (any (lambda (w) (path-under-dir? (car w) root))
                                        (cache-entry-watch-fps entry)))))
                  (let ((slot (hash-table-ref/default %direnv-inflight k #f)))
                    (when slot (cancel-inflight-slot! k slot)))
                  (hash-table-delete! %direnv-cache k))))
            (hash-table-keys %direnv-cache))
           (for-each
            (lambda (ak)
              (let* ((parts (string-split ak "\x0;"))
                     (raw (if (pair? (cdr parts)) (cadr parts) ""))
                     (target-key (hash-table-ref/default %dir->cache-key ak #f)))
                (when (or (path-under-dir? raw root)
                          (and target-key (not (hash-table-contains? %direnv-cache target-key))))
                  (hash-table-delete! %dir->cache-key ak))))
            (hash-table-keys %dir->cache-key))))))

    (define (execution-site-env-snapshot site #:refresh? [refresh? #f])
      "Resolve and return the primary `env-snapshot` for SITE.
Pass REFRESH? #t to bypass cached snapshots. Returns a `none` snapshot
for `endpoint-base` and `user-trusted` sites."
      (let ((site (execution-site-for site)))
        (if (not (eq? (execution-site-policy site) 'project))
            (make-env-snapshot 'none 'none)
            (let loop ((names (default-env-providers)) (last-snap (make-env-snapshot 'none 'none)))
              (if (null? names)
                  last-snap
                  (let ((proc (find-env-provider (car names))))
                    (if (not proc)
                        (loop (cdr names) last-snap)
                        (let ((snap (guard (e ((task-cancelled-condition? e) (raise e))
                                              (#t (make-env-snapshot (car names) 'error
                                                                     #:message "Environment provider failed")))
                                      (let ((res (proc site refresh?)))
                                        (if (env-snapshot? res)
                                            res
                                            (make-env-snapshot (car names) 'error
                                                               #:message "Environment provider returned invalid snapshot"))))))
                          (if (memq (env-snapshot-status snap) '(active blocked denied error))
                              snap
                              (loop (cdr names) snap))))))))))

    (define (execution-site-resolved-environment site #:refresh? [refresh? #f] #:environment [environment '()])
      "Return the launch environment alist for SITE.
Starts from the verified clean host baseline for local sites (failing
closed if an inherited startup `DIRENV_*` overlay could not be cleanly
unloaded), combines active provider deltas (for `project` policy only,
honoring REFRESH?), SITE's configured overrides, and explicit
ENVIRONMENT overrides (last wins)."
      (let* ((site (execution-site-for site))
             (remote? (pair? (execution-site-route site)))
             (dprog (and (not remote?) (direnv-program)))
             (_ (unless (or remote? (trusted-baseline-clean? #:direnv-program dprog))
                  (error "execution-site: refusing local process launch; startup direnv environment could not be cleanly unloaded")))
             (base (if remote?
                       '()
                       (trusted-baseline-environment #:direnv-program dprog)))
             (delta (if (eq? (execution-site-policy site) 'project)
                        (let ((snap (execution-site-env-snapshot site #:refresh? refresh?)))
                          (if (eq? (env-snapshot-status snap) 'active)
                              (env-snapshot-delta snap)
                              '()))
                        '())))
        (append base delta (execution-site-environment site) environment)))

    (define (last-env-value env name)
      (let loop ((xs (reverse env)))
        (cond ((null? xs) 'missing)
              ((string=? (caar xs) name) (cdar xs))
              (else (loop (cdr xs))))))

    (define (execution-site-find-program site program #:refresh? [refresh? #f])
      "Return the path to executable PROGRAM under SITE's PATH, or #f.
Checks PROGRAM directly if it contains `/` (resolving relative paths
against SITE's directory); otherwise searches PATH from SITE's resolved
environment (passing REFRESH? through, falling back to clean host PATH).
Returns #f for remote sites, non-executable entries, or when an
inherited startup `DIRENV_*` overlay could not be cleanly unloaded."
      (let ((site (execution-site-for site)))
        (cond
          ((not (and (string? program) (> (string-length program) 0))) #f)
          ((pair? (execution-site-route site)) #f)
          ((not (trusted-baseline-clean? #:direnv-program (direnv-program))) #f)
          ((string-contains program "/")
           (let ((cand (if (string-prefix? "/" program)
                           program
                           (path-join (execution-site-directory site) program))))
             (and (executable-file? cand) cand)))
          (else
           (let* ((env (execution-site-resolved-environment site #:refresh? refresh?))
                  (path-entry (last-env-value env "PATH"))
                  (path-str (cond ((eq? path-entry 'missing) (trusted-baseline-path #:direnv-program (direnv-program)))
                                  ((string? path-entry) path-entry)
                                  (else "")))
                  (site-dir (execution-site-directory site)))
             (let loop ((dirs (string-split path-str ":")))
               (if (null? dirs)
                   #f
                   (let* ((d (car dirs))
                          (base (cond ((string=? d "") #f)
                                      ((string=? d ".") site-dir)
                                      ((string-prefix? "/" d) d)
                                      (else (path-join site-dir d))))
                          (cand (and base (path-join base program))))
                     (if (and cand (executable-file? cand))
                         cand
                         (loop (cdr dirs)))))))))))

    (define (direnv-status-found-rc canon program)
      (let ((st (run-endpoint-base canon program '("status" "--json")
                                   #:timeout (default-direnv-status-timeout)
                                   #:label "direnv")))
        (if (not (and (eq? (car st) 'ok) (eqv? (cadr st) 0)))
            (list 'error (cond
                           ((eq? (car st) 'invalid-data-dir)
                            "direnv-data-dir must be an absolute path ending in /direnv")
                           ((eq? (car st) 'ok)
                            (string-append "direnv status failed (exit " (number->string (cadr st)) ")"))
                           (else
                            (string-append "direnv status failed: " (symbol->string (car st))))))
            (let* ((json (guard (e (#t #f)) (string->json (caddr st))))
                   (state (and (dict? json) (dict-ref json "state" #f)))
                   (found (and (dict? state) (dict-ref state "foundRC" #f)))
                   (found-path (and (dict? found) (dict-ref found "path" #f))))
              (cond
                ((not (dict? json)) (list 'error "direnv status returned invalid JSON"))
                ((and (string? found-path) (path-absolute? found-path)) (list 'ok found-path))
                (else (list 'none)))))))

    (define (direnv-target-path canon program)
      (let ((res (direnv-status-found-rc canon program)))
        (if (eq? (car res) 'ok)
            (cadr res)
            (or (find-nearest-envrc canon) canon))))

    (define (target-root-dir target)
      (let ((real (guard (e (#t target)) (file-truename target))))
        (if (eq? (file-kind real) 'directory) target (path-directory target))))

    ;; `direnv deny <file>` binds direnv directly to the exact `.envrc` file path
    ;; for regular `.envrc` files, avoiding directory re-discovery. When `.envrc`
    ;; is a symlink, `direnv`'s CLI resolves the argument before checking its
    ;; basename and allowlist path, so symlinked `.envrc` files must be addressed
    ;; via their containing directory `root`.
    (define (direnv-cli-arg target canon-target root)
      (if (and (string? target)
               (string? canon-target)
               (string=? (strip-trailing-slash target) canon-target)
               (eq? (file-kind canon-target) 'file))
          canon-target
          root))

    (define (direnv-allow-hash target bytes)
      (hex-encode (sha-256 (bytevector-append (string->utf8 (string-append target "\n")) bytes))))

    (define (direnv-deny-hash target)
      (hex-encode (sha-256 (string->utf8 (string-append target "\n")))))

    (define (execution-site-direnv-target dir)
      "Return the `.envrc` path that `direnv` resolves for local DIR.
Raises an error for remote locations or when an inherited startup
`direnv` overlay could not be cleanly unloaded."
      (when (remote-path? dir)
        (error "direnv: remote direnv is not supported yet"))
      (when (eq? (direnv-data-dir-override-xdg) 'invalid)
        (error "direnv: direnv-data-dir must be an absolute path ending in /direnv"))
      (let ((program (direnv-program)))
        (unless (trusted-baseline-clean? #:direnv-program program)
          (error "direnv: startup direnv environment could not be cleanly unloaded"))
        (direnv-target-path (canonical-local-dir dir) program)))

    (define (execution-site-direnv-file-digest path)
      "Return the lowercase hex SHA-256 digest of local file PATH, or #f.
Returns #f if PATH is missing, a directory, unreadable, or larger than
the bounded read limit."
      (and (string? path)
           (not (remote-path? path))
           (let ((real (guard (e (#t #f)) (file-truename path))))
             (and real
                  (path-absolute? real)
                  (guard (e ((task-cancelled-condition? e) (raise e))
                            (#t #f))
                    (let ((bytes (read-confined-bytes-nofollow (path-directory real)
                                                               (path-file-name real)
                                                               %default-max-stdout)))
                      (and (bytevector? bytes) (hex-encode (sha-256 bytes)))))))))

    (define (execution-site-direnv-allow! dir #:expected-target [expected-target #f]
                                          #:expected-digest [expected-digest #f])
      "Approve the `.envrc` for local DIR bound to its verified bytes.
Fails closed if an inherited startup `direnv` overlay could not be cleanly
unloaded or if an existing `direnv` deny record is present (Techne never
automatically deletes deny records). Reads the canonical `.envrc` bytes once
without following symlinks, checks EXPECTED-TARGET and EXPECTED-DIGEST when
provided, records only the digest-bound allow entry for those exact bytes inside
the `direnv` data store without running `direnv allow <path>`, verifies
acceptance via `direnv status --json`, and rolls back on any mismatch,
concurrent denial, or verification failure. Invalidates cached snapshots under
the `.envrc` directory and returns the refreshed `env-snapshot`."
      (when (remote-path? dir)
        (error "direnv: remote direnv allow is not supported yet"))
      (let ((program (direnv-program)))
        (unless (trusted-baseline-clean? #:direnv-program program)
          (error "direnv allow failed: startup direnv environment could not be cleanly unloaded"))
        (let* ((canon (canonical-local-dir dir))
               (found (direnv-status-found-rc canon program))
               (target (cond
                         ((eq? (car found) 'ok) (cadr found))
                         ((eq? (car found) 'none)
                          (error "direnv allow failed: no .envrc found"))
                         (else
                          (error (string-append "direnv allow failed: " (cadr found))))))
               (canon-target (canonical-path target))
               (root (target-root-dir target)))
          (when expected-target
            (unless (string=? canon-target (strip-trailing-slash expected-target))
              (error "direnv allow: resolved .envrc target changed since confirmation; please retry")))
          (let ((bytes (guard (e ((task-cancelled-condition? e) (raise e))
                                 (#t #f))
                         (read-confined-bytes-nofollow (path-directory canon-target)
                                                       (path-file-name canon-target)
                                                       %default-max-stdout))))
            (unless (bytevector? bytes)
              (error "direnv allow: could not read target .envrc safely"))
            (when expected-digest
              (let ((actual-digest (hex-encode (sha-256 bytes))))
                (unless (string=? actual-digest expected-digest)
                  (error "direnv allow: .envrc contents changed since confirmation; please retry"))))
            (let ((data-dir (direnv-data-dir)))
              (unless (and (string? data-dir) (path-absolute? data-dir))
                (error "direnv allow failed: cannot determine absolute direnv data directory"))
              (let* ((allow-dir (path-join data-dir "allow"))
                     (deny-dir (path-join data-dir "deny"))
                     (allow-hash (direnv-allow-hash target bytes))
                     (deny-hash (direnv-deny-hash target))
                     (record-bytes (string->utf8 (string-append target "\n")))
                     (allow-handle #f)
                     (deny-handle #f))
                (dynamic-wind
                  (lambda () #f)
                  (lambda ()
                    (set! deny-handle (open-pinned-dir deny-dir #f))
                    (when (and deny-handle (pinned-dir-read-bytes deny-handle deny-hash 65536))
                      (error "direnv allow: existing deny record found; clear denial explicitly before approving"))
                    (set! allow-handle (open-pinned-dir allow-dir #t))
                    ;; Serialize Techne approval transactions on the pinned allow directory
                    ;; from initial read through verification and rollback so a failing caller
                    ;; cannot undo another caller's concurrent successful approval.
                    (let ((lock-deadline (+ (current-milliseconds)
                                            (max 5000 (+ (default-direnv-status-timeout) 2000)))))
                      (let loop ()
                        (unless (pinned-dir-try-lock! allow-handle)
                          (when (>= (current-milliseconds) lock-deadline)
                            (error "direnv allow: timed out waiting for approval lock"))
                          (sleep 5)
                          (loop))))
                    (unless deny-handle
                      (set! deny-handle (open-pinned-dir deny-dir #f)))
                    (when (and deny-handle (pinned-dir-read-bytes deny-handle deny-hash 65536))
                      (error "direnv allow: existing deny record found; clear denial explicitly before approving"))
                    (let ((prev-allow (pinned-dir-read-bytes allow-handle allow-hash 65536))
                          (wrote-allow? #f))
                      (define (rollback-err-msg e)
                        (if (error-object? e) (error-object-message e) (repr e)))
                      (define (rollback!)
                        (let ((failures '()))
                          (when wrote-allow?
                            (guard (e (#t (set! failures
                                                (cons (string-append "allow-dir rollback failed: " (rollback-err-msg e))
                                                      failures))))
                              (cond
                                ((not prev-allow)
                                 (pinned-dir-delete! allow-handle allow-hash))
                                ((not (equal? prev-allow record-bytes))
                                 (pinned-dir-write-private! allow-handle allow-hash prev-allow)))))
                          (reverse failures)))
                      (guard (e (#t
                                 (let ((rollback-failures (rollback!)))
                                   (execution-site-invalidate! root)
                                   (cond
                                     ((pair? rollback-failures)
                                      (error (string-append (rollback-err-msg e)
                                                            "; ROLLBACK FAILED (approval may remain active): "
                                                            (string-join rollback-failures "; "))))
                                     ((task-cancelled-condition? e)
                                      (raise e))
                                     (wrote-allow?
                                      (error (string-append (rollback-err-msg e) "; rolled back")))
                                     (else (raise e))))))
                        (unless (equal? prev-allow record-bytes)
                          (pinned-dir-publish-private! allow-handle allow-hash record-bytes)
                          (set! wrote-allow? #t))
                        (%direnv-sync-dir-hook allow-dir allow-handle)
                        (unless deny-handle
                          (set! deny-handle (open-pinned-dir deny-dir #f)))
                        (when (and deny-handle (pinned-dir-read-bytes deny-handle deny-hash 65536))
                          (error "direnv allow: concurrent deny record observed"))
                        (let* ((st (run-endpoint-base root program '("status" "--json")
                                                      #:timeout (default-direnv-status-timeout)
                                                      #:label "direnv"))
                               (ok-status?
                                (and (eq? (car st) 'ok)
                                     (eqv? (cadr st) 0)
                                     (let* ((json (guard (e (#t #f)) (string->json (caddr st))))
                                            (state (and (dict? json) (dict-ref json "state" #f)))
                                            (found-rc (and (dict? state) (dict-ref state "foundRC" #f)))
                                            (found-path (and (dict? found-rc) (dict-ref found-rc "path" #f)))
                                            (allowed (and (dict? found-rc) (dict-ref found-rc "allowed" #f))))
                                       (and (string? found-path)
                                            (string=? found-path target)
                                            (eqv? allowed 0)))))
                               (post-canon (canonical-path target))
                               (target-ok? (if expected-target
                                               (string=? post-canon (strip-trailing-slash expected-target))
                                               (string=? post-canon canon-target))))
                          (unless (and ok-status? target-ok?)
                            (error (cond
                                     ((not target-ok?)
                                      "direnv allow: .envrc target changed during approval")
                                     ((not (and (eq? (car st) 'ok) (eqv? (cadr st) 0)))
                                      "direnv allow: failed to verify approval via direnv status")
                                     (else
                                      "direnv allow: direnv did not accept the digest-bound approval"))))))))
                  (lambda ()
                    (when allow-handle
                      (guard (e (#t #f)) (pinned-dir-close! allow-handle))
                      (set! allow-handle #f))
                    (when deny-handle
                      (guard (e (#t #f)) (pinned-dir-close! deny-handle))
                      (set! deny-handle #f))))
                (execution-site-invalidate! root)
                (execution-site-env-snapshot (make-execution-site canon #:policy 'project) #:refresh? #t)))))))

    (define (execution-site-direnv-deny! dir)
      "Revoke approval for the `.envrc` of local DIR via `direnv deny`.
Fails closed if an inherited startup `direnv` overlay could not be cleanly
unloaded. Because revocation strictly reduces trust, `direnv deny` does not
require interactive confirmation or content-digest pinning. Invalidates the
cache and returns the refreshed `env-snapshot`."
      (when (remote-path? dir)
        (error "direnv: remote direnv deny is not supported yet"))
      (when (eq? (direnv-data-dir-override-xdg) 'invalid)
        (error "direnv deny failed: direnv-data-dir must be an absolute path ending in /direnv"))
      (let ((program (direnv-program)))
        (unless (trusted-baseline-clean? #:direnv-program program)
          (error "direnv deny failed: startup direnv environment could not be cleanly unloaded"))
        (let* ((canon (canonical-local-dir dir))
               (target (direnv-target-path canon program))
               (canon-target (canonical-path target))
               (root (target-root-dir target))
               (cli-arg (direnv-cli-arg target canon-target root))
               (res (run-endpoint-base root program (list "deny" cli-arg)
                                       #:timeout (default-direnv-timeout)
                                       #:label "direnv")))
          (unless (and (eq? (car res) 'ok) (eqv? (cadr res) 0))
            (error (if (eq? (car res) 'ok)
                       (string-append "direnv deny failed (exit " (number->string (cadr res)) ")")
                       (string-append "direnv deny failed: " (symbol->string (car res))))))
          (execution-site-invalidate! root)
          (execution-site-env-snapshot (make-execution-site canon #:policy 'project) #:refresh? #t))))

    ;;; Unified process launch on an execution-site

    (define (spawn-at-site site program args #:pty [pty #f] #:persist [persist #f]
                           #:environment [environment '()] #:stdin [stdin #f]
                           #:stdout [stdout #f] #:stderr [stderr #f])
      "Start PROGRAM with ARGS at execution site SITE; return the process.
Respects SITE's launch policy (`project`, `endpoint-base`, or
`user-trusted`), working directory, remote endpoint, and resolved
environment. Caller ENVIRONMENT overrides site environment; PTY,
PERSIST, STDIN, STDOUT, and STDERR forward to `process-spawn`."
      (let* ((site (execution-site-for site))
             (env (execution-site-resolved-environment site #:environment environment))
             (node (execution-site-endpoint site))
             (dir (execution-site-directory site)))
        (process-spawn program args
                       #:pty pty #:node node #:persist persist
                       #:directory dir #:environment env
                       #:stdin stdin #:stdout stdout #:stderr stderr)))

    (define (call-with-process-at-site site program args f #:pty [pty #f] #:persist [persist #f]
                                       #:environment [environment '()] #:stdin [stdin #f]
                                       #:stdout [stdout #f] #:stderr [stderr #f])
      "Call F with a process running PROGRAM with ARGS at execution site SITE.
Kills the process when F returns, raises an error, or is cancelled.
ENVIRONMENT overrides site environment; PTY, PERSIST, STDIN, STDOUT,
and STDERR forward to `call-with-process`."
      (let* ((site (execution-site-for site))
             (env (execution-site-resolved-environment site #:environment environment))
             (node (execution-site-endpoint site))
             (dir (execution-site-directory site)))
        (call-with-process program args f
                           #:pty pty #:node node #:persist persist
                           #:directory dir #:environment env
                           #:stdin stdin #:stdout stdout #:stderr stderr)))))
