;;; Operations as targets (doc/operations.md, "Operations as targets"):
;;; \\[operations] lists the operations kept, pending ones first, each a
;;; target; inspecting one says what ran, how it ended, its parent and the
;;; changes it made, as locations; its changes show as a lens, edited
;;; through. \\[operation-at-point] says which operation wrote the text at
;;; point: blame, from a change to the command that made it.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "buffers.scm")
(require "views.scm")
(require "lens.scm")
(require "inspect.scm")
(require "operations.scm")

(provide operations operation-at-point inspect-operation show-operation-changes undo-operation! undo-operation forget-operation)

(define (serial r) (operation-serial (record-operation r)))

;; The name of the buffer of the document with ID, or #f.
(define (buffer-of-document id)
  (let ((b (find (lambda (b) (= (document-id (buffer-document b)) id)) (buffer-list))))
    (and b (buffer-name b))))

(define (outcome-text r)
  (let ((op (record-operation r)))
    (if (record-pending? r)
        (string-append "pending (" (symbol->string (operation-phase op)) ")")
        (symbol->string (operation-outcome op)))))

(define (changes-text r)
  (let* ((op (record-operation r)) (n (operation-change-total op))
         (saves (+ (length (operation-saves op)) (cadr (operation-omitted op)))))
    (string-append (if (= n 0) "" (string-append (number->string n) (if (= n 1) " change" " changes")))
                   (if (and (> n 0) (> saves 0)) ", " "")
                   (if (= saves 0) "" (string-append "wrote " (number->string saves))))))

