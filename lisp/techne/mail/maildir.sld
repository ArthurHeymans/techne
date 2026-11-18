;;; Maildir backend. A root is one mailbox; `maildir-folders` finds the
;;; mailboxes of a tree of them, as mbsync or offlineimap keep. A message
;;; is never written: only renamed, to change its flags or its folder,
;;; as mu4e does and mbsync then carries to the server.
(define-library (techne mail maildir)
  (export maildir-read maildir-for-each maildir-source-bytes maildir-folders maildir-keys
          maildir-check-root maildir-path-key maildir-path-flags maildir-locate maildir-set-flag! maildir-move!)
  (import (techne) (techne path) (techne task))
  (begin
    (define (%maildir-flags suffix)
      (filter-map
       (lambda (entry) (and (string-contains suffix (string (car entry))) (cdr entry)))
       '((#\D . "\\Draft") (#\F . "\\Flagged") (#\P . "$Forwarded")
         (#\R . "\\Answered") (#\S . "\\Seen") (#\T . "\\Deleted"))))
    (define (%maildir-paths root [ordered? #t])
      (let* ((base (file-truename root))
             (paths (append-map
          (lambda (sub)
            (let ((dir (path-join base sub)))
              (if (not (file-exists? dir)) '()
                  (filter-map (lambda (name)
                                (and (> (string-length name) 0)
                                     (not (char=? (string-ref name 0) #\.))
                                     (not (string-suffix? "/" name))
                                     (path-join dir name))) (directory-list dir)))))
          '("cur" "new"))))
        (if ordered? (sort paths string<?) paths)))
    (define (maildir-folders root #:depth [depth 4])
      "Return the Maildirs at and under ROOT as (NAME . PATH) pairs.
A Maildir is a directory holding cur or new. NAME is its path under
ROOT, empty for ROOT itself: Inbox, [Gmail]/All Mail. They come sorted by
name, a folder before those inside it. Folders nest at most DEPTH deep;
cur, new and tmp are not entered. A ROOT that is missing has none.
Nothing is created or changed."
      (let walk ((dir root) (name "") (depth depth))
        (let* ((entries (guard (e ((not (task-cancelled-condition? e)) '())) (directory-list dir)))
               (subs (filter-map (lambda (e) (and (string-suffix? "/" e) (substring e 0 (- (string-length e) 1)))) entries))
               (here (if (or (member "cur" subs) (member "new" subs)) (list (cons name dir)) '())))
          (append here
                  (if (= depth 0) '()
                      (append-map (lambda (sub)
                                    (walk (path-join dir sub) (if (string=? name "") sub (string-append name "/" sub)) (- depth 1)))
                                  (sort (remove (lambda (s) (member s '("cur" "new" "tmp"))) subs) string<?)))))))
    (define (%maildir-message path raw-bytes?)
      ;; Reject known-large regular files without opening/allocating their bodies.
      ;; Keep the bounded read below: a file can grow after this metadata check.
      (let ((a (file-attributes path)))
        (when (and a (eq? (car a) 'file) (> (cadr a) 16777216))
          (error "Maildir message exceeds 16 MiB")))
      (let* ((name (path-file-name path)) (marker (string-contains name ":2,"))
             (key (if marker (substring name 0 marker) name))
             (suffix (if marker (substring name (+ marker 3) (string-length name)) ""))
             (bytes (call-with-port (open-binary-input-file path)
                      (lambda (p)
                        (let ((bytes (read-bytevector 16777217 p)))
                          (when (and (bytevector? bytes) (> (bytevector-length bytes) 16777216))
                            (error "Maildir message exceeds 16 MiB"))
                          (if (eof-object? bytes) #u8() bytes))))))
        `((key . ,key) (path . ,path) (flags . ,(%maildir-flags suffix))
          (raw . ,(if raw-bytes? bytes (utf8->string bytes))))))
    (define (maildir-read root #:raw-bytes? [raw-bytes? #f])
      "Read ROOT/cur and ROOT/new as message alists: key, path, flags and raw.
ROOT names one mailbox. The key is the filename before :2,; path is absolute.
Results are sorted by
path. tmp, dotfiles and directories are ignored; missing cur/new are empty.
D/F/P/R/S/T map to \\Draft, \\Flagged, $Forwarded, \\Answered, \\Seen, \\Deleted.
RAW-BYTES? #t preserves octets for MIME decoding; otherwise raw is UTF-8 text.
Each message is bounded to 16 MiB. No files change. Concurrent source moves or
deletes can raise I/O errors; callers may retry. Use maildir-for-each to avoid
holding every body of a large mailbox in memory."
      (map (lambda (path) (%maildir-message path raw-bytes?)) (%maildir-paths root)))
    (define (maildir-path-key path)
      "Return the source key of Maildir PATH, without its flag suffix."
      (let* ((name (path-file-name path)) (marker (string-contains name ":2,")))
        (if marker (substring name 0 marker) name)))
    (define (%maildir-priority path)
      (let* ((name (path-file-name path)) (marker (string-contains name ":2,"))
             (flags (if marker (substring name (+ marker 3) (string-length name)) ""))
             (digits (let loop ((i 0))
                       (if (and (< i (string-length name)) (char<=? #\0 (string-ref name i) #\9))
                           (loop (+ i 1)) i))))
        (vector path (not (string-contains flags "S"))
                (if (> digits 0) (string->number (substring name 0 digits)) 0))))
    (define (%maildir-unread-newest paths)
      (map (lambda (entry) (vector-ref entry 0))
           (sort (map %maildir-priority paths)
                 (lambda (a b)
                   (cond ((not (eq? (vector-ref a 1) (vector-ref b 1))) (vector-ref a 1))
                         ((not (= (vector-ref a 2) (vector-ref b 2))) (> (vector-ref a 2) (vector-ref b 2)))
                         (else (string>? (vector-ref a 0) (vector-ref b 0))))))))
    (define (maildir-check-root root)
      "Require at least one cur or new directory directly inside ROOT."
      (let ((base (file-truename root)))
        (unless (any (lambda (sub)
                       (let ((a (file-attributes (path-join base sub))))
                         (and a (eq? (car a) 'directory)))) '("cur" "new"))
          (error "Maildir requires cur or new" root))))
    (define (maildir-keys root)
      "Return Maildir ROOT's message keys and paths without reading bodies.
An unavailable Maildir is an error, not an empty mailbox."
      (maildir-check-root root)
      (let ((keys (make-hash-table)))
        (for-each (lambda (path) (hash-table-set! keys (maildir-path-key path) path)) (%maildir-paths root #f))
        keys))
    (define (%maildir-suffix name)
      (let ((marker (string-contains name ":2,")))
        (if marker (substring name (+ marker 3) (string-length name)) "")))
    (define (maildir-path-flags path)
      "Return the flags the name of the Maildir message at PATH gives it.
They are as `maildir-read' gives them: \\Seen, \\Flagged, \\Deleted..."
      (%maildir-flags (%maildir-suffix (path-file-name path))))
    (define (maildir-locate path key)
      "Return where the Maildir message KEY last seen at PATH is now, or #f.
A flag change renames it, and a delivery moves it from new to cur: its
cur and new siblings are searched for KEY, the whole key, not a prefix."
      (if (file-exists? path) path
          (let ((root (path-directory (path-directory path))))
            (find (lambda (p) (string=? (maildir-path-key p) key))
                  (guard (e ((not (task-cancelled-condition? e)) '())) (%maildir-paths root #f))))))
    (define (maildir-set-flag! path key letter on?)
      "Set flag LETTER of Maildir message KEY at PATH if ON?, else clear it.
LETTER is #\\S, #\\F, #\\T or another Maildir flag.
The message is renamed into cur with its flags, sorted, the others kept,
as the Maildir convention has it; its content is not touched. Return its
path now, or #f when it is no longer there (moved by another program)."
      (let ((current (maildir-locate path key)))
        (and current
             (let* ((name (path-file-name current))
                    (old (string->list (%maildir-suffix name)))
                    (new (list->string (sort (if on? (if (memv letter old) old (cons letter old))
                                                 (remove (lambda (c) (char=? c letter)) old))
                                             char<?)))
                    (cur (path-join (path-directory (path-directory current)) "cur"))
                    (target (path-join cur (string-append key ":2," new))))
               (cond ((string=? target current) current)
                     ((file-exists? target) (error "Maildir message already has that name" target))
                     (else (%cur! cur) (rename-file current target) target))))))
    (define (%cur! dir)
      ;; A Maildir has cur; one delivered to new alone gets it when a
      ;; message first moves there.
      (unless (file-exists? dir) (create-directory dir #o700)))
    (define (%without-mbsync-uid key)
      ;; With default mbsync UID mapping this infix belongs to the old folder.
      (let ((at (string-contains key ",U=")))
        (if (not at) key
            (let ((end (let loop ((i (+ at 3)))
                         (if (and (< i (string-length key)) (char<=? #\0 (string-ref key i) #\9))
                             (loop (+ i 1)) i))))
              (if (and (> end (+ at 3)) (or (= end (string-length key)) (char=? #\, (string-ref key end))))
                  (string-append (substring key 0 at) (substring key end (string-length key)))
                  key)))))
    (define (maildir-move! path key root #:before [before (lambda (from to) #t)])
      "Move message KEY, at PATH, into the cur of Maildir folder ROOT.
Preserve flags and content but drop the old folder's mbsync UID. Call
BEFORE with source and destination paths before renaming. Return the new
path, or #f if the source is gone."
      (let ((current (maildir-locate path key)))
        (and current
             (let* ((dir (path-join root "cur"))
                    (name (path-file-name current))
                    (old-key (maildir-path-key current))
                    (new-key (%without-mbsync-uid old-key))
                    (target (path-join dir (string-append new-key (substring name (string-length old-key) (string-length name))))))
                (before current target)
                (%cur! dir)
                (rename-file current target)
                target))))
    (define (maildir-source-bytes path key)
      "Read immutable source KEY at Maildir PATH, following flag renames.
Search only its cur/new siblings on a missing path; compare full keys, not
prefixes. Reject oversized or unavailable sources without changing any file."
      (let ((candidate (maildir-locate path key)))
        (unless (and candidate (string=? (maildir-path-key candidate) key))
          (error "Maildir source is no longer available; cached metadata remains"))
        (cdr (assq 'raw (%maildir-message candidate #t)))))
    (define (maildir-for-each root proc #:skip? [skip? #f] #:order [order 'path] #:on-error [on-error #f] #:cached-keys [cached-keys #f])
      "Call PROC with each message of Maildir ROOT, preserving raw octets.
SKIP?, when a procedure, receives a source key before its body is opened;
true skips that immutable Maildir message. ORDER is path (the default) or
unread-newest: unread first, then descending filename timestamp and path.
This prioritizes ingestion, not the message's authoritative Date header.
Paths are enumerated once; only one body is held at a time. PROC may suspend
to cache it. No source file is modified. Each body is bounded to 16 MiB.
ON-ERROR, if given, receives (key condition) for a source read failure and
iteration continues. Errors from PROC still propagate, cache failures too.
CACHED-KEYS, an optional hash set, removes known sources before sorting;
SKIP? still runs before opening remaining bodies: pending duplicates can skip."
      (unless (memq order '(path unread-newest)) (error "Unknown Maildir order" order))
      (let* ((paths (%maildir-paths root #f))
             (paths (if cached-keys (filter (lambda (p) (not (hash-table-ref/default cached-keys (maildir-path-key p) #f))) paths) paths)))
        (for-each (lambda (path)
                    (unless (and skip? (skip? (maildir-path-key path)))
                      (let ((m (guard (e ((not (task-cancelled-condition? e)) (if on-error (begin (on-error (maildir-path-key path) e) #f) (raise e))))
                                 (%maildir-message path #t))))
                        (when m (proc m)))))
                  (if (eq? order 'unread-newest) (%maildir-unread-newest paths) (sort paths string<?)))))))
