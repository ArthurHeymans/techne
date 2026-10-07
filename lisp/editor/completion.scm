;;; Completion in the buffer, as Arthur's Corfu (EDITOR.md, section 10):
;;; what completes the text before the caret, in a popup under it. The
;;; buffer's mode says what completes there (`#:complete`, modes.scm):
;;; Scheme buffers and itl the names their module sees, from the language
;;; (`module-completions`), as the REPL and nREPL complete them.
;;;
;;; As Arthur has Corfu: the popup opens by itself after two characters
;;; of an identifier and a pause (corfu-auto), or at once with C-M-i;
;;; candidates are matched as the minibuffer matches them (Orderless),
;;; the shortest first, and nothing is selected until TAB or C-n (the
;;; prompt is preselected). The selected candidate is put in the text in
;;; place of what was typed (corfu-preview-current); typing on keeps it
;;; and goes on completing. RET takes a selected candidate, else does
;;; what it does without the popup; C-g puts back what was typed. The
;;; popup's keys are a keymap over the buffer's while it is open.

(require "session.scm")
(require "modes.scm")
(require "commands.scm")
(require "minibuffer.scm")

(provide completion-at-point identifier-span scheme-completion editor-completion completion-rows)

;; Rows shown at once, as Arthur's corfu-count.
(define completion-rows 16)
;; The popup opens by itself after this many characters of an identifier
;; and this many milliseconds, as corfu-auto-prefix and corfu-auto-delay.
(define completion-auto-prefix 2)
(define completion-auto-delay 240)

;;; Identifiers, as the reader ends them.

