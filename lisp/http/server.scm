;;; An HTTP/1.1 server over techne-net's TCP: one task per connection,
;;; requests on a connection one after another (keep-alive).
;;;
;;;   (http-serve (tcp-listen 8080) (lambda (request) (response "Hello")))
;;;
;;; A handler takes a request and returns a response, as `http.scm`
;;; describes them, or a string, the body of a 200 response; what it
;;; returns is conformed to `response-schema`, so its defaults fill in.
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

;; The bytes answering REQUEST: HANDLER's response, else ON-ERROR's for
;; HANDLER's failure, or for a response that is not one. A cancelled
;; task stays cancelled.
(define (respond handler request on-error close)
  (let ((bytes (lambda (r)
                 (response->bytes (conform response-schema (if (string? r) (response r) r) #:what "response")
                                  #:head (string=? (dict-ref request 'method) "HEAD") #:close close #:date (date-now)))))
    (call-with-current-continuation
     (lambda (k)
       (with-exception-handler
        (lambda (condition)
          (if (cancelled? condition)
              (raise condition)
              (k (bytes (on-error condition request)))))
        (lambda () (bytes (handler request))))))))

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
             (let ((close (not (keep-alive? request))))
               (send (respond handler request on-error close))
               (unless close (loop))))))))))

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
