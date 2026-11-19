;;; The editor's built-in packages, loaded after the application and its
;;; interface for extensions, (techne editor): each a package of its own,
;;; as any extension is, from its file in packages/. Which load, and in
;;; what order, is decided here.

(require "files.scm")
(require "dispatch.scm")
(require "terminal-options.scm")

(define builtin-packages (quote (compile dired ibuffer vc markdown-preview mail terminal))
  "The editor's own packages, loaded at start in this order.")

;; This file, which packages/ is beside.
(define %here (current-module))

(for-each (lambda (name)
            (let ((path (string-append (directory-of %here) "packages/" (symbol->string name) ".scm")))
              (case name
                ((terminal)
                 (register-command-package! name path
                   (quote ((terminal "Open a new terminal running the user’s shell.")
                           (terminal-command "Read a shell command and run it on a new terminal.")
                           (terminal-serial "Open a local serial console using socat at the configured baud rate.")))))
                ((markdown-preview)
                 (register-command-package! name path (quote ((markdown-preview "Show this markdown buffer rendered, beside it.")))))
                (else (load-package name path)))))
          builtin-packages)
