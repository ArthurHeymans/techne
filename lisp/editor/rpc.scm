;;; JSON-RPC over a process's pipes (PLAN.md, the language step's "JSON-RPC
;;; over streams"): what the language server client stands on, and what a
;;; debugger's client (DAP is the same framing with other verbs) or MCP
;;; would.
;;;
;;; A connection is a process, a framer (Rust reads the Content-Length
;;; headers, `techne_editor::rpc`), a writer task and a reader task. The
;;; reader waits in a task of its own: nothing is polled. A request waits
;;; on a channel its answer is sent to, with `select`'s timeout, so the
;;; waiting is that task's and the editor goes on without it. Whatever
;;; happens to a request's waiting — an answer, no answer, a failed send,
;;; this task cancelled — the waiter is taken out of the table.
;;;
;;; The reader never waits on the writer: a request of the peer's is
;;; answered from a task of its own, and frames to write queue without
;;; bound. A reader that waited for its answer to be written could wait on
;;; a peer that is itself waiting, blocked on a full pipe, to be read.
;;;
;;; Nothing here knows LSP: a method is a string, params and results are
;;; JSON values, and the caller says what to do with a notification
;;; (ON-NOTE) and with a request of the peer's (ON-ASK).

(provide rpc-connect rpc? rpc-request! rpc-notify! rpc-reply! rpc-close! rpc-closed? rpc-process rpc-waiting
         rpc-unsupported rpc-timeout)

(define-record-type rpc
  (%make-rpc process framer pending out notes closed on-note on-ask on-dead)
  rpc?
  (process rpc-process)
  ;; It reads the frames, and hands out request ids (one native call
  ;; each: tasks are preempted anywhere in Lisp).
  (framer rpc-framer)
  ;; The requests waiting: id -> the channel their answer is sent on.
  (pending rpc-pending)
  ;; Frames to write, in order: the writer task's.
  (out rpc-out)
  ;; Notifications to hand to ON-NOTE, in order: a task of their own
  ;; does, so a handler that asks the peer something is not waiting on
  ;; the reader that would bring the answer.
  (notes rpc-notes)
  ;; #t once the peer is gone or spoke out of frame.
  (closed rpc-closed? set-rpc-closed!)
  (on-note rpc-on-note)
  (on-ask rpc-on-ask)
  (on-dead rpc-on-dead))

