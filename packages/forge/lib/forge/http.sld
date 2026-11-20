;;; Native Scheme HTTP/1.1 + HTTPS client over `tls-connect` / `tcp-connect`.
;;; Supports REST and GraphQL, chunked transfer encoding, Link pagination,
;;; rate-limit headers, and an injectable transport for offline testing.
(define-library (forge http)
  (export forge-http-request forge-http-json forge-http-link-next forge-http-rate-limit
          forge-http-error? forge-http-error-status forge-http-error-message forge-http-error-body
          forge-url-encode forge-query-string)
  (import (techne) (techne task))
  (begin
    (define %max-body-bytes (* 16 1024 1024))

    (define-record-type forge-http-error
      (make-forge-http-error status message body)
      forge-http-error?
      (status forge-http-error-status)
      (message forge-http-error-message)
      (body forge-http-error-body))

    (define (unreserved-byte? b)
      (or (<= 65 b 90) (<= 97 b 122) (<= 48 b 57) (= b 45) (= b 95) (= b 46) (= b 126)))

    (define %hex "0123456789ABCDEF")

    (define (forge-url-encode s)
      "Percent-encode string S for use in a URL query parameter."
      (let* ((bv (string->utf8 s))
             (n (bytevector-length bv)))
        (let loop ((i 0) (out '()))
          (if (= i n)
              (apply string-append (reverse out))
              (let ((b (bytevector-u8-ref bv i)))
                (if (unreserved-byte? b)
                    (loop (+ i 1) (cons (string (integer->char b)) out))
                    (loop (+ i 1)
                          (cons (string #\%
                                        (string-ref %hex (quotient b 16))
                                        (string-ref %hex (remainder b 16)))
                                out))))))))

    (define (forge-query-string params)
      "Encode query alist PARAMS into a URL query string."
      (string-join
       (filter-map (lambda (kv)
                     (and (cdr kv)
                          (string-append (forge-url-encode (car kv)) "="
                                         (forge-url-encode (if (number? (cdr kv))
                                                               (number->string (cdr kv))
                                                               (cdr kv))))))
                   params)
       "&"))

    (define (bv-concat chunks)
      (let* ((total (apply + (map bytevector-length chunks)))
             (out (make-bytevector total)))
        (let loop ((cs chunks) (pos 0))
          (unless (null? cs)
            (let* ((c (car cs)) (len (bytevector-length c)))
              (bytevector-copy! out pos c 0 len)
              (loop (cdr cs) (+ pos len)))))
        out))

    (define (bv-sub bv start end)
      (let* ((len (max 0 (- end start)))
             (out (make-bytevector len)))
        (when (> len 0) (bytevector-copy! out 0 bv start end))
        out))

    ;; Find \r\n\r\n in bytevector BV starting at START.
    (define (find-crlf-crlf bv start)
      (let ((limit (- (bytevector-length bv) 3)))
        (let loop ((i start))
          (cond ((>= i limit) #f)
                ((and (= (bytevector-u8-ref bv i) 13)
                      (= (bytevector-u8-ref bv (+ i 1)) 10)
                      (= (bytevector-u8-ref bv (+ i 2)) 13)
                      (= (bytevector-u8-ref bv (+ i 3)) 10))
                 i)
                (else (loop (+ i 1)))))))

    (define (find-crlf bv start)
      (let ((limit (- (bytevector-length bv) 1)))
        (let loop ((i start))
          (cond ((>= i limit) #f)
                ((and (= (bytevector-u8-ref bv i) 13)
                      (= (bytevector-u8-ref bv (+ i 1)) 10))
                 i)
                (else (loop (+ i 1)))))))

    (define (has-crlf? s)
      (and (string? s)
           (or (string-index s #\return) (string-index s #\newline))))

    (define (validate-header! k v)
      (when (or (not (string? k)) (not (string? v)) (has-crlf? k) (has-crlf? v)
                (string=? (string-trim k) "") (string-index k #\:))
        (error "Invalid HTTP header" k)))

    (define (decode-chunked bv)
      (let ((len (bytevector-length bv)))
        (let loop ((pos 0) (chunks '()) (total 0))
          (let ((eol (find-crlf bv pos)))
            (unless eol (error "Truncated HTTP chunked response: missing terminator"))
            (let* ((line (string-trim (utf8->string (bv-sub bv pos eol))))
                   (semi (string-index line #\;))
                   (hex (if semi (string-trim (substring line 0 semi)) line))
                   (size (string->number hex 16)))
              (cond
                ((or (not (integer? size)) (< size 0)) (error "Invalid HTTP chunk size" line))
                ((= size 0) (bv-concat (reverse chunks)))
                ((> (+ total size) %max-body-bytes) (error "HTTP response exceeds size limit"))
                (else
                 (let ((data-start (+ eol 2))
                       (data-end (+ eol 2 size)))
                   (if (> (+ data-end 2) len)
                       (error "Truncated HTTP chunked response")
                       (loop (+ data-end 2) (cons (bv-sub bv data-start data-end) chunks) (+ total size)))))))))))

    (define (parse-headers header-text)
      (let* ((lines (string-split header-text "\r\n"))
             (status-line (car lines))
             (parts (string-split status-line " "))
             (status (and (>= (length parts) 2) (string->number (cadr parts)))))
        (unless (integer? status) (error "Invalid HTTP status line" status-line))
        (let ((headers (make-hash-table)))
          (for-each
           (lambda (line)
             (let ((colon (string-index line #\:)))
               (when colon
                 (let ((k (string-downcase (string-trim (substring line 0 colon))))
                       (v (string-trim (substring line (+ colon 1) (string-length line)))))
                   (let ((prev (hash-table-ref/default headers k #f)))
                     (hash-table-set! headers k (if prev (string-append prev ", " v) v)))))))
           (cdr lines))
          (cons status headers))))

    (define (read-all-response conn)
      (let loop ((chunks '()) (total 0))
        (let ((chunk (tcp-read-bytes conn)))
          (cond
            ((eof-object? chunk) (bv-concat (reverse chunks)))
            ((> (+ total (bytevector-length chunk)) %max-body-bytes)
             (error "HTTP response exceeds size limit"))
            (else (loop (cons chunk chunks) (+ total (bytevector-length chunk))))))))

    (define (default-transport req)
      (let* ((scheme (dict-ref req "scheme" "https"))
             (host (dict-ref req "host"))
             (port (dict-ref req "port" (if (string=? scheme "http") 80 443)))
             (method (dict-ref req "method" "GET"))
             (path (dict-ref req "path" "/"))
             (headers (dict-ref req "headers" '()))
             (body (dict-ref req "body" #f)))
        (when (or (has-crlf? method) (has-crlf? path) (has-crlf? host)
                  (string-index path #\space))
          (error "Invalid HTTP request target or method"))
        (for-each (lambda (kv)
                    (validate-header! (car kv) (cdr kv))
                    (when (and (string=? scheme "http")
                               (string-ci=? (car kv) "authorization")
                               (not (member host '("127.0.0.1" "localhost" "::1"))))
                      (error "Refusing to send Authorization header over plain HTTP")))
                  headers)
        (let* ((body-bv (cond ((not body) #u8())
                              ((bytevector? body) body)
                              ((string? body) (string->utf8 body))
                              (else (string->utf8 (json->string body)))))
               (host-header (if (or (and (string=? scheme "https") (= port 443))
                                    (and (string=? scheme "http") (= port 80)))
                                host
                                (string-append host ":" (number->string port))))
               (head-lines
                (append
                 (list (string-append method " " path " HTTP/1.1")
                       (string-append "Host: " host-header)
                       "User-Agent: techne-forge/1.0"
                       "Connection: close")
                 (if (or (> (bytevector-length body-bv) 0) (member method '("POST" "PUT" "PATCH")))
                     (list (string-append "Content-Length: " (number->string (bytevector-length body-bv))))
                     '())
                 (map (lambda (kv) (string-append (car kv) ": " (cdr kv))) headers)))
               (head-bytes (string->utf8 (string-append (string-join head-lines "\r\n") "\r\n\r\n")))
               (conn (if (string=? scheme "http")
                         (tcp-connect host port)
                         (tls-connect host port))))
          (dynamic-wind
            (lambda () #f)
            (lambda ()
              (tcp-write conn head-bytes)
              (when (> (bytevector-length body-bv) 0) (tcp-write conn body-bv))
              (let ((raw (read-all-response conn)))
                ;; Skip any 1xx interim responses before the final response.
                (let parse-response ((offset 0))
                  (let ((split (find-crlf-crlf raw offset)))
                    (unless split (error "Malformed HTTP response: missing header terminator"))
                    (let* ((parsed (parse-headers (utf8->string (bv-sub raw offset split))))
                           (status (car parsed))
                           (resp-headers (cdr parsed)))
                      (if (and (>= status 100) (< status 200))
                          (parse-response (+ split 4))
                          (let* ((raw-body (bv-sub raw (+ split 4) (bytevector-length raw)))
                                 (te (hash-table-ref/default resp-headers "transfer-encoding" ""))
                                 (cl (hash-table-ref/default resp-headers "content-length" #f))
                                 (body-bytes
                                  (cond ((string-contains (string-downcase te) "chunked")
                                         (decode-chunked raw-body))
                                        ((and cl (string->number (string-trim cl)))
                                         => (lambda (n)
                                              (when (< (bytevector-length raw-body) n)
                                                (error "Truncated HTTP response body"))
                                              (bv-sub raw-body 0 n)))
                                        (else raw-body))))
                            (dict "status" status
                                  "headers" resp-headers
                                  "body" (utf8->string body-bytes)))))))))
            (lambda ()
              (guard (e ((not (task-cancelled-condition? e)) #f))
                (tcp-close conn)))))))

    (define (forge-http-request host path #:method [method "GET"] #:scheme [scheme "https"]
                                #:port [port #f] #:headers [headers '()] #:body [body #f]
                                #:query [query #f] #:transport [transport #f])
      "Perform an HTTP METHOD request to HOST at PATH.
Uses SCHEME, PORT, HEADERS, BODY, QUERY, and optional TRANSPORT.
Returns a dict with \"status\", \"headers\", and \"body\"."
      (let* ((full-path (if (and query (> (string-length query) 0))
                            (string-append path (if (string-index path #\?) "&" "?") query)
                            path))
             (req (dict "scheme" scheme
                        "host" host
                        "port" (or port (if (string=? scheme "http") 80 443))
                        "method" method
                        "path" full-path
                        "headers" headers
                        "body" body))
             (fn (or transport default-transport)))
        (fn req)))

    (define (forge-http-json host path #:method [method "GET"] #:scheme [scheme "https"]
                             #:port [port #f] #:headers [headers '()] #:body [body #f]
                             #:query [query #f] #:transport [transport #f])
      "Perform a JSON METHOD request to HOST at PATH.
Uses SCHEME, PORT, HEADERS, BODY, QUERY, and optional TRANSPORT.
Returns (status headers parsed-json) or raises `forge-http-error`."
      (let* ((all-headers (append (list (cons "Accept" "application/vnd.github+json")
                                        (cons "X-GitHub-Api-Version" "2022-11-28"))
                                  (if body (list (cons "Content-Type" "application/json")) '())
                                  headers))
             (resp (forge-http-request host path #:method method #:scheme scheme #:port port
                                       #:headers all-headers #:body body #:query query #:transport transport))
             (status (dict-ref resp "status" 0))
             (resp-headers (dict-ref resp "headers" (make-hash-table)))
             (raw-body (dict-ref resp "body" ""))
             (nonempty (and (string? raw-body) (> (string-length (string-trim raw-body)) 0)))
             (parsed (if nonempty
                         (guard (e (#t (list 'invalid-json raw-body))) (string->json raw-body))
                         #f)))
        (when (or (< status 200) (>= status 300))
          (let ((msg (if (and (dict? parsed) (string? (dict-ref parsed "message" #f)))
                         (dict-ref parsed "message")
                         (string-append "HTTP " (number->string status)))))
            (raise (make-forge-http-error status msg (if (and (pair? parsed) (eq? (car parsed) 'invalid-json)) raw-body parsed)))))
        (when (and (pair? parsed) (eq? (car parsed) 'invalid-json))
          (raise (make-forge-http-error status "Invalid JSON response from server" raw-body)))
        (list status resp-headers parsed)))

    (define (forge-http-link-next headers)
      "Extract the rel=\"next\" URL from response HEADERS, or #f."
      (let ((link (and (hash-table? headers) (hash-table-ref/default headers "link" #f))))
        (and (string? link)
             (let ((parts (string-split link ",")))
               (let ((next (find (lambda (p) (string-contains p "rel=\"next\"")) parts)))
                 (and next
                      (let ((lt (string-index next #\<))
                            (gt (string-index next #\>)))
                        (and lt gt (< lt gt) (substring next (+ lt 1) gt)))))))))

    (define (forge-http-rate-limit headers)
      "Return (remaining . reset) from GitHub rate-limit HEADERS, or #f."
      (and (hash-table? headers)
           (let ((rem (hash-table-ref/default headers "x-ratelimit-remaining" #f))
                 (rst (hash-table-ref/default headers "x-ratelimit-reset" #f)))
             (and rem rst (cons (string->number rem) (string->number rst))))))))
