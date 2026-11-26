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
;;; The session, protocol, diagnostic, inlay-hint, and editing runtime
;;; lives in `lsp/runtime.scm` and is loaded lazily on first use so
;;; editor startup pays only for declarations and hook wiring.

(require "session.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "faces.scm")
(require "commands.scm")
(require "buffers.scm")
(require "files.scm")
(require "remote.scm")
(require "syntax.scm")
(require "rpc.scm")
(require "xref.scm")
(require "eldoc.scm")
(require "project-settings.scm")

(provide define-lsp-server register-lsp-server! lsp-server-names lsp-server-doc
         lsp lsp-shutdown lsp-restart lsp-next-diagnostic lsp-previous-diagnostic lsp-diagnostics
         lsp-rename lsp-format lsp-format-buffer lsp-code-actions lsp-events lsp-apply-workspace-edit!
         lsp-attach! lsp-buffer-sessions lsp-session? lsp-session-server-name lsp-session-env-id
         lsp-session-capability lsp-session-encoding lsp-effective-settings lsp-refresh-configuration!
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
  #:type 'boolean
  #:directory-safe #t)

(define-option lsp-auto #t
  "Whether a file's buffer attaches to its language servers as it opens.
As Eglot's `eglot-ensure': a server whose program is not installed is
passed over quietly; \\[lsp] says why."
  #:type 'boolean)

(define-option lsp-settings (dict)
  "Language server configuration overlaid onto server default settings.
Keys are top-level LSP configuration sections such as
`\"rust-analyzer\"`. Uses deep JSON-object merge so nested tables compose
and `(unset)` removes keys from lower layers or server defaults. Project
`.techne.scm` values require explicit user trust."
  #:type '(dict-of string any)
  #:merge 'json-object
  #:directory-safe #f)

(define-option lsp-autoreconnect #t
  "Whether a language server that ends unasked is started again.
As Eglot's `eglot-autoreconnect': one that was ready, after a second,
then three, ten and thirty, each time it ends again before it has been
ready a minute; then not, until \\[lsp-restart]. Its buffers' diagnostics
go meanwhile."
  #:type 'boolean)

(define-option lsp-format-tab-size #f
  "The width of an indentation step a language server formats with.
When `#f`, falls back to the buffer's `indent-width`."
  #:type '(or (one-of #f) (integer #:min 1 #:max 32))
  #:directory-safe #t)

(define-option lsp-format-insert-spaces 'inherit
  "Whether a language server formats indentation with spaces, not tabs.
When `'inherit`, falls back to `(not (option b 'indent-tabs))`."
  #:type '(one-of inherit #t #f)
  #:directory-safe #t)

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

;;; Lazy runtime loading

(define %lsp-module-name (current-module))
(define %lsp-runtime-table #f)

(define (%lsp-runtime-hooks)
  (dict 'lsp-server-name lsp-server-name
        'lsp-server-command lsp-server-command
        'lsp-server-modes lsp-server-modes
        'lsp-server-languages lsp-server-languages
        'lsp-server-roots lsp-server-roots
        'lsp-server-settings lsp-server-settings
        'lsp-server-initialization lsp-server-initialization
        'servers-for servers-for))

(define %ensure-lsp-runtime!
  (scope-procedure
   (lambda ()
     (or %lsp-runtime-table
         (let ((init (eval-source "(begin (require \"lsp/runtime.scm\") lsp-runtime-init!)" %lsp-module-name %lsp-module-name)))
           (set! %lsp-runtime-table (init (%lsp-runtime-hooks)))
           %lsp-runtime-table)))))

(define (lsp-rt name)
  (dict-ref (%ensure-lsp-runtime!) name))

;;; Public API wrappers

(define (file-uri path)
  "Return PATH as a file URI.
Every byte of its UTF-8 that is not unreserved (or the path's own
separator) is percent-coded: %, #, ? and the non-ASCII alike."
  ((lsp-rt 'file-uri) path))

(define (uri-file-path uri)
  "Return the path the file URI names, percent-decoded, or #f.
#f for another scheme, a host that is not this one, and percent-coding
that does not decode."
  ((lsp-rt 'uri-file-path) uri))

(define (lsp-session? obj)
  "Return #t if OBJ is an `lsp-session` record."
  (and %lsp-runtime-table ((dict-ref %lsp-runtime-table 'lsp-session?) obj)))

(define (lsp-session-server-name ls)
  "Return the name of the language server of the session LS."
  ((lsp-rt 'lsp-session-server-name) ls))

(define (lsp-session-env-id ls)
  "Return the environment identity hash of the language server session LS."
  ((lsp-rt 'lsp-session-env-id) ls))

(define (lsp-session-capability ls . path)
  "Return what the server of session LS said of the capability at PATH.
PATH is a list of keys into its `capabilities', as \"hoverProvider\";
#f where it said nothing (or false)."
  (apply (lsp-rt 'lsp-session-capability) ls path))

(define (lsp-session-encoding ls)
  "Return the position encoding agreed with LS's server.
That is \"utf-8\" where it takes it, else \"utf-16\"."
  ((lsp-rt 'lsp-session-encoding) ls))

(define (lsp-request ls method params #:timeout [patience rpc-timeout] #:sent [sent #f] #:on-error [on-error #f])
  "Ask METHOD of the server of session LS with PARAMS; return its answer.
The waiting is the calling task's. An error answer, a dead server and no
answer in PATIENCE milliseconds (#:timeout) are errors. SENT and
ON-ERROR, if given, are as `rpc-request!` takes them."
  ((lsp-rt 'lsp-request) ls method params #:timeout patience #:sent sent #:on-error on-error))

(define (lsp-notify ls method params)
  "Send the server of session LS the notification METHOD with PARAMS."
  ((lsp-rt 'lsp-notify) ls method params))

(define (lsp-effective-settings server where)
  "Return the effective LSP settings dict for SERVER at WHERE.
Overlays `lsp-settings` at WHERE (preserving `(unset)` removals) onto
SERVER's declared defaults."
  ((lsp-rt 'lsp-effective-settings) server where))

(define (lsp-refresh-configuration! [dir #f])
  "Push `workspace/didChangeConfiguration` to live sessions under DIR.
When DIR is `#f`, checks all live sessions and notifies those whose
effective settings changed without restarting them. Sessions still
initializing are marked dirty and flushed upon becoming ready."
  (when %lsp-runtime-table
    ((dict-ref %lsp-runtime-table 'lsp-refresh-configuration!) dir)))

(define (lsp-text-document d)
  "Return the TextDocumentIdentifier of document D."
  ((lsp-rt 'lsp-text-document) d))

(define (lsp-attach! b #:quiet [quiet #f])
  "Attach buffer B to the language servers serving it.
Return their sessions. A server starts where none runs for the buffer's
project. Unless QUIET, a buffer no installed server serves is an error
saying so; QUIET passes it over, as opening a file does, and leaves out
servers whose project files (#:roots) are not there."
  ((lsp-rt 'lsp-attach!) b #:quiet quiet))

(define (lsp-buffer-sessions b)
  "Return the live sessions buffer B's document is open in."
  (if %lsp-runtime-table
      ((dict-ref %lsp-runtime-table 'lsp-buffer-sessions) b)
      '()))

(define (lsp-sync! ls d)
  "Wait in this task for session LS to be ready, with document D as it is.
Return #f if it died first."
  ((lsp-rt 'lsp-sync!) ls d))

(define (lsp-position ls d pos)
  "Return position POS of document D as session LS's server counts it."
  ((lsp-rt 'lsp-position) ls d pos))

(define (lsp-range-positions ls d range #:revision [revision (document-revision d)])
  "Return RANGE of document D as of REVISION as (from to), mapped to now.
RANGE is an LSP range as session LS's server counts; #f when that
revision's text is no longer known."
  ((lsp-rt 'lsp-range-positions) ls d range #:revision revision))

(define (lsp-location ls loc)
  "Return the `location' an LSP Location or LocationLink LOC names, or #f.
Its file is opened (as a document) to read the position in, counted as
session LS's server counts."
  ((lsp-rt 'lsp-location) ls loc))

(define (lsp-diagnostics-at d pos)
  "Return the diagnostics of document D around POS.
Each is (from to severity message source raw), positions as of now,
RAW the diagnostic as the server said it."
  (if %lsp-runtime-table
      ((dict-ref %lsp-runtime-table 'lsp-diagnostics-at) d pos)
      '()))

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
  ((lsp-rt 'lsp-apply-workspace-edit!) ls edit revisions))

;;; Mode, mode-line, xref, and eldoc

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
  #:layer (lambda (d from to)
            (if %lsp-runtime-table
                ((dict-ref %lsp-runtime-table 'diagnostic-layer) d from to)
                '()))
  #:inlays (lambda (d from to)
             (if %lsp-runtime-table
                 ((dict-ref %lsp-runtime-table 'document-inlays) d from to)
                 '())))

(register-mode-line-segment! 'lsp
  (lambda (b)
    (and %lsp-runtime-table
         ((dict-ref %lsp-runtime-table 'lsp-mode-line-text) b))))

(define-xref-backend lsp
  "What the buffer's language servers know: definitions, references,
implementations, types, and the project's names."
  #:applies (lambda (s b) (and (option b 'lsp-mode) (pair? (lsp-buffer-sessions b))))
  #:find (lambda (s kind d pos pattern) ((lsp-rt 'xref-find) s kind d pos pattern))
  #:wait #t #:priority 10)

(define-eldoc-provider lsp-diagnostics
  "The diagnostics at point, as Flymake's."
  #:applies (lambda (b) (option b 'lsp-mode))
  #:say (lambda (s d pos full)
          (and %lsp-runtime-table
               ((dict-ref %lsp-runtime-table 'eldoc-diagnostics-say) s d pos full)))
  #:priority 20)

(define-eldoc-provider lsp-signature
  "The signature of the call point is in, its argument there marked."
  #:applies (lambda (b) (option b 'lsp-mode))
  #:say (lambda (s d pos full)
          (and %lsp-runtime-table
               ((dict-ref %lsp-runtime-table 'eldoc-signature-say) s d pos full)))
  #:priority 10)

(define-eldoc-provider lsp-hover
  "What the language server says of the name at point: its type,
signature and documentation."
  #:applies (lambda (b) (option b 'lsp-mode))
  #:say (lambda (s d pos full)
          (and %lsp-runtime-table
               ((dict-ref %lsp-runtime-table 'eldoc-hover-say) s d pos full)))
  #:markdown #t)

;;; Commands

(define-command (lsp s n)
  "Attach the focused buffer to the language servers serving it.
Each starts in the project's root unless it runs there already; its
diagnostics show under `lsp-mode'."
  ((lsp-rt 'run-lsp!) s n))

(define-command (lsp-events s n)
  "Show what went between the editor and a language server of this buffer.
As Eglot's events buffer: the messages each way, the JSON as it went,
and what the server wrote to stderr, followed as more come. With more
than one server (or none here, but others), which is asked."
  ((lsp-rt 'run-lsp-events!) s n))

(define-command (lsp-shutdown s n)
  "Shut down the language servers of the focused buffer.
Their buffers leave `lsp-mode'."
  ((lsp-rt 'run-lsp-shutdown!) s n))

(define-command (lsp-restart s n)
  "Restart the language servers of the focused buffer.
Each is shut down and started again, with every buffer it served. Where
they ended (crashed), they are started again for this buffer."
  ((lsp-rt 'run-lsp-restart!) s n))

(define-command (lsp-next-diagnostic s n)
  "Go to the next diagnostic of the buffer, and say what it is."
  ((lsp-rt 'run-lsp-next-diagnostic!) s n))

(define-command (lsp-previous-diagnostic s n)
  "Go to the previous diagnostic of the buffer, and say what it is."
  ((lsp-rt 'run-lsp-previous-diagnostic!) s n))

(define-command (lsp-diagnostics s n)
  "Go to a diagnostic of the buffer, previewing each while choosing.
As `consult-flymake': each says its severity, line and message."
  ((lsp-rt 'run-lsp-diagnostics!) s n))

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
           (name context #:value ,(lambda (s args) ((lsp-rt 'name-at-location) (dict-ref args 'at))))
           (new-name ,(lambda (s args k) ((lsp-rt 'read-new-name) s args k))))
  #:effects '(read query edit)
  #:stageable #t
  #:expose '(agents)
  ;; It renames later, when the server answers: its operation says how.
  #:result 'any
  ((lsp-rt 'run-lsp-rename!) s at new-name name))

(define-command (lsp-format-buffer s n)
  "Format the buffer as its language server does (rustfmt, for Rust)."
  ((lsp-rt 'run-lsp-format-buffer!) s n))

(define-command (lsp-format s n)
  "Format the region as the language server does, else the buffer.
Where the server formats only whole buffers, whether to format the
whole buffer is asked. Not for more than one region yet."
  ((lsp-rt 'run-lsp-format!) s n))

(define-command (lsp-code-actions s n)
  "Choose one of the language server's actions at point and do it.
They are its fixes for the diagnostics there and its refactorings of
the region, or of the thing at point."
  ((lsp-rt 'run-lsp-code-actions!) s n))

;;; Hooks

(add-option-changed-hook! 'lsp-config
  (lambda (opt-name)
    (when (and %lsp-runtime-table (eq? opt-name 'lsp-settings))
      (lsp-refresh-configuration!))))

(add-project-settings-changed-hook! 'lsp-config
  (lambda (dir)
    (when %lsp-runtime-table
      (lsp-refresh-configuration! dir))))

(add-hook! 'file-buffer-added 'lsp
           (lambda (s b)
             (when (and (option b 'lsp-auto)
                        (not (document-private? (buffer-document b)))
                        (pair? (servers-for b)))
               ((lsp-rt 'on-file-buffer-added) s b))))

(add-hook! 'buffer-killed 'lsp
           (lambda (s b)
             (when %lsp-runtime-table
               ((dict-ref %lsp-runtime-table 'on-buffer-killed) s b))))

(add-hook! 'document-saved 'lsp
           (lambda (s d)
             (when %lsp-runtime-table
               ((dict-ref %lsp-runtime-table 'on-document-saved) s d))))

(add-hook! 'document-moved 'lsp
           (lambda (s d)
             (when %lsp-runtime-table
               ((dict-ref %lsp-runtime-table 'on-document-moved) s d))))

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
