;;; Files: paths, and the documents of files. A file is opened once, with
;;; its journal (or without one under the private profile): a file visited
;;; again is the same document. Codec-backed files decode stored bytes on
;;; open/reload and encode text asynchronously before checked saves.

(import (techne codec params))
(require "modes.scm")
(require "remote.scm")
(require "session.scm")
(require "dispatch.scm")
(require "operations.scm")

(provide file-document absolute-path directory-of file-name working-directory normal-directory abbreviate-path state-file
         open-file-documents open-document-at cached-open-document-at visit-needs-async?
         file-document-moved! save-document! reload-document! revert-document! directory-entries
         changed-on-disk? file-text
         path-exists?
         document-codec document-codec-envelope document-unretained? document-private? mark-document-private! document-logical-path private-path?
         document-save-description
         %attach-file-document
         codec? codec-name codec-doc codec-private? codec-priority
         codec-result? make-codec-result codec-result-text codec-result-envelope
         register-codec! define-codec find-codec codec-names
         codecs-for-path codec-for-path path-private-by-codec? codec-logical-path codec-init-envelope
         codec-decode codec-encode codec-save-description
         default-gzip-program default-codec-max-bytes default-codec-timeout
         default-gpg-program default-gpg-homedir default-gpg-recipients
         default-gpg-id-roots default-gpg-trust-file-recipients
         default-gpg-armor default-gpg-batch default-gpg-timeout
         default-age-program default-age-identities default-age-recipients
         default-age-recipients-file default-age-recipient-roots
         default-age-armor default-age-timeout)

;;; Lazy bridge to `(techne codec)`.
;;; Loaded on first codec call or built-in codec filename match.

