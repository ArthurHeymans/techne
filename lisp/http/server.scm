;;; An HTTP/1.1 server over techne-net's TCP: one task per connection,
;;; requests on a connection one after another (keep-alive).
;;;
;;;   (http-serve (tcp-listen 8080) (lambda (request) (response "Hello")))
;;;
;;; A handler takes a request and returns a response, as `http.scm`
;;; describes them, or a string, the body of a 200 response; what it
;;; returns is conformed to `response-schema`, so its defaults fill in.
;;; A streamed body is sent as it is written, in chunks, each write within
;;; TIMEOUT (how long the whole body takes is the host's to bound). It is
;;; written while the body procedure runs, from its task. A failure while
;;; it streams goes to ON-ERROR too, but its response cannot be sent any
;;; more: the response is cut short, the connection closed.
;;; A handler that fails is answered by ON-ERROR, called where the failure
;;; was raised, with its frames still there: a debugger can be entered
;;; before it answers. The default writes the failure to the error port
;;; and answers 500.
;;;
;;; Reading a request, its head and body, must finish within TIMEOUT
;;; milliseconds of its start, the next request on a connection kept open
;;; must start within it too, and a response must be sent within it, else
;;; the connection is closed: a client trickling bytes or reading slowly
;;; holds a task no longer. Past MAX-CONNECTIONS open, no more are
;;; accepted until one closes. How long a handler may take is its host's
;;; to bound.

(require "../schema.scm")
(require "http.scm")

(provide http-serve http-serve-connection)

(define (default-on-error condition request)
  (let ((port (current-error-port)))
    (display "http: " port)
    (display (dict-ref request 'method) port)
    (display " " port)
    (display (dict-ref request 'path) port)
    (display ": " port)
    (display (if (error-object? condition) (condition/report-string condition) condition) port)
    (newline port))
  (response "Internal Server Error" #:status 500))

(define (cancelled? condition)
  (and (error-object? condition) (equal? (error-object-message condition) "task cancelled")))

;; The date header for now, written once a second.
(define %date (cons #f #f))
(define (date-now)
  (let ((s (exact (floor (current-second)))))
    (unless (eqv? (car %date) s) (set! %date (cons s (http-date s))))
    (cdr %date)))

;; How to answer REQUEST: (bytes stream close). BYTES are HANDLER's
;; response, else ON-ERROR's for HANDLER's failure or for a response that
;; is not one; STREAM its body to send after, if it streams; CLOSE whether
;; the connection closes after, as it must after a body streamed to an
;; HTTP/1.0 client, which knows no chunks. A cancelled task stays
;; cancelled.
(define (respond handler request on-error close)
  (let ((answer (lambda (r)
                  (let* ((r (conform response-schema (if (string? r) (response r) r) #:what "response"))
                         (head (string=? (dict-ref request 'method) "HEAD"))
                         (chunked (string=? (dict-ref request 'version) "HTTP/1.1"))
                         (body (dict-ref r 'body))
                         (stream (and (procedure? body) (not head) (not (memv (dict-ref r 'status) '(204 205 304))) body))
                         (close (or close (and stream (not chunked)))))
                    (list (response->bytes r #:head head #:close close #:date (date-now) #:chunked chunked)
                          (and stream (cons stream chunked))
                          close)))))
    (call-with-current-continuation
     (lambda (k)
       (with-exception-handler
        (lambda (condition)
          (if (cancelled? condition)
              (raise condition)
              (k (answer (on-error condition request)))))
        (lambda () (answer (handler request))))))))

;; Send the streamed body STREAM, (procedure . chunked), with SEND; return
;; #t if it was sent whole. A failure of the procedure is given to
;; ON-ERROR where it is raised, for its report, too late for its
;; response; a client gone ends it quietly. Either way the response is
;; cut short, and the connection must close. The procedure's writer works
;; while it runs, in its task: written later or from another task, data
;; would land in another response.
(define (stream! stream request on-error send)
  (let ((chunked (cdr stream)) (owner (current-task)) (open #t))
    (call-with-current-continuation
     (lambda (k)
       (let ((send (lambda (b)
                     (guard (e ((and (error-object? e) (not (cancelled? e))) (k #f)))
                       (send b)))))
         (with-exception-handler
          (lambda (condition)
            (if (cancelled? condition)
                (raise condition)
                (begin (on-error condition request) (k #f))))
          (lambda ()
            (dynamic-wind
             (lambda () #f)
             (lambda ()
               ((car stream)
                (lambda (data)
                  (unless open (error "The response has ended: no more can be written to it"))
                  (unless (eq? (current-task) owner) (error "A response is written from the task serving it"))
                  (unless (or (string? data) (bytevector? data)) (error "Not a string or bytevector to stream:" data))
                  (let ((b (if chunked (chunk->bytes data) (if (string? data) (string->utf8 data) data))))
                    (when (> (bytevector-length b) 0) (send b))))))
             (lambda () (set! open #f)))
            (when chunked (send last-chunk))
            #t)))))))

;; Milliseconds since some fixed time.
(define (now-ms)
  (quotient (* (current-jiffy) 1000) (jiffies-per-second)))

;; Call (F within) with a procedure WITHIN: (within ms thunk) calls THUNK,
;; and closes CONNECTION if it takes more than MS milliseconds, which
;; makes the reads and writes it waits on fail. One task watches the
;; deadline, which WITHIN only sets: it sleeps until the deadline, or for
;; MS while there is none, and looks again.
(define (with-watchdog connection ms f)
  (let* ((deadline #f)
         (watchdog (spawn (lambda ()
                            (let loop ()
                              (let ((left (if deadline (- deadline (now-ms)) ms)))
                                (if (and deadline (<= left 0))
                                    (tcp-close connection)
                                    (begin (sleep (max left 1)) (loop))))))))
         (within (lambda (ms thunk)
                   (dynamic-wind (lambda () (set! deadline (+ (now-ms) ms))) thunk (lambda () (set! deadline #f))))))
    (dynamic-wind (lambda () #f) (lambda () (f within)) (lambda () (task-cancel watchdog)))))

(define (http-serve-connection connection handler #:on-error [on-error default-on-error] #:timeout [timeout 30000]
                               #:max-head [max-head 16384] #:max-body [max-body 1048576])
  "Answer the requests on CONNECTION with HANDLER until it closes.
ON-ERROR, TIMEOUT, MAX-HEAD and MAX-BODY are as `http-serve` takes
them. CONNECTION is left open."
  (with-watchdog
   connection timeout
   (lambda (within)
     (let ((r (make-http-reader (lambda () (tcp-read-bytes connection))))
           (send (lambda (bytes) (within timeout (lambda () (tcp-write connection bytes)))))
           (continue (string->utf8 "HTTP/1.1 100 Continue\r\n\r\n")))
       (let loop ()
         (let ((request (guard (e ((http-failure? e)
                                   (send (response->bytes (response (http-failure-message e) #:status (http-failure-status e))
                                                          #:close #t #:date (date-now)))
                                   (eof-object))
                                  ;; Closed at the deadline, or by the client.
                                  ((and (error-object? e) (not (cancelled? e))) (eof-object)))
                          (within timeout
                                  (lambda ()
                                    (read-request r #:max-head max-head #:max-body max-body
                                                  #:continue (lambda () (tcp-write connection continue))))))))
           (unless (eof-object? request)
             (let* ((answer (respond handler request on-error (not (keep-alive? request))))
                    (stream (cadr answer)))
               (send (car answer))
               (when (and (or (not stream) (stream! stream request on-error send))
                          (not (caddr answer)))
                 (loop))))))))))

(define (http-serve listener handler #:on-error [on-error default-on-error] #:timeout [timeout 30000]
                    #:max-head [max-head 16384] #:max-body [max-body 1048576] #:max-connections [max-connections 1024])
  "Answer the HTTP requests to LISTENER with HANDLER.
Each connection is served in a task of its own, until the listener is
closed. HANDLER takes a request and returns a response, or a string to
answer with. ON-ERROR takes a failure of HANDLER and its request, where
it was raised, and returns the response. TIMEOUT is how many
milliseconds a request may take to arrive, and a response to be sent;
MAX-HEAD and MAX-BODY how many bytes a request's head and body may
have. Past MAX-CONNECTIONS open, no more are accepted until one closes."
  ;; A place in SLOTS for each connection open.
  (let ((slots (make-channel max-connections)))
    (let loop ()
      (channel-send slots #t)
      (let ((connection (tcp-accept listener)))
        (spawn (lambda ()
                 (dynamic-wind
                  (lambda () #f)
                  (lambda ()
                    (call-with-socket connection
                                      (lambda (c)
                                        (http-serve-connection c handler #:on-error on-error #:timeout timeout
                                                               #:max-head max-head #:max-body max-body))))
                  (lambda () (channel-recv slots)))))
        (loop)))))
