;;; techne web: a browser attached to the editor running on this machine.
;;;
;;; The gateway serves the browser frontend (index.html, app.js and
;;; style.css, beside this file) on IPv4 loopback, and attaches each
;;; browser that proves it holds the URL's secret to the editor, as
;;; another frontend: it is shown the editor's presentations and sends
;;; its input, nothing else. The editor's server requests (`techne eval`)
;;; are not reachable through it.
;;;
;;; Who may attach: a WebSocket upgrade naming this gateway as its host and
;;; its origin, with the subprotocols `techne-v3` and the secret. The
;;; secret is in the URL's fragment, which browsers keep out of requests
;;; and referrers; the page moves it to session storage. Anyone with the
;;; URL operates the editor: it is printed once, to whoever started the
;;; gateway.
;;;
;;; What a browser is shown is bounded (`presentation`): a buffer too
;;; large to draw is refused, with a message, rather than drawn slowly or
;;; not at all. What it sends is checked against the inputs a browser may
;;; make (`browser-input`), and an input meant for a runtime that has
;;; since restarted is dropped.
;;;
;;; The natives (`%editor-attach` and the others) are `techne web`'s:
;;; crates/techne-cli/src/web.rs.

(require "../http/http.scm")
(import (srfi 151) (techne crypto))

(provide web-serve sha-1 websocket-accept frame->bytes read-message make-frame-reader
         browser-input presentation authorized?)

(define protocol "techne-v3")
(define max-input (* 256 1024))
(define max-text (* 2 1024 1024))
(define max-output (* 4 1024 1024))
(define max-rows 4000)
(define max-parts 16000)
(define max-panes 32)
(define max-clients 8)
(define max-connections 64)
;; How long, in milliseconds, a request may take to arrive, and a frame to
;; be sent: a browser that stops reading is let go.
(define request-timeout 10000)
(define send-timeout 3000)
(define too-large
  "This presentation exceeds the browser's size limit. Open a smaller buffer or use a native frontend.")

;;; SHA-1 (FIPS 180-4), for the WebSocket handshake only: the protocol
;;; names it. It is no secret's protection here.

