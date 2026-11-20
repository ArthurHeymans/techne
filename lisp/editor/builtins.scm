;;; The editor's built-in packages, loaded after the application and its
;;; interface for extensions, (techne editor): each a package of its own,
;;; as any extension is, from its file in packages/. Which load, and in
;;; what order, is decided here. Mail is a package directory, packages/mail
;;; beside Techne's Lisp, advertised from its manifest as an installed
;;; package is (doc/packages.md).
;;;
;;; `(techne package)`, which checks a manifest and follows a package's
;;; dependencies, is not loaded for that at start: the manifests Techne
;;; ships are checked by its tests (lisp/tests/shipped-packages-tests.scm), which
;;; also hold them to depending on nothing, and reading one costs a
;;; fraction of loading the library (editor startup has a budget,
;;; runtime/bench/icount.scm).

(require "files.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "terminal-options.scm")
(require "secrets-options.scm")

(define builtin-packages (quote (compile dired ibuffer vc markdown-preview mail forge terminal))
  "The editor's own packages, loaded at start in this order.")

;; This file, which packages/ is beside.
(define %here (current-module))

;; Advertise the package directory NAME among those Techne ships: its
;; libraries are found under its lib/, its options declared and its
;; commands offered, nothing of it loaded.
(define (advertise-shipped-package! name)
  (let* ((found (or (%package-manifest (string-append (directory-of %here) "../../packages/" (symbol->string name)))
                    (error "No package shipped as" name)))
         (root (car found))
         (editor (cdr (assq (quote editor) (cddr (read (open-input-string (cadr found)))))))
         (options (assq (quote options) (cdr editor))))
    (%provide-libraries! (symbol->string name) (string-append root "/lib"))
    ;; It depends on nothing: its files import its own libraries only.
    (%package-imports! root (symbol->string name) (list (symbol->string name)))
    (advertise-package! name (string-append root "/" (car editor))
                        (cdr (assq (quote commands) (cdr editor))) (if options (cdr options) (quote ())))))

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
                ((mail forge) (advertise-shipped-package! name))
                (else (load-package name path)))))
          builtin-packages)