(define %codec-env #f)

(define (ensure-codec!)
  (or %codec-env
      (let ((e (environment '(techne codec))))
        (set! %codec-env e)
        e)))

(define (codec-ref name)
  (eval name (ensure-codec!)))

(define (maybe-builtin-codec-path? path)
  (and (string? path)
       (or (string-suffix? ".gz" path)
           (string-suffix? ".gpg" path)
           (string-suffix? ".age" path))))

(define (codec-bridge-active?)
  (or %codec-env (codec-registry-loaded?)))

(define (codec? x)
  "Return #t if X is a codec."
  (and (codec-bridge-active?) ((codec-ref 'codec?) x)))

(define (codec-name c)
  "Return the symbol name of codec C."
  ((codec-ref 'codec-name) c))

(define (codec-doc c)
  "Return the documentation string of codec C."
  ((codec-ref 'codec-doc) c))

(define (codec-private? c)
  "Return #t if codec C requires the private profile."
  ((codec-ref 'codec-private?) c))

(define (codec-priority c)
  "Return the numeric matching priority of codec C."
  ((codec-ref 'codec-priority) c))

(define (codec-result? x)
  "Return #t if X is a codec decode result."
  (and (codec-bridge-active?) ((codec-ref 'codec-result?) x)))

(define (make-codec-result text #:envelope [envelope #f])
  "Construct a codec decode result with TEXT and optional ENVELOPE."
  ((codec-ref 'make-codec-result) text #:envelope envelope))

(define (codec-result-text r)
  "Return the decoded text string of codec result R."
  ((codec-ref 'codec-result-text) r))

(define (codec-result-envelope r)
  "Return the envelope metadata of codec result R, or #f."
  ((codec-ref 'codec-result-envelope) r))

(define (register-codec! name doc
                         #:match match-proc
                         #:decode decode-proc
                         #:encode encode-proc
                         #:logical-path [logical-path-proc #f]
                         #:init-envelope [init-envelope-proc #f]
                         #:private [private? #f]
                         #:priority [priority 0]
                         #:override-builtin-private? [override-builtin-private? #f])
  "Register scope-owned codec NAME documented by DOC.
MATCH-PROC matches paths, DECODE-PROC decodes bytes, ENCODE-PROC encodes
text; optional LOGICAL-PATH-PROC, INIT-ENVELOPE-PROC, PRIVATE?, PRIORITY,
OVERRIDE-BUILTIN-PRIVATE?."
  ((codec-ref 'register-codec!) name doc
                                #:match match-proc
                                #:decode decode-proc
                                #:encode encode-proc
                                #:logical-path logical-path-proc
                                #:init-envelope init-envelope-proc
                                #:private private?
                                #:priority priority
                                #:override-builtin-private? override-builtin-private?))

(define-syntax define-codec
  (syntax-rules ()
    "Define and register a scope-owned codec NAME documented by DOC.
The remaining arguments are keywords as `register-codec!` takes them."
    ((_ name doc arg ...)
     (register-codec! 'name doc arg ...))))

(define (find-codec name)
  "Return the registered codec named NAME (a symbol), or #f."
  ((codec-ref 'find-codec) name))

(define (codec-names)
  "Return the names of all registered codecs."
  ((codec-ref 'codec-names)))

(define (codecs-for-path path)
  "Return all codecs matching PATH ordered by priority."
  (if (or (codec-bridge-active?) (maybe-builtin-codec-path? path))
      ((codec-ref 'codecs-for-path) path)
      '()))

(define (codec-for-path path)
  "Return the highest-priority codec matching PATH, or #f."
  (and (or (codec-bridge-active?) (maybe-builtin-codec-path? path))
       ((codec-ref 'codec-for-path) path)))

(define (path-private-by-codec? path)
  "Return #t if any codec matching PATH requires the private profile."
  (and (or (codec-bridge-active?) (maybe-builtin-codec-path? path))
       ((codec-ref 'path-private-by-codec?) path)))

(define (codec-logical-path c path)
  "Return the logical path for PATH under codec C."
  ((codec-ref 'codec-logical-path) c path))

(define (codec-init-envelope c path)
  "Return the initial envelope for a new file at PATH under codec C."
  ((codec-ref 'codec-init-envelope) c path))

(define (codec-decode c path raw-bytes)
  "Decode RAW-BYTES for PATH using codec C into a `codec-result`."
  ((codec-ref 'codec-decode) c path raw-bytes))

(define (codec-encode c path text envelope)
  "Encode TEXT for PATH with ENVELOPE using codec C into a bytevector."
  ((codec-ref 'codec-encode) c path text envelope))

(define (codec-save-description c path envelope)
  "Return a short save description for PATH with ENVELOPE under codec C."
  ((codec-ref 'codec-save-description) c path envelope))

;;; User-scope codec options bridging editor configuration to `(techne codec)` parameters.
;;; All codec options are `#:user-only #t` and read with `(option #f ...)` so untrusted
;;; project, buffer, or mode settings can never hijack crypto helpers or recipients.

(define-option gzip-program "gzip"
  "Executable name or absolute path used by the built-in `gzip` codec.
User-scope only; resolved on the clean host PATH."
  #:type '(string #:min 1)
  #:user-only #t)

(define-option codec-max-bytes 67108864
  "Maximum decoded or encoded byte count accepted by built-in codecs.
User-scope only."
  #:type '(natural #:min 1)
  #:user-only #t)

(define-option codec-timeout 30000
  "Default subprocess timeout in milliseconds for built-in codecs.
User-scope only."
  #:type '(natural #:min 1)
  #:user-only #t)

(define-option gpg-program "gpg"
  "Executable name or absolute path used by the built-in `gpg` codec.
User-scope only; resolved on the clean host PATH."
  #:type '(string #:min 1)
  #:user-only #t)

(define-option gpg-homedir #f
  "Optional explicit GNUPGHOME directory for the built-in `gpg` codec.
User-scope only; `#f` uses GnuPG's default home directory."
  #:type '(or (one-of #f) (string #:min 1))
  #:user-only #t)

(define-option gpg-recipients '()
  "List of GPG recipient key IDs or emails for encrypting `.gpg` files.
User-scope only; takes precedence over `.gpg-id` and file metadata."
  #:type '(list-of (string #:min 1))
  #:user-only #t)

(define-option gpg-id-roots '()
  "Trusted local roots (or `#t`) allowing `.gpg-id` recipient discovery.
User-scope only; empty by default so untrusted repositories are ignored."
  #:type '(or boolean (list-of (string #:min 1)))
  #:user-only #t)

(define-option gpg-trust-file-recipients #f
  "Whether saving a `.gpg` file may reuse unauthenticated `ENC_TO` IDs.
Applies when no explicit `gpg-recipients` or trusted `.gpg-id` policy
is configured. Off by default; user-scope only."
  #:type 'boolean
  #:user-only #t)

(define-option gpg-armor #f
  "Whether newly created `.gpg` files use ASCII armor (`--armor`).
Existing files always preserve their own armor format. User-scope only."
  #:type 'boolean
  #:user-only #t)

(define-option gpg-batch #f
  "Whether to pass `--batch` to `gpg` on decrypt (disables pinentry).
User-scope only."
  #:type 'boolean
  #:user-only #t)

(define-option gpg-timeout 120000
  "Subprocess timeout in milliseconds for the built-in `gpg` codec.
User-scope only."
  #:type '(natural #:min 1)
  #:user-only #t)

(define-option age-program "age"
  "Executable name or absolute path used by the built-in `age` codec.
User-scope only; resolved on the clean host PATH."
  #:type '(string #:min 1)
  #:user-only #t)

(define-option age-identities '()
  "List of identity file paths for decrypting `.age` files.
User-scope only; empty falls back to `~/.config/age/keys.txt`."
  #:type '(list-of (string #:min 1))
  #:user-only #t)

(define-option age-recipients '()
  "List of age recipient public keys for encrypting `.age` files.
User-scope only."
  #:type '(list-of (string #:min 1))
  #:user-only #t)

(define-option age-recipients-file #f
  "Optional path to a trusted age recipients file.
User-scope only."
  #:type '(or (one-of #f) (string #:min 1))
  #:user-only #t)

(define-option age-recipient-roots '()
  "Trusted local roots (or `#t`) allowing `.age-recipients` discovery.
User-scope only; empty by default so untrusted repositories are ignored."
  #:type '(or boolean (list-of (string #:min 1)))
  #:user-only #t)

(define-option age-armor #f
  "Whether newly created `.age` files use ASCII armor (`-a`).
Existing files always preserve their own armor format. User-scope only."
  #:type 'boolean
  #:user-only #t)

(define-option age-timeout 60000
  "Subprocess timeout in milliseconds for the built-in `age` codec.
User-scope only."
  #:type '(natural #:min 1)
  #:user-only #t)

(set-codec-option-hook!
  (lambda (opt-name fallback)
    (guard (e (#t fallback)) (option #f opt-name))))

(define (with-editor-codec-options thunk)
  (thunk))

(define (last-slash path)
  (let loop ((i (- (string-length path) 1)))
    (cond ((< i 0) #f)
          ((char=? (string-ref path i) #\/) i)
          (else (loop (- i 1))))))

(define (directory-of path)
  "Return the directory part of PATH, with its slash, or \"\"."
  (let ((i (last-slash path))) (if i (substring path 0 (+ i 1)) "")))

(define (file-name path)
  "Return the part of PATH after its directory."
  (substring path (string-length (directory-of path)) (string-length path)))

(define (home) (or (get-environment-variable "HOME") "/"))

(define (state-file name)
  "Return the file NAME of the editor's state, kept across runs.
It is in techne in $XDG_STATE_HOME, else in ~/.local/state, as XDG
says, a relative $XDG_STATE_HOME being left out; that directory is made
if missing, and only its user can read it."
  (let* ((xdg (get-environment-variable "XDG_STATE_HOME"))
         (dir (string-append (if (and xdg (string-prefix? "/" xdg)) xdg (string-append (home) "/.local/state")) "/techne/")))
    (make-directory dir)
    (set-file-mode! dir #o700)
    (string-append dir name)))
(define (working-directory)
  "Return the directory the editor was started in."
  (or (get-environment-variable "PWD") (home)))

(define (absolute-path path)
  "Return PATH as an absolute path, with ~ for the home directory.
PATH may be a location on another machine (doc/remote.md): its path
there is made absolute, ~ standing for the home directory there."
  (cond ((remote-path? path) (remote-absolute-path path))
        ((string-prefix? "/" path) path)
        ((string-prefix? "~/" path) (string-append (home) (substring path 1 (string-length path))))
        (else (string-append (working-directory) "/" path))))

(define (normal-directory path)
  "Return the directory PATH, absolute, without its . and .. parts.
They are taken away as written, a symbolic link not followed. The
result ends with a slash. On another machine, it is its path there that
is made normal."
  (let* ((path (absolute-path path))
         (parts (fold (lambda (part acc)
                        (cond ((or (string=? part "") (string=? part ".")) acc)
                              ((string=? part "..") (if (null? acc) acc (cdr acc)))
                              (else (cons part acc))))
                      '()
                      (string-split (location-path path) "/"))))
    (location-at path (string-append "/" (apply string-append (map (lambda (p) (string-append p "/")) (reverse parts)))))))

(define (abbreviate-path path)
  "Return PATH with ~ for the home directory when it is inside it.
A home directory that is the root is left, as Emacs leaves it."
  (let ((h (let ((h (if (remote-path? path) "/" (home))))
             (if (and (> (string-length h) 1) (string-suffix? "/" h)) (substring h 0 (- (string-length h) 1)) h))))
    (cond ((string=? h "/") path)
          ((string=? path h) "~")
          ((and (string-prefix? h path) (> (string-length path) (string-length h))
                (char=? (string-ref path (string-length h)) #\/))
           (string-append "~" (substring path (string-length h) (string-length path))))
          (else path))))

(define %documents (make-hash-table))

(define (clone-envelope-value v)
  (cond
    ((or (boolean? v) (symbol? v) (number? v) (null? v) (char? v)) v)
    ((string? v) (string-copy v))
    ((bytevector? v) (bytevector-copy v))
    ((pair? v) (cons (clone-envelope-value (car v)) (clone-envelope-value (cdr v))))
    ((vector? v)
     (let* ((n (vector-length v))
            (out (make-vector n)))
       (let loop ((i 0))
         (when (< i n)
           (vector-set! out i (clone-envelope-value (vector-ref v i)))
           (loop (+ i 1))))
       out))
    ((dict? v)
     (fold (lambda (k acc)
             (dict-set acc (clone-envelope-value k) (clone-envelope-value (dict-ref v k))))
           (dict)
           (dict-keys v)))
    ((hash-table? v)
     (let ((out (make-hash-table)))
       (for-each (lambda (k)
                   (hash-table-set! out (clone-envelope-value k)
                                    (clone-envelope-value (hash-table-ref v k))))
                 (hash-table-keys v))
       out))
    (else v)))

(define-record-type %doc-meta-record
  (%make-doc-meta-raw codec envelope private?)
  %doc-meta?
  (codec %doc-meta-codec)
  (envelope %doc-meta-envelope %set-doc-meta-envelope-raw!)
  (private? %doc-meta-private? %set-doc-meta-private?!))

(define (%make-doc-meta codec envelope private?)
  (%make-doc-meta-raw codec (clone-envelope-value envelope) private?))

(define (%set-doc-meta-envelope! m envelope)
  (%set-doc-meta-envelope-raw! m (clone-envelope-value envelope)))

(define %doc-meta (make-hash-table))
(define %saving-docs (make-hash-table))

(define (doc-meta-of d)
  (hash-table-ref/default %doc-meta (document-id d) #f))

(define (document-codec d)
  "Return the codec associated with document D, or #f."
  (let ((m (doc-meta-of d)))
    (and m (%doc-meta-codec m))))

(define (document-codec-envelope d)
  "Return the codec envelope stored for document D, or #f."
  (let ((m (doc-meta-of d)))
    (and m (clone-envelope-value (%doc-meta-envelope m)))))

(define (document-unretained? d)
  "Return #t if document D uses unretained (`none`) journal retention."
  (and d (eq? (guard (e (#t #f)) (document-retention d)) 'none)))

(define (document-private? d)
  "Return #t if document D uses the private profile.
Private documents keep no journal, refuse LSP attachment, and are
excluded from session persistence."
  (and d
       (or (document-unretained? d)
           (let ((m (guard (e (#t #f)) (doc-meta-of d))))
             (and m (%doc-meta-private? m) #t))
           (and (presentation? d)
                (any document-private? (presentation-sources d))))))

(define (mark-document-private! d)
  "Mark in-memory document D as private for redaction and history.
History, clipboard export, and operation traces treat D as private.
This does not disable an existing file journal."
  (let ((m (doc-meta-of d)))
    (if m
        (%set-doc-meta-private?! m #t)
        (hash-table-set! %doc-meta (document-id d) (%make-doc-meta #f #f #t))))
  d)

(set-document-private-hook! document-private?)

(define (document-logical-path d)
  "Return the logical path of document D, or #f.
Used for mode and syntax detection; for codec-backed documents, applies
`codec-logical-path` to `document-path`."
  (let ((path (document-path d)))
    (and path
         (let ((c (document-codec d)))
           (if c (codec-logical-path c path) path)))))

(set-document-logical-path-hook! document-logical-path)

(define (canonical-file-location abs)
  (if (remote-path? abs)
      (location-at abs (node-file-canonical (location-endpoint abs) (location-path abs)))
      (%canonical-file-path abs)))

(define (canonical-path-or-self path)
  (let ((abs (absolute-path path)))
    (guard (e (#t abs)) (canonical-file-location abs))))

(define (local-canonical-or-self abs)
  (if (remote-path? abs)
      abs
      (guard (e (#t abs)) (%canonical-file-path abs))))

(define (document-save-description d)
  "Return a short save description for codec-backed document D, or #f."
  (let ((codec (document-codec d))
        (path (document-path d)))
    (and codec path
         (guard (e (#t #f))
           (with-editor-codec-options
            (lambda ()
              (codec-save-description codec path (document-codec-envelope d))))))))

(define (document-matches-canonical? existing canon)
  (let ((ep (document-path existing)))
    (and (string? ep) (string=? ep canon))))

(define (path-compatible-with-document? abs canon existing)
  (let* ((canon-codec (codec-for-path canon))
         (abs-codec (if (string=? canon abs) canon-codec (codec-for-path abs)))
         (need-private? (or (and canon-codec (codec-private? canon-codec) #t)
                            (path-private-by-codec? canon)
                            (and (not (string=? canon abs)) (path-private-by-codec? abs)))))
    (and (document-matches-canonical? existing canon)
         (not (and abs-codec (not (eq? abs-codec canon-codec))))
         (not (and need-private? (not (document-unretained? existing))))
         (not (and canon-codec (not (document-decoded? existing))))
         (not (and (not canon-codec) (document-decoded? existing)))
         (not (and canon-codec (document-codec existing) (not (eq? (document-codec existing) canon-codec)))))))

(define (private-path? path)
  "Return #t if PATH or its local canonical target matches a private codec."
  (and (string? path)
       (let* ((abs (absolute-path path))
              (canon (local-canonical-or-self abs))
              (open (cached-open-document-at abs)))
         (or (and open (document-private? open) #t)
             (path-private-by-codec? abs)
             (and (not (string=? canon abs))
                  (path-private-by-codec? canon))))))

(define (cached-open-document-at path)
  "Return the already-open document for PATH without remote I/O, or #f.
Only returns a cached document when PATH and its canonical target match
the open document's canonical path, codec, and privacy profile."
  (and (string? path)
       (let ((abs (absolute-path path)))
         (if (remote-path? abs)
             (let ((existing (hash-table-ref/default %documents abs #f)))
               (and existing
                    (path-compatible-with-document? abs abs existing)
                    existing))
             (let ((canon (guard (e (#t #f)) (%canonical-file-path abs))))
               (and canon
                    (let ((existing (hash-table-ref/default %documents canon #f)))
                      (and existing
                           (path-compatible-with-document? abs canon existing)
                           existing))))))))

(define (open-document-at path)
  "Return the open document for PATH or its canonical target, or #f."
  (and (string? path)
       (let* ((abs (absolute-path path))
              (canon (canonical-path-or-self abs))
              (existing (hash-table-ref/default %documents canon #f)))
         (and existing
              (or (document-matches-canonical? existing canon)
                  (not (document-path existing)))
              existing))))

(define (visit-needs-async? path)
  "Return #t if opening PATH should run in a background task.
Already-open documents do not need async; remote paths and local paths
matching a registered codec do."
  (and (string? path)
       (let ((abs (absolute-path path)))
         (cond
           ((cached-open-document-at abs) #f)
           ((remote-path? abs) #t)
           (else
            (let ((canon (local-canonical-or-self abs)))
              (and (or (codec-for-path canon)
                       (and (not (string=? canon abs))
                            (codec-for-path abs)))
                   #t)))))))

(define (resolve-file-codec path codec-arg)
  (cond ((eq? codec-arg 'auto) (codec-for-path path))
        ((not codec-arg) #f)
        ((codec? codec-arg) codec-arg)
        ((symbol? codec-arg)
         (or (find-codec codec-arg)
             (error "file-document: unknown codec" codec-arg)))
        (else (error "file-document: invalid #:codec argument" codec-arg))))

(define (register-open-document! canon d)
  ;; Key strictly by canonical target path (never by a mutable symlink alias)
  ;; so retargeting a symlink never reuses the previous target's open document.
  (hash-table-set! %documents canon d)
  (let ((actual (document-path d)))
    (when (and actual (not (string=? actual canon)))
      (hash-table-set! %documents actual d)))
  d)

(define (decoded-raced-on-disk? e)
  (and (error-object? e)
       (string-contains (error-object-message e) "file changed on disk while decoding")
       #t))

(define (open-local-codec-document alias canon codec need-private? journal-path)
  (with-editor-codec-options
   (lambda ()
     (let loop ((attempts 0))
       (let* ((raw (%read-file-bytes canon (default-codec-max-bytes)))
              (res (codec-decode codec canon raw))
              (env (or (codec-result-envelope res)
                       (and (not raw) (codec-init-envelope codec canon))))
              (journal-arg (cond (need-private? #f)
                                 ((string? journal-path) journal-path)
                                 (else #t))))
         (guard (e ((and (< attempts 1) (decoded-raced-on-disk? e))
                    (loop (+ attempts 1))))
           (values (%open-file-decoded canon raw (codec-result-text res) journal-arg alias) env)))))))

(define (file-document path #:document [d #f] #:private [private? #f] #:codec [codec-arg 'auto]
                       #:journal-path [journal-path #f] #:alias [alias #f] #:pinned [pinned? #f])
  "Return the document of the file at PATH.
It is opened once; D, a document already open, is registered as the file's.
Codec and privacy matching follow symlinks via the canonical file path (both
locally and on remote nodes, failing closed if canonicalization fails or if
a symlink's own name matches a different codec than its canonical target), and
any matching private codec forces the private profile even if a higher-priority
non-private codec also matches. When PRIVATE? is true or the path is private,
the document uses `Retention::None` (no journal or lock file) and fails closed
if the file is already open with a journal or has an existing journal on disk.
CODEC-ARG defaults to `'auto` (match via `codec-for-path`), may be `#f` to open
raw bytes without a codec, or a codec name/object. JOURNAL-PATH, when given for
a non-private local file, overrides the default state-directory journal path.
ALIAS records the original symlink pathname when PATH is already canonical, and
PINNED? suppresses re-canonicalization so a previously pinned path cannot adopt
a different target on disk."
  (let* ((abs (string-copy (absolute-path path)))
         (remote? (remote-path? abs))
         (canon (if (and pinned? (not remote?))
                    (string-copy abs)
                    (string-copy (canonical-file-location abs))))
         (effective-alias (cond ((string? alias)
                                 (let ((a (string-copy (absolute-path alias))))
                                   (and (not (string=? a canon)) a)))
                                ((not (string=? canon abs)) (string-copy abs))
                                (else #f)))
         (codec (resolve-file-codec canon codec-arg))
         (abs-codec (if (and (eq? codec-arg 'auto) effective-alias)
                        (codec-for-path effective-alias)
                        #f))
         (need-private? (or (and private? #t)
                            (and codec (codec-private? codec) #t)
                            (path-private-by-codec? canon)
                            (and effective-alias (path-private-by-codec? effective-alias))))
         (existing (let ((cand (hash-table-ref/default %documents canon #f)))
                     (and cand
                          (or (document-matches-canonical? cand canon)
                              (not (document-path cand)))
                          cand))))
    (when (and abs-codec (not (eq? abs-codec codec)))
      (error (string-append (abbreviate-path effective-alias)
                            ": symlink codec mismatch with target "
                            (abbreviate-path canon))))
    (cond
      (existing
       (when (and need-private? (not (document-unretained? existing)))
         (error (string-append (abbreviate-path canon) ": already open without private profile")))
       (when (and codec (not (document-decoded? existing)))
         (error (string-append (abbreviate-path canon) ": already open without a codec")))
       (when (and (not codec) (document-decoded? existing))
         (error (string-append (abbreviate-path canon) ": already open through a codec")))
       (when (and codec (document-codec existing) (not (eq? (document-codec existing) codec)))
         (error (string-append (abbreviate-path canon) ": already open with a different codec")))
       (when (and (not (eq? codec-arg 'auto)) (not (eq? (document-codec existing) codec)))
         (error (string-append (abbreviate-path canon) ": already open with a different codec")))
       (register-open-document! canon existing))
      (d
       (when (and need-private? (not (document-unretained? d)))
         (error (string-append (abbreviate-path canon) ": document is not private")))
       (when (and codec (not (document-decoded? d)))
         (error (string-append (abbreviate-path canon) ": cannot attach codec to a raw document")))
       (when (and (document-decoded? d) (not (doc-meta-of d)) (not codec))
         (error (string-append (abbreviate-path canon) ": decoded document is missing codec metadata")))
       (when (and (or codec need-private?) (not (doc-meta-of d)))
         (hash-table-set! %doc-meta (document-id d)
                          (%make-doc-meta codec
                                          (and codec (with-editor-codec-options (lambda () (codec-init-envelope codec canon))))
                                          need-private?)))
       (register-open-document! canon d))
      (codec
       (let-values (((doc env)
                     (if remote?
                         (with-editor-codec-options
                          (lambda ()
                            (let* ((gen (%remote-open-prepare canon))
                                   (raw (%node-file-read-pinned (location-endpoint canon) (location-path canon)))
                                   (res (codec-decode codec canon raw))
                                   (env (or (codec-result-envelope res)
                                            (and (not raw) (codec-init-envelope codec canon)))))
                              (values (%open-remote-file-decoded canon gen raw (codec-result-text res) (not need-private?)) env))))
                         (open-local-codec-document effective-alias canon codec need-private? journal-path))))
         (when (and need-private? (not (document-unretained? doc)))
           (error (string-append (abbreviate-path canon) ": already open with a journal")))
         (let ((m (doc-meta-of doc)))
           (cond ((not m)
                  (hash-table-set! %doc-meta (document-id doc) (%make-doc-meta codec env need-private?)))
                 ((not (eq? (%doc-meta-codec m) codec))
                  (error (string-append (abbreviate-path canon) ": already open with a different codec")))))
         (register-open-document! canon doc)))
      (need-private?
       (let ((doc (if remote?
                      (let* ((gen (%remote-open-prepare canon))
                             (raw (%node-file-read-pinned (location-endpoint canon) (location-path canon))))
                        (%open-remote-file-without-journal canon gen raw))
                      (%open-file-without-journal canon effective-alias))))
         (unless (document-unretained? doc)
           (error (string-append (abbreviate-path canon) ": already open with a journal")))
         (unless (doc-meta-of doc)
           (hash-table-set! %doc-meta (document-id doc) (%make-doc-meta #f #f #t)))
         (register-open-document! canon doc)))
      (else
       (let ((doc (if remote?
                      (%open-remote-file (location-endpoint canon) (location-path canon) canon)
                      (%open-file-pinned canon (and (string? journal-path) journal-path) effective-alias))))
         (register-open-document! canon doc))))))

(define (%attach-file-document path journal-path [alias #f])
  "Internal helper for `Runtime::attach` preserving journal and alias."
  (file-document path #:journal-path journal-path #:alias alias #:pinned #t))

(define (save-document! d #:overwrite [overwrite #f])
  "Write the document D to its file, refusing an external change.
With OVERWRITE, an external change is replaced. A codec-backed document
encodes a snapshot of D's text asynchronously and commits via a checked
save at that revision, preserving any edits made while encoding. A file
on another machine is written through its endpoint, only if it still has
the stored bytes D was read or last saved with. Then `document-saved` runs.
Refused, the error offers recovery choices (doc/operations.md,
\"Recovery\"): to overwrite the file, to revert the buffer to it, to
compare the two; each expects D as it is now, and is refused once D has
changed since."
  (when (and (document-codec d) (hash-table-ref/default %saving-docs (document-id d) #f))
    (error "a save of this document is under way"))
  (run-hook! (current-session) 'before-save d)
  (let* ((path (document-path d))
         (codec (document-codec d)))
    (when (and (document-decoded? d) (not codec))
      (error "a decoded document cannot be saved without its codec"))
    (when (and codec (hash-table-ref/default %saving-docs (document-id d) #f))
      (error "a save of this document is under way"))
    (guard (e ((changed-on-disk? e)
               (let ((cur-path (or (document-path d) path)))
                 (error-with-choices (save-choices d)
                                     (string-append (if cur-path (abbreviate-path cur-path) "document")
                                                    " changed on disk since it was read")))))
      (cond
        (codec
         (let ((id (document-id d)))
           (when (hash-table-ref/default %saving-docs id #f)
             (error "a save of this document is under way"))
           (hash-table-set! %saving-docs id #t)
           (dynamic-wind
             (lambda () #f)
             (lambda ()
               ;; Sample path, codec, revision, location-generation, text, and
               ;; envelope together without yielding so recipient/path lookup
               ;; cannot split across a concurrent or pre-save move.
               (let* ((cur-path (document-path d))
                      (cur-codec (document-codec d))
                      (rev (document-revision d))
                      (loc-gen (document-location-generation d))
                      (text (document-string d))
                      (env (document-codec-envelope d))
                      (encoded (with-editor-codec-options
                                (lambda () (codec-encode cur-codec cur-path text env)))))
                 (if (and cur-path (remote-path? cur-path))
                     (%document-save-remote-encoded! d (location-endpoint cur-path) (location-path cur-path) rev encoded overwrite loc-gen)
                     (%document-save-encoded! d rev encoded overwrite loc-gen))))
             (lambda ()
               (hash-table-delete! %saving-docs id)))))
        ((and path (remote-path? path))
         (%document-save-remote! d (location-endpoint path) (location-path path) overwrite))
        (overwrite (document-save-overwriting! d))
        (else (document-save! d))))
    (run-hook! (current-session) 'document-saved d)))

(define (changed-on-disk? e)
  "Return #t if E is the error of a save refused: the file changed since."
  (and (error-object? e)
       (let ((msg (error-object-message e)))
         (and (string-contains msg "changed on disk")
              (not (string-contains msg "target changed on disk"))
              #t))))

;; What a save of D refused offers: D as it is now, the revision those
;; that change it expect.
(define (save-choices d)
  (let ((at (dict "document" (dict "file" (document-path d) "revision" (document-revision d)))))
    (list (make-choice "Overwrite" 'overwrite-file at
                       #:doc "Write the buffer's text over the file's, losing what changed in the file.")
          (make-choice "Revert" 'revert-buffer at
                       #:doc "Read the file again, the buffer's unsaved edits replaced; undo takes them back.")
          (make-choice "Compare" 'diff-buffer-with-file at #:settles #f
                       #:doc "Show what differs between the buffer and the file."))))

(define-hook before-save
  "Before a document is written to its file: (session document).
The session is the current one then, or #f when saved headless.")

(define-hook document-saved
  "After a document is written to its file: (session document).
The session is the current one then, or #f when saved headless.")

(define (reload-codec-document! d codec revert-as)
  (let ((path (document-path d)))
    (unless path (error "the document has no file"))
    (when (and (not revert-as) (document-dirty? d))
      (error "the text has unsaved edits"))
    (with-editor-codec-options
     (lambda ()
       (let* ((remote? (remote-path? path))
              (state (if remote? (%remote-reload-prepare d) (%local-reload-prepare d)))
              (raw (if remote?
                       (%node-file-read-pinned (location-endpoint path) (location-path path))
                       (%read-file-bytes path (default-codec-max-bytes)))))
         (if (and (not (document-dirty? d)) (%document-stored-matches? d raw))
             #f
             (let* ((res (codec-decode codec path raw))
                    (env (or (codec-result-envelope res)
                             (and (not raw) (codec-init-envelope codec path))))
                    (changed (if remote?
                                 (%document-reload-remote-decoded! d state raw (codec-result-text res) revert-as)
                                 (%document-reload-decoded! d state raw (codec-result-text res) revert-as))))
               (let ((m (doc-meta-of d)))
                 (when m (%set-doc-meta-envelope! m env)))
               changed)))))))

(define (reload-document! d)
  "Read the file of the document D again if it changed since it was read.
It is refused while D has unsaved edits. Return #t if the text changed."
  (let ((path (document-path d))
        (codec (document-codec d)))
    (when (and (document-decoded? d) (not codec))
      (error "a decoded document cannot be reloaded without its codec"))
    (cond (codec (reload-codec-document! d codec #f))
          ((and path (remote-path? path))
           (%document-reload-remote! d (location-endpoint path) (location-path path) #f))
          (else (document-reload! d)))))

(define (revert-document! d)
  "Read the file of the document D again, its unsaved edits replaced.
The text becomes the file's as one edit by the current principal, which
undo takes back. Return #t if the text changed."
  (let ((path (document-path d))
        (codec (document-codec d)))
    (when (and (document-decoded? d) (not codec))
      (error "a decoded document cannot be reverted without its codec"))
    (cond (codec (reload-codec-document! d codec (current-principal)))
          ((and path (remote-path? path))
           (%document-reload-remote! d (location-endpoint path) (location-path path) (current-principal)))
          (else (document-revert! d (current-principal))))))

(define (file-text d)
  "Return the text of the file of the document D now, \"\" if it has none.
A file on another machine is read through its endpoint; a codec-backed
document's stored bytes are decoded through its codec."
  (let ((path (document-path d))
        (codec (document-codec d)))
    (when (and (document-decoded? d) (not codec))
      (error "a decoded document cannot be read without its codec"))
    (cond ((not path) (error "This document is in no file"))
          (codec
           (with-editor-codec-options
            (lambda ()
              (let ((raw (if (remote-path? path)
                             (%node-file-read-pinned (location-endpoint path) (location-path path))
                             (%read-file-bytes path (default-codec-max-bytes)))))
                (if raw (codec-result-text (codec-decode codec path raw)) "")))))
          ((remote-path? path)
           (let ((bytes (%node-file-read-pinned (location-endpoint path) (location-path path))))
             (if bytes (utf8->string bytes) "")))
          (else
           (let ((bytes (%read-file-bytes path #f)))
             (if bytes (utf8->string bytes) ""))))))

(define (directory-entries dir)
  "Return the entries of the directory DIR, sorted.
A directory's has a slash after its name. DIR may be a location on
another machine."
  (if (remote-path? dir)
      (node-directory-list (location-endpoint dir) (location-path dir))
      (directory-list dir)))

(define (path-exists? path)
  "Return #t if something is at PATH, here or on another machine."
  (if (remote-path? path)
      (and (node-file-stat (location-endpoint path) (location-path path)) #t)
      (file-exists? path)))

(define (open-file-documents)
  "Return the documents of the files open, each once."
  (fold (lambda (d acc) (if (any (lambda (o) (same-document? o d)) acc) acc (cons d acc)))
        '() (hash-table-values %documents)))

(define (same-document? a b) (= (document-id a) (document-id b)))

(define (forget-file-document! d)
  (for-each (lambda (k) (when (same-document? (hash-table-ref %documents k) d) (hash-table-delete! %documents k)))
            (hash-table-keys %documents)))

(define-hook document-moved
  "After a document's file moves with the document: (session document).
The session is the current one then.")

(define (file-document-moved! d)
  "Register the document D under the path it has now, its file moved.
Then `document-moved` runs."
  (forget-file-document! d)
  (hash-table-set! %documents (document-path d) d)
  (when (current-session) (run-hook! (current-session) 'document-moved d)))
