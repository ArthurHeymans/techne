;;; Headless editor tests for Forge & Code Review workflows.
(import (techne editor))
(require "../../../lisp/test.scm")
(require "../../../lisp/editor/main.scm")

;; Load the shipped forge package and its test hooks.
(set-option! 'forge-database ":memory:")
(set-option! 'secret-sources '())
(set-option! 'forge-allow-gh-cli-token #f)
(set-option! 'forge-credential-provider (lambda (h u) #f))
(run-command (current-session) 'forge-list-repositories 1)
(import (forge store))
(require "../editor.scm")
(forge-reset-db! #:path ":memory:")

(provide run-forge-tests)

(define (wait-until pred)
  (let loop ((i 0))
    (cond ((pred) #t)
          ((>= i 200) #f)
          (else (sleep 10) (loop (+ i 1))))))

(define %base-sha "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
(define %head-sha "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
(define %submitted-reviews '())

(define (mock-ok data)
  (dict "status" 200 "headers" (make-hash-table) "body" (json->string data)))

(forge-set-test-transport!
 (lambda (req)
   (let ((m (dict-ref req "method")) (p (dict-ref req "path")) (b (dict-ref req "body" #f)))
     (cond
       ((and (string=? m "GET") (string=? p "/repos/octocat/demo"))
        (mock-ok (dict "default_branch" "main")))
       ((and (string=? m "GET") (string-prefix? "/repos/octocat/demo/labels" p))
        (mock-ok (vector (dict "name" "enhancement" "color" "a2eeef" "description" ""))))
       ((and (string=? m "GET") (string-prefix? "/repos/octocat/demo/pulls?" p))
        (mock-ok (vector (dict "number" 42 "title" "Add code review UI" "body" "Implements interactive review"
                               "state" "open" "draft" #f "user" (dict "login" "alice")
                               "head" (dict "ref" "feat/review" "sha" %head-sha "repo" (dict "full_name" "octocat/demo"))
                               "base" (dict "ref" "main" "sha" %base-sha)
                               "updated_at" "2026-04-04T09:00:00Z"))))
       ((and (string=? m "GET") (string-prefix? "/repos/octocat/demo/issues?" p))
        (mock-ok (vector (dict "number" 43 "title" "Support Jujutsu PR checkout" "body" "Works with jj"
                               "state" "open" "user" (dict "login" "bob")
                               "updated_at" "2026-04-04T10:00:00Z"))))
       ((and (string=? m "GET") (string=? p "/repos/octocat/demo/issues/42"))
        (mock-ok (dict "number" 42 "pull_request" (dict "url" "https://api.github.com/repos/octocat/demo/pulls/42"))))
       ((and (string=? m "GET") (string=? p "/repos/octocat/demo/pulls/42"))
        (mock-ok (dict "number" 42 "title" "Add code review UI" "body" "Implements interactive review"
                       "state" "open" "draft" #f "user" (dict "login" "alice")
                       "head" (dict "ref" "feat/review" "sha" %head-sha "repo" (dict "full_name" "octocat/demo"))
                       "base" (dict "ref" "main" "sha" %base-sha)
                       "updated_at" "2026-04-04T09:00:00Z")))
       ((and (string=? m "GET") (string-prefix? "/repos/octocat/demo/issues/42/comments" p))
        (mock-ok (vector (dict "id" 1 "user" (dict "login" "bob") "body" "Great work" "created_at" "2026-04-04T09:30:00Z"))))
       ((and (string=? m "GET") (string-prefix? "/repos/octocat/demo/pulls/42/reviews" p))
        (mock-ok (vector)))
       ((and (string=? m "GET") (string-prefix? "/repos/octocat/demo/pulls/42/comments" p))
        (mock-ok (vector (dict "id" 2 "user" (dict "login" "carol") "body" "Check nil case" "path" "lib/app.scm" "line" 2 "side" "RIGHT" "commit_id" %head-sha "created_at" "2026-04-04T09:35:00Z"))))
       ((and (string=? m "GET") (string-prefix? (string-append "/repos/octocat/demo/commits/" %head-sha "/check-runs") p))
        (mock-ok (dict "total_count" 1 "check_runs" (vector (dict "name" "tests" "status" "completed" "conclusion" "success" "details_url" "")))))
       ((and (string=? m "GET") (string-prefix? (string-append "/repos/octocat/demo/compare/" %base-sha "..." %head-sha) p))
        (mock-ok (dict "files" (vector (dict "filename" "lib/app.scm" "status" "modified" "additions" 2 "deletions" 1
                                             "patch" "@@ -1,2 +1,3 @@\n (define (ok)\n-  #f)\n+  (display \"hi\")\n+  #t)")))))
       ((and (string=? m "POST") (string=? p "/repos/octocat/demo/pulls/42/reviews"))
        (set! %submitted-reviews (cons b %submitted-reviews))
        (mock-ok (dict "id" 99 "user" (dict "login" "reviewer") "body" (dict-ref b "body" "") "state" (dict-ref b "event") "commit_id" %head-sha "submitted_at" "2026-04-04T11:00:00Z")))
       (else (dict "status" 404 "headers" (make-hash-table) "body" "{\"message\":\"Not Found\"}"))))))

(define (run-forge-tests)
(define s (current-session))
(set-option! 'vc-forge-repository "octocat/demo")

;; 1. Pull repository and list topics.
(run-command s 'forge-list-topics 1)
(run-command s 'forge-pull 1)
(wait-until (lambda () (string-contains (document-substring (doc s) 0 (document-length (doc s))) "#43")))
(let ((txt (document-substring (doc s) 0 (document-length (doc s)))))
  (check "topic list shows PR #42" #t (and (string-contains txt "PR #42") #t))
  (check "topic list shows issue #43" #t (and (string-contains txt "#43") #t))
  (check "topic list mode active" 'forge-topic-list-mode (buffer-mode (session-buffer s))))

;; 2. Open PR #42 in topic view, then switch to code-review-mode.
(run-command s 'forge-next-item 1)
(run-command s 'forge-next-item 1)
(let ((t (target-at (doc s) (point s))))
  ;; Find PR #42 regardless of sort order.
  (unless (and t (eq? (target-kind t) 'forge-topic) (= (forge-ref (cadr (target-value t)) 'number 0) 42))
    (move! s (lambda (_) 0))
    (let loop ((i 0))
      (when (< i 10)
        (run-command s 'forge-next-item 1)
        (let ((cur (target-at (doc s) (point s))))
          (unless (and cur (eq? (target-kind cur) 'forge-topic) (= (forge-ref (cadr (target-value cur)) 'number 0) 42))
            (loop (+ i 1))))))))
(run-command s 'forge-list-visit-at-point 1)
(check "topic detail mode active" 'forge-topic-mode (buffer-mode (session-buffer s)))

;; 3. Start code review on PR #42.
(run-command s 'code-review-forge-pr-at-point 1)
(wait-until (lambda () (string-contains (document-substring (doc s) 0 (document-length (doc s))) "lib/app.scm")))
(check "code-review-mode active" 'code-review-mode (buffer-mode (session-buffer s)))
(let ((txt (document-substring (doc s) 0 (document-length (doc s)))))
  (check "code review shows file" #t (and (string-contains txt "lib/app.scm") #t))
  (check "code review shows inline comment" #t (and (string-contains txt "Check nil case") #t))
  (check "code review shows added diff line" #t (and (string-contains txt "+  (display \"hi\")") #t)))

;; 4. Stage an inline suggestion on a diff line, verify it appears as pending, then submit APPROVE.
(let loop ((i 0))
  (when (< i 20)
    (let ((t (target-at (doc s) (point s))))
      (unless (and t (eq? (target-kind t) 'code-review-line))
        (run-command s 'forge-next-item 1)
        (loop (+ i 1))))))
(run-command s 'code-review-suggest-at-point 1)
(check "suggestion opens forge-post-mode" 'forge-post-mode (buffer-mode (session-buffer s)))
(let ((draft (document-substring (doc s) 0 (document-length (doc s)))))
  (check "suggestion prefilled fence" #t (and (string-contains draft "```suggestion") #t)))
(run-command s 'forge-post-submit 1)
(check "returned to code-review-mode" 'code-review-mode (buffer-mode (session-buffer s)))
(let ((txt (document-substring (doc s) 0 (document-length (doc s)))))
  (check "pending draft shown inline" #t (and (string-contains txt "[PENDING REVIEW COMMENT]") #t))
  (check "stats show 1 pending draft" #t (and (string-contains txt "1 pending draft") #t)))

;; 5. Submit review via code-review-menu -> APPROVE.
(run-command s 'code-review-menu 1)
(check "transient menu open" #t (transient-open? s))
;; Trigger APPROVE ('a') via editor-press.
(editor-press s "a")
(check "review submit buffer open" 'forge-post-mode (buffer-mode (session-buffer s)))
(insert-text! s "Ship it!" 'new)
(run-command s 'forge-post-submit 1)
(wait-until (lambda () (pair? %submitted-reviews)))
(check "review submitted to API" 1 (length %submitted-reviews))
(check "review event is APPROVE" "APPROVE" (dict-ref (car %submitted-reviews) "event"))
(check "review included staged inline comment" 1 (vector-length (dict-ref (car %submitted-reviews) "comments")))

(test-failures))
