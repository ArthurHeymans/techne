;;; Negative security tests for #402/#403:
;;; 1. Moving a codec-backed document during `before-save` between two directories
;;;    with different recipient policies uses the post-move directory's recipients.
;;; 2. Two simultaneous live LSP workspaces: `workspace/configuration` for workspace A
;;;    never discloses workspace B's open-buffer/project config, symlink targets outside A,
;;;    or private open buffers inside A.
;;; 3. Approval UI displays full escaped effective values without truncation and maps
;;;    multi-entry candidates by stable target identity rather than display text.
;;; 4. Trust ledger entries and cached snapshots return defensive copies, and
;;;    memory-only ('none) trust stays isolated from file-backed editor ledgers.
(import (techne) (techne editorconfig))
(import (techne test))
(test-file #:script #t #:environment (quote editor))
(require "../main.scm")
(require "../api.scm")
(require "../modes.scm")
(require "../files.scm")
(require "../buffers.scm")
(require "../session.scm")
(require "../dispatch.scm")
(require "../minibuffer.scm")
(require "../operations.scm")
(require "../project-settings-bridge.scm")
(require "../project-settings.scm")
(require "../remote.scm")
(require "../lsp.scm")

(define tmp-raw (string-append (or (get-environment-variable "TMPDIR") "/tmp")
                               "/techne-ps-sec-" (number->string (current-milliseconds))))
(create-directory tmp-raw #o700)
(define tmp (file-truename tmp-raw))

(define (assert-true name expr)
  (check name #t (and expr #t)))

(define (assert-err name thunk)
  (check name #t
         (guard (e (#t #t))
           (thunk)
           #f)))

(define (make-symlink target link-path)
  (call-with-process "ln" (list "-sfn" target link-path) (lambda (p) (process-wait p))))

(define (read-file-string path)
  (call-with-input-file path
    (lambda (p)
      (let loop ((chunks '()))
        (let ((c (read-string 4096 p)))
          (if (eof-object? c)
              (apply string-append (reverse chunks))
              (loop (cons c chunks))))))))

;; 0. Symlinked trust ledger, symlinked ledger parent, and in-root symlinked
;;    `.techne.scm` / `.editorconfig` must fail closed atomically via O_NOFOLLOW.
(let* ((sym-dir (string-append tmp "/sym-ledger-proj"))
       (sym-store-dir (string-append tmp "/sym-store-dir"))
       (sym-store-dir-alias (string-append tmp "/sym-store-dir-alias"))
       (sym-store (string-append sym-store-dir "/sym-trust.scm"))
       (sym-target (string-append sym-store-dir "/sym-trust-target.scm")))
  (create-directory sym-dir #o700)
  (create-directory sym-store-dir #o700)
  (make-symlink sym-store-dir sym-store-dir-alias)
  (parameterize ((default-trust-store-path sym-store))
    (project-trust-approve! sym-dir 'compile-command "cargo build")
    (check "initial trust verdict trusted before symlink swap" 'trusted
           (project-trust-verdict sym-dir 'compile-command "cargo build"))
    (project-settings-load! sym-dir)
    (let ((saved (read-file-string sym-store)))
      (call-with-output-file sym-target (lambda (p) (display saved p)))
      (delete-path sym-store #f)
      ;; Relative in-root leaf symlink pointing to a sibling in the same dir.
      (make-symlink "sym-trust-target.scm" sym-store)
      (project-settings-load! sym-dir)
      (check "in-root symlinked trust ledger fails closed to unknown on cached dir reload" 'unknown
             (project-trust-verdict sym-dir 'compile-command "cargo build"))
      ;; Restoring regular file works; loading through symlinked parent dir fails closed.
      (delete-path sym-store #f)
      (call-with-output-file sym-store (lambda (p) (display saved p)))
      (project-settings-load! sym-dir)
      (check "restored regular trust ledger recovers to trusted" 'trusted
             (project-trust-verdict sym-dir 'compile-command "cargo build"))))
  (parameterize ((default-trust-store-path (string-append sym-store-dir-alias "/sym-trust.scm")))
    (project-settings-load! sym-dir)
    (check "trust ledger under symlinked parent directory fails closed to unknown" 'unknown
           (project-trust-verdict sym-dir 'compile-command "cargo build")))
  ;; In-root leaf symlinked `.techne.scm` and `.editorconfig` must both be refused.
  (let ((inroot-dir (string-append tmp "/inroot-symlink-proj")))
    (create-directory inroot-dir #o700)
    (call-with-output-file (string-append inroot-dir "/real-settings.scm")
      (lambda (p) (display "(techne-settings 1 (all (fill-column 99)))\n" p)))
    (call-with-output-file (string-append inroot-dir "/real-ec.ini")
      (lambda (p) (display "root = true\n[*]\nindent_size = 7\n" p)))
    (make-symlink "real-settings.scm" (string-append inroot-dir "/.techne.scm"))
    (make-symlink "real-ec.ini" (string-append inroot-dir "/.editorconfig"))
    (let* ((snap (project-settings-load! inroot-dir))
           (diags (project-settings-snapshot-diagnostics snap))
           (main-rs (string-append inroot-dir "/main.rs")))
      (assert-true "in-root symlinked .techne.scm records an error diagnostic" (pair? diags))
      (call-with-values
       (lambda () (project-settings-resolve-option snap main-rs '(rust-mode) '() 'fill-column #:safe #t))
       (lambda (applied pending rejected)
         (check "in-root symlinked .techne.scm does not apply fill-column" '() applied)))
      (call-with-values
       (lambda () (project-settings-resolve-option snap main-rs '(rust-mode) '() 'indent-width #:safe #t))
       (lambda (applied pending rejected)
         (check "in-root symlinked .editorconfig does not apply indent-width" '() applied)))))
  ;; Symlinked project directory / ancestor must fail closed for both `.techne.scm`
  ;; and `.editorconfig`, must not fall through to an outer ancestor, and must not
  ;; poison the real directory's cache.
  (let* ((outer-dir (string-append tmp "/sym-ancestor-outer"))
         (real-proj (string-append outer-dir "/real-proj"))
         (link-proj (string-append outer-dir "/link-proj")))
    (create-directory outer-dir #o700)
    (create-directory real-proj #o700)
    (call-with-output-file (string-append outer-dir "/.techne.scm")
      (lambda (p) (display "(techne-settings 1 (root #t) (all (fill-column 77)))\n" p)))
    (call-with-output-file (string-append outer-dir "/.editorconfig")
      (lambda (p) (display "root = true\n[*]\nindent_size = 5\n" p)))
    (call-with-output-file (string-append real-proj "/.techne.scm")
      (lambda (p) (display "(techne-settings 1 (root #t) (all (fill-column 99)))\n" p)))
    (call-with-output-file (string-append real-proj "/.editorconfig")
      (lambda (p) (display "root = true\n[*]\nindent_size = 7\n" p)))
    (make-symlink real-proj link-proj)
    (let* ((ps-env (environment '(techne project-settings)))
           (ec-env (environment '(techne editorconfig)))
           (parse-ps (eval 'parse-project-settings-file ps-env))
           (parse-ec (eval 'parse-editorconfig-file ec-env))
           (ps-diags (eval 'project-settings-file-diagnostics ps-env))
           (ps-rules (eval 'project-settings-file-rules ps-env))
           (direct-sf (parse-ps (string-append link-proj "/.techne.scm")))
           (direct-ec (parse-ec (string-append link-proj "/.editorconfig"))))
      (assert-true "parse-project-settings-file refuses symlinked ancestor"
                   (and direct-sf (pair? (ps-diags direct-sf)) (null? (ps-rules direct-sf))))
      (check "parse-editorconfig-file refuses symlinked ancestor" #f direct-ec))
    (project-settings-invalidate!)
    (let* ((link-snap (project-settings-load! link-proj))
           (link-main (string-append link-proj "/main.rs")))
      (assert-true "project-settings-load! on symlinked dir records error diagnostic"
                   (pair? (project-settings-snapshot-diagnostics link-snap)))
      (call-with-values
       (lambda () (project-settings-resolve-option link-snap link-main '(rust-mode) '() 'fill-column #:safe #t))
       (lambda (applied pending rejected)
         (check "symlinked dir applies neither target nor outer .techne.scm" '() applied)))
      (call-with-values
       (lambda () (project-settings-resolve-option link-snap link-main '(rust-mode) '() 'indent-width #:safe #t))
       (lambda (applied pending rejected)
         (check "symlinked dir applies neither target nor outer .editorconfig" '() applied))))
    (let* ((real-snap (project-settings-load! real-proj))
           (real-main (string-append real-proj "/main.rs")))
      (check "symlinked load did not poison real project diagnostics"
             '() (project-settings-snapshot-diagnostics real-snap))
      (call-with-values
       (lambda () (project-settings-resolve-option real-snap real-main '(rust-mode) '() 'fill-column #:safe #t))
       (lambda (applied pending rejected)
         (check "real project still applies its own fill-column" 99 (list-ref (car applied) 3))))
      (call-with-values
       (lambda () (project-settings-resolve-option real-snap real-main '(rust-mode) '() 'indent-width #:safe #t))
       (lambda (applied pending rejected)
         (check "real project still applies its own indent-width" 7 (list-ref (car applied) 3))))))
  ;; Paths containing `..` (e.g. `/work/link/../project` where `link` points elsewhere)
  ;; must NEVER be lexically collapsed to `/work/project` on either a cold or warm cache.
  (let* ((work (string-append tmp "/dotdot-work"))
         (other (string-append tmp "/dotdot-other"))
         (other-sub (string-append other "/sub"))
         (proj-a (string-append work "/project"))
         (proj-b (string-append other "/project"))
         (link (string-append work "/link"))
         (tricky-dir (string-append link "/../project"))
         (tricky-file (string-append tricky-dir "/main.rs"))
         (ps-env (environment '(techne project-settings)))
         (ec-env (environment '(techne editorconfig)))
         (ps-discover (eval 'project-settings-discover ps-env))
         (parse-ps (eval 'parse-project-settings-file ps-env))
         (ec-discover (eval 'editorconfig-discover ec-env))
         (ec-resolve (eval 'editorconfig-resolve ec-env))
         (parse-ec (eval 'parse-editorconfig-file ec-env)))
    (create-directory work #o700)
    (create-directory other #o700)
    (create-directory other-sub #o700)
    (create-directory proj-a #o700)
    (create-directory proj-b #o700)
    (call-with-output-file (string-append proj-a "/.techne.scm")
      (lambda (p) (display "(techne-settings 1 (root #t) (all (fill-column 88)))\n" p)))
    (call-with-output-file (string-append proj-a "/.editorconfig")
      (lambda (p) (display "root = true\n[*]\nindent_size = 8\n" p)))
    (call-with-output-file (string-append proj-b "/.techne.scm")
      (lambda (p) (display "(techne-settings 1 (root #t) (all (fill-column 44)))\n" p)))
    (call-with-output-file (string-append proj-b "/.editorconfig")
      (lambda (p) (display "root = true\n[*]\nindent_size = 4\n" p)))
    (make-symlink other-sub link)
    ;; Cold cache: tricky-dir must NOT collapse to proj-a (or load proj-b via symlink).
    (project-settings-invalidate!)
    (check "cold cache: project-settings-load! refuses '..' path" #f
           (project-settings-load! tricky-dir))
    (check "cold cache: project-settings-discover refuses '..' path" '()
           (ps-discover tricky-dir))
    (check "cold cache: parse-project-settings-file refuses '..' path" #f
           (parse-ps (string-append tricky-dir "/.techne.scm")))
    (check "cold cache: editorconfig-discover refuses '..' path" '()
           (ec-discover tricky-dir))
    (check "cold cache: editorconfig-resolve refuses '..' path" '()
           (ec-resolve tricky-file))
    (check "cold cache: parse-editorconfig-file refuses '..' path" #f
           (parse-ec (string-append tricky-dir "/.editorconfig")))
    ;; Warm cache: populate proj-a in caches AND approve a trust entry for proj-a,
    ;; then verify tricky-dir STILL does not hit proj-a's cache or trust entry.
    (let ((warm-snap (project-settings-load! proj-a)))
      (assert-true "warm cache: real proj-a loads cleanly" (and warm-snap #t))
      (check "warm cache: project-settings-load! refuses '..' path" #f
             (project-settings-load! tricky-dir))
      (check "warm cache: project-settings-cached-snapshot refuses '..' path" #f
             (project-settings-cached-snapshot tricky-dir))
      (check "warm cache: project-settings-discover refuses '..' path" '()
             (ps-discover tricky-dir))
      (check "warm cache: editorconfig-discover refuses '..' path" '()
             (ec-discover tricky-dir))
      (check "warm cache: editorconfig-resolve refuses '..' path" '()
             (ec-resolve tricky-file))
      (check "warm cache: editorconfig-resolve with explicit files refuses '..' path" '()
             (ec-resolve tricky-file #:files (ec-discover proj-a)))
      (check "warm cache: canonical-dir resolves link/.. through kernel to proj-b, not proj-a"
             (string-append proj-b "/")
             (project-settings-canonical-dir tricky-dir))
      ;; Public `option` boundary must reject `..` before lexical normalization or
      ;; snapshot lookup for directory strings, settings contexts, execution sites, and buffers.
      (let* ((default-fc (option #f 'fill-column))
             (default-tw (option #f 'tab-width))
             (buf-scope (make-scope 'dotdot-buffer-test)))
        (dynamic-wind
          (lambda ()
            (set-option! 'tab-width 2 #:directory proj-a))
          (lambda ()
            ;; Directory string
            (check "public option: real proj-a hits cached .techne.scm" 88 (option proj-a 'fill-column))
            (check "public option: real proj-a hits user dir override" 2 (option proj-a 'tab-width))
            (check "public option: '..' directory string does not hit proj-a snapshot"
                   default-fc (option tricky-dir 'fill-column))
            (check "public option: '..' directory string does not hit proj-a user override"
                   default-tw (option tricky-dir 'tab-width))
            ;; Settings context
            (check "public option: real settings-context hits proj-a snapshot"
                   88 (option (make-settings-context proj-a #:load? #f) 'fill-column))
            (check "public option: '..' settings-context (load? #f) does not hit proj-a snapshot"
                   default-fc (option (make-settings-context tricky-dir #:load? #f) 'fill-column))
            (check "public option: '..' settings-context (load? #t) does not hit proj-a snapshot"
                   default-fc (option (make-settings-context tricky-dir #:load? #t) 'fill-column))
            (check "public option: settings-context with '..' path does not hit proj-a snapshot"
                   default-fc (option (make-settings-context proj-a #:path tricky-file #:load? #f) 'fill-column))
            (check "public option: '..' settings-context does not hit proj-a user override"
                   default-tw (option (make-settings-context tricky-dir #:load? #f) 'tab-width))
            ;; Execution site
            (check "public option: real execution-site hits proj-a snapshot"
                   88 (option (execution-site-for proj-a #:policy 'project) 'fill-column))
            (check "public option: '..' execution-site does not hit proj-a snapshot"
                   default-fc (option (execution-site-for tricky-dir #:policy 'project) 'fill-column))
            (check "public option: '..' execution-site does not hit proj-a user override"
                   default-tw (option (execution-site-for tricky-dir #:policy 'project) 'tab-width))
            ;; Buffer
            (with-scope buf-scope
              (define-mode dotdot-buf-mode "Mode supplying buffer-directory from buffer-state."
                #:directory (lambda (b) (buffer-state b)))
              (let ((b-ok (make-buffer (make-document "") "*dotdot-ok*" 'dotdot-buf-mode #:state proj-a))
                    (b-tricky (make-buffer (make-document "") "*dotdot-tricky*" 'dotdot-buf-mode #:state tricky-dir)))
                (check "public option: real buffer hits proj-a snapshot" 88 (option b-ok 'fill-column))
                (check "public option: real buffer hits proj-a user override" 2 (option b-ok 'tab-width))
                (check "public option: '..' buffer does not hit proj-a snapshot"
                       default-fc (option b-tricky 'fill-column))
                (check "public option: '..' buffer does not hit proj-a user override"
                       default-tw (option b-tricky 'tab-width)))))
          (lambda ()
            (unset-option! 'tab-width #:directory proj-a)
            (scope-shutdown! buf-scope)))))
    ;; Failed canonicalization on unresolvable `..` scopes (including missing, dangling,
    ;; or non-directory final components) and non-UTF-8 canonical targets must fail closed
    ;; before mutating the ledger.
    (let* ((dotdot-ledger (string-append tmp "/dotdot-trust-ledger.scm"))
           (missing-scope (string-append work "/missing-dir/../project"))
           (missing-leaf-scope (string-append proj-a "/../missing-leaf"))
           (dangling-leaf (string-append work "/dangling-leaf"))
           (dangling-leaf-scope (string-append proj-a "/../dangling-leaf"))
           (regular-leaf (string-append work "/regular-leaf-file"))
           (regular-leaf-scope (string-append proj-a "/../regular-leaf-file"))
           (rel-scope "../project")
           (remote-scope "/ssh:host:/work/a/../b"))
      (call-with-process "ln" (list "-s" (string-append work "/no-such-target") dangling-leaf)
                         (lambda (p) (process-wait p)) #:stdin 'null)
      (call-with-output-file regular-leaf (lambda (p) (display "not a directory\n" p)))
      (parameterize ((default-trust-store-path dotdot-ledger))
        (project-trust-approve! proj-a 'compile-command "cargo check")
        (let ((before-ledger (read-file-string dotdot-ledger)))
          (check "warm trust: proj-a is trusted" 'trusted
                 (project-trust-verdict proj-a 'compile-command "cargo check"))
          (check "warm trust: '..' path never matches proj-a trust verdict" 'unknown
                 (project-trust-verdict tricky-dir 'compile-command "cargo check"))
          (assert-err "canonical-dir rejects unresolvable '..' local path"
                      (lambda () (project-settings-canonical-dir missing-scope)))
          (assert-err "canonical-dir rejects '..' with missing final component"
                      (lambda () (project-settings-canonical-dir missing-leaf-scope)))
          (assert-err "canonical-dir rejects '..' with dangling symlink final component"
                      (lambda () (project-settings-canonical-dir dangling-leaf-scope)))
          (assert-err "canonical-dir rejects '..' with regular file final component"
                      (lambda () (project-settings-canonical-dir regular-leaf-scope)))
          (assert-err "canonical-dir rejects direct dangling symlink"
                      (lambda () (project-settings-canonical-dir dangling-leaf)))
          (assert-err "canonical-dir rejects relative '..' path"
                      (lambda () (project-settings-canonical-dir rel-scope)))
          (assert-err "canonical-dir rejects remote '..' path"
                      (lambda () (project-settings-canonical-dir remote-scope)))
          (assert-err "set-option! #:directory rejects unresolvable '..' path"
                      (lambda () (set-option! 'tab-width 3 #:directory missing-scope)))
          (assert-err "set-option! #:directory rejects '..' with missing final component"
                      (lambda () (set-option! 'tab-width 3 #:directory missing-leaf-scope)))
          (for-each
           (lambda (pair)
             (let ((label (car pair))
                   (bad-scope (cdr pair)))
               (assert-err (string-append "project-trust-approve! fails closed on " label)
                           (lambda () (project-trust-approve! bad-scope 'compile-command "evil-cmd")))
               (assert-err (string-append "project-trust-deny! fails closed on " label)
                           (lambda () (project-trust-deny! bad-scope 'compile-command "cargo check")))
               (assert-err (string-append "scoped project-trust-revoke! (with option) fails closed on " label)
                           (lambda () (project-trust-revoke! bad-scope 'compile-command)))
               (assert-err (string-append "scoped project-trust-revoke! (all options) fails closed on " label)
                           (lambda () (project-trust-revoke! bad-scope)))))
           (list (cons "missing intermediate '..'" missing-scope)
                 (cons "missing final '..'" missing-leaf-scope)
                 (cons "dangling final '..'" dangling-leaf-scope)
                 (cons "regular-file final '..'" regular-leaf-scope)
                 (cons "direct dangling symlink" dangling-leaf)))
          (check "failed mutations did not trust proj-a for evil-cmd" 'unknown
                 (project-trust-verdict proj-a 'compile-command "evil-cmd"))
          (check "failed mutations left proj-a trusted" 'trusted
                 (project-trust-verdict proj-a 'compile-command "cargo check"))
          (check "failed mutations left trust ledger file untouched" before-ledger
                 (read-file-string dotdot-ledger))))
      ;; Non-UTF-8 canonical target via UTF-8 symlink must fail closed and NEVER
      ;; alias a sibling directory containing literal U+FFFD in its name.
      (let* ((fffd-ledger (string-append tmp "/fffd-trust-ledger.scm"))
             (fffd-dir (string-append work "/bad-" (string (integer->char #xFFFD)) "-dir"))
             (utf8-alias (string-append work "/utf8-to-nonutf8")))
        (create-directory fffd-dir #o700)
        (call-with-process
         "python3"
         (list "-c"
               "import os, sys; base = os.fsencode(sys.argv[1]); target = os.path.join(base, b'bad-\\xff-dir'); os.mkdir(target, 0o700); os.symlink(target, os.path.join(base, b'utf8-to-nonutf8'))"
               work)
         (lambda (p) (process-wait p))
         #:stdin 'null)
        (dynamic-wind
          (lambda () #f)
          (lambda ()
            (parameterize ((default-trust-store-path fffd-ledger))
              (project-trust-approve! fffd-dir 'compile-command "real-fffd-cmd")
              (let ((before-ledger (read-file-string fffd-ledger)))
                (check "literal U+FFFD dir can be approved under its own key" 'trusted
                       (project-trust-verdict fffd-dir 'compile-command "real-fffd-cmd"))
                (assert-err "file-truename rejects symlink to non-UTF-8 directory"
                            (lambda () (file-truename utf8-alias)))
                (assert-err "canonical-dir rejects symlink to non-UTF-8 directory"
                            (lambda () (project-settings-canonical-dir utf8-alias)))
                (assert-err "canonical-dir rejects missing child under symlink to non-UTF-8 directory"
                            (lambda () (project-settings-canonical-dir (string-append utf8-alias "/missing-sub"))))
                (assert-err "project-trust-approve! rejects symlink to non-UTF-8 directory"
                            (lambda () (project-trust-approve! utf8-alias 'compile-command "aliased-evil")))
                (assert-err "project-trust-deny! rejects symlink to non-UTF-8 directory"
                            (lambda () (project-trust-deny! utf8-alias 'compile-command "real-fffd-cmd")))
                (assert-err "project-trust-revoke! rejects symlink to non-UTF-8 directory"
                            (lambda () (project-trust-revoke! utf8-alias 'compile-command)))
                (check "non-UTF-8 alias did NOT approve literal U+FFFD sibling" 'unknown
                       (project-trust-verdict fffd-dir 'compile-command "aliased-evil"))
                (check "non-UTF-8 alias did NOT deny or revoke literal U+FFFD sibling" 'trusted
                       (project-trust-verdict fffd-dir 'compile-command "real-fffd-cmd"))
                (check "non-UTF-8 alias left trust ledger untouched" before-ledger
                       (read-file-string fffd-ledger)))))
          (lambda ()
            (call-with-process
             "python3"
             (list "-c"
                   "import os, sys; base = os.fsencode(sys.argv[1]); target = os.path.join(base, b'bad-\\xff-dir'); link = os.path.join(base, b'utf8-to-nonutf8'); (os.unlink(link) if os.path.lexists(link) else None); (os.rmdir(target) if os.path.isdir(target) else None)"
                   work)
             (lambda (p) (process-wait p))
             #:stdin 'null)))))))

;; 0d. `project-settings-invalidate!` must clear `.editorconfig` cache on
;;     canonicalization failure (matching `%techne-file-cache` and `%snapshot-cache`)
;;     and invalidate canonical targets when invoked with a symlink alias.
(let* ((ec-other (string-append tmp "/ec-inv-other"))
       (ec-other-file (string-append ec-other "/.editorconfig"))
       (ec-real (string-append tmp "/ec-inv-real"))
       (ec-real-file (string-append ec-real "/.editorconfig"))
       (ec-alias (string-append tmp "/ec-inv-alias"))
       (ec-dangling (string-append tmp "/ec-inv-dangling")))
  (define (write-text path text)
    (call-with-output-file path (lambda (p) (display text p))))
  (define (ec-indent file)
    (let ((p (assoc "indent_size" (editorconfig-resolve file))))
      (and p (cdr p))))
  (create-directory ec-other #o700)
  (create-directory ec-real #o700)
  (make-symlink ec-real ec-alias)
  (make-symlink (string-append tmp "/ec-inv-missing-target") ec-dangling)
  (write-text ec-other-file "root = true\n[*]\nindent_size = 2\n")
  (write-text ec-real-file "root = true\n[*]\nindent_size = 3\n")
  (project-settings-invalidate!)
  ;; Populate `%ec-cache` for both unrelated `ec-other` and `ec-real`.
  (check "ec-other initial indent_size is 2" "2" (ec-indent (string-append ec-other "/a.rs")))
  (check "ec-real initial indent_size is 3" "3" (ec-indent (string-append ec-real "/a.rs")))
  ;; Overwrite both files on disk; cached entries remain stale until invalidated.
  (write-text ec-other-file "root = true\n[*]\nindent_size = 7\n")
  (write-text ec-real-file "root = true\n[*]\nindent_size = 5\n")
  (check "ec-other stays cached before invalidation" "2" (ec-indent (string-append ec-other "/a.rs")))
  ;; 1. Invalidation via symlink alias must evict the canonical directory's .editorconfig cache.
  (project-settings-invalidate! ec-alias)
  (check "symlink alias invalidation refreshes canonical .editorconfig" "5" (ec-indent (string-append ec-real "/a.rs")))
  (check "symlink alias invalidation leaves unrelated .editorconfig cached" "2" (ec-indent (string-append ec-other "/a.rs")))
  ;; 2. Invalidation on an uncanonicalizable path (dangling symlink) must fall back to a full clear.
  (project-settings-invalidate! ec-dangling)
  (check "dangling-path invalidation fallback clears unrelated .editorconfig cache" "7" (ec-indent (string-append ec-other "/a.rs"))))

;; ---------------------------------------------------------------------------
;; 1. Move during `before-save` between two directories with different recipient
;;    policies must encode with the post-move directory's recipient policy.
;; ---------------------------------------------------------------------------
(let* ((sc (make-scope 'before-save-move-test))
       (dir-a (string-append tmp "/recip-a"))
       (dir-b (string-append tmp "/recip-b"))
       (file-a (string-append dir-a "/secret.recodec"))
       (file-b (string-append dir-b "/secret.recodec"))
       (moved? #f))
  (create-directory dir-a #o700)
  (create-directory dir-b #o700)
  (call-with-output-file (string-append dir-a "/.recip") (lambda (p) (display "KEY-ALPHA" p)))
  (call-with-output-file (string-append dir-b "/.recip") (lambda (p) (display "KEY-BETA" p)))
  (call-with-output-file file-a (lambda (p) (display "ENC[KEY-ALPHA]:initial" p)))
  (with-scope sc
    (register-codec! 'recodec "Directory-recipient test codec."
                     #:match (lambda (p) (string-suffix? ".recodec" p))
                     #:decode (lambda (_p raw)
                                (let ((s (if raw (utf8->string raw) "")))
                                  (let ((colon (string-index s #\:)))
                                    (if colon (substring s (+ colon 1) (string-length s)) s))))
                     #:encode (lambda (p text _env)
                                (let ((recip (read-file-string (string-append (directory-of p) ".recip"))))
                                  (string->utf8 (string-append "ENC[" recip "]:" text))))
                     #:private #t)
    (add-hook! 'before-save 'move-before-save-test
      (lambda (_s d)
        (when (and (not moved?) (equal? (document-path d) file-a))
          (set! moved? #t)
          (move-path! file-a file-b)))))
  (let* ((d (file-document file-a))
         (v (make-view d "user")))
    (check "initial decode reads file A" "initial" (document-string d))
    (view-edit! v (list (list 0 (document-length d) "updated-secret")) "new")
    (save-document! d)
    (check "before-save hook moved document path to dir B" file-b (document-path d))
    (check "post-move file exists in dir B" #t (file-exists? file-b))
    (check "pre-move file no longer exists in dir A" #f (file-exists? file-a))
    (check "saved ciphertext in dir B uses dir B recipient policy"
           "ENC[KEY-BETA]:updated-secret" (read-file-string file-b)))
  (scope-shutdown! sc))

;; ---------------------------------------------------------------------------
;; 2. Two simultaneous live LSP workspaces + overlapping nested inner workspace:
;;    foreign-workspace scopeUri, symlink escape, private open buffer, and
;;    unattached inner-session open buffers must never leak configuration.
;; ---------------------------------------------------------------------------
(let* ((sc-lsp (make-scope 'two-ws-lsp-test))
       (ws-a (string-append tmp "/ws-a"))
       (ws-sub (string-append ws-a "/sub"))
       (ws-b (string-append tmp "/ws-b"))
       (file-a (string-append ws-a "/a.rs"))
       (file-sub (string-append ws-sub "/inner.rs"))
       (priv-a (string-append ws-a "/priv.rs"))
       (esc-a (string-append ws-a "/escape.rs"))
       (gpg-a (string-append ws-a "/secret.txt.gpg"))
       (file-b (string-append ws-b "/b.rs"))
       (srv-script (string-append tmp "/fake-lsp-scope.py"))
       (store-path (string-append tmp "/lsp-trust.scm")))
  (create-directory ws-a #o700)
  (create-directory (string-append ws-a "/.git") #o700)
  (create-directory ws-sub #o700)
  (create-directory (string-append ws-sub "/.git") #o700)
  (create-directory ws-b #o700)
  (create-directory (string-append ws-b "/.git") #o700)
  (call-with-output-file file-a (lambda (p) (display "fn a() {}\n" p)))
  (call-with-output-file file-sub (lambda (p) (display "fn inner() {}\n" p)))
  (call-with-output-file priv-a (lambda (p) (display "fn secret() {}\n" p)))
  (call-with-output-file file-b (lambda (p) (display "fn b() {}\n" p)))
  (make-symlink file-b esc-a)
  (call-with-output-file (string-append ws-a "/.techne.scm")
    (lambda (p)
      (display "(techne-settings 1 (root #t) (all (lsp-settings (dict \"fake\" (dict \"owner\" \"A\")))))\n" p)))
  (call-with-output-file (string-append ws-b "/.techne.scm")
    (lambda (p)
      (display "(techne-settings 1 (root #t) (all (lsp-settings (dict \"fake\" (dict \"owner\" \"B\" \"secret\" \"from-B\")))))\n" p)))
  (call-with-output-file srv-script
    (lambda (p)
      (display
       (string-append
        "import json, sys\n"
        "def send(msg):\n"
        "    data = json.dumps(msg).encode('utf-8')\n"
        "    sys.stdout.buffer.write(f'Content-Length: {len(data)}\\r\\n\\r\\n'.encode('ascii') + data)\n"
        "    sys.stdout.buffer.flush()\n"
        "def recv():\n"
        "    headers = {}\n"
        "    while True:\n"
        "        line = sys.stdin.buffer.readline()\n"
        "        if not line: return None\n"
        "        if line in (b'\\r\\n', b'\\n'): break\n"
        "        k, v = line.decode('ascii').split(':', 1)\n"
        "        headers[k.strip().lower()] = v.strip()\n"
        "    n = int(headers.get('content-length', '0'))\n"
        "    return json.loads(sys.stdin.buffer.read(n).decode('utf-8'))\n"
        "next_id = 100\n"
        "pending = {}\n"
        "while True:\n"
        "    msg = recv()\n"
        "    if msg is None: break\n"
        "    if 'method' in msg:\n"
        "        m = msg['method']\n"
        "        if m == 'initialize':\n"
        "            send({'jsonrpc': '2.0', 'id': msg['id'], 'result': {'capabilities': {'textDocumentSync': 1}}})\n"
        "        elif m == 'shutdown':\n"
        "            send({'jsonrpc': '2.0', 'id': msg['id'], 'result': None})\n"
        "        elif m == 'exit':\n"
        "            break\n"
        "        elif m == 'test/probeConfig':\n"
        "            req_id = next_id\n"
        "            next_id += 1\n"
        "            pending[req_id] = msg['id']\n"
        "            send({'jsonrpc': '2.0', 'id': req_id, 'method': 'workspace/configuration', 'params': {'items': msg['params']['items']}})\n"
        "    elif 'id' in msg and msg['id'] in pending:\n"
        "        caller_id = pending.pop(msg['id'])\n"
        "        send({'jsonrpc': '2.0', 'id': caller_id, 'result': msg.get('result')})\n")
       p)))
  (parameterize ((default-trust-store-path store-path))
    (project-settings-invalidate!)
    (project-trust-approve! ws-a 'lsp-settings (dict "fake" (dict "owner" "A")))
    (project-trust-approve! ws-b 'lsp-settings (dict "fake" (dict "owner" "B" "secret" "from-B")))
    (with-scope sc-lsp
      (register-lsp-server! 'fake-scope-srv "Fake LSP server for scopeUri isolation tests."
                            #:command (list "python3" srv-script)
                            #:languages '(rust)
                            #:settings (dict "fake" (dict "base" "yes"))))
    (let* ((s (start-session (make-view (make-document "") "user") "emacs"))
           (buf-a (add-buffer! (file-document file-a)))
           (bsub (add-buffer! (file-document file-sub)))
           (bb (add-buffer! (file-document file-b)))
           (bpriv (add-buffer! (file-document priv-a)))
           (dpriv (buffer-document bpriv)))
      (mark-document-private! dpriv)
      (set-option! 'lsp-settings (dict "fake" (dict "innerBufSecret" "inner-only")) #:buffer bsub)
      (set-option! 'lsp-settings (dict "fake" (dict "bufLocal" "from-B-buf")) #:buffer bb)
      (set-option! 'lsp-settings (dict "fake" (dict "privKey" "top-secret")) #:buffer bpriv)
      (project-settings-load! ws-a)
      (project-settings-load! ws-sub)
      (project-settings-load! ws-b)
      (let* ((ls-a (car (lsp-attach! buf-a)))
             (ls-sub (car (lsp-attach! bsub)))
             (ls-b (car (lsp-attach! bb)))
             (_sync-a (lsp-sync! ls-a (buffer-document buf-a)))
             (_sync-sub (lsp-sync! ls-sub (buffer-document bsub)))
             (_sync-b (lsp-sync! ls-b (buffer-document bb)))
             (items (vector
                     (dict "scopeUri" (file-uri file-a) "section" "fake")
                     (dict "scopeUri" (file-uri file-b) "section" "fake")
                     (dict "scopeUri" (file-uri priv-a) "section" "fake")
                     (dict "scopeUri" (file-uri esc-a) "section" "fake")
                     (dict "scopeUri" (file-uri gpg-a) "section" "fake")
                     (dict "scopeUri" (file-uri file-sub) "section" "fake")))
             (res (lsp-request ls-a "test/probeConfig" (dict "items" items) #:timeout 5000))
             (r-own (vector-ref res 0))
             (r-foreign (vector-ref res 1))
             (r-priv (vector-ref res 2))
             (r-esc (vector-ref res 3))
             (r-gpg (vector-ref res 4))
             (r-nested-from-outer (vector-ref res 5))
             (res-inner (lsp-request ls-sub "test/probeConfig"
                                     (dict "items" (vector (dict "scopeUri" (file-uri file-sub) "section" "fake")))
                                     #:timeout 5000))
             (r-nested-from-inner (vector-ref res-inner 0)))
        (check "two distinct live LSP sessions exist" #f (eq? ls-a ls-b))
        (check "nested inner workspace has its own distinct LSP session" #f (eq? ls-a ls-sub))
        (check "own workspace file sees workspace A owner" "A" (dict-ref r-own "owner" #f))
        (check "foreign open buffer scopeUri falls back to workspace A owner" "A" (dict-ref r-foreign "owner" #f))
        (check "foreign open buffer scopeUri never leaks workspace B project secret" #f (dict-ref r-foreign "secret" #f))
        (check "foreign open buffer scopeUri never leaks workspace B buffer-local setting" #f (dict-ref r-foreign "bufLocal" #f))
        (check "private open buffer scopeUri falls back to workspace A owner" "A" (dict-ref r-priv "owner" #f))
        (check "private open buffer scopeUri never leaks private buffer-local setting" #f (dict-ref r-priv "privKey" #f))
        (check "symlink escape scopeUri never leaks workspace B owner" "A" (dict-ref r-esc "owner" #f))
        (check "symlink escape scopeUri never leaks workspace B secret" #f (dict-ref r-esc "secret" #f))
        (check "private codec path scopeUri falls back to workspace A owner" "A" (dict-ref r-gpg "owner" #f))
        (check "outer session querying nested inner buffer NEVER receives inner buffer-local setting"
               #f (dict-ref r-nested-from-outer "innerBufSecret" #f))
        (check "inner session querying its own attached buffer DOES receive its buffer-local setting"
               "inner-only" (dict-ref r-nested-from-inner "innerBufSecret" #f))
        (show-buffer! s buf-a)
        (lsp-shutdown s 1)
        (show-buffer! s bsub)
        (lsp-shutdown s 1)
        (show-buffer! s bb)
        (lsp-shutdown s 1))))
  (scope-shutdown! sc-lsp))

;; ---------------------------------------------------------------------------
;; 4. Approval UI shows complete escaped values (no truncation) and maps
;;    candidates by stable target identity through the real runtime package.
;; ---------------------------------------------------------------------------
(define-option test-risky-opt-a "" "Risky option A." #:type 'string)
(define-option test-risky-opt-b "" "Risky option B." #:type 'string)

(let* ((ui-dir (string-append tmp "/ui-approval"))
       (ui-file (string-append ui-dir "/main.txt"))
       (ui-store (string-append tmp "/ui-trust.scm"))
       (prefix (make-string 150 #\x))
       (val-a (string-append prefix "-TAIL-ALPHA-" (string (integer->char 27)) "-END-A"))
       (val-b (string-append prefix "-TAIL-BETA-END-B")))
  (create-directory ui-dir #o700)
  (call-with-output-file ui-file (lambda (p) (display "hello\n" p)))
  (call-with-output-file (string-append ui-dir "/.techne.scm")
    (lambda (p)
      (display "(techne-settings 1 (root #t) (all\n  (test-risky-opt-a " p)
      (write val-a p)
      (display ")\n  (test-risky-opt-b " p)
      (write val-b p)
      (display ")))\n" p)))
  (parameterize ((default-trust-store-path ui-store))
    (project-settings-invalidate!)
    (let* ((s (start-session (make-view (make-document "") "user") "emacs"))
           (b (add-buffer! (file-document ui-file))))
      (show-buffer! s b)
      (project-settings-load! ui-dir)
      ;; Invoke project-settings-trust interactively as user principal.
      (run-command s 'project-settings-trust 1)
      (assert-true "selector minibuffer opened for 2 pending entries" (minibuffer-open? s))
      (let* ((cands (minibuffer-candidates s))
             (cand0 (car cands))
             (cand1 (cadr cands))
             (txt0 (candidate-text cand0))
             (txt1 (candidate-text cand1)))
        (check "two pending candidates offered" 2 (length cands))
        (assert-true "candidate 0 displays full untruncated tail" (and (string-contains txt0 "-TAIL-ALPHA-") #t))
        (assert-true "candidate 0 escapes ESC control character visibly" (and (string-contains txt0 "\\x1b;") #t))
        (assert-true "candidate 0 has no ellipsis truncation" (not (string-contains txt0 "...")))
        (assert-true "candidate 1 displays full untruncated tail" (and (string-contains txt1 "-TAIL-BETA-END-B") #t))
        (assert-true "candidate 1 carries stable target entry" (and (list? (candidate-target cand1))
                                                                    (eq? (list-ref (candidate-target cand1) 1) 'test-risky-opt-b)))
        ;; Select candidate 1 (second entry) and accept -> opens confirmation prompt for opt-b.
        (press-keys s "C-n RET")
        (assert-true "confirmation minibuffer opened" (minibuffer-open? s))
        ;; Approve in confirmation prompt (choices are: cancel, approve, deny, visit -> next once to approve).
        (press-keys s "C-n RET")
        (check "only selected candidate B was approved" 'trusted
               (project-trust-verdict ui-dir 'test-risky-opt-b val-b))
        (check "unselected candidate A remains unknown" 'unknown
               (project-trust-verdict ui-dir 'test-risky-opt-a val-a))
        (check "approved option B takes effect on buffer" val-b (option b 'test-risky-opt-b))
        (check "unapproved option A stays at default" "" (option b 'test-risky-opt-a))))))

;; ---------------------------------------------------------------------------
;; 4. Trust ledger entries and cached snapshots return defensive copies, and
;;    memory-only ('none) trust stays isolated from file-backed editor ledgers.
;; ---------------------------------------------------------------------------
(let* ((mut-proj-a (string-append tmp "/mut-proj-a"))
       (mut-proj-b (string-append tmp "/mut-proj-b"))
       (ledger-real (string-append tmp "/ledger-a.scm"))
       (ledger-evil (string-append tmp "/ledger-b.scm"))
       (ps-env (environment '(techne project-settings)))
       (snap-techne-files (eval 'project-settings-snapshot-techne-files ps-env))
       (file-rules (eval 'project-settings-file-rules ps-env))
       (rule-source-dir (eval 'settings-rule-source-dir ps-env))
       (rule-value (eval 'settings-rule-value ps-env)))
  (create-directory mut-proj-a #o700)
  (create-directory mut-proj-b #o700)
  (call-with-output-file (string-append mut-proj-a "/.techne.scm")
    (lambda (p) (display "(techne-settings 1 (root #t) (all (compile-command \"shared-cmd\")))\n" p)))
  (call-with-output-file (string-append mut-proj-b "/.techne.scm")
    (lambda (p) (display "(techne-settings 1 (root #t) (all (compile-command \"shared-cmd\")))\n" p)))
  (parameterize ((default-trust-store-path ledger-evil))
    (project-trust-approve! mut-proj-b 'compile-command "shared-cmd"))
  (let ((store-arg (string-copy ledger-real)))
    (parameterize ((default-trust-store-path store-arg))
      ;; Mutating the caller's original string must not redirect default-trust-store-path.
      (string-copy! store-arg 0 ledger-evil)
      (check "mutating caller string does not alter default-trust-store-path"
             ledger-real (default-trust-store-path))
      ;; Approve mut-proj-a in ledger-real; mutate returned digest string.
      (let ((d (project-trust-approve! mut-proj-a 'compile-command "shared-cmd")))
        (string-fill! d #\0)
        (check "mutating returned digest does not corrupt %trust-table" 'trusted
               (project-trust-verdict mut-proj-a 'compile-command "shared-cmd")))
      (project-settings-invalidate!)
      (project-settings-load! mut-proj-a)
      (project-settings-load! mut-proj-b)
      ;; Mutating returned trust entries or cached snapshot lists/strings must not corrupt caches.
      (let ((entries (project-trust-entries)))
        (for-each (lambda (e)
                    (when (= (string-length (car e)) (string-length (string-append mut-proj-b "/")))
                      (string-copy! (car e) 0 (string-append mut-proj-b "/")))
                    (string-fill! (caddr e) #\f))
                  entries))
      (let* ((snap-b (project-settings-cached-snapshot mut-proj-b))
             (tfs (snap-techne-files snap-b))
             (rules (file-rules (car tfs)))
             (rule (car rules)))
        (string-copy! (rule-source-dir rule) 0 (string-append mut-proj-a "/"))
        (string-copy! (rule-value rule) 0 "hacked-cmd")
        (set-car! rules 'bogus)
        (set-car! tfs 'bogus))
      (check "mutating trust-entries/cached-rule never trusts mut-proj-b"
             'unknown (project-trust-verdict mut-proj-b 'compile-command "shared-cmd"))
      (check "mut-proj-a remains trusted after returned-copy mutations"
             'trusted (project-trust-verdict mut-proj-a 'compile-command "shared-cmd"))
      (let* ((snap-b (project-settings-cached-snapshot mut-proj-b))
             (rule (car (file-rules (car (snap-techne-files snap-b))))))
        (check "cached rule source-dir unaffected by returned-copy mutation"
               (string-append mut-proj-b "/") (rule-source-dir rule))
        (check "cached rule value unaffected by returned-copy mutation"
               "shared-cmd" (rule-value rule)))
      (check "mut-proj-b compile-command stays at default (untrusted)"
             (option #f 'compile-command) (option mut-proj-b 'compile-command)))))

(let ((iso-dir (string-append tmp "/iso-editor-proj"))
      (iso-ledger (string-append tmp "/iso-editor-trust.scm")))
  (create-directory iso-dir #o700)
  (call-with-output-file (string-append iso-dir "/.techne.scm")
    (lambda (p) (display "(techne-settings 1 (root #t) (all (compile-command \"editor-cmd\")))\n" p)))
  ;; Verify 'none memory-only isolation through editor wrappers.
  (parameterize ((default-trust-store-path iso-ledger))
    (project-trust-approve! iso-dir 'compile-command "editor-cmd-A")
    (parameterize ((default-trust-store-path 'none))
      (project-trust-approve! iso-dir 'compile-command "editor-cmd-mem-B")
      (check "editor 'none approves B in memory" 'trusted
             (project-trust-verdict iso-dir 'compile-command "editor-cmd-mem-B")))
    (check "editor file-backed store does not trust memory-only B" 'unknown
           (project-trust-verdict iso-dir 'compile-command "editor-cmd-mem-B"))
    (project-trust-approve! iso-dir 'compile-command "editor-cmd-C")
    (project-trust-clear-memory!)
    (check "editor reloaded file-backed entries exclude memory-only B" 2
           (length (project-trust-entries)))
    (check "editor reloaded file-backed store keeps B unknown" 'unknown
           (project-trust-verdict iso-dir 'compile-command "editor-cmd-mem-B"))))

(delete-path tmp #t)
(test-failures)
