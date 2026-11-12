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
               (< (length (names-in (tenant-directory cloud tenant))) max-projects)
               (begin (create-directory directory #o700) #t))))
    (lambda () (channel-send (cloud-turn cloud) #t))))

(define (ensure-directory directory)
  (unless (file-exists? directory) (create-directory directory #o700)))

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
      (names-in (tenant-directory cloud tenant))))
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

;; The files MESSAGE, a `deploy`, uploads, decoded: bytes by path.
(define (uploaded message)
  (let ((files (dict-ref message "files" #f)))
    (unless (and (dict? files) (<= 1 (dict-size files) max-files)) (error "A deployment has 1 to 1000 files"))
    (let ((decoded (fold (lambda (path decoded)
                           (unless (file-path? path) (error "Not a path of a deployment's file:" path))
                           (let ((text (dict-ref files path)))
                             (unless (string? text) (error "A file is its bytes in base64:" path))
                             (dict-set decoded path (base64-decode text))))
                         (dict) (dict-keys files))))
      (when (> (fold (lambda (path total) (+ total (bytevector-length (dict-ref decoded path)))) 0 (dict-keys decoded)) max-bytes)
        (error "A deployment's files are 16 MiB at most"))
      decoded)))

(define (report e)
  (if (error-object? e) (condition/report-string e) "it failed"))

;; What MESSAGE, an operation OP, is to do, checked: a `deploy`'s entry
;; and files, a `secret-put`'s name and value, each a pair; #t for
;; another. An error for what is none.
(define (check-operation op message)
  (cond ((equal? op "deploy")
         (let ((entry (dict-ref message "entry" #f))
               (files (uploaded message)))
           (unless (and (file-path? entry) (dict-ref files entry #f)) (error "The entry is one of the files:" entry))
           ;; Its entry at the top, as a directory deployed is.
           (when (member #\/ (string->list entry)) (error "The entry is at the top of the files:" entry))
           (cons entry files)))
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
                          ((member op '("deploy" "rollback" "secret-put")) "deploy")
                          (else #f))))
        (cond ((not message) (reply 400 '("error" "not-an-operation" "done")))
              ((not scope) (reply 400 '("error" "unknown-op" "done")))
              ((not (and (cloud-name? tenant) (cloud-name? project))) (reply 400 '("error" "not-a-project" "done")))
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
                     (checked (check-operation op message)))
                 (cond
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
                  (else
                   (let ((entry (car checked))
                         (files (cdr checked)))
                     (apply reply 200 '("done")
                            (dict->fields (control-deploy! (control-of cloud tenant project)
                                                           #:stage (lambda () (stage! directory files))
                                                           #:entry (string-append directory "/source/" entry)))))))))))))
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
            ((not (and named (equal? named target))) (refuse request 400 "error" "not-a-project" "done"))
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
