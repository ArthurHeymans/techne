;;; TRAMP-style locations and route methods: `/METHOD:USER@HOST#PORT|...:PATH`.
;;; Pure headless library with no editor dependencies.

(define-library (techne location)
  (export hop? make-hop hop-method hop-user hop-host hop-port
          remote-path? location-route location-path location-prefix location-at
          route->string define-method find-method method-names)
  (import (techne))
  (begin
    (define-record-type hop
      (%make-hop method user host port)
      hop?
      (method hop-method)
      (user hop-user)
      (host hop-host)
      (port hop-port))

    (define (make-hop method user host port)
      "Construct a route hop for METHOD with optional USER, HOST and PORT."
      (%make-hop method user host port))

    (define (hop->string h)
      (string-append (symbol->string (hop-method h)) ":"
                     (if (hop-user h) (string-append (hop-user h) "@") "")
                     (or (hop-host h) "")
                     (if (hop-port h) (string-append "#" (hop-port h)) "")))

    (define (char-index s c start)
      (let loop ((i start))
        (cond ((>= i (string-length s)) #f)
              ((char=? (string-ref s i) c) i)
              (else (loop (+ i 1))))))

    ;;; Methods: built-ins survive scope shutdown; extensions are scope-owned.

    (define %builtin-methods (make-hash-table))
    (define %extension-methods (make-registry 'location-methods))

    (define (register-builtin-method! name doc command jump)
      (hash-table-set! %builtin-methods name (list name doc command jump))
      name)

    (define (find-method name)
      "Return the method record `(NAME DOC COMMAND JUMP)` for NAME, or #f."
      (let ((sym (if (symbol? name) name (string->symbol name))))
        (or (registry-ref %extension-methods sym)
            (hash-table-ref/default %builtin-methods sym #f))))

    (define (define-method name doc #:command command #:jump [jump #f])
      "Define the method NAME of a route's hops, documented by DOC.
(COMMAND hop jumps program) returns the command, a list of strings,
running the list of strings PROGRAM one hop further, as the hop HOP
says: its user (`hop-user`), host and port. The first hop's command
runs here, a later one's on the endpoint before it. With JUMP, leading
hops of this method are one command: JUMPS are the hops before HOP it
goes through (ssh's -J), else '()."
      (registry-add! %extension-methods name
                     (list name doc (scope-procedure command) jump))
      name)

    (define (method-names)
      "Return the names of the methods defined."
      (let ((ext (registry-keys %extension-methods))
            (builtins (hash-table-keys %builtin-methods)))
        (let loop ((xs builtins) (acc ext))
          (cond ((null? xs) acc)
                ((memq (car xs) acc) (loop (cdr xs) acc))
                (else (loop (cdr xs) (cons (car xs) acc)))))))

    (define (shell-word s)
      (string-append "'" (string-join (string-split s "'") "'\\''") "'"))

    (register-builtin-method!
     'ssh
     "Log in to another machine with OpenSSH.
Its configuration applies: host aliases, keys, ProxyJump. It does not
ask for passwords: a key or an agent must do."
     (lambda (h jumps program)
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
               (map shell-word program)))
     #t)

    (register-builtin-method!
     'sudo
     "Become another user, root by default, on the same machine.
It does not ask for a password yet: sudo must not need one (NOPASSWD)."
     (lambda (h jumps program)
       (append (list "sudo" "-n" "-u" (or (hop-user h) "root") "--") program))
     #f)

    (register-builtin-method!
     'doas
     "Become another user, root by default, with doas.
It does not ask for a password yet: doas must not need one (nopass)."
     (lambda (h jumps program)
       (append (list "doas" "-n" "-u" (or (hop-user h) "root") "--") program))
     #f)

    (register-builtin-method!
     'podman
     "Enter the running container HOST with podman exec.
The node's program must be in the container."
     (lambda (h jumps program)
       (append (list "podman" "exec" "-i") (if (hop-user h) (list "-u" (hop-user h)) '())
               (list (hop-host h)) program))
     #f)

    (register-builtin-method!
     'docker
     "Enter the running container HOST with docker exec.
The node's program must be in the container."
     (lambda (h jumps program)
       (append (list "docker" "exec" "-i") (if (hop-user h) (list "-u" (hop-user h)) '())
               (list (hop-host h)) program))
     #f)

    ;;; Location parsing

    (define (parse-hop name spec)
      (let* ((at (char-index spec #\@ 0))
             (user (and at (substring spec 0 at)))
             (rest (if at (substring spec (+ at 1) (string-length spec)) spec))
             (hash (char-index rest #\# 0))
             (host (if hash (substring rest 0 hash) rest))
             (port (and hash (substring rest (+ hash 1) (string-length rest)))))
        ;; A user or host read as an option by the command it is given to
        ;; is not one: the location is not one either.
        (and (not (string-prefix? "-" host))
             (not (and user (string-prefix? "-" user)))
             (%make-hop name
                        (and user (not (string=? user "")) user)
                        (and (not (string=? host "")) host)
                        port))))

    (define (parse-location s)
      (and (string-prefix? "/" s)
           (let loop ((i 1) (hops '()))
             (let ((colon (char-index s #\: i)))
               (and colon
                    (let ((name (substring s i colon)))
                      (and (find-method name)
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

    (define (remote-path? s)
      "Return #t if S names a location on another machine.
It is written as TRAMP writes it: /METHOD:USER@HOST#PORT|...:PATH, with
methods `define-method` defined."
      (and (string? s) (parse-location s) #t))

    (define (location-route s)
      "Return the route of the location S, a list of hops, or '() if local."
      (let ((l (and (string? s) (parse-location s)))) (if l (car l) '())))

    (define (location-path s)
      "Return the path of the location S on the machine it is on.
For a local path, that is S."
      (let ((l (and (string? s) (parse-location s)))) (if l (cdr l) s)))

    (define (route->string hops)
      "Format a route HOPS as a TRAMP prefix such as `/ssh:host:`, or \"\"."
      (if (null? hops) "" (string-append "/" (string-join (map hop->string hops) "|") ":")))

    (define (location-prefix s)
      "Return the route part of the location S, as /ssh:host:, or \"\"."
      (route->string (location-route s)))

    (define (location-at s path)
      "Return the location of PATH on the machine the location S is on."
      (string-append (location-prefix s) path))))