(define (u32 x) (bitwise-and x #xffffffff))
(define (rotl x k) (u32 (bitwise-ior (arithmetic-shift x k) (arithmetic-shift x (- k 32)))))

(define (sha-1 data)
  "Return the SHA-1 digest of DATA, a bytevector, as 20 bytes."
  (let* ((n (bytevector-length data))
         (total (* 64 (+ 1 (quotient (+ n 8) 64))))
         (m (make-bytevector total 0))
         (w (make-vector 80 0))
         (h (vector #x67452301 #xEFCDAB89 #x98BADCFE #x10325476 #xC3D2E1F0)))
    (bytevector-copy! m 0 data)
    (bytevector-u8-set! m n #x80)
    (do ((i 0 (+ i 1))) ((= i 8))
      (bytevector-u8-set! m (- total 1 i) (bitwise-and (arithmetic-shift (* 8 n) (* -8 i)) 255)))
    (do ((block 0 (+ block 64))) ((= block total))
      (do ((t 0 (+ t 1))) ((= t 16))
        (let ((at (+ block (* 4 t))))
          (vector-set! w t (bitwise-ior (arithmetic-shift (bytevector-u8-ref m at) 24)
                                        (arithmetic-shift (bytevector-u8-ref m (+ at 1)) 16)
                                        (arithmetic-shift (bytevector-u8-ref m (+ at 2)) 8)
                                        (bytevector-u8-ref m (+ at 3))))))
      (do ((t 16 (+ t 1))) ((= t 80))
        (vector-set! w t (rotl (bitwise-xor (vector-ref w (- t 3)) (vector-ref w (- t 8))
                                            (vector-ref w (- t 14)) (vector-ref w (- t 16)))
                               1)))
      (let loop ((t 0) (a (vector-ref h 0)) (b (vector-ref h 1)) (c (vector-ref h 2)) (d (vector-ref h 3)) (e (vector-ref h 4)))
        (if (= t 80)
            (for-each (lambda (i v) (vector-set! h i (u32 (+ (vector-ref h i) v)))) '(0 1 2 3 4) (list a b c d e))
            (let* ((f (cond ((< t 20) (bitwise-ior (bitwise-and b c) (bitwise-and (u32 (bitwise-not b)) d)))
                            ((< t 40) (bitwise-xor b c d))
                            ((< t 60) (bitwise-ior (bitwise-and b c) (bitwise-and b d) (bitwise-and c d)))
                            (else (bitwise-xor b c d))))
                   (k (cond ((< t 20) #x5A827999) ((< t 40) #x6ED9EBA1) ((< t 60) #x8F1BBCDC) (else #xCA62C1D6)))
                   (next (u32 (+ (rotl a 5) f e k (vector-ref w t)))))
              (loop (+ t 1) next a (rotl b 30) c d)))))
    (let ((out (make-bytevector 20)))
      (do ((i 0 (+ i 1))) ((= i 20) out)
        (bytevector-u8-set! out i (bitwise-and (arithmetic-shift (vector-ref h (quotient i 4)) (* -8 (- 3 (remainder i 4)))) 255))))))

(define (websocket-accept key)
  "Return the Sec-WebSocket-Accept answering the handshake KEY (RFC 6455)."
  (base64-encode (sha-1 (string->utf8 (string-append key "258EAFA5-E914-47DA-95CA-C5AB0DC85B11")))))

;;; WebSocket frames (RFC 6455): the browser's masked, the gateway's not.

(define (frame->bytes opcode payload)
  "Return a final, unmasked frame of OPCODE carrying PAYLOAD, a bytevector."
  (let* ((n (bytevector-length payload))
         (head (cond ((< n 126) (bytevector (bitwise-ior #x80 opcode) n))
                     ((< n 65536) (bytevector (bitwise-ior #x80 opcode) 126 (arithmetic-shift n -8) (bitwise-and n 255)))
                     (else (let ((h (make-bytevector 10 0)))
                             (bytevector-u8-set! h 0 (bitwise-ior #x80 opcode))
                             (bytevector-u8-set! h 1 127)
                             (do ((i 0 (+ i 1))) ((= i 8) h)
                               (bytevector-u8-set! h (- 9 i) (bitwise-and (arithmetic-shift n (* -8 i)) 255))))))))
    (bytevector-append head payload)))

;; Bytes as READ gives them (a bytevector, or the eof object), taken N at
;; a time.
(define-record-type frame-reader
  (%frame-reader read buffer at)
  frame-reader?
  (read reader-read)
  (buffer reader-buffer reader-buffer-set!)
  (at reader-at reader-at-set!))

(define (make-frame-reader read)
  "Return a reader of WebSocket messages from READ, which returns bytes."
  (%frame-reader read #u8() 0))

(define (take! r n)
  (let loop ()
    (let ((b (reader-buffer r)) (at (reader-at r)))
      (if (>= (- (bytevector-length b) at) n)
          (begin (reader-at-set! r (+ at n)) (bytevector-copy b at (+ at n)))
          (let ((more ((reader-read r))))
            (if (eof-object? more)
                more
                (begin (reader-buffer-set! r (bytevector-append (bytevector-copy b at) more))
                       (reader-at-set! r 0)
                       (loop))))))))

(define (number-of bytes)
  (let loop ((i 0) (n 0))
    (if (= i (bytevector-length bytes)) n (loop (+ i 1) (+ (* n 256) (bytevector-u8-ref bytes i))))))

;; The next frame: (fin opcode payload), the eof object at the end, or #f
;; for a frame no browser sends (unmasked, reserved bits, past MAX).
(define (read-frame r max)
  (let ((head (take! r 2)))
    (if (eof-object? head)
        head
        (let* ((b0 (bytevector-u8-ref head 0)) (b1 (bytevector-u8-ref head 1)) (short (bitwise-and b1 127))
               (extended (case short ((126) (take! r 2)) ((127) (take! r 8)) (else #f)))
               (n (if extended (if (eof-object? extended) -1 (number-of extended)) short)))
          (cond ((< n 0) (eof-object))
                ((or (not (zero? (bitwise-and b0 #x70))) (zero? (bitwise-and b1 #x80)) (> n max)) #f)
                (else
                 (let ((mask (take! r 4)) (payload (take! r n)))
                   (if (or (eof-object? mask) (eof-object? payload))
                       (eof-object)
                       (begin
                         (do ((i 0 (+ i 1))) ((= i n))
                           (bytevector-u8-set! payload i (bitwise-xor (bytevector-u8-ref payload i)
                                                                      (bytevector-u8-ref mask (remainder i 4)))))
                         (list (not (zero? (bitwise-and b0 #x80))) (bitwise-and b0 15) payload))))))))))

(define (read-message r control #:max [max max-input])
  "Return the next text message from R, a string, or the eof object.
A control frame on the way is given to CONTROL, (opcode payload), which
answers pings. A close, a binary message, a message past MAX bytes or a
malformed frame ends the stream: the eof object."
  (let loop ((pieces '()) (size 0))
    (let ((frame (read-frame r (- max size))))
      (if (or (not frame) (eof-object? frame))
          (eof-object)
          (let ((fin (car frame)) (opcode (cadr frame)) (payload (caddr frame)))
            (cond ((= opcode 8) (control opcode payload) (eof-object))
                  ((>= opcode 8) (control opcode payload) (loop pieces size))
                  ;; A text message starts with opcode 1; continuations are 0.
                  ((not (= opcode (if (null? pieces) 1 0))) (eof-object))
                  (else
                   (let ((pieces (cons payload pieces)) (size (+ size (bytevector-length payload))))
                     (if fin
                         (utf8->string (apply bytevector-append (reverse pieces)))
                         (loop pieces size))))))))))

;;; What a browser may send: a dict {generation, input}, the input one of
;;; these, with exactly these fields. Anything else ends its session.

(define (natural? x) (and (exact-integer? x) (>= x 0)))
(define (boolean-field? x) (boolean? x))
(define (key? x) (and (string? x) (<= 1 (bytevector-length (string->utf8 x)) 64)))
(define (caret? x) (or (eq? x 'null) (natural? x)))

;; Each input type: its fields and what each must be.
(define inputs
  `(("key" ("key" ,key?))
    ("click" ("view" ,natural?) ("revision" ,natural?) ("pos" ,natural?) ("extend" ,boolean-field?))
    ("scroll" ("view" ,natural?) ("revision" ,natural?) ("anchor" ,natural?) ("caret" ,caret?))
    ("edge" ("view" ,natural?) ("end" ,boolean-field?))
    ("text" ("text" ,string?))
    ("viewport" ("columns" ,natural?) ("rows" ,natural?) ("split_gap" ,natural?))
    ("focus" ("focused" ,boolean-field?))))

(define (exactly? d names)
  (and (dict? d) (= (dict-size d) (length names)) (every (lambda (name) (dict-contains? d name)) names)))

(define (clamp x low high) (max low (min high x)))

(define (browser-input text)
  "Return the (generation . input) TEXT, a browser's message, asks for.
INPUT is the editor's JSON of it, or #f for one that changes nothing (an
empty text). Return #f for a message no browser sends."
  (let ((frame (guard (e (#t #f)) (string->json text))))
    (and (exactly? frame '("generation" "input"))
         (natural? (dict-ref frame "generation"))
         (let* ((input (dict-ref frame "input"))
                (type (and (dict? input) (dict-ref input "type" #f)))
                (fields (and (string? type) (assoc type inputs))))
           (and fields
                (exactly? input (cons "type" (map car (cdr fields))))
                (every (lambda (field) ((cadr field) (dict-ref input (car field)))) (cdr fields))
                (cons (dict-ref frame "generation") (editor-input type input)))))))

(define (editor-input type input)
  (let ((get (lambda (name) (dict-ref input name))))
    (cond ((string=? type "key") (dict "Key" (dict "key" (get "key"))))
          ((string=? type "click")
           (dict "Click" (dict "view" (get "view") "revision" (get "revision") "pos" (get "pos") "extend" (get "extend"))))
          ((string=? type "scroll")
           (dict "Scroll" (dict "view" (get "view") "revision" (get "revision") "anchor" (get "anchor") "caret" (get "caret"))))
          ((string=? type "edge") (dict "Edge" (dict "view" (get "view") "end" (get "end"))))
          ((string=? type "text") (and (not (string=? (get "text") "")) (dict "Text" (dict "text" (get "text")))))
          ((string=? type "viewport")
           (dict "Viewport" (dict "columns" (clamp (get "columns") 1 4096) "rows" (clamp (get "rows") 2 4096)
                                  "split_gap" (clamp (get "split_gap") 1 64))))
          ((string=? type "focus") (dict "Focus" (dict "focused" (get "focused")))))))

;;; What a browser is shown: the snapshot's JSON as the editor has it,
;;; bounded, and its panes' text.

(define (items x) (cond ((vector? x) (vector-length x)) ((list? x) (length x)) (else 0)))
(define (row-parts rows)
  (fold (lambda (row n) (let ((columns (dict-ref row "columns")))
                          (+ n 1 (items columns) (fold (lambda (c m) (+ m (items c))) 0 (vector->list columns)))))
        0 (vector->list rows)))
(define (rows-of x) (if (dict? x) (row-parts (dict-ref x "rows")) 0))

(define (presentation snapshot pane-size pane-text generation)
  "Return the browser's message for SNAPSHOT, the editor's JSON of it
without text, or #f if it exceeds the browser's budgets. (PANE-SIZE I)
is pane I's text's (bytes lines), (PANE-TEXT I) its text, asked for only
once the budgets allow it. GENERATION is the runtime's, as the browser
must send it back."
  (let* ((panes (vector->list (dict-ref snapshot "panes")))
         (parts (+ (items (dict-ref snapshot "styles")) (* 4 (items (dict-ref snapshot "key_hints")))
                   (rows-of (dict-ref snapshot "minibuffer")) (rows-of (dict-ref snapshot "completion")))))
    (and (<= (length panes) max-panes) (<= parts max-parts)
         (let loop ((i 0) (panes panes) (bytes 0) (rows 0) (parts parts) (out '()))
           (if (null? panes)
               (dict "type" "snapshot" "generation" generation "id" (dict-ref snapshot "id") "panes" (reverse out)
                     "focus" (dict-ref snapshot "focus") "echo" (dict-ref snapshot "echo")
                     "minibuffer" (dict-ref snapshot "minibuffer") "completion" (dict-ref snapshot "completion")
                     "hints" (dict-ref snapshot "key_hints") "styles" (dict-ref snapshot "styles")
                     "chrome" (dict-ref snapshot "chrome"))
               (let* ((p (car panes)) (size (pane-size i)) (lines (cadr size))
                      (bytes (+ bytes (car size))) (rows (+ rows lines))
                      ;; Bound the nodes drawn and the selection checks per row.
                      (parts (+ parts (* 2 (items (dict-ref p "layers"))) (* 2 (items (dict-ref p "inlays")))
                                (items (dict-ref p "blocks")) (items (dict-ref p "hangs"))
                                (* lines (items (dict-ref p "selections"))))))
                 (and (<= bytes max-text) (<= rows max-rows) (<= parts max-parts)
                      (loop (+ i 1) (cdr panes) bytes rows parts (cons (dict-set p "text" (pane-text i)) out)))))))))

;;; HTTP: the page, its script and style, and the upgrade to a session.

(define (authorized? authority token request)
  "Return whether REQUEST, an upgrade, is the page's: its host and origin
AUTHORITY, the gateway's, its subprotocols the gateway's and TOKEN, the
secret, exactly."
  (let ((header (lambda (name) (request-header request name))))
    (and (equal? (header "host") authority)
         (equal? (header "origin") (string-append "http://" authority))
         (let* ((offered (header "sec-websocket-protocol"))
                (parts (if offered (map string-trim (string-split offered ",")) '())))
           (and (= (length parts) 2)
                (string=? (car parts) protocol)
                (digest=? (string->utf8 (cadr parts)) (string->utf8 token)))))))

(define-record-type gateway
  (make-gateway authority token socket file assets clients)
  gateway?
  (authority gateway-authority)
  (token gateway-token)
  (socket gateway-socket)
  (file gateway-file)
  (assets gateway-assets)
  (clients gateway-clients gateway-clients-set!))

(define security-headers
  (dict "cache-control" "no-store" "x-content-type-options" "nosniff" "referrer-policy" "no-referrer"
        "content-security-policy"
        "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"))

(define (answer c status #:body [body ""] #:type [type #f])
  (tcp-write c (response->bytes (response body #:status status #:headers security-headers #:type type)
                                #:close #t #:date (http-date (exact (floor (current-second)))))))

(define (within c ms thunk)
  "Call THUNK; close C if it has not returned within MS milliseconds."
  (let ((watchdog (spawn (lambda () (sleep ms) (tcp-close c)))))
    (dynamic-wind (lambda () #f) thunk (lambda () (task-cancel watchdog)))))

(define (serve-connection g c)
  (let* ((r (make-http-reader (lambda () (tcp-read-bytes c))))
         (request (guard (e (#t #f)) (within c request-timeout (lambda () (read-request r #:max-head 16384 #:max-body 1024)))))
         (header (lambda (name) (and request (request-header request name)))))
    (cond ((or (not request) (eof-object? request)) #f)
          ((not (equal? (header "host") (gateway-authority g))) (within c send-timeout (lambda () (answer c 403))))
          ((not (string=? (dict-ref request 'method) "GET")) (within c send-timeout (lambda () (answer c 405))))
          ((assoc (dict-ref request 'path) (gateway-assets g))
           => (lambda (asset) (within c send-timeout (lambda () (answer c 200 #:body (caddr asset) #:type (cadr asset))))))
          ((not (string=? (dict-ref request 'path) "/attach")) (within c send-timeout (lambda () (answer c 404))))
          ((not (and (authorized? (gateway-authority g) (gateway-token g) request)
                     (equal? (header "sec-websocket-version") "13")
                     (header "sec-websocket-key")
                     (member "websocket" (map (lambda (s) (string-downcase (string-trim s))) (string-split (or (header "upgrade") "") ",")))))
           (within c send-timeout (lambda () (answer c 403))))
          ((>= (gateway-clients g) max-clients) (within c send-timeout (lambda () (answer c 503))))
          (else
           (gateway-clients-set! g (+ (gateway-clients g) 1))
           (dynamic-wind
            (lambda () #f)
            (lambda ()
              (within c send-timeout
                      (lambda ()
                        (tcp-write c (string->utf8
                                      (string-append "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n"
                                                     "Connection: Upgrade\r\nSec-WebSocket-Accept: "
                                                     (websocket-accept (header "sec-websocket-key"))
                                                     "\r\nSec-WebSocket-Protocol: " protocol "\r\n\r\n")))))
              (session g c))
            (lambda () (gateway-clients-set! g (- (gateway-clients g) 1))))))))

;;; A session: one browser, attached to the editor as a frontend.

(define (session g c)
  ;; One writer, so frames never interleave; a frame not taken within
  ;; the send timeout ends the session (the browser stopped reading).
  (let* ((scope (make-scope 'web-session))
         (out (with-scope scope (make-channel 4)))
         (events (with-scope scope (make-channel 16))))
    (define (send! opcode payload)
      (guard (e (#t #f)) (channel-send out (frame->bytes opcode payload)) #t))
    (define (send-json! value)
      (let ((text (json->string value)))
        (if (> (string-length text) max-output)
            (let ((bytes (string->utf8 text)))
              (if (> (bytevector-length bytes) max-output)
                  (begin (send-json! (dict "type" "error" "message" too-large)) #f)
                  (send! 1 bytes)))
            (send! 1 (string->utf8 text)))))
    (define (error! message) (send-json! (dict "type" "error" "message" message)))
    (dynamic-wind
     (lambda () #f)
     (lambda ()
       (with-scope scope
         (let ((writer
                (spawn (lambda ()
                         ;; Past a failed write, frames are dropped: the
                         ;; connection is closed, and the reader ends.
                         (let loop ((open #t))
                           (let ((frame (channel-recv out)))
                             (unless (eof-object? frame)
                               (loop (and open (guard (e (#t (tcp-close c) #f))
                                                 (within c send-timeout (lambda () (tcp-write c frame)))
                                                 #t))))))))))
         (let ((attachment (guard (e ((error-object? e) (error! (condition/report-string e)) #f))
                             (or (%editor-attach (gateway-socket g) "emacs" (gateway-file g))
                                 (begin (error! "No editor is running. Start techne daemon, then reconnect.") #f)))))
           (when attachment
             (scope-own! attachment %editor-close!)
             (spawn (lambda ()
                      (let ((r (make-frame-reader (lambda () (tcp-read-bytes c)))))
                        (let loop ()
                          (let ((message (guard (e (#t (eof-object)))
                                           (read-message r (lambda (opcode payload) (when (= opcode 9) (send! 10 payload)))))))
                            (channel-send events (cons 'browser message))
                            (unless (eof-object? message) (loop)))))))
             (spawn (lambda ()
                      (let loop ()
                        (let ((event (%editor-event attachment)))
                          (channel-send events (cons 'editor event))
                          (unless (eof-object? event) (loop))))))
             (run-session attachment events send-json! error!)))
         (send! 8 (bytevector #x03 #xe8))
         ;; What is queued is written before the session's scope ends.
         (channel-close out)
         (task-join writer))))
     (lambda () (scope-shutdown! scope)))))

(define (run-session attachment events send-json! error!)
  (let loop ((generation 0) (ready #f))
    (let* ((item (channel-recv events)) (what (cdr item)))
      (if (eq? (car item) 'browser)
          (cond ((eof-object? what) #f)
                ((browser-input what)
                 => (lambda (asked)
                      (when (and ready (= (car asked) generation) (cdr asked))
                        (%editor-send! attachment (json->string (cdr asked)) generation))
                      (loop generation ready)))
                (else (error! "Invalid frontend message.")))
          (if (eof-object? what)
              #f
              (case (car what)
                ((snapshot)
                 (let* ((s (cadr what))
                        (message (presentation (string->json (%snapshot-json s))
                                               (lambda (i) (%snapshot-pane-size s i))
                                               (lambda (i) (%snapshot-pane-text s i))
                                               generation)))
                   (cond ((not message) (error! too-large))
                         ((send-json! message) (loop generation #t)))))
                ((capture)
                 (%editor-refuse-capture! attachment (cadr what) "the web frontend does not support editor captures")
                 (loop generation ready))
                ((clipboard) (when (send-json! (dict "type" "clipboard" "text" (cadr what))) (loop generation ready)))
                ((ended) (when (send-json! (dict "type" "restart")) (loop (+ generation 1) #f)))
                ((quit) (send-json! (dict "type" "quit")))
                ((failed refused) (error! (cadr what)))
                ((overflow) (error! "Browser fell behind. Reconnect to continue."))
                (else (loop generation ready))))))))

;;; The gateway.

(define (asset directory name type)
  (list (string-append "/" (if (string=? name "index.html") "" name)) type
        (file->string (string-append directory "/" name))))

(define (web-serve socket file directory)
  "Serve the browser frontend on IPv4 loopback, attaching browsers to the
editor on SOCKET, its private socket, each opening FILE unless it is #f.
DIRECTORY has the page, its script and its style. Print the private URL,
then serve until the process ends."
  (let* ((listener (tcp-listen 0 #:host "127.0.0.1"))
         (authority (string-append "127.0.0.1:" (number->string (cadr (tcp-local-address listener)))))
         (token (hex-encode (random-bytes 32)))
         (g (make-gateway authority token socket file
                          (list (asset directory "index.html" "text/html; charset=utf-8")
                                (asset directory "app.js" "text/javascript; charset=utf-8")
                                (asset directory "style.css" "text/css; charset=utf-8"))
                          0))
         (slots (make-channel max-connections)))
    ;; The fragment stays out of requests and referrers.
    (display (string-append "http://" authority "/#" token "\n"))
    (flush-output-port)
    (let loop ()
      (channel-send slots #t)
      (let ((c (tcp-accept listener)))
        (spawn (lambda ()
                 (dynamic-wind
                  (lambda () #f)
                  (lambda () (call-with-socket c (lambda (c) (guard (e (#t #f)) (serve-connection g c)))))
                  (lambda () (channel-recv slots)))))
        (loop)))))
