;;; A tool client, not an agent. Uses the existing named node sessions.
(import (techne) (techne tools) (techne crypto))
(define args (cdr (command-line)))
(define executable (car args))
;; The flags may come anywhere but in FORM, which is one argument of its
;; own: --text displays a string answer as it is, not written; --stdin
;; binds `input` in FORM to the text read from stdin, so text needs no
;; escaping (patches for `tool-apply!`, say); --help asks for help. Where
;; they stood used to matter, and a --text after FORM was quietly taken
;; for SESSION's name: now an argument beginning `--` that is no flag is
;; an error saying so, and `--` ends the flags when a positional
;; argument (including a FORM that is exactly `--text`) begins so.
(define usage "usage: techne tool [--text] [--stdin] ROOT [FORM|-|@FILE [SESSION]]; try techne tool --help")
(define parsed
  (let loop ((rest (cdr args)) (seen '()) (positional '()) (flags? #t))
    (cond ((null? rest) (cons (reverse seen) (reverse positional)))
          ((and flags? (string=? (car rest) "--"))
           (loop (cdr rest) seen positional #f))
          ((and flags? (member (car rest) '("--text" "--stdin" "--help" "-h")))
           (loop (cdr rest) (cons (car rest) seen) positional flags?))
          ((and flags? (string-prefix? "--" (car rest)))
           (error (string-append "techne tool: unknown flag " (car rest)
                                 "; the flags are --text, --stdin and --help, and -- ends them; " usage)))
          (else (loop (cdr rest) seen (cons (car rest) positional) flags?)))))
(define request (cdr parsed))
(define flags (car parsed))
(define text? (and (member "--text" flags) #t))
(define (read-all port)
  (let loop ((chunks '()))
    (let ((s (read-string 65536 port)))
      (if (eof-object? s) (apply string-append (reverse chunks)) (loop (cons s chunks))))))
;; Read only once help is ruled out: help needs nothing, stdin included.
(define input #f)
;; FORM itself: `-` reads it from stdin, `@FILE` from FILE, so that a long
;; form needs no shell quoting.
(define (form-source text)
  (cond ((string=? text "-")
         (when input (error "techne tool: --stdin and a FORM of - would both read stdin; use @FILE for one"))
         (read-all (current-input-port)))
        ((string-prefix? "@" text) (file->string (substring text 1 (string-length text))))
        (else text)))
;; A connection to SESSION's daemon, restarted if it runs other code than
;; STAMP, this client's: an older build, or tool sources changed since it
;; loaded them. Restarting forgets its snapshots and stops its processes.
(define (session-node session stamp)
  (let loop ((tries 0))
    (let* ((node (node-connect (list executable "node" "--session" session)))
           (theirs (and stamp (guard (e (#t #f)) (node-eval node "(begin (import (techne tools)) (tool-stamp))")))))
      ;; No stamp of its own (a program older than its tools): no check.
      (cond ((or (not stamp) (equal? theirs stamp)) node)
            ((>= tries 3)
             (node-close node)
             (error "techne tool: the session runs other code even when restarted; name another" session))
            (else
             (when (= tries 0)
               (display (string-append "techne tool: session " session " ran older code; restarting it\n") (current-error-port)))
             (guard (e (#t #f)) (node-shutdown node))
             (guard (e (#t #f)) (node-close node))
             (sleep 200)
             (loop (+ tries 1)))))))
(define (written value) (call-with-output-string (lambda (p) (write value p))))
;; Without a SESSION, this program's own: one per techne binary (its
;; path, as current_exe gives it) and the Lisp it loads, so that two
;; checkouts, a debug and a release build, or one binary given other
;; Lisp do not each find the other's session running other code and
;; restart it, forgetting what it held.
;; The library path's directories as the loader finds them, from here: a
;; relative one names another directory from another directory.
(define (library-path)
  (map (lambda (dir) (guard (e (#t dir)) (%repository-canonical-root dir)))
       (remove (lambda (d) (string=? d ""))
               (string-split (or (get-environment-variable "TECHNE_LIBRARY_PATH") "") #\:))))
(define (default-session)
  (let ((identity (string-join (append (list executable (%lisp-directory)) (library-path)) "\x0;")))
    (string-append "tools-" (substring (hex-encode (sha-256 (string->utf8 identity))) 0 12))))
;; Help is --help or -h anywhere, or no ROOT at all.
(define help-request?
  (or (null? request) (member "--help" flags) (member "-h" flags)))
(cond (help-request? (write (tool-help)) (newline))
      ((> (length request) 3)
       (error (string-append "techne tool: ROOT, at most one FORM and at most one SESSION, not "
                             (number->string (length request)) " arguments; " usage)))
      (else
        (when (member "--stdin" flags) (set! input (read-all (current-input-port))))
        (let* ((root (%repository-canonical-root (car request)))
               (source (form-source (if (pair? (cdr request)) (cadr request) "(tool-help)")))
               (session (if (= (length request) 3) (caddr request) (default-session)))
               (port (open-input-string source)) (form (read port)))
          (when (eof-object? form) (error "techne tool: expected a Scheme form"))
          (unless (eof-object? (read port)) (error "techne tool: expected one form; use begin or list to batch"))
          (let ((node (session-node session (guard (e (#t #f)) (tool-stamp #:now #t)))))
            (dynamic-wind (lambda () #f)
              (lambda ()
                ;; FORM runs at the session's top level, `project` and
                ;; `input` this call's: what it defines stays for later
                ;; calls, until the session ends or is restarted.
                (let ((value (node-eval node (string-append
                                               "(import (techne tools))\n"
                                               (written `(define project (tool-session-project ,root))) "\n"
                                               (written `(define input ,input)) "\n"
                                               (written form)))))
                  (cond ((eq? value (if #f #f)))
                        ((and text? (string? value)) (display value) (newline))
                        (else (write value) (newline)))))
              (lambda () (node-close node)))))))
