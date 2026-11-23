;;; Files other programs open in the running editor, as with Emacs's
;;; server and emacsclient: `techne --open FILE` shows the file in the
;;; current session; with `--wait`, the program waits until the file is
;;; done with, so the editor serves as $EDITOR. It is done with when its
;;; buffer is killed, by \[server-edit] (which saves it first), by the
;;; quit command (:wq and :q in the modal profile), or by \[kill-buffer].
;;;
;;; And code other programs evaluate in it, as with emacsclient --eval:
;;; `techne eval FORM` evaluates FORM in the session used last, so that
;;; what it shows is shown where you look, or with --detached in one no
;;; frontend shows. *Server log* says what was evaluated, for whom, and
;;; what came of it; the option `server-eval` turns it off.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "files.scm")
(require "buffers.scm")
(require "help.scm")
(require "emacs.scm")
(require "operations.scm")
(require "requests.scm")
(require "remote.scm")
(require "shell.scm")

(provide editor-open! editor-take-opened! editor-take-failed! editor-take-done! editor-forget!
         waited? finish-waited! server-edit editor-eval! editor-eval-ended! server-eval-max-chars)

;; The programs waiting, (id . document); in-flight async opens, id -> cancel-thunk;
;; and settlement queues for the runtime to deliver to clients.
(define %waiting '())
(define %pending-opens (make-hash-table))
(define %opened '())
(define %failed '())
(define %done '())

(define (editor-open! path id [wait? #t])
  "Show the file at PATH in the current session, for a program.
ID identifies the request; with WAIT? true (the default when ID is given),
the program waits until the file is done with, and ID is among those
`editor-take-done!` returns then. A document of the file without unsaved
edits is read again: the program may have written the file since it was
last shown, as Git writes COMMIT_EDITMSG.
Returns \"opened\" when completed synchronously or \"pending\" when an
asynchronous open is in flight (settled via `editor-take-opened!` or
`editor-take-failed!`)."
  (let* ((s (or (current-session) (error "No frontend is attached")))
         (wait-for? (and id wait? #t))
         (went-async? #f)
         (settled? #f))
    (define (record-failed! msg)
      (when (and id (not settled?))
        (set! settled? #t)
        (hash-table-delete! %pending-opens id)
        (set! %failed (append %failed (list (list (number->string id) msg))))))
    (file-visit!
     s path
     #:reload-clean #t
     #:interactive #f
     #:on-pending
     (lambda (cancel!)
       (set! went-async? #t)
       (when id (hash-table-set! %pending-opens id cancel!)))
     #:on-ready
     (lambda (sess d)
       (when (or (not id) (not went-async?) (hash-table-ref/default %pending-opens id #f))
         (when id (hash-table-delete! %pending-opens id))
         (let ((target (cond ((memq sess (attached-sessions)) sess)
                             ((current-session) => (lambda (cs) cs))
                             (else #f))))
           (if (not target)
               (record-failed! "the frontend detached before the file opened")
               (let ((in-mini? (and (sget target 'minibuffer) #t)))
                 (if in-mini?
                     (add-buffer! d)
                     (show-document! target d))
                 (when wait-for?
                   (set! %waiting (cons (cons id d) %waiting)))
                 (when (and id went-async? (not settled?))
                   (set! settled? #t)
                   (set! %opened (append %opened (list id))))
                 (message! target
                           (cond ((not wait-for?)
                                  (string-append "Opened " (absolute-path path)
                                                 (if in-mini? " in background" "")))
                                 (in-mini?
                                  (string-append "Opened " (abbreviate-path (absolute-path path))
                                                 " in background for waiting program"))
                                 ((eq? (profile-name (sget target 'profile)) 'modal)
                                  "When done with the file, type :wq")
                                 (else
                                  (substitute-command-keys target "When done with the file, type \\[server-edit]")))))))))
     #:on-fail
     (lambda (sess e)
       (record-failed! (error-text e))
       (when (memq sess (attached-sessions))
         (failed! sess 'editor-open! e)))
     #:on-cancel
     (lambda (sess)
       (record-failed! "open cancelled")))
    (if went-async? "pending" "opened")))

(define (waited? d)
  "Return #t if a program waits until the document D is done with."
  (any (lambda (w) (document=? (cdr w) d)) %waiting))

(define (done! d)
  (let ((waiting (filter (lambda (w) (document=? (cdr w) d)) %waiting)))
    (set! %waiting (remove (lambda (w) (memq w waiting)) %waiting))
    (set! %done (append %done (map car waiting)))))

(define (finish-waited! d)
  "Signal any waiting programs that document D is done."
  (done! d))

(define (editor-forget! id)
  "Forget the program ID waiting or opening: it stopped waiting."
  (let ((cancel! (hash-table-ref/default %pending-opens id #f)))
    (hash-table-delete! %pending-opens id)
    (when cancel! (guard (e (#t #f)) (cancel!))))
  (let ((id-str (number->string id)))
    (set! %opened (remove (lambda (x) (eqv? x id)) %opened))
    (set! %failed (remove (lambda (x) (equal? (car x) id-str)) %failed))
    (set! %waiting (remove (lambda (w) (eqv? (car w) id)) %waiting))))

(define (editor-take-opened!)
  "Return the ids of async server opens that succeeded since last asked."
  (let ((ids %opened))
    (set! %opened '())
    ids))

(define (editor-take-failed!)
  "Return failed async server opens since last asked.
Each entry is an `(id-string message)` pair."
  (let ((xs %failed))
    (set! %failed '())
    xs))

(define (editor-take-done!)
  "Return the ids of the programs done waiting since last asked."
  (let ((ids %done))
    (set! %done '())
    ids))

(define-command (server-edit s n)
  "Be done with the file a program waits for: save it and kill its buffer."
  (let ((d (doc s))
        (b (current-buffer s)))
    (unless (waited? d) (error "No program waits for this buffer"))
    (if (document-dirty? d)
        (save-document-in-session!
         s d 'server-edit
         #:after (lambda ()
                   (if (document-dirty? d)
                       (when (memq s (attached-sessions))
                         (message! s "Wrote earlier revision; newer edits remain — run C-x # again to finish"))
                       (begin
                         (done! d)
                         (when (memq b (buffer-list))
                           (if (memq s (attached-sessions))
                               (drop-buffer! s b)
                               (forget-buffer! b)))))))
        (begin
          (done! d)
          (drop-buffer! s b)))))

(add-hook! 'buffer-killed 'server (lambda (s b) (done! (buffer-document b))))

;;; Evaluating for other programs

(define-option server-eval #t
  "Whether other programs may evaluate code in the editor: `techne eval`.
Only programs running as you reach the editor's socket, but those can
then do what the editor can, as with Emacs's server. *Server log* says
what they evaluated. User-scope only."
  #:type 'boolean
  #:user-only #t)

(define server-eval-max-chars (* 64 1024)
  "The most characters of a value, an error or output `techne eval` gets.
More is cut there, and says how much was cut.")

(define server-log-max-bytes (* 1024 1024) "The bytes *Server log* keeps, about.")

(define (clipped text max)
  (let ((n (string-length text)))
    (if (<= n max)
        text
        (string-append (substring text 0 max) "\n[" (number->string (- n max)) " more characters]"))))

;; The session of programs asking for one no frontend shows: made the
;; first time one asks, kept after, so that what one shows there the next
;; finds.
(define %detached #f)
(define (detached-session)
  (unless %detached
    (set! %detached (make-session-for-view (make-view (make-document "") "user") emacs-profile))
    (add-unseen-session! %detached))
  %detached)

;; The operation of the evaluation running, if one is: what a kill leaves
;; of it is ended by `editor-eval-ended!`.
(define %evaluating #f)

;; *Server log*'s document and the view it is written through.
(define %log #f)
(define (server-log! text)
  (unless %log
    (let ((d (make-document "")))
      (document-limit-history! d log-history)
      (set! %log (cons d (make-view d "server")))))
  (unless (buffer-named "*Server log*")
    (make-generated-buffer! "*Server log*" (car %log) 'log-mode))
  (append-bounded-output! %log text server-log-max-bytes))

(define (commented text)
  (string-join (map (lambda (l) (string-append ";; " l)) (string-split text "\n")) "\n"))

;; FRAMES, where a condition was raised, as lines: up to the evaluator's
;; own, the same for every evaluation.
(define (site-text frames)
  (let loop ((frames frames) (text ""))
    (if (or (null? frames) (string-prefix? "%eval-forms " (car frames)))
        text
        (loop (cdr frames) (string-append text "\n  in " (car frames))))))

(define (editor-eval! source module detached principal)
  "Evaluate SOURCE for another program, `techne eval`; say what came of it.
It is evaluated in MODULE, in the session used last or, with DETACHED (or
no frontend attached), in one no frontend shows, as an operation run for
PRINCIPAL. Return (\"value\" VALUE OUTPUT), VALUE written (\"\" for none),
or (\"error\" ERROR OUTPUT), ERROR with the frames it was raised in;
OUTPUT is what the code wrote. Each is cut at `server-eval-max-chars`.
*Server log* gets the code before it runs, and then what came of it."
  (unless (option #f 'server-eval)
    (error "Programs may not evaluate code in this editor: server-eval is off"))
  (let* ((shown (current-session))
         (s (if (or detached (not shown)) (detached-session) shown))
         (out (open-output-string))
         (logged (server-log! (string-append ";; " principal (if (eq? s shown) "" ", detached") ", in " module ":\n"
                                             source (if (string-suffix? "\n" source) "" "\n"))))
         (result
          (dynamic-wind
           (lambda ()
             (unless (eq? s shown)
               (set! %resume (list shown))
               (set-current-session! s)))
           (lambda ()
             (let ((op (start-operation! 'eval s #:principal principal)))
               (set! %evaluating op)
               (guard (e (#t (list "error" (string-append (error-text e) (site-text (raise-site e))))))
                 (let ((v (call-in-operation op (lambda () (parameterize ((current-output-port out))
                                                              (eval-source source module "<techne eval>"))))))
                   (list "value" (if (eq? v (if #f #f)) "" (call-with-output-string (lambda (p) (write v p)))))))))
           (lambda () (set! %evaluating #f) (resume!))))
         (text (clipped (cadr result) server-eval-max-chars)))
    (editor-eval-ended! (cond ((string=? (car result) "error") (string-append "error: " text))
                              ((string=? text "") "no value")
                              (else (string-append "=> " text))))
    (list (car result) text (clipped (get-output-string out) server-eval-max-chars))))

;; The session used last, (session), while an evaluation runs detached:
;; it is current again once that ends, however it ends.
(define %resume #f)
(define (resume!)
  (when %resume
    (set-current-session! (car %resume))
    (set! %resume #f)))

(define (editor-eval-ended! outcome)
  "Say in *Server log* that the evaluation logged last ended with OUTCOME.
The runtime says so too when it killed the evaluation; the session used
last is the current one again, and the operations it left are ended."
  (resume!)
  (when %evaluating
    (operation-abandon! %evaluating)
    (set! %evaluating #f))
  (server-log! (string-append (commented (clipped outcome 2000)) "\n\n")))
