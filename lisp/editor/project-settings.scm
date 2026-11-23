;;; Editor integration for per-project settings (`.techne.scm` and
;;; `.editorconfig`): zero-I/O mode-line status, lifecycle hooks,
;;; undoable before-save formatting edits, change notifications,
;;; and lazy interactive command registration (`project-settings/runtime.scm`).

(require "session.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "files.scm")
(require "buffers.scm")
(require "operations.scm")
(require "remote.scm")
(require "project-settings-bridge.scm")
(require "../schema.scm")

(provide project-settings-approve! project-settings-deny! project-settings-revoke!
         %interactive-approve! %interactive-deny! %interactive-revoke!
         add-project-settings-changed-hook! remove-project-settings-changed-hook!
         notify-project-settings-changed!
         buffer-project-settings-pending buffer-project-settings-diagnostics
         local-dir-of-buffer techne-settings-file-path? editor-option-meta
         reload-open-buffers-under!)

;;; Change-notification registry (decouples project-settings from LSP and other consumers)

(define %project-settings-changed-hooks (make-registry 'project-settings-changed-hooks))

(define (add-project-settings-changed-hook! name proc)
  "Register PROC under NAME to run when project settings or trust change.
PROC is called as `(PROC dir)` where DIR is the affected directory or `#f`."
  (registry-add! %project-settings-changed-hooks name (scope-procedure proc))
  name)

(define (remove-project-settings-changed-hook! name)
  "Remove the project-settings-changed hook registered under NAME."
  (registry-remove! %project-settings-changed-hooks name))

(define (notify-project-settings-changed! [dir #f])
  "Notify project-settings-changed hooks about changes under DIR."
  (for-each (lambda (k)
              (let ((p (registry-ref %project-settings-changed-hooks k)))
                (when p (guard (e (#t #f)) (p dir)))))
            (registry-keys %project-settings-changed-hooks)))

;;; Helpers

(define (ensure-slash dir)
  (if (string-suffix? "/" dir) dir (string-append dir "/")))

(define (editor-option-meta name)
  "Return `(values known? user-only? safe valid? merge)` for option NAME."
  (let ((o (find-option name)))
    (if (not o)
        (values #f #f #f (lambda (v) #t) 'replace)
        (values #t
                (option-user-only? o)
                (option-directory-safe o)
                (lambda (v) (valid? (option-type o) v))
                (option-merge o)))))

(define (local-dir-of-buffer b)
  "Return the local directory with trailing slash for buffer B, or #f."
  (let* ((d (and b (buffer-document b)))
         (p (and d (document-path d))))
    (cond
      ((and p (not (remote-path? p))) (ensure-slash (directory-of p)))
      ((and b (not p))
       (let ((bd (buffer-directory b)))
         (and (string? bd) (not (remote-path? bd)) (ensure-slash bd))))
      (else #f))))

(define (techne-settings-file-path? path)
  "Return #t if PATH names a `.techne.scm` file."
  (and (string? path)
       (string=? (file-name path) ".techne.scm")))

;; Zero-I/O pending cache keyed by `(dir raw-path path modes)` and invalidated by generation.
(define %pending-cache-gen -1)
(define %pending-cache (make-hash-table))

(define (buffer-project-settings-pending b)
  "Return pending untrusted `.techne.scm` entries for buffer B without I/O.
Each entry is `(scope option-name effective-value source-path verdict)`."
  (if (not (project-settings-loaded?))
      '()
      (let* ((dir (local-dir-of-buffer b))
             (snap (and dir (project-settings-cached-snapshot dir))))
        (if (not snap)
            '()
            (let* ((gen (project-settings-generation))
                   (d (buffer-document b))
                   (raw-path (and d (document-path d)))
                   (path (and d (document-logical-path d)))
                   (modes (map mode-name (mode-chain (buffer-mode b))))
                   (key (list dir raw-path path modes)))
              (unless (= gen %pending-cache-gen)
                (for-each (lambda (k) (hash-table-delete! %pending-cache k))
                          (hash-table-keys %pending-cache))
                (set! %pending-cache-gen gen))
              (if (hash-table-contains? %pending-cache key)
                  (hash-table-ref %pending-cache key)
                  (let ((res (if (techne-settings-file-path? raw-path)
                                 (project-settings-collect-all-pending snap editor-option-meta)
                                 (project-settings-collect-pending snap path modes editor-option-meta))))
                    (hash-table-set! %pending-cache key res)
                    res)))))))

(define (buffer-project-settings-diagnostics b)
  "Return cached `.techne.scm` diagnostics for buffer B without I/O.
Each entry is a `settings-diagnostic` record."
  (if (not (project-settings-loaded?))
      '()
      (let* ((dir (local-dir-of-buffer b))
             (snap (and dir (project-settings-cached-snapshot dir))))
        (if snap (project-settings-snapshot-diagnostics snap) '()))))

;;; Zero-I/O mode-line segment

(register-mode-line-segment! 'project-settings
  (lambda (b)
    (and (project-settings-loaded?)
         (let* ((dir (local-dir-of-buffer b))
                (snap (and dir (project-settings-cached-snapshot dir))))
           (and snap
                (cond
                  ((pair? (project-settings-snapshot-diagnostics snap)) "settings:err")
                  ((any (lambda (p) (eq? (list-ref p 4) 'unknown))
                        (buffer-project-settings-pending b))
                   "settings:pending")
                  (else #f)))))))

;;; Reloading helpers & lifecycle hooks

;; Track document-id -> path for watched settings files so `document-moved`
;; invalidates even when a watched file is renamed away.
(define %watched-doc-paths (make-hash-table))

(define (record-watched-doc! d)
  (let ((path (and d (document-path d))))
    (if (and path (not (remote-path? path)) (project-settings-watched-path? path))
        (hash-table-set! %watched-doc-paths (document-id d) path)
        (when d (hash-table-delete! %watched-doc-paths (document-id d))))))

(define (reload-open-buffers-under! dir)
  "Reload cached project settings for all open local buffers under DIR."
  (let* ((prefix (ensure-slash dir))
         (canon (guard (e (#t #f)) (project-settings-canonical-dir prefix))))
    (when canon
      (for-each
       (lambda (b)
         (let ((bd (local-dir-of-buffer b)))
           (when (and bd
                      (or (string=? bd prefix) (string-prefix? prefix bd)
                          (string=? bd canon) (string-prefix? canon bd)))
             (guard (e (#t #f)) (project-settings-load! bd)))))
       (buffer-list)))))

(define (refresh-watched-dir! dir)
  (let ((d (ensure-slash dir)))
    (project-settings-invalidate! d)
    (guard (e (#t #f)) (project-settings-load! d))
    (reload-open-buffers-under! d)
    (project-settings-refresh-display!)
    (notify-project-settings-changed! d)))

(add-hook! 'file-buffer-added 'project-settings
  (lambda (s b)
    (record-watched-doc! (buffer-document b))
    (let ((dir (local-dir-of-buffer b)))
      (when dir
        (guard (e (#t #f))
          (project-settings-load! dir)
          (project-settings-refresh-display!))))))

(add-hook! 'document-saved 'project-settings
  (lambda (s d)
    (record-watched-doc! d)
    (let ((path (document-path d)))
      (when (and path
                 (not (remote-path? path))
                 (project-settings-watched-path? path))
        (refresh-watched-dir! (directory-of path))))))

(add-hook! 'document-moved 'project-settings
  (lambda (s d)
    (let* ((id (document-id d))
           (old-path (hash-table-ref/default %watched-doc-paths id #f))
           (path (document-path d)))
      (record-watched-doc! d)
      (when (and old-path (not (remote-path? old-path)))
        (refresh-watched-dir! (directory-of old-path)))
      (when (and path (not (remote-path? path)))
        (let ((dir (directory-of path)))
          (if (project-settings-watched-path? path)
              (refresh-watched-dir! dir)
              (begin
                (guard (e (#t #f)) (project-settings-load! dir))
                (project-settings-refresh-display!))))))))

;;; Before-save formatting edits (undoable)

(define (detect-eol bv n)
  (let loop ((i 0))
    (cond
      ((>= i n) "\n")
      ((= (bytevector-u8-ref bv i) 13)
       (if (and (< (+ i 1) n) (= (bytevector-u8-ref bv (+ i 1)) 10))
           "\r\n"
           "\r"))
      ((= (bytevector-u8-ref bv i) 10) "\n")
      (else (loop (+ i 1))))))

(define (compute-before-save-edits text trim? ensure-nl? eol)
  (let* ((bv (string->utf8 text))
         (n (bytevector-length bv))
         (edits '()))
    (when (and trim? (> n 0))
      (let loop ((i 0))
        (when (< i n)
          (let find-end ((j i))
            (cond
              ((or (= j n)
                   (= (bytevector-u8-ref bv j) 10)
                   (= (bytevector-u8-ref bv j) 13))
               (let back ((k j))
                 (if (and (> k i)
                          (let ((b (bytevector-u8-ref bv (- k 1))))
                            (or (= b 32) (= b 9))))
                     (back (- k 1))
                     (when (< k j)
                       (set! edits (cons (list k j "") edits)))))
               (cond
                 ((= j n) #f)
                 ((and (= (bytevector-u8-ref bv j) 13)
                       (< (+ j 1) n)
                       (= (bytevector-u8-ref bv (+ j 1)) 10))
                  (loop (+ j 2)))
                 (else (loop (+ j 1)))))
              (else (find-end (+ j 1))))))))
    (let ((asc (reverse edits)))
      (if (and ensure-nl? (> n 0))
          (let* ((last-b (bytevector-u8-ref bv (- n 1)))
                 (nl (case eol
                       ((crlf) "\r\n")
                       ((cr) "\r")
                       ((lf) "\n")
                       (else (detect-eol bv n))))
                 (already-has-nl?
                  (case eol
                    ((cr) (= last-b 13))
                    ((lf crlf) (= last-b 10))
                    (else (or (= last-b 10) (= last-b 13))))))
            (if already-has-nl?
                asc
                (if (and (pair? asc) (= (cadr (car edits)) n))
                    ;; Merge final newline into the last trim edit ending at EOF.
                    (reverse (cons (list (car (car edits)) n nl) (cdr edits)))
                    (append asc (list (list n n nl))))))
          asc))))

(define (apply-before-save-formatting! s d)
  (let ((b (document-buffer d)))
    (when (and b
               (not (presentation? d))
               (not (option b 'read-only)))
      (let* ((trim? (and (option b 'trim-trailing-whitespace) #t))
             (ensure-nl? (eq? (option b 'insert-final-newline) #t))
             (eol (option b 'end-of-line)))
        (when (or trim? ensure-nl?)
          (let ((edits (compute-before-save-edits (document-string d) trim? ensure-nl? eol)))
            (when (pair? edits)
              (let ((v (or (and s (document=? (view-document (pane-view s)) d) (pane-view s))
                           (make-view d (current-principal)))))
                (view-edit! v edits "new")))))))))

(add-hook! 'before-save 'project-settings
  (lambda (s d)
    (apply-before-save-formatting! s d)))

;;; Programmatic trust wrappers (keep display and subscribers in sync)

(define (project-settings-approve! scope option-name effective-value)
  "Approve EFFECTIVE-VALUE for OPTION-NAME in SCOPE and refresh live state.
User principal only; refreshes display caches and notifies subscribers."
  (let ((digest (project-trust-approve! scope option-name effective-value)))
    (project-settings-refresh-display!)
    (notify-project-settings-changed! scope)
    digest))

(define (project-settings-deny! scope option-name effective-value)
  "Deny EFFECTIVE-VALUE for OPTION-NAME in SCOPE and refresh live state.
User principal only; refreshes display caches and notifies subscribers."
  (let ((digest (project-trust-deny! scope option-name effective-value)))
    (project-settings-refresh-display!)
    (notify-project-settings-changed! scope)
    digest))

(define (project-settings-revoke! scope [option-name #f])
  "Revoke stored trust in SCOPE for OPTION-NAME (or all when `#f`).
User principal only; refreshes display caches and notifies subscribers."
  (let ((count (project-trust-revoke! scope option-name)))
    (project-settings-refresh-display!)
    (notify-project-settings-changed! scope)
    count))

(define %root-approve! (scope-procedure project-settings-approve!))
(define %root-deny! (scope-procedure project-settings-deny!))
(define %root-revoke! (scope-procedure project-settings-revoke!))

(define %project-settings-runtime-path
  (string-append (directory-of (current-module)) "project-settings/runtime.scm"))

(define (project-settings-runtime-scope? s)
  (let ((pkg (find-package 'project-settings-runtime)))
    (and pkg
         (eq? s (package-scope pkg))
         (equal? (package-path pkg) %project-settings-runtime-path))))

(define (require-interactive-runtime! who)
  (let ((auth (authority-scope)))
    (unless (or (root-authority?) (project-settings-runtime-scope? auth))
      (error (string-append who ": may only be called from root or project-settings-runtime scope")
             (and (scope? auth) (scope-name auth)))))
  (let ((who-p (current-principal)))
    (unless (equal? who-p "user")
      (error (string-append who ": only the user principal may change project trust") who-p))))

(define (%interactive-approve! scope option-name effective-value)
  "Approve EFFECTIVE-VALUE for OPTION-NAME in SCOPE from interactive UI."
  (require-interactive-runtime! "project-trust-approve!")
  (%root-approve! scope option-name effective-value))

(define (%interactive-deny! scope option-name effective-value)
  "Deny EFFECTIVE-VALUE for OPTION-NAME in SCOPE from interactive UI."
  (require-interactive-runtime! "project-trust-deny!")
  (%root-deny! scope option-name effective-value))

(define (%interactive-revoke! scope [option-name #f])
  "Revoke stored trust in SCOPE for OPTION-NAME from interactive UI."
  (require-interactive-runtime! "project-trust-revoke!")
  (%root-revoke! scope option-name))

;;; Lazy interactive commands

(register-command-package! 'project-settings-runtime
  %project-settings-runtime-path
  '((project-settings-status "Show discovered `.techne.scm` and `.editorconfig` files and trust state.")
    (project-settings-trust "Review and approve or deny pending untrusted project settings.")
    (project-settings-revoke "Revoke stored trust verdicts for project settings.")
    (project-settings-reload "Reload `.techne.scm` and `.editorconfig` for the current directory.")))
