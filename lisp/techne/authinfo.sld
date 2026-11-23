;;; Passwords from an authinfo or netrc file, as Emacs's auth-source and
;;; curl read them: lines of `machine HOST login USER port PORT password
;;; SECRET`, a value with spaces in double quotes. Plain files and GPG-
;;; encrypted `.gpg`/`.asc` files are supported; GPG runs in the user's
;;; own environment with `--no-tty` so `gpg-agent` handles pinentry.
;;; Nothing read is kept or logged.

(define-library (techne authinfo)
  (export authinfo-parse authinfo-default-files authinfo-entries authinfo-match authinfo-password)
  (import (techne) (techne trusted-process))
  (begin
    (define %max-authinfo-chars 1048576)

    (define (assq-ref alist key)
      (let ((e (assq key alist))) (and e (cdr e))))

    (define (encrypted-authinfo? path)
      (let ((lower (string-downcase path)))
        (or (string-suffix? ".gpg" lower)
            (string-suffix? ".asc" lower))))

    (define (strip-cr line)
      (let ((n (string-length line)))
        (if (and (> n 0) (char=? (string-ref line (- n 1)) #\return))
            (substring line 0 (- n 1))
            line)))

    ;; The words of LINE, a quoted one with its quotes and backslashes taken.
    (define (words line)
      (let loop ((cs (string->list (strip-cr line))) (word #f) (quoted? #f) (out '()))
        (define (done) (if word (cons (list->string (reverse word)) out) out))
        (cond ((null? cs) (reverse (done)))
              (quoted? (cond ((char=? (car cs) #\") (loop (cdr cs) (or word '()) #f out))
                             ((and (char=? (car cs) #\\) (pair? (cdr cs)))
                              (loop (cddr cs) (cons (cadr cs) (or word '())) #t out))
                             (else (loop (cdr cs) (cons (car cs) (or word '())) #t out))))
              ((char=? (car cs) #\") (loop (cdr cs) (or word '()) #t out))
              ((char-whitespace? (car cs)) (loop (cdr cs) #f #f (done)))
              (else (loop (cdr cs) (cons (car cs) (or word '())) #f out)))))

    (define (normalize-port p)
      (cond ((not p) #f)
            ((number? p) (number->string p))
            ((symbol? p) (normalize-port (symbol->string p)))
            ((string? p)
             (let ((s (string-downcase (string-trim p))))
               (cond ((string=? s "imaps") "993")
                     ((string=? s "imap") "143")
                     ((string=? s "smtps") "465")
                     ((string=? s "submission") "587")
                     ((string=? s "smtp") "25")
                     ((string=? s "https") "443")
                     ((string=? s "http") "80")
                     (else s))))
            (else #f)))

    (define (gpg-decrypt-file path gpg)
      (let* ((res (run-trusted-tool gpg (list "--decrypt" "--quiet" "--no-tty" "--" path)
                                    #:max-stdout %max-authinfo-chars
                                    #:strict-baseline #t
                                    #:label "authinfo"))
             (status (car res))
             (out (cadr res)))
        (unless (equal? status 0)
          ;; Fixed, redacted error: never forward raw helper stderr.
          (error (string-append "authinfo: gpg decryption failed for " path)))
        out))

    (define (read-authinfo-file path gpg)
      (let ((resolved (expand-trusted-home path)))
        (cond ((not (file-exists? resolved)) #f)
              ((encrypted-authinfo? resolved) (gpg-decrypt-file resolved gpg))
              (else
               (call-with-input-file resolved
                 (lambda (p)
                   (let ((s (read-string (+ %max-authinfo-chars 1) p)))
                     (cond ((eof-object? s) "")
                           ((> (string-length s) %max-authinfo-chars)
                            (error (string-append "authinfo: file exceeded size limit: " resolved)))
                           (else s)))))))))

    (define (authinfo-parse text)
      "Return the authinfo entries parsed from TEXT as a list of alists.
Each entry maps key symbols such as `machine`, `login`, `port` and
`password` to their values. Blank lines and `#` comments are ignored."
      (if (not (string? text)) '()
          (filter-map
           (lambda (line)
             (let ((ws (words line)))
               (and (pair? ws) (not (string-prefix? "#" (car ws)))
                    (let pairs ((ws ws) (out '()))
                      (if (or (null? ws) (null? (cdr ws))) (and (pair? out) (reverse out))
                          (pairs (cddr ws) (cons (cons (string->symbol (car ws)) (cadr ws)) out)))))))
           (string-split text "\n"))))

    (define (authinfo-default-files)
      "Return the default authinfo paths, `~/.authinfo.gpg` then `~/.authinfo`.
Returns the empty list when `HOME` is unset or not an existing absolute path."
      (let ((home (trusted-home)))
        (if home
            (list (string-append home "/.authinfo.gpg")
                  (string-append home "/.authinfo"))
            '())))

    (define (target-files file)
      (cond ((not file) (authinfo-default-files))
            ((list? file) file)
            (else (list file))))

    (define (authinfo-entries #:file [file #f] #:gpg [gpg "gpg"])
      "Return the entries of authinfo FILE as alists.
When FILE is #f, read `~/.authinfo.gpg` and `~/.authinfo` in order.
FILE may also be a path string or a list of paths; `.gpg` and `.asc`
files are decrypted with GPG. Missing files contribute no entries."
      (apply append
             (map (lambda (path)
                    (let ((text (read-authinfo-file path gpg)))
                      (if text (authinfo-parse text) '())))
                  (target-files file))))

    (define (authinfo-match entries #:host host #:user [user #f] #:port [port #f] #:login-suffix [login-suffix #f])
      "Return the first password in ENTRIES matching HOST, or #f.
HOST comparison is case-insensitive. USER, if given, must equal the
entry's login; otherwise LOGIN-SUFFIX, if given, must match the end of
the login. PORT, if given, matches numerically or by service name
(such as `imaps` or `smtps`) unless the entry omits a port."
      (let ((want-port (normalize-port port)))
        (let ((entry (find (lambda (e)
                             (let ((machine (assq-ref e 'machine))
                                   (login (assq-ref e 'login))
                                   (entry-port (normalize-port (assq-ref e 'port))))
                               (and (string? machine) (string? host) (string-ci=? machine host)
                                    (cond (user (equal? login user))
                                          (login-suffix (and (string? login) (string-suffix? login-suffix login)))
                                          (else #t))
                                    (or (not want-port) (not entry-port) (equal? entry-port want-port)))))
                           entries)))
          (and entry (assq-ref entry 'password)))))

    (define (authinfo-password #:host host #:user [user #f] #:port [port #f] #:file [file #f] #:gpg [gpg "gpg"])
      "Return the password of the first entry in FILE for HOST, or #f.
USER, if given, must match its login and PORT, if given, its port; an
entry without a port matches any. FILE defaults to `~/.authinfo.gpg`
then `~/.authinfo`, decrypted with GPG when ending in `.gpg` or `.asc`."
      (let loop ((files (target-files file)))
        (if (null? files) #f
            (let* ((text (read-authinfo-file (car files) gpg))
                   (pw (and text (authinfo-match (authinfo-parse text) #:host host #:user user #:port port))))
              (or pw (loop (cdr files)))))))))
