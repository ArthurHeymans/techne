;;; Global VC options and entry points, without loading the status runtime.
(import (techne editor))
(require "../files.scm")

(define-option vc-show-hints #t
  "Whether version control views show a line of the keys most used."
  #:type 'boolean)

(define-option vc-refresh-interval #f
  "How often local status buffers refresh themselves, in milliseconds.
With #f refresh explicitly. Remote status always refreshes on demand."
  #:type '(or (one-of #f) natural))

(define-option vc-refresh-on-save #t
  "Whether saving a file refreshes its repository's status where shown.
A Jujutsu working copy is recorded first, as Magit and Majutsu do."
  #:type 'boolean)

(define-option vc-forge-repository #f
  "Override the GitHub repository for this buffer, as owner/repo."
  #:type '(or (one-of #f) string))

(register-command-package! 'vc-runtime
  (string-append (directory-of (current-module)) "vc/runtime.scm")
  '((vc-status "Show the state of the repository of this buffer's file.")
    (vc-file-stage "Stage this buffer's file in its Git index.")
    (vc-file-unstage "Unstage this buffer's file from its Git index.")
    (vc-file-diff "Inspect this file's staged or unstaged Git patch, or its jj patch.")
    (vc-file-history "Show the history of this buffer's file.")
    (vc-file-annotate "Show who last changed each line of this buffer's file.")
    (vc-git-conflicts "Inspect and resolve Git conflict blocks.")
    (vc-find-change "Choose a changed file of this buffer's repository.")
    (vc-remote-outcome "Inspect an unresolved remote write without replaying it.")
    (vc-forge "List open pull requests from the GitHub origin.")
    (vc-process-log "Show the version control process log.")))

(for-each (lambda (binding) (define-key! emacs-map (car binding) (cadr binding)))
  '(("C-x g" vc-status) ("C-x v d" vc-status)
    ("C-x v s" vc-file-stage) ("C-x v u" vc-file-unstage)
    ("C-x v =" vc-file-diff) ("C-x v l" vc-file-history)
    ("C-x v g" vc-file-annotate) ("C-x v m" vc-git-conflicts)
    ("C-x v f" vc-find-change) ("C-x v !" vc-remote-outcome)))
(name-prefix! emacs-map "C-x v" "vc")
