;;; Global entry points remain available before opening a compilation.
(import (techne editor))
(require "../files.scm")

(define-option compile-command #f
  "Default shell command offered by `compile` for a project or buffer.
When `#f`, `compile` chooses `cargo build` if `Cargo.toml` exists, else
`make -k `. Project `.techne.scm` values require explicit user trust."
  #:type '(or (one-of #f) (string #:min 1))
  #:directory-safe #f)

(register-command-package! (quote compile-runtime)
  (string-append (directory-of (current-module)) "../compile/runtime.scm")
  (quote ((compile "Run a command in the project root; show its output as it comes.")
          (recompile "Run the compilation again.")
          (kill-compilation "Stop the compilation running.")
          (next-error "Go to the next place *compilation* names, in another pane.")
          (previous-error "Go to the previous place *compilation* names, in another pane."))))

(for-each (lambda (b) (define-key! emacs-map (car b) (cadr b)))
          '(("M-g n" next-error) ("M-g M-n" next-error) ("M-g p" previous-error) ("M-g M-p" previous-error)
            ("C-x `" next-error) ("C-c c c" compile) ("C-c c C" recompile)))
(for-each (lambda (b) (define-key! modal-map (car b) (cadr b)))
          '(("] e" next-error) ("[ e" previous-error) ("SPC c c" compile) ("SPC c C" recompile)))
(name-prefix! emacs-map "M-g" "goto")
(for-each (lambda (n) (name-prefix! modal-map (car n) (cadr n))) '(("]" "next") ("[" "previous")))
