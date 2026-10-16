;;; HTTP messages: requests and responses as dicts, and HTTP/1.1 (RFC 9112)
;;; read and written as bytes.
;;;
;;; A request is a dict, as `request-schema` describes it:
;;;
;;;   {method "GET" path "/notes/1" query "x=1" version "HTTP/1.1"
;;;    headers {"host" "example.com" ...} body #u8()}
;;;
;;; METHOD is as sent (methods are case-sensitive), PATH and QUERY as sent,
;;; not decoded; QUERY is #f without a `?`. Header names are lowercase; a
;;; header sent more than once has its values joined by \", \".
;;;
;;; A response is a dict too (`response-schema`): its `status`, `headers`
;;; and `body`, a string (sent as UTF-8) or a bytevector. `response` and
;;; `json-response` make one.
;;;
;;; `read-request` reads one request from bytes as they arrive, strictly:
;;; what a lenient reader and a proxy in front of it could read differently
;;; (a body with both a length and chunks, a line ending in a bare CR or
;;; LF, a header folded over lines, whitespace before a header's colon) is
;;; refused, as are a head or body past their limits. A refusal raises an
;;; `http-failure` with the status to answer.

(require "../schema.scm")

(provide request-schema response-schema response json-response request-header request-text request-json
         make-http-reader read-request keep-alive? response->bytes
         http-failure? http-failure-status http-failure-message)

(define request-schema
  '(dict (method string)
         (path string)
         (query (or string (one-of #f)))
         (version (one-of "HTTP/1.0" "HTTP/1.1"))
         (headers (dict-of string string))
         (body bytevector))
  "The schema of a request, as `read-request` returns it.")

(define response-schema
  '(dict (status (integer #:min 200 #:max 599) #:default 200)
         (headers (dict-of string (or string (list-of string))) #:default {})
         (body (or string bytevector) #:default ""))
  "The schema of a response, as a handler returns it.
STATUS is 200, HEADERS empty and BODY empty unless given. A header's
value is a string, or a list of them to send the header once for each,
as for set-cookie.")

(define (response body #:status [status 200] #:headers [headers (dict)] #:type [type #f])
  "Return a response of STATUS with BODY, a string or a bytevector.
HEADERS is a dict of header names and values; TYPE, if given, is its
content-type."
  (dict 'status status 'headers (if type (dict-set headers "content-type" type) headers) 'body body))

(define (json-response data #:status [status 200] #:headers [headers (dict)])
  "Return a response of STATUS with DATA written as JSON.
HEADERS is a dict of header names and values."
  (response (json->string data) #:status status #:headers headers #:type "application/json"))

(define (request-header request name [default #f])
  "Return the value of the header NAME in REQUEST, else DEFAULT.
NAME is compared without case."
  (dict-ref (dict-ref request 'headers) (ascii-downcase name) default))

(define (request-text request)
  "Return the body of REQUEST as a string, decoded as UTF-8."
  (utf8->string (dict-ref request 'body)))

(define (request-json request)
  "Return the body of REQUEST read as JSON."
  (string->json (request-text request)))

;; ----- failures -----

(define-record-type http-failure
  (make-http-failure status message)
  http-failure?
  (status http-failure-status)
  (message http-failure-message))

(define (fail status message)
  (raise (make-http-failure status message)))

;; ----- characters -----

(define (ascii-downcase s)
  (string-map (lambda (c) (if (char<=? #\A c #\Z) (integer->char (+ (char->integer c) 32)) c)) s))

(define (tchar? c)
  (or (char<=? #\a c #\z) (char<=? #\A c #\Z) (char<=? #\0 c #\9) (and (string-index "!#$%&'*+-.^_`|~" c) #t)))

;; Whether every character of S passes PRED.
(define (chars-every? pred s) (every pred (string->list s)))

(define (token? s)
  (and (> (string-length s) 0) (chars-every? tchar? s)))

;; A character a header's value may have: visible, a space or tab, or
;; beyond ASCII; no other control character.
(define (field-char? c)
  (let ((n (char->integer c))) (or (= n 9) (<= 32 n 126) (>= n 128))))

;; S without the spaces and tabs around it.
(define (trim-ows s)
  (let* ((ows? (lambda (i) (memv (string-ref s i) '(#\space #\tab))))
         (start (let loop ((i 0)) (if (and (< i (string-length s)) (ows? i)) (loop (+ i 1)) i)))
         (end (let loop ((i (string-length s))) (if (and (> i start) (ows? (- i 1))) (loop (- i 1)) i))))
    (substring s start end)))

(define (comma-tokens value)
  (map (lambda (s) (ascii-downcase (trim-ows s))) (string-split value ",")))

;; ----- reading -----

;; Bytes from (MORE), which returns a bytevector or the eof object, and
;; those received but not yet read: BUFFER from START.
(define-record-type http-reader
  (%make-http-reader more buffer start)
  http-reader?
  (more reader-more)
  (buffer reader-buffer set-reader-buffer!)
  (start reader-start set-reader-start!))

(define (make-http-reader more)
  "Return a reader of the bytes (MORE) returns, for `read-request`.
MORE returns a bytevector of what arrived next, or the eof object at the
end; a reader keeps what one request leaves for the next."
  (%make-http-reader more (bytevector) 0))

;; Replace R's buffer, read to its end, by what arrives next; #f at the
;; end of the input.
(define (refill! r)
  (let ((b ((reader-more r))))
    (and (not (eof-object? b))
         (begin (set-reader-buffer! r b) (set-reader-start! r 0) #t))))

;; PIECES, bytevectors in order, as one.
(define (concat pieces)
  (let ((out (make-bytevector (fold (lambda (p n) (+ n (bytevector-length p))) 0 pieces))))
    (fold (lambda (p at) (bytevector-copy! out at p) (+ at (bytevector-length p))) 0 pieces)
    out))

;; Read through CR LF (CR LF CR LF with BLANK) and return what is before
;; it as a string, at most LIMIT bytes, else fail with TOO-LONG; the eof
;; object if the input ends before anything is read. Each byte is copied
;; once however the input arrives.
(define (read-through! r limit too-long blank)
  (let ((delim (if blank #u8(13 10 13 10) #u8(13 10))))
    (let loop ((pieces '()) (size 0) (k 0))
      (let* ((b (reader-buffer r)) (start (reader-start r)) (n (bytevector-length b))
             ;; Where the delimiter ends, else #f; and how much of it
             ;; the buffer ends with.
             (found (let scan ((i start) (k k))
                      (cond ((= k (bytevector-length delim)) (cons i k))
                            ((= i n) (cons #f k))
                            (else (let ((x (bytevector-u8-ref b i)))
                                    (scan (+ i 1) (cond ((= x (bytevector-u8-ref delim k)) (+ k 1)) ((= x 13) 1) (else 0))))))))
             (end (or (car found) n))
             (size (+ size (- end start)))
             (pieces (cons (bytevector-copy b start end) pieces)))
        (set-reader-start! r end)
        (cond ((> size (+ limit (bytevector-length delim))) (fail too-long "The head is too long"))
              ((car found)
               (let ((line (concat (reverse pieces))))
                 (guard (e ((error-object? e) (fail 400 "The head is not UTF-8")))
                   (utf8->string line 0 (- size (bytevector-length delim))))))
              ((refill! r) (loop pieces size (cdr found)))
              ((= size 0) (eof-object))
              (else (fail 400 "The input ended within a request")))))))

;; Read N bytes exactly.
(define (read-exactly! r n)
  (let loop ((pieces '()) (got 0))
    (let* ((b (reader-buffer r)) (start (reader-start r)) (take (min (- (bytevector-length b) start) (- n got))))
      (cond ((= got n) (concat (reverse pieces)))
            ((> take 0)
             (set-reader-start! r (+ start take))
             (loop (cons (bytevector-copy b start (+ start take)) pieces) (+ got take)))
            ((refill! r) (loop pieces got))
            (else (fail 400 "The input ended within a body"))))))

;; A line of the head must not hold a bare CR or LF, which another reader
;; could take for the end of the line.
(define (check-line line)
  (when (or (string-index line #\return) (string-index line #\newline))
    (fail 400 "A bare CR or LF in the head")))

;; The field LINE as (name . value), its name lowercase. A name with
;; whitespace in or around it, or a folded line (which starts with
;; whitespace), is refused.
(define (parse-field line)
  (check-line line)
  (let ((colon (string-index line #\:)))
    (unless colon (fail 400 "A header without a colon"))
    (let ((name (substring line 0 colon))
          (value (trim-ows (substring line (+ colon 1) (string-length line)))))
      (unless (token? name) (fail 400 "A malformed header name"))
      (unless (chars-every? field-char? value) (fail 400 "A control character in a header"))
      (cons (ascii-downcase name) value))))

;; The headers of LINES as a dict.
(define (parse-headers lines max-headers)
  (when (> (length lines) max-headers) (fail 431 "Too many headers"))
  (fold (lambda (line headers)
          (let* ((field (parse-field line)) (name (car field)) (value (cdr field)) (old (dict-ref headers name #f)))
            (cond ((not old) (dict-set headers name value))
                  ((string=? name "host") (fail 400 "Two host headers"))
                  ((string=? name "content-length")
                   (if (string=? old value) headers (fail 400 "Conflicting content-length headers")))
                  (else (dict-set headers name (string-append old ", " value))))))
        (dict) lines))

;; Whether S is a request target's host: a name, an IPv4 or IPv6 address,
;; and a port, in the characters a URI's authority has.
(define (host? s)
  (chars-every? (lambda (c) (or (char<=? #\a c #\z) (char<=? #\A c #\Z) (char<=? #\0 c #\9) (string-index "-._~!$&'()*+,;=:[]%" c))) s))

;; Whether EXT, what follows a chunk's size, is chunk extensions:
;; ;name or ;name=value, VALUE a token or a quoted string, with spaces and
;; tabs around the ; and =.
(define (chunk-extensions? ext)
  (let ((n (string-length ext)))
    (define (ows i) (if (and (< i n) (memv (string-ref ext i) '(#\space #\tab))) (ows (+ i 1)) i))
    (define (token-end i) (if (and (< i n) (tchar? (string-ref ext i))) (token-end (+ i 1)) i))
    (define (quoted-end i)
      (cond ((>= i n) #f)
            ((char=? (string-ref ext i) #\") (+ i 1))
            ((char=? (string-ref ext i) #\\) (and (< (+ i 1) n) (field-char? (string-ref ext (+ i 1))) (quoted-end (+ i 2))))
            ((field-char? (string-ref ext i)) (quoted-end (+ i 1)))
            (else #f)))
    (let loop ((i (ows 0)))
      (cond ((= i n) #t)
            ((not (char=? (string-ref ext i) #\;)) #f)
            (else
             (let* ((name-start (ows (+ i 1))) (name-end (token-end name-start)))
               (and (> name-end name-start)
                    (let ((i (ows name-end)))
                      (if (and (< i n) (char=? (string-ref ext i) #\=))
                          (let* ((v (ows (+ i 1)))
                                 (v-end (if (and (< v n) (char=? (string-ref ext v) #\")) (quoted-end (+ v 1)) (token-end v))))
                            (and v-end (> v-end v) (loop (ows v-end))))
                          (loop i))))))))))

;; A chunked body. The chunks' sizes, extensions and line ends count
;; against MAX-BODY too; trailers, which are read and dropped, are
;; bounded as the head is.
(define (read-chunked! r max-body max-head max-headers)
  (let loop ((chunks '()) (size 0) (overhead 0))
    (let ((line (read-through! r 1024 400 #f)))
      (when (eof-object? line) (fail 400 "The input ended within a body"))
      (check-line line)
      (let* ((digits-end (let scan ((i 0)) (if (and (< i (string-length line)) (string-index "0123456789abcdefABCDEF" (string-ref line i))) (scan (+ i 1)) i)))
             (digits (substring line 0 digits-end))
             (n (and (> digits-end 0) (<= digits-end 16) (string->number digits 16)))
             (overhead (+ overhead (bytevector-length (string->utf8 line)) 4)))
        (unless (and n (chunk-extensions? (substring line digits-end (string-length line))))
          (fail 400 "A malformed chunk size"))
        (when (or (> (+ size n) max-body) (> overhead max-body)) (fail 413 "The body is too large"))
        (if (= n 0)
            (let trailers ((left max-head) (count 0))
              (let ((t (read-through! r left 431 #f)))
                (cond ((eof-object? t) (fail 400 "The input ended within a body"))
                      ((string=? t "") (concat (reverse chunks)))
                      ((>= count max-headers) (fail 431 "Too many trailers"))
                      ((member (car (parse-field t)) '("content-length" "transfer-encoding" "host"))
                       (fail 400 "A trailer that frames or routes the request"))
                      (else (trailers (- left (bytevector-length (string->utf8 t)) 2) (+ count 1))))))
            (let ((data (read-exactly! r n)))
              (unless (equal? (read-exactly! r 2) #u8(13 10)) (fail 400 "A chunk without its CR LF"))
              (loop (cons data chunks) (+ size n) overhead)))))))

(define (read-request r #:max-head [max-head 16384] #:max-headers [max-headers 100] #:max-body [max-body 1048576]
                      #:continue [continue #f])
  "Read the next request from the reader R, as a dict.
Return the eof object if the input ends before one starts. MAX-HEAD
bounds the head's bytes, MAX-HEADERS the headers and MAX-BODY the body's
bytes. CONTINUE, if given, is called before the body of an HTTP/1.1
request expecting \"100-continue\" is read, to tell the client to send
it. A request this refuses raises an `http-failure` with the status to
answer."
  (let ((head (read-through! r max-head 431 #t)))
    (if (eof-object? head)
        head
        (let* ((lines (string-split head "\r\n"))
               (start (car lines))
               (parts (begin (check-line start) (string-split start " "))))
          (unless (and (= (length parts) 3) (token? (car parts))) (fail 400 "A malformed request line"))
          (let* ((method (car parts)) (target (cadr parts)) (version (caddr parts))
                 (headers (parse-headers (cdr lines) max-headers))
                 (q (string-index target #\?)))
            (unless (member version '("HTTP/1.0" "HTTP/1.1")) (fail 505 "Only HTTP/1.0 and HTTP/1.1 are served"))
            (unless (and (chars-every? (lambda (c) (char<=? #\! c #\~)) target)
                         (or (string-prefix? "/" target) (and (string=? target "*") (string=? method "OPTIONS"))))
              (fail 400 "A request target must be a path"))
            (let ((host (dict-ref headers "host" #f)))
              (cond ((and (string=? version "HTTP/1.1") (not host)) (fail 400 "No host header"))
                    ((and host (not (host? host))) (fail 400 "A malformed host header"))))
            (let* ((te (dict-ref headers "transfer-encoding" #f))
                   (cl (dict-ref headers "content-length" #f))
                   (length (and cl (> (string-length cl) 0) (chars-every? (lambda (c) (char<=? #\0 c #\9)) cl) (string->number cl))))
              (cond ((and te (string=? version "HTTP/1.0")) (fail 400 "Transfer-encoding in HTTP/1.0"))
                    ((and te cl) (fail 400 "Both transfer-encoding and content-length"))
                    ((and te (not (equal? (comma-tokens te) '("chunked")))) (fail 501 "Only chunked transfer-encoding is understood"))
                    ((and cl (not length)) (fail 400 "A malformed content-length"))
                    ((and length (> length max-body)) (fail 413 "The body is too large")))
              (when (and continue (string=? version "HTTP/1.1") (or te (and length (> length 0)))
                         (member "100-continue" (comma-tokens (dict-ref headers "expect" ""))))
                (continue))
              (dict 'method method
                    'path (if q (substring target 0 q) target)
                    'query (and q (substring target (+ q 1) (string-length target)))
                    'version version
                    'headers headers
                    'body (cond (te (read-chunked! r max-body max-head max-headers))
                                (length (read-exactly! r length))
                                (else (bytevector))))))))))

(define (keep-alive? request)
  "Return #t if the connection REQUEST came on stays open after it.
In HTTP/1.1 it does unless the request says \"close\"; in HTTP/1.0 only
if it says \"keep-alive\" and not \"close\"."
  (let ((tokens (comma-tokens (request-header request "connection" ""))))
    (and (not (member "close" tokens))
         (or (string=? (dict-ref request 'version) "HTTP/1.1") (and (member "keep-alive" tokens) #t)))))

;; ----- writing -----

(define reasons
  '((100 . "Continue") (101 . "Switching Protocols") (200 . "OK") (201 . "Created") (202 . "Accepted")
    (204 . "No Content") (205 . "Reset Content") (206 . "Partial Content") (301 . "Moved Permanently") (302 . "Found")
    (303 . "See Other") (304 . "Not Modified") (307 . "Temporary Redirect") (308 . "Permanent Redirect")
    (400 . "Bad Request") (401 . "Unauthorized") (403 . "Forbidden") (404 . "Not Found")
    (405 . "Method Not Allowed") (408 . "Request Timeout") (409 . "Conflict") (410 . "Gone")
    (413 . "Content Too Large") (415 . "Unsupported Media Type") (422 . "Unprocessable Content")
    (429 . "Too Many Requests") (431 . "Request Header Fields Too Large") (500 . "Internal Server Error")
    (501 . "Not Implemented") (502 . "Bad Gateway") (503 . "Service Unavailable") (504 . "Gateway Timeout")
    (505 . "HTTP Version Not Supported")))

(define (response->bytes response #:head [head #f] #:close [close #f])
  "Return RESPONSE, a valid response dict, as the bytes to send.
Its content-length is computed, and a string body has a content-type of
UTF-8 text unless it has one. With HEAD the body is left out, as the
answer to a HEAD request; with CLOSE the connection is said to close.
A header name or value that could end its line is an error."
  (let* ((status (dict-ref response 'status))
         ;; Statuses without content: 204 and 304 say no length, 205 a
         ;; length of 0.
         (empty (memv status '(204 205 304)))
         (body (if empty #u8() (dict-ref response 'body)))
         (body (if (string? body) (string->utf8 body) body))
         (given (dict-ref response 'headers))
         (fields (append-map
                  (lambda (name)
                    (unless (token? name) (error "Not a header name:" name))
                    (let ((values (dict-ref given name)))
                      (map (lambda (value)
                             (unless (chars-every? field-char? value) (error "A control character in the header:" name))
                             (cons name value))
                           (if (string? values) (list values) values))))
                  (dict-keys given)))
         (fields (remove (lambda (f) (member (ascii-downcase (car f)) '("content-length" "transfer-encoding" "connection"))) fields))
         (fields (if (and (string? (dict-ref response 'body)) (not empty)
                          (not (find (lambda (f) (string=? (ascii-downcase (car f)) "content-type")) fields)))
                     (cons (cons "content-type" "text/plain; charset=utf-8") fields)
                     fields))
         (fields (if (memv status '(204 304))
                     fields
                     (append fields (list (cons "content-length" (number->string (bytevector-length body)))))))
         (fields (if close (append fields (list (cons "connection" "close"))) fields))
         (text (string-append "HTTP/1.1 " (number->string status) " " (cond ((assv status reasons) => cdr) (else ""))
                              "\r\n"
                              (apply string-append (map (lambda (f) (string-append (car f) ": " (cdr f) "\r\n")) fields))
                              "\r\n")))
    (if head
        (string->utf8 text)
        (bytevector-append (string->utf8 text) body))))
