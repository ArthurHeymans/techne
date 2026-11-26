;;; Buffers (EDITOR.md, section 1): what can be switched to (modes.scm).
;;; A file's buffer is named by its file and gets the major mode for its
;;; name; a document of no file is *scratch*, in scheme-mode. Each buffer
;;; remembers the view it was last shown in, so going back to it finds its
;;; caret and scroll where they were.
;;;
;;; Files, buffers and locations are targets, with their actions.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "files.scm")
(require "remote.scm")
(require "minibuffer.scm")
(require "requests.scm")
(require "operations.scm")

(provide add-buffer! show-document! show-buffer! visit! file-visit! cancel-file-visit! file-visit-pending?
         save-document-in-session! save-progress-message
         default-directory drop-buffer!
         show-in-other-pane! display-buffer! document-module read-file-name read-directory-name file-name-map find-file switch-to-buffer kill-buffer line-candidate
         search-lines search-all-buffers make-generated-buffer! buffer-named move-path! delete-path-and-buffers!)

;;; Making buffers

(define (buffer-named name)
  "Return the buffer called NAME, or #f."
  (find (lambda (b) (equal? (buffer-name b) name)) (buffer-list)))

(define-hook file-buffer-added
  "After a file's buffer is made: (session buffer).
The session is the current one then. What follows a file while it has
a buffer (a language server) starts here.")

(define (add-buffer! d)
  "Return the buffer of D, made if it has none, first in the list.
A file's is named by the file, with its directory's name after it when
that name is taken. A file's new buffer runs `file-buffer-added`."
  (let* ((new (not (document-buffer d)))
         (b (or (document-buffer d)
                (let ((path (document-path d))
                      (logical (document-logical-path d)))
                  (when path
                    (file-document path #:document d #:codec (document-codec d) #:private (document-unretained? d)))
                  (let ((buf (make-buffer d (if path (file-buffer-name path #f) "*scratch*")
                                          (if path (mode-for-file (or logical path)) 'scheme-mode))))
                    (when (and (document-private? d) (find-option 'lsp-auto))
                      (set-option! 'lsp-auto #f #:buffer buf))
                    buf)))))
    (remember-buffer! b)
    (when (and new (document-path d) (current-session))
      (run-hook! (current-session) 'file-buffer-added b))
    b))

;; The name of the buffer of the file at PATH: the file's, with as many
;; of its directories after it as tell it from the buffers but SELF.
(define (file-buffer-name path self)
  (let ((free? (lambda (name) (let ((b (buffer-named name))) (or (not b) (eq? b self)))))
        (dirs (reverse (remove (lambda (p) (string=? p "")) (string-split (directory-of (absolute-path path)) "/")))))
    (let loop ((n 0))
      (let ((name (if (= n 0)
                      (file-name path)
                      (string-append (file-name path) "<" (string-join (reverse (take dirs n)) "/") ">"))))
        (if (or (free? name) (>= n (length dirs))) name (loop (+ n 1)))))))

;;; Files moved and deleted under open documents. An entry is named
;;; truly up to its last part: a symbolic link moved or deleted is the
;;; link, never the file it points to, whose documents stay as they are.

;; The true name of the entry PATH, its last part not followed.
(define (entry-truename path)
  (let* ((path (if (and (string-suffix? "/" path) (> (string-length path) 1))
                   (substring path 0 (- (string-length path) 1))
                   path))
         (dir (file-truename (let ((d (directory-of (absolute-path path)))) (if (string=? d "") "." d)))))
    (string-append dir (if (string-suffix? "/" dir) "" "/") (file-name path))))

;; The open files' documents at the true name TRUE, or inside it.
(define (documents-under true)
  (filter (lambda (d)
            (let ((p (document-path d)))
              (and p (or (string=? p true) (string-prefix? (string-append true "/") p)))))
          (open-file-documents)))

(define (move-path! from to)
  "Move the file or directory FROM to TO, which must not exist.
The open files moved with it follow: their unsaved edits stay, saving
writes where they are now, and their buffers are named anew. Nothing
moves when the journal of one of them cannot move with it. Return what
kept a journal where it was, a list of texts."
  (let* ((true (entry-truename from))
         (to-true (entry-truename to))
         (moved (documents-under true))
         (new (lambda (d) (string-append to-true (substring (document-path d) (string-length true) (string-length (document-path d))))))
         ;; Refuse crossing a codec or privacy boundary while a document is open.
         (_ (for-each (lambda (d)
                        (let* ((dest (new d))
                               (old-codec (document-codec d))
                               (dest-codec (codec-for-path dest))
                               (old-priv? (document-unretained? d))
                               (dest-priv? (private-path? dest)))
                          (when (not (eq? old-codec dest-codec))
                            (error "Cannot move open document across a codec boundary"
                                   (abbreviate-path (document-path d))
                                   (abbreviate-path dest)))
                          (when (and dest-priv? (not old-priv?))
                            (error "Cannot move non-private open document to a private path"
                                   (abbreviate-path (document-path d))
                                   (abbreviate-path dest)))))
                      moved))
         ;; Each journal is locked before the move, so none can fail after.
         (plans (let loop ((ds moved) (acc '()))
                  (if (null? ds)
                      (reverse acc)
                      (loop (cdr ds)
                            (cons (guard (e (#t (for-each relocation-cancel! acc) (raise e)))
                                    (document-prepare-relocation (car ds) (new (car ds))))
                                  acc))))))
    (guard (e (#t (for-each relocation-cancel! plans) (raise e)))
      (rename-file from to))
    (filter (lambda (problem) problem)
            (map (lambda (d plan)
                   (let ((problem (document-relocate! d plan)))
                     (file-document-moved! d)
                     (let ((b (document-buffer d)))
                       (when b (set-buffer-name! b (file-buffer-name (document-path d) b))))
                     problem))
                 moved plans))))

(define (delete-path-and-buffers! s path)
  "Delete the file or directory PATH, with all it holds, in session S.
The buffers of the open files deleted are killed. It is an error, and
nothing is deleted, when one of them has unsaved edits: they would be
left with no file to be recovered against."
  (let* ((deleted (documents-under (entry-truename path)))
         (dirty (filter document-dirty? deleted)))
    (unless (null? dirty)
      (error "Unsaved edits: save or undo them first" (string-join (map document-path dirty) ", ")))
    (delete-path path #t)
    (for-each (lambda (d)
                ;; Its text is the file's now: none.
                (reload-document! d)
                (let ((b (document-buffer d)))
                  (when (and b (> (length (buffer-list)) 1)) (drop-buffer! s b))))
              deleted)))

(define (make-generated-buffer! name d mode #:state [state #f])
  "Return a new buffer NAME of the document D in MODE, with STATE.
LENS is the lens whose document D is, if any. A buffer of that name is
replaced."
  (let ((old (buffer-named name)))
    (when old (forget-buffer! old))
    (let ((b (make-buffer d name mode #:state state)))
      (remember-buffer! b)
      b)))

(define (new-view b) (make-view (buffer-document b) "user"))

;;; Showing buffers

(define (show-document! s d #:remember [remember #t])
  "Show the document D in the focused pane of S, as `show-buffer!` does.
REMEMBER is as `show-buffer!` takes it."
  (show-buffer! s (or (document-buffer d) (add-buffer! d)) #:remember remember))

(define (show-buffer! s b #:remember [remember #t])
  "Show the buffer B in the focused pane of S.
It is shown in the view it was last shown in, unless another pane, of
S or of another attached session, shows that one. With REMEMBER #f (a
preview) the buffer list's order stays."
  (let ((leaving (pane-view s)))
    (if (document=? (view-document leaving) (buffer-document b))
        leaving
        (let* ((last (buffer-view b))
               (v (cond ((not last) (new-view b))
                        ((any (lambda (p) (view=? p last)) (shown-views s)) (view-split last))
                        (else last)))
               (left (document-buffer (view-document leaving))))
          (when left (set-buffer-view! left leaving))
          (set-buffer-view! b v)
          (when remember (remember-buffer! b))
          (set-pane-view! s v)
          v))))

;; Dedicated persistent scope owning shared background decode tasks so shutting
;; a transient waiter scope cancels only that waiter unless it is the last waiter,
;; without granting root authority to decode workers.
(define %visit-scope (make-scope 'visit-workers))
(define %spawn-in-visit-scope
  (with-scope %visit-scope
    (scope-procedure (lambda (thunk) (spawn thunk)))))
;; Map (abs journal-path reload-cached?) -> %visit-job for in-flight async opens.
(define %in-flight-visits (make-hash-table))

(define-record-type %visit-job
  (%make-visit-job key task waiters settled?)
  %visit-job?
  (key %visit-job-key)
  (task %visit-job-task %set-visit-job-task!)
  (waiters %visit-job-waiters %set-visit-job-waiters!)
  (settled? %visit-job-settled? %set-visit-job-settled?!))

(define-record-type %visit-waiter
  (%make-visit-waiter job session interactive? on-deliver on-fail on-cancel release owned disown done?)
  %visit-waiter?
  (job %visit-waiter-job)
  (session %visit-waiter-session)
  (interactive? %visit-waiter-interactive?)
  (on-deliver %visit-waiter-on-deliver)
  (on-fail %visit-waiter-on-fail)
  (on-cancel %visit-waiter-on-cancel)
  (release %visit-waiter-release %set-visit-waiter-release!)
  (owned %visit-waiter-owned)
  (disown %visit-waiter-disown)
  (done? %visit-waiter-done? %set-visit-waiter-done?!))

(define (clear-interactive-waiter! w)
  (let ((s (%visit-waiter-session w)))
    (when (and (%visit-waiter-interactive? w)
               (eq? (sget s 'visit-waiter) w))
      (sset! s 'visit-waiter #f)
      (sset! s 'visit-path #f)
      (sset! s 'visit-key #f)
      (sset! s 'visit-plain? #f))))

(define (settle-waiter-deliver! w d)
  (unless (%visit-waiter-done? w)
    (%set-visit-waiter-done?! w #t)
    (clear-interactive-waiter! w)
    (guard (e (#t #f)) ((%visit-waiter-disown w)))
    (let ((rel (%visit-waiter-release w)))
      (guard (e (#t
                 (when rel (rel #f e))
                 (let ((fail (%visit-waiter-on-fail w)))
                   (when fail (guard (e2 (#t #f)) (fail e))))))
        ((%visit-waiter-on-deliver w) d)
        (when rel (rel #f #f))))))

(define (settle-waiter-fail! w e)
  (unless (%visit-waiter-done? w)
    (%set-visit-waiter-done?! w #t)
    (clear-interactive-waiter! w)
    (guard (e2 (#t #f)) ((%visit-waiter-disown w)))
    (let ((rel (%visit-waiter-release w)))
      (when rel (rel #f e)))
    (let ((fail (%visit-waiter-on-fail w)))
      (when fail (guard (e2 (#t #f)) (fail e))))))

(define (cancel-visit-waiter! w #:shut [shut? #f])
  (unless (%visit-waiter-done? w)
    (%set-visit-waiter-done?! w #t)
    (clear-interactive-waiter! w)
    (unless shut?
      (guard (e (#t #f)) ((%visit-waiter-disown w))))
    (let ((job (%visit-waiter-job w)))
      (%set-visit-job-waiters! job (remove (lambda (x) (eq? x w)) (%visit-job-waiters job)))
      (when (and (null? (%visit-job-waiters job))
                 (not (%visit-job-settled? job)))
        (%set-visit-job-settled?! job #t)
        (when (eq? (hash-table-ref/default %in-flight-visits (%visit-job-key job) #f) job)
          (hash-table-delete! %in-flight-visits (%visit-job-key job)))
        (let ((t (%visit-job-task job)))
          (when (and t (not (task-done? t)))
            (guard (e (#t #f)) (task-cancel t))))))
    (let ((rel (%visit-waiter-release w)))
      (when rel (rel #t #f)))
    (let ((cb (%visit-waiter-on-cancel w)))
      (when cb (guard (e (#t #f)) (cb))))))

(define (wrap-waiter-callback proc op)
  (and proc
       (let ((scoped (scope-procedure proc)))
         (if op
             (lambda args (call-in-operation op (lambda () (apply scoped args)) #:fail #f))
             (lambda args (without-operation (lambda () (apply scoped args))))))))

(define (start-or-join-visit-job! key target alias pinned-local? journal-path reload-clean?)
  (or (let ((existing (hash-table-ref/default %in-flight-visits key #f)))
        (and existing (not (%visit-job-settled? existing)) existing))
      (let ((job (%make-visit-job key #f '() #f)))
        (hash-table-set! %in-flight-visits key job)
        (let ((t (without-operation
                  (lambda ()
                    (%spawn-in-visit-scope
                     (lambda ()
                       (let ((outcome #f))
                         (dynamic-wind
                          (lambda () #f)
                          (lambda ()
                            (set! outcome
                                  (guard (e ((not (cancelled-condition? e)) (cons 'failed e)))
                                    (when (and pinned-local?
                                               (not (string=? (%canonical-file-path target) target)))
                                      (error (string-append (abbreviate-path target)
                                                            ": file target changed while opening")))
                                    (let* ((was-open (and (cached-open-document-at target) #t))
                                           (d (file-document target
                                                             #:journal-path journal-path
                                                             #:alias alias
                                                             #:pinned pinned-local?)))
                                      (when (and pinned-local?
                                                 (not (equal? (document-path d) target)))
                                        (error (string-append (abbreviate-path target)
                                                              ": file target changed while opening")))
                                      (when (and reload-clean? was-open (not (document-dirty? d)))
                                        (reload-document! d))
                                      (cons 'done d)))))
                          (lambda ()
                            (unless (%visit-job-settled? job)
                              (%set-visit-job-settled?! job #t)
                              (when (eq? (hash-table-ref/default %in-flight-visits key #f) job)
                                (hash-table-delete! %in-flight-visits key))
                              (let ((ws (reverse (%visit-job-waiters job))))
                                (%set-visit-job-waiters! job '())
                                (cond
                                  ((and (pair? outcome) (eq? (car outcome) 'done))
                                   (for-each (lambda (w) (settle-waiter-deliver! w (cdr outcome))) ws))
                                  ((and (pair? outcome) (eq? (car outcome) 'failed))
                                   (for-each (lambda (w) (settle-waiter-fail! w (cdr outcome))) ws))
                                  (else
                                   (for-each (lambda (w) (cancel-visit-waiter! w)) ws))))))))))))))
          (unless t
            (%set-visit-job-settled?! job #t)
            (when (eq? (hash-table-ref/default %in-flight-visits key #f) job)
              (hash-table-delete! %in-flight-visits key))
            (error "file-visit!: visit worker scope is closed"))
          (%set-visit-job-task! job t)
          job))))

(define (file-visit-pending? s)
  "Return #t if session S has an interactive async file visit in flight."
  (let ((w (sget s 'visit-waiter)))
    (and w (not (%visit-waiter-done? w)) #t)))

(define (cancel-file-visit! s)
  "Cancel any pending interactive asynchronous file visit in session S."
  (let ((w (sget s 'visit-waiter)))
    (sset! s 'visit-waiter #f)
    (sset! s 'visit-path #f)
    (sset! s 'visit-key #f)
    (sset! s 'visit-plain? #f)
    (when w (cancel-visit-waiter! w))))

(add-hook! 'keyboard-quit-hook 'cancel-file-visit
           (lambda (s)
             (when (file-visit-pending? s)
               (cancel-file-visit! s))))

(define (visit-status-verb abs)
  (cond
    ((remote-path? abs) "Opening ")
    ((private-path? abs) "Decrypting ")
    ((visit-needs-async? abs) "Decoding ")
    (else "Opening ")))

(define (visit-position! d v line column)
  (when line
    (let ((p (line-down d 0 (max 0 (- line 1)) (max 0 (- (or column 1) 1)))))
      (view-set-ranges! v (list (list p p)) 0)
      (view-set-scroll! v (line-start d p)))))

(define (file-visit! s path
                     #:other-pane [other-pane? #f]
                     #:line [line #f]
                     #:column [column #f]
                     #:async [force-async? #f]
                     #:reload-clean [reload-clean? #f]
                     #:journal-path [journal-path #f]
                     #:alias [alias #f]
                     #:pinned [pinned? #f]
                     #:interactive [interactive? #t]
                     #:on-pending [on-pending #f]
                     #:on-shown [on-shown #f]
                     #:on-ready [on-ready #f]
                     #:on-fail [on-fail #f]
                     #:on-cancel [on-cancel #f])
  "Open PATH in session S, running slow or remote opens asynchronously.
Already-open documents and fast local non-codec files open immediately.
Remote files and codec-backed files (or when FORCE-ASYNC? is true) share a
deduplicated background decode task per path. When INTERACTIVE? is true (the
default), opens supersede a prior interactive open in S and avoid hijacking
focus if S moved panes or opened a minibuffer while waiting; non-interactive
opens (such as server `--wait` requests) run with independent waiters that are
not cancelled by interactive navigation or `C-g`.
OTHER-PANE? displays the buffer in another pane; LINE and COLUMN move point
once opened; RELOAD-CLEAN? reloads an unmodified open document; JOURNAL-PATH
overrides the journal file for a non-private local file; ALIAS records the
original symlink pathname when PATH is already canonical; PINNED? suppresses
re-canonicalization for a previously pinned local path; ON-PENDING, when an
open goes asynchronous, is called with a zero-argument cancellation thunk;
ON-SHOWN is called with (session document view) when displayed in S; ON-READY,
ON-FAIL, and ON-CANCEL override completion, failure, and cancellation handling."
  (let* ((abs (absolute-path path))
         (cached (if (or pinned? alias)
                     #f
                     (cached-open-document-at abs)))
         (cached-needs-async-reload?
          (and cached reload-clean? (not (document-dirty? cached))
               (or (document-codec cached) (remote-path? abs))))
         (origin-view (pane-view s))
         (origin-focus (session-focus s))
         (attached-at-start? (and (memq s (attached-sessions)) #t)))
    (define (session-still-valid?)
      (or (not attached-at-start?) (and (memq s (attached-sessions)) #t)))
    (define (focus-unchanged?)
      (and (session-still-valid?)
           (equal? (session-focus s) origin-focus)
           (view=? (pane-view s) origin-view)
           (not (sget s 'minibuffer))))
    (define (finish! d async?)
      (let ((b (or (document-buffer d) (add-buffer! d))))
        (cond
          (on-ready
           (on-ready s d))
          ((or (not async?) (focus-unchanged?))
           (let ((v (if other-pane? (show-in-other-pane! s d) (show-document! s d))))
             (visit-position! d v line column)
             (when on-shown (on-shown s d v))
             (when async?
               (message! s (string-append (if (document-private? d) "Decrypted " "Opened ")
                                          (abbreviate-path abs))))
             v))
          ((session-still-valid?)
           (when line
             (let* ((last (buffer-view b))
                    (shown? (and last (any (lambda (p) (view=? p last)) (shown-views s))))
                    (v (if (or (not last) shown?)
                           (let ((nv (new-view b)))
                             (set-buffer-view! b nv)
                             nv)
                           last)))
               (visit-position! d v line column)))
           (message! s (string-append "Opened " (abbreviate-path abs) " in background"))
           #f)
          (else #f))))
    (cond
      ((and cached (not force-async?) (not cached-needs-async-reload?))
       (when interactive? (cancel-file-visit! s))
       (when (and reload-clean? (not (document-dirty? cached)))
         (reload-document! cached))
       (finish! cached #f)
       cached)
      ((or force-async? cached-needs-async-reload? (visit-needs-async? abs))
       (let* ((remote? (remote-path? abs))
              (local-target (if remote?
                                abs
                                (if pinned? abs (%canonical-file-path abs))))
              (pinned-local? (not remote?))
              (alias-path (cond ((string? alias)
                                 (let ((a (absolute-path alias)))
                                   (and (not (string=? a local-target)) a)))
                                ((not (string=? local-target abs)) abs)
                                (else #f)))
              (verb (visit-status-verb abs))
              (key (list local-target (or alias-path abs) journal-path (and cached reload-clean? #t)))
              (plain-request? (and (not reload-clean?)
                                   (not journal-path)
                                   (not on-pending)
                                   (not on-shown)
                                   (not on-ready)
                                   (not on-fail)
                                   (not on-cancel)
                                   (not line)
                                   (not other-pane?))))
         (if (and interactive?
                  plain-request?
                  (file-visit-pending? s)
                  (sget s 'visit-plain?)
                  (equal? (sget s 'visit-key) key))
             (begin
               (message! s (string-append verb (abbreviate-path abs) "..."))
               #f)
             (let* ((prev-waiter (and interactive? (sget s 'visit-waiter)))
                    (job (start-or-join-visit-job! key local-target alias-path pinned-local? journal-path reload-clean?))
                    (op (current-operation))
                    (owned (list 'file-visit))
                    (disown (scope-procedure (lambda () (scope-disown! owned))))
                    (w (%make-visit-waiter
                        job
                        s
                        interactive?
                        (wrap-waiter-callback (lambda (d) (finish! d #t)) op)
                        (wrap-waiter-callback
                         (lambda (e)
                           (if on-fail
                               (on-fail s e)
                               (when (session-still-valid?)
                                 (failed! s 'find-file e))))
                         op)
                        (and on-cancel (wrap-waiter-callback (lambda () (on-cancel s)) op))
                        #f
                        owned
                        disown
                        #f)))
               (when op
                 (let* ((released? #f)
                        (hold (operation-hold! op #:cancel (lambda () (cancel-visit-waiter! w)))))
                   (%set-visit-waiter-release!
                    w
                    (lambda (cancelled? err)
                      (unless released?
                        (set! released? #t)
                        (when err (operation-failed! op err))
                        (operation-release! op hold #:cancelled cancelled?))))))
               (scope-own! owned (lambda (r) (cancel-visit-waiter! w #:shut #t)))
               (%set-visit-job-waiters! job (cons w (%visit-job-waiters job)))
               (when interactive?
                 (sset! s 'visit-waiter w)
                 (sset! s 'visit-path abs)
                 (sset! s 'visit-key key)
                 (sset! s 'visit-plain? plain-request?)
                 (when (and prev-waiter (not (eq? prev-waiter w)))
                   (cancel-visit-waiter! prev-waiter)))
               (when (session-still-valid?)
                 (message! s (string-append verb (abbreviate-path abs) "...")))
               (when on-pending
                 (on-pending (lambda () (cancel-visit-waiter! w))))
               #f))))
      (else
       (when interactive? (cancel-file-visit! s))
       (let ((d (file-document abs
                               #:journal-path journal-path
                               #:alias alias
                               #:pinned pinned?)))
         (finish! d #f)
         d)))))

(define (visit! s path line column)
  "Show line LINE, column COLUMN, of the file PATH in S's focused pane.
Lines and columns count from 1. Where the pane was is remembered for
\\[xref-go-back] once navigation succeeds."
  (let* ((v (pane-view s))
         (entry (cons v (location (view-document v) (view-point v)))))
    (file-visit! s path
                 #:line line
                 #:column column
                 #:on-shown (lambda (sess d shown-v)
                              (sset! sess 'visited (cons entry (or (sget sess 'visited) '())))))))

(define (save-progress-message-immediate d)
  (let* ((path (document-path d))
         (verb (if (document-private? d) "Encrypting " "Saving ")))
    (string-append verb (abbreviate-path path) "...")))

(define (save-progress-message d)
  "Return a progress message for an asynchronous save of document D."
  (let* ((path (document-path d))
         (verb (if (document-private? d) "Encrypting " "Saving "))
         (detail (document-save-description d)))
    (if detail
        (string-append verb (abbreviate-path path) " (" detail ")...")
        (string-append verb (abbreviate-path path) "..."))))

(define (save-document-in-session! s d who #:after [after #f] #:on-fail [on-fail #f] #:on-cancel [on-cancel #f]
                                   #:overwrite [overwrite #f] #:if-clean [if-clean 'write])
  "Save document D for session S, running codec or remote writes async.
WHO names the command for error reporting. AFTER, when given, runs once
the save succeeds; ON-FAIL runs with the error condition if the save
fails; ON-CANCEL runs if an asynchronous save is cancelled. OVERWRITE
bypasses the external-change check. When IF-CLEAN is `'skip`, an
already-saved unmodified document is not rewritten and AFTER runs
immediately."
  (let* ((path (document-path d))
         (attached-at-start? (and (memq s (attached-sessions)) #t))
         (session-valid? (lambda () (or (not attached-at-start?) (and (memq s (attached-sessions)) #t))))
         (skip? (and (eq? if-clean 'skip)
                     path
                     (not (document-dirty? d))
                     (document-disk-hash d)))
         (async? (and (not skip?) path (or (document-codec d) (remote-path? path)))))
    (cond
      (skip?
       (when (session-valid?) (message! s "(No changes need to be saved)"))
       (when after (after)))
      ((not async?)
       (let ((ok? #f))
         (guard (e (#t
                    (when on-fail (guard (e2 (#t #f)) (on-fail e)))
                    (raise e)))
           (save-document! d #:overwrite overwrite)
           (set! ok? #t))
         (when ok?
           (when (session-valid?) (message! s (string-append "Wrote " (document-path d))))
           (when after (after)))))
      (else
       (let ((op (current-operation)))
         (when (session-valid?) (message! s (save-progress-message-immediate d)))
         (spawn-held
          (lambda ()
            (let ((ok? #f))
              (guard (e ((cancelled-condition? e)
                         (when (session-valid?) (message! s "Save cancelled"))
                         (when on-cancel (guard (e2 (#t #f)) (on-cancel)))
                         (raise e))
                        (#t
                         (when op (operation-failed! op e))
                         (when (session-valid?) (failed! s who e))
                         (when on-fail (guard (e2 (#t #f)) (on-fail e)))))
                (when (session-valid?)
                  (let ((msg (save-progress-message d)))
                    (when (session-valid?) (message! s msg))))
                (save-document! d #:overwrite overwrite)
                (set! ok? #t))
              (when ok?
                (when (session-valid?) (message! s (string-append "Wrote " (document-path d))))
                (when after (after)))))))))))

;;; Commands

(define (directory-name? path) (string-suffix? "/" path))

;;; The keys of a file name being read, over the minibuffer's. Reading one
;;; (`read-file-name`) completes a directory at a time, so deleting at a
;;; slash goes up a directory, as vertico-directory does in Arthur's Emacs:
;;; with the input "/home/arthur/src/" a DEL leaves "/home/arthur/", rather
;;; than deleting the slash and leaving "src" as the pattern there.

(define file-name-map (make-keymap)
  "The keys of a file name being read, over the minibuffer's own.")

;; Where going up a directory from POS in D starts: just after the slash
;; before the name ending at POS, when the character before POS is a slash
;; and an earlier one is there; #f when not. Characters are stepped over
;; as graphemes, positions being bytes; `directory-of` splits the whole
;; name the same way.
(define (directory-up-start d pos)
  (and (> pos 0)
       (let ((slash (prev-grapheme d pos)))
         (and (string=? (document-substring d slash pos) "/")
              (let loop ((p slash))
                (and (> p 0)
                     (let ((q (prev-grapheme d p)))
                       (if (string=? (document-substring d q p) "/") p (loop q)))))))))

;; POS taken up N directories, or as far as it goes.
(define (directory-up d pos n)
  (let loop ((pos pos) (n n))
    (let ((start (and (> n 0) (directory-up-start d pos))))
      (if start (loop start (- n 1)) pos))))

;; The directory PATH, with its slash, taken up N directories.
(define (directories-up path n)
  (let loop ((path path) (n n))
    (if (or (<= n 0) (<= (string-length path) 1))
        path
        (loop (directory-of (substring path 0 (- (string-length path) 1))) (- n 1)))))

;; The edit (from to text) taking HEAD in D up N directories, or #f. From
;; "~/" it is up from the home directory it stands for.
(define (directory-up-edit d head n)
  (let ((start (directory-up d head n)))
    (cond ((< start head) (list start head ""))
          ((string=? (document-substring d 0 head) "~/")
           (list 0 head (directories-up (absolute-path "~/") n)))
          (else #f))))

;; The edit deleting what MOTION covers from the head of range R, N times,
;; what OPPOSITE covers when N is negative.
(define (motion-edit s r motion opposite n)
  (let ((e (motion-extent s (if (< n 0) opposite motion) (cadr r) (abs n))))
    (list (car e) (cadr e) "")))

;; Delete from every range of S backwards: up a directory at a slash,
;; else what MOTION covers. With KILL the first range's text is saved as
;; a kill, at the front unless N is negative.
(define (delete-in-file-name! s n motion opposite kill)
  (open-minibuffer s)
  (let* ((d (doc s))
         (edits (filter (lambda (e) (or (< (car e) (cadr e)) (not (string=? (caddr e) ""))))
                        (map (lambda (r)
                               (or (and (= (car r) (cadr r)) (> n 0) (directory-up-edit d (cadr r) n))
                                   (motion-edit s r motion opposite n)))
                             (ranges s)))))
    (when (and kill (pair? edits))
      (let ((e (car edits))) (kill-save! s (document-substring d (car e) (cadr e)) #f (> n 0))))
    (when (pair? edits) (edit! s edits 'new))))

(define-command (file-name-delete-backward-char s n)
  "Delete the previous characters in a file name, going up a directory.
When the character before point is a slash, the directory name before
it is deleted too, N directories in all, as
vertico-directory-delete-char does in Emacs. Else the previous N
characters are deleted, as \\[delete-backward-char] does."
  (delete-in-file-name! s n char-backward char-forward #f))

(define-command (file-name-backward-kill-word s n)
  "Kill the previous word in a file name, going up a directory.
When the character before point is a slash, the directory name before
it is killed too, the whole of it however it is spelt, N directories
in all. Else the previous N words are killed, as
\\[backward-kill-word] does."
  (delete-in-file-name! s n emacs-word-backward emacs-word-forward #t))

;; After a slash is typed in a file name: one after a slash leaves the
;; root, and one after a ~ that follows a slash leaves the home directory
;; it stands for (still spelt ~), as Emacs's shadowed file names, tidied
;; by vertico-directory-tidy, do. The tidying is one change with the slash.
(define-command (file-name-insert-slash s n)
  "Insert a slash in a file name, tidying the name it shadows.
A slash after a slash leaves only the root before point; a slash
after a ~ that follows a slash leaves only ~/ before point."
  (open-minibuffer s)
  (insert-text! s "/" 'new)
  (let* ((d (doc s)) (p (point s)))
    (when (and (> p 0) (= p (document-length d)))
      (let ((slash (prev-grapheme d p)))
        (when (and (> slash 0) (string=? (document-substring d slash p) "/"))
          (let* ((before (prev-grapheme d slash)) (ch (document-substring d before slash)))
            (cond ((string=? ch "/")
                   (delete-extents! s (list (list 0 slash #f)) #f 'extend))
                  ((and (string=? ch "~") (> before 0)
                        (string=? (document-substring d (prev-grapheme d before) before) "/"))
                   (delete-extents! s (list (list 0 before #f)) #f 'extend)))))))))

(for-each (lambda (b) (define-key! file-name-map (car b) (cadr b)))
          '(("DEL" file-name-delete-backward-char) ("M-DEL" file-name-backward-kill-word)
            ("/" file-name-insert-slash)))

;; Whether INPUT, read as a file name, names a directory that exists.
(define (existing-directory? input)
  (and (directory-name? input) (guard (e (#t #f)) (path-exists? (absolute-path input)))))

;; Whether taking candidate C reading a file name goes into a directory.
(define (goes-into? c)
  (let ((t (candidate-target c))) (and t (directory-name? (target-value t)))))

(define (read-file-name s prompt initial accept)
  "Read a file name in the minibuffer of S with PROMPT and INITIAL.
The name is completed a directory at a time: the candidates are the
entries of the input's directory. Taking a directory goes into it;
(ACCEPT session path) gets a file's absolute path, also one that does
not exist yet. The input naming a directory that exists is selected
first, as vertico selects it: taking it as it is gives ACCEPT the
directory's path, with its slash."
  (completing-read s prompt
                   (lambda (input)
                     (let ((dir (directory-of input)))
                       (map (lambda (name) (candidate name #:target (target 'file (string-append dir name))))
                            (guard (e (#t '()))
                              (directory-entries (if (string=? dir "") (working-directory) (absolute-path dir)))))))
                   #:initial initial
                   #:pattern file-name
                   #:keymap file-name-map
                   #:require-match #f
                   #:select-input existing-directory?
                   #:history 'file-name
                   #:remember (lambda (entry c)
                                (let* ((t (candidate-target c))
                                       (full (absolute-path (if t (target-value t) entry))))
                                  (and (not (goes-into? c))
                                       (not (guard (e (#t #t))
                                              (or (private-path? full)
                                                  (let ((d (open-document-at full)))
                                                    (and d (document-private? d))))))
                                       entry)))
                   #:accept (lambda (s c)
                              (let ((t (candidate-target c)))
                                (if (goes-into? c)
                                    (read-file-name s prompt (target-value t) accept)
                                    (accept s (absolute-path (if t (target-value t) (candidate-text c)))))))))

(define (default-directory s)
  "Return the directory of the focused pane's file in S, with a slash.
Without a file, it is the directory its buffer's mode says (a
directory's listing is in that directory), else the working directory."
  (let* ((d (view-document (pane-view s)))
         (b (document-buffer d))
         (path (document-path d)))
    (or (and (not path) b (buffer-directory b))
        (directory-of (absolute-path (or path (string-append (working-directory) "/")))))))

(define (read-directory-name s prompt initial accept)
  "Read a directory's name in the minibuffer of S with PROMPT and INITIAL.
It is completed a directory at a time, as `read-file-name` completes:
the candidates are ./, the directory of the input itself, then ../ and
the subdirectories. Taking ./ calls (ACCEPT session directory) with its
absolute name, with a slash; taking another goes into it. The input
naming a directory that exists is selected first, so that RET takes
it, whatever subdirectory was taken before and is listed first.
\\[minibuffer-accept-input] takes the input as it is, also a directory
that does not exist yet."
  ;; The directory taken with candidate C, or #f when it goes into one.
  (define (taken c)
    (let ((t (candidate-target c)))
      (and (or (not t) (string=? (candidate-text c) "./"))
           (normal-directory (absolute-path (if t (target-value t) (candidate-text c)))))))
  (completing-read s prompt
                   (lambda (input)
                     (let ((dir (directory-of input)))
                       (map (lambda (name) (candidate name #:target (target 'file (string-append dir name))))
                            (append '("./" "../")
                                   (filter directory-name?
                                           (guard (e (#t '()))
                                             (directory-entries (if (string=? dir "") (working-directory) (absolute-path dir)))))))))
                   #:initial initial
                   #:pattern file-name
                   #:keymap file-name-map
                   #:require-match #f
                   #:select-input existing-directory?
                   #:history 'file-name
                   #:remember (lambda (entry c) (let ((dir (taken c))) (and dir (abbreviate-path dir))))
                   #:accept (lambda (s c)
                              (let ((dir (taken c)))
                                (if dir
                                    (accept s dir)
                                    (read-directory-name s prompt (abbreviate-path (normal-directory (target-value (candidate-target c)))) accept))))))

(define-command (find-file s file)
  "Open FILE in the focused pane; a directory by its default action.
A directory's default action shows its listing where dired is loaded.
From keys, FILE is completed, starting in this buffer's directory,
with ~ for the home directory, as Emacs abbreviates it."
  #:arguments '((file file #:doc "The file's path. A directory is shown by the default action on it."))
  #:read '((file file #:prompt "Find file: "))
  (let ((path (absolute-path file)))
    (cond ((not (directory-name? path)) (file-visit! s path))
          ((null? (actions-for 'directory)) (error "Is a directory" path))
          (else (act-default! s (target 'directory path))))))

;; A file's buffer is marked modified while it has unsaved edits; other
;; buffers are not saved anywhere.
(define (buffer-annotation b)
  (let ((d (buffer-document b)))
    (string-append (if (and (document-path d) (document-dirty? d)) "modified  " "") (or (document-path d) ""))))

(define-command (switch-to-buffer s n)
  "Show another buffer in the focused pane, previewing it while choosing."
  (let ((current (current-buffer s)))
    (completing-read s "Switch to buffer: "
                     (map (lambda (b) (candidate (buffer-name b) #:annotation (buffer-annotation b) #:target (target 'buffer b)))
                          (append (remove (lambda (b) (eq? b current)) (buffer-list)) (if current (list current) '())))
                     #:preview (lambda (s c) (show-buffer! s (target-value (candidate-target c)) #:remember #f)))))

(define-hook buffer-killed "After a buffer is taken off the buffer list: (session buffer).")

(define (drop-buffer! s b)
  "Take the buffer B off the buffer list, in session S.
Panes showing it, in S and in every other attached session, show the
next buffer, also those \\[minibuffer-abort] in an open minibuffer
would bring back. A file's document stays open, with its unsaved edits."
  (let ((rest (remove (lambda (x) (eq? x b)) (buffer-list))))
    (when (null? rest) (error "The only buffer"))
    (for-each (lambda (s)
                (let ((focus (session-focus s)))
                  (for-each (lambda (i)
                              (when (document=? (view-document (list-ref (session-panes s) i)) (buffer-document b))
                                (sset! s 'focus i)
                                (show-buffer! s (car rest) #:remember #f)))
                            (iota (length (session-panes s))))
                  (sset! s 'focus focus)
                  (minibuffer-replace-document! s (buffer-document b)
                                                (lambda () (new-view (car rest))))))
              (cons s (remove (lambda (x) (eq? x s)) (attached-sessions))))
    (forget-buffer! b)
    (run-hook! s 'buffer-killed b)))

(define-command (kill-buffer s n)
  "Take the focused buffer off the buffer list.
A file's unsaved edits stay in its journal."
  (drop-buffer! s (or (current-buffer s) (error "No buffer"))))

(define (show-in-other-pane! s d)
  "Show the document D in a new pane of S below the focused one.
The new pane is focused."
  (let ((i (session-focus s)))
    (split-pane! s 'below (view-split (pane-view s)))
    (sset! s 'focus (+ i 1))
    (show-document! s d)))

(define (document-module d)
  "Return the name of the module the code of document D evaluates in.
That is its file's when it is Scheme, else \"user\"."
  (let ((b (document-buffer d)) (path (document-path d)))
    (if (and path b (derived-mode? (buffer-mode b) 'scheme-mode)) path "user")))

(define (display-buffer! s d)
  "Show the document D in a pane of S without leaving the focused one.
That is the pane showing it already, else a new one below."
  (unless (any (lambda (v) (document=? (view-document v) d)) (session-panes s))
    (let ((focus (session-focus s)))
      (show-in-other-pane! s d)
      (sset! s 'focus focus))))

;;; Searching lines

(define (line-candidate d pos #:annotation [annotation ""])
  "Return a candidate for the line at POS of D.
Its text is the line, its annotation ANNOTATION and the line's number,
its target the line's location."
  (candidate (line-candidate-text d pos)
             #:annotation (string-append annotation (if (string=? annotation "") "" ":")
                                         (number->string (line-number d pos)))
             #:target (target 'location (location d (line-start d pos)))))

(define (preview-target s c) (act-default! s (candidate-target c)))

(define-command (search-lines s n)
  "Go to a line of this buffer, previewing each line while choosing."
  ;; The lines' starts are as of the revision now: their candidates are
  ;; made later, maybe after edits.
  (let* ((d (doc s)) (m (lines-matcher d)) (revision (document-revision d)))
    (completing-read s "Go to line: "
                     (candidate-table m (lambda (e)
                                          (let ((l (matcher-line m e)))
                                            (candidate (matcher-text m e)
                                                       #:annotation (number->string (cadr l))
                                                       #:target (target 'location (location d (car l) #:revision revision))))))
                     #:preview preview-target
                     #:history 'search
                     #:remember 'input)))

(define-command (search-all-buffers s n)
  "Go to a line of any buffer, previewing each line while choosing."
  ;; As `search-lines`, over the lines of every buffer.
  (let* ((bs (list->vector (buffer-list)))
         (revisions (vector-map (lambda (b) (document-revision (buffer-document b))) bs))
         (m (texts-lines-matcher (map buffer-document (vector->list bs)))))
    (completing-read s "Go to line in buffers: "
                     (candidate-table m (lambda (e)
                                          (let* ((l (matcher-line m e)) (b (vector-ref bs (caddr l))))
                                            (candidate (matcher-text m e)
                                                       #:annotation (string-append (buffer-name b) ":" (number->string (cadr l)))
                                                       #:target (target 'location (location (buffer-document b) (car l)
                                                                                            #:revision (vector-ref revisions (caddr l))))))))
                     #:preview preview-target
                     #:history 'search
                     #:remember 'input)))

;;; Actions on files, buffers and locations; the first is the default.

(define-action file (visit-file s path) "Open the file." (file-visit! s path))
(define-action file (visit-file-other-pane s path) "Open the file in a new pane below." (file-visit! s path #:other-pane #t))
(define-action file (copy-file-name s path) "Save the file's name as a kill." (kill-save! s path #f #f))

(define-action buffer (show-buffer s b) "Show the buffer." (show-buffer! s b))
(define-action buffer (show-buffer-other-pane s b) "Show the buffer in a new pane below." (show-in-other-pane! s (buffer-document b)))
(define-action buffer (save-buffer-document s b)
  "Write the buffer to its file."
  (save-document-in-session! s (buffer-document b) 'save-buffer-document))
(define-action buffer (kill-buffer-target s b) "Take the buffer off the buffer list." (drop-buffer! s b))

;; The focused pane shows the location's document with the caret there.
(define (goto-location! s l show)
  (let* ((p (location-position l)) (v (show s (location-document l))))
    (view-set-ranges! v (list (list p p)) 0)))

(define-action location (goto-location s l) "Go to the location." (goto-location! s l show-document!))
(define-action location (goto-location-other-pane s l) "Go to the location in a new pane below." (goto-location! s l show-in-other-pane!))
(define-action location (copy-location-line s l)
  "Save the location's line as a kill."
  (let ((d (location-document l)))
    (kill-save! s (line-candidate-text d (location-position l)) #f #f #:private (private-document? d))))
