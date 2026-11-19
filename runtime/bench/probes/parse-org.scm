;; Real Org parser, not the synthetic steady/orgparse kernel. Exclude file I/O.
(require "../../../lisp/org/org.scm")
(define lines (file->lines (cadr (command-line))))
(do ((i 0 (+ i 1))) ((= i 6))
  (let* ((start (current-jiffy)) (doc (parse-org lines)) (end (current-jiffy)))
    (write (list (- end start) (length (all-headings doc)) (org-file-todo-states doc))) (newline)))
