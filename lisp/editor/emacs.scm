;;; The Emacs chord profile: point and mark are the primary range and its
;;; anchor; C-SPC starts a region that motions extend. Prefix keys wait for
;;; the rest of their sequence; C-g cancels whatever is pending.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")

(provide emacs-profile emacs-map prefix-count)

(define emacs-map (make-keymap)
  "The keys of the Emacs profile, after the modes'.")

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
            ("C-/" undo) ("C-_" undo) ("C-?" redo) ("M-_" redo)
            ("C-DEL" backward-kill-word) ("C-<delete>" kill-word) ("C-x C-x" exchange-point-and-mark)
            ("C-<home>" beginning-of-buffer) ("C-<end>" end-of-buffer) ("s-v" yank) ("s-c" copy-region-as-kill) ("C-M-_" redo)
            ("C-s" isearch-forward) ("C-r" isearch-backward)
            ;; Read before the keymap (`emacs-key`), bound for help to see.
            ("C-g" keyboard-quit)
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
;;; first match from where the search started, letters matching whatever
;;; their case unless it has an upper-case one; the match is highlighted,
;;; and the others shown (host.scm draws them); C-s/C-r go to the next one,
;;; or with nothing typed search for the last string again; RET keeps the
;;; position, C-g goes back. Any other key ends the search and then does what
;;; it normally does.

(define (isearch-start s forward)
  (sset! s 'isearch (list forward "" (ranges s))))

(define-command (isearch-forward s n) "Search forward as you type." (isearch-start s #t))
(define-command (isearch-backward s n) "Search backward as you type." (isearch-start s #f))

(define (isearch-goto s needle from forward)
  (let ((m (search-text (doc s) from needle forward (fold-case-for needle))))
    (sset! s 'isearch-match m)
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
;;; Prefix arguments, as Emacs reads them: C-u is 4 (C-u C-u 16), digits
;;; after it make a number, - negates; C-0..C-9 and M-0..M-9 start a
;;; number, C-- and M-- a negative one. The command that follows gets the
;;; number as its count, and the argument itself as `current-prefix`.

(define (digit-of key)
  (let ((c (string-ref key (- (string-length key) 1))))
    (and (char-numeric? c) (- (char->integer c) 48))))

;; Whether KEY continues a prefix argument; if so it is read.
(define (prefix-key! s key)
  (let* ((arg (sget s 'prefix-arg))
         (bare-digit (and arg (= (string-length key) 1) (digit-of key)))
         (mod-digit (and (member (substring key 0 (min 2 (string-length key))) '("C-" "M-"))
                         (= (string-length key) 3) (digit-of key)))
         (set (lambda (v) (sset! s 'prefix-arg v) (sset! s 'prefix-keys (append (or (sget s 'prefix-keys) '()) (list key))) #t)))
    (cond ((string=? key "C-u") (set (if (pair? arg) (list (* 4 (car arg))) '(4))))
          ((or bare-digit mod-digit)
           => (lambda (d)
                (set (cond ((integer? arg) (if (< arg 0) (- (* 10 arg) d) (+ (* 10 arg) d)))
                           ((eq? arg '-) (- d))
                           (else d)))))
          ((or (member key '("C--" "M--")) (and (string=? key "-") (pair? arg))) (set '-))
          (else #f))))

(define (prefix-count arg)
  "Return the count the prefix argument ARG gives a command."
  (cond ((not arg) 1) ((pair? arg) (car arg)) ((eq? arg '-) -1) (else arg)))

(define (emacs-run s name)
  (let ((rev (document-revision (doc s))) (arg (sget s 'prefix-arg)))
    (sset! s 'prefix-arg #f)
    (sset! s 'prefix-keys '())
    (when (and (sget s 'extend) (eq? name 'self-insert))
      (sset! s 'extend #f)
      (move! s (lambda (p) p)))
    (sset! s 'current-prefix arg)
    (run-command s name (prefix-count arg))
    (sset! s 'current-prefix #f)
    (when (and (sget s 'extend) (not (= rev (document-revision (doc s)))))
      (sset! s 'extend #f)
      (move! s (lambda (p) p)))))

(define (emacs-key s key)
  (cond ((sget s 'isearch) (isearch-key s key))
        ((string=? key "C-g")
         (sset! s 'pending '())
         (sset! s 'prefix-arg #f)
         (sset! s 'prefix-keys '())
         (run-command s 'keyboard-quit 1))
        ((and (null? (sget s 'pending)) (prefix-key! s key)) #t)
        (else
         (let* ((pending (sget s 'pending))
                (keys (append pending (list key)))
                (binding (key-binding (active-keymaps s 'chord) keys)))
           (cond ((keymap? binding) (sset! s 'pending keys))
                 ((symbol? binding) (sset! s 'pending '()) (emacs-run s binding))
                 ((and (null? pending) (printable-key? key))
                  (sset! s 'key key)
                  (emacs-run s 'self-insert))
                 (else
                  (sset! s 'pending '())
                  (sset! s 'prefix-arg #f)
                  (sset! s 'prefix-keys '())
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
                emacs-click
                (list (cons 'chord emacs-map)))
  "The Emacs profile: chords, point and mark, prefix arguments.")
