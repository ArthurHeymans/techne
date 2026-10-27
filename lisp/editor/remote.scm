;;; Remote access (doc/remote.md): locations on other machines, the routes
;;; reaching them and the endpoints at their ends.
;;;
;;; A location is written as TRAMP writes it, a route and a path:
;;; /ssh:arthur@build#2222|sudo:root@build:/etc/hosts. Each hop of the
;;; route names a method and its user, host and port. A method says how to
;;; run a command one hop further (`define-method`); leading ssh hops are
;;; one ssh command, through `-J`, and each later hop is started by the
;;; endpoint before it. An endpoint is a node: every location on a route
;;; shares it, and one that is lost is connected again when next used.
;;;
;;; Locations are carried as strings, in that syntax, wherever paths are:
;;; a document's path, a buffer's directory. Paths of other machines are
;;; absolute there, `~` resolved against the endpoint's home.

(require "modes.scm")

(provide remote-path? location-route location-path location-prefix location-at
         hop? hop-method hop-user hop-host hop-port
         define-method method-names route-endpoint location-endpoint remote-endpoints
         remote-absolute-path call-with-process-at)

;;; Locations

;; A hop: its method's name (a symbol), user, host and port, or #f.
(define-record-type hop
  (make-hop method user host port)
  hop?
  (method hop-method)
  (user hop-user)
  (host hop-host)
  (port hop-port))

(define (hop->string h)
  (string-append (symbol->string (hop-method h)) ":"
                 (if (hop-user h) (string-append (hop-user h) "@") "")
                 (or (hop-host h) "")
                 (if (hop-port h) (string-append "#" (hop-port h)) "")))

