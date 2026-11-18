;;; Passwords from an authinfo or netrc file, as Emacs's auth-source and
;;; curl read them: lines of `machine HOST login USER port PORT password
;;; SECRET`, a value with spaces in double quotes. Only a plain file: one
;;; encrypted with gpg is decrypted by whoever keeps it so (sops-nix, a
;;; gpg agent), not here. Nothing read is kept or logged.

(define-library (techne authinfo)
  (export authinfo-password authinfo-entries)
  (import (techne))
  (begin
    ;; The words of LINE, a quoted one with its quotes and backslashes taken.
    (define (words line)
      (let loop ((cs (string->list line)) (word #f) (quoted? #f) (out '()))
        (define (done) (if word (cons (list->string (reverse word)) out) out))
        (cond ((null? cs) (reverse (done)))
              (quoted? (cond ((char=? (car cs) #\") (loop (cdr cs) (or word '()) #f out))
                             ((and (char=? (car cs) #\\) (pair? (cdr cs))) (loop (cddr cs) (cons (cadr cs) (or word '())) #t out))
                             (else (loop (cdr cs) (cons (car cs) (or word '())) #t out))))
              ((char=? (car cs) #\") (loop (cdr cs) (or word '()) #t out))
              ((char-whitespace? (car cs)) (loop (cdr cs) #f #f (done)))
              (else (loop (cdr cs) (cons (car cs) (or word '())) #f out)))))

    (define (default-file)
      (string-append (or (get-environment-variable "HOME") ".") "/.authinfo"))

    (define (authinfo-entries #:file [file (default-file)])
      "Return the entries of authinfo FILE, ~/.authinfo by default, as alists.
Each has machine, login, port and password, those the line gives: a line
of `key value` pairs, `#` starting a comment. A missing file has none."
      (if (not (file-exists? file)) '()
          (filter-map
           (lambda (line)
             (let ((ws (words line)))
               (and (pair? ws) (not (string-prefix? "#" (car ws)))
                    (let pairs ((ws ws) (out '()))
                      (if (or (null? ws) (null? (cdr ws))) (and (pair? out) (reverse out))
                          (pairs (cddr ws) (cons (cons (string->symbol (car ws)) (cadr ws)) out)))))))
           (string-split (call-with-input-file file (lambda (p) (let ((s (read-string 1048576 p))) (if (eof-object? s) "" s)))) "\n"))))

    (define (authinfo-password #:host host #:user [user #f] #:port [port #f] #:file [file (default-file)])
      "Return the password of the first entry of FILE for HOST, or #f.
USER, if given, must be its login and PORT, if given, its port; an
entry without a port matches any, as auth-source has it. FILE is
~/.authinfo by default."
      (let ((port (and port (if (number? port) (number->string port) port))))
        (let ((entry (find (lambda (e)
                             (and (equal? (assq-ref e 'machine) host)
                                  (or (not user) (equal? (assq-ref e 'login) user))
                                  (or (not port) (not (assq-ref e 'port)) (equal? (assq-ref e 'port) port))))
                           (authinfo-entries #:file file))))
          (and entry (assq-ref entry 'password)))))

    (define (assq-ref alist key)
      (let ((e (assq key alist))) (and e (cdr e))))))
