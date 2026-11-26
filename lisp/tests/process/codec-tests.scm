;;; Tests for `(techne codec)` and binary `run-trusted-tool-bytes`.
(import (techne) (techne codec) (techne trusted-process) (techne task) (techne uuid))
(require "../../test.scm")

(define tmp-raw (string-append (or (get-environment-variable "TMPDIR") "/tmp") "/techne-codec-" (uuid-v4)))
(create-directory tmp-raw #o700)
(define tmp (file-truename tmp-raw))

(define (write-text path text)
  (call-with-output-file path (lambda (p) (display text p))))

(define (make-script path body)
  (write-text path body)
  (call-with-process "chmod" (list "+x" path) (lambda (p) (process-wait p))))

(define (rm-rf path)
  (call-with-process "rm" (list "-rf" "--" path) (lambda (p) (process-wait p))))

(define (repeat-string s n)
  (let loop ((i 0) (acc '()))
    (if (= i n) (apply string-append acc) (loop (+ i 1) (cons s acc)))))

;;; 1. Built-in gzip codec registration & matching
(check "gzip codec registered" #t (and (memq 'gzip (codec-names)) #t))
(let ((gz (find-codec 'gzip)))
  (check "find-codec returns codec" #t (codec? gz))
  (check "gzip codec-name" 'gzip (codec-name gz))
  (check "gzip is not private" #f (codec-private? gz))
  (check "gzip priority is 0" 0 (codec-priority gz))
  (check "gzip matches .scm.gz" 'gzip (codec-name (codec-for-path "/tmp/foo.scm.gz")))
  (check "gzip matches .md.gz" 'gzip (codec-name (codec-for-path "/tmp/notes.md.gz")))
  (check "gzip matches remote path" 'gzip (codec-name (codec-for-path "/ssh:box:/var/log/app.log.gz")))
  (check "gzip excludes .tar.gz" #f (codec-for-path "/tmp/archive.tar.gz"))
  (check "gzip excludes bare .gz" #f (codec-for-path "/tmp/.gz"))
  (check "gzip excludes plain .scm" #f (codec-for-path "/tmp/foo.scm"))
  (check "gzip logical path strips .gz" "/tmp/foo.scm" (codec-logical-path gz "/tmp/foo.scm.gz"))
  (check "gzip init-envelope" '((format . gzip)) (codec-init-envelope gz "/tmp/new.txt.gz")))

;;; 2. Gzip encode/decode round-trip, empty/absent, and error cases
(let* ((gz (find-codec 'gzip))
       (sample ";; Scheme + Unicode: λ α β γ 日本語\n(define (square x) (* x x))\n")
       (enc1 (codec-encode gz "/tmp/test.scm.gz" sample '((format . gzip))))
       (enc2 (codec-encode gz "/tmp/test.scm.gz" sample '((format . gzip))))
       (dec (codec-decode gz "/tmp/test.scm.gz" enc1)))
  (check "gzip encode returns bytevector" #t (bytevector? enc1))
  (check "gzip magic header 0x1f 0x8b" #t
         (and (>= (bytevector-length enc1) 2)
              (= (bytevector-u8-ref enc1 0) #x1f)
              (= (bytevector-u8-ref enc1 1) #x8b)))
  (check "gzip -n output is deterministic" #t (equal? enc1 enc2))
  (check "gzip decode returns codec-result" #t (codec-result? dec))
  (check "gzip round-trip text exact" sample (codec-result-text dec))
  (check "gzip round-trip envelope" '((format . gzip)) (codec-result-envelope dec)))

(let* ((gz (find-codec 'gzip))
       (enc-empty (codec-encode gz "/tmp/empty.txt.gz" "" #f))
       (dec-empty (codec-decode gz "/tmp/empty.txt.gz" enc-empty))
       (dec-absent (codec-decode gz "/tmp/new.txt.gz" #f))
       (dec-zero (codec-decode gz "/tmp/zero.txt.gz" #u8())))
  (check "gzip empty string round-trip" "" (codec-result-text dec-empty))
  (check "gzip decode #f returns empty string" "" (codec-result-text dec-absent))
  (check "gzip decode #u8() returns empty string" "" (codec-result-text dec-zero)))

(check "gzip corrupt bytes raises error" #t
       (guard (e (#t (and (error-object? e)
                          (string-contains (error-object-message e) "gzip")
                          #t)))
         (codec-decode 'gzip "/tmp/bad.gz" #u8(1 2 3 4 5 6 7 8))
         #f))

;; Gzip stream of invalid UTF-8 bytes must fail decoding cleanly
(let* ((bad-utf8 #u8(#xff #xfe #x00 #x80))
       (res (run-trusted-tool-bytes "gzip" '("-c" "-n") #:stdin bad-utf8))
       (gz-bad (cadr res)))
  (check "gzip compressed invalid utf8 cleanly" 0 (car res))
  (check "gzip decode rejects invalid UTF-8" #t
         (guard (e (#t (and (error-object? e)
                            (string-contains (error-object-message e) "UTF-8")
                            #t)))
           (codec-decode 'gzip "/tmp/bin.gz" gz-bad)
           #f)))

;;; 3. Scope-owned codec registry, priority ordering, override & teardown
(let ((sc (make-scope 'custom-codec-test)))
  (with-scope sc
    (define-codec custom-box
      "Test codec with custom envelope."
      #:match (lambda (p) (string-suffix? ".box" p))
      #:logical-path (lambda (p) (substring p 0 (- (string-length p) 4)))
      #:init-envelope (lambda (_p) '((tag . "INIT")))
      #:decode (lambda (_p raw)
                 (if (not raw)
                     (make-codec-result "" #:envelope '((tag . "INIT")))
                     (let* ((s (utf8->string raw))
                            (nl (string-index s #\newline)))
                       (if nl
                           (make-codec-result (substring s (+ nl 1))
                                              #:envelope (list (cons 'tag (substring s 0 nl))))
                           (make-codec-result s #:envelope '((tag . "NONE")))))))
      #:encode (lambda (_p text env)
                 (let ((tag (or (and (pair? env) (let ((e (assq 'tag env))) (and e (cdr e)))) "DEFAULT")))
                   (string->utf8 (string-append tag "\n" text))))
      #:private #t
      #:priority 10)
    (define-codec gzip
      "Temporary override of built-in gzip."
      #:match (lambda (p) (string-suffix? ".gz" p))
      #:decode (lambda (_p _raw) (make-codec-result "OVERRIDDEN"))
      #:encode (lambda (_p _text _env) #u8(42))
      #:priority 5)
    (let* ((box (codec-for-path "/tmp/secret.yaml.box"))
           (encoded (codec-encode box "/tmp/secret.yaml.box" "k: v\n" '((tag . "KEY-1"))))
           (decoded (codec-decode box "/tmp/secret.yaml.box" encoded)))
      (check "custom codec matched" 'custom-box (codec-name box))
      (check "custom codec private?" #t (codec-private? box))
      (check "custom codec logical path" "/tmp/secret.yaml" (codec-logical-path box "/tmp/secret.yaml.box"))
      (check "custom codec round-trip text" "k: v\n" (codec-result-text decoded))
      (check "custom codec round-trip envelope" '((tag . "KEY-1")) (codec-result-envelope decoded))
      (check "scoped override shadows built-in gzip" "OVERRIDDEN"
             (codec-result-text (codec-decode 'gzip "/tmp/a.gz" #u8(1 2))))))
  (scope-shutdown! sc)
  (check "scoped codec removed on shutdown" #f (find-codec 'custom-box))
  (check "built-in gzip restored after override scope shutdown" #t
         (and (codec? (find-codec 'gzip))
              (string=? "hello"
                        (codec-result-text
                         (codec-decode 'gzip "/tmp/a.gz"
                                       (codec-encode 'gzip "/tmp/a.gz" "hello" #f)))))))

;; Priority ordering: higher priority wins regardless of registration order,
;; and a higher-priority non-private codec cannot strip privacy from a lower-priority private codec.
(let ((sc (make-scope 'priority-test)))
  (with-scope sc
    (define-codec low-prio "Low private." #:match (lambda (p) (string-suffix? ".dup" p))
      #:decode (lambda (_p _r) "low") #:encode (lambda (_p _t _e) #u8()) #:private #t #:priority 1)
    (define-codec high-prio "High non-private." #:match (lambda (p) (string-suffix? ".dup" p))
      #:decode (lambda (_p _r) "high") #:encode (lambda (_p _t _e) #u8()) #:private #f #:priority 20)
    (check "higher priority codec wins" 'high-prio (codec-name (codec-for-path "/tmp/x.dup")))
    (check "codecs-for-path returns all matches in priority order"
           '(high-prio low-prio)
           (map codec-name (codecs-for-path "/tmp/x.dup")))
    (check "path-private-by-codec? stays #t when any matching codec is private"
           #t (path-private-by-codec? "/tmp/x.dup")))
  (scope-shutdown! sc))

;; Name-shadowing a private codec from another scope must NEVER weaken privacy
;; (neither by setting #:private #f nor by narrowing #:match on the same name).
(let ((sc1 (make-scope 'priv-owner))
      (sc2 (make-scope 'priv-shadower)))
  (with-scope sc1
    (define-codec vault-shadow "Original private codec."
      #:match (lambda (p) (string-suffix? ".vlt" p))
      #:decode (lambda (_p _r) "secret")
      #:encode (lambda (_p t _e) (string->utf8 t))
      #:private #t))
  (with-scope sc2
    (check "shadowing private codec with #:private #f is rejected" 'rejected
           (guard (e (#t 'rejected))
             (define-codec vault-shadow "Attempted downgrade."
               #:match (lambda (p) (string-suffix? ".vlt" p))
               #:decode (lambda (_p _r) "plain")
               #:encode (lambda (_p t _e) (string->utf8 t))
               #:private #f)
             'allowed))
    ;; Even shadowing with #:private #t and narrower #:match keeps shadowed private path private!
    (define-codec vault-shadow "Narrowed matcher."
      #:match (lambda (_p) #f)
      #:decode (lambda (_p _r) "narrow")
      #:encode (lambda (_p t _e) (string->utf8 t))
      #:private #t)
    (check "path-private-by-codec? stays #t when shadowed private codec matches"
           #t (path-private-by-codec? "/tmp/secret.vlt")))
  (scope-shutdown! sc2)
  (scope-shutdown! sc1))

(check "override-builtin-private? rejected when #:private #f even in root scope" 'rejected
       (guard (e (#t 'rejected))
         (register-codec! 'root-nonpriv-override "Attempted non-private override."
                          #:match (lambda (_p) #f)
                          #:decode (lambda (_p _r) "")
                          #:encode (lambda (_p t _e) (string->utf8 t))
                          #:private #f
                          #:override-builtin-private? #t)
         'allowed))

;; Mutating path strings inside a codec matcher must not affect the caller's
;; string, subsequent matchers, or privacy classification.
(let ((sc-mut (make-scope 'mutating-matcher-scope))
      (sc-priv (make-scope 'target-private-scope)))
  (with-scope sc-priv
    (register-codec! 'priv-mut-target "Private codec behind mutating higher-priority matcher."
                     #:match (lambda (p) (string-suffix? ".mutpriv" p))
                     #:decode (lambda (_p r) (if r (utf8->string r) ""))
                     #:encode (lambda (_p t _e) (string->utf8 t))
                     #:private #t
                     #:priority 5))
  (with-scope sc-mut
    (register-codec! 'mutating-matcher "Hostile matcher that mutates its path argument."
                     #:match (lambda (p)
                               (when (and (string? p) (> (string-length p) 0))
                                 (string-fill! p #\X))
                               #f)
                     #:decode (lambda (_p r) (if r (utf8->string r) ""))
                     #:encode (lambda (_p t _e) (string->utf8 t))
                     #:private #f
                     #:priority 1000))
  (let ((p (string-copy "/tmp/secret.mutpriv")))
    (check "mutating matcher does not defeat codec-for-path on subsequent matcher"
           'priv-mut-target (codec-name (codec-for-path p)))
    (check "caller path string is unchanged after codec-for-path"
           "/tmp/secret.mutpriv" p)
    (check "mutating matcher does not defeat codecs-for-path"
           '(priv-mut-target) (map codec-name (codecs-for-path p)))
    (check "caller path string is unchanged after codecs-for-path"
           "/tmp/secret.mutpriv" p)
    (check "mutating matcher does not defeat path-private-by-codec?"
           #t (path-private-by-codec? p))
    (check "caller path string is unchanged after path-private-by-codec?"
           "/tmp/secret.mutpriv" p))
  (scope-shutdown! sc-mut)
  (scope-shutdown! sc-priv))

;;; 4. run-trusted-tool-bytes: large payload (>256 KiB), early exit, size limit, timeout
(let* ((big-text (repeat-string "0123456789abcdef\n" 18000)) ; ~306 KiB > OS pipe buffer
       (enc (codec-encode 'gzip "/tmp/big.txt.gz" big-text #f))
       (dec (codec-decode 'gzip "/tmp/big.txt.gz" enc)))
  (check "large >256KiB gzip round-trip without deadlock"
         (string-length big-text) (string-length (codec-result-text dec)))
  (check "large >256KiB gzip round-trip exact content" #t (string=? big-text (codec-result-text dec))))

;; Early non-zero exit while writer pushes large stdin preserves exit code and stderr
(let ((fail-script (string-append tmp "/early-exit.sh")))
  (make-script fail-script "#!/bin/sh\necho 'boom on stderr' >&2\nexit 7\n")
  (let* ((big-stdin (string->utf8 (repeat-string "X" 200000)))
         (res (run-trusted-tool-bytes fail-script '() #:stdin big-stdin #:label "early-exit")))
    (check "early exit returns child exit code" 7 (car res))
    (check "early exit captures stderr" #t (and (string-contains (caddr res) "boom on stderr") #t))))

;; Helper that exits 0 after discarding stdin MUST fail closed instead of succeeding with truncated input
(let ((silent-trunc (string-append tmp "/silent-trunc.sh")))
  (make-script silent-trunc "#!/bin/sh\nhead -c 1 >/dev/null\nexit 0\n")
  (let ((big-stdin (string->utf8 (repeat-string "Z" 200000))))
    (check "exit 0 after truncated stdin fails closed" #t
           (guard (e (#t (and (error-object? e)
                              (string-contains (error-object-message e) "stdin delivery failed before helper exited 0")
                              #t)))
             (run-trusted-tool-bytes silent-trunc '() #:stdin big-stdin #:label "silent-trunc")
             #f))))

;; max-stdout bound enforced
(check "run-trusted-tool-bytes enforces max-stdout" #t
       (guard (e (#t (and (error-object? e)
                          (string-contains (error-object-message e) "exceeded size limit")
                          #t)))
         (run-trusted-tool-bytes "cat" '() #:stdin (repeat-string "A" 2048) #:max-stdout 512 #:label "cat")
         #f))

;; timeout kills hung helper
(let ((sleep-script (string-append tmp "/hang.sh")))
  (make-script sleep-script "#!/bin/sh\nsleep 30\n")
  (check "run-trusted-tool-bytes enforces timeout" #t
         (guard (e (#t (and (error-object? e)
                            (string-contains (error-object-message e) "timed out")
                            #t)))
           (run-trusted-tool-bytes sleep-script '() #:stdin "hi" #:timeout 150 #:label "hang")
           #f)))

;; relative path with slash rejected
(check "run-trusted-tool-bytes rejects relative path with slash" #t
       (guard (e (#t (and (error-object? e)
                          (string-contains (error-object-message e) "relative executable")
                          #t)))
         (run-trusted-tool-bytes "./gzip" '("-c"))
         #f))

;; M1: bare helper name symlinked from accepted PATH dir into dirty DIRENV_DIR root must be rejected
(let* ((dirty-dir (string-append tmp "/dirty-proj"))
       (safe-bin (string-append tmp "/safe-bin"))
       (evil-helper (string-append dirty-dir "/evil-helper"))
       (link-helper (string-append safe-bin "/evil-helper"))
       (probe-scm (string-append tmp "/probe-m1.scm"))
       (self-exe (guard (e (#t #f)) (file-truename "/proc/self/exe"))))
  (create-directory dirty-dir #o700)
  (create-directory safe-bin #o700)
  (make-script evil-helper "#!/bin/sh\necho PWNED\n")
  (call-with-process "ln" (list "-s" evil-helper link-helper) (lambda (p) (process-wait p)))
  (write-text probe-scm
              "(import (techne) (techne trusted-process))\n\
               (display (if (resolve-trusted-program \"evil-helper\") \"ALLOWED\" \"REJECTED\"))\n")
  (when self-exe
    (let* ((orig-path (or (get-environment-variable "PATH") "/usr/bin:/bin"))
           (res (call-with-process
                 self-exe
                 (list "run" probe-scm)
                 (lambda (p)
                   (let ((out (let loop ((acc '()))
                                (let ((c (process-read p 'stdout)))
                                  (if (eof-object? c) (apply string-append (reverse acc)) (loop (cons c acc))))))
                         (code (process-wait p)))
                     (list code out)))
                 #:environment (list (cons "DIRENV_DIR" (string-append "-" dirty-dir))
                                     (cons "PATH" (string-append safe-bin ":" orig-path))))))
      (check "PATH symlink targeting DIRENV_DIR helper is rejected" '(0 "REJECTED") res))))

;;; 5. Built-in gpg codec (real gpg with ephemeral GNUPGHOME + edge cases)
(check "gpg codec registered" #t (and (memq 'gpg (codec-names)) #t))
(let ((g (find-codec 'gpg)))
  (check "gpg is private" #t (codec-private? g))
  (check "gpg priority is 0" 0 (codec-priority g))
  (check "gpg matches .scm.gpg" 'gpg (codec-name (codec-for-path "/tmp/secret.scm.gpg")))
  (check "gpg matches remote .gpg" 'gpg (codec-name (codec-for-path "/ssh:box:/etc/vault.yaml.gpg")))
  (check "gpg excludes bare .gpg" #f (codec-for-path "/tmp/.gpg"))
  (check "gpg logical path strips .gpg" "/tmp/secret.scm" (codec-logical-path g "/tmp/secret.scm.gpg"))
  (check "gpg path-private-by-codec?" #t (path-private-by-codec? "/tmp/secret.scm.gpg")))

(when (resolve-trusted-program "gpg")
  (let* ((gnupg (string-append tmp "/gnupg"))
         (run-gpg (lambda (args stdin-bytes)
                    (run-trusted-tool-bytes "gpg"
                                            (append (list "--homedir" gnupg "--batch" "--no-tty" "--yes" "--quiet") args)
                                            #:stdin stdin-bytes
                                            #:label "gpg-setup"))))
    (create-directory gnupg #o700)
    (let ((r1 (run-gpg '("--passphrase" "" "--quick-generate-key" "Alice Test <alice@example.test>" "default" "default" "never") #f))
          (r2 (run-gpg '("--passphrase" "" "--quick-generate-key" "Bob Test <bob@example.test>" "default" "default" "never") #f)))
      (check "generated ephemeral Alice gpg key" 0 (car r1))
      (check "generated ephemeral Bob gpg key" 0 (car r2)))

    ;; 5a. Binary public-key round-trip + envelope recipient preservation
    (parameterize ((default-gpg-homedir gnupg)
                   (default-gpg-batch #t)
                   (default-gpg-armor #f)
                   (default-gpg-recipients '("alice@example.test")))
      (let* ((sample ";; Secret Scheme: λ\n(define api-key \"s3cr3t\")\n")
             (p (string-append tmp "/one.scm.gpg"))
             (init-env (codec-init-envelope 'gpg p))
             (ct (codec-encode 'gpg p sample init-env))
             (dec (codec-decode 'gpg p ct))
             (env (codec-result-envelope dec)))
        (check "gpg binary ciphertext is bytevector" #t (bytevector? ct))
        (check "gpg binary ciphertext is not armored" #f
               (and (guard (e (#t #f)) (string-prefix? "-----BEGIN PGP MESSAGE-----" (utf8->string ct))) #t))
        (check "gpg binary decode matches plaintext" sample (codec-result-text dec))
        (check "gpg decode mode is public-key" 'public-key (cdr (assq 'mode env)))
        (check "gpg decode armor is #f" #f (cdr (assq 'armor env)))
        (check "gpg decode captured recipient key id" #t (pair? (cdr (assq 'recipients env))))
        ;; By default (default-gpg-trust-file-recipients = #f), re-encrypting with no trusted
        ;; policy MUST refuse unauthenticated ciphertext ENC_TO recipients!
        (parameterize ((default-gpg-recipients '()))
          (check "gpg refuses unauthenticated ENC_TO recipients by default" #t
                 (guard (e (#t (and (error-object? e)
                                    (string-contains (error-object-message e) "unauthenticated ciphertext ENC_TO")
                                    #t)))
                   (codec-encode 'gpg p "refused\n" env)
                   #f))
          (check "gpg save-description is #f when ENC_TO is untrusted" #f
                 (codec-save-description (find-codec 'gpg) p env)))
        ;; Explicit opt-in via default-gpg-trust-file-recipients=#t preserves ENC_TO recipients
        (parameterize ((default-gpg-recipients '())
                       (default-gpg-trust-file-recipients #t))
          (check "gpg save-description notes file ENC_TO when opted in"
                 "1 recipient from file ENC_TO"
                 (codec-save-description (find-codec 'gpg) p env))
          (let* ((ct2 (codec-encode 'gpg p (string-append sample "(define extra 2)\n") env))
                 (dec2 (codec-decode 'gpg p ct2)))
            (check "gpg re-encrypt from envelope round-trips when opted in"
                   (string-append sample "(define extra 2)\n")
                   (codec-result-text dec2))))
        ;; Trusted explicit policy takes precedence over ENC_TO even if trust-file-recipients is #t
        (parameterize ((default-gpg-recipients '("bob@example.test"))
                       (default-gpg-trust-file-recipients #t))
          (check "gpg save-description prefers explicit gpg-recipients"
                 "1 recipient from gpg-recipients"
                 (codec-save-description (find-codec 'gpg) p env))
          (let* ((ct3 (codec-encode 'gpg p "for bob only\n" env))
                 (dec3 (codec-decode 'gpg p ct3))
                 (env3 (codec-result-envelope dec3)))
            (check "gpg trusted explicit policy overrides file ENC_TO"
                   #f (equal? (cdr (assq 'recipients env)) (cdr (assq 'recipients env3))))
            (check "gpg trusted policy override round-trips" "for bob only\n" (codec-result-text dec3))))))

    ;; 5b. Multi-recipient preservation (with explicit opt-in)
    (parameterize ((default-gpg-homedir gnupg)
                   (default-gpg-batch #t)
                   (default-gpg-recipients '("alice@example.test" "bob@example.test")))
      (let* ((p (string-append tmp "/multi.txt.gpg"))
             (ct1 (codec-encode 'gpg p "shared secret\n" (codec-init-envelope 'gpg p)))
             (dec1 (codec-decode 'gpg p ct1))
             (env1 (codec-result-envelope dec1)))
        (check "gpg multi-recipient decode sees 2 recipients" 2 (length (cdr (assq 'recipients env1))))
        (parameterize ((default-gpg-recipients '())
                       (default-gpg-trust-file-recipients #t))
          (let* ((ct2 (codec-encode 'gpg p "shared secret v2\n" env1))
                 (dec2 (codec-decode 'gpg p ct2))
                 (env2 (codec-result-envelope dec2)))
            (check "gpg multi-recipient re-encode preserves 2 recipients" 2 (length (cdr (assq 'recipients env2))))
            (check "gpg multi-recipient text round-trips" "shared secret v2\n" (codec-result-text dec2))))))

    ;; 5c. ASCII armor preservation
    (parameterize ((default-gpg-homedir gnupg)
                   (default-gpg-batch #t)
                   (default-gpg-armor #t)
                   (default-gpg-recipients '("alice@example.test")))
      (let* ((p (string-append tmp "/armored.txt.gpg"))
             (ct1 (codec-encode 'gpg p "armored text\n" (codec-init-envelope 'gpg p)))
             (dec1 (codec-decode 'gpg p ct1))
             (env1 (codec-result-envelope dec1)))
        (check "gpg armored output starts with PGP header" #t
               (string-prefix? "-----BEGIN PGP MESSAGE-----" (utf8->string ct1)))
        (check "gpg armored decode records armor=#t" #t (cdr (assq 'armor env1)))
        (parameterize ((default-gpg-armor #f)
                       (default-gpg-recipients '())
                       (default-gpg-trust-file-recipients #t))
          (let* ((ct2 (codec-encode 'gpg p "armored v2\n" env1))
                 (dec2 (codec-decode 'gpg p ct2)))
            (check "gpg re-encode preserves armor even when default-gpg-armor is #f" #t
                   (string-prefix? "-----BEGIN PGP MESSAGE-----" (utf8->string ct2)))
            (check "gpg armored v2 round-trips" "armored v2\n" (codec-result-text dec2))))))

    ;; 5d. .gpg-id discovery is opt-in and rejects symlink escape, dangling links, and empty files
    (let* ((store (string-append tmp "/pass-store"))
           (sub (string-append store "/nested"))
           (outside (string-append tmp "/outside-gpg-id"))
           (escape-store (string-append tmp "/escape-store"))
           (dangling-store (string-append tmp "/dangling-store"))
           (empty-store (string-append tmp "/empty-store")))
      (create-directory store #o700)
      (create-directory sub #o700)
      (create-directory escape-store #o700)
      (create-directory dangling-store #o700)
      (create-directory empty-store #o700)
      (write-text (string-append store "/.gpg-id") "# team keys\nalice@example.test\nbob@example.test\n")
      (write-text outside "alice@example.test\n")
      (write-text (string-append empty-store "/.gpg-id") "# only comments\n\n")
      (call-with-process "ln" (list "-s" outside (string-append escape-store "/.gpg-id")) (lambda (p) (process-wait p)))
      (call-with-process "ln" (list "-s" (string-append dangling-store "/missing") (string-append dangling-store "/.gpg-id"))
                         (lambda (p) (process-wait p)))
      (parameterize ((default-gpg-homedir gnupg)
                     (default-gpg-batch #t)
                     (default-gpg-recipients '())
                     (default-gpg-id-roots '()))
        (let ((env (codec-init-envelope 'gpg (string-append sub "/entry.txt.gpg"))))
          (check ".gpg-id ignored when default-gpg-id-roots is empty" '() (cdr (assq 'recipients env))))
        (check "gpg encode fails closed without recipients or opted-in .gpg-id" #t
               (guard (e (#t (and (error-object? e)
                                  (string-contains (error-object-message e) "no recipients")
                                  #t)))
                 (codec-encode 'gpg (string-append sub "/entry.txt.gpg") "hi\n" #f)
                 #f)))
      (parameterize ((default-gpg-homedir gnupg)
                     (default-gpg-batch #t)
                     (default-gpg-recipients '())
                     (default-gpg-id-roots (list store)))
        (let* ((p (string-append sub "/entry.txt.gpg"))
               (env (codec-init-envelope 'gpg p))
               (ct (codec-encode 'gpg p "from gpg-id\n" env))
               (dec (codec-decode 'gpg p ct)))
          (check "opted-in .gpg-id discovers both recipients"
                 '("alice@example.test" "bob@example.test")
                 (cdr (assq 'recipients env)))
          (check "gpg save-description reports .gpg-id source"
                 "2 recipients from .gpg-id"
                 (codec-save-description (find-codec 'gpg) p env))
          (check "opted-in .gpg-id round-trips" "from gpg-id\n" (codec-result-text dec))))
      (parameterize ((default-gpg-homedir gnupg)
                     (default-gpg-batch #t)
                     (default-gpg-recipients '())
                     (default-gpg-id-roots (list escape-store)))
        (check ".gpg-id symlink escaping allowed root is rejected" #t
               (guard (e (#t (and (error-object? e)
                                  (string-contains (error-object-message e) "outside allowed root")
                                  #t)))
                 (codec-init-envelope 'gpg (string-append escape-store "/x.gpg"))
                 #f)))
      (parameterize ((default-gpg-homedir gnupg)
                     (default-gpg-batch #t)
                     (default-gpg-recipients '())
                     (default-gpg-id-roots (list dangling-store)))
        (check ".gpg-id dangling symlink fails closed" #t
               (guard (e (#t (error-object? e)))
                 (codec-init-envelope 'gpg (string-append dangling-store "/x.gpg"))
                 #f)))
      (parameterize ((default-gpg-homedir gnupg)
                     (default-gpg-batch #t)
                     (default-gpg-recipients '())
                     (default-gpg-id-roots (list empty-store)))
        (check ".gpg-id with no recipients fails closed" #t
               (guard (e (#t (and (error-object? e)
                                  (string-contains (error-object-message e) "contains no recipients")
                                  #t)))
                 (codec-init-envelope 'gpg (string-append empty-store "/x.gpg"))
                 #f)))
      ;; Symlinked root or symlinked ancestor of root must fail closed on both
      ;; read-confined-file and end-to-end GPG .gpg-id discovery.
      (let ((root-alias (string-append tmp "/store-root-alias")))
        (call-with-process "ln" (list "-s" store root-alias) (lambda (p) (process-wait p)))
        (check "read-confined-file refuses symlinked root" #t
               (guard (e (#t (error-object? e)))
                 (read-confined-file root-alias ".gpg-id" 1024)
                 #f))
        (check "read-confined-file refuses symlinked ancestor of root" #t
               (guard (e (#t (error-object? e)))
                 (read-confined-file (string-append root-alias "/nested") ".gpg-id" 1024)
                 #f))
        (parameterize ((default-gpg-homedir gnupg)
                       (default-gpg-batch #t)
                       (default-gpg-recipients '())
                       (default-gpg-id-roots (list root-alias)))
          (check "gpg discovery refuses symlinked allowlist root via alias path" #t
                 (guard (e (#t (error-object? e)))
                   (codec-init-envelope 'gpg (string-append root-alias "/nested/entry.txt.gpg"))
                   #f))
          (check "gpg encode fails closed for symlinked allowlist root via alias path" #t
                 (guard (e (#t (error-object? e)))
                   (codec-encode 'gpg (string-append root-alias "/nested/entry.txt.gpg") "hi\n" #f)
                   #f))
          (check "gpg discovery does not match canonical target of symlinked allowlist root" '()
                 (cdr (assq 'recipients (codec-init-envelope 'gpg (string-append sub "/entry.txt.gpg"))))))
        (parameterize ((default-gpg-homedir gnupg)
                       (default-gpg-batch #t)
                       (default-gpg-recipients '())
                       (default-gpg-id-roots (list (string-append root-alias "/nested"))))
          (check "gpg discovery refuses symlinked ancestor of allowlist root" #t
                 (guard (e (#t (error-object? e)))
                   (codec-init-envelope 'gpg (string-append root-alias "/nested/entry.txt.gpg"))
                   #f)))))

    ;; 5e. Hidden recipients (-R) — fail closed without policy, and preserve --hidden-recipient on re-encrypt
    (let* ((mixed-res (run-gpg '("--encrypt" "--trust-model" "always"
                                 "--recipient" "bob@example.test"
                                 "--hidden-recipient" "alice@example.test")
                               (string->utf8 "mixed hidden secret\n")))
           (mixed-ct (cadr mixed-res)))
      (check "created mixed visible+hidden ciphertext" 0 (car mixed-res))
      (parameterize ((default-gpg-homedir gnupg)
                     (default-gpg-batch #t)
                     (default-gpg-recipients '())
                     (default-gpg-trust-file-recipients #t))
        (let* ((dec (codec-decode 'gpg (string-append tmp "/mixed.gpg") mixed-ct))
               (env (codec-result-envelope dec)))
          (check "mixed hidden decrypt succeeds" "mixed hidden secret\n" (codec-result-text dec))
          (check "mixed hidden sets hidden-recipients flag" #t (cdr (assq 'hidden-recipients env)))
          (check "mixed hidden re-encode fails closed even with trust-file-recipients=#t" #t
                 (guard (e (#t (and (error-object? e)
                                    (string-contains (error-object-message e) "hidden recipients")
                                    #t)))
                   (codec-encode 'gpg (string-append tmp "/mixed.gpg") "edited\n" env)
                   #f)))
        (parameterize ((default-gpg-recipients '("alice@example.test")))
          (let* ((dec (codec-decode 'gpg (string-append tmp "/mixed.gpg") mixed-ct))
                 (ct2 (codec-encode 'gpg (string-append tmp "/mixed.gpg") "edited\n" (codec-result-envelope dec)))
                 (dec2 (codec-decode 'gpg (string-append tmp "/mixed.gpg") ct2))
                 (env2 (codec-result-envelope dec2)))
            (check "mixed hidden re-encode succeeds with explicit default-gpg-recipients"
                   "edited\n" (codec-result-text dec2))
            (check "mixed hidden re-encode preserves hidden recipients via --hidden-recipient"
                   #t (cdr (assq 'hidden-recipients env2)))
            (check "mixed hidden re-encode leaks no visible ENC_TO key IDs"
                   '() (cdr (assq 'recipients env2)))))))

    ;; 5f. Fail-closed edge cases: 0-byte file, absent file, cleartext, flag injection
    (parameterize ((default-gpg-homedir gnupg)
                   (default-gpg-batch #t))
      (check "gpg absent file (#f) decodes to empty string" ""
             (codec-result-text (codec-decode 'gpg (string-append tmp "/new.gpg") #f)))
      (check "gpg 0-byte existing file (#u8()) fails closed" #t
             (guard (e (#t (and (error-object? e)
                                (string-contains (error-object-message e) "empty file is not valid encrypted data")
                                #t)))
               (codec-decode 'gpg (string-append tmp "/zero.gpg") #u8())
               #f))
      (check "gpg unencrypted cleartext fails closed" #t
             (guard (e (#t (error-object? e)))
               (codec-decode 'gpg (string-append tmp "/plain.gpg") (string->utf8 "not encrypted at all\n"))
               #f))
      (parameterize ((default-gpg-recipients '("--output=/tmp/pwn")))
        (check "gpg rejects dash-prefixed recipient flag injection" #t
               (guard (e (#t (and (error-object? e)
                                  (string-contains (error-object-message e) "must not start with '-'")
                                  #t)))
                 (codec-encode 'gpg (string-append tmp "/flag.gpg") "hi\n" #f)
                 #f))))

    ;; 5f2. Hostile gpg.conf (encrypt-to-default-key, output, armor) must not add
    ;; recipients, leak plaintext to disk, or force ASCII armor on binary saves.
    (let ((gpg-conf (string-append gnupg "/gpg.conf"))
          (leak-file (string-append tmp "/gpg-conf-leak.txt")))
      (dynamic-wind
       (lambda ()
         (call-with-output-file gpg-conf
           (lambda (p)
             (display "default-key bob@example.test\n" p)
             (display "encrypt-to-default-key\n" p)
             (display "armor\n" p)
             (display (string-append "output " leak-file "\n") p))))
       (lambda ()
         (parameterize ((default-gpg-homedir gnupg)
                        (default-gpg-batch #t)
                        (default-gpg-armor #f)
                        (default-gpg-recipients '("alice@example.test")))
           (let* ((ct (codec-encode 'gpg (string-append tmp "/hostile.gpg") "strict policy\n" #f))
                  (dec (codec-decode 'gpg (string-append tmp "/hostile.gpg") ct))
                  (env (codec-result-envelope dec)))
             (check "hostile gpg.conf: decrypt succeeds via stdout" "strict policy\n" (codec-result-text dec))
             (check "hostile gpg.conf: binary save stays non-armored" #f (cdr (assq 'armor env)))
             (check "hostile gpg.conf: encrypt-to-default-key does not add extra recipient"
                    1 (length (cdr (assq 'recipients env))))
             (check "hostile gpg.conf: output directive does not write file"
                    #f (file-exists? leak-file)))))
       (lambda ()
         (guard (e (#t #f)) (delete-file gpg-conf))
         (guard (e (#t #f)) (delete-file leak-file)))))))

;; Shadowed built-in codec name must not receive built-in save description.
(let ((sc (make-scope 'shadow-age-desc)))
  (with-scope sc
    (register-codec! 'age "Custom shadow age."
                     #:match (lambda (p) (string-suffix? ".age" p))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #t)
    (parameterize ((default-age-recipients '("age1alice")))
      (check "shadowed age codec gets #f from codec-save-description"
             #f (codec-save-description (find-codec 'age) (string-append tmp "/x.age") #f))))
  (scope-shutdown! sc))

;; 5g. Symmetric, hybrid refusal, strict status parsing, and bounded/sanitized diagnostics (mock gpg)
(let ((mock-sym-gpg (string-append tmp "/mock-sym-gpg.sh"))
      (mock-hybrid-gpg (string-append tmp "/mock-hybrid-gpg.sh"))
      (mock-bad-status-gpg (string-append tmp "/mock-bad-status-gpg.sh"))
      (mock-noisy-fail-gpg (string-append tmp "/mock-noisy-fail-gpg.sh")))
  (make-script
   mock-sym-gpg
   "#!/bin/sh\n\
mode=''\n\
seen_no_encrypt_to=0\n\
seen_no_default=0\n\
for a in \"$@\"; do\n\
  case \"$a\" in\n\
    --decrypt) mode=d ;;\n\
    --symmetric) mode=s ;;\n\
    --no-encrypt-to) seen_no_encrypt_to=1 ;;\n\
    --no-default-recipient) seen_no_default=1 ;;\n\
  esac\n\
done\n\
if [ \"$mode\" = d ]; then\n\
  echo '[GNUPG:] NEED_PASSPHRASE_SYM 9 3 2' >&2\n\
  echo '[GNUPG:] BEGIN_DECRYPTION' >&2\n\
  echo '[GNUPG:] DECRYPTION_OKAY' >&2\n\
  echo '[GNUPG:] END_DECRYPTION' >&2\n\
  sed 's/^SYM://'\n\
  exit 0\n\
elif [ \"$mode\" = s ]; then\n\
  [ \"$seen_no_encrypt_to\" = 1 ] && [ \"$seen_no_default\" = 1 ] || { echo 'missing hardening flags' >&2; exit 2; }\n\
  echo '[GNUPG:] BEGIN_ENCRYPTION 0 9' >&2\n\
  echo '[GNUPG:] END_ENCRYPTION' >&2\n\
  printf 'SYM:'\n\
  cat\n\
  exit 0\n\
fi\n\
echo 'unexpected args' >&2\n\
exit 2\n")
  (make-script
   mock-hybrid-gpg
   "#!/bin/sh\n\
cat >/dev/null\n\
echo '[GNUPG:] ENC_TO 0123456789ABCDEF 1 0' >&2\n\
echo '[GNUPG:] NEED_PASSPHRASE_SYM 9 3 2' >&2\n\
echo '[GNUPG:] BEGIN_DECRYPTION' >&2\n\
echo '[GNUPG:] DECRYPTION_OKAY' >&2\n\
echo '[GNUPG:] END_DECRYPTION' >&2\n\
printf 'hybrid plaintext\\n'\n\
exit 0\n")
  (make-script
   mock-bad-status-gpg
   "#!/bin/sh\n\
case \"$1\" in\n\
  *)\n\
    cat >/dev/null\n\
    printf '[GNUPG:] BEGIN_DECRYPTION\\n[GNUPG:] DECRYPTION_OKAY\\n[GNUPG:] END_DECRYPTION' >&2\n\
    printf 'unterminated\\n'\n\
    exit 0 ;;\n\
esac\n")
  (make-script
   mock-noisy-fail-gpg
   "#!/bin/sh\n\
printf 'line1\\033[31mred\\033[0m\\t' >&2\n\
i=0\n\
while [ $i -lt 400 ]; do printf 'x' >&2; i=$((i+1)); done\n\
exit 2\n")
  (parameterize ((default-gpg-program mock-sym-gpg)
                 (default-gpg-recipients '()))
    (let* ((p (string-append tmp "/sym.txt.gpg"))
           (dec (codec-decode 'gpg p (string->utf8 "SYM:hello symmetric\n")))
           (env (codec-result-envelope dec))
           (ct2 (codec-encode 'gpg p "updated symmetric\n" env)))
      (check "gpg symmetric decode text" "hello symmetric\n" (codec-result-text dec))
      (check "gpg symmetric envelope mode" 'symmetric (cdr (assq 'mode env)))
      (check "gpg symmetric save-description" "symmetric" (codec-save-description (find-codec 'gpg) p env))
      (check "gpg symmetric re-encode uses --symmetric + hardening flags" "SYM:updated symmetric\n" (utf8->string ct2))))
  (parameterize ((default-gpg-program mock-hybrid-gpg)
                 (default-gpg-recipients '("alice@example.test")))
    (let* ((p (string-append tmp "/hybrid.txt.gpg"))
           (dec (codec-decode 'gpg p (string->utf8 "HYBRID")))
           (env (codec-result-envelope dec)))
      (check "gpg hybrid decode succeeds read-only" "hybrid plaintext\n" (codec-result-text dec))
      (check "gpg hybrid envelope mode is hybrid" 'hybrid (cdr (assq 'mode env)))
      (check "gpg hybrid save fails closed" #t
             (guard (e (#t (and (error-object? e)
                                (string-contains (error-object-message e) "dual public-key + symmetric")
                                #t)))
               (codec-encode 'gpg p "edited\n" env)
               #f))))
  (parameterize ((default-gpg-program mock-bad-status-gpg))
    (check "gpg unterminated trailing status line fails closed" #t
           (guard (e (#t (and (error-object? e)
                              (string-contains (error-object-message e) "verified decrypted OpenPGP message")
                              #t)))
             (codec-decode 'gpg (string-append tmp "/trunc.gpg") (string->utf8 "CT"))
             #f)))
  (parameterize ((default-gpg-program mock-noisy-fail-gpg))
    (let* ((fail-path (string-append tmp "/fail.gpg"))
           (msg (guard (e (#t (if (error-object? e) (error-object-message e) "")))
                  (codec-decode 'gpg fail-path (string->utf8 "CT"))
                  "")))
      (check "gpg error message includes fixed decrypt failure category" #t
             (and (string-contains msg "gpg: failed to decrypt ") #t))
      (check "gpg error message includes exit code" #t
             (and (string-contains msg "(exit 2)") #t))
      (check "gpg error diagnostic strips ESC control bytes" #f (string-contains msg "\x1b;"))
      (check "gpg error redacts raw helper stderr" #f (string-contains msg "line1"))
      (check "gpg error redacts noisy stderr payload" #f (string-contains msg "xxxx")))))

;;; 6. Built-in age codec (realistic mock age helper validating exact CLI & formats)
(check "age codec registered" #t (and (memq 'age (codec-names)) #t))
(let ((a (find-codec 'age)))
  (check "age is private" #t (codec-private? a))
  (check "age priority is 0" 0 (codec-priority a))
  (check "age matches .md.age" 'age (codec-name (codec-for-path "/tmp/notes.md.age")))
  (check "age matches remote .age" 'age (codec-name (codec-for-path "/ssh:box:/etc/secret.age")))
  (check "age excludes bare .age" #f (codec-for-path "/tmp/.age"))
  (check "age logical path strips .age" "/tmp/notes.md" (codec-logical-path a "/tmp/notes.md.age"))
  (check "age path-private-by-codec?" #t (path-private-by-codec? "/tmp/notes.md.age")))

(let* ((mock-age (string-append tmp "/mock-age.sh"))
       (id-file (string-append tmp "/age-key.txt"))
       (recips-file (string-append tmp "/age-recips.txt"))
       (age-root (string-append tmp "/age-vault"))
       (age-sub (string-append age-root "/sub"))
       (age-escape (string-append tmp "/age-escape"))
       (outside-recips (string-append tmp "/outside-age-recips")))
  (create-directory age-root #o700)
  (create-directory age-sub #o700)
  (create-directory age-escape #o700)
  (write-text id-file "AGE-SECRET-KEY-TEST\n")
  (write-text recips-file "# trusted recipients\nage1filekey111\n")
  (write-text (string-append age-root "/.age-recipients") "# vault recipients\nage1vaultkey222\n")
  (write-text outside-recips "age1outsidekey999\n")
  (call-with-process "ln" (list "-s" outside-recips (string-append age-escape "/.age-recipients")) (lambda (p) (process-wait p)))
  (make-script
   mock-age
   "#!/bin/sh\n\
mode=''\n\
armor=0\n\
recips=''\n\
id_ok=0\n\
while [ $# -gt 0 ]; do\n\
  case \"$1\" in\n\
    -d) mode=d; shift ;;\n\
    -e) mode=e; shift ;;\n\
    -a) armor=1; shift ;;\n\
    -i) shift; [ -f \"$1\" ] && grep -q 'AGE-SECRET-KEY-TEST' \"$1\" && id_ok=1; shift ;;\n\
    -r) shift; recips=\"$recips,$1\"; shift ;;\n\
    -R) shift; [ -f \"$1\" ] || { echo 'missing -R file' >&2; exit 2; }; recips=\"$recips,FILE:$1\"; shift ;;\n\
    *) echo \"unexpected arg: $1\" >&2; exit 2 ;;\n\
  esac\n\
done\n\
if [ \"$mode\" = e ]; then\n\
  [ -n \"$recips\" ] || { echo 'no recipients' >&2; exit 1; }\n\
  if [ \"$armor\" = 1 ]; then\n\
    printf '%s\\nAGE-HDR:%s\\n' '-----BEGIN AGE ENCRYPTED FILE-----' \"$recips\"\n\
  else\n\
    printf 'AGE-BIN:%s\\n' \"$recips\"\n\
  fi\n\
  cat\n\
  exit 0\n\
elif [ \"$mode\" = d ]; then\n\
  [ \"$id_ok\" = 1 ] || { echo 'identity did not match' >&2; exit 1; }\n\
  IFS= read -r first || { echo 'empty input' >&2; exit 1; }\n\
  if [ \"$first\" = '-----BEGIN AGE ENCRYPTED FILE-----' ]; then\n\
    IFS= read -r hdr || { echo 'truncated armored' >&2; exit 1; }\n\
    case \"$hdr\" in AGE-HDR:*) cat; exit 0 ;; *) echo 'bad armored header' >&2; exit 1 ;; esac\n\
  else\n\
    case \"$first\" in AGE-BIN:*) cat; exit 0 ;; *) echo 'not an age file' >&2; exit 1 ;; esac\n\
  fi\n\
else\n\
  echo 'missing mode' >&2; exit 2\n\
fi\n")

  ;; 6a. Binary & ASCII-armored round-trip + armor preservation
  (parameterize ((default-age-program mock-age)
                 (default-age-identities (list id-file))
                 (default-age-recipients '("age1pubkey001"))
                 (default-age-armor #f))
    (let* ((p (string-append tmp "/secret.md.age"))
           (sample "# Secret\nλ body\n")
           (ct (codec-encode 'age p sample (codec-init-envelope 'age p)))
           (dec (codec-decode 'age p ct)))
      (check "age binary decode matches plaintext" sample (codec-result-text dec))
      (check "age binary envelope has armor=#f" #f (cdr (assq 'armor (codec-result-envelope dec)))))
    (parameterize ((default-age-armor #t))
      (let* ((p (string-append tmp "/armored.md.age"))
             (ct1 (codec-encode 'age p "armored age\n" (codec-init-envelope 'age p)))
             (dec1 (codec-decode 'age p ct1))
             (env1 (codec-result-envelope dec1)))
        (check "age armored ciphertext has header" #t
               (string-prefix? "-----BEGIN AGE ENCRYPTED FILE-----" (utf8->string ct1)))
        (check "age armored decode records armor=#t" #t (cdr (assq 'armor env1)))
        (parameterize ((default-age-armor #f))
          (let* ((ct2 (codec-encode 'age p "armored age v2\n" env1))
                 (dec2 (codec-decode 'age p ct2)))
            (check "age re-encode preserves armor from envelope" #t
                   (string-prefix? "-----BEGIN AGE ENCRYPTED FILE-----" (utf8->string ct2)))
            (check "age armored v2 round-trips" "armored age v2\n" (codec-result-text dec2)))))))

  ;; 6b. Read-only decrypt works without recipients; encode fails closed without recipients
  (parameterize ((default-age-program mock-age)
                 (default-age-identities (list id-file))
                 (default-age-recipients '("age1pubkey001")))
    (let* ((p (string-append tmp "/ro.txt.age"))
           (ct (codec-encode 'age p "read only text\n" #f)))
      (parameterize ((default-age-recipients '())
                     (default-age-recipients-file #f)
                     (default-age-recipient-roots '()))
        (let ((dec (codec-decode 'age p ct)))
          (check "age read-only decrypt succeeds without recipients" "read only text\n" (codec-result-text dec))
          (check "age encode fails closed without recipients" #t
                 (guard (e (#t (and (error-object? e)
                                    (string-contains (error-object-message e) "no recipients")
                                    #t)))
                   (codec-encode 'age p "edited\n" (codec-result-envelope dec))
                   #f))))))

  ;; 6c. default-age-recipients-file and opt-in .age-recipients (with symlink escape rejection)
  (parameterize ((default-age-program mock-age)
                 (default-age-identities (list id-file))
                 (default-age-recipients '())
                 (default-age-recipients-file recips-file))
    (let* ((p (string-append tmp "/by-file.txt.age"))
           (ct (codec-encode 'age p "via recips file\n" (codec-init-envelope 'age p)))
           (dec (codec-decode 'age p ct)))
      (check "age recipients-file round-trips" "via recips file\n" (codec-result-text dec))))

  (parameterize ((default-age-program mock-age)
                 (default-age-identities (list id-file))
                 (default-age-recipients '())
                 (default-age-recipients-file #f)
                 (default-age-recipient-roots '()))
    (check ".age-recipients ignored when default-age-recipient-roots is empty" #t
           (guard (e (#t (error-object? e)))
             (codec-encode 'age (string-append age-sub "/vault.txt.age") "v\n" #f)
             #f)))

  (parameterize ((default-age-program mock-age)
                 (default-age-identities (list id-file))
                 (default-age-recipients '())
                 (default-age-recipients-file #f)
                 (default-age-recipient-roots (list age-root)))
    (let* ((p (string-append age-sub "/vault.txt.age"))
           (env (codec-init-envelope 'age p))
           (ct (codec-encode 'age p "via vault\n" env))
           (dec (codec-decode 'age p ct)))
      (check "age save-description reports .age-recipients source"
             "1 recipient from .age-recipients"
             (codec-save-description (find-codec 'age) p env))
      (check "opted-in .age-recipients round-trips" "via vault\n" (codec-result-text dec))))

  (parameterize ((default-age-program mock-age)
                 (default-age-identities (list id-file))
                 (default-age-recipients '())
                 (default-age-recipients-file #f)
                 (default-age-recipient-roots (list age-escape)))
    (check ".age-recipients symlink escaping allowed root is rejected" #t
           (guard (e (#t (and (error-object? e)
                              (string-contains (error-object-message e) "outside allowed root")
                              #t)))
             (codec-init-envelope 'age (string-append age-escape "/x.age"))
             #f)))

  (let ((age-root-alias (string-append tmp "/age-root-alias")))
    (call-with-process "ln" (list "-s" age-root age-root-alias) (lambda (p) (process-wait p)))
    (parameterize ((default-age-program mock-age)
                   (default-age-identities (list id-file))
                   (default-age-recipients '())
                   (default-age-recipients-file #f)
                   (default-age-recipient-roots (list age-root-alias)))
      (check "age discovery refuses symlinked allowlist root via alias path" #t
             (guard (e (#t (error-object? e)))
               (codec-init-envelope 'age (string-append age-root-alias "/sub/vault.txt.age"))
               #f))
      (check "age encode fails closed for symlinked allowlist root via alias path" #t
             (guard (e (#t (error-object? e)))
               (codec-encode 'age (string-append age-root-alias "/sub/vault.txt.age") "v\n" #f)
               #f))
      (check "age discovery does not match canonical target of symlinked allowlist root" #t
             (guard (e (#t (and (error-object? e)
                                (string-contains (error-object-message e) "no recipients")
                                #t)))
               (codec-encode 'age (string-append age-sub "/vault.txt.age") "v\n" #f)
               #f)))
    (parameterize ((default-age-program mock-age)
                   (default-age-identities (list id-file))
                   (default-age-recipients '())
                   (default-age-recipients-file #f)
                   (default-age-recipient-roots (list (string-append age-root-alias "/sub"))))
      (check "age discovery refuses symlinked ancestor of allowlist root" #t
             (guard (e (#t (error-object? e)))
               (codec-init-envelope 'age (string-append age-root-alias "/sub/vault.txt.age"))
               #f))))

  ;; 6d. Fail-closed edge cases: 0-byte file, absent file, missing/directory identity, dash-prefixed recipient
  (parameterize ((default-age-program mock-age)
                 (default-age-identities (list id-file))
                 (default-age-recipients '("age1pubkey001")))
    (check "age save-description reports explicit age-recipients"
           "1 recipient from age-recipients"
           (codec-save-description (find-codec 'age) (string-append tmp "/new.age") #f))
    (check "age absent file (#f) decodes to empty string" ""
           (codec-result-text (codec-decode 'age (string-append tmp "/new.age") #f)))
    (check "age 0-byte existing file (#u8()) fails closed" #t
           (guard (e (#t (and (error-object? e)
                              (string-contains (error-object-message e) "empty file is not valid encrypted data")
                              #t)))
             (codec-decode 'age (string-append tmp "/zero.age") #u8())
             #f)))
  (parameterize ((default-age-program mock-age)
                 (default-age-identities (list (string-append tmp "/no-such-key.txt"))))
    (check "age missing identity fails closed" #t
           (guard (e (#t (and (error-object? e)
                              (string-contains (error-object-message e) "identity file is missing or not a regular file")
                              #t)))
             (codec-decode 'age (string-append tmp "/x.age") (string->utf8 "AGE-BIN:,k\nhi\n"))
             #f)))
  (parameterize ((default-age-program mock-age)
                 (default-age-identities (list age-root)))
    (check "age directory identity fails closed" #t
           (guard (e (#t (and (error-object? e)
                              (string-contains (error-object-message e) "identity file is missing or not a regular file")
                              #t)))
             (codec-decode 'age (string-append tmp "/x.age") (string->utf8 "AGE-BIN:,k\nhi\n"))
             #f)))
  (parameterize ((default-age-program mock-age)
                 (default-age-recipients '("-o/tmp/pwn")))
    (check "age rejects dash-prefixed recipient flag injection" #t
           (guard (e (#t (and (error-object? e)
                              (string-contains (error-object-message e) "must not start with '-'")
                              #t)))
             (codec-encode 'age (string-append tmp "/flag.age") "hi\n" #f)
             #f)))

  ;; 6e. age helper stderr is redacted on both decrypt and encrypt failures
  (let ((mock-leaky-age (string-append tmp "/mock-leaky-age.sh")))
    (make-script mock-leaky-age
                 "#!/bin/sh\necho 'SENSITIVE-AGE-STDERR-MARKER' >&2\nexit 3\n")
    (parameterize ((default-age-program mock-leaky-age)
                   (default-age-identities (list id-file))
                   (default-age-recipients '("age1pubkey001")))
      (let ((dec-msg (guard (e (#t (if (error-object? e) (error-object-message e) "")))
                       (codec-decode 'age (string-append tmp "/leak.age") (string->utf8 "AGE-BIN:,k\nhi\n"))
                       ""))
            (enc-msg (guard (e (#t (if (error-object? e) (error-object-message e) "")))
                       (codec-encode 'age (string-append tmp "/leak.age") "hi\n" #f)
                       "")))
        (check "age decrypt failure includes fixed category" #t
               (and (string-contains dec-msg "age: failed to decrypt ") #t))
        (check "age decrypt failure redacts raw stderr" #f
               (string-contains dec-msg "SENSITIVE-AGE-STDERR-MARKER"))
        (check "age encrypt failure includes fixed category" #t
               (and (string-contains enc-msg "age: failed to encrypt ") #t))
        (check "age encrypt failure redacts raw stderr" #f
               (string-contains enc-msg "SENSITIVE-AGE-STDERR-MARKER"))))))

;;; 7. Automatic selection protects built-in private codecs against differently-named overrides
(let ((sc-child (make-scope 'ext-overlap-private)))
  (with-scope sc-child
    (register-codec! 'evil-plain-age "Unauthorized high-priority pass-through matching .age."
                     #:match (lambda (p) (string-suffix? ".age" p))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #f
                     #:priority 500)
    (register-codec! 'evil-priv-gpg "Unauthorized high-priority private pass-through matching .gpg."
                     #:match (lambda (p) (string-suffix? ".gpg" p))
                     #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                     #:encode (lambda (_p text _env) (string->utf8 text))
                     #:private #t
                     #:priority 500))
  (check "child non-private high-priority codec cannot displace built-in age"
         'age (codec-name (codec-for-path "/tmp/new-secret.age")))
  (check "child private high-priority codec cannot displace built-in gpg"
         'gpg (codec-name (codec-for-path "/tmp/new-secret.gpg")))
  (scope-shutdown! sc-child))

;; Even in root scope, a differently-named codec without #:override-builtin-private? #t
;; cannot displace built-in age or gpg on automatic selection.
(register-codec! 'root-unauth-age "Root-scope codec without explicit override flag."
                 #:match (lambda (p) (string-suffix? "/root-unauth.age" p))
                 #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                 #:encode (lambda (_p text _env) (string->utf8 text))
                 #:private #t
                 #:priority 500)
(check "root codec without #:override-builtin-private? #t cannot displace built-in age"
       'age (codec-name (codec-for-path "/tmp/root-unauth.age")))

;; Explicit root-scope private override IS honored for automatic selection.
(register-codec! 'root-explicit-age "Explicitly authorized root-scope private override."
                 #:match (lambda (p) (string-suffix? "/root-explicit.age" p))
                 #:decode (lambda (_p raw) (if raw (utf8->string raw) ""))
                 #:encode (lambda (_p text _env) (string->utf8 text))
                 #:private #t
                 #:priority 500
                 #:override-builtin-private? #t)
(check "root codec with #:private #t #:override-builtin-private? #t can override built-in age"
       'root-explicit-age (codec-name (codec-for-path "/tmp/root-explicit.age")))

(rm-rf tmp)
(test-summary)
