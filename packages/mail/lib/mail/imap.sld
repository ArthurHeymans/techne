;;; IMAP protocol and policy live in Scheme; only TLS is native.
(define-library (mail imap)
  (export imap-connect make-imap-session imap-mailboxes imap-select!
          imap-fetch imap-for-each-message imap-store-flags! imap-close)
  (import (techne))
  (include "imap.scm"))
