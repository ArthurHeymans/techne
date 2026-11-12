;;; The control of one project's deployments (doc/serverless.md,
;;; "Deploying"): what is deployed, published and rolled back, and the
;;; record of it, which outlives the supervisor. The supervisor has the
;;; mechanism (`%cloud-prepare`, `%cloud-publish`, `%cloud-route`,
;;; `%cloud-bundle`, `%cloud-prepare-bundle`:
;;; crates/techne-host/src/supervisor.rs); what is done with it is here.
;;;
;;; To deploy is to prepare the deployment as its files and manifest are
;;; now, keep its bundle, and publish it if the project is still routed
;;; where this saw it: a deployment that does not load, or one overtaken
;;; by another, changes nothing. To roll back is to prepare a deployment
;;; again from the bundle kept of it, bindings and all, and publish it
;;; the same way: its code and bindings come back, what it kept in
;;; storage meanwhile stays.
;;;
;;; The record is kept in a directory: `deployments.scm`, the ids
;;; published and when, the latest first, and `bundles/`, a file for each
;;; of the latest few, by id. A bundle holds the secrets its deployment
;;; was bound to: the directory is as secret as they are.
;;;
;;; `control-serve` does what the supervisor's own side asks (a deploy
;;; when files change); `control-ops-handler` answers the operations
;;; `deploy`, `deployments` and `rollback` as JSON over HTTP (`POST
;;; /v1/ops`), for a token that allows them on the project
;;; (`tokens.scm`): `deploy` to deploy and roll back, `inspect` to list.

(require "tokens.scm")
(require "../http/http.scm")

(provide make-control control? control-name control-deploy! control-rollback! control-restore! control-deployments
         control-serve control-ops-handler)

