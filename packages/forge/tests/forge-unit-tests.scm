;;; Unit and integration tests for (forge remote), (forge auth), (forge diff),
;;; (forge http), (forge store), and (forge github).
(import (techne) (techne package) (techne path))
(require "../../../lisp/test.scm")

(define (here) #t)
(define pkg-root (path-join (path-directory (car (procedure-location here))) ".."))
(use-package-directory! pkg-root)

(import (forge remote) (forge auth) (forge diff) (forge http) (forge store) (forge github))

;;; 1. Remote parsing & URL builders
(let ((r1 (forge-parse-remote-url "git@github.com:Octocat/Hello-World.git"))
      (r2 (forge-parse-remote-url "https://github.com/Octocat/Hello-World/"))
      (r3 (forge-parse-remote-url "ssh://git@github.com/octocat/hello-world.git"))
      (r4 (forge-parse-remote-url "https://example.com/a/b.git")))
  (check "ssh remote slug" "octocat/hello-world" (forge-ref r1 'slug))
  (check "ssh remote api-host" "api.github.com" (forge-ref r1 'api-host))
  (check "https remote slug" "octocat/hello-world" (forge-ref r2 'slug))
  (check "ssh:// remote slug" "octocat/hello-world" (forge-ref r3 'slug))
  (check "unknown host rejected" #f r4))

(let ((ghe '(("git.corp.test" "api.corp.test" "git.corp.test" github))))
  (let ((r (forge-parse-remote-url "git@git.corp.test:team/service.git" #:alist ghe)))
    (check "enterprise api-host" "api.corp.test" (forge-ref r 'api-host))
    (check "enterprise slug" "team/service" (forge-ref r 'slug))))

(let ((u1 (forge-parse-topic-url "https://github.com/magit/forge/pull/601#discussion_r123"))
      (u2 (forge-parse-topic-url "https://github.com/magit/forge/issues/42?foo=bar"))
      (u3 (forge-parse-topic-url "https://github.com/magit/forge/commits/main")))
  (check "PR url kind" 'pullreq (forge-ref u1 'topic-kind))
  (check "PR url number" 601 (forge-ref u1 'number))
  (check "issue url kind" 'issue (forge-ref u2 'topic-kind))
  (check "issue url number" 42 (forge-ref u2 'number))
  (check "non-topic url rejected" #f u3))

(check "topic web url" "https://github.com/a/b/pull/7" (forge-topic-web-url "github.com" "a" "b" 'pullreq 7))
(check "comment web url" "https://github.com/a/b/issues/3#issuecomment-99"
       (forge-topic-web-url "github.com" "a" "b" 'issue 3 #:comment-id 99))
(check "blob web url range" "https://github.com/a/b/blob/main/src/x.scm#L10-L20"
       (forge-blob-web-url "github.com" "a" "b" "main" "src/x.scm" #:start-line 10 #:end-line 20))

;;; 2. Auth resolution (via `(techne secrets)`)
(import (techne secrets) (techne uuid))
(check "provider callback wins" "tok-provider"
       (forge-token "api.github.com" #:provider (lambda (h u) "tok-provider") #:allow-gh-cli #f))
(let ((s (forge-secret "api.github.com" #:provider (lambda (h u) (make-secret "tok-opaque")) #:allow-gh-cli #f)))
  (check "forge-secret returns opaque secret" #t (secret? s))
  (check "forge-auth-header accepts opaque secret" "Bearer tok-opaque" (forge-auth-header s)))
(check "auth header" "Bearer secret123" (forge-auth-header "secret123"))
(check "empty auth header" #f (forge-auth-header "   "))
(let* ((tmp-dir (string-append (or (get-environment-variable "TMPDIR") "/tmp") "/techne-forge-auth-" (uuid-v4)))
       (af (string-append tmp-dir "/authinfo"))
       (stub-gh (string-append tmp-dir "/stub-gh")))
  (create-directory tmp-dir #o700)
  (call-with-output-file af
    (lambda (p)
      (display "machine api.github.com login octocat^forge password tok-tagged\n" p)
      (display "machine api.github.com login octocat password tok-plain\n" p)
      (display "machine api.ghe.test login bot password tok-ghe\n" p)))
  (call-with-output-file stub-gh
    (lambda (p)
      (display "#!/bin/sh\nif [ \"$1\" = \"auth\" ] && [ \"$2\" = \"token\" ]; then echo \"gh-tok-$3\"; exit 0; fi\nexit 1\n" p)))
  (call-with-process "chmod" (list "+x" stub-gh) (lambda (p) (process-wait p)))

  (check "forge authinfo prefers user^forge" "tok-tagged"
         (forge-token "api.github.com" #:user "octocat" #:authinfo-file af #:allow-gh-cli #f #:allow-env #f))
  (check "forge authinfo finds ^forge without user" "tok-tagged"
         (forge-token "api.github.com" #:authinfo-file af #:allow-gh-cli #f #:allow-env #f))
  (check "forge authinfo matches GHE host" "tok-ghe"
         (forge-token "api.ghe.test" #:authinfo-file af #:allow-gh-cli #f #:allow-env #f))
  ;; Explicit user must NOT fall through to another user's token on the same host.
  (check "forge authinfo explicit user does not fall through to another user" #f
         (forge-token "api.github.com" #:user "other-user" #:authinfo-file af #:allow-gh-cli #f #:allow-env #f))
  ;; Provider returning a secret-ref inherits the Forge query (host, user, login-tag).
  (check "forge provider secret-ref inherits query" "tok-tagged"
         (forge-token "api.github.com" #:user "octocat"
                      #:provider (lambda (h u) (secret-ref 'authinfo #:file af))
                      #:allow-gh-cli #f #:allow-env #f))

  ;; gh-cli backend requires service='forge, explicit web-host, and matching api-host/web-host.
  (let ((sc (make-scope 'forge-auth-scope-test)))
    (with-scope sc
      (forge-register-secret-backends!)
      (check "gh-cli backend registered in scope" #t (procedure? (find-secret-backend 'gh-cli)))
      (let ((gh-src (list (list 'gh-cli #:gh stub-gh))))
        (check "gh-cli returns token for matching forge host"
               "gh-tok---hostname=github.com"
               (secret-reveal (secret-lookup #:service 'forge #:host "api.github.com"
                                             #:extra '((web-host . "github.com"))
                                             #:sources gh-src)))
        (check "gh-cli refuses mail service" #f
               (secret-lookup #:service 'mail #:host "api.github.com"
                              #:extra '((web-host . "github.com"))
                              #:sources gh-src))
        (check "gh-cli refuses missing web-host" #f
               (secret-lookup #:service 'forge #:host "api.ghe.test"
                              #:sources gh-src))
        (check "gh-cli refuses mismatched api-host and web-host" #f
               (secret-lookup #:service 'forge #:host "evil.example"
                              #:extra '((web-host . "github.com"))
                              #:sources gh-src))))
    (scope-shutdown! sc)
    (check "gh-cli backend cleaned up on scope shutdown" #f (find-secret-backend 'gh-cli)))

  (call-with-process "rm" (list "-rf" "--" tmp-dir) (lambda (p) (process-wait p))))

;;; 3. Unified diff line mapper (for code-review)
(check "hunk header" '(10 . 12) (forge-parse-hunk-header "@@ -10,4 +12,5 @@ (define (foo))"))
(let* ((patch "@@ -10,3 +12,4 @@\n ctx1\n-old1\n+new1\n+new2\n ctx2")
       (parsed (forge-parse-patch-lines patch)))
  (check "parsed line count" 6 (length parsed))
  (check "line 0 is hunk" 'hunk (forge-ref (list-ref parsed 0) 'kind))
  (check "ctx1 line" 12 (forge-ref (list-ref parsed 1) 'line))
  (check "old1 side LEFT" "LEFT" (forge-ref (list-ref parsed 2) 'side))
  (check "old1 line 11" 11 (forge-ref (list-ref parsed 2) 'line))
  (check "new1 side RIGHT" "RIGHT" (forge-ref (list-ref parsed 3) 'side))
  (check "new1 line 13" 13 (forge-ref (list-ref parsed 3) 'line))
  (check "new2 line 14" 14 (forge-ref (list-ref parsed 4) 'line))
  (check "new2 code" "new2" (forge-ref (list-ref parsed 4) 'code))
  (check "ctx2 line 15" 15 (forge-ref (list-ref parsed 5) 'line)))
(check "suggestion block" "```suggestion\nfoo\n```\n" (forge-suggestion-block "foo"))

;;; 4. SQLite store + FTS5 + pending code-review drafts
(let* ((db (forge-store-open ":memory:"))
       (repo (forge-store-upsert-repo! db '((host . "github.com") (api-host . "api.github.com")
                                            (web-host . "github.com") (kind . github)
                                            (owner . "octocat") (name . "project") (slug . "octocat/project"))))
       (rid (forge-ref repo 'id)))
  (forge-store-upsert-topic! db rid
    '((kind . pullreq) (number . 10) (state . open) (draft . #f)
      (title . "Add SQLite caching") (body . "Uses FTS5 for instant search")
      (author . "alice") (head-ref . "feat/cache") (base-ref . "main")
      (head-sha . "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
      (base-sha . "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
      (updated-at . "2026-04-01T12:00:00Z")
      (labels . ("enhancement" "forge")) (assignees . ("alice")) (reviewers . ("bob"))))
  (forge-store-upsert-topic! db rid
    '((kind . issue) (number . 11) (state . open) (title . "Fix terminal colors")
      (body . "Theme mismatch on status line") (author . "bob")
      (updated-at . "2026-04-02T12:00:00Z") (labels . ("bug"))))
  (check "all open topics" 2 (length (forge-store-topics db #:repo-id rid #:state 'open)))
  (check "PR filter" 1 (length (forge-store-topics db #:repo-id rid #:kind 'pullreq)))
  (check "label filter" 1 (length (forge-store-topics db #:repo-id rid #:label "bug")))
  (check "FTS5 search" 1 (length (forge-store-topics db #:repo-id rid #:query "FTS5")))
  (check "topic counts" '((pullreqs . 1) (issues . 1)) (forge-store-topic-counts db rid))

  (let* ((pr (forge-store-topic db rid 10))
         (tid (forge-ref pr 'id))
         (cid (forge-store-add-review-comment! db tid "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                                               "src/main.scm" 42 "Consider using guard here")))
    (check "pending review comment added" 1 (length (forge-store-review-comments db tid)))
    (forge-store-update-review-comment! db cid "Updated review suggestion")
    (check "pending review comment updated" "Updated review suggestion"
           (forge-ref (car (forge-store-review-comments db tid)) 'body))
    (forge-store-clear-review-comments! db tid)
    (check "pending review comments cleared" 0 (length (forge-store-review-comments db tid))))

  (forge-store-save-draft! db "draft-1" rid 'new-issue "My draft" "Draft body")
  (check "draft persisted" "My draft" (forge-ref (forge-store-draft db "draft-1") 'title))
  (forge-store-delete-draft! db "draft-1")
  (check "draft deleted" #f (forge-store-draft db "draft-1"))
  (forge-store-close db))

;;; 5. GitHub API client + code-review submission via mock transport
(let* ((db (forge-store-open ":memory:"))
       (repo '((host . "github.com") (api-host . "api.github.com") (web-host . "github.com")
               (kind . github) (owner . "octocat") (name . "project") (slug . "octocat/project")))
       (seen-requests '())
       (base-sha "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
       (head-sha "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
       (ok (lambda (data) (dict "status" 200 "headers" (make-hash-table) "body" (json->string data))))
       (transport
        (lambda (req)
          (let ((method (dict-ref req "method")) (path (dict-ref req "path")) (body (dict-ref req "body" #f)))
            (set! seen-requests (cons (list method path body) seen-requests))
            (cond
              ((and (string=? method "GET") (string=? path "/repos/octocat/project"))
               (ok (dict "default_branch" "main")))
              ((and (string=? method "GET") (string-prefix? "/repos/octocat/project/labels" path))
               (ok (vector (dict "name" "bug" "color" "d73a4a" "description" "Something isn't working"))))
              ((and (string=? method "GET") (string-prefix? "/repos/octocat/project/pulls?" path))
               (ok (vector (dict "number" 5 "title" "Add Forge" "body" "Full PR body" "state" "open" "draft" #f
                                 "user" (dict "login" "alice")
                                 "head" (dict "ref" "feat/forge" "sha" head-sha "repo" (dict "full_name" "octocat/project"))
                                 "base" (dict "ref" "main" "sha" base-sha)
                                 "updated_at" "2026-04-03T10:00:00Z"))))
              ((and (string=? method "GET") (string-prefix? "/repos/octocat/project/issues?" path))
               (ok (vector (dict "number" 6 "title" "Bug report" "body" "Details" "state" "open"
                                 "user" (dict "login" "bob") "updated_at" "2026-04-03T11:00:00Z"))))
              ((and (string=? method "GET") (string=? path "/repos/octocat/project/issues/5"))
               (ok (dict "number" 5 "pull_request" (dict "url" "https://api.github.com/repos/octocat/project/pulls/5"))))
              ((and (string=? method "GET") (string=? path "/repos/octocat/project/pulls/5"))
               (ok (dict "number" 5 "title" "Add Forge" "body" "Full PR body" "state" "open" "draft" #f
                         "user" (dict "login" "alice")
                         "head" (dict "ref" "feat/forge" "sha" head-sha "repo" (dict "full_name" "octocat/project"))
                         "base" (dict "ref" "main" "sha" base-sha)
                         "updated_at" "2026-04-03T10:00:00Z")))
              ((and (string=? method "GET") (string-prefix? "/repos/octocat/project/issues/5/comments" path))
               (ok (vector (dict "id" 101 "user" (dict "login" "bob") "body" "Looks promising" "created_at" "2026-04-03T10:05:00Z"))))
              ((and (string=? method "GET") (string-prefix? "/repos/octocat/project/pulls/5/reviews" path))
               (ok (vector)))
              ((and (string=? method "GET") (string-prefix? "/repos/octocat/project/pulls/5/comments" path))
               (ok (vector (dict "id" 201 "user" (dict "login" "carol") "body" "Nit: rename this" "path" "a.scm" "line" 2 "side" "RIGHT" "commit_id" head-sha "created_at" "2026-04-03T10:06:00Z"))))
              ((and (string=? method "GET") (string-prefix? (string-append "/repos/octocat/project/commits/" head-sha "/check-runs") path))
               (ok (dict "total_count" 1 "check_runs" (vector (dict "name" "CI" "status" "completed" "conclusion" "success" "details_url" "")))))
              ((and (string=? method "GET") (string-prefix? (string-append "/repos/octocat/project/compare/" base-sha "..." head-sha) path))
               (ok (dict "files" (vector (dict "filename" "a.scm" "status" "modified" "additions" 1 "deletions" 1 "patch" "@@ -1,2 +1,2 @@\n-old\n+new\n ctx")))))
              ((and (string=? method "POST") (string=? path "/repos/octocat/project/pulls/5/reviews"))
               (ok (dict "id" 301 "user" (dict "login" "me") "body" (dict-ref body "body" "") "state" "APPROVED" "commit_id" head-sha "submitted_at" "2026-04-03T10:10:00Z")))
              ((and (string=? method "POST") (string=? path "/repos/octocat/project/issues"))
               (ok (dict "number" 7 "title" (dict-ref body "title") "body" (dict-ref body "body") "state" "open" "user" (dict "login" "me") "updated_at" "2026-04-03T12:00:00Z")))
              ((and (string=? method "PUT") (string=? path "/repos/octocat/project/pulls/5/merge"))
               (ok (dict "merged" #t "message" "Pull Request successfully merged")))
              (else (dict "status" 404 "headers" (make-hash-table) "body" "{\"message\":\"Not Found\"}")))))))
  (let ((res (forge-github-pull-repo! db repo #:token "t" #:transport transport)))
    (check "pulled 1 PR" 1 (forge-ref res 'pullreqs))
    (check "pulled 1 issue" 1 (forge-ref res 'issues)))
  (let* ((pr (forge-github-pull-topic! db repo 5 #:token "t" #:transport transport))
         (tid (forge-ref pr 'id))
         (files (forge-github-fetch-pr-files repo 5 #:base-sha base-sha #:head-sha head-sha #:token "t" #:transport transport)))
    (check "PR has 2 posts (issue + review comment)" 2 (length (forge-store-posts db tid)))
    (check "PR has 1 check" 1 (length (forge-store-checks db tid head-sha)))
    (check "PR has 1 changed file" "a.scm" (forge-ref (car files) 'filename))
    ;; Stage inline review comment and submit atomic APPROVE review.
    (forge-store-add-review-comment! db tid head-sha "a.scm" 1 "Nice cleanup!" #:side "RIGHT")
    (let ((pending (forge-store-review-comments db tid #:head-sha head-sha)))
      (forge-github-submit-review! db repo 5 "APPROVE" "LGTM!" #:commit-id head-sha #:comments pending #:token "t" #:transport transport)
      (check "pending drafts cleared after submit" 0 (length (forge-store-review-comments db tid)))
      (let ((review-req (find (lambda (r) (and (string=? (car r) "POST") (string=? (cadr r) "/repos/octocat/project/pulls/5/reviews"))) seen-requests)))
        (check "review payload sent APPROVE" "APPROVE" (dict-ref (caddr review-req) "event"))
        (check "review payload included 1 inline comment" 1 (vector-length (dict-ref (caddr review-req) "comments"))))))
  (let ((created (forge-github-create-issue! db repo "New issue" "Body text" #:token "t" #:transport transport)))
    (check "created issue cached as #7" 7 (forge-ref created 'number)))
  (forge-github-merge-pr! db repo 5 #:method "squash" #:token "t" #:transport transport)
  (let ((merged (forge-store-topic db (forge-ref (forge-store-repo db "github.com" "octocat/project") 'id) 5)))
    (check "merged PR state updated locally" 'merged (forge-ref merged 'state)))
  (forge-store-close db))

;;; 6. Regression tests from code review (headers, draft preservation, SHA merge, GHE prefix, strict HTTP)

;; Diff parser ignores file headers before first @@ hunk.
(let ((parsed (forge-parse-patch-lines "--- a/foo.scm\n+++ b/foo.scm\n@@ -1,1 +1,1 @@\n-old\n+new")))
  (check "file header --- is meta" 'meta (forge-ref (list-ref parsed 0) 'kind))
  (check "file header +++ has no line anchor" #f (forge-ref (list-ref parsed 1) 'line))
  (check "first del after hunk is line 1" 1 (forge-ref (list-ref parsed 3) 'line)))

;; Upsert repo without default-branch preserves previously synced default-branch.
(let* ((db (forge-store-open ":memory:"))
       (r1 (forge-store-upsert-repo! db '((host . "github.com") (api-host . "api.github.com")
                                          (web-host . "github.com") (owner . "a") (name . "b")
                                          (slug . "a/b") (default-branch . "trunk"))))
       (r2 (forge-store-upsert-repo! db '((host . "github.com") (api-host . "api.github.com")
                                          (web-host . "github.com") (owner . "a") (name . "b")
                                          (slug . "a/b")))))
  (check "default-branch preserved on identity upsert" "trunk" (forge-ref r2 'default-branch))
  (forge-store-close db))

;; Submitting a review only deletes exact submitted drafts whose body did not change.
(let* ((db (forge-store-open ":memory:"))
       (repo '((host . "github.com") (api-host . "api.github.com") (web-host . "github.com")
               (kind . github) (owner . "o") (name . "r") (slug . "o/r")))
       (stored (forge-store-upsert-repo! db repo))
       (pr (forge-store-upsert-topic! db (forge-ref stored 'id)
             '((kind . pullreq) (number . 9) (state . open) (title . "T") (head-sha . "2222222222222222222222222222222222222222"))))
       (tid (forge-ref pr 'id))
       (old-id (forge-store-add-review-comment! db tid "1111111111111111111111111111111111111111" "a.scm" 1 "Old sha draft"))
       (sub-id (forge-store-add-review-comment! db tid "2222222222222222222222222222222222222222" "a.scm" 2 "Submitted draft"))
       (edited-id (forge-store-add-review-comment! db tid "2222222222222222222222222222222222222222" "a.scm" 3 "Before edit"))
       (snapshot (forge-store-review-comments db tid #:head-sha "2222222222222222222222222222222222222222"))
       (transport (lambda (req)
                    ;; Simulate user editing a draft while review POST is in flight.
                    (forge-store-update-review-comment! db edited-id "After edit")
                    (dict "status" 200 "headers" (make-hash-table)
                          "body" (json->string (dict "id" 55 "user" (dict "login" "me") "body" "ok" "state" "COMMENTED"))))))
  (forge-github-submit-review! db repo 9 "COMMENT" "ok"
                               #:commit-id "2222222222222222222222222222222222222222"
                               #:comments snapshot #:token "t" #:transport transport)
  (let ((remaining (map (lambda (d) (forge-ref d 'id)) (forge-store-review-comments db tid))))
    (check "old-sha draft preserved" #t (and (memv old-id remaining) #t))
    (check "in-flight edited draft preserved" #t (and (memv edited-id remaining) #t))
    (check "unchanged submitted draft deleted" #f (and (memv sub-id remaining) #t)))
  (forge-store-close db))

;; Merge requires merged:true and sends sha; GHE api-host path prefix is prepended.
(let* ((db (forge-store-open ":memory:"))
       (ghe-repo '((host . "ghe.corp.test") (api-host . "ghe.corp.test/api/v3") (web-host . "ghe.corp.test")
                   (kind . github) (owner . "team") (name . "app") (slug . "team/app")))
       (stored (forge-store-upsert-repo! db ghe-repo))
       (pr (forge-store-upsert-topic! db (forge-ref stored 'id)
             '((kind . pullreq) (number . 12) (state . open) (title . "GHE PR")
               (head-sha . "cccccccccccccccccccccccccccccccccccccccc"))))
       (captured #f)
       (reject-transport (lambda (req)
                           (set! captured req)
                           (dict "status" 200 "headers" (make-hash-table)
                                 "body" "{\"merged\":false,\"message\":\"Head branch was modified\"}"))))
  (let ((err (guard (e (#t (error-object-message e)))
               (forge-github-merge-pr! db ghe-repo 12 #:sha "cccccccccccccccccccccccccccccccccccccccc"
                                       #:token "t" #:transport reject-transport)
               #f)))
    (check "GHE path prefixed with /api/v3" "/api/v3/repos/team/app/pulls/12/merge" (dict-ref captured "path"))
    (check "GHE host split cleanly" "ghe.corp.test" (dict-ref captured "host"))
    (check "merge request sends sha" "cccccccccccccccccccccccccccccccccccccccc" (dict-ref (dict-ref captured "body") "sha"))
    (check "unmerged response raises error" #t (and (string? err) (string-contains err "not merged") #t))
    (check "PR stays open locally" 'open (forge-ref (forge-store-topic db (forge-ref stored 'id) 12) 'state)))
  (forge-store-close db))

;; Strict HTTP status & JSON validation: 302 redirect and invalid JSON must error.
(let ((redir-err (guard (e ((forge-http-error? e) (forge-http-error-status e)))
                   (forge-http-json "api.github.com" "/x"
                                    #:transport (lambda (_) (dict "status" 302 "headers" (make-hash-table) "body" "")))
                   #f))
      (json-err (guard (e ((forge-http-error? e) (forge-http-error-message e)))
                  (forge-http-json "api.github.com" "/x"
                                   #:transport (lambda (_) (dict "status" 200 "headers" (make-hash-table) "body" "<html>bad</html>")))
                  #f)))
  (check "302 redirect rejected" 302 redir-err)
  (check "invalid JSON rejected" "Invalid JSON response from server" json-err))

;; Wire HTTP/1.1 loopback test over real TCP (chunked encoding + Link header).
(let* ((listener (tcp-listen 0 #:host "127.0.0.1"))
       (addr (tcp-local-address listener))
       (port (cadr addr))
       (server (spawn (lambda ()
                        (let ((conn (tcp-accept listener)))
                          (dynamic-wind
                            (lambda () #f)
                            (lambda ()
                              (tcp-read-bytes conn)
                              (tcp-write conn
                                (string-append
                                 "HTTP/1.1 200 OK\r\n"
                                 "Content-Type: application/json\r\n"
                                 "Transfer-Encoding: chunked\r\n"
                                 "Link: <https://api.github.com/repos/a/b/issues?page=2>; rel=\"next\"\r\n"
                                 "Connection: close\r\n\r\n"
                                 "b\r\n{\"ok\":true}\r\n"
                                 "0\r\n\r\n")))
                            (lambda () (tcp-close conn) (tcp-close listener))))))))
  (let* ((res (forge-http-json "127.0.0.1" "/test" #:scheme "http" #:port port))
         (status (car res))
         (headers (cadr res))
         (body (caddr res)))
    (task-join server)
    (check "wire HTTP 200" 200 status)
    (check "wire chunked JSON parsed" #t (dict-ref body "ok" #f))
    (check "wire Link next parsed" "https://api.github.com/repos/a/b/issues?page=2"
           (forge-http-link-next headers))))

(test-summary)
