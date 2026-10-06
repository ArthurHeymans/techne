;;; The editor application: a session over a view the host made, commands
;;; for files, and what the host asks of Lisp to present the session (the
;;; status line and the cursor shape). The runtime (techne-editor) calls the
;;; procedures provided here.

(require "session.scm")
(require "commands.scm")
(require "emacs.scm")
(require "modal.scm")

(provide start-session editor-press editor-click editor-message! status-line cursor-shape session-quit?)

(define (start-session view profile-name)
  (make-session-for-view view (if (equal? profile-name "modal") modal-profile emacs-profile)))

(define (editor-press s key) (press s key))
(define (editor-click s pos extend) ((profile-click (sget s 'profile)) s pos extend))
(define (editor-message! s text) (message! s text))
(define (session-quit? s) (sget s 'quit))

(define-command (save-buffer s n)
  "Write the document to its file."
  (let ((d (doc s)))
    (document-save! d)
    (message! s (string-append "Wrote " (document-path d)))))

(define-command (quit s n)
  "End the session. Unsaved edits stay in the journal for the next start."
  (sset! s 'quit #t))

(define-key! emacs-map "C-x C-s" 'save-buffer)
(define-key! emacs-map "C-x C-c" 'quit)

(define (state-name s)
  (case (sget s 'mode)
    ((insert) "INSERT")
    ((visual) "VISUAL")
    ((normal) "NORMAL")
    (else #f)))

;; File, modified mark, line, the modal state, keys waiting for the rest of
;; their sequence, and the message or open prompt.
(define (status-line s)
  (let* ((d (doc s))
         (prompt (and (eq? (profile-name (sget s 'profile)) 'modal) (modal-prompt s)))
         (pending (sget s 'pending))
         (parts (list (or (document-path d) "*scratch*")
                      (if (document-dirty? d) "[+]" #f)
                      (string-append "L" (number->string (line-number d (point s))))
                      (state-name s)
                      (and (pair? pending) (string-append (string-join pending " ") "-"))
                      (and (sget s 'isearch) (string-append "I-search: " (cadr (sget s 'isearch))))
                      (or prompt (sget s 'message)))))
    (string-join (filter (lambda (x) x) parts) "  ")))

(define (cursor-shape s)
  (if (memq (sget s 'mode) '(normal visual)) 'block 'bar))
