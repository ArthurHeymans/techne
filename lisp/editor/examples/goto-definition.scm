;;; Canonical example 4 (EDITOR.md, section 11): a minibuffer completion
;;; source with preview. In Emacs Lisp, with Consult:
;;;
;;;   (defun goto-definition ()
;;;     "Go to a top-level definition, previewing each while choosing."
;;;     (interactive)
;;;     (let ((defs (save-excursion
;;;                   (goto-char (point-min))
;;;                   (cl-loop while (re-search-forward "^(define" nil t)
;;;                            collect (cons (buffer-substring (pos-bol) (pos-eol)) (pos-bol))))))
;;;       (goto-char (consult--read defs :prompt "Definition: "
;;;                                 :lookup #'consult--lookup-cdr
;;;                                 :state (consult--jump-state)))))
;;;   (global-set-key (kbd "C-c d") #'goto-definition)
;;;
;;; The candidates are lines, whose targets are locations: going there is
;;; the default action, so it is both the preview and what RET does, and
;;; every other action on locations works on them (C-; in the minibuffer).

(import (techne editor))

(define (definitions d)
  (filter-map (lambda (m) (and (= (car m) (line-start d (car m))) (line-candidate d (car m))))
              (search-all d "(define" 0 (document-length d))))

(define-command (goto-definition s n)
  "Go to a top-level definition, previewing each while choosing."
  (completing-read s "Definition: " (definitions (doc s)) #:preview take-target))

(define-key! emacs-map "C-c d" 'goto-definition)
