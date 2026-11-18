(import (techne) (techne authinfo) (techne uuid))
(require "../test.scm")
(define file (string-append (or (get-environment-variable "TMPDIR") "/tmp") "/techne-authinfo-" (uuid-v4)))
(call-with-output-file file
  (lambda (p)
    (display "# a comment\n" p)
    (display "machine imap.example.org login ada@example.org port 993 password imap-secret\n" p)
    (display "machine smtp.example.org login ada@example.org port 587 password \"with space \\\" quote\"\n" p)
    (display "machine smtp.example.org login ben@example.org password ben-secret\n" p)
    (display "\n" p)))
(check "by host and login" "imap-secret" (authinfo-password #:host "imap.example.org" #:user "ada@example.org" #:file file))
(check "a quoted password, its escapes taken" "with space \" quote"
       (authinfo-password #:host "smtp.example.org" #:user "ada@example.org" #:file file))
(check "a port given must be the entry's" #f
       (authinfo-password #:host "smtp.example.org" #:user "ada@example.org" #:port 465 #:file file))
(check "an entry without a port matches any" "ben-secret"
       (authinfo-password #:host "smtp.example.org" #:user "ben@example.org" #:port 465 #:file file))
(check "the first entry of the host without a login" "with space \" quote" (authinfo-password #:host "smtp.example.org" #:file file))
(check "no entry" #f (authinfo-password #:host "nowhere.example.org" #:file file))
(check "a missing file has none" #f (authinfo-password #:host "imap.example.org" #:file (string-append file "-absent")))
(check "entries, comments left out" 3 (length (authinfo-entries #:file file)))
(delete-file file)
(test-summary)
