(import (techne) (techne authinfo) (techne uuid))
(require "../../test.scm")

(define tmp (string-append (or (get-environment-variable "TMPDIR") "/tmp") "/techne-authinfo-" (uuid-v4)))
(create-directory tmp #o700)

(define (write-text path text)
  (call-with-output-file path (lambda (p) (display text p))))

(define (make-script path body)
  (write-text path body)
  (call-with-process "chmod" (list "+x" path) (lambda (p) (process-wait p))))

(define (rm-rf path)
  (call-with-process "rm" (list "-rf" "--" path) (lambda (p) (process-wait p))))

(define file (string-append tmp "/plain"))
(write-text file
  (string-append
   "# a comment\n"
   "machine imap.example.org login ada@example.org port 993 password imap-secret\n"
   "machine smtp.example.org login ada@example.org port 587 password \"with space \\\" quote\"\r\n"
   "machine smtp.example.org login ben@example.org password ben-secret\n"
   "machine api.github.com login arthur^forge password forge-secret\n"
   "\n"))

;;; Plain file parsing & matching
(check "by host and login" "imap-secret" (authinfo-password #:host "imap.example.org" #:user "ada@example.org" #:file file))
(check "a quoted password, its escapes taken (CRLF)" "with space \" quote"
       (authinfo-password #:host "smtp.example.org" #:user "ada@example.org" #:file file))
(check "a port given must be the entry's" #f
       (authinfo-password #:host "smtp.example.org" #:user "ada@example.org" #:port 465 #:file file))
(check "an entry without a port matches any" "ben-secret"
       (authinfo-password #:host "smtp.example.org" #:user "ben@example.org" #:port 465 #:file file))
(check "the first entry of the host without a login" "with space \" quote" (authinfo-password #:host "smtp.example.org" #:file file))
(check "no entry" #f (authinfo-password #:host "nowhere.example.org" #:file file))
(check "a missing file has none" #f (authinfo-password #:host "imap.example.org" #:file (string-append file "-absent")))
(check "entries, comments left out" 4 (length (authinfo-entries #:file file)))

;;; Pure parse and login-suffix matching
(let ((parsed (authinfo-parse "machine api.github.com login arthur^forge password tok-1\n")))
  (check "pure parse count" 1 (length parsed))
  (check "match by login-suffix" "tok-1" (authinfo-match parsed #:host "api.github.com" #:login-suffix "^forge"))
  (check "login-suffix mismatch" #f (authinfo-match parsed #:host "api.github.com" #:login-suffix "^other")))

;;; Default files order: ~/.authinfo.gpg before ~/.authinfo
(let ((defaults (authinfo-default-files)))
  (check "default files count" 2 (length defaults))
  (check "gpg checked first" #t (string-suffix? "/.authinfo.gpg" (car defaults)))
  (check "plain checked second" #t (string-suffix? "/.authinfo" (cadr defaults))))

;;; GPG decryption via stub (verifies flags, -- separator, and stdout non-leak on error)
(define gpg-file (string-append tmp "/authinfo.gpg"))
(write-text gpg-file "placeholder-ciphertext")
(define stub-gpg (string-append tmp "/stub-gpg"))
(make-script stub-gpg
  (string-append
   "#!/bin/sh\n"
   "if [ \"$1\" != \"--decrypt\" ] || [ \"$2\" != \"--quiet\" ] || [ \"$3\" != \"--no-tty\" ] || [ \"$4\" != \"--\" ]; then\n"
   "  echo \"bad flags: $*\" >&2\n"
   "  exit 2\n"
   "fi\n"
   "echo 'machine gpg.example.org login ada port 993 password decrypted-pw'\n"))

(check "gpg decrypted lookup" "decrypted-pw"
       (authinfo-password #:host "gpg.example.org" #:user "ada" #:port 993 #:file gpg-file #:gpg stub-gpg))
(check "list of files short-circuits in order" "decrypted-pw"
       (authinfo-password #:host "gpg.example.org" #:user "ada" #:file (list gpg-file file) #:gpg stub-gpg))
(check "list of files falls through to second" "imap-secret"
       (authinfo-password #:host "imap.example.org" #:user "ada@example.org" #:file (list gpg-file file) #:gpg stub-gpg))

;;; Case-insensitive host & service-name port normalization
(let ((svc-file (string-append tmp "/svc")))
  (write-text svc-file
    (string-append
     "machine IMAP.Example.ORG login ada port imaps password imaps-pw\n"
     "machine smtp.example.org login ada port submission password sub-pw\n"
     "machine smtps.example.org login ada port smtps password smtps-pw\n"))
  (check "case-insensitive host match" "imaps-pw"
         (authinfo-password #:host "imap.example.org" #:user "ada" #:port 993 #:file svc-file))
  (check "service name submission matches 587" "sub-pw"
         (authinfo-password #:host "smtp.example.org" #:user "ada" #:port 587 #:file svc-file))
  (check "service name smtps matches 465" "smtps-pw"
         (authinfo-password #:host "smtps.example.org" #:user "ada" #:port 465 #:file svc-file)))

;;; Oversized plain file is rejected instead of silently truncated
(let ((big-file (string-append tmp "/oversized")))
  (call-with-output-file big-file
    (lambda (p)
      (let ((chunk (make-string 4096 #\a)))
        (let loop ((n 0))
          (when (< n 260)
            (display chunk p)
            (loop (+ n 1)))))))
  (let ((err (guard (e (#t (error-object-message e)))
               (authinfo-entries #:file big-file)
               #f)))
    (check "oversized plain file raises error" #t
           (and (string? err) (string-contains err "size limit") #t))))

;;; GPG error never leaks stdout or raw stderr
(define fail-gpg (string-append tmp "/fail-gpg"))
(make-script fail-gpg
  "#!/bin/sh\necho 'TOP-SECRET-LEAK'\necho 'gpg: decryption failed: SECRET-STDERR-LEAK' >&2\nexit 2\n")
(let ((err (guard (e (#t (error-object-message e)))
             (authinfo-password #:host "gpg.example.org" #:file gpg-file #:gpg fail-gpg)
             #f)))
  (check "gpg failure raises error" #t (string? err))
  (check "gpg failure reports redacted message" #t (and (string? err) (string-contains err "gpg decryption failed") #t))
  (check "gpg failure never leaks stdout" #f (and (string? err) (string-contains err "TOP-SECRET-LEAK") #t))
  (check "gpg failure never leaks raw stderr" #f (and (string? err) (string-contains err "SECRET-STDERR-LEAK") #t)))

;;; GPG writing > 64 KiB to stderr never deadlocks
(define noisy-gpg (string-append tmp "/noisy-gpg"))
(make-script noisy-gpg
  (string-append
   "#!/bin/sh\n"
   "dd if=/dev/zero bs=1024 count=128 2>/dev/null | tr '\\0' 'x' >&2\n"
   "echo 'machine noisy.example.org login ada password noisy-pw'\n"))
(check "noisy stderr > 64 KiB does not deadlock" "noisy-pw"
       (authinfo-password #:host "noisy.example.org" #:user "ada" #:file gpg-file #:gpg noisy-gpg))

(rm-rf tmp)
(test-summary)
