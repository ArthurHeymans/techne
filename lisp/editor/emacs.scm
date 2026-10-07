;;; The Emacs chord profile: point and mark are the primary range and its
;;; anchor; C-SPC starts a region that motions extend. Prefix keys wait for
;;; the rest of their sequence; C-g cancels whatever is pending.

(require "session.scm")
(require "commands.scm")

(provide emacs-profile emacs-map)

(define emacs-map (make-keymap))

(for-each (lambda (b) (define-key! emacs-map (car b) (cadr b)))
          '(("C-f" forward-char) ("C-b" backward-char)
            ("M-f" forward-word) ("M-b" backward-word)
            ("C-n" next-line) ("C-p" previous-line)
            ("C-a" beginning-of-line) ("C-e" end-of-line)
            ("M-<" beginning-of-buffer) ("M->" end-of-buffer)
            ("C-d" delete-char) ("DEL" delete-backward-char)
            ("M-d" kill-word) ("M-DEL" backward-kill-word) ("C-k" kill-line)
            ("C-w" kill-region) ("M-w" copy-region-as-kill) ("C-y" yank)
            ("C-SPC" set-mark) ("RET" newline)
            ("C-/" undo) ("C-?" redo) ("C-M-_" redo)
            ("C-s" isearch-forward) ("C-r" isearch-backward)
            ("<left>" backward-char) ("<right>" forward-char) ("<up>" previous-line) ("<down>" next-line)
            ("<home>" beginning-of-line) ("<end>" end-of-line) ("<delete>" delete-char)))

;; Self-inserted characters join one undo unit, up to 20 of them, as Emacs
;; amalgamates them.
(define-command (self-insert s n)
  "Insert the character of the key pressed."
  (let* ((run (if (eq? (sget s 'last-command) 'self-insert) (+ 1 (or (sget s 'insert-run) 0)) 1))
         (group (if (and (> run 1) (<= run 20)) 'extend 'new)))
    (sset! s 'insert-run (if (> run 20) 1 run))
    (insert-text! s (make-string n (key-char (sget s 'key))) group)))

;;; Incremental search: typing extends the search string and moves to the
;;; first match from where the search started; C-s/C-r go to the next one,
;;; or with nothing typed search for the last string again; RET keeps the
;;; position, C-g goes back. Any other key ends the search and then does what
;;; it normally does.

(define (isearch-start s forward)
  (sset! s 'isearch (list forward "" (ranges s))))

(define-command (isearch-forward s n) "Search forward as you type." (isearch-start s #t))
(define-command (isearch-backward s n) "Search backward as you type." (isearch-start s #f))

(define (isearch-goto s needle from forward)
  (let ((m (search-text (doc s) from needle forward)))
    (if m
        (let ((p (if forward (cadr m) (car m))))
          (view-set-ranges! (session-view s) (list (list p p)) 0)
          (message! s #f))
        (message! s (string-append "Failing search: " needle)))))

(define (isearch-key s key)
  (let* ((state (sget s 'isearch))
         (forward (car state))
         (needle (cadr state))
         (origin (caddr state))
         (origin-point (cadr (car origin)))
         (restart (lambda (needle)
                    (sset! s 'isearch (list forward needle origin))
                    (isearch-goto s needle origin-point forward))))
    (cond ((printable-key? key) (restart (string-append needle (string (key-char key)))))
          ((string=? key "DEL")
           (restart (if (string=? needle "") needle (substring needle 0 (- (string-length needle) 1)))))
          ;; With nothing typed yet, search for the last string again.
          ((and (member key '("C-s" "C-r")) (string=? needle "") (sget s 'isearch-last))
           (restart (sget s 'isearch-last)))
          ((member key '("C-s" "C-r"))
           (let ((fwd (string=? key "C-s")))
             (sset! s 'isearch (list fwd needle origin))
             ;; From a match's end going back (or its start going on) the
             ;; first stop is the other end of the same match, as in Emacs.
             (isearch-goto s needle (point s) fwd)))
          ((string=? key "RET")
           (unless (string=? needle "") (sset! s 'isearch-last needle))
           (sset! s 'isearch #f))
          ((string=? key "C-g")
           (sset! s 'isearch #f)
           (view-set-ranges! (session-view s) origin 0)
           (message! s "Quit"))
          (else (sset! s 'isearch #f) (emacs-key s key)))))

;; As in Emacs's transient mark mode, a change to the text deactivates the
;; region (typing inserts at point rather than replacing it).
(define (emacs-run s name)
  (let ((rev (document-revision (doc s))))
    (when (and (sget s 'extend) (eq? name 'self-insert))
      (sset! s 'extend #f)
      (move! s (lambda (p) p)))
    (run-command s name 1)
    (when (and (sget s 'extend) (not (= rev (document-revision (doc s)))))
      (sset! s 'extend #f)
      (move! s (lambda (p) p)))))

(define (emacs-key s key)
  (cond ((sget s 'isearch) (isearch-key s key))
        ((string=? key "C-g") (sset! s 'pending '()) (run-command s 'keyboard-quit 1))
        (else
         (let* ((pending (sget s 'pending))
                (keys (append pending (list key)))
                ;; Minor modes' bindings come first.
                (binding (or (mode-binding s keys) (lookup-key emacs-map keys))))
           (cond ((keymap? binding) (sset! s 'pending keys))
                 ((symbol? binding) (sset! s 'pending '()) (emacs-run s binding))
                 ((and (null? pending) (printable-key? key))
                  (sset! s 'key key)
                  (emacs-run s 'self-insert))
                 (else
                  (sset! s 'pending '())
                  (message! s (string-append (string-join keys " ") " is undefined"))))))))

;; A click puts point there; with extend, the region runs to it.
(define (emacs-click s pos extend)
  (let ((v (session-view s)))
    (if extend
        (let ((anchor (car (list-ref (view-ranges v) (view-primary v)))))
          (sset! s 'extend #t)
          (view-set-ranges! v (list (list anchor pos)) 0))
        (begin (sset! s 'extend #f)
               (view-set-ranges! v (list (list pos pos)) 0)))))

(define emacs-profile
  (make-profile 'emacs
                (lambda (s) (sset! s 'pending '()))
                emacs-key
                emacs-click))
