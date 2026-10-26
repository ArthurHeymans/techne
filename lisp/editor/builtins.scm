;;; The editor's built-in packages, loaded after the application and its
;;; interface for extensions, (techne editor): each a package of its own,
;;; as any extension is, from its file in packages/. Which load, and in
;;; what order, is decided here.

(require "files.scm")

(define builtin-packages '(compile dired ibuffer vc)
  "The editor's own packages, loaded at start in this order.")

;; This file, which packages/ is beside.
(define %here (current-module))

(for-each (lambda (name)
            (load-package name (string-append (directory-of %here) "packages/" (symbol->string name) ".scm")))
          builtin-packages)
