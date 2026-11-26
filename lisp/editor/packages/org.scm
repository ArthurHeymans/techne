;;; Early Org options, lazy `org-mode` entry, and global keys; the
;;; full Org runtime (`org/runtime.scm` and `lisp/org/*.scm`) loads on demand.
(require "../keymaps.scm")
(require "../emacs.scm")
(require "../modal.scm")
(require "../modes.scm")
(require "../dispatch.scm")
(require "../files.scm")

;; ----- Lazy runtime activation -----

(register-command-package! 'org-runtime
  (string-append (directory-of (current-module)) "org/runtime.scm")
  '((org-agenda "Open `*Org Agenda*` for today.")
    (org-capture "Capture an Org entry, daily note, or meeting note.")
    (org-node-find "Find and visit an Org node.")
    (org-node-insert-link "Insert an `id:` link to an Org node.")
    (org-node-backlinks "Show Org backlinks to the current node.")
    (org-node-visit-random "Visit a random Org node.")
    (org-node-grep "Search full text across indexed Org files.")
    (org-search "Search Org headings.")
    (org-open-meeting-link "Open a meeting link at point.")))

(define (ensure-org-runtime!)
  (unless (find-package 'org-runtime)
    (activate-command-package! 'org-runtime)))

(define-mode org-mode
  "Outline-based notes management and organizer."
  #:parent 'text-mode
  #:files '(".org")
  #:input (lambda (s key)
            (ensure-org-runtime!)
            #f))

(add-hook! 'file-buffer-added 'org-lazy-load
  (lambda (s b)
    (when (eq? (buffer-mode b) 'org-mode)
      (ensure-org-runtime!))))

;; Global Org entry points (Doom leader on SPC in modal, C-c in emacs).
(let ((m (make-keymap)))
  (for-each (lambda (e) (define-key! m (car e) (cdr e)))
            '(("f" . org-node-find) ("i" . org-node-insert-link) ("b" . org-node-backlinks)
              ("r" . org-node-visit-random) ("g" . org-node-grep) ("s" . org-search)))
  (define-key! modal-map "SPC o A" 'org-agenda)
  (define-key! modal-map "SPC X" 'org-capture)
  (define-key! modal-map "SPC N" m)
  (define-key! emacs-map "C-c o A" 'org-agenda)
  (define-key! emacs-map "C-c X" 'org-capture)
  (define-key! emacs-map "C-c N" m))
(name-prefix! modal-map "SPC N" "org-node")
