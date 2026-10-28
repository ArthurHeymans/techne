;;; Language servers: clients of them live here, next to the editor
;;; facilities they feed (EDITOR.md, section 3's layers, and the
;;; targets, completion and inspection the modes already have).
;;;
;;; A server is declared (`define-lsp-server') by the command that runs
;;; it and the modes it serves, in a scope-owned registry like modes:
;;;
;;;   (define-lsp-server rust-analyzer "Rust, by rust-analyzer."
;;;     #:command '("rust-analyzer") #:modes '(rust-mode))
;;;
;;; A buffer attaches to every declared server whose modes its major
;;; mode is or derives from — several at once, each with a session of
;;; its own per project root, started on first use. `M-x lsp' attaches
;;; the focused buffer. What a server pushes (diagnostics) is stored
;;; with the revision it was computed against and shown by the
;;; `lsp-mode' minor mode's layer, mapped through edits since; what is
;;; asked (definition, by `lsp-find-definition') goes to the first
;;; attached server that answers.
;;;
;;; Underneath is a small JSON-RPC over a process's pipes: frames are
;;; read in a task per connection, requests wait on cells the reader
;;; fills. Nothing in that section knows LSP: a debugger client (DAP
;;; is the same framing with other verbs) would stand on it as this
;;; does. Documents sync in full on a watcher task that polls
;;; revisions; incremental sync wants an after-change hook the editor
;;; does not have yet, and servers diff for themselves from full text.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "faces.scm")
(require "commands.scm")
(require "targets.scm")
(require "buffers.scm")
(require "requests.scm")
(require "files.scm")
(require "projects.scm")
(require "remote.scm")

(provide define-lsp-server register-lsp-server! lsp-server-names
         lsp lsp-restart lsp-find-definition
         ;; The JSON-RPC layer and the conversions under it, for the
         ;; clients that will stand on them (and the suites).
         rpc-connect rpc-notify! rpc-request!
         frame-length file-uri uri-file-path text-position-byte)

