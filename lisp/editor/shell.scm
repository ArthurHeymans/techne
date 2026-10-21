;;; Shell commands, as Emacs runs them: M-! waits for the command and
;;; shows its output, in the echo area when it is one line, else in
;;; *Shell Command Output*; M-& shows the output in *Async Shell Command*
;;; as it comes; M-| gives the region to the command as its input, and
;;; with C-u puts the output in its place. The commands run with sh in the
;;; focused buffer's directory, in a task, so the editor never waits for
;;; them: the region is replaced as it was when the command was given,
;;; moved past edits made meanwhile, and not at all when they touched it.
;;; Output buffers are read-only views; the task writes through a view of
;;; its own. They keep their text, but as logs (`log-history`), not every
;;; revision; M-! and M-| keep `shell-output-max` characters of output.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "minibuffer.scm")
(require "buffers.scm")

(provide shell-command async-shell-command shell-command-on-region run-shell! output-collector)

(define (shell-quote s)
  (string-append "'" (string-join (string-split s "'") "'\\''") "'"))

;; A buffer NAME for a command's output, in log-mode, emptied: (document
;; . writer).
(define (output-buffer name)
  (let* ((old (buffer-named name))
         (d (if old (buffer-document old) (buffer-document (make-generated-buffer! name (make-document "") 'log-mode)))))
    (document-limit-history! d log-history)
    (let ((w (make-view d "process")))
      (view-edit! w (list (list 0 (document-length d) "")) "new")
      (cons d w))))

(define (append-output! out text)
  (let ((len (document-length (car out))))
    (view-edit! (cdr out) (list (list len len text)) "new")))

(define (run-shell! s command dir input on-output on-exit)
  "Run COMMAND with sh in DIR, in a task of session S.
INPUT, a string or #f, is its input; its errors go with its output.
(ON-OUTPUT text) is called for each piece of output, then (ON-EXIT
status)."
  (spawn (lambda ()
           (guard (e (#t (message! s (string-append command ": " (error-text e)))))
             (call-with-process "sh" (list "-c" (string-append "cd " (shell-quote dir) " && { " command "\n} 2>&1"))
             (lambda (p)
               (if input (process-write p input) #f)
               (process-close-input p)
               (let loop ()
                 (let ((c (process-read p 'stdout)))
                   (unless (eof-object? c)
                     (on-output c)
                     (loop))))
               (on-exit (process-wait p))))))))

(define shell-output-max (* 8 1024 1024)
  "The most characters of output kept of a command waited for (M-!, M-|).
More is read and dropped, and the output is said to be truncated.")

(define (output-collector #:limit [limit shell-output-max])
  "Return what keeps a command's output, up to LIMIT characters.
It is a list of procedures: (add! text) keeps a piece, as much of it as
fits; (text) returns what was kept; (truncated?) whether more came."
  (let ((chunks '()) (size 0) (over #f))
    (list (lambda (text)
            (let* ((n (string-length text)) (room (- limit size)))
              (when (> n room) (set! over #t))
              (when (> (min n room) 0)
                (set! chunks (cons (if (> n room) (substring text 0 room) text) chunks))
                (set! size (+ size (min n room))))))
          (lambda () (apply string-append (reverse chunks)))
          (lambda () over))))

(define (truncation-text output)
  (if ((caddr output))
      (string-append " (output truncated at " (number->string shell-output-max) " characters)")
      ""))

(define (status-text status)
  (if (eqv? status 0) "" (string-append " (exit " (call-with-output-string (lambda (p) (display status p))) ")")))

(define (read-command s prompt run)
  (completing-read s prompt '() #:require-match #f #:accept (lambda (s c) (run s (candidate-text c)))))

;; Output of one line goes to the echo area, more to a buffer shown.
(define (show-output! s command input)
  (let ((output (output-collector)))
    (run-shell! s command (default-directory s) input
                (car output)
                (lambda (status)
                  (let* ((text ((cadr output)))
                         (trimmed (if (string-suffix? "\n" text) (substring text 0 (- (string-length text) 1)) text))
                         (status (string-append (status-text status) (truncation-text output))))
                    (cond ((string=? trimmed "")
                           (message! s (string-append "(Shell command finished with no output)" status)))
                          ((not (string-index trimmed #\newline)) (message! s (string-append trimmed status)))
                          (else
                           (let ((out (output-buffer "*Shell Command Output*")))
                             (append-output! out text)
                             (display-buffer! s (car out))
                             (message! s (string-append "Shell command finished" status))))))))))

(define-command (shell-command s n)
  "Run a shell command in the buffer's directory; show its output."
  (read-command s "Shell command: " (lambda (s command) (show-output! s command #f))))

(define-command (shell-command-on-region s n)
  "Run a shell command with the region as its input; show its output.
With a prefix argument, put the output in place of the region."
  (let* ((input (region-text s))
         (v (session-view s))
         (r (list-ref (view-ranges v) (view-primary v)))
         (region (list (min (car r) (cadr r)) (max (car r) (cadr r))))
         (revision (document-revision (view-document v)))
         (replace (current-prefix s)))
    (read-command s (if replace "Shell command on region, replacing it: " "Shell command on region: ")
                  (lambda (s command)
                    (if replace
                        (replace-with-output! s command input v revision region)
                        (show-output! s command input))))))

;; Run COMMAND on INPUT; its output replaces REGION of the text V showed at
;; REVISION, unless it was truncated.
(define (replace-with-output! s command input v revision region)
  (let ((output (output-collector)))
    (run-shell! s command (default-directory s) input
                (car output)
                (lambda (status)
                  (guard (e (#t (message! s (string-append "The region was not replaced. " (error-text e)))))
                    (when ((caddr output))
                      (error (string-append "The output is over " (number->string shell-output-max) " characters")))
                    (view-edit-at! v revision (list (list (car region) (cadr region) ((cadr output)))) "new")
                    (message! s (string-append "Shell command finished" (status-text status))))))))

(define-command (async-shell-command s n)
  "Run a shell command in the background.
Its output shows in *Async Shell Command* as it comes."
  (read-command s "Async shell command: "
                (lambda (s command)
                  (let ((out (output-buffer "*Async Shell Command*")))
                    (display-buffer! s (car out))
                    (run-shell! s command (default-directory s) #f
                                (lambda (text) (append-output! out text))
                                (lambda (status)
                                  (message! s (string-append "Shell command finished" (status-text status)))))))))
