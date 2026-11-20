;;; Remote repository detection (Git and Jujutsu) and GitHub web URL helpers.
(define-library (forge remote)
  (export forge-default-alist forge-valid-segment? forge-parse-remote-url
          forge-host-entry forge-parse-topic-url
          forge-detect-repository forge-repo-slug
          forge-repo-web-url forge-topic-web-url forge-commit-web-url
          forge-branch-web-url forge-blob-web-url)
  (import (techne))
  (begin
    (define forge-default-alist
      '(("github.com" "api.github.com" "github.com" github))
      "Default forge hosts as (GIT-HOST API-HOST WEB-HOST KIND).")

    (define (forge-valid-segment? s)
      "Return #t when S is a valid repository owner or name segment."
      (and (string? s) (> (string-length s) 0) (not (member s '("." "..")))
           (every (lambda (c)
                    (or (char<=? #\a c #\z) (char<=? #\A c #\Z) (char<=? #\0 c #\9)
                        (memv c '(#\- #\_ #\.))))
                  (string->list s))))

    (define (strip-git-suffix s)
      (if (string-suffix? ".git" s) (substring s 0 (- (string-length s) 4)) s))

    (define (strip-trailing-slash s)
      (if (and (> (string-length s) 0) (char=? (string-ref s (- (string-length s) 1)) #\/))
          (substring s 0 (- (string-length s) 1))
          s))

    (define (forge-host-entry host #:alist [alist forge-default-alist])
      "Return the entry for HOST in ALIST, or #f."
      (let ((h (string-downcase host)))
        (find (lambda (e)
                (or (string-ci=? (car e) h) (string-ci=? (caddr e) h)))
              alist)))

    (define (split-host-and-path url)
      ;; Supports:
      ;;   https://github.com/owner/repo[.git]
      ;;   http://github.com/owner/repo[.git]
      ;;   ssh://git@github.com[:port]/owner/repo[.git]
      ;;   git@github.com:owner/repo[.git]
      (let ((u (string-trim url)))
        (cond
          ((or (string-prefix? "https://" u) (string-prefix? "http://" u))
           (let* ((after (substring u (+ (string-index u #\/) 2) (string-length u)))
                  (slash (string-index after #\/)))
             (and slash
                  (let* ((hostport (substring after 0 slash))
                         (at (string-index hostport #\@))
                         (hp (if at (substring hostport (+ at 1) (string-length hostport)) hostport))
                         (colon (string-index hp #\:))
                         (host (if colon (substring hp 0 colon) hp))
                         (path (substring after (+ slash 1) (string-length after))))
                    (cons host path)))))
          ((string-prefix? "ssh://" u)
           (let* ((after (substring u 6 (string-length u)))
                  (slash (string-index after #\/)))
             (and slash
                  (let* ((hostport (substring after 0 slash))
                         (at (string-index hostport #\@))
                         (hp (if at (substring hostport (+ at 1) (string-length hostport)) hostport))
                         (colon (string-index hp #\:))
                         (host (if colon (substring hp 0 colon) hp))
                         (path (substring after (+ slash 1) (string-length after))))
                    (cons host path)))))
          ((string-index u #\:)
           => (lambda (colon)
                (let* ((left (substring u 0 colon))
                       (at (string-index left #\@))
                       (host (if at (substring left (+ at 1) (string-length left)) left))
                       (path (substring u (+ colon 1) (string-length u))))
                  (and (not (string-index host #\/)) (cons host path)))))
          (else #f))))

    (define (forge-parse-remote-url url #:alist [alist forge-default-alist])
      "Parse Git remote URL against ALIST into a repository alist, or #f.
Keys: host, api-host, web-host, kind, owner, name, slug."
      (let ((hp (and (string? url) (split-host-and-path url))))
        (and hp
             (let* ((entry (forge-host-entry (car hp) #:alist alist))
                    (clean (strip-git-suffix (strip-trailing-slash (cdr hp))))
                    (parts (string-split clean "/")))
               (and entry (= (length parts) 2) (every forge-valid-segment? parts)
                    (let ((owner (car parts)) (name (cadr parts)))
                      `((host . ,(car entry))
                        (api-host . ,(cadr entry))
                        (web-host . ,(caddr entry))
                        (kind . ,(cadddr entry))
                        (owner . ,owner)
                        (name . ,name)
                        (slug . ,(string-downcase (string-append owner "/" name))))))))))

    (define (forge-repo-slug owner name)
      "Return the lowercase slug for OWNER and NAME."
      (string-downcase (string-append owner "/" name)))

    (define (forge-parse-topic-url url #:alist [alist forge-default-alist])
      "Parse a GitHub issue or pull-request URL using ALIST, or #f.
Returns host, api-host, web-host, kind, owner, name, slug, topic-kind,
and number."
      (let ((hp (and (string? url) (split-host-and-path url))))
        (and hp
             (let* ((entry (forge-host-entry (car hp) #:alist alist))
                    ;; Strip fragment and query.
                    (raw (cdr hp))
                    (q (or (string-index raw #\?) (string-index raw #\#)))
                    (path (if q (substring raw 0 q) raw))
                    (parts (string-split (strip-trailing-slash path) "/")))
               (and entry (>= (length parts) 4)
                    (forge-valid-segment? (list-ref parts 0))
                    (forge-valid-segment? (list-ref parts 1))
                    (member (list-ref parts 2) '("issues" "pull"))
                    (let ((num (string->number (list-ref parts 3))))
                      (and (integer? num) (> num 0)
                           `((host . ,(car entry))
                             (api-host . ,(cadr entry))
                             (web-host . ,(caddr entry))
                             (kind . ,(cadddr entry))
                             (owner . ,(list-ref parts 0))
                             (name . ,(list-ref parts 1))
                             (slug . ,(forge-repo-slug (list-ref parts 0) (list-ref parts 1)))
                             (topic-kind . ,(if (string=? (list-ref parts 2) "pull") 'pullreq 'issue))
                             (number . ,num)))))))))

    (define (run-capture dir prog args)
      (guard (e (#t #f))
        (call-with-process-at dir prog args
          (lambda (p)
            (let* ((out (process-read-all p 'stdout))
                   (status (process-wait p)))
              (and (= status 0) (string? out) (string-trim out)))))))

    (define (git-remotes dir)
      (let ((out (run-capture dir "git" '("remote"))))
        (if (and out (> (string-length out) 0))
            (filter (lambda (s) (> (string-length s) 0))
                    (map string-trim (string-split out "\n")))
            '())))

    (define (git-remote-url dir remote)
      (run-capture dir "git" (list "remote" "get-url" remote)))

    (define (jj-remotes dir)
      (let ((out (run-capture dir "jj" '("git" "remote" "list"))))
        (if (and out (> (string-length out) 0))
            (filter-map
             (lambda (line)
               (let* ((t (string-trim line))
                      (sp (string-index t #\space)))
                 (and sp (> sp 0)
                      (cons (substring t 0 sp) (string-trim (substring t (+ sp 1) (string-length t)))))))
             (string-split out "\n"))
            '())))

    (define (pick-remote-url dir tool configured-remote)
      (if (eq? tool 'jj)
          (let ((remotes (jj-remotes dir)))
            (cond
              ((null? remotes) #f)
              ((null? (cdr remotes)) (cons (caar remotes) (cdar remotes)))
              (else
               (let ((chosen (or (and configured-remote (assoc configured-remote remotes))
                                 (assoc "upstream" remotes)
                                 (assoc "origin" remotes)
                                 (car remotes))))
                 chosen))))
          (let ((remotes (git-remotes dir)))
            (cond
              ((null? remotes) #f)
              ((null? (cdr remotes))
               (let ((u (git-remote-url dir (car remotes)))) (and u (cons (car remotes) u))))
              (else
               (let* ((name (or (and configured-remote (member configured-remote remotes) configured-remote)
                                (and (member "upstream" remotes) "upstream")
                                (and (member "origin" remotes) "origin")
                                (car remotes)))
                      (u (git-remote-url dir name)))
                 (and u (cons name u))))))))

    (define (forge-detect-repository dir #:tool [tool #f] #:override [override #f]
                                     #:alist [alist forge-default-alist])
      "Detect the Forge repository for DIR using TOOL (git or jj) and ALIST.
OVERRIDE, if given as \"owner/repo\", takes precedence. Otherwise checks
`forge.remote`, then `upstream`, then `origin`. Returns an alist or #f."
      (if (and (string? override) (> (string-length (string-trim override)) 0))
          (let ((parsed (forge-parse-remote-url (string-append "https://github.com/" (string-trim override))
                                                #:alist alist)))
            (and parsed (cons '(remote-name . "origin") parsed)))
          (let* ((actual-tool (or tool
                                  (if (run-capture dir "jj" '("root")) 'jj 'git)))
                 (configured (run-capture dir "git" '("config" "--get" "forge.remote")))
                 (found (pick-remote-url dir actual-tool configured)))
            (and found
                 (let ((parsed (forge-parse-remote-url (cdr found) #:alist alist)))
                   (and parsed (cons (cons 'remote-name (car found)) parsed)))))))

    (define (forge-repo-web-url web-host owner name)
      "Return the HTTPS web URL for OWNER and NAME on WEB-HOST."
      (string-append "https://" web-host "/" owner "/" name))

    (define (forge-topic-web-url web-host owner name kind number #:comment-id [comment-id #f])
      "Return the HTTPS URL for topic NUMBER of KIND on WEB-HOST.
Uses OWNER, NAME, and optional COMMENT-ID."
      (let ((base (string-append (forge-repo-web-url web-host owner name)
                                 (if (eq? kind 'pullreq) "/pull/" "/issues/")
                                 (number->string number))))
        (if comment-id
            (string-append base "#issuecomment-" (if (number? comment-id) (number->string comment-id) comment-id))
            base)))

    (define (forge-commit-web-url web-host owner name sha)
      "Return the HTTPS URL for commit SHA of OWNER and NAME on WEB-HOST."
      (string-append (forge-repo-web-url web-host owner name) "/commit/" sha))

    (define (forge-branch-web-url web-host owner name branch)
      "Return the HTTPS URL for BRANCH of OWNER and NAME on WEB-HOST."
      (string-append (forge-repo-web-url web-host owner name) "/tree/" branch))

    (define (forge-blob-web-url web-host owner name ref path #:start-line [start-line #f] #:end-line [end-line #f])
      "Return the HTTPS URL for PATH at REF on WEB-HOST.
Uses OWNER, NAME, and optional START-LINE and END-LINE."
      (let ((base (string-append (forge-repo-web-url web-host owner name) "/blob/" ref "/" path)))
        (cond ((and start-line end-line (not (= start-line end-line)))
               (string-append base "#L" (number->string start-line) "-L" (number->string end-line)))
              (start-line (string-append base "#L" (number->string start-line)))
              (else base))))))