;;; The faces diagnostics are drawn with, by severity.
(define-face diagnostic "A language server's diagnostic." #:underline 'curl)
(define-face diagnostic.error "An error, from a language server." #:fg 'red #:underline '(curl red))
(define-face diagnostic.warning "A warning, from a language server." #:fg 'yellow #:underline '(curl yellow))
(define-face diagnostic.info "A note, from a language server." #:fg 'sky #:underline 'dotted)
(define-face diagnostic.hint "A hint, from a language server." #:fg 'overlay #:underline 'dotted)

;;; JSON-RPC over a process

(define-record-type rpc
  (%make-rpc process next pending inbox inbox-bytes head scanned parsed on-note on-ask)
  rpc?
  (process rpc-process)
  ;; The next request's id, and the requests waiting: id -> a one-cell
  ;; list, its car the answer ((result) or (error message)) when there.
  (next rpc-next set-rpc-next!)
  (pending rpc-pending)
  ;; Bytes read and not yet a whole frame, newest first, and how
  ;; many they are (counted as they arrive).
  (inbox rpc-inbox set-rpc-inbox!)
  (inbox-bytes rpc-inbox-bytes set-rpc-inbox-bytes!)
  ;; The inbox's first bytes, at most a header's worth, joined as
  ;; they arrive while no header is read yet; and how much of them
  ;; the delimiter search has covered.
  (head rpc-head set-rpc-head!)
  (scanned rpc-scanned set-rpc-scanned!)
  ;; The frame being assembled: (header-end . body-length) once its
  ;; header is read, else #f.
  (parsed rpc-parsed set-rpc-parsed!)
  ;; (method params) for a notification, (method params) -> result
  ;; for a request the peer makes of us.
  (on-note rpc-on-note)
  (on-ask rpc-on-ask))

(define (rpc-send! conn value)
  (let* ((body (string->utf8 (json->string value)))
         (head (string->utf8 (string-append "Content-Length: " (number->string (bytevector-length body)) "\r\n\r\n"))))
    (process-write (rpc-process conn) (bytevector-append head body))))

(define (rpc-notify! conn method params)
  "Send the notification METHOD with PARAMS over CONN."
  (rpc-send! conn (dict "jsonrpc" "2.0" "method" method "params" params)))

(define rpc-timeout 30000
  "How long a request waits for its answer, in milliseconds.
Starting a server and its first answer are the slow ones.")

(define (rpc-request! conn method params #:timeout [timeout rpc-timeout])
  "Ask METHOD of CONN's peer with PARAMS and wait for the answer.
The answer's result is returned; an error answer, a dead connection
and no answer in TIMEOUT milliseconds are errors. The waiting is
this task's: the editor goes on without it."
  (let* ((id (rpc-next conn)) (cell (list #f)))
    (set-rpc-next! conn (+ id 1))
    ;; However the waiting ends — an answer, no answer, a failed
    ;; send, this task cancelled (C-g, its scope shut) — the waiter
    ;; is not left in the table for an answer nobody wants. It is
    ;; registered inside the wind: outside, a cancellation between
    ;; registering and entering would strand it the same way.
    (dynamic-wind
      (lambda () #f)
      (lambda ()
        (hash-table-set! (rpc-pending conn) id cell)
        (rpc-send! conn (dict "jsonrpc" "2.0" "id" id "method" method "params" params))
        (let loop ((waited 0))
          (cond ((car cell) => (lambda (answer) (if (eq? (car answer) 'error) (error (cadr answer)) (cadr answer))))
                ((>= waited timeout) (error "no answer to" method))
                (else (sleep 5) (loop (+ waited 5))))))
      (lambda () (hash-table-delete! (rpc-pending conn) id)))))

;; The bytes "\r\n\r\n" in BV from AT: where a header ends, or #f.
(define (frame-end bv at)
  (let loop ((i at))
    (cond ((> (+ i 3) (- (bytevector-length bv) 1)) #f)
          ((and (= (bytevector-u8-ref bv i) 13) (= (bytevector-u8-ref bv (+ i 1)) 10)
                (= (bytevector-u8-ref bv (+ i 2)) 13) (= (bytevector-u8-ref bv (+ i 3)) 10)) i)
          (else (loop (+ i 1))))))

;; A frame's bounds: a peer past them is not speaking this protocol
;; (or not in good faith), and waiting for the rest of its frame
;; would hold the connection — and its waiters — forever.
(define rpc-max-header 8192)
(define rpc-max-body (* 32 1024 1024))

;; A Content-Length value, strictly: digits only, or #f. ("-5" is not
;; 5, and "5, please" is not a length either.)
(define (header-length-value value)
  (and (positive? (string-length value))
       (let loop ((cs (string->list value)))
         (cond ((null? cs) (string->number value))
               ((char-numeric? (car cs)) (loop (cdr cs)))
               (else #f)))))

(define (frame-length header)
  "Return the Content-Length of frame HEADER (bytes), in bytes.
Return 'bad where there is none, or it is not a plain number, or
two such headers differ. Header names are case-insensitive, as the
base protocol (MIME) has them."
  (guard (e (#t 'bad))
    (let loop ((lines (string-split (utf8->string header) "\n")) (found #f))
      (if (null? lines)
          (or found 'bad)
          (let* ((line (string-trim (car lines)))
                 (colon (string-index line #\:)))
            (cond ((string=? line "") (loop (cdr lines) found))
                  ((not colon) 'bad)
                  ((string=? (string-downcase (string-trim (substring line 0 colon))) "content-length")
                   (let ((n (header-length-value (string-trim (substring line (+ colon 1) (string-length line))))))
                     (cond ((not n) 'bad)
                           ((and found (not (= found n))) 'bad)
                           (else (loop (cdr lines) n)))))
                  (else (loop (cdr lines) found))))))))

;; Another CHUNK of the peer's bytes: kept, counted, and — while no
;; header is read yet — trailed into the header's own first bytes.
;; A fragment of a frame whose header is read is a cons and a sum:
;; assembling a frame from however many fragments it arrives in is
;; the frame's own size of work, once, not its square in fragments.
(define (rpc-feed! conn chunk)
  (set-rpc-inbox! conn (cons chunk (rpc-inbox conn)))
  (set-rpc-inbox-bytes! conn (+ (rpc-inbox-bytes conn) (bytevector-length chunk)))
  (unless (rpc-parsed conn)
    (let ((room (- (+ rpc-max-header 1) (bytevector-length (rpc-head conn)))))
      (when (positive? room)
        (set-rpc-head! conn (bytevector-append (rpc-head conn)
                                               (if (<= (bytevector-length chunk) room) chunk (bytevector-copy chunk 0 room))))))))

;; One whole frame out of CONN's inbox, or #f until one is whole. A
;; malformed or oversized frame is an error: the reader treats it as
;; the connection's end, failing its waiters, rather than waiting.
;; The header is read once and kept (`parsed'): an incomplete body
;; then costs a comparison per fragment, and a whole one is joined
;; once to be taken.
(define (rpc-frame! conn)
  (let ((parsed (rpc-parsed conn)))
    (if parsed
        (let ((end (car parsed)) (len (cdr parsed)))
          (and (>= (rpc-inbox-bytes conn) (+ end 4 len))
               (let* ((whole (apply bytevector-append (reverse (rpc-inbox conn))))
                      (body (bytevector-copy whole (+ end 4) (+ end 4 len)))
                      (rest (bytevector-copy whole (+ end 4 len) (bytevector-length whole))))
                 (set-rpc-inbox! conn (list rest))
                 (set-rpc-inbox-bytes! conn (bytevector-length rest))
                 (set-rpc-parsed! conn #f)
                 (set-rpc-head! conn (bytevector-copy rest 0 (min (bytevector-length rest) (+ rpc-max-header 1))))
                 (set-rpc-scanned! conn 0)
                 (string->json (utf8->string body)))))
        (let* ((head (rpc-head conn))
               ;; A delimiter spanning the last search is the only
               ;; one to look behind for.
               (from (max 0 (- (min (rpc-scanned conn) (bytevector-length head)) 3)))
               (end (frame-end head from)))
          (cond ((not end)
                 (set-rpc-scanned! conn (bytevector-length head))
                 (if (> (rpc-inbox-bytes conn) rpc-max-header) (error "rpc: a frame header runs past the limit") #f))
                (else (let ((len (frame-length (bytevector-copy head 0 end))))
                        (cond ((eq? len 'bad) (error "rpc: a malformed frame header"))
                              ((> len rpc-max-body) (error "rpc: a frame runs past the size limit"))
                              (else (set-rpc-parsed! conn (cons end len)) (rpc-frame! conn))))))))))

;; A message arrived: an answer wakes its waiter, a notification and
;; a request go by their handlers (a request's answer goes back).
(define (rpc-deliver! conn message)
  (let ((id (dict-ref message "id" #f)) (method (dict-ref message "method" #f)))
    (cond ((and id method)
           (let ((result (guard (e (#t 'null)) ((rpc-on-ask conn) method (dict-ref message "params" (dict))))))
             (rpc-send! conn (dict "jsonrpc" "2.0" "id" id "result" result))))
          (id (let ((cell (hash-table-ref/default (rpc-pending conn) id #f)))
                (when cell
                  (hash-table-delete! (rpc-pending conn) id)
                  (set-car! cell (if (dict-ref message "error" #f)
                                     (list 'error (dict-ref (dict-ref message "error") "message" "an error"))
                                     (list 'result (dict-ref message "result" 'null)))))))
          (method ((rpc-on-note conn) method (dict-ref message "params" (dict)))))))

(define (rpc-connect program args #:directory [directory #f] #:on-note [on-note (lambda (m p) #f)] #:on-ask [on-ask (lambda (m p) 'null)] #:on-dead [on-dead (lambda () #f)])
  "Start PROGRAM with ARGS in DIRECTORY, speaking JSON-RPC on its pipes.
Return the connection. A task reads it, in the scope calling: frames
go to ON-NOTE (method params) and ON-ASK (method params) -> result,
answers wake `rpc-request!'s. The process's stderr is read and
thrown away as it comes: nobody downstream wants it, but a peer
whose log nobody reads blocks on a full pipe and then on nothing
else, ever. When the process ends — or speaks out of frame, which
ends it from here — the waiters fail and ON-DEAD runs. The scope
owns the process, as every process is: shutting it kills the server."
  (let* ((process (process-spawn program args #:directory directory))
         (conn (%make-rpc process 1 (make-hash-table) '() 0 (bytevector) 0 #f on-note on-ask)))
    (spawn (lambda ()
             (guard (e (#t #f))
               (let loop ()
                 (let ((chunk (process-read-bytes process 'stderr)))
                   (unless (eof-object? chunk) (loop)))))))
    (spawn (lambda ()
             (guard (e (#t #f))
               (let loop ()
                 (let ((chunk (process-read-bytes process 'stdout)))
                   (unless (eof-object? chunk)
                     (rpc-feed! conn chunk)
                     (let frames () (let ((m (rpc-frame! conn))) (when m (rpc-deliver! conn m) (frames))))
                     (loop)))))
             ;; Ended, or ended by us for speaking out of frame: no
             ;; half a peer is left running against a dead reader.
             (guard (e (#t #f)) (process-kill process))
             (let ((waiters (hash-table->alist (rpc-pending conn))))
               (for-each (lambda (kv) (set-car! (cdr kv) '(error "the connection ended"))
                                       (hash-table-delete! (rpc-pending conn) (car kv)))
                         waiters))
             (on-dead)))
    conn))

;;; Positions, between the editor's bytes and LSP's lines and
;;; UTF-16 columns.

(define (utf16-length text)
  (let loop ((cs (string->list text)) (n 0))
    (if (null? cs) n (loop (cdr cs) (+ n (if (> (char->integer (car cs)) #xFFFF) 2 1))))))

(define (position-of d pos)
  (dict "line" (- (line-number d pos) 1)
        "character" (utf16-length (document-substring d (line-start d pos) pos))))

;; The byte of a position dict in D, its line counted by walks down.
(define (position-byte d at)
  (let* ((line (dict-ref at "line" 0)) (col (dict-ref at "character" 0))
         (start (if (= line 0) 0 (line-down d 0 line 0)))
         (end (line-end d start)) (text (document-substring d start end)))
    (let loop ((cs (string->list text)) (units col) (bytes start))
      (cond ((or (null? cs) (<= units 0)) bytes)
            (else (loop (cdr cs) (- units (if (> (char->integer (car cs)) #xFFFF) 2 1))
                        (+ bytes (bytevector-length (string->utf8 (string (car cs)))))))))))

(define (range-of at)
  (cons (position-byte (cdr at) (dict-ref (car at) "start")) (position-byte (cdr at) (dict-ref (car at) "end"))))

(define (char-utf8-length c)
  (let ((n (char->integer c)))
    (cond ((< n #x80) 1) ((< n #x800) 2) ((< n #x10000) 3) (else 4))))

;; A converter of positions in TEXT to its bytes, its lines indexed
;; once: a publication's every endpoint goes through the one made
;; for it. The walk is over each line's characters in a list,
;; never `string-ref' by index — on a non-ASCII string that
;; rescans from its start per character, and a position deep in a
;; 60 KB document of them took half a minute.
(define (make-text-positioner text)
  (let* ((texts (list->vector (string-split text "\n")))
         (starts (make-vector (vector-length texts) 0))
         (whole (bytevector-length (string->utf8 text))))
    (let loop ((i 0) (bytes 0))
      (when (< i (vector-length texts))
        (vector-set! starts i bytes)
        (loop (+ i 1) (+ bytes (bytevector-length (string->utf8 (vector-ref texts i))) 1))))
    (lambda (at)
      (let ((line (max 0 (dict-ref at "line" 0))) (col (dict-ref at "character" 0)))
        (if (>= line (vector-length texts))
            whole
            (let cols ((cs (string->list (vector-ref texts line))) (units col) (bytes (vector-ref starts line)))
              (cond ((or (null? cs) (<= units 0)) bytes)
                    ;; A CR closing the line is its break's other
                    ;; half, not a character of it.
                    ((and (char=? (car cs) #\return) (null? (cdr cs))) bytes)
                    (else (cols (cdr cs) (- units (if (> (char->integer (car cs)) #xFFFF) 2 1))
                                (+ bytes (char-utf8-length (car cs))))))))))))

(define (text-position-byte text at)
  "Return the byte of position AT in TEXT, a plain string.
As the byte of a position in a document, for a document's earlier
revision, whose text a session kept as it was sent (no document
answers for those)."
  ((make-text-positioner text) at))

;; D's revision and text read as one: (revision . text), or #f when
;; the document moved between the readings. Tasks are preemptible,
;; and a version named apart from its text is how a server comes to
;; hold bytes it will reason about under another revision's name.
(define (document-snapshot d)
  ;; `document-string', one call: a length read apart from its
  ;; substring is invalidated by the very edit this guards against
  ;; — thrown as a bad range, not read as a moved document.
  (let* ((before (document-revision d)) (text (document-string d)))
    (and (= before (document-revision d)) (cons before text))))

;;; Servers and sessions

(define-record-type lsp-server
  (%make-lsp-server name doc command modes languages)
  lsp-server?
  (name lsp-server-name)
  (doc lsp-server-doc)
  (command lsp-server-command)
  (modes lsp-server-modes)
  (languages lsp-server-languages))

(define %lsp-servers (make-registry 'lsp-servers))

(define (register-lsp-server! name doc #:command [command '()] #:modes [modes '()] #:languages [languages '()])
  "Declare the language server NAME, documented by DOC.
COMMAND is the program and its arguments as `process-spawn' takes
them; MODES the major modes it serves (a buffer whose mode derives
from one attaches), LANGUAGES the syntax languages it answers for:
where both are given, a buffer must satisfy both, so a server under
a broad mode keeps to its own language. No command declares a
server the user completes in their own code, by declaring it again."
  (registry-add! %lsp-servers name (%make-lsp-server name doc command modes languages))
  name)

(define-syntax define-lsp-server
  (syntax-rules ()
    "Declare the language server NAME, documented by DOC.
The other arguments are keywords, as `register-lsp-server!' takes them."
    ((_ name doc arg ...) (register-lsp-server! 'name doc arg ...))))

(define (lsp-server-names)
  "Return the names of the language servers declared."
  (registry-keys %lsp-servers))

(define-record-type lsp-session
  (%make-lsp-session server root conn state editor open generation)
  lsp-session?
  (server session-server)
  (root session-root)
  (conn session-conn set-session-conn!)
  ;; starting, ready or dead.
  (state session-state set-session-state!)
  ;; The editor session that started it, told what happens.
  (editor session-editor)
  ;; URI -> (document version language path sent): what the server
  ;; has open. `sent' is the texts as they were sent, an alist
  ;; (version . text) newest first and bounded, against which the
  ;; server's positions still mean what it computed them from.
  (open session-open)
  ;; Which run of its server this is, so a predecessor's leavings
  ;; (its diagnostics, its death) are never its successor's.
  (generation session-generation))

;; Sessions by (root . server), one process each, however many
;; buffers they serve.
(define %lsp-sessions (make-hash-table))
(define %lsp-generation 0)

;; Diagnostics by document path: an alist (generation findings...)
;; per session that reported on it, each finding (revision from to
;; face message server) in that revision's bytes. A buffer's servers
;; each keep their own say: one publishing nothing clears its own
;; findings and nobody else's.
(define %lsp-diagnostics (make-hash-table))

(define (say ls text) (guard (e (#t #f)) (message! (session-editor ls) text)))

;; File URIs, both directions: or a file called `100%.rs' is
;; unvisitable by the definition a server answers with.
(define (file-uri path)
  "Return PATH as a file URI.
Every byte of its UTF-8 that is not unreserved (or the path's own
separator) is percent-coded: %, #, ? and the non-ASCII alike."
  (let ((bytes (string->utf8 path)) (hex "0123456789ABCDEF"))
    (define (kept? b)
      (or (<= 48 b 57) (<= 65 b 90) (<= 97 b 122) (memv b '(45 46 47 95 126))))
    (apply string-append "file://"
           (let loop ((i 0) (parts '()))
             (if (>= i (bytevector-length bytes))
                 (reverse parts)
                 (let ((b (bytevector-u8-ref bytes i)))
                   (loop (+ i 1)
                         (cons (if (kept? b) (string (integer->char b))
                                   (string #\% (string-ref hex (quotient b 16)) (string-ref hex (remainder b 16))))
                               parts))))))))

(define (hex-digit-value c)
  (let loop ((digits (string->list "0123456789abcdef")) (n 0))
    (cond ((null? digits) #f)
          ((char=? (car digits) (char-downcase c)) n)
          (else (loop (cdr digits) (+ n 1))))))

(define (uri-file-path uri)
  "Return the path file URI names, percent-decoded, or #f.
#f for another scheme, a host that is not this one, and
percent-coding that does not decode."
  (guard (e (#t #f))
    (and (string? uri) (string-prefix? "file://" uri)
         (let* ((rest (substring uri 7 (string-length uri)))
                (path (if (string-prefix? "localhost/" rest) (substring rest 9 (string-length rest)) rest)))
           (and (string-prefix? "/" path)
                (let ((parts (let loop ((i 0) (parts '()))
                               (cond ((>= i (string-length path)) (reverse parts))
                                     ((and (char=? (string-ref path i) #\%) (< (+ i 2) (string-length path))
                                           (hex-digit-value (string-ref path (+ i 1))) (hex-digit-value (string-ref path (+ i 2))))
                                      (loop (+ i 3) (cons (bytevector (+ (* 16 (hex-digit-value (string-ref path (+ i 1))))
                                                                         (hex-digit-value (string-ref path (+ i 2)))))
                                                          parts)))
                                     ;; A % that codes nothing is not
                                     ;; a literal %: the URI is not
                                     ;; this decoder's to guess at.
                                     ((char=? (string-ref path i) #\%) #f)
                                     (else (loop (+ i 1) (cons (string->utf8 (string (string-ref path i))) parts)))))))
                  (and parts (utf8->string (apply bytevector-append parts)))))))))

;; The languageId of a buffer for its servers: its syntax language's
;; name, in LSP's own words where they differ.
(define (lsp-language-id b)
  (let ((language (buffer-language b)))
    (case language ((bash) "shellscript") ((tsx) "typescriptreact") (else (and language (symbol->string language))))))

(define (entry-history entry) (if (< 4 (length entry)) (list-ref entry 4) '()))

;; HISTORY with TEXT as the one sent at VERSION, newest first, the
;; last few kept: long enough for a server thinking one edit
;; behind, no longer (each is a whole document). Eight of them at
;; most, and by bytes as well as by count — a document's versions
;; run to its own size, so the newest's size stands for them: some
;; eight megabytes per document and session, always the newest kept.
(define (history-with history version text)
  (let ((keep (max 1 (min 8 (quotient (* 8 1024 1024) (max 1 (bytevector-length (string->utf8 text))))))))
    (let trim ((kept (cons (cons version text) (filter (lambda (e) (not (= (car e) version))) history))) (n keep))
      (if (or (null? kept) (= n 0)) '() (cons (car kept) (trim (cdr kept) (- n 1)))))))

;; LS's open entry for URI: by the URI as sent, else by the path it
;; decodes to. Servers normalize the URIs they are given (their
;; URL library's spelling, `c++' for the `c%2B%2B' sent), and a
;; publication under that spelling is this document's all the same.
(define (session-entry ls uri)
  (or (hash-table-ref/default (session-open ls) uri #f)
      (let ((path (uri-file-path uri)))
        (and path (find (lambda (e) (equal? (cadddr e) path)) (hash-table-values (session-open ls)))))))

;; The open file of LS a URI names: its path, or #f.
(define (session-path-of ls uri)
  (let ((entry (session-entry ls uri))) (and entry (cadddr entry))))

;; Forget LS's findings on PATH (its document closed, it ended):
;; other sessions' findings there stand.
(define (forget-diagnostics! ls path)
  (let ((by (hash-table-ref/default %lsp-diagnostics path #f)))
    (when by
      (let ((kept (filter (lambda (e) (not (= (car e) (session-generation ls)))) by)))
        (if (null? kept) (hash-table-delete! %lsp-diagnostics path) (hash-table-set! %lsp-diagnostics path kept))))))

;; Diagnostics of LS arrived for URI. Their positions are read here,
;; against the text of the revision they were computed from — the
;; one their version names, as it was sent — and stored as that
;; revision's bytes, to be mapped through later edits where they
;; show. A publication whose revision is not one kept, and a
;; versionless one arriving while the document has moved past what
;; was last sent, name positions in a text nobody can reconstruct:
;; they are dropped (the server publishes again for the new text).
(define (note-diagnostics! ls uri params)
  (let ((entry (session-entry ls uri)))
    (when entry
      ;; The whole publication resolves against one immutable text
      ;; under its revision's name, taken before any endpoint is
      ;; read: converting against the live document under a stamp
      ;; chosen earlier mixes revisions whenever an edit lands
      ;; while a long list of findings is being read.
      (let* ((doc (car entry)) (path (cadddr entry))
             (given (dict-ref params "version" #f))
             (kept (and given (find (lambda (kv) (= (car kv) given)) (entry-history entry))))
             (now (and given (not kept) (document-snapshot doc)))
             (latest (and (not given) (= (cadr entry) (document-revision doc))
                          (find (lambda (kv) (= (car kv) (cadr entry))) (entry-history entry))))
             (base (cond (kept (cons given (cdr kept)))
                         ((and now (= (car now) given)) now)
                         (latest (cons (cadr entry) (cdr latest)))
                         (else #f))))
        (when base
          (let* ((version (car base)) (byte (make-text-positioner (cdr base)))
                 (face-of (lambda (severity) (case severity ((2) 'diagnostic.warning) ((3) 'diagnostic.info) ((4) 'diagnostic.hint) (else 'diagnostic.error))))
                 (found (filter-map (lambda (d)
                                      (let* ((range (dict-ref d "range" #f))
                                             (a (and range (dict-ref range "start" #f)))
                                             (b (and range (dict-ref range "end" #f))))
                                        (and a b (list version (byte a) (byte b) (face-of (dict-ref d "severity" 1))
                                                       (dict-ref d "message" "") (lsp-server-name (session-server ls))))))
                                    (vector->list (dict-ref params "diagnostics" #())))))
            (hash-table-set! %lsp-diagnostics path
                             (cons (cons (session-generation ls) found)
                                   (filter (lambda (e) (not (= (car e) (session-generation ls))))
                                           (hash-table-ref/default %lsp-diagnostics path '()))))))))))

(define (on-notification ls method params)
  ;; A dead session's last words (read off its pipes as it was being
  ;; replaced) are not its successor's to show.
  (unless (eq? (session-state ls) 'dead)
    (cond ((equal? method "textDocument/publishDiagnostics") (note-diagnostics! ls (dict-ref params "uri" "") params))
          ((equal? method "window/showMessage") (say ls (dict-ref params "message" "")))
          (else #f))))

;; Send ENTRY's document's opening: its text and version read
;; together, now (an entry made while the server was starting names
;; the revision of then, which the text has since left), and the
;; text kept under that version for the server's positions.
(define (did-open! ls uri entry)
  ;; Read again until the document holds still for one reading:
  ;; an opening whose text is not its version's poisons every
  ;; position the server later reasons about.
  (let* ((doc (car entry)) (snap (let again () (or (document-snapshot doc) (again))))
         (version (car snap)) (text (cdr snap)))
    (rpc-notify! (session-conn ls) "textDocument/didOpen"
                 (dict "textDocument" (dict "uri" uri "languageId" (caddr entry) "version" version "text" text)))
    (when (hash-table-contains? (session-open ls) uri)
      (hash-table-set! (session-open ls) uri
                       (list doc version (caddr entry) (cadddr entry) (history-with (entry-history entry) version text))))))

;; Bring the server to LS's document at URI as it stands: opened if
;; it never was, sent whole if it changed, closed if its buffer
;; went. The watcher runs it over everything; a request whose
;; meaning depends on the server holding the current text (a
;; position in it, say) runs it for its document first.
(define (sync-document! ls uri)
  (let ((entry (session-entry ls uri)))
    (when entry
      (let ((doc (car entry)))
        (cond ((not (document-buffer doc))
               (rpc-notify! (session-conn ls) "textDocument/didClose" (dict "textDocument" (dict "uri" uri)))
              (hash-table-delete! (session-open ls) uri)
              (forget-diagnostics! ls (cadddr entry)))
             ((null? (entry-history entry)) (did-open! ls uri entry))
             ((not (= (document-revision doc) (cadr entry)))
              ;; One consistent reading, or this pass sends nothing:
              ;; the next one (50 ms on) catches a held-still text.
              (let ((snap (document-snapshot doc)))
                (when (and snap (not (= (car snap) (cadr entry))))
                  (rpc-notify! (session-conn ls) "textDocument/didChange"
                               (dict "textDocument" (dict "uri" uri "version" (car snap))
                                     "contentChanges" (vector (dict "text" (cdr snap)))))
                  (hash-table-set! (session-open ls) uri
                                   (list doc (car snap) (caddr entry) (cadddr entry)
                                         (history-with (entry-history entry) (car snap) (cdr snap))))))))))))

(define (sync-session! ls)
  (for-each (lambda (uri) (sync-document! ls uri)) (map car (hash-table->alist (session-open ls)))))

(define (session-key root name) (string-append root "\n" (symbol->string name)))

(define (start-session! s server root)
  (set! %lsp-generation (+ %lsp-generation 1))
  (define ls (%make-lsp-session server root #f 'starting s (make-hash-table) %lsp-generation))
  (let ((conn (rpc-connect (car (lsp-server-command server)) (cdr (lsp-server-command server))
                            #:directory root
                            #:on-note (lambda (m p) (on-notification ls m p))
                            #:on-ask (lambda (m p) (if (equal? m "workspace/configuration")
                                                       (make-vector (vector-length (dict-ref p "items" #())) 'null)
                                                       'null))
                            #:on-dead (lambda ()
                                        (set-session-state! ls 'dead)
                                        ;; Only ever its own slot: a
                                        ;; replacement may hold the key
                                        ;; by the time this runs. One
                                        ;; that no longer holds it was
                                        ;; ended on purpose (a restart,
                                        ;; a failed start): its ending
                                        ;; is not news twice.
                                        (let ((held (eq? (hash-table-ref/default %lsp-sessions (session-key root (lsp-server-name server)) #f) ls)))
                                          (when held
                                            (hash-table-delete! %lsp-sessions (session-key root (lsp-server-name server))))
                                          (for-each (lambda (entry) (forget-diagnostics! ls (cadddr entry)))
                                                    (hash-table-values (session-open ls)))
                                          (when held (say ls (string-append (symbol->string (lsp-server-name server)) " ended"))))))))
    (set-session-conn! ls conn)
    (hash-table-set! %lsp-sessions (session-key root (lsp-server-name server)) ls)
    (spawn (lambda ()
             ;; A start that fails leaves nothing discoverable
             ;; behind: out of the table, its process killed, so the
             ;; next attach starts the server again rather than
             ;; finding this one's corpse in its slot.
             (guard (e (#t (set-session-state! ls 'dead)
                           (when (eq? (hash-table-ref/default %lsp-sessions (session-key root (lsp-server-name server)) #f) ls)
                             (hash-table-delete! %lsp-sessions (session-key root (lsp-server-name server))))
                           (guard (e2 (#t #f)) (process-kill (rpc-process conn)))
                           (say ls (string-append (symbol->string (lsp-server-name server)) ": " (error-text e)))))
                             (rpc-request! conn "initialize"
                             (dict "processId" 'null "rootUri" (file-uri root)
                                   "capabilities" (dict "textDocument" (dict "synchronization" (dict "didSave" #f)
                                                                             "publishDiagnostics" (dict)
                                                                             "definition" (dict))
                                                        "workspace" (dict "configuration" #t))
                                   "workspaceFolders" (vector (dict "uri" (file-uri root) "name" root)))
                             #:timeout 60000)
                             (rpc-notify! conn "initialized" (dict))
               ;; Open what is waiting, then `ready': what the
               ;; state promises a waiter is a server that has
               ;; the documents, not one about to be sent them.
               (hash-table-for-each (session-open ls) (lambda (uri entry) (did-open! ls uri entry)))
               (set-session-state! ls 'ready)
               (say ls (string-append (symbol->string (lsp-server-name server)) " ready in " (abbreviate-path root))))))
    (spawn (lambda () (let loop () (sleep 50) (when (eq? (session-state ls) 'ready) (guard (e (#t #f)) (sync-session! ls))) (unless (eq? (session-state ls) 'dead) (loop)))))
    ls))

(define (session-for! s server root)
  ;; A dead session found in the slot (ended, its removal not yet
  ;; run) is no session: starting its successor takes the slot,
  ;; and the dead one's notices check identity before touching it.
  (let ((found (hash-table-ref/default %lsp-sessions (session-key root (lsp-server-name server)) #f)))
    (if (and found (not (eq? (session-state found) 'dead))) found (start-session! s server root))))

(define (attach! s b server root)
  (let* ((ls (session-for! s server root))
         (doc (buffer-document b))
         (path (document-path doc))
         (uri (file-uri path))
         (entry (list doc (document-revision doc) (lsp-language-id b) path '())))
    (unless (hash-table-contains? (session-open ls) uri)
      (hash-table-set! (session-open ls) uri entry)
      (when (eq? (session-state ls) 'ready) (did-open! ls uri entry)))
    ls))

;; Open a previous session's document (its ENTRY there) in LS, at
;; its current revision: a restart's inheritance.
(define (reopen! ls entry)
  (let* ((doc (car entry)) (uri (file-uri (cadddr entry)))
         (new (list doc (document-revision doc) (caddr entry) (cadddr entry) '())))
    (unless (hash-table-contains? (session-open ls) uri)
      (hash-table-set! (session-open ls) uri new)
      (when (eq? (session-state ls) 'ready) (did-open! ls uri new)))
    ls))

;; The servers serving buffer B's major mode.
(define (servers-for b)
  (filter-map (lambda (name)
                (let ((server (registry-ref %lsp-servers name)))
                  (and (pair? (lsp-server-command server))
                       (find (lambda (m) (derived-mode? (buffer-mode b) m)) (lsp-server-modes server))
                       (let ((languages (lsp-server-languages server)))
                         (or (null? languages) (memq (buffer-language b) languages)))
                       server)))
              (lsp-server-names)))

(define (diagnostic-layer d from to)
  ;; Every reporting session's findings, their stored bytes of the
  ;; revision they were computed against mapped to now.
  (let ((found (append-map cdr (hash-table-ref/default %lsp-diagnostics (document-path d) '()))))
    (filter-map (lambda (e)
                  (let* ((revision (car e))
                         (moved (lambda (p) (if (= revision (document-revision d)) p (document-map-position d p revision))))
                         (a (moved (cadr e))) (b (moved (caddr e))))
                    (and a b (< a b) (list a b (list-ref e 3)))))
                found)))

(define-minor-mode lsp-mode
  "Show the diagnostics of the buffer's language servers.
Turned on by \\[lsp] where it attaches one."
  #:layer (lambda (d from to) (diagnostic-layer d from to)))

(define (lsp-attach! s b)
  "Attach buffer B to the language servers serving its mode, in session S.
The servers start where none runs for the buffer's project. Return
their sessions."
  (let ((servers (servers-for b)))
    (if (null? servers)
        (begin (message! s "No language server serves this buffer") '())
        (begin
          (unless (document-path (buffer-document b)) (error "lsp: the buffer has no file"))
          (when (remote-path? (document-path (buffer-document b))) (error "lsp: not on another machine yet"))
          (let ((root (project-root (default-directory s))))
            (set-option! 'lsp-mode #t #:buffer b)
            (map (lambda (server) (attach! s b server root)) servers))))))

(define-command (lsp s n)
  "Attach the focused buffer to its language servers.
They start in the project's root, one process per server and root;
their diagnostics show in the buffer, under `lsp-mode'."
  (lsp-attach! s (or (current-buffer s) (error "No buffer"))))

;; Shut ROOT's session of SERVER down and start its successor,
;; every document the old one had open opened in the new. The old
;; reader's death notice, arriving whenever it does, takes nothing
;; of the successor's: not its slot (checked by identity there),
;; not its diagnostics (kept by generation).
(define (restart-server! s server root)
  (let ((old (hash-table-ref/default %lsp-sessions (session-key root (lsp-server-name server)) #f)))
    (when old
      (let ((entries (hash-table-values (session-open old))))
        (set-session-state! old 'dead)
        (when (eq? (hash-table-ref/default %lsp-sessions (session-key root (lsp-server-name server)) #f) old)
          (hash-table-delete! %lsp-sessions (session-key root (lsp-server-name server))))
        (for-each (lambda (entry) (forget-diagnostics! old (cadddr entry))) entries)
        (guard (e (#t #f)) (process-kill (rpc-process (session-conn old))))
        (let ((ls (start-session! s server root)))
          (for-each (lambda (entry) (when (document-buffer (car entry)) (reopen! ls entry))) entries)
          ls)))))

(define-command (lsp-restart s n)
  "Restart the language servers of the focused buffer.
Each is shut down and started again, its buffers opened again."
  (let* ((b (or (current-buffer s) (error "No buffer"))) (root (project-root (default-directory s))))
    (for-each (lambda (server) (restart-server! s server root)) (servers-for b))
    (lsp-attach! s b)))

;;; Asking: definition, the first question with an answer to jump to.

;; A slot per position asked about: a newer ask replaces an older.
(define lsp-ask-slot (make-request-slot))

;; Whether LS became ready, waited out in this task like a
;; request's answer: a server starting up (and the documents being
;; sent to it) is not a server that has nothing to say.
(define (await-ready ls)
  (let loop ((waited 0))
    (cond ((eq? (session-state ls) 'ready) #t)
          ((eq? (session-state ls) 'dead) #f)
          ((>= waited 60000) #f)
          (else (sleep 10) (loop (+ waited 10))))))

(define-command (lsp-find-definition s n)
  "Go to the name at point's definition, as a language server tells it.
The first of the buffer's servers that answers says where: its file
is visited, at the place. Without servers attached, `lsp' attaches them."
  (let* ((b (or (current-buffer s) (error "No buffer")))
         (doc (buffer-document b))
         (path (or (document-path doc) (error "lsp-find-definition: the buffer has no file")))
         (sessions (lsp-attach! s b))
         (at (point s)) (at-revision (document-revision doc)))
    (when (null? sessions) (error "No language server serves this buffer"))
    (request! lsp-ask-slot
              (lambda ()
                (let loop ((todo sessions))
                  (and (pair? todo)
                       (let* ((ls (car todo)) (conn (session-conn ls)) (uri (file-uri path))
                              (answer (guard (e (#t #f))
                                        ;; Ready, with this document
                                        ;; as it stands, or the
                                        ;; position means nothing to
                                        ;; the server yet.
                                        (and (await-ready ls)
                                             (begin (sync-document! ls uri)
                                                    (let ((entry (session-entry ls uri)))
                                                      (and entry (pair? (entry-history entry))
                                                           (let ((moved (if (= at-revision (document-revision doc)) at
                                                                            (document-map-position doc at at-revision))))
                                                             (and moved (rpc-request! conn "textDocument/definition"
                                                                                      (dict "textDocument" (dict "uri" uri) "position" (position-of doc moved))))))))))))
                         (or (and answer (not (eq? answer 'null))
                                  (let ((one (if (dict? answer) answer (and (vector? answer) (> (vector-length answer) 0) (vector-ref answer 0)))))
                                    (and one (let ((uri (dict-ref one "uri" (dict-ref one "targetUri" #f)))
                                                   (range (dict-ref one "range" (dict-ref one "targetSelectionRange" #f))))
                                               (and uri range (cons uri range))))))
                             (loop (cdr todo)))))))
              (lambda (found)
                (if found
                    (let ((there (uri-file-path (car found))))
                      (if there
                          (let* ((d (file-document there))
                                 (v (show-document! s d))
                                 (pos (position-byte d (dict-ref (cdr found) "start"))))
                            (view-set-ranges! v (list (list pos pos)) 0))
                          (message! s "The definition is not in a file here")))
                    (message! s "No definition there")))
              #:fail (lambda (e) (message! s (error-text e))))))

;;; The servers built in.

(define-lsp-server rust-analyzer
  "Rust, by rust-analyzer."
  #:command '("rust-analyzer") #:modes '(prog-mode) #:languages '(rust))
