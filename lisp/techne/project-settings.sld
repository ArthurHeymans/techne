;;; Headless per-project settings (`.techne.scm` + `.editorconfig`),
;;; compiled selectors, JSON-object overlay merging, and value-based trust.
;;;
;;; Security invariants:
;;; - `.techne.scm` is parsed with pure `read`, NEVER `eval`.
;;; - Unclassified options are risky by default and remain inert until the
;;;   user principal approves the exact `(scope, option, effective-value)`.
;;; - `#:user-only` options are rejected unconditionally from project files.
;;; - Option lookup against a cached snapshot performs zero filesystem I/O.

(define-library (techne project-settings)
  (export default-project-settings-max-bytes
          default-trust-store-path
          set-trust-principal-hook!
          json-unset json-unset?
          json-object-strip-unset json-object-overlay json-object-compose-overlays
          canonical-value-string value-trust-digest
          settings-rule? settings-rule-selector-kind settings-rule-selector-arg
          settings-rule-option settings-rule-value
          settings-rule-source-path settings-rule-source-dir
          settings-diagnostic? settings-diagnostic-path
          settings-diagnostic-severity settings-diagnostic-message
          project-settings-file? project-settings-file-path
          project-settings-file-directory project-settings-file-version
          project-settings-file-root? project-settings-file-rules
          project-settings-file-diagnostics
          parse-project-settings-string parse-project-settings-file
          project-settings-discover
          project-settings-snapshot? project-settings-snapshot-directory
          project-settings-snapshot-generation
          project-settings-snapshot-techne-files
          project-settings-snapshot-editorconfig-files
          project-settings-snapshot-diagnostics
          project-settings-generation
          project-settings-canonical-dir
          project-settings-load! project-settings-cached-snapshot
          project-settings-invalidate! project-settings-invalidate-ancestors!
          project-settings-watched-path?
          settings-context? make-settings-context
          settings-context-directory settings-context-path
          settings-context-modes settings-context-snapshot
          project-trust-verdict project-trust-approve!
          project-trust-deny! project-trust-revoke!
          project-trust-entries project-trust-load! project-trust-clear-memory!
          project-settings-resolve-option
          project-settings-collect-pending
          project-settings-collect-all-pending
          set-trust-store-path-hook!)
  (import (techne) (techne path) (techne crypto) (techne location) (techne editorconfig)
          (techne project-settings params))
  (begin

    (define default-project-settings-max-bytes
      (make-parameter
       262144
       (lambda (v)
         (unless (and (integer? v) (exact? v) (> v 0))
           (error "default-project-settings-max-bytes: expected positive exact integer" v))
         v))
      "Maximum size in bytes accepted when reading a `.techne.scm` file.")

    (define %trust-store-path-hook (lambda () #f))

    (define (set-trust-store-path-hook! proc)
      "Register PROC returning the default trust-store path.
Used when `default-trust-store-path` is `#f`."
      (unless (procedure? proc)
        (error "set-trust-store-path-hook!: expected procedure" proc))
      (set! %trust-store-path-hook proc))

    (define (effective-trust-store-path)
      (let ((p (default-trust-store-path)))
        (cond ((eq? p 'none) #f)
              ((string? p) (string-copy p))
              (else (let ((h (%trust-store-path-hook)))
                      (cond ((string? h) (string-copy h))
                            ((eq? h 'untrusted-baseline) 'untrusted-baseline)
                            (else #f)))))))

    (define %trust-principal-hook (lambda () 'user))

    (define (set-trust-principal-hook! proc)
      "Register PROC returning the active principal symbol for trust writes.
Only `'user` (or `\"user\"`) may approve, deny, or revoke project-setting
trust."
      (unless (procedure? proc)
        (error "set-trust-principal-hook!: expected procedure" proc))
      (set! %trust-principal-hook proc))

    (define (check-user-principal! who)
      (let ((p (%trust-principal-hook)))
        (unless (or (eq? p 'user) (equal? p "user"))
          (error (string-append who ": only the user principal may change project trust") p))))

    ;;; Path normalization helpers

    (define (has-parent-component? p)
      (and (string? p)
           (any (lambda (seg) (string=? seg "..")) (string-split p "/"))))

    (define (local-absolute-path? p)
      (and (string? p)
           (string-prefix? "/" p)
           (not (remote-path? p))
           (not (has-parent-component? p))))

    (define (normalize-lexical-path p)
      (if (not (string? p))
          "/"
          (let* ((abs? (string-prefix? "/" p))
                 (parts (string-split p "/"))
                 (stack (let loop ((xs parts) (acc '()))
                          (if (null? xs)
                              (reverse acc)
                              (let ((seg (car xs)))
                                (cond ((or (string=? seg "") (string=? seg "."))
                                       (loop (cdr xs) acc))
                                      ((string=? seg "..")
                                       (loop (cdr xs) (if (pair? acc) (cdr acc) acc)))
                                      (else (loop (cdr xs) (cons seg acc)))))))))
            (if abs?
                (if (null? stack) "/" (string-append "/" (string-join stack "/")))
                (string-join stack "/")))))

    (define (ensure-trailing-slash dir)
      (cond ((or (not (string? dir)) (string=? dir "")) "/")
            ((remote-path? dir)
             (if (string-suffix? "/" dir) dir (string-append dir "/")))
            (else
             (let ((norm (normalize-lexical-path dir)))
               (cond ((string=? norm "") "/")
                     ((string-suffix? "/" norm) norm)
                     (else (string-append norm "/")))))))

    (define (parent-dir dir)
      (let* ((d (ensure-trailing-slash dir))
             (n (string-length d)))
        (if (<= n 1)
            #f
            (let ((trimmed (substring d 0 (- n 1))))
              (let loop ((i (- (string-length trimmed) 1)))
                (cond ((< i 0) "/")
                      ((char=? (string-ref trimmed i) #\/)
                       (substring trimmed 0 (+ i 1)))
                      (else (loop (- i 1)))))))))

    (define (project-settings-canonical-dir dir)
      "Return the canonical local directory path for DIR with trailing `/`.
Resolves symlinks and validates UTF-8 via `canonical-dir-path`. Raises an
error if DIR contains an unresolvable `..` component, points through a
dangling symlink, names a non-directory, or resolves to a non-UTF-8 path."
      (cond
        ((and (string? dir) (string-prefix? "/" dir) (not (remote-path? dir)))
         (let* ((stripped (if (and (> (string-length dir) 1) (string-suffix? "/" dir))
                              (substring dir 0 (- (string-length dir) 1))
                              dir))
                (real (canonical-dir-path stripped)))
           (when (or (not (string? real))
                     (not (string-prefix? "/" real))
                     (has-parent-component? real))
             (error "project-settings-canonical-dir: cannot resolve directory path" dir))
           (ensure-trailing-slash real)))
        ((has-parent-component? dir)
         (error "project-settings-canonical-dir: cannot resolve path containing '..'" dir))
        (else
         (ensure-trailing-slash dir))))

    (define (canonical-dir dir)
      (project-settings-canonical-dir dir))

    (define (path-depth dir)
      (let ((d (ensure-trailing-slash dir)))
        (if (string=? d "/")
            0
            (let loop ((i 1) (n (string-length d)) (count 0))
              (cond ((>= i n) count)
                    ((char=? (string-ref d i) #\/) (loop (+ i 1) n (+ count 1)))
                    (else (loop (+ i 1) n count)))))))

    (define (relative-path-under base-dir file-path)
      (let ((b (ensure-trailing-slash base-dir)))
        (cond
          ((not (string? file-path)) #f)
          ((string-prefix? b file-path)
           (substring file-path (string-length b) (string-length file-path)))
          (else (path-file-name file-path)))))

    (define (vc-root-dir? dir)
      (let ((d (ensure-trailing-slash dir)))
        (and (local-absolute-path? d)
             (or (file-exists? (string-append d ".git"))
                 (file-exists? (string-append d ".jj"))))))

    (define %max-datum-nodes 65536)

    (define (consume-node! budget who)
      (let ((rem (vector-ref budget 0)))
        (when (<= rem 0)
          (error (string-append who ": data exceeds maximum node budget")))
        (vector-set! budget 0 (- rem 1))))

    (define (contains-json-unset? val)
      (cond
        ((json-unset? val) #t)
        ((dict? val)
         (any (lambda (k) (contains-json-unset? (dict-ref val k))) (dict-keys val)))
        ((vector? val)
         (let ((n (vector-length val)))
           (let loop ((i 0))
             (and (< i n)
                  (or (contains-json-unset? (vector-ref val i))
                      (loop (+ i 1)))))))
        ((pair? val)
         (or (contains-json-unset? (car val)) (contains-json-unset? (cdr val))))
        (else #f)))

    ;;; Canonical value serialization and SHA-256 digest

    (define (write-string-escaped s port)
      (write s port))

    (define (write-canonical val port depth budget)
      (when (> depth 64)
        (error "project-settings: value nesting exceeds maximum depth"))
      (consume-node! budget "project-settings")
      (cond
        ((json-unset? val) (display "(unset)" port))
        ((boolean? val) (display (if val "#t" "#f") port))
        ((null? val) (display "()" port))
        ((number? val)
         (display "(num " port)
         (write val port)
         (display ")" port))
        ((string? val)
         (display "(str " port)
         (write-string-escaped val port)
         (display ")" port))
        ((symbol? val)
         (display "(sym " port)
         (write-string-escaped (symbol->string val) port)
         (display ")" port))
        ((keyword? val)
         (display "(kw " port)
         (write-string-escaped (keyword->string val) port)
         (display ")" port))
        ((char? val)
         (display "(chr " port)
         (display (char->integer val) port)
         (display ")" port))
        ((bytevector? val)
         (display "(bv " port)
         (display (hex-encode val) port)
         (display ")" port))
        ((vector? val)
         (display "(vec" port)
         (let ((n (vector-length val)))
           (let loop ((i 0))
             (when (< i n)
               (display " " port)
               (write-canonical (vector-ref val i) port (+ depth 1) budget)
               (loop (+ i 1)))))
         (display ")" port))
        ((dict? val)
         (display "(dict" port)
         (let* ((raw-keys (dict-keys val))
                (key-pairs (map (lambda (k)
                                  (cons (call-with-output-string (lambda (p) (write-canonical k p (+ depth 1) budget)))
                                        k))
                                raw-keys))
                (sorted (sort key-pairs (lambda (a b) (string<? (car a) (car b))))))
           (for-each (lambda (kp)
                       (display " (" port)
                       (display (car kp) port)
                       (display " " port)
                       (write-canonical (dict-ref val (cdr kp)) port (+ depth 1) budget)
                       (display ")" port))
                     sorted))
         (display ")" port))
        ((list? val)
         (display "(list" port)
         (for-each (lambda (x)
                     (display " " port)
                     (write-canonical x port (+ depth 1) budget))
                   val)
         (display ")" port))
        ((pair? val)
         (display "(pair " port)
         (write-canonical (car val) port (+ depth 1) budget)
         (display " " port)
         (write-canonical (cdr val) port (+ depth 1) budget)
         (display ")" port))
        (else
         (error "project-settings: value cannot be canonically serialized" val))))

    (define (canonical-value-string val)
      "Return a deterministic canonical string representation of settings VAL."
      (call-with-output-string (lambda (p) (write-canonical val p 0 (vector %max-datum-nodes)))))

    (define (value-trust-digest scope option-name effective-value)
      "Return the SHA-256 hex digest for trusting OPTION-NAME in SCOPE.
Binds lexically normalized SCOPE, OPTION-NAME, and the canonical
serialization of EFFECTIVE-VALUE without performing filesystem I/O."
      (let* ((norm-scope (ensure-trailing-slash scope))
             (payload (call-with-output-string
                       (lambda (p)
                         (display "techne-trust-v2\n" p)
                         (write-string-escaped norm-scope p)
                         (newline p)
                         (write-string-escaped (symbol->string option-name) p)
                         (newline p)
                         (write-canonical effective-value p 0 (vector %max-datum-nodes))))))
        (hex-encode (sha-256 payload))))

    ;;; Pure data conversion for `.techne.scm` values (read-only, never eval)

    (define (datum->settings-value datum depth budget allow-unset? in-dict-val?)
      (when (> depth 64)
        (error "project-settings: datum nesting exceeds maximum depth"))
      (consume-node! budget "project-settings")
      (cond
        ((or (boolean? datum) (number? datum) (string? datum)
             (symbol? datum) (keyword? datum) (char? datum) (bytevector? datum))
         datum)
        ((null? datum) '())
        ((vector? datum)
         (let* ((n (vector-length datum))
                (out (make-vector n)))
           (let loop ((i 0))
             (when (< i n)
               (vector-set! out i (datum->settings-value (vector-ref datum i) (+ depth 1) budget #f #f))
               (loop (+ i 1))))
           out))
        ((pair? datum)
         (cond
           ((not (list? datum))
            (error "project-settings: improper list is not allowed in settings data"))
           ((and (eq? (car datum) 'unset) (null? (cdr datum)))
            (if (and allow-unset? in-dict-val?)
                json-unset
                (error "project-settings: `(unset)` is only valid as a direct dict entry value")))
           ((eq? (car datum) 'dict)
            (let ((args (cdr datum)))
              (when (odd? (length args))
                (error "project-settings: (dict ...) requires an even number of key/value items"))
              (let loop ((xs args) (acc (dict)))
                (if (null? xs)
                    acc
                    (let ((k (car xs))
                          (v (datum->settings-value (cadr xs) (+ depth 1) budget allow-unset? allow-unset?)))
                      (unless (or (string? k) (symbol? k))
                        (error "project-settings: dict key must be a string or symbol" k))
                      (let ((key (if (symbol? k) (symbol->string k) k)))
                        (loop (cddr xs) (dict-set acc key v))))))))
           (else
            (map (lambda (x) (datum->settings-value x (+ depth 1) budget #f #f)) datum))))
        (else
         (error "project-settings: unsupported datum in settings file" datum))))

    (define (clone-settings-value val)
      (cond
        ((or (boolean? val) (number? val) (symbol? val) (keyword? val)
             (char? val) (null? val) (json-unset? val))
         val)
        ((string? val) (string-copy val))
        ((bytevector? val) (bytevector-copy val))
        ((pair? val) (cons (clone-settings-value (car val)) (clone-settings-value (cdr val))))
        ((vector? val)
         (let* ((n (vector-length val))
                (out (make-vector n)))
           (let loop ((i 0))
             (when (< i n)
               (vector-set! out i (clone-settings-value (vector-ref val i)))
               (loop (+ i 1))))
           out))
        ((dict? val)
         (let loop ((keys (dict-keys val)) (out (dict)))
           (if (null? keys)
               out
               (let ((k (car keys)))
                 (loop (cdr keys)
                       (dict-set out (clone-settings-value k)
                                 (clone-settings-value (dict-ref val k))))))))
        ((hash-table? val)
         (let ((out (make-hash-table)))
           (for-each (lambda (kv)
                       (hash-table-set! out
                                        (clone-settings-value (car kv))
                                        (clone-settings-value (cdr kv))))
                     (hash-table->alist val))
           out))
        (else val)))

    (define (shallow-list-copy xs)
      (map (lambda (x) x) xs))

    ;;; Records

    (define-record-type settings-rule
      (%make-settings-rule selector-kind selector-arg matcher option value source-path source-dir)
      settings-rule?
      (selector-kind settings-rule-selector-kind)
      (selector-arg %settings-rule-selector-arg)
      (matcher %settings-rule-matcher)
      (option settings-rule-option)
      (value %settings-rule-value)
      (source-path %settings-rule-source-path)
      (source-dir %settings-rule-source-dir))

    (define (settings-rule-selector-arg r)
      "Return the selector argument of settings rule R."
      (clone-settings-value (%settings-rule-selector-arg r)))
    (define (settings-rule-value r)
      "Return the option value of settings rule R."
      (clone-settings-value (%settings-rule-value r)))
    (define (settings-rule-source-path r)
      "Return the source `.techne.scm` file path of settings rule R."
      (clone-settings-value (%settings-rule-source-path r)))
    (define (settings-rule-source-dir r)
      "Return the source directory of settings rule R."
      (clone-settings-value (%settings-rule-source-dir r)))

    (define-record-type settings-diagnostic
      (%make-settings-diagnostic path severity message)
      settings-diagnostic?
      (path %settings-diagnostic-path)
      (severity settings-diagnostic-severity)
      (message %settings-diagnostic-message))

    (define (settings-diagnostic-path d)
      "Return the file path associated with settings diagnostic D."
      (clone-settings-value (%settings-diagnostic-path d)))
    (define (settings-diagnostic-message d)
      "Return the message string of settings diagnostic D."
      (clone-settings-value (%settings-diagnostic-message d)))

    (define-record-type settings-file
      (%make-project-settings-file path directory version root? rules diagnostics)
      project-settings-file?
      (path %project-settings-file-path)
      (directory %project-settings-file-directory)
      (version project-settings-file-version)
      (root? project-settings-file-root?)
      (rules %project-settings-file-rules)
      (diagnostics %project-settings-file-diagnostics))

    (define (project-settings-file-path f)
      "Return the file path of parsed project settings file F."
      (clone-settings-value (%project-settings-file-path f)))
    (define (project-settings-file-directory f)
      "Return the directory of parsed project settings file F."
      (clone-settings-value (%project-settings-file-directory f)))
    (define (project-settings-file-rules f)
      "Return the list of rules in parsed project settings file F."
      (shallow-list-copy (%project-settings-file-rules f)))
    (define (project-settings-file-diagnostics f)
      "Return the list of diagnostics in parsed project settings file F."
      (shallow-list-copy (%project-settings-file-diagnostics f)))

    ;;; `.techne.scm` parser (pure `read`, never `eval`)

    (define (parse-project-settings-string text #:path [path #f] #:directory [directory #f])
      "Parse `.techne.scm` TEXT using pure `read` without `eval`.
Returns a `project-settings-file` containing compiled rules and any
diagnostics. Optional PATH and DIRECTORY record the file's location."
      (let* ((dir (ensure-trailing-slash (or directory (and path (path-directory path)) "/")))
             (label (or path (string-append dir ".techne.scm")))
             (file-budget (vector %max-datum-nodes))
             (diags '())
             (rules '())
             (root? #f))
        (define (diag! sev msg)
          (set! diags (cons (%make-settings-diagnostic label sev msg) diags)))
        (define (add-bindings! sel-kind sel-arg matcher bindings)
          (for-each
           (lambda (entry)
             (cond
               ((and (list? entry) (= (length entry) 2) (symbol? (car entry)))
                (guard (e (#t (diag! 'error
                                     (string-append "Option `" (symbol->string (car entry)) "`: "
                                                    (if (error-object? e) (error-object-message e) "invalid datum")))))
                  (let ((val (datum->settings-value (cadr entry) 0 file-budget #t #f)))
                    (set! rules
                          (cons (%make-settings-rule sel-kind sel-arg matcher (car entry) val label dir)
                                rules)))))
               (else
                (diag! 'error "Expected `(option value)` binding inside settings clause"))))
           bindings))
        (let ((form (guard (e (#t (diag! 'error
                                         (string-append "Read error: "
                                                        (if (error-object? e) (error-object-message e) "malformed S-expression")))
                                  'read-error))
                      (let* ((p (open-input-string text))
                             (first (read p)))
                        (if (eof-object? first)
                            'empty
                            (let ((extra (read p)))
                              (unless (eof-object? extra)
                                (diag! 'warning "Ignoring trailing form after `(techne-settings ...)`"))
                              first))))))
          (cond
            ((or (eq? form 'read-error) (eq? form 'empty))
             (when (eq? form 'empty)
               (diag! 'error "Empty `.techne.scm`: expected `(techne-settings 1 ...)`"))
             (%make-project-settings-file label dir #f #f '() (reverse diags)))
            ((not (and (list? form) (>= (length form) 2) (eq? (car form) 'techne-settings)))
             (diag! 'error "Expected top-level `(techne-settings 1 ...)` form")
             (%make-project-settings-file label dir #f #f '() (reverse diags)))
            ((not (eqv? (cadr form) 1))
             (diag! 'error "Unsupported `.techne.scm` version; expected version 1")
             (%make-project-settings-file label dir (cadr form) #f '() (reverse diags)))
            (else
             (for-each
              (lambda (clause)
                (cond
                  ((not (and (list? clause) (pair? clause) (symbol? (car clause))))
                   (diag! 'error "Malformed clause in `(techne-settings 1 ...)`"))
                  ((eq? (car clause) 'root)
                   (if (and (= (length clause) 2) (boolean? (cadr clause)))
                       (set! root? (cadr clause))
                       (diag! 'error "`(root ...)` expects a single boolean (`#t` or `#f`)")))
                  ((eq? (car clause) 'all)
                   (add-bindings! 'all #f (lambda (p) #t) (cdr clause)))
                  ((eq? (car clause) 'mode)
                   (if (and (>= (length clause) 2) (symbol? (cadr clause)))
                       (add-bindings! 'mode (cadr clause) #f (cddr clause))
                       (diag! 'error "`(mode NAME (opt val) ...)` expects a mode symbol")))
                  ((eq? (car clause) 'files)
                   (if (and (>= (length clause) 2) (string? (cadr clause)))
                       (guard (e (#t (diag! 'error "Invalid glob pattern in `(files ...)`")))
                         (let ((m (editorconfig-glob-compile (cadr clause))))
                           (add-bindings! 'files (cadr clause) m (cddr clause))))
                       (diag! 'error "`(files \"GLOB\" (opt val) ...)` expects a glob string")))
                  ((= (length clause) 2)
                   ;; Shorthand bare `(opt val)` == `(all (opt val))`.
                   (add-bindings! 'all #f (lambda (p) #t) (list clause)))
                  (else
                   (diag! 'error (string-append "Unknown clause `" (symbol->string (car clause)) "` in `.techne.scm`")))))
              (cddr form))
             (%make-project-settings-file label dir 1 root? (reverse rules) (reverse diags)))))))

    (define (parse-project-settings-file path)
      "Read and parse `.techne.scm` at PATH, or return #f if missing.
Refuses remote or relative paths, symlinks, non-regular files, and files
exceeding `(default-project-settings-max-bytes)`."
      (if (not (local-absolute-path? path))
          #f
          (let* ((dir (ensure-trailing-slash (path-directory path)))
                 (name (path-file-name path)))
            (guard (e (#t
                       (%make-project-settings-file
                        path dir #f #f '()
                        (list (%make-settings-diagnostic
                               path 'error
                               (if (error-object? e)
                                   (error-object-message e)
                                   "Failed to read `.techne.scm`"))))))
              (let ((text (read-confined-file-nofollow dir name (default-project-settings-max-bytes))))
                (and text
                     (parse-project-settings-string text #:path path #:directory dir)))))))

    ;;; Discovery, generation counter, and zero-I/O snapshot cache

    (define %settings-generation 0)
    (define %techne-file-cache (make-hash-table)) ; dir -> project-settings-file | 'none
    (define %snapshot-cache (make-hash-table))    ; dir -> project-settings-snapshot

    (define (bump-settings-generation!)
      (set! %settings-generation (+ %settings-generation 1))
      %settings-generation)

    (define (project-settings-generation)
      "Return the monotonic generation counter for cached project settings."
      %settings-generation)

    (define (load-dir-techne-file dir)
      (if (not (local-absolute-path? dir))
          #f
          (let ((d (ensure-trailing-slash dir)))
            (if (hash-table-contains? %techne-file-cache d)
                (let ((v (hash-table-ref %techne-file-cache d)))
                  (if (eq? v 'none) #f v))
                (let* ((p (string-append d ".techne.scm"))
                       (parsed (parse-project-settings-file p)))
                  (hash-table-set! %techne-file-cache d (or parsed 'none))
                  parsed)))))

    (define (project-settings-discover dir)
      "Return `.techne.scm` files applying to DIR, ordered farthest-to-nearest.
Walks upward from DIR stopping at a file with `(root #t)`, an unreadable or
symlink-rejected directory/file, the VC root (`.git`/`.jj`), or filesystem
root. Returns `()` for remote or relative paths."
      (if (not (local-absolute-path? dir))
          '()
          (let loop ((d (ensure-trailing-slash dir)) (acc '()))
            (let* ((sf (load-dir-techne-file d))
                   (next-acc (if sf (cons sf acc) acc)))
              (if (or (and sf (or (project-settings-file-root? sf)
                                  (not (project-settings-file-version sf))))
                      (vc-root-dir? d))
                  next-acc
                  (let ((p (parent-dir d)))
                    (if p (loop p next-acc) next-acc)))))))

    (define-record-type settings-snapshot
      (%make-project-settings-snapshot dir gen techne ec diags)
      project-settings-snapshot?
      (dir %project-settings-snapshot-directory)
      (gen project-settings-snapshot-generation)
      (techne %project-settings-snapshot-techne-files)
      (ec %project-settings-snapshot-editorconfig-files)
      (diags %project-settings-snapshot-diagnostics))

    (define (project-settings-snapshot-directory s)
      "Return the directory path of project settings snapshot S."
      (clone-settings-value (%project-settings-snapshot-directory s)))
    (define (project-settings-snapshot-techne-files s)
      "Return the `.techne.scm` files in project settings snapshot S."
      (shallow-list-copy (%project-settings-snapshot-techne-files s)))
    (define (project-settings-snapshot-editorconfig-files s)
      "Return the `.editorconfig` files in project settings snapshot S."
      (shallow-list-copy (%project-settings-snapshot-editorconfig-files s)))
    (define (project-settings-snapshot-diagnostics s)
      "Return the diagnostics collected in project settings snapshot S."
      (shallow-list-copy (%project-settings-snapshot-diagnostics s)))

    (define (project-settings-load! dir)
      "Load and cache the project-settings snapshot for directory DIR.
Discovers ancestor `.techne.scm` and `.editorconfig` files and returns
an immutable `project-settings-snapshot`. Returns `#f` for remote or
relative paths."
      (if (not (local-absolute-path? dir))
          #f
          (begin
            (guard (e (#t #f)) (ensure-trust-loaded!))
            (let* ((d (ensure-trailing-slash dir))
                   (cached (hash-table-ref/default %snapshot-cache d #f)))
              (or cached
                  (let* ((tfs (project-settings-discover d))
                         (ecs (editorconfig-discover d))
                         (diags (append-map project-settings-file-diagnostics tfs))
                         (snap (%make-project-settings-snapshot d %settings-generation tfs ecs diags)))
                    (hash-table-set! %snapshot-cache d snap)
                    snap))))))

    (define (project-settings-cached-snapshot dir)
      "Return the cached snapshot for DIR without I/O, or #f."
      (and (local-absolute-path? dir)
           (hash-table-ref/default %snapshot-cache (ensure-trailing-slash dir) #f)))

    (define (clear-table! ht)
      (for-each (lambda (k) (hash-table-delete! ht k)) (hash-table-keys ht)))

    (define (ancestor-dirs dir)
      (let loop ((d (ensure-trailing-slash dir)) (acc '()))
        (let ((next (cons d acc))
              (p (parent-dir d)))
          (if p (loop p next) next))))

    (define (project-settings-invalidate! [dir #f])
      "Invalidate cached `.techne.scm` and `.editorconfig` state for DIR.
When DIR is `#f`, clears all cached settings and snapshots. Bumps
`project-settings-generation`."
      (if (or (not dir) (not (local-absolute-path? dir)))
          (begin
            (editorconfig-invalidate! #f)
            (clear-table! %techne-file-cache)
            (clear-table! %snapshot-cache))
          (let* ((prefix (ensure-trailing-slash dir))
                 (canon (guard (e (#t #f)) (project-settings-canonical-dir prefix))))
            (if (not canon)
                (begin
                  (editorconfig-invalidate! #f)
                  (clear-table! %techne-file-cache)
                  (clear-table! %snapshot-cache))
                (let ((prefixes (if (string=? prefix canon) (list prefix) (list prefix canon))))
                  (define (under-any? k)
                    (any (lambda (p) (or (string=? k p) (string-prefix? p k))) prefixes))
                  (for-each editorconfig-invalidate! prefixes)
                  (for-each (lambda (k)
                              (when (under-any? k)
                                (hash-table-delete! %techne-file-cache k)))
                            (hash-table-keys %techne-file-cache))
                  (for-each (lambda (k)
                              (when (under-any? k)
                                (hash-table-delete! %snapshot-cache k)))
                            (hash-table-keys %snapshot-cache))))))
      (bump-settings-generation!))

    (define (project-settings-invalidate-ancestors! dir)
      "Invalidate cached settings for DIR, its descendants, and its ancestors.
Ensures reloads triggered from a subdirectory also pick up edits made to
an ancestor `.techne.scm` or `.editorconfig` file. Bumps
`project-settings-generation`."
      (if (not (local-absolute-path? dir))
          (project-settings-invalidate! #f)
          (let* ((prefix (ensure-trailing-slash dir))
                 (canon (guard (e (#t #f)) (project-settings-canonical-dir prefix))))
            (if (not canon)
                (project-settings-invalidate! #f)
                (let ((ancs (append (ancestor-dirs prefix)
                                    (if (string=? prefix canon) '() (ancestor-dirs canon)))))
                  (editorconfig-invalidate-ancestors! prefix)
                  (unless (string=? prefix canon)
                    (editorconfig-invalidate-ancestors! canon))
                  (for-each (lambda (k)
                              (when (or (string=? k prefix) (string-prefix? prefix k)
                                        (string=? k canon) (string-prefix? canon k)
                                        (member k ancs))
                                (hash-table-delete! %techne-file-cache k)))
                            (hash-table-keys %techne-file-cache))
                  (for-each (lambda (k)
                              (let ((snap (hash-table-ref/default %snapshot-cache k #f)))
                                (when (or (string=? k prefix) (string-prefix? prefix k)
                                          (string=? k canon) (string-prefix? canon k)
                                          (member k ancs)
                                          (and snap
                                               (or (any (lambda (tf)
                                                          (member (project-settings-file-directory tf) ancs))
                                                        (project-settings-snapshot-techne-files snap))
                                                   (any (lambda (ec)
                                                          (member (editorconfig-file-directory ec) ancs))
                                                        (project-settings-snapshot-editorconfig-files snap)))))
                                  (hash-table-delete! %snapshot-cache k))))
                            (hash-table-keys %snapshot-cache))
                  (bump-settings-generation!))))))

    (define (project-settings-watched-path? path)
      "Return #t if PATH is a `.techne.scm` or `.editorconfig` settings file."
      (and (string? path)
           (let ((name (path-file-name path)))
             (and (member name '(".techne.scm" ".editorconfig")) #t))))

    ;;; Bufferless settings context

    (define-record-type settings-context
      (%make-settings-context directory path modes snapshot)
      settings-context?
      (directory %settings-context-directory)
      (path %settings-context-path)
      (modes %settings-context-modes)
      (snapshot settings-context-snapshot))

    (define (settings-context-directory ctx)
      "Return the directory path of settings context CTX, or `#f`."
      (clone-settings-value (%settings-context-directory ctx)))
    (define (settings-context-path ctx)
      "Return the optional file path of settings context CTX, or `#f`."
      (clone-settings-value (%settings-context-path ctx)))
    (define (settings-context-modes ctx)
      "Return the mode symbol list of settings context CTX."
      (clone-settings-value (%settings-context-modes ctx)))

    (define (make-settings-context dir #:path [path #f] #:modes [modes '()] #:load? [load? #t])
      "Return a `settings-context` for DIR with optional file PATH and MODES.
MODES is a list of mode symbols ordered nearest-first (as `mode-chain`).
When LOAD? is true (the default), loads the settings snapshot if not
already cached; when `#f`, uses only cached snapshots (zero I/O)."
      (let* ((raw-d (if (and (not dir) path) (path-directory path) dir))
             (local? (and (local-absolute-path? raw-d)
                          (or (not path) (local-absolute-path? path))))
             (d (and local? (string-copy (ensure-trailing-slash raw-d))))
             (snap (and d
                        (if load?
                            (project-settings-load! d)
                            (project-settings-cached-snapshot d)))))
        (%make-settings-context d (and local? (clone-settings-value path))
                                (clone-settings-value modes) snap)))

    ;;; Value-based trust store (scope, option-name, canonical-digest) -> trusted | denied

    ;; File-backed trust ledger cache (associated with %trust-loaded-path).
    (define %trust-table (make-hash-table)) ; (scope option-name digest) -> 'trusted | 'denied
    ;; Memory-only trust table used strictly when effective-trust-store-path is #f
    ;; (i.e. default-trust-store-path is 'none, or headless mode without a file hook).
    ;; Kept completely separate from %trust-table so switching to 'none and back
    ;; never leaks in-memory verdicts into a file-backed ledger or persists them.
    (define %memory-trust-table (make-hash-table))
    (define %trust-loaded-path #f)
    ;; SHA-256 hex digest of the last cleanly loaded trust ledger text,
    ;; `'absent` when the store path was cleanly observed as non-existent,
    ;; or `#f` when unloaded or after a load failure.
    (define %trust-loaded-hash #f)
    ;; Whether the in-memory file cache is currently authoritative for a file-backed
    ;; trust store. A reload failure preserves %trust-table for diagnostics but
    ;; flips %trust-authoritative? to #f so project-trust-verdict fails closed.
    (define %trust-authoritative? #t)

    (define (active-trust-table path)
      (if (string? path) %trust-table %memory-trust-table))

    (define (trust-content-hash text)
      (hex-encode (sha-256 text)))

    (define (check-datum-budget! x budget [depth 0])
      (when (> depth 64)
        (error "project-trust-load!: datum nesting exceeds maximum depth"))
      (consume-node! budget "project-trust-load!")
      (cond
        ((pair? x)
         (let loop ((p x))
           (when (pair? p)
             (consume-node! budget "project-trust-load!")
             (check-datum-budget! (car p) budget (+ depth 1))
             (let ((next (cdr p)))
               (if (pair? next)
                   (loop next)
                   (when (not (null? next))
                     (check-datum-budget! next budget (+ depth 1))))))))
        ((vector? x)
         (let ((n (vector-length x)))
           (let loop ((i 0))
             (when (< i n)
               (check-datum-budget! (vector-ref x i) budget (+ depth 1))
               (loop (+ i 1))))))))

    (define (ensure-trust-loaded!)
      (let ((p (effective-trust-store-path)))
        (cond
          ((eq? p 'untrusted-baseline)
           (when %trust-authoritative?
             (set! %trust-authoritative? #f)
             (set! %trust-loaded-hash #f)
             (bump-settings-generation!))
           #f)
          ((string? p)
           (project-trust-load*! p #t))
          (else #f))))

    (define (project-trust-clear-memory!)
      "Clear all in-memory trust verdicts and reset the loaded store path."
      (clear-table! %trust-table)
      (clear-table! %memory-trust-table)
      (set! %trust-loaded-path #f)
      (set! %trust-loaded-hash #f)
      (set! %trust-authoritative? #t)
      (bump-settings-generation!))

    (define (split-trust-store-path who path)
      (unless (local-absolute-path? path)
        (error (string-append who ": trust store path must be local and absolute") path))
      (let ((dir (ensure-trailing-slash (path-directory path)))
            (leaf (path-file-name path)))
        (when (or (not (string? leaf)) (string=? leaf "") (string=? leaf ".") (string=? leaf ".."))
          (error (string-append who ": trust store path must name a file") path))
        (values dir leaf)))

    (define (apply-trust-ledger-text! path text skip-unchanged?)
      (if (not text)
          (unless (and skip-unchanged?
                       %trust-authoritative?
                       (equal? path %trust-loaded-path)
                       (eq? %trust-loaded-hash 'absent))
            (clear-table! %trust-table)
            (set! %trust-loaded-path path)
            (set! %trust-loaded-hash 'absent)
            (set! %trust-authoritative? #t)
            (bump-settings-generation!))
          (let ((digest (trust-content-hash text)))
            (unless (and skip-unchanged?
                         %trust-authoritative?
                         (equal? path %trust-loaded-path)
                         (string? %trust-loaded-hash)
                         (string=? digest %trust-loaded-hash))
              (let* ((port (open-input-string text))
                     (form (read port))
                     (extra (read port)))
                (unless (eof-object? extra)
                  (error "project-trust-load!: unexpected trailing form in trust store" path))
                (check-datum-budget! form (vector %max-datum-nodes))
                (unless (and (list? form) (>= (length form) 2)
                             (eq? (car form) 'techne-project-trust)
                             (eqv? (cadr form) 1))
                  (error "project-trust-load!: malformed trust store header" path))
                (let ((parsed '()))
                  (for-each
                   (lambda (entry)
                     (unless (and (list? entry) (= (length entry) 4)
                                  (local-absolute-path? (car entry))
                                  (symbol? (cadr entry))
                                  (string? (caddr entry))
                                  (memq (cadddr entry) '(trusted denied)))
                       (error "project-trust-load!: malformed trust store entry" entry))
                     (set! parsed
                           (cons (cons (list (ensure-trailing-slash (car entry))
                                             (cadr entry)
                                             (caddr entry))
                                       (cadddr entry))
                                 parsed)))
                   (cddr form))
                  (clear-table! %trust-table)
                  (for-each (lambda (kv) (hash-table-set! %trust-table (car kv) (cdr kv)))
                            (reverse parsed))
                  (set! %trust-loaded-path path)
                  (set! %trust-loaded-hash digest)
                  (set! %trust-authoritative? #t)
                  (bump-settings-generation!)))))))

    (define (project-trust-load*! path [skip-unchanged? #f])
      (when (string? path)
        (guard (e (#t
                   (set! %trust-authoritative? #f)
                   (set! %trust-loaded-hash #f)
                   (bump-settings-generation!)
                   (raise e)))
          (call-with-values
           (lambda () (split-trust-store-path "project-trust-load!" path))
           (lambda (dir leaf)
             (let ((text (read-confined-file-nofollow dir leaf (* 4 1024 1024))))
               (apply-trust-ledger-text! path text skip-unchanged?)))))))

    (define (project-trust-load-locked! path lock leaf [skip-unchanged? #f])
      (guard (e (#t
                 (set! %trust-authoritative? #f)
                 (set! %trust-loaded-hash #f)
                 (bump-settings-generation!)
                 (raise e)))
        (let ((text (file-lock-read-text lock leaf (* 4 1024 1024))))
          (apply-trust-ledger-text! path text skip-unchanged?))))

    (define (project-trust-load! path)
      "Load persisted trust verdicts from PATH into memory.
Refuses symlinks, non-regular files, and corrupt store files without
clobbering existing diagnostic state, while revoking authorization until
a clean reload succeeds."
      (project-trust-load*! path #f))

    (define (trust-entries-snapshot table)
      (sort (map (lambda (kv)
                   (list (string-copy (car (car kv)))
                         (cadr (car kv))
                         (string-copy (caddr (car kv)))
                         (cdr kv)))
                 (hash-table->alist table))
            (lambda (a b)
              (or (string<? (car a) (car b))
                  (and (string=? (car a) (car b))
                       (or (string<? (symbol->string (cadr a)) (symbol->string (cadr b)))
                           (and (eq? (cadr a) (cadr b))
                                (string<? (caddr a) (caddr b)))))))))

    (define (restore-trust-table! table saved-alist)
      (clear-table! table)
      (for-each (lambda (kv) (hash-table-set! table (car kv) (cdr kv))) saved-alist))

    (define (require-trusted-ledger-target! who)
      (when (eq? (effective-trust-store-path) 'untrusted-baseline)
        (error (string-append who ": cannot modify project trust with an untrusted startup environment"))))

    (define (with-trust-ledger-lock path thunk)
      (if (not (string? path))
          (thunk #f #f)
          (call-with-values
           (lambda () (split-trust-store-path "with-trust-ledger-lock" path))
           (lambda (dir leaf)
             (let* ((lock-leaf (string-append leaf ".lock"))
                    (deadline (+ (current-milliseconds) 5000))
                    (lock (let loop ()
                            (let ((l (file-try-lock-pinned dir lock-leaf)))
                              (cond
                                (l l)
                                ((>= (current-milliseconds) deadline)
                                 (error "with-trust-ledger-lock: timed out waiting for trust store lock"
                                        path))
                                (else
                                 (sleep 5)
                                 (loop)))))))
               (dynamic-wind
                (lambda () #f)
                (lambda () (thunk lock leaf))
                (lambda () (file-unlock lock))))))))

    (define (persist-trust! path lock leaf)
      (when (eq? path 'untrusted-baseline)
        (error "persist-trust!: cannot persist project trust with an untrusted startup environment"))
      (when (and (string? path) lock leaf)
        (let* ((entries (trust-entries-snapshot %trust-table))
               (content (call-with-output-string
                         (lambda (p)
                           (display "(techne-project-trust 1\n" p)
                           (for-each (lambda (e)
                                       (display "  " p)
                                       (write e p)
                                       (newline p))
                                     entries)
                           (display ")\n" p)))))
          (file-lock-replace-private! lock leaf content)
          (set! %trust-loaded-path path)
          (set! %trust-loaded-hash (trust-content-hash content))
          (set! %trust-authoritative? #t))))

    (define (project-trust-entries)
      "Return all stored trust entries as `(SCOPE OPTION DIGEST VERDICT)`."
      (let ((path (effective-trust-store-path)))
        (cond
          ((eq? path 'untrusted-baseline) '())
          ((string? path)
           (ensure-trust-loaded!)
           (trust-entries-snapshot %trust-table))
          (else
           (trust-entries-snapshot %memory-trust-table)))))

    (define (project-trust-verdict scope option-name effective-value)
      "Return `'trusted`, `'denied`, or `'unknown` for OPTION-NAME in SCOPE.
Performs zero filesystem I/O and checks the canonical SHA-256 digest of
EFFECTIVE-VALUE against the active trust ledger. Fails closed to
`'unknown` when the startup environment is untrusted or when the active
file-backed trust store has not loaded cleanly."
      (let ((active-path (effective-trust-store-path)))
        (cond
          ((eq? active-path 'untrusted-baseline) 'unknown)
          ((has-parent-component? scope) 'unknown)
          ((string? active-path)
           (if (or (not %trust-authoritative?)
                   (not (equal? active-path %trust-loaded-path)))
               'unknown
               (let* ((norm (ensure-trailing-slash scope))
                      (digest (value-trust-digest norm option-name effective-value)))
                 (hash-table-ref/default %trust-table (list norm option-name digest) 'unknown))))
          (else
           (let* ((norm (ensure-trailing-slash scope))
                  (digest (value-trust-digest norm option-name effective-value)))
             (hash-table-ref/default %memory-trust-table (list norm option-name digest) 'unknown))))))

    (define (project-trust-approve! scope option-name effective-value)
      "Approve EFFECTIVE-VALUE for OPTION-NAME in SCOPE (user principal only)."
      (check-user-principal! "project-trust-approve!")
      (require-trusted-ledger-target! "project-trust-approve!")
      (let* ((norm (canonical-dir scope))
             (path (effective-trust-store-path))
             (table (active-trust-table path)))
        (with-trust-ledger-lock
         path
         (lambda (lock leaf)
           (when lock
             (project-trust-load-locked! path lock leaf #t))
           (let* ((digest (value-trust-digest norm option-name effective-value))
                  (saved (hash-table->alist table)))
             (hash-table-set! table (list (string-copy norm) option-name (string-copy digest)) 'trusted)
             (guard (e (#t (restore-trust-table! table saved) (raise e)))
               (persist-trust! path lock leaf))
             (bump-settings-generation!)
             (string-copy digest))))))

    (define (project-trust-deny! scope option-name effective-value)
      "Deny EFFECTIVE-VALUE for OPTION-NAME in SCOPE (user principal only)."
      (check-user-principal! "project-trust-deny!")
      (require-trusted-ledger-target! "project-trust-deny!")
      (let* ((norm (canonical-dir scope))
             (path (effective-trust-store-path))
             (table (active-trust-table path)))
        (with-trust-ledger-lock
         path
         (lambda (lock leaf)
           (when lock
             (project-trust-load-locked! path lock leaf #t))
           (let* ((digest (value-trust-digest norm option-name effective-value))
                  (saved (hash-table->alist table)))
             (hash-table-set! table (list (string-copy norm) option-name (string-copy digest)) 'denied)
             (guard (e (#t (restore-trust-table! table saved) (raise e)))
               (persist-trust! path lock leaf))
             (bump-settings-generation!)
             (string-copy digest))))))

    (define (project-trust-revoke! [scope #f] [option-name #f])
      "Revoke stored trust verdicts matching SCOPE and OPTION-NAME.
Passing `#f` for SCOPE or OPTION-NAME acts as a wildcard. Only the user
principal may revoke trust. Returns the number of entries removed."
      (check-user-principal! "project-trust-revoke!")
      (require-trusted-ledger-target! "project-trust-revoke!")
      (let* ((norm (and scope (canonical-dir scope)))
             (path (effective-trust-store-path))
             (table (active-trust-table path)))
        (with-trust-ledger-lock
         path
         (lambda (lock leaf)
           (when lock
             (project-trust-load-locked! path lock leaf #t))
           (let ((saved (hash-table->alist table))
                 (removed 0))
             (for-each
              (lambda (k)
                (when (and (or (not norm) (string=? (car k) norm))
                           (or (not option-name) (eq? (cadr k) option-name)))
                  (hash-table-delete! table k)
                  (set! removed (+ removed 1))))
              (hash-table-keys table))
             (when (> removed 0)
               (guard (e (#t (restore-trust-table! table saved) (raise e)))
                 (persist-trust! path lock leaf))
               (bump-settings-generation!))
             removed)))))

    ;;; Directory option resolution (zero I/O on cached snapshots)

    (define (mode-index mode-sym mode-chain)
      (let loop ((ms mode-chain) (i 0))
        (cond ((null? ms) #f)
              ((eq? (car ms) mode-sym) i)
              (else (loop (cdr ms) (+ i 1))))))

    (define (rule-specificity rule mode-chain file-path)
      ;; Returns specificity score (higher = more specific) or #f if rule does not match.
      (case (settings-rule-selector-kind rule)
        ((all) 10)
        ((mode)
         (let ((idx (mode-index (settings-rule-selector-arg rule) mode-chain)))
           (and idx (- 1000 idx))))
        ((files)
         (and file-path
              (not (has-parent-component? file-path))
              (let ((rel (relative-path-under (settings-rule-source-dir rule) file-path)))
                (and rel ((%settings-rule-matcher rule) rel) 2000))))
        (else #f)))

    (define (option-value-safe? safe-spec val)
      (cond
        ((eq? safe-spec #t) #t)
        ((procedure? safe-spec) (and (guard (e (#t #f)) (safe-spec (clone-settings-value val))) #t))
        (else #f)))

    ;; Entry record during resolution:
    ;;   (dir source-rank specificity order-idx value source-kind source-label)
    ;; source-rank: 3 = user, 2 = techne-scm, 1 = editorconfig
    ;; Higher dir depth (nearer dir), higher source-rank, higher specificity, higher order-idx wins.
    (define (entry-precedes? a b)
      (let ((da (path-depth (list-ref a 0)))
            (db (path-depth (list-ref b 0))))
        (cond
          ((not (= da db)) (> da db))
          ((not (= (list-ref a 1) (list-ref b 1))) (> (list-ref a 1) (list-ref b 1)))
          ((not (= (list-ref a 2) (list-ref b 2))) (> (list-ref a 2) (list-ref b 2)))
          (else (> (list-ref a 3) (list-ref b 3))))))

    (define (collect-raw-entries snap file-path mode-chain user-entries option-name)
      ;; Returns list of raw entries matching OPTION-NAME, sorted highest-precedence first.
      (let ((acc '())
            (idx 0))
        (define (push! dir srank spec val skind slabel)
          (set! idx (+ idx 1))
          (set! acc (cons (list (ensure-trailing-slash dir) srank spec idx val skind slabel) acc)))
        ;; 1. User directory overrides: each is (dir mode-or-#f value by)
        (for-each
         (lambda (entry)
           (let* ((udir (string-copy (ensure-trailing-slash (car entry))))
                  (umode (cadr entry))
                  (uval (clone-settings-value (caddr entry)))
                  (uby (clone-settings-value (cadddr entry)))
                  (spec (cond
                          ((not umode) 10)
                          ((mode-index umode mode-chain) => (lambda (i) (- 1000 i)))
                          (else #f))))
             (when spec
               (push! udir 3 spec uval 'user uby))))
         user-entries)
        ;; 2. .techne.scm rules (farthest to nearest; within a file, later wins on tie via idx)
        (when snap
          (for-each
           (lambda (tf)
             (for-each
              (lambda (rule)
                (when (eq? (settings-rule-option rule) option-name)
                  (let ((spec (rule-specificity rule mode-chain file-path)))
                    (when spec
                      (push! (settings-rule-source-dir rule)
                             2
                             spec
                             (settings-rule-value rule)
                             'techne-scm
                             (settings-rule-source-path rule))))))
              (project-settings-file-rules tf)))
           (project-settings-snapshot-techne-files snap))
          ;; 3. .editorconfig properties (only when a file path is known)
          (when (and file-path (pair? (project-settings-snapshot-editorconfig-files snap)))
            (let* ((ec-props (editorconfig-resolve-with-sources
                              file-path
                              #:files (project-settings-snapshot-editorconfig-files snap)))
                   (ec-opts (editorconfig-sourced-options ec-props)))
              (for-each
               (lambda (eo)
                 (when (eq? (car eo) option-name)
                   (push! (cadddr eo) 1 10 (cadr eo) 'editorconfig (caddr eo))))
               ec-opts))))
        (sort acc entry-precedes?)))

    (define (project-settings-resolve-option snap file-path mode-chain user-entries
                                             option-name
                                             #:known? [known? #t]
                                             #:user-only? [user-only? #f]
                                             #:safe [safe-spec #f]
                                             #:valid? [valid-proc (lambda (v) #t)]
                                             #:merge [merge-mode 'replace])
      "Resolve directory-cell entries for OPTION-NAME without performing I/O.
Returns `(values applied-entries pending-entries rejected-entries)`.
APPLIED-ENTRIES is highest-precedence-first `(dir kind label value)`
entries that passed validation and trust. PENDING-ENTRIES is a list of
`(scope option effective-value source-path verdict)` where VERDICT is
`'unknown` or `'denied`. REJECTED-ENTRIES is `(source-label reason)`.
SNAP may be `#f`, in which case only USER-ENTRIES apply. FILE-PATH,
MODE-CHAIN, KNOWN?, USER-ONLY?, SAFE-SPEC, VALID-PROC, and MERGE-MODE
control matching, validation, trust, and overlay merging."
      (let* ((raw (collect-raw-entries snap file-path mode-chain user-entries option-name))
             (applied '())
             (pending '())
             (rejected '()))
        (cond
          ((not known?)
           ;; Unknown options from project files remain inert until declared.
           (for-each
            (lambda (e)
              (let ((skind (list-ref e 5))
                    (slabel (list-ref e 6)))
                (if (eq? skind 'user)
                    (set! applied (cons (list (list-ref e 0) skind slabel (list-ref e 4)) applied))
                    (set! rejected (cons (list slabel "unknown option (pending declaration)") rejected)))))
            raw)
           (values (reverse applied) '() (reverse rejected)))
          ((eq? merge-mode 'json-object)
           ;; For JSON-object overlay options:
           ;; Separate user entries from .techne.scm entries; validate & trust the
           ;; EFFECTIVE combined .techne.scm overlay as a single atomic unit!
           (let ((techne-raw (filter (lambda (e) (eq? (list-ref e 5) 'techne-scm)) raw)))
             (when (and user-only? (pair? techne-raw))
               (for-each (lambda (e)
                           (set! rejected (cons (list (list-ref e 6) "user-only option cannot be set by project settings") rejected)))
                         techne-raw)
               (set! techne-raw '()))
             ;; Check every .techne.scm fragment is a dict before overlaying.
             (let ((bad-frags (filter (lambda (e) (not (dict? (list-ref e 4)))) techne-raw)))
               (for-each (lambda (e)
                           (set! rejected (cons (list (list-ref e 6) "expected a dict value for json-object option") rejected)))
                         bad-frags)
               (set! techne-raw (filter (lambda (e) (dict? (list-ref e 4))) techne-raw)))
             (let ((techne-approved? #f))
               (when (pair? techne-raw)
                 ;; Combine from lowest precedence to highest precedence.
                 (let* ((eff-techne (fold (lambda (e acc)
                                            (json-object-compose-overlays acc (list-ref e 4)))
                                          (dict)
                                          (reverse techne-raw)))
                        (clean-eff (json-object-strip-unset eff-techne))
                        (anchor (car techne-raw))
                        (scope (list-ref anchor 0))
                        (src-path (list-ref anchor 6)))
                   (cond
                     ((not (valid-proc (clone-settings-value clean-eff)))
                      (set! rejected (cons (list src-path "effective merged value failed option schema") rejected)))
                     ((option-value-safe? safe-spec eff-techne)
                      (set! techne-approved? #t))
                     (else
                      (let ((v (project-trust-verdict scope option-name eff-techne)))
                        (if (eq? v 'trusted)
                            (set! techne-approved? #t)
                            (set! pending (cons (list scope option-name eff-techne src-path v) pending))))))))
             ;; Emit applied entries in highest-precedence-first order, skipping unapproved techne-scm entries.
             (for-each
              (lambda (e)
                (let ((skind (list-ref e 5))
                      (slabel (list-ref e 6))
                      (val (list-ref e 4))
                      (dir (list-ref e 0)))
                  (cond
                    ((eq? skind 'user)
                     (set! applied (cons (list dir skind slabel val) applied)))
                    ((and (eq? skind 'techne-scm) techne-approved? (dict? val))
                     (set! applied (cons (list dir skind slabel val) applied))))))
              raw)
             (values (reverse applied) (reverse pending) (reverse rejected)))))
          (else
           ;; Standard 'replace merge: evaluate user and editorconfig entries,
           ;; and evaluate ONLY the winning (highest-precedence) .techne.scm rule
           ;; so shadowed lower-precedence trusted rules never activate and
           ;; shadowed untrusted rules never stick as pending.
           (let ((seen-techne? #f))
             (for-each
              (lambda (e)
                (let ((dir (list-ref e 0))
                      (val (list-ref e 4))
                      (skind (list-ref e 5))
                      (slabel (list-ref e 6)))
                  (cond
                    ((eq? skind 'user)
                     (set! applied (cons (list dir skind slabel val) applied)))
                    (user-only?
                     (set! rejected (cons (list slabel "user-only option cannot be set by project settings") rejected)))
                    ((eq? skind 'editorconfig)
                     (if (and (option-value-safe? safe-spec val) (valid-proc (clone-settings-value val)))
                         (set! applied (cons (list dir skind slabel val) applied))
                         (set! rejected (cons (list slabel "value failed safety or schema check") rejected))))
                    ((eq? skind 'techne-scm)
                     (unless seen-techne?
                       (set! seen-techne? #t)
                       (cond
                         ((contains-json-unset? val)
                          (set! rejected (cons (list slabel "`(unset)` is only valid inside JSON-object dicts") rejected)))
                         ((not (valid-proc (clone-settings-value val)))
                          (set! rejected (cons (list slabel "value failed option schema") rejected)))
                         ((option-value-safe? safe-spec val)
                          (set! applied (cons (list dir skind slabel val) applied)))
                         (else
                          (let ((v (project-trust-verdict dir option-name val)))
                            (if (eq? v 'trusted)
                                (set! applied (cons (list dir skind slabel val) applied))
                                (set! pending (cons (list dir option-name val slabel v) pending)))))))))))
              raw))
           (values (reverse applied) (reverse pending) (reverse rejected))))))

    (define (project-settings-collect-pending snap file-path mode-chain lookup-meta)
      "Return pending or denied settings for SNAP, FILE-PATH, and MODE-CHAIN.
LOOKUP-META is `(lambda (option-name) ...)` returning `(values known?
user-only? safe-spec valid-proc merge-mode)`. Returns a deduplicated
list of `(SCOPE OPTION-NAME EFFECTIVE-VALUE SOURCE-PATH VERDICT)`."
      (if (not snap)
          '()
          (let ((opt-names '())
                (pending '()))
            (for-each
             (lambda (tf)
               (for-each
                (lambda (rule)
                  (let ((opt (settings-rule-option rule)))
                    (unless (memq opt opt-names)
                      (set! opt-names (cons opt opt-names)))))
                (project-settings-file-rules tf)))
             (project-settings-snapshot-techne-files snap))
            (for-each
             (lambda (opt)
               (call-with-values
                (lambda () (lookup-meta opt))
                (lambda (known? user-only? safe-spec valid-proc merge-mode)
                  (call-with-values
                   (lambda ()
                     (project-settings-resolve-option
                      snap file-path mode-chain '() opt
                      #:known? known?
                      #:user-only? user-only?
                      #:safe safe-spec
                      #:valid? valid-proc
                      #:merge merge-mode))
                   (lambda (applied pend rej)
                     (for-each (lambda (p) (set! pending (cons p pending))) pend))))))
             (reverse opt-names))
            (reverse pending))))

    (define (project-settings-collect-all-pending snap lookup-meta)
      "Return pending or denied settings across all selectors in SNAP.
Evaluates `(all ...)`, every declared `(mode NAME ...)`, and every
`(files GLOB ...)` clause so users inspecting `.techne.scm` directly can
review and approve rules targeting any mode or file pattern. LOOKUP-META
has the same signature as in `project-settings-collect-pending`."
      (if (not snap)
          '()
          (let ((seen (make-hash-table))
                (result '())
                (modes '())
                (file-rules '()))
            (define (add-pending! item)
              (let* ((scope (list-ref item 0))
                     (opt (list-ref item 1))
                     (val (list-ref item 2))
                     (key (list scope opt (value-trust-digest scope opt val))))
                (unless (hash-table-contains? seen key)
                  (hash-table-set! seen key #t)
                  (set! result (cons item result)))))
            (for-each
             (lambda (tf)
               (for-each
                (lambda (rule)
                  (case (settings-rule-selector-kind rule)
                    ((mode)
                     (let ((m (settings-rule-selector-arg rule)))
                       (unless (memq m modes)
                         (set! modes (cons m modes)))))
                    ((files)
                     (set! file-rules (cons rule file-rules)))))
                (project-settings-file-rules tf)))
             (project-settings-snapshot-techne-files snap))
            ;; 1. Base `(all ...)` rules
            (for-each add-pending! (project-settings-collect-pending snap #f '() lookup-meta))
            ;; 2. Each declared `(mode M ...)` selector
            (for-each (lambda (m)
                        (for-each add-pending!
                                  (project-settings-collect-pending snap #f (list m) lookup-meta)))
                      (reverse modes))
            ;; 3. Each `(files GLOB ...)` rule
            (for-each
             (lambda (rule)
               (let ((opt (settings-rule-option rule))
                     (val (settings-rule-value rule))
                     (scope (settings-rule-source-dir rule))
                     (src-path (settings-rule-source-path rule)))
                 (call-with-values
                  (lambda () (lookup-meta opt))
                  (lambda (known? user-only? safe-spec valid-proc merge-mode)
                    (when (and known? (not user-only?))
                      (if (eq? merge-mode 'json-object)
                          (when (dict? val)
                            ;; Compose all `(all ...)` rules for OPT with this `(files ...)` rule.
                            (let* ((base-entries
                                    (filter (lambda (e) (and (eq? (list-ref e 5) 'techne-scm)
                                                             (dict? (list-ref e 4))))
                                            (collect-raw-entries snap #f '() '() opt)))
                                   (with-file
                                    (sort (cons (list scope 2 2000 (+ (length base-entries) 1) val 'techne-scm src-path)
                                                base-entries)
                                          entry-precedes?))
                                   (eff (fold (lambda (e acc)
                                                (json-object-compose-overlays acc (list-ref e 4)))
                                              (dict)
                                              (reverse with-file)))
                                   (clean (json-object-strip-unset eff))
                                   (anchor (car with-file))
                                   (escope (list-ref anchor 0))
                                   (esrc (list-ref anchor 6)))
                              (when (and (valid-proc (clone-settings-value clean))
                                         (not (option-value-safe? safe-spec eff)))
                                (let ((v (project-trust-verdict escope opt eff)))
                                  (unless (eq? v 'trusted)
                                    (add-pending! (list escope opt eff esrc v)))))))
                          (when (and (not (contains-json-unset? val))
                                     (valid-proc (clone-settings-value val))
                                     (not (option-value-safe? safe-spec val)))
                            (let ((v (project-trust-verdict scope opt val)))
                              (unless (eq? v 'trusted)
                                (add-pending! (list scope opt val src-path v)))))))))))
             (reverse file-rules))
            (reverse result))))))
