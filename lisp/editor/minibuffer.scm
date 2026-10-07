;;; The minibuffer (EDITOR.md, section 10): a prompt, an input document
;;; edited with the usual commands, and the candidates matching the input,
;;; one selected. The reference is Vertico with Orderless: the input is
;;; split at spaces into parts that must all occur in a candidate, in any
;;; order; the candidates keep their source's order.
;;;
;;; `completing-read` opens it and returns at once: what is chosen is handed
;;; to the accept procedure later, when RET is pressed. A preview procedure
;;; sees each candidate the selection moves to; C-g puts back the panes the
;;; preview changed.
;;;
;;; While it is open the minibuffer takes the keys of either profile: its
;;; keymap first, printable keys insert into the input.
;;;
;;; Candidates with targets can be acted on (C-;): the actions on
;;; the target's kind are offered in the minibuffer in turn, as Embark
;;; offers them. The same works on the target at point in a buffer.

(require "session.scm")
(require "commands.scm")
(require "targets.scm")

(provide completing-read candidate candidate? candidate-text candidate-annotation candidate-target
         minibuffer-map minibuffer-open? minibuffer-input minibuffer-candidates minibuffer-selected
         editor-minibuffer close-minibuffer! abort-minibuffer! with-pane act-on! act-at-point act-default-at-point
         take-target)

;;; Candidates: text to match and show, a suffix shown after it (a key),
;;; an annotation in a column of its own, as Marginalia aligns them, and a
;;; target, what it stands for.

(define-record-type candidate
  (%candidate text suffix annotation target folded)
  candidate?
  (text candidate-text)
  ;; Shown right after the text, not matched: a command's key.
  (suffix candidate-suffix)
  (annotation candidate-annotation)
  (target candidate-target)
  ;; The text in lower case, once matching needed it.
  (folded %candidate-folded set-candidate-folded!))

