;;; Pure-data lockfile (`techne.lock`), content-addressed `tree-sha256`
;;; store, transitive Git/path dependency resolver, one-version
;;; diamond-conflict rejection, and strictly offline verification.
;;;
;;; Nothing here needs the editor. Pure-data lockfile parsing, writing,
;;; integrity verification, and offline VM wiring (`use-locked-package!`)
;;; run in any VM; resolution (`resolve-project`, `lock-project!`) uses
;;; `git` through `call-with-process` when fetching Git dependencies.

(define-library (techne lock)
  (export lockfile-name
          make-lockfile lockfile? lockfile-techne lockfile-packages
          make-locked-package locked-package?
          locked-package-name locked-package-version locked-package-source
          locked-package-tree-sha256 locked-package-depends
          parse-lockfile read-lockfile format-lockfile write-lockfile
          packages-data-dir package-store-dir
          materialize-package-tree!
          resolve-project resolve-package-source lock-project!
          verify-lockfile check-lockfile use-locked-package!
          verify-world-lockfile)
  (import (techne) (techne crypto) (techne package) (techne path))
  (include "lock.scm"))
