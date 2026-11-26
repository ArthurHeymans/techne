;;; Negative tests for cold library-registered private codecs, shadowing,
;;; 0o600 enforcement on private replacement, and mid-decode symlink retargeting.
(import (techne))
(require "../../test.scm")
(require "../main.scm")
(require "../files.scm")
(require "../session.scm")
(require "../commands.scm")

(define (raises? thunk)
  (guard (e (#t #t))
    (thunk)
    #f))

(define tmp (string-append (or (get-environment-variable "TMPDIR") "/tmp")
                           "/techne-editor-codec-priv-" (number->string (current-milliseconds))))
(create-directory tmp #o700)

;; 1. Cold private codec registered directly via (techne codec) BEFORE files.scm
;; has ever warmed %codec-env must still classify and open private/unjournaled.
(let ((sc (make-scope 'cold-codec-lib))
      (secret-path (string-append tmp "/direct.vlt")))
  (with-scope sc
    (let ((codec-lib (environment '(techne codec))))
      ((eval 'register-codec! codec-lib)
       'cold-vlt "Direct library private codec."
       #:match (lambda (p) (string-suffix? ".vlt" p))
       #:decode (lambda (_p raw)
                  (if raw (string-append "DEC:" (utf8->string raw)) ""))
       #:encode (lambda (_p text _env) (string->utf8 text))
       #:private #t)))
  ;; Write an existing 0o644 file on disk.
  (call-with-output-file secret-path (lambda (p) (display "payload" p)))
  (set-file-mode! secret-path #o644)
  (check "cold library-registered private codec detected by path-private-by-codec?"
         #t (path-private-by-codec? secret-path))
  (let* ((d (file-document secret-path))
         (s (start-session (make-view d "user") "emacs")))
    (check "cold library-registered private codec opens document private"
           #t (document-private? d))
    (check "cold library-registered private codec decodes stored bytes"
           "DEC:payload" (document-string d))
    ;; Replacing an existing 0o644 private file tightens permissions to 0o600.
    (insert-text! s "!" "new")
    (save-document! d)
    (let* ((attrs (file-attributes secret-path))
           (mode (and (pair? attrs) (modulo (list-ref attrs 3) #o1000))))
      (check "private replacement tightens existing 0o644 file to 0o600" #o600 mode)))
  (scope-shutdown! sc))

;; 2. Shadowing a private codec by name from another scope cannot drop privacy.
(let ((sc1 (make-scope 'editor-priv-1))
      (sc2 (make-scope 'editor-priv-2))
      (vault-path (string-append tmp "/shadowed.shv")))
  (with-scope sc1
    (register-codec! 'shv "Private shv."
                     #:match (lambda (p) (string-suffix? ".shv" p))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #t))
  (with-scope sc2
    (check "editor rejects non-private shadow of private codec" #t
           (raises? (lambda ()
                      (register-codec! 'shv "Downgrade."
                                       #:match (lambda (p) (string-suffix? ".shv" p))
                                       #:decode (lambda (_p _r) "")
                                       #:encode (lambda (_p _t _e) #u8())
                                       #:private #f))))
    (register-codec! 'shv "Narrow shadow."
                     #:match (lambda (_p) #f)
                     #:decode (lambda (_p _r) "")
                     #:encode (lambda (_p _t _e) #u8())
                     #:private #t)
    (call-with-output-file vault-path (lambda (p) (display "secret" p)))
    (let ((d (file-document vault-path)))
      (check "document stays private even when private codec name is shadowed"
             #t (document-private? d))))
  (scope-shutdown! sc2)
  (scope-shutdown! sc1))

;; 3. Retargeting a symlink mid-decode to a private file with IDENTICAL stored
;; bytes must bind the open and journal to the inspected canonical target (pub),
;; never opening the private target with the public target's codec/journal.
(let* ((sc (make-scope 'mid-decode-retarget))
       (pub-path (string-append tmp "/a.tpub"))
       (priv-path (string-append tmp "/b.tpriv"))
       (alias-path (string-append tmp "/alias.link"))
       (retargeted? #f))
  (call-with-output-file pub-path (lambda (p) (display "identical-stored-bytes" p)))
  (call-with-output-file priv-path (lambda (p) (display "identical-stored-bytes" p)))
  (call-with-process "ln" (list "-s" pub-path alias-path) (lambda (p) (process-wait p)) #:stdin 'null)
  (with-scope sc
    (register-codec! 'tpub "Public codec that retargets alias mid-decode."
                     #:match (lambda (p) (string-suffix? ".tpub" p))
                     #:decode (lambda (_p raw)
                                (unless retargeted?
                                  (set! retargeted? #t)
                                  (call-with-process "ln" (list "-sf" priv-path alias-path)
                                    (lambda (p) (process-wait p)) #:stdin 'null))
                                (if raw (string-append "PUB:" (utf8->string raw)) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #f)
    (register-codec! 'tpriv "Private codec."
                     #:match (lambda (p) (string-suffix? ".tpriv" p))
                     #:decode (lambda (_p raw)
                                (if raw (string-append "PRIV:" (utf8->string raw)) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #t)
    (let ((d1 (file-document alias-path))
          (canon-pub (file-truename pub-path))
          (canon-priv (file-truename priv-path)))
      (check "decode callback retargeted alias mid-decode" #t retargeted?)
      (check "mid-decode retarget still opens inspected public canonical target"
             canon-pub (document-path d1))
      (check "mid-decode retarget keeps public codec on public target"
             'tpub (and (document-codec d1) (codec-name (document-codec d1))))
      (check "private target is NOT prematurely opened with public codec"
             #f (open-document-at canon-priv))
      ;; Subsequent open of alias now resolves to the private target with private profile & no journal.
      (let ((d2 (file-document alias-path)))
        (check "subsequent alias open resolves to private target"
               canon-priv (document-path d2))
        (check "private target gets private codec"
               'tpriv (and (document-codec d2) (codec-name (document-codec d2))))
        (check "private target opens private with retention none"
               #t (and (document-private? d2)
                       (eq? (document-retention d2) 'none))))))
  (scope-shutdown! sc))

;; 4. Replacing the CLASSIFIED PATHNAME ITSELF with a symlink to a private file
;; with IDENTICAL stored bytes during decoding must fail closed BEFORE acquiring
;; any journal or registering any document.
(let* ((sc (make-scope 'mid-decode-self-symlink))
       (pub-path (string-append tmp "/swap-pub.tpub2"))
       (priv-path (string-append tmp "/swap-priv.tpriv2"))
       (swapped? #f))
  (call-with-output-file pub-path (lambda (p) (display "identical-self-swap-bytes" p)))
  (call-with-output-file priv-path (lambda (p) (display "identical-self-swap-bytes" p)))
  (with-scope sc
    (register-codec! 'tpub2 "Public codec that replaces the classified path itself mid-decode."
                     #:match (lambda (p) (string-suffix? ".tpub2" p))
                     #:decode (lambda (_p raw)
                                (unless swapped?
                                  (set! swapped? #t)
                                  (delete-path pub-path #f)
                                  (call-with-process "ln" (list "-s" priv-path pub-path)
                                    (lambda (p) (process-wait p)) #:stdin 'null))
                                (if raw (string-append "PUB2:" (utf8->string raw)) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #f)
    (register-codec! 'tpriv2 "Private codec."
                     #:match (lambda (p) (string-suffix? ".tpriv2" p))
                     #:decode (lambda (_p raw)
                                (if raw (string-append "PRIV2:" (utf8->string raw)) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #t)
    (let* ((canon-priv (file-truename priv-path))
           (err-msg (guard (e (#t (error-object-message e)))
                      (file-document pub-path)
                      #f)))
      (check "decode callback replaced classified path with symlink mid-decode" #t swapped?)
      (check "classified path replaced with symlink mid-decode fails closed"
             #t (and (string? err-msg)
                     (string-contains err-msg "file target changed on disk while decoding")
                     #t))
      (check "private target was NOT registered as an open document"
             #f (open-document-at canon-priv))
      ;; If a journal had been created for canon-priv before refusing, opening it
      ;; privately would fail closed in check_no_existing_journal.
      (let ((d-priv (file-document priv-path)))
        (check "private target opens cleanly with no preexisting journal and private profile"
               #t (and (document-private? d-priv)
                       (eq? (document-retention d-priv) 'none)
                       (string=? "PRIV2:identical-self-swap-bytes" (document-string d-priv)))))))
  (scope-shutdown! sc))

;; 5. Replacing a PARENT DIRECTORY of the classified pathname with a symlink
;; to a private directory containing a regular file with IDENTICAL stored bytes
;; during decoding must fail closed BEFORE acquiring any journal or registering
;; any document.
(let* ((sc (make-scope 'mid-decode-parent-symlink))
       (pub-dir (string-append tmp "/pubdir"))
       (pub-backup (string-append tmp "/pubdir.orig"))
       (priv-dir (string-append tmp "/privdir"))
       (pub-item (string-append pub-dir "/item.tpub3"))
       (priv-item (string-append priv-dir "/item.tpub3"))
       (swapped? #f))
  (create-directory pub-dir #o700)
  (create-directory priv-dir #o700)
  (call-with-output-file pub-item (lambda (p) (display "identical-parent-swap-bytes" p)))
  (call-with-output-file priv-item (lambda (p) (display "identical-parent-swap-bytes" p)))
  (with-scope sc
    (register-codec! 'tpub3 "Public codec that swaps parent directory mid-decode."
                     #:match (lambda (p) (string-suffix? ".tpub3" p))
                     #:decode (lambda (_p raw)
                                (unless swapped?
                                  (set! swapped? #t)
                                  (rename-file pub-dir pub-backup)
                                  (call-with-process "ln" (list "-s" priv-dir pub-dir)
                                    (lambda (p) (process-wait p)) #:stdin 'null))
                                (if raw (string-append "PUB3:" (utf8->string raw)) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #f)
    (register-codec! 'tprivdir "Private codec matching privdir."
                     #:match (lambda (p) (and (string-contains p "/privdir/") #t))
                     #:decode (lambda (_p raw)
                                (if raw (string-append "PRIVDIR:" (utf8->string raw)) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #t)
    (let* ((canon-priv (file-truename priv-item))
           (err-msg (guard (e (#t (error-object-message e)))
                      (file-document pub-item)
                      #f)))
      (check "decode callback swapped parent directory with symlink mid-decode" #t swapped?)
      (check "parent directory swapped with symlink mid-decode fails closed"
             #t (and (string? err-msg)
                     (string-contains err-msg "file target changed on disk while decoding")
                     #t))
      (check "private target under swapped parent was NOT registered"
             #f (open-document-at canon-priv))
      (let ((d-priv (file-document priv-item)))
        (check "private target opens cleanly with no journal after refused parent swap"
               #t (and (document-private? d-priv)
                       (eq? (document-retention d-priv) 'none)
                       (string=? "PRIVDIR:identical-parent-swap-bytes" (document-string d-priv)))))))
  (scope-shutdown! sc))

;; 6. Classified raw opens (`%open-file-pinned` and `file-document #:pinned #t`)
;; must refuse both a leaf symlink and a parent-directory symlink pointing to a
;; private UTF-8 file without creating any journal or registering a document.
(let* ((sc (make-scope 'pinned-raw-open-symlinks))
       (priv-raw-dir (string-append tmp "/priv-raw-dir"))
       (priv-raw-file (string-append priv-raw-dir "/plain.privraw"))
       (leaf-link (string-append tmp "/raw-leaf.txt"))
       (parent-link (string-append tmp "/raw-parent-link")))
  (create-directory priv-raw-dir #o700)
  (call-with-output-file priv-raw-file (lambda (p) (display "secret-plain-utf8" p)))
  (call-with-process "ln" (list "-s" priv-raw-file leaf-link) (lambda (p) (process-wait p)) #:stdin 'null)
  (call-with-process "ln" (list "-s" priv-raw-dir parent-link) (lambda (p) (process-wait p)) #:stdin 'null)
  (with-scope sc
    (register-codec! 'privraw "Private codec matching .privraw."
                     #:match (lambda (p) (string-suffix? ".privraw" p))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #t)
    (let* ((canon-priv (file-truename priv-raw-file))
           (leaf-err (guard (e (#t (error-object-message e)))
                       (%open-file-pinned leaf-link #f #f)
                       #f))
           (parent-path (string-append parent-link "/plain.privraw"))
           (parent-err (guard (e (#t (error-object-message e)))
                         (file-document parent-path #:codec #f #:pinned #t)
                         #f)))
      (check "pinned raw open refuses leaf symlink"
             #t (and (string? leaf-err)
                     (string-contains leaf-err "file target changed on disk while opening")
                     #t))
      (check "pinned raw open refuses parent-directory symlink"
             #t (and (string? parent-err)
                     (string-contains parent-err "file target changed on disk while opening")
                     #t))
      (check "private raw target was NOT registered by refused pinned opens"
             #f (open-document-at canon-priv))
      (let ((d-priv (file-document priv-raw-file)))
        (check "private target opens cleanly with no preexisting journal"
               #t (and (document-private? d-priv)
                       (eq? (document-retention d-priv) 'none)
                       (string=? "secret-plain-utf8" (document-string d-priv)))))))
  (scope-shutdown! sc))

;; 7. A file growing past default-codec-max-bytes during decode must fail closed
;; with bounded post-decode verification (and bounded retry).
(let* ((sc (make-scope 'mid-decode-bounded-growth))
       (grow-path (string-append tmp "/grow.tgrow"))
       (grew? #f))
  (call-with-output-file grow-path (lambda (p) (display "short" p)))
  (with-scope sc
    (register-codec! 'tgrow "Codec whose file grows past max bytes mid-decode."
                     #:match (lambda (p) (string-suffix? ".tgrow" p))
                     #:decode (lambda (_p raw)
                                (unless grew?
                                  (set! grew? #t)
                                  (call-with-output-file grow-path
                                    (lambda (p) (display (make-string 128 #\x) p))))
                                (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #f))
  (let ((err-msg (parameterize ((default-codec-max-bytes 32))
                   (guard (e (#t (error-object-message e)))
                     (file-document grow-path)
                     #f))))
    (check "decode callback grew file past max-bytes" #t grew?)
    (check "mid-decode growth past max-bytes fails closed with size bound error"
           #t (and (string? err-msg)
                   (string-contains err-msg "exceeds maximum size of 32 bytes")
                   #t)))
  (scope-shutdown! sc))

;; 8. Raw reload and revert must refuse leaf and parent-directory symlink swaps
;; so private UTF-8 content cannot enter an existing public journaled buffer.
(let* ((pub-dir (string-append tmp "/rr-pub"))
       (pub-bak (string-append tmp "/rr-pub.bak"))
       (priv-dir (string-append tmp "/rr-priv"))
       (pub-leaf (string-append pub-dir "/leaf.txt"))
       (pub-parent (string-append pub-dir "/parent.txt"))
       (priv-secret (string-append priv-dir "/secret.txt")))
  (create-directory pub-dir #o700)
  (create-directory priv-dir #o700)
  (call-with-output-file pub-leaf (lambda (p) (display "pub-leaf" p)))
  (call-with-output-file pub-parent (lambda (p) (display "pub-parent" p)))
  (call-with-output-file priv-secret (lambda (p) (display "PRIVATE-UTF8" p)))
  (call-with-output-file (string-append priv-dir "/parent.txt") (lambda (p) (display "PRIVATE-PARENT-UTF8" p)))
  (let* ((d-leaf (file-document pub-leaf))
         (d-parent (file-document pub-parent))
         (s-leaf (start-session (make-view d-leaf "user") "emacs"))
         (s-parent (start-session (make-view d-parent "user") "emacs")))
    ;; Swap leaf to symlink pointing at priv-secret.
    (delete-path pub-leaf #f)
    (call-with-process "ln" (list "-s" priv-secret pub-leaf) (lambda (p) (process-wait p)) #:stdin 'null)
    (let ((reload-err (guard (e (#t (error-object-message e))) (reload-document! d-leaf) #f))
          (revert-err (guard (e (#t (error-object-message e))) (revert-document! d-leaf) #f))
          (read-err (guard (e (#t (error-object-message e))) (file-text d-leaf) #f)))
      (check "raw reload refuses leaf symlink swap"
             #t (and (string? reload-err) (string-contains reload-err "file target changed on disk while reloading") #t))
      (check "raw revert refuses leaf symlink swap"
             #t (and (string? revert-err) (string-contains revert-err "file target changed on disk while reverting") #t))
      (check "file-text refuses leaf symlink swap"
             #t (and (string? read-err) (string-contains read-err "file target changed on disk while reading") #t))
      (check "public leaf buffer text unchanged after refused reload/revert"
             "pub-leaf" (document-string d-leaf)))
    ;; Swap parent directory to symlink pointing at priv-dir.
    (rename-file pub-dir pub-bak)
    (call-with-process "ln" (list "-s" priv-dir pub-dir) (lambda (p) (process-wait p)) #:stdin 'null)
    (let ((reload-err (guard (e (#t (error-object-message e))) (reload-document! d-parent) #f))
          (revert-err (guard (e (#t (error-object-message e))) (revert-document! d-parent) #f)))
      (check "raw reload refuses parent-directory symlink swap"
             #t (and (string? reload-err) (string-contains reload-err "file target changed on disk while reloading") #t))
      (check "raw revert refuses parent-directory symlink swap"
             #t (and (string? revert-err) (string-contains revert-err "file target changed on disk while reverting") #t))
      (check "public parent buffer text unchanged after refused reload/revert"
             "pub-parent" (document-string d-parent)))
    (let ((_ (list s-leaf s-parent))) #f)))

;; 9. Saving a document whose parent directory was swapped with a symlink must
;; fail closed and write neither the target nor any temp file into the symlink target.
(let* ((pub-dir (string-append tmp "/sv-pub"))
       (pub-bak (string-append tmp "/sv-pub.bak"))
       (priv-dir (string-append tmp "/sv-priv"))
       (pub-same (string-append pub-dir "/same.txt"))
       (priv-same (string-append priv-dir "/same.txt"))
       (pub-new (string-append pub-dir "/new.txt")))
  (create-directory pub-dir #o700)
  (create-directory priv-dir #o700)
  (call-with-output-file pub-same (lambda (p) (display "MATCH" p)))
  (call-with-output-file priv-same (lambda (p) (display "MATCH" p)))
  (let* ((d-same (file-document pub-same))
         (d-new (file-document pub-new))
         (s-same (start-session (make-view d-same "user") "emacs"))
         (s-new (start-session (make-view d-new "user") "emacs")))
    (insert-text! s-same "!" "new")
    (insert-text! s-new "created" "new")
    (rename-file pub-dir pub-bak)
    (call-with-process "ln" (list "-s" priv-dir pub-dir) (lambda (p) (process-wait p)) #:stdin 'null)
    (let ((same-err (guard (e (#t (error-object-message e))) (save-document! d-same) #f))
          (ow-err (guard (e (#t (error-object-message e))) (save-document! d-same #:overwrite #t) #f))
          (new-err (guard (e (#t (error-object-message e))) (save-document! d-new) #f)))
      (check "save of matching-bytes file refuses parent-directory symlink swap"
             #t (and (string? same-err) (string-contains same-err "file target changed on disk while saving") #t))
      (check "overwrite save refuses parent-directory symlink swap"
             #t (and (string? ow-err) (string-contains ow-err "file target changed on disk while saving") #t))
      (check "save of absent new file refuses parent-directory symlink swap"
             #t (and (string? new-err) (string-contains new-err "file target changed on disk while saving") #t))
      (check "replacement directory file was NOT modified"
             "MATCH" (file->string priv-same))
      (check "replacement directory contains no new file or stray temp files"
             (list "same.txt") (directory-list priv-dir)))))

;; 10. Ordinary file visits through a UTF-8 symlink pointing to a non-UTF-8
;; directory must fail closed and NEVER open or register a sibling file under
;; a distinct directory whose name contains literal U+FFFD.
(let* ((fffd-dir (string-append tmp "/bad-" (string (integer->char #xFFFD)) "-dir"))
       (fffd-plain (string-append fffd-dir "/note.txt"))
       (fffd-gz (string-append fffd-dir "/note.txt.gz"))
       (utf8-plain-link (string-append tmp "/utf8-nonutf8-plain.txt"))
       (utf8-gz-link (string-append tmp "/utf8-nonutf8-codec.txt.gz")))
  (create-directory fffd-dir #o700)
  (call-with-output-file fffd-plain (lambda (p) (display "SIBLING-FFFD-PLAIN" p)))
  (call-with-output-file fffd-gz (lambda (p) (display "not-gzip-sibling" p)))
  (call-with-process
   "python3"
   (list "-c"
         "import gzip, os, sys; base = os.fsencode(sys.argv[1]); d = os.path.join(base, b'bad-\\xff-dir'); os.mkdir(d, 0o700); p = os.path.join(d, b'note.txt'); g = os.path.join(d, b'note.txt.gz'); open(p, 'wb').write(b'NON-UTF8-PLAIN'); open(g, 'wb').write(gzip.compress(b'NON-UTF8-GZ')); os.symlink(p, os.path.join(base, b'utf8-nonutf8-plain.txt')); os.symlink(g, os.path.join(base, b'utf8-nonutf8-codec.txt.gz'))"
         tmp)
   (lambda (p) (process-wait p))
   #:stdin 'null)
  (dynamic-wind
    (lambda () #f)
    (lambda ()
      (check "%canonical-file-path rejects symlink to non-UTF-8 plain file" 'err
             (guard (e (#t 'err)) (%canonical-file-path utf8-plain-link) 'ok))
      (check "%canonical-file-path rejects symlink to non-UTF-8 codec file" 'err
             (guard (e (#t 'err)) (%canonical-file-path utf8-gz-link) 'ok))
      (check "file-document rejects symlink to non-UTF-8 plain file" 'err
             (guard (e (#t 'err)) (file-document utf8-plain-link) 'ok))
      (check "file-document rejects symlink to non-UTF-8 codec file" 'err
             (guard (e (#t 'err)) (file-document utf8-gz-link) 'ok))
      (check "ordinary plain visit did NOT open or register literal U+FFFD sibling" #f
             (open-document-at fffd-plain))
      (check "ordinary codec visit did NOT open or register literal U+FFFD sibling" #f
             (open-document-at fffd-gz)))
    (lambda ()
      (call-with-process
       "python3"
       (list "-c"
             "import os, shutil, sys; base = os.fsencode(sys.argv[1]); [os.unlink(p) for p in [os.path.join(base, b'utf8-nonutf8-plain.txt'), os.path.join(base, b'utf8-nonutf8-codec.txt.gz')] if os.path.lexists(p)]; d = os.path.join(base, b'bad-\\xff-dir'); shutil.rmtree(d) if os.path.isdir(d) else None"
             tmp)
       (lambda (p) (process-wait p))
       #:stdin 'null))))

;; 11. mark-document-private! is a display/redaction/history marker and MUST NOT
;; satisfy the no-journal requirement on existing-document or supplied-document
;; private file visits.
(let* ((marked-path (string-append tmp "/marked-journaled.txt"))
       (other-path (string-append tmp "/marked-supplied.txt")))
  (call-with-output-file marked-path (lambda (p) (display "journaled-content" p)))
  (let ((d (file-document marked-path)))
    (check "normal file document has journal retention" #f (eq? (document-retention d) 'none))
    (mark-document-private! d)
    (check "mark-document-private! marks document-private? for redaction" #t (document-private? d))
    (check "mark-document-private! does not strip existing journal retention" #f (eq? (document-retention d) 'none))
    (check "private visit refuses reusing marked journal-backed existing document" 'err
           (guard (e (#t 'err))
             (file-document marked-path #:private #t)
             'ok))
    (check "private visit refuses attaching marked journal-backed supplied document" 'err
           (guard (e (#t 'err))
             (file-document other-path #:document d #:private #t)
             'ok))
    (check "supplied-document failure did not register other-path" #f
           (open-document-at other-path))))

;; 11b. cached-open-document-at and move-path! require document-unretained?,
;; not merely mark-document-private!, when a path has a private codec match.
(let* ((sc (make-scope 'r19-mix-scope))
       (mix-path (string-append tmp "/marked-cache.r19mix"))
       (mix-dest (string-append tmp "/marked-cache-dest.r19mix")))
  (call-with-output-file mix-path (lambda (p) (display "mix-content" p)))
  (with-scope sc
    (register-codec! 'r19-mix-plain "R19 Mix Plain"
      #:match (lambda (p) (string-suffix? ".r19mix" p))
      #:decode (lambda (p b) (if b (utf8->string b) ""))
      #:encode (lambda (p t e) (string->utf8 t))
      #:priority 10))
  (dynamic-wind
    (lambda () #f)
    (lambda ()
      (let ((d (file-document mix-path)))
        (check "mix document starts journaled" #f (document-unretained? d))
        (mark-document-private! d)
        (check "mix document is marked private" #t (document-private? d))
        (check "mix document remains journaled after mark-document-private!" #f (document-unretained? d))
        (with-scope sc
          (register-codec! 'r19-mix-priv "R19 Mix Priv"
            #:match (lambda (p) (string-suffix? ".r19mix" p))
            #:decode (lambda (p b) (if b (utf8->string b) ""))
            #:encode (lambda (p t e) (string->utf8 t))
            #:private #t
            #:priority 5))
        (check "cached-open-document-at rejects journaled mark-document-private! doc on private path"
               #f (cached-open-document-at mix-path))
        (check "move-path! refuses moving journaled mark-document-private! doc to private path"
               'err
               (guard (e (#t 'err))
                 (move-path! mix-path mix-dest)
                 'ok))))
    (lambda ()
      (scope-shutdown! sc))))

(delete-path tmp #t)

(test-failures)
