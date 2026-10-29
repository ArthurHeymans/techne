;;; The Emacs chord profile: point and mark are the primary range and its
;;; anchor; C-SPC starts a region that motions extend. Prefix keys wait for
;;; the rest of their sequence; C-g cancels whatever is pending.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "indent.scm")
(require "minibuffer.scm")

(provide emacs-profile emacs-map isearch-map prefix-count)

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
            ("C-SPC" set-mark) ("RET" newline-and-indent) ("M-m" back-to-indentation) ("C-M-\\" indent-region)
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

;;; Incremental search, as Emacs's isearch: typing extends the search string
;;; and moves to the first match from where the search started, letters
;;; matching whatever their case unless it has an upper-case one; the match
;;; is highlighted, and the others shown (host.scm draws them). While it
;;; goes on `isearch-map` is looked up before every other keymap (Emacs's
;;; overriding-terminal-local-map): C-s/C-r go to the next match, or with
;;; nothing typed search for the last string again; C-y, C-w and M-s C-e add
;;; the last kill, the word or the line after the match; M-p and M-n go
;;; through the strings searched for before; M-e edits the string in the
;;; minibuffer; RET keeps the position, C-g goes back. Any other key ends
;;; the search and then does what it normally does.
;;;
;;; The search is (forward needle origin primary): the direction, the
;;; string, and the selection it started from with its primary range.

(define isearch-map (make-keymap)
  "The keys of an incremental search.
They are looked up before every other keymap while it goes on; other
keys end the search.")

(for-each (lambda (b) (define-key! isearch-map (car b) (cadr b)))
          '(("C-s" isearch-repeat-forward) ("C-r" isearch-repeat-backward)
            ("DEL" isearch-delete-char) ("RET" isearch-exit) ("C-g" isearch-abort)
            ("C-y" isearch-yank-kill) ("s-v" isearch-yank-kill) ("M-y" isearch-yank-pop)
            ("C-w" isearch-yank-word-or-char) ("M-s C-e" isearch-yank-line)
            ("M-p" isearch-ring-retreat) ("M-n" isearch-ring-advance)
            ("M-e" isearch-edit-string)))

