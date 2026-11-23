;;; Remote access (doc/remote.md): locations on other machines, the routes
;;; reaching them, the endpoints at their ends, and execution sites.
;;;
;;; The headless mechanics live in `(techne location)`, `(techne endpoint)`
;;; and `(techne execution-site)`; this module bridges them to editor
;;; options (`remote-node-command`) and effect tracking (`note-effect!`).

(require "modes.scm")
(require "operations.scm")

(provide remote-path? location-route location-path location-prefix location-at
         hop? hop-method hop-user hop-host hop-port
         define-method method-names
         route-endpoint location-endpoint remote-endpoints
         remote-absolute-path endpoint-project-root call-with-process-at
         execution-site-loaded? ensure-execution-site!
         execution-site? execution-site-for
         execution-site-route execution-site-directory
         env-snapshot-status
         execution-site-env-snapshot execution-site-cached-snapshot
         execution-site-resolved-environment execution-site-find-program
         execution-site-needs-eval? execution-site-watched-path?
         execution-site-invalidate!
         spawn-at-site call-with-process-at-site)

(define-option remote-node-command
  '("sh" "-c" "n=\"$HOME/.cache/techne/bin/techne-node\"; [ -x \"$n\" ] || n=techne-node; exec \"$n\"")
  "The command starting a node at the first hop of a route.
It runs the node from where it is deployed, ~/.cache/techne/bin, else
`techne-node` from the PATH. A later hop runs the program of the node
before it. User-scope only."
  #:type '(list-of string)
  #:user-only #t)

;;; Lazy bridges to `(techne location)`, `(techne endpoint)`, and
;;; `(techne execution-site)`. Loaded on first use rather than at startup.

(define %loc-env #f)

(define (ensure-location!)
  (or %loc-env
      (let ((e (environment '(techne location))))
        (set! %loc-env e)
        e)))

(define (loc-ref name)
  (eval name (ensure-location!)))

;; A TRAMP path starts with "/" and has ":" before the next "/".
(define (maybe-tramp? s)
  (and (string? s)
       (> (string-length s) 2)
       (char=? (string-ref s 0) #\/)
       (let loop ((i 1) (n (string-length s)))
         (and (< i n)
              (let ((c (string-ref s i)))
                (cond ((char=? c #\:) #t)
                      ((char=? c #\/) #f)
                      (else (loop (+ i 1) n))))))))

(define (remote-path? s)
  "Return #t if S names a location on another machine."
  (and (maybe-tramp? s) ((loc-ref 'remote-path?) s)))

(define (location-route s)
  "Return the route of location S, a list of hops, or '() if local."
  (if (maybe-tramp? s) ((loc-ref 'location-route) s) '()))

(define (location-path s)
  "Return the path of location S on the machine it is on."
  (if (maybe-tramp? s) ((loc-ref 'location-path) s) s))

(define (location-prefix s)
  "Return the route part of location S, as /ssh:host:, or \"\"."
  (if (maybe-tramp? s) ((loc-ref 'location-prefix) s) ""))

(define (location-at s path)
  "Return the location of PATH on the machine location S is on."
  (if (maybe-tramp? s) ((loc-ref 'location-at) s path) path))

(define (hop? x)
  "Return #t if X is a route hop."
  (and %loc-env ((loc-ref 'hop?) x)))

(define (hop-method h)
  "Return the method symbol of hop H."
  ((loc-ref 'hop-method) h))

(define (hop-user h)
  "Return the user string of hop H, or #f."
  ((loc-ref 'hop-user) h))

(define (hop-host h)
  "Return the host string of hop H, or #f."
  ((loc-ref 'hop-host) h))

(define (hop-port h)
  "Return the port string of hop H, or #f."
  ((loc-ref 'hop-port) h))

(define (define-method name doc #:command command #:jump [jump #f])
  "Define route hop method NAME with DOC, COMMAND, and optional JUMP."
  ((loc-ref 'define-method) name doc #:command command #:jump jump))

(define (method-names)
  "Return the names of the route methods defined."
  ((loc-ref 'method-names)))

(define %ep-env #f)

(define %init-ep-env!
  (scope-procedure
    (lambda ()
      (or %ep-env
          (let ((e (environment '(techne endpoint))))
            ((eval 'set-remote-node-command-hook! e) (lambda () (option #f 'remote-node-command)))
            (set! %ep-env e)
            e)))))

(define (ensure-endpoint!)
  (or %ep-env (%init-ep-env!)))

(define (ep-ref name)
  (eval name (ensure-endpoint!)))

(define (route-endpoint hops)
  "Return the remote node endpoint for HOPS, or #f when local."
  (and (pair? hops) ((ep-ref 'route-endpoint) hops)))

(define (location-endpoint loc)
  "Return the remote node endpoint for LOC, or #f when local."
  (and (remote-path? loc) ((ep-ref 'location-endpoint) loc)))

(define (remote-endpoints)
  "Return the currently connected remote node endpoints."
  (if %ep-env ((ep-ref 'remote-endpoints)) '()))

(define (remote-absolute-path path)
  "Resolve remote PATH to a normalized absolute location string."
  ((ep-ref 'remote-absolute-path) path))

(define (endpoint-project-root dir)
  "Return the version-control root above DIR, or DIR itself."
  ((ep-ref 'endpoint-project-root) dir))

(define %es-env #f)

(define (execution-site-loaded?)
  "Return #t if `(techne execution-site)` is loaded in this runtime."
  (and %es-env #t))

(define %init-es-env!
  (scope-procedure
    (lambda ()
      (or %es-env
          (begin
            (ensure-endpoint!)
            (let ((e (environment '(techne execution-site))))
              ((eval 'set-direnv-enabled-hook! e) (lambda () (option #f 'direnv-enabled)))
              ((eval 'set-direnv-program-hook! e) (lambda () (option #f 'direnv-program)))
              ((eval 'set-direnv-data-dir-hook! e) (lambda () (option #f 'direnv-data-dir)))
              (set! %es-env e)
              e))))))

(define (ensure-execution-site!)
  "Load `(techne execution-site)` with editor direnv hooks wired."
  (or %es-env (%init-es-env!)))

(define (es-ref name)
  (eval name (ensure-execution-site!)))

(define (execution-site? x)
  "Return #t if X is an `execution-site`."
  (and %es-env ((es-ref 'execution-site?) x)))

(define (execution-site-for location #:policy [policy #f] #:environment [environment '()])
  "Return an `execution-site` for LOCATION under POLICY and ENVIRONMENT.
If LOCATION is already an `execution-site`, keep its policy unless
POLICY is explicitly provided; otherwise default POLICY to `project`."
  ((es-ref 'execution-site-for) location #:policy policy #:environment environment))

(define (execution-site-route site)
  "Return the hop route of SITE."
  ((es-ref 'execution-site-route) site))

(define (execution-site-directory site)
  "Return the working directory path of SITE."
  ((es-ref 'execution-site-directory) site))

(define (env-snapshot-status snap)
  "Return the status symbol of SNAP."
  ((es-ref 'env-snapshot-status) snap))

(define (execution-site-env-snapshot site #:refresh? [refresh? #f])
  "Return the `env-snapshot` for SITE, re-evaluating when REFRESH? is true."
  ((es-ref 'execution-site-env-snapshot) site #:refresh? refresh?))

(define (execution-site-cached-snapshot dir)
  "Return the cached `env-snapshot` for DIR without I/O, or #f."
  (and %es-env ((es-ref 'execution-site-cached-snapshot) dir)))

(define (execution-site-resolved-environment site #:refresh? [refresh? #f] #:environment [environment '()])
  "Return the launch environment alist for SITE.
Honors REFRESH? and overlays explicit ENVIRONMENT overrides last."
  ((es-ref 'execution-site-resolved-environment) site #:refresh? refresh? #:environment environment))

(define (execution-site-find-program site program #:refresh? [refresh? #f])
  "Resolve PROGRAM against SITE's environment, honoring REFRESH?."
  ((es-ref 'execution-site-find-program) site program #:refresh? refresh?))

(define (execution-site-needs-eval? dir)
  "Return #t if DIR's environment cache is missing or stale."
  ((es-ref 'execution-site-needs-eval?) dir))

(define (execution-site-watched-path? path)
  "Return #t if PATH is watched by a cached environment snapshot."
  (and %es-env ((es-ref 'execution-site-watched-path?) path)))

(define (execution-site-invalidate! [dir #f])
  "Clear cached environment snapshots for DIR, or all if omitted."
  (when %es-env ((es-ref 'execution-site-invalidate!) dir)))

(define (spawn-at-site site program args #:pty [pty #f] #:persist [persist #f]
                       #:environment [environment '()] #:stdin [stdin #f]
                       #:stdout [stdout #f] #:stderr [stderr #f])
  "Start PROGRAM with ARGS at execution site SITE; return the process.
ENVIRONMENT overrides site variables; PTY, PERSIST, STDIN, STDOUT, and
STDERR forward to `process-spawn`."
  ((es-ref 'spawn-at-site) site program args
                           #:pty pty #:persist persist #:environment environment
                           #:stdin stdin #:stdout stdout #:stderr stderr))

(define (call-with-process-at-site site program args f #:pty [pty #f] #:persist [persist #f]
                                   #:environment [environment '()] #:stdin [stdin #f]
                                   #:stdout [stdout #f] #:stderr [stderr #f])
  "Call F with a process running PROGRAM with ARGS at execution site SITE.
ENVIRONMENT overrides site variables; PTY, PERSIST, STDIN, STDOUT, and
STDERR forward to `call-with-process`."
  ((es-ref 'call-with-process-at-site) site program args f
                                       #:pty pty #:persist persist #:environment environment
                                       #:stdin stdin #:stdout stdout #:stderr stderr))

(define (call-with-process-at dir program args f #:pty [pty #f] #:policy [policy #f] #:environment [environment '()])
  "Call F with a new process running PROGRAM with ARGS in DIR.
DIR is a directory here, a remote location, or an `execution-site`.
POLICY defaults to `project` for a string DIR (or DIR's policy for an
`execution-site`): `project` applies active project environment such as
direnv, `endpoint-base` uses the unmodified endpoint base environment,
and `user-trusted` runs in the local trusted working directory.
ENVIRONMENT overrides site variables. PTY is as `process-spawn` takes it."
  (note-effect! 'external)
  (call-with-process-at-site (execution-site-for dir #:policy policy)
                             program args f #:pty pty #:environment environment))

(set-remote-path-hook! remote-path?)
(set-execution-site-inspector-hook!
 (lambda (x)
   (and (execution-site? x)
        (let ((dir (execution-site-directory x)))
          (cons (or (pair? (execution-site-route x))
                    (and dir (remote-path? dir)))
                dir)))))
