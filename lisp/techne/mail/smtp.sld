;;; SMTP submission is Scheme. TLS is the network host's small mechanism.
(define-library (techne mail smtp)
  (export mail-submit! mail-compose-message mail-addresses smtp-session smtp-submit!)
  (import (techne) (techne crypto) (techne uuid) (techne time) (techne mail address))
  (begin
    (define (clean-header text)
      (unless (every (lambda (c) (let ((n (char->integer c))) (and (or (>= n 32) (= n 9)) (not (= n 127)))))
                     (string->list text))
        (error "A mail header must be one line without controls"))
      text)

    (define (mail-addresses text)
      "Extract ASCII envelope addresses from mailbox list TEXT.
Quoted commas, escaped names, comments and mailbox boundaries share the
header formatter's Scheme parser. Header/SMTP injection is refused."
      (map (lambda (mailbox) (cdr (assq 'address mailbox))) (mail-parse-addresses text)))

    (define (encoded-header text)
      (clean-header text)
      (if (and (<= (string-length text) 60) (every (lambda (c) (< (char->integer c) 127)) (string->list text))) text
          (string-join
           (let loop ((at 0) (out '()))
             (if (>= at (string-length text)) (reverse out)
                 (let ((end (min (string-length text) (+ at 10))))
                   (loop end (cons (string-append "=?UTF-8?B?" (base64-encode (string->utf8 (substring text at end))) "?=") out)))))
           "\r\n ")))

    (define (address-header text)
      (string-join
       (map (lambda (mailbox)
              (let* ((address (cdr (assq 'address mailbox))) (name (cdr (assq 'name mailbox)))
                     (encoded (encoded-header name)))
                (if (string=? name "") address
                    (string-append
                     (if (string=? encoded name)
                         (string-append "\"" (apply string-append
                              (map (lambda (c) (if (memv c '(#\\ #\")) (string #\\ c) (string c))) (string->list name))) "\"")
                         encoded)
                     " <" address ">")))) (mail-parse-addresses text)) ",\r\n "))

    (define (rfc-date)
      (let* ((c (instant->civil (current-milliseconds) "UTC"))
             (two (lambda (n) (if (< n 10) (string-append "0" (number->string n)) (number->string n)))))
        (string-append (number->string (list-ref c 2)) " "
                       (list-ref '("Jan" "Feb" "Mar" "Apr" "May" "Jun" "Jul" "Aug" "Sep" "Oct" "Nov" "Dec") (- (cadr c) 1))
                       " " (number->string (car c)) " " (two (list-ref c 3)) ":" (two (list-ref c 4)) ":" (two (list-ref c 5)) " +0000")))

    (define (mail-compose-message from to subject body #:cc [cc ""] #:reply-to [reply-to ""] #:references [references ""])
      "Build an RFC message from FROM, TO, SUBJECT and plain BODY.
CC, REPLY-TO (the parent Message-ID) and REFERENCES are optional. UTF-8 body is
base64 encoded; Bcc is envelope-only and must never be supplied as a header."
      (unless (= (length (mail-addresses from)) 1) (error "Exactly one From address is required"))
      (mail-addresses to) (mail-addresses cc)
      (let* ((body (string-join (string-split (string-join (string-split body "\r\n") "\n") "\n") "\r\n"))
             (encoded (base64-encode (string->utf8 body)))
             (lines (let loop ((at 0) (out '()))
                      (if (>= at (string-length encoded)) (reverse out)
                          (let ((end (min (+ at 76) (string-length encoded))))
                            (loop end (cons (substring encoded at end) out)))))))
        (string-append "From: " (address-header from) "\r\nTo: " (address-header to)
                       (if (string=? cc "") "" (string-append "\r\nCc: " (address-header cc)))
                       "\r\nSubject: " (encoded-header subject) "\r\nDate: " (rfc-date)
                       "\r\nMessage-ID: <" (uuid-v4) "@techne.invalid>"
                       (if (string=? reply-to "") "" (string-append "\r\nIn-Reply-To: " (clean-header reply-to)))
                       (if (string=? references "") "" (string-append "\r\nReferences: " (clean-header references)))
                       "\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n"
                       (string-join lines "\r\n") "\r\n")))

    (define (smtp-session read write close)
      "Make an SMTP transport from READ, WRITE and CLOSE procedures.
READ returns a string chunk or #f at EOF. Tests can inject a transcript;
production must use a certificate-verified TLS connection."
      (vector read write close ""))

    (define (smtp-line s)
      (let loop ()
        (let* ((buffer (vector-ref s 3)) (end (string-contains buffer "\r\n")))
          (cond (end (let ((line (substring buffer 0 end)))
                       (vector-set! s 3 (substring buffer (+ end 2) (string-length buffer))) line))
                ((> (string-length buffer) 65536) (error "SMTP response exceeds limit"))
                (else (let ((chunk ((vector-ref s 0))))
                        (unless (and (string? chunk) (> (string-length chunk) 0)) (error "SMTP connection closed"))
                        (vector-set! s 3 (string-append buffer chunk)) (loop)))))))

    (define (smtp-response s expected [extension (lambda (text) #f)])
      (let loop ((count 0))
        (when (> count 100) (error "Too many SMTP response lines"))
        (let* ((line (smtp-line s)) (code (and (>= (string-length line) 4) (substring line 0 3))))
          (unless (and code (string=? code (number->string expected))
                       (memv (string-ref line 3) '(#\space #\-)))
            ;; Do not echo arbitrary server text or credentials into logs.
            (error "SMTP server refused the operation" (or (and code (string->number code)) "invalid response")))
          ;; An EHLO reply's first line is a greeting, not an extension.
          (when (> count 0) (extension (substring line 4 (string-length line))))
          (if (char=? (string-ref line 3) #\-) (loop (+ count 1)) expected))))

    (define (smtp-command s command expected [extension (lambda (text) #f)])
      ((vector-ref s 1) (string-append command "\r\n"))
      (smtp-response s expected extension))

    (define (smtp-starttls! s upgrade)
      (let ((offered? #f))
        (unless (string=? (vector-ref s 3) "") (error "Unexpected plaintext before SMTP EHLO"))
        (smtp-command s "EHLO techne.invalid" 250
                      (lambda (text) (when (string-ci=? text "STARTTLS") (set! offered? #t))))
        (unless offered? (error "SMTP server did not advertise STARTTLS"))
        (unless (string=? (vector-ref s 3) "") (error "Unexpected plaintext before SMTP STARTTLS"))
        (smtp-command s "STARTTLS" 220)
        (unless (string=? (vector-ref s 3) "") (error "Unexpected plaintext after SMTP STARTTLS"))
        (upgrade)))

    (define (smtp-submit! s user password from recipients raw #:starttls [upgrade #f] #:on-data [on-data (lambda () #t)])
      "Submit RAW through SMTP session S as FROM to RECIPIENTS.
S must use verified TLS, or UPGRADE (passed via STARTTLS) must be a procedure
that establishes verified TLS or raises. USER and PASSWORD go only over TLS.
ON-DATA runs immediately before writing the body: a failure before it is
definitely not submitted; a failure after it may be ambiguous. Return #t
only after DATA is accepted. No automatic retries occur."
      (unless (and (= (length (mail-addresses from)) 1) (pair? recipients)) (error "Missing SMTP envelope"))
      (for-each (lambda (a) (unless (equal? (mail-addresses a) (list a)) (error "Invalid SMTP recipient"))) recipients)
      (smtp-response s 220)
      (when upgrade (smtp-starttls! s upgrade))
      ;; STARTTLS resets SMTP state; do not use pre-upgrade capabilities.
      (smtp-command s "EHLO techne.invalid" 250)
      (unless (string=? user "")
        (when (or (string-index user #\x0) (string-index password #\x0)) (error "Invalid authentication value"))
        (smtp-command s (string-append "AUTH PLAIN " (base64-encode (string->utf8 (string-append "\x0;" user "\x0;" password)))) 235))
      (smtp-command s (string-append "MAIL FROM:<" (car (mail-addresses from)) ">") 250)
      (for-each (lambda (a) (smtp-command s (string-append "RCPT TO:<" a ">") 250)) recipients)
      (smtp-command s "DATA" 354)
      (on-data)
      (let* ((normalized (string-join (string-split (string-join (string-split raw "\r\n") "\n") "\n") "\n"))
             (lines (string-split normalized "\n"))
             (stuffed (string-join (map (lambda (line) (if (string-prefix? "." line) (string-append "." line) line)) lines) "\r\n")))
        ((vector-ref s 1) (string-append stuffed (if (string-suffix? "\r\n" stuffed) "" "\r\n") ".\r\n")))
      (smtp-response s 250)
      ;; DATA acceptance is authoritative: close without another network wait.
      #t)

    (define (mail-submit! host port user password from recipients raw #:security [security 'implicit-tls] #:on-data [on-data (lambda () #t)])
      "Submit RAW to HOST on PORT over verified TLS.
SECURITY is 'implicit-tls (usually 465) or 'starttls (usually 587). There is
no plaintext fallback. USER and PASSWORD authenticate; FROM and RECIPIENTS
are envelope addresses. See smtp-submit! for ON-DATA and delivery semantics."
      (let ((c (case security
                 ((implicit-tls) (tls-connect host port))
                 ((starttls) (tcp-connect host port))
                 (else (error "Invalid SMTP security mode")))))
        (call-with-socket c
          (lambda (c)
            (smtp-submit! (smtp-session (lambda () (tcp-read c)) (lambda (text) (tcp-write c text)) (lambda () (tcp-close c)))
                          user password from recipients raw
                          #:starttls (and (eq? security 'starttls) (lambda () (tls-upgrade! c host)))
                          #:on-data on-data)))))))
