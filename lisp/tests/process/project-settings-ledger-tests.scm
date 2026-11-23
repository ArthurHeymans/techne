;;; Multi-process concurrency and locking tests for `(techne project-settings)` trust ledger.
(import (techne) (techne project-settings) (techne uuid))
(require "../../test.scm")

(define techne-bin (or (get-environment-variable "TECHNE_BIN") "techne"))
(define tmp-raw (string-append (or (get-environment-variable "TMPDIR") "/tmp")
                               "/techne-trust-ledger-" (uuid-v4)))
(create-directory tmp-raw #o700)
(define tmp (file-truename tmp-raw))

(define (write-text path text)
  (call-with-output-file path (lambda (p) (display text p))))

(define (run-child-script name code)
  (let ((script (string-append tmp "/" name ".scm")))
    (write-text script code)
    (call-with-process techne-bin (list "run" script)
      (lambda (p)
        (let* ((out (process-read-all p 'stdout))
               (err (process-read-all p 'stderr))
               (st (process-wait p)))
          (list st out err))))))

(define (spawn-child-script name code)
  (spawn (lambda () (run-child-script name code))))

(define (wait-child t)
  (task-join t))

(define proj (string-append tmp "/proj"))
(create-directory proj #o700)
(define canon-proj (project-settings-canonical-dir proj))

;;; 1. Stale-snapshot writer must not resurrect a concurrent denial or revocation.
(let ((store (string-append tmp "/stale-store.scm")))
  (parameterize ((default-trust-store-path store))
    (project-trust-clear-memory!)
    (project-trust-approve! proj 'opt-a "val-a")
    (check "stale test: initial opt-a trusted" 'trusted
           (project-trust-verdict canon-proj 'opt-a "val-a"))
    ;; Process A denies opt-a on disk while parent still holds the stale snapshot.
    (let ((res (run-child-script
                "child-deny"
                (string-append
                 "(import (techne) (techne project-settings))\n"
                 "(parameterize ((default-trust-store-path " (repr store) "))\n"
                 "  (project-trust-deny! " (repr proj) " 'opt-a \"val-a\"))\n"))))
      (check "child-deny exits cleanly" 0 (car res)))
    ;; Parent (still holding stale memory where opt-a was trusted) approves opt-b.
    (project-trust-approve! proj 'opt-b "val-b")
    ;; Reload cleanly from disk and verify opt-a denial was preserved alongside opt-b approval.
    (project-trust-clear-memory!)
    (project-trust-load! store)
    (check "stale writer preserves external denial of opt-a" 'denied
           (project-trust-verdict canon-proj 'opt-a "val-a"))
    (check "stale writer persists its own approval of opt-b" 'trusted
           (project-trust-verdict canon-proj 'opt-b "val-b"))
    ;; Now test external revocation: Process A revokes opt-b while parent holds stale snapshot.
    (let ((res (run-child-script
                "child-revoke"
                (string-append
                 "(import (techne) (techne project-settings))\n"
                 "(parameterize ((default-trust-store-path " (repr store) "))\n"
                 "  (project-trust-revoke! " (repr proj) " 'opt-b))\n"))))
      (check "child-revoke exits cleanly" 0 (car res)))
    (project-trust-approve! proj 'opt-c "val-c")
    (project-trust-clear-memory!)
    (project-trust-load! store)
    (check "stale writer preserves external revocation of opt-b" 'unknown
           (project-trust-verdict canon-proj 'opt-b "val-b"))
    (check "stale writer keeps opt-a denied" 'denied
           (project-trust-verdict canon-proj 'opt-a "val-a"))
    (check "stale writer persists opt-c" 'trusted
           (project-trust-verdict canon-proj 'opt-c "val-c"))))

;;; 2. Active lock contention: a child writer blocks until the lock holder finishes,
;;; then reloads the freshly written ledger and merges its update.
(let* ((store (string-append tmp "/contend-store.scm")))
  (parameterize ((default-trust-store-path store))
    (project-trust-clear-memory!)
    (project-trust-approve! proj 'opt-a "val-a")
    (let ((lock (file-try-lock-pinned tmp "contend-store.scm.lock")))
      (check "parent acquired ledger lock" #t (and lock #t))
      (check "second non-blocking try-lock fails while held" #f
             (file-try-lock-pinned tmp "contend-store.scm.lock"))
      (let ((child (spawn-child-script
                    "child-contend"
                    (string-append
                     "(import (techne) (techne project-settings))\n"
                     "(parameterize ((default-trust-store-path " (repr store) "))\n"
                     "  (project-trust-approve! " (repr proj) " 'opt-b \"val-b\"))\n"))))
        ;; Wait long enough for child to reach the lock loop, then update store while holding lock.
        (sleep 120)
        (project-trust-clear-memory!)
        (project-trust-load! store)
        (check "child has not written opt-b while parent holds lock" 'unknown
               (project-trust-verdict canon-proj 'opt-b "val-b"))
        ;; Write opt-a = denied directly to disk while still holding the lock.
        (let ((d-a (value-trust-digest canon-proj 'opt-a "val-a")))
          (file-lock-replace-private!
           lock "contend-store.scm"
           (call-with-output-string
            (lambda (p)
              (display "(techne-project-trust 1\n  " p)
              (write (list canon-proj 'opt-a d-a 'denied) p)
              (display "\n)\n" p)))))
        (file-unlock lock)
        (let ((res (wait-child child)))
          (check "contending child exits cleanly after unlock" 0 (car res))))
      (project-trust-clear-memory!)
      (project-trust-load! store)
      (check "contending child preserved parent's denial written under lock" 'denied
             (project-trust-verdict canon-proj 'opt-a "val-a"))
      (check "contending child persisted its own approval after lock release" 'trusted
             (project-trust-verdict canon-proj 'opt-b "val-b")))))

;;; 3. Concurrent multi-process write storm: 4 simultaneous writers must lose zero updates.
(let ((store (string-append tmp "/storm-store.scm")))
  (let ((children
         (map (lambda (i)
                (let ((opt (string->symbol (string-append "storm-opt-" (number->string i))))
                      (val (string-append "val-" (number->string i))))
                  (spawn-child-script
                   (string-append "storm-" (number->string i))
                   (string-append
                    "(import (techne) (techne project-settings))\n"
                    "(parameterize ((default-trust-store-path " (repr store) "))\n"
                    "  (project-trust-approve! " (repr proj) " '" (symbol->string opt) " " (repr val) "))\n"))))
              '(1 2 3 4))))
    (for-each (lambda (p)
                (let ((res (wait-child p)))
                  (check "storm child exited cleanly" 0 (car res))))
              children)
    (parameterize ((default-trust-store-path store))
      (project-trust-clear-memory!)
      (project-trust-load! store)
      (for-each (lambda (i)
                  (let ((opt (string->symbol (string-append "storm-opt-" (number->string i))))
                        (val (string-append "val-" (number->string i))))
                    (check (string-append "storm preserved " (symbol->string opt)) 'trusted
                           (project-trust-verdict canon-proj opt val))))
                '(1 2 3 4)))))

;;; 4. Symlinked .lock file fails closed without modifying target or creating store.
(let* ((store (string-append tmp "/sym-store.scm"))
       (lock-path (string-append store ".lock"))
       (victim (string-append tmp "/victim.txt")))
  (write-text victim "untouched")
  (call-with-process "ln" (list "-s" victim lock-path) (lambda (p) (process-wait p)))
  (parameterize ((default-trust-store-path store))
    (project-trust-clear-memory!)
    (check "symlinked lock file fails closed on approve" #t
           (guard (e (#t #t))
             (project-trust-approve! proj 'opt-x "val-x")
             #f))
    (check "store file was not created when lock was a symlink" #f (file-exists? store))
    (check "symlink target remained untouched" "untouched" (read-regular-file victim 1024))))

;;; 5. Symlinked ancestor directory on write fails closed with ZERO side effects
;;;    (creates neither sub/, nor .lock, nor project-trust.scm in the target).
(let* ((real-target (string-append tmp "/alias-target"))
       (alias (string-append tmp "/alias-link"))
       (alias-store (string-append alias "/sub/project-trust.scm")))
  (create-directory real-target #o700)
  (call-with-process "ln" (list "-s" real-target alias) (lambda (p) (process-wait p)))
  (parameterize ((default-trust-store-path alias-store))
    (project-trust-clear-memory!)
    (check "symlinked ancestor trust-store write fails closed" #t
           (guard (e (#t #t))
             (project-trust-approve! proj 'opt-alias "val-alias")
             #f))
    (check "rejected symlinked ancestor did not create sub/" #f
           (file-exists? (string-append real-target "/sub")))
    (check "rejected symlinked ancestor did not create lock" #f
           (file-exists? (string-append real-target "/sub/project-trust.scm.lock")))
    (check "rejected symlinked ancestor did not create store" #f
           (file-exists? (string-append real-target "/sub/project-trust.scm")))))

;;; 6. Parent directory swapped to a symlink to attacker dir after lock acquisition
;;;    must keep both reload and publication anchored to the original directory fd.
(let* ((orig-dir (string-append tmp "/pinned-state"))
       (moved-dir (string-append tmp "/pinned-state-moved"))
       (attacker-dir (string-append tmp "/pinned-attacker"))
       (attacker-file (string-append attacker-dir "/project-trust.scm")))
  (create-directory attacker-dir #o700)
  (write-text attacker-file "ATTACKER-UNCHANGED")
  (let ((lock (file-try-lock-pinned orig-dir "project-trust.scm.lock")))
    (check "acquired pinned lock on new directory" #t (and lock #t))
    (file-lock-replace-private! lock "project-trust.scm" "v1")
    ;; Rename orig-dir away and replace its path with a symlink to attacker-dir.
    (rename-file orig-dir moved-dir)
    (call-with-process "ln" (list "-s" attacker-dir orig-dir) (lambda (p) (process-wait p)))
    (check "pinned lock still reads from original directory fd" "v1"
           (file-lock-read-text lock "project-trust.scm" 4096))
    (file-lock-replace-private! lock "project-trust.scm" "v2")
    (file-unlock lock)
    (check "pinned write landed in original directory" "v2"
           (read-regular-file (string-append moved-dir "/project-trust.scm") 1024))
    (check "attacker directory remained untouched" "ATTACKER-UNCHANGED"
           (read-regular-file attacker-file 1024))))

(delete-path tmp #t)
(test-summary)
