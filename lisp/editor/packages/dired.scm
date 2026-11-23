;;; Early Dired options, global keys, and directory target actions; the
;;; listing, wdired, and dirvish runtime loads on demand.
(import (techne editor))
(require "../dispatch.scm")
(require "../files.scm")
(require "../targets.scm")

(define-option dired-details #t
  "Whether a directory's listing shows each entry's mode, size and date.
\\[dired-toggle-details] turns them on and off in one listing."
  #:type 'boolean)

(define-option dired-hide-dotfiles #f
  "Whether a directory's listing leaves out the dotfiles, named .something.
\\[dired-toggle-dotfiles] turns them on and off in one listing."
  #:type 'boolean)

(define-option dired-sort 'name
  "How a directory's listing is sorted, directories first.
By `name`, by `time` (the newest first) or by `size` (the largest
first); \\[dired-cycle-sort] goes to the next in one listing."
  #:type '(one-of name time size))

(define-option dired-preview-bytes 65536
  "The most bytes of a file the preview of dirvish shows."
  #:type 'natural)

(define %dired-runtime-path
  (string-append (directory-of (current-module)) "dired/runtime.scm"))

(register-command-package! 'dired-runtime
  %dired-runtime-path
  '((dired "Show the listing of a directory.")
    (dired-jump "Show the listing of this buffer's directory, at its file.")
    (dirvish "Show this buffer's directory as dirvish does, in three panes.")))

(define (dired-activate-action! s kind name path)
  (unless (find-package 'dired-runtime)
    (activate-command-package! 'dired-runtime))
  (let ((a (find (lambda (x) (eq? (action-name x) name)) (actions-for kind))))
    (run-action! s a (target kind path))))

(define-action directory (visit-directory s path)
  "List the directory."
  (dired-activate-action! s 'directory 'visit-directory path))
(define-action directory (visit-directory-other-pane s path)
  "List the directory in a new pane below."
  (dired-activate-action! s 'directory 'visit-directory-other-pane path))
(define-action directory (copy-directory-name s path)
  "Save the directory's name as a kill."
  (kill-save! s path #f #f))
(define-action file (list-file-directory s path)
  "List the file's directory, at the file."
  (dired-activate-action! s 'file 'list-file-directory path))

(define-key! emacs-map "C-x d" 'dired)
(define-key! emacs-map "C-x C-j" 'dired-jump)
(define-key! emacs-map "C-c o -" 'dired-jump)
(define-key! emacs-map "C-c o v" 'dirvish)
(name-prefix! emacs-map "C-c o" "open")
(define-key! modal-map "SPC o -" 'dired-jump)
(define-key! modal-map "SPC o v" 'dirvish)
(define-key! modal-map "SPC f d" 'dired)
(name-prefix! modal-map "SPC o" "open")
