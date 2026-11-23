;;; A cloud of tenants' projects, served by a fleet (doc/serverless.md,
;;; "Architecture on one VPS"; crates/techne-host/src/fleet.rs): who is
;;; invited, what each has deployed, and the operations on it, kept in a
;;; state directory:
;;;
;;;   token-key                      the key tokens are made with
;;;   tenants/TENANT/                a tenant invited
;;;   tenants/TENANT/PROJECT/        a project: its record of deployments
;;;                                  and their bundles (control.scm),
;;;     secrets.scm                  its secrets, by name
;;;     source/                      the files it was last deployed from
;;;
;;; A tenant is invited by whoever runs the service (`cloud-invite!`),
;;; and given a token for all its projects; there is no signing up. A
;;; project is there once deployed to.
;;;
;;; `cloud-ops-handler` answers the operations, JSON over HTTP (`POST
;;; /v1/ops`), each naming its `tenant` and `project`, for a token that
;;; allows it on `tenant/project` (tokens.scm): `deploy`, with the
;;; project's `files` (by path, in base64) and `entry`; `deployments`;
;;; `rollback`, to the one before or to `id`; `secret-put`, a `name` and
;;; `value`, bound by the next deployment whose manifest names it. A
;;; deployment is as control.scm deploys it, from the files uploaded.
;;;
;;; `cloud-restore!` publishes again, in a supervisor started anew, what
;;; each project had published.

(require "control.scm")
(require "tokens.scm")
(require "auth.scm")
(require "hosts.scm")
(require "../http/http.scm")
(require "../http/debug-route.scm")
(import (techne crypto))

(provide make-cloud cloud? cloud-invite! cloud-restore! cloud-ops-handler cloud-front-handler cloud-debug-serve)

;; A tenant's projects, a deployment's files, the bytes of them, and the
;; directories a file may be under, at most.
(define max-projects 16)
(define max-files 1000)
(define max-bytes (* 16 1024 1024))
(define max-depth 8)
;; Per-tenant cached dependency trees on disk, at most.
(define max-tenant-trees 100)
(define max-tenant-tree-bytes (* 64 1024 1024))
;; Operations at once, of all tenants: each may hold an upload.
(define max-operations 8)

;; Debugging sessions at once, of all tenants, and of one project.
(define max-sessions 32)
(define max-sessions-each 4)

(define-record-type cloud
  (%make-cloud state key controls operations turn sessions)
  cloud?
  ;; A box of the debugging sessions there are: (target . credential)
  ;; each, a request showing the credential being its project's debug
  ;; instance's to serve.
  (sessions cloud-sessions)
  (state cloud-state)
  (key cloud-key)
  ;; Each project's control, by its target: one, so that its deploys
  ;; take turns.
  (controls cloud-controls set-cloud-controls!)
  ;; A box of the targets operated on now.
  (operations cloud-operations)
  ;; A channel holding one token: who has it adds a project.
  (turn cloud-turn))

;; Take a place for an operation on TARGET, if there is one and none is
;; operating on it: whether it was taken. `release!` gives it back.
(define (admit! cloud target)
  (let* ((box (cloud-operations cloud))
         (now (unbox box)))
    (and (< (length now) max-operations)
         (not (member target now))
         (or (box-cas! box now (cons target now))
             (admit! cloud target)))))

(define (release! cloud target)
  (let* ((box (cloud-operations cloud))
         (now (unbox box)))
    (unless (box-cas! box now (remove (lambda (t) (equal? t target)) now))
      (release! cloud target))))

