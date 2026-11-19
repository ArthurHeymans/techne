;;; Recipes and manifests are read as data. Import this installer only in a
;;; configured editor, not in every VM that starts up or lists commands.
(define-library (techne packages)
  (export parse-package-source package-source? package-source-kind package-source-url package-source-ref
          git-ref? git-ref-kind git-ref-value register-package-recipe! package-install! package-restore!)
  (import (techne) (techne editor) (techne crypto) (techne path) (techne schema))
  (include "packages.scm"))
