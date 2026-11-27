;;; Negative tests for #402:
;;; 1. Retargeted symlink alias between same-codec files must not reuse old cached document.
;;; 2. Concurrent async file-visit! waiters from different scopes/operations run their
;;;    completion callbacks inside their own requesting scope and operation/principal.
;;; 3. save-document-in-session! invokes #:on-fail when an async codec save fails and
;;;    does NOT re-enter #:on-fail if #:after throws after a successful save.
;;; 4. Interactive duplicate suppression uses canonical visit identity so retargeting a
;;;    symlink while its first visit is pending opens the new target instead of suppressing.
;;; 5. Mixed ibuffer-do-save batch with a sync changed-on-disk refusal, a second sync file,
;;;    an async success, and an async cancellation settles every selected file with exact totals.
(import (techne))
(import (techne test))
(test-file #:script #t #:environment (quote editor))
(require "../main.scm")
(require "../files.scm")
(require "../buffers.scm")
(require "../session.scm")
(require "../operations.scm")
(require "../commands.scm")
(require "../api.scm")
(require "../packages/ibuffer.scm")
(require "../ibuffer/runtime.scm")

(define tmp (string-append (or (get-environment-variable "TMPDIR") "/tmp")
                           "/techne-async-visit-sec-" (number->string (current-milliseconds))))
(create-directory tmp #o700)

(define (make-symlink target link-path)
  (call-with-process "ln" (list "-sfn" target link-path) (lambda (p) (process-wait p))))

;; 1. Symlink retargeting between two same-codec files must open the new target,
;; never returning the stale cached document from the old symlink alias.
(let ((sc (make-scope 'symlink-retarget-codec))
      (file-a (string-append tmp "/a.tcodec"))
      (file-b (string-append tmp "/b.tcodec"))
      (link-p (string-append tmp "/current.tcodec")))
  (with-scope sc
    (register-codec! 'tcodec "Test codec for symlink retargeting."
                     #:match (lambda (p) (string-suffix? ".tcodec" p))
                     #:decode (lambda (_p raw)
                                (if raw (string-append "DEC:" (utf8->string raw)) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #f))
  (call-with-output-file file-a (lambda (p) (display "alpha" p)))
  (call-with-output-file file-b (lambda (p) (display "beta" p)))
  (make-symlink file-a link-p)
  (let ((doc-a (file-document link-p)))
    (check "initial symlink open decodes target A" "DEC:alpha" (document-string doc-a))
    (check "cached-open-document-at finds A while symlink points to A"
           (document-id doc-a) (document-id (cached-open-document-at link-p)))
    ;; Retarget symlink from A to B (same codec!).
    (make-symlink file-b link-p)
    (check "cached-open-document-at returns #f after symlink retargets to unopened B"
           #f (cached-open-document-at link-p))
    (check "open-document-at returns #f after symlink retargets to unopened B"
           #f (open-document-at link-p))
    (let ((doc-b (file-document link-p)))
      (check "retargeted symlink opens distinct document for B"
             #f (= (document-id doc-a) (document-id doc-b)))
      (check "retargeted symlink decodes target B"
             "DEC:beta" (document-string doc-b))))
  (scope-shutdown! sc))

;; 2. Two concurrent async file-visit! waiters from different scopes and operations
;; joining the same background decode task must execute their completion callbacks
;; in their own respective requesting scope and operation/principal.
(let ((sc-codec (make-scope 'slow-visit-codec))
      (sc-a (make-scope 'waiter-scope-a))
      (sc-b (make-scope 'waiter-scope-b))
      (slow-path (string-append tmp "/shared.slow"))
      (decode-calls 0)
      (seen-a #f)
      (seen-b #f)
      (done-a (make-channel 1))
      (done-b (make-channel 1)))
  (with-scope sc-codec
    (register-codec! 'slow-visit "Slow test codec."
                     #:match (lambda (p) (string-suffix? ".slow" p))
                     #:decode (lambda (_p raw)
                                (set! decode-calls (+ decode-calls 1))
                                (sleep 60)
                                (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #f))
  (call-with-output-file slow-path (lambda (p) (display "shared-payload" p)))
  (let* ((s (start-session (make-view (make-document "") "user") "emacs"))
         (op-a (start-operation! 'visit-a s #:principal "agent-alpha"))
         (op-b (start-operation! 'visit-b s #:principal "agent-beta")))
    (with-scope sc-a
      (call-in-operation op-a
        (lambda ()
          (file-visit! s slow-path
                       #:interactive #f
                       #:on-ready (lambda (_s d)
                                    (set! seen-a (list (current-scope) (current-principal) (current-operation) (document-string d)))
                                    (channel-send done-a #t))))))
    (with-scope sc-b
      (call-in-operation op-b
        (lambda ()
          (file-visit! s slow-path
                       #:interactive #f
                       #:on-ready (lambda (_s d)
                                    (set! seen-b (list (current-scope) (current-principal) (current-operation) (document-string d)))
                                    (channel-send done-b #t))))))
    (channel-recv done-a)
    (channel-recv done-b)
    (check "concurrent waiters deduplicated into one decode job" 1 decode-calls)
    (check "waiter A callback ran in scope A" #t (and (pair? seen-a) (eq? (car seen-a) sc-a)))
    (check "waiter A callback restored principal A" "agent-alpha" (and (pair? seen-a) (cadr seen-a)))
    (check "waiter A callback restored operation A" #t (and (pair? seen-a) (eq? (caddr seen-a) op-a)))
    (check "waiter B callback ran in scope B" #t (and (pair? seen-b) (eq? (car seen-b) sc-b)))
    (check "waiter B callback restored principal B" "agent-beta" (and (pair? seen-b) (cadr seen-b)))
    (check "waiter B callback restored operation B" #t (and (pair? seen-b) (eq? (caddr seen-b) op-b))))
  (scope-shutdown! sc-b)
  (scope-shutdown! sc-a)
  (scope-shutdown! sc-codec))

;; 3. Async save failure invokes #:on-fail in save-document-in-session!
(let ((sc (make-scope 'fail-save-codec))
      (fail-path (string-append tmp "/boom.fcodec"))
      (failed-ch (make-channel 1)))
  (with-scope sc
    (register-codec! 'fcodec "Failing encode codec."
                     #:match (lambda (p) (string-suffix? ".fcodec" p))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p _text _env)
                                (sleep 10)
                                (error "simulated encode failure"))
                     #:private #f))
  (call-with-output-file fail-path (lambda (p) (display "ok" p)))
  (let* ((d (file-document fail-path))
         (s (start-session (make-view d "user") "emacs")))
    (insert-text! s "!" "new")
    (save-document-in-session! s d 'test-save
                               #:after (lambda () (channel-send failed-ch 'after))
                               #:on-fail (lambda (_e) (channel-send failed-ch 'failed)))
    (check "async save failure settles via #:on-fail" 'failed (channel-recv failed-ch)))
  (scope-shutdown! sc))

;; 3b. Exception thrown by #:after in save-document-in-session! must NOT re-enter #:on-fail.
(let ((sc (make-scope 'after-throw-codec))
      (ok-path (string-append tmp "/after-throw.okc"))
      (after-ran (make-channel 1))
      (on-fail-ran #f))
  (with-scope sc
    (register-codec! 'okc "Succeeding codec."
                     #:match (lambda (p) (string-suffix? ".okc" p))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #f))
  (call-with-output-file ok-path (lambda (p) (display "v1" p)))
  (let* ((d (file-document ok-path))
         (s (start-session (make-view d "user") "emacs")))
    (insert-text! s "!" "new")
    (save-document-in-session! s d 'test-after
                               #:after (lambda ()
                                         (channel-send after-ran #t)
                                         (error "after-callback-boom"))
                               #:on-fail (lambda (_e) (set! on-fail-ran #t)))
    (channel-recv after-ran)
    (sleep 30)
    (check "exception in #:after does NOT invoke #:on-fail" #f on-fail-ran))
  (scope-shutdown! sc))

;; 4. Interactive duplicate suppression uses canonical visit identity:
;; retargeting a symlink while its first interactive visit is pending must open
;; the new target instead of being suppressed as a duplicate of the old path.
(let ((sc (make-scope 'interactive-retarget-codec))
      (file-a (string-append tmp "/pending-a.slow2"))
      (file-b (string-append tmp "/pending-b.slow2"))
      (link-p (string-append tmp "/live-link.slow2"))
      (a-started (make-channel 1))
      (a-release (make-channel 1)))
  (with-scope sc
    (register-codec! 'slow2 "Slow on A, fast on B."
                     #:match (lambda (p) (string-suffix? ".slow2" p))
                     #:decode (lambda (p raw)
                                (when (string-suffix? "pending-a.slow2" p)
                                  (channel-send a-started #t)
                                  (channel-recv a-release))
                                (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #f))
  (call-with-output-file file-a (lambda (p) (display "ALPHA" p)))
  (call-with-output-file file-b (lambda (p) (display "BETA" p)))
  (make-symlink file-a link-p)
  (let* ((b0 (add-buffer! (make-document "")))
         (_ (set-buffer-name! b0 "*scratch-retarget*"))
         (s (start-session (make-view (buffer-document b0) "user") "emacs")))
    ;; Ordinary interactive visit with no extra callbacks (hits fast-path check).
    (file-visit! s link-p)
    (channel-recv a-started)
    ;; Retarget symlink while visit to A is deterministically blocked on a-release,
    ;; then visit link-p again interactively.
    (make-symlink file-b link-p)
    (check "visit A still pending before retargeted visit B" #t (file-visit-pending? s))
    (file-visit! s link-p)
    (let wait ((n 0))
      (when (and (< n 200) (file-visit-pending? s))
        (sleep 10)
        (wait (+ n 1))))
    (check "visit B completed within bound" #f (file-visit-pending? s))
    (check "interactive retargeted symlink visit opens target B, not A"
           (file-truename file-b) (document-path (session-document s)))
    (check "interactive retargeted symlink displays target B text"
           "BETA" (document-string (session-document s))))
  (scope-shutdown! sc))

;; 4b. Retargeting a symlink synchronously BEFORE the spawned async visit worker starts
;; must still open the pinned target A that keyed the job (never delivering B to waiter 1),
;; while a subsequent visit through the retargeted symlink opens B.
(let ((sc (make-scope 'pre-worker-retarget-codec))
      (file-a (string-append tmp "/pre-a.pre"))
      (file-b (string-append tmp "/pre-b.pre"))
      (link-p (string-append tmp "/pre-link.pre"))
      (link-b (string-append tmp "/pre-link-b.pre"))
      (done-a (make-channel 1))
      (done-b (make-channel 1)))
  (with-scope sc
    (register-codec! 'pre "Codec for pre-worker symlink retarget test."
                     #:match (lambda (p) (string-suffix? ".pre" p))
                     #:decode (lambda (_p raw)
                                (sleep 10)
                                (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #f))
  (call-with-output-file file-a (lambda (p) (display "PRE-A" p)))
  (call-with-output-file file-b (lambda (p) (display "PRE-B" p)))
  (make-symlink file-a link-p)
  (make-symlink file-b link-b)
  (let* ((b0 (add-buffer! (make-document "")))
         (_ (set-buffer-name! b0 "*scratch-pre-retarget*"))
         (s (start-session (make-view (buffer-document b0) "user") "emacs")))
    ;; Queue async visit for link-p (pointing to file-a). Do NOT yield before retargeting!
    (file-visit! s link-p
                 #:interactive #f
                 #:on-ready (lambda (_s d)
                              (channel-send done-a (list 'ok (document-path d) (document-string d))))
                 #:on-fail (lambda (_s e)
                             (channel-send done-a (list 'fail (error-object-message e)))))
    ;; Synchronously retarget link-p -> file-b (no subprocess yield) before the spawned worker starts.
    (delete-path link-p #f)
    (rename-file link-b link-p)
    ;; Queue a second async visit after the retarget; it must open file-b.
    (file-visit! s link-p
                 #:interactive #f
                 #:on-ready (lambda (_s d)
                              (channel-send done-b (list 'ok (document-path d) (document-string d))))
                 #:on-fail (lambda (_s e)
                             (channel-send done-b (list 'fail (error-object-message e)))))
    (let ((res-a (channel-recv done-a))
          (res-b (channel-recv done-b)))
      (check "waiter 1 keyed on A receives A even after immediate symlink retarget"
             (list 'ok (file-truename file-a) "PRE-A") res-a)
      (check "waiter 2 queued after retarget receives B"
             (list 'ok (file-truename file-b) "PRE-B") res-b)))
  (scope-shutdown! sc))

;; 4c. Replacing the pinned target itself with a symlink to another file BEFORE the
;; spawned async visit worker starts must fail closed instead of delivering the new target.
(let ((sc (make-scope 'pre-worker-target-swap-codec))
      (file-a (string-append tmp "/swap-a.pre2"))
      (file-b (string-append tmp "/swap-b.pre2"))
      (link-b (string-append tmp "/swap-link-b.pre2"))
      (done (make-channel 1)))
  (with-scope sc
    (register-codec! 'pre2 "Codec for pre-worker target swap test."
                     #:match (lambda (p) (string-suffix? ".pre2" p))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #f))
  (call-with-output-file file-a (lambda (p) (display "SWAP-A" p)))
  (call-with-output-file file-b (lambda (p) (display "SWAP-B" p)))
  (make-symlink file-b link-b)
  (let* ((b0 (add-buffer! (make-document "")))
         (_ (set-buffer-name! b0 "*scratch-pre-swap*"))
         (s (start-session (make-view (buffer-document b0) "user") "emacs")))
    (file-visit! s file-a
                 #:interactive #f
                 #:on-ready (lambda (_s d)
                              (channel-send done (list 'ok (document-path d) (document-string d))))
                 #:on-fail (lambda (_s e)
                             (channel-send done (list 'fail (error-object-message e)))))
    ;; Synchronously replace file-a with a symlink to file-b before the worker starts.
    (delete-path file-a #f)
    (rename-file link-b file-a)
    (let ((res (channel-recv done)))
      (check "replacing pinned target before worker starts fails closed"
             'fail (car res))
      (check "replacing pinned target reports changed target"
             #t (and (string-contains (cadr res) "file target changed while opening") #t))))
  (scope-shutdown! sc))

;; 5. Mixed ibuffer-do-save batch: sync changed-on-disk refusal + second sync save +
;; async codec save success + async codec save cancellation must settle every file
;; and report exact totals without aborting the batch.
(let ((sc (make-scope 'ibuffer-mixed-save))
      (sync-conflict (string-append tmp "/ib-sync-conflict.txt"))
      (sync-ok (string-append tmp "/ib-sync-ok.txt"))
      (async-ok (string-append tmp "/ib-async-ok.ibok"))
      (async-cancel (string-append tmp "/ib-async-cancel.ibcancel")))
  (with-scope sc
    (register-codec! 'ibok "Async succeeding codec."
                     #:match (lambda (p) (string-suffix? ".ibok" p))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (sleep 15) (string->utf8 text))
                     #:private #f)
    (register-codec! 'ibcancel "Async cancelling codec."
                     #:match (lambda (p) (string-suffix? ".ibcancel" p))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p _text _env)
                                (task-cancel (current-task))
                                (sleep 10)
                                #u8())
                     #:private #f))
  (call-with-output-file sync-conflict (lambda (p) (display "orig-conflict" p)))
  (call-with-output-file sync-ok (lambda (p) (display "orig-sync-ok" p)))
  (call-with-output-file async-ok (lambda (p) (display "orig-async-ok" p)))
  (call-with-output-file async-cancel (lambda (p) (display "orig-async-cancel" p)))
  (let* ((d-conflict (file-document sync-conflict))
         (d-sync-ok (file-document sync-ok))
         (d-async-ok (file-document async-ok))
         (d-async-cancel (file-document async-cancel))
         (b-conflict (or (document-buffer d-conflict) (add-buffer! d-conflict)))
         (b-sync-ok (or (document-buffer d-sync-ok) (add-buffer! d-sync-ok)))
         (b-async-ok (or (document-buffer d-async-ok) (add-buffer! d-async-ok)))
         (b-async-cancel (or (document-buffer d-async-cancel) (add-buffer! d-async-cancel)))
         (s (start-session (make-view d-conflict "user") "emacs"))
         (read-text (lambda (path)
                      (call-with-input-file path
                        (lambda (p)
                          (let loop ((acc '()))
                            (let ((c (read-char p)))
                              (if (eof-object? c)
                                  (list->string (reverse acc))
                                  (loop (cons c acc))))))))))
    ;; Edit all 4 buffers.
    (for-each (lambda (buf)
                (show-buffer! s buf)
                (insert-text! s "edit-" "new"))
              (list b-conflict b-sync-ok b-async-ok b-async-cancel))
    ;; Mutate sync-conflict externally on disk so sync save fails with changed-on-disk.
    (call-with-output-file sync-conflict (lambda (p) (display "external-change" p)))
    ;; Open ibuffer, mark all 4 buffers, and run ibuffer-do-save.
    (check "ibuffer command opens buffer list" #t (run-command s 'ibuffer 1))
    (check "ibuffer error message" #f (sget s 'message))
    (check "ibuffer buffer is current" "*Ibuffer*" (buffer-name (current-buffer s)))
    (let ((ib (current-buffer s)))
      (for-each (lambda (buf)
                  (hash-table-set! (listing-get ib 'marks) (buffer-name buf) #\*))
                (list b-conflict b-sync-ok b-async-ok b-async-cancel))
      (run-command s 'ibuffer-do-save 1)
      (let wait ((n 0))
        (when (and (< n 150)
                   (not (equal? (sget s 'message) "Saved 2 buffers, 1 failed, 1 cancelled")))
          (sleep 10)
          (wait (+ n 1))))
      (check "mixed ibuffer-do-save reports exact saved/failed/cancelled totals"
             "Saved 2 buffers, 1 failed, 1 cancelled" (sget s 'message))
      (check "second sync file was still saved despite earlier sync refusal"
             "edit-orig-sync-ok" (read-text sync-ok))
      (check "async-ok file was saved"
             "edit-orig-async-ok" (read-text async-ok))
      (check "sync-conflict file was NOT overwritten on disk"
             "external-change" (read-text sync-conflict))))
  (scope-shutdown! sc))

;; 6. Unresolvable local paths (missing parent directory or symlink loop) must
;; fail closed immediately in file-visit! rather than falling back to an
;; unpinned background visit that could adopt a target created later.
(let* ((sc (make-scope 'unresolvable-local-visit))
       (missing-dir (string-append tmp "/not-created-yet"))
       (missing-file (string-append missing-dir "/late.unres"))
       (loop-a (string-append tmp "/loop-a.unres"))
       (loop-b (string-append tmp "/loop-b.unres"))
       (decoded-any? #f)
       (pending-called? #f))
  (with-scope sc
    (register-codec! 'unres "Codec for unresolvable local path fail-closed test."
                     #:match (lambda (p) (string-suffix? ".unres" p))
                     #:decode (lambda (_p raw)
                                (set! decoded-any? #t)
                                (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #f))
  (make-symlink loop-b loop-a)
  (make-symlink loop-a loop-b)
  (let* ((b0 (add-buffer! (make-document "")))
         (_ (set-buffer-name! b0 "*scratch-unres*"))
         (s (start-session (make-view (buffer-document b0) "user") "emacs"))
         (missing-err (guard (e (#t (error-object-message e)))
                        (file-visit! s missing-file
                                     #:interactive #f
                                     #:on-pending (lambda (_c) (set! pending-called? #t)))
                        #f))
         (loop-err (guard (e (#t (error-object-message e)))
                     (file-visit! s loop-a
                                  #:interactive #f
                                  #:on-pending (lambda (_c) (set! pending-called? #t)))
                     #f)))
    ;; Even if the missing directory and file are created immediately afterward,
    ;; no unpinned background task was queued.
    (create-directory missing-dir #o700)
    (call-with-output-file missing-file (lambda (p) (display "late-payload" p)))
    (sleep 40)
    (check "missing parent local async visit fails closed synchronously"
           #t (and (string? missing-err) (> (string-length missing-err) 0)))
    (check "symlink loop local async visit fails closed synchronously"
           #t (and (string? loop-err) (> (string-length loop-err) 0)))
    (check "unresolvable local visit never queues a pending background task"
           #f pending-called?)
    (check "unresolvable local visit never runs codec decode"
           #f decoded-any?)
    (check "late-created file was not opened by a stale unpinned visit"
           #f (open-document-at missing-file)))
  (scope-shutdown! sc))

;; 7. Differently-named high-priority overlapping codecs cannot silently replace
;; built-in private encryption codecs (age/gpg) on automatic visit and first save:
;; - Without recipients, first save must fail closed and leave no file on disk.
;; - With configured age helper + recipients, stored bytes on disk must be produced
;;   by built-in age (never plaintext from the pass-through encoder).
;; - An explicit root-scope #:private #t #:override-builtin-private? #t codec IS honored.
(let* ((sc-evil (make-scope 'evil-crypto-overlap))
       (evil-encode-calls 0)
       (new-age-refuse (string-append tmp "/new-refuse.age"))
       (new-gpg-refuse (string-append tmp "/new-refuse.gpg"))
       (new-age-enc (string-append tmp "/new-encrypted.age"))
       (override-age (string-append tmp "/explicit-override-only.age"))
       (mock-age (string-append tmp "/mock-age-encrypt.sh"))
       (old-age-prog (option #f 'age-program))
       (old-age-recips (option #f 'age-recipients))
       (old-age-rfile (option #f 'age-recipients-file))
       (old-age-roots (option #f 'age-recipient-roots))
       (old-gpg-recips (option #f 'gpg-recipients))
       (old-gpg-roots (option #f 'gpg-id-roots))
       (read-raw-string (lambda (path)
                          (call-with-input-file path
                            (lambda (p)
                              (let loop ((acc '()))
                                (let ((c (read-char p)))
                                  (if (eof-object? c)
                                      (list->string (reverse acc))
                                      (loop (cons c acc))))))))))
  (with-scope sc-evil
    (register-codec! 'evil-age-passthrough "Malicious pass-through matching .age."
                     #:match (lambda (p) (string-suffix? ".age" p))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env)
                                (set! evil-encode-calls (+ evil-encode-calls 1))
                                (string->utf8 text))
                     #:private #f
                     #:priority 1000)
    (register-codec! 'evil-gpg-passthrough "Malicious private pass-through matching .gpg."
                     #:match (lambda (p) (string-suffix? ".gpg" p))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env)
                                (set! evil-encode-calls (+ evil-encode-calls 1))
                                (string->utf8 text))
                     #:private #t
                     #:priority 1000))
  ;; Also register a root-scope differently-named codec WITHOUT #:override-builtin-private? #t.
  (register-codec! 'root-no-override-age "Root codec without explicit override."
                   #:match (lambda (p) (string-suffix? ".age" p))
                   #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                   #:encode (lambda (_p text _env)
                              (set! evil-encode-calls (+ evil-encode-calls 1))
                              (string->utf8 text))
                   #:private #t
                   #:priority 900)
  (call-with-output-file mock-age
    (lambda (p)
      (display "#!/bin/sh\nmode=''\nrecips=''\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    -e) mode=e; shift ;;\n    -a) shift ;;\n    -r) shift; recips=\"$recips,$1\"; shift ;;\n    *) echo \"unexpected: $1\" >&2; exit 2 ;;\n  esac\ndone\n[ \"$mode\" = e ] && [ -n \"$recips\" ] || exit 1\nprintf 'MOCK-AGE-CIPHERTEXT:%s:' \"$recips\"\ntr 'A-Za-z' 'N-ZA-Mn-za-m'\n" p)))
  (call-with-process "chmod" (list "700" mock-age) (lambda (p) (process-wait p)))

  (dynamic-wind
    (lambda ()
      (set-option! 'age-recipients '())
      (set-option! 'age-recipients-file #f)
      (set-option! 'age-recipient-roots '())
      (set-option! 'gpg-recipients '())
      (set-option! 'gpg-id-roots '()))
    (lambda ()
      ;; 7a. New .age and .gpg files with no recipients must refuse on first save and write nothing.
      (let* ((d-age (file-document new-age-refuse))
             (d-gpg (file-document new-gpg-refuse))
             (s (start-session (make-view d-age "user") "emacs")))
        (check "new .age automatic visit selects built-in age codec" 'age (codec-name (document-codec d-age)))
        (check "new .gpg automatic visit selects built-in gpg codec" 'gpg (codec-name (document-codec d-gpg)))
        (insert-text! s "TOP-SECRET-AGE-PLAINTEXT" "new")
        (let ((age-err (guard (e (#t (error-object-message e)))
                         (save-document! d-age)
                         #f)))
          (check "new .age first save without recipients refuses" #t
                 (and (string? age-err) (string-contains age-err "no recipients") #t))
          (check "refused .age first save leaves no file on disk" #f (file-exists? new-age-refuse)))
        (let ((v-gpg (make-view d-gpg "user")))
          (view-edit! v-gpg (list (list 0 0 "TOP-SECRET-GPG-PLAINTEXT")) "new"))
        (let ((gpg-err (guard (e (#t (error-object-message e)))
                         (save-document! d-gpg)
                         #f)))
          (check "new .gpg first save without recipients refuses" #t
                 (and (string? gpg-err) (string-contains gpg-err "no recipients") #t))
          (check "refused .gpg first save leaves no file on disk" #f (file-exists? new-gpg-refuse))))

      ;; 7b. New .age file opened via async file-visit! with configured age helper + recipient
      ;; writes encrypted ciphertext bytes on first save, never plaintext.
      (set-option! 'age-program mock-age)
      (set-option! 'age-recipients '("age1testrecipient"))
      (let* ((b0 (add-buffer! (make-document "")))
             (_ (set-buffer-name! b0 "*scratch-age-overlap*"))
             (s (start-session (make-view (buffer-document b0) "user") "emacs"))
             (ready-ch (make-channel 1)))
        (file-visit! s new-age-enc
                     #:interactive #f
                     #:on-ready (lambda (_s d) (channel-send ready-ch d))
                     #:on-fail (lambda (_s e) (channel-send ready-ch e)))
        (let* ((d-enc (channel-recv ready-ch))
               (v-enc (make-view d-enc "user")))
          (check "async file-visit! on new .age selects built-in age codec"
                 'age (codec-name (document-codec d-enc)))
          (check "configured .age document is private" #t (document-private? d-enc))
          (view-edit! v-enc (list (list 0 0 "SUPER-SECRET-PAYLOAD")) "new")
          (save-document! d-enc)
          (let ((stored (read-raw-string new-age-enc)))
            (check "stored .age bytes start with mock age ciphertext header" #t
                   (string-prefix? "MOCK-AGE-CIPHERTEXT:,age1testrecipient:" stored))
            (check "stored .age bytes do NOT contain plaintext secret" #f
                   (string-contains stored "SUPER-SECRET-PAYLOAD")))))
      (check "unauthorized overlapping pass-through encoders were never invoked" 0 evil-encode-calls)

      ;; 7c. Explicit root-scope #:private #t #:override-builtin-private? #t override is honored.
      (register-codec! 'custom-root-age "Explicitly authorized root-scope private override."
                       #:match (lambda (p) (string-suffix? "/explicit-override-only.age" p))
                       #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                       #:encode (lambda (_p text _env) (string->utf8 (string-append "CUSTOM-ROOT-ENC:" text)))
                       #:private #t
                       #:priority 2000
                       #:override-builtin-private? #t)
      (let* ((d-ov (file-document override-age))
             (v-ov (make-view d-ov "user")))
        (check "explicit root override codec is selected for targeted .age path"
               'custom-root-age (codec-name (document-codec d-ov)))
        (check "explicit root override document remains private" #t (document-private? d-ov))
        (view-edit! v-ov (list (list 0 0 "override-body")) "new")
        (save-document! d-ov)
        (check "explicit root override encodes stored bytes"
               "CUSTOM-ROOT-ENC:override-body" (read-raw-string override-age))))
    (lambda ()
      (set-option! 'age-program old-age-prog)
      (set-option! 'age-recipients old-age-recips)
      (set-option! 'age-recipients-file old-age-rfile)
      (set-option! 'age-recipient-roots old-age-roots)
      (set-option! 'gpg-recipients old-gpg-recips)
      (set-option! 'gpg-id-roots old-gpg-roots)
      (scope-shutdown! sc-evil))))

;; 8. Hostile codec matchers that mutate their path string in place (e.g. rewriting
;; `.age` to `.txt` via `string-copy!` or `string-fill!`) must not defeat built-in
;; private codec protection, privacy checks, or alter the opened document path.
(let* ((sc-mut (make-scope 'hostile-mutating-matchers))
       (mut-encode-calls 0)
       (mut-age-expected (string-append (file-truename tmp) "/mut-target.age"))
       (mut-age-path (string-copy mut-age-expected))
       (mut-txt-path (string-append (file-truename tmp) "/mut-target.txt"))
       (mut-ext-expected (string-append (file-truename tmp) "/mut-ext.mutpriv2"))
       (mut-ext-path (string-copy mut-ext-expected))
       (mut-ext-corrupt (string-append (file-truename tmp) "/mut-ext.plain000"))
       (old-age-recips (option #f 'age-recipients))
       (old-age-rfile (option #f 'age-recipients-file))
       (old-age-roots (option #f 'age-recipient-roots)))
  (with-scope sc-mut
    (register-codec! 'mutating-age-hijack "Hostile matcher that rewrites .age to .txt in place."
                     #:match (lambda (p)
                               (if (and (string? p) (string-suffix? ".age" p))
                                   (begin
                                     (string-copy! p (- (string-length p) 4) ".txt")
                                     #t)
                                   #f))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env)
                                (set! mut-encode-calls (+ mut-encode-calls 1))
                                (string->utf8 text))
                     #:private #f
                     #:priority 2500)
    (register-codec! 'mutating-priv-probe "Hostile private matcher that mutates its path."
                     #:match (lambda (p)
                               (when (and (string? p) (string-suffix? ".mutpriv2" p))
                                 (string-copy! p (- (string-length p) 9) ".plain000"))
                               #f)
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #t
                     #:priority 2400)
    (register-codec! 'legit-mutpriv2 "Legitimate extension private codec."
                     #:match (lambda (p) (string-suffix? ".mutpriv2" p))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 (string-append "PRIV2:" text)))
                     #:private #t
                     #:priority 10))
  (dynamic-wind
    (lambda ()
      (set-option! 'age-recipients '())
      (set-option! 'age-recipients-file #f)
      (set-option! 'age-recipient-roots '()))
    (lambda ()
      (let ((d-age (file-document mut-age-path)))
        (check "caller .age path string unchanged after file-document"
               mut-age-expected mut-age-path)
        (check "mutating matcher cannot displace built-in age codec"
               'age (codec-name (document-codec d-age)))
        (check "opened .age document path unchanged despite mutating matcher"
               mut-age-expected (document-path d-age))
        (check "opened .age document remains private despite mutating matcher"
               #t (document-private? d-age))
        (let ((v (make-view d-age "user")))
          (view-edit! v (list (list 0 0 "MUT-TEST-SECRET")) "new"))
        (let ((err (guard (e (#t (error-object-message e)))
                     (save-document! d-age)
                     #f)))
          (check "save of .age document with mutating matcher still refuses without recipients" #t
                 (and (string? err) (string-contains err "no recipients") #t))
          (check "mutating matcher encode was never invoked" 0 mut-encode-calls)
          (check "refused save did not write .age file" #f (file-exists? mut-age-expected))
          (check "refused save did not write mutated .txt path" #f (file-exists? mut-txt-path))))
      (let ((d-ext (file-document mut-ext-path)))
        (check "caller .mutpriv2 path string unchanged after file-document"
               mut-ext-expected mut-ext-path)
        (check "lower-priority private codec still selected behind mutating matcher"
               'legit-mutpriv2 (codec-name (document-codec d-ext)))
        (check "opened .mutpriv2 document path unchanged despite mutating matcher"
               mut-ext-expected (document-path d-ext))
        (check "opened .mutpriv2 document remains private despite mutating matcher"
               #t (document-private? d-ext))
        (check "mutated .plain000 file was never created" #f (file-exists? mut-ext-corrupt))))
    (lambda ()
      (set-option! 'age-recipients old-age-recips)
      (set-option! 'age-recipients-file old-age-rfile)
      (set-option! 'age-recipient-roots old-age-roots)
      (scope-shutdown! sc-mut))))

;; 13. Async visits initiated from child scopes succeed cleanly.
(let* ((sc-codec (make-scope 'child-visit-codec))
       (sc-caller (make-scope 'child-visit-caller))
       (p (string-append tmp "/child-async.cvis"))
       (done (make-channel 1)))
  (dynamic-wind
    (lambda () #f)
    (lambda ()
      (with-scope sc-codec
        (register-codec! 'cvis "Child-triggered async visit codec."
                         #:match (lambda (path) (string-suffix? ".cvis" path))
                         #:decode (lambda (_path raw)
                                    (if raw (utf8->string raw) ""))
                         #:encode (lambda (_path text _env) (string->utf8 text))
                         #:private #f))
      (call-with-output-file p (lambda (out) (display "CHILD-VISIT-OK" out)))
      (let* ((b0 (add-buffer! (make-document "")))
             (_ (set-buffer-name! b0 "*scratch-child-visit*"))
             (s (start-session (make-view (buffer-document b0) "user") "emacs")))
        (with-scope sc-caller
          (file-visit! s p
                       #:interactive #f
                       #:on-ready (lambda (_s d)
                                    (channel-send done (list 'ok (document-string d))))
                       #:on-fail (lambda (_s e)
                                   (channel-send done (list 'fail (error-object-message e))))))
        (check "child-initiated async visit succeeds cleanly"
               (list 'ok "CHILD-VISIT-OK") (channel-recv done))))
    (lambda ()
      (scope-shutdown! sc-caller)
      (scope-shutdown! sc-codec))))

;; 14. Cached file-visit! must reject reusing a journaled document that only has
;; mark-document-private! set when the path requires private (unretained) handling.
(let* ((sc (make-scope 'r19-cached-visit-priv))
       (p (string-append tmp "/marked-cache-visit.r19v"))
       (outcome (make-channel 1)))
  (call-with-output-file p (lambda (out) (display "JOURNALED-SECRET" out)))
  (with-scope sc
    (register-codec! 'r19-vis-plain "R19 Visit Plain"
                     #:match (lambda (path) (string-suffix? ".r19v" path))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:priority 10))
  (dynamic-wind
    (lambda () #f)
    (lambda ()
      (let* ((d (file-document p))
             (b0 (add-buffer! (make-document "")))
             (_ (set-buffer-name! b0 "*scratch-r19-cache*"))
             (s (start-session (make-view (buffer-document b0) "user") "emacs")))
        (check "initial document has journal retention" #f (document-unretained? d))
        (mark-document-private! d)
        (check "marked document has document-private? = #t" #t (document-private? d))
        (check "marked document still has document-unretained? = #f" #f (document-unretained? d))
        (with-scope sc
          (register-codec! 'r19-vis-priv "R19 Visit Priv"
                           #:match (lambda (path) (string-suffix? ".r19v" path))
                           #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                           #:encode (lambda (_p text _env) (string->utf8 text))
                           #:private #t
                           #:priority 5))
        (check "cached-open-document-at refuses journaled mark-document-private! document"
               #f (cached-open-document-at p))
        (let ((ret (file-visit! s p
                                #:interactive #f
                                #:on-ready (lambda (_s doc) (channel-send outcome (list 'ready doc)))
                                #:on-fail (lambda (_s e) (channel-send outcome (list 'fail (error-object-message e)))))))
          (check "file-visit! did not take synchronous cached hit" #f ret)
          (let ((res (channel-recv outcome)))
            (check "file-visit! failed closed instead of reusing journaled marked document"
                   'fail (car res))))))
    (lambda ()
      (scope-shutdown! sc))))

;; 15. Settled async visit waiter is disowned so caller scope shutdown does not
;; fire on-cancel.
(let* ((sc-codec (make-scope 'r19-disown-codec))
       (sc-caller (make-scope 'r19-disown-caller))
       (p (string-append tmp "/r19-disown.r19d"))
       (waiter-cancelled #f)
       (done (make-channel 1)))
  (dynamic-wind
    (lambda () #f)
    (lambda ()
      (with-scope sc-codec
        (register-codec! 'r19-disown "R19 disown codec."
                         #:match (lambda (path) (string-suffix? ".r19d" path))
                         #:decode (lambda (_p raw)
                                    (if raw (utf8->string raw) ""))
                         #:encode (lambda (_p text _env) (string->utf8 text))
                         #:private #f))
      (call-with-output-file p (lambda (out) (display "DISOWN-OK" out)))
      (let* ((b0 (add-buffer! (make-document "")))
             (_ (set-buffer-name! b0 "*scratch-r19-disown*"))
             (s (start-session (make-view (buffer-document b0) "user") "emacs")))
        (with-scope sc-caller
          (file-visit! s p
                       #:interactive #f
                       #:on-ready (lambda (_s d) (channel-send done (document-string d)))
                       #:on-cancel (lambda (_s) (set! waiter-cancelled #t))
                       #:on-fail (lambda (_s e) (channel-send done (list 'fail (error-object-message e))))))
        (check "async visit settled cleanly"
               "DISOWN-OK" (channel-recv done))
        (scope-shutdown! sc-caller)
        (check "settled waiter was disowned so caller scope shutdown did not fire on-cancel"
               #f waiter-cancelled)))
    (lambda ()
      (scope-shutdown! sc-caller)
      (scope-shutdown! sc-codec))))

(delete-path tmp #t)

(test-failures)
