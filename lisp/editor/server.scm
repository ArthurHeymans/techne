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
(require "shell.scm")

(provide editor-open! editor-take-done! editor-forget! waited? server-edit editor-eval! editor-eval-ended! server-eval-max-chars)

;; The programs waiting, (id . document), and those done waiting, by id,
;; for the runtime to tell them.
(define %waiting '())
(define %done '())

(define (editor-open! path id)
  "Show the file at PATH in the current session, for a program.
With ID, the program waits until the file is done with, and ID is
among those `editor-take-done!` returns then. A document of the file
without unsaved edits is read again: the program may have written the
file since it was last shown, as Git writes COMMIT_EDITMSG."
  (let* ((s (or (current-session) (error "No frontend is attached")))
         (d (file-document path)))
    (unless (document-dirty? d) (reload-document! d))
    (show-document! s d)
    (when id (set! %waiting (cons (cons id d) %waiting)))
    (message! s (cond ((not id) (string-append "Opened " (absolute-path path)))
                      ((eq? (profile-name (sget s 'profile)) 'modal) "When done with the file, type :wq")
                      (else (substitute-command-keys s "When done with the file, type \\[server-edit]"))))))

(define (waited? d)
  "Return #t if a program waits until the document D is done with."
  (any (lambda (w) (document=? (cdr w) d)) %waiting))

(define (done! d)
  (let ((waiting (filter (lambda (w) (document=? (cdr w) d)) %waiting)))
    (set! %waiting (remove (lambda (w) (memq w waiting)) %waiting))
    (set! %done (append %done (map car waiting)))))

(define (editor-forget! id)
  "Forget the program ID waiting: it stopped waiting."
  (set! %waiting (remove (lambda (w) (eqv? (car w) id)) %waiting)))

(define (editor-take-done!)
  "Return the ids of the programs done waiting since last asked."
  (let ((ids %done))
    (set! %done '())
    ids))

(define-command (server-edit s n)
  "Be done with the file a program waits for: save it and kill its buffer."
  (let ((d (doc s)))
    (unless (waited? d) (error "No program waits for this buffer"))
    (when (document-dirty? d) (save-document! d))
    (done! d)
    (drop-buffer! s (current-buffer s))))

(add-hook! 'buffer-killed 'server (lambda (s b) (done! (buffer-document b))))

;;; Evaluating for other programs

(define-option server-eval #t
  "Whether other programs may evaluate code in the editor: `techne eval`.
Only programs running as you reach the editor's socket, but those can
then do what the editor can, as with Emacs's server. *Server log* says
what they evaluated."
  #:type 'boolean)

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
