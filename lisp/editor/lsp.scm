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
(require "projects.scm")
(require "remote.scm")
(require "syntax.scm")
(require "rpc.scm")
(require "requests.scm")
(require "xref.scm")
(require "eldoc.scm")

(provide define-lsp-server register-lsp-server! lsp-server-names lsp-server-doc
         lsp lsp-shutdown lsp-restart lsp-next-diagnostic lsp-previous-diagnostic lsp-diagnostics
         lsp-rename lsp-format-buffer lsp-code-actions lsp-apply-workspace-edit!
         lsp-attach! lsp-buffer-sessions lsp-session? lsp-session-server-name lsp-session-capability lsp-session-encoding
         lsp-sync! lsp-request lsp-notify lsp-text-document lsp-position lsp-range-positions lsp-location
         lsp-diagnostics-at file-uri uri-file-path)

;;; Faces and options

(define-face diagnostic "A language server's diagnostic." #:underline 'curl)
(define-face diagnostic.error "An error, from a language server." #:underline '(curl red))
(define-face diagnostic.warning "A warning, from a language server." #:underline '(curl yellow))
(define-face diagnostic.info "A note, from a language server." #:underline '(dotted sky))
(define-face diagnostic.hint "A hint, from a language server." #:underline '(dotted overlay))

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
  (%make-session server root conn state ready gone capabilities documents progress generation)
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
  (generation session-generation))

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

(define (lsp-request ls method params #:timeout [patience rpc-timeout])
  "Ask METHOD of the server of session LS with PARAMS; return its answer.
The waiting is the calling task's. An error answer, a dead server and no
answer in PATIENCE milliseconds (#:timeout) are errors."
  (rpc-request! (session-conn ls) method params #:timeout patience))

(define (lsp-notify ls method params)
  "Send the server of session LS the notification METHOD with PARAMS."
  (rpc-notify! (session-conn ls) method params))

;;; What the client says it does. The answers' handling in xref.scm and
;;; eldoc.scm extends it.

(define (client-capabilities)
  (dict "general" (dict "positionEncodings" '("utf-8" "utf-16"))
        "workspace" (dict "configuration" #t "workspaceFolders" #t "applyEdit" #t
                          "workspaceEdit" (dict "documentChanges" #t)
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
        ((equal? method "workspace/workspaceFolders")
         (list (dict "uri" (file-uri (session-root ls)) "name" (session-root ls))))
        ((equal? method "workspace/applyEdit")
         (guard (e (#t (dict "applied" #f "failureReason" (error-text e))))
           (lsp-apply-workspace-edit! ls (dict-ref params "edit"))
           (dict "applied" #t)))
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
  (let ((ls (%make-session server root #f 'starting (make-channel) (make-channel) (dict) (make-hash-table) (make-hash-table) %generation))
        (name (symbol->string (lsp-server-name server))))
    (set-session-conn! ls (rpc-connect (car (lsp-server-command server)) (cdr (lsp-server-command server))
                                       #:directory root
                                       #:on-note (lambda (m p) (on-note ls m p))
                                       #:on-ask (lambda (m p) (on-ask ls m p))
                                       #:on-dead (lambda () (session-ended! ls))))
    (hash-table-set! %sessions (session-key root server) ls)
    (spawn (lambda ()
             (guard (e (#t (say (string-append name ": " (error-text e))) (end-session! ls)))
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
                 (for-each (lambda (t) (open-tracked! ls t)) (hash-table-values (session-documents ls)))
                 (channel-close (session-ready ls))
                 (say (string-append name " ready in " (abbreviate-path root)))))))
    ls))

;; The process ended: by itself, said; asked to, by `end-session!'.
(define (session-ended! ls)
  (guard (e (#t #f)) (channel-close (session-gone ls)))
  (unless (memq (session-state ls) '(stopping dead))
    (set-session-state! ls 'dead)
    (say (string-append (symbol->string (lsp-server-name (session-server ls))) " ended"))
    (forget-session! ls)))

;; LS is no more: out of the table (its slot, unless a successor took
;; it), what waits for it let go, its documents' followers ended.
(define (forget-session! ls)
  (guard (e (#t #f)) (channel-close (session-ready ls)))
  (when (eq? (hash-table-ref/default %sessions (session-key (session-root ls) (session-server ls)) #f) ls)
    (hash-table-delete! %sessions (session-key (session-root ls) (session-server ls))))
  (for-each forget-tracked! (hash-table-values (session-documents ls))))

;; End LS: a server that is up is asked to shut down and exit, and given
;; a second to; then its process is killed.
(define (end-session! ls)
  (let ((was (session-state ls)))
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
      (say (string-append (symbol->string (lsp-server-name (session-server ls))) " shut down")))))

(define (await-ready ls)
  "Wait in this task for the session LS to be ready; return #f if it died."
  (channel-recv (session-ready ls))
  (eq? (session-state ls) 'ready))

;;; Documents: each followed by a task of its own while it is open.

(define-record-type tracked
  (%make-tracked document path language sent lock task diagnostics)
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
  (diagnostics tracked-diagnostics set-tracked-diagnostics!))

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
      (let ((t (%make-tracked d path (language-id b) #f (make-channel 1) #f '())))
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

(define (lsp-document-request ls d method make #:timeout [patience rpc-timeout])
  "Ask METHOD of session LS's server about document D as it is.
MAKE is called with the revision the server has, with nothing sent of D
meanwhile, and gives the params (positions in that revision's text). Its
frame is queued before any edit after it is sent. Return (revision .
answer), or #f if the session died first or D is not open in it."
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
                      (cons revision (rpc-request! (session-conn ls) method params #:timeout patience #:sent release!))))))))))

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
           ("C-c c r" lsp-rename) ("C-c c f" lsp-format-buffer) ("C-c c a" lsp-code-actions)
           ("C-c c x" lsp-diagnostics) ("C-c c k" eldoc-doc-buffer))
  #:normal '(("] e" lsp-next-diagnostic) ("[ e" lsp-previous-diagnostic) ("SPC c x" lsp-diagnostics)
             ("K" eldoc-doc-buffer) ("SPC c k" eldoc-doc-buffer)
             ("SPC c r" lsp-rename) ("SPC c f" lsp-format-buffer) ("SPC c a" lsp-code-actions))
  #:layer (lambda (d from to) (diagnostic-layer d from to)))

;; The mode line: the servers' work in progress, else the diagnostics'
;; count by severity, as Flymake's lighter.
(register-mode-line-segment! 'lsp
  (lambda (b)
    (and (option b 'lsp-mode)
         (let ((sessions (lsp-buffer-sessions b)))
           (and (pair? sessions)
                (let ((busy (append-map (lambda (ls) (map cadr (hash-table-values (session-progress ls)))) sessions)))
                  (cond ((any (lambda (ls) (eq? (session-state ls) 'starting)) sessions) "lsp:starting")
                        ((pair? busy) (string-append "lsp:" (car busy)))
                        (else
                         (let* ((found (document-diagnostics (buffer-document b)))
                                (count (lambda (n) (length (filter (lambda (x) (= (caddr x) n)) found)))))
                           (string-append "lsp:" (number->string (count 1)) "E " (number->string (count 2)) "W"))))))))))

;;; Commands

(define-command (lsp s n)
  "Attach the focused buffer to the language servers serving it.
Each starts in the project's root unless it runs there already; its
diagnostics show under `lsp-mode'."
  (let ((sessions (lsp-attach! (or (current-buffer s) (error "No buffer")))))
    (when (null? sessions) (message! s "No language server serves this buffer"))))

(define-command (lsp-shutdown s n)
  "Shut down the language servers of the focused buffer.
Their buffers leave `lsp-mode'."
  (let ((b (or (current-buffer s) (error "No buffer"))))
    (for-each (lambda (ls)
                (for-each (lambda (t) (let ((tb (document-buffer (tracked-document t))))
                                        (when tb (set-option! 'lsp-mode #f #:buffer tb))))
                          (hash-table-values (session-documents ls)))
                (spawn (lambda () (end-session! ls))))
              (lsp-buffer-sessions b))))

(define-command (lsp-restart s n)
  "Restart the language servers of the focused buffer.
Each is shut down and started again, with every buffer it served. Where
they ended (crashed), they are started again for this buffer."
  (let ((b (or (current-buffer s) (error "No buffer"))))
    (when (null? (lsp-buffer-sessions b))
      (lsp-attach! b))
    (for-each (lambda (ls)
                (let ((buffers (filter-map (lambda (t) (document-buffer (tracked-document t))) (hash-table-values (session-documents ls))))
                      (next (begin (hash-table-delete! %sessions (session-key (session-root ls) (session-server ls)))
                                   (start-session! (session-server ls) (session-root ls)))))
                  (spawn (lambda () (end-session! ls)))
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

;; Markup as text: MarkupContent, MarkedString, or a list of them; code
;; fences dropped, their code kept.
(define (markup-text v)
  (cond ((string? v) v)
        ((dict? v) (let ((value (dict-ref v "value" "")))
                     (if (dict-contains? v "language") value (strip-fences value))))
        ((or (vector? v) (pair? v)) (string-join (map markup-text (as-list v)) "\n\n"))
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
                 (let ((text (markup-text (dict-ref (found-answer found) "contents" ""))))
                   (and (not (string=? (string-trim text) ""))
                        ;; The echo area: the signature, not the
                        ;; documentation under the rule.
                        (if full text (car (string-split text "\n---")))))))))

;;; Edits a server sends: a workspace edit, a document's text edits.

;; Apply TEXT-EDITS to document D, their positions those of D's text at
;; REVISION (as LS counts them), as the user's one undoable change.
(define (apply-text-edits! ls d revision text-edits)
  (unless (null? text-edits)
    (let* ((points (append-map (lambda (e) (let ((r (dict-ref e "range")))
                                             (list (point-of (dict-ref r "start")) (point-of (dict-ref r "end")))))
                               text-edits))
           (offsets (or (document-lsp-offsets d revision (lsp-session-encoding ls) points)
                        (error "lsp: the text the edits were made against is gone")))
           (edits (let loop ((es text-edits) (os offsets) (out '()))
                    ;; In the server's order where they start alike: two
                    ;; inserted at one place go in as it gave them.
                    (if (null? es)
                        (sort (reverse out) (lambda (a b) (< (car a) (car b))))
                        (loop (cdr es) (cddr os) (cons (list (car os) (cadr os) (dict-ref (car es) "newText" "")) out)))))
           (b (document-buffer d))
           (v (if b (or (buffer-view b) (make-view d "user")) (make-view d "user"))))
      (check-editable! v)
      (view-edit-at! v revision edits "new"))))

(define (lsp-apply-workspace-edit! ls edit #:revisions [revisions '()])
  "Make the changes of EDIT, a WorkspaceEdit from session LS's server.
Each file's are one change, undone together; a file not open is opened,
in a buffer of its own. Creating, renaming and deleting files are
refused. An edit naming no version is of the text the server had: for
a file in REVISIONS, an alist (path . revision) taken when it was asked,
that revision's; else the one last sent it, or the file as it is."
  (let ((changes (append
                  (map (lambda (e) (cons (dict-ref (dict-ref e "textDocument") "uri")
                                         (cons (dict-ref (dict-ref e "textDocument") "version" 'null) (as-list (dict-ref e "edits")))))
                       (filter (lambda (e) (or (dict-contains? e "textDocument")
                                               (error "lsp: creating, renaming and deleting files is not done yet")))
                               (as-list (dict-ref edit "documentChanges" #()))))
                  (map (lambda (kv) (cons (car kv) (cons 'null (as-list (cdr kv)))))
                       (let ((c (dict-ref edit "changes" #f))) (if (dict? c) (dict->alist c) '()))))))
    (for-each (lambda (change)
                (let* ((path (or (uri-file-path (car change)) (error "lsp: not a file:" (car change))))
                       (d (file-document path))
                       (version (cadr change))
                       (t (tracked-in ls d))
                       (revision (cond ((number? version) version)
                                       ((assoc path revisions) => cdr)
                                       ((and t (tracked-sent t)))
                                       (else (document-revision d)))))
                  ;; A file changed is a buffer to see and save.
                  (add-buffer! d)
                  (apply-text-edits! ls d revision (cddr change))))
              changes)
    (length changes)))

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

(define-command (lsp-rename s n)
  "Rename the name at point everywhere, as the language server finds it.
The new name is read in the minibuffer, the old one to edit."
  (let* ((d (doc s)) (pos (point s)) (span (name-span d pos))
         (old (document-substring d (car span) (cadr span))))
    (completing-read s (string-append "Rename " old " to: ") '()
                     #:initial old #:require-match #f
                     #:accept (lambda (s c)
                                (let ((new (candidate-text c)))
                                  (asking s "Renaming…"
                                          (lambda ()
                                            ;; The revisions of every document as the
                                            ;; server had them when asked: the edit is of those.
                                            (let* ((revisions '())
                                                   (found (ask-first d "renameProvider" "textDocument/rename"
                                                                     (lambda (ls)
                                                                       (set! revisions (sent-revisions ls))
                                                                       (dict-set (lsp-text-document-position ls d pos) "newName" new)))))
                                              (unless found (error "No language server here renames"))
                                              (lsp-apply-workspace-edit! (found-session found) (found-answer found) #:revisions revisions)))
                                          (lambda (files)
                                            (message! s (string-append "Renamed " old " to " new " in " (number->string files)
                                                                       (if (= files 1) " file" " files"))))))))))

(define-command (lsp-format-buffer s n)
  "Format the buffer as its language server does (rustfmt, for Rust)."
  (let ((d (doc s)))
    (asking s "Formatting…"
            (lambda ()
              (let ((found (ask-first d "documentFormattingProvider" "textDocument/formatting"
                                      (lambda (ls)
                                        (dict "textDocument" (lsp-text-document d)
                                              "options" (dict "tabSize" 4 "insertSpaces" #t))))))
                (unless found (error "No language server here formats"))
                (apply-text-edits! (found-session found) d (found-revision found) (as-list (found-answer found)))))
            (lambda (r) (message! s "Formatted")))))

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
              (let ((found (ask-first d "codeActionProvider" "textDocument/codeAction"
                                      (lambda (ls)
                                        (dict "textDocument" (lsp-text-document d)
                                              "range" (dict "start" (lsp-position ls d from) "end" (lsp-position ls d to))
                                              "context" (code-action-context d from to))))))
                (and found (cons (found-session found) (as-list (found-answer found))))))
            (lambda (found)
              (if (or (not found) (null? (cdr found)))
                  (message! s "No actions here")
                  (let ((ls (car found)))
                    (completing-read s "Action: "
                                     ;; Each candidate holds its action: two of one
                                     ;; title are two actions.
                                     (map (lambda (a) (candidate (dict-ref a "title" "?")
                                                                 #:annotation (let ((k (dict-ref a "kind" #f))) (if (string? k) k ""))
                                                                 #:target (target 'code-action a)))
                                          (cdr found))
                                     #:accept (lambda (s c)
                                                (let ((action (target-value (candidate-target c))))
                                                  (asking s "Acting…"
                                                          (lambda () (do-code-action! ls action))
                                                          (lambda (r) (message! s (candidate-text c)))))))))))))

;; Do ACTION, a CodeAction or a Command: its edit (resolved first if it
;; comes without), then its command.
(define (do-code-action! ls action)
  (let* ((action (if (and (not (dict-contains? action "edit")) (dict-contains? action "data")
                          (lsp-session-capability ls "codeActionProvider" "resolveProvider"))
                     (lsp-request ls "codeAction/resolve" action)
                     action))
         (edit (dict-ref action "edit" #f))
         (command (dict-ref action "command" #f)))
    (when (dict? edit) (lsp-apply-workspace-edit! ls edit))
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
