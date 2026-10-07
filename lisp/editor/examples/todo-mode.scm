;;; Canonical example 2 (EDITOR.md, section 11): a minor mode with a keymap
;;; and a highlighting layer. In Emacs Lisp:
;;;
;;;   (defun next-todo () "Move to the next TODO." (interactive) (search-forward "TODO"))
;;;   (defvar todo-mode-map
;;;     (let ((m (make-sparse-keymap))) (define-key m (kbd "C-c t") #'next-todo) m))
;;;   (define-minor-mode todo-mode
;;;     "Highlight TODOs; C-c t moves to the next one."
;;;     :keymap todo-mode-map
;;;     (if todo-mode
;;;         (font-lock-add-keywords nil '(("TODO" 0 'warning t)))
;;;       (font-lock-remove-keywords nil '(("TODO" 0 'warning t))))
;;;     (font-lock-flush))

(import (techne editor))

(define-command (next-todo s n)
  "Move to the next TODO."
  (goto-next! s "TODO"))

(define (todos doc from to)
  (map (lambda (m) (list (car m) (cadr m) 'warning)) (search-all doc "TODO" from to)))

(define-minor-mode todo-mode
  "Highlight TODOs; C-c t moves to the next one."
  #:keys '(("C-c t" next-todo))
  #:layer todos)