(define (operation-row r)
  (let ((op (record-operation r)))
    (row (string-append (number->string (serial r)) " ")
         (list (list (symbol->string (operation-command op)) 'function)
               (if (> (record-runs r) 1) (string-append " ×" (number->string (record-runs r))) ""))
         (string-append "  " (outcome-text r) (if (operation-staged? op) " (staged)" ""))
         (string-append "  " (or (and (record-document r) (buffer-of-document (record-document r))) ""))
         (string-append "  " (changes-text r))
         (if (record-condition r) (list "  " (list (record-condition r) 'error)) "")
         #:key (serial r)
         #:target (target 'operation r))))

(define-command (operations s n)
  "List the operations kept: those that changed something or failed.
Pending ones come first, but for this one and those running it
(\\[execute-extended-command]).
RET inspects one; \\[act-at-point] offers its other actions, as
showing the changes it made."
  (show-view! s "*operations*"
              (lambda (s)
                (let* ((listing (running-serials))
                       (pending (remove (lambda (r) (memv (serial r) listing)) (pending-operation-records))))
                  (map operation-row
                       (append pending
                               (remove (lambda (r) (or (memq r pending) (memv (serial r) listing))) (kept-operation-records))))))))

;; The serials of the current operation and those it runs in.
(define (running-serials)
  (let loop ((op (current-operation)) (acc '()))
    (if (not op)
        acc
        (let* ((parent (operation-parent-serial op)) (r (and parent (find-operation-record parent))))
          (loop (and r (record-operation r)) (cons (operation-serial op) acc))))))

;;; One operation

;; The changes as locations, one row a line of a document changed in a
;; run of revisions (a word typed): where the newest is now. A change of
;; several places shows each.
(define (change-rows op)
  (append
   (map (lambda (g)
          (let ((d (car g)) (line (cadr g)) (revs (caddr g)) (pos (cadddr g)))
            (row "changed"
                 (string-append (document-label d) ":" (number->string line)
                                (if (= (length revs) 1)
                                    (string-append "  revision " (number->string (car revs)))
                                    (string-append "  revisions " (number->string (last revs)) "–" (number->string (car revs)))))
                 #:target (target 'location (location d pos)))))
        (reverse
         (fold (lambda (c groups)
                 ;; Each span's line: a change of several places shows each.
                 (let ((d (car c)) (rev (cadr c)))
                   (fold (lambda (span groups)
                           (let* ((pos (car span)) (line (line-number d pos)))
                             (if (and (pair? groups) (document=? (car (car groups)) d) (= (cadr (car groups)) line))
                                 (if (memv rev (caddr (car groups)))
                                     groups
                                     (cons (list d line (cons rev (caddr (car groups))) pos) (cdr groups)))
                                 (cons (list d line (list rev) pos) groups))))
                         groups
                         (or (document-change-spans d rev) '()))))
               '()
               (operation-changes op))))
   (map (lambda (c)
          (row "changed" (string-append (number->string (cadr c)) " more in " (document-label (car c)))))
        (operation-change-counts op))
   (map (lambda (path) (row "wrote" path #:target (target 'file path))) (operation-saves op))
   ;; Past the bounds of what an operation records.
   (let ((omitted (operation-omitted op)))
     (append (if (> (car omitted) 0)
                 (list (row "changed" (string-append (number->string (car omitted)) " more, not named")))
                 '())
             (if (> (cadr omitted) 0)
                 (list (row "wrote" (string-append (number->string (cadr omitted)) " more files, not named")))
                 '())))))

(define (operation-rows r)
  (let* ((op (record-operation r))
         (parent (operation-parent-serial op))
         (parent-record (and parent (find-operation-record parent)))
         (name (operation-command op)))
    (append
     (list (row "operation" (number->string (serial r)))
           (row "command" (symbol->string name) #:target (target 'command name))
           (row "outcome" (outcome-text r)))
     (if (operation-staged? op)
         (list (row "mode" "staged")
               (row "proposals" (number->string (length (operation-proposals op)))
                    #:target (target 'staged-operation r)))
         '())
     (if (> (record-runs r) 1) (list (row "runs" (number->string (record-runs r)))) '())
     (if parent
         (list (if parent-record
                   (row "parent" (string-append (number->string parent) " " (symbol->string (operation-command (record-operation parent-record))))
                        #:target (target 'operation parent-record))
                   (row "parent" (string-append (number->string parent) ", forgotten"))))
         '())
     (let ((b (and (record-document r) (buffer-of-document (record-document r)))))
       (if b (list (row "started in" (string-append b ", revision " (number->string (record-revision r))))) '()))
     (if (record-prefix r) (list (row "prefix" (call-with-output-string (lambda (p) (write (record-prefix r) p))))) '())
     (if (record-code r)
         (list (row "code" (if (and (memq name (command-names)) (eqv? (command-code name) (record-code r)))
                               (string-append "as defined now (" (number->string (record-code r)) ")")
                               (string-append "redefined since (" (number->string (record-code r)) ")"))
                    #:target (and (memq name (command-names)) (target 'command name))))
         '())
     (if (not (equal? (record-principal r) "user")) (list (row "for" (record-principal r))) '())
     (map (lambda (a)
            (row "argument" (string-append (symbol->string (car a)) " "
                                           (if (string? (cdr a)) (cdr a) (call-with-output-string (lambda (p) (write (cdr a) p)))))))
          (or (record-arguments r) '()))
     (if (record-effects r)
         (list (row "effects" (if (null? (record-effects r)) "none" (string-join (map symbol->string (record-effects r)) " "))))
         '())
     (if (record-violation r) (list (row "violation" (list (list (record-violation r) 'warning)))) '())
     (if (record-condition r) (list (row "failed with" (list (list (record-condition r) 'error)))) '())
     (change-rows op))))

(define (show-operation! s r)
  (show-view! s (string-append "*operation " (number->string (serial r)) "*") (lambda (s) (operation-rows r))))

(define-action operation (inspect-operation s r)
  "Show the operation: what ran, how it ended, the changes it made."
  (show-operation! s r))

(define-action operation (show-operation-changes s r)
  "Show the text the operation changed as a lens, edited through."
  (let* ((places (append-map (lambda (c)
                               (let ((d (car c)))
                                 (append-map (lambda (span) (span-lines d (car span) (cadr span)))
                                             (or (document-change-spans d (cadr c)) '()))))
                             (operation-changes (record-operation r))))
         ;; One row a line.
         (same? (lambda (a b) (and (document=? (car a) (car b)) (= (cdr a) (cdr b)))))
         (lines (reverse
                 (fold (lambda (p acc)
                         (let ((l (cons (car p) (line-start (car p) (cdr p)))))
                           (if (any (lambda (x) (same? x l)) acc) acc (cons l acc))))
                       '()
                       places))))
    (if (null? lines)
        (message! s "It changed no text that is still open")
        (begin
          (check-lens-size (length lines))
          (show-lens! s (string-append "*changes of " (number->string (serial r)) "*") (line-rows lines))))))

;;; Undo of an operation (doc/operations.md, "Undo of an operation")

(define (labels docs) (string-join (map document-label docs) ", "))

;; The documents OP changed, in the order it first changed them.
(define (changed-documents op)
  (fold (lambda (d acc) (if (any (lambda (x) (document=? x d)) acc) acc (append acc (list d))))
        '()
        (append (map car (operation-changes op)) (map car (operation-change-counts op)))))

(define (undo-operation! s r)
  "Undo the changes of R's operation from session S, as an operation.
In each document they must all still be in its history, made as one
actor (an accepted proposal's are its grant's), and be that actor's
latest undo units, holding no other operation's changes; no later
change by anyone may touch the same text. Else it is refused, saying
why, before any document changes: all are checked in one call, which
no task interrupts. The undo is a child of the operation undone, run
for the current principal, a revision a unit in each document. A
failure part way (a journal) says which documents it was undone in and
which not. Return the undo's operation."
  (let* ((op (record-operation r))
         (changes (operation-changes op))
         (counts (operation-change-counts op))
         (docs (changed-documents op))
         ;; The revision its first change in each made: the native checks
         ;; it is still in its history, and with it all the later ones.
         (earliest (map (lambda (d)
                          (apply min (append (map cadr (filter (lambda (c) (document=? (car c) d)) changes))
                                             (map caddr (filter (lambda (c) (document=? (car c) d)) counts)))))
                        docs))
         (refuse (lambda (why) (error (string-append "undo-operation " (number->string (serial r)) ": " why)))))
    (when (record-pending? r) (refuse "it is still pending"))
    (when (= 0 (operation-change-total op)) (refuse "it changed no text"))
    (when (< (+ (length changes) (fold + 0 (map cadr counts))) (operation-change-total op))
      (refuse "some of its changes are in documents closed since"))
    ;; Run for whoever asks, a child of the operation undone.
    (let ((undo (start-operation! 'undo-operation s #:parent op #:principal (current-principal))))
      (call-in-operation undo
        (lambda ()
          (let* ((results (documents-undo-operation! docs earliest op))
                 (where (lambda (prefix) (filter-map (lambda (p) (and (string-prefix? prefix (cdr p)) (car p))) (map cons docs results))))
                 (said (lambda (prefix) (find (lambda (r) (string-prefix? prefix r)) results))))
            (cond ((said "refused: ")
                   => (lambda (r) (refuse (string-append (document-label (car (where "refused: "))) ": " (substring r 9 (string-length r))))))
                  ((said "failed: ")
                   => (lambda (r)
                        (let ((done (where "undone")) (failed (car (where "failed: "))))
                          (refuse (string-append (if (null? done) "undone nowhere" (string-append "undone in " (labels done)))
                                                 ", not in " (labels (cons failed (where "untouched"))) ": "
                                                 (document-label failed) ": " (substring r 8 (string-length r)))))))
                  (else (message! s (string-append "Undid operation " (number->string (serial r)) " in " (labels docs))))))))
      undo)))

(define-action operation (undo-operation s r)
  "Undo the operation's changes in every document, as one operation.
Refused, changing nothing, unless they are the latest of its principal's
in each, or if a later change touches the same text."
  (undo-operation! s r))

(define-action operation (forget-operation s r)
  "Forget the operation's record, rejecting any proposals still pending.
Its committed edits stay; no source text is changed by forgetting it."
  (forget-operation! r)
  (message! s (string-append "Forgot operation " (number->string (serial r)))))

;; The starts of the lines of D that FROM to TO is on, each (D . start):
;; a span ending at a line's start is not on that line, a deletion (an
;; empty span) is on the line it was on.
(define (span-lines d from to)
  (let ((last (if (and (> to from) (= to (line-start d to))) (prev-grapheme d to) to)))
    (let loop ((at (line-start d from)) (acc '()))
      (if (> at last)
          (reverse acc)
          (let ((end (line-end d at)))
            (if (>= end (document-length d))
                (reverse (cons (cons d at) acc))
                (loop (next-grapheme d end) (cons (cons d at) acc))))))))

;;; Blame

(define-command (operation-at-point s n)
  "Say which operation wrote the character at point, and show it.
That is the command run that made the change, while it is remembered."
  (let* ((here (if (presentation? (doc s))
                   ;; In a lens: the text of the document it shows there.
                   (presentation-source-at (doc s) (point s))
                   (list (doc s) (point s))))
         (d (and here (car here)))
         (rev (and here (document-change-at d (cadr here))))
         (id (and rev (document-operation d rev))))
    (cond ((not here) (message! s "Not the text of a document here"))
          ((not rev) (message! s "No change since it was opened wrote this"))
          ((not id) (message! s "Written in no operation (by the editor, a hook or another program)"))
          ((not (= (car id) (operation-runtime)))
           (message! s (string-append "Written in operation " (number->string (cadr id)) " of an earlier run")))
          (else
           (let ((r (find-operation-record (cadr id))))
             (if r
                 (show-operation! s r)
                 (message! s (string-append "Written in operation " (number->string (cadr id)) ", forgotten"))))))))
