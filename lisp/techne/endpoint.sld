;;; Endpoints at the end of TRAMP routes: node connection lifecycle,
;;; path normalization, and project root discovery without editor state.

(define-library (techne endpoint)
  (export default-remote-node-command set-remote-node-command-hook!
          route-endpoint location-endpoint remote-endpoints
          remote-absolute-path endpoint-project-root)
  (import (techne) (techne location) (techne path) (techne task))
  (begin
    (define %default-node-command
      '("sh" "-c" "n=\"$HOME/.cache/techne/bin/techne-node\"; [ -x \"$n\" ] || n=techne-node; exec \"$n\""))

    (define (clone-command-list cmd)
      (and cmd (map string-copy cmd)))

    (define %remote-node-command-hook (lambda () (clone-command-list %default-node-command)))

    (define (set-remote-node-command-hook! proc)
      "Register PROC as the fallback thunk returning the remote node command.
The editor calls this to read its `remote-node-command` option."
      (unless (procedure? proc)
        (error "set-remote-node-command-hook!: expected procedure" proc))
      (set! %remote-node-command-hook proc))

    (define default-remote-node-command
      (make-parameter #f
        (lambda (v)
          (unless (or (not v) (and (list? v) (pair? v) (every string? v)))
            (error "default-remote-node-command: expected non-empty list of strings or #f" v))
          v))
      "Parameter overriding the command starting a node at the first hop.
When #f, calls the registered remote node command hook.")

    (define (resolve-node-command)
      (let ((cmd (or (default-remote-node-command) (%remote-node-command-hook))))
        (clone-command-list cmd)))

    (define (route-key hops)
      (map (lambda (h)
             (list (hop-method h) (hop-user h) (hop-host h) (hop-port h)))
           hops))

    ;; The endpoints connected: route-key -> (list node-cmd method-procs (node . ancestors) display).
    (define %endpoints (make-hash-table))

    ;; In-flight cold connects: route-key -> (list done-channel result-box node-cmd method-procs).
    (define %endpoint-inflight (make-hash-table))

    (define (remote-endpoints)
      "Return the routes connected, each `(route . node)`."
      (map (lambda (e) (cons (list-ref (cdr e) 3) (car (caddr (cdr e)))))
           (hash-table->alist %endpoints)))

    (define (usable? entry)
      (not (any node-lost entry)))

    (define (route-method-procs hops)
      (map (lambda (h)
             (let ((m (find-method (hop-method h))))
               (and m (caddr m))))
           hops))

    (define (same-procs? a b)
      (cond ((and (null? a) (null? b)) #t)
            ((or (null? a) (null? b)) #f)
            ((eq? (car a) (car b)) (same-procs? (cdr a) (cdr b)))
            (else #f)))

    (define (cached-entry-valid? cached node-cmd procs)
      (and cached
           (equal? (car cached) node-cmd)
           (same-procs? (cadr cached) procs)
           (usable? (caddr cached))))

    (define (connect hops node-cmd)
      (let* ((h (last hops))
             (m (or (find-method (hop-method h)) (error "No such method:" (hop-method h))))
             (before (reverse (cdr (reverse hops))))
             (jump? (lambda (x) (cadddr (find-method (hop-method x))))))
        (if (or (null? before) (and (jump? h) (every jump? before)))
            (list (node-connect ((caddr m) h before node-cmd)))
            (let* ((parent (route-entry before))
                   (facts (node-facts (car parent)))
                   (command ((caddr m) h '() (cons (cdr (assq 'program facts)) (cdr (assq 'arguments facts))))))
              (cons (node-connect-via (car parent) (car command) (cdr command)) parent)))))

    (define (route-entry hops)
      (let* ((key (route-key hops))
             (display (route->string hops))
             (node-cmd (resolve-node-command))
             (procs (route-method-procs hops))
             (cached (hash-table-ref/default %endpoints key #f)))
        (if (cached-entry-valid? cached node-cmd procs)
            (caddr cached)
            (let ((inflight (hash-table-ref/default %endpoint-inflight key #f)))
              (if (and inflight
                       (equal? (list-ref inflight 2) node-cmd)
                       (same-procs? (list-ref inflight 3) procs))
                  (begin
                    (channel-recv (car inflight))
                    (let ((res (car (cadr inflight))))
                      (cond ((and (pair? res) (eq? (car res) 'ok)) (cdr res))
                            ((eq? res 'cancelled) (route-entry hops))
                            ((and (pair? res) (eq? (car res) 'err)) (raise (cdr res)))
                            (else (error "remote endpoint connection failed" display)))))
                  (let* ((done (make-channel 1))
                         (box (list #f))
                         (slot (list done box node-cmd procs)))
                    (hash-table-delete! %endpoints key)
                    (hash-table-set! %endpoint-inflight key slot)
                    (dynamic-wind
                      (lambda () #f)
                      (lambda ()
                        (let ((outcome
                               (guard (e ((task-cancelled-condition? e)
                                          (set-car! box 'cancelled)
                                          (raise e))
                                         (#t (cons 'err e)))
                                 (let ((entry (connect hops node-cmd)))
                                   ;; Its hello first: a connection that fails is not kept.
                                   (node-facts (car entry))
                                   (hash-table-set! %endpoints key (list node-cmd procs entry display))
                                   (cons 'ok entry)))))
                          (set-car! box outcome)
                          (if (eq? (car outcome) 'ok)
                              (cdr outcome)
                              (raise (cdr outcome)))))
                      (lambda ()
                        (when (eq? (hash-table-ref/default %endpoint-inflight key #f) slot)
                          (hash-table-delete! %endpoint-inflight key))
                        (channel-close done)))))))))

    (define (route-endpoint hops)
      "Return the node at the end of the route HOPS, connected if need be.
Routes sharing a beginning share its endpoints; '() is #f, here. A
connection found lost, or going through one lost, is made again; one
that cannot be made raises an error and is not kept."
      (and (pair? hops) (car (route-entry hops))))

    (define (location-endpoint s)
      "Return the node the location S is on, or #f for a local path.
A connection found lost is made again, as `route-endpoint` does."
      (route-endpoint (location-route s)))

    (define (remote-absolute-path s)
      "Return the location S with its path absolute on its machine.
A path starting with ~, or relative, starts at the endpoint's home."
      (let ((path (location-path s)))
        (if (string-prefix? "/" path)
            s
            (let* ((home (cdr (assq 'home (node-facts (location-endpoint s)))))
                   (home (if (string-suffix? "/" home) home (string-append home "/"))))
              (location-at s (cond ((string=? path "") home)
                                   ((string=? path "~") home)
                                   ((string-prefix? "~/" path) (string-append home (substring path 2 (string-length path))))
                                   (else (string-append home path))))))))

    (define (with-trailing-slash path)
      (if (string-suffix? "/" path) path (string-append path "/")))

    (define (local-project-root dir)
      (let ((start (with-trailing-slash dir)))
        (let up ((d start))
          (cond ((or (file-exists? (string-append d ".jj"))
                     (file-exists? (string-append d ".git")))
                 d)
                ((or (string=? d "/") (string=? d "")) dir)
                (else
                 (let ((parent (with-trailing-slash (path-directory d))))
                   (if (string=? parent d) dir (up parent))))))))

    (define (endpoint-project-root dir)
      "Return the version-control root above DIR, local or remote.
Finds the nearest directory up from DIR with `.jj` or `.git`, else DIR
itself; with a slash, as DIR has. Remote locations query their endpoint."
      (if (remote-path? dir)
          (let ((found (node-find-up (location-endpoint dir) (location-path dir) '(".jj" ".git"))))
            (if found (location-at dir (with-trailing-slash found)) dir))
          (local-project-root dir)))))
