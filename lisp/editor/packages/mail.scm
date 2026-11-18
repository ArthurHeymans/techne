;;; Keep options and entry points available without loading the mail runtime.
(import (techne editor))
(require "../files.scm")
(require "../dispatch.scm")

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

(define %mail-runtime (string-append (directory-of (current-module)) "mail-runtime.scm"))

(define-syntax %mail-entry
  (syntax-rules ()
    ((_ name doc)
     (define-command (name s n) doc
       (unless (find-package 'mail-runtime)
         (load-package 'mail-runtime %mail-runtime))
       (let ((implementation (command 'name)))
         (when (eq? implementation name) (error "Mail command was not loaded" 'name))
         (with-scope (package-scope (find-package 'mail-runtime)) (implementation s n)))))))

(%mail-entry mail "Open the mail dashboard. Configure accounts with `mail-accounts`.")
(%mail-entry mail-dashboard "Show the mail dashboard.")
(%mail-entry mail-inbox "List the Inbox of every mail account.")
(%mail-entry mail-unread "List the unread mail of every account.")
(%mail-entry mail-starred "List the starred mail of every account.")
(%mail-entry mail-all "List all mail of every account.")
(%mail-entry mail-search "Search the local mail cache.")
(%mail-entry mail-groups "Choose a mail group, account, or folder to open.")
(%mail-entry mail-sync "Fetch configured IMAP or Maildir accounts into the local cache.")
(%mail-entry mail-sync-status "Show progress and problems from the last mail sync.")
(%mail-entry mail-compose "Compose a new message.")
(%mail-entry mail-drafts "List the saved mail drafts.")
