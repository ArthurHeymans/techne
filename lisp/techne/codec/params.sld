;;; Configuration parameters shared by `(techne codec)` and editor options.
;;; Kept separate so the editor can bind parameters without eagerly loading
;;; `(techne codec)` and its process/path dependencies at startup.

(define-library (techne codec params)
  (export default-gzip-program default-codec-max-bytes default-codec-timeout
          set-codec-option-hook!
          codec-registry-loaded? note-codec-registry-loaded!
          default-gpg-program default-gpg-homedir default-gpg-recipients
          default-gpg-id-roots default-gpg-trust-file-recipients
          default-gpg-armor default-gpg-batch default-gpg-timeout
          default-age-program default-age-identities default-age-recipients
          default-age-recipients-file default-age-recipient-roots
          default-age-armor default-age-timeout)
  (import (techne))
  (begin
    (define (require-root-authority! who)
      (unless (root-authority?)
        (error (string-append who ": may only be called with root authority")
               (scope-name (authority-scope)))))

    (define %codec-option-hook #f)

    (define (set-codec-option-hook! proc)
      "Register PROC resolving editor codec options for unbound parameters.
PROC is called as `(PROC opt-name fallback)`. Root authority only."
      (require-root-authority! "set-codec-option-hook!")
      (unless (or (not proc) (procedure? proc))
        (error "set-codec-option-hook!: expected procedure or #f" proc))
      (set! %codec-option-hook proc))

    (define (make-guarded-codec-param name opt-name init validate clone)
      (let ((p #f))
        (set! p
          (make-root-parameter init
            (lambda (v)
              (validate v)
              v)
            (lambda (v)
              (clone
               (if (and %codec-option-hook (not (parameter-bound? p)))
                   (%codec-option-hook opt-name v)
                   v)))))
        p))

    (define (validate-nonempty-string who v)
      (unless (and (string? v) (> (string-length v) 0))
        (error (string-append who ": expected non-empty string") v)))

    (define (validate-optional-nonempty-string who v)
      (unless (or (not v) (and (string? v) (> (string-length v) 0)))
        (error (string-append who ": expected #f or non-empty string") v)))

    (define (validate-string-list who v)
      (unless (and (list? v) (every (lambda (s) (and (string? s) (> (string-length s) 0))) v))
        (error (string-append who ": expected list of non-empty strings") v)))

    (define (validate-roots-or-bool who v)
      (unless (or (boolean? v)
                  (and (list? v) (every (lambda (s) (and (string? s) (> (string-length s) 0))) v)))
        (error (string-append who ": expected boolean or list of non-empty strings") v)))

    (define (validate-boolean who v)
      (unless (boolean? v)
        (error (string-append who ": expected boolean") v)))

    (define (validate-positive-int who v)
      (unless (and (integer? v) (exact? v) (> v 0))
        (error (string-append who ": expected positive integer") v)))

    (define (clone-optional-string v)
      (if (string? v) (string-copy v) v))

    (define (clone-string-list v)
      (map string-copy v))

    (define (clone-roots-or-bool v)
      (if (list? v) (map string-copy v) v))

    (define (identity-clone v) v)

    (define default-gzip-program
      (make-guarded-codec-param
        "default-gzip-program" 'gzip-program "gzip"
        (lambda (v) (validate-nonempty-string "default-gzip-program" v))
        string-copy)
      "Executable name or absolute path used by the built-in `gzip` codec.
Root authority only.")
    (define default-codec-max-bytes
      (make-guarded-codec-param
        "default-codec-max-bytes" 'codec-max-bytes 67108864
        (lambda (v) (validate-positive-int "default-codec-max-bytes" v))
        identity-clone)
      "Maximum decoded or encoded byte count accepted by built-in codecs.
Root authority only.")
    (define default-codec-timeout
      (make-guarded-codec-param
        "default-codec-timeout" 'codec-timeout 30000
        (lambda (v) (validate-positive-int "default-codec-timeout" v))
        identity-clone)
      "Default subprocess timeout in milliseconds for built-in codecs.
Root authority only.")
    (define %codec-registry-loaded? #f)
    (define (codec-registry-loaded?)
      "Return #t if `(techne codec)` has been loaded in this VM."
      %codec-registry-loaded?)
    (define (note-codec-registry-loaded!)
      "Mark `(techne codec)` as loaded so lazy bridges consult its registry."
      (set! %codec-registry-loaded? #t)
      #t)

    (define default-gpg-program
      (make-guarded-codec-param
        "default-gpg-program" 'gpg-program "gpg"
        (lambda (v) (validate-nonempty-string "default-gpg-program" v))
        string-copy)
      "Executable name or absolute path used by the built-in `gpg` codec.
Root authority only.")
    (define default-gpg-homedir
      (make-guarded-codec-param
        "default-gpg-homedir" 'gpg-homedir #f
        (lambda (v) (validate-optional-nonempty-string "default-gpg-homedir" v))
        clone-optional-string)
      "Optional explicit GNUPGHOME directory for the built-in `gpg` codec.
Root authority only.")
    (define default-gpg-recipients
      (make-guarded-codec-param
        "default-gpg-recipients" 'gpg-recipients '()
        (lambda (v) (validate-string-list "default-gpg-recipients" v))
        clone-string-list)
      "Trusted GPG recipients (fingerprints, key IDs, or emails) for `.gpg`.
Root authority only.")
    (define default-gpg-id-roots
      (make-guarded-codec-param
        "default-gpg-id-roots" 'gpg-id-roots '()
        (lambda (v) (validate-roots-or-bool "default-gpg-id-roots" v))
        clone-roots-or-bool)
      "Trusted local root directories (or `#t`) allowing `.gpg-id` discovery.
Opting in delegates recipient selection to whoever can write `.gpg-id`
files inside those roots; `#t` trusts every local directory. Root
authority only.")
    (define default-gpg-trust-file-recipients
      (make-guarded-codec-param
        "default-gpg-trust-file-recipients" 'gpg-trust-file-recipients #f
        (lambda (v) (validate-boolean "default-gpg-trust-file-recipients" v))
        identity-clone)
      "Allow reusing unauthenticated `ENC_TO` recipients on `.gpg` save.
When true, saving an existing visible-recipient `.gpg` file may reuse
the ciphertext's `ENC_TO` key IDs when no trusted recipient policy
(`default-gpg-recipients` or `.gpg-id`) is configured. Defaults to `#f`
so untrusted ciphertext metadata cannot choose recipients. Root
authority only.")
    (define default-gpg-armor
      (make-guarded-codec-param
        "default-gpg-armor" 'gpg-armor #f
        (lambda (v) (validate-boolean "default-gpg-armor" v))
        identity-clone)
      "When true, newly created `.gpg` files use ASCII armor (`--armor`).
Root authority only.")
    (define default-gpg-batch
      (make-guarded-codec-param
        "default-gpg-batch" 'gpg-batch #f
        (lambda (v) (validate-boolean "default-gpg-batch" v))
        identity-clone)
      "When true, pass `--batch --pinentry-mode error` to `gpg` on decrypt.
Disables interactive pinentry; otherwise a GUI pinentry configured in
`gpg-agent` is required because helpers run without a controlling tty.
Root authority only.")
    (define default-gpg-timeout
      (make-guarded-codec-param
        "default-gpg-timeout" 'gpg-timeout 120000
        (lambda (v) (validate-positive-int "default-gpg-timeout" v))
        identity-clone)
      "Subprocess timeout in milliseconds for the built-in `gpg` codec.
Root authority only.")

    (define default-age-program
      (make-guarded-codec-param
        "default-age-program" 'age-program "age"
        (lambda (v) (validate-nonempty-string "default-age-program" v))
        string-copy)
      "Executable name or absolute path used by the built-in `age` codec.
Root authority only.")
    (define default-age-identities
      (make-guarded-codec-param
        "default-age-identities" 'age-identities '()
        (lambda (v) (validate-string-list "default-age-identities" v))
        clone-string-list)
      "List of identity file paths for decrypting `.age` files.
Supports identity-based decryption only; passphrase-encrypted `.age`
files are not supported. Root authority only.")
    (define default-age-recipients
      (make-guarded-codec-param
        "default-age-recipients" 'age-recipients '()
        (lambda (v) (validate-string-list "default-age-recipients" v))
        clone-string-list)
      "List of age recipient public keys for encrypting `.age` files.
Note: `.age` ciphertexts do not carry recipient metadata, so saving
re-encrypts to the configured or discovered recipients. Root authority
only.")
    (define default-age-recipients-file
      (make-guarded-codec-param
        "default-age-recipients-file" 'age-recipients-file #f
        (lambda (v) (validate-optional-nonempty-string "default-age-recipients-file" v))
        clone-optional-string)
      "Optional path to a trusted age recipients file.
Root authority only.")
    (define default-age-recipient-roots
      (make-guarded-codec-param
        "default-age-recipient-roots" 'age-recipient-roots '()
        (lambda (v) (validate-roots-or-bool "default-age-recipient-roots" v))
        clone-roots-or-bool)
      "Trusted local roots (or `#t`) allowing `.age-recipients` discovery.
Opting in delegates recipient selection to whoever can write `.age-recipients`
files inside those roots; `#t` trusts every local directory. Root
authority only.")
    (define default-age-armor
      (make-guarded-codec-param
        "default-age-armor" 'age-armor #f
        (lambda (v) (validate-boolean "default-age-armor" v))
        identity-clone)
      "When true, newly created `.age` files use ASCII armor (`-a`).
Root authority only.")
    (define default-age-timeout
      (make-guarded-codec-param
        "default-age-timeout" 'age-timeout 60000
        (lambda (v) (validate-positive-int "default-age-timeout" v))
        identity-clone)
      "Subprocess timeout in milliseconds for the built-in `age` codec.
Root authority only.")))
