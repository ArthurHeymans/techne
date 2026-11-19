;;; Early buffer-list options and global keys; listing commands load on demand.
(import (techne editor))

(define-option ibuffer-sort 'recency
  "How the buffer list is sorted within each group.
By `recency` (the most recently shown first), `name`, `size` (the
largest first) or `mode`."
  #:type '(one-of recency name size mode))

(define-option ibuffer-group-by-project #t
  "Whether the buffer list groups file buffers by project."
  #:type 'boolean)

(require "../files.scm")
(register-command-package! (quote ibuffer-runtime)
  (string-append (directory-of (current-module)) "../ibuffer/runtime.scm")
  (quote ((ibuffer "Show the buffers, grouped by project, to act on."))))

(define-key! emacs-map "C-x C-b" 'ibuffer)
(define-key! modal-map "SPC b i" 'ibuffer)