;; HOP's user@host#port part parsed, for the method NAME; #f if its
;; user or host starts with a dash.
(define (parse-hop name spec)
  (let* ((at (char-index spec #\@ 0))
         (user (and at (substring spec 0 at)))
         (rest (if at (substring spec (+ at 1) (string-length spec)) spec))
         (hash (char-index rest #\# 0))
         (host (if hash (substring rest 0 hash) rest))
         (port (and hash (substring rest (+ hash 1) (string-length rest)))))
    ;; A user or host read as an option by the command it is given to
    ;; is not one: the location is not one either.
    (and (not (string-prefix? "-" host)) (not (and user (string-prefix? "-" user)))
         (make-hop name (and user (not (string=? user "")) user) (and (not (string=? host "")) host) port))))

;; (hops . path) of the location S, or #f when S is a local path: S is
;; "/" then hops separated by "|", each METHOD:SPEC with a method
;; defined, then ":" and the path.
(define (parse-location s)
  (and (string-prefix? "/" s)
       (let loop ((i 1) (hops '()))
         (let ((colon (char-index s #\: i)))
           (and colon
                (let ((name (substring s i colon)))
                  (and (method name)
                       (let find-end ((j (+ colon 1)))
                         (cond ((>= j (string-length s)) #f)
                               ((char=? (string-ref s j) #\|)
                                (let ((h (parse-hop (string->symbol name) (substring s (+ colon 1) j))))
                                  (and h (loop (+ j 1) (cons h hops)))))
                               ((char=? (string-ref s j) #\:)
                                (let ((h (parse-hop (string->symbol name) (substring s (+ colon 1) j))))
                                  (and h (cons (reverse (cons h hops)) (substring s (+ j 1) (string-length s))))))
                               ((char=? (string-ref s j) #\/) #f)
                               (else (find-end (+ j 1))))))))))))

(define (char-index s c start)
  (let loop ((i start))
    (cond ((>= i (string-length s)) #f)
          ((char=? (string-ref s i) c) i)
          (else (loop (+ i 1))))))

(define (remote-path? s)
  "Return #t if S names a location on another machine.
It is written as TRAMP writes it: /METHOD:USER@HOST#PORT|...:PATH, with
methods `define-method` defined."
  (and (string? s) (parse-location s) #t))

(define (location-route s)
  "Return the route of the location S, a list of hops, or '() if local."
  (let ((l (parse-location s))) (if l (car l) '())))

(define (location-path s)
  "Return the path of the location S on the machine it is on.
For a local path, that is S."
  (let ((l (parse-location s))) (if l (cdr l) s)))

(define (route->string hops)
  (if (null? hops) "" (string-append "/" (string-join (map hop->string hops) "|") ":")))

(define (location-prefix s)
  "Return the route part of the location S, as /ssh:host:, or \"\"."
  (route->string (location-route s)))

(define (location-at s path)
  "Return the location of PATH on the machine the location S is on."
  (string-append (location-prefix s) path))

;;; Methods

(define %methods (make-hash-table))

;; (name doc command jump)
(define (method name) (hash-table-ref/default %methods (if (symbol? name) name (string->symbol name)) #f))

(define (define-method name doc #:command command #:jump [jump #f])
  "Define the method NAME of a route's hops, documented by DOC.
(COMMAND hop jumps program) returns the command, a list of strings,
running the list of strings PROGRAM one hop further, as the hop HOP
says: its user (`hop-user`), host and port. The first hop's command
runs here, a later one's on the endpoint before it. With JUMP, leading
hops of this method are one command: JUMPS are the hops before HOP it
goes through (ssh's -J), else '()."
  (hash-table-set! %methods name (list name doc command jump))
  name)

(define (method-names)
  "Return the names of the methods defined."
  (hash-table-keys %methods))

(define (shell-word s)
  (string-append "'" (string-join (string-split s "'") "'\\''") "'"))

(define-method 'ssh "Log in to another machine with OpenSSH.
Its configuration applies: host aliases, keys, ProxyJump. It does not
ask for passwords: a key or an agent must do."
  #:jump #t
  #:command (lambda (h jumps program)
              (append (list "ssh" "-o" "BatchMode=yes")
                      (if (null? jumps)
                          '()
                          (list "-J" (string-join (map (lambda (j)
                                                         (string-append (if (hop-user j) (string-append (hop-user j) "@") "")
                                                                        (hop-host j)
                                                                        (if (hop-port j) (string-append ":" (hop-port j)) "")))
                                                       jumps)
                                                  ",")))
                      (if (hop-port h) (list "-p" (hop-port h)) '())
                      (if (hop-user h) (list "-l" (hop-user h)) '())
                      ;; No option after it: the host is one, never an option.
                      (list "--" (or (hop-host h) "localhost"))
                      ;; The remote shell reads it as one line.
                      (map shell-word program))))

(define-method 'sudo "Become another user, root by default, on the same machine.
It does not ask for a password yet: sudo must not need one (NOPASSWD)."
  #:command (lambda (h jumps program)
              (append (list "sudo" "-n" "-u" (or (hop-user h) "root") "--") program)))

(define-method 'doas "Become another user, root by default, with doas.
It does not ask for a password yet: doas must not need one (nopass)."
  #:command (lambda (h jumps program)
              (append (list "doas" "-n" "-u" (or (hop-user h) "root") "--") program)))

(define-method 'podman "Enter the running container HOST with podman exec.
The node's program must be in the container."
  #:command (lambda (h jumps program)
              (append (list "podman" "exec" "-i") (if (hop-user h) (list "-u" (hop-user h)) '())
                      (list (hop-host h)) program)))

(define-method 'docker "Enter the running container HOST with docker exec.
The node's program must be in the container."
  #:command (lambda (h jumps program)
              (append (list "docker" "exec" "-i") (if (hop-user h) (list "-u" (hop-user h)) '())
                      (list (hop-host h)) program)))

;;; Endpoints

(define-option remote-node-command
  '("sh" "-c" "n=\"$HOME/.cache/techne/bin/techne-node\"; [ -x \"$n\" ] || n=techne-node; exec \"$n\"")
  "The command starting a node at the first hop of a route.
It runs the node from where it is deployed, ~/.cache/techne/bin, else
`techne-node` from the PATH. A later hop runs the program of the node
before it."
  #:type '(list-of string))

;; The endpoints connected: route string -> (node . ancestors), the
;; nodes its connection goes through.
(define %endpoints (make-hash-table))

(define (remote-endpoints)
  "Return the routes connected, each (route . node)."
  (map (lambda (e) (cons (car e) (cadr e))) (hash-table->alist %endpoints)))

;; Whether the endpoint ENTRY, (node . ancestors), can be used: neither its
;; connection nor one it goes through is lost.
(define (usable? entry) (not (any node-lost entry)))

;; Connect to the end of the route HOPS: (node . ancestors). The first
;; hop's command runs here, as do those of leading hops that jump; a
;; later hop's, on the endpoint before it, running its own program.
(define (connect hops)
  (let* ((h (last hops))
         (m (or (method (hop-method h)) (error "No such method:" (hop-method h))))
         (before (reverse (cdr (reverse hops))))
         (jump? (lambda (x) (cadddr (method (hop-method x))))))
    (if (or (null? before) (and (jump? h) (every jump? before)))
        (list (node-connect ((caddr m) h before (option #f 'remote-node-command))))
        (let* ((parent (route-entry before))
               (facts (node-facts (car parent)))
               (command ((caddr m) h '() (cons (cdr (assq 'program facts)) (cdr (assq 'arguments facts))))))
          (cons (node-connect-via (car parent) (car command) (cdr command)) parent)))))

;; The endpoint entry of the route HOPS, connected again if it was lost.
(define (route-entry hops)
  (let* ((key (route->string hops))
         (cached (hash-table-ref/default %endpoints key #f)))
    (if (and cached (usable? cached))
        cached
        (let ((entry (connect hops)))
          (hash-table-delete! %endpoints key)
          ;; Its hello first: a connection that fails is not kept.
          (node-facts (car entry))
          (hash-table-set! %endpoints key entry)
          entry))))

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

(define (call-with-process-at dir program args f #:pty [pty #f])
  "Call F with a new process running PROGRAM with ARGS in DIR.
DIR is a directory here, or a location: the process then runs on its
machine. PTY is as `process-spawn` takes it."
  (call-with-process program args f #:pty pty #:node (location-endpoint dir) #:directory (location-path dir)))