(define (isearch-state s) (or (sget s 'isearch) (error "Not searching")))
(define (isearch-forward? s) (car (isearch-state s)))
(define (isearch-needle s) (cadr (isearch-state s)))

(define (isearch-start s forward)
  (sset! s 'isearch (list forward "" (ranges s) (view-primary (session-view s))))
  (sset! s 'isearch-match #f)
  (sset! s 'isearch-ring-index 0)
  (sset! s 'overriding-map (cons 'isearch isearch-map)))

;; End the search where it is, remembering its string.
(define (isearch-done! s)
  (let ((needle (isearch-needle s)))
    (unless (string=? needle "") (add-history! 'isearch needle)))
  (sset! s 'isearch #f)
  (sset! s 'overriding-map #f)
  (sset! s 'pending '()))

(define-command (isearch-forward s n) "Search forward as you type." (isearch-start s #t))
(define-command (isearch-backward s n) "Search backward as you type." (isearch-start s #f))

(define (isearch-goto s needle from forward)
  (let ((m (and (not (string=? needle "")) (search-text (doc s) from needle forward (fold-case-for needle)))))
    (sset! s 'isearch-match m)
    (cond
     ;; Nothing to search for: back where it started.
     ((string=? needle "")
      (view-set-ranges! (session-view s) (list (list from from)) 0)
      (message! s #f))
     (m
      (let ((p (if forward (cadr m) (car m))))
        (view-set-ranges! (session-view s) (list (list p p)) 0)
        (message! s #f)))
     (else (message! s (string-append "Failing search: " needle))))))

;; Search for NEEDLE instead, from where the search started.
(define (isearch-restart! s needle)
  (let* ((state (isearch-state s)) (forward (car state)) (origin (caddr state)) (primary (cadddr state)))
    (sset! s 'isearch (list forward needle origin primary))
    (isearch-goto s needle (cadr (list-ref origin primary)) forward)))

;; Search for NEEDLE, grown from the string searched for, where the match
;; is: from its start going forward, or going back from where NEEDLE
;; would end there; from where the search started without a match. The
;; match is extended rather than another found, as in Emacs.
(define (isearch-search-on! s needle)
  (let ((m (sget s 'isearch-match)) (state (isearch-state s)))
    (if m
        (let ((forward (car state)))
          (sset! s 'isearch (cons forward (cons needle (cddr state))))
          (isearch-goto s needle
                        (if forward
                            (car m)
                            (min (document-length (doc s)) (+ (car m) (bytevector-length (string->utf8 needle)))))
                        forward))
        (isearch-restart! s needle))))

(define (isearch-append! s text) (isearch-search-on! s (string-append (isearch-needle s) text)))

;; Search on in direction FORWARD from point.
(define (isearch-repeat! s forward)
  (let ((state (isearch-state s)) (last (let ((h (minibuffer-history 'isearch))) (and (pair? h) (car h)))))
    (sset! s 'isearch (cons forward (cdr state)))
    (cond ((not (string=? (cadr state) ""))
           ;; From a match's end going back (or its start going on) the
           ;; first stop is the other end of the same match, as in Emacs.
           (isearch-goto s (cadr state) (point s) forward))
          ;; With nothing typed yet, search for the last string again.
          (last (isearch-restart! s last)))))

(define-command (isearch-repeat-forward s n)
  "Go to the next match, or search again for the last string."
  (isearch-repeat! s #t))
(define-command (isearch-repeat-backward s n)
  "Go to the previous match, or search again for the last string."
  (isearch-repeat! s #f))

(define-command (isearch-printing-char s n)
  "Add the character of the key pressed to the string being searched for."
  (isearch-append! s (string (key-char (sget s 'key)))))

(define-command (isearch-delete-char s n)
  "Remove the last character of the string being searched for."
  (let ((needle (isearch-needle s)))
    (isearch-restart! s (if (string=? needle "") needle (substring needle 0 (- (string-length needle) 1))))))

(define-command (isearch-exit s n)
  "End the search where it is."
  (isearch-done! s))

(define-command (isearch-abort s n)
  "End the search and go back to where it started."
  (let ((state (isearch-state s)))
    (sset! s 'isearch #f)
    (sset! s 'overriding-map #f)
    (view-set-ranges! (session-view s) (caddr state) (cadddr state))
    (message! s "Quit")))

;; Where text added from the buffer starts: the end of the match, in
;; either direction, else point.
(define (isearch-from s)
  (let ((m (sget s 'isearch-match)))
    (if m (cadr m) (point s))))

;; Add the last kill; remember it, (index length), for M-y to replace.
(define (isearch-yank! s)
  (let ((text (car (yank-text s))))
    (isearch-append! s text)
    (sset! s 'isearch-yank (list 0 (string-length text)))))

(define-command (isearch-yank-kill s n)
  "Add the last kill to the string being searched for."
  (isearch-yank! s))

(define-command (isearch-yank-pop s n)
  "Replace the kill just added to the search string with the one before.
Not right after a kill was added, add the last one."
  (let ((last (and (memq (sget s 'last-command) '(isearch-yank-kill isearch-yank-pop)) (sget s 'isearch-yank))))
    (if last
        (let* ((i (+ (car last) 1))
               (ring (kill-ring s))
               (i (if (< i (length ring)) i 0))
               (kept (substring (isearch-needle s) 0 (- (string-length (isearch-needle s)) (cadr last))))
               (text (car (list-ref ring i))))
          (isearch-restart! s (string-append kept text))
          (sset! s 'isearch-yank (list i (string-length text))))
        (isearch-yank! s))))

(define-command (isearch-yank-word-or-char s n)
  "Add the word after the match to the string being searched for."
  (let* ((from (isearch-from s)) (to ((motion-move emacs-word-forward) s from 1)))
    (isearch-append! s (document-substring (doc s) from to))))

(define-command (isearch-yank-line s n)
  "Add the rest of the line after the match to the search string.
At the end of a line, add the line break and the next line."
  (let* ((d (doc s))
         (from (isearch-from s))
         (end (line-end d from))
         (to (if (and (= end from) (< from (document-length d))) (line-end d (+ from 1)) end)))
    (isearch-append! s (document-substring d from to))))

;; Put the Nth string searched for before in the search string, the
;; newest first; 0 is none.
(define (isearch-ring-move! s n)
  (let* ((ring (minibuffer-history 'isearch)) (i (+ (or (sget s 'isearch-ring-index) 0) n)))
    (cond ((> i (length ring)) (message! s "Beginning of history; no preceding item"))
          ((< i 0) (message! s "End of history; no next item"))
          (else (sset! s 'isearch-ring-index i)
                (isearch-restart! s (if (= i 0) "" (list-ref ring (- i 1))))))))

(define-command (isearch-ring-retreat s n)
  "Search for the string searched for before the one searched for now."
  (isearch-ring-move! s 1))
(define-command (isearch-ring-advance s n)
  "Search for the string searched for after the one searched for now."
  (isearch-ring-move! s -1))

(define-command (isearch-edit-string s n)
  "Edit the string being searched for in the minibuffer.
\\[minibuffer-accept] searches for it from where the search started;
\\[minibuffer-abort] goes back to the search as it was."
  (completing-read s (if (isearch-forward? s) "I-search: " "I-search backward: ") '()
                   #:initial (isearch-needle s) #:require-match #f #:select-input (lambda (input) #t)
                   ;; Its history is gone through but not added to: the
                   ;; search remembers its string when it ends.
                   #:history 'isearch #:remember (lambda (entry c) #f)
                   #:accept (lambda (s c) (when (sget s 'isearch) (isearch-restart! s (candidate-text c))))))

(define (isearch-key s key)
  (let* ((keys (append (sget s 'pending) (list key)))
         (binding (lookup-key isearch-map keys)))
    (cond ((keymap? binding) (sset! s 'pending keys))
          ((symbol? binding) (sset! s 'pending '()) (run-command s binding 1))
          ((and (null? (cdr keys)) (printable-key? key))
           (sset! s 'key key)
           (run-command s 'isearch-printing-char 1))
          ;; Another key ends the search and does what it does.
          (else (sset! s 'pending '())
                (isearch-done! s)
                (for-each (lambda (k) (emacs-key s k)) keys)))))

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
  (when (sget s 'isearch) (isearch-done! s))
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
