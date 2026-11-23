;;; Run: techne run packages/mail/tests/mail-backend-tests.scm
(import (techne test))
(import (techne mail imap) (techne mail maildir) (techne mail mime) (techne uuid))

(define (field row key) (cdr (assq key row)))
(define (rejected? thunk)
  (guard (e (#t #t)) (thunk) #f))

;; Return a session, an output accessor, and a closed? accessor. Bytes can
;; be fragmented anywhere, including CRLF, UTF-8 and literal length markers.
(define (fake text [size 65536] #:raw-bytes? [raw-bytes? #f])
  (let ((bytes (if (bytevector? text) text (string->utf8 text))) (at 0) (writes '()) (closed #f))
    (let ((s (make-imap-session
              (lambda ()
                (if (= at (bytevector-length bytes)) (eof-object)
                    (let* ((end (min (+ at size) (bytevector-length bytes)))
                           (part (bytevector-copy bytes at end)))
                      (set! at end) part)))
              (lambda (text) (set! writes (cons text writes)))
              (lambda () (set! closed #t)) #:raw-bytes? raw-bytes?)))
      (list s (lambda () (reverse writes)) (lambda () closed)))))
(define (try-imap-starttls chunks upgrade)
  (let ((writes '()) (closed? #f))
    (let ((mailboxes
           (guard (e (#t #f))
             (imap-mailboxes
              (make-imap-session
               (lambda () (if (null? chunks) (eof-object)
                              (let ((c (car chunks))) (set! chunks (cdr chunks)) (string->utf8 c))))
               (lambda (text) (set! writes (cons text writes)))
               (lambda () (set! closed? #t))
               #:starttls (lambda () (set! writes (cons 'upgrade writes)) (upgrade)))))))
      (list mailboxes (reverse writes) closed?))))
(define plain-starttls "T1 STARTTLS\r\n")
(check "IMAP upgrades in place without a second greeting and keeps command tags"
       (list '("INBOX") (list plain-starttls 'upgrade "T2 LIST \"\" \"*\"\r\n") #f)
       (try-imap-starttls '("* OK greeting\r\n" "T1 OK Begin TLS\r" "\n" "* LIST () NIL INBOX\r\nT2 OK listed\r\n") (lambda () #t)))
(check "IMAP rejects plaintext PREAUTH" '(#f () #t)
       (try-imap-starttls '("* PREAUTH greeting\r\n") (lambda () #t)))
(for-each
 (lambda (reply)
   (check "IMAP rejects refusal or extra plaintext and closes" (list #f (list plain-starttls) #t)
          (try-imap-starttls (list "* OK greeting\r\n" reply) (lambda () #t))))
 '("T1 BAD unsupported\r\n" "T1 NO unavailable\r\n" "T1 OK Begin TLS\r\n* OK injected\r\n"))
(check "IMAP closes after failed handshake" (list #f (list plain-starttls 'upgrade) #t)
       (try-imap-starttls '("* OK greeting\r\n" "T1 OK Begin TLS\r\n") (lambda () (error "certificate rejected"))))

(define selected "* 2 EXISTS\r\n* OK [UIDVALIDITY 321] valid\r\nT1 OK selected\r\n")
(define raw "From: a@example.test\r\nSubject: λ\r\n\r\nT2 OK not-a-tag\r\n* BYE not-a-disconnect\r\n\x0;tail")
(define fetch-reply
  (string-append "* 77 FETCH (FLAGS (\\Seen $label) BODY[] {"
                 (number->string (bytevector-length (string->utf8 raw))) "}\r\n"
                 raw " UID 900)\r\n"
                 "* 78 FETCH (UID 901 FLAGS () BODY[] {0}\r\n)\r\n"
                 "* 77 FETCH (UID 900 FLAGS (\\Seen))\r\n"
                 "T2 OK fetched\r\nT3 OK stored\r\n"))
(for-each
 (lambda (size)
   (let* ((f (fake (string-append "* OK ready\r\n" selected fetch-reply) size)) (s (car f)))
     (check "select metadata" '((uidvalidity . 321) (count . 2)) (imap-select! s "INBOX"))
     (let ((rows (imap-fetch s 899)))
       (check "UID is not sequence number" '(900 901) (map (lambda (r) (field r 'uid)) rows))
       (check "literal preserves embedded tags and UTF-8" raw (field (car rows) 'raw))
       (check "fetch flags" '("\\Seen" "$label") (field (car rows) 'flags))
       (check "empty literal and flags" '("" ()) (list (field (cadr rows) 'raw) (field (cadr rows) 'flags))))
     (check "store result" #t (imap-store-flags! s 900 '("\\Seen" "custom")))
     (check "safe wire commands"
            '("T1 SELECT \"INBOX\"\r\n" "T2 UID FETCH 900:* (UID FLAGS BODY.PEEK[])\r\n"
              "T3 UID STORE 900 FLAGS.SILENT (\\Seen custom)\r\n")
            ((cadr f)))
     (imap-close s) (imap-close s)
     (check "closed idempotently" #t ((caddr f)))
     (check "closed session refused" #t (rejected? (lambda () (imap-mailboxes s))))))
 '(1 2 3 7 65536))

(let* ((f (fake "* PREAUTH ready\r\n* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n* LIST () NIL {6}\r\na\"b\\cd\r\nT1 OK list\r\nT2 OK selected\r\n" 1))
       (s (car f)))
  (check "LIST quoted and literal names" '("INBOX" "a\"b\\cd") (imap-mailboxes s))
  ;; This lacks SELECT metadata, but the command's escaping is still checked.
  (check "missing select metadata refused" #t (rejected? (lambda () (imap-select! s "a\"b\\cd"))))
  (check "mailbox quotes and backslashes escaped" "T2 SELECT \"a\\\"b\\\\cd\"\r\n" (cadr ((cadr f)))))

(for-each
 (lambda (name)
   (let* ((f (fake "* OK ready\r\n")) (s (car f)))
     (check "mailbox injection rejected" #t (rejected? (lambda () (imap-select! s name))))
     (check "injection writes nothing" '() ((cadr f)))
     (check "validation leaves session usable" #f ((caddr f)))))
 '("INBOX\r\nT1 LOGOUT" "INBOX\n" "a\x0;b"))

(for-each
 (lambda (response)
   (let* ((f (fake (string-append "* OK ready\r\n" response) 1)) (s (car f)))
     (check "rejected or broken command" #t (rejected? (lambda () (imap-mailboxes s))))
     (check "broken session closed" #t ((caddr f)))))
 '("T1 NO no credentials echoed\r\n" "T1 BAD invalid\r\n" "* BYE goodbye\r\n"
   "T2 OK wrong tag\r\n" "+ continuation\r\n" "T1 OK" "* LIST () NIL {33554433}\r\n"
   "* LIST () NIL {10}\r\nshort" "* LIST () NIL \"unterminated\r\n"
   "* LIST () NIL \"bad\\q\"\r\n" "* LIST (() NIL x\r\n"))

(let* ((f (fake "* OK welcome (unclosed \"quote {123}\r\n* LIST () NIL \"INBOX\"\r\nT1 OK human text ) \" {999}\r\n" 1))
       (s (car f)))
  (check "status human text is not protocol syntax" '("INBOX") (imap-mailboxes s)))
(let* ((f (fake "* OK ready\r\n* 0 EXISTS\r\nT1 OK [UIDVALIDITY 42] selected (\r\n")) (s (car f)))
  (check "UIDVALIDITY in tagged response and empty mailbox" '((uidvalidity . 42) (count . 0))
         (imap-select! s "empty")))
(let* ((f (fake (string-append "* OK ready\r\n* LIST () NIL " (make-string 65537 #\x) "\r\n")))
       (s (car f)))
  (check "oversized protocol line rejected" #t (rejected? (lambda () (imap-mailboxes s)))))

(check "greeting rejection" #t (rejected? (lambda () (fake "* BYE refused\r\n"))))
(check "plaintext greeting rejected" #t (rejected? (lambda () (fake "hello\r\n"))))
(let* ((f (fake (string-append "* OK ready\r\n" selected))) (s (car f)))
  (imap-select! s "INBOX")
  (for-each (lambda (uid)
              (check "unsafe UID refused" #t (rejected? (lambda () (imap-store-flags! s uid '())))))
            '(0 -1 4294967296 "1:*" 1.5))
  (for-each (lambda (flag)
              (check "unsafe/read-only flag refused" #t
                     (rejected? (lambda () (imap-store-flags! s 1 (list flag))))))
            '("\\Recent" "\\Seen)\r\nT9 LOGOUT" "a b" ""))
  (check "validation never issued STORE" 1 (length ((cadr f))))
  (check "max UID has no successor" '() (imap-fetch s 4294967295)))

;; A reversed 901:* server range can include UID 900 when no new mail
;; exists; filter by UID, never treat the old row as a new message.
(let* ((f (fake (string-append "* OK ready\r\n" selected
                              "* 1 FETCH (UID 900 FLAGS () BODY[] {0}\r\n)\r\nT2 OK done\r\n")))
       (s (car f)))
  (imap-select! s "INBOX")
  (check "UID range reversal filtered" '() (imap-fetch s 900)))

(let* ((body (bytevector-append (string->utf8 "Content-Type: text/plain; charset=iso-8859-1\r\n\r\ncaf") #u8(233)))
       (wire (bytevector-append (string->utf8 (string-append "* OK ready\r\n" selected
                                                         "* 1 FETCH (UID 900 FLAGS () BODY[] {" (number->string (bytevector-length body)) "}\r\n"))
                               body (string->utf8 ")\r\nT2 OK fetched\r\n")))
       (f (fake wire 1 #:raw-bytes? #t)) (s (car f)))
  (imap-select! s "INBOX")
  (let ((raw (field (car (imap-fetch s)) 'raw)))
    (check "production preserves non-UTF8 octets" body raw)
    (check "MIME charset decodes preserved octets" "café" (mail-text (mail-parse raw)))))
(let* ((f (fake (string-append "* OK ready\r\n" selected
                               "* 2 FETCH (UID 901)\r\nT2 OK upper bound\r\n"
                               "* SEARCH 901 900 899\r\nT3 OK searched\r\n"
                               "* 3 FETCH (UID 999 FLAGS () BODY[] {0}\r\n)\r\n* 1 FETCH (UID 900 FLAGS () BODY[] {0}\r\n)\r\nT4 OK fetched\r\n"
                               "* 2 FETCH (UID 901 FLAGS () BODY[] {3}\r\nbye)\r\nT5 OK fetched\r\n")))
       (s (car f)) (rows '()))
  (imap-select! s "INBOX")
  (imap-for-each-message s (lambda (m) (set! rows (append rows (list m)))) 899)
  (check "incremental callback visits sorted new UIDs" '(900 901) (map (lambda (m) (field m 'uid)) rows))
  (check "one fetch body per command" '("T1 SELECT \"INBOX\"\r\n" "T2 UID FETCH * (UID)\r\n" "T3 UID SEARCH UID 900:901\r\n"
                                      "T4 UID FETCH 900 (UID FLAGS BODY.PEEK[])\r\n" "T5 UID FETCH 901 (UID FLAGS BODY.PEEK[])\r\n") ((cadr f))))

;; Cancellation closes an in-flight protocol transaction; a second command
;; must never consume the first command's tagged reply.
(let ((first #t) (closed #f))
  (let* ((s (make-imap-session
             (lambda () (if first (begin (set! first #f) (string->utf8 "* OK ready\r\n"))
                            (begin (sleep 100000) (eof-object))))
             (lambda (text) #t) (lambda () (set! closed #t))))
         (task (spawn (lambda () (imap-mailboxes s)))))
    (sleep 10)
    (check "concurrent command rejected" #t (rejected? (lambda () (imap-mailboxes s))))
    (check "concurrent refusal does not close active command" #f closed)
    (task-cancel task)
    (guard (e (#t #f)) (task-join task))
    (check "cancelled command closes transport" #t closed)))

;; Maildir fixture is created only by tests, never by maildir-read.
(let ((root (string-append "/tmp/techne-mail-backend-" (uuid-v4))))
  (dynamic-wind
    (lambda () (make-directory (string-append root "/cur/nested"))
               (make-directory (string-append root "/new"))
               (make-directory (string-append root "/tmp")))
    (lambda ()
      (replace-file (string-append root "/cur/b:2,DFPRSTz") "Subject: old\r\n\r\nold")
      (replace-file (string-append root "/new/a") "Subject: new\r\n\r\nnew")
      (replace-file (string-append root "/cur/.hidden") "ignore")
      (replace-file (string-append root "/tmp/not-delivered") "ignore")
      (let* ((rows (maildir-read root)) (old (car rows)) (new (cadr rows)))
        (check "Maildir cur/new deterministic ordering" '("b" "a") (map (lambda (r) (field r 'key)) rows))
        (check "Maildir all flags" '("\\Draft" "\\Flagged" "$Forwarded" "\\Answered" "\\Seen" "\\Deleted") (field old 'flags))
        (check "Maildir new flags" '() (field new 'flags))
        (check "Maildir raw" "Subject: new\r\n\r\nnew" (field new 'raw))
        (check "Maildir absolute path" (string-append root "/cur/b:2,DFPRSTz") (field old 'path))
        (check "Maildir no writes" rows (maildir-read root)))
      (delete-file (string-append root "/cur/b:2,DFPRSTz"))
      (delete-file (string-append root "/new/a"))
      (for-each (lambda (path) (replace-file (string-append root path) "Subject: fixture\n\nbody"))
                '("/cur/100:2,S" "/cur/300:2," "/new/200"))
      (let ((keys '()))
        (maildir-for-each root (lambda (m) (set! keys (cons (field m 'key) keys))) #:order 'unread-newest)
        (check "Maildir unread first, timestamp newest first" '("300" "200" "100") (reverse keys)))
      ;; Skipping happens before opening: this body would exceed the read limit.
      (replace-file (string-append root "/cur/400:2,") (make-string 16777217 #\x))
      (let ((keys '()) (checked '()))
        (maildir-for-each root (lambda (m) (set! keys (cons (field m 'key) keys)))
          #:skip? (lambda (key) (set! checked (cons key checked)) (string=? key "400")) #:order 'unread-newest)
        (check "Maildir skipped body is never read" '("300" "200" "100") (reverse keys))
        (check "Maildir skip sees source key before body read" #t (and (member "400" checked) #t)))
      (check "Maildir unknown order rejected" #t (rejected? (lambda () (maildir-for-each root (lambda (m) #f) #:order 'unknown))))
      (check "Maildir missing directories empty" '() (maildir-read (string-append root "/empty")))
      ;; A tree of folders, as mbsync keeps a Gmail account.
      (make-directory (string-append root "/Inbox/cur"))
      (make-directory (string-append root "/[Gmail]/All Mail/new"))
      (make-directory (string-append root "/[Gmail]/Sent Mail/cur"))
      (make-directory (string-append root "/lists/coreboot/cur"))
      (make-directory (string-append root "/not-mail/attachments"))
      (check "Maildir folders at and under a root, by their place in it"
             '("" "Inbox" "[Gmail]/All Mail" "[Gmail]/Sent Mail" "lists/coreboot")
             (map car (maildir-folders root)))
      (check "Maildir folder paths" (string-append root "/[Gmail]/All Mail")
             (cdr (assoc "[Gmail]/All Mail" (maildir-folders root))))
      (check "Maildir folders stop at a depth" '("" "Inbox") (map car (maildir-folders root #:depth 1)))
      (check "Maildir folders of a missing root" '() (maildir-folders (string-append root "/absent")))
      (check "Maildir keys of an empty folder" '() (hash-table-keys (maildir-keys (string-append root "/Inbox"))))
      (check "A directory without cur or new is not an empty Maildir" #t
             (rejected? (lambda () (maildir-keys (string-append root "/not-mail")))))
      (check "Maildir keys of a missing folder are an error, not none" #t
             (rejected? (lambda () (maildir-keys (string-append root "/absent"))))))
    (lambda () (delete-path root #t))))

(let* ((f (fake (string-append "* OK ready\r\n* OK [UIDNEXT 13] next\r\n" selected
                               "* SEARCH 10 11 12\r\nT2 OK search\r\n"
                               "* 1 FETCH (RFC822.SIZE 1 UID 10)\r\nT3 OK size\r\n"
                               "* 1 FETCH (UID 10 FLAGS () BODY[] {1}\r\na)\r\nT4 OK body\r\n"
                               "* 2 FETCH (UID 11 RFC822.SIZE 16777217)\r\nT5 OK size\r\n"
                               "* 3 FETCH (UID 12 RFC822.SIZE 1)\r\nT6 OK size\r\n"
                               "* 3 FETCH (UID 12 FLAGS () BODY[] {1}\r\nb)\r\nT7 OK body\r\n") 11))
       (s (car f)) (events '()))
  (imap-select! s "INBOX")
  (imap-for-each-message s (lambda (m) (set! events (cons (field m 'uid) events))) 0
    #:max-bytes 16777216 #:on-oversized (lambda (uid) (set! events (cons (list 'oversized uid) events))))
  (check "IMAP continues past an oversized message" '(10 (oversized 11) 12) (reverse events))
  (check "IMAP never requests the oversized body" #f (member "T6 UID FETCH 11 (UID FLAGS BODY.PEEK[])\r\n" ((cadr f))))
  (check "IMAP only issues size and body requests for the two small messages and a size request for the large one"
         7 (length ((cadr f)))))
;; A backlog spanning 10,000 UIDs must not be requested in one SEARCH.
;; Include empty space and messages at both sides of a page boundary.
(let* ((f (fake (string-append "* OK ready\r\n* OK [UIDNEXT 110000] next\r\n" selected
                               "* SEARCH 104095\r\nT2 OK first page\r\n"
                               "* 1 FETCH (UID 104095 FLAGS () BODY[] {0}\r\n)\r\nT3 OK body\r\n"
                               "* SEARCH 104096\r\nT4 OK second page\r\n"
                               "* 2 FETCH (UID 104096 FLAGS () BODY[] {0}\r\n)\r\nT5 OK body\r\n"
                               "* SEARCH\r\nT6 OK last page\r\n")))
       (s (car f)) (uids '()))
  (imap-select! s "INBOX")
  (imap-for-each-message s (lambda (m) (set! uids (cons (field m 'uid) uids))) 99999)
  (check "callbacks finish in UID order across pages" '(104095 104096) (reverse uids))
  (check "searches bound the UID count per response"
         '("T2 UID SEARCH UID 100000:104095\r\n" "T4 UID SEARCH UID 104096:108191\r\n" "T6 UID SEARCH UID 108192:109999\r\n")
         (filter (lambda (cmd) (string-contains cmd "UID SEARCH")) ((cadr f)))))
(test-summary)