;; Whether NAME is a valid project name ("trees" is reserved for the
;; tenant's content-addressed dependency tree cache).
(define (project-name? name)
  (and (cloud-name? name) (not (equal? name "trees"))))

;; Whether TENANT's project in DIRECTORY is there, made if it is not and
;; MAKE says so and the tenant has room for one: one at a time, so that
;; none is made past the most a tenant has.
(define (project-there? cloud tenant directory make)
  (channel-recv (cloud-turn cloud))
  (dynamic-wind
    (lambda () #f)
    (lambda ()
      (or (file-exists? directory)
          (and make
               (< (length (project-names-in (tenant-directory cloud tenant))) max-projects)
               (begin (create-directory directory #o700) #t))))
    (lambda () (channel-send (cloud-turn cloud) #t))))

(define (ensure-directory directory)
  (unless (file-exists? directory) (create-directory directory #o700)))

(define (tenant-trees-directory cloud tenant)
  (string-append (tenant-directory cloud tenant) "/trees"))

(define (tree-directory cloud tenant sha)
  (string-append (tenant-trees-directory cloud tenant) "/" sha))

(define (make-cloud state)
  "Return the cloud kept in the directory STATE, made if it is not there.
Its token key is the one kept there, made if none is."
  (ensure-directory state)
  (ensure-directory (string-append state "/tenants"))
  (let ((turn (make-channel 1)))
    (channel-send turn #t)
    (%make-cloud state (token-key-in state) (dict) (box '()) turn (box '()))))

;; The credentials that route to TARGET's debug instance now.
(define (credentials-of cloud target)
  (filter-map (lambda (s) (and (equal? (car s) target) (cdr s))) (unbox (cloud-sessions cloud))))

;; Change the sessions there are by CHANGE, a procedure of them; what it
;; returns #f for changes nothing, and is #f.
(define (change-sessions! cloud change)
  (let* ((box (cloud-sessions cloud))
         (now (unbox box))
         (next (change now)))
    (and next (or (box-cas! box now next) (change-sessions! cloud change)))))

(define (cloud-front-handler cloud domain serve)
  "Return an HTTP handler serving CLOUD's projects by host name, in DOMAIN.
(SERVE target request debug) answers REQUEST for TARGET,
`tenant/project`, as `host-routing` finds it, from its debug instance
if DEBUG: which is for a request whose `techne-debug-route` header is
the credential of a debugging session of that project there is now
(`cloud-debug-serve`), and no other. The header reaches no project."
  (host-routing domain
                (lambda (target request)
                  ((debug-routing (lambda (request debug) (serve target request debug))
                                  (lambda () (credentials-of cloud target)))
                   request))))

(define (cloud-debug-serve cloud #:revoked? [revoked? (lambda (id) #f)])
  "Open and close CLOUD's debugging sessions as asked, for ever.
Asked (\"open\" token target credential): a session of the project
TARGET, for a token that holds and allows `prod` on it, of a tenant
invited, answered with whom the token is for; CREDENTIAL, which who
asks made, 32 hexadecimal digits no one can guess,
routes requests to the project's debug instance while the session
lasts; refused otherwise (a token REVOKED? says is revoked does not
hold, as `verify-token` has it), and past the sessions there may be. Asked
(\"close\" target credential): that session is over."
  (let loop ()
    (let* ((asked (%debug-next))
           (id (car asked))
           (what (cadr asked)))
      (guard (e (#t (%debug-reply id #f "refused")))
        (cond
         ((and (list? what) (= (length what) 4) (equal? (car what) "open"))
          (let* ((target (caddr what))
                 (claims (verify-token (cloud-key cloud) (cadr what) #:revoked? revoked?))
                 (named (and (string? target) (host-target (string-append (target-host target) ".x") "x")))
                 (credential (cadddr what)))
            (if (and claims named (equal? named target)
                     (not (string-suffix? "/trees" target))
                     (string? credential) (= (string-length credential) 32)
                     (guard (e (#t #f)) (= (bytevector-length (hex-decode credential)) 16))
                     (token-allows? claims target "prod")
                     (file-exists? (string-append (cloud-state cloud) "/tenants/" target))
                     (change-sessions! cloud
                                       (lambda (now)
                                         (and (< (length now) max-sessions)
                                              (< (length (filter (lambda (s) (equal? (car s) target)) now)) max-sessions-each)
                                              (cons (cons target credential) now)))))
                (%debug-reply id #t (dict-ref claims "sub"))
                (%debug-reply id #f "refused"))))
         ((and (list? what) (= (length what) 3) (equal? (car what) "close"))
          (change-sessions! cloud (lambda (now) (remove (lambda (s) (equal? s (cons (cadr what) (caddr what)))) now)))
          (%debug-reply id #t #t))
         (else (%debug-reply id #f "refused"))))
      (loop))))

(define (tenant-directory cloud tenant)
  (string-append (cloud-state cloud) "/tenants/" tenant))

(define (project-directory cloud tenant project)
  (string-append (tenant-directory cloud tenant) "/" project))

(define (invited? cloud tenant)
  (and (cloud-name? tenant) (file-exists? (tenant-directory cloud tenant))))

(define (cloud-invite! cloud tenant #:days [days 90])
  "Invite TENANT to CLOUD, and return a token for all its projects.
The token allows everything on them for DAYS days. A tenant invited
before is given another token."
  (unless (cloud-name? tenant) (error "Not a tenant's name:" tenant))
  (ensure-directory (tenant-directory cloud tenant))
  (make-token (cloud-key cloud) tenant
              #:projects (list (string-append tenant "/*"))
              #:scopes token-scopes
              #:expires (+ (current-second) (* days 24 3600))))

;; The names in DIRECTORY that name tenants or projects.
(define (names-in directory)
  (if (file-exists? directory)
      (filter cloud-name? (map (lambda (entry) (if (string-suffix? "/" entry) (substring entry 0 (- (string-length entry) 1)) entry))
                               (directory-list directory)))
      '()))

;; The project names in a tenant's DIRECTORY (excluding the reserved "trees" cache).
(define (project-names-in directory)
  (filter project-name? (names-in directory)))

;; The control of TENANT's PROJECT, the one there is of it.
(define (control-of cloud tenant project)
  (let ((target (string-append tenant "/" project)))
    (or (dict-ref (cloud-controls cloud) target #f)
        (let* ((directory (project-directory cloud tenant project))
               (c (make-control target (string-append directory "/source/app.scm") directory)))
          (set-cloud-controls! cloud (dict-set (cloud-controls cloud) target c))
          c))))

(define (cloud-restore! cloud)
  "Publish again what each project of CLOUD had published.
For a fleet started anew. What cannot be is said on the error port, and
the others go on. The targets published, `tenant/project`."
  (append-map
   (lambda (tenant)
     (%fleet-tenant tenant)
     (filter-map
      (lambda (project)
        (guard (e (#t (let ((port (current-error-port)))
                        (display (string-append "techne cloud: " tenant "/" project " is not restored: "
                                                (if (error-object? e) (condition/report-string e) "it failed") "\n")
                                 port)
                        #f)))
          (and (control-restore! (control-of cloud tenant project))
               (string-append tenant "/" project))))
      (project-names-in (tenant-directory cloud tenant))))
   (names-in (string-append (cloud-state cloud) "/tenants"))))

;; Whether PATH names a file of a deployment: relative, its parts plain
;; names, none hidden, not too deep.
(define (file-path? path)
  (and (string? path)
       (<= 1 (string-length path) 512)
       (let loop ((parts (string-split path #\/)) (depth 0))
         (or (null? parts)
             (let ((part (car parts)))
               (and (<= depth max-depth)
                    (<= 1 (string-length part) 128)
                    (not (char=? (string-ref part 0) #\.))
                    (let chars ((i 0))
                      (or (= i (string-length part))
                          (let ((ch (string-ref part i)))
                            (and (or (char<=? #\a ch #\z) (char<=? #\A ch #\Z) (char<=? #\0 ch #\9) (memv ch '(#\. #\_ #\-)))
                                 (chars (+ i 1))))))
                    (loop (cdr parts) (+ depth 1))))))))

(define (string-split s ch)
  (let loop ((i 0) (start 0) (parts '()))
    (cond ((= i (string-length s)) (reverse (cons (substring s start i) parts)))
          ((char=? (string-ref s i) ch) (loop (+ i 1) (+ i 1) (cons (substring s start i) parts)))
          (else (loop (+ i 1) start parts)))))

(define (write-bytes file bytes)
  (call-with-port (open-binary-output-file file) (lambda (port) (write-bytevector bytes port))))

;; Make the directories PATH, a file's under ROOT, is in.
(define (ensure-parents root path)
  (let loop ((parts (string-split path #\/)) (directory root))
    (when (pair? (cdr parts))
      (let ((next (string-append directory "/" (car parts))))
        (ensure-directory next)
        (loop (cdr parts) next)))))

(define (secrets-of directory)
  (let ((file (string-append directory "/secrets.scm")))
    (if (file-exists? file)
        (let ((kept (call-with-input-file file read)))
          (if (dict? kept) kept (error "The project's secrets cannot be read")))
        (dict))))

;; Whether S is a canonical 64-character lowercase hexadecimal SHA-256 digest.
(define (sha256-hex? s)
  (and (string? s)
       (= (string-length s) 64)
       (let loop ((i 0))
         (or (= i 64)
             (let ((ch (string-ref s i)))
               (and (or (char<=? #\0 ch #\9) (char<=? #\a ch #\f)) (loop (+ i 1))))))))

;; Canonical tree-sha256 of FILES (a dict of relative path -> bytevector),
;; matching `directory-tree-sha256` in `(techne package)` 1:1.
(define (files-tree-sha256 files)
  (let ((paths (sort (dict-keys files) string<?))
        (out (open-output-bytevector)))
    (for-each
     (lambda (rel)
       (let ((bytes (dict-ref files rel)))
         (write-bytevector (string->utf8 "F ") out)
         (write-bytevector (string->utf8 rel) out)
         (write-u8 0 out)
         (write-bytevector (string->utf8 (number->string (bytevector-length bytes))) out)
         (write-u8 0 out)
         (write-bytevector bytes out)
         (write-u8 10 out)))
     paths)
    (hex-encode (sha-256 (get-output-bytevector out)))))

;; Read all bytes of FILE, up to max-bytes.
(define (read-file-bytes file)
  (call-with-port (open-binary-input-file file)
    (lambda (port)
      (let loop ((chunks '()) (total 0))
        (let ((chunk (read-bytevector 65536 port)))
          (cond ((eof-object? chunk) (apply bytevector-append (reverse chunks)))
                ((> (+ total (bytevector-length chunk)) max-bytes) (error "File is too large:" file))
                (else (loop (cons chunk chunks) (+ total (bytevector-length chunk))))))))))

;; Put FILES, bytes by path, where the project in DIRECTORY is deployed
;; from, in place of what was there, with its secrets beside them.
(define (stage! directory files)
  (let ((source (string-append directory "/source")))
    (when (file-exists? source) (delete-path source #t))
    (ensure-directory source)
    (for-each (lambda (path)
                (ensure-parents source path)
                (write-bytes (string-append source "/" path) (dict-ref files path)))
              (dict-keys files))
    (replace-file (string-append source "/.secrets.scm")
                  (call-with-output-string (lambda (port) (write (secrets-of directory) port))))))

(define (strip-slash name)
  (if (string-suffix? "/" name)
      (substring name 0 (- (string-length name) 1))
      name))

(define (quarantine-tree! dir)
  (guard (e (#t #f))
    (when (file-exists? dir) (delete-path dir #t))))

;; Walk a cached tree directory DIR without following symlinks, enforcing
;; max-depth, max-files, max-bytes, and file-path? rules.
;; Returns (list files-dict file-count byte-count).
(define (read-tree-files-bounded dir)
  (let ((root-attrs (file-attributes dir)))
    (unless (and root-attrs (eq? (car root-attrs) 'directory))
      (error "Cached tree is not a directory:" dir))
    (let ((files (dict))
          (file-count 0)
          (byte-count 0))
      (let walk ((cur dir) (prefix "") (depth 0))
        (when (> depth max-depth)
          (error "Cached tree exceeds max depth:" dir))
        (for-each
         (lambda (raw)
           (let* ((name (strip-slash raw))
                  (rel (if (string=? prefix "") name (string-append prefix "/" name)))
                  (full (string-append cur "/" name))
                  (attrs (file-attributes full)))
             (unless (and attrs (file-path? rel))
               (error "Invalid entry in cached tree:" rel))
             (cond
              ((eq? (car attrs) 'directory)
               (walk full rel (+ depth 1)))
              ((eq? (car attrs) 'file)
               (set! file-count (+ file-count 1))
               (when (> file-count max-files)
                 (error "Cached tree exceeds max files:" dir))
               (let ((bytes (read-file-bytes full)))
                 (set! byte-count (+ byte-count (bytevector-length bytes)))
                 (when (> byte-count max-bytes)
                   (error "Cached tree exceeds max bytes:" dir))
                 (set! files (dict-set files rel bytes))))
              (else
               (error "Non-regular entry in cached tree:" rel)))))
         (directory-list cur)))
      (list files file-count byte-count))))

;; Verify that TENANT's cached tree SHA exists on disk and matches SHA.
;; Returns #f if missing; deletes (quarantines) and returns #f if corrupted
;; or unreadable; returns (file-count . byte-count) if valid.
(define (verify-cached-tree! cloud tenant sha)
  (let ((dir (tree-directory cloud tenant sha)))
    (and (file-exists? dir)
         (guard (e (#t (quarantine-tree! dir) #f))
           (let* ((res (read-tree-files-bounded dir))
                  (files (car res))
                  (count (cadr res))
                  (bytes (caddr res)))
             (if (equal? (files-tree-sha256 files) sha)
                 (cons count bytes)
                 (begin (quarantine-tree! dir) #f)))))))

(define (tree-disk-bytes dir)
  (guard (e (#t 0))
    (let walk ((cur dir) (acc 0))
      (fold (lambda (raw total)
              (let* ((name (strip-slash raw))
                     (full (string-append cur "/" name))
                     (attrs (file-attributes full)))
                (cond ((not attrs) total)
                      ((eq? (car attrs) 'directory) (walk full total))
                      ((eq? (car attrs) 'file) (+ total (cadr attrs)))
                      (else total))))
            acc
            (directory-list cur)))))

;; Prune oldest cached dependency trees in TENANT's tree cache if the tenant
;; exceeds max-tenant-trees or max-tenant-tree-bytes, never evicting SHAs in
;; KEEP-SHAS.
(define (prune-tenant-trees! cloud tenant keep-shas)
  (let ((tdir (tenant-trees-directory cloud tenant)))
    (when (file-exists? tdir)
      (let* ((entries
              (filter-map
               (lambda (raw)
                 (let ((name (strip-slash raw)))
                   (and (sha256-hex? name)
                        (let* ((path (string-append tdir "/" name))
                               (attrs (guard (e (#t #f)) (file-attributes path))))
                          (and attrs (eq? (car attrs) 'directory)
                               (list name path (caddr attrs) (tree-disk-bytes path)))))))
               (guard (e (#t '())) (directory-list tdir))))
             (sorted (sort entries (lambda (a b) (< (caddr a) (caddr b)))))
             (count (length sorted))
             (total-bytes (fold (lambda (e acc) (+ acc (cadddr e))) 0 sorted)))
        (let loop ((rest sorted) (n count) (bytes total-bytes))
          (when (and (pair? rest)
                     (or (> n max-tenant-trees) (> bytes max-tenant-tree-bytes)))
            (let ((e (car rest)))
              (if (member (car e) keep-shas)
                  (loop (cdr rest) n bytes)
                  (begin
                    (guard (ex (#t #f)) (delete-path (cadr e) #t))
                    (loop (cdr rest) (- n 1) (- bytes (cadddr e))))))))))))

(define (write-tree-files! dir files)
  (create-directory dir #o700)
  (for-each (lambda (path)
              (ensure-parents dir path)
              (write-bytes (string-append dir "/" path) (dict-ref files path)))
            (dict-keys files)))

;; Atomically cache verified FILES under `<state>/tenants/<tenant>/trees/<sha>/`
;; if not present.
(define (cache-tree! cloud tenant sha files)
  (let ((dest (tree-directory cloud tenant sha)))
    (unless (file-exists? dest)
      (ensure-directory (tenant-trees-directory cloud tenant))
      (let ((tmp (string-append (tenant-trees-directory cloud tenant) "/.tmp-" sha "-" (hex-encode (random-bytes 8)))))
        (guard (e (#t (when (file-exists? tmp) (delete-path tmp #t))
                      (unless (file-exists? dest) (raise e))))
          (write-tree-files! tmp files)
          (rename-file tmp dest))))))

;; Stage a packaged deployment in an ephemeral STAGE-DIR without mutating
;; `<tenant>/trees/` or `<project>/source/`.
(define (stage-packaged-ephemeral! cloud tenant directory stage-dir root-pair packages)
  (let ((stage-source (string-append stage-dir "/source"))
        (stage-trees (string-append stage-dir "/trees")))
    (create-directory stage-dir #o700)
    (create-directory stage-source #o700)
    (create-directory stage-trees #o700)
    (for-each (lambda (pkg)
                (let ((sha (caddr pkg))
                      (files (list-ref pkg 4)))
                  (when files
                    (let ((dest (string-append stage-trees "/" sha)))
                      (unless (file-exists? dest)
                        (write-tree-files! dest files))))))
              packages)
    (let* ((root-pkg (find (lambda (p) (and (equal? (car p) (car root-pair)) (equal? (cadr p) (cadr root-pair)))) packages))
           (root-sha (and root-pkg (caddr root-pkg)))
           (root-files (and root-pkg (list-ref root-pkg 4))))
      (unless root-sha (error "Root package missing from deployment packages"))
      (let ((manifest-bytes
             (cond ((and root-files (dict-ref root-files "manifest.scm" #f))
                    => (lambda (b) b))
                   (else
                    (let ((cached (string-append (tree-directory cloud tenant root-sha) "/manifest.scm")))
                      (and (file-exists? cached) (read-file-bytes cached)))))))
        (when manifest-bytes
          (write-bytes (string-append stage-source "/manifest.scm") manifest-bytes)))
      (replace-file (string-append stage-source "/.secrets.scm")
                    (call-with-output-string (lambda (port) (write (secrets-of directory) port)))))))

;; Persist verified non-root dependency trees into `<tenant>/trees/` and
;; update `<project>/source/` (`manifest.scm` + `.secrets.scm`) ONLY AFTER
;; validation and deployment succeed. Root package trees are never cached.
(define (commit-packaged-trees! cloud tenant directory stage-source root-pair packages)
  (let ((non-root
         (filter (lambda (p)
                   (not (and (equal? (car p) (car root-pair))
                             (equal? (cadr p) (cadr root-pair)))))
                 packages)))
    (for-each (lambda (pkg)
                (let ((sha (caddr pkg))
                      (files (list-ref pkg 4)))
                  (when files (cache-tree! cloud tenant sha files))))
              non-root)
    (prune-tenant-trees! cloud tenant (map caddr non-root))
    (let ((source (string-append directory "/source"))
          (staged-manifest (string-append stage-source "/manifest.scm")))
      (when (file-exists? source) (delete-path source #t))
      (ensure-directory source)
      (when (file-exists? staged-manifest)
        (write-bytes (string-append source "/manifest.scm") (read-file-bytes staged-manifest)))
      (replace-file (string-append source "/.secrets.scm")
                    (call-with-output-string (lambda (port) (write (secrets-of directory) port)))))))

;; Decode a dict of relative-path -> base64 string, enforcing per-package bounds.
(define (decode-files files)
  (unless (and (dict? files) (<= 1 (dict-size files) max-files)) (error "A deployment has 1 to 1000 files"))
  (let ((decoded (fold (lambda (path decoded)
                         (unless (file-path? path) (error "Not a path of a deployment's file:" path))
                         (let ((text (dict-ref files path)))
                           (unless (string? text) (error "A file is its bytes in base64:" path))
                           (dict-set decoded path (base64-decode text))))
                       (dict) (dict-keys files))))
    (when (> (fold (lambda (path total) (+ total (bytevector-length (dict-ref decoded path)))) 0 (dict-keys decoded)) max-bytes)
      (error "A deployment's files are 16 MiB at most"))
    decoded))

;; The files MESSAGE, a legacy `deploy`, uploads, decoded: bytes by path.
(define (uploaded message)
  (decode-files (dict-ref message "files" #f)))

(define (package-name-str? s)
  (and (string? s) (<= 1 (string-length s) 128)))

(define (parse-package-pair v)
  (unless (and (vector? v) (= (vector-length v) 2)
               (package-name-str? (vector-ref v 0))
               (package-name-str? (vector-ref v 1)))
    (error "A package name is a 2-element array [owner, pkg]"))
  (list (vector-ref v 0) (vector-ref v 1)))

(define (check-packaged-deploy cloud tenant entry pkg-obj)
  (unless (file-path? entry) (error "Not a path of a deployment's entry:" entry))
  (unless (dict? pkg-obj) (error "A packaged deploy's package field is an object"))
  (let ((root-pair (parse-package-pair (dict-ref pkg-obj "root" #f)))
        (raw-pkgs (dict-ref pkg-obj "packages" #f))
        (total-files 0)
        (total-bytes 0))
    (unless (and (vector? raw-pkgs) (<= 1 (vector-length raw-pkgs) 128))
      (error "A packaged deployment has 1 to 128 packages"))
    (let ((parsed
           (map (lambda (p)
                  (unless (dict? p) (error "Each package entry is an object"))
                  (let ((owner (dict-ref p "owner" #f))
                        (pkg (dict-ref p "pkg" #f))
                        (sha (dict-ref p "tree_sha256" #f))
                        (raw-deps (dict-ref p "depends" (vector)))
                        (raw-files (dict-ref p "files" #f)))
                    (unless (and (package-name-str? owner) (package-name-str? pkg))
                      (error "A package has owner and pkg strings"))
                    (unless (sha256-hex? sha) (error "Not a package tree_sha256:" sha))
                    (unless (and (vector? raw-deps) (<= (vector-length raw-deps) 128))
                      (error "A package's depends is an array"))
                    (let ((deps (map parse-package-pair (vector->list raw-deps)))
                          (decoded (and raw-files (decode-files raw-files))))
                      (if decoded
                          (begin
                            (set! total-files (+ total-files (dict-size decoded)))
                            (set! total-bytes (+ total-bytes
                                                 (fold (lambda (k acc) (+ acc (bytevector-length (dict-ref decoded k))))
                                                       0 (dict-keys decoded))))
                            (when (> total-files max-files) (error "A deployment has 1000 files at most"))
                            (when (> total-bytes max-bytes) (error "A deployment's files are 16 MiB at most"))
                            (let ((actual (files-tree-sha256 decoded)))
                              (unless (equal? actual sha)
                                (error (string-append "Package tree-sha256 mismatch for (" owner " " pkg "): expected " sha ", got " actual)))))
                          (let ((cached-stats (verify-cached-tree! cloud tenant sha)))
                            (unless cached-stats
                              (error "Missing cached package tree:" sha))
                            (set! total-files (+ total-files (car cached-stats)))
                            (set! total-bytes (+ total-bytes (cdr cached-stats)))
                            (when (> total-files max-files) (error "A deployment has 1000 files at most"))
                            (when (> total-bytes max-bytes) (error "A deployment's files are 16 MiB at most"))))
                      (list owner pkg sha deps decoded))))
                (vector->list raw-pkgs))))
      (unless (any (lambda (p) (and (equal? (car p) (car root-pair)) (equal? (cadr p) (cadr root-pair)))) parsed)
        (error "Root package is missing from packages"))
      (list 'packaged entry root-pair parsed))))

(define (report e)
  (if (error-object? e) (condition/report-string e) "it failed"))

;; What MESSAGE, an operation OP, is to do, checked: a `deploy`'s entry
;; and files (or packaged descriptor), `has-trees`'s list of digests, a
;; `secret-put`'s name and value; #t for another. An error for what is none.
(define (check-operation cloud tenant op message)
  (cond ((equal? op "has-trees")
         (let ((trees (dict-ref message "trees" #f)))
           (unless (and (vector? trees) (<= (vector-length trees) 128))
             (error "trees is an array of up to 128 sha256 digests"))
           (let ((list-trees (vector->list trees)))
             (for-each (lambda (sha) (unless (sha256-hex? sha) (error "Not a sha256 digest:" sha))) list-trees)
             list-trees)))
        ((equal? op "deploy")
         (let ((entry (dict-ref message "entry" #f))
               (pkg-obj (dict-ref message "package" #f)))
           (if pkg-obj
               (check-packaged-deploy cloud tenant entry pkg-obj)
               (let ((files (uploaded message)))
                 (unless (and (file-path? entry) (dict-ref files entry #f)) (error "The entry is one of the files:" entry))
                 ;; Its entry at the top, as a directory deployed is.
                 (when (member #\/ (string->list entry)) (error "The entry is at the top of the files:" entry))
                 (list 'legacy entry files)))))
        ((equal? op "secret-put")
         (let ((name (dict-ref message "name" #f)) (value (dict-ref message "value" #f)))
           (unless (and (string? name) (<= 1 (string-length name) 128) (string? value) (<= (string-length value) 65536))
             (error "A secret is a name and a value, strings"))
           (cons name value)))
        (else #t)))

(define (bearer request)
  (let ((given (request-header request "authorization")))
    (and given (string-prefix? "Bearer " given) (substring given 7 (string-length given)))))

(define (dict->fields d)
  (append-map (lambda (key) (list key (dict-ref d key))) (dict-keys d)))

(define (cloud-ops-handler cloud #:revoked? [revoked? (lambda (id) #f)])
  "Return an HTTP handler answering the operations on CLOUD's projects.
`POST /v1/ops` with a JSON object of its `op`, `tenant` and `project`,
and the operation's own fields, as this file says them. Answered with a
JSON array of one reply, its `status` done, or error and what kind, with
a `message`. The request's bearer token must hold (REVOKED? as
`verify-token` has it) and allow the operation on `tenant/project`:
`inspect` to list deployments, `deploy` for the others. A tenant not
invited has no projects, whatever token is shown.

Serve it with bodies that stream: a request's body is read only once
its token holds, the project it names in its `techne-project` header
(`tenant/project`, which its body must then say too) is its token's,
and it has a place among the few operations at once, one a project; one
without is answered 503, its body unread."
  (define (reply status statuses . fields)
    (json-response (vector (apply dict "status" (list->vector statuses) fields)) #:status status))
  (define (operate request claims target)
    (guard (e (#t (reply 422 '("error" "failed" "done") "message" (report e))))
      (let* ((message (guard (e (#t #f)) (request-json request)))
             (message (and (dict? message) message))
             (op (and message (dict-ref message "op" #f)))
             (tenant (and message (dict-ref message "tenant" #f)))
             (project (and message (dict-ref message "project" #f)))
             (scope (cond ((equal? op "deployments") "inspect")
                          ((member op '("deploy" "rollback" "secret-put" "has-trees")) "deploy")
                          (else #f))))
        (cond ((not message) (reply 400 '("error" "not-an-operation" "done")))
              ((not scope) (reply 400 '("error" "unknown-op" "done")))
              ((not (and (cloud-name? tenant) (project-name? project))) (reply 400 '("error" "not-a-project" "done")))
              ;; Refused alike for a tenant not invited and one that is
              ;; another's: neither says which there are.
              ((not (and (equal? target (string-append tenant "/" project))
                         (token-allows? claims target scope)
                         (invited? cloud tenant)))
               (reply 403 '("error" "refused" "done")))
              (else
               (%fleet-tenant tenant)
               ;; Checked first: one that is no operation makes no project.
               (let ((directory (project-directory cloud tenant project))
                     (checked (check-operation cloud tenant op message)))
                 (cond
                  ((equal? op "has-trees")
                   (let loop ((shas checked) (have '()) (missing '()))
                     (if (null? shas)
                         (reply 200 '("done") "have" (list->vector (reverse have)) "missing" (list->vector (reverse missing)))
                         (let ((sha (car shas)))
                           (if (verify-cached-tree! cloud tenant sha)
                               (loop (cdr shas) (cons sha have) missing)
                               (loop (cdr shas) have (cons sha missing)))))))
                  ((not (project-there? cloud tenant directory (member op '("deploy" "secret-put"))))
                   (reply 404 '("error" "no-such-project" "done")))
                  ((equal? op "deployments")
                   (apply reply 200 '("done") (dict->fields (control-deployments (control-of cloud tenant project)))))
                  ((equal? op "rollback")
                   (let ((id (dict-ref message "id" #f)))
                     (unless (or (not id) (string? id)) (error "An id is a string:" id))
                     (apply reply 200 '("done") (dict->fields (control-rollback! (control-of cloud tenant project) id)))))
                  ((equal? op "secret-put")
                   (let ((name (car checked)) (value (cdr checked)))
                     (let ((secrets (secrets-of directory)))
                       (unless (or (dict-ref secrets name #f) (< (dict-size secrets) 128)) (error "A project has 128 secrets at most"))
                       (replace-file (string-append directory "/secrets.scm")
                                     (call-with-output-string (lambda (port) (write (dict-set secrets name value) port)))))
                     (reply 200 '("done"))))
                  ((eq? (car checked) 'packaged)
                   (let* ((entry (cadr checked))
                          (root-pair (caddr checked))
                          (packages (cadddr checked))
                          (specs (map (lambda (p) (list (car p) (cadr p) (caddr p) (cadddr p))) packages))
                          (stage-dir (string-append directory "/.stage-" (hex-encode (random-bytes 8))))
                          (stage-source (string-append stage-dir "/source"))
                          (stage-trees (string-append stage-dir "/trees"))
                          (tenant-trees (tenant-trees-directory cloud tenant)))
                     (dynamic-wind
                       (lambda () #f)
                       (lambda ()
                         (let ((result
                                (control-deploy!
                                 (control-of cloud tenant project)
                                 #:stage (lambda () (stage-packaged-ephemeral! cloud tenant directory stage-dir root-pair packages))
                                 #:prepare (lambda ()
                                             (%cloud-prepare-packaged
                                              (string-append tenant "/" project)
                                              stage-source
                                              stage-trees
                                              tenant-trees
                                              entry
                                              root-pair
                                              specs)))))
                           (commit-packaged-trees! cloud tenant directory stage-source root-pair packages)
                           (apply reply 200 '("done") (dict->fields result))))
                       (lambda ()
                         (when (file-exists? stage-dir) (delete-path stage-dir #t))))))
                  (else
                   (let ((entry (cadr checked))
                         (files (caddr checked)))
                     (apply reply 200 '("done")
                            (dict->fields (control-deploy! (control-of cloud tenant project)
                                                           #:stage (lambda () (stage! directory files))
                                                           #:prepare (lambda ()
                                                                       (%cloud-prepare-legacy-directory
                                                                        (string-append tenant "/" project)
                                                                        (string-append directory "/source/" entry)))))))))))))))
  ;; Answer a request refused from its head: a little of its body is
  ;; read and dropped first, so that one that small is answered rather
  ;; than cut off; more is not read, and its connection closed.
  (define (refuse request status . statuses)
    (let ((body (dict-ref request 'body #f)))
      (when (procedure? body)
        (let loop ((read 0))
          (when (< read 65536)
            (let ((piece (body)))
              (unless (eof-object? piece) (loop (+ read (bytevector-length piece)))))))))
    (reply status statuses))
  (lambda (request)
    ;; From its head alone, before any of its body is kept.
    (let* ((claims (verify-token (cloud-key cloud) (bearer request) #:revoked? revoked?))
           (target (request-header request "techne-project"))
           (named (and (string? target) (host-target (string-append (target-host target) ".x") "x"))))
      (cond ((not claims) (refuse request 401 "error" "unauthenticated" "done"))
            ((not (and (equal? (dict-ref request 'method) "POST") (equal? (dict-ref request 'path) "/v1/ops")))
             (refuse request 400 "error" "not-an-operation" "done"))
            ((not (and named (equal? named target) (not (string-suffix? "/trees" target))))
             (refuse request 400 "error" "not-a-project" "done"))
            ((not (or (token-allows? claims target "inspect") (token-allows? claims target "deploy")))
             (refuse request 403 "error" "refused" "done"))
            ((not (admit! cloud target)) (refuse request 503 "error" "busy" "done"))
            (else
             (dynamic-wind
               (lambda () #f)
               (lambda () (operate request claims target))
               (lambda () (release! cloud target))))))))

;; TARGET, `tenant/project`, as the host that names it: `project.tenant`.
(define (target-host target)
  (let loop ((i 0))
    (cond ((= i (string-length target)) target)
          ((char=? (string-ref target i) #\/)
           (string-append (substring target (+ i 1) (string-length target)) "." (substring target 0 i)))
          (else (loop (+ i 1))))))
