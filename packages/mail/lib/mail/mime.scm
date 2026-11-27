;;; MIME messages as proper alists with dotted entries, never editor state.
;;; Internally strings are octets (one character per byte). Only decoded
;;; text crosses into Unicode. Thus binary attachments survive CRLF intact.

(define (m-ref xs key fallback)
  (let ((p (assoc key xs))) (if p (cdr p) fallback)))
(define (m-white? c) (memv c '(#\space #\tab #\return #\newline)))
(define (m-trim s)
  (let ((n (string-length s)))
    (let start ((a 0))
      (if (and (< a n) (m-white? (string-ref s a))) (start (+ a 1))
          (let end ((b n))
            (if (and (> b a) (m-white? (string-ref s (- b 1)))) (end (- b 1))
                (substring s a b)))))))
(define (m-find s needle start . limit)
  (let ((n (min (string-length s) (if (null? limit) (string-length s) (car limit)))))
    (and (<= start n) (string-contains s needle start n))))
(define (m-trim-right-boundary s)
  (let loop ((end (string-length s)))
    (if (and (> end 0) (memv (string-ref s (- end 1)) '(#\space #\tab)))
        (loop (- end 1)) (substring s 0 end))))
(define (m-prefix? prefix s)
  (and (<= (string-length prefix) (string-length s))
       (string=? prefix (substring s 0 (string-length prefix)))))
(define (m-bytes->octets b) (latin1->string b))
(define (m-octets->bytes s) (string->latin1 s))

;; Text of a charset. Deterministic: what cannot be decoded is replaced.
(define (m-decode b charset)
  ;; UTF-8 unless CHARSET is a single-byte one (charset.sld): a label not
  ;; known, a multi-byte charset included, is tried as UTF-8.
  (cond ((or (string-ci=? charset "utf-8") (string-ci=? charset "us-ascii")) (m-decode-utf8 b))
        ((string=? (mail-charset-name charset) "iso-8859-1") (m-bytes->octets b))
        ((mail-charset-table charset)
         => (lambda (table)
              (let ((s (m-bytes->octets b)))
                (do ((i 0 (+ i 1))) ((= i (string-length s)) s)
                  (let ((v (char->integer (string-ref s i))))
                    (when (>= v 128) (string-set! s i (integer->char (vector-ref table (- v 128))))))))))
        (else (m-decode-utf8 b))))
(define (m-decode-utf8 b)
  (guard (e ((not (task-cancelled-condition? e)) (m-decode-replacing b))) (utf8->string b)))
(define (m-decode-replacing b)
  ;; UTF-8 with each malformed octet replaced, the same way every time.
  (let ((n (bytevector-length b)) (out (open-output-string)))
    (let loop ((i 0))
      (when (< i n)
        (let* ((v (bytevector-u8-ref b i))
               (k (cond ((< v 128) 1) ((<= 194 v 223) 2) ((<= 224 v 239) 3)
                        ((<= 240 v 244) 4) (else 0)))
               (code (and (> k 0) (<= (+ i k) n)
                          (let more ((j 1) (v (modulo v (expt 2 (- 7 k)))))
                            (if (= j k) v
                                (let ((x (bytevector-u8-ref b (+ i j))))
                                  (and (<= 128 x 191) (more (+ j 1) (+ (* v 64) (- x 128))))))))))
          (if (and code (or (= k 1) (>= code (case k ((2) 128) ((3) 2048) (else 65536))))
                   (<= code 1114111) (not (<= 55296 code 57343)))
              (begin (write-char (integer->char (if (= k 1) v code)) out) (loop (+ i k)))
              (begin (write-char #\xFFFD out) (loop (+ i 1)))))))
    (get-output-string out)))

(define m-hex-values
  (let ((v (make-vector 128 #f)))
    (do ((n 0 (+ n 1))) ((= n 16) v)
      (vector-set! v (+ (if (< n 10) 48 87) n) n)
      (when (>= n 10) (vector-set! v (+ 55 n) n)))))
(define (m-hex c)
  (let ((n (char->integer c))) (and (< n 128) (vector-ref m-hex-values n))))
(define (m-qp s word?)
  ;; Bulk-copy normal runs. Look up each escape's digits once, not twice.
  ;; Keep soft-break boundaries until decoding; joining first can create
  ;; a new escape from malformed =A=LF B or ==LF41 input.
  (let* ((parts (string-split (if word? (string-replace s "_" " ") s) "="))
         (out (open-output-string)))
    (display (car parts) out)
    (for-each
     (lambda (part)
       (let* ((n (string-length part))
              (a (and (>= n 2) (m-hex (string-ref part 0))))
              (b (and a (m-hex (string-ref part 1)))))
         (cond ((m-prefix? "\r\n" part) (display (substring part 2 n) out))
               ((m-prefix? "\n" part) (display (substring part 1 n) out))
               (b (write-char (integer->char (+ (* 16 a) b)) out) (display (substring part 2 n) out))
               (else (display "=" out) (display part out)))))
     (cdr parts))
    (m-octets->bytes (get-output-string out))))
(define (m-transfer body encoding)
  (cond ((string-ci=? encoding "base64")
         (base64-decode (string-replace (string-replace (string-replace (string-replace body " " "") "\t" "") "\r" "") "\n" "")))
        ((string-ci=? encoding "quoted-printable") (m-qp body #f))
        (else (m-octets->bytes body))))

;; Decode encoded words only when the complete word is well formed. Adjacent
;; encoded words suppress the folding whitespace between them (RFC 2047).
(define (m-word s at)
  (and (<= (+ at 2) (string-length s))
       (char=? (string-ref s at) #\=)
       (char=? (string-ref s (+ at 1)) #\?)
       ;; RFC 2047 allows a word 75 characters, and senders write longer
       ;; ones, which other readers decode: a header line's 998 is the
       ;; bound here. Each search is bounded, including on malformed
       ;; input with many incomplete word prefixes.
       (let* ((limit (+ at 998))
              (a (m-find s "?" (+ at 2) limit)) (b (and a (m-find s "?" (+ a 1) limit)))
              (end (and b (m-find s "?=" (+ b 1) limit))))
         (and a b end (= b (+ a 2))
              (guard (e ((not (task-cancelled-condition? e)) #f))
                (let* ((encoding (string-downcase (substring s (+ a 1) b)))
                       (payload (substring s (+ b 1) end))
                       (bytes (cond ((string=? encoding "b") (base64-decode payload))
                                    ((string=? encoding "q") (m-qp payload #t)) (else #f))))
                  (and bytes (cons (+ end 2) (m-decode bytes (substring s (+ at 2) a))))))))))
(define (m-decode-words s)
  (let ((n (string-length s)))
    (if (not (m-find s "=?" 0))
        (m-decode (m-octets->bytes s) "utf-8")
        (let ((out (open-output-string)))
          (let loop ((i 0) (plain 0) (previous? #f))
            (let ((next (m-find s "=?" i)))
              (if (not next)
                  (begin (display (m-decode (m-octets->bytes (substring s plain n)) "utf-8") out)
                         (get-output-string out))
                  (let ((word (m-word s next)))
                    (if word
                        (let ((gap (substring s plain next)))
                          (unless (and previous? (string=? (m-trim gap) ""))
                            (display (m-decode (m-octets->bytes gap) "utf-8") out))
                          (display (cdr word) out) (loop (car word) (car word) #t))
                        (loop (+ next 2) plain previous?))))))))))

;; Line callbacks use the original offsets: binary bodies are never normalized.
(define (m-line s at)
  (let* ((end (or (m-find s "\n" at) (string-length s)))
         (stop (if (and (> end at) (char=? (string-ref s (- end 1)) #\return)) (- end 1) end)))
    (list (substring s at stop) (if (< end (string-length s)) (+ end 1) end))))
(define (m-field-name? s)
  (and (> (string-length s) 0)
       (every (lambda (c) (let ((v (char->integer c))) (and (<= 33 v 126) (not (char=? c #\:)))))
              (string->list s))))
(define (m-index-header? name)
  (member name '("subject" "from" "to" "cc" "reply-to" "date" "message-id" "in-reply-to" "references"
                 "content-type" "content-disposition" "content-transfer-encoding")))
(define (m-head s [index? #f])
  (let ((n (string-length s)))
    ;; SEEN? tracks syntax, even for discarded headers. KEEP? tracks the
    ;; current field, so its continuation cannot leak into a retained one.
    (let loop ((at 0) (headers '()) (seen? #f) (keep? #f))
      (when (> at 262144) (error "mail: header exceeds 256 KiB"))
      (if (= at n) (cons (reverse headers) "")
          (let* ((line+ (m-line s at)) (line (car line+)) (next (cadr line+))
                 (colon (m-find line ":" 0)))
            (cond ((string=? line "") (cons (reverse headers) (substring s next n)))
                  ((and seen? (> (string-length line) 0)
                        (memv (string-ref line 0) '(#\space #\tab)))
                   (if keep?
                       (loop next (cons (cons (caar headers) (string-append (cdar headers) " " (m-trim line))) (cdr headers)) #t #t)
                       (loop next headers #t #f)))
                  ((and colon (m-field-name? (substring line 0 colon)))
                   (let* ((name (string-downcase (substring line 0 colon)))
                          (keep? (or (not index?) (m-index-header? name))))
                     (loop next (if keep? (cons (cons name (m-trim (substring line (+ colon 1) (string-length line)))) headers) headers) #t keep?)))
                  ((not seen?) (cons '() s)) ; headerless text
                  (else (loop next headers seen? keep?))))))))

;; MIME parameters, with quoted semicolons and escaped quotes preserved.
(define (m-parameters s)
  (let ((n (string-length s)))
    (let loop ((i 0) (from 0) (quoted? #f) (escaped? #f) (parts '()))
      (cond ((= i n)
             (let* ((parts (reverse (cons (m-trim (substring s from n)) parts)))
                    (params
                     (filter-map
                      (lambda (part)
                        (let ((eq (m-find part "=" 0)))
                          (and eq
                               (let* ((key (string-downcase (m-trim (substring part 0 eq))))
                                      (value (m-trim (substring part (+ eq 1) (string-length part)))))
                                 (cons key
                                       (if (and (>= (string-length value) 2) (char=? (string-ref value 0) #\"))
                                           (let ((out (open-output-string)))
                                             (let unquote ((j 1))
                                               (when (< j (string-length value))
                                                 (let ((c (string-ref value j)))
                                                   (cond ((char=? c #\") #f)
                                                         ((and (char=? c #\\) (< (+ j 1) (string-length value)))
                                                          (write-char (string-ref value (+ j 1)) out) (unquote (+ j 2)))
                                                         (else (write-char c out) (unquote (+ j 1)))))))
                                             (get-output-string out)) value))))))
                      (cdr parts))))
               (cons (string-downcase (car parts)) params)))
            ((and (not quoted?) (char=? (string-ref s i) #\;))
             (loop (+ i 1) (+ i 1) #f #f (cons (m-trim (substring s from i)) parts)))
            (else
             (let ((c (string-ref s i)))
               (loop (+ i 1) from
                     (if (and (not escaped?) (char=? c #\")) (not quoted?) quoted?)
                     (and quoted? (not escaped?) (char=? c #\\)) parts)))))))
(define (m-filename params)
  (let ((extended (m-ref params "filename*" #f)))
    (or (and extended
             (let* ((a (m-find extended "'" 0)) (b (and a (m-find extended "'" (+ a 1)))))
               (and b
                    (let ((out (open-output-string)) (n (string-length extended)))
                      (let loop ((i (+ b 1)))
                        (when (< i n)
                          (if (and (char=? (string-ref extended i) #\%) (< (+ i 2) n)
                                   (m-hex (string-ref extended (+ i 1))) (m-hex (string-ref extended (+ i 2))))
                              (begin (write-char (integer->char (+ (* 16 (m-hex (string-ref extended (+ i 1))))
                                                                 (m-hex (string-ref extended (+ i 2))))) out)
                                     (loop (+ i 3)))
                              (begin (write-char (string-ref extended i) out) (loop (+ i 1))))))
                      (m-decode (m-octets->bytes (get-output-string out)) (substring extended 0 a))))))
        (m-ref params "filename" #f) (m-ref params "name" #f))))

(define (mail-header msg name . default)
  "Return MSG's first case-insensitive header NAME.
DEFAULT is empty text when omitted."
  (m-ref (m-ref msg 'headers '()) (string-downcase name) (if (null? default) "" (car default))))
(define (mail-header-raw msg name . default)
  "Return unfolded header NAME from MSG without decoding encoded words.
DEFAULT is empty text when omitted. Use this for address syntax: decoding a
comma in a display word before tokenization would change mailbox boundaries."
  (m-ref (m-ref msg 'headers-raw '()) (string-downcase name) (if (null? default) "" (car default))))
(define (mail-decode-header text)
  "Decode RFC 2047 words in UTF-8 header TEXT, preserving ordinary text."
  (m-decode-words (m-bytes->octets (string->utf8 text))))
(define (mail-decode-utf8 bytes)
  "Decode BYTES as UTF-8, replacing malformed octets; BYTES are unchanged."
  (unless (bytevector? bytes) (error "mail: expected wire bytes"))
  (m-decode bytes "utf-8"))
(define (mail-text msg)
  "Return MSG's decoded plain text, or #f; it is not Markdown."
  (m-ref msg 'text #f))
(define (mail-html msg)
  "Return MSG's decoded, still untrusted HTML string, or #f."
  (m-ref msg 'html #f))
(define (mail-attachments msg)
  "Return MSG's attachment alists, with name, type and bytevector bytes."
  (m-ref msg 'attachments '()))

(define (mail-parse raw #:attachments? [attachments? #t] #:index? [index? #f])
  "Parse RAW, a UTF-8 string or wire bytevector, to a message alist.
Headers are lowercase and unfolded, including RFC 2047 UTF-8/Latin-1 words.
The keys are headers, headers-raw, text, html, attachments, message-id,
in-reply-to, references and errors. Text/html are #f when absent.
Attachment names are metadata, never safe filesystem paths. Invalid parts
are skipped and noted in errors; intact siblings survive. Unknown charsets
use replacement UTF-8.
Reject more than 16 MiB up front; cap headers at 256 KiB, nesting at 24 and
parts at 1024. HTML is NOT sanitized here: pass it to mail-html->rich.
With ATTACHMENTS? false, omit decoded attachments. INDEX? additionally decodes
only searchable/envelope and MIME structure headers, ignoring transport/spam
header payloads; attachments are never decoded in that mode."
  (unless (or (string? raw) (bytevector? raw)) (error "mail: expected string or bytevector"))
  (when (> (if (string? raw) (string-length raw) (bytevector-length raw)) 16777216)
    (error "mail: message exceeds 16 MiB"))
  (let* ((bytes (if (string? raw) (string->utf8 raw) raw))
         (errors '()) (parts 0))
    (when (> (bytevector-length bytes) 16777216) (error "mail: message exceeds 16 MiB"))
    (letrec
        ((empty (lambda () (list (cons 'text #f) (cons 'html #f) (cons 'attachments '()))))
         (bad (lambda (reason) (set! errors (cons reason errors)) (empty)))
         (multipart
          (lambda (body boundary depth alternative?)
            (if (or (not boundary) (= (string-length boundary) 0) (> (string-length boundary) 200))
                (bad "invalid or missing multipart boundary")
                (let* ((n (string-length body)) (mark (string-append "--" boundary))
                       (line-mark (string-append "\n" mark)))
                  ;; Search delimiter candidates directly, not every line of a
                  ;; multi-megabyte attachment. Exact whole-line validation is
                  ;; still Scheme, including prefix collisions and CRLF edges.
                  (let loop ((at 0) (start #f) (result (empty)))
                    (if (or (= at n) (>= parts 1024))
                        (begin
                          (when (>= parts 1024) (set! errors (cons "part count exceeds 1024" errors)))
                          (if (and start (< start n) (< parts 1024))
                              (merge result (part (substring body start n) (+ depth 1)) alternative?) result))
                        (let ((found (or (and (= at 0) (m-prefix? mark body) 0)
                                         (let ((p (m-find body line-mark (max 0 (- at 1))))) (and p (+ p 1))))))
                          (if (not found) (loop n start result)
                              (let* ((line+ (m-line body found))
                                     (line (m-trim-right-boundary (car line+))) (next (cadr line+))
                                     (end? (string=? line (string-append mark "--")))
                                     (boundary? (or end? (string=? line mark))))
                                (if boundary?
                                    (let* ((stop (if (and (> found 0) (char=? (string-ref body (- found 1)) #\newline))
                                                     (if (and (> found 1) (char=? (string-ref body (- found 2)) #\return)) (- found 2) (- found 1)) found))
                                           (result (if start (merge result (part (substring body start (max start stop)) (+ depth 1)) alternative?) result)))
                                      (if end? result (loop next next result)))
                                    (loop next start result)))))))))))
         (merge
          (lambda (a b last?)
            (list (cons 'text (if last? (or (mail-text b) (mail-text a)) (or (mail-text a) (mail-text b))))
                  (cons 'html (if last? (or (mail-html b) (mail-html a)) (or (mail-html a) (mail-html b))))
                  (cons 'attachments (append (mail-attachments a) (mail-attachments b))))))
         (entity
          (lambda (headers body depth)
            (let* ((ct (m-parameters (m-ref headers "content-type" "text/plain")))
                   (cd (m-parameters (m-ref headers "content-disposition" "")))
                   (type (car ct)) (name (or (m-filename (cdr cd)) (m-filename (cdr ct))))
                   (attachment? (or name (string=? (car cd) "attachment"))))
              (cond ((and (m-prefix? "multipart/" type) (not attachment?))
                     (multipart body (m-ref (cdr ct) "boundary" #f) depth (string=? type "multipart/alternative")))
                    ((and (or index? (not attachments?)) (or attachment? (not (member type '("text/plain" "text/html"))))) (empty))
                    (else
                     (let ((data (m-transfer body (string-downcase (m-trim (m-ref headers "content-transfer-encoding" ""))))))
                       (cond ((or attachment? (not (member type '("text/plain" "text/html"))))
                              (list (cons 'text #f) (cons 'html #f)
                                    (cons 'attachments (list (list (cons 'name (or name "")) (cons 'type type) (cons 'bytes data))))))
                             (else
                              (let ((text (m-decode data (m-ref (cdr ct) "charset" "utf-8"))))
                                (list (cons 'text (and (string=? type "text/plain") text))
                                      (cons 'html (and (string=? type "text/html") text)) (cons 'attachments '())))))))))))
         (part
          (lambda (s depth)
            (set! parts (+ parts 1))
            (cond ((> depth 24) (bad "MIME nesting exceeds 24"))
                  ((> parts 1024) (bad "part count exceeds 1024"))
                  (else (guard (e ((not (task-cancelled-condition? e)) (bad "malformed MIME part")))
                          (let* ((head (m-head s index?))
                                 (headers (map (lambda (h) (cons (car h) (m-decode-words (cdr h)))) (car head))))
                            (entity headers (cdr head) depth))))))))
      (let* ((head (guard (e ((not (task-cancelled-condition? e)) (set! errors (cons "malformed or oversized headers" errors)) (cons '() "")))
                     (m-head (m-bytes->octets bytes) index?)))
             (raw-headers (car head))
             (headers (map (lambda (h) (cons (car h) (m-decode-words (cdr h)))) raw-headers))
             (content (guard (e ((not (task-cancelled-condition? e)) (bad "malformed MIME body"))) (entity headers (cdr head) 0))))
        (append (list (cons 'headers headers)
                      (cons 'headers-raw (map (lambda (h) (cons (car h) (m-decode (m-octets->bytes (cdr h)) "utf-8"))) raw-headers))) content
                (list (cons 'message-id (m-ref headers "message-id" ""))
                      (cons 'in-reply-to (m-ref headers "in-reply-to" ""))
                      (cons 'references (m-ref headers "references" "")) (cons 'errors (reverse errors))))))))
