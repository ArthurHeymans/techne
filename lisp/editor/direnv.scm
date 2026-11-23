;;; Editor integration for local `direnv` execution-site environments:
;;; options, 0-I/O mode-line status, watched-file save invalidation, and
;;; lazy interactive command registration (`lisp/editor/direnv/runtime.scm`).

(require "dispatch.scm")
(require "modes.scm")
(require "files.scm")
(require "buffers.scm")
(require "remote.scm")

(define-option direnv-enabled #t
  "Whether local project processes load environments via `direnv`.
Applies to `project` execution sites (compile, shell, terminal, VC,
LSP); never affects `user-trusted` tools such as `gpg` or `sops`.
User-scope only; untrusted project settings must never override this."
  #:type 'boolean
  #:user-only #t)

(define-option direnv-program "direnv"
  "Executable name or path used to evaluate local `.envrc` files.
Must be an absolute path or a bare executable name; always resolved
against the clean host `PATH`, never a project's `PATH`. User-scope only."
  #:type 'string
  #:user-only #t)

(define-option direnv-data-dir #f
  "Optional absolute directory holding direnv's `allow/` and `deny/` data.
When #f, uses `$XDG_DATA_HOME/direnv` or `~/.local/share/direnv` from the
clean host baseline environment. User-scope only."
  #:type '(or (one-of #f) string)
  #:user-only #t)

(define (buffer-dir b)
  (let ((path (and b (document-path (buffer-document b)))))
    (and path (directory-of path))))

;; Mode line reads only pure in-memory cached state: zero I/O, zero subprocesses.
(register-mode-line-segment! 'direnv
  (lambda (b)
    (and (execution-site-loaded?)
         (option #f 'direnv-enabled)
         (let* ((dir (buffer-dir b))
                (snap (and dir (execution-site-cached-snapshot dir))))
           (and snap
                (case (env-snapshot-status snap)
                  ((active) "direnv")
                  ((blocked) "direnv:blocked")
                  ((denied) "direnv:denied")
                  ((error) "direnv:err")
                  (else #f)))))))

(define (direnv-config-basename? path)
  (let ((base (file-name path)))
    (or (string=? base ".envrc") (string=? base ".env"))))

(add-hook! 'document-saved 'direnv
           (lambda (s d)
             (let ((path (document-path d)))
               (when (and path
                          (not (remote-path? path))
                          (or (execution-site-watched-path? path)
                              (direnv-config-basename? path)))
                 (let ((dir (directory-of path)))
                   (execution-site-invalidate! dir)
                   (when (option #f 'direnv-enabled)
                     (spawn (lambda ()
                              (guard (e ((task-cancelled-condition? e) (raise e))
                                        (#t #f))
                                (execution-site-env-snapshot (execution-site-for dir #:policy 'project)
                                                             #:refresh? #t))))))))))

(register-command-package! 'direnv-runtime
  (string-append (directory-of (current-module)) "direnv/runtime.scm")
  '((direnv-status "Show the direnv execution-site status for the current directory.")
    (direnv-allow "Approve the `.envrc` for the current directory after confirmation.")
    (direnv-deny "Deny the `.envrc` for the current directory and clear its cache.")
    (direnv-reload "Reload the cached direnv environment for the current directory.")))
