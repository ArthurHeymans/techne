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
;;; A source can be asynchronous (`async-source`): its candidates for an
;;; input come from a task, a service's answer say, while typing goes on;
;;; a newer input cancels the request for the one before. They match the
;;; input already, and are not matched again.
;;;
;;; Candidates with targets can be acted on (C-;): the actions on
;;; the target's kind are offered in the minibuffer in turn, as Embark
;;; offers them. The same works on the target at point in a buffer.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "commands.scm")
(require "targets.scm")
(require "requests.scm")

(provide completing-read async-source candidate candidate? candidate-text candidate-annotation candidate-target candidate-table
         minibuffer-map minibuffer-keymaps minibuffer-open? minibuffer-input minibuffer-candidates minibuffer-match-count minibuffer-selected
         editor-minibuffer open-minibuffer close-minibuffer! abort-minibuffer! minibuffer-replace-document! with-pane act-on! act-at-point act-default-at-point
         take-target pattern-parts matches? match-spans candidate-row)

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
  "Return a candidate of the minibuffer, showing TEXT.
SUFFIX is shown right after it, not matched (a command's key);
ANNOTATION in a column of its own; TARGET is what it stands for."
  (%candidate text suffix annotation target #f))

(define (candidate-folded c)
  (or (%candidate-folded c)
      (let ((f (string-downcase (candidate-text c))))
        (set-candidate-folded! c f)
        f)))

(define (as-candidate c) (if (candidate? c) c (candidate c)))

;;; A source of many candidates: a matcher's entries (`make-matcher`,
;;; `lines-matcher`), matched natively, each made a candidate only when it
;;; is shown, selected or asked for. A list source becomes one.

(define-record-type candidate-table
  (%candidate-table matcher make made)
  candidate-table?
  (matcher table-matcher)
  (make table-make)
  ;; The candidates made, by entry: an entry has one.
  (made table-made))

(define (candidate-table matcher make)
  "Return a source of candidates for `completing-read`: MATCHER's entries.
MATCHER is made by `make-matcher` or `lines-matcher`; (MAKE entry)
returns an entry's candidate (or string), once it is shown, selected
or asked for."
  (%candidate-table matcher make (make-hash-table)))

(define (table-ref t entry)
  (or (hash-table-ref/default (table-made t) entry #f)
      (let ((c (as-candidate ((table-make t) entry))))
        (hash-table-set! (table-made t) entry c)
        c)))

(define (list-table cs)
  (let* ((cs (map as-candidate cs)) (v (list->vector cs)))
    (%candidate-table (make-matcher (map candidate-text cs)) (lambda (e) (vector-ref v e)) (make-hash-table))))

;;; A source whose candidates come later.

(define-record-type async-source
  (%async-source produce min-input)
  async-source?
  (produce async-produce)
  (min-input async-min-input))

(define (async-source produce #:min-input [min-input 3])
  "Return a source of candidates for `completing-read` that come later.
(PRODUCE input) returns the candidates for the input, as a list; it
runs in a task, so it can wait, for a service's answer say. The input
changing cancels it, for the new input. An input shorter than
MIN-INPUT has no candidates. Until they come, those for the input
before are shown."
  (%async-source produce min-input))

;;; The open minibuffer.

(define-record-type minibuffer
  (make-minibuffer prompt view source pattern accept preview require-match restore)
  minibuffer?
  (prompt mb-prompt)
  ;; The input: a view of a document of its own.
  (view mb-view)
  ;; A list of candidates, a candidate table, a procedure
  ;; (input) -> list, or an async source.
  (source mb-source)
  ;; (input) -> the part of it candidates are matched against.
  (pattern mb-pattern)
  (accept mb-accept)
  (preview mb-preview)
  (require-match mb-require-match)
  ;; What the panes were when it opened, for C-g after previews.
  (restore mb-restore set-mb-restore!)
  ;; How many candidates match the input at `revision`; the selected one;
  ;; the first one shown; the one last previewed.
  (matches mb-matches set-mb-matches!)
  (revision mb-revision set-mb-revision!)
  (selected mb-selected* set-mb-selected!)
  (offset mb-offset set-mb-offset!)
  (previewed mb-previewed set-mb-previewed!)
  ;; The candidate table matched: the source's, or for a procedure, its
  ;; candidates for the input.
  (table mb-table set-mb-table!)
  ;; (session) -> undoes what previews did beyond moving in panes.
  (abort mb-abort set-mb-abort!)
  ;; For an async source: its request slot, the input last asked for,
  ;; and (thunk) -> calls THUNK in the scope `completing-read` was
  ;; called in, so that the request's task is owned there.
  (slot mb-slot set-mb-slot!)
  (asked mb-asked set-mb-asked!)
  (in-scope mb-in-scope set-mb-in-scope!)
  (session mb-session set-mb-session!)
  ;; The prompt's own keymap, looked up before the minibuffer's, or #f.
  (keymap mb-keymap set-mb-keymap!)
  ;; (input) -> whether the input is selected first, or #f.
  (select-input mb-select-input set-mb-select-input!))

;; Candidates shown at once.
;; Candidates shown at once, as Arthur's vertico-count.
(define minibuffer-rows 17)

(define (minibuffer s) (sget s 'minibuffer))
(define (open-minibuffer s)
  "Return the open minibuffer of session S, for its commands.
\\[execute-extended-command] can run them when it is not open, which
is an error."
  (or (minibuffer s) (error "The minibuffer is not open")))
(define (minibuffer-open? s)
  "Return #t if the minibuffer of session S is open."
  (and (minibuffer s) #t))
(define (minibuffer-keymaps s)
  "Return the keymaps keys are looked up in while reading in session S.
The prompt's own keymap first, then `minibuffer-map`."
  (let ((mb (minibuffer s)))
    (append (if (and mb (mb-keymap mb)) (list (mb-keymap mb)) '()) (list minibuffer-map))))

(define (completing-read s prompt source
                         #:accept [accept take-target]
                         #:preview [preview #f]
                         #:initial [initial ""]
                         #:pattern [pattern (lambda (input) input)]
                         #:require-match [require-match #t]
                         #:abort [abort #f]
                         #:keymap [keymap #f]
                         #:select-input [select-input #f])
  "Read a choice in the minibuffer of session S with PROMPT and INITIAL.
SOURCE is a list of candidates (strings or `candidate`s), a
`candidate-table` (for many), a procedure from the input to a list,
or an `async-source`.
On \\[minibuffer-accept],
(ACCEPT session candidate) is called with the selected candidate, or
one made of the input when nothing matches and REQUIRE-MATCH is false
(\\[minibuffer-accept-input] takes the input as it is); by default,
the default action on the candidate's target is done. PREVIEW, if
given, is called the same way for each candidate selected while
reading; \\[minibuffer-abort] undoes what it did to the panes, and
calls ABORT, if given, for what else they did. PATTERN gives the part
of the input candidates are matched against (the file name after its
directory). KEYMAP, if given, holds the keys of what is being read
(a file name's, say): it is looked up before `minibuffer-map`.
SELECT-INPUT, if given, is a procedure (input) saying when the input
itself is selected first rather than the first candidate, as vertico
selects an existing directory's name. The procedures run in the scope
this is called in, as the command calling it does."
  (when (minibuffer s) (close-minibuffer! s))
  (let* ((owned (lambda (p) (and p (scope-procedure p))))
         (source (cond ((procedure? source) (owned source))
                       ((async-source? source) (%async-source (owned (async-produce source)) (async-min-input source)))
                       ;; Its own matcher: filtering is the minibuffer's.
                       ((candidate-table? source)
                        (%candidate-table (matcher-copy (table-matcher source)) (owned (table-make source)) (table-made source)))
                       (else source)))
         (accept (owned accept))
         (preview (owned preview))
         (abort (owned abort))
         (d (make-document initial))
         (v (make-view d "user"))
         (pane (pane-view s))
         (restore (list (session-panes s) (session-focus s) pane (view-ranges pane) (view-scroll pane) (session-tree s)))
         (mb (make-minibuffer prompt v source pattern accept preview require-match restore)))
    (view-set-ranges! v (list (list (document-length d) (document-length d))) 0)
    (set-mb-revision! mb #f)
    (set-mb-offset! mb 0)
    (set-mb-previewed! mb #f)
    (set-mb-table! mb (cond ((procedure? source) #f)
                            ((candidate-table? source) source)
                            ((async-source? source) (list-table '()))
                            (else (list-table source))))
    (set-mb-abort! mb abort)
    (set-mb-slot! mb (and (async-source? source) (make-request-slot)))
    (set-mb-asked! mb #f)
    (set-mb-in-scope! mb (owned (lambda (thunk) (thunk))))
    (set-mb-session! mb s)
    (set-mb-keymap! mb keymap)
    (set-mb-select-input! mb (owned select-input))
    (sset! s 'minibuffer mb)
    (sset! s 'input-view v)
    (sset! s 'transient minibuffer-key)
    (sset! s 'transient-map (append (if keymap (list (cons 'prompt keymap)) '())
                                     (list (cons 'minibuffer minibuffer-map))))
    (sset! s 'extend #f)
    (preview! s)))

(define (close-minibuffer! s)
  "Close the minibuffer of session S, keeping what it previewed."
  (let ((mb (minibuffer s)))
    (when (and mb (mb-slot mb)) (cancel-request! (mb-slot mb))))
  (sset! s 'minibuffer #f)
  (sset! s 'input-view #f)
  (sset! s 'transient #f)
  (sset! s 'transient-map #f)
  (sset! s 'mb-pending '())
  (sset! s 'extend #f))

(define (minibuffer-input s)
  "Return the input of the open minibuffer of session S."
  (minibuffer-input* (minibuffer s)))
(define (minibuffer-input* mb) (document-string (view-document (mb-view mb))))

(define (with-pane s thunk)
  "Call THUNK with the commands of S acting on the focused pane.
While the minibuffer is open they act on its input otherwise."
  (let ((v (sget s 'input-view)))
    (sset! s 'input-view #f)
    (guard (e (#t (sset! s 'input-view v) (raise e)))
      (let ((r (thunk)))
        (sset! s 'input-view v)
        r))))

;;; Matching

(define (pattern-parts pattern)
  "Return the parts of PATTERN, the words it has.
Each matches without case unless it has an upper-case letter."
  (filter (lambda (p) (not (string=? p ""))) (string-split pattern " ")))

(define (fold-case? part) (not (any char-upper-case? (string->list part))))

;; Where PART starts in candidate C's text, or #f. Without case, both
;; are folded (a title-case letter is not upper case, but folds).
(define (part-index c part)
  (if (fold-case? part)
      (string-contains (candidate-folded c) (string-downcase part))
      (string-contains (candidate-text c) part)))

(define (matches? c parts)
  "Return #t if every one of PARTS occurs in the text of candidate C."
  (every (lambda (p) (part-index c p)) parts))

;; Where PART occurs in candidate C's text: (from to), or #f.
(define (find-part c part)
  (let ((i (part-index c part)))
    (and i (list i (+ i (string-length part))))))

(define (match-spans c parts)
  "Return the spans each of PARTS matches in candidate C's text.
Return #f if one does not occur."
  (let loop ((parts parts) (spans '()))
    (if (null? parts)
        (sort spans (lambda (a b) (< (car a) (car b))))
        (let ((m (find-part c (car parts))))
          (and m (loop (cdr parts) (cons m spans)))))))

;; How many candidates match the current input. Which parts of them
;; match is found again for the few shown.
(define (matches mb)
  (let* ((d (view-document (mb-view mb))) (rev (document-revision d)))
    (unless (eqv? rev (mb-revision mb))
      (let ((input (document-string d)))
        (when (procedure? (mb-source mb))
          (set-mb-table! mb (list-table ((mb-source mb) input))))
        (when (async-source? (mb-source mb))
          (ask! mb input))
        (let ((n (matcher-filter! (table-matcher (mb-table mb))
                                  (if (async-source? (mb-source mb)) "" ((mb-pattern mb) input)))))
          (set-mb-matches! mb n)
          (set-mb-revision! mb rev)
          (set-mb-selected! mb (cond ((and (allow-prompt? mb) (mb-select-input mb) ((mb-select-input mb) input)) -1)
                                     ((> n 0) 0)
                                     ((allow-prompt? mb) -1)
                                     (else #f)))
          (set-mb-offset! mb 0))))
    (mb-matches mb)))

;; Ask an async source for the candidates for INPUT, unless asked
;; already. When they come they replace those shown, the first selected
;; and previewed, if the minibuffer is still open.
(define (ask! mb input)
  (unless (equal? input (mb-asked mb))
    (set-mb-asked! mb input)
    (let ((source (mb-source mb)) (slot (mb-slot mb)) (s (mb-session mb)))
      (if (< (string-length input) (async-min-input source))
          (begin (cancel-request! slot) (set-mb-table! mb (list-table '())))
          ((mb-in-scope mb)
           (lambda ()
             (request! slot (lambda () ((async-produce source) input))
                       (lambda (cs)
                         (set-mb-table! mb (list-table cs))
                         (set-mb-revision! mb #f)
                         (when (eq? (minibuffer s) mb) (preview! s)))
                       #:fail (lambda (e) (failed! s 'candidates e)))))))))

;; The Kth candidate matching the input.
(define (match-ref mb k)
  (matches mb)
  (table-ref (mb-table mb) (matcher-match (table-matcher (mb-table mb)) k)))

(define (mb-selected mb) (matches mb) (mb-selected* mb))

(define (minibuffer-candidates s)
  "Return the candidates matching the input of S's minibuffer."
  (let ((mb (minibuffer s)))
    (map (lambda (k) (match-ref mb k)) (iota (matches mb)))))

(define (minibuffer-match-count s)
  "Return how many candidates match the input of S's minibuffer.
Unlike `minibuffer-candidates`, it makes none of them."
  (matches (minibuffer s)))

(define (minibuffer-selected s)
  "Return the selected candidate of S's minibuffer, or #f.
It is #f also when the input itself is selected."
  (let* ((mb (minibuffer s)) (i (mb-selected mb)))
    (and i (>= i 0) (match-ref mb i))))

;; Where the input does not have to match, the input itself can be
;; selected, as vertico's prompt: before the first candidate (index -1).
(define (allow-prompt? mb) (not (mb-require-match mb)))

;; Select candidate I, cycling through the candidates and, when it can be
;; selected, the input, as with vertico-cycle.
(define (select! s i)
  (let* ((mb (minibuffer s)) (n (matches mb))
         (low (if (allow-prompt? mb) -1 0))
         (span (- n low)))
    (when (> span 0)
      (set-mb-selected! mb (+ low (modulo (- i low) span))))))

;; Show the selected candidate if it is not the one shown. A preview that
;; fails shows its error, as a command does: it runs after the key's
;; command, outside it.
(define (preview! s)
  (let ((mb (minibuffer s)))
    (when (and mb (mb-preview mb))
      (let ((c (minibuffer-selected s)))
        (when (and c (not (eq? c (mb-previewed mb))))
          (set-mb-previewed! mb c)
          (guard (e (#t (failed! s 'preview e)))
            (with-pane s (lambda () ((mb-preview mb) s c)))))))))

;;; Commands

(define-command (minibuffer-next s n)
  "Select the next candidate."
  (open-minibuffer s)
  (let ((i (mb-selected (minibuffer s)))) (when i (select! s (+ i n)))))

(define-command (minibuffer-previous s n)
  "Select the previous candidate."
  (open-minibuffer s)
  (let ((i (mb-selected (minibuffer s)))) (when i (select! s (- i n)))))

(define-command (minibuffer-first s n)
  "Select the first candidate."
  (open-minibuffer s)
  (select! s 0))
(define-command (minibuffer-last s n)
  "Select the last candidate."
  (open-minibuffer s)
  (select! s (- (matches (minibuffer s)) 1)))

(define-command (minibuffer-complete s n)
  "Put the selected candidate's text in the input.
It goes after the part the pattern leaves out."
  (open-minibuffer s)
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
  "Take the selected candidate.
Without one, or with the input selected, take the input if a match is
not required."
  (open-minibuffer s)
  (let ((c (minibuffer-selected s)) (mb (minibuffer s)))
    (cond (c (accept! s c))
          ((mb-require-match mb) (message! s "No match"))
          (else (accept! s (candidate (minibuffer-input s)))))))

(define-command (minibuffer-accept-input s n)
  "Take the input as typed, whatever it matches.
Where a match is required, the input must be a candidate's text."
  (let* ((mb (open-minibuffer s)) (input (minibuffer-input s)))
    (if (mb-require-match mb)
        ;; Found natively: making every candidate to compare took 200 ms
        ;; among the lines of a 100k-line file.
        (let ((entry (begin (matches mb) (matcher-find (table-matcher (mb-table mb)) input))))
          (if entry (accept! s (table-ref (mb-table mb) entry)) (message! s "No match")))
        (accept! s (candidate input)))))

(define-command (minibuffer-abort s n)
  "Close the minibuffer; panes go back to how they were before previews."
  (open-minibuffer s)
  (abort-minibuffer! s)
  (message! s "Quit"))

(define (minibuffer-replace-document! s d new-view)
  "Make \\[minibuffer-abort] in S bring back no view of the document D.
Panes it would bring back showing D show (NEW-VIEW) instead."
  (let ((mb (minibuffer s)))
    (when mb
      (let* ((r (mb-restore mb))
             (gone? (lambda (v) (document=? (view-document v) d)))
             (focused (if (gone? (caddr r)) (new-view) (caddr r))))
        (set-mb-restore! mb (if (gone? (caddr r))
                                (list (map (lambda (v) (cond ((eq? v (caddr r)) focused) ((gone? v) (new-view)) (else v))) (car r))
                                      (cadr r) focused (view-ranges focused) (view-scroll focused) (list-ref r 5))
                                (cons (map (lambda (v) (if (gone? v) (new-view) v)) (car r)) (cdr r))))))))

(define (abort-minibuffer! s)
  "Close the minibuffer of session S, undoing its previews."
  (let ((mb (minibuffer s)))
    (close-minibuffer! s)
    (when (mb-abort mb) (with-pane s (lambda () ((mb-abort mb) s))))
    (when (mb-preview mb)
      (let ((r (mb-restore mb)))
        (set-session-panes! s (car r) (cadr r))
        (set-session-tree! s (list-ref r 5))
        ;; The focused pane's view, which is the one saved unless another
        ;; session took it meanwhile.
        (view-set-ranges! (pane-view s) (cadddr r) 0)
        (view-set-scroll! (pane-view s) (list-ref r 4))))))

;;; Acting on targets

(define (take-target s c)
  "Do the default action on the target of candidate C in session S.
This is how `completing-read` accepts by default."
  (if (target? (candidate-target c))
      (act-default! s (candidate-target c))
      (error "No target for" (candidate-text c))))

(define (act-on! s t name)
  "Choose an action on the target T, described by NAME, and do it.
The actions are offered in the minibuffer of session S."
  (completing-read s (string-append "Act on " name ": ")
                   (map (lambda (a)
                          (candidate (symbol->string (action-name a)) #:annotation (action-doc a) #:target a))
                        (actions-for (target-kind t)))
                   #:accept (lambda (s c) (run-action! s (candidate-target c) t))))

(define-command (minibuffer-act s n)
  "Act on the selected candidate's target: choose an action for its kind.
The minibuffer closes first, its previews undone."
  (open-minibuffer s)
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

(define minibuffer-map (make-keymap)
  "The keys of the minibuffer, in either profile.")

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

;; The binding of KEYS in MAPS, the first map binding them winning.
(define (minibuffer-binding maps keys)
  (and (pair? maps) (or (lookup-key (car maps) keys) (minibuffer-binding (cdr maps) keys))))

(define (minibuffer-key s key)
  (let* ((keys (append (or (sget s 'mb-pending) '()) (list key)))
         (binding (minibuffer-binding (minibuffer-keymaps s) keys)))
    (cond ((keymap? binding) (sset! s 'mb-pending keys))
          ((symbol? binding) (sset! s 'mb-pending '()) (run-command s binding 1))
          ((and (null? (cdr keys)) (printable-key? key))
           (insert-text! s (string (key-char key)) "new"))
          (else
           (sset! s 'mb-pending '())
           (message! s (string-append (string-join keys " ") " is undefined in the minibuffer")))))
  (preview! s))

;;; What the frontend shows: (prompt input-view rows selected
;;; input-selected?), rows around the selected one.

(define (candidate-row c spans)
  "Return candidate C as the frontend shows a row of it.
SPANS are where the input matches, highlighted."
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
  "Return the minibuffer of session S for the frontend, or #f.
It is (prompt input-view rows selected input-selected?), rows around
the selected one."
  (let ((mb (minibuffer s)))
    (and mb
         (let* ((n (matches mb))
                (i (mb-selected mb))
                (offset (cond ((or (not i) (< i 0)) 0)
                              ((< i (mb-offset mb)) i)
                              ((>= i (+ (mb-offset mb) minibuffer-rows)) (+ (- i minibuffer-rows) 1))
                              (else (mb-offset mb))))
                (shown (let loop ((k offset) (acc '()))
                         (if (or (>= k n) (>= k (+ offset minibuffer-rows)))
                             (reverse acc)
                             (loop (+ k 1) (cons (match-ref mb k) acc))))))
           (set-mb-offset! mb offset)
           ;; The count, unless the input is read without candidates.
           (list (if (equal? (mb-source mb) '())
                     (mb-prompt mb)
                     (string-append (if (and (mb-slot mb) (request-pending? (mb-slot mb))) "… " "")
                                    (if (and i (>= i 0)) (number->string (+ i 1)) "*") "/" (number->string n) " " (mb-prompt mb)))
                 (mb-view mb)
                 (map (lambda (c) (candidate-row c (or (match-spans c (pattern-parts ((mb-pattern mb) (minibuffer-input* mb)))) '())))
                      shown)
                 (and i (>= i 0) (- i offset))
                 (eqv? i -1))))))
