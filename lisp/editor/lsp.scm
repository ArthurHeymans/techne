;;; Language servers (EDITOR.md, section 3's layers; PLAN.md's language
;;; step): a client of them, as Eglot is Emacs's.
;;;
;;; A server is declared by the command that runs it and the buffers it
;;; serves, in a scope-owned registry like modes:
;;;
;;;   (define-lsp-server rust-analyzer "Rust, by rust-analyzer."
;;;     #:command '("rust-analyzer") #:languages '(rust))
;;;
;;; A file's buffer attaches to the servers serving it as it is opened
;;; (`lsp-auto', as `eglot-ensure'), or with M-x lsp: one session per
;;; server and project root, started on first use, however many buffers
;;; it serves. What the server pushes — diagnostics — shows under the
;;; `lsp-mode' minor mode, as underlines and a count in the mode line;
;;; M-g n and M-g p go from one to the next, M-g f lists them.
;;;
;;; A document's server follows it without asking every so often: a task
;;; per document waits for the next edit (`document-await-change') and
;;; sends what changed since the version the server has, incrementally,
;;; in the position encoding the two agreed (UTF-8 where the server takes
;;; it, else UTF-16). A version is the document's revision. What a server
;;; says of a version — a diagnostic's range — is read against that
;;; version's text and mapped through the edits since, so it stays on the
;;; text it was said of while the server catches up.
;;;
;;; Underneath is `rpc.scm': nothing there knows LSP.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "faces.scm")
(require "commands.scm")
(require "targets.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "files.scm")
(require "markdown.scm")
(require "projects.scm")
(require "remote.scm")
(require "syntax.scm")
(require "rpc.scm")
(require "requests.scm")
(require "xref.scm")
(require "eldoc.scm")
(require "operations.scm")
(require "lsp-log.scm")

(provide define-lsp-server register-lsp-server! lsp-server-names lsp-server-doc
         lsp lsp-shutdown lsp-restart lsp-next-diagnostic lsp-previous-diagnostic lsp-diagnostics
         lsp-rename lsp-format lsp-format-buffer lsp-code-actions lsp-events lsp-apply-workspace-edit!
         lsp-attach! lsp-buffer-sessions lsp-session? lsp-session-server-name lsp-session-capability lsp-session-encoding
         lsp-sync! lsp-request lsp-notify lsp-text-document lsp-position lsp-range-positions lsp-location
         lsp-diagnostics-at file-uri uri-file-path)

;;; Faces and options

(define-face diagnostic "A language server's diagnostic." #:underline 'curl)
(define-face diagnostic.error "An error, from a language server." #:underline '(curl red))
(define-face diagnostic.warning "A warning, from a language server." #:underline '(curl yellow))
(define-face diagnostic.info "A note, from a language server." #:underline '(dotted sky))
(define-face diagnostic.hint "A hint, from a language server." #:underline '(dotted overlay))
(define-face lsp-inlay-hint "What a language server shows in the line: a type, a parameter's name." #:inherit 'shadow #:slant 'italic)
(define-face lsp-inlay-hint.type "A type a language server shows after a name." #:inherit 'lsp-inlay-hint)
(define-face lsp-inlay-hint.parameter "A parameter's name a language server shows before an argument." #:inherit 'lsp-inlay-hint)

(define-option lsp-inlay-hints #t
  "Whether a buffer shows its language servers' inlay hints.
As Eglot's `eglot-inlay-hints-mode': the types of names that do not
spell theirs out, the parameters' names before arguments."
  #:type 'boolean)

(define-option lsp-auto #t
  "Whether a file's buffer attaches to its language servers as it opens.
As Eglot's `eglot-ensure': a server whose program is not installed is
passed over quietly; \\[lsp] says why."
  #:type 'boolean)

;;; URIs and positions

(define (file-uri path)
  "Return PATH as a file URI.
Every byte of its UTF-8 that is not unreserved (or the path's own
separator) is percent-coded: %, #, ? and the non-ASCII alike."
  (let ((bytes (string->utf8 path)) (hex "0123456789ABCDEF"))
    (define (kept? b) (or (<= 48 b 57) (<= 65 b 90) (<= 97 b 122) (memv b '(45 46 47 95 126))))
    (apply string-append "file://"
           (let loop ((i 0) (parts '()))
             (if (>= i (bytevector-length bytes))
                 (reverse parts)
                 (let ((b (bytevector-u8-ref bytes i)))
                   (loop (+ i 1)
                         (cons (if (kept? b)
                                   (string (integer->char b))
                                   (string #\% (string-ref hex (quotient b 16)) (string-ref hex (remainder b 16))))
                               parts))))))))

(define (hex-value c)
  (let ((n (char->integer (char-downcase c))))
    (cond ((<= 48 n 57) (- n 48)) ((<= 97 n 102) (- n 87)) (else #f))))

(define (uri-file-path uri)
  "Return the path the file URI names, percent-decoded, or #f.
#f for another scheme, a host that is not this one, and percent-coding
that does not decode."
  (and (string? uri) (string-prefix? "file://" uri)
       (let* ((rest (substring uri 7 (string-length uri)))
              (path (if (string-prefix? "localhost/" rest) (substring rest 9 (string-length rest)) rest))
              (n (string-length path)))
         (and (string-prefix? "/" path)
              (let loop ((i 0) (parts '()))
                (cond ((>= i n) (guard (e (#t #f)) (utf8->string (apply bytevector-append (reverse parts)))))
                      ((char=? (string-ref path i) #\%)
                       (let ((hi (and (< (+ i 2) n) (hex-value (string-ref path (+ i 1)))))
                             (lo (and (< (+ i 2) n) (hex-value (string-ref path (+ i 2))))))
                         (and hi lo (loop (+ i 3) (cons (bytevector (+ (* 16 hi) lo)) parts)))))
                      (else (loop (+ i 1) (cons (string->utf8 (string (string-ref path i))) parts)))))))))

;;; Servers

(define-record-type lsp-server
  (%make-lsp-server name doc command modes languages roots settings initialization)
  lsp-server?
  (name lsp-server-name)
  (doc lsp-server-doc*)
  ;; The program and its arguments.
  (command lsp-server-command)
  ;; Major modes it serves (derived ones too) and syntax languages.
  (modes lsp-server-modes)
  (languages lsp-server-languages)
  ;; Files one of which a project of its language has (Cargo.toml): a
  ;; file opened where none is, in its directory or one up to the
  ;; project's root, attaches only when asked to.
  (roots lsp-server-roots)
  ;; What `workspace/configuration' answers, a dict by section; and the
  ;; `initializationOptions'.
  (settings lsp-server-settings)
  (initialization lsp-server-initialization))

(define %servers (make-registry 'lsp-servers))

(define (register-lsp-server! name doc #:command [command '()] #:modes [modes '()] #:languages [languages '()]
                              #:roots [roots '()] #:settings [settings (dict)] #:initialization [initialization 'null])
  "Declare the language server NAME, documented by DOC.
COMMAND is the program and its arguments, as `process-spawn' takes them.
A buffer is served when its major mode is or derives from one of MODES,
or its syntax language is one of LANGUAGES. ROOTS are file names one of
which its projects have (\"Cargo.toml\"): a file opened with none in its
directory or one above attaches only when asked to (\\[lsp]). The
server runs in the file's version-control root, else in the nearest
directory with one of ROOTS. SETTINGS is a dict of what
the server asks for by section (`workspace/configuration'), as
{\"rust-analyzer\": {...}}; INITIALIZATION its `initializationOptions'.
Declaring a server again (a package's own) replaces the declaration."
  (registry-add! %servers name (%make-lsp-server name doc command modes languages roots settings initialization))
  name)

(define-syntax define-lsp-server
  (syntax-rules ()
    "Declare the language server NAME, documented by DOC.
The other arguments are keywords, as `register-lsp-server!' takes them."
    ((_ name doc arg ...) (register-lsp-server! 'name doc arg ...))))

(define (lsp-server-names)
  "Return the names of the language servers declared."
  (registry-keys %servers))

(define (lsp-server-doc name)
  "Return the documentation of the language server NAME, or #f."
  (let ((s (registry-ref %servers name))) (and s (lsp-server-doc* s))))

;; The servers serving buffer B.
(define (servers-for b)
  (filter-map (lambda (name)
                (let ((server (registry-ref %servers name)))
                  (and (pair? (lsp-server-command server))
                       (or (any (lambda (m) (derived-mode? (buffer-mode b) m)) (lsp-server-modes server))
                           (memq (buffer-language b) (lsp-server-languages server)))
                       server)))
              (sort (lsp-server-names) (lambda (a b) (string<? (symbol->string a) (symbol->string b))))))

;; Where a server for the file at PATH runs, and whether its project's
;; files (#:roots) are there: (root . found). Up from the file to its
;; version-control root, else to /, the nearest directory with one of
;; them; the root is the version-control root, else that directory,
;; else the file's own.
(define (server-root server path)
  (let ((markers (lsp-server-roots server)))
    (let up ((dir (directory-of path)) (found #f))
      (let* ((found (or found (and (any (lambda (m) (file-exists? (string-append dir m))) markers) dir)))
             (vc (or (file-exists? (string-append dir ".git")) (file-exists? (string-append dir ".jj")))))
        (cond (vc (cons dir (or (null? markers) (and found #t))))
              ((or (string=? dir "/") (string=? dir ""))
               (cons (or found (directory-of path)) (or (null? markers) (and found #t))))
              (else (up (directory-of (substring dir 0 (- (string-length dir) 1))) found)))))))

;; Whether PROGRAM is one to run: a path to a file, or a name on PATH.
(define (installed? program)
  (if (string-contains program "/")
      (file-exists? program)
      (any (lambda (dir) (and (not (string=? dir "")) (file-exists? (string-append dir "/" program))))
           (string-split (or (getenv "PATH") "") ":"))))

;;; Sessions: a server's process for a project, and the documents it has.

(define-record-type lsp-session
  (%make-session server root conn state ready gone capabilities documents progress generation ready-at)
  lsp-session?
  (server session-server)
  (root session-root)
  (conn session-conn set-session-conn!)
  ;; starting, ready, stopping (asked to shut down) or dead.
  (state session-state set-session-state!)
  ;; Closed once the server is ready, or dead: what waits for it waits
  ;; on this, not by looking every so often.
  (ready session-ready)
  ;; Closed once its process has ended.
  (gone session-gone)
  ;; What the server said it does (its `capabilities'), a dict.
  (capabilities session-capabilities set-session-capabilities!)
  ;; Path -> the document's state in this session (`tracked').
  (documents session-documents)
  ;; Work the server reports progress on: token -> its text now.
  (progress session-progress)
  ;; Which run this is: a predecessor's leavings are never its successor's.
  (generation session-generation)
  ;; When it became ready (`current-milliseconds'), #f before.
  (ready-at session-ready-at set-session-ready-at!))

(define (lsp-session-server-name ls)
  "Return the name of the language server of the session LS."
  (lsp-server-name (session-server ls)))

;; Sessions by root and server name.
(define %sessions (make-hash-table))
(define %generation 0)
(define (session-key root server) (string-append root "\n" (symbol->string (lsp-server-name server))))

(define (say text)
  (let ((s (current-session))) (when s (message! s text))))

(define (lsp-session-capability ls . path)
  "Return what the server of session LS said of the capability at PATH.
PATH is a list of keys into its `capabilities', as \"hoverProvider\";
#f where it said nothing (or false)."
  (let loop ((v (session-capabilities ls)) (path path))
    (cond ((null? path) (and (not (eq? v 'null)) v))
          ((dict? v) (loop (dict-ref v (car path) #f) (cdr path)))
          (else #f))))

(define (lsp-session-encoding ls)
  "Return the position encoding agreed with LS's server.
That is \"utf-8\" where it takes it, else \"utf-16\"."
  (if (equal? (lsp-session-capability ls "positionEncoding") "utf-8") "utf-8" "utf-16"))

;; Whether the server is told of documents opening and closing: always
;; for a bare sync kind, else as its options say.
(define (open-close? ls)
  (let ((sync (lsp-session-capability ls "textDocumentSync")))
    (cond ((number? sync) #t)
          ((dict? sync) (eq? (dict-ref sync "openClose" #f) #t))
          (else #f))))

;; How the server takes a document's changes: 0 not at all, 1 whole, 2 incrementally.
(define (sync-kind ls)
  (let ((sync (lsp-session-capability ls "textDocumentSync")))
    (cond ((number? sync) sync)
          ((dict? sync) (dict-ref sync "change" 0))
          (else 0))))

(define (wants-save? ls)
  (let ((sync (lsp-session-capability ls "textDocumentSync")))
    (and (dict? sync) (let ((save (dict-ref sync "save" #f))) (and save (not (eq? save 'null)) save)))))

(define (lsp-request ls method params #:timeout [patience rpc-timeout] #:sent [sent #f] #:on-error [on-error #f])
  "Ask METHOD of the server of session LS with PARAMS; return its answer.
The waiting is the calling task's. An error answer, a dead server and no
answer in PATIENCE milliseconds (#:timeout) are errors. SENT and
ON-ERROR, if given, are as `rpc-request!` takes them."
  (rpc-request! (session-conn ls) method params #:timeout patience #:sent sent #:on-error on-error))

(define (lsp-notify ls method params)
  "Send the server of session LS the notification METHOD with PARAMS."
  (rpc-notify! (session-conn ls) method params))

;;; What the client says it does. The answers' handling in xref.scm and
;;; eldoc.scm extends it.

(define (client-capabilities)
  (dict "general" (dict "positionEncodings" '("utf-8" "utf-16"))
        "workspace" (dict "configuration" #t "workspaceFolders" #t "applyEdit" #t
                          "inlayHint" (dict "refreshSupport" #t)
                          ;; Text edits only, all or none: those made are
                          ;; undone if one cannot be.
                          "workspaceEdit" (dict "documentChanges" #t "failureHandling" "undo")
                          "symbol" (dict) "didChangeConfiguration" (dict))
        "textDocument" (dict "synchronization" (dict "didSave" #t "willSave" #f)
                             "publishDiagnostics" (dict "versionSupport" #t "relatedInformation" #f)
                             "definition" (dict "linkSupport" #t)
                             "declaration" (dict "linkSupport" #t)
                             "typeDefinition" (dict "linkSupport" #t)
                             "implementation" (dict "linkSupport" #t)
                             "references" (dict)
                             "documentSymbol" (dict "hierarchicalDocumentSymbolSupport" #t)
                             "hover" (dict "contentFormat" '("plaintext" "markdown"))
                             "signatureHelp" (dict "signatureInformation"
                                                   (dict "documentationFormat" '("plaintext" "markdown")
                                                         "parameterInformation" (dict "labelOffsetSupport" #t)))
                             "rename" (dict "prepareSupport" #t)
                             "formatting" (dict) "rangeFormatting" (dict)
                             "codeAction" (dict "codeActionLiteralSupport"
                                                (dict "codeActionKind"
                                                      (dict "valueSet" '("" "quickfix" "refactor" "refactor.extract" "refactor.inline"
                                                                         "refactor.rewrite" "source" "source.organizeImports")))
                                                "resolveSupport" (dict "properties" '("edit")))
                             "inlayHint" (dict))
        "window" (dict "workDoneProgress" #t "showMessage" (dict))))

;;; The server's notifications and requests.

;; The setting at SECTION ("a.b") of SETTINGS, or null.
(define (setting settings section)
  (let loop ((v settings) (keys (if (or (not section) (eq? section 'null)) '() (string-split section "."))))
    (cond ((null? keys) v)
          ((and (dict? v) (dict-contains? v (car keys))) (loop (dict-ref v (car keys)) (cdr keys)))
          (else 'null))))

(define (on-ask ls method params)
  (cond ((equal? method "workspace/configuration")
         (map (lambda (item) (setting (lsp-server-settings (session-server ls)) (dict-ref item "section" #f)))
              (vector->list (dict-ref params "items" #()))))
        ((member method '("window/workDoneProgress/create" "client/registerCapability" "client/unregisterCapability"))
         'null)
        ((equal? method "workspace/inlayHint/refresh")
         ;; Asked again where they are next shown.
         ;; Kept meanwhile, as of no revision: asked again where shown,
         ;; and what was being asked is not wanted.
         (for-each (lambda (t)
                     (set-tracked-asking! t '())
                     (set-tracked-hints! t (map (lambda (w) (cons -1 (cdr w))) (tracked-hints t))))
                   (hash-table-values (session-documents ls)))
         'null)
        ((equal? method "workspace/workspaceFolders")
         (list (dict "uri" (file-uri (session-root ls)) "name" (session-root ls))))
        ((equal? method "workspace/applyEdit")
         (let ((r (guard (e (#t (list 'failed 0 (error-text e)))) (apply-workspace-edit ls (dict-ref params "edit") '()))))
           (if (eq? (car r) 'ok)
               (dict "applied" #t)
               (begin
                 (say (caddr r))
                 (dict "applied" #f "failureReason" (caddr r) "failedChange" (cadr r))))))
        ((equal? method "window/showMessageRequest")
         (say (dict-ref params "message" ""))
         'null)
        (else rpc-unsupported)))

(define (on-note ls method params)
  (unless (eq? (session-state ls) 'dead)
    (cond ((equal? method "textDocument/publishDiagnostics") (note-diagnostics! ls params))
          ((equal? method "$/progress") (note-progress! ls params))
          ((equal? method "window/showMessage")
           (when (<= (dict-ref params "type" 4) 2)
             (say (string-append (symbol->string (lsp-server-name (session-server ls))) ": " (dict-ref params "message" "")))))
          (else #f))))

;; Progress the server reports: shown in the mode line while it lasts.
(define (note-progress! ls params)
  (let* ((token (dict-ref params "token" #f)) (value (dict-ref params "value" (dict))) (kind (dict-ref value "kind" "")))
    (cond ((equal? kind "end") (hash-table-delete! (session-progress ls) token))
          (else
           (let* ((old (hash-table-ref/default (session-progress ls) token #f))
                  (title (dict-ref value "title" (if old (car old) "")))
                  (message (dict-ref value "message" #f))
                  (percent (dict-ref value "percentage" #f)))
             (hash-table-set! (session-progress ls) token
                              (list title
                                    (string-append title
                                                   (if (and message (not (eq? message 'null))) (string-append " " message) "")
                                                   (if (number? percent) (string-append " " (number->string (exact (round percent))) "%") "")))))))))

;;; Starting and ending

(define (start-session! server root)
  (set! %generation (+ %generation 1))
  (let* ((ls (%make-session server root #f 'starting (make-channel) (make-channel) (dict) (make-hash-table) (make-hash-table) %generation #f))
         (name (symbol->string (lsp-server-name server)))
         (log (session-log ls))
         (stderr (lsp-log-stderr-count log)))
    (lsp-log-mark! log (string-append "started: " (string-join (lsp-server-command server) " ")))
    (set-session-conn! ls (rpc-connect (car (lsp-server-command server)) (cdr (lsp-server-command server))
                                       #:directory root
                                       #:on-note (lambda (m p) (on-note ls m p))
                                       #:on-ask (lambda (m p) (on-ask ls m p))
                                       #:on-dead (lambda () (session-ended! ls stderr))
                                       #:on-event (lambda (direction text) (lsp-log-event! log direction text))))
    (hash-table-set! %sessions (session-key root server) ls)
    (spawn (lambda ()
             (guard (e (#t (say (string-append name ": " (error-text e)))
                           (let ((buffers (session-buffers ls)))
                             (end-session! ls)
                             (session-lost! ls buffers))))
               (let ((answer (lsp-request ls "initialize"
                                          (dict "processId" 'null
                                                "clientInfo" (dict "name" "techne")
                                                "rootUri" (file-uri root) "rootPath" root
                                                "workspaceFolders" (list (dict "uri" (file-uri root) "name" root))
                                                "initializationOptions" (lsp-server-initialization server)
                                                "capabilities" (client-capabilities))
                                          #:timeout 60000)))
                 (set-session-capabilities! ls (dict-ref answer "capabilities" (dict)))
                 (lsp-notify ls "initialized" (dict))
                 (when (pair? (dict-keys (lsp-server-settings server)))
                   (lsp-notify ls "workspace/didChangeConfiguration" (dict "settings" (lsp-server-settings server))))
                 ;; The documents waiting are opened, then it is ready:
                 ;; what waits for it finds them there.
                 (set-session-state! ls 'ready)
                 (set-session-ready-at! ls (current-milliseconds))
                 (for-each (lambda (t) (open-tracked! ls t)) (hash-table-values (session-documents ls)))
                 (channel-close (session-ready ls))
                 (say (string-append name " ready in " (abbreviate-path root)))))))
    ls))

;; The process ended: by itself, said; asked to, by `end-session!'.
;; STDERR is where its run's stderr starts in its log.
(define (session-ended! ls [stderr 0])
  (guard (e (#t #f)) (channel-close (session-gone ls)))
  (lsp-log-mark! (session-log ls) "ended")
  (unless (memq (session-state ls) '(stopping dead))
    (set-session-state! ls 'dead)
    (let ((last (lsp-log-stderr-tail (session-log ls) stderr)) (buffers (session-buffers ls)))
      (forget-session! ls)
      (let ((again (session-lost! ls buffers)))
        (say (string-append (symbol->string (lsp-server-name (session-server ls))) " ended"
                            (if last (string-append ": " last) "")
                            (cond ((number? again) (string-append "; reconnecting in " (number->string (quotient again 1000)) "s"))
                                  ((eq? again 'gave-up) "; not again, after that many tries: M-x lsp-restart")
                                  (else ""))))))))

;; The buffers LS's documents are shown in.
(define (session-buffers ls)
  (filter-map (lambda (t) (document-buffer (tracked-document t))) (hash-table-values (session-documents ls))))

;;; Reconnecting: a server that ended unasked, having been ready (or
;;; starting again after that), is started again for its buffers after
;;; a while, a longer one each time; reset once it has been ready a
;;; minute. What was being asked of it is not asked again.

(define-option lsp-autoreconnect #t
  "Whether a language server that ends unasked is started again.
As Eglot's `eglot-autoreconnect': one that was ready, after a second,
then three, ten and thirty, each time it ends again before it has been
ready a minute; then not, until \\[lsp-restart]. Its buffers' diagnostics
go meanwhile."
  #:type 'boolean)

(define lsp-reconnect-delays '(1000 3000 10000 30000))

;; A server's and root's reconnecting: the tries made, its buffers,
;; whether one is waited for (pending), or it gave up.
(define-record-type reconnect
  (make-reconnect attempts buffers state)
  reconnect?
  (attempts reconnect-attempts)
  (buffers reconnect-buffers)
  (state reconnect-state set-reconnect-state!))

;; By session key.
(define %reconnects (make-hash-table))

;; LS ended unasked, serving BUFFERS: start it again later, or not.
;; Return the delay in milliseconds, `gave-up', or #f.
(define (session-lost! ls buffers)
  (let* ((key (session-key (session-root ls) (session-server ls)))
         (old (hash-table-ref/default %reconnects key #f))
         (ready-at (session-ready-at ls))
         (attempts (cond ((and ready-at (>= (- (current-milliseconds) ready-at) 60000)) 0)
                         (old (reconnect-attempts old))
                         (else 0)))
         (buffers (filter (lambda (b) (option b 'lsp-mode)) buffers)))
    (cond ((not (and (or ready-at old) (option #f 'lsp-autoreconnect) (pair? buffers)))
           (hash-table-delete! %reconnects key)
           #f)
          ((>= attempts (length lsp-reconnect-delays))
           (hash-table-set! %reconnects key (make-reconnect attempts buffers 'gave-up))
           'gave-up)
          (else
           (let ((entry (make-reconnect (+ attempts 1) buffers 'pending))
                 (delay (list-ref lsp-reconnect-delays attempts))
                 (server (session-server ls)) (root (session-root ls)))
             (hash-table-set! %reconnects key entry)
             (spawn (lambda ()
                      (sleep delay)
                      ;; Not if asked otherwise meanwhile (M-x lsp,
                      ;; lsp-restart, lsp-shutdown).
                      (when (eq? (hash-table-ref/default %reconnects key #f) entry)
                        (set-reconnect-state! entry 'started)
                        (let* ((found (hash-table-ref/default %sessions key #f))
                               (next (if (and found (memq (session-state found) '(starting ready))) found (start-session! server root))))
                          (for-each (lambda (b) (when (and (memq b (buffer-list)) (option b 'lsp-mode)) (track! next b)))
                                    buffers)))))
             delay)))))

;; Forget the reconnecting of buffer B's servers: asked for by hand.
(define (forget-reconnects! b)
  (for-each (lambda (kv) (when (memq b (reconnect-buffers (cdr kv))) (hash-table-delete! %reconnects (car kv))))
            (hash-table->alist %reconnects)))

;; How buffer B's servers' reconnecting is, for the mode line: pending,
;; gave-up, or #f.
(define (reconnect-state-of b)
  (let ((states (filter-map (lambda (r) (and (memq b (reconnect-buffers r)) (reconnect-state r)))
                            (hash-table-values %reconnects))))
    (cond ((memq 'pending states) 'pending)
          ((memq 'gave-up states) 'gave-up)
          (else #f))))

;; LS's log: its server's and root's, across its runs.
(define (session-log ls)
  (lsp-log (session-key (session-root ls) (session-server ls))
           (string-append (symbol->string (lsp-server-name (session-server ls))) " " (abbreviate-path (session-root ls)))))

;; LS is no more: out of the table (its slot, unless a successor took
;; it), what waits for it let go, its documents' followers ended.
(define (forget-session! ls)
  (guard (e (#t #f)) (channel-close (session-ready ls)))
  (when (eq? (hash-table-ref/default %sessions (session-key (session-root ls) (session-server ls)) #f) ls)
    (hash-table-delete! %sessions (session-key (session-root ls) (session-server ls))))
  (for-each forget-tracked! (hash-table-values (session-documents ls))))

;; End LS: a server that is up (WAS ready) is asked to shut down and
;; exit, and given a second to; then its process is killed.
(define (end-session! ls #:was [was (session-state ls)])
  (unless (eq? was 'dead)
    (set-session-state! ls 'stopping)
    (when (eq? was 'ready)
      (guard (e (#t #f))
        (lsp-request ls "shutdown" 'null #:timeout 2000)
        (lsp-notify ls "exit" 'null)
        (select (recv (session-gone ls) (v) #t) (timeout 1000 #f))))
    (rpc-close! (session-conn ls))
    (set-session-state! ls 'dead)
    (forget-session! ls)
    (say (string-append (symbol->string (lsp-server-name (session-server ls))) " shut down"))))

(define (await-ready ls)
  "Wait in this task for the session LS to be ready; return #f if it died."
  (channel-recv (session-ready ls))
  (eq? (session-state ls) 'ready))

;;; Documents: each followed by a task of its own while it is open.

(define-record-type tracked
  (%make-tracked document path language sent lock task diagnostics hints asking)
  tracked?
  (document tracked-document)
  (path tracked-path)
  (language tracked-language)
  ;; The revision the server has, #f before it is opened there.
  (sent tracked-sent set-tracked-sent!)
  ;; A channel holding one token: who sends to the server holds it, so
  ;; the follower and a request syncing first never send the same edit.
  (lock tracked-lock)
  (task tracked-task set-tracked-task!)
  ;; What the server last said is wrong: (revision diagnostic ...), each
  ;; diagnostic (from to severity message source raw), positions of that
  ;; revision's text, mapped to newer ones where they are shown.
  (diagnostics tracked-diagnostics set-tracked-diagnostics!)
  ;; Its inlay hints, a few windows of them (`ask-hints!`), and what is
  ;; being asked for.
  (hints tracked-hints set-tracked-hints!)
  (asking tracked-asking set-tracked-asking!))

(define (with-lock t thunk)
  (dynamic-wind (lambda () (channel-recv (tracked-lock t)))
                thunk
                (lambda () (channel-send (tracked-lock t) 'token))))

;; Under T's lock, as `with-lock`, THUNK given a procedure that lets the
;; lock go early, once: a request's frame queued, the edits after it may
;; follow it while it waits for its answer.
(define (with-lock-released-by t thunk)
  (let ((held #f))
    (define (release!) (when held (set! held #f) (channel-send (tracked-lock t) 'token)))
    (dynamic-wind (lambda () (channel-recv (tracked-lock t)) (set! held #t))
                  (lambda () (thunk release!))
                  release!)))

(define (lsp-text-document d)
  "Return the TextDocumentIdentifier of document D."
  (dict "uri" (file-uri (document-path d))))

;; The document's revision and text, read as one: an edit between the
;; two readings is read again.
(define (snapshot d)
  (let loop ()
    (let* ((revision (document-revision d)) (text (document-string d)))
      (if (= revision (document-revision d)) (cons revision text) (loop)))))

(define (open-tracked! ls t)
  (with-lock t
    (lambda ()
      (unless (tracked-sent t)
        (let ((now (snapshot (tracked-document t))))
          (when (open-close? ls)
            (lsp-notify ls "textDocument/didOpen"
                        (dict "textDocument" (dict "uri" (file-uri (tracked-path t)) "languageId" (tracked-language t)
                                                   "version" (car now) "text" (cdr now)))))
          (set-tracked-sent! t (car now))))))
  ;; A server that takes no changes has nothing to follow.
  (unless (or (tracked-task t) (= (sync-kind ls) 0))
    (set-tracked-task! t (spawn (lambda () (follow! ls t))))))

;; Bring the server to the document as it is now. A request at a
;; position syncs first: the position is in the text as it is.
(define (sync-tracked! ls t)
  (with-lock t (lambda () (sync-now! ls t))))

;; As `sync-tracked!`, its lock held by the caller.
(define (sync-now! ls t)
      (let* ((d (tracked-document t)) (sent (tracked-sent t)) (kind (sync-kind ls)))
        (when (and sent (not (= sent (document-revision d))) (> kind 0))
          (let* ((changes (let loop ()
                            (let* ((revision (document-revision d))
                                   (changes (and (= kind 2) (document-lsp-changes d sent (lsp-session-encoding ls)))))
                              (if (= revision (document-revision d)) (cons revision changes) (loop)))))
                 (revision (car changes))
                 (content (if (cdr changes)
                              (map (lambda (c)
                                     (dict "range" (dict "start" (dict "line" (car c) "character" (cadr c))
                                                         "end" (dict "line" (caddr c) "character" (cadddr c)))
                                           "text" (list-ref c 4)))
                                   (cdr changes))
                              (let ((now (snapshot d)))
                                (set! revision (car now))
                                (list (dict "text" (cdr now)))))))
            (lsp-notify ls "textDocument/didChange"
                        (dict "textDocument" (dict "uri" (file-uri (tracked-path t)) "version" revision)
                              "contentChanges" content))
            (set-tracked-sent! t revision)))))

;; The follower: wakes at each edit, sends what changed. Ends when the
;; document goes or the session does.
(define (follow! ls t)
  (let loop ()
    (let ((now (document-await-change (tracked-document t) (tracked-sent t))))
      (when (and now (eq? (session-state ls) 'ready) (eq? (hash-table-ref/default (session-documents ls) (tracked-path t) #f) t))
        (guard (e (#t #f)) (sync-tracked! ls t))
        (loop)))))

(define (forget-tracked! t)
  (let ((task (tracked-task t)))
    (set-tracked-task! t #f)
    (set-tracked-diagnostics! t '())
    (set-tracked-hints! t '())
    (set-tracked-asking! t '())
    (when (and task (not (task-done? task))) (task-cancel task))))

(define (close-tracked! ls t)
  (hash-table-delete! (session-documents ls) (tracked-path t))
  (forget-tracked! t)
  ;; Under the lock: after whatever was being sent of it, never before.
  (when (and (eq? (session-state ls) 'ready) (tracked-sent t) (open-close? ls))
    (with-lock t
      (lambda ()
        (guard (e (#t #f))
          (lsp-notify ls "textDocument/didClose" (dict "textDocument" (dict "uri" (file-uri (tracked-path t))))))))))

;; The languageId of a buffer: its syntax language's name, in LSP's own
;; words where they differ.
(define (language-id b)
  (let ((l (buffer-language b)))
    (case l
      ((bash) "shellscript")
      ((tsx) "typescriptreact")
      ((#f) (let ((m (symbol->string (buffer-mode b))))
              (if (string-suffix? "-mode" m) (substring m 0 (- (string-length m) 5)) m)))
      (else (symbol->string l)))))

;;; Attaching

;; Have session LS follow buffer B's document, opened there now if it is
;; ready, else once it is.
(define (track! ls b)
  (let* ((d (buffer-document b)) (path (document-path d)))
    (unless (hash-table-contains? (session-documents ls) path)
      (let ((t (%make-tracked d path (language-id b) #f (make-channel 1) #f '() '() '())))
        (channel-send (tracked-lock t) 'token)
        (hash-table-set! (session-documents ls) path t)
        (when (eq? (session-state ls) 'ready) (open-tracked! ls t))))))

(define (lsp-attach! b #:quiet [quiet #f])
  "Attach buffer B to the language servers serving it.
Return their sessions. A server starts where none runs for the buffer's
project. Unless QUIET, a buffer no installed server serves is an error
saying so; QUIET passes it over, as opening a file does, and leaves out
servers whose project files (#:roots) are not there."
  (let* ((d (buffer-document b)) (path (document-path d)))
    (cond ((not path) (if quiet '() (error "lsp: the buffer has no file")))
          ((remote-path? path) (if quiet '() (error "lsp: not for files on other machines yet")))
          (else
           (let* ((servers (servers-for b))
                  (runnable (filter (lambda (s) (and (installed? (car (lsp-server-command s)))
                                                     (or (not quiet) (cdr (server-root s path)))))
                                    servers)))
             (when (and (not quiet) (null? runnable))
               (error (if (null? servers)
                          "No language server serves this buffer"
                          (string-append "Not installed: "
                                         (string-join (map (lambda (s) (car (lsp-server-command s))) servers) ", ")))))
             (begin
               (unless (null? runnable) (set-option! 'lsp-mode #t #:buffer b))
               (map (lambda (server)
                      (let* ((root (car (server-root server path)))
                             (found (hash-table-ref/default %sessions (session-key root server) #f))
                             (ls (if (and found (memq (session-state found) '(starting ready))) found (start-session! server root))))
                        (track! ls b)
                        ls))
                    runnable)))))))

(define (lsp-buffer-sessions b)
  "Return the live sessions buffer B's document is open in."
  (let ((path (document-path (buffer-document b))))
    (filter (lambda (ls) (and (not (eq? (session-state ls) 'dead)) (hash-table-contains? (session-documents ls) path)))
            (hash-table-values %sessions))))

(define (tracked-in ls d) (hash-table-ref/default (session-documents ls) (document-path d) #f))

(define (lsp-sync! ls d)
  "Wait in this task for session LS to be ready, with document D as it is.
Return #f if it died first."
  (and (await-ready ls)
       (let ((t (tracked-in ls d)))
         (and t (begin (unless (tracked-sent t) (open-tracked! ls t)) (sync-tracked! ls t) #t)))))

(define (lsp-document-request ls d method make #:timeout [patience rpc-timeout] #:on-error [on-error #f])
  "Ask METHOD of session LS's server about document D as it is.
MAKE is called with the revision the server has, with nothing sent of D
meanwhile, and gives the params (positions in that revision's text). Its
frame is queued before any edit after it is sent. Return (revision .
answer), or #f if the session died first or D is not open in it.
ON-ERROR is as `rpc-request!` takes it."
  (and (await-ready ls)
       (let ((t (tracked-in ls d)))
         (and t
              (begin
                (unless (tracked-sent t) (open-tracked! ls t))
                (sync-tracked! ls t)
                (with-lock-released-by t
                  (lambda (release!)
                    ;; Brought up to date again under the lock: an edit
                    ;; between the sync and the lock is sent first.
                    (sync-now! ls t)
                    (let* ((revision (tracked-sent t)) (params (make revision)))
                      (cons revision (rpc-request! (session-conn ls) method params #:timeout patience #:sent release!
                                                   #:on-error on-error))))))))))

(define (lsp-position ls d pos)
  "Return position POS of document D as session LS's server counts it."
  (let ((at (document-lsp-position d pos (lsp-session-encoding ls))))
    (dict "line" (car at) "character" (cadr at))))

(define (lsp-text-document-position ls d pos)
  (dict "textDocument" (dict "uri" (file-uri (document-path d))) "position" (lsp-position ls d pos)))

(define (point-of p) (list (dict-ref p "line" 0) (dict-ref p "character" 0)))

(define (lsp-range-positions ls d range #:revision [revision (document-revision d)])
  "Return RANGE of document D as of REVISION as (from to), mapped to now.
RANGE is an LSP range as session LS's server counts; #f when that
revision's text is no longer known."
  (let ((both (document-lsp-offsets d revision (lsp-session-encoding ls)
                                    (list (point-of (dict-ref range "start")) (point-of (dict-ref range "end"))))))
    (and both
         (let ((from (document-map-position d (car both) revision)) (to (document-map-position d (cadr both) revision)))
           (and from to (list from to))))))

(define (lsp-location ls loc)
  "Return the `location' an LSP Location or LocationLink LOC names, or #f.
Its file is opened (as a document) to read the position in, counted as
session LS's server counts."
  (let* ((uri (dict-ref loc "targetUri" (dict-ref loc "uri" #f)))
         (range (dict-ref loc "targetSelectionRange" (dict-ref loc "range" #f)))
         (path (uri-file-path uri)))
    (and path range (file-exists? path)
         (let* ((d (file-document path))
                (at (document-lsp-offsets d (document-revision d) (lsp-session-encoding ls) (list (point-of (dict-ref range "start"))))))
           (and at (location d (car at)))))))

;;; Diagnostics

(define (severity-face n)
  (case n ((2) 'diagnostic.warning) ((3) 'diagnostic.info) ((4) 'diagnostic.hint) (else 'diagnostic.error)))

(define (note-diagnostics! ls params)
  (let* ((path (uri-file-path (dict-ref params "uri" "")))
         (t (and path (hash-table-ref/default (session-documents ls) path #f))))
    (when (and t (tracked-sent t))
      (let* ((d (tracked-document t))
             (version (dict-ref params "version" 'null))
             ;; Of the version it names, else of what it was last sent.
             (revision (if (number? version) version (tracked-sent t)))
             (found (vector->list (dict-ref params "diagnostics" #())))
             (points (append-map (lambda (x) (let ((r (dict-ref x "range"))) (list (point-of (dict-ref r "start")) (point-of (dict-ref r "end")))))
                                 found))
             (offsets (document-lsp-offsets d revision (lsp-session-encoding ls) points)))
        (when offsets
          (set-tracked-diagnostics!
           t (cons revision
                   (let loop ((found found) (offsets offsets) (out '()))
                     (if (null? found)
                         (reverse out)
                         (let* ((x (car found)) (source (dict-ref x "source" #f)) (code (dict-ref x "code" #f)))
                           (loop (cdr found) (cddr offsets)
                                 (cons (list (car offsets) (cadr offsets) (dict-ref x "severity" 1)
                                             (dict-ref x "message" "")
                                             (string-append (if (string? source) source (symbol->string (lsp-server-name (session-server ls))))
                                                            (cond ((string? code) (string-append " " code))
                                                                  ((number? code) (string-append " " (number->string code)))
                                                                  (else "")))
                                             ;; As the server said it, for
                                             ;; the code actions on it.
                                             x)
                                       out)))))))
          ;; Mapped to the text now at once, while that is cheap, and
          ;; again as it changes.
          (current-diagnostics t))))))

;; T's diagnostics in its document's text now: mapped from the revision
;; they were stored at, and stored again at this one.
(define (current-diagnostics t)
  (let* ((stored (tracked-diagnostics t)) (d (tracked-document t)) (now (document-revision d)))
    (cond ((null? stored) '())
          ((= (car stored) now) (cdr stored))
          (else
           (let ((moved (filter-map (lambda (x)
                                      (let ((from (document-map-position d (car x) (car stored)))
                                            (to (document-map-position d (cadr x) (car stored))))
                                        (and from to (cons from (cons to (cddr x))))))
                                    (cdr stored))))
             ;; Stored as of NOW only if every endpoint was mapped to
             ;; it: an edit landing partway (this may be preempted) would
             ;; leave some of another revision's under its name.
             (if (= now (document-revision d))
                 (begin (set-tracked-diagnostics! t (cons now moved)) moved)
                 (current-diagnostics t)))))))

;; Every session's diagnostics of document D now, by position.
(define (document-diagnostics d)
  (let ((path (document-path d)))
    (if (not path)
        '()
        (sort (append-map (lambda (ls) (let ((t (hash-table-ref/default (session-documents ls) path #f)))
                                         (if t (current-diagnostics t) '())))
                          (hash-table-values %sessions))
              (lambda (a b) (< (car a) (car b)))))))

(define (lsp-diagnostics-at d pos)
  "Return the diagnostics of document D around POS.
Each is (from to severity message source raw), positions as of now,
RAW the diagnostic as the server said it."
  (filter (lambda (x) (and (<= (car x) pos) (or (< pos (cadr x)) (= (car x) (cadr x) pos)))) (document-diagnostics d)))

(define (diagnostic-layer d from to)
  (filter-map (lambda (x)
                (let* ((a (car x))
                       ;; One that covers nothing (a missing semicolon)
                       ;; is drawn under the character there.
                       (b (if (< a (cadr x)) (cadr x) (min (document-length d) (next-grapheme d a)))))
                  (and (< a b) (< a to) (> b from) (list a b (severity-face (caddr x))))))
              (document-diagnostics d)))

(define-minor-mode lsp-mode
  "A buffer served by language servers: their diagnostics shown in it.
Turned on as it attaches (\\[lsp])."
  #:keys '(("M-g n" lsp-next-diagnostic) ("M-g M-n" lsp-next-diagnostic)
           ("M-g p" lsp-previous-diagnostic) ("M-g M-p" lsp-previous-diagnostic)
           ("M-g f" lsp-diagnostics) ("C-h ." eldoc-doc-buffer)
           ;; Doom's code prefix, C-c c.
           ("C-c c r" lsp-rename) ("C-c c f" lsp-format) ("C-c c a" lsp-code-actions)
           ("C-c c x" lsp-diagnostics) ("C-c c k" eldoc-doc-buffer) ("C-c c l" lsp-events))
  #:normal '(("] e" lsp-next-diagnostic) ("[ e" lsp-previous-diagnostic) ("SPC c x" lsp-diagnostics)
             ("K" eldoc-doc-buffer) ("SPC c k" eldoc-doc-buffer)
             ("SPC c r" lsp-rename) ("SPC c f" lsp-format) ("SPC c a" lsp-code-actions)
             ("SPC c l" lsp-events))
  #:layer (lambda (d from to) (diagnostic-layer d from to))
  #:inlays (lambda (d from to) (document-inlays d from to)))

;; The mode line: the servers' work in progress, else the diagnostics'
;; count by severity, as Flymake's lighter.
(register-mode-line-segment! 'lsp
  (lambda (b)
    (and (option b 'lsp-mode)
         (let ((sessions (lsp-buffer-sessions b)))
           (if (null? sessions)
               (case (reconnect-state-of b)
                 ((pending) "lsp:reconnecting")
                 ((gave-up) "lsp:offline")
                 (else #f))
                (let ((busy (append-map (lambda (ls) (map cadr (hash-table-values (session-progress ls)))) sessions)))
                  (cond ((any (lambda (ls) (eq? (session-state ls) 'starting)) sessions) "lsp:starting")
                        ((pair? busy) (string-append "lsp:" (car busy)))
                        (else
                         (let* ((found (document-diagnostics (buffer-document b)))
                                (count (lambda (n) (length (filter (lambda (x) (= (caddr x) n)) found)))))
                           (string-append "lsp:" (number->string (count 1)) "E " (number->string (count 2)) "W"))))))))))

;;; Inlay hints: asked for the part of a document shown, as it is drawn
;;; and after a pause in its edits; kept with the revision they were
;;; computed for and mapped through the edits since, as diagnostics are.

(define lsp-inlay-hint-delay 200
  "The milliseconds a document holds still before its inlay hints are asked for.")

(define (hint-text h)
  (let ((label (dict-ref h "label" "")))
    (string-append (if (eq? (dict-ref h "paddingLeft" #f) #t) " " "")
                   (if (string? label) label (apply string-append (map (lambda (part) (dict-ref part "value" "")) (as-list label))))
                   (if (eq? (dict-ref h "paddingRight" #f) #t) " " ""))))

;; T's hints are kept as a few windows, the latest first, each (revision
;; from to (position text kind) ...): panes showing different parts of a
;; document each keep theirs. What is being asked is a list of (revision
;; from to), each answer stored only if it is still asked for.
(define hint-windows 4)

;; Ask LS for T's hints from FROM to TO, unless a window has them for
;; this revision or they are being asked for: in a task, after the
;; document holds still.
(define (ask-hints! ls t from to)
  (let* ((d (tracked-document t)) (now (document-revision d)) (here (list now from to)))
    (unless (or (member here (tracked-asking t))
                (any (lambda (w) (and (= (car w) now) (<= (cadr w) from) (>= (caddr w) to))) (tracked-hints t)))
      (set-tracked-asking! t (cons here (tracked-asking t)))
      (spawn (lambda ()
               (dynamic-wind
                 (lambda () #f)
                 (lambda ()
                   (sleep lsp-inlay-hint-delay)
                   (when (= (document-revision d) now)
                     (let ((found (guard (e (#t #f))
                                    (lsp-document-request ls d "textDocument/inlayHint"
                                                          (lambda (revision)
                                                            (dict "textDocument" (lsp-text-document d)
                                                                  "range" (dict "start" (lsp-position ls d from) "end" (lsp-position ls d to))))))))
                       ;; Stored only while it is still asked for: a
                       ;; refresh, or the session going, drops it.
                       (when (and found (member here (tracked-asking t)))
                         (let* ((revision (car found)) (hints (as-list (cdr found)))
                                (at (document-lsp-offsets d revision (lsp-session-encoding ls) (map (lambda (h) (point-of (dict-ref h "position"))) hints))))
                           (when at
                             (let ((window (cons revision (cons from (cons to (map (lambda (h p) (list p (hint-text h) (dict-ref h "kind" 0))) hints at))))))
                               (set-tracked-hints! t (cons window
                                                           (let keep ((ws (tracked-hints t)) (n 1))
                                                             (cond ((or (null? ws) (>= n hint-windows)) '())
                                                                   ;; One it covers is replaced by it.
                                                                   ((and (<= from (cadr (car ws))) (>= to (caddr (car ws))))
                                                                    (keep (cdr ws) n))
                                                                   (else (cons (car ws) (keep (cdr ws) (+ n 1)))))))))))))))
                 (lambda () (set-tracked-asking! t (remove (lambda (a) (equal? a here)) (tracked-asking t))))))))))

;; T's hints from FROM to TO in the text now: from the latest window
;; overlapping them, mapped through the edits since its revision.
(define (current-hints t from to)
  (let* ((d (tracked-document t)) (now (document-revision d))
         (window (find (lambda (w) (and (< (cadr w) to) (> (caddr w) from))) (tracked-hints t))))
    (if (not window)
        '()
        (filter-map (lambda (h)
                      (let ((p (if (= (car window) now) (car h) (document-map-position d (car h) (car window)))))
                        (and p (<= from p to)
                             (list p (cadr h) (case (caddr h) ((1) 'lsp-inlay-hint.type) ((2) 'lsp-inlay-hint.parameter) (else 'lsp-inlay-hint))))))
                    (cdddr window)))))

(define (document-inlays d from to)
  (let ((b (document-buffer d)))
    (if (not (and b (option b 'lsp-inlay-hints) (document-path d)))
        '()
        (append-map (lambda (ls)
                      (let ((t (tracked-in ls d)))
                        (if (and t (eq? (session-state ls) 'ready) (lsp-session-capability ls "inlayHintProvider"))
                            (begin (ask-hints! ls t from to) (current-hints t from to))
                            '())))
                    (lsp-buffer-sessions b)))))

;;; Commands

(define-command (lsp s n)
  "Attach the focused buffer to the language servers serving it.
Each starts in the project's root unless it runs there already; its
diagnostics show under `lsp-mode'."
  (let ((b (or (current-buffer s) (error "No buffer"))))
    (forget-reconnects! b)
    (when (null? (lsp-attach! b)) (message! s "No language server serves this buffer"))))

(define-command (lsp-events s n)
  "Show what went between the editor and a language server of this buffer.
As Eglot's events buffer: the messages each way, the JSON as it went,
and what the server wrote to stderr, followed as more come. With more
than one server (or none here, but others), which is asked."
  (let* ((b (current-buffer s))
         (here (if b (map session-log (lsp-buffer-sessions b)) '()))
         (live (map session-log (hash-table-values %sessions)))
         ;; With no server left running, the logs of those that ran.
         (logs (cond ((pair? here) here) ((pair? live) live) (else (lsp-logs)))))
    (cond ((null? logs) (error "No language server's log"))
          ((null? (cdr logs)) (show-lsp-log! s (car logs)))
          (else (completing-read s "Log of: " (map lsp-log-name logs)
                                 #:accept (lambda (s c)
                                            (let ((log (find (lambda (l) (equal? (lsp-log-name l) (candidate-text c))) logs)))
                                              (when log (show-lsp-log! s log)))))))))

(define-command (lsp-shutdown s n)
  "Shut down the language servers of the focused buffer.
Their buffers leave `lsp-mode'."
  (let ((b (or (current-buffer s) (error "No buffer"))))
    (forget-reconnects! b)
    (set-option! 'lsp-mode #f #:buffer b)
    (for-each (lambda (ls)
                (for-each (lambda (t) (let ((tb (document-buffer (tracked-document t))))
                                        (when tb (set-option! 'lsp-mode #f #:buffer tb))))
                          (hash-table-values (session-documents ls)))
                (stop-session! ls))
              (lsp-buffer-sessions b))))

;; End LS in a task of its own, marked stopping now: its process ending
;; meanwhile is not a crash to reconnect after.
(define (stop-session! ls)
  (let ((was (session-state ls)))
    (unless (memq was '(stopping dead))
      (set-session-state! ls 'stopping)
      (spawn (lambda () (end-session! ls #:was was))))))

(define-command (lsp-restart s n)
  "Restart the language servers of the focused buffer.
Each is shut down and started again, with every buffer it served. Where
they ended (crashed), they are started again for this buffer."
  (let ((b (or (current-buffer s) (error "No buffer"))))
    (forget-reconnects! b)
    (when (null? (lsp-buffer-sessions b))
      (lsp-attach! b))
    (for-each (lambda (ls)
                (let ((buffers (filter-map (lambda (t) (document-buffer (tracked-document t))) (hash-table-values (session-documents ls))))
                      (next (begin (hash-table-delete! %sessions (session-key (session-root ls) (session-server ls)))
                                   (start-session! (session-server ls) (session-root ls)))))
                  (stop-session! ls)
                  (for-each (lambda (b) (track! next b)) buffers)))
              (lsp-buffer-sessions b))))

(define (goto-diagnostic! s forward)
  (let* ((p (point s))
         (found (document-diagnostics (doc s)))
         (next (if forward
                   (find (lambda (x) (> (car x) p)) found)
                   (find (lambda (x) (< (car x) p)) (reverse found)))))
    (if next
        (begin (move! s (lambda (h) (car next)))
               (message! s (cadddr next)))
        (message! s (if forward "No diagnostic after point" "No diagnostic before point")))))

(define-command (lsp-next-diagnostic s n)
  "Go to the next diagnostic of the buffer, and say what it is."
  (goto-diagnostic! s #t))

(define-command (lsp-previous-diagnostic s n)
  "Go to the previous diagnostic of the buffer, and say what it is."
  (goto-diagnostic! s #f))

(define-command (lsp-diagnostics s n)
  "Go to a diagnostic of the buffer, previewing each while choosing.
As `consult-flymake': each says its severity, line and message."
  (let* ((d (doc s)) (revision (document-revision d)))
    (when (null? (document-diagnostics d)) (error "No diagnostics"))
    (completing-read s "Diagnostic: "
                     (map (lambda (x)
                            (candidate (string-append (case (caddr x) ((1) "error") ((2) "warning") ((3) "info") (else "hint"))
                                                      ": " (car (string-split (cadddr x) "\n")))
                                       #:annotation (string-append (list-ref x 4) ":" (number->string (line-number d (car x))))
                                       #:target (target 'location (location d (car x) #:revision revision))))
                          (document-diagnostics d))
                     #:preview (lambda (s c) (act-default! s (candidate-target c))))))

;;; Asking: the answers that take the editor somewhere or change the text.

;; The sessions of buffer B whose servers say they do CAPABILITY.
(define (sessions-with b capability)
  (filter (lambda (ls) (or (eq? (session-state ls) 'starting) (lsp-session-capability ls capability)))
          (lsp-buffer-sessions b)))

;; Ask the first of the sessions of D's buffer doing CAPABILITY, with the
;; params MAKE gives (session) once it has D as it is, queued before any
;; edit after it; return (session revision . answer), REVISION the one of
;; D the server answered of, or #f where none answers.
(define (ask-first d capability method make)
  (let ((b (document-buffer d)))
    (let loop ((sessions (if b (sessions-with b capability) '())))
      (and (pair? sessions)
           (let* ((ls (car sessions))
                  (answer (and (await-ready ls) (lsp-session-capability ls capability)
                               (lsp-document-request ls d method (lambda (revision) (make ls))))))
             (if (and answer (not (eq? (cdr answer) 'null))) (cons ls answer) (loop (cdr sessions))))))))

;; What an answer of `ask-first` holds: its session, the revision it was
;; of, and the answer.
(define (found-session found) (car found))
(define (found-revision found) (cadr found))
(define (found-answer found) (cddr found))

(define (as-list v) (cond ((vector? v) (vector->list v)) ((dict? v) (list v)) ((pair? v) v) (else '())))

;; Locations an answer names, as xref items.
(define (location-items ls answer)
  (filter-map (lambda (loc) (let ((l (lsp-location ls loc))) (and l (xref-item l)))) (as-list answer)))

(define symbol-kinds
  '#("" "file" "module" "namespace" "package" "class" "method" "property" "field" "constructor" "enum" "interface"
     "function" "variable" "constant" "string" "number" "boolean" "array" "object" "key" "null" "enum member"
     "struct" "event" "operator" "type parameter"))

(define (xref-find s kind d pos pattern)
  (case kind
    ((symbols)
     (let ((found (ask-first d "workspaceSymbolProvider" "workspace/symbol" (lambda (ls) (dict "query" pattern)))))
       (and found
            (filter-map (lambda (sym)
                          (let ((l (lsp-location (found-session found) (dict-ref sym "location" (dict)))))
                            (and l (xref-item l #:summary (string-append (dict-ref sym "name" "") "  "
                                                                         (let ((k (dict-ref sym "kind" 0)))
                                                                           (if (< 0 k (vector-length symbol-kinds)) (vector-ref symbol-kinds k) "")))))))
                        (as-list (found-answer found))))))
    (else
     (let* ((what (case kind
                    ((definitions) '("definitionProvider" . "textDocument/definition"))
                    ((references) '("referencesProvider" . "textDocument/references"))
                    ((implementations) '("implementationProvider" . "textDocument/implementation"))
                    (else '("typeDefinitionProvider" . "textDocument/typeDefinition"))))
            (found (ask-first d (car what) (cdr what)
                              (lambda (ls)
                                (let ((at (lsp-text-document-position ls d pos)))
                                  (if (eq? kind 'references) (dict-set at "context" (dict "includeDeclaration" #t)) at))))))
       (and found (location-items (found-session found) (found-answer found)))))))

(define-xref-backend lsp
  "What the buffer's language servers know: definitions, references,
implementations, types, and the project's names."
  #:applies (lambda (s b) (and (option b 'lsp-mode) (pair? (lsp-buffer-sessions b))))
  #:find xref-find #:wait #t #:priority 10)

;;; Eldoc: the diagnostics at point, then what a call takes, then the
;;; name's hover.

;; Markup as markdown: MarkupContent, MarkedString, or a list of them.
;; With FENCES #f, code fences are dropped, their code kept: text for the
;; echo area. Else it is markdown to render (*eldoc*): a MarkedString's
;; code fenced in its language, plain text escaped to read as it is.
(define (markup-text v #:fences [fences #f])
  (cond ((string? v) v)
        ((dict? v) (let ((value (dict-ref v "value" "")))
                     (cond ((and fences (equal? (dict-ref v "kind" #f) "plaintext")) (markdown-escape value))
                           ((not (dict-contains? v "language")) (if fences value (strip-fences value)))
                           (fences (string-append "```" (dict-ref v "language" "") "\n" value "\n```"))
                           (else value))))
        ((or (vector? v) (pair? v)) (string-join (map (lambda (x) (markup-text x #:fences fences)) (as-list v)) "\n\n"))
        (else "")))

(define (strip-fences text)
  (string-join (filter (lambda (l) (not (string-prefix? "```" (string-trim l)))) (string-split text "\n")) "\n"))

(define-eldoc-provider lsp-diagnostics
  "The diagnostics at point, as Flymake's."
  #:applies (lambda (b) (option b 'lsp-mode))
  #:say (lambda (s d pos full)
          (let ((found (lsp-diagnostics-at d pos)))
            (and (pair? found)
                 (string-join (map (lambda (x) (string-append (cadddr x) " [" (list-ref x 4) "]")) found) "\n"))))
  #:priority 20)

;; The character index of TEXT at UTF-16 unit U (clamped to its end).
(define (utf16-index text u)
  (let loop ((cs (string->list text)) (units 0) (i 0))
    (if (or (null? cs) (>= units u))
        i
        (loop (cdr cs) (+ units (if (> (char->integer (car cs)) #xFFFF) 2 1)) (+ i 1)))))

(define (utf16-length text) (fold (lambda (c n) (+ n (if (> (char->integer c) #xFFFF) 2 1))) 0 (string->list text)))

(define-eldoc-provider lsp-signature
  "The signature of the call point is in, its argument there marked."
  #:applies (lambda (b) (option b 'lsp-mode))
  #:say (lambda (s d pos full)
          (and (not full)
               (let* ((found (ask-first d "signatureHelpProvider" "textDocument/signatureHelp"
                                        (lambda (ls) (lsp-text-document-position ls d pos))))
                      (help (and found (found-answer found)))
                      (signatures (and help (as-list (dict-ref help "signatures" #())))))
                 (and (pair? signatures)
                      (let* ((n (dict-ref help "activeSignature" 0))
                             (sig (list-ref signatures (if (and (integer? n) (< -1 n (length signatures))) n 0)))
                             (label (dict-ref sig "label" ""))
                             (params (as-list (dict-ref sig "parameters" #())))
                             (k (dict-ref sig "activeParameter" (dict-ref help "activeParameter" #f)))
                             (param (and (integer? k) (< -1 k (length params)) (dict-ref (list-ref params k) "label" #f))))
                        ;; The argument at point in brackets, as Eldoc bolds
                        ;; it. Its offsets count UTF-16 units, whatever
                        ;; the positions' encoding.
                        (let ((span (cond ((and (pair? param) (= (length param) 2)) param)
                                          ((and (vector? param) (= (vector-length param) 2)) (vector->list param))
                                          ;; A parameter given by its text: where it is.
                                          ((string? param)
                                           (let ((at (string-contains label param)))
                                             (and at (list (utf16-length (substring label 0 at))
                                                           (utf16-length (substring label 0 (+ at (string-length param))))))))
                                          (else #f))))
                          (if (not span)
                              label
                              (let ((a (utf16-index label (car span))) (b (utf16-index label (cadr span))))
                                (string-append (substring label 0 a) "[" (substring label a b) "]"
                                               (substring label b (string-length label)))))))))))
  #:priority 10)

(define-eldoc-provider lsp-hover
  "What the language server says of the name at point: its type,
signature and documentation."
  #:applies (lambda (b) (option b 'lsp-mode))
  #:say (lambda (s d pos full)
          (let ((found (ask-first d "hoverProvider" "textDocument/hover" (lambda (ls) (lsp-text-document-position ls d pos)))))
            (and found
                 (let* ((contents (dict-ref (found-answer found) "contents" ""))
                        (text (markup-text contents)))
                   (and (not (string=? (string-trim text) ""))
                        ;; The echo area: the signature, not the
                        ;; documentation under the rule. *eldoc*: all of
                        ;; it, as markdown, rendered.
                        (if full (markup-text contents #:fences #t) (car (string-split text "\n---"))))))))
  #:markdown #t)

;;; Edits a server sends: a workspace edit, a document's text edits.

;; TEXT-EDITS of document D, their positions those of D's text at
;; REVISION (as LS counts them), as offsets of that text: (from to text),
;; in order, two inserted at one place as the server gave them.
(define (text-edit-offsets ls d revision text-edits)
  (let* ((points (append-map (lambda (e) (let ((r (dict-ref e "range")))
                                           (list (point-of (dict-ref r "start")) (point-of (dict-ref r "end")))))
                             text-edits))
         (offsets (or (document-lsp-offsets d revision (lsp-session-encoding ls) points)
                      (error "lsp: the text the edits were made against is gone"))))
    (let loop ((es text-edits) (os offsets) (out '()))
      (if (null? es)
          (sort (reverse out) (lambda (a b) (< (car a) (car b))))
          (loop (cdr es) (cddr os) (cons (list (car os) (cadr os) (dict-ref (car es) "newText" "")) out))))))

;; The view a server's edits of D are made through: the user's through
;; their view of its buffer, their own undo; another principal's (an
;; agent's) through a view of its own.
(define (edit-view d)
  (let ((b (document-buffer d)) (principal (current-principal)))
    (if (and b (equal? principal "user"))
        (or (buffer-view b) (make-view d principal))
        (make-view d principal))))

;; Apply TEXT-EDITS to document D, of its text at REVISION, as the user's
;; one undoable change.
(define (apply-text-edits! ls d revision text-edits)
  (unless (null? text-edits)
    (let ((v (edit-view d)))
      (check-editable! v)
      (view-edit-at! v revision (text-edit-offsets ls d revision text-edits) "new"))))

;; EDIT, a WorkspaceEdit, as its changes in order: each (index uri
;; version text-edits), INDEX its place (in documentChanges, for the
;; server's failedChange), VERSION a number or null; or (index #f kind)
;; for creating, renaming or deleting a file.
(define (workspace-changes edit)
  (let ((document-changes (as-list (dict-ref edit "documentChanges" #())))
        (changes (let ((c (dict-ref edit "changes" #f))) (if (dict? c) (dict->alist c) '()))))
    (if (pair? document-changes)
        (let loop ((cs document-changes) (i 0) (out '()))
          (if (null? cs)
              (reverse out)
              (let ((c (car cs)))
                (loop (cdr cs) (+ i 1)
                      (cons (if (dict-contains? c "textDocument")
                                (list i (dict-ref (dict-ref c "textDocument") "uri")
                                      (dict-ref (dict-ref c "textDocument") "version" 'null) (as-list (dict-ref c "edits" #())))
                                (list i #f (dict-ref c "kind" "?")))
                            out)))))
        (let loop ((cs changes) (i 0) (out '()))
          (if (null? cs)
              (reverse out)
              (loop (cdr cs) (+ i 1) (cons (list i (car (car cs)) 'null (as-list (cdr (car cs)))) out)))))))

;; A file's part of a planned workspace edit: the first change naming
;; it, its document, the view to edit it through, the revision its edits
;; are of, and the edits, offsets of that revision's text.
(define-record-type planned
  (make-planned index path document view revision edits)
  planned?
  (index planned-index)
  (path planned-path)
  (document planned-document)
  (view planned-view)
  (revision planned-revision)
  (edits planned-edits set-planned-edits!))

;; Plan EDIT from session LS's server, changing nothing: two values, the
;; plan (a planned per file) and what is wrong with it, each (index
;; why), every problem found, not only the first. A change naming no
;; version is of the text the server had: for a file in REVISIONS
;; (path . revision), taken when it was asked, that revision's; else
;; the one last sent it; else, the file being none of the server's, the
;; file as it is — refused if the buffer has changes it has not seen.
(define (plan-workspace-edit ls edit revisions)
  (let loop ((changes (workspace-changes edit)) (plan '()) (problems '()))
    (if (null? changes)
        (values (reverse plan) (reverse problems))
        (let* ((change (car changes)) (index (car change)))
          (guard (e (#t (loop (cdr changes) plan (cons (list index (error-text e)) problems))))
            (unless (cadr change)
              (error (string-append "lsp: " (let ((kind (caddr change)))
                                              (cond ((equal? kind "create") "creating")
                                                    ((equal? kind "rename") "renaming")
                                                    ((equal? kind "delete") "deleting")
                                                    (else "changing")))
                                    " files is not supported")))
            (let* ((path (or (uri-file-path (cadr change)) (error "lsp: not a file:" (cadr change))))
                   (open (find (lambda (b) (equal? (document-path (buffer-document b)) path)) (buffer-list)))
                   (_ (unless (or open (remote-path? path) (file-exists? path)) (error "lsp: no such file:" path)))
                   (d (file-document path))
                   (version (caddr change))
                   (t (tracked-in ls d))
                   (revision (cond ((number? version) version)
                                   ((assoc path revisions) => cdr)
                                   ((and t (tracked-sent t)))
                                   ((document-dirty? d)
                                    (error "lsp: the server edited the saved file, not the buffer's changes:" (abbreviate-path path)))
                                   (else (document-revision d))))
                   (b (document-buffer d))
                   (edits (text-edit-offsets ls d revision (cadddr change)))
                   (same (find (lambda (p) (equal? (planned-path p) path)) plan)))
              (when (and b (option b 'read-only)) (error "lsp: read-only:" (abbreviate-path path)))
              (cond ((null? edits) (loop (cdr changes) plan problems))
                    (same
                     (unless (= (planned-revision same) revision)
                       (error "lsp: two versions of one file:" (abbreviate-path path)))
                     (set-planned-edits! same (sort (append (planned-edits same) edits) (lambda (a b) (< (car a) (car b)))))
                     (view-rebase-at (planned-view same) revision (planned-edits same))
                     (loop (cdr changes) plan problems))
                    (else
                     (let ((v (edit-view d)))
                       ;; Refused now as it would be when made: edits
                       ;; overlapping, or text changed under them since.
                       (view-rebase-at v revision edits)
                       (loop (cdr changes) (cons (make-planned index path d v revision edits) plan) problems))))))))))

;; Apply EDIT, all or none: (ok . files changed), or (failed index why),
;; INDEX the change that failed.
(define (apply-workspace-edit ls edit revisions)
  (let-values (((plan problems) (plan-workspace-edit ls edit revisions)))
    (if (pair? problems)
        (begin
          (for-each (lambda (p) (log-message! (string-append "lsp: not applied: " (cadr p)))) problems)
          (cons 'failed (car problems)))
        (guard (e (#t (let* ((why (error-text e))
                             ;; "views-edit-at!: K: why": the K-th planned.
                             (rest (and (string-prefix? "views-edit-at!: " why) (substring why 16 (string-length why))))
                             (colon (and rest (string-contains rest ": ")))
                             (k (and colon (string->number (substring rest 0 colon))))
                             (p (and k (< k (length plan)) (list-ref plan k))))
                        (log-message! (string-append "lsp: not applied: " why))
                        (list 'failed (if p (planned-index p) 0)
                              (if p (string-append (abbreviate-path (planned-path p)) ": " (substring rest (+ colon 2) (string-length rest))) why)))))
          (views-edit-at! (map (lambda (p) (list (planned-view p) (planned-revision p) (planned-edits p))) plan) "new")
          ;; A file changed is a buffer to see and save.
          (for-each (lambda (p) (add-buffer! (planned-document p))) plan)
          (cons 'ok (length plan))))))

(define (lsp-apply-workspace-edit! ls edit #:revisions [revisions '()])
  "Make the changes of EDIT, a WorkspaceEdit from session LS's server.
All or none: every file's are checked before any is made — each a file,
not read-only, its edits not overlapping nor touching text changed since
— and if one cannot be made, none is (those made are undone). Each
file's are one change, undone together; a file not open is opened, in a
buffer of its own. Creating, renaming and deleting files are refused.
An edit naming no version is of the text the server had: for a file in
REVISIONS, an alist (path . revision) taken when it was asked, that
revision's; else the one last sent it; else the file as it is, unless
its buffer has changes the server has not seen. Return how many files
changed; raise the first problem, every one logged to *Messages*."
  (let ((r (apply-workspace-edit ls edit revisions)))
    (if (eq? (car r) 'ok) (cdr r) (error (caddr r)))))

;;; Rename, format, code actions

;; The name around POS in D: letters, digits and underscores, as most
;; languages a server serves spell one.
(define (name-span d pos)
  (let ((name? (lambda (p) (let ((c (string-ref (document-substring d p (next-grapheme d p)) 0)))
                             (or (char-alphabetic? c) (char-numeric? c) (char=? c #\_))))))
    (list (let back ((p pos)) (if (and (> p 0) (name? (prev-grapheme d p))) (back (prev-grapheme d p)) p))
          (let on ((p pos)) (if (and (< p (document-length d)) (name? p)) (on (next-grapheme d p)) p)))))

;; Run (PRODUCE) in a task; give (DELIVER result) in S, a failure said.
(define (asking s what produce deliver)
  (message! s what)
  (request! (or (sget s 'lsp-slot) (let ((slot (make-request-slot))) (sset! s 'lsp-slot slot) slot))
            produce (lambda (r) (message! s #f) (deliver r)) #:fail (lambda (e) (message! s (error-text e)))))

;; What the server has of each of LS's documents: (path . revision).
(define (sent-revisions ls)
  (filter-map (lambda (t) (and (tracked-sent t) (cons (tracked-path t) (tracked-sent t)))) (hash-table-values (session-documents ls))))

;; The name at L now, as the text spells it there ("" for none).
(define (name-at-location l)
  (let* ((d (location-document l)) (span (name-span d (location-position l))))
    (document-substring d (car span) (cadr span))))

;; The session renaming in D: the first of its buffer's that can.
(define (rename-session d)
  (let* ((b (document-buffer d)) (all (if b (sessions-with b "renameProvider") '())))
    (if (pair? all) (car all) (error "No language server here renames"))))

;; Read the new name: the server is asked first what is renamed at AT,
;; and says where (prepareRename) — or that nothing is, and nothing is
;; asked; its placeholder is the name to edit.
(define (read-new-name s args k)
  (let* ((at (dict-ref args 'at)) (d (location-document at)) (ls (rename-session d)))
    (asking s "Renaming…"
            (lambda () (rename-target ls d (location-position at) (document-revision d)))
            (lambda (target)
              (let ((old (list-ref target 3)))
                (completing-read s (string-append "Rename " old " to: ") '()
                                 #:initial old #:require-match #f
                                 #:accept (lambda (s c) (k (candidate-text c)))))))))

(define-command (lsp-rename s at new-name name)
  "Rename the name at AT to NEW-NAME wherever the language server finds it.
The server is asked first what is renamed at AT, where it says (as
Eglot's prepareRename); renamed by the same server, where the name is
then. With NAME, refused unless it is still the name there. From keys,
AT is point, NAME the name there, and NEW-NAME is read, the old name
(as the server says it) to edit."
  #:arguments '((at location #:doc "Where the name is.")
                (new-name (string #:min 1) #:doc "Its new name.")
                (name string #:default "" #:doc "The name expected at AT, refused if another is there now; \"\" for any."))
  #:read `((at point)
           (name context #:value ,(lambda (s args) (name-at-location (dict-ref args 'at))))
           (new-name ,read-new-name))
  #:effects '(read query edit)
  #:expose '(agents)
  ;; It renames later, when the server answers: its operation says how.
  #:result 'any
  (let ((d (location-document at)) (old (name-at-location at)))
    (unless (or (string=? name "") (string=? name old))
      (error "The name changed while the new one was read; rename again"))
    (let ((ls (rename-session d)))
      (asking s "Renaming…"
              (lambda () (rename! ls d (rename-target ls d (location-position at) (document-revision d)) new-name))
              (lambda (files)
                (message! s (string-append (if (and (current-operation) (operation-staged? (current-operation))) "Staged rename of " "Renamed ")
                                           old " to " new-name " in " (number->string files)
                                           (if (= files 1) " file" " files"))))))
    #t))

;; What is renamed at POS of D as of REVISION, as session LS's server says
;; (prepareRename), or the name there where it says nothing of it:
;; (from to revision old was), FROM and TO of D's text at REVISION, OLD
;; the name to edit, WAS the text from FROM to TO then. An error where
;; the server says nothing is renamed there.
(define (rename-target ls d pos revision)
  (let* ((asked (and (await-ready ls) (lsp-session-capability ls "renameProvider" "prepareProvider")
                     (lsp-document-request ls d "textDocument/prepareRename"
                                           (lambda (r) (lsp-text-document-position ls d (or (document-map-position d pos revision)
                                                                                            (error "lsp: the text renamed in is gone"))))
                                           #:on-error (lambda (code message)
                                                        (if (= code -32601) 'unsupported (error message))))))
         (answer (and asked (cdr asked)))
         (now (document-revision d))
         (here (lambda ()
                 (let* ((at (or (document-map-position d pos revision) (error "lsp: the text renamed in is gone")))
                        (span (name-span d at)))
                   (when (= (car span) (cadr span)) (error "No name here to rename"))
                   (let ((text (document-substring d (car span) (cadr span))))
                     (list (car span) (cadr span) now text text))))))
    (cond ((or (not asked) (eq? answer 'unsupported) (and (dict? answer) (dict-contains? answer "defaultBehavior"))) (here))
          ((not (dict? answer)) (error "Nothing here can be renamed"))
          (else
           (let* ((range (if (dict-contains? answer "range") (dict-ref answer "range") answer))
                  (span (or (lsp-range-positions ls d range #:revision (car asked)) (error "lsp: the text renamed in is gone")))
                  (text (document-substring d (car span) (cadr span))))
             (list (car span) (cadr span) now (let ((p (dict-ref answer "placeholder" #f))) (if (string? p) p text)) text))))))

;; Rename TARGET, from `rename-target`, of D to NEW, by session LS's
;; server: refused if its text changed since. Return the files changed.
(define (rename! ls d target new)
  (let* ((revision (list-ref target 2))
         (from (document-map-position d (car target) revision))
         (to (document-map-position d (cadr target) revision))
         (was (list-ref target 4)))
    (unless (and from to (equal? (document-substring d from to) was))
      (error "The name changed while the new one was read; rename again"))
    (let* ((revisions '())
           (asked (lsp-document-request ls d "textDocument/rename"
                                        (lambda (r)
                                          ;; The revisions of every document as the server
                                          ;; has them as it is asked: its edit is of those.
                                          (set! revisions (sent-revisions ls))
                                          (dict-set (lsp-text-document-position ls d from) "newName" new)))))
      (unless asked (error "The language server ended"))
      (if (eq? (cdr asked) 'null)
          (error "The language server renamed nothing")
          (lsp-apply-workspace-edit! ls (cdr asked) #:revisions revisions)))))

(define-option lsp-format-tab-size 4
  "The width of an indentation step a language server formats with."
  #:type 'integer)

(define-option lsp-format-insert-spaces #t
  "Whether a language server formats indentation with spaces, not tabs."
  #:type 'boolean)

;; What a server is told to format with, of buffer B's options.
(define (formatting-options b)
  (dict "tabSize" (option b 'lsp-format-tab-size) "insertSpaces" (option b 'lsp-format-insert-spaces)))

;; Format document D, or its text from FROM to TO, by the first server
;; of its buffer doing so, in session S.
(define (format! s d from to)
  (let ((b (document-buffer d)))
    (asking s "Formatting…"
            (lambda ()
              (let ((found (if from
                               (ask-first d "documentRangeFormattingProvider" "textDocument/rangeFormatting"
                                          (lambda (ls)
                                            (dict "textDocument" (lsp-text-document d)
                                                  "range" (dict "start" (lsp-position ls d from) "end" (lsp-position ls d to))
                                                  "options" (formatting-options b))))
                               (ask-first d "documentFormattingProvider" "textDocument/formatting"
                                          (lambda (ls)
                                            (dict "textDocument" (lsp-text-document d) "options" (formatting-options b)))))))
                (unless found (error "No language server here formats"))
                ;; As the server gives them, those outside the region too.
                (apply-text-edits! (found-session found) d (found-revision found) (as-list (found-answer found)))))
            (lambda (r) (message! s "Formatted")))))

(define (formats? b capability)
  (any (lambda (ls) (lsp-session-capability ls capability)) (lsp-buffer-sessions b)))

(define-command (lsp-format-buffer s n)
  "Format the buffer as its language server does (rustfmt, for Rust)."
  (format! s (doc s) #f #f))

(define-command (lsp-format s n)
  "Format the region as the language server does, else the buffer.
Where the server formats only whole buffers, whether to format the
whole buffer is asked. Not for more than one region yet."
  (let* ((d (doc s)) (b (or (current-buffer s) (error "No buffer")))
         (regions (if (sget s 'extend)
                      (filter-map (lambda (r) (and (not (= (car r) (cadr r))) (list (min (car r) (cadr r)) (max (car r) (cadr r)))))
                                  (ranges s))
                      '())))
    (cond ((null? regions) (format! s d #f #f))
          ((pair? (cdr regions)) (error "Formatting more than one region is not done yet"))
          (else
           ;; What the servers do is known once they are ready.
           (asking s "Formatting…"
                   (lambda () (for-each await-ready (lsp-buffer-sessions b)) #t)
                   (lambda (ready)
                     (cond ((formats? b "documentRangeFormattingProvider")
                            (sset! s 'extend #f)
                            (format! s d (car (car regions)) (cadr (car regions))))
                           ((formats? b "documentFormattingProvider")
                            (completing-read s "The server formats only whole buffers. Format the whole buffer? " '("yes" "no")
                                             #:accept (lambda (s c)
                                                        (when (equal? (candidate-text c) "yes")
                                                          (sset! s 'extend #f)
                                                          (format! s d #f #f)))))
                           (else (message! s "No language server here formats")))))))))

;; The diagnostics from FROM to TO as their server said them, for the
;; code actions on them.
(define (code-action-context d from to)
  (dict "diagnostics" (filter-map (lambda (x) (and (<= (car x) to) (>= (cadr x) from) (list-ref x 5)))
                                  (document-diagnostics d))
        "triggerKind" 1))

(define-command (lsp-code-actions s n)
  "Choose one of the language server's actions at point and do it.
They are its fixes for the diagnostics there and its refactorings of
the region, or of the thing at point."
  (let* ((d (doc s)) (r (list-ref (ranges s) (view-primary (session-view s))))
         (from (min (car r) (cadr r))) (to (max (car r) (cadr r))))
    (asking s "Asking for actions…"
            (lambda ()
              (let* ((revisions '())
                     (found (ask-first d "codeActionProvider" "textDocument/codeAction"
                                      (lambda (ls)
                                        ;; Their edits are of the text the server has.
                                        (set! revisions (sent-revisions ls))
                                        (dict "textDocument" (lsp-text-document d)
                                              "range" (dict "start" (lsp-position ls d from) "end" (lsp-position ls d to))
                                              "context" (code-action-context d from to))))))
                (and found (list (found-session found) revisions (as-list (found-answer found))))))
            (lambda (found)
              (if (or (not found) (null? (caddr found)))
                  (message! s "No actions here")
                  (let ((ls (car found)) (revisions (cadr found)))
                    (completing-read s "Action: "
                                     ;; Each candidate holds its action: two of one
                                     ;; title are two actions.
                                     (map (lambda (a) (candidate (dict-ref a "title" "?")
                                                                 #:annotation (let ((k (dict-ref a "kind" #f))) (if (string? k) k ""))
                                                                 #:target (target 'code-action a)))
                                          (caddr found))
                                     #:accept (lambda (s c)
                                                (let ((action (target-value (candidate-target c))))
                                                  (asking s "Acting…"
                                                          (lambda () (do-code-action! ls action revisions))
                                                          (lambda (r) (message! s (candidate-text c)))))))))))))

;; Do ACTION, a CodeAction or a Command: its edit (resolved first if it
;; comes without), then its command. REVISIONS are what the server had
;; when it gave the action, (path . revision): its edit is of those,
;; unless resolved, then of what it had then.
(define (do-code-action! ls action revisions)
  (let* ((resolve (and (not (dict-contains? action "edit")) (dict-contains? action "data")
                       (lsp-session-capability ls "codeActionProvider" "resolveProvider")))
         (action (if resolve
                     (lsp-request ls "codeAction/resolve" action #:sent (lambda () (set! revisions (sent-revisions ls))))
                     action))
         (edit (dict-ref action "edit" #f))
         (command (dict-ref action "command" #f)))
    (when (dict? edit) (lsp-apply-workspace-edit! ls edit #:revisions revisions))
    (when (or (dict? command) (string? command)) (note-effect! 'external))
    (cond ((dict? command)
           (lsp-request ls "workspace/executeCommand"
                        (dict "command" (dict-ref command "command") "arguments" (dict-ref command "arguments" #()))))
          ((string? command)
           (lsp-request ls "workspace/executeCommand" (dict "command" command "arguments" (dict-ref action "arguments" #())))))))

;;; Following buffers

(add-hook! 'file-buffer-added 'lsp
           (lambda (s b) (when (option b 'lsp-auto) (lsp-attach! b #:quiet #t))))

(add-hook! 'buffer-killed 'lsp
           (lambda (s b)
             (let ((path (document-path (buffer-document b))))
               (when path
                 (for-each (lambda (ls)
                             (let ((t (hash-table-ref/default (session-documents ls) path #f)))
                               (when t (close-tracked! ls t))))
                           (hash-table-values %sessions))))))

;; Saved: the server has the text up to now, then is told of the save,
;; with the text as it was saved — here, as the save returns, before an
;; edit or a close can come between.
(add-hook! 'document-saved 'lsp
           (lambda (s d)
             (let ((path (document-path d)) (text (document-string d)))
               (for-each (lambda (ls)
                           (let ((t (and (eq? (session-state ls) 'ready) (hash-table-ref/default (session-documents ls) path #f)))
                                 (save (wants-save? ls)))
                             (when (and t save (tracked-sent t))
                               (with-lock t
                                 (lambda ()
                                   (sync-now! ls t)
                                   (lsp-notify ls "textDocument/didSave"
                                               (if (and (dict? save) (eq? (dict-ref save "includeText" #f) #t))
                                                   (dict "textDocument" (dict "uri" (file-uri path)) "text" text)
                                                   (dict "textDocument" (dict "uri" (file-uri path))))))))))
                         (hash-table-values %sessions)))))

;; A file moved under its buffer: its servers close the old name and open
;; the new, as Eglot does when a buffer's file name changes.
(add-hook! 'document-moved 'lsp
           (lambda (s d)
             (for-each (lambda (ls)
                         (for-each (lambda (t)
                                     (when (and (eq? (tracked-document t) d) (not (equal? (tracked-path t) (document-path d))))
                                       (close-tracked! ls t)))
                                   (hash-table-values (session-documents ls))))
                       (hash-table-values %sessions))
             (let ((b (document-buffer d)))
               (when (and b (option b 'lsp-mode)) (lsp-attach! b #:quiet #t)))))

;;; The servers built in: each attaches where its program is installed.

(define-lsp-server rust-analyzer
  "Rust, by rust-analyzer."
  #:command '("rust-analyzer") #:languages '(rust) #:roots '("Cargo.toml" "rust-project.json")
  #:settings (dict "rust-analyzer" (dict "check" (dict "command" "clippy")
                                         "inlayHints" (dict "typeHints" (dict "enable" #t)
                                                            "parameterHints" (dict "enable" #t)
                                                            "chainingHints" (dict "enable" #t)))))

(define-lsp-server clangd "C and C++, by clangd." #:command '("clangd") #:languages '(c cpp))
(define-lsp-server pyright "Python, by Pyright." #:command '("pyright-langserver" "--stdio") #:languages '(python))
(define-lsp-server gopls "Go, by gopls." #:command '("gopls") #:languages '(go) #:roots '("go.mod"))
(define-lsp-server typescript-language-server
  "TypeScript and JavaScript, by typescript-language-server."
  #:command '("typescript-language-server" "--stdio") #:languages '(typescript tsx javascript))
(define-lsp-server nil "Nix, by nil." #:command '("nil") #:languages '(nix))
