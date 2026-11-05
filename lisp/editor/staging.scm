;;; Reviewing staged operations: proposals, never source edits disguised
;;; as a preview. The native proposal table keeps transactions and checks
;;; every preview revision before any document changes. This view and its
;;; actions are Lisp policy; accepting is a new, linked operation.

(require "session.scm")
(require "dispatch.scm")
(require "contracts.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "buffers.scm")
(require "views.scm")
(require "operations.scm")
(require "operation-views.scm")
(require "diff.scm")
(require "lens.scm")

(provide review-operation-proposals accept-operation-proposals!)

(define (serial r) (operation-serial (record-operation r)))
(define (pending-proposals r)
  ;; A missing/forgotten proposal must be shown as a conflict, not silently
  ;; omitted from a batch someone might then accept as complete.
  (filter (lambda (p) (guard (e (#t #t)) (not (member (proposal-state p) '("applied" "rejected")))))
          (operation-proposals (record-operation r))))

(define-record-type proposal-review
  (make-proposal-review record ids seen)
  proposal-review?
  (record review-record)
  (ids review-ids set-review-ids!)
  (seen review-seen set-review-seen!))

(define (ready! r)
  (when (record-pending? r) (error "The operation is still pending; review it once its work settles"))
  (unless (operation-staged? (record-operation r)) (error "This operation was not staged")))

(define (accept-operation-proposals! s r ids seen)
  "Accept the staged proposals IDS of operation record R in S.
SEEN gives each proposal's preview revision. Every revision and conflict
is checked before anything changes; a later journal failure can leave a
partial result, recorded as such. Acceptance is a child operation of R.
Refuse while R is pending or its pending proposal set changed."
  (ready! r)
  (invoke s 'accept-staged-proposals (dict 'operation (serial r) 'proposals ids 'seen seen)
          #:parent (record-operation r) #:principal (current-principal)))

(define-command (accept-staged-proposals s operation proposals seen)
  "Accept OPERATION's PROPOSALS as previewed at revisions SEEN.
Refuse if the proposal set or a document changed. All documents are
checked before any edit; a journal failure can still be partial."
  #:arguments '((operation natural #:doc "The staged operation's number.")
                (proposals (list-of natural) #:doc "Every pending proposal, in order.")
                (seen (list-of natural) #:doc "Their preview revisions, in the same order."))
  #:effects '(edit)
  (let ((r (or (find-operation-record operation) (error "The staged operation was forgotten"))))
    (ready! r)
    (unless (and (pair? proposals) (equal? proposals (pending-proposals r)))
      (error "The pending proposals changed; review them again"))
    (for-each (lambda (id) (check-editable! (make-view (proposal-document id) (current-principal)))) proposals)
    (proposals-accept! proposals seen)
    (message! s (string-append "Accepted " (number->string (length proposals)) (if (= (length proposals) 1) " proposal of operation " " proposals of operation ") (number->string operation)))))

;; Source text is read only to make the diff; SEEN records exactly the
;; revision displayed, not one obtained later when acceptance is chosen.
(define (preview-rows id preview)
  (let* ((d (proposal-document id)) (edits (cadr preview))
         (before (document-string d))
         (after (apply string-append
                       (let loop ((es edits) (at 0) (parts '()))
                         (if (null? es)
                             (reverse (cons (document-substring d at (document-length d)) parts))
                             (let ((e (car es)))
                               (loop (cdr es) (cadr e)
                                     (cons (caddr e) (cons (document-substring d at (car e)) parts))))))))
         (label (document-label d))
         (lines (string-split (unified-diff before after #:from label #:to label) "\n")))
    (cons (row "file  " label #:key (string-append "file-" (number->string id))
               #:target (target 'location (location d (if (null? edits) 0 (car (car edits))))))
          (map (lambda (line i)
                 (row "  " (list (list line (cond ((or (string-prefix? "+++" line) (string-prefix? "---" line)) 'diff.header)
                                                 ((string-prefix? "@@" line) 'diff.hunk)
                                                 ((string-prefix? "+" line) 'diff.added)
                                                 ((string-prefix? "-" line) 'diff.removed)
                                                 (else 'default))))
                      #:key (string-append (number->string id) ":" (number->string i))))
               lines (iota (length lines))))))

(define (review-rows review)
  (let* ((r (review-record review)) (ids (pending-proposals r)) (seen '()))
    (set-review-ids! review ids)
    (let ((rows (append-map
                 (lambda (id)
                   (guard (e (#t (set! seen (append seen (list #f)))
                                  (list (row "conflict  " (list (list (error-text e) 'error))))))
                     (let* ((preview (proposal-preview id)) (rows (preview-rows id preview)))
                       (set! seen (append seen (list (car preview))))
                       rows))) ids)))
      (set-review-seen! review seen)
      (append (list (row "staged operation  " (number->string (serial r)) #:key 'operation #:target (target 'operation r))
                    (row "keys  " "C-c C-c accept all   C-c C-k reject all   g refresh   q quit" #:key 'keys))
              (if (null? ids) (list (row "proposals  " "None pending")) rows)))))

(define-mode proposal-review-mode
  "Read-only diffs of a staged operation's pending proposals.
\\[accept-reviewed-proposals] accepts exactly the revisions shown;
\\[reject-reviewed-proposals] rejects them; \\[view-refresh] refreshes."
  #:parent 'rows-mode
  #:keys '(("C-c C-c" accept-reviewed-proposals) ("C-c C-k" reject-reviewed-proposals) ("g" view-refresh) ("q" quit-window))
  #:normal '(("C-c C-c" accept-reviewed-proposals) ("C-c C-k" reject-reviewed-proposals) ("g" view-refresh) ("q" quit-window)))

(define-action operation (review-operation-proposals s r)
  "Review the operation's staged proposals as read-only diffs.
Accept only after review: changed text or proposals require refreshing."
  (ready! r)
  (let ((review (make-proposal-review r '() '())))
    (show-view! s (string-append "*proposals: " (number->string (serial r)) "*")
                (lambda (s) (review-rows review)) #:mode 'proposal-review-mode #:data review)))

(define-action staged-operation (review-staged-operation s r)
  "Review this operation's proposals, without editing their sources."
  (review-operation-proposals s r))

(define (here s)
  (let ((review (view-data (current-buffer s))))
    (if (proposal-review? review) review (error "Not a proposal review"))))

(define-command (accept-reviewed-proposals s n)
  "Accept all the proposals exactly as this review showed them.
A changed document or proposal set is refused; refresh and review again."
  (let ((review (here s)))
    (unless (every exact-integer? (review-seen review)) (error "Resolve the conflicts before accepting"))
    (accept-operation-proposals! s (review-record review) (review-ids review) (review-seen review))
    (view-refresh s 1)))

(define-command (reject-reviewed-proposals s n)
  "Reject all pending proposals shown in this review; change no source."
  (let ((review (here s)))
    (unless (equal? (review-ids review) (pending-proposals (review-record review)))
      (error "The pending proposals changed; refresh before rejecting"))
    (for-each proposal-reject! (review-ids review))
    (view-refresh s 1)))
