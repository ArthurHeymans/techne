;;; Tests for `(techne location)`, `(techne endpoint)`, and
;;; `(techne execution-site)` including launch policies, direnv lifecycle,
;;; poisoned project PATH isolation, concurrency dedup, and real direnv E2E.
(import (techne) (techne location) (techne endpoint) (techne execution-site)
        (techne trusted-process) (techne uuid) (techne crypto))
(import (techne test))
(test-file #:environment (quote host))

(define (techne-bin)
  (or (get-environment-variable "TECHNE_BIN")
      (and (file-exists? "./target/release/techne") "./target/release/techne")
      "./target/debug/techne"))

(define tmp (string-append (or (get-environment-variable "TMPDIR") "/tmp") "/techne-exec-site-" (uuid-v4)))
(create-directory tmp #o700)

(define (write-text path text)
  (call-with-output-file path (lambda (p) (display text p))))

(define (make-script path body)
  (write-text path body)
  (call-with-process "chmod" (list "+x" path) (lambda (p) (process-wait p))))

(define (rm-rf path)
  (call-with-process "rm" (list "-rf" "--" path) (lambda (p) (process-wait p))))

(define (capture-site site program args #:environment [environment '()])
  (call-with-process-at-site site program args
    (lambda (p)
      (let ((out (process-read-all p 'stdout)))
        (process-wait p)
        (string-trim out)))
    #:environment environment
    #:stdin 'null))

;;; 1. Headless (techne location)
(check "local path not remote" #f (remote-path? "/tmp/file.txt"))
(check "ssh path is remote" #t (remote-path? "/ssh:arthur@build#2222:/etc/hosts"))
(check "multi-hop route length" 2 (length (location-route "/ssh:arthur@jump|sudo:root@jump:/etc/hosts")))
(check "location-path extracts remote path" "/etc/hosts" (location-path "/ssh:arthur@build#2222:/etc/hosts"))
(check "location-prefix extracts prefix" "/ssh:arthur@build#2222:" (location-prefix "/ssh:arthur@build#2222:/etc/hosts"))
(check "location-at joins prefix and path" "/ssh:build:/var/log" (location-at "/ssh:build:/etc/hosts" "/var/log"))
(check "option-injection host rejected" #f (remote-path? "/ssh:-oProxyCommand=evil:/tmp/x"))

(let ((sc (make-scope 'loc-method-test)))
  (with-scope sc
    (define-method 'custom-hop "Temporary hop method."
      #:command (lambda (h jumps prog) prog))
    (check "scoped method recognized" #t (remote-path? "/custom-hop:box:/tmp/a")))
  (scope-shutdown! sc)
  (check "scoped method removed after shutdown" #f (find-method 'custom-hop))
  (check "built-in ssh survives scope shutdown" #t (and (find-method 'ssh) #t)))

;;; 2. Headless (techne endpoint): project root discovery
(let* ((repo (string-append tmp "/repo"))
       (sub (string-append repo "/a/b")))
  (make-directory sub)
  (create-directory (string-append repo "/.git") #o700)
  (check "endpoint-project-root finds .git above subdir"
         (string-append repo "/")
         (endpoint-project-root sub)))

;;; 3. Launch policy validation & scope-owned env providers
(check "invalid policy raises" #t
       (guard (e (#t #t))
         (make-execution-site tmp #:policy 'bogus)
         #f))

(let ((sc (make-scope 'env-provider-test)))
  (with-scope sc
    (define-env-provider (fake-venv site refresh?)
      "Temporary test env provider."
      (make-env-snapshot 'fake-venv 'active #:delta '(("FAKE_VENV" . "on"))))
    (check "scoped env provider registered" #t (procedure? (find-env-provider 'fake-venv))))
  (parameterize ((default-env-providers '(fake-venv)))
    (let ((out (capture-site (make-execution-site tmp #:policy 'project) "sh" '("-c" "printf %s \"$FAKE_VENV\""))))
      (check "scoped env provider supplies project env" "on" out)))
  (scope-shutdown! sc)
  (check "scoped env provider removed on shutdown" #f (find-env-provider 'fake-venv))
  (check "built-in direnv survives scope shutdown" #t (procedure? (find-env-provider 'direnv))))

;;; 4. Stub direnv: zero-spawn fast path, blocked, denied, failing partial diff, active, unset, PATH, dedup
(define plain-dir (string-append tmp "/plain-no-rc"))
(create-directory plain-dir #o700)

(define fail-if-called (string-append tmp "/fail-if-called"))
(make-script fail-if-called "#!/bin/sh\necho SHOULD_NOT_RUN >&2\nexit 99\n")

(unless (get-environment-variable "DIRENV_DIR")
  (parameterize ((default-direnv-enabled #t) (default-direnv-program fail-if-called))
    (execution-site-invalidate!)
    (let ((snap (execution-site-env-snapshot (make-execution-site plain-dir #:policy 'project))))
      (check "no .envrc returns none without spawning direnv" 'none (env-snapshot-status snap))
      (check "no .envrc has empty delta" '() (env-snapshot-delta snap)))))

(define stub-proj (string-append tmp "/stub-proj"))
(define stub-bin (string-append stub-proj "/bin"))
(define evil-bin (string-append stub-proj "/evil-bin"))
(make-directory stub-bin)
(make-directory evil-bin)
(write-text (string-append stub-proj "/.envrc") "export STUB_VAR=hello\n")

(make-script (string-append stub-bin "/stub-proj-tool")
  "#!/bin/sh\necho FROM-PROJ-TOOL:$STUB_VAR\n")
(make-script (string-append evil-bin "/gpg")
  "#!/bin/sh\necho POISONED-GPG\n")
(make-script (string-append evil-bin "/direnv")
  "#!/bin/sh\necho POISONED-DIRENV\n")

(define stub-mode-file (string-append tmp "/stub-mode"))
(define stub-count-file (string-append tmp "/stub-count"))
(write-text stub-count-file "0\n")

(define stub-direnv (string-append tmp "/stub-direnv"))
(make-script stub-direnv
  (string-append
   "#!/bin/sh\n"
   "mode=$(cat '" stub-mode-file "')\n"
   "if [ \"$1\" = \"status\" ] && [ \"$2\" = \"--json\" ]; then\n"
   "  case \"$mode\" in\n"
   "    blocked) printf '{\"state\":{\"foundRC\":{\"allowed\":1,\"path\":\"" stub-proj "/.envrc\"}}}' ;;\n"
   "    denied)  printf '{\"state\":{\"foundRC\":{\"allowed\":2,\"path\":\"" stub-proj "/.envrc\"}}}' ;;\n"
   "    active|failing|slow) printf '{\"state\":{\"foundRC\":{\"allowed\":0,\"path\":\"" stub-proj "/.envrc\"}}}' ;;\n"
   "  esac\n"
   "  exit 0\n"
   "fi\n"
   "if [ \"$1\" = \"export\" ] && [ \"$2\" = \"json\" ]; then\n"
   "  c=$(cat '" stub-count-file "'); echo $((c + 1)) > '" stub-count-file "'\n"
   "  case \"$mode\" in\n"
   "    blocked) echo 'should not export blocked' >&2; exit 99 ;;\n"
   "    failing)\n"
   "      printf '{\"PARTIAL_LEAK\":\"bad\"}'\n"
   "      echo 'direnv: error exit status 1' >&2\n"
   "      exit 1\n"
   "      ;;\n"
   "    slow)\n"
   "      sleep 0.15\n"
   "      printf '{\"STUB_VAR\":\"from-slow\",\"PATH\":\"" stub-bin ":" evil-bin ":'\"$PATH\"'\"}'\n"
   "      exit 0\n"
   "      ;;\n"
   "    active)\n"
   "      printf '{\"STUB_VAR\":\"hello-stub\",\"TO_UNSET\":null,\"PATH\":\"" stub-bin ":" evil-bin ":'\"$PATH\"'\"}'\n"
   "      exit 0\n"
   "      ;;\n"
   "  esac\n"
   "fi\n"
   "exit 2\n"))

(parameterize ((default-direnv-enabled #t) (default-direnv-program stub-direnv))
  ;; Blocked: status blocked, empty delta, never invokes export.
  (write-text stub-mode-file "blocked")
  (execution-site-invalidate!)
  (let ((snap (execution-site-env-snapshot (make-execution-site stub-proj #:policy 'project))))
    (check "blocked .envrc reports blocked" 'blocked (env-snapshot-status snap))
    (check "blocked .envrc has empty delta" '() (env-snapshot-delta snap))
    (check "blocked .envrc never ran export" "0" (string-trim (file->string stub-count-file))))

  ;; Denied: status denied, empty delta.
  (write-text stub-mode-file "denied")
  (execution-site-invalidate!)
  (let ((snap (execution-site-env-snapshot (make-execution-site stub-proj #:policy 'project))))
    (check "denied .envrc reports denied" 'denied (env-snapshot-status snap))
    (check "denied .envrc has empty delta" '() (env-snapshot-delta snap)))

  ;; Failing export: non-zero exit NEVER publishes partial stdout JSON!
  (write-text stub-mode-file "failing")
  (execution-site-invalidate!)
  (let ((snap (execution-site-env-snapshot (make-execution-site stub-proj #:policy 'project))))
    (check "failing export reports error" 'error (env-snapshot-status snap))
    (check "failing export never publishes partial delta" '() (env-snapshot-delta snap)))

  ;; Active: project policy gets STUB_VAR + project PATH + unset; endpoint-base & user-trusted DO NOT!
  (write-text stub-mode-file "active")
  (write-text stub-count-file "0\n")
  (execution-site-invalidate!)
  (let* ((proj-site (make-execution-site stub-proj #:policy 'project))
         (base-site (make-execution-site stub-proj #:policy 'endpoint-base))
         (trust-site (make-execution-site stub-proj #:policy 'user-trusted))
         (snap (execution-site-env-snapshot proj-site)))
    (check "active .envrc reports active" 'active (env-snapshot-status snap))
    (check "execution-site-find-program finds project-only tool"
           (string-append stub-bin "/stub-proj-tool")
           (execution-site-find-program proj-site "stub-proj-tool"))
    (check "project policy runs tool from project PATH with env"
           "FROM-PROJ-TOOL:hello-stub"
           (capture-site proj-site "stub-proj-tool" '()))
    (check "caller environment overrides direnv delta"
           "FROM-PROJ-TOOL:override"
           (capture-site proj-site "stub-proj-tool" '() #:environment '(("STUB_VAR" . "override"))))
    (check "cached snapshot does not re-spawn direnv export"
           "1"
           (string-trim (file->string stub-count-file)))
    ;; Endpoint-base policy: runs in stub-proj dir, NO direnv vars, NO poisoned PATH!
    (check "endpoint-base does not inherit project env"
           ""
           (capture-site base-site "sh" '("-c" "printf %s \"$STUB_VAR\"")))
    (check "endpoint-base runs in project directory"
           stub-proj
           (capture-site base-site "pwd" '()))
    (check "endpoint-base does not find poisoned gpg on PATH"
           #f
           (equal? (execution-site-find-program base-site "gpg") (string-append evil-bin "/gpg")))
    ;; User-trusted policy: runs in trusted-cwd, NO project env, NO poisoned gpg!
    (check "user-trusted runs in trusted-cwd, never project dir"
           (trusted-cwd)
           (capture-site trust-site "pwd" '()))
    (check "user-trusted does not inherit project env"
           ""
           (capture-site trust-site "sh" '("-c" "printf %s \"$STUB_VAR\"")))
    (check "run-trusted-tool never sees poisoned project gpg"
           #f
           (let ((res (run-trusted-tool "sh" '("-c" "command -v gpg || true"))))
             (equal? (string-trim (cadr res)) (string-append evil-bin "/gpg")))))

  ;; Single-flight concurrency deduplication: 4 concurrent callers spawn direnv export ONCE.
  (write-text stub-mode-file "slow")
  (write-text stub-count-file "0\n")
  (execution-site-invalidate!)
  (let* ((site (make-execution-site stub-proj #:policy 'project))
         (tasks (list (spawn (lambda () (execution-site-env-snapshot site)))
                      (spawn (lambda () (execution-site-env-snapshot site)))
                      (spawn (lambda () (execution-site-env-snapshot site)))
                      (spawn (lambda () (execution-site-env-snapshot site)))))
         (snaps (map task-join tasks)))
    (check "all 4 concurrent callers get active snapshot" #t
           (every (lambda (s) (eq? (env-snapshot-status s) 'active)) snaps))
    (check "single-flight dedup ran direnv export exactly once" "1"
           (string-trim (file->string stub-count-file)))))

;; Missing direnv executable degrades cleanly to 'unavailable with empty delta.
(parameterize ((default-direnv-enabled #t) (default-direnv-program "/nonexistent/techne-direnv-missing"))
  (execution-site-invalidate!)
  (let ((snap (execution-site-env-snapshot (make-execution-site stub-proj #:policy 'project))))
    (check "missing direnv reports unavailable" 'unavailable (env-snapshot-status snap))
    (check "missing direnv has empty delta" '() (env-snapshot-delta snap))))

;;; 5. Real direnv end-to-end (isolated XDG_DATA_HOME / XDG_CONFIG_HOME)
(define (command-available? name)
  (guard (e (#t #f))
    (call-with-process "sh" (list "-c" (string-append "command -v " name " >/dev/null 2>&1"))
      (lambda (p) (equal? (process-wait p) 0))
      #:stdin 'null)))

(when (command-available? "direnv")
  (let* ((real-xdg-data (string-append tmp "/xdg-data"))
         (real-xdg-cfg (string-append tmp "/xdg-cfg"))
         (real-proj (string-append tmp "/real-direnv-proj"))
         (real-sub (string-append real-proj "/sub"))
         (real-bin (string-append real-proj "/bin"))
         (direnv-wrap (string-append tmp "/real-direnv-wrap")))
    (make-directory real-xdg-data)
    (make-directory real-xdg-cfg)
    (make-directory real-sub)
    (make-directory real-bin)
    (make-script (string-append real-bin "/real-direnv-tool")
      "#!/bin/sh\necho REAL-OK:$TECHNE_REAL_DIRENV\n")
    (write-text (string-append real-proj "/.envrc")
      (string-append "export TECHNE_REAL_DIRENV=loaded-42\nPATH_add \"" real-bin "\"\n"))
    ;; Wrap real direnv with isolated XDG dirs and clean inherited DIRENV_* so tests never touch user allowlist.
    (make-script direnv-wrap
      (string-append
       "#!/bin/sh\n"
       "unset DIRENV_DIR DIRENV_DIFF DIRENV_FILE DIRENV_WATCHES\n"
       "export XDG_DATA_HOME='" real-xdg-data "'\n"
       "export XDG_CONFIG_HOME='" real-xdg-cfg "'\n"
       "exec direnv \"$@\"\n"))
    (parameterize ((default-direnv-enabled #t)
                   (default-direnv-program direnv-wrap)
                   (default-direnv-data-dir (string-append (file-truename real-xdg-data) "/direnv")))
      (execution-site-invalidate!)
      (let ((s0 (execution-site-env-snapshot (make-execution-site real-sub #:policy 'project))))
        (check "real direnv starts blocked" 'blocked (env-snapshot-status s0))
        (check "real direnv blocked has empty delta" '() (env-snapshot-delta s0)))
      ;; Approve from subdirectory; finds parent .envrc, allows it, and reloads.
      (let ((s1 (execution-site-direnv-allow! real-sub)))
        (check "real direnv allow activates snapshot" 'active (env-snapshot-status s1))
        (check "real direnv finds tool in PATH_add dir"
               (string-append real-bin "/real-direnv-tool")
               (execution-site-find-program (make-execution-site real-sub #:policy 'project) "real-direnv-tool"))
        (check "real direnv runs tool with exported env"
               "REAL-OK:loaded-42"
               (capture-site (make-execution-site real-sub #:policy 'project) "real-direnv-tool" '())))
      ;; Deny revokes approval and clears project env.
      (let ((s2 (execution-site-direnv-deny! real-sub)))
        (check "real direnv deny transitions to denied" 'denied (env-snapshot-status s2))
        (check "real direnv denied no longer sets exported var"
               ""
               (capture-site (make-execution-site real-sub #:policy 'project)
                             "sh" '("-c" "printf %s \"$TECHNE_REAL_DIRENV\""))))

      ;; Watch-file & external direnv deny invalidation via DIRENV_WATCHES.
      (let* ((watch-proj (string-append tmp "/watch-proj"))
             (watch-file (string-append watch-proj "/watched.cfg")))
        (make-directory watch-proj)
        (write-text watch-file "v1\n")
        (write-text (string-append watch-proj "/.envrc")
          (string-append "watch_file watched.cfg\nexport WATCH_VAL=\"$(cat watched.cfg)\"\n"))
        (let ((s3 (execution-site-direnv-allow! watch-proj)))
          (check "watch_file initial active" 'active (env-snapshot-status s3))
          (check "watch_file initial value" "v1"
                 (capture-site (make-execution-site watch-proj #:policy 'project)
                               "sh" '("-c" "printf %s \"$WATCH_VAL\""))))
        ;; Edit watched.cfg (change size so even same-second fingerprint changes).
        (write-text watch-file "v2-updated\n")
        (check "editing watch_file marks site needing eval" #t
               (execution-site-needs-eval? watch-proj))
        (let ((s4 (execution-site-env-snapshot (make-execution-site watch-proj #:policy 'project))))
          (check "watch_file edit reloads new value" "v2-updated"
                 (capture-site (make-execution-site watch-proj #:policy 'project)
                               "sh" '("-c" "printf %s \"$WATCH_VAL\""))))
        ;; External direnv deny (outside Techne's API) immediately busts cache via DIRENV_WATCHES allow/deny files.
        (call-with-process direnv-wrap (list "deny" (string-append watch-proj "/.envrc"))
          (lambda (p) (process-wait p))
          #:directory watch-proj #:stdin 'null)
        (check "external direnv deny marks site needing eval" #t
               (execution-site-needs-eval? watch-proj))
        (let ((s5 (execution-site-env-snapshot (make-execution-site watch-proj #:policy 'project))))
          (check "external direnv deny transitions cached active to denied" 'denied (env-snapshot-status s5))))

      ;; Failing .envrc stderr must NEVER leak secret values into snapshot message.
      (let ((err-proj (string-append tmp "/err-proj")))
        (make-directory err-proj)
        (write-text (string-append err-proj "/.envrc")
          "echo 'TOP_SECRET_STDERR_9999' >&2\nexit 1\n")
        (let ((s-err (execution-site-direnv-allow! err-proj)))
          (check "failing .envrc returns error snapshot" 'error (env-snapshot-status s-err))
          (check "failing .envrc message never leaks stderr" #f
                 (and (string? (env-snapshot-message s-err))
                      (string-contains (env-snapshot-message s-err) "TOP_SECRET_STDERR_9999")
                      #t))))

      ;; Symlinked .envrc allow & target edit invalidation.
      (let* ((sym-proj (string-append tmp "/sym-proj"))
             (sym-target (string-append tmp "/real-envrc-target"))
             (sym-link (string-append sym-proj "/.envrc")))
        (make-directory sym-proj)
        (write-text sym-target "export SYM_VAR=one\n")
        (call-with-process "ln" (list "-s" sym-target sym-link) (lambda (p) (process-wait p)) #:stdin 'null)
        (let ((s-sym (execution-site-direnv-allow! sym-proj)))
          (check "symlinked .envrc allow succeeds" 'active (env-snapshot-status s-sym))
          (check "symlinked .envrc exports var" "one"
                 (capture-site (make-execution-site sym-proj #:policy 'project)
                               "sh" '("-c" "printf %s \"$SYM_VAR\""))))
        (write-text sym-target "export SYM_VAR=two-longer\n")
        (check "editing symlink target invalidates freshness" #t
               (execution-site-needs-eval? sym-proj)))

      ;; Cryptographic digest-bound allow: writes only allow/<sha256(target + "\n" + GOOD)>,
      ;; blocks EVIL edits, refuses automatic removal of deny/, and rejects symlinked direnv-data-dir.
      (let* ((db-proj (string-append tmp "/digest-bound-proj"))
             (db-rc (string-append db-proj "/.envrc"))
             (good-text "export DIGEST_BOUND=good\n")
             (evil-text "export DIGEST_BOUND=evil\n")
             (db-data (string-append (file-truename real-xdg-data) "/direnv"))
             (allow-dir (string-append db-data "/allow"))
             (deny-dir (string-append db-data "/deny")))
        (make-directory db-proj)
        (write-text db-rc good-text)
        (let* ((canon-rc (file-truename db-rc))
               (good-digest (execution-site-direnv-file-digest canon-rc))
               (good-allow-hash (hex-encode (sha-256 (string->utf8 (string-append canon-rc "\n" good-text)))))
               (evil-allow-hash (hex-encode (sha-256 (string->utf8 (string-append canon-rc "\n" evil-text)))))
               (deny-hash (hex-encode (sha-256 (string->utf8 (string-append canon-rc "\n"))))))
          ;; Mismatched expected-digest fails closed and writes no allow record.
          (check "mismatched expected-digest refused" 'refused
                 (guard (e (#t 'refused))
                   (execution-site-direnv-allow! db-proj #:expected-target canon-rc #:expected-digest "0000")
                   'allowed))
          (check "mismatched expected-digest writes no allow record" #f
                 (file-exists? (string-append allow-dir "/" good-allow-hash)))
          ;; Matching digest writes ONLY good-allow-hash with mode 0600 and activates.
          (let ((s-good (execution-site-direnv-allow! db-proj #:expected-target canon-rc #:expected-digest good-digest)))
            (check "digest-bound allow activates with real direnv" 'active (env-snapshot-status s-good))
            (check "digest-bound allow wrote exact allow hash" #t
                   (file-exists? (string-append allow-dir "/" good-allow-hash)))
            (check "digest-bound allow file is owner-only 0600" #o600
                   (modulo (list-ref (file-attributes (string-append allow-dir "/" good-allow-hash)) 3) #o1000))
            (check "digest-bound allow did not write evil hash" #f
                   (file-exists? (string-append allow-dir "/" evil-allow-hash))))
          ;; Modifying .envrc to EVIL stays blocked by real direnv.
          (write-text db-rc evil-text)
          (execution-site-invalidate! db-proj)
          (let ((s-evil (execution-site-env-snapshot (make-execution-site db-proj #:policy 'project) #:refresh? #t)))
            (check "mutated EVIL .envrc stays blocked" 'blocked (env-snapshot-status s-evil))
            (check "EVIL .envrc has empty delta" '() (env-snapshot-delta s-evil)))
          ;; Restore GOOD, deny, then attempt re-allow:
          ;; Techne refuses automatic removal of an existing deny record (fail-closed);
          ;; once the deny record is explicitly cleared, digest-bound allow activates again.
          (write-text db-rc good-text)
          (let ((s-den (execution-site-direnv-deny! db-proj)))
            (check "deny creates deny record" 'denied (env-snapshot-status s-den))
            (check "deny hash file exists" #t (file-exists? (string-append deny-dir "/" deny-hash))))
          (let ((msg (guard (e (#t (if (error-object? e) (error-object-message e) "")))
                       (execution-site-direnv-allow! db-proj #:expected-target canon-rc #:expected-digest good-digest)
                       "unexpected-ok")))
            (check "allow refuses existing deny record" #t
                   (and (string-contains msg "existing deny record") #t))
            (check "existing deny hash file preserved on refused allow" #t
                   (file-exists? (string-append deny-dir "/" deny-hash)))
            (let ((s-still (execution-site-env-snapshot (make-execution-site db-proj #:policy 'project) #:refresh? #t)))
              (check "status remains denied after refused allow" 'denied (env-snapshot-status s-still))
              (check "delta remains empty after refused allow" '() (env-snapshot-delta s-still))))
          (delete-file (string-append deny-dir "/" deny-hash))
          (let ((s-re (execution-site-direnv-allow! db-proj #:expected-target canon-rc #:expected-digest good-digest)))
            (check "allow after explicit deny removal activates" 'active (env-snapshot-status s-re))
            (check "allow hash file written after explicit deny removal" #t
                   (file-exists? (string-append allow-dir "/" good-allow-hash))))
          ;; Symlinked direnv-data-dir fails closed without creating any dirs/files in target.
          (let* ((sym-outside (string-append tmp "/sym-data-outside"))
                 (sym-parent (string-append tmp "/sym-data-xdg"))
                 (sym-data (string-append sym-parent "/direnv")))
            (make-directory sym-outside)
            (make-directory sym-parent)
            (call-with-process "ln" (list "-s" sym-outside sym-data) (lambda (p) (process-wait p)) #:stdin 'null)
            (parameterize ((default-direnv-data-dir sym-data))
              (check "symlinked direnv-data-dir refused" 'refused
                     (guard (e (#t 'refused))
                       (execution-site-direnv-allow! db-proj #:expected-target canon-rc #:expected-digest good-digest)
                       'allowed)))
            (check "symlinked direnv-data-dir creates nothing in outside target" '()
                   (directory-list sym-outside)))))

      ;; Timeout kills hanging .envrc promptly.
      (let ((hang-proj (string-append tmp "/hang-proj")))
        (make-directory hang-proj)
        (write-text (string-append hang-proj "/.envrc") "sleep 5\nexport NEVER=1\n")
        (call-with-process direnv-wrap (list "allow" (string-append hang-proj "/.envrc"))
          (lambda (p) (process-wait p)) #:directory hang-proj #:stdin 'null)
        (parameterize ((default-direnv-timeout 200))
          (let ((s-to (execution-site-env-snapshot (make-execution-site hang-proj #:policy 'project))))
            (check "hanging .envrc times out cleanly" 'error (env-snapshot-status s-to))
            (check "timeout message says timed out" #t
                   (and (string? (env-snapshot-message s-to))
                        (string-contains (env-snapshot-message s-to) "timed out")
                        #t)))))

      ;; Inherited startup DIRENV_* baseline cleanup (verified in child techne process).
      (let* ((inherited-proj (string-append tmp "/inherited-proj"))
             (inherited-bin (string-append inherited-proj "/bin"))
             (other-dir (string-append tmp "/inherited-other"))
             (probe-script (string-append tmp "/inherited-probe.scm")))
        (make-directory inherited-bin)
        (make-directory other-dir)
        (make-script (string-append inherited-bin "/fake-poison-tool") "#!/bin/sh\necho POISONED\n")
        (write-text (string-append inherited-proj "/.envrc")
          (string-append "export INHERITED_LEAK=from-startup-shell\nPATH_add \"" inherited-bin "\"\n"))
        (call-with-process direnv-wrap (list "allow" (string-append inherited-proj "/.envrc"))
          (lambda (p) (process-wait p)) #:directory inherited-proj #:stdin 'null)
        ;; Capture real direnv export env from inherited-proj to simulate launching techne inside that shell.
        (let* ((read-stdout
                (lambda (p)
                  (let loop ((xs '()))
                    (let ((c (process-read p 'stdout)))
                      (if (eof-object? c) (apply string-append (reverse xs)) (loop (cons c xs)))))))
               (exp-json (call-with-process direnv-wrap '("export" "json")
                           (lambda (p)
                             (let ((out (read-stdout p)))
                               (process-wait p)
                               out))
                           #:directory inherited-proj #:stdin 'null))
               (parsed (string->json (string-trim exp-json)))
               (child-env (append
                           (list (cons "XDG_DATA_HOME" real-xdg-data)
                                 (cons "XDG_CONFIG_HOME" real-xdg-cfg))
                           (filter-map (lambda (k)
                                         (let ((v (dict-ref parsed k 'missing)))
                                           (and (string? v) (cons k v))))
                                       (dict-keys parsed)))))
          (write-text probe-script
            (string-append
             "(import (techne) (techne execution-site) (techne trusted-process))\n"
             "(define (run-sh site)\n"
             "  (call-with-process-at-site site \"sh\" '(\"-c\" \"printf '%s|%s' \\\"$INHERITED_LEAK\\\" \\\"$(command -v fake-poison-tool || echo clean)\\\"\")\n"
             "    (lambda (p)\n"
             "      (let loop ((xs '()))\n"
             "        (let ((c (process-read p 'stdout)))\n"
             "          (if (eof-object? c) (begin (process-wait p) (apply string-append (reverse xs)))\n"
             "              (loop (cons c xs))))))))\n"
             "(display (run-sh (make-execution-site \"" other-dir "\" #:policy 'endpoint-base)))\n"
             "(display \"\\n\")\n"
             "(display (run-sh (make-execution-site \"" other-dir "\" #:policy 'user-trusted)))\n"
             "(display \"\\n\")\n"
             "(display (run-sh (make-execution-site \"" other-dir "\" #:policy 'project)))\n"
             "(display \"\\n\")\n"))
          (let ((out (call-with-process (techne-bin) (list "run" probe-script)
                       (lambda (p)
                         (let ((s (read-stdout p)))
                           (process-wait p)
                           s))
                       #:environment child-env #:stdin 'null)))
            (check "inherited DIRENV_* unloaded in endpoint-base, user-trusted, and clean project"
                   "|clean\n|clean\n|clean"
                   (string-trim out))))))))

;;; 6. Public name lists, cold-import survival, find-program, bad providers, relative direnv-program
(check "method-names includes ssh" #t (and (memq 'ssh (method-names)) #t))
(check "env-provider-names includes direnv" #t (and (memq 'direnv (env-provider-names)) #t))

;; Cold-import inside disposable scope: built-ins must survive scope shutdown.
(let ((cold-script (string-append tmp "/cold-import.scm")))
  (write-text cold-script
    "(let ((sc (make-scope 'cold)))\n\
       (with-scope sc\n\
         (eval '(begin (import (techne location) (techne execution-site)) (method-names) (env-provider-names))))\n\
       (scope-shutdown! sc))\n\
     (import (techne location) (techne execution-site))\n\
     (display (and (memq 'ssh (method-names)) (memq 'direnv (env-provider-names)) 'ok))\n")
  (let ((out (call-with-process (techne-bin) (list "run" cold-script)
               (lambda (p)
                 (let loop ((xs '()))
                   (let ((c (process-read p 'stdout)))
                     (if (eof-object? c) (begin (process-wait p) (apply string-append (reverse xs)))
                         (loop (cons c xs))))))
               #:stdin 'null)))
    (check "cold-import built-in methods and providers survive scope shutdown" "ok" (string-trim out))))

;; execution-site-find-program rejects directories, non-executable files, and local paths on remote sites.
(let ((fp-dir (string-append tmp "/fp-proj")))
  (make-directory (string-append fp-dir "/subdir"))
  (write-text (string-append fp-dir "/nonexec") "#!/bin/sh\necho nope\n")
  (make-script (string-append fp-dir "/rel-exec") "#!/bin/sh\necho ok\n")
  (let ((site (make-execution-site fp-dir #:policy 'endpoint-base
                                   #:environment (list (cons "PATH" fp-dir)))))
    (check "find-program ignores directory on PATH" #f (execution-site-find-program site "subdir"))
    (check "find-program ignores non-executable file on PATH" #f (execution-site-find-program site "nonexec"))
    (check "find-program resolves relative slash path against site dir"
           (string-append fp-dir "/./rel-exec")
           (execution-site-find-program site "./rel-exec"))
    (check "find-program returns #f for remote site even with absolute local path" #f
           (execution-site-find-program (make-execution-site "/ssh:host:/tmp" #:policy 'endpoint-base)
                                        (string-append fp-dir "/rel-exec")))))

;; Relative direnv-program containing '/' is rejected cleanly.
(parameterize ((default-direnv-enabled #t) (default-direnv-program "./evil-direnv"))
  (execution-site-invalidate!)
  (let ((snap (execution-site-env-snapshot (make-execution-site stub-proj #:policy 'project))))
    (check "relative direnv-program fails closed with error" 'error (env-snapshot-status snap))))

;; Bad custom provider cannot crash execution-site-env-snapshot.
(let ((sc (make-scope 'bad-prov)))
  (with-scope sc
    (define-env-provider (exploding site refresh?) "Raises on purpose." (error "boom"))
    (define-env-provider (bogus site refresh?) "Returns wrong type." 12345))
  (parameterize ((default-env-providers '(exploding)))
    (let ((s (execution-site-env-snapshot (make-execution-site tmp #:policy 'project))))
      (check "raising custom provider becomes error snapshot" 'error (env-snapshot-status s))))
  (parameterize ((default-env-providers '(bogus)))
    (let ((s (execution-site-env-snapshot (make-execution-site tmp #:policy 'project))))
      (check "invalid return from custom provider becomes error snapshot" 'error (env-snapshot-status s))))
  (scope-shutdown! sc))

;; Zero-I/O mode-line cached snapshot returns #f on nonexistent path without raising.
(check "cached-snapshot on remote or unknown path returns #f" #f
       (execution-site-cached-snapshot "/ssh:host:/tmp/foo"))

;;; 7. Security & concurrency regression tests (child-scope worker, cold in-flight invalidation, allow TOCTOU)

;; 7a. Legitimate direnv evaluation triggered from a child scope spawns a root worker cleanly.
(parameterize ((default-direnv-enabled #t) (default-direnv-program stub-direnv))
  (write-text stub-mode-file "active")
  (let ((child (make-scope 'child)))
    (with-scope child
      (let ((snap (execution-site-env-snapshot (make-execution-site stub-proj #:policy 'project) #:refresh? #t)))
        (check "child-scope direnv evaluation spawns root worker cleanly" 'active
               (env-snapshot-status snap))))
    (scope-shutdown! child)))

;; 7b. Cold in-flight evaluation invalidated mid-flight must NEVER deliver stale active delta.
(let* ((race-proj (string-append tmp "/race-proj"))
       (race-rc (string-append race-proj "/.envrc"))
       (race-mode (string-append tmp "/race-mode"))
       (race-started (string-append tmp "/race-started"))
       (race-stub (string-append tmp "/race-direnv")))
  (make-directory race-proj)
  (write-text race-rc "export RACE=initial\n")
  (write-text race-mode "slow-stale")
  (make-script race-stub
    (string-append
     "#!/bin/sh\n"
     "mode=$(cat '" race-mode "' 2>/dev/null || echo blocked)\n"
     "case \"$1\" in\n"
     "  status)\n"
     "    case \"$mode\" in\n"
     "      blocked) printf '{\"state\":{\"foundRC\":{\"allowed\":1,\"path\":\"" race-rc "\"}}}' ;;\n"
     "      *) printf '{\"state\":{\"foundRC\":{\"allowed\":0,\"path\":\"" race-rc "\"}}}' ;;\n"
     "    esac ;;\n"
     "  export)\n"
     "    if [ \"$mode\" = \"slow-stale\" ]; then\n"
     "      echo started > '" race-started "'\n"
     "      sleep 1\n"
     "      printf '{\"RACE\":\"STALE_SECRET\"}'\n"
     "    else\n"
     "      printf '{\"RACE\":\"FRESH_OK\"}'\n"
     "    fi ;;\n"
     "  *) exit 0 ;;\n"
     "esac\n"))
  (parameterize ((default-direnv-enabled #t) (default-direnv-program race-stub))
    (execution-site-invalidate!)
    (let* ((site (make-execution-site race-proj #:policy 'project))
           (t1 (spawn (lambda () (execution-site-env-snapshot site))))
           (t2 (spawn (lambda ()
                        (let wait-start ((n 0))
                          (when (and (< n 100) (not (file-exists? race-started)))
                            (sleep 10)
                            (wait-start (+ n 1))))
                        ;; Cold cache has no entry yet; waiter joins in-flight slot then invalidates.
                        (execution-site-env-snapshot site)))))
      (let wait-start ((n 0))
        (when (and (< n 100) (not (file-exists? race-started)))
          (sleep 10)
          (wait-start (+ n 1))))
      ;; Switch mode to blocked and invalidate cold in-flight evaluation!
      (write-text race-mode "blocked")
      (execution-site-invalidate! race-proj)
      (let ((s1 (task-join t1))
            (s2 (task-join t2)))
        (check "cold invalidation waiter 1 never gets stale active delta" #f
               (and (eq? (env-snapshot-status s1) 'active)
                    (equal? (assoc "RACE" (env-snapshot-delta s1)) '("RACE" . "STALE_SECRET"))))
        (check "cold invalidation waiter 2 never gets stale active delta" #f
               (and (eq? (env-snapshot-status s2) 'active)
                    (equal? (assoc "RACE" (env-snapshot-delta s2)) '("RACE" . "STALE_SECRET"))))
        (check "cold invalidation retries to fresh blocked status" 'blocked (env-snapshot-status s1))))))

;; 7c. execution-site-direnv-allow! binds approval to verified bytes (never calls `direnv allow`),
;; rejects target swap / content change, and rolls back on mid-approval target swap.
(let* ((toctou-proj (string-append tmp "/toctou-proj"))
       (toctou-target-a (string-append tmp "/toctou-target-a"))
       (toctou-target-b (string-append tmp "/toctou-target-b"))
       (toctou-rc (string-append toctou-proj "/.envrc"))
       (toctou-data (string-append (file-truename tmp) "/toctou-data/direnv"))
       (allow-log (string-append tmp "/toctou-allow.log"))
       (swap-flag (string-append tmp "/toctou-swap"))
       (toctou-stub (string-append tmp "/toctou-direnv")))
  (make-directory toctou-proj)
  (write-text toctou-target-a "export SAFE=1\n")
  (write-text toctou-target-b "export EVIL=1\n")
  (call-with-process "ln" (list "-s" toctou-target-a toctou-rc) (lambda (p) (process-wait p)) #:stdin 'null)
  (make-script toctou-stub
    (string-append
     "#!/bin/sh\n"
     "case \"$1\" in\n"
     "  status)\n"
     "    if [ -f '" swap-flag "' ] && [ -d '" toctou-data "/allow' ]; then\n"
     "      ln -sf '" toctou-target-b "' '" toctou-rc "'\n"
     "    fi\n"
     "    printf '{\"state\":{\"foundRC\":{\"allowed\":0,\"path\":\"" toctou-rc "\"}}}' ;;\n"
     "  allow)  echo \"$2\" >> '" allow-log "' ;;\n"
     "  export) printf '{\"SAFE\":\"1\"}' ;;\n"
     "  *) exit 0 ;;\n"
     "esac\n"))
  (parameterize ((default-direnv-enabled #t)
                 (default-direnv-program toctou-stub)
                 (default-direnv-data-dir toctou-data))
    (let* ((canon-a (file-truename toctou-rc))
           (digest-a (execution-site-direnv-file-digest canon-a))
           (allow-hash-a (hex-encode (sha-256 (string->utf8 (string-append toctou-rc "\nexport SAFE=1\n")))))
           (allow-file-a (string-append toctou-data "/allow/" allow-hash-a)))
      (check "file digest returns 64-char sha256 hex" 64 (and (string? digest-a) (string-length digest-a)))
      ;; Content mutated after confirmation: refused without writing allow record or calling direnv allow.
      (write-text toctou-target-a "export TAMPERED=1\n")
      (check "allow refuses changed .envrc contents" 'refused
             (guard (e (#t 'refused))
               (execution-site-direnv-allow! toctou-proj #:expected-target canon-a #:expected-digest digest-a)
               'allowed))
      (check "direnv allow CLI was NOT invoked on content mismatch" #f (file-exists? allow-log))
      (check "no allow record written on content mismatch" #f (file-exists? allow-file-a))
      ;; Restore original content, then retarget symlink to another file with identical content.
      (write-text toctou-target-a "export SAFE=1\n")
      (write-text toctou-target-b "export SAFE=1\n")
      (call-with-process "ln" (list "-sf" toctou-target-b toctou-rc) (lambda (p) (process-wait p)) #:stdin 'null)
      (check "allow refuses retargeted .envrc symlink" 'refused
             (guard (e (#t 'refused))
               (execution-site-direnv-allow! toctou-proj #:expected-target canon-a #:expected-digest digest-a)
               'allowed))
      (check "direnv allow CLI was NOT invoked on target mismatch" #f (file-exists? allow-log))
      ;; Mid-approval symlink retarget (even with identical bytes) rolls back the written allow entry!
      (call-with-process "ln" (list "-sf" toctou-target-a toctou-rc) (lambda (p) (process-wait p)) #:stdin 'null)
      (write-text swap-flag "1")
      (check "mid-approval symlink retarget refused and rolled back" 'refused
             (guard (e (#t 'refused))
               (execution-site-direnv-allow! toctou-proj #:expected-target canon-a #:expected-digest digest-a)
               'allowed))
      (check "mid-approval rollback deleted written allow entry" #f (file-exists? allow-file-a))
      (delete-file swap-flag)
      ;; Matching symlink target & digest succeeds without ever invoking `direnv allow` CLI.
      (call-with-process "ln" (list "-sf" toctou-target-a toctou-rc) (lambda (p) (process-wait p)) #:stdin 'null)
      (let ((snap (execution-site-direnv-allow! toctou-proj #:expected-target canon-a #:expected-digest digest-a)))
        (check "allow succeeds when target and digest match" 'active (env-snapshot-status snap))
        (check "allow wrote exact digest-bound entry" #t (file-exists? allow-file-a))
        (check "direnv allow CLI is NEVER invoked" #f (file-exists? allow-log))))))

;; 7d. Incompatible direnv helper (ignoring allow/ store) triggers full rollback
;; (removes newly written allow entry and restores prior deny entry) and fails closed;
;; regular .envrc deny still passes the exact canonical .envrc file path.
(let* ((reg-proj (string-append tmp "/reg-allow-proj"))
       (reg-rc (string-append reg-proj "/.envrc"))
       (reg-data (string-append (file-truename tmp) "/reg-data/direnv"))
       (reg-log (string-append tmp "/reg-allow.log"))
       (incompat-flag (string-append tmp "/reg-incompat"))
       (reg-stub (string-append tmp "/reg-direnv")))
  (make-directory reg-proj)
  (write-text reg-rc "export REG=1\n")
  (make-script reg-stub
    (string-append
     "#!/bin/sh\n"
     "case \"$1\" in\n"
     "  status)\n"
     "    if [ -f '" incompat-flag "' ]; then\n"
     "      printf '{\"state\":{\"foundRC\":{\"allowed\":1,\"path\":\"" reg-rc "\"}}}'\n"
     "    else\n"
     "      printf '{\"state\":{\"foundRC\":{\"allowed\":0,\"path\":\"" reg-rc "\"}}}'\n"
     "    fi ;;\n"
     "  allow) echo \"allow:$2\" >> '" reg-log "' ;;\n"
     "  deny)  echo \"deny:$2\" >> '" reg-log "' ;;\n"
     "  export) printf '{\"REG\":\"1\"}' ;;\n"
     "  *) exit 0 ;;\n"
     "esac\n"))
  (parameterize ((default-direnv-enabled #t)
                 (default-direnv-program reg-stub)
                 (default-direnv-data-dir reg-data))
    (let* ((canon-rc (file-truename reg-rc))
           (digest-rc (execution-site-direnv-file-digest canon-rc))
           (allow-hash (hex-encode (sha-256 (string->utf8 (string-append reg-rc "\nexport REG=1\n")))))
           (deny-hash (hex-encode (sha-256 (string->utf8 (string-append reg-rc "\n")))))
           (allow-file (string-append reg-data "/allow/" allow-hash))
           (deny-file (string-append reg-data "/deny/" deny-hash)))
      ;; When a prior deny entry exists, allow refuses immediately without writing any allow entry
      ;; or removing the deny entry, and never invokes `direnv allow` CLI.
      (make-directory (string-append reg-data "/deny"))
      (write-text deny-file (string-append reg-rc "\n"))
      (let ((msg (guard (e (#t (if (error-object? e) (error-object-message e) "")))
                   (execution-site-direnv-allow! reg-proj #:expected-target canon-rc #:expected-digest digest-rc)
                   "unexpected-ok")))
        (check "existing deny refused before touching allow-dir" #t
               (and (string-contains msg "existing deny record") #t))
        (check "existing deny wrote no allow entry" #f (file-exists? allow-file))
        (check "existing deny left deny entry intact" #t (file-exists? deny-file))
        (check "existing deny never invoked direnv allow CLI" #f (file-exists? reg-log)))
      ;; After removing the deny entry, an incompatible direnv helper fails closed and rolls back allow.
      (delete-file deny-file)
      (write-text incompat-flag "1")
      (let ((msg (guard (e (#t (if (error-object? e) (error-object-message e) "")))
                   (execution-site-direnv-allow! reg-proj #:expected-target canon-rc #:expected-digest digest-rc)
                   "unexpected-ok")))
        (check "incompatible direnv helper fails closed with rolled back message" #t
               (and (string-contains msg "rolled back") #t))
        (check "incompatible helper rollback removed newly written allow entry" #f (file-exists? allow-file))
        (check "incompatible helper never invoked direnv allow CLI" #f (file-exists? reg-log)))
      ;; When compatible (and no deny entry), allow succeeds and never invokes direnv allow CLI.
      (delete-file incompat-flag)
      (let ((snap (execution-site-direnv-allow! reg-proj #:expected-target canon-rc #:expected-digest digest-rc)))
        (check "regular .envrc allow succeeds" 'active (env-snapshot-status snap))
        (check "regular .envrc allow wrote allow entry" #t (file-exists? allow-file))
        (check "regular .envrc allow never invoked direnv allow CLI" #f (file-exists? reg-log)))
      ;; 7d2. Fault-injection tests for post-mutation allow-dir fsync failures,
      ;; directory rename/replace races, rollback failures, and concurrent revocation.
      (let* ((fault-data (string-append (file-truename tmp) "/fault-data/direnv"))
             (fault-allow-dir (string-append fault-data "/allow"))
             (fault-allow-file (string-append fault-allow-dir "/" allow-hash))
             (deny-payload (string-append reg-rc "\n")))
        (parameterize ((default-direnv-data-dir fault-data))
          ;; Fault 1: allow-dir fsync fails AFTER renameat published fault-allow-file.
          (let ((saw-published-allow? #f))
            (dynamic-wind
              (lambda ()
                (set-direnv-sync-dir-hook!
                 (lambda (d handle)
                   (if (string=? d fault-allow-dir)
                       (begin
                         (set! saw-published-allow? (file-exists? fault-allow-file))
                         (error "injected allow-dir fsync failure"))
                       (pinned-dir-sync! handle)))))
              (lambda ()
                (let ((msg (guard (e (#t (if (error-object? e) (error-object-message e) "")))
                             (execution-site-direnv-allow! reg-proj #:expected-target canon-rc #:expected-digest digest-rc)
                             "unexpected-ok")))
                  (check "post-rename allow fsync fault saw allow published on disk" #t saw-published-allow?)
                  (check "post-rename allow fsync fault raises" #t
                         (and (string-contains msg "injected allow-dir fsync failure") #t))
                  (check "post-rename allow fsync rollback removes published allow record" #f
                         (file-exists? fault-allow-file))))
              (lambda () (set-direnv-sync-dir-hook! #f))))
          ;; Fault 3a: allow-dir is renamed and replaced before sync fails;
          ;; rollback must remove the published allow entry from the ORIGINAL inode,
          ;; not touch the decoy entry in the replacement directory.
          (let* ((moved-allow-dir (string-append fault-data "/allow-moved"))
                 (moved-allow-file (string-append moved-allow-dir "/" allow-hash))
                 (decoy-payload "decoy-allow\n"))
            (dynamic-wind
              (lambda ()
                (set-direnv-sync-dir-hook!
                 (lambda (d handle)
                   (if (string=? d fault-allow-dir)
                       (begin
                         (rename-file fault-allow-dir moved-allow-dir)
                         (make-directory fault-allow-dir)
                         (write-text fault-allow-file decoy-payload)
                         (error "injected allow-dir rename+sync failure"))
                       (pinned-dir-sync! handle)))))
              (lambda ()
                (let ((msg (guard (e (#t (if (error-object? e) (error-object-message e) "")))
                             (execution-site-direnv-allow! reg-proj #:expected-target canon-rc #:expected-digest digest-rc)
                             "unexpected-ok")))
                  (check "allow-dir rename+sync fault raises" #t
                         (and (string-contains msg "injected allow-dir rename+sync failure") #t))
                  (check "allow-dir rename rollback removes published allow from original inode" #f
                         (file-exists? moved-allow-file))
                  (check "allow-dir rename rollback leaves replacement decoy untouched" decoy-payload
                         (and (file-exists? fault-allow-file) (file->string fault-allow-file)))))
              (lambda () (set-direnv-sync-dir-hook! #f)))
            (delete-file fault-allow-file))
          ;; Fault 4a: failure DURING allow-dir rollback leaves allow record on disk;
          ;; error MUST report ROLLBACK FAILED (approval may remain active) and NOT "rolled back".
          (let* ((rb-data (string-append (file-truename tmp) "/rb-fault-data/direnv"))
                 (rb-allow-dir (string-append rb-data "/allow"))
                 (rb-allow-file (string-append rb-allow-dir "/" allow-hash)))
            (parameterize ((default-direnv-data-dir rb-data))
              (dynamic-wind
                (lambda ()
                  (set-direnv-sync-dir-hook!
                   (lambda (d handle)
                     (if (string=? d rb-allow-dir)
                         (begin
                           (pinned-dir-close! handle)
                           (error "injected allow-dir sync failure before rollback failure"))
                         (pinned-dir-sync! handle)))))
                (lambda ()
                  (let ((msg (guard (e (#t (if (error-object? e) (error-object-message e) "")))
                               (execution-site-direnv-allow! reg-proj #:expected-target canon-rc #:expected-digest digest-rc)
                               "unexpected-ok")))
                    (check "allow rollback fault preserves original error" #t
                           (and (string-contains msg "injected allow-dir sync failure before rollback failure") #t))
                    (check "allow rollback fault reports ROLLBACK FAILED" #t
                           (and (string-contains msg "ROLLBACK FAILED") #t))
                    (check "allow rollback fault warns approval may remain active" #t
                           (and (string-contains msg "approval may remain active") #t))
                    (check "allow rollback fault includes allow-dir failure detail" #t
                           (and (string-contains msg "allow-dir rollback failed") #t))
                    (check "allow rollback fault does NOT falsely claim rolled back" #f
                           (and (string-contains msg "rolled back") #t))
                    (check "allow rollback fault left published allow record on disk" #t
                           (file-exists? rb-allow-file))))
                (lambda () (set-direnv-sync-dir-hook! #f)))))
          ;; Fault 5a/5b/5c: Concurrent and pre-existing revocation during execution-site-direnv-allow!
          ;; Uses a store-backed mock direnv where deny/ overrides allow/ just like real direnv,
          ;; and verifies both on-disk deny persistence AND final evaluated status = 'denied.
          (let* ((cr-data (string-append (file-truename tmp) "/cr-data/direnv"))
                 (cr-allow-dir (string-append cr-data "/allow"))
                 (cr-deny-dir (string-append cr-data "/deny"))
                 (cr-allow-file (string-append cr-allow-dir "/" allow-hash))
                 (cr-deny-file (string-append cr-deny-dir "/" deny-hash))
                 (cr-stub (string-append tmp "/cr-direnv")))
            (make-directory cr-allow-dir)
            (make-script cr-stub
              (string-append
               "#!/bin/sh\n"
               "case \"$1\" in\n"
               "  status)\n"
               "    if [ -f '" cr-deny-file "' ]; then\n"
               "      printf '{\"state\":{\"foundRC\":{\"allowed\":2,\"path\":\"" reg-rc "\"}}}'\n"
               "    elif [ -f '" cr-allow-file "' ]; then\n"
               "      printf '{\"state\":{\"foundRC\":{\"allowed\":0,\"path\":\"" reg-rc "\"}}}'\n"
               "    else\n"
               "      printf '{\"state\":{\"foundRC\":{\"allowed\":1,\"path\":\"" reg-rc "\"}}}'\n"
               "    fi ;;\n"
               "  export)\n"
               "    if [ -f '" cr-deny-file "' ] || [ ! -f '" cr-allow-file "' ]; then exit 1; fi\n"
               "    printf '{\"CR_LEAK\":\"active\"}' ;;\n"
               "  *) exit 0 ;;\n"
               "esac\n"))
            (parameterize ((default-direnv-program cr-stub)
                           (default-direnv-data-dir cr-data))
              ;; Fault 5a: No prior allow and no deny/ directory at snapshot; concurrent client
              ;; creates deny/ and deny/<hash> during allow-dir sync. Must detect the concurrent
              ;; deny, roll back the newly written allow record, preserve deny, and evaluate 'denied.
              (dynamic-wind
                (lambda ()
                  (set-direnv-sync-dir-hook!
                   (lambda (d handle)
                     (pinned-dir-sync! handle)
                     (when (string=? d cr-allow-dir)
                       (make-directory cr-deny-dir)
                       (write-text cr-deny-file deny-payload)))))
                (lambda ()
                  (let ((msg (guard (e (#t (if (error-object? e) (error-object-message e) "")))
                               (execution-site-direnv-allow! reg-proj #:expected-target canon-rc #:expected-digest digest-rc)
                               "unexpected-ok")))
                    (check "concurrent deny in new deny/ fails approval" #t
                           (and (string-contains msg "concurrent deny record") #t))
                    (check "concurrent deny in new deny/ rolled back newly written allow" #f
                           (file-exists? cr-allow-file))
                    (check "concurrent deny in new deny/ is preserved on disk" deny-payload
                           (and (file-exists? cr-deny-file) (file->string cr-deny-file)))
                    (let ((final-snap (execution-site-env-snapshot reg-proj #:refresh? #t)))
                      (check "final status after concurrent deny in new deny/ is denied" 'denied
                             (env-snapshot-status final-snap))
                      (check "final delta after concurrent deny in new deny/ is empty" '()
                             (env-snapshot-delta final-snap)))))
                (lambda () (set-direnv-sync-dir-hook! #f)))
              ;; Fault 5b: Pre-existing matching allow record, existing deny/ dir with NO deny at snapshot;
              ;; concurrent client creates deny record during allow-dir sync.
              ;; Must preserve the newly observed deny (never unlink it) and leave final state 'denied.
              (delete-file cr-deny-file)
              (write-text cr-allow-file deny-payload)
              (dynamic-wind
                (lambda ()
                  (set-direnv-sync-dir-hook!
                   (lambda (d handle)
                     (pinned-dir-sync! handle)
                     (when (string=? d cr-allow-dir)
                       (write-text cr-deny-file deny-payload)))))
                (lambda ()
                  (let ((msg (guard (e (#t (if (error-object? e) (error-object-message e) "")))
                               (execution-site-direnv-allow! reg-proj #:expected-target canon-rc #:expected-digest digest-rc)
                               "unexpected-ok")))
                    (check "concurrent new deny causes approval to fail" #t
                           (and (string-contains msg "concurrent deny record") #t))
                    (check "concurrent new deny record is preserved on disk" deny-payload
                           (and (file-exists? cr-deny-file) (file->string cr-deny-file)))
                    (let ((final-snap (execution-site-env-snapshot reg-proj #:refresh? #t)))
                      (check "final direnv status after concurrent new deny is denied" 'denied
                             (env-snapshot-status final-snap))
                      (check "final direnv delta after concurrent new deny is empty" '()
                             (env-snapshot-delta final-snap)))))
                (lambda () (set-direnv-sync-dir-hook! #f)))
              ;; Fault 5c: Pre-existing deny record at snapshot; must refuse immediately
              ;; before writing any allow record or running sync hook, leaving deny intact and 'denied.
              (delete-file cr-allow-file)
              (let ((sync-called? #f))
                (dynamic-wind
                  (lambda ()
                    (set-direnv-sync-dir-hook!
                     (lambda (d handle)
                       (set! sync-called? #t)
                       (pinned-dir-sync! handle))))
                  (lambda ()
                    (let ((msg (guard (e (#t (if (error-object? e) (error-object-message e) "")))
                                 (execution-site-direnv-allow! reg-proj #:expected-target canon-rc #:expected-digest digest-rc)
                                 "unexpected-ok")))
                      (check "pre-existing deny fails immediately" #t
                             (and (string-contains msg "existing deny record") #t))
                      (check "pre-existing deny never invoked sync hook" #f sync-called?)
                      (check "pre-existing deny wrote no allow record" #f (file-exists? cr-allow-file))
                      (check "pre-existing deny stays intact on disk" deny-payload
                             (and (file-exists? cr-deny-file) (file->string cr-deny-file)))
                      (let ((final-snap (execution-site-env-snapshot reg-proj #:refresh? #t)))
                        (check "final direnv status with pre-existing deny is denied" 'denied
                               (env-snapshot-status final-snap))
                        (check "final direnv delta with pre-existing deny is empty" '()
                               (env-snapshot-delta final-snap)))))
                  (lambda () (set-direnv-sync-dir-hook! #f)))))
          ;; Fault 6: Two concurrent Techne callers approving the same .envrc.
          ;; Caller A publishes allow-file, holds the lock through a slow failing status check
          ;; and rollback; Caller B starts as soon as A publishes and must wait for A's transaction
          ;; to finish so A's rollback can never delete B's successful approval.
          (let* ((tc-data (string-append (file-truename tmp) "/tc-data/direnv"))
                 (tc-allow-dir (string-append tc-data "/allow"))
                 (tc-allow-file (string-append tc-allow-dir "/" allow-hash))
                 (tc-stub-a (string-append tmp "/tc-direnv-a"))
                 (tc-stub-b (string-append tmp "/tc-direnv-b")))
            (make-directory tc-allow-dir)
            ;; Stub A: during verification (once allow-file exists), sleeps 150ms so Caller B
            ;; is guaranteed to contend on the lock, then rejects approval (allowed=1) to force rollback.
            (make-script tc-stub-a
              (string-append
               "#!/bin/sh\n"
               "case \"$1\" in\n"
               "  status)\n"
               "    if [ -f '" tc-allow-file "' ]; then sleep 0.15; fi\n"
               "    printf '{\"state\":{\"foundRC\":{\"allowed\":1,\"path\":\"" reg-rc "\"}}}' ;;\n"
               "  *) exit 1 ;;\n"
               "esac\n"))
            ;; Stub B: normal store-backed direnv stub where allow-file grants allowed=0 and TC_OK=yes.
            (make-script tc-stub-b
              (string-append
               "#!/bin/sh\n"
               "case \"$1\" in\n"
               "  status)\n"
               "    if [ -f '" tc-allow-file "' ]; then\n"
               "      printf '{\"state\":{\"foundRC\":{\"allowed\":0,\"path\":\"" reg-rc "\"}}}'\n"
               "    else\n"
               "      printf '{\"state\":{\"foundRC\":{\"allowed\":1,\"path\":\"" reg-rc "\"}}}'\n"
               "    fi ;;\n"
               "  export)\n"
               "    if [ ! -f '" tc-allow-file "' ]; then exit 1; fi\n"
               "    printf '{\"TC_OK\":\"yes\"}' ;;\n"
               "  *) exit 0 ;;\n"
               "esac\n"))
            (let ((a-published (make-channel 1))
                  (signaled? #f))
              (dynamic-wind
                (lambda ()
                  (set-direnv-sync-dir-hook!
                   (lambda (d handle)
                     (pinned-dir-sync! handle)
                     (when (and (string=? d tc-allow-dir) (not signaled?))
                       (set! signaled? #t)
                       (channel-send a-published #t)))))
                (lambda ()
                  (let* ((task-a
                          (spawn
                           (lambda ()
                             (dynamic-wind
                               (lambda () #f)
                               (lambda ()
                                 (parameterize ((default-direnv-program tc-stub-a)
                                                (default-direnv-data-dir tc-data))
                                   (guard (e (#t (if (error-object? e) (error-object-message e) "err")))
                                     (execution-site-direnv-allow! reg-proj #:expected-target canon-rc #:expected-digest digest-rc)
                                     "unexpected-ok")))
                               (lambda ()
                                 (unless signaled?
                                   (set! signaled? #t)
                                   (channel-send a-published 'failed)))))))
                         (pub-status (channel-recv a-published)))
                    (check "two-caller: caller A reached publish point" #t (eq? pub-status #t))
                    (if (eq? pub-status #t)
                        (let* ((task-b
                                (spawn
                                 (lambda ()
                                   (parameterize ((default-direnv-program tc-stub-b)
                                                  (default-direnv-data-dir tc-data))
                                     (execution-site-direnv-allow! reg-proj #:expected-target canon-rc #:expected-digest digest-rc)))))
                               (res-a (task-join task-a))
                               (res-b (task-join task-b)))
                          (check "two-caller: failing caller A rolled back cleanly" #t
                                 (and (string? res-a) (string-contains res-a "rolled back") #t))
                          (check "two-caller: succeeding caller B returned active snapshot" 'active
                                 (env-snapshot-status res-b))
                          (check "two-caller: allow record remains on disk after A failed and B succeeded" #t
                                 (file-exists? tc-allow-file))
                          (parameterize ((default-direnv-program tc-stub-b)
                                         (default-direnv-data-dir tc-data))
                            (let ((final-snap (execution-site-env-snapshot reg-proj #:refresh? #t)))
                              (check "two-caller: final status remains active" 'active
                                     (env-snapshot-status final-snap))
                              (check "two-caller: final delta preserves B approval" '(("TC_OK" . "yes"))
                                     (env-snapshot-delta final-snap)))))
                        (task-join task-a))))
                (lambda () (set-direnv-sync-dir-hook! #f)))
              ;; Reverse ordering: B already approved; subsequent failing caller A must not delete allow-file.
              (parameterize ((default-direnv-program tc-stub-a)
                             (default-direnv-data-dir tc-data))
                (let ((msg (guard (e (#t (if (error-object? e) (error-object-message e) "")))
                             (execution-site-direnv-allow! reg-proj #:expected-target canon-rc #:expected-digest digest-rc)
                             "unexpected-ok")))
                  (check "two-caller reverse: failing caller A raises" #f (string=? msg "unexpected-ok"))
                  (check "two-caller reverse: allow record remains on disk" #t (file-exists? tc-allow-file))))
              (parameterize ((default-direnv-program tc-stub-b)
                             (default-direnv-data-dir tc-data))
                (let ((final-snap (execution-site-env-snapshot reg-proj #:refresh? #t)))
                  (check "two-caller reverse: final status remains active" 'active
                         (env-snapshot-status final-snap))
                  (check "two-caller reverse: final delta still active" '(("TC_OK" . "yes"))
                         (env-snapshot-delta final-snap)))))))))
      ;; Deny still passes the exact canonical .envrc file path to `direnv deny`.
      (execution-site-direnv-deny! reg-proj)
      (check "regular .envrc passes exact canonical file path to direnv deny"
             (string-append "deny:" canon-rc)
             (string-trim (file->string reg-log))))))

;; 7e. Uncleaned startup DIRENV_DIR blocks direnv helpers AND local program lookup/launches.
(when (techne-bin)
  (let* ((dirty-proj (string-append tmp "/unclean-dirty-proj"))
         (target-proj (string-append tmp "/unclean-target-proj"))
         (evil-bin (string-append tmp "/unclean-evil-bin"))
         (evil-direnv (string-append evil-bin "/direnv"))
         (evil-lsp (string-append evil-bin "/evil-lsp"))
         (ran-marker (string-append tmp "/unclean-evil-direnv-ran"))
         (lsp-ran-marker (string-append tmp "/unclean-evil-lsp-ran"))
         (probe-script (string-append tmp "/probe-unclean-direnv.scm")))
    (make-directory dirty-proj)
    (make-directory target-proj)
    (make-directory evil-bin)
    (write-text (string-append target-proj "/.envrc") "export SECRET=1\n")
    (make-script evil-direnv
      (string-append
       "#!/bin/sh\n"
       "echo \"$@\" >> '" ran-marker "'\n"
       "printf '{\"state\":{\"foundRC\":{\"allowed\":0,\"path\":\"" target-proj "/.envrc\"}}}'\n"))
    (make-script evil-lsp
      (string-append
       "#!/bin/sh\n"
       "echo \"$@\" >> '" lsp-ran-marker "'\n"))
    (write-text probe-script
      (string-append
       "(import (techne) (techne execution-site))\n"
       "(define (fails? th) (guard (e (#t #t)) (th) #f))\n"
       "(parameterize ((default-direnv-enabled #t) (default-direnv-program \"direnv\"))\n"
       "  (let* ((site (make-execution-site \"" target-proj "\" #:policy 'project))\n"
       "         (snap (execution-site-env-snapshot site)))\n"
       "    (write (list (env-snapshot-status snap)\n"
       "                 (fails? (lambda () (execution-site-direnv-target \"" target-proj "\")))\n"
       "                 (fails? (lambda () (execution-site-direnv-allow! \"" target-proj "\")))\n"
       "                 (fails? (lambda () (execution-site-direnv-deny! \"" target-proj "\")))\n"
       "                 (execution-site-find-program site \"evil-lsp\")\n"
       "                 (execution-site-find-program site \"" evil-lsp "\")\n"
       "                 (fails? (lambda () (execution-site-resolved-environment site)))\n"
       "                 (fails? (lambda () (call-with-process-at-site site \"evil-lsp\" '() process-wait)))))))\n"))
    (let ((out (call-with-process (techne-bin) (list "run" probe-script)
                 (lambda (p)
                   (let loop ((xs '()))
                     (let ((c (process-read p 'stdout)))
                       (if (eof-object? c)
                           (begin (process-wait p) (apply string-append (reverse xs)))
                           (loop (cons c xs))))))
                 #:environment (list (cons "DIRENV_DIR" (string-append "-" dirty-proj))
                                     (cons "DIRENV_DIFF" "bogus")
                                     (cons "PATH" (string-append evil-bin ":/nonexistent-only")))
                 #:stdin 'null)))
      (check "failed cleanup reports direnv unavailable and blocks target/allow/deny + local lookup/launch"
             "(unavailable #t #t #t #f #f #t #t)" (string-trim out))
      (check "failed cleanup causes ZERO direnv helper invocations from inherited PATH"
             #f (file-exists? ran-marker))
      (check "failed cleanup causes ZERO local process launches from inherited PATH"
             #f (file-exists? lsp-ran-marker)))))

;; 7f. Concurrent evaluation of two sites whose `.envrc` entries symlink to the same
;;     target file (only one approved) must NOT share an in-flight or cache key.
(let* ((canon-tmp (file-truename tmp))
       (shared-target (string-append canon-tmp "/shared-rc-target"))
       (proj-ok (string-append canon-tmp "/shared-proj-ok"))
       (proj-blocked (string-append canon-tmp "/shared-proj-blocked"))
       (ok-started (string-append canon-tmp "/shared-ok-started"))
       (shared-stub (string-append canon-tmp "/shared-direnv")))
  (write-text shared-target "export SHARED_SECRET=from-allowed\n")
  (make-directory proj-ok)
  (make-directory proj-blocked)
  (call-with-process "ln" (list "-s" shared-target (string-append proj-ok "/.envrc"))
    (lambda (p) (process-wait p)) #:stdin 'null)
  (call-with-process "ln" (list "-s" shared-target (string-append proj-blocked "/.envrc"))
    (lambda (p) (process-wait p)) #:stdin 'null)
  (make-script shared-stub
    (string-append
     "#!/bin/sh\n"
     "here=\"$(pwd -P)\"\n"
     "case \"$1\" in\n"
     "  status)\n"
     "    if [ \"$here\" = '" proj-ok "' ]; then\n"
     "      printf '{\"state\":{\"foundRC\":{\"allowed\":0,\"path\":\"" proj-ok "/.envrc\"}}}'\n"
     "    else\n"
     "      printf '{\"state\":{\"foundRC\":{\"allowed\":1,\"path\":\"" proj-blocked "/.envrc\"}}}'\n"
     "    fi ;;\n"
     "  export)\n"
     "    if [ \"$here\" = '" proj-ok "' ]; then\n"
     "      echo started > '" ok-started "'\n"
     "      sleep 0.25\n"
     "      printf '{\"SHARED_SECRET\":\"from-allowed\"}'\n"
     "    else\n"
     "      exit 99\n"
     "    fi ;;\n"
     "  *) exit 0 ;;\n"
     "esac\n"))
  (parameterize ((default-direnv-enabled #t)
                 (default-direnv-program shared-stub))
    (execution-site-invalidate!)
    (let* ((ok-ch (make-channel 1))
           (ok-task (spawn (lambda ()
                             (channel-send ok-ch (execution-site-env-snapshot proj-ok))))))
      ;; Wait until proj-ok's direnv export is actively in-flight.
      (let wait-started ((n 0))
        (when (and (< n 100) (not (file-exists? ok-started)))
          (sleep 10)
          (wait-started (+ n 1))))
      (check "shared-target approved worker is actively in-flight" #t (file-exists? ok-started))
      ;; Concurrently evaluate the blocked site whose .envrc symlinks to the same file.
      (let* ((blocked-snap (execution-site-env-snapshot proj-blocked))
             (blocked-env (execution-site-resolved-environment proj-blocked))
             (ok-snap (channel-recv ok-ch)))
        (task-join ok-task)
        (check "concurrent blocked symlink site returns blocked status" 'blocked
               (env-snapshot-status blocked-snap))
        (check "concurrent blocked symlink site receives empty delta" '()
               (env-snapshot-delta blocked-snap))
        (check "concurrent blocked symlink site launch env does not inherit approved delta" #f
               (assoc "SHARED_SECRET" blocked-env))
        (check "approved symlink site returns active status" 'active
               (env-snapshot-status ok-snap))
        (check "approved symlink site receives its own delta"
               '(("SHARED_SECRET" . "from-allowed"))
               (env-snapshot-delta ok-snap))
        ;; Even after proj-ok populates the cache, proj-blocked stays blocked with empty delta.
        (let ((post-blocked (execution-site-env-snapshot proj-blocked)))
          (check "post-cache blocked symlink site stays blocked" 'blocked
                 (env-snapshot-status post-blocked))
          (check "post-cache blocked symlink site has empty delta" '()
                 (env-snapshot-delta post-blocked)))))))

;; 7g. default-direnv-data-dir synchronizes XDG_DATA_HOME with direnv subprocesses
;;     and rejects overrides that do not end in `/direnv`.
(let* ((canon-tmp (file-truename tmp))
       (xdg-parent (string-append canon-tmp "/custom-xdg"))
       (xdg-data (string-append xdg-parent "/direnv"))
       (bad-data (string-append canon-tmp "/bad-custom-store"))
       (xdg-proj (string-append canon-tmp "/xdg-sync-proj"))
       (xdg-rc (string-append xdg-proj "/.envrc"))
       (xdg-seen (string-append canon-tmp "/xdg-seen.txt"))
       (xdg-stub (string-append canon-tmp "/xdg-sync-direnv")))
  (make-directory xdg-proj)
  (write-text xdg-rc "export XDG_TEST=1\n")
  (make-script xdg-stub
    (string-append
     "#!/bin/sh\n"
     "printf '%s' \"$XDG_DATA_HOME\" > '" xdg-seen "'\n"
     "case \"$1\" in\n"
     "  status) printf '{\"state\":{\"foundRC\":{\"allowed\":0,\"path\":\"" xdg-rc "\"}}}' ;;\n"
     "  export) printf '{\"XDG_SYNCED\":\"yes\"}' ;;\n"
     "  *) exit 0 ;;\n"
     "esac\n"))
  (parameterize ((default-direnv-enabled #t)
                 (default-direnv-program xdg-stub)
                 (default-direnv-data-dir bad-data))
    (execution-site-invalidate!)
    (let ((s-bad (execution-site-env-snapshot xdg-proj)))
      (check "non-/direnv data-dir override returns error snapshot" 'error (env-snapshot-status s-bad))
      (check "non-/direnv data-dir override does not invoke helper" #f (file-exists? xdg-seen)))
    (check "non-/direnv data-dir override fails execution-site-direnv-target" 'err
           (guard (e (#t 'err)) (execution-site-direnv-target xdg-proj) 'ok))
    (check "non-/direnv data-dir override fails execution-site-direnv-allow!" 'err
           (guard (e (#t 'err)) (execution-site-direnv-allow! xdg-proj) 'ok))
    (check "non-/direnv data-dir override fails execution-site-direnv-deny!" 'err
           (guard (e (#t 'err)) (execution-site-direnv-deny! xdg-proj) 'ok)))
  (parameterize ((default-direnv-enabled #t)
                 (default-direnv-program xdg-stub)
                 (default-direnv-data-dir xdg-data))
    (execution-site-invalidate!)
    (let ((s-ok (execution-site-env-snapshot xdg-proj)))
      (check "valid /direnv override activates" 'active (env-snapshot-status s-ok))
      (check "valid /direnv override passes matching XDG_DATA_HOME to direnv" xdg-parent
             (and (file-exists? xdg-seen) (file->string xdg-seen))))))

;; 7h. DIRENV_WATCHES recorded exists/modtime mismatch during export triggers retry
;;     instead of caching a stale snapshot.
(let* ((canon-tmp (file-truename tmp))
       (wm-proj (string-append canon-tmp "/watch-modtime-proj"))
       (wm-rc (string-append wm-proj "/.envrc"))
       (wm-cfg (string-append wm-proj "/cfg.txt"))
       (wm-count (string-append canon-tmp "/wm-count"))
       (wm-stub (string-append canon-tmp "/wm-direnv")))
  (make-directory wm-proj)
  (write-text wm-rc "export WM=1\n")
  (write-text wm-cfg "current\n")
  (write-text wm-count "0\n")
  (let ((cfg-mtime (list-ref (file-attributes wm-cfg) 2)))
    (make-script wm-stub
      (string-append
       "#!/bin/sh\n"
       "case \"$1\" in\n"
       "  status) printf '{\"state\":{\"foundRC\":{\"allowed\":0,\"path\":\"" wm-rc "\"}}}' ;;\n"
       "  export)\n"
       "    c=$(cat '" wm-count "'); n=$((c + 1)); echo \"$n\" > '" wm-count "'\n"
       "    if [ \"$n\" -eq 1 ]; then\n"
       "      printf '{\"WM_VAL\":\"stale\",\"DIRENV_WATCHES\":\"w-stale\"}'\n"
       "    else\n"
       "      printf '{\"WM_VAL\":\"fresh\",\"DIRENV_WATCHES\":\"w-fresh\"}'\n"
       "    fi ;;\n"
       "  show_dump)\n"
       "    if [ \"$2\" = \"w-stale\" ]; then\n"
       "      printf '[{\"path\":\"" wm-cfg "\",\"exists\":true,\"modtime\":" (number->string (- cfg-mtime 10)) "}]'\n"
       "    else\n"
       "      printf '[{\"path\":\"" wm-cfg "\",\"exists\":true,\"modtime\":" (number->string cfg-mtime) "}]'\n"
       "    fi ;;\n"
       "  *) exit 0 ;;\n"
       "esac\n"))
    (parameterize ((default-direnv-enabled #t)
                   (default-direnv-program wm-stub))
      (execution-site-invalidate!)
      (let ((snap (execution-site-env-snapshot wm-proj)))
        (check "stale DIRENV_WATCHES modtime retried to active" 'active (env-snapshot-status snap))
        (check "stale DIRENV_WATCHES modtime delivered fresh second export"
               "fresh" (cdr (assoc "WM_VAL" (env-snapshot-delta snap))))
        (check "stale DIRENV_WATCHES triggered second export" "2"
               (string-trim (file->string wm-count))))))
  ;; Watched symlink: link lstat mtime is older than target mtime (direnv records max(link, target)).
  ;; When target mtime advances during export while link lstat mtime stays unchanged, must detect
  ;; the mismatch against max(lstat, target) and retry instead of caching the stale export.
  (let* ((sym-target (string-append wm-proj "/sym-target.cfg"))
         (sym-link (string-append wm-proj "/sym-link.cfg"))
         (swm-count (string-append canon-tmp "/swm-count"))
         (swm-stub (string-append canon-tmp "/swm-direnv")))
    (write-text sym-target "v1\n")
    (call-with-process "ln" (list "-s" sym-target sym-link) (lambda (p) (process-wait p)) #:stdin 'null)
    (call-with-process "touch" (list "-h" "-d" "2025-01-01 00:00:00 UTC" sym-link)
      (lambda (p) (process-wait p)) #:stdin 'null)
    (write-text swm-count "0\n")
    (let* ((link-mtime (list-ref (file-attributes sym-link) 2))
           (target-mtime (list-ref (file-attributes sym-target) 2)))
      (make-script swm-stub
        (string-append
         "#!/bin/sh\n"
         "case \"$1\" in\n"
         "  status) printf '{\"state\":{\"foundRC\":{\"allowed\":0,\"path\":\"" wm-rc "\"}}}' ;;\n"
         "  export)\n"
         "    c=$(cat '" swm-count "'); n=$((c + 1)); echo \"$n\" > '" swm-count "'\n"
         "    if [ \"$n\" -eq 1 ]; then\n"
         "      printf '{\"SWM_VAL\":\"stale-link\",\"DIRENV_WATCHES\":\"sw-stale\"}'\n"
         "    else\n"
         "      printf '{\"SWM_VAL\":\"fresh-target\",\"DIRENV_WATCHES\":\"sw-fresh\"}'\n"
         "    fi ;;\n"
         "  show_dump)\n"
         "    if [ \"$2\" = \"sw-stale\" ]; then\n"
         "      printf '[{\"path\":\"" sym-link "\",\"exists\":true,\"modtime\":" (number->string link-mtime) "}]'\n"
         "    else\n"
         "      printf '[{\"path\":\"" sym-link "\",\"exists\":true,\"modtime\":" (number->string target-mtime) "}]'\n"
         "    fi ;;\n"
         "  *) exit 0 ;;\n"
         "esac\n"))
      (parameterize ((default-direnv-enabled #t)
                     (default-direnv-program swm-stub))
        (execution-site-invalidate!)
        (let ((snap (execution-site-env-snapshot wm-proj)))
          (check "watched symlink target mtime mismatch retried to active" 'active (env-snapshot-status snap))
          (check "watched symlink delivered fresh second export"
                 "fresh-target" (cdr (assoc "SWM_VAL" (env-snapshot-delta snap))))
          (check "watched symlink triggered second export" "2"
                 (string-trim (file->string swm-count))))))))

;; 7i. Subdirectory with its own direnv-selected config (e.g. load_dotenv `.env`)
;;     does not reuse an ancestor directory's cached snapshot.
(let* ((canon-tmp (file-truename tmp))
       (parent-dir (string-append canon-tmp "/dotenv-parent"))
       (child-dir (string-append parent-dir "/sub"))
       (parent-rc (string-append parent-dir "/.envrc"))
       (child-env (string-append child-dir "/.env"))
       (dotenv-stub (string-append canon-tmp "/dotenv-direnv")))
  (make-directory child-dir)
  (write-text parent-rc "export WHICH=parent\n")
  (write-text child-env "WHICH=child\n")
  (make-script dotenv-stub
    (string-append
     "#!/bin/sh\n"
     "here=\"$(pwd -P)\"\n"
     "case \"$1\" in\n"
     "  status)\n"
     "    if [ \"$here\" = '" child-dir "' ]; then\n"
     "      printf '{\"state\":{\"foundRC\":{\"allowed\":1,\"path\":\"" child-env "\"}}}'\n"
     "    else\n"
     "      printf '{\"state\":{\"foundRC\":{\"allowed\":0,\"path\":\"" parent-rc "\"}}}'\n"
     "    fi ;;\n"
     "  export)\n"
     "    if [ \"$here\" = '" parent-dir "' ]; then\n"
     "      printf '{\"WHICH\":\"parent\"}'\n"
     "    else\n"
     "      exit 99\n"
     "    fi ;;\n"
     "  *) exit 0 ;;\n"
     "esac\n"))
  (parameterize ((default-direnv-enabled #t)
                 (default-direnv-program dotenv-stub))
    (execution-site-invalidate!)
    (let ((p-snap (execution-site-env-snapshot parent-dir)))
      (check "parent dir evaluates active" 'active (env-snapshot-status p-snap))
      (check "child dir does not inherit parent cached snapshot before eval" #f
             (execution-site-cached-snapshot child-dir))
      (check "child dir still needs eval after parent is cached" #t
             (execution-site-needs-eval? child-dir))
      (let ((c-snap (execution-site-env-snapshot child-dir)))
        (check "child dir evaluates its own blocked .env instead of reusing parent" 'blocked
               (env-snapshot-status c-snap))
        (check "child dir has empty delta" '() (env-snapshot-delta c-snap))))))

;; 7j. Transient error snapshots are NOT cached as fresh, so subsequent calls retry.
(let* ((canon-tmp (file-truename tmp))
       (te-proj (string-append canon-tmp "/transient-err-proj"))
       (te-rc (string-append te-proj "/.envrc"))
       (te-count (string-append canon-tmp "/te-count"))
       (te-stub (string-append canon-tmp "/te-direnv")))
  (make-directory te-proj)
  (write-text te-rc "export TE=ok\n")
  (write-text te-count "0\n")
  (make-script te-stub
    (string-append
     "#!/bin/sh\n"
     "case \"$1\" in\n"
     "  status) printf '{\"state\":{\"foundRC\":{\"allowed\":0,\"path\":\"" te-rc "\"}}}' ;;\n"
     "  export)\n"
     "    c=$(cat '" te-count "'); n=$((c + 1)); echo \"$n\" > '" te-count "'\n"
     "    if [ \"$n\" -eq 1 ]; then exit 1; fi\n"
     "    printf '{\"TE\":\"recovered\"}' ;;\n"
     "  *) exit 0 ;;\n"
     "esac\n"))
  (parameterize ((default-direnv-enabled #t)
                 (default-direnv-program te-stub))
    (execution-site-invalidate!)
    (let ((s1 (execution-site-env-snapshot te-proj)))
      (check "first transient failure returns error snapshot" 'error (env-snapshot-status s1))
      (check "transient error snapshot is not cached" #f (execution-site-cached-snapshot te-proj))
      (check "transient error still needs eval" #t (execution-site-needs-eval? te-proj)))
    (let ((s2 (execution-site-env-snapshot te-proj)))
      (check "second call retries without #:refresh? and succeeds" 'active (env-snapshot-status s2))
      (check "second call receives recovered delta" '(("TE" . "recovered")) (env-snapshot-delta s2)))))

(rm-rf tmp)
(test-summary)
