;;; Configuration parameters shared by `(techne codec)` and editor options.
;;; Kept separate so the editor can bind parameters without eagerly loading
;;; `(techne codec)` and its process/path dependencies at startup.

(define-library (techne codec params)
  (export default-gzip-program default-codec-max-bytes default-codec-timeout
          set-codec-option-hook!
          codec-registry-loaded? note-codec-registry-loaded!)
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
      #t)))
