(require "../test.scm")
(require "../schema.scm")
(require "../http/http.scm")

;; A reader of TEXT, arriving in pieces of N bytes.
(define (reader-of text [n 1000000])
  (let ((b (string->utf8 text)) (at 0))
    (make-http-reader
     (lambda ()
       (if (= at (bytevector-length b))
           (eof-object)
           (let ((end (min (bytevector-length b) (+ at n))))
             (let ((piece (bytevector-copy b at end))) (set! at end) piece)))))))

(define (read-one text . options)
  (apply read-request (reader-of text) options))

;; The status a request is refused with, or #f.
(define (refused text . options)
  (guard (e ((http-failure? e) (http-failure-status e)))
    (apply read-one text options)
    #f))

(define get "GET /notes/1?x=1&y HTTP/1.1\r\nHost: example.com\r\nAccept: a\r\naccept:  b \r\n\r\n")

;; Reading
(let ((r (read-one get)))
  (check "method" "GET" (dict-ref r 'method))
  (check "path" "/notes/1" (dict-ref r 'path))
  (check "query" "x=1&y" (dict-ref r 'query))
  (check "header" "example.com" (request-header r "HOST"))
  (check "joined headers" "a, b" (request-header r "accept"))
  (check "no body" #u8() (dict-ref r 'body))
  (check-true "a valid request" (valid? request-schema r))
  (check-true "keep-alive in 1.1" (keep-alive? r)))
(check "no query" #f (dict-ref (read-one "GET / HTTP/1.1\r\nHost: x\r\n\r\n") 'query))
(check "nothing to read" #t (eof-object? (read-one "")))
(check "1.0 closes" #f (keep-alive? (read-one "GET / HTTP/1.0\r\n\r\n")))
(check "close" #f (keep-alive? (read-one "GET / HTTP/1.1\r\nHost: x\r\nConnection: Upgrade, close\r\n\r\n")))

;; Bodies, in pieces of any size, and requests one after another
(define post "POST /n HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhelloPUT /m HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n3;ext=1\r\nwor\r\n2\r\nld\r\n0\r\nTrailer: x\r\n\r\nGET / HTTP/1.1\r\nHost: x\r\n\r\n")
(for-each
 (lambda (n)
   (let* ((r (reader-of post n))
          (a (read-request r)) (b (read-request r)) (c (read-request r)))
     (check (string-append "pieces of " (number->string n))
            '("hello" "world" "GET" #t)
            (list (request-text a) (request-text b) (dict-ref c 'method) (eof-object? (read-request r))))))
 '(1 2 3 7 1000))
(check "json body" 3 (dict-ref (request-json (read-one "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 7\r\n\r\n{\"a\":3}")) "a"))

;; 100-continue
(let ((told #f))
  (read-one "POST / HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nContent-Length: 1\r\n\r\nx" #:continue (lambda () (set! told #t)))
  (check "continue" #t told))

;; Refusals
(check "both lengths" 400 (refused "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"))
(check "two lengths" 400 (refused "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabcd"))
(check "a signed length" 400 (refused "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: +3\r\n\r\nabc"))
(check "another coding" 501 (refused "POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip, chunked\r\n\r\n"))
(check "space before colon" 400 (refused "GET / HTTP/1.1\r\nHost: x\r\nHost : x\r\n\r\n"))
(check "folded header" 400 (refused "GET / HTTP/1.1\r\nHost: x\r\nA: b\r\n c\r\n\r\n"))
(check "bare LF" 400 (refused "GET / HTTP/1.1\r\nHost: x\r\nA: b\nC: d\r\n\r\n"))
(check "bad request line" 400 (refused "GET  / HTTP/1.1\r\nHost: x\r\n\r\n"))
(check "version" 505 (refused "GET / HTTP/2.0\r\n\r\n"))
(check "long head" 431 (refused (string-append "GET / HTTP/1.1\r\nHost: x\r\nA: " (make-string 100 #\a) "\r\n\r\n") #:max-head 64))
(check "many headers" 431 (refused "GET / HTTP/1.1\r\nHost: x\r\nA: 1\r\nB: 2\r\n\r\n" #:max-headers 1))
(check "large body" 413 (refused "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 10\r\n\r\n0123456789" #:max-body 9))
(check "large chunks" 413 (refused "POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n01234\r\n5\r\n56789\r\n0\r\n\r\n" #:max-body 9))
(check "a cut body" 400 (refused "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 10\r\n\r\nabc"))
(check "a cut head" 400 (refused "GET / HTTP/1.1\r\nHost: x"))
(check "a bad chunk size" 400 (refused "POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n"))

;; Refusals of what another reader could read otherwise
(check "no host" 400 (refused "GET / HTTP/1.1\r\n\r\n"))
(check "two hosts" 400 (refused "GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n"))
(check "a bad host" 400 (refused "GET / HTTP/1.1\r\nHost: a/b\r\n\r\n"))
(check "1.0 needs no host" "GET" (dict-ref (read-one "GET / HTTP/1.0\r\n\r\n") 'method))
(check "NUL in a value" 400 (refused "GET / HTTP/1.1\r\nHost: x\r\nA: b\x0;c\r\n\r\n"))
(check "a tab in the target" 400 (refused "GET /a\tb HTTP/1.1\r\nHost: x\r\n\r\n"))
(check "a non-ASCII method" 400 (refused "GÉT / HTTP/1.1\r\nHost: x\r\n\r\n"))
(check "the Kelvin sign is no k" 501 (refused "POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chun\x212A;ed\r\n\r\n0\r\n\r\n"))
(check "chunked in any case" "ab" (request-text (read-one "POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: Chunked\r\n\r\n2\r\nab\r\n0\r\n\r\n")))
(check "chunked in 1.0" 400 (refused "POST / HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"))
(check "no continue for 1.0" #f
       (let ((told #f))
         (read-one "POST / HTTP/1.0\r\nExpect: 100-continue\r\nContent-Length: 1\r\n\r\nx" #:continue (lambda () (set! told #t)))
         told))
(check "chunk extensions" "ab" (request-text (read-one "POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n2 ; a=b ;c=\"d;e\"\r\nab\r\n0\r\n\r\n")))
(check "malformed extensions" 400 (refused "POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n0;;\r\n\r\n"))
(check "a framing trailer" 400 (refused "POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nContent-Length: 5\r\n\r\n"))
(check "a folded trailer" 400 (refused "POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nA: b\r\n c\r\n\r\n"))
(check "trailers in bytes" 431
       (refused (string-append "POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nA: " (make-string 40 #\é) "\r\n\r\n")
                #:max-head 80))
(check "chunk overhead" 413
       (refused (string-append "POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n" (apply string-append (make-list 20 "1\r\na\r\n")) "0\r\n\r\n")
                #:max-body 50))
(check "chunk overhead in bytes" 413
       (refused (string-append "POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n0;a=\"" (make-string 20 #\é) "\"\r\n\r\n")
                #:max-body 30))
(check "close wins in 1.0" #f (keep-alive? (read-one "GET / HTTP/1.0\r\nConnection: keep-alive, close\r\n\r\n")))

;; A body arriving a byte at a time is read in linear time.
(let* ((n 65536)
       (text (string-append "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: " (number->string n) "\r\n\r\n" (make-string n #\a))))
  (check "a byte at a time" n (bytevector-length (dict-ref (read-request (reader-of text 1)) 'body))))

;; Targets in absolute form
(let ((r (read-one "GET HTTP://example.com:8080?a=1 HTTP/1.1\r\nHost: other\r\n\r\n")))
  (check "absolute path" "/" (dict-ref r 'path))
  (check "absolute query" "a=1" (dict-ref r 'query))
  (check "the authority is the host" "example.com:8080" (request-header r "host")))
(check "absolute with a path" "/a/b" (dict-ref (read-one "GET https://x/a/b HTTP/1.1\r\nHost: x\r\n\r\n") 'path))
(check "user information" 400 (refused "GET http://u@x/ HTTP/1.1\r\nHost: x\r\n\r\n"))
(check "no authority" 400 (refused "GET http:///a HTTP/1.1\r\nHost: x\r\n\r\n"))
(check "another scheme" 400 (refused "GET ftp://x/ HTTP/1.1\r\nHost: x\r\n\r\n"))
(check "an IPv6 authority" "[::1]:80" (request-header (read-one "GET http://[::1]:80/ HTTP/1.1\r\nHost: x\r\n\r\n") "host"))
(check "an unclosed bracket" 400 (refused "GET http://[::1/a HTTP/1.1\r\nHost: x\r\n\r\n"))
(check "a port of letters" 400 (refused "GET http://x:abc/ HTTP/1.1\r\nHost: x\r\n\r\n"))
(check "a bad escape" 400 (refused "GET http://%GG/ HTTP/1.1\r\nHost: x\r\n\r\n"))
(check "a bad escape in the path" 400 (refused "GET /a%2 HTTP/1.1\r\nHost: x\r\n\r\n"))
(check "a fragment" 400 (refused "GET http://x/?q#f HTTP/1.1\r\nHost: x\r\n\r\n"))
(check "a bad host header" 400 (refused "GET / HTTP/1.1\r\nHost: x:1:2\r\n\r\n"))
(check "still a host header" 400 (refused "GET http://x/ HTTP/1.1\r\n\r\n"))

;; Dates
(check "epoch" "Thu, 01 Jan 1970 00:00:00 GMT" (http-date 0))
(check "RFC example" "Sun, 06 Nov 1994 08:49:37 GMT" (http-date 784111777))
(check "a four-digit year" "Mon, 01 Jan 0001 00:00:00 GMT" (http-date -62135596800))
(check "leap day" "Thu, 29 Feb 2024 23:59:59 GMT" (http-date 1709251199.9))

;; Writing
(define (text-of resp . options) (utf8->string (apply response->bytes resp options)))
(check "text" "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 3\r\n\r\nhé"
       (text-of (response "hé")))
(check "json, closing" "HTTP/1.1 201 Created\r\ncontent-type: application/json\r\ncontent-length: 7\r\nconnection: close\r\n\r\n{\"a\":1}"
       (text-of (json-response (dict "a" 1) #:status 201) #:close #t))
(check "head" "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 2\r\n\r\n"
       (text-of (response "hi") #:head #t))
(check "computed length wins" "HTTP/1.1 204 No Content\r\n\r\n"
       (text-of (response #u8() #:status 204 #:headers (dict "Content-Length" "9"))))
(check "no splitting" #f (guard (e ((error-object? e) #f)) (text-of (response "" #:headers (dict "a" "b\r\nc: d")))))
(check "205 has no content" "HTTP/1.1 205 Reset Content\r\ncontent-length: 0\r\n\r\n" (text-of (response "x" #:status 205)))
(check "repeated headers" "HTTP/1.1 200 OK\r\nset-cookie: a=1\r\nset-cookie: b=2\r\ncontent-length: 0\r\n\r\n"
       (text-of (response #u8() #:headers (dict "set-cookie" '("a=1" "b=2")))))
(check "no control characters" #f (guard (e ((error-object? e) #f)) (text-of (response "" #:headers (dict "a" "b\x0B;")))))
(check "no informational responses" #f (valid? response-schema (dict 'status 100 'headers (dict) 'body "")))
(check "date" "HTTP/1.1 200 OK\r\ndate: Thu, 01 Jan 1970 00:00:00 GMT\r\ncontent-length: 0\r\n\r\n"
       (text-of (response #u8()) #:date (http-date 0)))
(check "no splitting by date" #f (guard (e ((error-object? e) #f)) (text-of (response "") #:date "x\r\nInjected: yes")))
(check "a date given wins" "HTTP/1.1 200 OK\r\ndate: x\r\ncontent-length: 0\r\n\r\n"
       (text-of (response #u8() #:headers (dict "date" "x")) #:date (http-date 0)))
(check "conformed" "HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n" (text-of (conform response-schema (dict 'body #u8()))))

(test-summary)
