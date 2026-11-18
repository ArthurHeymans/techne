;;; A local PTY and a screen per buffer. The existing process primitives
;;; own the child; all policy, parsing, projection and input live here.
(import (techne editor) (techne terminal))
(require "../session.scm")
(require "../terminal-render.scm")

(define-record-type terminal-session
  (%terminal screen projection directory process reader writer pending size status input pane-size)
  terminal-session?
  (screen screen) (projection projection) (directory directory)
  (process process process-set!) (reader reader reader-set!) (writer writer writer-set!)
  (pending pending pending-set!) (size size size-set!)
  (status status status-set!) (input input-mode input-mode-set!)
  (pane-size pane-size pane-size-set!))

(define (copying? state) (eq? (input-mode state) 'copy))

(define history-limit 2000)
(define (session-of b)
  (let ((state (and b (buffer-state b)))) (and (terminal-session? state) state)))

(define (follow! s b state)
  (unless (copying? state)
    (let* ((p (projection state)) (caret (terminal-caret p)) (start (terminal-live-start p)))
      (for-each
       (lambda (v)
         (when (document=? (view-document v) (buffer-document b))
           (unless (equal? (view-ranges v) (list (list caret caret)))
             (view-set-ranges! v (list (list caret caret)) 0))
           (unless (= (view-scroll v) start) (view-set-scroll! v start))))
       (shown-views s)))))

(define (render! s b state)
  (terminal-project! (projection state) (screen state))
  (let ((visible (or (copying? state) (terminal-mode? (screen state) 'cursor))))
    (unless (eq? (option b 'cursor-visible) visible) (set-option! 'cursor-visible visible #:buffer b)))
  (follow! s b state))

;; Frame dimensions are text-cell estimates in the window frontend. Subtract
;; the echo row before tiling, then the mode line and any right divider.
(define (geometry s)
  (let* ((place (list-ref (pane-places s) (session-focus s)))
         (x (car place)) (y (cadr place)) (right (+ x (caddr place))) (bottom (+ y (cadddr place)))
         (cols (or (sget s 'viewport-columns) 80)) (rows (- (or (sget s 'viewport-rows) 26) 1))
         (cells (lambda (f n) (exact (floor (+ (* f n) 0.5))))))
    (list (min 256 (max 1 (- (cells bottom rows) (cells y rows) 1)))
          (min 512 (max 1 (- (cells right cols) (cells x cols) (if (< right 0.9999) (or (sget s 'viewport-split-gap) 1) 0)))))))

(define (fit! s b state)
  (let* ((size (geometry s)) (t (screen state)) (p (process state)))
    ;; Keep a child-requested grid size until the pane itself changes.
    (unless (equal? size (pane-size state))
      (pane-size-set! state size)
      (when p (guard (e (#t #f)) (apply process-resize p size)))
      (apply terminal-resize! t size)
      (render! s b state))))

(define (fit-focused! s)
  (let* ((b (current-buffer s)) (state (session-of b)))
    (when (and state (process state) (not (copying? state))) (fit! s b state))))
(add-hook! 'viewport-changed 'terminal (lambda (s cols rows) (fit-focused! s)))
(add-hook! 'after-key 'terminal (lambda (s key) (unless (sget s 'transient) (fit-focused! s))))

;; One writer preserves ordering of keys, paste and query replies without
;; waiting on a full PTY from the input handler or during VT parsing.
(define (send! s state text)
  (when (process state)
    (let ((bytes (bytevector-length (string->utf8 text))))
      (if (> (+ bytes (size state)) 65536)
          (message! s "Terminal input queue full (64 KiB); input not sent")
          (begin
            (pending-set! state (cons text (pending state)))
            (size-set! state (+ (size state) bytes))
            (unless (writer state)
              (writer-set!
               state
               (without-operation
                (lambda ()
                  (spawn
                   (lambda ()
                     (guard (e ((not (cancelled-condition? e))
                                (when (process state) (message! s (error-text e)))))
                       (let loop ()
                         (let ((items (reverse (pending state))) (p (process state)))
                           (pending-set! state '()) (size-set! state 0)
                           (when (and p (pair? items))
                             (process-write p (apply string-append items)) (loop)))))
                     (writer-set! state #f))))))))))))

;; Semi-char leaves Ebb's editor chords and modified navigation to the
;; active profile. Once that profile starts a sequence, it gets the rest.
(define-option terminal-editor-keys
  '("C-c" "C-x" "C-\\" "C-g" "C-h" "C-u" "C-M-c" "M-x" "M-:" "M-!" "M-&")
  "Keys left to the editor profile in terminal semi-char mode."
  #:type '(list-of string))

(define (semi-editor-key? b key)
  (or (member key (option b 'terminal-editor-keys))
      (let ((i (string-index key #\<)))
        (and i (> i 0)
             (member (substring key i) '("<up>" "<down>" "<left>" "<right>" "<home>" "<end>"
                                         "<prior>" "<next>" "<insert>" "<delete>"))))))

(define (send-key! s b state key)
  (message! s #f)
  (let ((text (terminal-key (screen state) key)))
    (when text (fit! s b state) (send! s state text)))
  #t)

(define (terminal-input s key)
  (let* ((b (current-buffer s)) (state (session-of b)) (quote-for (sget s 'terminal-quote)))
    (when quote-for (sset! s 'terminal-quote #f))
    (and state (process state) (not (copying? state))
         (cond
           ((eq? quote-for b) (send-key! s b state key))
           ((eq? (input-mode state) 'char)
            (cond ((member key '("C-M-m" "C-M-RET" "M-RET")) (run-command s 'terminal-semi-char-mode 1) #t)
                  ((string=? key "<f12>") (run-command s 'terminal-copy-mode 1) #t)
                  (else (send-key! s b state key))))
           ((or (sget s 'isearch) (sget s 'prefix-arg)
                (pair? (sget s 'pending)) (pair? (sget s 'mode-pending))) #f)
           ((string=? key "C-q") (sset! s 'terminal-quote b) (message! s "Send key: ") #t)
           ((member key '("C-y" "S-<insert>" "C-S-v" "C-V" "S-C-v")) (run-command s 'terminal-yank 1) #t)
           ((string=? key "M-y") (run-command s 'terminal-yank-pop 1) #t)
           ((or (string=? key "<f12>") (semi-editor-key? b key)) #f)
           (else (send-key! s b state key))))))

(define terminal-keys
  '(("C-c" "terminal") ("C-c C-e" terminal-emacs-mode) ("C-c C-j" terminal-semi-char-mode)
    ("C-c M-d" terminal-char-mode) ("C-c C-c" terminal-interrupt) ("C-c C-k" terminal-kill-process)
    ("<f12>" terminal-copy-mode) ("M-x" execute-extended-command)))

(define-mode terminal-mode
  "An interactive local terminal, initially in semi-char mode.
Use \\[terminal-emacs-mode] to browse and copy, \\[terminal-semi-char-mode]
to resume input, or \\[terminal-char-mode] to pass most keys to the child."
  #:parent 'special-mode
  #:keys terminal-keys
  #:normal terminal-keys
  #:input terminal-input
  #:options '((line-numbers . #f) (truncate-lines . #t) (eob-marker . ""))
  #:directory (lambda (b) (let ((state (session-of b))) (and state (directory state))))
  #:layer (lambda (d from to)
            (let ((state (session-of (document-buffer d))))
              (if state (terminal-layers (projection state) from to) '()))))

(register-mode-line-segment!
 'terminal (lambda (b)
             (let ((state (session-of b)))
               (and state
                    (or (status state)
                        (let ((title (terminal-title (screen state)))
                              (mode (case (input-mode state) ((semi) "SEMI") ((char) "CHAR") (else "COPY"))))
                          (if (string=? title "") mode
                              (string-append mode " | " (substring title 0 (min 60 (string-length title)))))))))))

(define (live-state s)
  (let ((state (session-of (current-buffer s))))
    (if (and state (process state)) state (error "No live terminal"))))

(define (switch-input! s mode)
  (let ((state (live-state s)) (b (current-buffer s)))
    (sset! s 'terminal-quote #f)
    (input-mode-set! state mode)
    (unless (copying? state) (fit! s b state))
    (render! s b state)))

(define-command (terminal-emacs-mode s n)
  "Use the editor's normal keys to browse and copy terminal output."
  (switch-input! s 'copy))

(define-command (terminal-semi-char-mode s n)
  "Send most keys to the terminal, leaving editor shortcuts available."
  (switch-input! s 'semi))

(define-command (terminal-char-mode s n)
  "Send editor shortcuts to the terminal as well as ordinary keys."
  (switch-input! s 'char))

(define-command (terminal-copy-mode s n)
  "Toggle between browsing terminal output and semi-char input."
  (switch-input! s (if (copying? (live-state s)) 'semi 'copy)))

(define-command (terminal-interrupt s n)
  "Send a literal control-C to the foreground program on the PTY."
  (send! s (live-state s) (string #\x03)))

(define-command (terminal-kill-process s n)
  "Stop the terminal's process, leaving the output buffer readable."
  (process-kill (process (live-state s))))

(define (paste-kill! s next?)
  (let* ((state (live-state s)) (ring (kill-ring s)) (prior (sget s 'terminal-yank))
         (id (document-id (buffer-document (current-buffer s))))
         (i (if next? (+ 1 (if (and prior (= id (car prior)) (eq? ring (cadr prior))) (caddr prior) 0)) 0)))
    (when (null? ring) (error "The kill ring is empty"))
    (let ((i (modulo i (length ring))))
      (send! s state (terminal-paste (screen state) (car (list-ref ring i))))
      (sset! s 'terminal-yank (list id ring i)))))

(define-command (terminal-yank s n)
  "Send the latest kill or clipboard text to this terminal, as paste."
  (paste-kill! s #f))

(define-command (terminal-yank-pop s n)
  "Paste the next older kill without erasing any text in the child."
  (paste-kill! s #t))

(define (read-output! s b state p)
  (let ((t (screen state)))
    (let read ()
      (let ((text (process-read p 'stdout)))
        (if (eof-object? text) (process-wait p)
            (begin
              (let chunk ((from 0))
                (let ((to (min (+ from 4096) (string-length text))))
                  (terminal-feed! t text #:start from #:end to)
                  (render! s b state)
                  ;; Ready output must not monopolize an editor turn.
                  (sleep 1)
                  (when (< to (string-length text)) (chunk to))))
              (read)))))))

(define-option terminal-close-on-exit #t
  "Whether to close an interactive terminal after its shell exits zero.
Buffers from the terminal-command prompt and unsuccessful exits stay open."
  #:type 'boolean)

(define (start-terminal! s dir program args #:interactive [interactive #f])
  (when (or (not (string? dir)) (remote-path? dir)) (error "Terminal needs a local directory"))
  (let* ((name (let loop ((i 1))
                 (let ((name (if (= i 1) "*terminal*" (string-append "*terminal " (number->string i) "*"))))
                   (if (buffer-named name) (loop (+ i 1)) name))))
         (d (make-document "")) (state #f) (size (geometry s))
         (t (make-terminal (car size) (cadr size) #:history history-limit
                           #:reply (lambda (text) (send! s state text))
                           #:resize (lambda (rows cols)
                                      (let ((p (process state)))
                                        (when p (process-resize p rows cols))))))
         (projection (make-terminal-projection d history-limit)))
    (set! state (%terminal t projection dir #f #f #f '() 0 "Starting" 'semi size))
    (let ((b (make-generated-buffer! name d 'terminal-mode #:state state)))
      (document-limit-history! d log-history)
      (show-document! s d)
      (render! s b state)
      (note-effect! 'external)
      (reader-set!
       state
       (without-operation
        (lambda ()
          (spawn
           (lambda ()
              (let ((exit-status
                     (guard (e ((not (cancelled-condition? e)) (error-text e)))
                       ;; The host initially opens every PTY at 24x80. Set
                       ;; the pane size before the child's first instruction.
                       (call-with-process "sh"
                         (append (list "-c" "stty rows \"$1\" cols \"$2\" && shift 2 && exec \"$@\"" "techne-terminal"
                                       (number->string (terminal-rows t)) (number->string (terminal-columns t)) program)
                                 args)
                         (lambda (p)
                           (process-set! state p) (status-set! state #f)
                           (read-output! s b state p))
                         #:pty #t #:directory dir
                         ;; Inherited dimensions describe the parent PTY.
                         #:environment '(("TERM" . "xterm-256color") ("COLORTERM" . "truecolor")
                                         ("COLUMNS" . #f) ("LINES" . #f))))))
                (process-set! state #f)
                (pending-set! state '()) (size-set! state 0)
                (when (writer state) (task-cancel (writer state)) (writer-set! state #f))
                (reader-set! state #f)
                (if (and interactive (eqv? exit-status 0) (option b 'terminal-close-on-exit)
                         (any (lambda (other) (not (eq? b other))) (buffer-list)))
                    (drop-buffer! (or (current-session) s) b)
                    (begin
                      (status-set! state (cond ((integer? exit-status) (string-append "Exit " (number->string exit-status)))
                                               ((string? exit-status) exit-status) (else "Child exited")))
                      (input-mode-set! state 'copy)
                      (render! s b state)))))))))
      b)))

(add-hook! 'buffer-killed 'terminal
           (lambda (s b)
             (let ((state (session-of b)))
               (when state
                 (process-set! state #f)
                 (when (writer state) (task-cancel (writer state)))
                 (when (and (reader state) (not (task-done? (reader state)))) (task-cancel (reader state)))))))

(define-command (terminal s n)
  "Open a new terminal running the user's shell.
Start it in the current buffer's local directory."
  (let ((shell (get-environment-variable "SHELL")))
    (start-terminal! s (default-directory s) (if (and shell (not (string=? shell ""))) shell "sh") '() #:interactive #t)))

(define-command (terminal-command s n)
  "Read a shell command and run it on a new terminal."
  (let ((dir (default-directory s)))
    (completing-read s "Terminal command: " '() #:require-match #f #:history 'terminal-command
                     #:accept (lambda (s c) (start-terminal! s dir "sh" (list "-c" (candidate-text c)))))))
