;;; Forge & Code Review: GitHub issues, pull requests, notifications, and
;;; interactive pull-request code review for Techne (Git and Jujutsu).
(import (techne editor) (techne time) (techne path) (techne task)
        (forge auth) (forge remote) (forge http) (forge store) (forge github) (forge diff))

(provide forge-reset-db! forge-set-test-transport!)

;; Register Forge's `gh-cli` backend in this package's scope.
(forge-register-secret-backends!)

;;; Faces

(define-face forge.heading "Heading in Forge and Code Review buffers." #:fg 'blue #:weight 'bold #:height 1.2)
(define-face forge.section "Foldable section heading in Forge and VC status buffers." #:fg 'yellow #:weight 'bold)
(define-face forge.number "Issue or pull request number." #:fg 'mauve #:weight 'bold)
(define-face forge.open "Open topic badge." #:fg 'green #:weight 'bold)
(define-face forge.closed "Closed topic badge." #:fg 'red #:weight 'bold)
(define-face forge.merged "Merged pull request badge." #:fg 'mauve #:weight 'bold)
(define-face forge.draft "Draft pull request badge." #:fg 'overlay2)
(define-face forge.label "Topic label pill." #:fg 'peach)
(define-face forge.author "Topic or comment author." #:fg 'sky #:weight 'bold)
(define-face forge.meta "Metadata and timestamps." #:fg 'overlay2)
(define-face forge.review-approved "Approved review state." #:fg 'green #:weight 'bold)
(define-face forge.review-changes "Changes requested review state." #:fg 'red #:weight 'bold)
(define-face forge.review-pending "Local unsent code-review comment." #:fg 'yellow #:weight 'bold)
(define-face forge.diff-add "Added line in a pull request diff." #:fg 'green)
(define-face forge.diff-del "Removed line in a pull request diff." #:fg 'red)
(define-face forge.diff-hunk "Hunk header in a pull request diff." #:fg 'blue #:weight 'bold)
(define-face forge.file "Changed filename in a code review." #:fg 'sky #:weight 'bold)
(define-face forge.check-ok "Passing CI check." #:fg 'green)
(define-face forge.check-fail "Failing CI check." #:fg 'red #:weight 'bold)
(define-face forge.check-pending "In-progress or queued CI check." #:fg 'yellow)
(define-face forge.unread "Unread notification or topic." #:fg 'text #:weight 'bold)

;;; State & Utilities

(define %db #f)
(define %dir-repo-cache (make-hash-table))
(define %forge-transport #f)
(define %forge-scheme "https")
(define %forge-port #f)

(define (buffer-read-slot b)
  (let ((st (and b (buffer-state b))))
    (if (hash-table? st)
        (or (hash-table-ref/default st 'read-slot #f)
            (let ((slot (make-request-slot)))
              (hash-table-set! st 'read-slot slot)
              slot))
        (make-request-slot))))

(define (safe-text text)
  (if (not (string? text))
      ""
      (list->string
       (map (lambda (c)
              (let ((n (char->integer c)))
                (cond ((memv c '(#\newline #\tab)) c)
                      ((< n 32) (integer->char (+ 9216 n)))
                      ((= n 127) #\x2421)
                      ((or (<= 8234 n 8238) (<= 8294 n 8297)) #\xFFFD)
                      (else c))))
            (string->list text)))))

(define (one-line text)
  (list->string (map (lambda (c) (if (memv c '(#\newline #\tab #\return)) #\space c))
                     (string->list (safe-text text)))))

(define (short text n)
  (let ((t (one-line text)))
    (if (> (string-length t) n) (string-append (substring t 0 (- n 1)) "…") t)))

(define (ensure-directory! path)
  (unless (file-exists? path)
    (let ((parent (path-directory path)))
      (unless (or (string=? parent path) (string=? parent "")) (ensure-directory! parent))
      (create-directory path #o700))))

(define (forge-db!)
  (or %db
      (let* ((path (or (option #f 'forge-database)
                       (string-append (or (get-environment-variable "XDG_DATA_HOME")
                                          (string-append (or (get-environment-variable "HOME") ".") "/.local/share"))
                                      "/techne/forge/cache.sqlite"))))
        (unless (string=? path ":memory:") (ensure-directory! (path-directory path)))
        (set! %db (forge-store-open path))
        %db)))

(define (forge-set-test-transport! transport #:scheme [scheme "https"] #:port [port #f])
  "Configure HTTP TRANSPORT with SCHEME and PORT for Forge tests."
  (set! %forge-transport transport)
  (set! %forge-scheme scheme)
  (set! %forge-port port))

(define (forge-reset-db! #:path [path #f])
  "Close the Forge database and optionally set `forge-database` to PATH."
  (when %db (forge-store-close %db) (set! %db #f))
  (set! %dir-repo-cache (make-hash-table))
  (when path (set-option! 'forge-database path)))

(define (now-iso)
  (guard (e (#t ""))
    (format-time (current-time) "%Y-%m-%dT%H:%M:%SZ")))

(define (get-alist)
  (or (option #f 'forge-alist) forge-default-alist))

(define (token-for repo)
  (let ((api-host (forge-ref repo 'api-host "api.github.com"))
        (web-host (forge-ref repo 'web-host "github.com")))
    (forge-token api-host
                 #:web-host web-host
                 #:provider (option #f 'forge-credential-provider)
                 #:sources (option #f 'secret-sources)
                 #:allow-gh-cli (option #f 'forge-allow-gh-cli-token))))

(define (run-proc dir prog args)
  (call-with-process-at dir prog args
    (lambda (p)
      (let* ((err-t (spawn (lambda () (process-read-all p 'stderr))))
             (out (process-read-all p 'stdout))
             (err (task-join err-t))
             (code (process-wait p)))
        (if (= code 0)
            (string-trim (or out ""))
            (error (string-append prog " failed: " (string-trim (if (and (string? err) (> (string-length err) 0)) err (or out ""))))))))))

(define (detect-vcs-tool dir)
  (guard (e (#t 'git))
    (run-proc dir "jj" '("root"))
    'jj))

(define (current-forge-repo s #:require [require? #t])
  (let* ((b (session-buffer s))
         (st (and b (buffer-state b)))
         (from-buf (and (hash-table? st) (hash-table-ref/default st 'forge-repo #f))))
    (or from-buf
        (let* ((dir (default-directory s))
               (override (and b (option b 'vc-forge-repository)))
               (detected (forge-detect-repository dir #:override override #:alist (get-alist))))
          (cond
            (detected
             (let* ((stored (forge-store-repo (forge-db!) (forge-ref detected 'web-host "github.com")
                                              (forge-ref detected 'slug "")))
                    (res (cons (cons 'local-dir dir) (or stored detected))))
               (hash-table-set! %dir-repo-cache dir res)
               res))
            (require? (error "No GitHub repository detected; run M-x forge-add-repository or set vc-forge-repository"))
            (else #f))))))

(define (ensure-stored-repo! repo)
  (let* ((db (forge-db!))
         (stored (forge-store-upsert-repo! db repo)))
    (if (forge-ref repo 'local-dir #f)
        (cons (cons 'local-dir (forge-ref repo 'local-dir)) stored)
        stored)))

(define (open-in-browser! s url)
  (spawn (lambda ()
           (guard (e (#t #f))
             (call-with-process-at (default-directory s) "xdg-open" (list url)
               (lambda (p) (process-wait p))
               #:policy 'user-trusted))))
  (message! s (string-append "Opening " url)))

(define (state-face topic)
  (cond ((forge-ref topic 'draft #f) 'forge.draft)
        ((eq? (forge-ref topic 'state 'open) 'merged) 'forge.merged)
        ((eq? (forge-ref topic 'state 'open) 'closed) 'forge.closed)
        (else 'forge.open)))

(define (state-badge topic)
  (cond ((forge-ref topic 'draft #f) "draft")
        ((eq? (forge-ref topic 'state 'open) 'merged) "merged")
        ((eq? (forge-ref topic 'state 'open) 'closed) "closed")
        (else "open")))

(define (topic-summary-row repo topic #:show-slug [show-slug #f])
  (let* ((num (forge-ref topic 'number 0))
         (kind (forge-ref topic 'kind 'issue))
         (prefix (if (eq? kind 'pullreq) "PR " ""))
         (title (short (forge-ref topic 'title "") 78))
         (author (one-line (forge-ref topic 'author "")))
         (slug (forge-ref topic 'repo-slug ""))
         (labels (forge-store-topic-labels (forge-db!) (forge-ref topic 'id)))
         (unread? (eq? (forge-ref topic 'read-state 'unread) 'unread)))
    (row (append
          (list "  "
                (list (string-append prefix "#" (number->string num)) 'forge.number)
                " "
                (list (string-append "[" (state-badge topic) "]") (state-face topic))
                " ")
          (if (and show-slug (> (string-length slug) 0))
              (list (list (string-append slug " ") 'forge.meta))
              '())
          (list (if unread? (list title 'forge.unread) title))
          (if (> (string-length author) 0)
              (list (list (string-append "  @" author) 'forge.meta))
              '())
          (if (pair? labels)
              (list (list (string-append "  [" (string-join labels ", ") "]") 'forge.label))
              '()))
         #:key (list 'topic (forge-ref topic 'repo-id) num)
         #:target (target 'forge-topic (list repo topic)))))

(define (step-target! s direction)
  (let* ((p (doc s)) (b (session-buffer s))
         (rows (hash-table-ref/default (buffer-state b) 'rows #f))
         (at (point s)))
    (when rows
      (let* ((spans (filter (lambda (sp)
                              (let ((r (row-at p rows (cadr sp))))
                                (and r (row-target r))))
                            (presentation-rows-between p 0 (document-length p))))
             (found (if (> direction 0)
                        (find (lambda (sp) (> (cadr sp) at)) spans)
                        (find (lambda (sp) (< (caddr sp) at)) (reverse spans)))))
        (when found (move! s (lambda (_) (cadr found))))))))

(define (target-at-point s)
  (target-at (doc s) (point s)))

(define (forge-topic-at-point s)
  (let* ((b (session-buffer s))
         (st (and b (buffer-state b)))
         (t (target-at-point s)))
    (cond
      ((and t (eq? (target-kind t) 'forge-topic)) (target-value t))
      ((and (hash-table? st) (hash-table-ref/default st 'forge-topic #f))
       (list (hash-table-ref st 'forge-repo) (hash-table-ref st 'forge-topic)))
      (else #f))))

(define (refresh-vc-status-buffers!)
  (for-each (lambda (b)
              (when (memq (buffer-mode b) '(vc-git-status-mode vc-jj-status-mode))
                (let ((st (buffer-state b)))
                  (when (hash-table? st)
                    (let ((s (current-session)))
                      (when (and s (eq? (session-buffer s) b))
                        (guard (e (#t #f)) (run-command s 'vc-status-refresh 1))))))))
            (buffer-list)))

;;; VC Status Integration (foldable Pull requests & Issues sections, cache-only)

(define (vc-forge-status-rows s repo st closed add-rows!)
  ;; Never spawn git/jj or open a cold DB on the synchronous status render path.
  (when (and %db (option #f 'forge-status-sections))
    (guard (e (#t #f))
      (let* ((dir (cadr repo))
             (cached (hash-table-ref/default %dir-repo-cache dir #f))
             (stored (and cached
                          (forge-store-repo %db (forge-ref cached 'web-host "github.com")
                                            (forge-ref cached 'slug "")))))
        (when stored
          (let* ((limit (let ((v (option #f 'forge-status-topic-limit)))
                          (if (and (integer? v) (> v 0)) v 10)))
                 (repo-with-dir (cons (cons 'local-dir dir) stored))
                 (repo-id (forge-ref stored 'id))
                 (prs (forge-store-topics %db #:repo-id repo-id #:kind 'pullreq #:state 'open #:limit limit))
                 (issues (forge-store-topics %db #:repo-id repo-id #:kind 'issue #:state 'open #:limit limit))
                 (pr-closed? (memq 'forge-pullreqs closed))
                 (iss-closed? (memq 'forge-issues closed)))
            (when (or (pair? prs) (pair? issues))
              (add-rows!
               (append
                (list (row "" #:key 'forge-gap))
                (list (row (list (list (string-append "Pull requests (" (number->string (length prs)) ")") 'forge.section)
                                 (list (if pr-closed? "  …" "") 'shadow))
                           #:key '(section forge-pullreqs) #:target (target 'vc-section 'forge-pullreqs)))
                (if pr-closed? '() (map (lambda (t) (topic-summary-row repo-with-dir t)) prs))
                (list (row (list (list (string-append "Issues (" (number->string (length issues)) ")") 'forge.section)
                                 (list (if iss-closed? "  …" "") 'shadow))
                           #:key '(section forge-issues) #:target (target 'vc-section 'forge-issues)))
                (if iss-closed? '() (map (lambda (t) (topic-summary-row repo-with-dir t)) issues)))))))))))

(add-hook! 'vc-status-sections 'forge-status-rows vc-forge-status-rows)

;;; Target Actions

(define-action forge-topic (forge-visit-topic-action s val)
  "Inspect this Forge topic in its own buffer."
  (open-topic-buffer! s (car val) (cadr val)))

(define-action forge-topic (forge-code-review-action s val)
  "Open this pull request in the interactive code-review buffer."
  (let ((repo (car val)) (topic (cadr val)))
    (if (eq? (forge-ref topic 'kind 'issue) 'pullreq)
        (open-code-review! s repo topic)
        (message! s "Code review is only for pull requests"))))

(define-action forge-topic (forge-browse-topic-action s val)
  "Open this Forge topic in the web browser."
  (let ((repo (car val)) (topic (cadr val)))
    (open-in-browser! s (forge-topic-web-url (forge-ref repo 'web-host "github.com")
                                             (forge-ref repo 'owner "") (forge-ref repo 'name "")
                                             (forge-ref topic 'kind 'issue) (forge-ref topic 'number 0)))))

;;; Topic List Buffer

(define-mode forge-topic-list-mode
  "Browse and filter cached GitHub issues and pull requests."
  #:parent 'special-mode
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("RET" forge-list-visit-at-point) ("n" forge-next-item) ("p" forge-previous-item)
           ("C-c C-c" forge-topics-menu) ("/" forge-topics-menu) ("m" forge-topic-menu)
           ("r" code-review-forge-pr-at-point) ("c" forge-create-issue) ("P" forge-create-pullreq)
           ("g" forge-pull) ("o" forge-browse) ("w" forge-copy-url) ("N" forge-dispatch) ("q" quit-window))
  #:normal '(("RET" forge-list-visit-at-point) ("n" forge-next-item) ("p" forge-previous-item)
             ("C-c C-c" forge-topics-menu) ("/" forge-topics-menu) ("m" forge-topic-menu)
             ("r" code-review-forge-pr-at-point) ("c" forge-create-issue) ("P" forge-create-pullreq)
             ("g r" forge-pull) ("o" forge-browse) ("w" forge-copy-url) ("N" forge-dispatch) ("q" quit-window)))

(define (render-topic-list! b)
  (let* ((st (buffer-state b))
         (repo (hash-table-ref/default st 'forge-repo #f))
         (global? (hash-table-ref/default st 'global? #f))
         (kind (hash-table-ref/default st 'filter-kind 'all))
         (state (hash-table-ref/default st 'filter-state 'open))
         (author (hash-table-ref/default st 'filter-author #f))
         (label (hash-table-ref/default st 'filter-label #f))
         (assignee (hash-table-ref/default st 'filter-assignee #f))
         (query (hash-table-ref/default st 'filter-query #f))
         (db (forge-db!))
         (repos-by-id (let ((ht (make-hash-table)))
                        (for-each (lambda (r) (hash-table-set! ht (forge-ref r 'id) r)) (forge-store-repos db))
                        ht))
         (topics (forge-store-topics db
                   #:repo-id (and (not global?) repo (forge-ref repo 'id))
                   #:kind kind #:state state #:author author #:label label #:assignee assignee #:query query))
         (title (if global?
                    "Forge · All tracked repositories"
                    (string-append "Forge · " (forge-ref repo 'slug ""))))
         (filter-desc (string-join
                       (filter (lambda (x) x)
                               (list (string-append "kind:" (symbol->string kind))
                                     (string-append "state:" (symbol->string state))
                                     (and author (string-append "author:" author))
                                     (and label (string-append "label:" label))
                                     (and assignee (string-append "assignee:" assignee))
                                     (and query (string-append "search:\"" query "\""))))
                       " · ")))
    (hash-table-set! st 'rows
      (present! (buffer-document b)
        (append
         (list (row (list (list title 'forge.heading)) #:key 'head)
               (row (list (list (string-append filter-desc "  (" (number->string (length topics)) " topics)") 'forge.meta)) #:key 'filter)
               (row (list (list "RET visit · r code-review · / filter · m topic menu · c issue · P PR · g pull · o browse · q quit" 'shadow)) #:key 'hint)
               (row "" #:key 'gap))
         (if (null? topics)
             (list (row (list (list "No matching topics in local cache. Press g to pull from GitHub." 'shadow)) #:key 'empty))
             (map (lambda (t)
                    (let ((tr (if global?
                                  (or (hash-table-ref/default repos-by-id (forge-ref t 'repo-id) #f) repo)
                                  repo)))
                      (topic-summary-row tr t #:show-slug global?)))
                  topics)))))))

(define (open-topic-list! s repo #:kind [kind 'all] #:state [state 'open] #:global [global? #f])
  (let* ((stored (and repo (ensure-stored-repo! repo)))
         (bname (if global? "*forge topics*" (string-append "*forge " (forge-ref stored 'slug "") "*")))
         (b (or (buffer-named bname)
                (make-generated-buffer! bname (make-presentation) 'forge-topic-list-mode #:state (make-hash-table))))
         (st (buffer-state b)))
    (hash-table-set! st 'forge-repo stored)
    (hash-table-set! st 'global? global?)
    (hash-table-set! st 'filter-kind kind)
    (unless (hash-table-ref/default st 'filter-state #f)
      (hash-table-set! st 'filter-state state))
    (render-topic-list! b)
    (show-buffer! s b)))

(define-command (forge-next-item s n)
  "Move to the next interactive item in a Forge or Code Review buffer."
  (step-target! s 1))

(define-command (forge-previous-item s n)
  "Move to the previous interactive item in a Forge or Code Review buffer."
  (step-target! s -1))

(define-command (forge-list-visit-at-point s n)
  "Visit the topic at point in a Forge topic list."
  (let ((found (forge-topic-at-point s)))
    (if found
        (open-topic-buffer! s (car found) (cadr found))
        (message! s "No topic at point"))))

(define-command (forge-list-topics s n)
  "List cached issues and pull requests for the current repository."
  (open-topic-list! s (current-forge-repo s) #:kind 'all))

(define-command (forge-list-pullreqs s n)
  "List cached pull requests for the current repository."
  (open-topic-list! s (current-forge-repo s) #:kind 'pullreq))

(define-command (forge-list-issues s n)
  "List cached issues for the current repository."
  (open-topic-list! s (current-forge-repo s) #:kind 'issue))

(define-command (forge-list-global-topics s n)
  "List cached topics across all tracked repositories."
  (open-topic-list! s #f #:kind 'all #:global #t))

(define-command (forge-topics-menu s n)
  "Open the filter menu for the current Forge topic list."
  (let ((b (session-buffer s)))
    (unless (eq? (buffer-mode b) 'forge-topic-list-mode)
      (error "Open a Forge topic list first"))
    (let ((st (buffer-state b)))
      (define (set-f! k v) (hash-table-set! st k v) (render-topic-list! b))
      (transient! s
        `("Forge · Filter topics"
          ("Kind" (suffix "t" "All topics" ,(lambda (s _) (set-f! 'filter-kind 'all)))
                  (suffix "p" "Pull requests" ,(lambda (s _) (set-f! 'filter-kind 'pullreq)))
                  (suffix "i" "Issues" ,(lambda (s _) (set-f! 'filter-kind 'issue))))
          ("State" (suffix "o" "Open" ,(lambda (s _) (set-f! 'filter-state 'open)))
                   (suffix "c" "Closed" ,(lambda (s _) (set-f! 'filter-state 'closed)))
                   (suffix "m" "Merged" ,(lambda (s _) (set-f! 'filter-state 'merged)))
                   (suffix "a" "All states" ,(lambda (s _) (set-f! 'filter-state 'all))))
          ("Search & Metadata"
           (suffix "/" "Full-text search…"
                   ,(lambda (s _)
                      (completing-read s "Search topics (empty clears): " '() #:require-match #f
                        #:accept (lambda (s c)
                                   (let ((q (string-trim (candidate-text c))))
                                     (set-f! 'filter-query (and (> (string-length q) 0) q)))))))
           (suffix "l" "Filter by label…"
                   ,(lambda (s _)
                      (let* ((repo (hash-table-ref/default st 'forge-repo #f))
                             (cands (if repo (map (lambda (l) (forge-ref l 'name "")) (forge-store-repo-labels (forge-db!) (forge-ref repo 'id))) '())))
                        (completing-read s "Label (empty clears): " cands #:require-match #f
                          #:accept (lambda (s c)
                                     (let ((v (string-trim (candidate-text c))))
                                       (set-f! 'filter-label (and (> (string-length v) 0) v))))))))
           (suffix "A" "Filter by author…"
                   ,(lambda (s _)
                      (completing-read s "Author (empty clears): " '() #:require-match #f
                        #:accept (lambda (s c)
                                   (let ((v (string-trim (candidate-text c))))
                                     (set-f! 'filter-author (and (> (string-length v) 0) v)))))))
           (suffix "r" "Reset all filters"
                   ,(lambda (s _)
                      (hash-table-set! st 'filter-kind 'all)
                      (hash-table-set! st 'filter-state 'open)
                      (hash-table-set! st 'filter-author #f)
                      (hash-table-set! st 'filter-label #f)
                      (hash-table-set! st 'filter-assignee #f)
                      (hash-table-set! st 'filter-query #f)
                      (render-topic-list! b)))))))))

;;; Topic Detail Buffer (`forge-topic-mode`)

(define-mode forge-topic-mode
  "Inspect a GitHub issue or pull request, its posts, checks, and actions."
  #:parent 'special-mode
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("C-c C-c" forge-topic-menu) ("m" forge-topic-menu)
           ("C-c C-n" forge-create-post) ("r" forge-create-post)
           ("C-c C-e" forge-edit-post) ("e" forge-edit-post)
           ("C-c C-k" forge-delete-comment) ("k" forge-delete-comment)
           ("v" code-review-forge-pr-at-point) ("d" code-review-forge-pr-at-point)
           ("b" forge-checkout-pullreq) ("g" forge-refresh-current-topic)
           ("o" forge-browse) ("w" forge-copy-url)
           ("TAB" forge-topic-toggle-fold) ("n" forge-next-item) ("p" forge-previous-item)
           ("N" forge-dispatch) ("q" quit-window))
  #:normal '(("C-c C-c" forge-topic-menu) ("m" forge-topic-menu)
             ("C-c C-n" forge-create-post) ("r" forge-create-post)
             ("C-c C-e" forge-edit-post) ("e" forge-edit-post)
             ("C-c C-k" forge-delete-comment) ("k" forge-delete-comment)
             ("v" code-review-forge-pr-at-point) ("d" code-review-forge-pr-at-point)
             ("b" forge-checkout-pullreq) ("g r" forge-refresh-current-topic)
             ("o" forge-browse) ("w" forge-copy-url)
             ("TAB" forge-topic-toggle-fold) ("n" forge-next-item) ("p" forge-previous-item)
             ("N" forge-dispatch) ("q" quit-window)))

(define (body-lines text indent key-prefix)
  (let* ((safe (safe-text text))
         (ls (if (string=? (string-trim safe) "") '("(empty)") (string-split safe "\n"))))
    (map (lambda (line i) (row (string-append indent line) #:key (list key-prefix i)))
         ls (iota (length ls)))))

(define (render-topic-buffer! b)
  (let* ((st (buffer-state b))
         (repo (hash-table-ref st 'forge-repo))
         (topic (hash-table-ref st 'forge-topic))
         (tid (forge-ref topic 'id))
         (num (forge-ref topic 'number 0))
         (is-pr (eq? (forge-ref topic 'kind 'issue) 'pullreq))
         (db (forge-db!))
         (labels (forge-store-topic-labels db tid))
         (assignees (forge-store-topic-people db tid 'assignee))
         (reviewers (forge-store-topic-people db tid 'reviewer))
         (posts (forge-store-posts db tid))
         (checks (let ((sha (forge-ref topic 'head-sha "")))
                   (if (> (string-length sha) 0) (forge-store-checks db tid sha) '())))
         (pending-reviews (forge-store-review-comments db tid))
         (folded (hash-table-ref/default st 'folded '())))
    (hash-table-set! st 'rows
      (present! (buffer-document b)
        (append
         (list
          (row (list (list (string-append (if is-pr "PR #" "#") (number->string num) " ") 'forge.number)
                     (list (one-line (forge-ref topic 'title "")) 'forge.heading))
               #:key 'title)
          (row (list (list (string-append "[" (state-badge topic) "]") (state-face topic))
                     "  " (list (forge-ref repo 'slug "") 'forge.meta)
                     "  by " (list (string-append "@" (one-line (forge-ref topic 'author ""))) 'forge.author)
                     (let ((u (forge-ref topic 'updated-at "")))
                       (if (> (string-length u) 0) (list (string-append "  · updated " u) 'forge.meta) "")))
               #:key 'meta))
         (if is-pr
             (list (row (list "Branch:    "
                              (list (string-append (forge-ref topic 'head-ref "") " → " (forge-ref topic 'base-ref "")) 'forge.file)
                              (let ((sha (forge-ref topic 'head-sha "")))
                                (if (>= (string-length sha) 8)
                                    (list (string-append "  @" (substring sha 0 8)) 'forge.meta) "")))
                        #:key 'branch))
             '())
         (if (pair? labels)
             (list (row (list "Labels:    " (list (string-join labels ", ") 'forge.label)) #:key 'labels))
             '())
         (if (pair? assignees)
             (list (row (list "Assignees: " (list (string-join assignees ", ") 'forge.author)) #:key 'assignees))
             '())
         (if (and is-pr (pair? reviewers))
             (list (row (list "Reviewers: " (list (string-join reviewers ", ") 'forge.author)) #:key 'reviewers))
             '())
         (if (and is-pr (pair? pending-reviews))
             (list (row (list (list (string-append "Pending local code-review comments: "
                                                   (number->string (length pending-reviews))
                                                   " (press v for code-review)")
                                    'forge.review-pending))
                        #:key 'pending-review))
             '())
         (list (row (list (list (if is-pr
                                    "m menu · r reply · v code-review · b checkout · g refresh · o browse · q quit"
                                    "m menu · r reply · e edit · k delete · g refresh · o browse · q quit")
                                'shadow))
                    #:key 'hint)
               (row "" #:key 'gap-desc)
               (row (list (list "Description" 'forge.section)
                          (list (if (memq 'desc folded) "  …" "") 'shadow))
                    #:key 'desc-head #:target (target 'forge-fold 'desc)))
         (if (memq 'desc folded) '() (body-lines (forge-ref topic 'body "") "  " 'desc))
         (if (pair? checks)
             (append
              (list (row "" #:key 'gap-checks)
                    (row (list (list (string-append "Checks (" (number->string (length checks)) ")") 'forge.section)
                               (list (if (memq 'checks folded) "  …" "") 'shadow))
                         #:key 'checks-head #:target (target 'forge-fold 'checks)))
              (if (memq 'checks folded)
                  '()
                  (map (lambda (c i)
                         (let* ((concl (forge-ref c 'conclusion ""))
                                (status (forge-ref c 'status ""))
                                (face (cond ((string=? concl "success") 'forge.check-ok)
                                            ((member concl '("failure" "timed_out" "cancelled")) 'forge.check-fail)
                                            (else 'forge.check-pending))))
                           (row (list "  " (list (if (> (string-length concl) 0) concl status) face)
                                      "  " (one-line (forge-ref c 'name "")))
                                #:key (list 'check i))))
                       checks (iota (length checks)))))
             '())
         (list (row "" #:key 'gap-posts)
               (row (list (list (string-append "Conversation (" (number->string (length posts)) ")") 'forge.section))
                    #:key 'posts-head))
         (if (null? posts)
             (list (row (list (list "  No comments yet. Press r to reply, or g to fetch latest comments." 'shadow)) #:key 'no-posts))
             (apply append
               (map (lambda (post i)
                      (let* ((rid (forge-ref post 'remote-id ""))
                             (pkind (forge-ref post 'kind 'comment))
                             (author (one-line (forge-ref post 'author "unknown")))
                             (path (forge-ref post 'path ""))
                             (ln (forge-ref post 'line 0))
                             (rstate (forge-ref post 'review-state ""))
                             (folded? (member rid folded)))
                        (cons
                         (row (append
                               (list "  " (list (string-append "@" author) 'forge.author))
                               (cond
                                 ((eq? pkind 'review)
                                  (list "  " (list (string-append "[" rstate "]")
                                                   (cond ((string=? rstate "APPROVED") 'forge.review-approved)
                                                         ((string=? rstate "CHANGES_REQUESTED") 'forge.review-changes)
                                                         (else 'forge.meta)))))
                                 ((and (eq? pkind 'review_comment) (> (string-length path) 0))
                                  (list "  " (list (string-append (one-line path) (if (> ln 0) (string-append ":" (number->string ln)) " (outdated)")) 'forge.file)))
                                 (else '()))
                               (let ((ts (forge-ref post 'created-at "")))
                                 (if (> (string-length ts) 0) (list (list (string-append "  " ts) 'forge.meta)) '()))
                               (if folded? (list (list "  …" 'shadow)) '()))
                              #:key (list 'post rid)
                              #:target (target 'forge-post (list repo topic post)))
                         (if folded? '() (body-lines (forge-ref post 'body "") "    " (list 'post-body i))))))
                    posts (iota (length posts))))))))))

(define (open-topic-buffer! s repo topic)
  (let* ((stored-repo (ensure-stored-repo! repo))
         (num (forge-ref topic 'number 0))
         (bname (string-append "*forge " (forge-ref stored-repo 'slug "") " #" (number->string num) "*"))
         (b (or (buffer-named bname)
                (make-generated-buffer! bname (make-presentation) 'forge-topic-mode #:state (make-hash-table))))
         (st (buffer-state b)))
    (hash-table-set! st 'forge-repo stored-repo)
    (hash-table-set! st 'forge-topic topic)
    (when (eq? (forge-ref topic 'read-state 'unread) 'unread)
      (forge-store-set-read-state! (forge-db!) (forge-ref topic 'id) 'pending))
    (render-topic-buffer! b)
    (show-buffer! s b)))

(define-command (forge-topic-toggle-fold s n)
  "Fold or unfold the description, checks, or comment at point."
  (let* ((b (session-buffer s)) (st (buffer-state b))
         (t (target-at-point s))
         (folded (hash-table-ref/default st 'folded '())))
    (cond
      ((and t (eq? (target-kind t) 'forge-fold))
       (let ((k (target-value t)))
         (hash-table-set! st 'folded (if (memq k folded) (remove (lambda (x) (eq? x k)) folded) (cons k folded)))
         (render-topic-buffer! b)))
      ((and t (eq? (target-kind t) 'forge-post))
       (let ((rid (forge-ref (caddr (target-value t)) 'remote-id "")))
         (hash-table-set! st 'folded (if (member rid folded) (remove (lambda (x) (equal? x rid)) folded) (cons rid folded)))
         (render-topic-buffer! b)))
      (else (message! s "Nothing foldable at point")))))

(define-command (forge-refresh-current-topic s n)
  "Pull the latest details, comments, and checks for this topic."
  (let* ((b (session-buffer s)) (st (buffer-state b))
         (repo (hash-table-ref st 'forge-repo))
         (topic (hash-table-ref st 'forge-topic))
         (num (forge-ref topic 'number 0)))
    (message! s (string-append "Pulling #" (number->string num) "…"))
    (request! (buffer-read-slot b)
      (lambda ()
        (forge-github-pull-topic! (forge-db!) repo num
                                  #:token (token-for repo) #:scheme %forge-scheme
                                  #:port %forge-port #:transport %forge-transport))
      (lambda (updated)
        (when updated
          (hash-table-set! st 'forge-topic updated)
          (render-topic-buffer! b))
        (message! s (string-append "Updated #" (number->string num))))
      #:fail (lambda (e) (message! s (error-text e))))))

;;; Compose Buffer (`forge-post-mode`)

(define-mode forge-post-mode
  "Compose a Forge issue, pull request, comment, or code review.
Use \\[forge-post-submit] to submit, \\[forge-post-cancel] to cancel,
or \\[forge-post-dispatch] for options."
  #:keys '(("C-c C-c" forge-post-submit) ("C-c C-k" forge-post-cancel) ("C-c C-e" forge-post-dispatch)))

(define (parse-compose-headers text)
  (let ((lines (string-split text "\n")))
    (let loop ((ls lines) (headers '()))
      (cond
        ((null? ls) (cons (reverse headers) ""))
        ((string=? (string-trim (car ls)) "")
         (cons (reverse headers) (string-join (cdr ls) "\n")))
        ((string-index (car ls) #\:)
         => (lambda (colon)
              (let ((k (string-downcase (string-trim (substring (car ls) 0 colon))))
                    (v (string-trim (substring (car ls) (+ colon 1) (string-length (car ls))))))
                (loop (cdr ls) (cons (cons k v) headers)))))
        (else (cons (reverse headers) (string-join ls "\n")))))))

(define (split-csv s)
  (if (and (string? s) (> (string-length (string-trim s)) 0))
      (filter (lambda (x) (> (string-length x) 0)) (map string-trim (string-split s ",")))
      '()))

(define (open-compose-buffer! s name initial-text state-alist)
  (let* ((b (make-generated-buffer! name (make-document initial-text) 'forge-post-mode #:state (make-hash-table)))
         (st (buffer-state b)))
    (for-each (lambda (kv) (hash-table-set! st (car kv) (cdr kv))) state-alist)
    (pop-to-buffer! s b)
    (message! s "C-c C-c submit · C-c C-k cancel · C-c C-e options")))

(define (current-vcs-head-ref dir)
  (let ((tool (detect-vcs-tool dir)))
    (guard (e (#t ""))
      (if (eq? tool 'jj)
          (let ((bms (filter (lambda (x) (> (string-length x) 0))
                             (map string-trim
                                  (string-split (run-proc dir "jj" '("bookmark" "list" "-r" "@" "-T" "if(remote, \"\", name ++ \"\\n\")")) "\n")))))
            (if (pair? bms) (car bms) ""))
          (run-proc dir "git" '("symbolic-ref" "--quiet" "--short" "HEAD"))))))

(define-command (forge-create-issue s n)
  "Compose and submit a new GitHub issue."
  (let* ((repo (ensure-stored-repo! (current-forge-repo s)))
         (draft-id (string-append "new-issue:repo-" (number->string (forge-ref repo 'id 0))))
         (saved (forge-store-draft (forge-db!) draft-id))
         (initial (if (and saved (> (string-length (forge-ref saved 'body "")) 0))
                      (forge-ref saved 'body "")
                      "Title: \nLabels: \nAssignees: \n\n")))
    (open-compose-buffer! s (string-append "*forge new issue · " (forge-ref repo 'slug "") "*")
                          initial
                          `((kind . new-issue) (forge-repo . ,repo) (draft-id . ,draft-id)
                            (origin . ,(session-buffer s))))))

(define-command (forge-create-pullreq s n)
  "Compose and submit a new GitHub pull request."
  (let* ((repo (ensure-stored-repo! (current-forge-repo s)))
         (dir (or (forge-ref repo 'local-dir #f) (default-directory s)))
         (head (current-vcs-head-ref dir))
         (base (forge-ref repo 'default-branch "main"))
         (draft? (option #f 'forge-draft-by-default))
         (draft-id (string-append "new-pr:repo-" (number->string (forge-ref repo 'id 0)) ":" head))
         (saved (forge-store-draft (forge-db!) draft-id))
         (initial (if (and saved (> (string-length (forge-ref saved 'body "")) 0))
                      (forge-ref saved 'body "")
                      (string-append "Title: " head
                                     "\nHead: " head
                                     "\nBase: " base
                                     "\nDraft: " (if draft? "true" "false")
                                     "\n\n"))))
    (open-compose-buffer! s (string-append "*forge new PR · " (forge-ref repo 'slug "") "*")
                          initial
                          `((kind . new-pullreq) (forge-repo . ,repo) (draft-id . ,draft-id)
                            (draft? . ,draft?) (origin . ,(session-buffer s))))))

(define-command (forge-create-post s n)
  "Compose a reply comment on the current Forge topic.
If a region is active, quotes the selected text."
  (let ((found (forge-topic-at-point s)))
    (unless found (error "Open or move to a Forge topic first"))
    (let* ((repo (ensure-stored-repo! (car found))) (topic (cadr found))
           (num (forge-ref topic 'number 0))
           (quoted (let ((r (guard (e (#t #f)) (region-text s))))
                     (if (and (string? r) (> (string-length (string-trim r)) 0))
                         (string-append (string-join (map (lambda (l) (string-append "> " l)) (string-split r "\n")) "\n") "\n\n")
                         "")))
           (draft-id (string-append "post:repo-" (number->string (forge-ref repo 'id 0)) ":" (number->string num)))
           (saved (forge-store-draft (forge-db!) draft-id))
           (initial (cond ((> (string-length quoted) 0) quoted)
                          ((and saved (> (string-length (forge-ref saved 'body "")) 0)) (forge-ref saved 'body ""))
                          (else ""))))
      (open-compose-buffer! s (string-append "*forge reply #" (number->string num) "*")
                            initial
                            `((kind . new-post) (forge-repo . ,repo) (forge-topic . ,topic)
                              (draft-id . ,draft-id) (origin . ,(session-buffer s)))))))

(define-command (forge-edit-post s n)
  "Edit the Forge comment at point."
  (let ((t (target-at-point s)))
    (unless (and t (eq? (target-kind t) 'forge-post))
      (error "Move to a comment to edit"))
    (let* ((v (target-value t)) (repo (car v)) (topic (cadr v)) (post (caddr v))
           (rid (forge-ref post 'remote-id "")))
      (unless (or (string-prefix? "ic:" rid) (string-prefix? "rc:" rid))
        (error "Only issue and review comments can be edited"))
      (open-compose-buffer! s (string-append "*forge edit " rid "*")
                            (forge-ref post 'body "")
                            `((kind . edit-post) (forge-repo . ,repo) (forge-topic . ,topic)
                              (post . ,post) (origin . ,(session-buffer s)))))))

(define-command (forge-delete-comment s n)
  "Delete the Forge comment at point."
  (let ((t (target-at-point s)))
    (unless (and t (eq? (target-kind t) 'forge-post))
      (error "Move to a comment to delete"))
    (let* ((v (target-value t)) (repo (car v)) (topic (cadr v)) (post (caddr v))
           (rid (forge-ref post 'remote-id "")) (origin (session-buffer s)))
      (unless (or (string-prefix? "ic:" rid) (string-prefix? "rc:" rid))
        (error "Only issue and review comments can be deleted"))
      (read-char-choice! s "Delete this comment on GitHub? (y/n) "
        `(("y" . ,(lambda ()
                    (message! s "Deleting comment…")
                    (request! (make-request-slot)
                      (lambda ()
                        (forge-github-delete-comment! (forge-db!) repo (forge-ref topic 'id) rid
                                                      #:token (token-for repo) #:scheme %forge-scheme
                                                      #:port %forge-port #:transport %forge-transport))
                      (lambda (_)
                        (when (eq? (buffer-mode origin) 'forge-topic-mode) (render-topic-buffer! origin))
                        (message! s "Comment deleted"))
                      #:fail (lambda (e) (message! s (error-text e))))))
          ("n" . ,(lambda () (message! s "Cancelled"))))))))

(define-command (forge-post-dispatch s n)
  "Show options for the Forge compose buffer."
  (let* ((b (session-buffer s)) (st (buffer-state b)))
    (transient! s
      `("Forge · Post"
        ("Actions" (suffix "c" "Submit" forge-post-submit)
                   (suffix "k" "Cancel (save draft)" forge-post-cancel)
                   ,@(if (eq? (hash-table-ref/default st 'kind #f) 'new-pullreq)
                         `((suffix "d" "Toggle draft header"
                                   ,(lambda (s _)
                                      (let* ((d (doc s))
                                             (txt (document-substring d 0 (document-length d)))
                                             (cur (hash-table-ref/default st 'draft? #f))
                                             (next (not cur))
                                             (updated (if cur
                                                          (string-replace txt "Draft: true" "Draft: false")
                                                          (string-replace txt "Draft: false" "Draft: true"))))
                                        (hash-table-set! st 'draft? next)
                                        (edit! s (list (list 0 (document-length d) updated)) 'new)
                                        (message! s (if next "PR marked as draft" "PR marked ready"))))))
                         '()))))))

(define-command (forge-post-cancel s n)
  "Cancel the current Forge compose buffer, keeping a local draft."
  (let* ((b (session-buffer s)) (st (buffer-state b))
         (kind (hash-table-ref/default st 'kind #f))
         (draft-id (hash-table-ref/default st 'draft-id #f))
         (repo (hash-table-ref/default st 'forge-repo #f))
         (text (document-substring (doc s) 0 (document-length (doc s))))
         (origin (hash-table-ref/default st 'origin #f)))
    (when (and draft-id repo (memq kind '(new-issue new-pullreq new-post)) (> (string-length (string-trim text)) 0))
      ;; Store full compose text verbatim so headers, labels, assignees and replies round-trip.
      (forge-store-save-draft! (forge-db!) draft-id (forge-ref repo 'id) kind "" text #:updated-at (now-iso)))
    (drop-buffer! s b)
    (when (and origin (buffer? origin)) (show-buffer! s origin))
    (message! s (if draft-id "Draft saved locally" "Cancelled"))))

(define (run-forge-write! s b st work on-ok)
  (when (hash-table-ref/default st 'submitting? #f)
    (error "Submission already in flight; wait for it to finish"))
  (hash-table-set! st 'submitting? #t)
  ;; Use a dedicated request slot so unrelated refreshes never cancel a write.
  (request! (make-request-slot)
    work
    (lambda (res)
      (hash-table-set! st 'submitting? #f)
      (on-ok res))
    #:fail (lambda (e)
             (hash-table-set! st 'submitting? #f)
             (message! s (string-append "Write failed (check GitHub before retrying): " (error-text e))))))

(define-command (forge-post-submit s n)
  "Submit the current Forge compose or code-review buffer."
  (let* ((b (session-buffer s)) (st (buffer-state b))
         (kind (hash-table-ref st 'kind))
         (repo (hash-table-ref st 'forge-repo))
         (origin (hash-table-ref/default st 'origin #f))
         (draft-id (hash-table-ref/default st 'draft-id #f))
         (text (document-substring (doc s) 0 (document-length (doc s)))))
    (case kind
      ((new-issue)
       (let* ((parsed (parse-compose-headers text)) (hdrs (car parsed)) (body (cdr parsed))
              (title (or (let ((e (assoc "title" hdrs))) (and e (cdr e))) ""))
              (labels (split-csv (let ((e (assoc "labels" hdrs))) (and e (cdr e)))))
              (assignees (split-csv (let ((e (assoc "assignees" hdrs))) (and e (cdr e))))))
         (when (string=? (string-trim title) "") (error "Issue title cannot be empty"))
         (message! s "Creating GitHub issue…")
         (run-forge-write! s b st
           (lambda ()
             (forge-github-create-issue! (forge-db!) repo title body #:labels labels #:assignees assignees
                                         #:token (token-for repo) #:scheme %forge-scheme
                                         #:port %forge-port #:transport %forge-transport))
           (lambda (created)
             (when draft-id (forge-store-delete-draft! (forge-db!) draft-id))
             (drop-buffer! s b)
             (open-topic-buffer! s repo created)
             (message! s (string-append "Created issue #" (number->string (forge-ref created 'number 0))))))))
      ((new-pullreq)
       (let* ((parsed (parse-compose-headers text)) (hdrs (car parsed)) (body (cdr parsed))
              (title (or (let ((e (assoc "title" hdrs))) (and e (cdr e))) ""))
              (head (or (let ((e (assoc "head" hdrs))) (and e (cdr e))) ""))
              (base (or (let ((e (assoc "base" hdrs))) (and e (cdr e))) "main"))
              (draft? (equal? (string-downcase (or (let ((e (assoc "draft" hdrs))) (and e (cdr e))) "false")) "true")))
         (when (string=? (string-trim title) "") (error "Pull request title cannot be empty"))
         (when (string=? (string-trim head) "") (error "Head branch/bookmark cannot be empty"))
         (message! s "Creating GitHub pull request…")
         (run-forge-write! s b st
           (lambda ()
             (forge-github-create-pullreq! (forge-db!) repo title body head base #:draft draft?
                                           #:token (token-for repo) #:scheme %forge-scheme
                                           #:port %forge-port #:transport %forge-transport))
           (lambda (created)
             (when draft-id (forge-store-delete-draft! (forge-db!) draft-id))
             (drop-buffer! s b)
             (open-topic-buffer! s repo created)
             (message! s (string-append "Created PR #" (number->string (forge-ref created 'number 0))))))))
      ((new-post)
       (let ((topic (hash-table-ref st 'forge-topic)))
         (when (string=? (string-trim text) "") (error "Comment cannot be empty"))
         (message! s "Posting comment…")
         (run-forge-write! s b st
           (lambda ()
             (forge-github-create-comment! (forge-db!) repo (forge-ref topic 'number 0) text
                                           #:token (token-for repo) #:scheme %forge-scheme
                                           #:port %forge-port #:transport %forge-transport))
           (lambda (_)
             (when draft-id (forge-store-delete-draft! (forge-db!) draft-id))
             (drop-buffer! s b)
             (when (and origin (buffer? origin) (eq? (buffer-mode origin) 'forge-topic-mode))
               (render-topic-buffer! origin)
               (show-buffer! s origin))
             (message! s "Comment posted")))))
      ((edit-post)
       (let ((topic (hash-table-ref st 'forge-topic)) (post (hash-table-ref st 'post)))
         (message! s "Updating comment…")
         (run-forge-write! s b st
           (lambda ()
             (forge-github-update-comment! (forge-db!) repo (forge-ref topic 'id)
                                           (forge-ref post 'remote-id "") text
                                           #:token (token-for repo) #:scheme %forge-scheme
                                           #:port %forge-port #:transport %forge-transport))
           (lambda (_)
             (drop-buffer! s b)
             (when (and origin (buffer? origin) (eq? (buffer-mode origin) 'forge-topic-mode))
               (render-topic-buffer! origin)
               (show-buffer! s origin))
             (message! s "Comment updated")))))
      ((review-inline-add)
       (let ((topic (hash-table-ref st 'forge-topic))
             (head-sha (hash-table-ref st 'head-sha))
             (path (hash-table-ref st 'path))
             (line (hash-table-ref st 'line))
             (side (hash-table-ref st 'side)))
         (when (string=? (string-trim text) "") (error "Review comment cannot be empty"))
         (forge-store-add-review-comment! (forge-db!) (forge-ref topic 'id) head-sha path line text
                                          #:side side #:created-at (now-iso))
         (drop-buffer! s b)
         (when (and origin (buffer? origin) (eq? (buffer-mode origin) 'code-review-mode))
           (render-code-review! origin)
           (show-buffer! s origin))
         (message! s (string-append "Staged review comment on " (one-line path) ":" (number->string line)))))
      ((review-inline-edit)
       (let ((comment-id (hash-table-ref st 'comment-id)))
         (when (string=? (string-trim text) "") (error "Review comment cannot be empty"))
         (forge-store-update-review-comment! (forge-db!) comment-id text)
         (drop-buffer! s b)
         (when (and origin (buffer? origin) (eq? (buffer-mode origin) 'code-review-mode))
           (render-code-review! origin)
           (show-buffer! s origin))
         (message! s "Updated pending review comment")))
      ((review-reply)
       (let ((topic (hash-table-ref st 'forge-topic))
             (reply-to (hash-table-ref st 'reply-to)))
         (when (string=? (string-trim text) "") (error "Reply cannot be empty"))
         (message! s "Posting review reply…")
         (run-forge-write! s b st
           (lambda ()
             (forge-github-reply-review-comment! (forge-db!) repo (forge-ref topic 'number 0) reply-to text
                                                 #:token (token-for repo) #:scheme %forge-scheme
                                                 #:port %forge-port #:transport %forge-transport))
           (lambda (_)
             (drop-buffer! s b)
             (when (and origin (buffer? origin) (eq? (buffer-mode origin) 'code-review-mode))
               (render-code-review! origin)
               (show-buffer! s origin))
             (message! s "Review reply posted")))))
      ((review-submit)
       (let* ((topic (hash-table-ref st 'forge-topic))
              (event (hash-table-ref st 'review-event))
              (head-sha (forge-ref topic 'head-sha ""))
              (pending (forge-store-review-comments (forge-db!) (forge-ref topic 'id)
                                                    #:head-sha (and (> (string-length head-sha) 0) head-sha))))
         (when (and (string=? event "REQUEST_CHANGES")
                    (string=? (string-trim text) "")
                    (null? pending))
           (error "Requesting changes requires a review comment"))
         (message! s (string-append "Submitting " event " review (" (number->string (length pending)) " inline comments)…"))
         (run-forge-write! s b st
           (lambda ()
             (forge-github-submit-review! (forge-db!) repo (forge-ref topic 'number 0) event text
                                          #:commit-id head-sha #:comments pending
                                          #:token (token-for repo) #:scheme %forge-scheme
                                          #:port %forge-port #:transport %forge-transport))
           (lambda (_)
             (drop-buffer! s b)
             (when (and origin (buffer? origin) (eq? (buffer-mode origin) 'code-review-mode))
               (render-code-review! origin)
               (show-buffer! s origin))
             (message! s (string-append "Submitted " event " review"))))))
      (else (error "Unknown Forge compose kind" kind)))))

;;; Topic Context Menu (`forge-topic-menu`)

(define-command (forge-topic-menu s n)
  "Open the action menu for the current Forge topic."
  (let ((found (forge-topic-at-point s)))
    (unless found (error "No Forge topic at point"))
    (let* ((repo (car found)) (topic (cadr found))
           (origin (session-buffer s))
           (num (forge-ref topic 'number 0))
           (tid (forge-ref topic 'id))
           (is-pr (eq? (forge-ref topic 'kind 'issue) 'pullreq))
           (is-open (eq? (forge-ref topic 'state 'open) 'open)))
      (define (after-update! updated msg)
        (when (and (eq? (buffer-mode origin) 'forge-topic-mode) updated)
          (hash-table-set! (buffer-state origin) 'forge-topic updated)
          (render-topic-buffer! origin))
        (when (eq? (buffer-mode origin) 'forge-topic-list-mode)
          (render-topic-list! origin))
        (message! s msg))
      (transient! s
        `(,(string-append "Forge · #" (number->string num) " " (short (forge-ref topic 'title "") 40))
          ("Edit"
           (suffix "s" ,(if is-open "Close topic" "Reopen topic")
                   ,(lambda (s _)
                      (let ((next (if is-open "closed" "open")))
                        (message! s (string-append "Setting #" (number->string num) " " next "…"))
                        (request! (make-request-slot)
                          (lambda ()
                            (forge-github-update-topic! (forge-db!) repo num #:state next
                                                        #:token (token-for repo) #:scheme %forge-scheme
                                                        #:port %forge-port #:transport %forge-transport))
                          (lambda (u) (after-update! u (string-append "#" (number->string num) " is now " next)))
                          #:fail (lambda (e) (message! s (error-text e)))))))
           (suffix "t" "Edit title…"
                   ,(lambda (s _)
                      (completing-read s "New title: " (list (forge-ref topic 'title "")) #:require-match #f
                        #:accept (lambda (s c)
                                   (let ((nt (string-trim (candidate-text c))))
                                     (when (> (string-length nt) 0)
                                       (request! (make-request-slot)
                                         (lambda ()
                                           (forge-github-update-topic! (forge-db!) repo num #:title nt
                                                                       #:token (token-for repo) #:scheme %forge-scheme
                                                                       #:port %forge-port #:transport %forge-transport))
                                         (lambda (u) (after-update! u "Title updated"))
                                         #:fail (lambda (e) (message! s (error-text e))))))))))
           (suffix "l" "Set labels…"
                   ,(lambda (s _)
                      (let ((cur (string-join (forge-store-topic-labels (forge-db!) tid) ", ")))
                        (completing-read s "Labels (comma-separated): " (list cur) #:require-match #f
                          #:accept (lambda (s c)
                                     (let ((ls (split-csv (candidate-text c))))
                                       (request! (make-request-slot)
                                         (lambda ()
                                           (forge-github-set-labels! (forge-db!) repo num ls
                                                                     #:token (token-for repo) #:scheme %forge-scheme
                                                                     #:port %forge-port #:transport %forge-transport))
                                         (lambda (_) (after-update! topic "Labels updated"))
                                         #:fail (lambda (e) (message! s (error-text e))))))))))
           (suffix "a" "Set assignees…"
                   ,(lambda (s _)
                      (let ((cur (string-join (forge-store-topic-people (forge-db!) tid 'assignee) ", ")))
                        (completing-read s "Assignees (comma-separated): " (list cur) #:require-match #f
                          #:accept (lambda (s c)
                                     (let ((as (split-csv (candidate-text c))))
                                       (request! (make-request-slot)
                                         (lambda ()
                                           (forge-github-set-assignees! (forge-db!) repo num as
                                                                        #:token (token-for repo) #:scheme %forge-scheme
                                                                        #:port %forge-port #:transport %forge-transport))
                                         (lambda (_) (after-update! topic "Assignees updated"))
                                         #:fail (lambda (e) (message! s (error-text e))))))))))
           ,@(if is-pr
                 `((suffix "R" "Request reviewers…"
                           ,(lambda (s _)
                              (completing-read s "Reviewers (comma-separated): " '() #:require-match #f
                                #:accept (lambda (s c)
                                           (let ((rs (split-csv (candidate-text c))))
                                             (when (pair? rs)
                                               (request! (make-request-slot)
                                                 (lambda ()
                                                   (forge-github-request-reviewers! (forge-db!) repo num rs
                                                                                    #:token (token-for repo) #:scheme %forge-scheme
                                                                                    #:port %forge-port #:transport %forge-transport))
                                                 (lambda (_) (after-update! topic "Reviewers requested"))
                                                 #:fail (lambda (e) (message! s (error-text e)))))))))))
                 '()))
          ("Actions"
           (suffix "r" "Reply / comment" forge-create-post)
           ,@(if is-pr
                 `((suffix "v" "Code review" ,(lambda (s _) (open-code-review! s repo topic)))
                   (suffix "b" "Checkout PR locally" ,(lambda (s _) (checkout-pr! s repo topic)))
                   (suffix "M" "Merge PR…"
                           ,(lambda (s _)
                              (read-char-choice! s (string-append "Merge PR #" (number->string num) ": (m)erge (s)quash (r)ebase (q)uit ")
                                `(("m" . ,(lambda () (merge-pr! s origin repo topic "merge")))
                                  ("s" . ,(lambda () (merge-pr! s origin repo topic "squash")))
                                  ("r" . ,(lambda () (merge-pr! s origin repo topic "rebase"))))))))
                 '())
           (suffix "o" "Browse in browser" forge-browse)
           (suffix "w" "Copy URL" forge-copy-url)
           (suffix "u" "Mark done"
                   ,(lambda (s _)
                      (forge-store-set-read-state! (forge-db!) tid 'done)
                      (after-update! topic "Marked done")))
           (suffix "K" "Remove from local cache"
                   ,(lambda (s _)
                      (let ((drafts (forge-store-review-comments (forge-db!) tid)))
                        (define (do-remove!)
                          (forge-store-remove-topic! (forge-db!) tid)
                          (when (eq? (buffer-mode origin) 'forge-topic-list-mode) (render-topic-list! origin))
                          (message! s "Removed from local cache"))
                        (if (pair? drafts)
                            (read-char-choice! s (string-append "Topic has " (number->string (length drafts)) " pending review drafts; remove anyway? (y/n) ")
                              `(("y" . ,do-remove!) ("n" . ,(lambda () (message! s "Cancelled")))))
                            (do-remove!)))))))))))

(define (merge-pr! s origin repo topic method)
  (let ((num (forge-ref topic 'number 0))
        (sha (forge-ref topic 'head-sha "")))
    (unless (= (string-length sha) 40)
      (error "Pull request has no verified head SHA; refresh the PR before merging"))
    (message! s (string-append "Merging PR #" (number->string num) " (" method ")…"))
    (request! (make-request-slot)
      (lambda ()
        (forge-github-merge-pr! (forge-db!) repo num #:method method #:sha sha
                                #:token (token-for repo) #:scheme %forge-scheme
                                #:port %forge-port #:transport %forge-transport))
      (lambda (_)
        (let ((updated (forge-store-topic (forge-db!) (forge-ref repo 'id) num)))
          (when (and (eq? (buffer-mode origin) 'forge-topic-mode) updated)
            (hash-table-set! (buffer-state origin) 'forge-topic updated)
            (render-topic-buffer! origin))
          (when (eq? (buffer-mode origin) 'forge-topic-list-mode) (render-topic-list! origin)))
        (message! s (string-append "Merged PR #" (number->string num))))
      #:fail (lambda (e) (message! s (error-text e))))))

;;; Git & Jujutsu PR Checkout

(define (valid-remote-name? name)
  (and (string? name) (> (string-length name) 0)
       (not (char=? (string-ref name 0) #\-))
       (every (lambda (c) (or (char<=? #\a c #\z) (char<=? #\A c #\Z) (char<=? #\0 c #\9)
                              (memv c '(#\- #\_ #\.))))
              (string->list name))))

(define (safe-pr-branch-name num)
  (string-append "pr-" (number->string num)))

(define (git-branch-exists? dir branch)
  (guard (e (#t #f))
    (run-proc dir "git" (list "show-ref" "--verify" "--quiet" (string-append "refs/heads/" branch)))
    #t))

(define (checkout-pr! s repo topic)
  (let* ((dir (or (forge-ref repo 'local-dir #f) (default-directory s)))
         (num (forge-ref topic 'number 0))
         (expected-slug (forge-ref repo 'slug ""))
         (branch (safe-pr-branch-name num))
         (tool (detect-vcs-tool dir)))
    (message! s (string-append "Fetching PR #" (number->string num) " (" (symbol->string tool) ")…"))
    (request! (make-request-slot)
      (lambda ()
        ;; Resolve and verify the actual Git/jj remote at checkout time.
        (let* ((detected (forge-detect-repository dir #:tool tool #:alist (get-alist)))
               (remote (and detected (forge-ref detected 'remote-name #f))))
          (unless (and detected (valid-remote-name? remote)
                       (string-ci=? (forge-ref detected 'slug "") expected-slug))
            (error (string-append "No verified local remote matching " expected-slug)))
          (let* ((tracking (string-append "refs/remotes/" remote "/pr-" (number->string num)))
                 (refspec (string-append "+refs/pull/" (number->string num) "/head:" tracking)))
            (if (eq? tool 'jj)
                (begin
                  (run-proc dir "git" (list "fetch" "--" remote refspec))
                  (let ((fetched-sha (run-proc dir "git" (list "rev-parse" "--verify" tracking))))
                    (run-proc dir "jj" '("git" "import"))
                    (run-proc dir "jj" (list "new" "--" fetched-sha))
                    (string-append "jj new on PR #" (number->string num) " (" (substring fetched-sha 0 (min 12 (string-length fetched-sha))) ")")))
                (begin
                  (run-proc dir "git" (list "fetch" "--" remote refspec))
                  ;; Never reset an existing local branch; fall back to detached HEAD.
                  (if (git-branch-exists? dir branch)
                      (begin
                        (run-proc dir "git" (list "checkout" "--detach" tracking))
                        (string-append "Branch " branch " already exists; checked out PR #" (number->string num) " detached"))
                      (begin
                        (run-proc dir "git" (list "checkout" "-b" branch tracking))
                        (string-append "Checked out new Git branch " branch " for PR #" (number->string num)))))))))
      (lambda (msg)
        (refresh-vc-status-buffers!)
        (message! s msg))
      #:fail (lambda (e) (message! s (error-text e))))))

(define-command (forge-checkout-pullreq s n)
  "Fetch and check out a pull request locally in Git or Jujutsu."
  (let ((found (forge-topic-at-point s)))
    (if (and found (eq? (forge-ref (cadr found) 'kind 'issue) 'pullreq))
        (checkout-pr! s (car found) (cadr found))
        (let* ((repo (ensure-stored-repo! (current-forge-repo s)))
               (prs (forge-store-topics (forge-db!) #:repo-id (forge-ref repo 'id) #:kind 'pullreq #:state 'open))
               (cands (map (lambda (p)
                             (candidate (string-append "#" (number->string (forge-ref p 'number 0)) " " (one-line (forge-ref p 'title "")))
                                        #:target p))
                           prs)))
          (if (null? cands)
              (message! s "No cached open pull requests; run M-x forge-pull first")
              (completing-read s "Checkout pull request: " cands
                #:accept (lambda (s c) (checkout-pr! s repo (candidate-target c)))))))))

;;; Interactive Code Review (`code-review-mode`, modeled on Emacs code-review)

(define-mode code-review-mode
  "Interactive GitHub pull request code review.
Supports inline comments, suggestions, and atomic review submission."
  #:parent 'special-mode
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("RET" code-review-comment-at-point) ("c" code-review-comment-at-point)
           ("s" code-review-suggest-at-point) ("e" code-review-edit-comment-at-point)
           ("k" code-review-delete-comment-at-point)
           ("r" code-review-menu) ("C-c C-c" code-review-menu)
           ("TAB" code-review-toggle-fold)
           ("n" forge-next-item) ("p" forge-previous-item)
           ("]" code-review-next-comment) ("[" code-review-previous-comment)
           ("t" code-review-open-topic) ("b" forge-checkout-pullreq)
           ("g" code-review-refresh) ("o" forge-browse) ("w" forge-copy-url)
           ("q" quit-window))
  #:normal '(("RET" code-review-comment-at-point) ("c" code-review-comment-at-point)
             ("s" code-review-suggest-at-point) ("e" code-review-edit-comment-at-point)
             ("k" code-review-delete-comment-at-point)
             ("r" code-review-menu) ("C-c C-c" code-review-menu)
             ("TAB" code-review-toggle-fold)
             ("n" forge-next-item) ("p" forge-previous-item)
             ("]" code-review-next-comment) ("[" code-review-previous-comment)
             ("t" code-review-open-topic) ("b" forge-checkout-pullreq)
             ("g r" code-review-refresh) ("o" forge-browse) ("w" forge-copy-url)
             ("q" quit-window)))

(define (pad-ln n width)
  (let ((s (if (integer? n) (number->string n) "")))
    (string-append (make-string (max 0 (- width (string-length s))) #\space) s)))

(define (inline-comment-rows path line side existing-by-key pending-by-key)
  (let* ((k (list path line side))
         (existing (or (hash-table-ref/default existing-by-key k #f) '()))
         (pending (or (hash-table-ref/default pending-by-key k #f) '())))
    (append
     (apply append
       (map (lambda (c i)
              (let* ((rid (forge-ref c 'remote-id ""))
                     (ls (string-split (safe-text (forge-ref c 'body "")) "\n")))
                (cons
                 (row (list "      ┌─ " (list (string-append "@" (one-line (forge-ref c 'author ""))) 'forge.author)
                            (list (string-append " · " (forge-ref c 'created-at "") "  (RET to reply)") 'forge.meta))
                      #:key (list 'rc path line rid)
                      #:target (target 'code-review-existing-comment c))
                 (map (lambda (l j) (row (string-append "      │ " l) #:key (list 'rc-b path line rid j)))
                      ls (iota (length ls))))))
            existing (iota (length existing))))
     (apply append
       (map (lambda (d i)
              (let ((did (forge-ref d 'id 0))
                    (ls (string-split (safe-text (forge-ref d 'body "")) "\n")))
                (cons
                 (row (list "      ╔═ " (list "[PENDING REVIEW COMMENT]" 'forge.review-pending)
                            (list "  (e edit · k delete · r submit review)" 'forge.meta))
                      #:key (list 'rd path line did)
                      #:target (target 'code-review-pending-comment d))
                 (map (lambda (l j) (row (list (list (string-append "      ║ " l) 'forge.review-pending))
                                         #:key (list 'rd-b path line did j)))
                      ls (iota (length ls))))))
            pending (iota (length pending)))))))

(define (render-code-review! b)
  (let* ((st (buffer-state b))
         (repo (hash-table-ref st 'forge-repo))
         (topic (hash-table-ref st 'forge-topic))
         (files (hash-table-ref/default st 'files '()))
         (folded (hash-table-ref/default st 'folded '()))
         (tid (forge-ref topic 'id))
         (num (forge-ref topic 'number 0))
         (head-sha (forge-ref topic 'head-sha ""))
         (db (forge-db!))
         (posts (forge-store-posts db tid))
         (general (filter (lambda (p) (memq (forge-ref p 'kind 'comment) '(comment review))) posts))
         (all-rcomments (filter (lambda (p) (eq? (forge-ref p 'kind 'comment) 'review_comment)) posts))
         (rcomments (filter (lambda (p) (> (forge-ref p 'line 0) 0)) all-rcomments))
         (outdated-comments (filter (lambda (p) (<= (forge-ref p 'line 0) 0)) all-rcomments))
         (pending (forge-store-review-comments db tid #:head-sha (and (> (string-length head-sha) 0) head-sha)))
         (checks (if (> (string-length head-sha) 0) (forge-store-checks db tid head-sha) '()))
         (existing-by-key (make-hash-table))
         (pending-by-key (make-hash-table)))
    (for-each (lambda (c)
                (let* ((side (let ((s (forge-ref c 'side ""))) (if (string=? s "") "RIGHT" s)))
                       (k (list (forge-ref c 'path "") (forge-ref c 'line 0) side)))
                  (hash-table-set! existing-by-key k (append (hash-table-ref/default existing-by-key k '()) (list c)))))
              rcomments)
    (for-each (lambda (d)
                (let ((k (list (forge-ref d 'path "") (forge-ref d 'line 0) (forge-ref d 'side "RIGHT"))))
                  (hash-table-set! pending-by-key k (append (hash-table-ref/default pending-by-key k '()) (list d)))))
              pending)
    (hash-table-set! st 'rows
      (present! (buffer-document b)
        (append
         (list
          (row (list (list (string-append "Code Review · PR #" (number->string num) " ") 'forge.number)
                     (list (one-line (forge-ref topic 'title "")) 'forge.heading))
               #:key 'cr-head)
          (row (list (list (string-append "[" (state-badge topic) "]") (state-face topic))
                     "  " (list (forge-ref repo 'slug "") 'forge.meta)
                     "  " (list (string-append (forge-ref topic 'head-ref "") " → " (forge-ref topic 'base-ref "")) 'forge.file)
                     (if (>= (string-length head-sha) 8) (list (string-append "  @" (substring head-sha 0 8)) 'forge.meta) ""))
               #:key 'cr-meta)
          (row (list (list (string-append (number->string (length files)) " files · "
                                          (number->string (length rcomments)) " inline comments · "
                                          (number->string (length pending)) " pending draft"
                                          (if (= (length pending) 1) "" "s")
                                          " · " (number->string (length checks)) " checks")
                           (if (pair? pending) 'forge.review-pending 'forge.meta)))
               #:key 'cr-stats)
          (row (list (list "RET/c comment · s suggest · e/k edit/del draft · r submit review · TAB fold · ]/[ comments · q quit" 'shadow))
               #:key 'cr-hint)
          (row "" #:key 'cr-gap-1)
          (row (list (list "Description" 'forge.section)
                     (list (if (memq 'desc folded) "  …" "") 'shadow))
               #:key 'cr-desc #:target (target 'code-review-fold 'desc)))
         (if (memq 'desc folded) '() (body-lines (forge-ref topic 'body "") "  " 'cr-desc-line))
         (if (pair? general)
             (append
              (list (row "" #:key 'cr-gap-gen)
                    (row (list (list (string-append "Conversation & Reviews (" (number->string (length general)) ")") 'forge.section)
                               (list (if (memq 'general folded) "  …" "") 'shadow))
                         #:key 'cr-gen #:target (target 'code-review-fold 'general)))
              (if (memq 'general folded)
                  '()
                  (apply append
                    (map (lambda (p i)
                           (cons (row (list "  " (list (string-append "@" (one-line (forge-ref p 'author ""))) 'forge.author)
                                            (let ((rs (forge-ref p 'review-state "")))
                                              (if (> (string-length rs) 0) (list (string-append " [" rs "]") 'forge.meta) "")))
                                      #:key (list 'cr-gen-h i))
                                 (body-lines (forge-ref p 'body "") "    " (list 'cr-gen-b i))))
                         general (iota (length general))))))
             '())
         (if (pair? outdated-comments)
             (append
              (list (row "" #:key 'cr-gap-outdated)
                    (row (list (list (string-append "Outdated Inline Comments (" (number->string (length outdated-comments)) ")") 'forge.section)
                               (list (if (memq 'outdated folded) "  …" "") 'shadow))
                         #:key 'cr-outdated #:target (target 'code-review-fold 'outdated)))
              (if (memq 'outdated folded)
                  '()
                  (apply append
                    (map (lambda (c i)
                           (cons (row (list "  " (list (string-append "@" (one-line (forge-ref c 'author ""))) 'forge.author)
                                            "  " (list (one-line (forge-ref c 'path "")) 'forge.file)
                                            (let ((ol (forge-ref c 'original-line 0)))
                                              (if (> ol 0) (list (string-append " (was line " (number->string ol) ")") 'forge.meta) "")))
                                      #:key (list 'cr-out-h i)
                                      #:target (target 'code-review-existing-comment c))
                                 (body-lines (forge-ref c 'body "") "    " (list 'cr-out-b i))))
                         outdated-comments (iota (length outdated-comments))))))
             '())
         (list (row "" #:key 'cr-gap-files)
               (row (list (list (string-append "Files Changed (" (number->string (length files)) ")") 'forge.section))
                    #:key 'cr-files-head))
         (if (null? files)
             (list (row (list (list "  Loading changed files…" 'shadow)) #:key 'cr-loading))
             (apply append
               (map (lambda (f fi)
                      (let* ((fname (forge-ref f 'filename ""))
                             (adds (forge-ref f 'additions 0))
                             (dels (forge-ref f 'deletions 0))
                             (patch (forge-ref f 'patch #f))
                             (fold-key (list 'file fname))
                             (folded? (member fold-key folded))
                             (parsed (if (and (not folded?) (string? patch)) (forge-parse-patch-lines patch) '())))
                        (append
                         (list (row (list "  " (list (one-line fname) 'forge.file)
                                          "  " (list (string-append "+" (number->string adds)) 'forge.diff-add)
                                          " " (list (string-append "-" (number->string dels)) 'forge.diff-del)
                                          (if folded? (list "  …" 'shadow) ""))
                                    #:key (list 'file fname)
                                    #:target (target 'code-review-fold fold-key)))
                         (cond
                           (folded? '())
                           ((not (string? patch))
                            (list (row (list (list "    (Binary file or patch unavailable)" 'shadow)) #:key (list 'no-patch fi))))
                           (else
                            (apply append
                              (map (lambda (pl li)
                                     (let* ((k (forge-ref pl 'kind 'meta))
                                            (txt (safe-text (forge-ref pl 'text "")))
                                            (old-ln (forge-ref pl 'old-line #f))
                                            (new-ln (forge-ref pl 'new-line #f))
                                            (side (forge-ref pl 'side #f))
                                            (anchor-ln (forge-ref pl 'line #f))
                                            (gutter (if (eq? k 'hunk)
                                                        "    "
                                                        (string-append " " (pad-ln old-ln 4) " " (pad-ln new-ln 4) " ")))
                                            (face (case k
                                                    ((add) 'forge.diff-add)
                                                    ((del) 'forge.diff-del)
                                                    ((hunk) 'forge.diff-hunk)
                                                    (else #f)))
                                            (line-row
                                             (row (list (list gutter 'forge.meta)
                                                        (if face (list txt face) txt))
                                                  #:key (list 'diff fi li)
                                                  #:target (and anchor-ln side
                                                                (target 'code-review-line
                                                                        `((path . ,fname) (line . ,anchor-ln)
                                                                          (side . ,side) (code . ,(forge-ref pl 'code ""))))))))
                                       (if (and anchor-ln side)
                                           (cons line-row (inline-comment-rows fname anchor-ln side existing-by-key pending-by-key))
                                           (list line-row))))
                                   parsed (iota (length parsed)))))))))
                    files (iota (length files))))))))))

(define (open-code-review! s repo topic)
  (let* ((stored-repo (ensure-stored-repo! repo))
         (num (forge-ref topic 'number 0))
         (bname (string-append "*code-review " (forge-ref stored-repo 'slug "") " #" (number->string num) "*"))
         (b (or (buffer-named bname)
                (make-generated-buffer! bname (make-presentation) 'code-review-mode #:state (make-hash-table))))
         (st (buffer-state b)))
    (hash-table-set! st 'forge-repo stored-repo)
    (hash-table-set! st 'forge-topic topic)
    (unless (hash-table-ref/default st 'folded #f)
      (hash-table-set! st 'folded '(desc)))
    (render-code-review! b)
    (show-buffer! s b)
    (message! s (string-append "Loading code review for PR #" (number->string num) "…"))
    (request! (buffer-read-slot b)
      (lambda ()
        (let* ((fresh (or (guard (e (#t #f))
                            (forge-github-pull-topic! (forge-db!) stored-repo num
                                                      #:token (token-for stored-repo) #:scheme %forge-scheme
                                                      #:port %forge-port #:transport %forge-transport))
                          topic))
               (files (forge-github-fetch-pr-files stored-repo num
                                                   #:base-sha (forge-ref fresh 'base-sha #f)
                                                   #:head-sha (forge-ref fresh 'head-sha #f)
                                                   #:token (token-for stored-repo) #:scheme %forge-scheme
                                                   #:port %forge-port #:transport %forge-transport)))
          (cons fresh files)))
      (lambda (res)
        (hash-table-set! st 'forge-topic (car res))
        (hash-table-set! st 'files (cdr res))
        (render-code-review! b)
        (message! s (string-append "Code review ready for PR #" (number->string num))))
      #:fail (lambda (e) (message! s (error-text e))))))

(define-command (code-review-start s n)
  "Open a pull request in the interactive code-review buffer."
  (let ((found (forge-topic-at-point s)))
    (if (and found (eq? (forge-ref (cadr found) 'kind 'issue) 'pullreq))
        (open-code-review! s (car found) (cadr found))
        (let* ((repo (ensure-stored-repo! (current-forge-repo s)))
               (prs (forge-store-topics (forge-db!) #:repo-id (forge-ref repo 'id) #:kind 'pullreq #:state 'open))
               (cands (map (lambda (p)
                             (candidate (string-append "#" (number->string (forge-ref p 'number 0)) " " (one-line (forge-ref p 'title "")))
                                        #:target p))
                           prs)))
          (if (null? cands)
              (completing-read s "Pull request number to review: " '() #:require-match #f
                #:accept (lambda (s c)
                           (let ((num (string->number (string-trim (candidate-text c)))))
                             (unless (and (integer? num) (> num 0)) (error "Expected a positive PR number"))
                             (open-code-review! s repo `((kind . pullreq) (number . ,num) (id . 0) (title . ""))))))
              (completing-read s "Review pull request: " cands
                #:accept (lambda (s c) (open-code-review! s repo (candidate-target c)))))))))

(define-command (code-review-forge-pr-at-point s n)
  "Start a code review for the pull request at point."
  (let ((found (forge-topic-at-point s)))
    (unless (and found (eq? (forge-ref (cadr found) 'kind 'issue) 'pullreq))
      (error "Move to or open a pull request first"))
    (open-code-review! s (car found) (cadr found))))

(define-command (code-review-toggle-fold s n)
  "Fold or unfold the diff file or section at point."
  (let* ((b (session-buffer s)) (st (buffer-state b))
         (t (target-at-point s))
         (folded (hash-table-ref/default st 'folded '())))
    (if (and t (eq? (target-kind t) 'code-review-fold))
        (let ((k (target-value t)))
          (hash-table-set! st 'folded (if (member k folded) (remove (lambda (x) (equal? x k)) folded) (cons k folded)))
          (render-code-review! b))
        (message! s "Move to a file or section heading to fold"))))

(define-command (code-review-comment-at-point s n)
  "Add a pending inline comment on the diff line, or reply at point."
  (let* ((b (session-buffer s)) (st (buffer-state b))
         (repo (hash-table-ref st 'forge-repo))
         (topic (hash-table-ref st 'forge-topic))
         (t (target-at-point s)))
    (cond
      ((and t (eq? (target-kind t) 'code-review-line))
       (let* ((v (target-value t)) (path (forge-ref v 'path)) (line (forge-ref v 'line)) (side (forge-ref v 'side)))
         (open-compose-buffer! s (string-append "*review comment " (one-line path) ":" (number->string line) "*")
                               ""
                               `((kind . review-inline-add) (forge-repo . ,repo) (forge-topic . ,topic)
                                 (head-sha . ,(forge-ref topic 'head-sha ""))
                                 (path . ,path) (line . ,line) (side . ,side) (origin . ,b)))))
      ((and t (eq? (target-kind t) 'code-review-existing-comment))
       (let* ((c (target-value t)) (rid (forge-ref c 'remote-id "")))
         (open-compose-buffer! s (string-append "*review reply " rid "*")
                               ""
                               `((kind . review-reply) (forge-repo . ,repo) (forge-topic . ,topic)
                                 (reply-to . ,rid) (origin . ,b)))))
      ((and t (eq? (target-kind t) 'code-review-fold))
       (code-review-toggle-fold s n))
      (else (message! s "Move to a diff line or review comment")))))

(define-command (code-review-suggest-at-point s n)
  "Stage a GitHub code suggestion block on the diff line at point."
  (let* ((b (session-buffer s)) (st (buffer-state b))
         (repo (hash-table-ref st 'forge-repo))
         (topic (hash-table-ref st 'forge-topic))
         (t (target-at-point s)))
    (unless (and t (eq? (target-kind t) 'code-review-line))
      (error "Move to a diff line first"))
    (let* ((v (target-value t)) (path (forge-ref v 'path)) (line (forge-ref v 'line))
           (side (forge-ref v 'side)) (code (forge-ref v 'code "")))
      (open-compose-buffer! s (string-append "*review suggestion " (one-line path) ":" (number->string line) "*")
                            (forge-suggestion-block code)
                            `((kind . review-inline-add) (forge-repo . ,repo) (forge-topic . ,topic)
                              (head-sha . ,(forge-ref topic 'head-sha ""))
                              (path . ,path) (line . ,line) (side . ,side) (origin . ,b))))))

(define-command (code-review-edit-comment-at-point s n)
  "Edit the pending local code-review comment at point."
  (let* ((b (session-buffer s)) (st (buffer-state b))
         (repo (hash-table-ref st 'forge-repo))
         (t (target-at-point s)))
    (unless (and t (eq? (target-kind t) 'code-review-pending-comment))
      (error "Move to a pending local review comment"))
    (let ((d (target-value t)))
      (open-compose-buffer! s (string-append "*edit review draft #" (number->string (forge-ref d 'id 0)) "*")
                            (forge-ref d 'body "")
                            `((kind . review-inline-edit) (forge-repo . ,repo)
                              (comment-id . ,(forge-ref d 'id 0)) (origin . ,b))))))

(define-command (code-review-delete-comment-at-point s n)
  "Delete the pending local code-review comment at point."
  (let* ((b (session-buffer s)) (t (target-at-point s)))
    (unless (and t (eq? (target-kind t) 'code-review-pending-comment))
      (error "Move to a pending local review comment"))
    (forge-store-delete-review-comment! (forge-db!) (forge-ref (target-value t) 'id 0))
    (render-code-review! b)
    (message! s "Deleted pending review comment")))

(define (step-review-comment! s direction)
  (let* ((p (doc s)) (b (session-buffer s))
         (rows (hash-table-ref/default (buffer-state b) 'rows #f))
         (at (point s)))
    (when rows
      (let* ((spans (filter (lambda (sp)
                              (let* ((r (row-at p rows (cadr sp)))
                                     (t (and r (row-target r))))
                                (and t (memq (target-kind t) '(code-review-existing-comment code-review-pending-comment)))))
                            (presentation-rows-between p 0 (document-length p))))
             (found (if (> direction 0)
                        (find (lambda (sp) (> (cadr sp) at)) spans)
                        (find (lambda (sp) (< (caddr sp) at)) (reverse spans)))))
        (if found (move! s (lambda (_) (cadr found)))
            (message! s (if (> direction 0) "No next review comment" "No previous review comment")))))))

(define-command (code-review-next-comment s n)
  "Jump to the next inline review comment or pending draft."
  (step-review-comment! s 1))

(define-command (code-review-previous-comment s n)
  "Jump to the previous inline review comment or pending draft."
  (step-review-comment! s -1))

(define-command (code-review-open-topic s n)
  "Switch from code review to the full Forge topic buffer."
  (let ((st (buffer-state (session-buffer s))))
    (open-topic-buffer! s (hash-table-ref st 'forge-repo) (hash-table-ref st 'forge-topic))))

(define-command (code-review-refresh s n)
  "Refresh the pull request diff, comments, and checks from GitHub."
  (let ((st (buffer-state (session-buffer s))))
    (open-code-review! s (hash-table-ref st 'forge-repo) (hash-table-ref st 'forge-topic))))

(define (open-review-submit! s event)
  (let* ((b (session-buffer s)) (st (buffer-state b))
         (repo (hash-table-ref st 'forge-repo))
         (topic (hash-table-ref st 'forge-topic))
         (num (forge-ref topic 'number 0)))
    (open-compose-buffer! s (string-append "*submit " event " PR #" (number->string num) "*")
                          ""
                          `((kind . review-submit) (review-event . ,event)
                            (forge-repo . ,repo) (forge-topic . ,topic) (origin . ,b)))))

(define-command (code-review-menu s n)
  "Open the Code Review action menu to submit a review or manage drafts."
  (let* ((b (session-buffer s)) (st (buffer-state b))
         (topic (hash-table-ref st 'forge-topic))
         (tid (forge-ref topic 'id))
         (num (forge-ref topic 'number 0))
         (pending-count (length (forge-store-review-comments (forge-db!) tid))))
    (transient! s
      `(,(string-append "Code Review · PR #" (number->string num) " (" (number->string pending-count) " pending)")
        ("Submit review"
         (suffix "a" "Approve" ,(lambda (s _) (open-review-submit! s "APPROVE")))
         (suffix "x" "Request changes" ,(lambda (s _) (open-review-submit! s "REQUEST_CHANGES")))
         (suffix "m" "Comment only" ,(lambda (s _) (open-review-submit! s "COMMENT"))))
        ("Inline comments"
         (suffix "c" "Comment on line at point" code-review-comment-at-point)
         (suffix "s" "Suggest change at point" code-review-suggest-at-point)
         (suffix "e" "Edit pending comment" code-review-edit-comment-at-point)
         (suffix "k" "Delete pending comment" code-review-delete-comment-at-point)
         (suffix "C" "Clear all pending drafts"
                 ,(lambda (s _)
                    (forge-store-clear-review-comments! (forge-db!) tid)
                    (render-code-review! b)
                    (message! s "Cleared all pending review drafts"))))
        ("PR"
         (suffix "g" "Refresh diff & comments" code-review-refresh)
         (suffix "b" "Checkout PR locally" forge-checkout-pullreq)
         (suffix "t" "Open Forge topic" code-review-open-topic)
         (suffix "o" "Browse in browser" forge-browse))))))

;;; Notifications & Tracked Repositories

(define-mode forge-notifications-mode
  "Browse GitHub notifications across repositories."
  #:parent 'special-mode
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("RET" forge-notification-visit) ("m" forge-notification-mark-read)
           ("g" forge-pull-notifications) ("n" forge-next-item) ("p" forge-previous-item)
           ("N" forge-dispatch) ("q" quit-window))
  #:normal '(("RET" forge-notification-visit) ("m" forge-notification-mark-read)
             ("g r" forge-pull-notifications) ("n" forge-next-item) ("p" forge-previous-item)
             ("N" forge-dispatch) ("q" quit-window)))

(define (render-notifications! b)
  (let* ((st (buffer-state b))
         (items (forge-store-notifications (forge-db!))))
    (hash-table-set! st 'rows
      (present! (buffer-document b)
        (append
         (list (row (list (list (string-append "Forge · Notifications (" (number->string (length items)) ")") 'forge.heading)) #:key 'head)
               (row (list (list "RET pull & visit · m mark read · g refresh · q quit" 'shadow)) #:key 'hint)
               (row "" #:key 'gap))
         (if (null? items)
             (list (row (list (list "No cached notifications. Press g to pull from GitHub." 'shadow)) #:key 'empty))
             (map (lambda (it)
                    (row (list (if (forge-ref it 'unread #f) (list "● " 'forge.open) "  ")
                               (list (string-append (forge-ref it 'slug "") " #" (number->string (forge-ref it 'topic-number 0))) 'forge.number)
                               "  "
                               (if (forge-ref it 'unread #f)
                                   (list (short (forge-ref it 'title "") 64) 'forge.unread)
                                   (short (forge-ref it 'title "") 64))
                               (list (string-append "  [" (forge-ref it 'reason "") "]") 'forge.meta))
                         #:key (forge-ref it 'id "")
                         #:target (target 'forge-notification it)))
                  items)))))))

(define-command (forge-list-notifications s n)
  "List cached GitHub notifications."
  (let ((b (or (buffer-named "*forge notifications*")
               (make-generated-buffer! "*forge notifications*" (make-presentation) 'forge-notifications-mode #:state (make-hash-table)))))
    (render-notifications! b)
    (show-buffer! s b)))

(define-command (forge-notification-visit s n)
  "Pull and visit the topic referenced by the notification at point."
  (let ((t (target-at-point s)))
    (unless (and t (eq? (target-kind t) 'forge-notification)) (error "No notification at point"))
    (let* ((it (target-value t))
           (repo `((host . ,(forge-ref it 'host "github.com"))
                   (api-host . "api.github.com")
                   (web-host . ,(forge-ref it 'host "github.com"))
                   (kind . github)
                   (owner . ,(forge-ref it 'owner ""))
                   (name . ,(forge-ref it 'repo-name ""))
                   (slug . ,(forge-ref it 'slug ""))))
           (num (forge-ref it 'topic-number 0)))
      (message! s (string-append "Fetching " (forge-ref repo 'slug "") " #" (number->string num) "…"))
      (request! (buffer-read-slot (session-buffer s))
        (lambda ()
          (forge-github-pull-topic! (forge-db!) repo num
                                    #:token (token-for repo) #:scheme %forge-scheme
                                    #:port %forge-port #:transport %forge-transport))
        (lambda (topic)
          (if topic (open-topic-buffer! s repo topic) (message! s "Topic not found")))
        #:fail (lambda (e) (message! s (error-text e)))))))

(define-command (forge-notification-mark-read s n)
  "Mark the GitHub notification at point as read."
  (let* ((b (session-buffer s)) (t (target-at-point s)))
    (unless (and t (eq? (target-kind t) 'forge-notification)) (error "No notification at point"))
    (let* ((it (target-value t)) (id (forge-ref it 'id "")))
      (request! (make-request-slot)
        (lambda ()
          (forge-github-mark-notification-read! (forge-db!) id
                                                #:token (token-for '((api-host . "api.github.com") (web-host . "github.com")))
                                                #:scheme %forge-scheme #:port %forge-port #:transport %forge-transport))
        (lambda (_)
          (render-notifications! b)
          (message! s "Notification marked read"))
        #:fail (lambda (e) (message! s (error-text e)))))))

(define-mode forge-repos-mode
  "Browse repositories tracked in the local Forge database."
  #:parent 'special-mode
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("RET" forge-repos-visit) ("g" forge-repos-pull) ("a" forge-add-repository)
           ("K" forge-repos-remove) ("o" forge-browse) ("n" forge-next-item) ("p" forge-previous-item)
           ("N" forge-dispatch) ("q" quit-window))
  #:normal '(("RET" forge-repos-visit) ("g r" forge-repos-pull) ("a" forge-add-repository)
             ("K" forge-repos-remove) ("o" forge-browse) ("n" forge-next-item) ("p" forge-previous-item)
             ("N" forge-dispatch) ("q" quit-window)))

(define (render-repos! b)
  (let* ((st (buffer-state b))
         (db (forge-db!))
         (repos (forge-store-repos db)))
    (hash-table-set! st 'rows
      (present! (buffer-document b)
        (append
         (list (row (list (list (string-append "Forge · Tracked Repositories (" (number->string (length repos)) ")") 'forge.heading)) #:key 'head)
               (row (list (list "RET topics · g pull · a add · K remove · o browse · q quit" 'shadow)) #:key 'hint)
               (row "" #:key 'gap))
         (if (null? repos)
             (list (row (list (list "No tracked repositories. Press a to add one." 'shadow)) #:key 'empty))
             (map (lambda (r)
                    (let ((counts (forge-store-topic-counts db (forge-ref r 'id))))
                      (row (list "  " (list (forge-ref r 'slug "") 'forge.file)
                                 "  " (list (string-append (number->string (forge-ref counts 'pullreqs 0)) " PRs, "
                                                           (number->string (forge-ref counts 'issues 0)) " issues")
                                            'forge.meta)
                                 (let ((ts (forge-ref r 'synced-at "")))
                                   (if (> (string-length ts) 0) (list (string-append "  · synced " ts) 'shadow) "")))
                           #:key (forge-ref r 'id)
                           #:target (target 'forge-repo r))))
                  repos)))))))

(define-command (forge-list-repositories s n)
  "List repositories tracked in the local Forge database."
  (let ((b (or (buffer-named "*forge repositories*")
               (make-generated-buffer! "*forge repositories*" (make-presentation) 'forge-repos-mode #:state (make-hash-table)))))
    (render-repos! b)
    (show-buffer! s b)))

(define-command (forge-repos-visit s n)
  "Open the topic list for the repository at point."
  (let ((t (target-at-point s)))
    (unless (and t (eq? (target-kind t) 'forge-repo)) (error "No repository at point"))
    (open-topic-list! s (target-value t))))

(define-command (forge-repos-pull s n)
  "Pull topics for the repository at point."
  (let ((t (target-at-point s)))
    (unless (and t (eq? (target-kind t) 'forge-repo)) (error "No repository at point"))
    (pull-repo! s (target-value t))))

(define-command (forge-repos-remove s n)
  "Remove the repository at point from the local Forge database."
  (let* ((b (session-buffer s)) (t (target-at-point s)))
    (unless (and t (eq? (target-kind t) 'forge-repo)) (error "No repository at point"))
    (let ((r (target-value t)))
      (forge-store-remove-repo! (forge-db!) (forge-ref r 'id))
      (render-repos! b)
      (message! s (string-append "Removed " (forge-ref r 'slug "") " from local cache")))))

;;; Sync, Add, Visit & Browse Commands

(define (pull-repo! s repo)
  (let ((origin (session-buffer s)))
    (message! s (string-append "Pulling " (forge-ref repo 'slug "") " from GitHub…"))
    (request! (buffer-read-slot origin)
      (lambda ()
        (forge-github-pull-repo! (forge-db!) repo
                                 #:token (token-for repo) #:scheme %forge-scheme
                                 #:port %forge-port #:transport %forge-transport))
      (lambda (res)
        (when (eq? (buffer-mode origin) 'forge-topic-list-mode) (render-topic-list! origin))
        (when (eq? (buffer-mode origin) 'forge-repos-mode) (render-repos! origin))
        (refresh-vc-status-buffers!)
        (message! s (string-append "Pulled " (forge-ref (forge-ref res 'repo) 'slug "") ": "
                                   (number->string (forge-ref res 'pullreqs 0)) " PRs, "
                                   (number->string (forge-ref res 'issues 0)) " issues")))
      #:fail (lambda (e) (message! s (error-text e))))))

(define-command (forge-pull s n)
  "Pull issues and pull requests for the current repository from GitHub."
  (pull-repo! s (current-forge-repo s)))

(define-command (forge-pull-topic s n)
  "Pull a single issue or pull request by number and open it."
  (let ((repo (ensure-stored-repo! (current-forge-repo s))))
    (completing-read s "Pull topic number: " '() #:require-match #f
      #:accept (lambda (s c)
                 (let ((num (string->number (string-trim (candidate-text c)))))
                   (unless (and (integer? num) (> num 0)) (error "Expected a positive topic number"))
                   (message! s (string-append "Pulling #" (number->string num) "…"))
                   (request! (buffer-read-slot (session-buffer s))
                     (lambda ()
                       (forge-github-pull-topic! (forge-db!) repo num
                                                 #:token (token-for repo) #:scheme %forge-scheme
                                                 #:port %forge-port #:transport %forge-transport))
                     (lambda (topic)
                       (when topic (open-topic-buffer! s repo topic))
                       (message! s (string-append "Pulled #" (number->string num))))
                     #:fail (lambda (e) (message! s (error-text e)))))))))

(define-command (forge-pull-notifications s n)
  "Pull GitHub notifications into the local database."
  (let ((origin (session-buffer s)))
    (message! s "Pulling GitHub notifications…")
    (request! (buffer-read-slot origin)
      (lambda ()
        (forge-github-pull-notifications! (forge-db!)
                                          #:token (token-for '((api-host . "api.github.com") (web-host . "github.com")))
                                          #:scheme %forge-scheme #:port %forge-port #:transport %forge-transport))
      (lambda (count)
        (when (eq? (buffer-mode origin) 'forge-notifications-mode) (render-notifications! origin))
        (message! s (string-append "Pulled " (number->string count) " notifications")))
      #:fail (lambda (e) (message! s (error-text e))))))

(define-command (forge-add-repository s n)
  "Track a GitHub repository in Forge and pull its topics."
  (let ((detected (current-forge-repo s #:require #f)))
    (completing-read s "Track GitHub repository (owner/repo or URL): "
                     (if detected (list (forge-ref detected 'slug "")) '())
                     #:require-match #f
      #:accept (lambda (s c)
                 (let* ((raw (string-trim (candidate-text c)))
                        (url (if (or (string-contains raw "://") (string-contains raw ":"))
                                 raw
                                 (string-append "https://github.com/" raw)))
                        (parsed (forge-parse-remote-url url #:alist (get-alist))))
                   (unless parsed (error "Invalid GitHub repository" raw))
                   (pull-repo! s parsed))))))

(define-command (forge-visit-topic s n)
  "Choose and inspect a cached topic for the current repository."
  (let* ((repo (ensure-stored-repo! (current-forge-repo s)))
         (topics (forge-store-topics (forge-db!) #:repo-id (forge-ref repo 'id) #:state 'all))
         (cands (map (lambda (t)
                       (candidate (string-append "#" (number->string (forge-ref t 'number 0)) " [" (state-badge t) "] "
                                                 (one-line (forge-ref t 'title "")))
                                  #:target t))
                     topics)))
    (if (null? cands)
        (message! s "No cached topics; run M-x forge-pull first")
        (completing-read s "Visit topic: " cands
          #:accept (lambda (s c) (open-topic-buffer! s repo (candidate-target c)))))))

(define-command (forge-visit-topic-from-url s n)
  "Pull and inspect a GitHub issue or pull request by URL."
  (completing-read s "GitHub issue or PR URL: " '() #:require-match #f
    #:accept (lambda (s c)
               (let ((parsed (forge-parse-topic-url (string-trim (candidate-text c)) #:alist (get-alist))))
                 (unless parsed (error "Not a recognized GitHub issue or pull-request URL"))
                 (let ((num (forge-ref parsed 'number 0)))
                   (message! s (string-append "Pulling " (forge-ref parsed 'slug "") " #" (number->string num) "…"))
                   (request! (buffer-read-slot (session-buffer s))
                     (lambda ()
                       (forge-github-pull-topic! (forge-db!) parsed num
                                                 #:token (token-for parsed) #:scheme %forge-scheme
                                                 #:port %forge-port #:transport %forge-transport))
                     (lambda (topic)
                       (when topic (open-topic-buffer! s parsed topic))
                       (message! s (string-append "Loaded #" (number->string num))))
                     #:fail (lambda (e) (message! s (error-text e)))))))))

(define (url-at-point s)
  (let ((found (forge-topic-at-point s))
        (t (target-at-point s)))
    (cond
      ((and t (eq? (target-kind t) 'forge-repo))
       (let ((r (target-value t)))
         (forge-repo-web-url (forge-ref r 'web-host "github.com") (forge-ref r 'owner "") (forge-ref r 'name ""))))
      (found
       (let ((r (car found)) (topic (cadr found)))
         (forge-topic-web-url (forge-ref r 'web-host "github.com") (forge-ref r 'owner "") (forge-ref r 'name "")
                              (forge-ref topic 'kind 'issue) (forge-ref topic 'number 0))))
      (else
       (let ((r (current-forge-repo s)))
         (forge-repo-web-url (forge-ref r 'web-host "github.com") (forge-ref r 'owner "") (forge-ref r 'name "")))))))

(define-command (forge-browse s n)
  "Open the Forge topic or repository at point in the web browser."
  (open-in-browser! s (url-at-point s)))

(define-command (forge-copy-url s n)
  "Copy the GitHub URL at point to the kill ring."
  (let ((url (url-at-point s)))
    (kill-save! s url)
    (message! s (string-append "Copied " url))))

;;; Main Dispatch Menu (`forge-dispatch`)

(define-command (forge-dispatch s n)
  "Open the main Forge command menu."
  (transient! s
    '("Forge · GitHub"
      ("Fetch"
       (suffix "f f" "Pull repository" forge-pull)
       (suffix "f t" "Pull single topic" forge-pull-topic)
       (suffix "f n" "Pull notifications" forge-pull-notifications)
       (suffix "/ a" "Add repository" forge-add-repository))
      ("List"
       (suffix "l t" "Topics" forge-list-topics)
       (suffix "l p" "Pull requests" forge-list-pullreqs)
       (suffix "l i" "Issues" forge-list-issues)
       (suffix "l g" "Global topics" forge-list-global-topics)
       (suffix "l n" "Notifications" forge-list-notifications)
       (suffix "l r" "Repositories" forge-list-repositories))
      ("Create & Review"
       (suffix "c i" "Create issue" forge-create-issue)
       (suffix "c p" "Create pull request" forge-create-pullreq)
       (suffix "r" "Code review PR" code-review-start)
       (suffix "b f" "Checkout PR locally" forge-checkout-pullreq))
      ("Visit & Browse"
       (suffix "v t" "Visit topic" forge-visit-topic)
       (suffix "v u" "Visit from URL" forge-visit-topic-from-url)
       (suffix "o" "Browse in browser" forge-browse)
       (suffix "w" "Copy URL" forge-copy-url)))))
