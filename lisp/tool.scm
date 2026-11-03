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
(define input
  (and (member "--stdin" (car flags))
       (let loop ((chunks '()))
         (let ((s (read-string 65536 (current-input-port))))
           (if (eof-object? s) (apply string-append (reverse chunks)) (loop (cons s chunks)))))))
(define (written value) (call-with-output-string (lambda (p) (write value p))))
(cond ((or (null? request) (equal? request '("--help")) (equal? request '("-h")))
       (write (tool-help)) (newline))
      ((> (length request) 3) (error "usage: techne tool [--text] [--stdin] ROOT [FORM [SESSION]]; try techne tool --help"))
      (else
        (let* ((root (%repository-canonical-root (car request)))
               (source (if (pair? (cdr request)) (cadr request) "(tool-help)"))
               (session (if (= (length request) 3) (caddr request) "tools"))
               (port (open-input-string source)) (form (read port)))
          (when (eof-object? form) (error "techne tool: expected a Scheme form"))
          (unless (eof-object? (read port)) (error "techne tool: expected one form; use begin or list to batch"))
          (let ((node (node-connect (list executable "node" "--session" session))))
            (dynamic-wind (lambda () #f)
              (lambda ()
                (let ((value (node-eval node (string-append "(import (techne tools))\n"
                   (written `(let ((project (tool-session-project ,root)) (input ,input)) ,form))))))
                  (if (and text? (string? value)) (display value) (write value))
                  (newline)))
              (lambda () (node-close node)))))))
