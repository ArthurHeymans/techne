;;; Secret source configuration available before packages load.
(require "modes.scm")

(define-option secret-sources
  '((authinfo))
  "Ordered list of user-trusted secret sources for `(techne secrets)`.
Each entry is a backend symbol or `(BACKEND option ...)` such as
`(authinfo)`, `(sops #:file \"~/secrets.yaml\")`, `(pass)` or
`(env #:var \"TOKEN\" #:host \"api.github.com\")`. Fixed-value sources
(`env`, `sops` with `#:key`/`#:extract`/`#:format 'binary`, `pass` with
`#:entry`) must include a `#:when-host`, `#:when-service` or
`#:when-account` guard so credentials never cross services. User-scope
only; untrusted project settings must never override this option."
  #:type 'any
  #:user-only #t)
