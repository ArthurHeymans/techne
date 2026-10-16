(require "../../test.scm")
(require "../../http/http.scm")
(require "../../http/server.scm")

(define where (make-parameter 'outside))
(define streamed #f)
(define written 0)
(define kept #f)
(define errors '())

(define (handler request)
  (let ((path (dict-ref request 'path)))
    (cond ((string=? path "/fail") (parameterize ((where 'inside)) (error "it broke")))
          ((string=? path "/bad") (dict 'status "two hundred"))
          ((string=? path "/split") (response "" #:headers (dict "a" "b\r\nc: d")))
          ((string=? path "/big") (response (make-bytevector 16000000 65)))
          ((string=? path "/stream") (response (lambda (write) (set! streamed #t) (write "a") (write "") (write "bc"))))
          ((string=? path "/stream-fail") (response (lambda (write) (write "a") (parameterize ((where 'streaming)) (error "mid-stream")))))
          ((string=? path "/forever") (response (lambda (write) (let loop () (set! written (+ written 1)) (write "x") (sleep 5) (loop)))))
          ((string=? path "/keep") (response (lambda (write) (set! kept write) (write "a"))))
          ((string=? path "/late") (kept "LATE") "late")
          ((string=? path "/other-task") (response (lambda (write) (task-join (spawn (lambda () (write "x")))))))
          ((string=? path "/events") (event-stream (lambda (send) (send "one") (send (dict "n" 2) #:event "two"))))
          ((string=? path "/echo") (response (dict-ref request 'body) #:type "application/octet-stream"))
          (else "hello"))))

;; Failures are answered where they were raised: WHERE is still 'inside.
(define (on-error condition request)
  (set! errors (cons (where) errors))
  (response (symbol->string (where)) #:status 500))

(define listener (tcp-listen 0))
(define port (cadr (tcp-local-address listener)))
(spawn (lambda () (http-serve listener handler #:on-error on-error #:timeout 300)))

(define (read-all c)
  (let loop ((acc '()))
    (let ((s (tcp-read c)))
      (if (eof-object? s) (apply string-append (reverse acc)) (loop (cons s acc))))))

;; How many times PATTERN is in TEXT.
(define (occurrences text pattern)
  (let loop ((text text) (n 0))
    (let ((at (string-contains text pattern)))
      (if at (loop (substring text (+ at 1) (string-length text)) (+ n 1)) n))))

;; TEXT without its date headers, each checked to be a date; every
;; response but 100 Continue must have one.
(define (undated text)
  (check "a date on each response"
         (- (occurrences text "HTTP/1.1 ") (occurrences text "HTTP/1.1 100 "))
         (occurrences text "\r\ndate: "))
  (let loop ((text text))
    (let ((at (string-contains text "\r\ndate: ")))
      (if (not at)
          text
          (let ((date (substring text (+ at 8) (+ at 37))))
            (check-true "a date header" (and (string-suffix? " GMT" date) (string=? (substring date 3 5) ", ")))
            (loop (string-append (substring text 0 at) (substring text (+ at 37) (string-length text)))))))))

;; What the server sends for TEXT, until it closes the connection, but
;; for the date.
(define (exchange text)
  (undated (call-with-tcp-connection "127.0.0.1" port (lambda (c) (tcp-write c text) (read-all c)))))

(define ok "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 5\r\n\r\nhello")
(define ok-closing "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello")

(check "keep-alive, then close" (string-append ok ok-closing)
       (exchange "GET / HTTP/1.1\r\nHost: x\r\n\r\nGET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
(check "1.0" ok-closing (exchange "GET / HTTP/1.0\r\n\r\n"))
(check "head" "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 5\r\nconnection: close\r\n\r\n"
       (exchange "HEAD / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
(check "echo" "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: 3\r\nconnection: close\r\n\r\nabc"
       (exchange "POST /echo HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 3\r\n\r\nabc"))
(check "failure, answered where raised"
       "HTTP/1.1 500 Internal Server Error\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 6\r\nconnection: close\r\n\r\ninside"
       (exchange "GET /fail HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
(check "an invalid response is a failure" #t
       (string-prefix? "HTTP/1.1 500 " (exchange "GET /bad HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")))
(check "refused" "HTTP/1.1 400 Bad Request\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 41\r\nconnection: close\r\n\r\nBoth transfer-encoding and content-length"
       (exchange "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n"))
(check "a slow head is cut off" "" (exchange "GET / HTTP/1.1\r\nHost:"))
(check "an idle connection is closed after its request" ok (exchange "GET / HTTP/1.1\r\nHost: x\r\n\r\n"))

;; 100-continue: the body is sent once the server asks for it.
(check "continue"
       (list "HTTP/1.1 100 Continue\r\n\r\n"
             "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi")
       (call-with-tcp-connection "127.0.0.1" port
         (lambda (c)
           (tcp-write c "POST /echo HTTP/1.1\r\nHost: x\r\nConnection: close\r\nExpect: 100-continue\r\nContent-Length: 2\r\n\r\n")
           (let ((first (tcp-read c)))
             (tcp-write c "hi")
             (list first (undated (read-all c)))))))

(check "a response that cannot be written is a failure"
       "HTTP/1.1 500 Internal Server Error\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 7\r\nconnection: close\r\n\r\noutside"
       (exchange "GET /split HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))

;; A client that does not read is cut off at the deadline.
(check "a slow reader" #t
       (call-with-tcp-connection "127.0.0.1" port
         (lambda (c)
           (tcp-write c "GET /big HTTP/1.1\r\nHost: x\r\n\r\n")
           (sleep 1000)
           (let loop ((n 0))
             (let ((b (tcp-read-bytes c)))
               (if (eof-object? b) (< n 16000000) (loop (+ n (bytevector-length b)))))))))

;; Past its connections, a server accepts no more until one closes: the
;; second is answered after the first, idle, is closed at its deadline.
(define small (tcp-listen 0))
(spawn (lambda () (http-serve small handler #:timeout 300 #:max-connections 1)))
(define events '())
(define (note! e) (set! events (cons e events)))
(define first-conn (tcp-connect "127.0.0.1" (cadr (tcp-local-address small))))
(define watcher (spawn (lambda () (read-all first-conn) (note! 'first-closed))))
(yield)
(call-with-tcp-connection "127.0.0.1" (cadr (tcp-local-address small))
  (lambda (c)
    (tcp-write c "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
    (read-all c)
    (note! 'second-answered)))
(task-join watcher)
(check "one connection at a time" '(first-closed second-answered) (reverse events))

;; A client gone before its answer ends the connection, quietly.
(define gone (tcp-listen 0))
(define answering
  (spawn (lambda ()
           (call-with-socket (tcp-accept gone)
             (lambda (c) (http-serve-connection c (lambda (r) (sleep 200) (make-string 1000000 #\a))))))))
(call-with-tcp-connection "127.0.0.1" (cadr (tcp-local-address gone))
  (lambda (c) (tcp-write c "GET / HTTP/1.1\r\nHost: x\r\n\r\n")))
(check "a client gone" 'ended (guard (e ((error-object? e) (error-object-message e))) (task-join answering) 'ended))

;; A connection's task cancelled while its handler runs stays cancelled.
(define slow (tcp-listen 0))
(define served
  (spawn (lambda ()
           (call-with-socket (tcp-accept slow)
             (lambda (c) (http-serve-connection c (lambda (r) (sleep 10000) "late")))))))
(define client (tcp-connect "127.0.0.1" (cadr (tcp-local-address slow))))
(tcp-write client "GET / HTTP/1.1\r\nHost: x\r\n\r\n")
(sleep 100)
(task-cancel served)
(check "cancelled" "task cancelled"
       (guard (e ((error-object? e) (error-object-message e))) (task-join served) 'finished))
(check "nothing was answered" "" (read-all client))

;; Streamed responses
(check "chunked, then the next request"
       (string-append "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n1\r\na\r\n2\r\nbc\r\n0\r\n\r\n" ok-closing)
       (exchange "GET /stream HTTP/1.1\r\nHost: x\r\n\r\nGET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
(check "to HTTP/1.0, until closed" "HTTP/1.1 200 OK\r\nconnection: close\r\n\r\nabc"
       (exchange "GET /stream HTTP/1.0\r\nConnection: keep-alive\r\n\r\n"))
(set! streamed #f)
(check "head does not stream" "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n"
       (exchange "HEAD /stream HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
(check "nor run its body" #f streamed)
(set! errors '())
(check "a failure cuts it short" "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n1\r\na\r\n"
       (exchange "GET /stream-fail HTTP/1.1\r\nHost: x\r\n\r\nGET / HTTP/1.1\r\nHost: x\r\n\r\n"))
(check "and is reported where raised" '(streaming) errors)
(check "events"
       "HTTP/1.1 200 OK\r\ncache-control: no-cache\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\nb\r\ndata: one\n\n\r\n1a\r\nevent: two\ndata: {\"n\":2}\n\n\r\n0\r\n\r\n"
       (exchange "GET /events HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
;; A writer works while its body runs, in its task.
(set! errors '())
(check "no writing after the end"
       (string-append "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n1\r\na\r\n0\r\n\r\n"
                      "HTTP/1.1 500 Internal Server Error\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 7\r\nconnection: close\r\n\r\noutside")
       (exchange "GET /keep HTTP/1.1\r\nHost: x\r\n\r\nGET /late HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
(check "nor from another task" "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n"
       (exchange "GET /other-task HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
(check "each reported" '(outside outside) errors)

;; A client leaving ends the stream, quietly.
(set! errors '())
(call-with-tcp-connection "127.0.0.1" port
  (lambda (c)
    (tcp-write c "GET /forever HTTP/1.1\r\nHost: x\r\n\r\n")
    (tcp-read c)))
(sleep 200)
(define after-close written)
(sleep 200)
(check "the stream stopped" after-close written)
(check "and reported nothing" '() errors)

(test-summary)
