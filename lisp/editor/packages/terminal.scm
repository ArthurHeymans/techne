;;; A local PTY and a screen per buffer. The existing process primitives
;;; own the child; all policy, parsing, projection and input live here.
(import (techne editor) (techne terminal))
(require "../session.scm")
(require "../remote.scm")
(require "../operations.scm")
(require "../terminal-render.scm")
(require "../terminal-integration.scm")
(require "../terminal-graphics.scm")

(define-record-type terminal-session
  (%terminal screen projection directory process reader drawer dirty writer pending size status input pane-size graphics password-probe)
  terminal-session?
  (screen screen) (projection projection) (directory directory directory-set!)
  (process process process-set!) (reader reader reader-set!)
  (drawer drawer drawer-set!) (dirty draw-dirty? draw-dirty-set!)
  (writer writer writer-set!)
  (pending pending pending-set!) (size size size-set!)
  (status status status-set!) (input input-mode input-mode-set!)
  (pane-size pane-size pane-size-set!) (graphics graphics) (password-probe password-probe password-probe-set!))

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

(define (cancel-draw! state)
  (let ((task (drawer state)))
    (drawer-set! state #f)
    (when (and task (not (task-done? task))) (task-cancel task))))

(define (render! s b state)
  (cancel-draw! state)
  (terminal-project! (projection state) (screen state))
  (let ((visible (or (copying? state) (terminal-mode? (screen state) 'cursor))))
    (unless (eq? (option b 'cursor-visible) visible) (set-option! 'cursor-visible visible #:buffer b)))
  (let ((report (and (process state) (not (copying? state)) (terminal-mouse? (screen state)) #t)))
    (unless (eq? (option b 'report-pointer) report) (set-option! 'report-pointer report #:buffer b)))
  (follow! s b state)
  (draw-dirty-set! state #f))

;; One pending redraw per terminal, measured from the first unshown change.
;; Later chunks cannot postpone it. Both tasks share the editor VM, so the
;; drawer sees a consistent grid between parser slices, without IPC.
(define redraw-delay-ms 16)
(define (schedule-draw! s b state)
  ;; A draw can fail after its timer has fired. Keep the frame dirty
  ;; until projection succeeds, independently of the timer's lifetime.
  (draw-dirty-set! state #t)
  (unless (drawer state)
    (drawer-set! state
                 (spawn (lambda ()
                          (sleep redraw-delay-ms)
                          (drawer-set! state #f)
                          (guard (e ((not (cancelled-condition? e)) (message! s (error-text e))))
                            (render! s b state)
                            (check-terminal-password! s b state)))))))

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
    (let* ((input (if (bytevector? text) text (string->utf8 text))) (bytes (bytevector-length input)))
      (if (> (+ bytes (size state)) 65536)
          (message! s "Terminal input queue full (64 KiB); input not sent")
          (begin
            (pending-set! state (cons input (pending state)))
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
                             (process-write p (apply bytevector-append items)) (loop)))))
                     (writer-set! state #f))))))))))))

(add-hook! 'frontend-pointer 'terminal
  (lambda (s view col row code release)
    (let* ((b (document-buffer (view-document view))) (state (session-of b)))
      (when (and state (process state) (not (copying? state)))
        (let ((bytes (terminal-pointer (screen state) col row code release)))
          (when bytes (send! s state bytes)))))))
(add-hook! 'frontend-focus 'terminal
  (lambda (s focused)
    (let ((state (session-of (current-buffer s))))
      (when (and state (process state) (not (copying? state)))
        (let ((bytes (terminal-focus (screen state) focused)))
          (when bytes (send! s state bytes)))))))

;; Semi-char leaves Ebb's editor chords and modified navigation to the
;; active profile. Once that profile starts a sequence, it gets the rest.
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
    ("C-c C-p" terminal-previous-prompt) ("C-c C-n" terminal-next-prompt)
    ("C-c C-w" terminal-send-password)
    ("C-c M-p" terminal-previous-link) ("C-c M-n" terminal-next-link)
    ("RET" terminal-open-link)
    ("<f12>" terminal-copy-mode) ("M-x" execute-extended-command)))

(define-mode terminal-mode
  "An interactive terminal, initially in semi-char mode.
Use \\[terminal-emacs-mode] to browse and copy, \\[terminal-semi-char-mode]
to resume input, or \\[terminal-char-mode] to pass most keys to the child."
  #:parent 'special-mode
  #:keys terminal-keys
  #:normal terminal-keys
  #:input terminal-input
  #:options '((line-numbers . #f) (truncate-lines . #t) (eob-marker . ""))
  #:directory (lambda (b) (let ((state (session-of b))) (and state (directory state))))
  #:projections (lambda (d from to)
                  (let ((state (session-of (document-buffer d))))
                    (if state (terminal-graphics-projections (graphics state) (screen state) (projection state)) '())))
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

(define parse-slice-chars 4096)
(define (read-output! s b state p)
  (define (parse text from budget)
    (let* ((end (string-length text)) (to (min (+ from budget) end))
           (left (- budget (- to from))))
      (terminal-feed! (screen state) text #:start from #:end to)
      (schedule-draw! s b state)
      ;; Count across reads: small writes need not sleep on every read,
      ;; but a flood cannot monopolize the VM.
      (when (zero? left) (sleep 1) (set! left parse-slice-chars))
      (if (= to end) left (parse text to left))))
  (let read ((budget parse-slice-chars))
    (let ((text (process-read p 'stdout)))
      (if (eof-object? text)
          (begin
            ;; Publish the final state before leaving live input mode.
            (when (draw-dirty? state) (render! s b state))
            (process-wait p))
          (read (parse text 0 budget))))))

(define (check-terminal-password! s b state)
  (when (and (process state) (not (copying? state)) (option b 'terminal-detect-passwords)
             (not (eq? (password-probe state) 'checking)))
    (let* ((p (process state)) (seen (password-probe state)))
      (password-probe-set! state 'checking)
      (spawn
       (lambda ()
         (let* ((hidden (guard (e ((not (cancelled-condition? e)) #f)) (process-password-mode? p)))
                (session (or (current-session) s))
                (offer (and hidden (not (eq? seen 'seen)) (eq? p (process state))
                            (not (copying? state)) (eq? (current-buffer session) b)
                            (not (sget session 'transient)) (not (sget session 'private-key)))))
           (password-probe-set! state (and hidden (or offer (eq? seen 'seen)) 'seen))
           (when offer (run-command session 'terminal-send-password 1))))))))

;; Do not put the answer in a document, command argument, or after-key
;; hook. The user invokes this only once the child has turned off echo.
(define-command (terminal-send-password s n)
  "Send a hidden answer to a terminal program waiting for a password.
Press Return to send it; Escape or control-G cancels it."
  (let* ((state (live-state s)) (b (current-buffer s)) (p (process state))
         (secret (make-string 1024 #\space)) (length 0) (full #f)
         (token (list 'terminal-password)))
    (define (close!)
      (string-fill! secret #\space)
      (set! length 0)
      (sset! s 'private-key #f)
      (sset! s 'terminal-password #f)
      (message! s #f)
      (scope-disown! token))
    (define (input key)
      (guard (e (#t (close!) (message! s "Terminal password not sent")))
        (cond ((or (not (eq? (current-buffer s) b)) (not (eq? (process state) p))) (close!))
              ((member key '("C-g" "ESC")) (close!))
              ((string=? key "RET")
               (if full (begin (close!) (message! s "Terminal password too long; not sent"))
                   (let ((answer (string-append (substring secret 0 length) "\r")))
                     (close!) (send! s state answer))))
              ((string=? key "DEL")
               (when (> length 0) (set! length (- length 1)) (string-set! secret length #\space)))
              ((or (string=? key "SPC") (= (string-length key) 1))
               (let ((c (if (string=? key "SPC") #\space (string-ref key 0))))
                 (when (and (>= (char->integer c) 32) (not (<= 127 (char->integer c) 159)))
                   (if (= length 1024) (set! full #t)
                       (begin (string-set! secret length c) (set! length (+ length 1))))))))))
    (sset! s 'private-key (cons input close!))
    (sset! s 'terminal-password (cons state close!))
    (scope-own! token (lambda (ignored) (close!)))
    (message! s "Terminal password (hidden): ")))

(define (close-terminal-passwords! s state)
  (for-each (lambda (session)
              (let ((prompt (sget session 'terminal-password)))
                (when (and prompt (eq? (car prompt) state)) ((cdr prompt)))))
            (cons s (remove (lambda (session) (eq? session s)) (attached-sessions)))))

(define (goto-prompt! s direction)
  (let* ((b (current-buffer s)) (state (session-of b)))
    (unless state (error "Not a terminal buffer"))
    (when (draw-dirty? state) (render! s b state))
    (let ((pos (terminal-prompt-position (projection state) (point s) direction)))
      (if pos
          (begin (input-mode-set! state 'copy) (render! s b state) (move! s (lambda (old) pos)))
          (message! s "No more terminal prompts")))))

(define-command (terminal-previous-prompt s n)
  "Enter copy mode at the preceding shell prompt."
  (goto-prompt! s -1))

(define-command (terminal-next-prompt s n)
  "Enter copy mode at the next shell prompt."
  (goto-prompt! s 1))

(define (find-terminal-link s direction)
  (let* ((b (current-buffer s)) (state (session-of b)))
    (unless state (error "Not a terminal buffer"))
    (when (draw-dirty? state) (render! s b state))
    (terminal-link-position (projection state) (point s) direction)))

(define (goto-link! s direction)
  (let ((found (find-terminal-link s direction)))
    (if found
        (let* ((b (current-buffer s)) (state (session-of b)))
          (input-mode-set! state 'copy) (render! s b state)
          (move! s (lambda (old) (car found))) (message! s (cdr found)))
        (message! s "No more terminal hyperlinks"))))

(define-command (terminal-previous-link s n)
  "Move to the previous hyperlink in terminal output."
  (goto-link! s -1))

(define-command (terminal-next-link s n)
  "Move to the next hyperlink in terminal output."
  (goto-link! s 1))

(define-command (terminal-open-link s n)
  "Open the HTTP or HTTPS terminal hyperlink at point."
  (let ((found (find-terminal-link s 0)))
    (if found
        (begin (note-effect! 'external)
               (spawn (lambda ()
                        (guard (e (#t (message! s "Could not open terminal hyperlink")))
                          (call-with-process "xdg-open" (list (cdr found)) process-wait
                                             #:stdin 'null #:stdout 'null #:stderr 'null)))))
        (message! s "No terminal hyperlink at point"))))

;; The node owns the remote PTY; the editor keeps its grid locally.
(define (call-with-terminal-pty dir program args t read)
  (let* ((dir (if (remote-path? dir) (remote-absolute-path dir) dir))
         (node (location-endpoint dir))
         (facts (and node (node-facts node)))
         (host (if facts (cdr (assq 'host facts))
                   (guard (e (#t #f)) (string-trim (file->string "/proc/sys/kernel/hostname")))))
         (shell (and (not program) facts (cdr (assq 'shell facts))))
         (program (or program (and shell (not (string=? shell "")) shell) "/bin/sh"))
         (args (if (and shell (member (file-name program) '("bash" "zsh" "fish"))) (cons "-l" args) args))
         (command (append (list "-c" "unset COLUMNS LINES; stty rows \"$1\" cols \"$2\" && shift 2 && exec \"$@\""
                                "techne-terminal" (number->string (terminal-rows t))
                                (number->string (terminal-columns t)) program)
                          args)))
    (call-with-process "sh" command (lambda (p) (read p host)) #:pty #t #:node node #:directory (location-path dir)
                       #:environment '(("TERM" . "xterm-256color") ("COLORTERM" . "truecolor") ("TERM_PROGRAM" . "techne")))))

(define (start-terminal! s dir program args #:interactive [interactive #f])
  (let* ((name (let loop ((i 1))
                 (let ((name (if (= i 1) "*terminal*" (string-append "*terminal " (number->string i) "*"))))
                   (if (buffer-named name) (loop (+ i 1)) name))))
         (d (make-document "")) (state #f) (size (geometry s)) (host #f)
         (t (make-terminal (car size) (cadr size) #:history history-limit #:reflow #t
                           #:reply (lambda (text) (send! s state text))
                           #:event (lambda (kind value)
                                      (if (eq? kind 'directory)
                                          (let ((cwd (terminal-directory-uri dir host value)))
                                            (when cwd (directory-set! state cwd)))
                                          (terminal-graphics-event! (graphics state) (screen state) kind value
                                                                    (lambda (answer) (send! s state answer)))))
                           #:resize (lambda (rows cols)
                                      (let ((p (process state)))
                                        (when p (process-resize p rows cols))))))
         (projection (make-terminal-projection d history-limit)))
    (set! state (%terminal t projection dir #f #f #f #f #f '() 0 "Starting" 'semi size (make-terminal-graphics) #f))
    (let ((b (make-generated-buffer! name d 'terminal-mode #:state state)))
      ;; The buffer belongs to the editor; the PTY, input handlers and grid
      ;; belong to this generation. Preserve the last projected text, not
      ;; the dead generation, when the package unloads or is replaced.
      (scope-own! state
        (lambda (_)
          (when (eq? (buffer-state b) state)
            (close-terminal-passwords! s state)
            (for-each (lambda (session)
                        (when (eq? (sget session 'terminal-quote) b) (sset! session 'terminal-quote #f)))
                      (attached-sessions))
            (process-set! state #f)
            (set-buffer-state! b #f)
            (set-buffer-mode! b 'special-mode)
            (unset-option! 'report-pointer #:buffer b)
            (unset-option! 'cursor-visible #:buffer b))))
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
                        (call-with-terminal-pty dir program args t
                          (lambda (p endpoint-host)
                            (set! host endpoint-host)
                            (process-set! state p) (status-set! state #f)
                            (read-output! s b state p))))))
                (process-set! state #f)
                (pending-set! state '()) (size-set! state 0)
                (when (writer state) (task-cancel (writer state)) (writer-set! state #f))
                (reader-set! state #f)
                 (close-terminal-passwords! s state)
                 (cancel-draw! state)
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
                 (scope-disown! state)
                 (process-set! state #f)
                 (close-terminal-passwords! s state)
                 (when (writer state) (task-cancel (writer state)))
                 (cancel-draw! state)
                  (when (and (reader state) (not (task-done? (reader state)))) (task-cancel (reader state)))))))

(define-command (terminal s n)
  "Open a new terminal running the user's shell.
Start it on the machine of the current buffer's directory."
  (let* ((dir (default-directory s)) (shell (get-environment-variable "SHELL"))
         (program (and (not (remote-path? dir)) (if (and shell (not (string=? shell ""))) shell "sh"))))
    (start-terminal! s dir program '() #:interactive #t)))

(define-command (terminal-serial s n)
  "Open a local serial console using socat at the configured baud rate."
  (read-file-name s "Serial device: " "/dev/"
    (lambda (s device)
      (unless (and (string-prefix? "/dev/" device)
                   (every (lambda (c) (or (char-alphabetic? c) (char-numeric? c) (memv c '(#\/ #\- #\_ #\.))))
                          (string->list device))
                   (not (member ".." (string-split device "/"))))
        (error "Expected a local serial device under /dev"))
      (let ((baud (number->string (option (current-buffer s) 'terminal-serial-baud))))
        (start-terminal! s "/dev/" "socat"
          (list "STDIO,rawer,echo=0" (string-append "OPEN:" device ",rawer,echo=0,b" baud)))))))

(define-command (terminal-command s n)
  "Read a shell command and run it on a new terminal."
  (let ((dir (default-directory s)))
    (completing-read s "Terminal command: " '() #:require-match #f #:history 'terminal-command
                     #:accept (lambda (s c) (start-terminal! s dir "sh" (list "-c" (candidate-text c)))))))
