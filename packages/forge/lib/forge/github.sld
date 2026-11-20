;;; GitHub API client (REST + GraphQL) for Forge and Code Review.
;;; Normalizes wire JSON into Forge store alists and performs write mutations.
(define-library (forge github)
  (export forge-github-pull-repo! forge-github-pull-topic! forge-github-pull-notifications!
          forge-github-fetch-pr-files forge-github-fetch-checks
          forge-github-create-issue! forge-github-create-pullreq!
          forge-github-update-topic! forge-github-set-labels!
          forge-github-set-assignees! forge-github-request-reviewers!
          forge-github-create-comment! forge-github-update-comment! forge-github-delete-comment!
          forge-github-merge-pr! forge-github-submit-review!
          forge-github-reply-review-comment! forge-github-mark-notification-read!)
  (import (techne) (forge http) (forge store))
  (begin
    (define (auth-headers token)
      (if (and (string? token) (> (string-length token) 0))
          (list (cons "Authorization" (string-append "Bearer " token)))
          '()))

    (define (dstr d key . default)
      (let ((v (and (dict? d) (dict-ref d key #f))))
        (if (string? v) v (if (pair? default) (car default) ""))))

    (define (dint d key . default)
      (let ((v (and (dict? d) (dict-ref d key #f))))
        (if (integer? v) v (if (pair? default) (car default) 0))))

    (define (dlist d key)
      (let ((v (and (dict? d) (dict-ref d key #f))))
        (cond ((vector? v) (vector->list v))
              ((list? v) v)
              (else '()))))

    (define (login-of d)
      (if (dict? d) (dstr d "login" "") ""))

    ;; Split "ghe.example.com/api/v3" into ("ghe.example.com" . "/api/v3").
    (define (split-api-host api-host)
      (let ((slash (string-index api-host #\/)))
        (if slash
            (let* ((h (substring api-host 0 slash))
                   (raw-p (substring api-host slash (string-length api-host)))
                   (p (if (and (> (string-length raw-p) 1)
                               (char=? (string-ref raw-p (- (string-length raw-p) 1)) #\/))
                          (substring raw-p 0 (- (string-length raw-p) 1))
                          raw-p)))
              (cons h p))
            (cons api-host ""))))

    (define (api-call api-host path #:method [method "GET"] #:token [token #f]
                      #:body [body #f] #:query [query #f] #:scheme [scheme "https"]
                      #:port [port #f] #:transport [transport #f])
      (let* ((hp (split-api-host api-host))
             (host (car hp))
             (full-path (string-append (cdr hp) path)))
        (forge-http-json host full-path #:method method #:scheme scheme #:port port
                         #:headers (auth-headers token) #:body body #:query query #:transport transport)))

    ;; Normalize a GitHub REST issue or pull-request object into a topic alist.
    (define (normalize-rest-issue obj)
      (let* ((is-pr (or (dict? (dict-ref obj "pull_request" #f))
                        (dict? (dict-ref obj "head" #f))))
             (head (dict-ref obj "head" #f))
             (base (dict-ref obj "base" #f))
             (head-repo-d (and (dict? head) (dict-ref head "repo" #f)))
             (merged? (or (string? (dict-ref obj "merged_at" #f))
                          (eq? (dict-ref obj "merged" #f) #t)))
             (raw-state (string-downcase (dstr obj "state" "open")))
             (state (cond (merged? 'merged)
                          ((string=? raw-state "closed") 'closed)
                          (else 'open)))
             (labels (filter-map (lambda (l) (and (dict? l) (let ((n (dstr l "name" ""))) (and (> (string-length n) 0) n))))
                                 (dlist obj "labels")))
             (assignees (filter-map (lambda (u) (let ((l (login-of u))) (and (> (string-length l) 0) l)))
                                    (dlist obj "assignees")))
             (reviewers (filter-map (lambda (u) (let ((l (login-of u))) (and (> (string-length l) 0) l)))
                                    (dlist obj "requested_reviewers"))))
        `((kind . ,(if is-pr 'pullreq 'issue))
          (number . ,(dint obj "number" 0))
          (state . ,state)
          (draft . ,(eq? (dict-ref obj "draft" #f) #t))
          (title . ,(dstr obj "title" ""))
          (body . ,(dstr obj "body" ""))
          (author . ,(login-of (dict-ref obj "user" #f)))
          (base-ref . ,(if (dict? base) (dstr base "ref" "") ""))
          (head-ref . ,(if (dict? head) (dstr head "ref" "") ""))
          (head-repo . ,(if (dict? head-repo-d) (dstr head-repo-d "full_name" "") ""))
          (head-sha . ,(if (dict? head) (dstr head "sha" "") ""))
          (base-sha . ,(if (dict? base) (dstr base "sha" "") ""))
          (mergeable . ,(let ((m (dict-ref obj "mergeable_state" #f))) (if (string? m) m "")))
          (comments-count . ,(dint obj "comments" 0))
          (created-at . ,(dstr obj "created_at" ""))
          (updated-at . ,(dstr obj "updated_at" ""))
          (labels . ,labels)
          (assignees . ,assignees)
          (reviewers . ,reviewers))))

    (define (normalize-issue-comment obj)
      `((remote-id . ,(string-append "ic:" (number->string (dint obj "id" 0))))
        (kind . comment)
        (author . ,(login-of (dict-ref obj "user" #f)))
        (body . ,(dstr obj "body" ""))
        (created-at . ,(dstr obj "created_at" ""))
        (updated-at . ,(dstr obj "updated_at" ""))))

    (define (normalize-review-comment obj)
      (let ((reply (dict-ref obj "in_reply_to_id" #f)))
        `((remote-id . ,(string-append "rc:" (number->string (dint obj "id" 0))))
          (kind . review_comment)
          (author . ,(login-of (dict-ref obj "user" #f)))
          (body . ,(dstr obj "body" ""))
          (path . ,(dstr obj "path" ""))
          ;; Keep line=0 when GitHub reports line=null (outdated comment).
          (line . ,(if (integer? (dict-ref obj "line" #f)) (dict-ref obj "line") 0))
          (original-line . ,(dint obj "original_line" 0))
          (side . ,(dstr obj "side" "RIGHT"))
          (commit-id . ,(dstr obj "commit_id" ""))
          (in-reply-to . ,(if (integer? reply) (string-append "rc:" (number->string reply)) ""))
          (created-at . ,(dstr obj "created_at" ""))
          (updated-at . ,(dstr obj "updated_at" "")))))

    (define (normalize-review obj)
      `((remote-id . ,(string-append "rv:" (number->string (dint obj "id" 0))))
        (kind . review)
        (author . ,(login-of (dict-ref obj "user" #f)))
        (body . ,(dstr obj "body" ""))
        (commit-id . ,(dstr obj "commit_id" ""))
        (review-state . ,(dstr obj "state" ""))
        (created-at . ,(dstr obj "submitted_at" ""))
        (updated-at . ,(dstr obj "submitted_at" ""))))

    (define (forge-github-pull-repo! db repo #:token [token #f] #:scheme [scheme "https"]
                                     #:port [port #f] #:transport [transport #f]
                                     #:per-page [per-page 50] #:max-pages [max-pages 3])
      "Sync metadata, labels, PRs, and issues for REPO into DB.
Uses TOKEN, SCHEME, PORT, TRANSPORT, PER-PAGE, and MAX-PAGES."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (owner (forge-ref repo 'owner))
             (name (forge-ref repo 'name))
             (base (string-append "/repos/" owner "/" name))
             (stored (forge-store-upsert-repo! db repo))
             (repo-id (forge-ref stored 'id))
             (synced-at (forge-ref stored 'synced-at ""))
             (call (lambda (p #:query [q #f])
                     (caddr (api-call api-host p #:token token #:query q
                                      #:scheme scheme #:port port #:transport transport)))))
        ;; 1. Repository metadata (default branch).
        (let* ((meta (guard (e (#t #f)) (call base)))
               (default-branch (and (dict? meta) (dstr meta "default_branch" ""))))
          ;; 2. Repository labels (best-effort).
          (guard (e (#t #f))
            (let ((raw-labels (call (string-append base "/labels") #:query "per_page=100")))
              (when (vector? raw-labels)
                (forge-store-upsert-repo-labels!
                 db repo-id
                 (map (lambda (l)
                        `((name . ,(dstr l "name" ""))
                          (color . ,(dstr l "color" ""))
                          (description . ,(dstr l "description" ""))))
                      (vector->list raw-labels))))))
          ;; 3. Pull requests (gives full base/head/sha/draft/reviewers metadata).
          (let ((pr-count 0) (issue-count 0) (issue-latest synced-at) (issues-truncated? #f))
            (define (bump-issue-latest! ts)
              (when (and (string? ts) (string>? ts issue-latest)) (set! issue-latest ts)))
            (let loop ((page 1))
              (when (<= page max-pages)
                (let* ((q (forge-query-string `(("state" . "all") ("sort" . "updated") ("direction" . "desc")
                                                ("per_page" . ,per-page) ("page" . ,page))))
                       (items (call (string-append base "/pulls") #:query q))
                       (xs (if (vector? items) (vector->list items) '())))
                  (for-each (lambda (obj)
                              (let ((t (normalize-rest-issue obj)))
                                (forge-store-upsert-topic! db repo-id t)
                                (set! pr-count (+ pr-count 1))))
                            xs)
                  (when (= (length xs) per-page) (loop (+ page 1))))))
            ;; 4. Issues (skip PRs already fetched above so we don't overwrite head/base SHAs).
            ;; Only advance synced_at when the issue scan finishes without truncation.
            (let loop ((page 1))
              (if (> page max-pages)
                  (set! issues-truncated? #t)
                  (let* ((params (append `(("state" . "all") ("sort" . "updated") ("direction" . "desc")
                                           ("per_page" . ,per-page) ("page" . ,page))
                                         (if (> (string-length synced-at) 0) `(("since" . ,synced-at)) '())))
                         (items (call (string-append base "/issues") #:query (forge-query-string params)))
                         (xs (if (vector? items) (vector->list items) '())))
                    (for-each (lambda (obj)
                                (unless (dict? (dict-ref obj "pull_request" #f))
                                  (let ((t (normalize-rest-issue obj)))
                                    (bump-issue-latest! (forge-ref t 'updated-at ""))
                                    (forge-store-upsert-topic! db repo-id t)
                                    (set! issue-count (+ issue-count 1)))))
                              xs)
                    (when (= (length xs) per-page) (loop (+ page 1))))))
            (forge-store-set-repo-synced! db repo-id (if issues-truncated? synced-at issue-latest)
                                          #:default-branch default-branch)
            `((repo . ,(forge-store-repo db (forge-ref stored 'web-host) (forge-ref stored 'slug)))
              (pullreqs . ,pr-count)
              (issues . ,issue-count)
              (truncated . ,issues-truncated?))))))

    (define (forge-github-pull-topic! db repo number #:token [token #f] #:scheme [scheme "https"]
                                      #:port [port #f] #:transport [transport #f])
      "Fetch topic NUMBER of REPO with comments, reviews, and checks into DB.
Uses TOKEN, SCHEME, PORT, and optional TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (owner (forge-ref repo 'owner))
             (name (forge-ref repo 'name))
             (base (string-append "/repos/" owner "/" name))
             (stored-repo (forge-store-upsert-repo! db repo))
             (repo-id (forge-ref stored-repo 'id))
             (num-s (number->string number))
             (call (lambda (p #:query [q #f])
                     (caddr (api-call api-host p #:token token #:query q
                                      #:scheme scheme #:port port #:transport transport))))
             ;; Try issue endpoint first; if it has pull_request, fetch /pulls/N for SHAs and refs.
             (issue-obj (call (string-append base "/issues/" num-s)))
             (is-pr (dict? (dict-ref issue-obj "pull_request" #f)))
             (topic-obj (if is-pr (call (string-append base "/pulls/" num-s)) issue-obj))
             (topic (forge-store-upsert-topic! db repo-id (normalize-rest-issue topic-obj)))
             (topic-id (forge-ref topic 'id)))
        ;; Issue comments (general conversation).
        (let ((comments (guard (e (#t #())) (call (string-append base "/issues/" num-s "/comments") #:query "per_page=100"))))
          (when (vector? comments)
            (for-each (lambda (c) (forge-store-upsert-post! db topic-id (normalize-issue-comment c)))
                      (vector->list comments))))
        ;; For PRs: reviews, inline review comments, and check runs.
        (when is-pr
          (let ((reviews (guard (e (#t #())) (call (string-append base "/pulls/" num-s "/reviews") #:query "per_page=100"))))
            (when (vector? reviews)
              (for-each (lambda (rv)
                          (let ((norm (normalize-review rv)))
                            (when (or (> (string-length (forge-ref norm 'body "")) 0)
                                      (not (string=? (forge-ref norm 'review-state "") "COMMENTED")))
                              (forge-store-upsert-post! db topic-id norm))))
                        (vector->list reviews))))
          (let ((rcomments (guard (e (#t #())) (call (string-append base "/pulls/" num-s "/comments") #:query "per_page=100"))))
            (when (vector? rcomments)
              (for-each (lambda (c) (forge-store-upsert-post! db topic-id (normalize-review-comment c)))
                        (vector->list rcomments))))
          (let ((head-sha (forge-ref topic 'head-sha "")))
            (when (> (string-length head-sha) 0)
              (guard (e (#t #f))
                (let ((checks (forge-github-fetch-checks repo head-sha #:token token #:scheme scheme
                                                         #:port port #:transport transport)))
                  (forge-store-set-checks! db topic-id head-sha checks))))))
        (forge-store-topic db repo-id number)))

    (define (forge-github-fetch-pr-files repo number #:base-sha [base-sha #f] #:head-sha [head-sha #f]
                                         #:token [token #f] #:scheme [scheme "https"]
                                         #:port [port #f] #:transport [transport #f])
      "Fetch changed files and patches for PR NUMBER of REPO.
Compares BASE-SHA and HEAD-SHA when given; uses TOKEN, SCHEME, PORT,
and optional TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (owner (forge-ref repo 'owner))
             (name (forge-ref repo 'name))
             (base (string-append "/repos/" owner "/" name))
             (raw-files
              (if (and (string? base-sha) (= (string-length base-sha) 40)
                       (string? head-sha) (= (string-length head-sha) 40))
                  (let ((cmp (caddr (api-call api-host (string-append base "/compare/" base-sha "..." head-sha)
                                              #:query "per_page=1" #:token token
                                              #:scheme scheme #:port port #:transport transport))))
                    (dlist cmp "files"))
                  (let loop ((page 1) (acc '()))
                    (if (> page 3)
                        acc
                        (let* ((q (forge-query-string `(("per_page" . 100) ("page" . ,page))))
                               (res (caddr (api-call api-host (string-append base "/pulls/" (number->string number) "/files")
                                                     #:query q #:token token
                                                     #:scheme scheme #:port port #:transport transport)))
                               (xs (if (vector? res) (vector->list res) '()))
                               (next (append acc xs)))
                          (if (< (length xs) 100) next (loop (+ page 1) next))))))))
        (map (lambda (f)
               `((filename . ,(dstr f "filename" ""))
                 (status . ,(dstr f "status" "modified"))
                 (additions . ,(dint f "additions" 0))
                 (deletions . ,(dint f "deletions" 0))
                 (patch . ,(let ((p (dict-ref f "patch" #f))) (and (string? p) p)))))
             raw-files)))

    (define (forge-github-fetch-checks repo head-sha #:token [token #f] #:scheme [scheme "https"]
                                       #:port [port #f] #:transport [transport #f])
      "Fetch GitHub check runs for HEAD-SHA in REPO.
Uses TOKEN, SCHEME, PORT, and optional TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (owner (forge-ref repo 'owner))
             (name (forge-ref repo 'name))
             (resp (caddr (api-call api-host (string-append "/repos/" owner "/" name "/commits/" head-sha "/check-runs")
                                    #:query "per_page=100" #:token token
                                    #:scheme scheme #:port port #:transport transport))))
        (map (lambda (c)
               `((name . ,(dstr c "name" "Unnamed check"))
                 (status . ,(dstr c "status" "unknown"))
                 (conclusion . ,(dstr c "conclusion" ""))
                 (details-url . ,(dstr c "details_url" ""))))
             (dlist resp "check_runs"))))

    (define (forge-github-pull-notifications! db #:api-host [api-host "api.github.com"]
                                              #:web-host [web-host "github.com"]
                                              #:token [token #f] #:all [all #f]
                                              #:scheme [scheme "https"] #:port [port #f] #:transport [transport #f])
      "Pull GitHub notifications from API-HOST for WEB-HOST into DB.
Uses TOKEN, ALL, SCHEME, PORT, and optional TRANSPORT."
      (let* ((q (forge-query-string `(("all" . ,(if all "true" "false")) ("per_page" . 50))))
             (items (caddr (api-call api-host "/notifications" #:query q #:token token
                                     #:scheme scheme #:port port #:transport transport)))
             (xs (if (vector? items) (vector->list items) '())))
        (for-each
         (lambda (n)
           (let* ((subj (dict-ref n "subject" (dict)))
                  (r (dict-ref n "repository" (dict)))
                  (owner-d (dict-ref r "owner" (dict)))
                  (owner (login-of owner-d))
                  (rname (dstr r "name" ""))
                  (stype (dstr subj "type" "Issue"))
                  (surl (dstr subj "url" ""))
                  (parts (string-split surl "/"))
                  (num (or (and (pair? parts) (string->number (car (reverse parts)))) 0)))
             (forge-store-upsert-notification!
              db
              `((id . ,(dstr n "id" ""))
                (host . ,web-host)
                (owner . ,owner)
                (repo-name . ,rname)
                (slug . ,(string-downcase (string-append owner "/" rname)))
                (topic-kind . ,(if (string-ci=? stype "PullRequest") 'pullreq 'issue))
                (topic-number . ,num)
                (title . ,(dstr subj "title" ""))
                (reason . ,(dstr n "reason" ""))
                (unread . ,(eq? (dict-ref n "unread" #t) #t))
                (updated-at . ,(dstr n "updated_at" ""))))))
         xs)
        (length xs)))

    (define (forge-github-mark-notification-read! db id #:api-host [api-host "api.github.com"]
                                                  #:token [token #f] #:scheme [scheme "https"]
                                                  #:port [port #f] #:transport [transport #f])
      "Mark notification thread ID as read on API-HOST and in DB.
Uses TOKEN, SCHEME, PORT, and optional TRANSPORT."
      (api-call api-host (string-append "/notifications/threads/" id)
                #:method "PATCH" #:token token #:scheme scheme #:port port #:transport transport)
      (forge-store-mark-notification-read! db id))

    ;; Write mutations.

    (define (forge-github-create-issue! db repo title body #:labels [labels '()] #:assignees [assignees '()]
                                        #:token [token #f] #:scheme [scheme "https"]
                                        #:port [port #f] #:transport [transport #f])
      "Create an issue in REPO with TITLE and BODY and store it in DB.
Uses LABELS, ASSIGNEES, TOKEN, SCHEME, PORT, and optional TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (path (string-append "/repos/" (forge-ref repo 'owner) "/" (forge-ref repo 'name) "/issues"))
             (payload (let ((d (dict "title" title "body" body)))
                        (let ((d (if (pair? labels) (dict-set d "labels" (list->vector labels)) d)))
                          (if (pair? assignees) (dict-set d "assignees" (list->vector assignees)) d))))
             (created (caddr (api-call api-host path #:method "POST" #:body payload #:token token
                                       #:scheme scheme #:port port #:transport transport)))
             (stored-repo (forge-store-upsert-repo! db repo)))
        (forge-store-upsert-topic! db (forge-ref stored-repo 'id) (normalize-rest-issue created))))

    (define (forge-github-create-pullreq! db repo title body head base #:draft [draft #f]
                                          #:token [token #f] #:scheme [scheme "https"]
                                          #:port [port #f] #:transport [transport #f])
      "Create a PR in REPO from HEAD to BASE with TITLE and BODY in DB.
Uses DRAFT, TOKEN, SCHEME, PORT, and optional TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (path (string-append "/repos/" (forge-ref repo 'owner) "/" (forge-ref repo 'name) "/pulls"))
             (payload (dict "title" title "body" body "head" head "base" base "draft" (and draft #t)))
             (created (caddr (api-call api-host path #:method "POST" #:body payload #:token token
                                       #:scheme scheme #:port port #:transport transport)))
             (stored-repo (forge-store-upsert-repo! db repo)))
        (forge-store-upsert-topic! db (forge-ref stored-repo 'id) (normalize-rest-issue created))))

    (define (forge-github-update-topic! db repo number #:title [title #f] #:body [body #f]
                                        #:state [state #f] #:token [token #f]
                                        #:scheme [scheme "https"] #:port [port #f] #:transport [transport #f])
      "Update TITLE, BODY, or STATE on topic NUMBER of REPO in DB.
Uses TOKEN, SCHEME, PORT, and optional TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (path (string-append "/repos/" (forge-ref repo 'owner) "/" (forge-ref repo 'name) "/issues/" (number->string number)))
             (d (dict))
             (d (if title (dict-set d "title" title) d))
             (d (if body (dict-set d "body" body) d))
             (d (if state (dict-set d "state" (if (symbol? state) (symbol->string state) state)) d))
             (updated (caddr (api-call api-host path #:method "PATCH" #:body d #:token token
                                       #:scheme scheme #:port port #:transport transport)))
             (stored-repo (forge-store-upsert-repo! db repo)))
        (forge-store-upsert-topic! db (forge-ref stored-repo 'id) (normalize-rest-issue updated))))

    (define (forge-github-set-labels! db repo number labels #:token [token #f]
                                      #:scheme [scheme "https"] #:port [port #f] #:transport [transport #f])
      "Replace LABELS on topic NUMBER of REPO remotely and in DB.
Uses TOKEN, SCHEME, PORT, and optional TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (path (string-append "/repos/" (forge-ref repo 'owner) "/" (forge-ref repo 'name)
                                  "/issues/" (number->string number) "/labels"))
             (stored-repo (forge-store-upsert-repo! db repo))
             (topic (forge-store-topic db (forge-ref stored-repo 'id) number)))
        (api-call api-host path #:method "PUT" #:body (dict "labels" (list->vector labels)) #:token token
                  #:scheme scheme #:port port #:transport transport)
        (when topic (forge-store-set-topic-labels! db (forge-ref topic 'id) labels))
        labels))

    (define (forge-github-set-assignees! db repo number assignees #:token [token #f]
                                         #:scheme [scheme "https"] #:port [port #f] #:transport [transport #f])
      "Replace ASSIGNEES on topic NUMBER of REPO remotely and in DB.
Uses TOKEN, SCHEME, PORT, and optional TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (path (string-append "/repos/" (forge-ref repo 'owner) "/" (forge-ref repo 'name)
                                  "/issues/" (number->string number)))
             (stored-repo (forge-store-upsert-repo! db repo))
             (topic (forge-store-topic db (forge-ref stored-repo 'id) number)))
        (api-call api-host path #:method "PATCH" #:body (dict "assignees" (list->vector assignees)) #:token token
                  #:scheme scheme #:port port #:transport transport)
        (when topic (forge-store-set-topic-people! db (forge-ref topic 'id) 'assignee assignees))
        assignees))

    (define (forge-github-request-reviewers! db repo number reviewers #:token [token #f]
                                             #:scheme [scheme "https"] #:port [port #f] #:transport [transport #f])
      "Request REVIEWERS on PR NUMBER of REPO and update DB.
Uses TOKEN, SCHEME, PORT, and optional TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (path (string-append "/repos/" (forge-ref repo 'owner) "/" (forge-ref repo 'name)
                                  "/pulls/" (number->string number) "/requested_reviewers"))
             (stored-repo (forge-store-upsert-repo! db repo))
             (topic (forge-store-topic db (forge-ref stored-repo 'id) number)))
        (api-call api-host path #:method "POST" #:body (dict "reviewers" (list->vector reviewers)) #:token token
                  #:scheme scheme #:port port #:transport transport)
        (when topic
          (let ((existing (forge-store-topic-people db (forge-ref topic 'id) 'reviewer)))
            (forge-store-set-topic-people! db (forge-ref topic 'id) 'reviewer (append existing reviewers))))
        reviewers))

    (define (forge-github-create-comment! db repo number body #:token [token #f]
                                          #:scheme [scheme "https"] #:port [port #f] #:transport [transport #f])
      "Post comment BODY on topic NUMBER of REPO and cache it in DB.
Uses TOKEN, SCHEME, PORT, and optional TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (path (string-append "/repos/" (forge-ref repo 'owner) "/" (forge-ref repo 'name)
                                  "/issues/" (number->string number) "/comments"))
             (created (caddr (api-call api-host path #:method "POST" #:body (dict "body" body) #:token token
                                       #:scheme scheme #:port port #:transport transport)))
             (stored-repo (forge-store-upsert-repo! db repo))
             (topic (forge-store-topic db (forge-ref stored-repo 'id) number))
             (norm (normalize-issue-comment created)))
        (when topic (forge-store-upsert-post! db (forge-ref topic 'id) norm))
        norm))

    (define (forge-github-update-comment! db repo topic-id remote-id body #:token [token #f]
                                          #:scheme [scheme "https"] #:port [port #f] #:transport [transport #f])
      "Update comment REMOTE-ID on TOPIC-ID of REPO with BODY in DB.
Uses TOKEN, SCHEME, PORT, and optional TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (is-rc (string-prefix? "rc:" remote-id))
             (numeric (if (or (string-prefix? "ic:" remote-id) is-rc)
                          (substring remote-id 3 (string-length remote-id))
                          remote-id))
             (path (string-append "/repos/" (forge-ref repo 'owner) "/" (forge-ref repo 'name)
                                  (if is-rc "/pulls/comments/" "/issues/comments/") numeric))
             (updated (caddr (api-call api-host path #:method "PATCH" #:body (dict "body" body) #:token token
                                       #:scheme scheme #:port port #:transport transport)))
             (norm (if is-rc (normalize-review-comment updated) (normalize-issue-comment updated))))
        (forge-store-upsert-post! db topic-id norm)
        norm))

    (define (forge-github-delete-comment! db repo topic-id remote-id #:token [token #f]
                                          #:scheme [scheme "https"] #:port [port #f] #:transport [transport #f])
      "Delete comment REMOTE-ID on TOPIC-ID of REPO remotely and from DB.
Uses TOKEN, SCHEME, PORT, and optional TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (is-rc (string-prefix? "rc:" remote-id))
             (numeric (if (or (string-prefix? "ic:" remote-id) is-rc)
                          (substring remote-id 3 (string-length remote-id))
                          remote-id))
             (path (string-append "/repos/" (forge-ref repo 'owner) "/" (forge-ref repo 'name)
                                  (if is-rc "/pulls/comments/" "/issues/comments/") numeric)))
        (api-call api-host path #:method "DELETE" #:token token
                  #:scheme scheme #:port port #:transport transport)
        (forge-store-delete-post! db topic-id remote-id)))

    (define (forge-github-merge-pr! db repo number #:method [merge-method "merge"] #:commit-title [commit-title #f]
                                    #:sha [sha #f] #:token [token #f] #:scheme [scheme "https"]
                                    #:port [port #f] #:transport [transport #f])
      "Merge PR NUMBER of REPO using MERGE-METHOD and update DB.
Binds expected head SHA when given; uses optional COMMIT-TITLE, TOKEN,
SCHEME, PORT, and TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (path (string-append "/repos/" (forge-ref repo 'owner) "/" (forge-ref repo 'name)
                                  "/pulls/" (number->string number) "/merge"))
             (d (dict "merge_method" merge-method))
             (d (if (and (string? sha) (> (string-length sha) 0)) (dict-set d "sha" sha) d))
             (d (if commit-title (dict-set d "commit_title" commit-title) d))
             (res (caddr (api-call api-host path #:method "PUT" #:body d #:token token
                                   #:scheme scheme #:port port #:transport transport)))
             (merged? (and (dict? res) (eq? (dict-ref res "merged" #f) #t)))
             (stored-repo (forge-store-upsert-repo! db repo))
             (topic (forge-store-topic db (forge-ref stored-repo 'id) number)))
        (unless merged?
          (error (string-append "Pull request was not merged: "
                                (if (dict? res) (dstr res "message" "unknown reason") "unexpected response"))))
        (when topic
          (forge-store-upsert-topic! db (forge-ref stored-repo 'id)
                                     (cons '(state . merged) topic)))
        res))

    (define (forge-github-submit-review! db repo number event body #:commit-id [commit-id #f]
                                         #:comments [comments '()] #:token [token #f]
                                         #:scheme [scheme "https"] #:port [port #f] #:transport [transport #f])
      "Submit a batched EVENT review with BODY on PR NUMBER of REPO in DB.
Attaches COMMENTS at COMMIT-ID; uses TOKEN, SCHEME, PORT, and TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (path (string-append "/repos/" (forge-ref repo 'owner) "/" (forge-ref repo 'name)
                                  "/pulls/" (number->string number) "/reviews"))
             (wire-comments
              (map (lambda (c)
                     (let* ((d (dict "path" (forge-ref c 'path "")
                                     "line" (forge-ref c 'line 1)
                                     "side" (forge-ref c 'side "RIGHT")
                                     "body" (forge-ref c 'body "")))
                            (sl (forge-ref c 'start-line 0)))
                       (if (and (integer? sl) (> sl 0) (< sl (forge-ref c 'line 1)))
                           (dict-set (dict-set d "start_line" sl) "start_side" (forge-ref c 'side "RIGHT"))
                           d)))
                   comments))
             (d (dict "event" event "body" (if (string? body) body "")))
             (d (if (and (string? commit-id) (> (string-length commit-id) 0)) (dict-set d "commit_id" commit-id) d))
             (d (if (pair? wire-comments) (dict-set d "comments" (list->vector wire-comments)) d))
             (created (caddr (api-call api-host path #:method "POST" #:body d #:token token
                                       #:scheme scheme #:port port #:transport transport)))
             (stored-repo (forge-store-upsert-repo! db repo))
             (topic (forge-store-topic db (forge-ref stored-repo 'id) number)))
        (when topic
          (forge-store-upsert-post! db (forge-ref topic 'id) (normalize-review created))
          ;; Delete only the exact draft IDs submitted, preserving any edited
          ;; in flight or belonging to another revision.
          (for-each (lambda (c)
                      (let ((cid (forge-ref c 'id #f)) (cbody (forge-ref c 'body "")))
                        (when (integer? cid)
                          (forge-store-delete-review-comment-if-unchanged! db cid cbody))))
                    comments))
        created))

    (define (forge-github-reply-review-comment! db repo number comment-id body #:token [token #f]
                                                #:scheme [scheme "https"] #:port [port #f] #:transport [transport #f])
      "Reply with BODY to review COMMENT-ID on PR NUMBER of REPO in DB.
Uses TOKEN, SCHEME, PORT, and optional TRANSPORT."
      (let* ((api-host (forge-ref repo 'api-host "api.github.com"))
             (numeric (if (string-prefix? "rc:" comment-id)
                          (substring comment-id 3 (string-length comment-id))
                          comment-id))
             (path (string-append "/repos/" (forge-ref repo 'owner) "/" (forge-ref repo 'name)
                                  "/pulls/" (number->string number) "/comments/" numeric "/replies"))
             (created (caddr (api-call api-host path #:method "POST" #:body (dict "body" body) #:token token
                                       #:scheme scheme #:port port #:transport transport)))
             (stored-repo (forge-store-upsert-repo! db repo))
             (topic (forge-store-topic db (forge-ref stored-repo 'id) number))
             (norm (normalize-review-comment created)))
        (when topic (forge-store-upsert-post! db (forge-ref topic 'id) norm))
        norm))))
