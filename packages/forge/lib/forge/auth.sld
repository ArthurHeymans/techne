;;; Credential lookup for Forge hosts via `(techne secrets)`.
;;; Secrets are never persisted or logged.
(define-library (forge auth)
  (export forge-secret forge-token forge-auth-header forge-gh-cli-token forge-register-secret-backends!)
  (import (techne) (techne secrets) (techne trusted-process) (techne task))
  (begin
    (define %max-gh-token-chars 65536)

    (define (nonempty? s)
      (and (string? s) (> (string-length (string-trim s)) 0)))

    (define (assq-ref alist key [default #f])
      (let ((e (assq key alist))) (if e (cdr e) default)))

    (define (host-only s)
      (and (nonempty? s)
           (let* ((t (string-trim s))
                  (slash (string-index t #\/)))
             (if slash (substring t 0 slash) t))))

    ;; Ensure a token fetched for WEB-HOST can only be sent to WEB-HOST
    ;; itself (GHE) or `api.<WEB-HOST>` (GitHub).
    (define (api-matches-web-host? api-host web-host)
      (let ((api (host-only api-host))
            (web (host-only web-host)))
        (and api web
             (or (string-ci=? api web)
                 (string-ci=? api (string-append "api." web))))))

    (define (forge-gh-cli-token host #:gh [gh "gh"])
      "Return the GitHub CLI token for HOST via `gh auth token`, or #f.
Runs GH in the user's trusted environment with bounded output and
propagates task cancellation."
      (and (nonempty? host)
           (guard (e ((not (task-cancelled-condition? e)) #f))
             (let* ((res (run-trusted-tool gh (list "auth" "token" (string-append "--hostname=" (string-trim host)))
                                           #:max-stdout %max-gh-token-chars
                                           #:label "gh"))
                    (status (car res))
                    (out (cadr res))
                    (tok (and (equal? status 0) (string? out) (string-trim out))))
               (and (nonempty? tok) tok)))))

    (define (forge-register-secret-backends!)
      "Register Forge's scope-owned secret backends (`gh-cli`)."
      (register-secret-backend!
       'gh-cli
       (lambda (query spec)
         (let ((service (assq-ref query 'service))
               (api-host (assq-ref query 'host))
               (web-host (or (assq-ref spec 'web-host) (assq-ref query 'web-host)))
               (gh (assq-ref spec 'gh "gh")))
           (and (eq? service 'forge)
                (nonempty? web-host)
                (api-matches-web-host? api-host web-host)
                (forge-gh-cli-token web-host #:gh gh))))
       #:doc "Look up a GitHub token via `gh auth token` for Forge hosts."))

    (define (trim-secret s query)
      (cond ((not s) #f)
            ((secret? s)
             (let ((v (string-trim (secret-reveal s))))
               (and (> (string-length v) 0)
                    (make-secret v #:label (secret-label s)))))
            ((secret-ref? s) (trim-secret (secret-resolve s #:query query) query))
            ((string? s)
             (let ((v (string-trim s)))
               (and (> (string-length v) 0)
                    (make-secret v #:label "forge"))))
            (else #f)))

    (define %github-env-source
      '(env #:vars ("GITHUB_TOKEN" "GH_TOKEN") #:host "api.github.com"))

    (define (forge-secret api-host #:user [user #f] #:web-host [web-host #f]
                          #:provider [provider #f] #:authinfo-file [authinfo-file #f]
                          #:sources [sources #f] #:allow-env [allow-env #t]
                          #:allow-gh-cli [allow-gh-cli #f])
      "Return an opaque secret token for API-HOST, or #f if unauthenticated.
Lookup order: PROVIDER callback (returning a string, `secret` or
`secret-ref`), AUTHINFO-FILE or SOURCES via `(techne secrets)` matching
USER^forge, USER or API-HOST, GITHUB_TOKEN / GH_TOKEN for
`api.github.com` when ALLOW-ENV is true, and optionally `gh auth token`
for WEB-HOST when ALLOW-GH-CLI is true and API-HOST matches WEB-HOST."
      (let* ((eff-web-host (or web-host
                               (and (string? api-host) (string-ci=? api-host "api.github.com") "github.com")))
             (extra (append (if eff-web-host (list (cons 'web-host eff-web-host)) '())
                            (list (cons 'login-tag "^forge"))))
             (query (append (list (cons 'service 'forge) (cons 'host api-host))
                            (if user (list (cons 'user user)) '())
                            extra)))
        (or (and (procedure? provider)
                 (trim-secret (provider api-host user) query))
            (let* ((base-sources (cond (authinfo-file (list (list 'authinfo #:file authinfo-file)))
                                       (sources sources)
                                       (else (default-secret-sources))))
                   (all-sources (if allow-env
                                    (append base-sources (list %github-env-source))
                                    base-sources))
                   (found (secret-lookup #:service 'forge
                                         #:host api-host
                                         #:user user
                                         #:extra extra
                                         #:sources all-sources)))
              (trim-secret found query))
            (and allow-gh-cli
                 (nonempty? eff-web-host)
                 (api-matches-web-host? api-host eff-web-host)
                 (let ((tok (forge-gh-cli-token eff-web-host)))
                   (and (nonempty? tok) (make-secret tok #:label "gh-cli")))))))

    (define (forge-token api-host #:user [user #f] #:web-host [web-host #f]
                         #:provider [provider #f] #:authinfo-file [authinfo-file #f]
                         #:sources [sources #f] #:allow-env [allow-env #t]
                         #:allow-gh-cli [allow-gh-cli #f])
      "Return an access token string for API-HOST, or #f if unauthenticated.
Delegates to `forge-secret` with USER, WEB-HOST, PROVIDER,
AUTHINFO-FILE, SOURCES, ALLOW-ENV and ALLOW-GH-CLI."
      (let ((s (forge-secret api-host #:user user #:web-host web-host
                             #:provider provider #:authinfo-file authinfo-file
                             #:sources sources #:allow-env allow-env
                             #:allow-gh-cli allow-gh-cli)))
        (and s (secret-reveal s))))

    (define (forge-auth-header token)
      "Return the HTTP Authorization header value for TOKEN, or #f.
TOKEN may be a string or an opaque `secret`."
      (let ((raw (if (secret? token) (secret-reveal token) token)))
        (and (nonempty? raw) (string-append "Bearer " (string-trim raw)))))))
