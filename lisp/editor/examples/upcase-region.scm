;;; Canonical example 1 (EDITOR.md, section 11): a command on the region.
;;; In Emacs Lisp:
;;;
;;;   (defun shout-region (beg end)
;;;     "Upcase the text of the region."
;;;     (interactive "r")
;;;     (upcase-region beg end))
;;;   (global-set-key (kbd "C-c u") #'shout-region)

(import (techne editor))

(define-command (shout-region s n)
  "Upcase the text of the region."
  (replace-region! s string-upcase))

(define-key! emacs-map "C-c u" 'shout-region)
