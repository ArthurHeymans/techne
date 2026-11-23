;;; Trusted local secret lookup with an opaque `secret` type and pluggable,
;;; scope-owned backends (`authinfo`, `sops`, `pass`, `env`, and package-
;;; registered backends). Secrets are never cached across queries, never
;;; exposed by `write`/`display`/`repr`/`record-fields`, and never serialized.

(define-library (techne secrets)
  (export make-secret secret? secret-reveal call-with-secret secret-label
          secret-ref secret-ref? secret-ref-backend secret-ref-options secret-resolve
          register-secret-backend! define-secret-backend find-secret-backend secret-backend-names
          default-secret-sources secret-lookup sops-secret)
  (import (techne) (techne authinfo) (techne trusted-process))
  (begin
    (define %max-secret-chars 1048576)

    ;;; Opaque secret value

    ;; Private seals unreachable outside this library: even `record-fields`
    ;; only sees `#<procedure>`, which refuses calls without a private seal.
    (define %value-seal (list 'secret-value))
    (define %label-seal (list 'secret-label))

    (define-record-type secret
      (%make-secret sealed)
      %secret?
      (sealed %secret-sealed))

    (define (secret? obj)
      "Return #t if OBJ is an opaque secret."
      (%secret? obj))

    (define (make-secret value #:label [label #f])
      "Return an opaque secret holding VALUE, with optional LABEL.
VALUE must be a string. Printing, `equal?`, `record-fields` and data
serialization never expose VALUE."
      (unless (string? value)
        (error "make-secret: expected a string"))
      (when (and label (not (string? label)) (not (symbol? label)))
        (error "make-secret: LABEL must be a string, symbol or #f"))
      (let ((lbl (if (symbol? label) (symbol->string label) label)))
        (%make-secret
         (lambda (key)
           (cond ((eq? key %value-seal) value)
                 ((eq? key %label-seal) lbl)
                 (else (error "secret: use secret-reveal to access a secret")))))))

    (define (secret-reveal s)
      "Return the plaintext string inside secret S.
Keep the revealed string strictly at the boundary that needs it."
      (unless (%secret? s)
        (error "secret-reveal: expected a secret"))
      ((%secret-sealed s) %value-seal))

    (define (secret-label s)
      "Return the optional label of secret S, or #f."
      (unless (%secret? s)
        (error "secret-label: expected a secret"))
      ((%secret-sealed s) %label-seal))

    (define (call-with-secret s proc)
      "Call PROC with the revealed plaintext string of secret S."
      (proc (secret-reveal s)))

    ;;; Helpers

    (define (assq-ref alist key [default #f])
      (let ((e (assq key alist))) (if e (cdr e) default)))

    (define (nonempty? s)
      (and (string? s) (> (string-length (string-trim s)) 0)))

    (define (strip-trailing-newlines text)
      (let loop ((n (string-length text)))
        (if (and (> n 0) (let ((c (string-ref text (- n 1)))) (or (char=? c #\newline) (char=? c #\return))))
            (loop (- n 1))
            (substring text 0 n))))

    (define (first-line text)
      (let* ((s (or text ""))
             (i (string-index s #\newline))
             (line (if i (substring s 0 i) s))
             (n (string-length line)))
        (if (and (> n 0) (char=? (string-ref line (- n 1)) #\return))
            (substring line 0 (- n 1))
            line)))

    (define (parse-options items)
      (let loop ((xs items) (out '()))
        (cond ((null? xs) (reverse out))
              ((keyword? (car xs))
               (when (null? (cdr xs))
                 (error "secrets: keyword option missing value" (car xs)))
               (loop (cddr xs)
                     (cons (cons (string->symbol (keyword->string (car xs))) (cadr xs)) out)))
              ((and (pair? (car xs)) (symbol? (caar xs)))
               (loop (cdr xs) (cons (car xs) out)))
              (else (error "secrets: expected #:key value or (key . value) option" (car xs))))))

    ;;; Declarative secret references

    (define-record-type secret-ref-record
      (%make-secret-ref backend options)
      %secret-ref?
      (backend %secret-ref-backend)
      (options %secret-ref-options))

    (define (secret-ref? obj)
      "Return #t if OBJ is a declarative secret reference."
      (%secret-ref? obj))

    (define (secret-ref-backend ref)
      "Return the backend symbol of secret reference REF."
      (unless (%secret-ref? ref) (error "secret-ref-backend: expected a secret-ref"))
      (%secret-ref-backend ref))

    (define (secret-ref-options ref)
      "Return the option alist of secret reference REF."
      (unless (%secret-ref? ref) (error "secret-ref-options: expected a secret-ref"))
      (%secret-ref-options ref))

    (define (secret-ref backend . options)
      "Return a declarative reference resolved by BACKEND with OPTIONS.
BACKEND is a symbol such as `authinfo`, `sops`, `pass` or `env`.
OPTIONS are `#:key value` pairs or `(key . value)` alist entries."
      (unless (symbol? backend)
        (error "secret-ref: BACKEND must be a symbol" backend))
      (%make-secret-ref backend (parse-options options)))

    ;;; Backend registry: library-lifetime built-ins + scope-owned extensions

    (define %builtin-backends (make-hash-table eq?))
    (define %backends (make-registry 'secret-backends))

    (define-record-type secret-backend
      (%make-secret-backend name proc doc)
      %secret-backend?
      (name %secret-backend-name)
      (proc %secret-backend-proc)
      (doc %secret-backend-doc))

    (define (register-builtin-backend! name proc doc)
      (hash-table-set! %builtin-backends name (%make-secret-backend name proc doc)))

    (define (register-secret-backend! name proc #:doc [doc #f])
      "Register secret backend NAME with lookup procedure PROC and DOC.
PROC is called as `(PROC query spec)` where QUERY and SPEC are alists,
and returns a `secret`, a string or #f. The registration and PROC are
bound to the current scope."
      (unless (symbol? name) (error "register-secret-backend!: NAME must be a symbol" name))
      (unless (procedure? proc) (error "register-secret-backend!: PROC must be a procedure"))
      (when (and doc (not (string? doc)))
        (error "register-secret-backend!: DOC must be a string or #f"))
      (registry-add! %backends name (%make-secret-backend name (scope-procedure proc) doc)))

    (define-syntax define-secret-backend
      (syntax-rules ()
        "Define and register a scope-owned secret backend.
`(define-secret-backend (NAME QUERY SPEC) DOC BODY ...)` registers NAME
with docstring DOC so `secret-lookup` and `secret-resolve` can call it."
        ((_ (name query spec) doc body1 body ...)
         (register-secret-backend! 'name (lambda (query spec) body1 body ...) #:doc doc))))

    (define (find-secret-backend name)
      "Return the lookup procedure for secret backend NAME, or #f."
      (let ((b (or (registry-ref %backends name)
                   (hash-table-ref/default %builtin-backends name #f))))
        (and b (%secret-backend-proc b))))

    (define (secret-backend-names)
      "Return the names of all registered secret backends."
      (delete-duplicates (append (registry-keys %backends) (hash-table-keys %builtin-backends))))

    ;;; Query matching, destination guards & normalization

    (define (value-matches? expected actual)
      (cond ((not expected) #t)
            ((not actual) #f)
            ((list? expected) (any (lambda (e) (value-matches? e actual)) expected))
            ((and (string? expected) (string? actual)) (string-ci=? expected actual))
            (else (equal? expected actual))))

    (define (spec-applies? query spec)
      (and (value-matches? (assq-ref spec 'when-service) (assq-ref query 'service))
           (value-matches? (assq-ref spec 'when-host) (assq-ref query 'host))
           (value-matches? (assq-ref spec 'when-user) (assq-ref query 'user))
           (value-matches? (assq-ref spec 'when-account) (assq-ref query 'account))))

    (define (normalize-sops-format fmt)
      (let ((s (cond ((not fmt) 'auto)
                     ((symbol? fmt) (string->symbol (string-downcase (symbol->string fmt))))
                     ((string? fmt) (string->symbol (string-downcase (string-trim fmt))))
                     (else (error "sops: invalid format" fmt)))))
        (case s
          ((auto) 'auto)
          ((yaml yml) 'yaml)
          ((json) 'json)
          ((dotenv env) 'dotenv)
          ((ini) 'ini)
          ((binary raw) 'binary)
          ((authinfo) 'authinfo)
          (else (error "sops: unsupported format" fmt)))))

    (define (source-has-guard? name spec)
      (or (assq-ref spec 'when-service)
          (assq-ref spec 'when-host)
          (assq-ref spec 'when-user)
          (assq-ref spec 'when-account)
          (and (eq? name 'env)
               (or (assq-ref spec 'service)
                   (assq-ref spec 'host)
                   (assq-ref spec 'user)
                   (assq-ref spec 'account)))))

    (define (fixed-value-source? name spec)
      (case name
        ((env) #t)
        ((sops) (let ((fmt (normalize-sops-format (assq-ref spec 'format 'auto))))
                  (or (assq-ref spec 'key)
                      (assq-ref spec 'extract)
                      (eq? fmt 'binary)
                      (and (eq? fmt 'authinfo) (assq-ref spec 'host) #t))))
        ((pass) (and (assq-ref spec 'entry) #t))
        ((authinfo) (and (assq-ref spec 'host) #t))
        (else #f)))

    (define (normalize-result result backend)
      (cond ((not result) #f)
            ((%secret? result) result)
            ((string? result)
             (and (> (string-length result) 0)
                  (make-secret result #:label (symbol->string backend))))
            (else (error "secrets: backend returned neither a secret, string nor #f" backend))))

    (define (normalize-source source)
      (cond ((symbol? source) (cons source '()))
            ((%secret-ref? source) (cons (%secret-ref-backend source) (%secret-ref-options source)))
            ((and (pair? source) (symbol? (car source)))
             (cons (car source) (parse-options (cdr source))))
            (else (error "secrets: invalid secret source specification" source))))

    (define (invoke-source source query #:shared? [shared? #f])
      (let* ((norm (normalize-source source))
             (name (car norm))
             (spec (cdr norm)))
        (when (and shared? (fixed-value-source? name spec) (not (source-has-guard? name spec)))
          (error "secrets: fixed source requires #:when-host, #:when-service or #:when-account guard" name))
        (if (not (spec-applies? query spec))
            #f
            (let ((proc (find-secret-backend name)))
              (cond (proc (normalize-result (proc query spec) name))
                    ;; In shared `secret-sources`, skip backends whose package
                    ;; is not loaded yet (e.g. `gh-cli` before Forge loads).
                    (shared? #f)
                    (else (error "secrets: unknown secret backend" name)))))))

    (define default-secret-sources
      (make-parameter '((authinfo)))
      "Parameter holding the default ordered list of secret sources.
Each entry is a backend name, `(BACKEND option ...)`, or a `secret-ref`.")

    (define (secret-resolve ref #:query [query '()])
      "Resolve secret reference REF with optional QUERY alist.
Return an opaque `secret` or #f if no matching secret was found."
      (unless (%secret-ref? ref)
        (error "secret-resolve: expected a secret-ref"))
      (invoke-source ref query #:shared? #f))

    (define (secret-lookup #:service [service #f] #:host [host #f] #:user [user #f]
                           #:port [port #f] #:key [key #f] #:account [account #f]
                           #:extra [extra '()] #:sources [sources #f])
      "Look up a secret across SOURCES matching the given query fields.
Match SERVICE, HOST, USER, PORT, KEY, ACCOUNT and EXTRA (an optional
alist of backend-specific fields). SOURCES defaults to
`(default-secret-sources)`. Return an opaque `secret` or #f."
      (let* ((query (append (if service (list (cons 'service service)) '())
                            (if host (list (cons 'host host)) '())
                            (if user (list (cons 'user user)) '())
                            (if port (list (cons 'port port)) '())
                            (if key (list (cons 'key key)) '())
                            (if account (list (cons 'account account)) '())
                            extra))
             (srcs (or sources (default-secret-sources))))
        (let loop ((xs srcs))
          (if (null? xs) #f
              (or (invoke-source (car xs) query #:shared? #t)
                  (loop (cdr xs)))))))

    ;;; Shared authinfo matching (single read/decrypt per file)

    ;; When USER is explicitly requested, never fall through to another user's entry.
    (define (match-authinfo-entries entries host user port login-tag allow-any-login?)
      (if (nonempty? user)
          (let ((tagged-user (and (nonempty? login-tag) (string-append user login-tag))))
            (or (and tagged-user (authinfo-match entries #:host host #:user tagged-user #:port port))
                (authinfo-match entries #:host host #:user user #:port port)))
          (if (nonempty? login-tag)
              (or (authinfo-match entries #:host host #:port port #:login-suffix login-tag)
                  (and allow-any-login? (authinfo-match entries #:host host #:port port)))
              (authinfo-match entries #:host host #:port port))))

    ;;; Built-in backend: authinfo (.authinfo.gpg / .authinfo)

    (register-builtin-backend!
     'authinfo
     (lambda (query spec)
       (let* ((host (or (assq-ref spec 'host) (assq-ref query 'host)))
              (user (or (assq-ref spec 'user) (assq-ref query 'user)))
              (port (or (assq-ref spec 'port) (assq-ref query 'port)))
              (file (assq-ref spec 'file))
              (gpg (assq-ref spec 'gpg "gpg"))
              (login-tag (or (assq-ref spec 'login-tag) (assq-ref query 'login-tag)))
              (allow-any? (or (assq-ref spec 'allow-any-login) (assq-ref query 'allow-any-login)))
              (files (cond ((not file) (authinfo-default-files))
                           ((list? file) file)
                           (else (list file)))))
         (and host
              (let loop ((fs files))
                (if (null? fs) #f
                    (let* ((entries (authinfo-entries #:file (car fs) #:gpg gpg))
                           (pw (and (pair? entries)
                                    (match-authinfo-entries entries host user port login-tag allow-any?))))
                      (or pw (loop (cdr fs)))))))))
     "Look up credentials in plain or GPG-encrypted authinfo files.")

    ;;; Built-in backend: SOPS (read-only lookup via `sops decrypt`)

    (define (sops-known-extension? path)
      (let ((lower (string-downcase path)))
        (any (lambda (ext) (string-suffix? ext lower))
             '(".yaml" ".yml" ".json" ".env" ".ini"))))

    (define (sops-run-decrypt path #:extract [extract #f] #:input-type [input-type #f]
                              #:output-type [output-type #f] #:sops [sops "sops"])
      (let* ((resolved (expand-trusted-home path))
             (args (append (list "decrypt")
                           (if input-type (list "--input-type" (if (symbol? input-type) (symbol->string input-type) input-type)) '())
                           (if output-type (list "--output-type" (if (symbol? output-type) (symbol->string output-type) output-type)) '())
                           (if extract (list "--extract" extract) '())
                           (list "--" resolved)))
             (res (run-trusted-tool sops args #:max-stdout %max-secret-chars #:strict-baseline #t #:label "sops"))
             (status (car res))
             (out (cadr res)))
        (unless (equal? status 0)
          ;; Fixed, redacted error: never forward raw stderr or swallow decryption errors.
          (error (string-append "sops: decryption failed for " path)))
        out))

    (define (json-path-ref obj path)
      (let loop ((cur obj) (ks path))
        (cond ((null? ks) cur)
              ((not cur) #f)
              ((dict? cur)
               (let* ((k (car ks))
                      (kstr (if (symbol? k) (symbol->string k) (if (string? k) k #f))))
                 (and kstr (dict-contains? cur kstr) (loop (dict-ref cur kstr) (cdr ks)))))
              ((and (vector? cur) (integer? (car ks)) (exact? (car ks)) (>= (car ks) 0) (< (car ks) (vector-length cur)))
               (loop (vector-ref cur (car ks)) (cdr ks)))
              (else #f))))

    (define (node-matches-user? node want-user)
      (if (not (and (nonempty? want-user) (dict? node)))
          #t
          (let ((stored (cond ((dict-contains? node "user") (dict-ref node "user"))
                              ((dict-contains? node "login") (dict-ref node "login"))
                              (else #f))))
            (or (not stored) (equal? stored want-user)))))

    (define (extract-leaf-string node #:user [user #f])
      (cond ((string? node) node)
            ((and (dict? node) (node-matches-user? node user))
             (or (and (dict-contains? node "password") (string? (dict-ref node "password")) (dict-ref node "password"))
                 (and (dict-contains? node "token") (string? (dict-ref node "token")) (dict-ref node "token"))
                 (and (dict-contains? node "secret") (string? (dict-ref node "secret")) (dict-ref node "secret"))))
            (else #f)))

    (define (normalize-key-path key)
      (cond ((list? key)
             (when (null? key) (error "sops: key path cannot be empty"))
             key)
            (else (list key))))

    (define (sops-structured-lookup data query)
      (let* ((service (assq-ref query 'service))
             (host (assq-ref query 'host))
             (user (assq-ref query 'user))
             (account (assq-ref query 'account))
             (login-tag (assq-ref query 'login-tag))
             (tagged-user (and (nonempty? user) (nonempty? login-tag) (string-append user login-tag)))
             (svc (and service (if (symbol? service) (symbol->string service) service)))
             (candidates
              (filter-map
               (lambda (p) (and (every values p) p))
               (list (list svc host tagged-user)
                     (list svc host user)
                     (list host tagged-user)
                     (list host user)
                     (list svc host)
                     (list host)
                     (list svc account)
                     (list account)))))
        (let loop ((paths candidates))
          (if (null? paths) #f
              (or (extract-leaf-string (json-path-ref data (car paths)) #:user user)
                  (loop (cdr paths)))))))

    (define (sops-secret file #:key [key #f] #:extract [extract #f]
                         #:input-type [input-type #f] #:output-type [output-type #f]
                         #:trim? [trim? #t] #:sops [sops "sops"])
      "Decrypt SOPS FILE and return an opaque secret, or #f if absent.
KEY (a string, symbol, integer or path list) decrypts as JSON and walks
the path locally so missing keys return #f while decryption errors
raise. EXTRACT passes a raw `--extract` expression to `sops`.
INPUT-TYPE and OUTPUT-TYPE override SOPS format detection; TRIM? strips
trailing newlines; SOPS sets the binary."
      (let ((resolved (expand-trusted-home file)))
        (if (not (file-exists? resolved)) #f
            (cond
              (extract
               (let* ((raw (sops-run-decrypt resolved #:extract extract #:input-type input-type
                                             #:output-type output-type #:sops sops))
                      (val (if trim? (strip-trailing-newlines raw) raw)))
                 (and (> (string-length val) 0)
                      (make-secret val #:label "sops"))))
              (key
               (let* ((json-text (sops-run-decrypt resolved #:input-type input-type
                                                   #:output-type "json" #:sops sops))
                      (data (string->json json-text))
                      (node (json-path-ref data (normalize-key-path key)))
                      (leaf (extract-leaf-string node)))
                 (and leaf
                      (let ((val (if trim? (strip-trailing-newlines leaf) leaf)))
                        (and (> (string-length val) 0)
                             (make-secret val #:label "sops"))))))
              (else
               (let* ((itype (or input-type
                                 (and (not (sops-known-extension? resolved)) "binary")))
                      (otype (or output-type
                                 (and (equal? itype "binary") "binary")))
                      (raw (sops-run-decrypt resolved #:input-type itype
                                             #:output-type otype #:sops sops))
                      (val (if trim? (strip-trailing-newlines raw) raw)))
                 (and (> (string-length val) 0)
                      (make-secret val #:label "sops"))))))))

    (register-builtin-backend!
     'sops
     (lambda (query spec)
       (let ((file (assq-ref spec 'file)))
         (and file
              (let ((resolved (expand-trusted-home file)))
                (if (not (file-exists? resolved)) #f
                    (let* ((fmt (normalize-sops-format (assq-ref spec 'format 'auto)))
                           (sops-bin (assq-ref spec 'sops "sops"))
                           (itype (assq-ref spec 'input-type))
                           (otype (assq-ref spec 'output-type))
                           (trim? (assq-ref spec 'trim? #t))
                           (key (or (assq-ref spec 'key) (assq-ref query 'key)))
                           (extract (assq-ref spec 'extract)))
                      (cond
                        ((eq? fmt 'authinfo)
                         (let* ((host (or (assq-ref spec 'host) (assq-ref query 'host)))
                                (user (or (assq-ref spec 'user) (assq-ref query 'user)))
                                (port (or (assq-ref spec 'port) (assq-ref query 'port)))
                                (login-tag (or (assq-ref spec 'login-tag) (assq-ref query 'login-tag)))
                                (allow-any? (or (assq-ref spec 'allow-any-login) (assq-ref query 'allow-any-login)))
                                (eff-in (or itype (if (sops-known-extension? resolved) #f "binary")))
                                (eff-out (or otype (if (equal? eff-in "binary") "binary" #f))))
                           (and host
                                (let ((text (sops-run-decrypt resolved #:input-type eff-in #:output-type eff-out #:sops sops-bin)))
                                  (match-authinfo-entries (authinfo-parse text) host user port login-tag allow-any?)))))
                        (extract
                         (sops-secret resolved #:extract extract #:input-type itype
                                      #:output-type otype #:trim? trim? #:sops sops-bin))
                        (key
                         (let ((eff-in (or itype (if (memq fmt '(json yaml dotenv ini)) fmt #f))))
                           (sops-secret resolved #:key key #:input-type eff-in
                                        #:trim? trim? #:sops sops-bin)))
                        ((eq? fmt 'binary)
                         (sops-secret resolved #:input-type (or itype "binary")
                                      #:output-type (or otype "binary") #:trim? trim? #:sops sops-bin))
                        (else
                         (let* ((eff-in (or itype (if (memq fmt '(json yaml dotenv ini)) fmt #f)))
                                (json-text (sops-run-decrypt resolved #:input-type eff-in #:output-type "json" #:sops sops-bin))
                                (data (string->json json-text)))
                           (sops-structured-lookup data query))))))))))
     "Look up credentials from a SOPS-encrypted file via `sops decrypt`.")

    ;;; Built-in backend: pass (standard Unix password store)

    (define (join-pass-path prefix name)
      (cond ((or (not prefix) (string=? prefix "")) name)
            ((string-suffix? "/" prefix) (string-append prefix name))
            (else (string-append prefix "/" name))))

    (register-builtin-backend!
     'pass
     (lambda (query spec)
       (let* ((home (trusted-home))
              (raw-store (or (assq-ref spec 'store)
                             (trusted-environment-variable "PASSWORD_STORE_DIR")
                             (and home (string-append home "/.password-store")))))
         (and (nonempty? raw-store)
              (let* ((store (expand-trusted-home raw-store))
                     (prefix (assq-ref spec 'prefix ""))
                     (pass-bin (assq-ref spec 'pass "pass"))
                     (explicit (or (assq-ref spec 'entry) (assq-ref query 'key)))
                     (service (assq-ref query 'service))
                     (svc (and service (if (symbol? service) (symbol->string service) service)))
                     (host (assq-ref query 'host))
                     (user (assq-ref query 'user))
                     (account (assq-ref query 'account))
                     (raw-candidates
                      (if explicit
                          (list (if (symbol? explicit) (symbol->string explicit) explicit))
                          (filter-map
                           (lambda (parts)
                             (and (every nonempty? parts) (string-join parts "/")))
                           (list (list svc host user)
                                 (list host user)
                                 (list svc account)
                                 (list svc host)
                                 (list host)))))
                     (candidates (map (lambda (c) (join-pass-path prefix c)) raw-candidates))
                     (chosen (find (lambda (name) (file-exists? (string-append store "/" name ".gpg"))) candidates)))
                (and chosen
                     (let* ((env (if (assq-ref spec 'store) (list (cons "PASSWORD_STORE_DIR" store)) '()))
                            (res (run-trusted-tool pass-bin (list "show" "--" chosen)
                                                   #:environment env
                                                   #:max-stdout %max-secret-chars
                                                   #:strict-baseline #t
                                                   #:label "pass"))
                            (status (car res))
                            (out (cadr res)))
                       (unless (equal? status 0)
                         (error (string-append "pass: lookup failed for " chosen)))
                       (let ((pw (first-line out)))
                         (and (> (string-length pw) 0) pw))))))))
     "Look up credentials in a standard Unix `pass` password store.")

    ;;; Built-in backend: env (explicitly scoped environment variables)

    (register-builtin-backend!
     'env
     (lambda (query spec)
       (let ((host-guard (assq-ref spec 'host))
             (svc-guard (assq-ref spec 'service))
             (user-guard (assq-ref spec 'user))
             (acct-guard (assq-ref spec 'account)))
         (and (value-matches? host-guard (assq-ref query 'host))
              (value-matches? svc-guard (assq-ref query 'service))
              (value-matches? user-guard (assq-ref query 'user))
              (value-matches? acct-guard (assq-ref query 'account))
              (let* ((var (assq-ref spec 'var))
                     (vars (or (assq-ref spec 'vars) (and var (list var)) '())))
                (let loop ((vs vars))
                  (if (null? vs) #f
                      (let ((val (trusted-environment-variable (car vs))))
                        (if (nonempty? val)
                            (string-trim val)
                            (loop (cdr vs))))))))))
     "Look up a secret from explicitly configured environment variables.")))
