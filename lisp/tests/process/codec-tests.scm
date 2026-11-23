;;; Tests for `(techne codec)` and binary `run-trusted-tool-bytes`.
(import (techne) (techne codec) (techne trusted-process) (techne task) (techne uuid))
(require "../../test.scm")

(define tmp (string-append (or (get-environment-variable "TMPDIR") "/tmp") "/techne-codec-" (uuid-v4)))
(create-directory tmp #o700)

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

(rm-rf tmp)
(test-summary)
