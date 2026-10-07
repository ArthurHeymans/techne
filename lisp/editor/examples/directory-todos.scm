;;; Canonical example 3 (EDITOR.md, section 11): a structured view with
;;; targets and actions. In Emacs Lisp:
;;;
;;;   (define-derived-mode todos-mode tabulated-list-mode "TODOs"
;;;     (setq tabulated-list-format [("Where" 20 t) ("TODO" 0 nil)]))
;;;   (defun directory-todos ()
;;;     "TODO comments in the files of this buffer's directory."
;;;     (interactive)
;;;     (let ((rows (cl-loop for f in (directory-files default-directory t "^[^.]")
;;;                          unless (file-directory-p f)
;;;                          append (with-temp-buffer
;;;                                   (insert-file-contents f)
;;;                                   (cl-loop while (search-forward "TODO" nil t)
;;;                                            collect (let ((n (line-number-at-pos)))
;;;                                                      (list (cons f n)
;;;                                                            (vector (format "%s:%d" (file-name-nondirectory f) n)
;;;                                                                    (string-trim (thing-at-point 'line t))))))))))
;;;       (pop-to-buffer "*directory-todos*")
;;;       (todos-mode)
;;;       (setq tabulated-list-entries rows)
;;;       (tabulated-list-print)))
;;;   ;; and RET, to visit (car (tabulated-list-get-id)), bound in todos-mode-map.
;;;
;;; Each row's target is a location: RET goes there, and C-; offers what
;;; can be done with locations, with no code here.

(import (techne editor))

(define (todos path)
  (let loop ((lines (guard (e (#t '())) (file->lines path))) (n 1) (acc '()))
    (cond ((null? lines) (reverse acc))
          ((string-contains (car lines) "TODO")
           (loop (cdr lines) (+ n 1)
                 (cons (row (string-append (file-name path) ":" (number->string n)) (string-trim (car lines))
                            #:target (target 'location (file-location path n)))
                       acc)))
          (else (loop (cdr lines) (+ n 1) acc)))))

(define-view (directory-todos s)
  "TODO comments in the files of this buffer's directory."
  (let ((dir (default-directory s)))
    (append-map (lambda (name) (if (string-suffix? "/" name) '() (todos (string-append dir name))))
                (directory-list dir))))
