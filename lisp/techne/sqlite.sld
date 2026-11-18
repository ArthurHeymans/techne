;;; Bounded asynchronous SQLite. The host installs techne-store first.
;;; SQL/schema/migrations/search remain Scheme; Rust supplies only the database
;;; and capability/lifetime boundary. Public procedures document their limits.
(define-library (techne sqlite)
  (export sqlite-open sqlite-execute sqlite-query sqlite-close sqlite-closed?
          call-with-sqlite call-with-sqlite-lease)
  (import (techne)))