(define rpc-timeout 30000
  "How long a request waits for its answer, in milliseconds.
Starting a server and its first answer are the slow ones.")

(define rpc-unsupported (list 'unsupported)
  "What an ON-ASK handler returns for a request it does not serve.
The peer is answered with JSON-RPC's MethodNotFound.")

;; What an error says, as the echo area would.
(define (rpc-error-text e)
  (if (error-object? e)
      (let ((irritants (error-object-irritants e)))
        (apply string-append (error-object-message e)
               (map (lambda (x) (string-append " " (call-with-output-string (lambda (p) (display x p))))) irritants)))
      (call-with-output-string (lambda (p) (display e p)))))

;; A head and a body as one write: the length is the body's UTF-8's.
(define (rpc-frame body)
  (let ((bytes (string->utf8 body)))
    (bytevector-append (string->utf8 (string-append "Content-Length: " (number->string (bytevector-length bytes)) "\r\n\r\n"))
                       bytes)))

(define (rpc-send! conn value)
  (when (rpc-closed? conn) (error "the connection is closed"))
  (channel-send (rpc-out conn) (rpc-frame (json->string value))))

(define (rpc-notify! conn method params)
  "Send the notification METHOD with PARAMS over CONN."
  (rpc-send! conn (dict "jsonrpc" "2.0" "method" method "params" params)))

(define (rpc-reply! conn id result)
  "Answer the request ID of CONN's peer with RESULT, a JSON value."
  (rpc-send! conn (dict "jsonrpc" "2.0" "id" id "result" result)))

(define (rpc-fail! conn id code text)
  (rpc-send! conn (dict "jsonrpc" "2.0" "id" id "error" (dict "code" code "message" text))))

(define (rpc-request! conn method params #:timeout [patience rpc-timeout] #:sent [sent #f])
  "Ask METHOD of CONN's peer with PARAMS and wait for the answer.
Return the answer's result. An error answer, a dead connection and no
answer in PATIENCE milliseconds (#:timeout, `rpc-timeout` unless given)
are errors; on the last, the peer is told the request is cancelled. The
waiting is this task's: the editor goes on without it. SENT, if given,
is called once the request is queued to be written, before the waiting:
what must reach the peer after it (an edit after a position asked
about) may go then."
  (let ((id (rpc-framer-take-id! (rpc-framer conn))) (reply (make-channel 1)))
    ;; Registered inside the wind: outside, a cancellation between
    ;; registering and entering would strand the waiter. However it
    ;; ends, its channel is closed: a channel lives until it is.
    (dynamic-wind
      (lambda () #f)
      (lambda ()
        (hash-table-set! (rpc-pending conn) id reply)
        (rpc-send! conn (dict "jsonrpc" "2.0" "id" id "method" method "params" params))
        (when sent (sent))
        (let ((answer (select (recv reply (v) v) (timeout patience 'timeout))))
          (cond ((eq? answer 'timeout)
                 ;; An answer coming later finds nobody waiting.
                 (guard (e (#t #f)) (rpc-notify! conn "$/cancelRequest" (dict "id" id)))
                 (error "no answer to" method))
                ((eof-object? answer) (error "the connection ended"))
                ((eq? (car answer) 'error) (error (cadr answer)))
                (else (cadr answer)))))
      (lambda ()
        (hash-table-delete! (rpc-pending conn) id)
        (channel-close reply)))))

(define (rpc-waiting conn)
  "Return how many requests of CONN wait for their answers."
  (hash-table-count (rpc-pending conn)))

(define (rpc-close! conn)
  "Close CONN: its process is killed and its waiters fail."
  (unless (rpc-closed? conn)
    (set-rpc-closed! conn #t)
    (guard (e (#t #f)) (process-kill (rpc-process conn)))
    (guard (e (#t #f)) (channel-close (rpc-out conn)))
    (guard (e (#t #f)) (channel-close (rpc-notes conn)))
    (for-each (lambda (kv)
                (hash-table-delete! (rpc-pending conn) (car kv))
                (guard (e (#t #f)) (channel-close (cdr kv))))
              (hash-table->alist (rpc-pending conn)))))

;; A request of the peer's, answered from a task of its own.
(define (rpc-serve! conn id method params)
  (spawn (lambda ()
           (let ((result (guard (e (#t (cons 'failed (rpc-error-text e)))) ((rpc-on-ask conn) method params))))
             (guard (e (#t #f))
               (cond ((eq? result rpc-unsupported) (rpc-fail! conn id -32601 (string-append "no such method: " method)))
                     ((and (pair? result) (eq? (car result) 'failed)) (rpc-fail! conn id -32603 (cdr result)))
                     (else (rpc-reply! conn id result))))))))

;; A message arrived: an answer wakes its waiter, a notification goes to
;; the notes task, a request is served, a batch is each of these. A
;; handler's failure is its own, not the connection's.
(define (rpc-deliver! conn message)
  (cond
   ((vector? message) (vector-for-each (lambda (m) (rpc-deliver! conn m)) message))
   ((dict? message)
    (let ((id (dict-ref message "id" #f)) (method (dict-ref message "method" #f))
          (params (dict-ref message "params" (dict))))
      (cond ((and method id (not (eq? id 'null))) (rpc-serve! conn id method params))
            (method (channel-send (rpc-notes conn) (cons method params)))
            (else
             (let ((reply (and id (hash-table-ref/default (rpc-pending conn) id #f))))
               (when reply
                 (hash-table-delete! (rpc-pending conn) id)
                 (let ((failure (dict-ref message "error" #f)))
                   ;; Its waiter may have given up and closed it since.
                   (guard (e (#t #f))
                     (channel-send reply
                                   (if (dict? failure)
                                       (list 'error (dict-ref failure "message" "an error"))
                                       (list 'result (dict-ref message "result" 'null))))))))))))))

(define (rpc-connect program args #:directory [directory #f] #:on-note [on-note (lambda (m p) #f)]
                     #:on-ask [on-ask (lambda (m p) rpc-unsupported)] #:on-dead [on-dead (lambda () #f)])
  "Start PROGRAM with ARGS in DIRECTORY, speaking JSON-RPC on its pipes.
Return the connection. A task reads it, in the scope calling:
notifications go to ON-NOTE (method params), requests of the peer's to
ON-ASK (method params), which answers with a JSON value or
`rpc-unsupported`, and answers wake `rpc-request!`'s. The process's
stderr is read and thrown away as it comes: a peer whose log nobody
reads blocks on a full pipe and then on nothing else, ever. When the
process ends — or speaks out of frame, which ends it from here — the
waiters fail and ON-DEAD runs. The scope owns the process, as every
process is: shutting it kills the peer."
  (let* ((process (process-spawn program args #:directory directory))
         (conn (%make-rpc process (make-rpc-framer) (make-hash-table) (make-channel 1000000) (make-channel 1000000) #f on-note on-ask on-dead)))
    (spawn (lambda ()
             (let loop ()
               (let ((note (channel-recv (rpc-notes conn))))
                 (unless (eof-object? note)
                   ;; A handler's failure is its own, not the connection's.
                   (guard (e (#t #f)) ((rpc-on-note conn) (car note) (cdr note)))
                   (loop))))))
    (spawn (lambda ()
             (guard (e (#t #f))
               (let loop ()
                 (unless (eof-object? (process-read-bytes process 'stderr)) (loop))))))
    (spawn (lambda ()
             (guard (e (#t #f))
               (let loop ()
                 (let ((chunk (process-read-bytes process 'stdout)))
                   (unless (eof-object? chunk)
                     (rpc-framer-feed! (rpc-framer conn) chunk)
                     (let frames ()
                       (let ((body (rpc-framer-next! (rpc-framer conn))))
                         (when body
                           (rpc-deliver! conn (string->json body))
                           (frames))))
                     (loop)))))
             ;; Ended, or ended here for speaking out of frame: no half a
             ;; peer is left running against a dead reader.
             (rpc-close! conn)
             (guard (e (#t #f)) ((rpc-on-dead conn)))))
    (spawn (lambda ()
             (let loop ()
               (let ((frame (channel-recv (rpc-out conn))))
                 (unless (eof-object? frame)
                   (if (guard (e (#t #f)) (process-write process frame) #t)
                       (loop)
                       (rpc-close! conn)))))))
    conn))
