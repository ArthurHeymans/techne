;;; IMAP4rev1, serialized commands. Limits are per command, including literals:
;;; 64 MiB total, 32 MiB per literal, 64 KiB per protocol line, 100000 replies.
;;; Production RFC822 bodies remain bytevectors for MIME charset decoding.
;;; No server text or command (and especially no credentials) enters errors.

(define %imap-max-response (* 64 1024 1024))
(define %imap-max-literal (* 32 1024 1024))

;; session: read write close chunk offset next-tag closed busy bytes selected preauth
(define (%imap-fail) (error "imap: malformed, truncated or oversized response"))
(define (%imap-reset! s) (vector-set! s 8 0))
(define (%imap-charge! s n)
  (let ((total (+ (vector-ref s 8) n)))
    (when (> total %imap-max-response) (%imap-fail))
    (vector-set! s 8 total)))

(define (%imap-fill! s)
  (when (= (vector-ref s 4) (bytevector-length (vector-ref s 3)))
    (let ((b ((vector-ref s 0))))
      (unless (and (bytevector? b) (> (bytevector-length b) 0)
                   (<= (bytevector-length b) %imap-max-response))
        (%imap-fail))
      (vector-set! s 3 b)
      (vector-set! s 4 0))))

(define (%imap-byte! s)
  (%imap-fill! s)
  (%imap-charge! s 1)
  (let* ((at (vector-ref s 4)) (b (bytevector-u8-ref (vector-ref s 3) at)))
    (vector-set! s 4 (+ at 1)) b))