;; How many deployments' bundles are kept, and events recorded; the most
;; bytes a bundle kept may be (a deployment's files, and their encoding).
(define kept-bundles 10)
(define kept-events 200)
(define max-bundle-bytes (* 32 1024 1024))

(define-record-type control
  (%make-control name entry directory turn)
  control?
  (name control-name)
  (entry control-entry)
  (directory control-directory)
  ;; A channel holding one token: who has it deploys or rolls back, one
  ;; at a time, so that what is published, recorded and kept agree.
  (turn control-turn))

(define (make-control name entry directory)
  "Return the control of the project NAME, kept in DIRECTORY.
Its deployments are of the entry module ENTRY, a file, and the files and
manifest beside it. DIRECTORY is made when something is first kept, in a
directory that is there."
  (let ((turn (make-channel 1)))
    (channel-send turn #t)
    (%make-control name entry directory turn)))

;; Call THUNK when no other deploy or rollback of C runs, as its turn.
(define (in-turn c thunk)
  (channel-recv (control-turn c))
  (dynamic-wind
    (lambda () #f)
    thunk
    (lambda () (channel-send (control-turn c) #t))))

(define (path-directory path)
  (let loop ((i (- (string-length path) 1)))
    (cond ((< i 0) ".")
          ((char=? (string-ref path i) #\/) (if (= i 0) "/" (substring path 0 i)))
          (else (loop (- i 1))))))

(define (ensure-directory directory)
  (unless (file-exists? directory) (create-directory directory #o700)))

(define (history-file c) (string-append (control-directory c) "/deployments.scm"))
(define (bundles-directory c) (string-append (control-directory c) "/bundles"))

;; An id is a digest in hexadecimal: it names a file.
(define (deployment-id? id)
  (and (string? id)
       (= (string-length id) 64)
       (let loop ((i 0))
         (or (= i 64)
             (let ((ch (string-ref id i)))
               (and (or (char<=? #\0 ch #\9) (char<=? #\a ch #\f)) (loop (+ i 1))))))))

(define (bundle-file c id)
  (unless (deployment-id? id) (error "Not a deployment's id:" id))
  (string-append (bundles-directory c) "/" id ".bundle"))

;; The events recorded, the latest first: (id seconds) each. A record
;; that cannot be read, or is not one, is an error: nothing is published
;; over a record that could not then be kept.
(define (history c)
  (let ((file (history-file c)))
    (if (file-exists? file)
        (let ((kept (guard (e (#t #f)) (call-with-input-file file read))))
          (unless (and (list? kept)
                       (every (lambda (e) (and (list? e) (= (length e) 2) (deployment-id? (car e)) (real? (cadr e)))) kept))
            (error "The record of deployments cannot be read:" file))
          kept)
        '())))

;; The ids of EVENTS, the latest first, each once.
(define (distinct-ids events)
  (let loop ((events events) (seen '()))
    (cond ((null? events) (reverse seen))
          ((member (caar events) seen) (loop (cdr events) seen))
          (else (loop (cdr events) (cons (caar events) seen))))))

;; The bytes of FILE, a bundle kept: an error past what one may be.
(define (read-bytes file)
  (call-with-port (open-binary-input-file file)
    (lambda (port)
      (let loop ((chunks '()) (total 0))
        (let ((chunk (read-bytevector 65536 port)))
          (cond ((eof-object? chunk) (apply bytevector-append (reverse chunks)))
                ((> (+ total (bytevector-length chunk)) max-bundle-bytes) (error "The bundle kept is too large:" file))
                (else (loop (cons chunk chunks) (+ total (bytevector-length chunk))))))))))

;; Record that ID was published, as `record!` does: whether it could be.
;; It is published all the same, which the caller says.
(define (recorded? c id)
  (guard (e (#t #f))
    (record! c id)
    #t))

;; Keep the bundle of deployment ID, prepared, if it is not kept.
(define (keep-bundle! c id)
  (let ((file (bundle-file c id)))
    (unless (file-exists? file)
      (let ((bytes (%cloud-bundle (control-name c) id)))
        (unless bytes (error "The deployment is not prepared:" id))
        (ensure-directory (control-directory c))
        (ensure-directory (bundles-directory c))
        ;; Written beside, then named: a bundle there is a whole one.
        (let ((partial (string-append file ".partial")))
          (when (file-exists? partial) (delete-file partial))
          (call-with-port (open-binary-output-file partial) (lambda (port) (write-bytevector bytes port)))
          (rename-file partial file))))))

;; Record that ID was published now, and keep the bundles of the latest
;; few deployments only.
(define (record! c id)
  (let* ((events (cons (list id (current-second)) (history c)))
         (events (if (> (length events) kept-events) (take events kept-events) events))
         (ids (distinct-ids events))
         (kept (if (> (length ids) kept-bundles) (take ids kept-bundles) ids)))
    (ensure-directory (control-directory c))
    (replace-file (history-file c) (call-with-output-string (lambda (port) (write events port))))
    (when (file-exists? (bundles-directory c))
      (for-each (lambda (file)
                  (let ((id (and (string-suffix? ".bundle" file) (substring file 0 (- (string-length file) 7)))))
                    (unless (and id (member id kept))
                      (delete-file (string-append (bundles-directory c) "/" file)))))
                (directory-list (bundles-directory c))))))

(define (control-restore! c)
  "Publish again what the record of C says was published last.
For a supervisor started anew, which serves nothing yet: prepared from
the bundle kept, and published whatever is. #f, and nothing done, if
nothing was recorded; an error if its bundle is not kept, or is not it."
  (in-turn c
    (lambda ()
      (let ((events (history c)))
        (and (pair? events)
             (let* ((id (caar events))
                    (prepared (%cloud-prepare-bundle (control-name c) (read-bytes (bundle-file c id)))))
               (unless (equal? (car prepared) id) (error "The bundle kept is not of the deployment:" id))
               (%cloud-publish (control-name c) id #t)
               id))))))

(define (control-deploy! c #:stage [stage #f] #:entry [entry (control-entry c)])
  "Deploy the project of C as its files and manifest are now.
STAGE, if given, is called first, in this deploy's turn, to put them
there; ENTRY is its entry module, if not the one C was made with.
Prepared, its bundle kept, and published if the project is still routed
where it was when this began: a dict of its `id`, the id `previous` to
it (#f if none), the `output` of loading it, and whether it is
`recorded` (published, it is served even if the record could not be
written). One that is the deployment published already is not recorded
again. One that does not load, or cannot be bound, or whose record
cannot be read, is an error and changes nothing. One deploy or rollback
runs at a time."
  (in-turn c
    (lambda ()
      (when stage (stage))
      ;; Its manifest is read by the supervisor, in a world made for it:
      ;; not here, where what a tenant wrote would be read into a world
      ;; that lasts.
      (let* ((name (control-name c))
             (seen (%cloud-route name))
             (prepared (%cloud-prepare-directory name entry))
             (id (car prepared)))
        ;; Read before anything is published: it must be there to add
        ;; to. One published already and recorded as the last is left
        ;; so; one published and not recorded (the record could not be
        ;; written then) is recorded now.
        (let* ((events (history c))
               (last (and (pair? events) (caar events))))
          (dict "id" id "previous" seen "output" (cadr prepared)
                "recorded" (or (and (equal? id seen) (equal? id last))
                               (begin (keep-bundle! c id)
                                      (unless (equal? id seen) (%cloud-publish name id seen))
                                      (recorded? c id)))))))))

(define (control-rollback! c [id #f])
  "Publish again an earlier deployment of the project of C.
The one with ID, or the one published before the one that is now. Its
bundle must still be kept. A dict of its `id`, the id `previous` to it,
and whether it is `recorded`, as `control-deploy!` says. Code and
bindings come back; what was kept in storage stays."
  (in-turn c
    (lambda ()
      (let* ((name (control-name c))
             (current (%cloud-route name))
             (earlier (filter (lambda (i) (not (equal? i current))) (distinct-ids (history c))))
             (target (or id (and (pair? earlier) (car earlier)))))
        (unless target (error "No earlier deployment to roll back to"))
        (when (equal? target current) (error "That deployment is the one published:" target))
        (unless (file-exists? (bundle-file c target)) (error "No bundle is kept of the deployment:" target))
        (let ((prepared (%cloud-prepare-bundle name (read-bytes (bundle-file c target)))))
          (unless (equal? (car prepared) target) (error "The bundle kept is not of the deployment:" target))
          (%cloud-publish name target (or current #f))
          (dict "id" target "previous" current "recorded" (recorded? c target)))))))

(define (control-deployments c)
  "Return the deployments of the project of C that were published.
A dict of `current`, the id published now (#f if none), and
`deployments`, a vector of dicts, the latest first: each one's `id`,
when it was last published (`at`, in seconds), and whether its bundle
is `kept`, to roll back to."
  (let ((events (history c)))
    (dict "current" (%cloud-route (control-name c))
          "deployments"
          (list->vector
           (map (lambda (id)
                  (dict "id" id
                        "at" (cadr (assoc id events))
                        "kept" (file-exists? (bundle-file c id))))
                (distinct-ids events))))))

(define (report e)
  (if (error-object? e) (condition/report-string e) "it failed"))

(define (control-serve c)
  "Do for C what the supervisor's own side asks, for ever.
Each asks a list: (\"deploy\"), answered with the deployment's id, or
with why it was not deployed."
  (let loop ()
    (let* ((asked (%control-next))
           (id (car asked))
           (what (cadr asked)))
      (guard (e (#t (%control-reply id #f (report e))))
        (cond ((equal? what '("deploy")) (%control-reply id #t (dict-ref (control-deploy! c) "id")))
              (else (%control-reply id #f "no such thing to do"))))
      (loop))))

;; The token a request carries, or #f.
(define (bearer request)
  (let ((given (request-header request "authorization")))
    (and given (string-prefix? "Bearer " given) (substring given 7 (string-length given)))))

(define (control-ops-handler c key #:revoked? [revoked? (lambda (id) #f)])
  "Return an HTTP handler answering the operations on the project of C.
`POST /v1/ops` with a JSON object of its `op`: `deployments`; `deploy`;
`rollback`, with the `id` to go back to if not the one before. Answered
with a JSON array of one reply, its `status` done, or error and what
kind, with a `message`. The request's bearer token must hold under KEY
(and REVOKED?, as `verify-token` has them) and allow the operation on
the project: `inspect` to list, `deploy` for the others."
  (define (reply status statuses . fields)
    (json-response (vector (apply dict "status" (list->vector statuses) fields)) #:status status))
  (lambda (request)
    (guard (e (#t (reply 422 '("error" "failed" "done") "message" (report e))))
      (let* ((claims (verify-token key (bearer request) #:revoked? revoked?))
             (message (and claims
                           (equal? (dict-ref request 'method) "POST")
                           (equal? (dict-ref request 'path) "/v1/ops")
                           (guard (e (#t #f)) (request-json request))))
             (op (and (dict? message) (dict-ref message "op" #f)))
             (scope (cond ((equal? op "deployments") "inspect")
                          ((member op '("deploy" "rollback")) "deploy")
                          (else #f))))
        (cond ((not claims) (reply 401 '("error" "unauthenticated" "done")))
              ((not message) (reply 400 '("error" "not-an-operation" "done")))
              ((not scope) (reply 400 '("error" "unknown-op" "done")))
              ((not (token-allows? claims (control-name c) scope)) (reply 403 '("error" "refused" "done")))
              ((equal? op "deployments") (apply reply 200 '("done") (dict->fields (control-deployments c))))
              ((equal? op "deploy") (apply reply 200 '("done") (dict->fields (control-deploy! c))))
              (else
               (let ((id (dict-ref message "id" #f)))
                 (unless (or (not id) (string? id)) (error "An id is a string:" id))
                 (apply reply 200 '("done") (dict->fields (control-rollback! c id))))))))))

;; The keys and values of D, in a list, one after the other.
(define (dict->fields d)
  (append-map (lambda (key) (list key (dict-ref d key))) (dict-keys d)))
