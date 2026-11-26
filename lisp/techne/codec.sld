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
          codec-logical-path codec-init-envelope codec-save-description
          codec-decode codec-encode
          default-gzip-program default-codec-max-bytes default-codec-timeout
          default-gpg-program default-gpg-homedir default-gpg-recipients
          default-gpg-id-roots default-gpg-trust-file-recipients
          default-gpg-armor default-gpg-batch default-gpg-timeout
          default-age-program default-age-identities default-age-recipients
          default-age-recipients-file default-age-recipient-roots
          default-age-armor default-age-timeout)
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
                             #:priority 0)

    ;;; Shared helpers for encrypted codecs (gpg / age)

    (define %max-recipient-file-chars 65536)
    (define %gpg-max-stderr 65536)

    (define (strip-trailing-slash p)
      (if (and (> (string-length p) 1) (string-suffix? "/" p))
          (strip-trailing-slash (substring p 0 (- (string-length p) 1)))
          p))

    (define (dir-under-root? dir root)
      (let ((d (strip-trailing-slash dir))
            (r (strip-trailing-slash root)))
        (or (string=? r "/")
            (string=? d r)
            (string-prefix? (string-append r "/") d))))

    (define (bytevector-ascii-prefix? bv prefix)
      (let* ((plen (string-length prefix))
             (blen (bytevector-length bv)))
        (let skip ((start 0))
          (if (and (< start blen)
                   (< start 64)
                   (let ((b (bytevector-u8-ref bv start)))
                     (or (= b 32) (= b 9) (= b 10) (= b 13))))
              (skip (+ start 1))
              (and (>= (- blen start) plen)
                   (let loop ((i 0))
                     (cond ((= i plen) #t)
                           ((= (bytevector-u8-ref bv (+ start i))
                               (char->integer (string-ref prefix i)))
                            (loop (+ i 1)))
                           (else #f))))))))

    (define (validate-recipient-string raw caller)
      (unless (string? raw)
        (error (string-append caller ": recipient must be a string") raw))
      (let ((s (string-trim raw)))
        (when (string=? s "")
          (error (string-append caller ": recipient must not be empty")))
        (when (string-prefix? "-" s)
          (error (string-append caller ": recipient must not start with '-'") s))
        (when (or (string-index s #\newline)
                  (string-index s #\return)
                  (string-index s #\null))
          (error (string-append caller ": recipient contains illegal control character")))
        s))

    (define (validate-recipient-list lst caller)
      (unless (list? lst)
        (error (string-append caller ": recipients must be a list")))
      (delete-duplicates (map (lambda (r) (validate-recipient-string r caller)) lst)))

    (define (decode-utf8-for-codec bv path caller)
      (guard (e (#t (error (string-append caller ": decrypted output is not valid UTF-8: " path))))
        (utf8->string bv)))

    (define (resolve-trusted-abs-path raw caller)
      (unless (and (string? raw) (> (string-length raw) 0))
        (error (string-append caller ": path must be a non-empty string")))
      (let ((expanded (expand-trusted-home raw)))
        (unless (string-prefix? "/" expanded)
          (error (string-append caller ": relative path is not allowed") raw))
        expanded))

    (define (parse-recipient-lines content path caller)
      (let loop ((lines (string-split content "\n")) (acc '()))
        (if (null? lines)
            (let ((recips (delete-duplicates (reverse acc))))
              (when (null? recips)
                (error (string-append caller ": recipient file contains no recipients: " path)))
              recips)
            (let ((line (string-trim (car lines))))
              (if (or (string=? line "") (string-prefix? "#" line))
                  (loop (cdr lines) acc)
                  (loop (cdr lines)
                        (cons (validate-recipient-string line caller) acc)))))))

    (define (read-recipient-lines path caller)
      (let ((content (read-regular-file path %max-recipient-file-chars)))
        (parse-recipient-lines content path caller)))

    (define (nearest-existing-dir path)
      (let loop ((d (path-directory path)))
        (cond ((or (not d) (string=? d "")) #f)
              ((guard (e (#t #f)) (file-attributes d))
               (strip-trailing-slash d))
              ((string=? d "/") "/")
              (else
               (let ((parent (path-directory (strip-trailing-slash d))))
                 (if (or (not parent) (string=? parent d)) #f (loop parent)))))))

    (define (allowed-discovery-root dir roots caller)
      (cond
        ((eq? roots #t) "/")
        ((or (not roots) (null? roots)) #f)
        ((list? roots)
         (let loop ((xs roots) (best #f))
           (if (null? xs)
               best
               (let ((cand (guard (e (#t #f))
                             (strip-trailing-slash (resolve-trusted-abs-path (car xs) caller)))))
                 (if (and cand
                          (dir-under-root? dir cand)
                          (or (not best) (> (string-length cand) (string-length best))))
                     (loop (cdr xs) cand)
                     (loop (cdr xs) best))))))
        (else (error (string-append caller ": roots must be #f, #t, or a list of directory paths")))))

    (define (relative-under-root root dir filename)
      (cond
        ((string=? dir root) filename)
        ((string=? root "/")
         (let ((tail (if (string-prefix? "/" dir) (substring dir 1) dir)))
           (if (string=? tail "") filename (string-append tail "/" filename))))
        (else
         (string-append (substring dir (+ (string-length root) 1)) "/" filename))))

    (define (discover-recipient-file path filename roots caller)
      (and (string? path)
           (string-prefix? "/" path)
           (not (remote-path? path))
           (let ((start-dir (nearest-existing-dir path)))
             (and start-dir
                  (let ((stop-root (allowed-discovery-root start-dir roots caller)))
                    (and stop-root
                         (let loop ((dir start-dir))
                           (let* ((rel (relative-under-root stop-root dir filename))
                                  (cand (path-join dir filename))
                                  (content (read-confined-file stop-root rel %max-recipient-file-chars)))
                             (cond
                               ((string? content)
                                (parse-recipient-lines content cand caller))
                               ((string=? dir stop-root) #f)
                               ((string=? dir "/") #f)
                               (else
                                (let ((parent (strip-trailing-slash (path-directory dir))))
                                  (if (or (string=? parent dir)
                                          (not (dir-under-root? parent stop-root)))
                                      #f
                                      (loop parent)))))))))))))

    (define (envelope-ref env key default)
      (let ((p (and (list? env) (assq key env))))
        (if p (cdr p) default)))

    (define (append-flag-values flag values)
      (let loop ((xs values) (acc '()))
        (if (null? xs)
            (reverse acc)
            (loop (cdr xs) (cons (car xs) (cons flag acc))))))

    ;;; Built-in GPG codec (#:private #t)

    (define (gpg-match? path)
      (and (string? path)
           (let ((base (path-basename path)))
             (and (> (string-length base) 4)
                  (string-suffix? ".gpg" base)))))

    (define (gpg-logical-path path)
      (if (and (string? path) (string-suffix? ".gpg" path))
          (substring path 0 (- (string-length path) 4))
          path))

    (define (gpg-env)
      (let ((hd (default-gpg-homedir)))
        (if hd
            (list (cons "GNUPGHOME" (resolve-trusted-abs-path hd "gpg")))
            '())))

    (define (hex-char? ch)
      (or (and (char>=? ch #\0) (char<=? ch #\9))
          (and (char>=? ch #\a) (char<=? ch #\f))
          (and (char>=? ch #\A) (char<=? ch #\F))))

    (define (valid-gpg-keyid? s)
      (and (string? s)
           (= (string-length s) 16)
           (let loop ((i 0))
             (or (= i 16)
                 (and (hex-char? (string-ref s i))
                      (loop (+ i 1)))))))

    ;; Parse `[GNUPG:] ` status lines strictly.
    ;; Returns: (enc-to hidden? sym? dec-complete? enc-complete? bad-status?)
    (define (gpg-parse-status stderr)
      (let* ((raw (or stderr ""))
             (ends-nl? (or (string=? raw "") (string-suffix? "\n" raw)))
             (raw-lines (string-split raw "\n")))
        (let loop ((lines raw-lines)
                   (enc-to '())
                   (hidden? #f)
                   (sym? #f)
                   (dec-state 'none)    ; none -> begun -> okay -> ended
                   (dec-count 0)
                   (enc-state 'none)    ; none -> begun -> ended
                   (enc-count 0)
                   (bad? #f))
          (if (null? lines)
              (list (delete-duplicates (reverse enc-to))
                    hidden?
                    sym?
                    (and (not bad?) (eq? dec-state 'ended) (= dec-count 1))
                    (and (not bad?) (eq? enc-state 'ended) (= enc-count 1))
                    bad?)
              (let* ((is-last? (null? (cdr lines)))
                     (line (string-trim-right (car lines))))
                (if (string-prefix? "[GNUPG:] " line)
                    (if (and is-last? (not ends-nl?))
                        ;; Unterminated trailing status line: fail closed.
                        (loop (cdr lines) enc-to hidden? sym? dec-state dec-count enc-state enc-count #t)
                        (let* ((rest (substring line 9))
                               (parts (filter (lambda (s) (> (string-length s) 0))
                                              (string-split rest " ")))
                               (tag (and (pair? parts) (car parts))))
                          (cond
                            ((equal? tag "ENC_TO")
                             (let ((keyid (and (pair? (cdr parts)) (cadr parts))))
                               (cond
                                 ((not (valid-gpg-keyid? keyid))
                                  (loop (cdr lines) enc-to hidden? sym? dec-state dec-count enc-state enc-count #t))
                                 ((string=? keyid "0000000000000000")
                                  (loop (cdr lines) enc-to #t sym? dec-state dec-count enc-state enc-count bad?))
                                 (else
                                  (loop (cdr lines) (cons keyid enc-to) hidden? sym? dec-state dec-count enc-state enc-count bad?)))))
                            ((or (equal? tag "NEED_PASSPHRASE_SYM") (equal? tag "SYMKEY"))
                             (loop (cdr lines) enc-to hidden? #t dec-state dec-count enc-state enc-count bad?))
                            ((equal? tag "BEGIN_DECRYPTION")
                             (if (eq? dec-state 'none)
                                 (loop (cdr lines) enc-to hidden? sym? 'begun (+ dec-count 1) enc-state enc-count bad?)
                                 (loop (cdr lines) enc-to hidden? sym? dec-state (+ dec-count 1) enc-state enc-count #t)))
                            ((equal? tag "DECRYPTION_OKAY")
                             (if (eq? dec-state 'begun)
                                 (loop (cdr lines) enc-to hidden? sym? 'okay dec-count enc-state enc-count bad?)
                                 (loop (cdr lines) enc-to hidden? sym? dec-state dec-count enc-state enc-count #t)))
                            ((equal? tag "END_DECRYPTION")
                             (if (eq? dec-state 'okay)
                                 (loop (cdr lines) enc-to hidden? sym? 'ended dec-count enc-state enc-count bad?)
                                 (loop (cdr lines) enc-to hidden? sym? dec-state dec-count enc-state enc-count #t)))
                            ((equal? tag "BEGIN_ENCRYPTION")
                             (if (eq? enc-state 'none)
                                 (loop (cdr lines) enc-to hidden? sym? dec-state dec-count 'begun (+ enc-count 1) bad?)
                                 (loop (cdr lines) enc-to hidden? sym? dec-state dec-count enc-state (+ enc-count 1) #t)))
                            ((equal? tag "END_ENCRYPTION")
                             (if (eq? enc-state 'begun)
                                 (loop (cdr lines) enc-to hidden? sym? dec-state dec-count 'ended enc-count bad?)
                                 (loop (cdr lines) enc-to hidden? sym? dec-state dec-count enc-state enc-count #t)))
                            (else
                             (loop (cdr lines) enc-to hidden? sym? dec-state dec-count enc-state enc-count bad?)))))
                    (loop (cdr lines) enc-to hidden? sym? dec-state dec-count enc-state enc-count bad?)))))))

    ;; Returns (cons source recipients) where source is 'explicit, 'gpg-id, or #f.
    (define (gpg-trusted-policy path)
      (let ((explicit (default-gpg-recipients)))
        (cond
          ((and (list? explicit) (pair? explicit))
           (cons 'explicit (validate-recipient-list explicit "gpg")))
          ((not (list? explicit))
           (error "gpg: default-gpg-recipients must be a list"))
          (else
           (let ((found (discover-recipient-file path ".gpg-id" (default-gpg-id-roots) "gpg")))
             (if found
                 (cons 'gpg-id found)
                 (cons #f '())))))))

    (define (gpg-init-envelope path)
      (let ((pol (gpg-trusted-policy path)))
        `((format . gpg)
          (existing . #f)
          (mode . public-key)
          (recipients . ,(cdr pol))
          (recipient-source . ,(car pol))
          (armor . ,(and (default-gpg-armor) #t))
          (hidden-recipients . #f))))

    (define (gpg-batch-args)
      (if (default-gpg-batch) '("--batch" "--pinentry-mode" "error") '()))

    (define (gpg-decode path raw-bytes)
      (cond
        ((not raw-bytes)
         (make-codec-result "" #:envelope (gpg-init-envelope path)))
        ((= (bytevector-length raw-bytes) 0)
         (error (string-append "gpg: empty file is not valid encrypted data: " path)))
        (else
         (let* ((armored? (bytevector-ascii-prefix? raw-bytes "-----BEGIN PGP MESSAGE-----"))
                (args (append '("--no-options" "--no-tty" "--yes" "--quiet"
                                "--status-fd" "2" "--output" "-")
                              (gpg-batch-args)
                              '("--decrypt")))
                (res (run-trusted-tool-bytes (default-gpg-program) args
                                             #:stdin raw-bytes
                                             #:environment (gpg-env)
                                             #:max-stdout (default-codec-max-bytes)
                                             #:max-stderr %gpg-max-stderr
                                             #:strict-stderr #t
                                             #:strict-baseline #t
                                             #:timeout (default-gpg-timeout)
                                             #:label "gpg"))
                (status (car res))
                (out (cadr res))
                (parsed (gpg-parse-status (caddr res)))
                (enc-to (list-ref parsed 0))
                (hidden? (list-ref parsed 1))
                (sym? (list-ref parsed 2))
                (dec-ok? (list-ref parsed 3))
                (bad-status? (list-ref parsed 5)))
           (unless (eqv? status 0)
             ;; Fixed, redacted failure category: never forward raw helper stderr.
             (error (string-append "gpg: failed to decrypt " path " (exit " (number->string status) ")")))
           (when (or bad-status? (not dec-ok?))
             (error (string-append "gpg: output was not a verified decrypted OpenPGP message: " path)))
           (let* ((has-pk? (or (pair? enc-to) hidden?))
                  (mode (cond ((and has-pk? sym?) 'hybrid)
                              ((and (not has-pk?) sym?) 'symmetric)
                              (has-pk? 'public-key)
                              (else 'unknown)))
                  (env `((format . gpg)
                         (existing . #t)
                         (mode . ,mode)
                         (recipients . ,enc-to)
                         (armor . ,armored?)
                         (hidden-recipients . ,hidden?))))
             (make-codec-result (decode-utf8-for-codec out path "gpg") #:envelope env))))))

    (define (gpg-resolve-encode-recipients path env)
      (let* ((mode (envelope-ref env 'mode 'public-key))
             (existing? (and (envelope-ref env 'existing #f) #t))
             (hidden? (and (envelope-ref env 'hidden-recipients #f) #t))
             (env-recips (envelope-ref env 'recipients '())))
        (cond
          ((eq? mode 'hybrid)
           (error (string-append "gpg: dual public-key + symmetric encryption is not supported for saving " path)))
          ((eq? mode 'unknown)
           (error (string-append "gpg: existing file had no recognized recipient or symmetric metadata: " path)))
          ((eq? mode 'symmetric)
           (cons 'symmetric '()))
          (else
           (when (and existing? (null? env-recips) (not hidden?))
             (error (string-append "gpg: existing file had no recognized recipient metadata: " path)))
           (let ((pol (gpg-trusted-policy path)))
             (cond
               ((pair? (cdr pol)) pol)
               ((and existing? (not hidden?) (default-gpg-trust-file-recipients) (pair? env-recips))
                (cons 'file-enc-to (validate-recipient-list env-recips "gpg")))
               (hidden?
                (error (string-append "gpg: file uses hidden recipients; set default-gpg-recipients or default-gpg-id-roots to re-encrypt " path)))
               (existing?
                (error (string-append "gpg: refusing to re-encrypt using unauthenticated ciphertext ENC_TO recipients for " path
                                      "; set default-gpg-recipients or default-gpg-id-roots (or opt in via default-gpg-trust-file-recipients)")))
               (else
                (error (string-append "gpg: no recipients configured for " path
                                      "; set default-gpg-recipients or default-gpg-id-roots")))))))))

    (define (gpg-encode path text envelope)
      (let* ((env (or envelope (gpg-init-envelope path)))
             (mode (envelope-ref env 'mode 'public-key))
             (armor? (and (envelope-ref env 'armor (default-gpg-armor)) #t))
             (hidden? (and (envelope-ref env 'hidden-recipients #f) #t))
             (resolved (gpg-resolve-encode-recipients path env))
             (recips (cdr resolved))
             (recip-flag (if hidden? "--hidden-recipient" "--recipient")))
        (let* ((args (append '("--no-options" "--no-tty" "--yes" "--quiet"
                               "--status-fd" "2" "--output" "-"
                               "--no-encrypt-to" "--no-default-recipient")
                             (if armor? '("--armor") '("--no-armor"))
                             (if (eq? mode 'symmetric)
                                 (append (gpg-batch-args)
                                         '("--symmetric"))
                                 (append '("--batch" "--pinentry-mode" "error" "--encrypt")
                                         (append-flag-values recip-flag recips)))))
               (res (run-trusted-tool-bytes (default-gpg-program) args
                                            #:stdin (string->utf8 text)
                                            #:environment (gpg-env)
                                            #:max-stdout (default-codec-max-bytes)
                                            #:max-stderr %gpg-max-stderr
                                            #:strict-stderr #t
                                            #:strict-baseline #t
                                            #:timeout (default-gpg-timeout)
                                            #:label "gpg"))
               (status (car res))
               (out (cadr res))
               (parsed (gpg-parse-status (caddr res)))
               (enc-ok? (list-ref parsed 4))
               (bad-status? (list-ref parsed 5)))
          (unless (eqv? status 0)
            ;; Fixed, redacted failure category: never forward raw helper stderr.
            (error (string-append "gpg: failed to encrypt " path " (exit " (number->string status) ")")))
          (when (or bad-status? (not enc-ok?) (= (bytevector-length out) 0))
            (error (string-append "gpg: encryption produced incomplete output for " path)))
          out)))

    (register-builtin-codec! 'gpg
                             "OpenPGP/GPG encrypted UTF-8 text files (`.gpg`)."
                             #:match gpg-match?
                             #:logical-path gpg-logical-path
                             #:init-envelope gpg-init-envelope
                             #:decode gpg-decode
                             #:encode gpg-encode
                             #:private #t
                             #:priority 0)

    ;;; Built-in age codec (#:private #t)

    (define (age-match? path)
      (and (string? path)
           (let ((base (path-basename path)))
             (and (> (string-length base) 4)
                  (string-suffix? ".age" base)))))

    (define (age-logical-path path)
      (if (and (string? path) (string-suffix? ".age" path))
          (substring path 0 (- (string-length path) 4))
          path))

    (define (validate-age-identity-file abs)
      (let* ((real (guard (e (#t #f)) (file-truename abs)))
             (attrs (and real (guard (e (#t #f)) (file-attributes real)))))
        (unless (and real (pair? attrs) (eq? (car attrs) 'file))
          (error (string-append "age: identity file is missing or not a regular file: " abs)))
        abs))

    (define (resolve-age-identities)
      (let ((explicit (default-age-identities)))
        (unless (list? explicit)
          (error "age: default-age-identities must be a list of file paths"))
        (cond
          ((pair? explicit)
           (map (lambda (p) (validate-age-identity-file (resolve-trusted-abs-path p "age")))
                explicit))
          (else
           (let* ((h (trusted-home))
                  (def (and h (path-join h ".config/age/keys.txt"))))
             (if (and def (file-exists? def))
                 (list (validate-age-identity-file def))
                 (error "age: no identity file found; set default-age-identities or create ~/.config/age/keys.txt")))))))

    ;; Returns (cons source recipients) where source is 'explicit, 'recipients-file, 'age-recipients, or #f.
    (define (resolve-age-recipients-with-source path)
      (let ((explicit (default-age-recipients))
            (rfile (default-age-recipients-file)))
        (unless (list? explicit)
          (error "age: default-age-recipients must be a list"))
        (cond
          ((pair? explicit)
           (cons 'explicit (validate-recipient-list explicit "age")))
          (rfile
           (cons 'recipients-file (read-recipient-lines (resolve-trusted-abs-path rfile "age") "age")))
          (else
           (let ((found (discover-recipient-file path ".age-recipients" (default-age-recipient-roots) "age")))
             (if found
                 (cons 'age-recipients found)
                 (cons #f '())))))))

    (define (age-init-envelope path)
      (let ((pol (resolve-age-recipients-with-source path)))
        `((format . age)
          (existing . #f)
          (recipients . ,(cdr pol))
          (recipient-source . ,(car pol))
          (armor . ,(and (default-age-armor) #t)))))

    (define (age-decode path raw-bytes)
      (cond
        ((not raw-bytes)
         (make-codec-result "" #:envelope (age-init-envelope path)))
        ((= (bytevector-length raw-bytes) 0)
         (error (string-append "age: empty file is not valid encrypted data: " path)))
        (else
         (let* ((armored? (bytevector-ascii-prefix? raw-bytes "-----BEGIN AGE ENCRYPTED FILE-----"))
                (identities (resolve-age-identities))
                (args (cons "-d" (append-flag-values "-i" identities)))
                (res (run-trusted-tool-bytes (default-age-program) args
                                             #:stdin raw-bytes
                                             #:max-stdout (default-codec-max-bytes)
                                             #:strict-baseline #t
                                             #:timeout (default-age-timeout)
                                             #:label "age"))
                (status (car res))
                (out (cadr res)))
           (unless (eqv? status 0)
             ;; Fixed, redacted failure category: never forward raw helper stderr.
             (error (string-append "age: failed to decrypt " path " (exit " (number->string status) ")")))
           (let ((env `((format . age)
                        (existing . #t)
                        (armor . ,armored?))))
             (make-codec-result (decode-utf8-for-codec out path "age") #:envelope env))))))

    (define (age-encode path text envelope)
      (let* ((env (or envelope (age-init-envelope path)))
             (armor? (and (envelope-ref env 'armor (default-age-armor)) #t))
             (pol (resolve-age-recipients-with-source path))
             (recips (cdr pol)))
        (when (null? recips)
          (error (string-append "age: no recipients configured for " path
                                "; set default-age-recipients, default-age-recipients-file, or default-age-recipient-roots")))
        (let* ((args (append '("-e")
                             (if armor? '("-a") '())
                             (append-flag-values "-r" recips)))
               (res (run-trusted-tool-bytes (default-age-program) args
                                            #:stdin (string->utf8 text)
                                            #:max-stdout (default-codec-max-bytes)
                                            #:strict-baseline #t
                                            #:timeout (default-age-timeout)
                                            #:label "age"))
               (status (car res))
               (out (cadr res)))
          (unless (eqv? status 0)
            ;; Fixed, redacted failure category: never forward raw helper stderr.
            (error (string-append "age: failed to encrypt " path " (exit " (number->string status) ")")))
          (when (= (bytevector-length out) 0)
            (error (string-append "age: encryption produced empty output for " path)))
          out)))

    (define (codec-save-description codec path envelope)
      "Return a short description of how CODEC encodes PATH with ENVELOPE.
Describes details such as recipient count and source, or returns #f."
      (guard (e (#t #f))
        (let ((builtin-age (hash-table-ref/default %builtin-codecs 'age #f))
              (builtin-gpg (hash-table-ref/default %builtin-codecs 'gpg #f)))
          (cond
            ((and builtin-age (eq? codec builtin-age))
             (let* ((pol (resolve-age-recipients-with-source path))
                    (src (car pol))
                    (n (length (cdr pol))))
               (and (> n 0)
                    (string-append (number->string n)
                                   (if (= n 1) " recipient from " " recipients from ")
                                   (cond ((eq? src 'age-recipients) ".age-recipients")
                                         ((eq? src 'recipients-file) "age-recipients-file")
                                         (else "age-recipients"))))))
            ((and builtin-gpg (eq? codec builtin-gpg))
             (let ((mode (envelope-ref envelope 'mode 'public-key)))
               (cond
                 ((eq? mode 'symmetric) "symmetric")
                 ((eq? mode 'public-key)
                  (let* ((pol (gpg-resolve-encode-recipients path envelope))
                         (src (car pol))
                         (n (length (cdr pol))))
                    (and (> n 0)
                         (string-append (number->string n)
                                        (if (= n 1) " recipient from " " recipients from ")
                                        (cond ((eq? src 'gpg-id) ".gpg-id")
                                              ((eq? src 'file-enc-to) "file ENC_TO")
                                              (else "gpg-recipients"))))))
                 (else #f))))
            (else #f)))))

    (register-builtin-codec! 'age
                             "age encrypted UTF-8 text files (`.age`)."
                             #:match age-match?
                             #:logical-path age-logical-path
                             #:init-envelope age-init-envelope
                             #:decode age-decode
                             #:encode age-encode
                             #:private #t
                             #:priority 0)))
