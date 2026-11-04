;;; Recovery and pending decisions (doc/operations.md, "Recovery"): an
;;; operation that failed offering recovery choices is a decision, pending
;;; until one of them settles it, it is dismissed, or none applies any
;;; more. Choosing one invokes its command with its arguments, as a child
;;; of the operation that failed, checked as any invocation is then: a
;;; choice expecting a document as it was is refused once it has changed.
;;; \\[decisions] lists them; \\[recover] chooses for the latest, which
;;; the echo area offers when it fails.
;;;
;;; A save refused, the file changed on disk, offers to overwrite the
;;; file (\\[overwrite-file]), to revert the buffer to it
;;; (\\[revert-buffer]), and to compare the two
;;; (\\[diff-buffer-with-file]).

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "files.scm")
(require "buffers.scm")
(require "minibuffer.scm")
(require "views.scm")
(require "diff.scm")
(require "operations.scm")
(require "operation-views.scm")

(provide decisions recover recovery-note pending-decisions choice-applicable choose-recovery! dismiss-decision!
         overwrite-file revert-buffer diff-buffer-with-file)

(define (serial r) (operation-serial (record-operation r)))

;; The choices of R that settle it.
(define (settling r) (filter choice-settles? (record-choices r)))

(define (settled-text r)
  (let ((by (record-settled r)))
    (cond ((eq? by 'dismissed) "it was dismissed")
          ((pair? by) "a choice of it is being checked")
          ((find-operation-record by)
           => (lambda (c)
                (string-append "operation " (number->string by) " "
                               (if (record-pending? c) "is settling it" "settled it"))))
          (else (string-append "operation " (number->string by) " settled it")))))

(define (choice-applicable s r c)
  "Return #t if the choice C of R's failure can be chosen in session S.
Else why not: R is settled, or invoking C's command with its arguments
would be refused now (what it expects changed since)."
  (cond ((and (choice-settles? c) (record-settled r)) (settled-text r))
        (else (guard (e (#t (error-text e)))
                (invoke s (choice-command c) (choice-arguments c) #:mode 'check)
                #t))))

(define (pending-decisions s)
  "Return the records of the operations whose failure is undecided.
Newest first: those that failed offering choices that settle them, not
settled or dismissed, of which one at least applies in session S now."
  (filter (lambda (r)
            (and (pair? (settling r))
                 (not (record-settled r))
                 (any (lambda (c) (eq? #t (choice-applicable s r c))) (settling r))))
          (kept-operation-records)))

(define (choose-recovery! s r c)
  "Choose C, a recovery choice of R's failure, in session S.
Its command is invoked with its arguments, as a child of R's operation,
run for the current principal, who must be the user or the principal R
ran for. A choice that settles R does so once: it takes R before its
invocation is checked, which may wait, so that another chosen meanwhile,
or the same again, is refused; if it is refused or fails, R is open
again. Return what the command returns."
  (let ((who (current-principal)))
    (unless (or (equal? who "user") (equal? who (record-principal r)))
      (error "Only the user, or who it ran for, chooses how an operation recovers" (serial r)))
    (when (and (choice-settles? c) (record-settled r))
      (error (string-append "Operation " (number->string (serial r)) " is settled: " (settled-text r))))
    ;; This choosing's claim on R, until its operation starts.
    (let* ((claim (list 'choosing)) (held claim))
      (when (choice-settles? c) (set-record-settled! r claim))
      (guard (e (#t
                 ;; Refused or failed: the decision is open again, unless
                 ;; something else settled it meanwhile.
                 (when (and (choice-settles? c) (eqv? (record-settled r) held)) (set-record-settled! r #f))
                 (raise e)))
        (invoke s (choice-command c) (choice-arguments c)
                #:principal who #:parent (record-operation r)
                #:started (lambda (cr)
                            (when (choice-settles? c)
                              (set! held (serial cr))
                              (set-record-settled! r held))))))))

(define (dismiss-decision! r)
  "Leave R's failure undecided for good: it is no longer pending."
  (unless (record-settled r) (set-record-settled! r 'dismissed)))

;; Ask in session S which of R's choices to take, then take it.
(define (choose-from! s r)
  (let ((cs (record-choices r)))
    (if (null? cs)
        (message! s "It offers no choices")
        (completing-read
         s (string-append "Recover " (number->string (serial r)) " " (symbol->string (record-command r)) ": ")
         (map (lambda (c)
                (let ((why (choice-applicable s r c)))
                  (candidate (choice-label c)
                             #:annotation (if (eq? why #t) (choice-doc c) (string-append "(not now: " why ")")))))
              cs)
         #:accept (lambda (s cand)
                    (let ((c (find (lambda (c) (string=? (choice-label c) (candidate-text cand))) cs)))
                      (if c
                          (guard (e (#t (failed! s 'recover e)))
                            (choose-recovery! s r c))
                          (message! s "Not one of its choices"))))))))

;;; Commands

(define-command (recover s n)
  "Choose how the latest failure offering choices goes on.
A save refused, say, offers to overwrite the file, to revert the
buffer, or to compare them. \\[decisions] lists every failure
undecided."
  (let ((ds (pending-decisions s)))
    (if (null? ds)
        (message! s "Nothing to recover: no failure is undecided")
        (choose-from! s (car ds)))))

(define (decision-row s r)
  (row (string-append (number->string (serial r)) " ")
       (list (list (symbol->string (record-command r)) 'function))
       (if (record-condition r) (list "  " (list (record-condition r) 'error)) "")
       (string-append "  " (string-join (map choice-label (record-choices r)) ", "))
       #:key (serial r)
       #:target (target 'decision r)))

(define-command (decisions s n)
  "List the failures undecided, newest first: what each offers.
RET chooses for one; \\[act-at-point] offers dismissing it or
inspecting its operation too."
  (show-view! s "*decisions*" (lambda (s) (map (lambda (r) (decision-row s r)) (pending-decisions s)))))

(define-action decision (choose-decision s r)
  "Choose one of what the failure offers."
  (choose-from! s r))

(define-action decision (dismiss-decision s r)
  "Leave the failure undecided: it is no longer listed."
  (dismiss-decision! r)
  (message! s (string-append "Dismissed operation " (number->string (serial r)))))

(define-action decision (inspect-decision s r)
  "Show the operation that failed."
  (inspect-operation s r))

(define-action operation (recover-operation s r)
  "Choose one of what the operation's failure offers."
  (choose-from! s r))

(define (recovery-note e key)
  "Return what a failure's message adds: the choices of E, on KEY.
E is the condition; \"\" when it offers none that settle it. The
application adds it to failures' messages (main.scm)."
  (if (null? (filter choice-settles? (condition-choices e)))
      ""
      (string-append " (" key ": " (string-join (map choice-label (condition-choices e)) ", ") ")")))

;;; What a refused save offers

(define-command (overwrite-file s document)
  "Write DOCUMENT to its file, replacing what the file has now.
What changed in the file since it was read is lost. Chosen after a
save was refused, it writes the text the buffer had then; refused
once the buffer has changed since."
  #:arguments '((document document #:doc "The document to write; with a revision, as it was then."))
  #:effects '(external)
  (save-document! document #:overwrite #t)
  (message! s (string-append "Wrote " (document-path document))))

(define-command (revert-buffer s document)
  "Replace DOCUMENT's text with its file's, unsaved edits too.
It is one edit, which undo takes back. Chosen after a save was refused,
it is refused once the buffer has changed since."
  #:arguments '((document document #:doc "The document to revert; with a revision, as it was then."))
  #:effects '(edit)
  (message! s (if (revert-document! document)
                  (string-append "Reverted to " (document-path document) " (undo takes it back)")
                  "The buffer has the file's text already")))

(define-command (diff-buffer-with-file s document)
  "Show what differs between DOCUMENT's text and its file's.
In *diff* beside the focused pane: lines with - are the file's, with +
the buffer's."
  #:arguments '((document document #:doc "The document to compare with its file."))
  #:effects '(read)
  (let* ((path (document-path document))
         (text (unified-diff (file-text document) (document-string document)
                             #:from (string-append path " (file)") #:to (string-append path " (buffer)")))
         (b (or (buffer-named "*diff*") (make-generated-buffer! "*diff*" (make-document "") 'diff-mode)))
         (d (buffer-document b)))
    ;; The editor's own text, as *Messages*: changed in no operation.
    (without-operation
     (lambda () (view-edit! (make-view d "diff") (list (list 0 (document-length d) (if (string=? text "") "No differences.\n" text))) "new")))
    (display-buffer! s d)))
