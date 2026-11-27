;;; Headless tests for `(techne editorconfig)` and `(techne project-settings)`.

(import (techne) (techne path) (techne uuid) (techne editorconfig) (techne project-settings))
(import (techne test))
(test-file #:script #t)

(define (assert-true label v)
  (check label #t (and v #t)))

(define (assert-false label v)
  (check label #f (and v #t)))

(define (assert-err label thunk)
  (check label #t (guard (e (#t #t)) (thunk) #f)))

(define (write-text path text)
  (call-with-output-file path (lambda (p) (display text p))))

(define (make-temp-tree prefix)
  (let* ((base (string-append (or (get-environment-variable "TMPDIR") "/tmp")
                              "/techne-ps-test-" prefix "-" (uuid-v4))))
    (make-directory base)
    (let ((canon (project-settings-canonical-dir base)))
      (if (and (> (string-length canon) 1) (string-suffix? "/" canon))
          (substring canon 0 (- (string-length canon) 1))
          canon))))

(define (cleanup-tree dir)
  (guard (e (#t #f)) (delete-path dir #t)))

;;; 1. EditorConfig glob matching

(assert-true "glob * matches basename" (editorconfig-glob-match? "*" "foo.rs"))
(assert-true "glob * matches nested" (editorconfig-glob-match? "*" "a/b/foo.rs"))
(assert-true "glob *.rs matches root" (editorconfig-glob-match? "*.rs" "main.rs"))
(assert-true "glob *.rs matches nested" (editorconfig-glob-match? "*.rs" "src/lib/main.rs"))
(assert-false "glob *.rs rejects .bak" (editorconfig-glob-match? "*.rs" "main.rs.bak"))
(assert-true "glob src/*.rs matches direct child" (editorconfig-glob-match? "src/*.rs" "src/main.rs"))
(assert-false "glob src/*.rs rejects deeper child" (editorconfig-glob-match? "src/*.rs" "src/a/main.rs"))
(assert-true "glob src/**/*.rs matches deep child" (editorconfig-glob-match? "src/**/*.rs" "src/a/b/main.rs"))
(assert-true "glob /Makefile matches root" (editorconfig-glob-match? "/Makefile" "Makefile"))
(assert-false "glob /Makefile rejects nested" (editorconfig-glob-match? "/Makefile" "sub/Makefile"))
(assert-true "glob ? matches char" (editorconfig-glob-match? "a?c" "abc"))
(assert-false "glob ? rejects slash" (editorconfig-glob-match? "a?c" "a/c"))
(assert-true "glob class range matches" (editorconfig-glob-match? "[a-c]1" "b1"))
(assert-false "glob negated class rejects" (editorconfig-glob-match? "[!a-c]1" "b1"))
(assert-true "glob negated class matches" (editorconfig-glob-match? "[!a-c]1" "z1"))
(assert-true "glob brace alternation matches" (editorconfig-glob-match? "*.{js,ts,tsx}" "app.tsx"))
(assert-false "glob brace alternation rejects" (editorconfig-glob-match? "*.{js,ts,tsx}" "app.jsx"))
(assert-true "glob nested braces match" (editorconfig-glob-match? "{a,{b,c}}.txt" "c.txt"))
(assert-true "glob numeric range matches" (editorconfig-glob-match? "file{1..12}.txt" "file10.txt"))
(assert-false "glob numeric range rejects out of bounds" (editorconfig-glob-match? "file{1..12}.txt" "file13.txt"))
(assert-true "glob signed numeric range matches" (editorconfig-glob-match? "n{-3..2}.txt" "n-2.txt"))
(assert-true "glob single brace literal" (editorconfig-glob-match? "{single}" "{single}"))
(assert-true "glob escaped star matches literal" (editorconfig-glob-match? "a\\*b" "a*b"))
(assert-false "glob escaped star rejects wildcard" (editorconfig-glob-match? "a\\*b" "axxb"))
(assert-false "glob pathological stars stay fast" (editorconfig-glob-match? "*a*a*a*a*a*a*a*a*b" "aaaaaaaaaaaaaaaaaaaaaaaaaa"))

;;; 2. EditorConfig parsing, root=true, unset, and option mapping

(let ((root (make-temp-tree "ec")))
  (dynamic-wind
    (lambda () #f)
    (lambda ()
      (make-directory (string-append root "/repo/.git"))
      (make-directory (string-append root "/repo/sub"))
      ;; Parent above .git has root=true: EditorConfig MUST NOT stop at .git!
      (write-text (string-append root "/.editorconfig")
                  (string-append (string (integer->char #xFEFF))
                                 ";root = true\r\nroot = true\r\n\r\n[*]\r\n"
                                 "indent_style = space\r\nindent_size = 4\r\n"
                                 "trim_trailing_whitespace = true\r\n"
                                 "insert_final_newline = true\r\nmax_line_length = 100\r\n"))
      (write-text (string-append root "/repo/.editorconfig")
                  "# comment\n; semicolon comment\n[*.rs]\nindent_size = 2\n[*.md]\ntrim_trailing_whitespace = unset\nmax_line_length = off\n")
      (write-text (string-append root "/repo/sub/.editorconfig")
                  "[Makefile]\nindent_style = tab\nindent_size = tab\ntab_width = 8\nend_of_line = lf\n")

      (editorconfig-invalidate!)
      (let* ((rs-file (string-append root "/repo/sub/lib.rs"))
             (rs-props (editorconfig-resolve rs-file))
             (rs-opts (editorconfig-properties->options rs-props)))
        (check "ec rs indent-tabs" #f (cdr (assoc 'indent-tabs rs-opts)))
        (check "ec rs indent-width" 2 (cdr (assoc 'indent-width rs-opts)))
        (check "ec rs tab-width inherits indent-size" 2 (cdr (assoc 'tab-width rs-opts)))
        (check "ec rs trim-trailing-whitespace from ancestor above .git" #t (cdr (assoc 'trim-trailing-whitespace rs-opts)))
        (check "ec rs insert-final-newline" #t (cdr (assoc 'insert-final-newline rs-opts)))
        (check "ec rs fill-column" 100 (cdr (assoc 'fill-column rs-opts))))

      (let* ((md-file (string-append root "/repo/sub/README.md"))
             (md-props (editorconfig-resolve md-file))
             (md-opts (editorconfig-properties->options md-props)))
        (assert-false "ec md unset removes trim_trailing_whitespace" (assoc "trim_trailing_whitespace" md-props))
        (assert-false "ec md unset removes option" (assoc 'trim-trailing-whitespace md-opts))
        (check "ec md max_line_length=off" #f (cdr (assoc 'fill-column md-opts)))
        (check "ec md indent-width inherited" 4 (cdr (assoc 'indent-width md-opts))))

      (let* ((mk-file (string-append root "/repo/sub/Makefile"))
             (mk-opts (editorconfig-properties->options (editorconfig-resolve mk-file))))
        (check "ec Makefile indent-tabs" #t (cdr (assoc 'indent-tabs mk-opts)))
        (check "ec Makefile indent-width=tab uses tab_width" 8 (cdr (assoc 'indent-width mk-opts)))
        (check "ec Makefile tab-width" 8 (cdr (assoc 'tab-width mk-opts)))
        (check "ec Makefile end-of-line" 'lf (cdr (assoc 'end-of-line mk-opts)))))
    (lambda ()
      (editorconfig-invalidate!)
      (cleanup-tree root))))

;;; 3. `.techne.scm` pure data reader (NO eval!) and diagnostics

(let ((sf (parse-project-settings-string
           "(techne-settings 1
              (root #t)
              (all (fill-column 100) (bad-expr (+ 1 2)))
              (mode rust-mode (indent-width 4))
              (files \"docs/**/*.md\" (word-wrap #t))
              (compile-command \"cargo xtask test\")
              (lsp-settings (dict \"rust-analyzer\" (dict \"cargo\" (dict \"features\" #(\"xtask\")) \"check\" (unset))))
              (bogus-clause a b c))"
           #:path "/proj/.techne.scm"
           #:directory "/proj/")))
  (assert-true "parsed project-settings-file?" (project-settings-file? sf))
  (check "settings version" 1 (project-settings-file-version sf))
  (check "settings root?" #t (project-settings-file-root? sf))
  (check "one diagnostic for bogus clause" 1 (length (project-settings-file-diagnostics sf)))
  (let ((bad-rule (find (lambda (r) (eq? (settings-rule-option r) 'bad-expr))
                        (project-settings-file-rules sf))))
    (assert-true "bad-expr rule parsed" bad-rule)
    (check "read does NOT eval (+ 1 2)" '(+ 1 2) (settings-rule-value bad-rule)))
  (let ((lsp-rule (find (lambda (r) (eq? (settings-rule-option r) 'lsp-settings))
                        (project-settings-file-rules sf))))
    (assert-true "lsp-settings rule parsed" lsp-rule)
    (let* ((d (settings-rule-value lsp-rule))
           (ra (dict-ref d "rust-analyzer")))
      (assert-true "lsp-settings value is dict" (dict? d))
      (assert-true "nested dict parsed" (dict? ra))
      (assert-true "unset sentinel preserved" (json-unset? (dict-ref ra "check")))
      (check "nested vector preserved" #("xtask") (dict-ref (dict-ref ra "cargo") "features")))))

(let ((sf (parse-project-settings-string "(techne-settings 2 (all (fill-column 80)))")))
  (assert-true "unsupported version yields zero rules" (null? (project-settings-file-rules sf)))
  (assert-true "unsupported version records diagnostic" (pair? (project-settings-file-diagnostics sf))))

;;; 4. JSON-object overlay semantics

(let* ((base (dict "rust-analyzer"
                   (dict "check" (dict "command" "clippy" "extraArgs" #("--all"))
                         "inlayHints" (dict "enable" #t)
                         "keep" "yes")))
       (ov1 (dict "rust-analyzer"
                  (dict "check" (dict "extraArgs" #("--tests") "nullable" 'null)
                        "inlayHints" json-unset)))
       (ov2 (dict "rust-analyzer"
                  (dict "cargo" (dict "features" #("xtask")))))
       (composed (json-object-compose-overlays ov1 ov2))
       (merged (json-object-overlay base composed))
       (ra (dict-ref merged "rust-analyzer")))
  (assert-true "composed overlay keeps json-unset" (json-unset? (dict-ref (dict-ref composed "rust-analyzer") "inlayHints")))
  (check "overlay preserves sibling dict key" "clippy" (dict-ref (dict-ref ra "check") "command"))
  (check "overlay replaces array wholesale" #("--tests") (dict-ref (dict-ref ra "check") "extraArgs"))
  (check "overlay preserves JSON null" 'null (dict-ref (dict-ref ra "check") "nullable"))
  (assert-false "overlay removes json-unset key" (dict-contains? ra "inlayHints"))
  (check "overlay preserves untouched key" "yes" (dict-ref ra "keep"))
  (check "overlay adds new nested dict" #("xtask") (dict-ref (dict-ref ra "cargo") "features")))

;;; 5. Precedence collisions, value-based trust, fragment composition, principal checks, zero-I/O

(let ((root (make-temp-tree "ps")))
  (dynamic-wind
    (lambda () #f)
    (lambda ()
      (project-trust-clear-memory!)
      (project-settings-invalidate!)
      (make-directory (string-append root "/repo/.git"))
      (make-directory (string-append root "/repo/sub"))
      ;; Parent outside .git should NOT be read by .techne.scm discovery
      (write-text (string-append root "/.techne.scm")
                  "(techne-settings 1 (all (fill-column 42)))")
      (write-text (string-append root "/repo/.editorconfig")
                  "root = true\n[*]\nindent_size = 2\nmax_line_length = 90\n")
      (write-text (string-append root "/repo/.techne.scm")
                  "(techne-settings 1
                     (all (fill-column 100)
                          (compile-command \"cargo build\")
                          (secret-sources ((env #:var \"BAD\")))
                          (lsp-settings (dict \"ra\" (dict \"a\" 1))))
                     (mode rust-mode (indent-width 4))
                     (files \"**/*.rs\" (tab-width 4)))")
      (write-text (string-append root "/repo/sub/.editorconfig")
                  "[*]\nmax_line_length = 72\nindent_size = unset\n")
      (write-text (string-append root "/repo/sub/.techne.scm")
                  "(techne-settings 1
                     (all (indent-width 8)
                          (lsp-settings (dict \"ra\" (dict \"b\" 2)))))")

      (let* ((sub-dir (string-append root "/repo/sub/"))
             (repo-dir (string-append root "/repo/"))
             (rs-file (string-append sub-dir "main.rs"))
             (snap (project-settings-load! sub-dir)))

        (check "discovery stops at VC root" 2 (length (project-settings-snapshot-techne-files snap)))

        (call-with-values
         (lambda ()
           (project-settings-resolve-option snap rs-file '(rust-mode prog-mode) '() 'indent-width #:safe #t))
         (lambda (applied pending rejected)
           (assert-true "safe option no pending" (null? pending))
           (assert-true "safe option no rejected" (null? rejected))
           (check "nearer dir all beats farther dir mode" 8 (list-ref (car applied) 3))))

        (call-with-values
         (lambda ()
           (project-settings-resolve-option snap rs-file '(rust-mode) '() 'fill-column #:safe #t))
         (lambda (applied pending rejected)
           (check "nearer editorconfig beats farther .techne.scm" 72 (list-ref (car applied) 3))
           (check "winner source kind is editorconfig" 'editorconfig (list-ref (car applied) 1))))

        (call-with-values
         (lambda ()
           (project-settings-resolve-option
            snap rs-file '(rust-mode)
            (list (list sub-dir #f 120 'root))
            'fill-column #:safe #t))
         (lambda (applied pending rejected)
           (check "user dir override beats project files" 120 (list-ref (car applied) 3))
           (check "winner source kind is user" 'user (list-ref (car applied) 1))))

        (call-with-values
         (lambda ()
           (project-settings-resolve-option snap rs-file '(rust-mode) '() 'secret-sources
                                            #:user-only? #t #:safe #t))
         (lambda (applied pending rejected)
           (assert-true "user-only option not applied" (null? applied))
           (assert-true "user-only option not pending" (null? pending))
           (check "user-only option rejected" 1 (length rejected))))

        (call-with-values
         (lambda ()
           (project-settings-resolve-option snap rs-file '(rust-mode) '() 'compile-command #:safe #f))
         (lambda (applied pending rejected)
           (assert-true "unapproved risky option inert" (null? applied))
           (check "unapproved risky option pending" 1 (length pending))
           (check "verdict is unknown" 'unknown (list-ref (car pending) 4))))

        (set-trust-principal-hook! (lambda () 'agent))
        (assert-err "agent cannot approve trust" (lambda () (project-trust-approve! repo-dir 'compile-command "cargo build")))
        (assert-err "agent cannot deny trust" (lambda () (project-trust-deny! repo-dir 'compile-command "cargo build")))
        (assert-err "agent cannot revoke trust" (lambda () (project-trust-revoke! repo-dir 'compile-command)))
        (set-trust-principal-hook! (lambda () 'user))

        (let ((store-path (string-append root "/trust.scm")))
          (parameterize ((default-trust-store-path store-path))
            (project-trust-approve! repo-dir 'compile-command "cargo build")
            (call-with-values
             (lambda ()
               (project-settings-resolve-option snap rs-file '(rust-mode) '() 'compile-command #:safe #f))
             (lambda (applied pending rejected)
               (assert-true "approved risky option no pending" (null? pending))
               (check "approved risky option applied" "cargo build" (list-ref (car applied) 3))))
            (check "trust file mode is 0600" #o600 (modulo (list-ref (file-attributes store-path) 3) #o1000))
            (project-trust-approve! repo-dir 'compile-command "cargo test")
            (check "trust file mode stays 0600 after overwrite" #o600 (modulo (list-ref (file-attributes store-path) 3) #o1000))
            (project-trust-clear-memory!)
            (check "trust verdict is zero-I/O when memory cleared" 'unknown
                   (project-trust-verdict repo-dir 'compile-command "cargo build"))
            (project-trust-load! store-path)
            (check "trust verdict 1 survives reload" 'trusted (project-trust-verdict repo-dir 'compile-command "cargo build"))
            (check "trust verdict 2 survives reload" 'trusted (project-trust-verdict repo-dir 'compile-command "cargo test"))
            (check "revoke returns removed entry count" 2 (project-trust-revoke! repo-dir 'compile-command))
            (check "second revoke removes 0 entries" 0 (project-trust-revoke! repo-dir 'compile-command))
            ;; Corrupt trust store must raise instead of silently clobbering
            (replace-file store-path "(not-valid-trust-header")
            (assert-err "corrupt trust store raises on load" (lambda () (project-trust-load! store-path)))
            (assert-err "corrupt trust store blocks approve without clobbering"
                        (lambda () (project-trust-approve! repo-dir 'compile-command "cargo fmt")))))

        ;; JSON-object fragment composition defense
        (project-trust-approve! repo-dir 'lsp-settings (dict "ra" (dict "a" 1)))
        (project-trust-approve! sub-dir 'lsp-settings (dict "ra" (dict "b" 2)))
        (call-with-values
         (lambda ()
           (project-settings-resolve-option snap rs-file '(rust-mode) '() 'lsp-settings
                                            #:safe #f #:merge 'json-object))
         (lambda (applied pending rejected)
           (assert-true "unapproved combined overlay stays inert" (null? applied))
           (check "combined overlay pending" 1 (length pending))
           (let ((eff (list-ref (car pending) 2)))
             (check "effective overlay has a=1" 1 (dict-ref (dict-ref eff "ra") "a"))
             (check "effective overlay has b=2" 2 (dict-ref (dict-ref eff "ra") "b"))
             (project-trust-approve! (list-ref (car pending) 0) 'lsp-settings eff))))
        (call-with-values
         (lambda ()
           (project-settings-resolve-option snap rs-file '(rust-mode) '() 'lsp-settings
                                            #:safe #f #:merge 'json-object))
         (lambda (applied pending rejected)
           (assert-true "approved combined overlay no pending" (null? pending))
           (check "approved combined overlay applies both layers" 2 (length applied))))

        ;; Zero-I/O check & ancestor invalidation
        (delete-path (string-append sub-dir ".techne.scm") #f)
        (let ((cached (project-settings-cached-snapshot sub-dir)))
          (assert-true "cached snapshot survives disk delete" cached)
          (check "cached snapshot keeps 2 files without I/O" 2 (length (project-settings-snapshot-techne-files cached))))
        (project-settings-invalidate! sub-dir)
        (assert-false "invalidate clears cached snapshot" (project-settings-cached-snapshot sub-dir))
        (let ((reloaded (project-settings-load! sub-dir)))
          (check "reloaded snapshot sees 1 file" 1 (length (project-settings-snapshot-techne-files reloaded))))

        (replace-file (string-append repo-dir ".techne.scm")
                      "(techne-settings 1 (root #t) (all (indent-size 2)))")
        (project-settings-invalidate-ancestors! sub-dir)
        (assert-false "ancestor invalidate clears cached snapshot" (project-settings-cached-snapshot sub-dir))
        (let ((reloaded (project-settings-load! sub-dir)))
          (call-with-values
           (lambda () (project-settings-resolve-option reloaded rs-file '(rust-mode) '() 'indent-size #:safe #t))
           (lambda (applied pending rejected)
             (check "ancestor invalidation reloads edited parent file" 2 (list-ref (car applied) 3)))))

        ;; Winning-only replace rule evaluation (shadowed trusted rule must not activate)
        (replace-file (string-append repo-dir ".techne.scm")
                      "(techne-settings 1 (root #t) (all (compile-command \"trusted-root\")))")
        (replace-file (string-append sub-dir ".techne.scm")
                      "(techne-settings 1 (all (compile-command \"untrusted-sub\")))")
        (project-trust-clear-memory!)
        (project-trust-approve! repo-dir 'compile-command "trusted-root")
        (project-settings-invalidate-ancestors! sub-dir)
        (let ((snap2 (project-settings-load! sub-dir)))
          (call-with-values
           (lambda () (project-settings-resolve-option snap2 rs-file '(rust-mode) '() 'compile-command #:safe #f))
           (lambda (applied pending rejected)
             (assert-true "shadowed trusted root rule does not activate when subdir rule is untrusted" (null? applied))
             (check "winning untrusted subdir rule is pending" "untrusted-sub" (list-ref (car pending) 2)))))

        ;; Collect-all-pending discovers mode and files selectors
        (replace-file (string-append repo-dir ".techne.scm")
                      "(techne-settings 1 (root #t)
                         (mode rust-mode (compile-command \"cargo check\"))
                         (files \"*.py\" (compile-command \"pytest\")))")
        (delete-path (string-append sub-dir ".techne.scm") #f)
        (project-trust-clear-memory!)
        (project-settings-invalidate-ancestors! sub-dir)
        (let* ((snap3 (project-settings-load! repo-dir))
               (all-pend (project-settings-collect-all-pending
                          snap3
                          (lambda (opt) (values #t #f #f (lambda (v) #t) 'replace)))))
          (check "collect-all-pending finds mode and files rules" 2 (length all-pend)))

        ;; Security hardening tests: DAG bomb, (unset) placement, digest escaping, glob step budget
        (let ((dag-sf (parse-project-settings-string
                       "(techne-settings 1 (all (x #0=(#1=(#2=(#3=(#4=(#5=(#6=(#7=(#8=(#9=(#10=(#11=(#12=(#13=(#14=(#15=(#16=(1 1) #16#) #15#) #14#) #13#) #12#) #11#) #10#) #9#) #8#) #7#) #6#) #5#) #4#) #3#) #2#) #1#) #0#))))")))
          (assert-true "DAG bomb datum rejected with diagnostic" (pair? (project-settings-file-diagnostics dag-sf)))
          (assert-true "DAG bomb produces zero rules" (null? (project-settings-file-rules dag-sf))))

        (let ((unset-sf (parse-project-settings-string
                         "(techne-settings 1 (all (x (unset)) (y ((unset))) (z (dict \"k\" #((dict \"x\" (unset))))) (w (dict \"k\" ((dict \"x\" (unset)))))))")))
          (check "non-dict and container-nested (unset) rejected with 4 diagnostics" 4 (length (project-settings-file-diagnostics unset-sf)))
          (assert-true "non-dict and container-nested (unset) produce zero rules" (null? (project-settings-file-rules unset-sf))))

        ;; Cross-binding DAG bomb hits per-file node budget (shared across bindings)
        (let ((multi-dag-sf (parse-project-settings-string
                             "(techne-settings 1 (all (a #0=(#1=(#2=(#3=(#4=(#5=(#6=(#7=(#8=(#9=(#10=(#11=(#12=(1 1) #12#) #11#) #10#) #9#) #8#) #7#) #6#) #5#) #4#) #3#) #2#) #1#) #0#)) (b #0#) (c #0#) (d #0#) (e #0#) (f #0#) (g #0#) (h #0#)))")))
          (assert-true "cross-binding DAG bomb hits shared file budget" (pair? (project-settings-file-diagnostics multi-dag-sf))))

        ;; Over-nested braces in .editorconfig skip malformed section cleanly without aborting valid sections
        (let* ((ec-nested (parse-editorconfig-string
                           "root = true\n[{{{{{{{{{{{{{{{{{{a}}}}}}}}}}}}}}}}}}]\nindent_size = 9\n[*.rs]\nindent_size = 2\n"
                           #:directory "/repo/"))
               (ec-props (editorconfig-resolve "/repo/main.rs" #:files (list ec-nested))))
          (check "over-nested .editorconfig section skipped, valid section still resolves"
                 "2" (cdr (assoc "indent_size" ec-props))))

        ;; Memory-only ('none) trust decisions must stay isolated from file-backed ledgers
        ;; and never be persisted when switching back to a file-backed store.
        (let ((iso-store (string-append root "/iso-trust.scm")))
          (project-trust-clear-memory!)
          (parameterize ((default-trust-store-path iso-store))
            (project-trust-approve! repo-dir 'compile-command "file-entry-A")
            (check "file-entry-A trusted under file-backed store" 'trusted
                   (project-trust-verdict repo-dir 'compile-command "file-entry-A"))
            ;; Temporarily switch to memory-only ('none) and approve memory-only entry B.
            (parameterize ((default-trust-store-path 'none))
              (check "file-entry-A not leaked into 'none mode" 'unknown
                     (project-trust-verdict repo-dir 'compile-command "file-entry-A"))
              (assert-true "'none mode starts with empty entries" (null? (project-trust-entries)))
              (project-trust-approve! repo-dir 'compile-command "mem-only-entry-B")
              (check "mem-only-entry-B trusted inside 'none mode" 'trusted
                     (project-trust-verdict repo-dir 'compile-command "mem-only-entry-B")))
            ;; Back on file-backed store: B must not be trusted, listed, or persisted on next write.
            (check "file-entry-A still trusted after returning from 'none" 'trusted
                   (project-trust-verdict repo-dir 'compile-command "file-entry-A"))
            (check "mem-only-entry-B unknown after returning to file-backed store" 'unknown
                   (project-trust-verdict repo-dir 'compile-command "mem-only-entry-B"))
            (check "file-backed entries contain only A before C is approved" 1
                   (length (project-trust-entries)))
            (project-trust-approve! repo-dir 'compile-command "file-entry-C")
            (check "mem-only-entry-B still unknown after file-backed persist" 'unknown
                   (project-trust-verdict repo-dir 'compile-command "mem-only-entry-B"))
            (check "file-backed entries contain exactly A and C" 2
                   (length (project-trust-entries)))
            ;; Reload fresh from disk to prove B was never written to iso-store.
            (project-trust-clear-memory!)
            (project-trust-load! iso-store)
            (check "reloaded disk ledger trusts A" 'trusted
                   (project-trust-verdict repo-dir 'compile-command "file-entry-A"))
            (check "reloaded disk ledger trusts C" 'trusted
                   (project-trust-verdict repo-dir 'compile-command "file-entry-C"))
            (check "reloaded disk ledger does not trust mem-only B" 'unknown
                   (project-trust-verdict repo-dir 'compile-command "mem-only-entry-B"))
            (check "reloaded disk ledger has exactly 2 entries" 2
                   (length (project-trust-entries)))))

        ;; Corrupt or symlinked trust ledger reload must fail closed (never keep authorizing stale verdicts)
        (let ((fc-store (string-append root "/fail-closed-trust.scm"))
              (fc-target (string-append root "/symlink-target-trust.scm")))
          (parameterize ((default-trust-store-path fc-store))
            (project-trust-approve! repo-dir 'compile-command "cargo check")
            (check "approved value initially trusted" 'trusted
                   (project-trust-verdict repo-dir 'compile-command "cargo check"))
            ;; Warm snapshot cache for repo-dir, then corrupt ledger on disk.
            (project-settings-load! repo-dir)
            (replace-file fc-store "(corrupt-ledger")
            ;; Reloading an already-cached directory still detects trust store corruption and fails closed.
            (project-settings-load! repo-dir)
            (check "corrupt ledger reload fails closed to unknown" 'unknown
                   (project-trust-verdict repo-dir 'compile-command "cargo check"))
            ;; Restoring a valid ledger recovers cleanly.
            (let* ((digest (value-trust-digest repo-dir 'compile-command "cargo check"))
                   (valid-trusted (string-append "(techne-project-trust 1\n  ("
                                                 (call-with-output-string (lambda (p) (write repo-dir p)))
                                                 " compile-command "
                                                 (call-with-output-string (lambda (p) (write digest p)))
                                                 " trusted))\n"))
                   (valid-denied (string-append "(techne-project-trust 1\n  ("
                                                (call-with-output-string (lambda (p) (write repo-dir p)))
                                                " compile-command "
                                                (call-with-output-string (lambda (p) (write digest p)))
                                                " denied ))\n"))
                   (same-len-corrupt (make-string (string-length valid-trusted) #\()))
              (replace-file fc-store valid-trusted)
              (project-settings-load! repo-dir)
              (check "restored valid ledger recovers to trusted" 'trusted
                     (project-trust-verdict repo-dir 'compile-command "cargo check"))
              ;; Equal-length replacement within the same second (identical kind/size/mtime)
              ;; must still be detected by content-hash validation, never skipped by stat cache.
              (let retry-denied ((attempt 0))
                (replace-file fc-store valid-trusted)
                (project-settings-load! repo-dir)
                (let ((st-before (file-attributes fc-store)))
                  (replace-file fc-store valid-denied)
                  (let ((st-after (file-attributes fc-store)))
                    (if (and (< attempt 3)
                             (not (= (list-ref st-before 2) (list-ref st-after 2))))
                        (retry-denied (+ attempt 1))
                        (begin
                          (check "equal-length denied ledger preserves exact byte size"
                                 (list-ref st-before 1) (list-ref st-after 1))
                          (check "equal-length denied ledger preserves whole-second mtime"
                                 (list-ref st-before 2) (list-ref st-after 2))
                          (project-settings-load! repo-dir)
                          (check "same-second equal-length valid edit updates verdict to denied" 'denied
                                 (project-trust-verdict repo-dir 'compile-command "cargo check")))))))
              ;; Equal-length corrupt replacement within the same second must fail closed to 'unknown.
              (let retry-corrupt ((attempt 0))
                (replace-file fc-store valid-trusted)
                (project-settings-load! repo-dir)
                (let ((st-before (file-attributes fc-store)))
                  (replace-file fc-store same-len-corrupt)
                  (let ((st-after (file-attributes fc-store)))
                    (if (and (< attempt 3)
                             (not (= (list-ref st-before 2) (list-ref st-after 2))))
                        (retry-corrupt (+ attempt 1))
                        (begin
                          (check "equal-length corrupt ledger preserves exact byte size"
                                 (list-ref st-before 1) (list-ref st-after 1))
                          (check "equal-length corrupt ledger preserves whole-second mtime"
                                 (list-ref st-before 2) (list-ref st-after 2))
                          (project-settings-load! repo-dir)
                          (check "same-second equal-length corrupt ledger fails closed to unknown" 'unknown
                                 (project-trust-verdict repo-dir 'compile-command "cargo check")))))))
              ;; Replacing ledger with a non-regular entry (directory) fails closed too.
              (delete-path fc-store #f)
              (create-directory fc-store #o700)
              (project-settings-load! repo-dir)
              (check "non-regular ledger fails closed to unknown" 'unknown
                     (project-trust-verdict repo-dir 'compile-command "cargo check"))
              (delete-path fc-store #t))))

        ;; Untrusted startup baseline sentinel leaves values unapproved and refuses mutations
        (project-trust-clear-memory!)
        (project-trust-approve! repo-dir 'compile-command "cargo check")
        (check "in-memory verdict trusted before untrusted baseline hook" 'trusted
               (project-trust-verdict repo-dir 'compile-command "cargo check"))
        (dynamic-wind
          (lambda () (set-trust-store-path-hook! (lambda () 'untrusted-baseline)))
          (lambda ()
            (parameterize ((default-trust-store-path #f))
              (check "untrusted baseline fails verdict closed to unknown" 'unknown
                     (project-trust-verdict repo-dir 'compile-command "cargo check"))
              (assert-true "untrusted baseline exposes no authoritative entries"
                           (null? (project-trust-entries)))
              (assert-err "untrusted baseline refuses approve"
                          (lambda () (project-trust-approve! repo-dir 'compile-command "cargo check")))
              (assert-err "untrusted baseline refuses deny"
                          (lambda () (project-trust-deny! repo-dir 'compile-command "cargo check")))
              (assert-err "untrusted baseline refuses revoke"
                          (lambda () (project-trust-revoke! repo-dir 'compile-command)))))
          (lambda () (set-trust-store-path-hook! (lambda () #f))))

        (let ((d1 (value-trust-digest "/a/b/" 'opt "v"))
              (d2 (value-trust-digest "/a/b|\nopt/" 'other "v")))
          (assert-true "trust digests with delimiters never collide" (not (string=? d1 d2))))

        (let ((bad-glob (editorconfig-glob-compile "**/**/**/**/**/**/**/**/**/z")))
          (assert-false "pathological glob fails closed within step budget"
                        (bad-glob "a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/x")))

        (assert-false "remote path rejected by project-settings-load!"
                      (project-settings-load! "/ssh:example.com:/srv/app/"))
        (assert-true "remote path rejected by project-settings-discover"
                     (null? (project-settings-discover "/ssh:example.com:/srv/app/")))
        (assert-true "remote path rejected by editorconfig-discover"
                     (null? (editorconfig-discover "/ssh:example.com:/srv/app/")))))
    (lambda ()
      (set-trust-principal-hook! (lambda () 'user))
      (project-trust-clear-memory!)
      (project-settings-invalidate!)
      (cleanup-tree root))))

(test-summary)
