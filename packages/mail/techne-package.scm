;;; Mail: a native mail client for the editor, and the libraries it is
;;; made of under lib/, (mail imap), (mail smtp), (mail mime) and the
;;; rest, which a program without the editor imports as well
;;; (lisp/techne/package.sld). The editor reads this as data at start,
;;; for the options and the command below; editor.scm loads when `mail`
;;; is first run.
(package mail
  (editor "editor.scm"
    (commands
      (mail "Open the mail dashboard. Configure accounts with `mail-accounts`."))
    (options
      (mail-accounts () any
        "Mail accounts, as a list of alists, configured in Scheme.
An IMAP account has id, kind (imap), host, port, user and mailboxes.
A Maildir account has id, kind (maildir) and path, a tree of folders as
mbsync keeps one; folders lists the only ones to read, skip the ones not
to, and mailbox makes the path one folder of that name.
Sending uses from, smtp-host, smtp-port and smtp-user. The imap-security
and smtp-security settings default to implicit-tls (IMAP 993, SMTP 465).
Set either to starttls for mandatory STARTTLS (IMAP 143, SMTP 587).
Passwords are never stored here: `secret-sources` (or per-account `secret` /
`smtp-secret` refs, or `mail-credential-provider`) supplies them.")
      (mail-credential-provider #f any
        "Procedure (account purpose) returning a secret for imap or smtp.
Overrides `secret-sources` when set; may return a string, `secret` or
`secret-ref`. Secrets are not persisted or logged. No password is read
in the visible minibuffer.")
      (mail-database #f any
        "Local mail SQLite file, or #f for the XDG data directory.
The default is XDG_DATA_HOME/techne/mail/cache.sqlite. Set this before
opening mail in a new runtime. This file holds private messages and drafts.")
      (mail-prefer-html #t boolean
        "Whether messages with HTML use the private rich reader by default.
Scripts, styles, embedded content and remote images are never loaded.")
      (mail-reader-position bottom (one-of bottom side)
        "Position of the mail reader: bottom (the default) or side.
Below the list, rows have the frame's width for subjects. This is a
global workspace preference. Short bottom layouts and narrow side layouts
use one pane; f maximizes/restores a pane. To switch an open workspace
use mail-toggle-reader-position, or set this option in your mail
configuration."))))