(define (candidate text #:suffix [suffix #f] #:annotation [annotation ""] #:target [target #f])
  (%candidate text suffix annotation target #f))

(define (candidate-folded c)
  (or (%candidate-folded c)
      (let ((f (string-downcase (candidate-text c))))
        (set-candidate-folded! c f)
        f)))

(define (as-candidate c) (if (candidate? c) c (candidate c)))

;;; The open minibuffer.

(define-record-type minibuffer
  (make-minibuffer prompt view source pattern accept preview require-match restore)
  minibuffer?
  (prompt mb-prompt)
  ;; The input: a view of a document of its own.
  (view mb-view)
  ;; A list of candidates, or a procedure (input) -> list.
  (source mb-source)
  ;; (input) -> the part of it candidates are matched against.
  (pattern mb-pattern)
  (accept mb-accept)
  (preview mb-preview)
  (require-match mb-require-match)
  ;; What the panes were when it opened, for C-g after previews.
  (restore mb-restore)
  ;; The candidates matching the input at `revision`, a vector; the
  ;; selected one; the first one shown; the one last previewed.
  (matches mb-matches set-mb-matches!)
  (revision mb-revision set-mb-revision!)
  (selected mb-selected* set-mb-selected!)
  (offset mb-offset set-mb-offset!)
  (previewed mb-previewed set-mb-previewed!)
  ;; A list source's candidates, and the pattern the matches are for.
  (pool mb-pool set-mb-pool!)
  (matched mb-matched set-mb-matched!))

;; Candidates shown at once.
(define minibuffer-rows 10)

(define (minibuffer s) (sget s 'minibuffer))
(define (minibuffer-open? s) (and (minibuffer s) #t))

(define (completing-read s prompt source
                         #:accept [accept take-target]
                         #:preview [preview #f]
                         #:initial [initial ""]
                         #:pattern [pattern (lambda (input) input)]
                         #:require-match [require-match #t])
  "Read a choice in the minibuffer with PROMPT. SOURCE is a list of
candidates (strings or `candidate`s), or a procedure from the input to such
a list. On RET, (ACCEPT session candidate) is called with the selected
candidate, or one made of the input when nothing matches and REQUIRE-MATCH
is false (M-RET takes the input as it is); by default, the default action
on the candidate's target is done. PREVIEW, if given, is called
the same way for each candidate selected while reading; C-g undoes what it
did to the panes. PATTERN gives the part of the input candidates are
matched against (the file name after its directory)."
  (when (minibuffer s) (close-minibuffer! s))
  (let* ((d (make-document initial))
         (v (make-view d "user"))
         (pane (pane-view s))
         (restore (list (session-panes s) (session-focus s) pane (view-ranges pane) (view-scroll pane)))
         (mb (make-minibuffer prompt v source pattern accept preview require-match restore)))
    (view-set-ranges! v (list (list (document-length d) (document-length d))) 0)
    (set-mb-revision! mb #f)
    (set-mb-offset! mb 0)
    (set-mb-previewed! mb #f)
    (set-mb-pool! mb (and (not (procedure? source)) (map as-candidate source)))
    (set-mb-matched! mb #f)
    (sset! s 'minibuffer mb)
    (sset! s 'input-view v)
    (sset! s 'transient minibuffer-key)
    (sset! s 'extend #f)
    (preview! s)))

(define (close-minibuffer! s)
  (sset! s 'minibuffer #f)
  (sset! s 'input-view #f)
  (sset! s 'transient #f)
  (sset! s 'mb-pending '())
  (sset! s 'extend #f))

(define (minibuffer-input s) (minibuffer-input* (minibuffer s)))
(define (minibuffer-input* mb) (document-string (view-document (mb-view mb))))

;; Run THUNK with commands acting on the focused pane, not the input.
(define (with-pane s thunk)
  (let ((v (sget s 'input-view)))
    (sset! s 'input-view #f)
    (guard (e (#t (sset! s 'input-view v) (raise e)))
      (let ((r (thunk)))
        (sset! s 'input-view v)
        r))))

;;; Matching

;; The parts of a pattern; each matches case-insensitively unless it has
;; an upper-case letter.
(define (pattern-parts pattern)
  (filter (lambda (p) (not (string=? p ""))) (string-split pattern " ")))

(define (fold-case? part) (not (any char-upper-case? (string->list part))))

;; Where PART starts in candidate C's text, or #f.
(define (part-index c part)
  (if (fold-case? part)
      (string-contains (candidate-folded c) part)
      (string-contains (candidate-text c) part)))

(define (matches? c parts) (every (lambda (p) (part-index c p)) parts))

;; Where PART occurs in candidate C's text: (from to), or #f.
(define (find-part c part)
  (let ((i (part-index c part)))
    (and i (list i (+ i (string-length part))))))

;; The spans every part matches in C's text, or #f if one does not occur.
(define (match-spans c parts)
  (let loop ((parts parts) (spans '()))
    (if (null? parts)
        (sort spans (lambda (a b) (< (car a) (car b))))
        (let ((m (find-part c (car parts))))
          (and m (loop (cdr parts) (cons m spans)))))))

;; The candidates to match against PATTERN: a list source's matches for a
;; pattern it extends (they include all of its), else all of them.
(define (source-candidates mb input pattern)
  (let ((before (mb-matched mb)))
    (cond ((procedure? (mb-source mb)) (map as-candidate ((mb-source mb) input)))
          ((and before (string-prefix? before pattern)) (vector->list (mb-matches mb)))
          (else (mb-pool mb)))))

;; The candidates matching the current input, a vector. Which parts of
;; them match is found again for the few shown.
(define (matches mb)
  (let* ((d (view-document (mb-view mb))) (rev (document-revision d)))
    (unless (eqv? rev (mb-revision mb))
      (let* ((input (document-string d))
             (pattern ((mb-pattern mb) input))
             (parts (pattern-parts pattern))
             (found (filter (lambda (c) (matches? c parts)) (source-candidates mb input pattern))))
        (set-mb-matches! mb (list->vector found))
        (set-mb-matched! mb pattern)
        (set-mb-revision! mb rev)
        (set-mb-selected! mb (if (null? found) #f 0))
        (set-mb-offset! mb 0)))
    (mb-matches mb)))

(define (mb-selected mb) (matches mb) (mb-selected* mb))

(define (minibuffer-candidates s) (vector->list (matches (minibuffer s))))

;; The selected candidate, or #f.
(define (minibuffer-selected s)
  (let* ((mb (minibuffer s)) (i (mb-selected mb)))
    (and i (vector-ref (matches mb) i))))

(define (select! s i)
  (let* ((mb (minibuffer s)) (n (vector-length (matches mb))))
    (when (> n 0)
      (set-mb-selected! mb (modulo i n)))))

;; Show the selected candidate if it is not the one shown.
(define (preview! s)
  (let ((mb (minibuffer s)))
    (when (and mb (mb-preview mb))
      (let ((c (minibuffer-selected s)))
        (when (and c (not (eq? c (mb-previewed mb))))
          (set-mb-previewed! mb c)
          (with-pane s (lambda () ((mb-preview mb) s c))))))))

;;; Commands

(define-command (minibuffer-next s n)
  "Select the next candidate."
  (let ((i (mb-selected (minibuffer s)))) (when i (select! s (+ i n)))))

(define-command (minibuffer-previous s n)
  "Select the previous candidate."
  (let ((i (mb-selected (minibuffer s)))) (when i (select! s (- i n)))))

(define-command (minibuffer-first s n) "Select the first candidate." (select! s 0))
(define-command (minibuffer-last s n) "Select the last candidate." (select! s -1))

(define-command (minibuffer-complete s n)
  "Put the selected candidate's text in the input, after the part the
pattern leaves out."
  (let ((c (minibuffer-selected s)) (mb (minibuffer s)))
    (when c
      (let* ((input (minibuffer-input s))
             (keep (- (string-length input) (string-length ((mb-pattern mb) input))))
             (v (mb-view mb))
             (text (string-append (substring input 0 keep) (candidate-text c))))
        (view-edit! v (list (list 0 (document-length (view-document v)) text)) "new")
        (view-set-ranges! v (list (list (document-length (view-document v)) (document-length (view-document v)))) 0)))))

;; Close the minibuffer and hand CHOSEN to its accept procedure.
(define (accept! s chosen)
  (let ((accept (mb-accept (minibuffer s))))
    (close-minibuffer! s)
    (accept s chosen)))

(define-command (minibuffer-accept s n)
  "Take the selected candidate; without one, the input if a match is not required."
  (let ((c (minibuffer-selected s)) (mb (minibuffer s)))
    (cond (c (accept! s c))
          ((mb-require-match mb) (message! s "No match"))
          (else (accept! s (candidate (minibuffer-input s)))))))

(define-command (minibuffer-accept-input s n)
  "Take the input as typed, whatever it matches."
  (accept! s (candidate (minibuffer-input s))))

(define-command (minibuffer-abort s n)
  "Close the minibuffer; panes go back to how they were before previews."
  (abort-minibuffer! s)
  (message! s "Quit"))

;; Close the minibuffer, undoing its previews.
(define (abort-minibuffer! s)
  (let ((mb (minibuffer s)))
    (close-minibuffer! s)
    (when (mb-preview mb)
      (let ((r (mb-restore mb)))
        (set-session-panes! s (car r) (cadr r))
        (view-set-ranges! (caddr r) (cadddr r) 0)
        (view-set-scroll! (caddr r) (list-ref r 4))))))

;;; Acting on targets

;; The default accept procedure: the default action on the target.
(define (take-target s c)
  (if (target? (candidate-target c))
      (act-default! s (candidate-target c))
      (error "No target for" (candidate-text c))))

;; Choose an action on target T, described by NAME, and do it.
(define (act-on! s t name)
  (completing-read s (string-append "Act on " name ": ")
                   (map (lambda (a)
                          (candidate (symbol->string (action-name a)) #:annotation (action-doc a) #:target a))
                        (actions-for (target-kind t)))
                   #:accept (lambda (s c) (run-action! s (candidate-target c) t))))

(define-command (minibuffer-act s n)
  "Act on the selected candidate's target: choose an action for its kind.
The minibuffer closes first, its previews undone."
  (let* ((c (minibuffer-selected s)) (t (and c (candidate-target c))))
    (if (target? t)
        (begin (abort-minibuffer! s) (act-on! s t (candidate-text c)))
        (message! s "No target to act on"))))

(define (target-at-point s)
  (let ((d (doc s)))
    (or (target-at d (point s)) (error "No target at point"))))

(define-command (act-at-point s n)
  "Act on the target at point: choose an action for its kind."
  (let ((t (target-at-point s)))
    (act-on! s t (symbol->string (target-kind t)))))

(define-command (act-default-at-point s n)
  "Do the default action on the target at point."
  (act-default! s (target-at-point s)))

(define minibuffer-map (make-keymap))

(for-each (lambda (b) (define-key! minibuffer-map (car b) (cadr b)))
          '(("C-n" minibuffer-next) ("<down>" minibuffer-next) ("C-p" minibuffer-previous) ("<up>" minibuffer-previous)
            ("M-<" minibuffer-first) ("M->" minibuffer-last)
            ("RET" minibuffer-accept) ("M-RET" minibuffer-accept-input) ("TAB" minibuffer-complete)
            ("C-g" minibuffer-abort) ("ESC" minibuffer-abort) ("C-;" minibuffer-act)
            ;; Editing the input.
            ("C-f" forward-char) ("C-b" backward-char) ("M-f" forward-word) ("M-b" backward-word)
            ("C-a" beginning-of-line) ("C-e" end-of-line) ("<left>" backward-char) ("<right>" forward-char)
            ("<home>" beginning-of-line) ("<end>" end-of-line)
            ("C-d" delete-char) ("<delete>" delete-char) ("DEL" delete-backward-char)
            ("M-d" kill-word) ("M-DEL" backward-kill-word) ("C-k" kill-line) ("C-y" yank)
            ("C-/" undo)))

(define (minibuffer-key s key)
  (let* ((keys (append (or (sget s 'mb-pending) '()) (list key)))
         (binding (lookup-key minibuffer-map keys)))
    (cond ((keymap? binding) (sset! s 'mb-pending keys))
          ((symbol? binding) (sset! s 'mb-pending '()) (run-command s binding 1))
          ((and (null? (cdr keys)) (printable-key? key))
           (insert-text! s (string (key-char key)) "new"))
          (else
           (sset! s 'mb-pending '())
           (message! s (string-append (string-join keys " ") " is undefined in the minibuffer")))))
  (preview! s))

;;; What the frontend shows: (prompt input-view rows selected), rows
;;; around the selected one.

(define (candidate-row c spans)
  (let* ((text (candidate-text c))
         (runs (let loop ((at 0) (spans spans) (acc '()))
                 (cond ((null? spans) (reverse (if (< at (string-length text)) (cons (substring text at (string-length text)) acc) acc)))
                       ((< (caar spans) at) (loop at (cdr spans) acc))
                       (else (let ((from (caar spans)) (to (cadar spans)))
                               (loop to (cdr spans)
                                     (cons (list (substring text from to) 'match)
                                           (if (< at from) (cons (substring text at from) acc) acc)))))))))
    (let ((runs (if (candidate-suffix c) (append runs (list (list (string-append " " (candidate-suffix c)) 'key))) runs)))
      (if (string=? (candidate-annotation c) "")
          (list runs)
          (list runs (list (list (candidate-annotation c) 'comment)))))))

(define (editor-minibuffer s)
  (let ((mb (minibuffer s)))
    (and mb
         (let* ((all (matches mb))
                (n (vector-length all))
                (i (mb-selected mb))
                (offset (cond ((not i) 0)
                              ((< i (mb-offset mb)) i)
                              ((>= i (+ (mb-offset mb) minibuffer-rows)) (+ (- i minibuffer-rows) 1))
                              (else (mb-offset mb))))
                (shown (let loop ((k offset) (acc '()))
                         (if (or (>= k n) (>= k (+ offset minibuffer-rows)))
                             (reverse acc)
                             (loop (+ k 1) (cons (vector-ref all k) acc))))))
           (set-mb-offset! mb offset)
           (list (string-append (number->string (if i (+ i 1) 0)) "/" (number->string n) " " (mb-prompt mb))
                 (mb-view mb)
                 (map (lambda (c) (candidate-row c (match-spans c (pattern-parts ((mb-pattern mb) (minibuffer-input* mb))))))
                      shown)
                 (and i (- i offset)))))))