(define (%imap-line! s)
  (let loop ((out '()) (n 0))
    (when (> n 65536) (%imap-fail))
    (let ((b (%imap-byte! s)))
      (cond ((= b 13)
             (unless (= (%imap-byte! s) 10) (%imap-fail))
             (utf8->string (apply bytevector (reverse out))))
            ((or (= b 10) (= b 0)) (%imap-fail))
            (else (loop (cons b out) (+ n 1)))))))

(define (%imap-literal! s n)
  (unless (and (integer? n) (<= 0 n %imap-max-literal)) (%imap-fail))
  (%imap-charge! s n)
  (let ((out (make-bytevector n)))
    (let loop ((at 0))
      (if (= at n) out
          (begin
            (%imap-fill! s)
            (let* ((chunk (vector-ref s 3)) (offset (vector-ref s 4))
                   (take (min (- n at) (- (bytevector-length chunk) offset))))
              (bytevector-copy! out at chunk offset (+ offset take))
              (vector-set! s 4 (+ offset take))
              (loop (+ at take))))))))

;; Quoted strings and literals are vectors, never mistaken for syntax.
(define (%imap-tokens line)
  (let ((n (string-length line)))
    (define (delim? c) (memv c '(#\space #\tab #\( #\) #\[ #\])))
    (let scan ((i 0) (out '()))
      (if (= i n) (reverse out)
          (let ((c (string-ref line i)))
            (cond
             ((memv c '(#\space #\tab)) (scan (+ i 1) out))
             ((memv c '(#\( #\) #\[ #\]))
              (scan (+ i 1) (cons (case c ((#\() 'open) ((#\)) 'close)
                                         ((#\[) 'bracket) (else 'end-bracket)) out)))
             ((char=? c #\")
              (let quoted ((j (+ i 1)) (chars '()))
                (when (= j n) (%imap-fail))
                (let ((q (string-ref line j)))
                  (cond ((char=? q #\")
                         (scan (+ j 1) (cons (vector (list->string (reverse chars))) out)))
                        ((char=? q #\\)
                         (when (or (= (+ j 1) n)
                                   (not (memv (string-ref line (+ j 1)) '(#\\ #\"))))
                           (%imap-fail))
                         (quoted (+ j 2) (cons (string-ref line (+ j 1)) chars)))
                        (else (quoted (+ j 1) (cons q chars)))))))
             (else
              (let atom ((j (+ i 1)))
                (if (or (= j n) (delim? (string-ref line j)))
                    (scan j (cons (substring line i j) out))
                    (atom (+ j 1)))))))))))

(define (%imap-decimal s)
  (and (string? s) (> (string-length s) 0)
       (every (lambda (c) (char<=? #\0 c #\9)) (string->list s))
       (string->number s)))

(define (%imap-marker token)
  (and (string? token) (> (string-length token) 2)
       (char=? (string-ref token 0) #\{)
       (char=? (string-ref token (- (string-length token) 1)) #\})
       (%imap-decimal (substring token 1 (- (string-length token) 1)))))

;; resp-text is human text, NOT IMAP data syntax. Quotes, parentheses and
;; trailing {123} there must never be interpreted as strings/literals.
(define (%imap-status-line line)
  (let ((n (string-length line)))
    (define (space? i) (memv (string-ref line i) '(#\space #\tab)))
    (define (end i) (if (and (< i n) (not (space? i))) (end (+ i 1)) i))
    (define (skip i) (if (and (< i n) (space? i)) (skip (+ i 1)) i))
    (let* ((a (end 0)) (b (skip a)) (c (end b))
           (status (substring line b c)))
      (and (member (string-upcase status) '("OK" "NO" "BAD" "BYE" "PREAUTH"))
           (let* ((head (list (substring line 0 a) status)) (text (skip c)))
             (if (and (< text n) (char=? (string-ref line text) #\[))
                 (let ((stop (string-contains (substring line text n) "]")))
                   (unless stop (%imap-fail))
                   (append head (%imap-tokens (substring line text (+ text stop 1)))))
                 head))))))

(define (%imap-response! s)
  ;; Read whole logical responses BEFORE recognizing tags: a literal can
  ;; contain arbitrary CRLF and even a complete, matching tagged response.
  (let gather ((parts '()))
    (let* ((line (%imap-line! s))
           (status (and (null? parts) (%imap-status-line line)))
           (tokens (or status (%imap-tokens line)))
           (back (reverse tokens))
           (size (and (not status) (pair? back) (%imap-marker (car back)))))
      (if size
          (gather (cons (list (vector (%imap-literal! s size)))
                        (cons (reverse (cdr back)) parts)))
          (let ((flat (apply append (reverse (cons tokens parts)))))
            (let parse ((rest flat) (out '()) (stack '()))
              (cond
               ((null? rest)
                (unless (null? stack) (%imap-fail))
                (reverse out))
               ((eq? (car rest) 'open)
                (when (>= (length stack) 32) (%imap-fail))
                (parse (cdr rest) '() (cons out stack)))
               ((eq? (car rest) 'close)
                (when (null? stack) (%imap-fail))
                (parse (cdr rest) (cons (reverse out) (car stack)) (cdr stack)))
               (else (parse (cdr rest) (cons (car rest) out) stack)))))))))

(define (%imap-is? value text) (and (string? value) (string-ci=? value text)))
(define (%imap-text value)
  (cond ((vector? value) (let ((v (vector-ref value 0))) (if (bytevector? v) (utf8->string v) v)))
        ((string? value) value)
        (else (%imap-fail))))

(define (%imap-quote text)
  (unless (and (string? text)
               (every (lambda (c) (and (>= (char->integer c) 32)
                                      (not (= (char->integer c) 127))))
                      (string->list text)))
    (error "imap: control characters are forbidden in quoted arguments"))
  (string-append "\""
                 (apply string-append
                        (map (lambda (c) (if (memv c '(#\\ #\"))
                                             (string #\\ c) (string c)))
                             (string->list text)))
                 "\""))

(define (imap-close s)
  "Close session S idempotently, without CLOSE, EXPUNGE or LOGOUT.
This never changes mailbox contents. Scope cancellation also closes native TLS."
  (unless (vector-ref s 6)
    (vector-set! s 6 #t)
    ((vector-ref s 2))))

(define (%imap-guard s thunk)
  (when (vector-ref s 6) (error "imap: session is closed"))
  (when (vector-ref s 7) (error "imap: concurrent commands on one session are forbidden"))
  (dynamic-wind
    (lambda () (vector-set! s 7 #t))
    (lambda ()
      (guard (e (#t (imap-close s) (raise e)))
        (%imap-reset! s)
        (thunk)))
    (lambda ()
      ;; Cancellation while awaiting a reply must not leave a reusable,
      ;; desynchronized connection. Success explicitly clears busy first.
      (when (vector-ref s 7) (imap-close s))
      (vector-set! s 7 #f))))

(define (%imap-command s command)
  (%imap-guard
   s (lambda ()
       (let* ((n (+ (vector-ref s 5) 1)) (tag (string-append "T" (number->string n))))
         (vector-set! s 5 n)
         ((vector-ref s 1) (string-append tag " " command "\r\n"))
         (let loop ((out '()) (count 0))
           (when (>= count 100000) (%imap-fail))
           (let ((r (%imap-response! s)))
             (unless (and (pair? r) (pair? (cdr r))) (%imap-fail))
             (cond
              ((%imap-is? (car r) tag)
               (unless (%imap-is? (cadr r) "OK")
                 (error (if (%imap-is? (cadr r) "NO") "imap: command rejected (NO)"
                            "imap: command rejected (BAD or invalid status)")))
               (vector-set! s 7 #f)
               (reverse (cons r out)))
              ((and (%imap-is? (car r) "*") (not (%imap-is? (cadr r) "BYE")))
               (loop (cons r out) (+ count 1)))
              (else (error "imap: unexpected response or server disconnected")))))))))

(define (%imap-starttls! s upgrade)
  (define (require-empty-buffer)
    (unless (= (vector-ref s 4) (bytevector-length (vector-ref s 3)))
      (error "imap: unexpected plaintext at STARTTLS boundary")))
  (when (vector-ref s 10) (error "imap: refusing PREAUTH on plaintext connection"))
  (require-empty-buffer)
  ;; STARTTLS itself probes support; no pre-TLS capabilities are cached.
  (%imap-command s "STARTTLS")
  (require-empty-buffer)
  (upgrade))

(define (make-imap-session read write close #:raw-bytes? [raw-bytes? #f] #:starttls [upgrade #f])
  "Consume a greeting and return a session over a trusted transport.
READ returns bytevectors or EOF; WRITE accepts a command string; CLOSE closes.
If UPGRADE is given via STARTTLS, it must establish verified TLS or raise.
No LOGIN is sent here. With RAW-BYTES? #t fetched bodies stay bytevectors.
Commands are serialized; failure/cancellation closes a desynchronized session."
  (let ((s (vector read write close #u8() 0 0 #f #f 0 #f #f raw-bytes?)) (ready? #f))
    (dynamic-wind
      (lambda () #f)
      (lambda ()
        (%imap-guard
         s (lambda ()
             (let ((r (%imap-response! s)))
               (unless (and (>= (length r) 2) (%imap-is? (car r) "*")
                            (or (%imap-is? (cadr r) "OK") (%imap-is? (cadr r) "PREAUTH")))
                 (error "imap: server rejected connection"))
               (vector-set! s 10 (%imap-is? (cadr r) "PREAUTH")))
             (vector-set! s 7 #f)))
        (when upgrade (%imap-starttls! s upgrade))
        (set! ready? #t)
        s)
      (lambda () (unless ready? (imap-close s))))))

(define (imap-connect host port user password #:security [security 'implicit-tls])
  "Connect to HOST:PORT with verified TLS and LOGIN as USER using PASSWORD.
SECURITY is 'implicit-tls (usually 993) or 'starttls (usually 143). There is
no plaintext fallback or OAuth/XOAUTH2. Use an app password where required.
Credentials are not retained or logged."
  ;; Validate before allocating a transport; never interpolate raw input.
  (let* ((login (string-append "LOGIN " (%imap-quote user) " " (%imap-quote password)))
         (c (case security
              ((implicit-tls) (tls-connect host port))
              ((starttls) (tcp-connect host port))
              (else (error "Invalid IMAP security mode"))))
         (s (make-imap-session (lambda () (tcp-read-bytes c))
                               (lambda (text) (tcp-write c text))
                               (lambda () (tcp-close c)) #:raw-bytes? #t
                               #:starttls (and (eq? security 'starttls) (lambda () (tls-upgrade! c host))))))
    (unless (vector-ref s 10) (%imap-command s login))
    s))

(define (imap-mailboxes s)
  "Return session S's mailbox wire names as strings (LIST \"\" \"*\").
Names are IMAP modified-UTF-7 unless the server has enabled UTF-8 externally;
pass these opaque names back to imap-select!. No mailbox decoding is done."
  (filter-map
   (lambda (r)
     (and (>= (length r) 5) (%imap-is? (car r) "*") (%imap-is? (cadr r) "LIST")
          (%imap-text (list-ref r 4))))
   (%imap-command s "LIST \"\" \"*\"")))

(define (imap-select! s mailbox)
  "SELECT MAILBOX in session S; return uidvalidity and count entries.
The caller must discard cached UIDs if UIDVALIDITY changes. Names are safely
quoted, not commands; control characters are refused before any bytes go out."
  (let ((responses (%imap-command s (string-append "SELECT " (%imap-quote mailbox))))
        (validity #f) (count #f))
    (for-each
     (lambda (r)
       (when (and (>= (length r) 3) (%imap-is? (car r) "*")
                  (%imap-is? (list-ref r 2) "EXISTS"))
         (set! count (%imap-decimal (cadr r))))
       (let scan ((tokens r))
         (when (pair? tokens)
           (when (and (eq? (car tokens) 'bracket) (>= (length tokens) 4)
                      (%imap-is? (cadr tokens) "UIDVALIDITY")
                      (eq? (list-ref tokens 3) 'end-bracket))
             (set! validity (%imap-decimal (list-ref tokens 2))))
           (scan (cdr tokens)))))
     responses)
    (unless (and validity (> validity 0) (<= validity 4294967295) count)
      (imap-close s) (%imap-fail))
    (vector-set! s 9 #t)
    `((uidvalidity . ,validity) (count . ,count))))

(define (%imap-uid? n) (and (integer? n) (exact? n) (<= 1 n 4294967295)))

(define (%imap-fetch-row r since)
  (and (>= (length r) 4) (%imap-is? (car r) "*")
       (%imap-is? (list-ref r 2) "FETCH")
       (let ((items (list-ref r 3)) (uid #f) (flags #f) (raw #f))
         (unless (list? items) (%imap-fail))
         (let scan ((xs items))
           (unless (null? xs)
             (cond
              ((and (%imap-is? (car xs) "UID") (pair? (cdr xs)))
               (set! uid (%imap-decimal (cadr xs))) (scan (cddr xs)))
              ((and (%imap-is? (car xs) "FLAGS") (pair? (cdr xs)))
               (unless (list? (cadr xs)) (%imap-fail))
               (set! flags (map %imap-text (cadr xs))) (scan (cddr xs)))
              ((and (%imap-is? (car xs) "BODY") (>= (length xs) 4)
                    (eq? (cadr xs) 'bracket) (eq? (caddr xs) 'end-bracket))
               (unless (vector? (list-ref xs 3)) (%imap-fail))
               (set! raw (vector-ref (list-ref xs 3) 0)) (scan (list-tail xs 4)))
              (else (scan (cdr xs))))))
         ;; Unsolicited flag-only FETCH updates need not include a body.
         (and raw (begin (unless (and (%imap-uid? uid) flags) (%imap-fail))
                         (and (> uid since) `((uid . ,uid) (flags . ,flags) (raw . ,raw))))))))

(define (imap-fetch s [since-uid 0])
  "Return fetched message alists from session S.
Entries are uid, flags and raw. Fetch UIDs strictly greater than SINCE-UID
(default 0), using BODY.PEEK[] so
sync never marks mail seen. SELECT first. Results use UIDs, not sequence
numbers. Production sessions return raw bytevectors; injected sessions default
to UTF-8 text unless constructed with RAW-BYTES? #t. MIME decoding is separate."
  (unless (and (integer? since-uid) (exact? since-uid) (<= 0 since-uid 4294967295))
    (error "imap: invalid since UID"))
  (unless (vector-ref s 9) (error "imap: SELECT a mailbox first"))
  (if (= since-uid 4294967295) '()
      (guard (e (#t (imap-close s) (raise e)))
        (filter-map (lambda (r) (%imap-body-mode s (%imap-fetch-row r since-uid)))
                    (%imap-command s (string-append "UID FETCH " (number->string (+ since-uid 1))
                                                   ":* (UID FLAGS BODY.PEEK[])"))))))

(define (%imap-body-mode s message)
  (and message
       (if (vector-ref s 11) message
           (map (lambda (entry) (if (and (eq? (car entry) 'raw) (bytevector? (cdr entry)))
                                   (cons 'raw (utf8->string (cdr entry))) entry)) message))))

(define (%imap-message-size s uid)
  ;; Match the requested UID, not unsolicited flag updates.
  (define (size-row r)
    (and (>= (length r) 4) (%imap-is? (car r) "*") (%imap-is? (caddr r) "FETCH")
         (let loop ((items (list-ref r 3)) (found #f) (size #f))
           (cond ((not (list? items)) (%imap-fail))
                 ((null? items) (and (eqv? found uid) size))
                 ((null? (cdr items)) #f)
                 (else (loop (cddr items)
                             (if (%imap-is? (car items) "UID") (%imap-decimal (cadr items)) found)
                             (if (%imap-is? (car items) "RFC822.SIZE") (%imap-decimal (cadr items)) size)))))))
  (let ((responses (%imap-command s (string-append "UID FETCH " (number->string uid) " (UID RFC822.SIZE)"))))
    (or (any size-row responses)
        ;; A missing UID can have been expunged since SEARCH, but a FETCH
        ;; for our UID without a size must not silently drop the message.
        (if (any (lambda (r)
                   (and (>= (length r) 4) (%imap-is? (car r) "*") (%imap-is? (caddr r) "FETCH")
                        (let ((items (list-ref r 3)))
                          (and (list? items)
                               (let loop ((items items))
                                 (and (pair? items) (pair? (cdr items))
                                      (or (and (%imap-is? (car items) "UID") (eqv? (%imap-decimal (cadr items)) uid))
                                          (loop (cddr items))))))))) responses)
            (%imap-fail) #f))))

(define (imap-for-each-message s proc [since-uid 0] #:max-bytes [max-bytes #f]
                              #:on-oversized [on-oversized (lambda (uid) #f)])
  "Call PROC with each new message of S, after SINCE-UID, in UID order.
With MAX-BYTES, check RFC822.SIZE first; call ON-OVERSIZED with each UID
above the limit and continue without fetching its body. Fetch bodies one
at a time with PEEK. PROC and ON-OVERSIZED may suspend to persist progress."
  (unless (and (integer? since-uid) (exact? since-uid) (<= 0 since-uid 4294967295))
    (error "imap: invalid since UID"))
  (unless (vector-ref s 9) (error "imap: SELECT a mailbox first"))
  (unless (= since-uid 4294967295)
    (let ((uids
           (sort (delete-duplicates
                  (append-map
                   (lambda (r)
                     (if (and (>= (length r) 2) (%imap-is? (car r) "*") (%imap-is? (cadr r) "SEARCH"))
                         (filter-map (lambda (token)
                                       (let ((uid (%imap-decimal token)))
                                         (unless (%imap-uid? uid) (%imap-fail))
                                         (and (> uid since-uid) uid))) (cddr r)) '()))
                   (%imap-command s (string-append "UID SEARCH UID " (number->string (+ since-uid 1)) ":*")))) <)))
      (for-each
        (lambda (uid)
          (let ((size (and max-bytes (%imap-message-size s uid))))
            (cond ((and max-bytes (not size)) #f)
                  ((and max-bytes (> size max-bytes)) (on-oversized uid))
                  (else
                   (for-each (lambda (m)
                               (if (and max-bytes (> (bytevector-length (cdr (assq 'raw m))) max-bytes))
                                   (on-oversized uid) (proc (%imap-body-mode s m))))
                             (filter-map (lambda (r)
                                           (let ((message (%imap-fetch-row r since-uid)))
                                             (and message (= (cdr (assq 'uid message)) uid) message)))
                                         (%imap-command s (string-append "UID FETCH " (number->string uid) " (UID FLAGS BODY.PEEK[])"))))))))
       uids))))

(define (%imap-flag flag)
  (unless (and (string? flag) (> (string-length flag) 0)
               (or (member flag '("\\Seen" "\\Answered" "\\Flagged" "\\Deleted" "\\Draft"))
                   (every (lambda (c) (or (char<=? #\a c #\z) (char<=? #\A c #\Z)
                                         (char<=? #\0 c #\9) (memv c '(#\- #\_ #\.))))
                          (string->list flag))))
    (error "imap: invalid or read-only flag"))
  flag)

(define (imap-store-flags! s uid flags)
  "Replace precisely UID's FLAGS in session S, never a range.
FLAGS may contain writable system flags or safe ASCII keywords; \\Recent is
server-owned and refused. An empty list clears all flags. SELECT first."
  (unless (%imap-uid? uid) (error "imap: invalid UID"))
  (unless (vector-ref s 9) (error "imap: SELECT a mailbox first"))
  (let ((safe (map %imap-flag flags)))
    (%imap-command s (string-append "UID STORE " (number->string uid) " FLAGS.SILENT ("
                                   (string-join safe " ") ")"))
    #t))
