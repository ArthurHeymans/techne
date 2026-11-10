;;; Authenticating clients for a supervisor (doc/serverless.md,
;;; "Authority"): whoever accepts a connection asks here whether the
;;; token it was shown holds, before anything of the connection reaches
;;; a worker (`%auth-next`, `%auth-reply`: crates/techne-host/src/ask.rs).
;;; The answer is whom the token is for, its principal, by which a REPL
;;; keeps each one's sessions apart; or a refusal that says no more than
;;; that.

(require "tokens.scm")
(import (techne crypto))

(provide token-key-in auth-serve)

(define (token-key-in directory)
  "Return the token key kept in DIRECTORY, a bytevector.
One is made and kept there, in the file `token-key`, if none is; the
directory is made if missing, in one that is there. The world must be
granted entropy."
  (let ((file (string-append directory "/token-key")))
    (unless (file-exists? directory) (create-directory directory #o700))
    (unless (file-exists? file)
      (replace-file file (hex-encode (make-token-key))))
    (hex-decode (call-with-input-file file read-line))))

(define (auth-serve key project scope #:revoked? [revoked? (lambda (id) #f)])
  "Answer whether the tokens asked about allow SCOPE on PROJECT, for ever.
Each is a string, checked with KEY and REVOKED? as `verify-token` does.
One that holds and allows it is answered with its subject, a string;
any other is refused, without saying why."
  (let loop ()
    (let* ((asked (%auth-next))
           (id (car asked))
           (claims (guard (e (#t #f)) (verify-token key (cadr asked) #:revoked? revoked?))))
      (if (and claims (token-allows? claims project scope))
          (%auth-reply id #t (dict-ref claims "sub"))
          (%auth-reply id #f "unauthenticated"))
      (loop))))
