;;; Headless stored-bytes <-> text codecs.
;;;
;;; A codec defines how stored bytes on disk (or on a remote endpoint)
;;; decode into editor text and encode back into stored bytes, along with
;;; an optional per-document envelope (e.g. encryption recipients or format
;;; metadata), a logical path for mode/language detection, and whether
;;; documents using the codec require the private profile (no journal, no
;;; LSP, no session persistence).
;;;
;;; Built-in codecs live in `%builtin-codecs` so cold-importing `(techne codec)`
;;; inside a temporary scope never removes them on scope shutdown. User and
;;; extension codecs registered via `register-codec!` / `define-codec` are
;;; scope-owned and override built-ins of the same name while active.

(define-library (techne codec)
  (export codec? codec-name codec-doc codec-private? codec-priority
          codec-result? make-codec-result codec-result-text codec-result-envelope
          register-codec! define-codec find-codec codec-names
          codecs-for-path codec-for-path path-private-by-codec?
          codec-logical-path codec-init-envelope
          codec-decode codec-encode
          default-gzip-program default-codec-max-bytes default-codec-timeout)
  (import (techne) (techne codec params) (techne path) (techne location) (techne trusted-process))
  (begin
    (note-codec-registry-loaded!)

    (define-record-type codec
      (%make-codec name doc match-proc logical-path-proc init-envelope-proc
                   decode-proc encode-proc private? priority override-builtin-private?)
      codec?
      (name codec-name)
      (doc codec-doc)
      (match-proc %codec-match-proc)
      (logical-path-proc %codec-logical-path-proc)
      (init-envelope-proc %codec-init-envelope-proc)
      (decode-proc %codec-decode-proc)
      (encode-proc %codec-encode-proc)
      (private? codec-private?)
      (priority codec-priority)
      (override-builtin-private? %codec-override-builtin-private?))

    (define-record-type codec-result
      (%make-codec-result text envelope)
      codec-result?
      (text codec-result-text)
      (envelope codec-result-envelope))

    (define (make-codec-result text #:envelope [envelope #f])
      "Construct a codec decode result with TEXT and optional ENVELOPE."
      (unless (string? text)
        (error "make-codec-result: text must be a string"))
      (%make-codec-result text envelope))

    (define %builtin-codecs (make-hash-table))
    (define %extension-codecs (make-registry 'codecs))

    (define (register-builtin-codec! name doc
                                     #:match match-proc
                                     #:decode decode-proc
                                     #:encode encode-proc
                                     #:logical-path [logical-path-proc #f]
                                     #:init-envelope [init-envelope-proc #f]
                                     #:private [private? #f]
                                     #:priority [priority 0])
      (hash-table-set! %builtin-codecs name
                       (%make-codec name doc match-proc logical-path-proc
                                    init-envelope-proc decode-proc encode-proc
                                    (and private? #t) priority #f)))

    (define (register-codec! name doc
                             #:match match-proc
                             #:decode decode-proc
                             #:encode encode-proc
                             #:logical-path [logical-path-proc #f]
                             #:init-envelope [init-envelope-proc #f]
                             #:private [private? #f]
                             #:priority [priority 0]
                             #:override-builtin-private? [override-builtin-private? #f])
      "Register a scope-owned codec NAME documented by DOC.
MATCH-PROC maps `(path)` to a boolean, DECODE-PROC maps `(path raw-bytes)`
to a `codec-result` or string, and ENCODE-PROC maps `(path text envelope)`
to a bytevector. Optional LOGICAL-PATH-PROC maps a stored path to the
logical path used for mode and syntax detection; INIT-ENVELOPE-PROC
initializes an envelope for a new file; PRIVATE? marks the codec as
requiring the private retention profile; PRIORITY orders matching codecs
(higher wins, tie-broken by name); OVERRIDE-BUILTIN-PRIVATE? (root scope
and `#:private #t` only) explicitly authorizes a differently-named codec
to preempt a protected built-in private codec on automatic selection."
      (unless (symbol? name) (error "register-codec!: NAME must be a symbol" name))
      (unless (string? doc) (error "register-codec!: DOC must be a string"))
      (unless (procedure? match-proc) (error "register-codec!: #:match must be a procedure"))
      (unless (procedure? decode-proc) (error "register-codec!: #:decode must be a procedure"))
      (unless (procedure? encode-proc) (error "register-codec!: #:encode must be a procedure"))
      (when (and logical-path-proc (not (procedure? logical-path-proc)))
        (error "register-codec!: #:logical-path must be a procedure or #f"))
      (when (and init-envelope-proc (not (procedure? init-envelope-proc)))
        (error "register-codec!: #:init-envelope must be a procedure or #f"))
      (unless (and (number? priority) (real? priority))
        (error "register-codec!: #:priority must be a real number" priority))
      ;; Codecs intercept file reads and writes, so registration requires the Files grant.
      (file-exists? "/")
      (let* ((cur (current-scope))
             (builtin (hash-table-ref/default %builtin-codecs name #f))
             (other-private?
              (any (lambda (entry)
                     (and (not (eq? (cdr entry) cur))
                          (codec-private? (car entry))))
                   (registry-entries %extension-codecs name))))
        (when (and builtin (codec-private? builtin) (not (root-authority?)))
          (error "register-codec!: only root authority may shadow a built-in private codec" name))
        (when (and override-builtin-private? (not (root-authority?)))
          (error "register-codec!: only root authority may override built-in private codecs" name))
        (when (and override-builtin-private? (not private?))
          (error "register-codec!: cannot override built-in private codecs with a non-private codec" name))
        (when (and (not private?)
                   (or (and builtin (codec-private? builtin))
                       other-private?))
          (error "register-codec!: cannot weaken a private codec by shadowing it as non-private" name)))
      (registry-add! %extension-codecs name
                     (%make-codec name doc
                                  (scope-procedure match-proc)
                                  (and logical-path-proc (scope-procedure logical-path-proc))
                                  (and init-envelope-proc (scope-procedure init-envelope-proc))
                                  (scope-procedure decode-proc)
                                  (scope-procedure encode-proc)
                                  (and private? #t)
                                  priority
                                  (and override-builtin-private? #t)))
      name)

    (define-syntax define-codec
      (syntax-rules ()
        "Define and register a scope-owned codec NAME documented by DOC.
The remaining arguments are keywords as `register-codec!' takes them."
        ((_ name doc arg ...)
         (register-codec! 'name doc arg ...))))

    (define (find-codec name)
      "Return the registered codec named NAME (a symbol), or #f."
      (if (codec? name)
          name
          (or (registry-ref %extension-codecs name)
              (hash-table-ref/default %builtin-codecs name #f))))

    (define (codec-names)
      "Return the names of all registered codecs."
      (delete-duplicates (append (registry-keys %extension-codecs)
                                 (hash-table-keys %builtin-codecs))))

    (define (resolve-codec c caller)
      (let ((resolved (if (codec? c) c (and (symbol? c) (find-codec c)))))
        (unless resolved
          (error (string-append caller ": unknown codec") c))
        resolved))

    (define (insert-codec-ordered c sorted)
      (if (null? sorted)
          (list c)
          (let ((h (car sorted)))
            (if (or (> (codec-priority c) (codec-priority h))
                    (and (= (codec-priority c) (codec-priority h))
                         (string<? (symbol->string (codec-name c))
                                   (symbol->string (codec-name h)))))
                (cons c sorted)
                (cons h (insert-codec-ordered c (cdr sorted)))))))

    (define (ordered-codecs)
      (fold (lambda (name acc)
              (let ((c (find-codec name)))
                (if c (insert-codec-ordered c acc) acc)))
            '()
            (codec-names)))

    (define (copy-path-arg path)
      (if (string? path) (string-copy path) path))

    (define (codecs-for-path path)
      "Return all registered codecs matching PATH, ordered by priority."
      (if (string? path)
          (let ((key (string-copy path)))
            (filter (lambda (c) (and ((%codec-match-proc c) (string-copy key)) #t))
                    (ordered-codecs)))
          '()))

    (define (builtin-private-codecs-for-path path)
      (if (string? path)
          (fold (lambda (c acc)
                  (if (and (codec-private? c)
                           ((%codec-match-proc c) (string-copy path)))
                      (insert-codec-ordered c acc)
                      acc))
                '()
                (hash-table-values %builtin-codecs))
          '()))

    (define (codec-authorized-for-builtin-private? c protected)
      (or (any (lambda (b) (eq? (codec-name c) (codec-name b))) protected)
          (%codec-override-builtin-private? c)))

    (define (codec-for-path path)
      "Return the highest-priority registered codec matching PATH, or #f.
When PATH matches a protected built-in private codec, unauthorized
differently-named codecs cannot displace it during automatic selection:
only the matching built-in private codec, a root-scope same-name shadow
of it, or a root-scope private codec explicitly registered with
`#:override-builtin-private? #t` may be selected automatically."
      (if (string? path)
          (let* ((key (string-copy path))
                 (protected (builtin-private-codecs-for-path key)))
            (cond
              ((pair? protected)
               (let ((authorized (filter (lambda (c)
                                           (and (codec-authorized-for-builtin-private? c protected)
                                                ((%codec-match-proc c) (string-copy key))))
                                         (ordered-codecs))))
                 (if (pair? authorized)
                     (car authorized)
                     (car protected))))
              (else
               (let ((matches (codecs-for-path key)))
                 (and (pair? matches) (car matches))))))
          #f))

    (define (all-active-and-shadowed-codecs)
      (append (hash-table-values %builtin-codecs)
              (append-map (lambda (name)
                            (map car (registry-entries %extension-codecs name)))
                          (registry-keys %extension-codecs))))

    (define (path-private-by-codec? path)
      "Return #t if any registered codec matching PATH is marked private.
Checks all active and shadowed built-in and extension registrations so
neither a higher-priority nor a same-name codec can strip privacy."
      (and (string? path)
           (let ((key (string-copy path)))
             (or (pair? (builtin-private-codecs-for-path key))
                 (any (lambda (c)
                        (and (codec-private? c)
                             ((%codec-match-proc c) (string-copy key))))
                      (all-active-and-shadowed-codecs))))
           #t))

    (define (codec-logical-path c path)
      "Return the logical path for PATH under codec C (or PATH if unchanged)."
      (let* ((c (resolve-codec c "codec-logical-path"))
             (key (copy-path-arg path))
             (proc (%codec-logical-path-proc c)))
        (if proc
            (let ((out (proc (copy-path-arg key))))
              (if (string? out) (string-copy out) key))
            key)))

    (define (codec-init-envelope c path)
      "Return an initial envelope for a new file at PATH under codec C."
      (let* ((c (resolve-codec c "codec-init-envelope"))
             (proc (%codec-init-envelope-proc c)))
        (if proc (proc (copy-path-arg path)) #f)))

    (define (codec-decode c path raw-bytes)
      "Decode RAW-BYTES at PATH using codec C.
RAW-BYTES is `#f` for a new file or a bytevector. Returns a `codec-result`
holding UTF-8 text and an optional envelope."
      (let ((c (resolve-codec c "codec-decode")))
        (unless (or (not raw-bytes) (bytevector? raw-bytes))
          (error "codec-decode: raw-bytes must be #f or a bytevector"))
        (let ((res ((%codec-decode-proc c) (copy-path-arg path)
                                           (and raw-bytes (bytevector-copy raw-bytes)))))
          (cond ((codec-result? res) res)
                ((string? res) (make-codec-result res))
                (else (error "codec-decode: codec returned invalid result" (codec-name c)))))))

    (define (codec-encode c path text envelope)
      "Encode UTF-8 TEXT with ENVELOPE at PATH using codec C.
Returns a bytevector of stored bytes."
      (let ((c (resolve-codec c "codec-encode")))
        (unless (string? text)
          (error "codec-encode: text must be a string"))
        (let ((out ((%codec-encode-proc c) (copy-path-arg path) (string-copy text) envelope)))
          (unless (bytevector? out)
            (error "codec-encode: codec did not return a bytevector" (codec-name c)))
          out)))

    ;;; Built-in gzip codec (non-private test/utility codec)

    (define (path-basename path)
      (let loop ((i (- (string-length path) 1)))
        (cond ((< i 0) path)
              ((char=? (string-ref path i) #\/) (substring path (+ i 1)))
              (else (loop (- i 1))))))

    (define (gzip-match? path)
      (and (string? path)
           (let ((base (path-basename path)))
             (and (> (string-length base) 3)
                  (string-suffix? ".gz" base)
                  (not (string-suffix? ".tar.gz" base))))))

    (define (gzip-logical-path path)
      (if (and (string? path) (string-suffix? ".gz" path))
          (substring path 0 (- (string-length path) 3))
          path))

    (define (gzip-decode path raw-bytes)
      (if (or (not raw-bytes) (= (bytevector-length raw-bytes) 0))
          (make-codec-result "" #:envelope '((format . gzip)))
          (let* ((res (run-trusted-tool-bytes (default-gzip-program) '("-dc")
                                              #:stdin raw-bytes
                                              #:max-stdout (default-codec-max-bytes)
                                              #:timeout (default-codec-timeout)
                                              #:label "gzip"))
                 (status (car res))
                 (out (cadr res))
                 (err (string-trim (caddr res))))
            (unless (eqv? status 0)
              (error (if (string=? err "")
                         (string-append "gzip: failed to decompress " path)
                         (string-append "gzip: failed to decompress " path ": " err))))
            (make-codec-result (utf8->string out) #:envelope '((format . gzip))))))

    (define (gzip-encode path text envelope)
      (let* ((res (run-trusted-tool-bytes (default-gzip-program) '("-c" "-n")
                                          #:stdin (string->utf8 text)
                                          #:max-stdout (default-codec-max-bytes)
                                          #:timeout (default-codec-timeout)
                                          #:label "gzip"))
             (status (car res))
             (out (cadr res))
             (err (string-trim (caddr res))))
        (unless (eqv? status 0)
          (error (if (string=? err "")
                     (string-append "gzip: failed to compress " path)
                     (string-append "gzip: failed to compress " path ": " err))))
        out))

    (register-builtin-codec! 'gzip
                             "Gzip-compressed UTF-8 text files (`.gz`, excluding `.tar.gz`)."
                             #:match gzip-match?
                             #:logical-path gzip-logical-path
                             #:init-envelope (lambda (_path) '((format . gzip)))
                             #:decode gzip-decode
                             #:encode gzip-encode
                             #:private #f
                             #:priority 0)))