(define (identifier-char? c)
  (not (or (char-whitespace? c) (memv c '(#\( #\) #\[ #\] #\{ #\} #\' #\" #\` #\, #\;)))))

(define (identifier-at? d p) (identifier-char? (string-ref (document-substring d p (next-grapheme d p)) 0)))

;; The identifier around POS in D: (start end), empty when there is none.
(define (identifier-span d pos)
  (let ((from (let loop ((p pos)) (if (and (> p 0) (identifier-at? d (prev-grapheme d p))) (loop (prev-grapheme d p)) p)))
        (to (let loop ((p pos)) (if (and (< p (document-length d)) (identifier-at? d p)) (loop (next-grapheme d p)) p))))
    (list from to)))

;; What completes the identifier before POS in D with the names MODULE
;; sees: (start . candidates), the shortest first, each annotated with its
;; kind.
(define (scheme-completion d pos module)
  (cons (car (identifier-span d pos))
        (map (lambda (e) (candidate (car e) #:annotation (symbol->string (cadr e))))
             (sort (module-completions module)
                   (lambda (a b)
                     (let ((la (string-length (car a))) (lb (string-length (car b))))
                       (if (= la lb) (string<? (car a) (car b)) (< la lb))))))))

;;; The popup

(define-record-type popup
  (make-popup view start candidates typed matches selected offset)
  popup?
  (view popup-view)
  (start popup-start)
  (candidates popup-candidates)
  ;; What was typed from START, and the candidates matching it, a vector.
  (typed popup-typed set-popup-typed!)
  (matches popup-matches set-popup-matches!)
  ;; The selected candidate, -1 for none (the prompt); the first shown.
  (selected popup-selected set-popup-selected!)
  (offset popup-offset set-popup-offset!))

(define (popup s) (sget s 'completion))

(define (matching candidates typed)
  (let ((parts (pattern-parts typed)))
    (list->vector (filter (lambda (c) (matches? c parts)) candidates))))

(define (close-completion! s)
  (sset! s 'completion #f)
  (sset! s 'overlay-map #f))

;; Open the popup for the text before point; MANUAL says why there is none.
(define (open-completion! s manual)
  (let* ((b (current-buffer s)) (v (pane-view s)) (d (view-document v)) (p (point s))
         (found (and b (buffer-completion b p))))
    (cond ((not found) (when manual (message! s "No completion here")))
          (else
           (let* ((typed (document-substring d (car found) p))
                  (matches (matching (cdr found) typed)))
             (if (= (vector-length matches) 0)
                 (when manual (message! s "No match"))
                 (begin
                   (sset! s 'completion (make-popup v (car found) (cdr found) typed matches -1 0))
                   (sset! s 'overlay-map completion-map))))))))

;; Put TEXT in place of what the popup's identifier has now, the caret
;; after it, joining the undo unit of the typing.
(define (put-completion! s text)
  (let ((pop (popup s)))
    (view-edit! (popup-view pop) (list (list (popup-start pop) (point s) text)) "extend")))

;; Select candidate I, cycling through them and the prompt (-1), and show
;; it in the text.
(define (select-completion! s i)
  (let* ((pop (popup s)) (n (vector-length (popup-matches pop))) (i (- (modulo (+ i 1) (+ n 1)) 1)))
    (set-popup-selected! pop i)
    (put-completion! s (if (< i 0) (popup-typed pop) (candidate-text (vector-ref (popup-matches pop) i))))))

(define-command (completion-at-point s n)
  "Complete the text before point: show what completes it under it."
  (open-completion! s #t))

(define-command (completion-next s n)
  "Select the next candidate, showing it in the text."
  (select-completion! s (+ (popup-selected (popup s)) n)))

(define-command (completion-previous s n)
  "Select the previous candidate, showing it in the text."
  (select-completion! s (- (popup-selected (popup s)) n)))

(define-command (completion-accept s n)
  "Take the selected candidate; with none selected, close the popup and do
what RET does without it."
  (let ((pop (popup s)))
    (close-completion! s)
    (when (< (popup-selected pop) 0)
      ((profile-key (sget s 'profile)) s "RET"))))

(define completion-map (make-keymap))

(for-each (lambda (b) (define-key! completion-map (car b) (cadr b)))
          '(("TAB" completion-next) ("C-n" completion-next) ("<down>" completion-next)
            ("S-TAB" completion-previous) ("<backtab>" completion-previous) ("C-p" completion-previous) ("<up>" completion-previous)
            ("RET" completion-accept)))

;;; After each key: the open popup follows the typing, or closes; else,
;;; after typing an identifier's first characters and a pause, it opens.

(define completion-commands '(completion-at-point completion-next completion-previous completion-accept))

;; Whether KEY typed or deleted text, so the popup may open.
(define (typed? s key)
  (and (or (printable-key? key) (string=? key "DEL"))
       (memq (sget s 'this-command) '(self-insert delete-backward-char modal-action))
       (not (memq (sget s 'mode) '(normal visual search ex)))
       (not (minibuffer-open? s))))

(define (follow! s key)
  (let* ((pop (popup s)) (v (pane-view s)) (d (view-document v)))
    (cond ((or (not (view=? v (popup-view pop))) (minibuffer-open? s) (memq (sget s 'mode) '(normal visual search ex)))
           (close-completion! s))
          ((string=? key "C-g")
           (put-completion! s (popup-typed pop))
           (close-completion! s))
          ((memq (sget s 'this-command) completion-commands) #f)
          (else
           (let ((p (point s)))
             (if (or (< p (popup-start pop)) (not (= (car (identifier-span d p)) (popup-start pop))))
                 (close-completion! s)
                 (let* ((typed (document-substring d (popup-start pop) p))
                        (matches (matching (popup-candidates pop) typed)))
                   (if (= (vector-length matches) 0)
                       (close-completion! s)
                       (begin (set-popup-typed! pop typed)
                              (set-popup-matches! pop matches)
                              (set-popup-selected! pop -1)
                              (set-popup-offset! pop 0))))))))))

(define (completion-after-key s key)
  (cond ((popup s) (follow! s key))
        ((typed? s key)
         (let* ((v (pane-view s)) (d (view-document v)) (p (point s))
                (word (- p (car (identifier-span d p))))
                (token (+ 1 (or (sget s 'completion-token) 0)))
                (revision (document-revision d)))
           (sset! s 'completion-token token)
           (when (>= word completion-auto-prefix)
             (spawn (lambda ()
                      (sleep completion-auto-delay)
                      (when (and (eqv? (sget s 'completion-token) token) (not (popup s))
                                 (view=? (pane-view s) v) (= (document-revision d) revision) (= (point s) p))
                        (open-completion! s #f)))))))
        (else (sset! s 'completion-token (+ 1 (or (sget s 'completion-token) 0))))))

(add-hook! 'after-key 'completion (lambda (s key) (completion-after-key s key)))

;;; What the frontend shows: (view at rows selected), AT the position the
;;; popup is aligned with, rows around the selected one, or #f.

(define (editor-completion s)
  (let ((pop (popup s)))
    (and pop
         (let* ((all (popup-matches pop)) (n (vector-length all)) (i (popup-selected pop))
                (offset (cond ((< i 0) 0)
                              ((< i (popup-offset pop)) i)
                              ((>= i (+ (popup-offset pop) completion-rows)) (+ (- i completion-rows) 1))
                              (else (popup-offset pop))))
                (parts (pattern-parts (popup-typed pop))))
           (set-popup-offset! pop offset)
           (list (popup-view pop)
                 (popup-start pop)
                 (map (lambda (k) (let ((c (vector-ref all k))) (candidate-row c (match-spans c parts))))
                      (iota (min completion-rows (- n offset)) offset))
                 (and (>= i 0) (- i offset)))))))
