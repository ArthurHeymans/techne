;;; A tool client, not an agent. Uses the existing named node sessions.
(import (techne) (techne tools))
(define args (cdr (command-line)))
(define executable (car args))
;; Flags before ROOT: --text displays a string answer as it is, not
;; written; --stdin binds `input` in FORM to the text read from stdin, so
;; text needs no escaping (patches for `tool-apply!`, say).
(define flags
  (let loop ((rest (cdr args)) (seen '()))
    (if (and (pair? rest) (member (car rest) '("--text" "--stdin")))
        (loop (cdr rest) (cons (car rest) seen))
        (cons seen rest))))
(define request (cdr flags))
(define text? (and (member "--text" (car flags)) #t))
(define (read-all port)
  (let loop ((chunks '()))
    (let ((s (read-string 65536 port)))
      (if (eof-object? s) (apply string-append (reverse chunks)) (loop (cons s chunks))))))
(define input (and (member "--stdin" (car flags)) (read-all (current-input-port))))
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
(cond ((or (null? request) (equal? request '("--help")) (equal? request '("-h")))
       (write (tool-help)) (newline))
      ((> (length request) 3) (error "usage: techne tool [--text] [--stdin] ROOT [FORM|-|@FILE [SESSION]]; try techne tool --help"))
      (else
        (let* ((root (%repository-canonical-root (car request)))
               (source (form-source (if (pair? (cdr request)) (cadr request) "(tool-help)")))
               (session (if (= (length request) 3) (caddr request) "tools"))
               (port (open-input-string source)) (form (read port)))
          (when (eof-object? form) (error "techne tool: expected a Scheme form"))
          (unless (eof-object? (read port)) (error "techne tool: expected one form; use begin or list to batch"))
          (let ((node (session-node session (guard (e (#t #f)) (tool-stamp #:now #t)))))
            (dynamic-wind (lambda () #f)
              (lambda ()
                (let ((value (node-eval node (string-append "(import (techne tools))\n"
                   (written `(let ((project (tool-session-project ,root)) (input ,input)) ,form))))))
                  (if (and text? (string? value)) (display value) (write value))
                  (newline)))
              (lambda () (node-close node)))))))
