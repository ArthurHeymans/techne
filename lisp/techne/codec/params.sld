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
    (define %codec-option-hook #f)

    (define (set-codec-option-hook! proc)
      "Register PROC resolving editor codec options for unbound parameters.
PROC is called as `(PROC opt-name fallback)`."
      (unless (or (not proc) (procedure? proc))
        (error "set-codec-option-hook!: expected procedure or #f" proc))
      (set! %codec-option-hook proc))

    (define %unbound (list 'unbound))

    (define (make-codec-param opt-name init validate)
      (let ((key (%fresh-key)))
        (%record %parameter-type
                 (lambda ()
                   (let ((v (%task-local-ref key %unbound)))
                     (if (eq? v %unbound)
                         (if %codec-option-hook (%codec-option-hook opt-name init) init)
                         v)))
                 key
                 (lambda (v)
                   (validate v)
                   v))))

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

    (define default-gzip-program
      (make-codec-param
        'gzip-program "gzip"
        (lambda (v) (validate-nonempty-string "default-gzip-program" v)))
      "Executable name or absolute path used by the built-in `gzip` codec.")
    (define default-codec-max-bytes
      (make-codec-param
        'codec-max-bytes 67108864
        (lambda (v) (validate-positive-int "default-codec-max-bytes" v)))
      "Maximum decoded or encoded byte count accepted by built-in codecs.")
    (define default-codec-timeout
      (make-codec-param
        'codec-timeout 30000
        (lambda (v) (validate-positive-int "default-codec-timeout" v)))
      "Default subprocess timeout in milliseconds for built-in codecs.")
    (define %codec-registry-loaded? #f)
    (define (codec-registry-loaded?)
      "Return #t if `(techne codec)` has been loaded in this VM."
      %codec-registry-loaded?)
    (define (note-codec-registry-loaded!)
      "Mark `(techne codec)` as loaded so lazy bridges consult its registry."
      (set! %codec-registry-loaded? #t)
      #t)

    (define default-gpg-program
      (make-codec-param
        'gpg-program "gpg"
        (lambda (v) (validate-nonempty-string "default-gpg-program" v)))
      "Executable name or absolute path used by the built-in `gpg` codec.")
    (define default-gpg-homedir
      (make-codec-param
        'gpg-homedir #f
        (lambda (v) (validate-optional-nonempty-string "default-gpg-homedir" v)))
      "Optional explicit GNUPGHOME directory for the built-in `gpg` codec.")
    (define default-gpg-recipients
      (make-codec-param
        'gpg-recipients '()
        (lambda (v) (validate-string-list "default-gpg-recipients" v)))
      "Trusted GPG recipients (fingerprints, key IDs, or emails) for `.gpg`.")
    (define default-gpg-id-roots
      (make-codec-param
        'gpg-id-roots '()
        (lambda (v) (validate-roots-or-bool "default-gpg-id-roots" v)))
      "Trusted local root directories (or `#t`) allowing `.gpg-id` discovery.
Opting in delegates recipient selection to whoever can write `.gpg-id`
files inside those roots; `#t` trusts every local directory.")
    (define default-gpg-trust-file-recipients
      (make-codec-param
        'gpg-trust-file-recipients #f
        (lambda (v) (validate-boolean "default-gpg-trust-file-recipients" v)))
      "Allow reusing unauthenticated `ENC_TO` recipients on `.gpg` save.
When true, saving an existing visible-recipient `.gpg` file may reuse
the ciphertext's `ENC_TO` key IDs when no trusted recipient policy
(`default-gpg-recipients` or `.gpg-id`) is configured. Defaults to `#f`
so untrusted ciphertext metadata cannot choose recipients.")
    (define default-gpg-armor
      (make-codec-param
        'gpg-armor #f
        (lambda (v) (validate-boolean "default-gpg-armor" v)))
      "When true, newly created `.gpg` files use ASCII armor (`--armor`).")
    (define default-gpg-batch
      (make-codec-param
        'gpg-batch #f
        (lambda (v) (validate-boolean "default-gpg-batch" v)))
      "When true, pass `--batch --pinentry-mode error` to `gpg` on decrypt.
Disables interactive pinentry; otherwise a GUI pinentry configured in
`gpg-agent` is required because helpers run without a controlling tty.")
    (define default-gpg-timeout
      (make-codec-param
        'gpg-timeout 120000
        (lambda (v) (validate-positive-int "default-gpg-timeout" v)))
      "Subprocess timeout in milliseconds for the built-in `gpg` codec.")

    (define default-age-program
      (make-codec-param
        'age-program "age"
        (lambda (v) (validate-nonempty-string "default-age-program" v)))
      "Executable name or absolute path used by the built-in `age` codec.")
    (define default-age-identities
      (make-codec-param
        'age-identities '()
        (lambda (v) (validate-string-list "default-age-identities" v)))
      "List of identity file paths for decrypting `.age` files.
Supports identity-based decryption only; passphrase-encrypted `.age`
files are not supported.")
    (define default-age-recipients
      (make-codec-param
        'age-recipients '()
        (lambda (v) (validate-string-list "default-age-recipients" v)))
      "List of age recipient public keys for encrypting `.age` files.
Note: `.age` ciphertexts do not carry recipient metadata, so saving
re-encrypts to the configured or discovered recipients.")
    (define default-age-recipients-file
      (make-codec-param
        'age-recipients-file #f
        (lambda (v) (validate-optional-nonempty-string "default-age-recipients-file" v)))
      "Optional path to a trusted age recipients file.")
    (define default-age-recipient-roots
      (make-codec-param
        'age-recipient-roots '()
        (lambda (v) (validate-roots-or-bool "default-age-recipient-roots" v)))
      "Trusted local roots (or `#t`) allowing `.age-recipients` discovery.
Opting in delegates recipient selection to whoever can write `.age-recipients`
files inside those roots; `#t` trusts every local directory.")
    (define default-age-armor
      (make-codec-param
        'age-armor #f
        (lambda (v) (validate-boolean "default-age-armor" v)))
      "When true, newly created `.age` files use ASCII armor (`-a`).")
    (define default-age-timeout
      (make-codec-param
        'age-timeout 60000
        (lambda (v) (validate-positive-int "default-age-timeout" v)))
      "Subprocess timeout in milliseconds for the built-in `age` codec.")))
