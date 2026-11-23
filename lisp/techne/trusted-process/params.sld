;;; Lightweight hook library for `(techne trusted-process)` baseline cleanup.
;;; Kept separate so the editor can wire its `direnv-program` option at startup
;;; without eagerly loading `(techne trusted-process)` or `(techne execution-site)`.

(define-library (techne trusted-process params)
  (export trusted-direnv-program-hook set-trusted-direnv-program-hook!)
  (import (techne))
  (begin
    (define %direnv-program-hook (lambda () "direnv"))

    (define (trusted-direnv-program-hook)
      "Return the direnv executable name or path for baseline cleanup."
      (%direnv-program-hook))

    (define (set-trusted-direnv-program-hook! proc)
      "Register PROC returning the direnv executable for baseline cleanup."
      (unless (procedure? proc)
        (error "set-trusted-direnv-program-hook!: expected procedure" proc))
      (set! %direnv-program-hook proc))))
