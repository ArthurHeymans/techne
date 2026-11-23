;;; Tests for `(techne secrets)`: opaque secret non-leakage, scope-owned
;;; backend registry, `secret-ref`, `authinfo`, `sops`, `pass`, and `env`.
(import (techne) (techne secrets) (techne trusted-process) (techne task) (techne uuid))
(require "../../test.scm")

(define tmp (string-append (or (get-environment-variable "TMPDIR") "/tmp") "/techne-secrets-" (uuid-v4)))
(create-directory tmp #o700)

(define (write-text path text)
  (call-with-output-file path (lambda (p) (display text p))))

(define (make-script path body)
  (write-text path body)
  (call-with-process "chmod" (list "+x" path) (lambda (p) (process-wait p))))

(define (rm-rf path)
  (call-with-process "rm" (list "-rf" "--" path) (lambda (p) (process-wait p))))

;;; 1. Opaque secret invariants (no leakage through printing, equality, inspection or JSON)
(let* ((raw "SUPER-SECRET-TOKEN-999")
       (s1 (make-secret raw #:label "my-sensitive-label"))
       (s2 (make-secret raw #:label "my-sensitive-label"))
       (printed-repr (repr s1))
       (printed-write (call-with-output-string (lambda (p) (write s1 p))))
       (printed-display (call-with-output-string (lambda (p) (display s1 p))))
       (fields-repr (repr (record-fields s1)))
       (json-err (guard (e (#t (error-object-message e)))
                   (json->string s1)
                   #f)))
  (check "secret? predicate true" #t (secret? s1))
  (check "secret? predicate false on string" #f (secret? raw))
  (check "secret-reveal returns raw" raw (secret-reveal s1))
  (check "secret-label returns label" "my-sensitive-label" (secret-label s1))
  (check "call-with-secret passes raw" "len:22"
         (call-with-secret s1 (lambda (v) (string-append "len:" (number->string (string-length v))))))
  (check "repr does not contain plaintext" #f (and (string-contains printed-repr raw) #t))
  (check "repr does not contain label" #f (and (string-contains printed-repr "my-sensitive-label") #t))
  (check "write does not contain plaintext" #f (and (string-contains printed-write raw) #t))
  (check "display does not contain plaintext" #f (and (string-contains printed-display raw) #t))
  (check "record-fields does not contain plaintext" #f (and (string-contains fields-repr raw) #t))
  (check "calling extracted closure without seal raises" #t
         (guard (e (#t #t))
           ((cdar (record-fields s1)) 'forged)
           #f))
  (check "secret not equal? to raw string" #f (equal? s1 raw))
  (check "distinct secrets with same value not equal?" #f (equal? s1 s2))
  (check "json->string refuses secret" #t (string? json-err))
  (check "json->string error does not leak plaintext" #f (and (string? json-err) (string-contains json-err raw) #t))
  (check "json->string error does not leak label" #f (and (string? json-err) (string-contains json-err "my-sensitive-label") #t)))

;;; 2. Scope-owned backend registry
(check "built-in backends registered" #t
       (let ((names (secret-backend-names)))
         (and (memq 'authinfo names) (memq 'sops names) (memq 'pass names) (memq 'env names) #t)))

(let ((sc (make-scope 'custom-secret-test)))
  (with-scope sc
    (define-secret-backend (ephemeral-vault query spec)
      "Temporary test backend."
      (let ((host (let ((e (assq 'host query))) (and e (cdr e)))))
        (and (equal? host "vault.test") "vault-secret-42")))
    (let ((found (secret-lookup #:host "vault.test" #:sources '(ephemeral-vault))))
      (check "scoped backend works inside scope" #t (secret? found))
      (check "scoped backend value" "vault-secret-42" (secret-reveal found))))
  (scope-shutdown! sc)
  (check "scoped backend removed on scope shutdown" #f (find-secret-backend 'ephemeral-vault)))

;;; 3. Declarative secret-ref + secret-resolve
(define auth-file (string-append tmp "/authinfo"))
(write-text auth-file
  (string-append
   "machine imap.example.org login ada@example.org port 993 password mail-pw\n"
   "machine api.github.com login arthur^forge password forge-pw\n"
   "machine api.github.com login arthur password plain-gh-pw\n"))

(let* ((ref (secret-ref 'authinfo #:file auth-file #:host "imap.example.org" #:user "ada@example.org" #:port 993))
       (res (secret-resolve ref)))
  (check "secret-ref? predicate" #t (secret-ref? ref))
  (check "secret-ref-backend" 'authinfo (secret-ref-backend ref))
  (check "secret-resolve returns secret" #t (secret? res))
  (check "secret-resolve authinfo value" "mail-pw" (secret-reveal res)))

;;; 4. Authinfo backend & source filters (`when-service`, `when-host`)
(let ((forge-s (secret-lookup #:service 'forge #:host "api.github.com" #:user "arthur"
                              #:extra '((login-tag . "^forge"))
                              #:sources (list (list 'authinfo #:file auth-file))))
      (filtered-miss (secret-lookup #:service 'mail #:host "api.github.com" #:user "arthur"
                                    #:sources (list (list 'authinfo #:file auth-file #:when-service 'forge)))))
  (check "authinfo prefers login-tag ^forge" "forge-pw" (secret-reveal forge-s))
  (check "when-service filter skips non-matching source" #f filtered-miss))

;;; 5. Env backend host guard + destination guard enforcement on fixed-value sources
(let* ((present-var "HOME")
       (ok-hit (secret-lookup #:host "api.github.com"
                              #:sources (list (list 'env #:var present-var #:host "api.github.com"))))
       (other-host (secret-lookup #:host "ghe.corp.test"
                                  #:sources (list (list 'env #:var present-var #:host "api.github.com")))))
  (check "env matches guarded host" #t (secret? ok-hit))
  (check "env refuses different host" #f other-host))

;;; 6. SOPS backend (structured JSON, local key path, --extract, authinfo-in-SOPS, extensionless binary, error safety)
(define sops-yaml (string-append tmp "/secrets.yaml"))
(write-text sops-yaml "enc")
(define sops-auth-bin (string-append tmp "/authinfo-sops"))
(write-text sops-auth-bin "enc")
(define stub-sops (string-append tmp "/stub-sops"))
(make-script stub-sops
  (string-append
   "#!/bin/sh\n"
   "if [ \"$1\" != \"decrypt\" ]; then echo \"bad subcommand\" >&2; exit 2; fi\n"
   "shift\n"
   "itype=\"\"\n"
   "otype=\"\"\n"
   "extract=\"\"\n"
   "while [ $# -gt 0 ]; do\n"
   "  case \"$1\" in\n"
   "    --input-type) itype=\"$2\"; shift 2;;\n"
   "    --output-type) otype=\"$2\"; shift 2;;\n"
   "    --extract) extract=\"$2\"; shift 2;;\n"
   "    --) shift; break;;\n"
   "    *) break;;\n"
   "  esac\n"
   "done\n"
   "file=\"$1\"\n"
   "if [ \"$extract\" = '[\"raw\"][\"extract\"]' ]; then\n"
   "  printf 'sops-raw-extract-tok\\n'\n"
   "  exit 0\n"
   "fi\n"
   "if [ \"$file\" = \"" sops-auth-bin "\" ]; then\n"
   "  if [ \"$itype\" != \"binary\" ] || [ \"$otype\" != \"binary\" ]; then\n"
   "    echo \"expected binary input/output for extensionless file, got $itype/$otype\" >&2\n"
   "    exit 3\n"
   "  fi\n"
   "  printf 'machine smtp.sops.test login ada port 587 password sops-authinfo-pw\\n'\n"
   "  exit 0\n"
   "fi\n"
   "if [ \"$otype\" = \"json\" ]; then\n"
   "  printf '{\"api.github.com\":\"sops-gh-tok\",\"github\":{\"token\":\"sops-extracted-tok\"},\"items\":[\"first\"],\"mail\":{\"work\":{\"user\":\"ada\",\"password\":\"sops-mail-pw\"}}}'\n"
   "  exit 0\n"
   "fi\n"
   "printf 'raw-sops-secret\\n'\n"))

(let ((s-key (sops-secret sops-yaml #:key '("github" "token") #:sops stub-sops))
      (s-branch (sops-secret sops-yaml #:key "github" #:sops stub-sops))
      (s-oob (sops-secret sops-yaml #:key '("items" 5) #:sops stub-sops))
      (s-raw-ext (sops-secret sops-yaml #:extract "[\"raw\"][\"extract\"]" #:sops stub-sops))
      (s-miss (sops-secret sops-yaml #:key "missing" #:sops stub-sops))
      (s-absent (sops-secret (string-append tmp "/absent.yaml") #:key "x" #:sops stub-sops))
      (s-struct-host (secret-lookup #:host "api.github.com"
                                    #:sources (list (list 'sops #:file sops-yaml #:sops stub-sops))))
      (s-struct-acct (secret-lookup #:service 'mail #:account "work" #:user "ada"
                                    #:sources (list (list 'sops #:file sops-yaml #:sops stub-sops))))
      (s-struct-wrong-user (secret-lookup #:service 'mail #:account "work" #:user "other"
                                          #:sources (list (list 'sops #:file sops-yaml #:sops stub-sops))))
      (s-authinfo (secret-lookup #:service 'smtp #:host "smtp.sops.test" #:user "ada" #:port 587
                                 #:sources (list (list 'sops #:file sops-auth-bin #:format 'authinfo #:sops stub-sops)))))
  (check "sops-secret #:key walks decrypted JSON" "sops-extracted-tok" (secret-reveal s-key))
  (check "sops-secret #:key branch extracts leaf token" "sops-extracted-tok" (secret-reveal s-branch))
  (check "sops-secret out-of-bounds vector index returns #f" #f s-oob)
  (check "sops-secret explicit #:extract passes --extract" "sops-raw-extract-tok" (secret-reveal s-raw-ext))
  (check "sops-secret missing key returns #f" #f s-miss)
  (check "sops-secret absent file returns #f" #f s-absent)
  (check "sops structured lookup by host" "sops-gh-tok" (secret-reveal s-struct-host))
  (check "sops structured lookup matches dict user" "sops-mail-pw" (secret-reveal s-struct-acct))
  (check "sops structured lookup rejects mismatched dict user" #f s-struct-wrong-user)
  (check "sops extensionless authinfo lookup" "sops-authinfo-pw" (secret-reveal s-authinfo)))

;; Real SOPS decryption failure raises redacted error (never swallowed as #f, never leaks stdout/stderr).
(define fail-sops (string-append tmp "/fail-sops"))
(make-script fail-sops
  "#!/bin/sh\necho 'SOPS-STDOUT-LEAK'\necho 'Failed to get data key: SOPS-STDERR-LEAK key not found' >&2\nexit 128\n")
(let ((err (guard (e (#t (error-object-message e)))
             (sops-secret sops-yaml #:key "github" #:sops fail-sops)
             #f)))
  (check "sops decryption failure raises error" #t (string? err))
  (check "sops decryption error is redacted" #t (and (string? err) (string-contains err "sops: decryption failed") #t))
  (check "sops error never leaks stdout" #f (and (string? err) (string-contains err "SOPS-STDOUT-LEAK") #t))
  (check "sops error never leaks raw stderr" #f (and (string? err) (string-contains err "SOPS-STDERR-LEAK") #t)))

;;; 7. Pass backend (only spawns when .gpg exists; returns first line only)
(define pass-store (string-append tmp "/password-store"))
(create-directory pass-store #o700)
(create-directory (string-append pass-store "/mail") #o700)
(write-text (string-append pass-store "/mail/imap.pass.test.gpg") "enc")
(define stub-pass (string-append tmp "/stub-pass"))
(make-script stub-pass
  "#!/bin/sh\nif [ \"$1\" = \"show\" ] && [ \"$2\" = \"--\" ] && [ \"$3\" = \"mail/imap.pass.test\" ]; then\n  printf 'pass-line-1\\nusername: ada\\nurl: https://example.org\\n'\n  exit 0\nfi\necho 'unexpected pass args' >&2\nexit 1\n")

(let ((p-hit (secret-lookup #:host "imap.pass.test"
                            #:sources (list (list 'pass #:store pass-store #:prefix "mail/" #:pass stub-pass))))
      (p-miss (secret-lookup #:host "absent.pass.test"
                             #:sources (list (list 'pass #:store pass-store #:prefix "mail/" #:pass "/nonexistent/pass")))))
  (check "pass backend reads first line only" "pass-line-1" (secret-reveal p-hit))
  (check "pass backend skips missing entry without spawning" #f p-miss))

;;; 7b. Shared-source safety: fixed-value sources require guards; unknown backends skipped in lookup
(define (raises-guard-error? src)
  (let ((err (guard (e (#t (error-object-message e)))
               (secret-lookup #:host "api.github.com" #:sources (list src))
               #f)))
    (and (string? err) (string-contains err "guard") #t)))

(check "unguarded env source rejected in secret-lookup" #t
       (raises-guard-error? '(env #:var "HOME")))
(check "unguarded sops #:key source rejected in secret-lookup" #t
       (raises-guard-error? (list 'sops #:file sops-yaml #:key "github" #:sops stub-sops)))
(check "unguarded sops binary source rejected in secret-lookup" #t
       (raises-guard-error? (list 'sops #:file sops-yaml #:format 'binary #:sops stub-sops)))
(check "unguarded pass #:entry source rejected in secret-lookup" #t
       (raises-guard-error? (list 'pass #:entry "mail/imap.pass.test" #:store pass-store #:pass stub-pass)))
(check "unguarded authinfo #:host alias rejected in secret-lookup" #t
       (raises-guard-error? (list 'authinfo #:file auth-file #:host "imap.example.org")))

(let ((res (secret-lookup #:host "imap.example.org" #:user "ada@example.org" #:port 993
                          #:sources (list '(not-yet-loaded-backend) (list 'authinfo #:file auth-file))))
      (ref-err (guard (e (#t (error-object-message e)))
                 (secret-resolve (secret-ref 'not-yet-loaded-backend))
                 #f)))
  (check "unknown backend skipped in shared secret-lookup" "mail-pw" (secret-reveal res))
  (check "unknown backend raises in explicit secret-resolve" #t
         (and (string? ref-err) (string-contains ref-err "unknown secret backend") #t)))

;;; 7c. Trusted process: trusted cwd, cancellation propagation, and cold-scope built-in survival
(let ((pwd-res (run-trusted-tool "pwd" '())))
  (check "trusted helper exits 0" 0 (car pwd-res))
  (check "trusted helper runs in trusted-cwd" (trusted-cwd) (string-trim (cadr pwd-res))))

(let* ((t (spawn (lambda () (run-trusted-tool "sleep" '("30")))))
       (_ (spawn (lambda () (sleep 50) (task-cancel t))))
       (cancelled? (guard (e ((task-cancelled-condition? e) #t)
                             (#t #f))
                     (task-join t)
                     #f)))
  (check "task cancellation propagates through run-trusted-tool" #t cancelled?))

;; Verify built-ins survive when (techne secrets) is first imported inside a temporary scope that shuts down.
(let ((techne-bin (or (get-environment-variable "TECHNE_BIN")
                      (and (file-exists? "./target/release/techne") "./target/release/techne")
                      (and (file-exists? "./target/debug/techne") "./target/debug/techne"))))
  (when techne-bin
    (let ((probe-file (string-append tmp "/cold-scope-probe.scm")))
      (write-text probe-file
        (string-append
         "(import (techne))\n"
         "(let ((s (make-scope 'temp)))\n"
         "  (with-scope s (eval '(import (techne secrets))))\n"
         "  (scope-shutdown! s))\n"
         "(import (techne secrets))\n"
         "(display (length (secret-backend-names)))\n"))
      (let ((out (call-with-process techne-bin (list "run" probe-file)
                   (lambda (p)
                     (let ((s (process-read-all p 'stdout)))
                       (process-wait p)
                       (string-trim s)))
                   #:stdin 'null)))
        (check "cold import in temporary scope preserves all 4 built-ins" "4" out)))))

;;; 7d. Trusted baseline provenance, scrubbed cleanup launch env, hook authority, and strict fail-closed credentials
(let ((forged-root (make-scope 'root)))
  (check "forged 'root scope cannot replace set-trusted-direnv-program-hook!" #t
         (with-scope forged-root
           (guard (e (#t #t))
             (set-trusted-direnv-program-hook! (lambda () "/tmp/evil"))
             #f)))
  (scope-shutdown! forged-root))

(let ((techne-bin (or (get-environment-variable "TECHNE_BIN")
                      (and (file-exists? "./target/release/techne") "./target/release/techne")
                      (and (file-exists? "./target/debug/techne") "./target/debug/techne"))))
  (when techne-bin
    (let* ((dirty-proj (string-append tmp "/dirty-proj"))
           (evil-bin (string-append tmp "/outside-evil-bin"))
           (fake-direnv (string-append evil-bin "/direnv"))
           (bogus-direnv (string-append tmp "/bogus-direnv"))
           (inspect-direnv (string-append tmp "/inspect-direnv"))
           (env-dump (string-append tmp "/cleanup-env-dump.txt"))
           (probe-unclean (string-append tmp "/probe-unclean.scm"))
           (probe-scrub (string-append tmp "/probe-scrub.scm"))
           (gpg-auth (string-append tmp "/strict.authinfo.gpg")))
      (create-directory dirty-proj #o700)
      (create-directory evil-bin #o700)
      (write-text gpg-auth "fake-ciphertext")
      ;; A malicious direnv outside dirty-proj on PATH must NOT be executed as trusted cleanup.
      (let ((hijack-marker (string-append tmp "/hijack-ran")))
        (make-script fake-direnv
          (string-append "#!/bin/sh\ntouch '" hijack-marker "'\nprintf '{\"DIRENV_DIR\":null,\"HIJACKED\":\"1\"}'\n"))
        ;; An explicit direnv returning non-empty JSON that fails to unset DIRENV_DIR must NOT mark baseline clean.
        (make-script bogus-direnv
          "#!/bin/sh\nprintf '{\"FOO\":\"bar\"}'\n")
        ;; An explicit direnv that records its launch env and unsets DIRENV_DIR.
        (make-script inspect-direnv
          (string-append
           "#!/bin/sh\n"
           "printf 'LD_PRELOAD=%s|DIRENV_CONFIG=%s|PATH=%s' \"${LD_PRELOAD:-}\" \"${DIRENV_CONFIG:-}\" \"${PATH:-}\" > '" env-dump "'\n"
           "printf '{\"DIRENV_DIR\":null,\"DIRENV_DIFF\":null}'\n"))
        (write-text probe-unclean
          (string-append
           "(import (techne) (techne trusted-process) (techne authinfo) (techne secrets))\n"
           "(define (fails? th) (guard (e (#t #t)) (th) #f))\n"
           "(display (list\n"
           "  (trusted-baseline-clean? #:direnv-program \"direnv\")\n"
           "  (trusted-baseline-clean? #:direnv-program \"" bogus-direnv "\")\n"
           "  (begin (set-trusted-direnv-program-hook! (lambda () \"/nonexistent/direnv-missing\"))\n"
           "         (trusted-baseline-clean?))\n"
           "  (not (trusted-home))\n"
           "  (fails? (lambda () (run-trusted-tool \"pwd\" '())))\n"
           "  (fails? (lambda () (authinfo-entries #:file \"" gpg-auth "\" #:gpg \"" stub-sops "\")))\n"
           "  (fails? (lambda () (sops-secret \"" sops-yaml "\" #:key \"github\" #:sops \"" stub-sops "\")))\n"
           "  (equal? (trusted-baseline-path) \"/usr/bin:/bin\")\n"
           "  (fails? (lambda () (secret-lookup #:host \"imap.pass.test\"\n"
           "                                    #:sources (list (list 'pass #:store \"" pass-store "\" #:prefix \"mail/\" #:pass \"" stub-pass "\")))))))\n"))
        (let ((out (call-with-process techne-bin (list "run" probe-unclean)
                     (lambda (p)
                       (let ((s (process-read-all p 'stdout)))
                         (process-wait p)
                         (string-trim s)))
                     #:environment (list (cons "DIRENV_DIR" (string-append "-" dirty-proj))
                                         (cons "DIRENV_DIFF" "bogus")
                                         (cons "PATH" (string-append evil-bin ":/nonexistent-only")))
                     #:stdin 'null)))
          (check "untrusted PATH direnv never executed" #f (file-exists? hijack-marker))
          (check "untrusted PATH direnv & bogus delta rejected; credentials fail closed"
                 "(#f #f #f #t #t #t #t #t #t)" out)))
      (let ((trojan-helper (string-append evil-bin "/trojan-cred-helper"))
            (child-probe (string-append tmp "/child-path-probe")))
        (make-script trojan-helper "#!/bin/sh\necho TROJAN\n")
        (make-script child-probe "#!/bin/sh\nprintf '%s' \"${PATH:-}\"\n")
        (write-text probe-scrub
          (string-append
           "(import (techne) (techne trusted-process))\n"
           "(set-trusted-direnv-program-hook! (lambda () \"" inspect-direnv "\"))\n"
           "(let* ((clean? (trusted-baseline-clean?))\n"
           "       (delta (trusted-baseline-environment))\n"
           "       (_ (for-each (lambda (e)\n"
           "                      (when (string? (car e)) (string-fill! (car e) #\\K))\n"
           "                      (when (string? (cdr e)) (string-fill! (cdr e) #\\V))\n"
           "                      (set-car! e \"POISONED\")\n"
           "                      (set-cdr! e \"/tmp/evil\"))\n"
           "                    delta))\n"
           "       (path-copy (trusted-environment-variable \"PATH\"))\n"
           "       (_ (when (string? path-copy) (string-fill! path-copy #\\Z)))\n"
           "       (resolved-trojan (resolve-trusted-program \"trojan-cred-helper\"))\n"
           "       (baseline-path (trusted-baseline-path))\n"
           "       (env-path-after (trusted-environment-variable \"PATH\"))\n"
           "       (child-res (run-trusted-tool \"" child-probe "\" '())))\n"
           "  (write (list clean? resolved-trojan baseline-path env-path-after (cadr child-res))))\n"))
        (let ((out (call-with-process techne-bin (list "run" probe-scrub)
                     (lambda (p)
                       (let ((s (process-read-all p 'stdout)))
                         (process-wait p)
                         (string-trim s)))
                     #:environment (list (cons "DIRENV_DIR" (string-append "-" dirty-proj))
                                         (cons "LD_PRELOAD" "/tmp/evil.so")
                                         (cons "DIRENV_CONFIG" "/tmp/evil-direnv-cfg")
                                         (cons "PATH" (string-append evil-bin ":/usr/bin:/bin")))
                     #:stdin 'null)))
          (check "cleanup omitting PATH & mutating baseline readers keeps resolver & child on safe PATH"
                 "(#t #f \"/usr/bin:/bin\" \"/usr/bin:/bin\" \"/usr/bin:/bin\")" out)
          (check "cleanup direnv launched with scrubbed LD_PRELOAD, DIRENV_CONFIG, and PATH"
                 "LD_PRELOAD=|DIRENV_CONFIG=/dev/null|PATH=/usr/bin:/bin"
                 (file->string env-dump)))))))


;;; 8. Real GPG + real SOPS end-to-end decryption (isolated temp GNUPGHOME, skipped if absent)
(define (command-available? name)
  (guard (e (#t #f))
    (call-with-process "sh" (list "-c" (string-append "command -v " name " >/dev/null 2>&1"))
      (lambda (p) (equal? (process-wait p) 0))
      #:stdin 'null)))

(when (and (command-available? "gpg") (command-available? "sops"))
  (let* ((gnupg (string-append tmp "/gnupg"))
         (gpg-wrap (string-append tmp "/real-gpg"))
         (sops-wrap (string-append tmp "/real-sops"))
         (real-auth-gpg (string-append tmp "/real.authinfo.gpg"))
         (real-sops-yaml (string-append tmp "/real.sops.yaml"))
         (real-sops-bin (string-append tmp "/real-sops-authinfo")))
    (create-directory gnupg #o700)
    (make-script gpg-wrap
      (string-append "#!/bin/sh\nGNUPGHOME='" gnupg "' exec gpg \"$@\"\n"))
    (make-script sops-wrap
      (string-append "#!/bin/sh\nGNUPGHOME='" gnupg "' exec sops \"$@\"\n"))
    ;; Generate ephemeral passphraseless test key, encrypt authinfo.gpg + SOPS YAML + extensionless SOPS binary.
    (let ((setup-status
           (call-with-process "sh"
             (list "-c"
                   (string-append
                    "set -e\n"
                    "export GNUPGHOME='" gnupg "'\n"
                    "gpg --batch --pinentry-mode loopback --passphrase '' --quick-generate-key 'Techne Test <test@techne.invalid>' default default never >/dev/null 2>&1\n"
                    "KEY_ID=$(gpg --with-colons --list-keys test@techne.invalid | awk -F: '$1==\"f\"\"pr\"{print $10; exit}')\n"
                    "printf 'machine imap.real.test login ada port 993 password real-gpg-secret\\n' | gpg --batch --yes --trust-model always -e -r \"$KEY_ID\" -o '" real-auth-gpg "'\n"
                    "printf 'api.github.com: real-sops-gh-tok\\nmail:\\n  work:\\n    password: real-sops-mail-pw\\n' > '" real-sops-yaml ".plain'\n"
                    "sops encrypt --pgp \"$KEY_ID\" --input-type yaml --output-type yaml '" real-sops-yaml ".plain' > '" real-sops-yaml "'\n"
                    "printf 'machine smtp.real.test login bob port 587 password real-sops-authinfo-pw\\n' > '" real-sops-bin ".plain'\n"
                    "sops encrypt --pgp \"$KEY_ID\" --input-type binary --output-type binary '" real-sops-bin ".plain' > '" real-sops-bin "'\n"))
             (lambda (p) (process-wait p))
             #:stdin 'null)))
      (dynamic-wind
        (lambda () #f)
        (lambda ()
          (check "ephemeral gpg+sops fixture setup" 0 setup-status)
          (when (equal? setup-status 0)
            (let ((s-gpg (secret-lookup #:host "imap.real.test" #:user "ada" #:port 993
                                        #:sources (list (list 'authinfo #:file real-auth-gpg #:gpg gpg-wrap))))
                  (s-yaml (sops-secret real-sops-yaml #:key '("mail" "work" "password") #:sops sops-wrap))
                  (s-struct (secret-lookup #:host "api.github.com"
                                           #:sources (list (list 'sops #:file real-sops-yaml #:sops sops-wrap))))
                  (s-bin (secret-lookup #:service 'smtp #:host "smtp.real.test" #:user "bob" #:port 587
                                        #:sources (list (list 'sops #:file real-sops-bin #:format 'authinfo #:sops sops-wrap)))))
              (check "real gpg decrypts authinfo.gpg" "real-gpg-secret" (secret-reveal s-gpg))
              (check "real sops --extract decrypts yaml key" "real-sops-mail-pw" (secret-reveal s-yaml))
              (check "real sops structured lookup" "real-sops-gh-tok" (secret-reveal s-struct))
              (check "real sops extensionless binary authinfo" "real-sops-authinfo-pw" (secret-reveal s-bin)))))
        (lambda ()
          (guard (e (#t #f))
            (call-with-process "gpgconf" (list "--homedir" gnupg "--kill" "gpg-agent")
              (lambda (p) (process-wait p))
              #:stdin 'null)))))))

(rm-rf tmp)
(test-summary)
