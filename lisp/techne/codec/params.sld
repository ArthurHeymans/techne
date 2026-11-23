;;; Configuration parameters shared by `(techne codec)` and editor options.
;;; Kept separate so the editor can bind parameters without eagerly loading
;;; `(techne codec)` and its process/path dependencies at startup.

(define-library (techne codec params)
  (export default-gzip-program default-codec-max-bytes default-codec-timeout
          set-codec-option-hook!
          codec-registry-loaded? note-codec-registry-loaded!)
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

    (define (validate-positive-int who v)
      (unless (and (integer? v) (exact? v) (> v 0))
        (error (string-append who ": expected positive integer") v)))

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
        (lambda (v) v))
      "Maximum decoded or encoded byte count accepted by built-in codecs.
Root authority only.")
    (define default-codec-timeout
      (make-guarded-codec-param
        "default-codec-timeout" 'codec-timeout 30000
        (lambda (v) (validate-positive-int "default-codec-timeout" v))
        (lambda (v) v))
      "Default subprocess timeout in milliseconds for built-in codecs.
Root authority only.")
    (define %codec-registry-loaded? #f)
    (define (codec-registry-loaded?)
      "Return #t if `(techne codec)` has been loaded in this VM."
      %codec-registry-loaded?)
    (define (note-codec-registry-loaded!)
      "Mark `(techne codec)` as loaded so lazy bridges consult its registry."
      (set! %codec-registry-loaded? #t)
      #t)))
