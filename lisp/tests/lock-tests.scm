;;; Pure-data `techne.lock`, content-addressed `tree-sha256` store,
;;; diamond-conflict rejection, root overrides, and offline verification
;;; without Git or the editor.
(import (techne) (techne crypto) (techne lock) (techne package) (techne path) (techne uuid))
(require "../test.scm")

(define root (string-append (or (get-environment-variable "TMPDIR") "/tmp") "/techne-lock-" (uuid-v4)))
(define (at . parts) (apply path-join root parts))
(define (write-file! file . forms)
  (make-directory (path-directory file))
  (call-with-output-file file (lambda (port) (for-each (lambda (f) (write f port) (newline port)) forms))))
(define (why thunk) (guard (e (#t (condition/report-string e))) (thunk) #f))
(define (failed? thunk) (and (why thunk) #t))

(define oid1 "0123456789abcdef0123456789abcdef01234567")
(define oid2 "fedcba9876543210fedcba9876543210fedcba98")
(define h1 "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
(define h2 "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210")

;; Pure-data lockfile parsing, deterministic sorting, and round-trip.
(let* ((raw `(lock
               (techne "0.4")
               (package (bob zed) "0.2.0"
                 (source (github "bob/zed" (commit ,oid2) (subdir "packages/zed")))
                 (tree-sha256 ,h2)
                 (depends (alice util)))
               (package (alice util) "0.1.0"
                 (source (git "https://example.org/alice/util.git" (commit ,oid1)))
                 (tree-sha256 ,h1)
                 (depends))))
       (lf (parse-lockfile raw))
       (text (format-lockfile lf))
       (lock-path (at "roundtrip" "techne.lock")))
  (check "lockfile?" #t (lockfile? lf))
  (check "techne version" "0.4" (lockfile-techne lf))
  (check "packages sorted deterministically" '((alice util) (bob zed))
         (map locked-package-name (lockfile-packages lf)))
  (make-directory (at "roundtrip"))
  (write-lockfile lock-path lf)
  (let ((lf2 (read-lockfile lock-path)))
    (check "round-trip text identical" text (format-lockfile lf2))
    (check "subdir preserved" `(github "bob/zed" (commit ,oid2) (subdir "packages/zed"))
           (locked-package-source (cadr (lockfile-packages lf2))))))

;; Persisted `(ref ...)` selectors round-trip deterministically across tag, branch, and commit.
(let* ((raw `(lock
               (techne "0.4")
               (package (alice by-branch) "0.1.0"
                 (source (github "alice/b" (commit ,oid1) (ref (branch "main"))))
                 (tree-sha256 ,h1)
                 (depends))
               (package (alice by-commit) "0.1.0"
                 (source (git "https://example.org/c.git" (commit ,oid2) (ref (commit ,oid2))))
                 (tree-sha256 ,h2)
                 (depends))
               (package (alice by-tag) "0.1.0"
                 (source (local "/tmp/t" (commit ,oid1) (ref (tag "v1.2.3")) (subdir "pkg")))
                 (tree-sha256 ,h1)
                 (depends))))
       (lf (parse-lockfile raw))
       (text (format-lockfile lf))
       (lf2 (parse-lockfile (read (open-input-string text)))))
  (check "ref round-trip text identical" text (format-lockfile lf2))
  (check "ref branch preserved" `(github "alice/b" (commit ,oid1) (ref (branch "main")))
         (locked-package-source (list-ref (lockfile-packages lf2) 0)))
  (check "ref commit preserved" `(git "https://example.org/c.git" (commit ,oid2) (ref (commit ,oid2)))
         (locked-package-source (list-ref (lockfile-packages lf2) 1)))
  (check "ref tag + subdir preserved" `(local "/tmp/t" (commit ,oid1) (ref (tag "v1.2.3")) (subdir "pkg"))
         (locked-package-source (list-ref (lockfile-packages lf2) 2))))

;; Malformed lockfiles are rejected cleanly as pure data.
(for-each
  (lambda (case)
    (check (car case) #t (failed? (lambda () (parse-lockfile (cdr case))))))
  `(("not a lock form" . (package (alice a)))
    ("missing techne header" . (lock (package (alice a) "0.1.0" (source (path "../a")) (tree-sha256 ,h1))))
    ("duplicate techne header" . (lock (techne "0.4") (techne "0.4")))
    ("unknown top-level clause" . (lock (techne "0.4") (extra 1)))
    ("duplicate package entry" . (lock (techne "0.4")
                                   (package (alice a) "0.1.0" (source (path "../a")) (tree-sha256 ,h1))
                                   (package (alice a) "0.1.0" (source (path "../a")) (tree-sha256 ,h1))))
    ("reserved namespace" . (lock (techne "0.4")
                              (package (scheme a) "0.1.0" (source (path "../a")) (tree-sha256 ,h1))))
    ("missing commit on git source" . (lock (techne "0.4")
                                        (package (alice a) "0.1.0" (source (github "alice/a")) (tree-sha256 ,h1))))
    ("tag instead of commit in lockfile" . (lock (techne "0.4")
                                             (package (alice a) "0.1.0" (source (github "alice/a" (tag "v1"))) (tree-sha256 ,h1))))
    ("bad subdir traversal" . (lock (techne "0.4")
                                (package (alice a) "0.1.0" (source (github "alice/a" (commit ,oid1) (subdir "../escape"))) (tree-sha256 ,h1))))
    ("absolute path source" . (lock (techne "0.4")
                                (package (alice a) "0.1.0" (source (path "/abs/a")) (tree-sha256 ,h1))))
    ("short tree-sha256" . (lock (techne "0.4")
                             (package (alice a) "0.1.0" (source (path "../a")) (tree-sha256 "abcd"))))
    ("uppercase tree-sha256" . (lock (techne "0.4")
                                 (package (alice a) "0.1.0" (source (path "../a"))
                                   (tree-sha256 "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF"))))
    ("dangling dependency reference" . (lock (techne "0.4")
                                         (package (alice a) "0.1.0" (source (path "../a")) (tree-sha256 ,h1) (depends (bob missing)))))
    ("self dependency in lockfile" . (lock (techne "0.4")
                                       (package (alice a) "0.1.0" (source (path "../a")) (tree-sha256 ,h1) (depends (alice a)))))
    ("cycle in lockfile" . (lock (techne "0.4")
                             (package (alice a) "0.1.0" (source (path "../a")) (tree-sha256 ,h1) (depends (alice b)))
                             (package (alice b) "0.1.0" (source (path "../b")) (tree-sha256 ,h2) (depends (alice a)))))))

;; Content-addressed store materialization excludes dotfiles and is idempotent.
(let ((src (at "mat-src")) (store (at "store")))
  (write-file! (path-join src "techne-package.scm") '(package (acme mat) (version "0.1.0")))
  (write-file! (path-join src "lib/acme/mat.sld")
               '(define-library (acme mat) (export mat-val) (import (scheme base)) (begin (define mat-val 42))))
  (make-directory (path-join src ".git"))
  (call-with-output-file (path-join src ".git/HEAD") (lambda (p) (display "ref: refs/heads/main\n" p)))
  (call-with-output-file (path-join src ".secrets.scm") (lambda (p) (display "(secret)" p)))
  (let* ((res1 (materialize-package-tree! src #:store store))
         (hash (car res1))
         (dest (cdr res1)))
    (check "materialized under store/<tree-sha256>" (path-join store hash) dest)
    (check "materialized tree hash matches source" (package-tree-sha256 src) hash)
    (check "materialized tree hash matches dest" hash (package-tree-sha256 dest))
    (check "hidden files excluded from store" #f (file-exists? (path-join dest ".git")))
    (check "secrets excluded from store" #f (file-exists? (path-join dest ".secrets.scm")))
    (check "idempotent repeat materialization" res1 (materialize-package-tree! src #:store store))
    ;; Corrupting a store entry is detected on re-materialization.
    (call-with-output-file (path-join dest "tamper.txt") (lambda (p) (display "bad" p)))
    (check "corrupted store entry rejected" #t (failed? (lambda () (materialize-package-tree! src #:store store))))
    (delete-file (path-join dest "tamper.txt"))))

;; Local path diamond resolution, conflict rejection, override, check-lockfile, and use-locked-package!.
(let ((ws (at "diamond")) (store (at "diamond-store")))
  (define (w rel . forms) (apply write-file! (path-join ws rel) forms))
  (w "util-v1/techne-package.scm" '(package (bob util) (version "1.0.0")))
  (w "util-v1/lib/bob/util.sld"
     '(define-library (bob util) (export util-ver) (import (scheme base)) (begin (define util-ver "v1"))))
  (w "util-v2/techne-package.scm" '(package (bob util) (version "2.0.0")))
  (w "util-v2/lib/bob/util.sld"
     '(define-library (bob util) (export util-ver) (import (scheme base)) (begin (define util-ver "v2"))))
  (w "a/techne-package.scm" '(package (alice a) (version "0.1.0") (depends ((bob util) (path "../util-v1")))))
  (w "a/lib/alice/a.sld"
     '(define-library (alice a) (export a-ver) (import (scheme base) (bob util)) (begin (define a-ver util-ver))))
  (w "b-conflict/techne-package.scm" '(package (alice b) (version "0.1.0") (depends ((bob util) (path "../util-v2")))))
  (w "b-conflict/lib/alice/b.sld"
     '(define-library (alice b) (export b-ver) (import (scheme base) (bob util)) (begin (define b-ver util-ver))))
  (w "b-same/techne-package.scm" '(package (alice b) (version "0.1.0") (depends ((bob util) (path "../util-v1")))))
  (w "b-same/lib/alice/b.sld"
     '(define-library (alice b) (export b-ver) (import (scheme base) (bob util)) (begin (define b-ver util-ver))))

  ;; Conflicting diamond fails loudly naming BOTH full dependency chains.
  (w "app-conflict/techne-package.scm"
     '(package (root app) (version "0.1.0")
        (depends ((alice a) (path "../a")) ((alice b) (path "../b-conflict")))))
  (let ((msg (or (why (lambda () (lock-project! (path-join ws "app-conflict") #:store store))) "")))
    (check "diamond conflict mentions package" #t (and (string-contains msg "(bob util)") #t))
    (check "diamond conflict names chain 1" #t (and (string-contains msg "(root app) -> (alice a) -> (bob util)") #t))
    (check "diamond conflict names chain 2" #t (and (string-contains msg "(root app) -> (alice b) -> (bob util)") #t))
    (check "failed lock writes no techne.lock" #f (file-exists? (path-join ws "app-conflict/techne.lock"))))

  ;; Root override resolves the diamond conflict cleanly to util-v2.
  (w "app-override/techne-package.scm"
     '(package (root app) (version "0.1.0")
        (depends ((alice a) (path "../a")) ((alice b) (path "../b-conflict")))
        (override ((bob util) (path "../util-v2")))))
  (let ((lf (lock-project! (path-join ws "app-override") #:store store)))
    (check "override locks 3 packages" '((alice a) (alice b) (bob util))
           (map locked-package-name (lockfile-packages lf)))
    (check "check-lockfile succeeds" #t (lockfile? (check-lockfile (path-join ws "app-override") #:store store)))
    ;; Tampering with a dependency file causes check-lockfile to fail.
    (call-with-output-file (path-join ws "util-v2/extra.txt") (lambda (p) (display "tampered" p)))
    (check "tampered tree-sha256 rejected" #t
           (and (string-contains (or (why (lambda () (check-lockfile (path-join ws "app-override") #:store store))) "")
                                 "tree-sha256 mismatch")
                #t))
    (delete-file (path-join ws "util-v2/extra.txt"))
    ;; Wire into VM strictly offline via use-locked-package!.
    (use-locked-package! (path-join ws "app-override") #:store store)
    (check "overridden diamond resolves both to v2" '("v2" "v2")
           (list (eval 'a-ver (environment '(alice a)))
                 (eval 'b-ver (environment '(alice b)))))
    ;; Direct import boundary still enforced: root does not depend on (bob util).
    (w "app-override/bad-import.scm" '(import (bob util)))
    (check "transitive import refused for root" #t
           (and (string-contains (or (why (lambda () (load-package 'root-bad (path-join ws "app-override/bad-import.scm")))) "")
                                 "does not depend on (bob util)")
                #t)))

  ;; Stale lockfile detection when manifest adds/removes dependencies.
  (w "app-stale/techne-package.scm"
     '(package (root stale) (version "0.1.0")
        (depends ((alice a) (path "../a")))))
  (lock-project! (path-join ws "app-stale") #:store store)
  (check "fresh lockfile checks ok" #t (lockfile? (check-lockfile (path-join ws "app-stale") #:store store)))
  (w "app-stale/techne-package.scm"
     '(package (root stale) (version "0.1.0")
        (depends ((alice a) (path "../a")) ((alice b) (path "../b-same")))))
  (check "added dep makes lockfile stale" #t
         (and (string-contains (or (why (lambda () (check-lockfile (path-join ws "app-stale") #:store store))) "")
                               "stale")
              #t))
  (w "app-stale/techne-package.scm" '(package (root stale) (version "0.1.0")))
  (check "removed dep makes lockfile stale" #t
         (and (string-contains (or (why (lambda () (check-lockfile (path-join ws "app-stale") #:store store))) "")
                               "stale")
              #t))

  ;; Cycle detection names the cycle chain.
  (w "cyc-a/techne-package.scm" '(package (cyc a) (depends ((cyc b) (path "../cyc-b")))))
  (w "cyc-b/techne-package.scm" '(package (cyc b) (depends ((cyc a) (path "../cyc-a")))))
  (let ((msg (or (why (lambda () (lock-project! (path-join ws "cyc-a") #:store store))) "")))
    (check "cycle detected with chain" #t
           (and (string-contains msg "Dependency cycle") (string-contains msg "(cyc a) -> (cyc b) -> (cyc a)") #t)))

  ;; Frozen hydration rejects non-root `(local ...)` BEFORE accessing or fetching that local path.
  (let* ((app (path-join ws "frozen-local-reject"))
         (mid (path-join ws "frozen-mid"))
         (never-repo (path-join ws "must-never-be-fetched-nonroot-local"))
         (fresh-store (path-join ws "frozen-store")))
    (w "frozen-mid/techne-package.scm"
       `(package (alice mid) (version "0.1.0")
          (depends ((bob leaf) (local ,never-repo)))))
    (w "frozen-mid/lib/alice/mid.sld"
       '(define-library (alice mid) (export m) (import (scheme base)) (begin (define m 1))))
    (w "frozen-local-reject/techne-package.scm"
       '(package (root app) (version "0.1.0")
          (depends ((alice mid) (path "../frozen-mid")))))
    (let ((mid-sha (directory-tree-sha256 mid)))
      (write-lockfile
       (path-join app "techne.lock")
       (parse-lockfile
        `(lock
           (techne "0.4")
           (package (alice mid) "0.1.0"
             (source (path "../frozen-mid"))
             (tree-sha256 ,mid-sha)
             (depends (bob leaf)))
           (package (bob leaf) "0.1.0"
             (source (local ,never-repo (commit ,oid1)))
             (tree-sha256 ,h1)
             (depends))))))
    ;; `never-repo` does not exist on disk: if materialize-lockfile! attempted
    ;; to access or git-clone it before enforcing the non-root local ban, it
    ;; would fail with a git/missing-path error instead of the policy error.
    (let ((msg (or (why (lambda () (materialize-lockfile! app #:store fresh-store))) "")))
      (check "non-root local rejected before fetch" #t
             (and (string-contains msg "local")
                  (not (string-contains msg "git"))
                  (not (string-contains msg "clone"))
                  #t))
      (check "no checkout created in fresh store" #f (file-exists? (path-join fresh-store "trees" h1))))))

(delete-path root #t)
(test-summary)
