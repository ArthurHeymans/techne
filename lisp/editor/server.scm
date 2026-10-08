;;; Files other programs open in the running editor, as with Emacs's
;;; server and emacsclient: `techne --open FILE` shows the file in the
;;; current session; with `--wait`, the program waits until the file is
;;; done with, so the editor serves as $EDITOR. It is done with when its
;;; buffer is killed, by \[server-edit] (which saves it first), by the
;;; quit command (:wq and :q in the modal profile), or by \[kill-buffer].

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "files.scm")
(require "buffers.scm")
(require "help.scm")

(provide editor-open! editor-take-done! editor-forget! waited? server-edit)

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
    (unless (document-dirty? d) (document-reload! d))
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
    (when (document-dirty? d) (document-save! d))
    (done! d)
    (drop-buffer! s (current-buffer s))))

(add-hook! 'buffer-killed 'server (lambda (s b) (done! (buffer-document b))))
