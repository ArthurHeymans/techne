;;; Keep options and entry points available without loading the mail runtime.
(import (techne editor))
(require "../files.scm")


(define-option mail-accounts '()
  "Mail accounts, as a list of alists, configured in Scheme.
An IMAP account has id, kind (imap), host, port, user and mailboxes.
A Maildir account has id, kind (maildir) and path, a tree of folders as
mbsync keeps one; folders lists the only ones to read, skip the ones not
to, and mailbox makes the path one folder of that name.
Sending uses from, smtp-host, smtp-port and smtp-user. The imap-security
and smtp-security settings default to implicit-tls (IMAP 993, SMTP 465).
Set either to starttls for mandatory STARTTLS (IMAP 143, SMTP 587).
Passwords are never stored here: `mail-credential-provider` supplies them."
  #:type 'any)
(define-option mail-credential-provider #f
  "Procedure (account purpose) returning a secret for imap or smtp.
Use a secret manager in this procedure. Secrets are not persisted or logged.
No password is read in the visible minibuffer."
  #:type 'any)
(define-option mail-database #f
  "Local mail SQLite file, or #f for the XDG data directory.
The default is XDG_DATA_HOME/techne/mail/cache.sqlite. Set this before
opening mail in a new runtime. This file holds private messages and drafts."
  #:type 'any)
(define-option mail-prefer-html #t
  "Whether messages with HTML use the private rich reader by default.
Scripts, styles, embedded content and remote images are never loaded."
  #:type 'boolean)

(define-option mail-reader-position 'bottom
  "Position of the mail reader: bottom (the default) or side.
Below the list, rows have the frame's width for subjects. This is a
global workspace preference. Short bottom layouts and narrow side layouts
use one pane; f maximizes/restores a pane. To switch an open workspace
use mail-toggle-reader-position, or set this option in your mail
configuration."
  #:type '(one-of bottom side))

(define %mail-runtime (string-append (directory-of (current-module)) "../mail/runtime.scm"))

;; Only M-x mail is needed before visiting mail. The runtime publishes the
;; remaining commands and replaces this entry when it loads.
(define-command (mail s n)
  "Open the mail dashboard. Configure accounts with `mail-accounts`."
  (load-package 'mail-runtime %mail-runtime)
  (let ((implementation (command 'mail)))
    (when (eq? implementation mail) (error "Mail command was not loaded"))
    (with-scope (package-scope (find-package 'mail-runtime)) (implementation s n))))
