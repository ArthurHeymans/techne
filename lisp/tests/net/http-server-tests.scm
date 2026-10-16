(require "../../test.scm")
(require "../../http/http.scm")
(require "../../http/server.scm")

(define where (make-parameter 'outside))

(define (handler request)
  (let ((path (dict-ref request 'path)))
    (cond ((string=? path "/fail") (parameterize ((where 'inside)) (error "it broke")))
          ((string=? path "/bad") (dict 'status "two hundred"))
          ((string=? path "/split") (response "" #:headers (dict "a" "b\r\nc: d")))
          ((string=? path "/big") (response (make-bytevector 16000000 65)))
          ((string=? path "/echo") (response (dict-ref request 'body) #:type "application/octet-stream"))
          (else "hello"))))

;; Failures are answered where they were raised: WHERE is still 'inside.
(define (on-error condition request)
  (response (symbol->string (where)) #:status 500))

(define listener (tcp-listen 0))
(define port (cadr (tcp-local-address listener)))
(spawn (lambda () (http-serve listener handler #:on-error on-error #:timeout 300)))

(define (read-all c)
  (let loop ((acc '()))
    (let ((s (tcp-read c)))
      (if (eof-object? s) (apply string-append (reverse acc)) (loop (cons s acc))))))

;; TEXT without its date headers, each checked to be a date.
(define (undated text)
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

(test-summary)
