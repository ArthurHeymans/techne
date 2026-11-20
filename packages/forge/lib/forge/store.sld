;;; Local SQLite cache for Forge repositories, topics, posts, checks,
;;; notifications, compose drafts, and pending code-review comments.
(define-library (forge store)
  (export forge-store-open forge-store-close forge-ref
          forge-store-upsert-repo! forge-store-repo forge-store-repos forge-store-remove-repo!
          forge-store-set-repo-synced!
          forge-store-upsert-topic! forge-store-topic forge-store-topics forge-store-remove-topic!
          forge-store-set-read-state! forge-store-topic-counts
          forge-store-upsert-post! forge-store-posts forge-store-delete-post!
          forge-store-set-topic-labels! forge-store-topic-labels forge-store-upsert-repo-labels! forge-store-repo-labels
          forge-store-set-topic-people! forge-store-topic-people
          forge-store-set-checks! forge-store-checks
          forge-store-upsert-notification! forge-store-notifications forge-store-mark-notification-read!
          forge-store-save-draft! forge-store-draft forge-store-delete-draft!
          forge-store-add-review-comment! forge-store-review-comments forge-store-update-review-comment!
          forge-store-delete-review-comment! forge-store-delete-review-comment-if-unchanged!
          forge-store-clear-review-comments!)
  (import (techne) (techne sqlite) (techne task))
  (begin
    (define (forge-ref alist key . default)
      "Return KEY in alist ALIST, or DEFAULT (otherwise #f)."
      (let ((e (assq key alist)))
        (if e (cdr e) (if (pair? default) (car default) #f))))

    (define (sval x) (if (string? x) x ""))
    (define (ival x) (if (integer? x) x 0))
    (define (bval x) (if x 1 0))

    (define (forge-store-open path)
      "Open the Forge SQLite cache at PATH (\":memory:\" for tests)."
      (let ((db (sqlite-open path #:private? #t)))
        (for-each
         (lambda (sql) (sqlite-execute db sql '()))
         '("CREATE TABLE IF NOT EXISTS forge_repos (id INTEGER PRIMARY KEY, host TEXT NOT NULL, api_host TEXT NOT NULL, web_host TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'github', owner TEXT NOT NULL, name TEXT NOT NULL, slug TEXT NOT NULL, default_branch TEXT NOT NULL DEFAULT 'main', synced_at TEXT NOT NULL DEFAULT '', sparse INTEGER NOT NULL DEFAULT 0, UNIQUE(web_host, slug))"
           "CREATE TABLE IF NOT EXISTS forge_topics (id INTEGER PRIMARY KEY, repo_id INTEGER NOT NULL, kind TEXT NOT NULL, number INTEGER NOT NULL, state TEXT NOT NULL DEFAULT 'open', draft INTEGER NOT NULL DEFAULT 0, title TEXT NOT NULL DEFAULT '', body TEXT NOT NULL DEFAULT '', author TEXT NOT NULL DEFAULT '', base_ref TEXT NOT NULL DEFAULT '', head_ref TEXT NOT NULL DEFAULT '', head_repo TEXT NOT NULL DEFAULT '', head_sha TEXT NOT NULL DEFAULT '', base_sha TEXT NOT NULL DEFAULT '', mergeable TEXT NOT NULL DEFAULT '', review_decision TEXT NOT NULL DEFAULT '', checks_state TEXT NOT NULL DEFAULT '', comments_count INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL DEFAULT '', synced_at TEXT NOT NULL DEFAULT '', read_state TEXT NOT NULL DEFAULT 'unread', UNIQUE(repo_id, number))"
           "CREATE INDEX IF NOT EXISTS forge_topics_repo_idx ON forge_topics(repo_id, kind, state, updated_at DESC)"
           "CREATE TABLE IF NOT EXISTS forge_posts (id INTEGER PRIMARY KEY, topic_id INTEGER NOT NULL, remote_id TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'comment', author TEXT NOT NULL DEFAULT '', body TEXT NOT NULL DEFAULT '', path TEXT NOT NULL DEFAULT '', line INTEGER NOT NULL DEFAULT 0, original_line INTEGER NOT NULL DEFAULT 0, side TEXT NOT NULL DEFAULT '', commit_id TEXT NOT NULL DEFAULT '', in_reply_to TEXT NOT NULL DEFAULT '', review_state TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL DEFAULT '', UNIQUE(topic_id, remote_id))"
           "CREATE INDEX IF NOT EXISTS forge_posts_topic_idx ON forge_posts(topic_id, created_at ASC, id ASC)"
           "CREATE TABLE IF NOT EXISTS forge_labels (repo_id INTEGER NOT NULL, name TEXT NOT NULL, color TEXT NOT NULL DEFAULT '', description TEXT NOT NULL DEFAULT '', PRIMARY KEY(repo_id, name))"
           "CREATE TABLE IF NOT EXISTS forge_topic_labels (topic_id INTEGER NOT NULL, label TEXT NOT NULL, PRIMARY KEY(topic_id, label))"
           "CREATE TABLE IF NOT EXISTS forge_topic_people (topic_id INTEGER NOT NULL, role TEXT NOT NULL, login TEXT NOT NULL, PRIMARY KEY(topic_id, role, login))"
           "CREATE TABLE IF NOT EXISTS forge_checks (id INTEGER PRIMARY KEY, topic_id INTEGER NOT NULL, head_sha TEXT NOT NULL, name TEXT NOT NULL, status TEXT NOT NULL DEFAULT '', conclusion TEXT NOT NULL DEFAULT '', details_url TEXT NOT NULL DEFAULT '')"
           "CREATE INDEX IF NOT EXISTS forge_checks_topic_idx ON forge_checks(topic_id, head_sha)"
           "CREATE TABLE IF NOT EXISTS forge_notifications (id TEXT PRIMARY KEY, host TEXT NOT NULL, owner TEXT NOT NULL, repo_name TEXT NOT NULL, slug TEXT NOT NULL, topic_kind TEXT NOT NULL, topic_number INTEGER NOT NULL DEFAULT 0, title TEXT NOT NULL DEFAULT '', reason TEXT NOT NULL DEFAULT '', unread INTEGER NOT NULL DEFAULT 1, updated_at TEXT NOT NULL DEFAULT '')"
           "CREATE TABLE IF NOT EXISTS forge_drafts (id TEXT PRIMARY KEY, repo_id INTEGER NOT NULL, kind TEXT NOT NULL, topic_number INTEGER NOT NULL DEFAULT 0, post_remote_id TEXT NOT NULL DEFAULT '', title TEXT NOT NULL DEFAULT '', body TEXT NOT NULL DEFAULT '', meta_json TEXT NOT NULL DEFAULT '{}', updated_at TEXT NOT NULL DEFAULT '')"
           "CREATE TABLE IF NOT EXISTS forge_review_drafts (id INTEGER PRIMARY KEY, topic_id INTEGER NOT NULL, head_sha TEXT NOT NULL, path TEXT NOT NULL, line INTEGER NOT NULL, start_line INTEGER NOT NULL DEFAULT 0, side TEXT NOT NULL DEFAULT 'RIGHT', body TEXT NOT NULL, in_reply_to TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL DEFAULT '')"
           "CREATE INDEX IF NOT EXISTS forge_review_drafts_idx ON forge_review_drafts(topic_id, head_sha, path, line)"
           "CREATE VIRTUAL TABLE IF NOT EXISTS forge_search USING fts5(title, body, author, content='forge_topics', content_rowid='id')"
           "CREATE TRIGGER IF NOT EXISTS forge_search_ai AFTER INSERT ON forge_topics BEGIN INSERT INTO forge_search(rowid, title, body, author) VALUES(new.id, new.title, new.body, new.author); END"
           "CREATE TRIGGER IF NOT EXISTS forge_search_ad AFTER DELETE ON forge_topics BEGIN INSERT INTO forge_search(forge_search, rowid, title, body, author) VALUES('delete', old.id, old.title, old.body, old.author); END"
           "CREATE TRIGGER IF NOT EXISTS forge_search_au AFTER UPDATE OF title, body, author ON forge_topics BEGIN INSERT INTO forge_search(forge_search, rowid, title, body, author) VALUES('delete', old.id, old.title, old.body, old.author); INSERT INTO forge_search(rowid, title, body, author) VALUES(new.id, new.title, new.body, new.author); END"))
        db))

    (define (forge-store-close db)
      "Close the Forge SQLite store DB."
      (sqlite-close db))

    (define (row->repo r)
      `((id . ,(list-ref r 0))
        (host . ,(list-ref r 1))
        (api-host . ,(list-ref r 2))
        (web-host . ,(list-ref r 3))
        (kind . ,(string->symbol (list-ref r 4)))
        (owner . ,(list-ref r 5))
        (name . ,(list-ref r 6))
        (slug . ,(list-ref r 7))
        (default-branch . ,(list-ref r 8))
        (synced-at . ,(list-ref r 9))
        (sparse . ,(= (list-ref r 10) 1))))

    (define (forge-store-upsert-repo! db repo)
      "Insert or update repository REPO in DB and return the stored repo alist."
      (let* ((host (sval (forge-ref repo 'host "github.com")))
             (api-host (sval (forge-ref repo 'api-host "api.github.com")))
             (web-host (sval (forge-ref repo 'web-host "github.com")))
             (kind (let ((k (forge-ref repo 'kind 'github)))
                     (if (symbol? k) (symbol->string k) (sval k))))
             (owner (sval (forge-ref repo 'owner "")))
             (name (sval (forge-ref repo 'name "")))
             (slug (string-downcase (sval (forge-ref repo 'slug (string-append owner "/" name)))))
             (raw-branch (sval (forge-ref repo 'default-branch "")))
             (insert-branch (if (> (string-length raw-branch) 0) raw-branch "main"))
             (sparse (bval (forge-ref repo 'sparse #f))))
        (sqlite-execute db
          "INSERT INTO forge_repos(host, api_host, web_host, kind, owner, name, slug, default_branch, sparse)
           VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(web_host, slug) DO UPDATE SET
             host=excluded.host, api_host=excluded.api_host, kind=excluded.kind,
             owner=excluded.owner, name=excluded.name,
             default_branch=CASE WHEN ?='' THEN forge_repos.default_branch ELSE ? END,
             sparse=excluded.sparse"
          (list host api-host web-host kind owner name slug insert-branch sparse raw-branch raw-branch))
        (forge-store-repo db web-host slug)))

    (define (forge-store-repo db web-host slug)
      "Return the repository in DB for WEB-HOST and SLUG, or #f."
      (let ((rows (sqlite-query db
                    "SELECT id, host, api_host, web_host, kind, owner, name, slug, default_branch, synced_at, sparse
                     FROM forge_repos WHERE web_host=? AND slug=?"
                    (list web-host (string-downcase slug)))))
        (and (pair? rows) (row->repo (car rows)))))

    (define (forge-store-repos db)
      "Return all tracked repositories in DB, ordered by slug."
      (map row->repo
           (sqlite-query db
             "SELECT id, host, api_host, web_host, kind, owner, name, slug, default_branch, synced_at, sparse
              FROM forge_repos ORDER BY slug ASC"
             '())))

    (define (forge-store-set-repo-synced! db repo-id synced-at #:default-branch [default-branch #f])
      "Record SYNCED-AT (and optional DEFAULT-BRANCH) for REPO-ID in DB."
      (if (and (string? default-branch) (> (string-length default-branch) 0))
          (sqlite-execute db "UPDATE forge_repos SET synced_at=?, default_branch=? WHERE id=?"
                          (list synced-at default-branch repo-id))
          (sqlite-execute db "UPDATE forge_repos SET synced_at=? WHERE id=?"
                          (list synced-at repo-id))))

    (define (forge-store-remove-repo! db repo-id)
      "Remove REPO-ID and its cached topics, posts, and metadata from DB."
      (let ((tids (map car (sqlite-query db "SELECT id FROM forge_topics WHERE repo_id=?" (list repo-id)))))
        (for-each (lambda (tid) (forge-store-remove-topic! db tid)) tids))
      (sqlite-execute db "DELETE FROM forge_labels WHERE repo_id=?" (list repo-id))
      (sqlite-execute db "DELETE FROM forge_drafts WHERE repo_id=?" (list repo-id))
      (sqlite-execute db "DELETE FROM forge_repos WHERE id=?" (list repo-id)))

    (define (row->topic r)
      `((id . ,(list-ref r 0))
        (repo-id . ,(list-ref r 1))
        (kind . ,(string->symbol (list-ref r 2)))
        (number . ,(list-ref r 3))
        (state . ,(string->symbol (list-ref r 4)))
        (draft . ,(= (list-ref r 5) 1))
        (title . ,(list-ref r 6))
        (body . ,(list-ref r 7))
        (author . ,(list-ref r 8))
        (base-ref . ,(list-ref r 9))
        (head-ref . ,(list-ref r 10))
        (head-repo . ,(list-ref r 11))
        (head-sha . ,(list-ref r 12))
        (base-sha . ,(list-ref r 13))
        (mergeable . ,(list-ref r 14))
        (review-decision . ,(list-ref r 15))
        (checks-state . ,(list-ref r 16))
        (comments-count . ,(list-ref r 17))
        (created-at . ,(list-ref r 18))
        (updated-at . ,(list-ref r 19))
        (synced-at . ,(list-ref r 20))
        (read-state . ,(string->symbol (list-ref r 21)))
        (repo-slug . ,(if (> (length r) 22) (list-ref r 22) ""))))

    (define (forge-store-upsert-topic! db repo-id topic)
      "Insert or update TOPIC under REPO-ID in DB and return it."
      (let ((kind (let ((k (forge-ref topic 'kind 'issue))) (if (symbol? k) (symbol->string k) (sval k))))
            (number (ival (forge-ref topic 'number 0)))
            (state (let ((s (forge-ref topic 'state 'open))) (if (symbol? s) (symbol->string s) (sval s))))
            (draft (bval (forge-ref topic 'draft #f)))
            (title (sval (forge-ref topic 'title "")))
            (body (sval (forge-ref topic 'body "")))
            (author (sval (forge-ref topic 'author "")))
            (base-ref (sval (forge-ref topic 'base-ref "")))
            (head-ref (sval (forge-ref topic 'head-ref "")))
            (head-repo (sval (forge-ref topic 'head-repo "")))
            (head-sha (sval (forge-ref topic 'head-sha "")))
            (base-sha (sval (forge-ref topic 'base-sha "")))
            (mergeable (sval (forge-ref topic 'mergeable "")))
            (review-decision (sval (forge-ref topic 'review-decision "")))
            (checks-state (sval (forge-ref topic 'checks-state "")))
            (comments-count (ival (forge-ref topic 'comments-count 0)))
            (created-at (sval (forge-ref topic 'created-at "")))
            (updated-at (sval (forge-ref topic 'updated-at "")))
            (synced-at (sval (forge-ref topic 'synced-at ""))))
        (sqlite-execute db
          "INSERT INTO forge_topics(repo_id, kind, number, state, draft, title, body, author,
             base_ref, head_ref, head_repo, head_sha, base_sha, mergeable, review_decision,
             checks_state, comments_count, created_at, updated_at, synced_at)
           VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(repo_id, number) DO UPDATE SET
             kind=excluded.kind, state=excluded.state, draft=excluded.draft,
             title=excluded.title, body=excluded.body, author=excluded.author,
             base_ref=CASE WHEN excluded.base_ref='' THEN forge_topics.base_ref ELSE excluded.base_ref END,
             head_ref=CASE WHEN excluded.head_ref='' THEN forge_topics.head_ref ELSE excluded.head_ref END,
             head_repo=CASE WHEN excluded.head_repo='' THEN forge_topics.head_repo ELSE excluded.head_repo END,
             head_sha=CASE WHEN excluded.head_sha='' THEN forge_topics.head_sha ELSE excluded.head_sha END,
             base_sha=CASE WHEN excluded.base_sha='' THEN forge_topics.base_sha ELSE excluded.base_sha END,
             mergeable=CASE WHEN excluded.mergeable='' THEN forge_topics.mergeable ELSE excluded.mergeable END,
             review_decision=CASE WHEN excluded.review_decision='' THEN forge_topics.review_decision ELSE excluded.review_decision END,
             checks_state=CASE WHEN excluded.checks_state='' THEN forge_topics.checks_state ELSE excluded.checks_state END,
             comments_count=excluded.comments_count,
             created_at=CASE WHEN excluded.created_at='' THEN forge_topics.created_at ELSE excluded.created_at END,
             updated_at=excluded.updated_at,
             synced_at=CASE WHEN excluded.synced_at='' THEN forge_topics.synced_at ELSE excluded.synced_at END"
          (list repo-id kind number state draft title body author
                base-ref head-ref head-repo head-sha base-sha mergeable review-decision
                checks-state comments-count created-at updated-at synced-at))
        (let ((stored (forge-store-topic db repo-id number)))
          (when stored
            (let ((tid (forge-ref stored 'id)))
              (when (assq 'labels topic)
                (forge-store-set-topic-labels! db tid (forge-ref topic 'labels '())))
              (when (assq 'assignees topic)
                (forge-store-set-topic-people! db tid 'assignee (forge-ref topic 'assignees '())))
              (when (assq 'reviewers topic)
                (forge-store-set-topic-people! db tid 'reviewer (forge-ref topic 'reviewers '())))))
          stored)))

    (define (forge-store-topic db repo-id number)
      "Return the cached topic in DB for REPO-ID and NUMBER, or #f."
      (let ((rows (sqlite-query db
                    "SELECT t.id, t.repo_id, t.kind, t.number, t.state, t.draft, t.title, t.body, t.author,
                            t.base_ref, t.head_ref, t.head_repo, t.head_sha, t.base_sha, t.mergeable,
                            t.review_decision, t.checks_state, t.comments_count, t.created_at, t.updated_at,
                            t.synced_at, t.read_state, r.slug
                     FROM forge_topics t JOIN forge_repos r ON r.id=t.repo_id
                     WHERE t.repo_id=? AND t.number=?"
                    (list repo-id number))))
        (and (pair? rows) (row->topic (car rows)))))

    (define (forge-store-topics db #:repo-id [repo-id #f] #:kind [kind #f] #:state [state #f]
                                #:author [author #f] #:label [label #f] #:assignee [assignee #f]
                                #:read-state [read-state #f] #:query [query #f] #:limit [limit 200])
      "Query cached topics in DB matching optional filters up to LIMIT.
Filters by REPO-ID, KIND, STATE, AUTHOR, LABEL, ASSIGNEE, READ-STATE,
and FTS5 full-text QUERY."
      (let ((clauses '()) (params '()))
        (define (add! sql val)
          (set! clauses (append clauses (list sql)))
          (set! params (append params (list val))))
        (when repo-id (add! "t.repo_id=?" repo-id))
        (when (and kind (not (eq? kind 'all)))
          (add! "t.kind=?" (if (symbol? kind) (symbol->string kind) kind)))
        (when (and state (not (eq? state 'all)))
          (add! "t.state=?" (if (symbol? state) (symbol->string state) state)))
        (when (and (string? author) (> (string-length author) 0))
          (add! "lower(t.author)=lower(?)" author))
        (when (and (string? label) (> (string-length label) 0))
          (add! "EXISTS (SELECT 1 FROM forge_topic_labels l WHERE l.topic_id=t.id AND lower(l.label)=lower(?))" label))
        (when (and (string? assignee) (> (string-length assignee) 0))
          (add! "EXISTS (SELECT 1 FROM forge_topic_people p WHERE p.topic_id=t.id AND p.role='assignee' AND lower(p.login)=lower(?))" assignee))
        (when (and read-state (not (eq? read-state 'all)))
          (add! "t.read_state=?" (if (symbol? read-state) (symbol->string read-state) read-state)))
        (when (and (string? query) (> (string-length (string-trim query)) 0))
          (let ((fts (string-join
                      (filter-map (lambda (w)
                                    (let ((clean (list->string
                                                  (filter (lambda (c) (not (memv c '(#\" #\' #\* #\: #\( #\) #\^))))
                                                          (string->list w)))))
                                      (and (> (string-length clean) 0) (string-append "\"" clean "\"*"))))
                                  (string-split (string-trim query) " "))
                      " ")))
            (when (> (string-length fts) 0)
              (add! "t.id IN (SELECT rowid FROM forge_search WHERE forge_search MATCH ?)" fts))))
        (let* ((where (if (null? clauses) "" (string-append " WHERE " (string-join clauses " AND "))))
               (sql (string-append
                     "SELECT t.id, t.repo_id, t.kind, t.number, t.state, t.draft, t.title, t.body, t.author,
                             t.base_ref, t.head_ref, t.head_repo, t.head_sha, t.base_sha, t.mergeable,
                             t.review_decision, t.checks_state, t.comments_count, t.created_at, t.updated_at,
                             t.synced_at, t.read_state, r.slug
                      FROM forge_topics t JOIN forge_repos r ON r.id=t.repo_id"
                     where
                     " ORDER BY t.updated_at DESC, t.number DESC LIMIT ?")))
          (map row->topic (sqlite-query db sql (append params (list limit)))))))

    (define (forge-store-remove-topic! db topic-id)
      "Remove TOPIC-ID and all associated rows from DB."
      (sqlite-execute db "DELETE FROM forge_posts WHERE topic_id=?" (list topic-id))
      (sqlite-execute db "DELETE FROM forge_topic_labels WHERE topic_id=?" (list topic-id))
      (sqlite-execute db "DELETE FROM forge_topic_people WHERE topic_id=?" (list topic-id))
      (sqlite-execute db "DELETE FROM forge_checks WHERE topic_id=?" (list topic-id))
      (sqlite-execute db "DELETE FROM forge_review_drafts WHERE topic_id=?" (list topic-id))
      (sqlite-execute db "DELETE FROM forge_topics WHERE id=?" (list topic-id)))

    (define (forge-store-set-read-state! db topic-id read-state)
      "Set READ-STATE ('unread, 'pending, or 'done) of TOPIC-ID in DB."
      (sqlite-execute db "UPDATE forge_topics SET read_state=? WHERE id=?"
                      (list (if (symbol? read-state) (symbol->string read-state) read-state) topic-id)))

    (define (forge-store-topic-counts db repo-id)
      "Return open counts ((pullreqs . N) (issues . M)) for REPO-ID in DB."
      (let ((rows (sqlite-query db
                    "SELECT kind, COUNT(*) FROM forge_topics WHERE repo_id=? AND state='open' GROUP BY kind"
                    (list repo-id))))
        (let ((prs 0) (issues 0))
          (for-each (lambda (r)
                      (cond ((equal? (car r) "pullreq") (set! prs (cadr r)))
                            ((equal? (car r) "issue") (set! issues (cadr r)))))
                    rows)
          `((pullreqs . ,prs) (issues . ,issues)))))

    (define (row->post r)
      `((id . ,(list-ref r 0))
        (topic-id . ,(list-ref r 1))
        (remote-id . ,(list-ref r 2))
        (kind . ,(string->symbol (list-ref r 3)))
        (author . ,(list-ref r 4))
        (body . ,(list-ref r 5))
        (path . ,(list-ref r 6))
        (line . ,(list-ref r 7))
        (original-line . ,(list-ref r 8))
        (side . ,(list-ref r 9))
        (commit-id . ,(list-ref r 10))
        (in-reply-to . ,(list-ref r 11))
        (review-state . ,(list-ref r 12))
        (created-at . ,(list-ref r 13))
        (updated-at . ,(list-ref r 14))))

    (define (forge-store-upsert-post! db topic-id post)
      "Insert or update POST under TOPIC-ID in DB."
      (let ((remote-id (let ((rid (forge-ref post 'remote-id "")))
                         (if (number? rid) (number->string rid) (sval rid))))
            (kind (let ((k (forge-ref post 'kind 'comment))) (if (symbol? k) (symbol->string k) (sval k))))
            (author (sval (forge-ref post 'author "")))
            (body (sval (forge-ref post 'body "")))
            (path (sval (forge-ref post 'path "")))
            (line (ival (forge-ref post 'line 0)))
            (original-line (ival (forge-ref post 'original-line 0)))
            (side (sval (forge-ref post 'side "")))
            (commit-id (sval (forge-ref post 'commit-id "")))
            (in-reply-to (let ((r (forge-ref post 'in-reply-to "")))
                           (if (number? r) (number->string r) (sval r))))
            (review-state (sval (forge-ref post 'review-state "")))
            (created-at (sval (forge-ref post 'created-at "")))
            (updated-at (sval (forge-ref post 'updated-at ""))))
        (sqlite-execute db
          "INSERT INTO forge_posts(topic_id, remote_id, kind, author, body, path, line, original_line,
             side, commit_id, in_reply_to, review_state, created_at, updated_at)
           VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(topic_id, remote_id) DO UPDATE SET
             kind=excluded.kind, author=excluded.author, body=excluded.body,
             path=excluded.path, line=excluded.line, original_line=excluded.original_line,
             side=excluded.side, commit_id=excluded.commit_id,
             in_reply_to=excluded.in_reply_to, review_state=excluded.review_state,
             updated_at=excluded.updated_at"
          (list topic-id remote-id kind author body path line original-line
                side commit-id in-reply-to review-state created-at updated-at))))

    (define (forge-store-posts db topic-id #:kind [kind #f])
      "Return cached posts in DB for TOPIC-ID, optionally of KIND."
      (let ((rows (if kind
                      (sqlite-query db
                        "SELECT id, topic_id, remote_id, kind, author, body, path, line, original_line,
                                side, commit_id, in_reply_to, review_state, created_at, updated_at
                         FROM forge_posts WHERE topic_id=? AND kind=? ORDER BY created_at ASC, id ASC"
                        (list topic-id (if (symbol? kind) (symbol->string kind) kind)))
                      (sqlite-query db
                        "SELECT id, topic_id, remote_id, kind, author, body, path, line, original_line,
                                side, commit_id, in_reply_to, review_state, created_at, updated_at
                         FROM forge_posts WHERE topic_id=? ORDER BY created_at ASC, id ASC"
                        (list topic-id)))))
        (map row->post rows)))

    (define (forge-store-delete-post! db topic-id remote-id)
      "Delete post REMOTE-ID from TOPIC-ID in DB."
      (sqlite-execute db "DELETE FROM forge_posts WHERE topic_id=? AND remote_id=?"
                      (list topic-id (if (number? remote-id) (number->string remote-id) remote-id))))

    (define (forge-store-set-topic-labels! db topic-id labels)
      "Replace cached LABELS for TOPIC-ID in DB."
      (sqlite-execute db "DELETE FROM forge_topic_labels WHERE topic_id=?" (list topic-id))
      (for-each (lambda (lbl)
                  (when (and (string? lbl) (> (string-length lbl) 0))
                    (sqlite-execute db "INSERT OR IGNORE INTO forge_topic_labels(topic_id, label) VALUES(?, ?)"
                                    (list topic-id lbl))))
                labels))

    (define (forge-store-topic-labels db topic-id)
      "Return the list of label names on TOPIC-ID in DB."
      (map car (sqlite-query db "SELECT label FROM forge_topic_labels WHERE topic_id=? ORDER BY label ASC" (list topic-id))))

    (define (forge-store-upsert-repo-labels! db repo-id labels)
      "Replace known repository LABELS for REPO-ID in DB.
Each entry in LABELS is an alist with keys name, color, description."
      (sqlite-execute db "DELETE FROM forge_labels WHERE repo_id=?" (list repo-id))
      (for-each (lambda (l)
                  (let ((name (sval (forge-ref l 'name ""))))
                    (when (> (string-length name) 0)
                      (sqlite-execute db "INSERT OR REPLACE INTO forge_labels(repo_id, name, color, description) VALUES(?, ?, ?, ?)"
                                      (list repo-id name (sval (forge-ref l 'color "")) (sval (forge-ref l 'description "")))))))
                labels))

    (define (forge-store-repo-labels db repo-id)
      "Return known labels for REPO-ID in DB as a list of alists."
      (map (lambda (r) `((name . ,(list-ref r 0)) (color . ,(list-ref r 1)) (description . ,(list-ref r 2))))
           (sqlite-query db "SELECT name, color, description FROM forge_labels WHERE repo_id=? ORDER BY name ASC" (list repo-id))))

    (define (forge-store-set-topic-people! db topic-id role logins)
      "Replace cached LOGINS for TOPIC-ID and ROLE in DB."
      (let ((role-s (if (symbol? role) (symbol->string role) role)))
        (sqlite-execute db "DELETE FROM forge_topic_people WHERE topic_id=? AND role=?" (list topic-id role-s))
        (for-each (lambda (login)
                    (when (and (string? login) (> (string-length login) 0))
                      (sqlite-execute db "INSERT OR IGNORE INTO forge_topic_people(topic_id, role, login) VALUES(?, ?, ?)"
                                      (list topic-id role-s login))))
                  logins)))

    (define (forge-store-topic-people db topic-id role)
      "Return logins for TOPIC-ID and ROLE ('assignee or 'reviewer) in DB."
      (map car (sqlite-query db "SELECT login FROM forge_topic_people WHERE topic_id=? AND role=? ORDER BY login ASC"
                             (list topic-id (if (symbol? role) (symbol->string role) role)))))

    (define (forge-store-set-checks! db topic-id head-sha checks)
      "Replace cached CI CHECKS for TOPIC-ID at HEAD-SHA in DB."
      (sqlite-execute db "DELETE FROM forge_checks WHERE topic_id=? AND head_sha=?" (list topic-id head-sha))
      (for-each (lambda (c)
                  (sqlite-execute db "INSERT INTO forge_checks(topic_id, head_sha, name, status, conclusion, details_url) VALUES(?, ?, ?, ?, ?, ?)"
                                  (list topic-id head-sha (sval (forge-ref c 'name ""))
                                        (sval (forge-ref c 'status "")) (sval (forge-ref c 'conclusion ""))
                                        (sval (forge-ref c 'details-url "")))))
                checks))

    (define (forge-store-checks db topic-id head-sha)
      "Return cached CI check runs for TOPIC-ID at HEAD-SHA in DB."
      (map (lambda (r)
             `((name . ,(list-ref r 0)) (status . ,(list-ref r 1))
               (conclusion . ,(list-ref r 2)) (details-url . ,(list-ref r 3))))
           (sqlite-query db "SELECT name, status, conclusion, details_url FROM forge_checks WHERE topic_id=? AND head_sha=? ORDER BY id ASC"
                         (list topic-id head-sha))))

    (define (forge-store-upsert-notification! db notif)
      "Insert or update GitHub notification NOTIF in DB."
      (sqlite-execute db
        "INSERT INTO forge_notifications(id, host, owner, repo_name, slug, topic_kind, topic_number, title, reason, unread, updated_at)
         VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
           title=excluded.title, reason=excluded.reason, unread=excluded.unread, updated_at=excluded.updated_at"
        (list (sval (forge-ref notif 'id ""))
              (sval (forge-ref notif 'host "github.com"))
              (sval (forge-ref notif 'owner ""))
              (sval (forge-ref notif 'repo-name ""))
              (string-downcase (sval (forge-ref notif 'slug "")))
              (let ((k (forge-ref notif 'topic-kind 'issue))) (if (symbol? k) (symbol->string k) (sval k)))
              (ival (forge-ref notif 'topic-number 0))
              (sval (forge-ref notif 'title ""))
              (sval (forge-ref notif 'reason ""))
              (bval (forge-ref notif 'unread #t))
              (sval (forge-ref notif 'updated-at "")))))

    (define (forge-store-notifications db #:unread-only [unread-only #f])
      "Return cached notifications in DB, filtered when UNREAD-ONLY is true."
      (let ((rows (sqlite-query db
                    (if unread-only
                        "SELECT id, host, owner, repo_name, slug, topic_kind, topic_number, title, reason, unread, updated_at
                         FROM forge_notifications WHERE unread=1 ORDER BY updated_at DESC"
                        "SELECT id, host, owner, repo_name, slug, topic_kind, topic_number, title, reason, unread, updated_at
                         FROM forge_notifications ORDER BY updated_at DESC")
                    '())))
        (map (lambda (r)
               `((id . ,(list-ref r 0))
                 (host . ,(list-ref r 1))
                 (owner . ,(list-ref r 2))
                 (repo-name . ,(list-ref r 3))
                 (slug . ,(list-ref r 4))
                 (topic-kind . ,(string->symbol (list-ref r 5)))
                 (topic-number . ,(list-ref r 6))
                 (title . ,(list-ref r 7))
                 (reason . ,(list-ref r 8))
                 (unread . ,(= (list-ref r 9) 1))
                 (updated-at . ,(list-ref r 10))))
             rows)))

    (define (forge-store-mark-notification-read! db id)
      "Mark notification ID as read in DB."
      (sqlite-execute db "UPDATE forge_notifications SET unread=0 WHERE id=?" (list id)))

    (define (forge-store-save-draft! db id repo-id kind title body #:topic-number [topic-number 0]
                                     #:post-remote-id [post-remote-id ""] #:meta-json [meta-json "{}"]
                                     #:updated-at [updated-at ""])
      "Save compose draft ID for REPO-ID of KIND with TITLE and BODY in DB.
Optional TOPIC-NUMBER, POST-REMOTE-ID, META-JSON, and UPDATED-AT."
      (sqlite-execute db
        "INSERT INTO forge_drafts(id, repo_id, kind, topic_number, post_remote_id, title, body, meta_json, updated_at)
         VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
           title=excluded.title, body=excluded.body, meta_json=excluded.meta_json, updated_at=excluded.updated_at"
        (list id repo-id (if (symbol? kind) (symbol->string kind) kind)
              topic-number post-remote-id title body meta-json updated-at)))

    (define (forge-store-draft db id)
      "Return the saved compose draft for ID in DB, or #f."
      (let ((rows (sqlite-query db
                    "SELECT id, repo_id, kind, topic_number, post_remote_id, title, body, meta_json, updated_at
                     FROM forge_drafts WHERE id=?"
                    (list id))))
        (and (pair? rows)
             (let ((r (car rows)))
               `((id . ,(list-ref r 0))
                 (repo-id . ,(list-ref r 1))
                 (kind . ,(string->symbol (list-ref r 2)))
                 (topic-number . ,(list-ref r 3))
                 (post-remote-id . ,(list-ref r 4))
                 (title . ,(list-ref r 5))
                 (body . ,(list-ref r 6))
                 (meta-json . ,(list-ref r 7))
                 (updated-at . ,(list-ref r 8)))))))

    (define (forge-store-delete-draft! db id)
      "Delete compose draft ID from DB."
      (sqlite-execute db "DELETE FROM forge_drafts WHERE id=?" (list id)))

    ;; Pending code-review comments (persisted locally until submitted as a PR review).
    (define (row->review-draft r)
      `((id . ,(list-ref r 0))
        (topic-id . ,(list-ref r 1))
        (head-sha . ,(list-ref r 2))
        (path . ,(list-ref r 3))
        (line . ,(list-ref r 4))
        (start-line . ,(list-ref r 5))
        (side . ,(list-ref r 6))
        (body . ,(list-ref r 7))
        (in-reply-to . ,(list-ref r 8))
        (created-at . ,(list-ref r 9))))

    (define (forge-store-add-review-comment! db topic-id head-sha path line body
                                             #:start-line [start-line 0] #:side [side "RIGHT"]
                                             #:in-reply-to [in-reply-to ""] #:created-at [created-at ""])
      "Add a pending code-review comment in DB for TOPIC-ID at PATH:LINE.
Records HEAD-SHA, BODY, START-LINE, SIDE, IN-REPLY-TO, and CREATED-AT."
      (sqlite-execute db
        "INSERT INTO forge_review_drafts(topic_id, head_sha, path, line, start_line, side, body, in_reply_to, created_at)
         VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?)"
        (list topic-id head-sha path line start-line side body in-reply-to created-at))
      (let ((rows (sqlite-query db "SELECT last_insert_rowid()" '())))
        (and (pair? rows) (caar rows))))

    (define (forge-store-review-comments db topic-id #:head-sha [head-sha #f])
      "Return pending code-review comments in DB for TOPIC-ID and HEAD-SHA."
      (map row->review-draft
           (if head-sha
               (sqlite-query db
                 "SELECT id, topic_id, head_sha, path, line, start_line, side, body, in_reply_to, created_at
                  FROM forge_review_drafts WHERE topic_id=? AND head_sha=? ORDER BY path ASC, line ASC, id ASC"
                 (list topic-id head-sha))
               (sqlite-query db
                 "SELECT id, topic_id, head_sha, path, line, start_line, side, body, in_reply_to, created_at
                  FROM forge_review_drafts WHERE topic_id=? ORDER BY path ASC, line ASC, id ASC"
                 (list topic-id)))))

    (define (forge-store-update-review-comment! db id body)
      "Set BODY of pending code-review comment ID in DB."
      (sqlite-execute db "UPDATE forge_review_drafts SET body=? WHERE id=?" (list body id)))

    (define (forge-store-delete-review-comment! db id)
      "Delete pending code-review comment ID from DB."
      (sqlite-execute db "DELETE FROM forge_review_drafts WHERE id=?" (list id)))

    (define (forge-store-delete-review-comment-if-unchanged! db id body)
      "Delete pending review comment ID from DB only if its BODY matches."
      (sqlite-execute db "DELETE FROM forge_review_drafts WHERE id=? AND body=?" (list id body)))

    (define (forge-store-clear-review-comments! db topic-id)
      "Delete all pending code-review comments for TOPIC-ID from DB."
      (sqlite-execute db "DELETE FROM forge_review_drafts WHERE topic_id=?" (list topic-id)))))
