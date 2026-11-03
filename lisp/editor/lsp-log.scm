;;; What went between the editor and each language server: as Eglot's
;;; events buffer, M-x lsp-events.
;;;
;;; A log per server and project root, kept across its restarts (a mark
;;; says where each run started and ended): the messages each way as
;;; they were written and read, the JSON as it went, and what the server
;;; wrote to stderr. It is in memory only and bounded, the oldest going
;;; first: `lsp-log-max-entries' entries, `lsp-log-max-bytes' bytes, each
;;; cut to `lsp-log-max-entry'. What a server last wrote to stderr is
;;; kept apart, and kept even with `lsp-log-events' off: why it ended,
;;; when it does.
;;;
;;; Appending is a native's, whole: a task interrupted anywhere never
;;; leaves half an entry. Shown, a log is rendered once, then followed —
;;; what comes is added a quarter second later, together.

(require "session.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "shell.scm")

(provide lsp-log-max-entries lsp-log-max-bytes lsp-log-max-entry
         lsp-log lsp-log? lsp-log-name lsp-logs lsp-log-event! lsp-log-mark!
         lsp-log-stderr-count lsp-log-stderr-tail lsp-log-text
         show-lsp-log!)

(define-option lsp-log-events #t
  "Whether what goes between the editor and language servers is logged.
The messages, as \\[lsp-events] shows them: in memory, bounded. What a
server writes to stderr is kept anyway, its last lines: why it ended."
  #:type 'boolean)

(define lsp-log-max-entries 4000 "The most events a language server's log keeps.")
(define lsp-log-max-bytes (* 2 1024 1024) "The most bytes of text a language server's log keeps.")
(define lsp-log-max-entry 4096
  "The most bytes of one message a language server's log keeps.
A longer one is cut there, and says how long it was.")

;; The stderr kept apart: its last chunks, as the server wrote them,
;; each cut to its end (where why it ended is).
(define stderr-max-entries 200)
(define stderr-max-bytes (* 64 1024))

(define-record-type lsp-log
  (%make-log name events stderr follower)
  lsp-log?
  ;; As the log's buffer is called: the server's and the root's.
  (name lsp-log-name)
  (events log-events)
  (stderr log-stderr)
  ;; The channel waking the task following the log in its buffer, or #f.
  (follower log-follower set-log-follower!))

;; Logs by key, and the keys newest first.
(define %logs (make-hash-table))
(define %log-order '())

(define (lsp-log key name)
  "Return the log under KEY, made named NAME the first time.
KEY is a server's and root's: the log outlives each of its sessions."
  (or (hash-table-ref/default %logs key #f)
      (let ((log (%make-log name
                            (make-event-ring lsp-log-max-entries lsp-log-max-bytes lsp-log-max-entry)
                            (make-event-ring stderr-max-entries stderr-max-bytes lsp-log-max-entry)
                            #f)))
        (hash-table-set! %logs key log)
        (set! %log-order (cons key %log-order))
        log)))

(define (lsp-logs)
  "Return every language server's log kept, newest first."
  (map (lambda (key) (hash-table-ref %logs key)) %log-order))

(define (lsp-log-event! log direction text)
  "Add to LOG the event TEXT going DIRECTION: out, in or err.
Stderr's is kept whether events are logged or not. It never waits."
  (when (eq? direction 'err)
    (event-ring-push! (log-stderr log) "err"
                      (if (> (string-length text) lsp-log-max-entry)
                          (substring text (- (string-length text) lsp-log-max-entry) (string-length text))
                          text)))
  (when (option #f 'lsp-log-events)
    (event-ring-push! (log-events log) (symbol->string direction) text)
    (wake-follower! log)))

(define (lsp-log-mark! log text)
  "Add to LOG a line TEXT of its own: a run started, ended."
  (event-ring-push! (log-events log) "mark" text)
  (wake-follower! log))

(define (wake-follower! log)
  (let ((ch (log-follower log)))
    ;; Full means woken already: not waited for.
    (when ch (guard (e (#t #f)) (select (send ch #t) (timeout 0 #f))))))

(define (lsp-log-stderr-count log)
  "Return the number LOG's next chunk of stderr takes.
Given to `lsp-log-stderr-tail', it leaves out what came before."
  (car (event-ring-count (log-stderr log))))

(define (lsp-log-stderr-tail log [since 0])
  "Return the last line LOG's server wrote to stderr, or #f.
Only what it wrote from the chunk numbered SINCE on counts: a run's own."
  (let loop ((events (reverse (event-ring-since (log-stderr log) since))))
    (and (pair? events)
         (let ((lines (filter (lambda (l) (not (string=? (string-trim l) "")))
                              (string-split (list-ref (car events) 3) "\n"))))
           (if (null? lines) (loop (cdr events)) (string-trim (last lines)))))))

;; One event as a line (or lines, stderr's): when, which way, what.
(define (event-line e)
  (let* ((ms (list-ref e 1)) (direction (list-ref e 2)) (text (list-ref e 3)) (length (list-ref e 4))
         (time (string-append (number->string (quotient ms 1000)) "."
                              (let ((r (number->string (remainder ms 1000))))
                                (string-append (make-string (- 3 (string-length r)) #\0) r))))
         (cut (if (> length (bytevector-length (string->utf8 text)))
                  (string-append " […" (number->string length) " bytes]")
                  "")))
    (cond ((string=? direction "mark") (string-append "---- " time " " text " ----\n"))
          ((string=? direction "err")
           (string-append (string-join (map (lambda (l) (string-append time " stderr " l))
                                            (string-split (string-trim-right text) "\n"))
                                       "\n")
                          cut "\n"))
          (else (string-append time (if (string=? direction "out") " --> " " <-- ") text cut "\n")))))

(define (lsp-log-text log [since 0])
  "Return LOG's events numbered SINCE and after, as text.
Two values: the text, a line an event (a dropped run says so), and the
number the next event will take."
  ;; The next number is the events' own: one pushed between reading
  ;; them and counting would be skipped.
  (let* ((events (event-ring-since (log-events log) since))
         (gap (if (and (pair? events) (> (car (car events)) since))
                  (string-append "[" (number->string (- (car (car events)) since)) " earlier events dropped]\n")
                  "")))
    (values (apply string-append gap (map event-line events))
            (if (pair? events) (+ 1 (car (last events))) since))))

(define (show-lsp-log! s log)
  "Show LOG in its buffer in session S, following it as it grows."
  (let* ((name (string-append "*lsp-events " (lsp-log-name log) "*"))
         (b (make-generated-buffer! name (make-document "") 'log-mode))
         (d (buffer-document b))
         (out (cons d (make-view d "process"))))
    (document-limit-history! d log-history)
    (let-values (((text next) (lsp-log-text log 0)))
      (append-bounded-output! out text lsp-log-max-bytes)
      (follow! log b out next))
    (display-buffer! s d)))

;; Follow LOG into the buffer B, OUT its (document . writer): woken by
;; what comes, a quarter second later all of it is added. One follower a
;; log: a newer one ends the older. It ends with its buffer.
(define (follow! log b out next)
  (let ((old (log-follower log)) (ch (make-channel 1)))
    (when old (guard (e (#t #f)) (channel-close old)))
    (set-log-follower! log ch)
    (spawn (lambda ()
             (let loop ((next next))
               (let ((woken (channel-recv ch)))
                 (cond ((or (eof-object? woken) (not (memq b (buffer-list))))
                        (when (eq? (log-follower log) ch) (set-log-follower! log #f))
                        (guard (e (#t #f)) (channel-close ch)))
                       (else
                        (sleep 250)
                        (let-values (((text next) (lsp-log-text log next)))
                          (unless (string=? text "")
                            (guard (e (#t #f)) (append-bounded-output! out text lsp-log-max-bytes)))
                          (loop next))))))))))
