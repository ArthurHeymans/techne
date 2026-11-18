;;; Version control status: Git's index and working tree, or Jujutsu's
;;; recorded change graph. Rows carry typed targets for files, changes and
;;; sections. M-x vc-status opens the repository of the focused buffer.
;;;
;;; It is a package against (techne editor) like any extension, and the
;;; vertical slice of the architecture review (EDITOR.md, section 1):
;;;
;;; - its rows come from a process, in the background, a newer refresh
;;;   cancelling an older one (request slots); the view updates in place
;;;   and the caret stays on its file's row, keyed by path;
;;; - refresh is explicit by default. Optional local polling and pending
;;;   requests belong to the package; its next generation takes over the
;;;   open status buffers, whose state is plain data;
;;; - a vc-file target carries all its actions need (repository, tool,
;;;   path), so they work wherever the target comes from: the view, or the
;;;   minibuffer (M-x vc-find-change, then C-;), the view not even shown.

(import (techne editor) (techne crypto))
;; Its parts that need nothing of the views, in vc/.
(require "vc/state.scm")
(require "vc/conflict-blocks.scm")
(require "vc/jj-hunks.scm")
(require "vc/evidence.scm")
(require "vc/transient.scm")
(require "vc/refine.scm")
(require "../views.scm")
(require "../session.scm")

(define-face vc.section "A section's heading in a version control view." #:fg 'yellow #:weight 'bold)
(define-face vc.hash "A commit's abbreviated ID." #:fg 'overlay2)
(define-face vc.branch "A local branch or bookmark." #:fg 'sky #:weight 'bold)
(define-face vc.remote "A remote-tracking branch or bookmark." #:fg 'green)
(define-face vc.tag "A tag." #:fg 'yellow)
(define-face vc.file-status "How a file changed: modified, new file, deleted." #:fg 'peach)
(define-face vc.change-id "A Jujutsu change ID." #:fg 'mauve #:weight 'bold)
(define-face vc.commit-id "A Jujutsu commit ID." #:fg 'blue)
(define-face vc.working-copy "The Jujutsu working-copy change." #:fg 'green #:weight 'bold)
(define-face vc.marked "A change marked for the next command." #:fg 'base #:bg 'peach #:weight 'bold)
(define-face vc.hint "Keys suggested in a version control view." #:inherit 'shadow)
(define-face vc.graph "The lines of a history graph." #:fg 'overlay)
(define-face vc.author "Who made a commit, and when." #:fg 'overlay2)

(define-option vc-show-hints #t
  "Whether version control views show a line of the keys most used."
  #:type 'boolean)

(define-option vc-refresh-interval #f
  "How often local status buffers refresh themselves, in milliseconds.
With #f refresh explicitly. Remote status always refreshes on demand."
  #:type '(or (one-of #f) natural))

;;; Repositories, through their tools: (tool root), TOOL `jj` or `git`.

;; Run PROGRAM with ARGS in the directory DIR, here or on another
;; machine; its output, or an error with what it said. Its streams are
;; read together: one read to its end first would leave the tool
;; waiting for ever once it filled the other.
(define (run dir program args #:input [input #f] #:exit [on-exit #f] #:accept-exit [accept-exit '(0)] #:env [env #f])
  (call-with-process-at dir (if env "env" program) (if env (append env (cons program args)) args)
                     (lambda (p)
                       (let* ((err (spawn (lambda () (process-read-all p 'stderr))))
                              (writer (spawn (lambda ()
                                               (when input (process-write p input))
                                               (process-close-input p))))
                              (out (process-read-all p 'stdout))
                              (err (task-join err))
                              (status (process-wait p)))
                         (when on-exit (on-exit status))
                         (if (memv status accept-exit)
                             (begin (task-join writer) (set! %last-stderr err) out)
                             (error (string-append program ": " (string-trim err))))))))

;; What the last successful process wrote to its error stream: Git says
;; what a push or a rebase did there.
(define %last-stderr "")

;;; The process log, as Magit's: every write and what it said, per
;;; repository, newest last. $ shows it.

(define %process-log (make-hash-table))
(define %process-log-max 60)

(define (log-process! repo title args outcome output)
  (let* ((key (cadr repo)) (entries (hash-table-ref/default %process-log key '()))
         (entry (list title (cons (symbol->string (car repo)) args) outcome
                      (let ((text (string-trim output)))
                        (if (> (string-length text) 4000) (string-append (substring text 0 4000) "\n…") text)))))
    (hash-table-set! %process-log key
                     (let ((all (append entries (list entry))))
                       (if (> (length all) %process-log-max) (drop all (- (length all) %process-log-max)) all)))
    (let ((b (buffer-named (process-buffer-name repo))))
      (when b (render-process-log! b repo)))))

(define (process-buffer-name repo) (string-append "*vc-process " (cadr repo) "*"))

(define-mode vc-process-mode
  "The commands version control ran in a repository and what they said."
  #:parent 'special-mode
  #:keys '(("q" quit-window) ("g" vc-process-refresh))
  #:normal '(("q" quit-window)))

(define (render-process-log! b repo)
  (present! (buffer-document b)
    (append (list (row (list (list (string-append "Commands run in " (cadr repo)) 'heading)) #:key 'head)
                  (row (list (list "Newest last · q quit" 'vc.hint)) #:key 'keys))
            (let ((entries (hash-table-ref/default %process-log (cadr repo) '())))
              (if (null? entries) (list (row (list (list "No commands run yet" 'shadow)) #:key 'empty))
                  (append-map* (lambda (e i)
                                (cons (row (list (list (if (eq? (caddr e) 'ok) "  ok  " " fail ") (if (eq? (caddr e) 'ok) 'success 'error))
                                                 (list (car e) 'heading) "  "
                                                 (list (label (string-join (cadr e) " ")) 'shadow))
                                           #:key (list 'run i))
                                      (map (lambda (line j) (row (string-append "      " (label line)) #:key (list 'out i j)))
                                           (if (string=? (cadddr e) "") '() (string-split (cadddr e) "\n"))
                                           (iota (length (if (string=? (cadddr e) "") '() (string-split (cadddr e) "\n")))))))
                              entries (iota (length entries))))))))

(define-command (vc-process-refresh s n)
  "Show the version control process log again."
  (let ((b (session-buffer s)))
    (render-process-log! b (buffer-state b))))

(define-command (vc-process-log s n)
  "Show the commands version control ran here, and what they said."
  (let* ((b (session-buffer s)) (st (and b (buffer-state b)))
         (repo (or (and (hash-table? st) (hash-table-ref/default st 'repo #f))
                   (error "Open version control status first")))
         (log (or (buffer-named (process-buffer-name repo))
                  (make-generated-buffer! (process-buffer-name repo) (make-presentation) 'vc-process-mode #:state repo))))
    (render-process-log! log repo)
    (pop-to-buffer! s log)
    (let ((d (buffer-document log))) (move! s (lambda (_) (document-length d))))))

(define (chomp s) (if (string-suffix? "\n" s) (substring s 0 (- (string-length s) 1)) s))

;; append-map over several lists at once.
(define (append-map* f . lists) (apply append (apply map f lists)))

;; The repository DIR is in: Jujutsu's when there is one, else Git's. Its
;; root is a location on the machine DIR is on; the tools run there, in
;; it, given paths there.
(define (repository dir)
  ;; Connected first: failing to is said as itself, not as no repository.
  (location-endpoint dir)
  (let ((at (lambda (root) (location-at dir (chomp root))))
        (here (location-path dir)))
    (or (guard (e (#t #f)) (list 'jj (at (run dir "jj" (list "-R" here "root" "--ignore-working-copy")))))
        (guard (e (#t #f)) (list 'git (at (run dir "git" (list "-C" here "rev-parse" "--show-toplevel")))))
        (error "Not in a repository" dir))))

;; Run the tool of REPO in its root with ARGS.
(define (run-in repo args #:input [input #f] #:exit [on-exit #f])
  (run (cadr repo) (symbol->string (car repo)) args #:input input #:exit on-exit))

(define (nonempty-lines text)
  (filter (lambda (s) (not (string=? s ""))) (string-split text "\n")))

(define (git-summary repo)
  (let* ((fields (string-split (run-in repo '("--no-optional-locks" "status" "--porcelain=v1" "-z" "--branch" "--untracked-files=all"))
                               (string (integer->char 0))))
         (header (car fields)))
    (cons (if (string-prefix? "## " header) (substring header 3 (string-length header)) "")
          (let loop ((rest (if (string-prefix? "## " header) (cdr fields) fields)) (out '()))
            (if (or (null? rest) (< (string-length (car rest)) 3))
                (reverse out)
                (let* ((line (car rest)) (xy (substring line 0 2))
                       (renamed (or (memv (string-ref xy 0) '(#\R #\C)) (memv (string-ref xy 1) '(#\R #\C))))
                       (tail (cdr rest)))
                  (loop (if (and renamed (pair? tail)) (cdr tail) tail)
                        (cons (list xy (substring line 3 (string-length line))
                                    (and renamed (pair? tail) (car tail))) out))))))))

;; Neither snapshot the working copy nor integrate concurrent operations.
(define (jj-read repo args)
  (run-in repo (append '("--ignore-working-copy" "--no-integrate-operation" "--color" "never") args)))

(define %default-jj-revset "ancestors(@, 12) | visible_heads() | root()")

(define %jj-limit 60)

;; Graph lines: (prefix commit change conflict empty wc immutable bookmarks
;; remote-bookmarks tags author when description), or (prefix) alone for
;; a line of the graph between changes.
(define (jj-graph repo #:revset [revset %default-jj-revset] #:limit [limit %jj-limit])
  (map (lambda (line) (string-split line "\t"))
       (nonempty-lines
        (jj-read repo (list "log" (string-append "--revisions=" revset) "-n" (number->string limit) "-T" %jj-row-template)))))

;; Each entry ends with a field ~, so that empty fields before it count.
(define (jj-entry? fields) (and (>= (length fields) 14) (string=? (last fields) "~")))
(define (jj-entry-commit f) (list-ref f 1))
(define (jj-entry-change f) (list-ref f 2))
(define (jj-entry-description f) (let ((d (drop f 12))) (string-join (take d (- (length d) 1)) "\t")))

(define %git-sections '((conflict . "Conflicts") (untracked . "Untracked") (worktree . "Unstaged") (index . "Staged")))

(define (file-sides xy)
  (cond ((string=? xy "??") '(untracked))
        ((conflicted? xy) '(conflict))
        (else (append (if (char=? #\space (string-ref xy 1)) '() '(worktree))
                      (if (char=? #\space (string-ref xy 0)) '() '(index))))))

(define (git-file repo entry side)
  (let* ((xy (car entry)) (i (if (eq? side 'index) 0 1)))
    ;; A rename in the index doesn't make its source an unstaged path.
    (vc-file repo (cadr entry) (string-trim xy) #:side side
             #:old (and (memv (string-ref xy i) '(#\R #\C)) (caddr entry)))))

(define (changes repo)
  (if (eq? (car repo) 'jj)
      (map (lambda (line) (vc-file repo (substring line 2 (string-length line)) (substring line 0 1) #:side 'recorded))
           (nonempty-lines (jj-read repo '("diff" "--summary"))))
      (append-map (lambda (entry) (map (lambda (side) (git-file repo entry side)) (file-sides (car entry))))
                  (cdr (git-summary repo)))))

;; --absolute-git-dir is per worktree, not the common directory.
(define (git-in-progress repo)
  (let* ((root (cadr repo))
         (dir (chomp (run-in repo '("rev-parse" "--absolute-git-dir"))))
         (path (lambda (part) (location-at root (string-append dir "/" part))))
         (read-state (lambda (part)
                       (let ((p (path part)))
                         (if (remote-path? p)
                             (let ((bytes (node-file-read (location-endpoint p) (location-path p))))
                               (if bytes (chomp (utf8->string bytes)) ""))
                             (if (path-exists? p) (chomp (file->string p)) "")))))
         (rebase (spawn (lambda () (cond ((path-exists? (path "rebase-merge")) "rebase-merge")
                                         ((path-exists? (path "rebase-apply/rebasing")) "rebase-apply")
                                         (else #f)))))
         (merge (spawn (lambda () (read-state "MERGE_HEAD"))))
         (pick (spawn (lambda () (read-state "CHERRY_PICK_HEAD"))))
         (revert (spawn (lambda () (read-state "REVERT_HEAD"))))
         (rebase-dir (task-join rebase)) (merge-heads (task-join merge))
         (picked (task-join pick)) (reverted (task-join revert))
         ;; The steps done and still to do, without the todo's comments.
         (steps (lambda (part)
                  (filter (lambda (line) (not (string-prefix? "#" line)))
                          (nonempty-lines (read-state (string-append rebase-dir "/" part)))))))
    (cond (rebase-dir
           (let ((branch (read-state (string-append rebase-dir "/head-name")))
                 (onto (read-state (string-append rebase-dir "/onto"))))
             (list 'rebase (if (string-prefix? "refs/heads/" branch) (substring branch 11) branch) onto
                   (if (string=? rebase-dir "rebase-merge") (steps "done") '())
                   (if (string=? rebase-dir "rebase-merge") (steps "git-rebase-todo") '()))))
          ((not (string=? merge-heads "")) (cons 'merge (nonempty-lines merge-heads)))
          ((not (string=? picked "")) (list 'cherry-pick picked))
          ((not (string=? reverted "")) (list 'revert reverted))
          (else '()))))

(define (optional thunk)
  (guard (e ((not (cancelled-condition? e)) #f)) (thunk)))

;; Commits of RANGE, newest first: (sha short refs subject author when).
(define (git-commits repo range limit #:extra [extra '()])
  (map (lambda (line) (string-split line (string (integer->char 31))))
       (nonempty-lines
        (run-in repo (append (list "--no-optional-locks" "log" (string-append "--max-count=" (number->string limit))
                                   "--decorate=full" "--format=%H%x1f%h%x1f%D%x1f%s%x1f%an%x1f%ar")
                             extra (list range "--"))))))

(define (git-workspace repo)
  (let* ((later (lambda (thunk) (spawn (lambda () (optional thunk)))))
         (summary (spawn (lambda () (git-summary repo))))
         (operation (spawn (lambda () (git-in-progress repo))))
         (head (later (lambda () (optional-git-ref repo '("log" "-1" "--format=%h%x09%s" "--")))))
         (upstream (later (lambda () (optional-git-ref repo '("rev-parse" "--abbrev-ref" "--symbolic-full-name" "@{upstream}")))))
         (upstream-head (later (lambda () (optional-git-ref repo '("log" "-1" "--format=%h%x09%s" "@{upstream}" "--")))))
         (unpulled (later (lambda () (git-commits repo "HEAD..@{upstream}" 50))))
         (unpushed (later (lambda () (git-commits repo "@{upstream}..HEAD" 50))))
         (recent (later (lambda () (git-commits repo "HEAD" 10))))
         (stashes (later (lambda () (nonempty-lines (run-in repo '("--no-optional-locks" "stash" "list" "--format=%gd%x09%s"))))))
         (tag (later (lambda () (optional-git-ref repo '("describe" "--tags" "--abbrev=0" "HEAD")))))
         (push (later (lambda () (optional-git-ref repo '("rev-parse" "--abbrev-ref" "--symbolic-full-name" "@{push}"))))))
    (list (cons 'summary (task-join summary)) (cons 'operation (task-join operation)) (cons 'head-commit (task-join head))
          (cons 'upstream (task-join upstream)) (cons 'upstream-head (task-join upstream-head))
          (cons 'unpulled (or (task-join unpulled) '())) (cons 'unpushed (or (task-join unpushed) '()))
          (cons 'recent (or (task-join recent) '())) (cons 'stashes (or (task-join stashes) '()))
          (cons 'tag (task-join tag)) (cons 'push (task-join push)))))

;; The working-copy change and its parents, as graph rows are.
(define %jj-row-template
  (string-append
   "\"\\t\" ++ commit_id ++ \"\\t\" ++ change_id.shortest(8) ++ \"\\t\" ++ if(conflict, \"!\", \"\")"
   " ++ \"\\t\" ++ if(empty, \"empty\", \"\") ++ \"\\t\" ++ if(current_working_copy, \"@\", \"\")"
   " ++ \"\\t\" ++ if(immutable, \"immutable\", \"\")"
   " ++ \"\\t\" ++ local_bookmarks.map(|b| b.name()).join(\" \") ++ \"\\t\" ++ remote_bookmarks.map(|b| b.name() ++ \"@\" ++ b.remote()).join(\" \")"
   " ++ \"\\t\" ++ tags.map(|t| t.name()).join(\" \")"
   " ++ \"\\t\" ++ author.name() ++ \"\\t\" ++ committer.timestamp().ago()"
   " ++ \"\\t\" ++ description.first_line() ++ \"\\t~\\n\""))

(define (jj-workspace repo revset #:limit [limit %jj-limit])
  (let* ((graph (spawn (lambda () (jj-graph repo #:revset revset #:limit limit))))
         (files (spawn (lambda () (jj-file-entries repo "@"))))
         (info (spawn (lambda ()
                        (map (lambda (line) (string-split line "\t"))
                             (nonempty-lines (jj-read repo (list "log" "--no-graph" "-r" "@ | parents(@)" "-T" %jj-row-template)))))))
         (conflicts (spawn (lambda ()
                             (optional (lambda ()
                                         (nonempty-lines
                                          (jj-read repo (list "log" "--no-graph" "-r" "@" "-T"
                                                              "conflicted_files.map(|f| f.path() ++ \"\\n\").join(\"\")")))))))))
    (list (cons 'summary (task-join graph)) (cons 'operation '()) (cons 'head-commit #f)
          (cons 'wc-files (task-join files)) (cons 'wc-info (task-join info))
          (cons 'wc-conflicts (or (task-join conflicts) '())))))

(define (workspace repo #:revset [revset %default-jj-revset])
  (if (eq? (car repo) 'jj) (jj-workspace repo revset) (git-workspace repo)))

(define (diff repo path side old)
  (if (eq? (car repo) 'jj)
      (jj-read repo (if (eq? side 'revision) (list "diff" "--git" "-r" path) (list "diff" "--git" "--" path)))
      (if (eq? side 'untracked)
          (run (cadr repo) "git" (list "--no-optional-locks" "diff" "--no-color" "--no-ext-diff" "--no-textconv"
                                       "--no-index" "--" "/dev/null" path)
               #:accept-exit '(0 1))
          (run-in repo (append '("--no-optional-locks" "--literal-pathspecs" "-c" "diff.suppressBlankEmpty=false"
                                  "diff" "--no-color" "--no-ext-diff" "--no-textconv" "--src-prefix=a/" "--dst-prefix=b/")
                               (case side ((index) '("--cached")) ((worktree conflict) '()) (else '("HEAD")))
                               (if old (list "--" old path) (list "--" path)))))))

;; Treat a missing completion as uncertainty, not a retry request.
(define (write-evidence repo)
  (if (eq? (car repo) 'jj)
      (string-append "jj operation " (jj-operation-id repo))
      (let* ((head (chomp (run-in repo '("rev-parse" "--revs-only" "HEAD"))))
             (branch (optional-git-ref repo '("symbolic-ref" "--quiet" "HEAD")))
             (entries (run-in repo '("--no-optional-locks" "ls-files" "--stage" "-z"))))
        (string-append "Git HEAD " (if (string=? head "") "(unborn)" head)
                       " (" (or branch "detached") "); index entries SHA-256 " (hex-encode (sha-256 entries))))))

;; Git-backed jj exposes its Git directory through its own CLI, including
;; for a non-colocated or linked workspace. Only inspect it through Git.
(define (push-git repo args)
  (let ((prefix (if (eq? (car repo) 'jj)
                    (list "--git-dir" (chomp (jj-read repo '("git" "root")))) '())))
    (run (cadr repo) "git" (append prefix args))))

;; Resolve the effective push URL (not the fetch URL). Never display or
;; persist the URL: it can contain credentials.
(define (single-push-url repo remote)
  (let ((urls (guard (e ((not (cancelled-condition? e)) '()))
                (nonempty-lines (push-git repo (list "remote" "get-url" "--push" "--all" "--" remote))))))
    (unless (= (length urls) 1) (error "Expected exactly one configured Git push URL"))
    (car urls)))

(define (push-url-id url) (hex-encode (sha-256 url)))

(define (reviewed-push-url repo destination)
  (and (= (length destination) 4)
       (let ((url (single-push-url repo (cadr destination))))
         (and (string=? (cadddr destination) (push-url-id url)) url))))

(define (ensure-push-destination repo destination)
  (unless (or (null? destination) (reviewed-push-url repo destination))
    (error "Git push destination changed; reopen the push confirmation")))

(define (write-observation repo destination)
  (let ((state (write-evidence repo)))
    (if (null? destination) state
        (let* ((source (car destination)) (remote (cadr destination)) (ref (caddr destination))
               (observed
                (guard (e ((not (cancelled-condition? e)) "unavailable"))
                  (let ((url (reviewed-push-url repo destination)))
                    (cond ((not url) "unavailable (push destination changed or was not recorded)")
                          ((not (string=? url (chomp (push-git repo (list "ls-remote" "--get-url" "--" url)))))
                           "unavailable (fetch URL rewriting would change the push endpoint)")
                          (else
                           (let ((match (find (lambda (pair) (and (= (length pair) 2) (string=? (cadr pair) ref)))
                                              (map (lambda (line) (string-split line "\t"))
                                                   (nonempty-lines (push-git repo (list "ls-remote" "--refs" "--" url ref)))))))
                             (if match (car match) "missing"))))))))
          (string-append "Push " source " → " (label remote) ":" (label ref) "; observed tip " observed
                         (if (string=? source observed) " (matches requested commit)" "") "; " state)))))

(define (run-write! repo args title #:input [input #f] #:destination [destination '()] #:env [env #f])
  (guard (e ((not (cancelled-condition? e))
             (log-process! repo title args 'fail (error-text e))
             (raise e)))
    (let ((out (run-write-1! repo args title #:input input #:destination destination #:env env)))
      (log-process! repo title args 'ok (string-append (if (string? out) out "") %last-stderr))
      out)))

(define (run-write-1! repo args title #:input [input #f] #:destination [destination '()] #:env [env #f])
  (let ((st (vc-recovery-state repo)))
    (when (hash-table-ref/default st 'unresolved #f)
      (error "A remote write is unresolved; use C-x v ! before trying again"))
    (if (and (not (remote-path? (cadr repo))) (null? destination))
        (run (cadr repo) (symbol->string (car repo)) args #:input input #:env env)
        (let* ((before (write-observation repo destination))
               (attempt (begin (ensure-push-destination repo destination)
                               (vc-recovery-begin! repo title before #:destination destination))))
          (hash-table-set! st 'write-running #t)
          (dynamic-wind
            (lambda () #f)
            (lambda ()
              (run (cadr repo) (symbol->string (car repo)) args #:input input #:env env
                   #:exit (lambda (status)
                            (when (integer? status) (vc-recovery-complete! repo)))))
            (lambda ()
              (hash-table-set! st 'write-running #f)
              (when (eq? attempt (hash-table-ref/default st 'unresolved #f))
                (for-each (lambda (b)
                            (when (equal? repo (hash-table-ref (buffer-state b) 'repo)) (render-status! b)))
                          (status-buffers)))))))))

(define (revert repo path status side)
  (cond ((and (eq? (car repo) 'git) (eq? side 'worktree))
         (run-write! repo (list "--literal-pathspecs" "restore" "--worktree" "--" path) "Discard unstaged file edits"))
        (else (error "Cannot discard edits from this target" path))))

;;; Targets: a changed file, as (repo path status).

(define (vc-file repo path status #:side [side #f] #:old [old #f])
  (target 'vc-file (list repo path status side old)))
(define (file-path f) (string-append (cadr (car f)) "/" (cadr f)))

(define-action vc-file (vc-visit s f) "Open the file." (show-document! s (file-document (file-path f))))

(define-action vc-file (vc-diff s f)
  "Show what changed in the file."
  (show-diff! s f))

(define-action vc-file (vc-revert s f)
  "Discard the Git file's unstaged edits after asking."
  (unless (eq? (cadddr f) 'worktree) (error "Discard is only for unstaged Git files"))
  (completing-read s (string-append "Discard unstaged edits to " (label (cadr f)) "? ") '("yes" "no")
                   #:accept (lambda (s c)
                              (when (string=? (candidate-text c) "yes")
                                (request! (make-request-slot)
                                          (lambda () (revert (car f) (cadr f) (caddr f) (cadddr f)))
                                          (lambda (_)
                                            (message! s (string-append "Reverted " (cadr f)))
                                            (refresh-repository! (car f)))
                                          #:fail (lambda (e) (message! s (error-text e))))))))

(define-action vc-file (vc-copy-path s f) "Save the file's path as a kill." (kill-save! s (file-path f) #f #f))

(define (comparison t)
  (let ((v (target-value t)))
    (case (target-kind t)
      ((vc-change) (string-append "jj parents → " (substring (cadr v) 0 8) " (recorded)"))
      ((vc-jj-file) (string-append "Recorded jj file · " (label (caddr v)) " @ " (substring (cadr v) 0 8)))
      ((vc-index-preview) "Commit preview · HEAD → index")
      ((vc-amend-preview) "Amend preview · HEAD → index")
      ((vc-reword-preview) "Reword preview · existing HEAD patch; index is excluded")
      ((vc-jj-operation) (string-append "Jujutsu operation · " (substring (cadr v) 0 12)))
      ((vc-reviewed) (string-append "Reviewed revision · " (cadr v)))
      ((vc-git-commit vc-git-ref) (string-append "Git commit · " (cadr v)))
      ((vc-stash) (string-append "Stash · " (cadr v)))
      ((vc-git-range) (caddr v))
      ((vc-file-commit) (string-append (label (caddr v)) " @ " (substring (cadr v) 0 12)))
      (else (string-append (case (cadddr v) ((index) "HEAD → index") ((worktree) "Index → working tree")
                                                  ((conflict) "Conflict") ((untracked) "Untracked")
                                                  (else "jj working copy (recorded)"))
                           "  " (label (cadr v)))))))

(define (before-hunk lines)
  (let loop ((rest lines) (out '()))
    (if (or (null? rest) (string-prefix? "@@ " (car rest))) (reverse out)
        (loop (cdr rest) (cons (car rest) out)))))

;; Split one ordinary Git text patch; rename, mode and binary patches are
;; displayed but aren't cut into independently applicable fragments.
(define (split-index-patch text)
  (let* ((lines (string-split (chomp text) "\n"))
         (header (before-hunk lines))
         (body (drop lines (length header))))
    (and (= (length header) 4)
         (every (lambda (pair) (string-prefix? (cdr pair) (car pair)))
                (map cons header '("diff --git " "index " "--- a/" "+++ b/")))
         (not (any (lambda (line) (string-prefix? "diff --git " line)) body))
         (cons header
               (let loop ((rest body))
                 (if (null? rest) '()
                     (let ((part (cons (car rest) (before-hunk (cdr rest)))))
                       (cons part (loop (drop rest (length part)))))))))))

(define (patch-display lines #:hunk [hunk #f] #:part [part 0] #:key [key 'patch] #:indent [indent ""])
  (map (lambda (runs i)
         (row (cons indent runs) #:key (list key part i)
              #:target (and hunk (target 'vc-hunk (append (target-value hunk) (list i))))))
       (refine-patch-lines lines) (iota (length lines))))

(define-mode vc-diff-mode
  "A version control patch, with its comparison and Git hunk actions."
  #:parent 'special-mode
  #:directory (lambda (b)
                (let* ((st (buffer-state b)) (t (and (hash-table? st) (hash-table-ref/default st 'target #f)))
                       (v (and t (target-value t))))
                  (and (pair? v) (pair? (car v)) (string? (cadr (car v))) (normal-directory (cadr (car v))))))
  #:keys '(("s" vc-stage) ("u" vc-unstage) ("k" vc-discard-at-point) ("RET" vc-visit-at-point) ("n" vc-next-item) ("p" vc-previous-item)
           ("?" vc-diff-help) ("q" quit-window))
  #:normal '(("s" vc-stage) ("u" vc-unstage) ("k" vc-discard-at-point) ("RET" vc-visit-at-point) ("n" vc-next-item) ("p" vc-previous-item)
             ("?" vc-diff-help) ("q" quit-window))
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r)))))

(define-command (vc-diff-help s n)
  "Show what can be done with the patch at point."
  (let* ((t (target-at (doc s) (point s)))
         (scope (cond ((region-active? s) "selected lines") ((and t (eq? (target-kind t) 'vc-hunk)) "hunk") (else "file"))))
    (vc-transient! s
      `("Patch"
        ("At point" (suffix "s" ,(string-append "Stage " scope) vc-stage ,(lambda (s) (git-index-action? (target-at (doc s) (point s)) #t)))
                    (suffix "u" ,(string-append "Unstage " scope) vc-unstage ,(lambda (s) (git-index-action? (target-at (doc s) (point s)) #f)))
                    (suffix "k" ,(string-append "Discard " scope) vc-discard-at-point ,(lambda (s) (and (file-target (target-at (doc s) (point s))) #t)))
                    (suffix "RET" "Visit the file there" vc-visit-at-point))
        ("Move" (suffix "n" "Next hunk" vc-next-item) (suffix "p" "Previous hunk" vc-previous-item) (suffix "q" "Quit" quit-window))))))

(define (hunk-rows t text patch key indent)
  (apply append
         (map (lambda (part i)
                (let ((fragment (string-append (string-join (append (car patch) part) "\n") "\n")))
                  (patch-display part #:part i #:key key #:indent indent #:hunk (target 'vc-hunk (list t text fragment)))))
              (cdr patch) (iota (length (cdr patch)) 1))))

(define (render-patch! b t text)
  (let* ((st (buffer-state b))
         (side (and (eq? (target-kind t) 'vc-file) (cadddr (target-value t))))
         (editable (and (memq side '(worktree index)) (split-index-patch text)))
         (rows (cond ((string=? text "") (list (row "No differences" #:key 'empty)))
                     (editable
                      (append (patch-display (car editable))
                              (hunk-rows t text editable 'patch "")))
                     (else (patch-display (string-split (chomp text) "\n"))))))
    (hash-table-set! st 'target t)
    (hash-table-set! st 'rows
      (present! (buffer-document b)
                (append (list (row (list (list (comparison t) 'heading)) #:key 'comparison)
                              (row (list (list (cond ((and editable (eq? side 'worktree)) "n/p hunks  ·  s stage hunk or region  ·  k discard  ·  RET visit  ·  ? menu")
                                                     ((and editable (eq? side 'index)) "n/p hunks  ·  u unstage hunk or region  ·  RET visit  ·  ? menu")
                                                     (else "n/p hunks  ·  ? menu  ·  q quit")) 'vc.hint)) #:key 'keys))
                        rows)))))

;; The diff of the current target, shown beside the focused pane.
(define %diff-slot (make-request-slot))
(define (target-diff t)
  (let ((v (target-value t)))
    (case (target-kind t)
      ((vc-file) (diff (car v) (cadr v) (cadddr v) (list-ref v 4)))
      ((vc-change) (diff (car v) (cadr v) 'revision #f))
      ((vc-jj-file) (jj-read (car v) (append (list "diff" "--git" "-r" (cadr v) "--") (cadddr v))))
      ((vc-jj-operation) (jj-read (car v) (list "op" "diff" "--git" "--op" (cadr v))))
      ((vc-file-commit) (let ((repo (car v)) (sha (cadr v)) (path (caddr v)))
                          (if (eq? (car repo) 'jj)
                              (jj-read repo (list "diff" "--git" "-r" sha "--" (exact-jj-file path)))
                              (run-in repo (list "--no-lazy-fetch" "--literal-pathspecs" "show"
                                                 "--no-color" "--no-ext-diff" "--no-textconv" sha "--" path)))))
      ((vc-reviewed vc-git-commit vc-git-ref)
       (let ((repo (car v)) (sha (cadr v)))
         (if (eq? (car repo) 'jj) (jj-read repo (list "show" "--git" "-r" sha))
             (run-in repo (list "--no-lazy-fetch" "show" "--no-color" "--no-ext-diff" "--no-textconv" "--format=fuller"
                                "--stat" "--patch" "--decorate=short" sha "--")))))
      ((vc-stash)
       (run-in (car v) (list "stash" "show" "--no-color" "--no-ext-diff" "--include-untracked" "--stat" "--patch" (cadr v))))
      ((vc-git-range)
       (run-in (car v) (append '("--no-optional-locks" "diff" "--no-color" "--no-ext-diff" "--no-textconv" "--stat" "--patch")
                               (cadr v) '("--"))))
      (else (error "No diff at point")))))

(define-action vc-change (vc-show-change s change)
  "Inspect the recorded diff of this change."
  (show-target-diff! s (target 'vc-change change)))

(define (show-diff! s f) (show-target-diff! s (target 'vc-file f)))

(define (show-target-diff! s t)
  (request! %diff-slot
            (lambda () (target-diff t))
            (lambda (text)
              (let* ((name (string-append "*vc-diff " (cadr (car (target-value t))) "*"))
                     (b (or (buffer-named name)
                            (make-generated-buffer! name (make-presentation) 'vc-diff-mode #:state (make-hash-table)))))
                (guard (e (#t (message! s (error-text e))))
                  (render-patch! b t text)
                  (display-buffer! s (buffer-document b)))))
            #:fail (lambda (e) (message! s (error-text e)))))

;;; Status buffers. Their state is plain data, a table, so the package's
;;; next generation reads it as this one does.

;; Keys every repository view has, Magit's: sections, items, levels.
(define %section-keys
  '(("RET" vc-visit-at-point) ("TAB" vc-toggle-section) ("n" vc-next-item) ("p" vc-previous-item)
    ("M-n" vc-next-section) ("M-p" vc-previous-section) ("^" vc-parent-section)
    ("1" vc-show-level-1) ("2" vc-show-level-2) ("3" vc-show-level-3) ("4" vc-show-level-4)
    ("=" vc-diff-at-point) ("?" vc-dispatch) ("q" quit-window) ("$" vc-process-log)
    ("N" vc-forge-menu) ("!" vc-run-menu) ("C-c C-r" vc-status-refresh)))

(define-mode vc-status-mode
  "Show a repository's state in sections that fold.
That is Git's index and working tree, or Jujutsu's working-copy change
and graph.
\\[vc-dispatch] shows every command; \\[vc-visit-at-point] visits the
item at point, \\[vc-toggle-section] folds or shows it, \\[vc-status-refresh]
refreshes."
  #:parent 'special-mode
  #:directory (lambda (b) (normal-directory (cadr (hash-table-ref (buffer-state b) 'repo))))
  #:keys (append %section-keys '(("g" vc-status-refresh)))
  #:normal (append %section-keys '(("g r" vc-status-refresh)))
  #:target-at (lambda (b pos)
                (let ((st (buffer-state b)))
                  (and (hash-table? st)
                       (let ((r (row-at (buffer-document b) (hash-table-ref st 'rows) pos)))
                         (and r (row-target r)))))))

(define %git-status-keys
  '(("s" vc-stage) ("S" vc-stage-all) ("u" vc-unstage) ("U" vc-unstage-all) ("k" vc-discard-at-point)
    ("c" vc-commit-menu) ("b" vc-branch-menu) ("f" vc-fetch-menu) ("F" vc-pull-menu) ("P" vc-push-menu)
    ("m" vc-merge-menu) ("r" vc-rebase-menu) ("A" vc-cherry-pick-menu) ("V" vc-revert-menu)
    ("X" vc-reset-menu) ("z" vc-stash-menu) ("t" vc-tag-menu) ("l" vc-log-menu) ("d" vc-diff-menu)
    ("e" vc-conflicts-at-point) ("y" vc-show-refs)))

(define-mode vc-git-status-mode
  "Show Git's status as Magit does.
That is the branch and its upstream, untracked,
unstaged and staged changes, stashes, and the commits to pull and push.
\\[vc-stage] stages the file, hunk or lines at point, \\[vc-unstage]
unstages them, \\[vc-discard-at-point] discards them; \\[vc-commit-menu]
commits, \\[vc-push-menu] pushes, \\[vc-dispatch] shows every command."
  #:parent 'vc-status-mode
  #:keys %git-status-keys
  #:normal %git-status-keys)

(define %jj-status-keys
  '(("d" vc-jj-describe) ("c" vc-jj-commit) ("o" vc-jj-new-menu) ("O" vc-jj-new) ("e" vc-jj-edit)
    ("s" vc-jj-squash-menu) ("S" vc-jj-split) ("R" vc-jj-restore) ("r" vc-jj-rebase-menu)
    ("b" vc-jj-bookmark-menu) ("f" vc-jj-fetch-menu) ("P" vc-jj-push-menu) ("x" vc-jj-abandon) ("k" vc-jj-abandon)
    ("u" vc-jj-undo) ("U" vc-jj-redo) ("X" vc-jj-operations-menu) ("m" vc-jj-mark) ("M" vc-jj-unmark-all)
    ("v" vc-jj-evolog) ("A" vc-jj-absorb) ("y" vc-jj-duplicate) ("C" vc-jj-conflicts)
    ("/" vc-jj-filter) ("l" vc-jj-log-menu) ("+" vc-jj-more) ("G" vc-snapshot-working-copy)
    ("F" vc-jj-files) ("[" vc-jj-prev) ("]" vc-jj-next)))

(define-mode vc-jj-status-mode
  "Show Jujutsu's workspace as Majutsu does.
That is the working-copy change and its
files, then the graph of changes, each opening onto its files and hunks.
Commands act on the change, file or hunk at point, or on the changes
marked with \\[vc-jj-mark]. \\[vc-jj-undo] undoes the last operation;
\\[vc-dispatch] shows every command."
  #:parent 'vc-status-mode
  #:keys %jj-status-keys
  #:normal %jj-status-keys)

(define (status-mode? m) (and m (derived-mode? m 'vc-status-mode)))
(define (status-buffer? b) (and b (status-mode? (buffer-mode b))))
(define (status-buffers) (filter status-buffer? (buffer-list)))

(define (label s)
  (string-map (lambda (c) (cond ((char=? c #\newline) #\↵) ((char=? c #\tab) #\⇥)
                                ((char<? c #\space) #\?) (else c))) s))

(define (conflicted? xy) (member xy '("DD" "AU" "UD" "UA" "DU" "AA" "UU")))

;;; Status rows. A section is a heading row with a `vc-section` target,
;;; (section ID), and the rows after it to the next heading at its level.

(define (hint-rows text key)
  (if (option #f 'vc-show-hints) (list (row (list (list text 'vc.hint)) #:key key)) '()))

(define (section-heading id title n closed #:face [face 'vc.section])
  (row (list (list (string-append (if (memq id closed) "▸ " "▾ ") title) face)
             (if n (list (string-append " (" (number->string n) ")") 'shadow) ""))
       #:key (list 'section id) #:target (target 'vc-section id)))

;; A section: its heading and, unless folded, its rows.
(define (section id title items closed make-rows #:count [n #f] #:face [face 'vc.section])
  (if (null? items) '()
      (cons (section-heading id title (or n (length items)) closed #:face face)
            (if (memq id closed) '() (make-rows items)))))

(define (pad text width)
  (if (>= (string-length text) width) (string-append text " ")
      (string-append text (make-string (- width (string-length text)) #\space))))

(define (status-word code)
  (case code ((#\M) "modified") ((#\A) "new file") ((#\D) "deleted") ((#\R) "renamed")
        ((#\C) "copied") ((#\T) "typechange") ((#\?) "untracked") (else "changed")))

(define (conflict-word xy)
  (cond ((assoc xy '(("UU" . "both modified") ("AA" . "both added") ("DD" . "both deleted")
                     ("AU" . "added by us") ("UA" . "added by them") ("DU" . "deleted by us")
                     ("UD" . "deleted by them"))) => cdr)
        (else "unmerged")))

(define (git-file-rows repo entries side)
  (map (lambda (e)
         (let* ((f (git-file repo e side)) (xy (car e)) (path (cadr e)) (old (list-ref (target-value f) 4))
                (word (case side
                        ((index) (status-word (string-ref xy 0)))
                        ((worktree) (status-word (string-ref xy 1)))
                        ((conflict) (conflict-word xy))
                        (else #f))))
           (row (append (list "  ")
                        (if word (list (list (pad word 11) 'vc.file-status)) '())
                        (list (if old (string-append (label old) " → " (label path)) (label path))))
                #:key (list side path) #:target f)))
       entries))

;; Refs as %D writes them with --decorate=full: each in its face.
(define (ref-runs refs)
  (if (or (not refs) (string=? refs "")) '()
      (append
       (append-map
        (lambda (ref)
          (let* ((ref (string-trim ref))
                 (head? (string-prefix? "HEAD -> " ref))
                 (name (if head? (substring ref 8 (string-length ref)) ref)))
            (cond ((string=? name "HEAD") (list (list "HEAD" 'vc.branch) " "))
                  ((string-prefix? "tag: refs/tags/" name) (list (list (substring name 15 (string-length name)) 'vc.tag) " "))
                  ((string-prefix? "tag: " name) (list (list (substring name 5 (string-length name)) 'vc.tag) " "))
                  ((string-prefix? "refs/heads/" name)
                   (list (list (substring name 11 (string-length name)) 'vc.branch) " "))
                  ((string-prefix? "refs/remotes/" name)
                   (if (string-suffix? "/HEAD" name) '()
                       (list (list (substring name 13 (string-length name)) 'vc.remote) " ")))
                  ((string-prefix? "refs/stash" name) '())
                  (else (list (list (label name) 'vc.branch) " ")))))
        (string-split refs ","))
       '())))

;; A commit, as `git-commits` gives it: (sha short refs subject author when).
(define (commit-row repo c key #:graph [graph "  "] #:author [author #f])
  (row (append (list (list graph 'vc.graph) (list (cadr c) 'vc.hash) " ")
               (ref-runs (caddr c))
               (list (label (cadddr c)))
               (if (and author (> (length c) 5))
                   (list (list (string-append "  " (label (list-ref c 4)) ", " (list-ref c 5)) 'vc.author))
                   '()))
       #:key key #:target (target 'vc-git-commit (list repo (car c)))))

(define (git-branch-parts header)
  ;; "main...origin/main [ahead 1, behind 2]", "No commits yet on main",
  ;; "HEAD (no branch)".
  (let* ((bracket (string-contains header " ["))
         (names (if bracket (substring header 0 bracket) header))
         (counts (and bracket (substring header (+ bracket 1) (string-length header))))
         (dots (string-contains names "...")))
    (list (cond (dots (substring names 0 dots))
                ((string-prefix? "No commits yet on " names) (substring names 18 (string-length names)))
                ((string-prefix? "HEAD (no branch)" names) #f)
                (else names))
          counts)))

(define (git-operation-rows repo operation summary closed)
  (if (null? operation) '()
      (let* ((kind (car operation))
             (conflicts? (any (lambda (entry) (conflicted? (car entry))) (cdr summary)))
             (abbrev (lambda (s) (if (string=? s "") "unknown" (substring s 0 (min 8 (string-length s))))))
             (step-row (lambda (line face key)
                         (let ((words (string-split line " ")))
                           (row (list "  " (list (pad (car words) 7) face)
                                      (if (pair? (cdr words)) (list (abbrev (cadr words)) 'vc.hash) "")
                                      (if (> (length words) 2) (string-append " " (label (string-join (cddr words) " "))) ""))
                                #:key key
                                #:target (and (> (length words) 1) (target 'vc-git-commit (list repo (cadr words)))))))))
        (append
         (case kind
           ((rebase)
            (let ((done (list-ref operation 3)) (todo (list-ref operation 4)))
              (cons (section-heading 'sequence
                                     (string-append "Rebasing " (label (cadr operation)) " onto " (abbrev (caddr operation))
                                                    (if (pair? done)
                                                        (string-append " (" (number->string (length done)) "/"
                                                                       (number->string (+ (length done) (length todo))) ")")
                                                        ""))
                                     #f closed #:face 'warning)
                    (if (memq 'sequence closed) '()
                        (append (map (lambda (line i) (step-row line 'shadow (list 'todo i))) (reverse todo) (iota (length todo)))
                                (map (lambda (line i) (step-row line (if (= i 0) 'warning 'success) (list 'done i)))
                                     (reverse done) (iota (length done)))
                                (list (row (list "  " (list (pad "onto" 7) 'shadow) (list (abbrev (caddr operation)) 'vc.hash))
                                           #:key 'onto #:target (target 'vc-git-commit (list repo (caddr operation))))))))))
           ((merge) (list (row (list (list "Merging " 'warning) (list (string-join (map abbrev (cdr operation)) ", ") 'vc.hash))
                               #:key 'operation)))
           ((cherry-pick) (list (row (list (list "Cherry-picking " 'warning) (list (abbrev (cadr operation)) 'vc.hash))
                                     #:key 'operation #:target (target 'vc-git-commit (list repo (cadr operation))))))
           ((revert) (list (row (list (list "Reverting " 'warning) (list (abbrev (cadr operation)) 'vc.hash))
                                #:key 'operation #:target (target 'vc-git-commit (list repo (cadr operation))))))
           (else '()))
         (hint-rows (let ((verb (case kind ((rebase) "r") ((merge) "m") ((cherry-pick) "A") (else "V"))))
                      (if conflicts?
                          (string-append "e resolve conflict at point · s stage resolution · " verb " a abort")
                          (string-append (if (eq? kind 'merge) "c c commit the merge" (string-append verb " r continue"))
                                         " · " verb " a abort"
                                         (if (eq? kind 'rebase) " · r s skip" ""))))
                    'operation-next)))))

(define (git-status-rows repo st closed)
  (let* ((summary (hash-table-ref/default st 'summary #f))
         (get (lambda (k) (hash-table-ref/default st k #f)))
         (parts (and summary (git-branch-parts (car summary))))
         (branch (and parts (car parts)))
         (counts (and parts (cadr parts)))
         (head (get 'head-commit))
         (upstream (get 'upstream))
         (operation (or (get 'operation) '()))
         (entries (if summary (cdr summary) '()))
         (side-entries (lambda (side) (filter (lambda (e) (memq side (file-sides (car e)))) entries)))
         (unpushed (or (get 'unpushed) '()))
         (commits (lambda (id items) (map (lambda (c) (commit-row repo c (list id (car c)))) items))))
    (if (not summary)
        (list (row (list (list "Reading repository…" 'shadow)) #:key 'waiting))
        (append
         (list (row (append (list (pad "Head:" 10))
                            (if branch (list (list (label branch) 'vc.branch)) (list (list "(detached)" 'warning)))
                            (if head
                                (let ((p (string-split head "\t")))
                                  (list "  " (list (car p) 'vc.hash) " " (label (string-join (cdr p) "\t"))))
                                (list (list "  (no commits yet)" 'shadow))))
                    #:key 'head-commit
                    #:target (and head (target 'vc-git-commit (list repo (car (string-split head "\t")))))))
         (if upstream
             (list (row (append (list (pad "Merge:" 10) (list (label upstream) 'vc.remote))
                                (let ((u (get 'upstream-head)))
                                  (if u (let ((p (string-split u "\t")))
                                          (list "  " (list (car p) 'vc.hash) " " (label (string-join (cdr p) "\t"))))
                                      '()))
                                (if counts (list (list (string-append "  " counts) 'warning)) '()))
                        #:key 'upstream
                        #:target (let ((u (get 'upstream-head)))
                                   (and u (target 'vc-git-commit (list repo (car (string-split u "\t"))))))))
             '())
         (let ((push (get 'push)))
           (if (and push (not (equal? push upstream)))
               (list (row (list (pad "Push:" 10) (list (label push) 'vc.remote)) #:key 'push)) '()))
         (let ((tag (get 'tag)))
           (if tag (list (row (list (pad "Tag:" 10) (list (label tag) 'vc.tag)) #:key 'tag)) '()))
         (hint-rows "? all commands · s stage · u unstage · k discard · c commit · P push · TAB fold" 'hints)
         (git-operation-rows repo operation summary closed)
         (if (and (null? entries) (null? operation)) (list (row "No changes" #:key 'none)) '())
         (section 'conflict "Conflicts" (side-entries 'conflict) closed (lambda (es) (git-file-rows repo es 'conflict)))
         (section 'untracked "Untracked files" (side-entries 'untracked) closed (lambda (es) (git-file-rows repo es 'untracked)))
         (section 'worktree "Unstaged changes" (side-entries 'worktree) closed (lambda (es) (git-file-rows repo es 'worktree)))
         (section 'index "Staged changes" (side-entries 'index) closed (lambda (es) (git-file-rows repo es 'index)))
         (section 'stashes "Stashes" (or (get 'stashes) '()) closed
                  (lambda (items)
                    (map (lambda (line)
                           (let* ((p (string-split line "\t")) (name (car p)))
                             (row (list "  " (list name 'vc.hash) " " (label (string-join (cdr p) "\t")))
                                  #:key (list 'stash name) #:target (target 'vc-stash (list repo name)))))
                         items)))
         (if upstream
             (section 'unpulled (string-append "Unpulled from " upstream) (or (get 'unpulled) '()) closed
                      (lambda (items) (commits 'unpulled items)))
             '())
         (if upstream
             (section 'unpushed (string-append "Unmerged into " upstream) unpushed closed
                      (lambda (items) (commits 'unpushed items)))
             '())
         (if (null? unpushed)
             (section 'recent "Recent commits" (or (get 'recent) '()) closed (lambda (items) (commits 'recent items)))
             '())))))

;; The marker of a jj change: its graph node, then its IDs and names.
(define (jj-change-runs fields #:marked [marked #f])
  (let* ((prefix (car fields)) (commit (jj-entry-commit fields)) (change (jj-entry-change fields))
         (conflict (string=? (list-ref fields 3) "!")) (empty (string=? (list-ref fields 4) "empty"))
         (wc (string=? (list-ref fields 5) "@")) (immutable (string=? (list-ref fields 6) "immutable"))
         (words (lambda (s) (filter (lambda (w) (not (string=? w ""))) (string-split s " "))))
         (description (jj-entry-description fields)))
    (append (list (list prefix (if wc 'vc.working-copy 'vc.graph)))
            (if marked (list (list "*" 'vc.marked) " ") '())
            (list (list change 'vc.change-id) " " (list (substring commit 0 8) 'vc.commit-id) " ")
            (append-map (lambda (b) (list (list (label b) 'vc.branch) " ")) (words (list-ref fields 7)))
            ;; A colocated repository's @git bookmarks only echo the local ones.
            (append-map (lambda (b) (list (list (label b) 'vc.remote) " "))
                        (filter (lambda (b) (not (string-suffix? "@git" b))) (words (list-ref fields 8))))
            (append-map (lambda (b) (list (list (label b) 'vc.tag) " ")) (words (list-ref fields 9)))
            (if conflict (list (list "conflict" 'error) " ") '())
            (if empty (list (list "(empty)" 'shadow) " ") '())
            (list (if (string=? description "")
                      (list "(no description)" (if immutable 'shadow 'warning))
                      (label description)))
            (if (string=? (list-ref fields 10) "") '()
                (list (list (string-append "  " (label (list-ref fields 10)) ", " (list-ref fields 11)) 'vc.author))))))

(define (jj-file-status-word status)
  (cond ((string=? status "M") "modified") ((string=? status "A") "added") ((string=? status "D") "deleted")
        ((string=? status "R") "renamed") ((string=? status "C") "copied") (else status)))

;; Rows for the changed files of the recorded change COMMIT.
(define (jj-file-rows repo commit files indent #:change [change #f])
  (map (lambda (entry)
         (let* ((path (dict-ref entry "path")) (source (dict-ref entry "source"))
                (status (dict-ref entry "status"))
                (paired? (member status '("R" "C")))
                (paths (if (and paired? (not (string=? path source))) (list source path) (list path))))
           (row (list indent (list (pad (jj-file-status-word status) 10) 'vc.file-status)
                      (if paired? (string-append (label source) " → ") "") (label path)
                      (if paired? (list "  (inspect only)" 'shadow) ""))
                #:key (list 'jj-file (or change commit) path)
                #:target (target 'vc-jj-file (list repo commit path (map exact-jj-file paths) (not paired?) change)))))
       files))

(define (jj-status-rows repo st closed)
  (let* ((graph (hash-table-ref/default st 'summary #f))
         (info (or (hash-table-ref/default st 'wc-info #f) '()))
         (wc (find (lambda (f) (string=? (list-ref f 5) "@")) info))
         (parents (filter (lambda (f) (not (string=? (list-ref f 5) "@"))) info))
         (files (or (hash-table-ref/default st 'wc-files #f) '()))
         (conflicts (or (hash-table-ref/default st 'wc-conflicts #f) '()))
         (marks (or (hash-table-ref/default st 'marks #f) '()))
         (expanded (jj-expansions st))
         (revset (hash-table-ref/default st 'revset %default-jj-revset))
         (limit (hash-table-ref/default st 'limit %jj-limit))
         (seen (make-hash-table)))
    (if (not graph)
        (list (row (list (list "Reading repository…" 'shadow)) #:key 'waiting))
        (append
         (if wc
             (list (row (cons (pad "Working copy" 14) (cdr (jj-change-runs (cons "" (cdr wc)))))
                        #:key 'working-copy #:target (target 'vc-change (list repo (jj-entry-commit wc) (jj-entry-change wc)))))
             '())
         (map (lambda (p i)
                (row (cons (pad "Parent" 14) (cdr (jj-change-runs (cons "" (cdr p)))))
                     #:key (list 'parent i) #:target (target 'vc-change (list repo (jj-entry-commit p)))))
              parents (iota (length parents)))
         (list (row (list (pad "Revset" 14) (list (label revset) 'keyword)) #:key 'revset))
         (hint-rows "? all commands · d describe · o new · s squash · S split · r rebase · u undo · TAB fold"
                    'hints)
         (if wc
             (append
              (if (null? files)
                  (list (row (list (list "Working copy clean · g records edits since the last snapshot" 'shadow)) #:key 'clean))
                  (section 'wc-files "Working copy changes" files closed
                           (lambda (fs) (jj-file-rows repo (jj-entry-commit wc) fs "  " #:change (jj-entry-change wc)))))
              (section 'wc-conflicts "Conflicts" conflicts closed
                       (lambda (paths)
                         (map (lambda (path)
                                (row (list "  " (list (pad "conflict" 10) 'error) (label path))
                                     #:key (list 'conflict path)
                                     #:target (target 'vc-jj-conflict (list repo (jj-entry-commit wc) path))))
                              paths))))
             '())
         (cons (section-heading 'log (string-append "Log · " (label revset)) (count jj-entry? graph) closed)
               (if (memq 'log closed) '()
                   (append
                    (if (null? graph) (list (row "No changes match this revset" #:key 'empty)) '())
                    (append-map*
                     (lambda (fields i)
                       (if (not (jj-entry? fields))
                           (list (row (list (list (car fields) 'vc.graph)) #:key (list 'connector i)))
                           (let* ((commit (jj-entry-commit fields))
                                  (entry (hash-table-ref/default expanded (jj-entry-change fields) #f))
                                  ;; Files of another commit of the change: being read again.
                                  (open (and entry (if (and (string=? (car entry) commit) (not (eq? (cdr entry) 'loading)))
                                                       (cdr entry) 'loading)))
                                  ;; Keyed by change, so point stays on a change a
                                  ;; snapshot or rewrite gives a new commit; a
                                  ;; divergent change's other commits by commit.
                                  (change (jj-entry-change fields))
                                  (key (if (hash-table-ref/default seen change #f) (list 'jj change commit) (list 'jj change))))
                             (hash-table-set! seen change #t)
                             (cons (row (jj-change-runs fields #:marked (member commit marks))
                                        #:key key
                                        #:target (target 'vc-change (list repo commit change)))
                                   (cond ((not open) '())
                                         ((eq? open 'loading) (list (row (list (list "      Reading files…" 'shadow)) #:key (list 'loading commit))))
                                         ((null? open) (list (row (list (list "      No changed files" 'shadow)) #:key (list 'nofiles commit))))
                                         (else (jj-file-rows repo commit open "      " #:change change)))))))
                     graph (iota (length graph)))
                    (if (>= (count jj-entry? graph) limit)
                        (hint-rows (string-append "Up to " (number->string limit) " changes shown · + more · / revset") 'limit)
                        '()))))))))

(define (jj-expansions st)
  (or (hash-table-ref/default st 'expanded #f)
      (let ((table (make-hash-table))) (hash-table-set! st 'expanded table) table)))

(define (status-rows repo st)
  (let ((closed (hash-table-ref/default st 'closed '())) (failure (hash-table-ref/default st 'failure #f)))
    (append (list (row (list (list (if (eq? (car repo) 'git) "Git status" "Jujutsu workspace") 'heading)) #:key 'head)
                  (row (list (list (cadr repo) 'shadow)) #:key 'directory))
            (if failure (list (row (list (list (string-append "Stale: " (label failure)) 'warning)) #:key 'stale)) '())
            (if (eq? (car repo) 'git) (git-status-rows repo st closed) (jj-status-rows repo st closed)))))

;; The key an inline patch is kept under: a Git file's side and path, a
;; jj file's commit and path.
(define (file-key t)
  (let ((v (target-value t)))
    (if (eq? (target-kind t) 'vc-jj-file)
        (list 'jj (if (and (> (length v) 5) (list-ref v 5)) (list-ref v 5) (cadr v)) (caddr v))
        (list (cadddr v) (cadr v)))))

(define (inline-patches st)
  (or (hash-table-ref/default st 'inline #f)
      (let ((table (make-hash-table))) (hash-table-set! st 'inline table) table)))

(define (with-inline-patches rows cached)
  (append-map (lambda (r)
                (let* ((t (row-target r))
                       (key (and t (memq (target-kind t) '(vc-file vc-jj-file)) (file-key t)))
                       (text (and key (hash-table-ref/default cached key #f)))
                       (patch (and (string? text) (split-index-patch text)))
                       (first (and (pair? (row-columns r)) (car (row-columns r))))
                       (indent (if (and (pair? first) (string? (car first))) (string-append (car first) "  ") "    ")))
                  (cons r (cond ((and (pair? text) (eq? (car text) 'loading))
                                 (list (row (list (list (string-append indent "Reading patch…") 'shadow)) #:key (list key 'loading))))
                                (patch (append (hint-rows (string-append indent (hunk-hint t)) (list key 'hunk-hint))
                                               (hunk-rows t text patch key indent)))
                                ;; Only the hunks: the row says which file.
                                ((string? text)
                                 (let* ((lines (nonempty-lines text))
                                        (hunks (let loop ((ls lines)) (cond ((null? ls) lines) ((string-prefix? "@@" (car ls)) ls) (else (loop (cdr ls)))))))
                                   (patch-display hunks #:key key #:indent indent)))
                                (else '()))))) rows))

(define (hunk-hint t)
  (if (eq? (target-kind t) 'vc-jj-file)
      "n/p hunk · s squash · S split · R restore · C-SPC selects lines · RET visit"
      (case (cadddr (target-value t))
        ((index) "n/p hunk · u unstage · k discard · C-SPC selects lines · RET visit")
        (else "n/p hunk · s stage · k discard · C-SPC selects lines · RET visit"))))

(define (render-status! b)
  (let* ((st (buffer-state b)) (recovery (vc-recovery-state (hash-table-ref st 'repo))))
    (hash-table-set! st 'rows
      (present! (buffer-document b)
        (with-inline-patches
         (append (cond ((hash-table-ref/default recovery 'unresolved #f)
                        (list (row (list (list (if (hash-table-ref/default recovery 'write-running #f)
                                                 "Remote write awaiting reply"
                                                 "Remote write outcome unknown · C-x v ! inspect") 'warning)) #:key 'unresolved)))
                       ((hash-table-ref/default recovery 'last-unresolved #f)
                        (list (row (list (list "Previous remote write unresolved, acknowledged · C-x v ! evidence" 'shadow)) #:key 'unresolved)))
                       (else '()))
                 (status-rows (hash-table-ref st 'repo) st))
         (inline-patches st))))))

(define (show-changes! b workspace)
  (let ((st (buffer-state b)))
    (for-each (lambda (kv) (hash-table-set! st (car kv) (cdr kv))) workspace)
    (hash-table-set! st 'failure #f)
    ;; Marks name commits; a rewrite gives them new IDs.
    (let ((graph (hash-table-ref/default st 'summary '())))
      (when (and (list? graph) (eq? (car (hash-table-ref st 'repo)) 'jj))
        (hash-table-set! st 'marks
                         (filter (lambda (c) (any (lambda (f) (and (jj-entry? f) (string=? (jj-entry-commit f) c))) graph))
                                 (or (hash-table-ref/default st 'marks #f) '())))))
    (render-status! b)))

(define (refresh! b #:snapshot [snapshot #f] #:revset [new-revset #f])
  (let* ((st (buffer-state b)) (repo (hash-table-ref st 'repo))
         (revset (or new-revset (hash-table-ref/default st 'revset %default-jj-revset)))
         (limit (hash-table-ref/default st 'limit %jj-limit))
         ;; Open patches and changes are read again with the rest, so they
         ;; stay open; a jj change rewritten since is followed to its new
         ;; commit by its change ID.
         (open (filter (lambda (kv) (target? (cdr kv))) (hash-table->alist (inline-targets st))))
         (expanded (hash-table->alist (jj-expansions st))))
    (request! (hash-table-ref st 'slot)
              (lambda ()
                (when snapshot (run-write! repo '("status") "Snapshot Jujutsu working copy"))
                (if (eq? (car repo) 'git)
                    (let* ((patches (map (lambda (kv) (cons (car kv) (spawn (lambda () (optional (lambda () (target-diff (cdr kv))))))))
                                         open))
                           (found (git-workspace repo)))
                      (list found (map (lambda (p) (list (car p) #f (task-join (cdr p)))) patches) '()))
                    (let* ((found (jj-workspace repo revset #:limit limit))
                           (commits (make-hash-table)))
                      (for-each (lambda (f) (when (jj-entry? f) (hash-table-set! commits (jj-entry-change f) (jj-entry-commit f))))
                                (append (cdr (assq 'summary found)) (cdr (assq 'wc-info found))))
                      (let* ((now (lambda (change) (and change (hash-table-ref/default commits change #f))))
                             (patches
                              (map (lambda (kv)
                                     (let* ((t (cdr kv)) (v (target-value t))
                                            (change (and (eq? (target-kind t) 'vc-jj-file) (> (length v) 5) (list-ref v 5)))
                                            (commit (now change))
                                            (t (if (and commit (not (string=? commit (cadr v))))
                                                   (target 'vc-jj-file (append (list (car v) commit) (cddr v)))
                                                   t)))
                                       (cons (car kv) (cons t (spawn (lambda () (optional (lambda () (target-diff t)))))))))
                                   open))
                             (files
                              (filter-map (lambda (kv)
                                            (let ((commit (now (car kv))))
                                              (and commit
                                                   (cons (car kv) (cons commit (spawn (lambda () (optional (lambda () (jj-file-entries repo commit))))))))))
                                          expanded)))
                        (list found
                              (map (lambda (p) (list (car p) (cadr p) (task-join (cddr p)))) patches)
                              (map (lambda (f) (list (car f) (cadr f) (task-join (cddr f)))) files))))))
              (lambda (result)
                (when new-revset (hash-table-set! st 'revset new-revset))
                (let ((table (inline-patches st)) (targets (inline-targets st)))
                  (for-each (lambda (p)
                              (if (string? (caddr p))
                                  (begin (hash-table-set! table (car p) (caddr p))
                                         (when (cadr p) (hash-table-set! targets (car p) (cadr p))))
                                  (begin (hash-table-delete! table (car p)) (hash-table-delete! targets (car p)))))
                            (cadr result)))
                (let ((table (jj-expansions st)))
                  (for-each (lambda (kv) (hash-table-delete! table (car kv))) expanded)
                  (for-each (lambda (f) (when (list? (caddr f)) (hash-table-set! table (car f) (cons (cadr f) (caddr f)))))
                            (caddr result)))
                (show-changes! b (car result)))
              #:fail (lambda (e)
                       (hash-table-set! st 'failure (error-text e))
                       (render-status! b)))))

(define (inline-targets st)
  (or (hash-table-ref/default st 'inline-targets #f)
      (let ((table (make-hash-table))) (hash-table-set! st 'inline-targets table) table)))

(define (refresh-repository! repo)
  (for-each refresh! (filter (lambda (b) (equal? (hash-table-ref (buffer-state b) 'repo) repo)) (status-buffers)))
  ;; Logs and lists of refs read again too, where they are.
  (for-each (lambda (b)
              (let ((st (buffer-state b)))
                (when (and (hash-table? st) (not (status-buffer? b)) (equal? (hash-table-ref/default st 'repo #f) repo))
                  (let ((again (hash-table-ref/default st 'refresh #f)))
                    (when (procedure? again) (again))))))
            (buffer-list)))

(define %open-slot (make-request-slot))

(define-command (vc-status s n)
  "Show the state of the repository of this buffer's file.
It is Magit's or Majutsu's status. Its rows come in the background:
the editor never
waits for the tool."
  (let ((dir (default-directory s)))
    (message! s "Looking for the repository…")
    (request! %open-slot
              (lambda () (let* ((repo (repository dir)) (b (buffer-named (string-append "*vc " (cadr repo) "*")))
                               (revset (if b (hash-table-ref/default (buffer-state b) 'revset %default-jj-revset) %default-jj-revset)))
                          (cons repo (workspace repo #:revset revset))))
              (lambda (found)
                (let ((b (status-buffer (car found))))
                  (show-buffer! s b)
                  (show-changes! b (cdr found))
                  (message! s #f)))
              #:fail (lambda (e) (message! s (error-text e))))))

;; REPO's status buffer, made the first time.
(define (status-buffer repo)
  (let ((name (string-append "*vc " (cadr repo) "*")))
    (or (buffer-named name)
        (let ((st (make-hash-table)) (p (make-presentation)))
          (hash-table-set! st 'repo repo)
          (hash-table-set! st 'slot (make-request-slot))
          (hash-table-set! st 'rows (present! p (status-rows repo st)))
          (make-generated-buffer! name p (if (eq? (car repo) 'jj) 'vc-jj-status-mode 'vc-git-status-mode) #:state st)))))

(define (current-status s)
  (let ((b (session-buffer s)))
    (unless (status-buffer? b) (error "Not a version control status buffer"))
    b))

(define-command (vc-status-refresh s n)
  "Read the repository again, keeping open sections and patches open.
In a Jujutsu workspace, record the working copy first, as jj does."
  (let* ((b (current-status s)) (st (buffer-state b)) (repo (hash-table-ref st 'repo)))
    (refresh! b #:snapshot (and (eq? (car repo) 'jj)
                                (not (hash-table-ref/default (vc-recovery-state repo) 'unresolved #f))))))

(define-command (vc-diff-at-point s n)
  "Show the patch of the file, hunk, change or commit at point beside.
The patch is in another pane."
  (let* ((t (target-at (doc s) (point s)))
         (subject (or (file-target t) (jj-file-target t)
                      (and t (memq (target-kind t) '(vc-change vc-git-commit vc-stash)) t))))
    (if subject (show-target-diff! s subject) (error "No diff at point"))))

(define-command (vc-snapshot-working-copy s n)
  "Record this Jujutsu working copy, then read the graph again."
  (let ((b (session-buffer s)))
    (if (and (status-buffer? b) (eq? (car (hash-table-ref (buffer-state b) 'repo)) 'jj))
        (if (request-pending? (hash-table-ref (buffer-state b) 'slot)) (message! s "Already reading the repository")
            (refresh! b #:snapshot #t))
        (message! s "Snapshot is only for a Jujutsu workspace"))))

(define (toggle-file-patch! s b f #:show [show 'toggle])
  (let* ((st (buffer-state b)) (table (inline-patches st)) (key (file-key f))
         (old (hash-table-ref/default table key #f)))
    (cond ((and old (not (eq? show #t)))
           (hash-table-delete! table key) (hash-table-delete! (inline-targets st) key) (render-status! b))
          ((or old (eq? show #f)) #f)
          (else
           (let ((pending (list 'loading)))
             (hash-table-set! table key pending)
             (hash-table-set! (inline-targets st) key f)
             (render-status! b)
             (request! (make-request-slot) (lambda () (target-diff f))
                       (lambda (text)
                         (when (and (eq? table (inline-patches (buffer-state b)))
                                    (eq? pending (hash-table-ref/default table key #f)))
                           (hash-table-set! table key text)
                           (render-status! b)))
                       #:fail (lambda (e)
                                (when (eq? pending (hash-table-ref/default table key #f))
                                  (hash-table-delete! table key)
                                  (hash-table-delete! (inline-targets st) key)
                                  (render-status! b)
                                  (message! s (error-text e))))))))))

;; A change's files open below it, kept by change ID: (commit . files).
(define (toggle-change-files! s b t #:show [show 'toggle])
  (let* ((st (buffer-state b)) (table (jj-expansions st)) (v (target-value t))
         (repo (car v)) (commit (cadr v)) (key (if (> (length v) 2) (caddr v) commit))
         (old (hash-table-ref/default table key #f)))
    (cond ((and old (not (eq? show #t))) (hash-table-delete! table key) (render-status! b))
          ((or old (eq? show #f)) #f)
          (else
           (let ((pending (cons commit 'loading)))
             (hash-table-set! table key pending)
             (render-status! b)
             (request! (make-request-slot) (lambda () (jj-file-entries repo commit))
                       (lambda (files)
                         (when (eq? pending (hash-table-ref/default table key #f))
                           (hash-table-set! table key (cons commit files))
                           (render-status! b)))
                       #:fail (lambda (e) (hash-table-delete! table key) (render-status! b) (message! s (error-text e)))))))))

(define (row-key-string key) (call-with-output-string (lambda (p) (write key p))))

(define-command (vc-toggle-section s n)
  "Fold or show the section at point.
That is a section's items, a file's patch, or a change's files."
  (let* ((b (session-buffer s)) (t (target-at (doc s) (point s))) (st (buffer-state b)))
    (cond ((and t (eq? (target-kind t) 'vc-section))
           (let* ((closed (hash-table-ref/default st 'closed '())) (side (target-value t)))
             (hash-table-set! st 'closed (if (memq side closed) (filter (lambda (entry) (not (eq? entry side))) closed)
                                            (cons side closed)))
             (render-status! b)
             (listing-goto! s b (row-key-string (list 'section side)))))
          ((and t (eq? (target-kind t) 'vc-change) (status-buffer? b))
           (toggle-change-files! s b t)
           (let ((r (row-at (doc s) (hash-table-ref st 'rows) (point s))))
             (when r (move! s (lambda (p) p)))))
          ((or (file-target t) (jj-file-target t))
           => (lambda (f)
                (if (and (eq? (target-kind f) 'vc-file) (eq? (cadddr (target-value f)) 'conflict))
                    (message! s "e resolves a conflicted file")
                    (begin (toggle-file-patch! s b f)
                           (listing-goto! s b (row-key-string (if (eq? (target-kind f) 'vc-jj-file)
                                                                  (list 'jj-file (cadr (file-key f)) (caddr (target-value f)))
                                                                  (list (cadddr (target-value f)) (cadr (target-value f))))))))))
          (else (message! s "Nothing to fold at point")))))

;; The line of the file a patch line at I of FRAGMENT's hunk stands for:
;; its new side's, the line after a deletion.
(define (hunk-line fragment i)
  (let* ((lines (string-split (chomp fragment) "\n"))
         (body (let loop ((ls lines)) (if (or (null? ls) (string-prefix? "@@" (car ls))) ls (loop (cdr ls)))))
         (header (if (pair? body) (string-split (car body) " ") '()))
         (start (if (> (length header) 2)
                    (let ((c (caddr header)))
                      (string->number (car (string-split (substring c 1 (string-length c)) ","))))
                    1)))
    (let loop ((ls (if (pair? body) (cdr body) '())) (j 1) (line (or start 1)))
      (cond ((or (null? ls) (>= j (max i 1))) line)
            ((string-prefix? "-" (car ls)) (loop (cdr ls) (+ j 1) line))
            (else (loop (cdr ls) (+ j 1) (+ line 1)))))))

(define-command (vc-visit-at-point s n)
  "Visit what is at point.
That is a file at the line of the hunk at point, a commit or change's
patch; on a section's heading, fold the section."
  (let ((t (target-at (doc s) (point s))))
    (cond ((not t) (message! s "No item at point"))
          ((eq? (target-kind t) 'vc-section) (vc-toggle-section s n))
          ((memq (target-kind t) '(vc-change vc-git-commit vc-stash)) (show-target-diff! s t))
          ((eq? (target-kind t) 'vc-jj-conflict)
           (let ((v (target-value t)) (origin (session-buffer s)))
             (request! (make-request-slot)
                       (lambda ()
                         (let ((found (find (lambda (f) (equal? (dict-ref f "path") (caddr v))) (jj-conflicted-files (car v) (cadr v)))))
                           (if found (dict-ref found "sides") 0)))
                       (lambda (sides) (jj-conflict-preview! s origin (car v) (cadr v) (caddr v) sides))
                       #:fail (lambda (e) (message! s (error-text e))))))
          ((eq? (target-kind t) 'vc-hunk)
           (let* ((v (target-value t)) (f (car v)) (fv (target-value f)))
             (if (and (eq? (target-kind f) 'vc-jj-file) (not (jj-working-copy-commit? s (cadr fv))))
                 (show-target-diff! s f)
                 (visit! s (file-path (if (eq? (target-kind f) 'vc-jj-file) (list (car fv) (caddr fv)) fv))
                         (hunk-line (caddr v) (cadddr v)) 1))))
          ((eq? (target-kind t) 'vc-jj-file)
           (let ((v (target-value t)))
             (if (jj-working-copy-commit? s (cadr v))
                 (show-document! s (file-document (file-path (list (car v) (caddr v)))))
                 (show-target-diff! s t))))
          ((file-target t) => (lambda (f) (show-document! s (file-document (file-path (target-value f))))))
          (else (message! s "Nothing to visit at point")))))

(define (jj-working-copy-commit? s commit)
  (let* ((b (session-buffer s)) (st (and (status-buffer? b) (buffer-state b)))
         (info (and st (hash-table-ref/default st 'wc-info #f)))
         (wc (and info (find (lambda (f) (string=? (list-ref f 5) "@")) info))))
    (and wc (string=? (jj-entry-commit wc) commit))))

;; The spans of the rows of buffer B for which KEEP? holds of the row.
(define (row-spans s keep?)
  (let* ((p (doc s)) (b (session-buffer s)) (rows (hash-table-ref (buffer-state b) 'rows)))
    (filter (lambda (span)
              (let ((r (row-at p rows (cadr span))))
                (and r (keep? r span))))
            (presentation-rows-between p 0 (document-length p)))))

(define (step-item! s direction)
  (let* ((p (doc s)) (at (point s))
         (items (row-spans s (lambda (r span)
                               (and (row-target r)
                                    (or (not (eq? (target-kind (row-target r)) 'vc-hunk))
                                        (string-prefix? "@@ " (string-trim (document-substring p (cadr span) (caddr span)))))))))
         (found (if (> direction 0)
                    (find (lambda (span) (> (cadr span) at)) items)
                    (find (lambda (span) (< (caddr span) at)) (reverse items)))))
    (when found (move! s (lambda (_) (cadr found))))))

(define-command (vc-next-item s n)
  "Move to the next item: a file, hunk, commit or change."
  (step-item! s 1))

(define-command (vc-previous-item s n)
  "Move to the previous item: a file, hunk, commit or change."
  (step-item! s -1))

(define (section-spans s)
  (row-spans s (lambda (r span) (let ((t (row-target r))) (and t (eq? (target-kind t) 'vc-section))))))

(define-command (vc-next-section s n)
  "Move to the next section's heading."
  (let ((found (find (lambda (span) (> (cadr span) (point s))) (section-spans s))))
    (if found (move! s (lambda (_) (cadr found))) (message! s "No next section"))))

(define-command (vc-previous-section s n)
  "Move to the previous section's heading."
  (let ((found (find (lambda (span) (< (caddr span) (point s))) (reverse (section-spans s)))))
    (if found (move! s (lambda (_) (cadr found))) (message! s "No previous section"))))

(define-command (vc-parent-section s n)
  "Move to the heading of the section point is in."
  (let ((found (find (lambda (span) (<= (cadr span) (point s))) (reverse (section-spans s)))))
    (when found
      (if (= (cadr found) (point s))
          (vc-previous-section s n)
          (move! s (lambda (_) (cadr found)))))))

;; Magit's levels: 1 headings only, 2 their items, 3 and 4 with patches.
(define (show-level! s level)
  (let* ((b (current-status s)) (st (buffer-state b))
         (sections (filter-map (lambda (r) (let ((t (row-target r))) (and t (eq? (target-kind t) 'vc-section) (target-value t))))
                               (hash-table-values (hash-table-ref st 'rows)))))
    (hash-table-set! st 'closed (if (= level 1) sections '()))
    (render-status! b)
    (when (>= level 3)
      (let ((files (filter-map (lambda (r)
                                 (let ((t (row-target r)))
                                   (and t (or (and (eq? (target-kind t) 'vc-file) (memq (cadddr (target-value t)) '(index worktree untracked)) t)
                                              (and (eq? (target-kind t) 'vc-jj-file) t)))))
                               (hash-table-values (hash-table-ref st 'rows)))))
        (for-each (lambda (f) (toggle-file-patch! s b f #:show #t)) (take files (min 40 (length files))))))
    (when (= level 2)
      (for-each (lambda (k) (hash-table-delete! (inline-targets st) k) (hash-table-delete! (inline-patches st) k))
                (hash-table-keys (inline-patches st)))
      (hash-table-set! st 'expanded (make-hash-table))
      (render-status! b))
    (message! s (string-append "Level " (number->string level)))))

(define-command (vc-show-level-1 s n) "Show only the sections' headings." (show-level! s 1))
(define-command (vc-show-level-2 s n) "Show the sections' items, without patches." (show-level! s 2))
(define-command (vc-show-level-3 s n) "Show the sections' items and their patches." (show-level! s 3))
(define-command (vc-show-level-4 s n) "Show everything: items and their patches." (show-level! s 4))

(define %git-writes (make-hash-table))

(define (file-target t)
  (cond ((not t) #f) ((eq? (target-kind t) 'vc-file) t)
        ((and (eq? (target-kind t) 'vc-hunk) (eq? (target-kind (car (target-value t))) 'vc-file)) (car (target-value t)))
        (else #f)))

(define (jj-file-target t)
  (cond ((not t) #f) ((eq? (target-kind t) 'vc-jj-file) t)
        ((and (eq? (target-kind t) 'vc-hunk) (eq? (target-kind (car (target-value t))) 'vc-jj-file)) (car (target-value t)))
        (else #f)))

(define (git-index-action? t stage?)
  (let ((f (file-target t)))
    (and f (eq? (caar (target-value f)) 'git)
         (let ((side (cadddr (target-value f))))
           (if stage? (memq side '(worktree untracked)) (eq? side 'index))))))

(define (selected-patch-lines s b)
  (let ((active (filter (lambda (r) (not (= (car r) (cadr r)))) (ranges s))))
    (cond ((null? active) #f)
          ((not (= (length active) 1)) (error "Select a single line range within one hunk"))
          (else
           (let* ((range (car active)) (from (apply min range)) (to (apply max range)) (d (doc s))
                  (spans (filter (lambda (span) (and (< (cadr span) to) (> (caddr span) from)))
                                 (presentation-rows-between d from to))))
             (map (lambda (span)
                    (let ((r (row-at d (hash-table-ref (buffer-state b) 'rows) (cadr span))))
                      (and r (row-target r)))) spans))))))

;; Construct a genuine partial patch against the index, not a full patch
;; with its unselected lines removed. Keep unselected deletions as context;
;; omit unselected insertions. For unstaging, start from the reversed hunk.
;; SELECTED may span several hunks of one file: each gives a hunk of the
;; patch, whose lines past the first git apply --recount finds by context.
(define (partial-index-patch t selected stage?)
  (let* ((data (target-value t))
         (lines (filter (lambda (line) (and line (eq? (target-kind line) 'vc-hunk))) selected))
         (fragments (let loop ((ls lines) (out '()))
                      (cond ((null? ls) (reverse out))
                            ((member (caddr (target-value (car ls))) out) (loop (cdr ls) out))
                            (else (loop (cdr ls) (cons (caddr (target-value (car ls))) out)))))))
    (when (null? lines) (error "Select changed lines inside a file's hunks"))
    (unless (every (lambda (line) (equal? (take (target-value line) 2) (take data 2))) lines)
      (error "The region must stay inside one file's patch"))
    (let* ((header (take (string-split (chomp (car fragments)) "\n") 4))
           (hunks (filter-map (lambda (fragment)
                                (partial-hunk fragment
                                              (filter-map (lambda (line) (and (equal? (caddr (target-value line)) fragment)
                                                                              (cadddr (target-value line))))
                                                          lines)
                                              stage?))
                              fragments)))
      (when (null? hunks) (error "The selected region contains no changed lines"))
      (string-append (string-join (append (list (car header) (caddr header) (cadddr header)) (apply append hunks)) "\n") "\n"))))

(define (first-hunk selected)
  (or (find (lambda (t) (and t (eq? (target-kind t) 'vc-hunk))) selected)
      (error "Select changed lines inside a file's hunks")))

;; The range line and body of FRAGMENT's hunk with only the lines at
;; SELECTED-IDS changed, or #f when none of them is a change.
(define (partial-hunk fragment selected-ids stage?)
  (let* ((lines (string-split (chomp fragment) "\n"))
         (part (drop lines 4)) (positions (string-split (car part) " "))
         (coordinate (if stage? (cadr positions) (caddr positions)))
         (source-start (string->number (car (string-split (substring coordinate 1 (string-length coordinate)) ","))))
         (edits 0)
         (body (filter-map (lambda (entry)
                             (let* ((line (car entry)) (i (cdr entry))
                                    (marker (string-ref line 0))
                                    (sign (if stage? marker (case marker ((#\+) #\-) ((#\-) #\+) (else marker))))
                                    (selected? (memv i selected-ids))
                                    (content (substring line 1 (string-length line))))
                               (case sign
                                 ((#\space) line)
                                 ((#\\) line)
                                 ((#\+ #\-)
                                  (if selected?
                                      (begin (set! edits (+ 1 edits)) (string-append (string sign) content))
                                      (and (char=? sign #\-) (string-append " " content))))
                                 (else (error "Region staging needs an ordinary text hunk with complete lines")))))
                           (map cons (cdr part) (iota (length (cdr part)) 1)))))
    (and (> edits 0)
         (let* ((old (length (filter (lambda (line) (not (string-prefix? "+" line))) body)))
                (new (length (filter (lambda (line) (not (string-prefix? "-" line))) body)))
                (dest-start (cond ((= old 0) (+ source-start 1)) ((= new 0) (- source-start 1)) (else source-start))))
           (cons (string-append "@@ -" (number->string source-start) "," (number->string old)
                                " +" (number->string dest-start) "," (number->string new) " @@")
                 body)))))

(define (index-edit! s stage?)
  (let* ((b (session-buffer s)) (selected (selected-patch-lines s b))
         (t (if selected (first-hunk selected) (target-at (doc s) (point s))))
         (f (file-target t)))
    (when (and selected (not (and t (eq? (target-kind t) 'vc-hunk))))
      (error "Select changed lines inside one text hunk"))
    (unless (git-index-action? t stage?) (error (if stage? "No unstaged file or hunk at point" "No staged file or hunk at point")))
    (let* ((v (target-value f)) (repo (car v)) (path (cadr v)) (old (list-ref v 4))
           (paths (if old (list old path) (list path))) (hunk (eq? (target-kind t) 'vc-hunk))
           (slot (or (hash-table-ref/default %git-writes repo #f) (make-request-slot)))
           (partial (and selected (partial-index-patch t selected stage?))))
      (when (request-pending? slot) (error "Already writing this Git index"))
      (hash-table-set! %git-writes repo slot)
      (request! slot
                (lambda ()
                  (let ((fresh (find (lambda (item) (equal? (target-value item) v)) (changes repo))))
                    (unless fresh (error "File status changed; refresh before editing the index")))
                  (if hunk
                      (let* ((data (target-value t)) (shown (cadr data)) (fragment (or partial (caddr data))))
                        (unless (string=? (target-diff f) shown) (error "Patch changed; reopen the diff before editing the index"))
                        (run-write! repo (append '("apply" "--cached" "--recount")
                                                (if (or stage? partial) '() '("--reverse")) '("-"))
                                    (if stage? "Stage Git hunk" "Unstage Git hunk") #:input fragment))
                      (run-write! repo (append (if stage? '("--literal-pathspecs" "add" "-A" "--")
                                                  '("--literal-pathspecs" "reset" "-q" "--")) paths)
                                  (if stage? "Stage Git file" "Unstage Git file")))
                  (and hunk (target-diff f)))
                (lambda (patch)
                  (when (and patch (eq? (buffer-mode b) 'vc-diff-mode) (eq? f (hash-table-ref (buffer-state b) 'target)))
                    (render-patch! b f patch))
                  (for-each (lambda (view)
                              (let* ((st (buffer-state view)) (table (inline-patches st)) (key (file-key f)))
                                (when (and (equal? repo (hash-table-ref st 'repo)) (hash-table-ref/default table key #f))
                                  (if patch (hash-table-set! table key patch) (hash-table-delete! table key)))))
                            (status-buffers))
                  (when (and selected (eq? b (session-buffer s)))
                    (sset! s 'extend #f)
                    (move! s (lambda (p) p)))
                  (message! s (if stage? "Staged" "Unstaged"))
                  (refresh-repository! repo))
                #:fail (lambda (e)
                         (message! s (error-text e))
                         (refresh-repository! repo))))))

(define (region-active? s) (any (lambda (r) (not (= (car r) (cadr r)))) (ranges s)))

(define (section-at s)
  (let ((t (target-at (doc s) (point s))))
    (and t (eq? (target-kind t) 'vc-section) (not (region-active? s)) (target-value t))))

;; The Git files of the status section SIDE.
(define (section-files b side)
  (let* ((st (buffer-state b)) (repo (hash-table-ref st 'repo)) (summary (hash-table-ref/default st 'summary #f)))
    (if (and summary (eq? (car repo) 'git))
        (map (lambda (e) (git-file repo e side))
             (filter (lambda (e) (memq side (file-sides (car e)))) (cdr summary)))
        '())))

(define (file-paths files)
  (append-map (lambda (f) (let ((v (target-value f))) (if (list-ref v 4) (list (list-ref v 4) (cadr v)) (list (cadr v))))) files))

(define (git-index! s repo args title done)
  (let ((slot (or (hash-table-ref/default %git-writes repo #f) (make-request-slot))))
    (when (request-pending? slot) (error "Already writing this Git index"))
    (hash-table-set! %git-writes repo slot)
    (request! slot (lambda () (run-write! repo args title))
              (lambda (_) (message! s done) (refresh-repository! repo))
              #:fail (lambda (e) (message! s (error-text e)) (refresh-repository! repo)))))

(define (git-head? repo) (and (optional-git-ref repo '("rev-parse" "--verify" "--quiet" "HEAD")) #t))

(define (stage-section! s side stage?)
  (let* ((b (session-buffer s)) (repo (hash-table-ref (buffer-state b) 'repo)) (files (section-files b side)))
    (cond ((null? files) (message! s "Nothing in this section"))
          ((and stage? (memq side '(worktree untracked)))
           (git-index! s repo (append '("--literal-pathspecs" "add" "-A" "--") (file-paths files))
                       "Stage section" (string-append "Staged " (number->string (length files)) " files")))
          ((and (not stage?) (eq? side 'index))
           (git-index! s repo (append '("--literal-pathspecs" "reset" "-q" "--") (file-paths files))
                       "Unstage section" (string-append "Unstaged " (number->string (length files)) " files")))
          ((eq? side 'conflict) (message! s "Resolve each conflicted file, then stage it with s"))
          (else (message! s (if stage? "Nothing to stage in this section" "Nothing to unstage in this section"))))))

(define-command (vc-stage s n)
  "Stage the Git file, hunk or selected lines at point.
On a section's heading, stage every file of the section."
  (let ((side (section-at s)) (t (target-at (doc s) (point s))))
    (cond (side (stage-section! s side #t))
          ((git-conflict-target? (file-target t)) (vc-git-resolve s n))
          (else (index-edit! s #t)))))

(define-command (vc-unstage s n)
  "Unstage the Git file, hunk or selected lines at point.
On a section's heading, unstage every file of the section."
  (let ((side (section-at s)))
    (if side (stage-section! s side #f) (index-edit! s #f))))

(define-command (vc-stage-all s n)
  "Stage every change to tracked files.
With a prefix argument, untracked files too."
  (let ((repo (current-git-repo s)))
    (git-index! s repo (if (> n 1) '("add" "-A") '("add" "-u")) "Stage all" "Staged all changes")))

(define-command (vc-unstage-all s n)
  "Unstage everything staged, keeping the working tree as it is."
  (let ((repo (current-git-repo s)))
    (request! (make-request-slot) (lambda () (git-head? repo))
              (lambda (head?)
                (git-index! s repo (if head? '("reset" "-q" "HEAD" "--" ".") '("rm" "-r" "-q" "--cached" "."))
                            "Unstage all" "Unstaged everything"))
              #:fail (lambda (e) (message! s (error-text e))))))

;; Ask with one key, y or n, then do it.
(define (vc-y-or-n s question thunk)
  (vc-read-char-choice! s (string-append question " (y or n) ")
                        (list (cons "y" thunk) (cons "n" (lambda () (message! s "Nothing discarded"))))))

(define (discard-files! s repo side files)
  (let* ((paths (file-paths files)) (n (length files))
         (what (if (= n 1) (label (cadr (target-value (car files)))) (string-append (number->string n) " files"))))
    (case side
      ((untracked)
       (vc-y-or-n s (string-append "Delete untracked " what "?")
                  (lambda () (git-index! s repo (append '("--literal-pathspecs" "clean" "-f" "-q" "--") paths)
                                         "Delete untracked files" (string-append "Deleted " what)))))
      ((worktree)
       (vc-y-or-n s (string-append "Discard unstaged changes to " what "?")
                  (lambda () (git-index! s repo (append '("--literal-pathspecs" "restore" "--worktree" "--") paths)
                                         "Discard unstaged changes" (string-append "Discarded changes to " what)))))
      ((index)
       (vc-y-or-n s (string-append "Discard staged and unstaged changes to " what ", back to HEAD?")
                  (lambda () (git-index! s repo (append '("--literal-pathspecs" "restore" "--source=HEAD" "--staged" "--worktree" "--") paths)
                                         "Discard staged changes" (string-append "Reset " what " to HEAD")))))
      (else (message! s "Resolve a conflicted file with e, or abort the operation")))))

(define (discard-hunk! s t selected)
  (let* ((data (target-value t)) (f (car data)) (v (target-value f)) (repo (car v)) (side (cadddr v))
         (shown (cadr data))
         (patch (if selected (partial-index-patch t selected #f) (caddr data)))
         (args (append '("apply" "--recount")
                       (if (eq? side 'index) '("--index") '())
                       ;; A partial patch is already reversed; a whole hunk is reversed here.
                       (if selected '() '("--reverse")) '("-"))))
    (when (and selected (eq? side 'index)) (error "Unstage the lines first, then discard them"))
    (vc-y-or-n s (string-append "Discard " (if selected "the selected lines" "this hunk") " of " (label (cadr v)) "?")
      (lambda ()
        (request! (make-request-slot)
                  (lambda ()
                    (unless (string=? (target-diff f) shown) (error "Patch changed; look at it again before discarding"))
                    (run-write! repo args "Discard hunk" #:input patch))
                  (lambda (_)
                    (sset! s 'extend #f)
                    (message! s "Discarded")
                    (let ((b (session-buffer s)))
                      (when (status-buffer? b) (hash-table-delete! (inline-patches (buffer-state b)) (file-key f))
                        (toggle-file-patch! s b f #:show #t)))
                    (refresh-repository! repo))
                  #:fail (lambda (e) (message! s (error-text e)) (refresh-repository! repo)))))))

(define-command (vc-discard-at-point s n)
  "Discard the change at point after asking.
That is the lines selected, a hunk, a file, or every file of a section.
Untracked files are deleted."
  (let* ((b (session-buffer s)) (side (section-at s)))
    (if side
        (discard-files! s (hash-table-ref (buffer-state b) 'repo) side (section-files b side))
        (let* ((selected (selected-patch-lines s b))
               (t (if selected (first-hunk selected) (target-at (doc s) (point s)))))
          (cond ((and t (eq? (target-kind t) 'vc-hunk) (file-target t)) (discard-hunk! s t selected))
                ((file-target t) => (lambda (f) (discard-files! s (car (target-value f)) (cadddr (target-value f)) (list f))))
                ((and t (eq? (target-kind t) 'vc-stash)) (vc-stash-drop s n))
                ((jj-file-target t) (vc-jj-restore s n))
                ((and t (eq? (target-kind t) 'vc-change)) (vc-jj-abandon s n))
                (else (message! s "Nothing to discard at point")))))))

(define-mode vc-commit-mode
  "Edit a Git commit message while reviewing the staged snapshot."
  #:keys '(("C-c C-c" vc-commit-finish) ("C-c C-k" vc-commit-cancel)))

(define (git-index-token repo)
  (cons (list (chomp (run-in repo '("rev-parse" "--revs-only" "HEAD")))
              (chomp (run-in repo '("rev-parse" "--revs-only" "MERGE_HEAD")))
              (optional-git-ref repo '("symbolic-ref" "--quiet" "HEAD")))
        (chomp (run-in repo '("write-tree")))))

(define (git-index-snapshot repo)
  (let* ((token (git-index-token repo)) (head (caar token))
         (base (if (string=? head "") (chomp (run-in repo '("hash-object" "-t" "tree" "-w" "--stdin") #:input "")) head)))
    ;; Build the preview from immutable objects, not from a moving index.
    (cons token (run-in repo (list "diff" "--no-color" "--no-ext-diff" "--no-textconv" "--no-relative"
                                  "--ignore-submodules=none" "--submodule=short" "--full-index" "--binary"
                                  base (cdr token) "--")))))

;; Reuse cancelled messages only for the same operation and revision.
(define (open-vc-draft! s repo origin kind identity initial plan preview-target patch)
  (let* ((suffix (string-append " " (cadr repo) "*"))
         (name (string-append "*vc-" (symbol->string kind) suffix))
         (old (buffer-named name))
         (old-data (and old (buffer-state old)))
         (b (if (and (list? old-data) (= (length old-data) 5) (equal? (list-ref old-data 4) identity))
                old (make-generated-buffer! name (make-document initial) 'vc-commit-mode)))
         (preview-name (string-append "*vc-message-preview" suffix))
         (preview (or (buffer-named preview-name)
                      (make-generated-buffer! preview-name (make-presentation) 'vc-diff-mode #:state (make-hash-table)))))
    (set-buffer-state! b (list repo plan origin kind identity))
    (render-patch! preview preview-target patch)
    (show-buffer! s b)
    (display-buffer! s (buffer-document preview))
    (message! s "C-c C-c saves the message · C-c C-k keeps the draft and returns")))

(define (git-draft! s kind)
  (let* ((repo (current-git-repo s)) (origin (session-buffer s)) (args (vc-transient-args s))
         (extra (filter (lambda (a) (member a '("--no-verify" "--signoff" "--allow-empty"))) args)))
    (request! (make-request-slot)
              (lambda ()
                ;; -a stages first, so the preview shows what is committed.
                (when (and (member "--all" args) (not (eq? kind 'reword)))
                  (run-write! repo '("add" "-u") "Stage all modified files"))
                (let* ((snapshot (git-index-snapshot repo)) (token (car snapshot)) (head (caar token))
                       (new? (eq? kind 'commit)) (reword? (eq? kind 'reword)))
                  (when (and (not new?) (or (string=? head "") (not (string=? (cadar token) ""))))
                    (error "Amend and reword require an existing HEAD outside a merge"))
                  (if (and new? (string=? (cdr snapshot) "") (string=? (cadar token) "") (not (member "--allow-empty" args))) #f
                      (list token
                            (if reword? (chomp (run-in repo (list "rev-parse" (string-append head "^{tree}")))) (cdr token))
                            (if new? "" (chomp (run-in repo (list "show" "-s" "--format=%B" head "--"))))
                            (if reword? (target-diff (target 'vc-git-commit (list repo head))) (cdr snapshot))
                            head))))
              (lambda (draft)
                (guard (e (#t (message! s (error-text e))))
                  (if draft
                      (open-vc-draft! s repo origin kind (list-ref draft 4) (caddr draft) (append (take draft 2) (list extra))
                                      (target (case kind ((commit) 'vc-index-preview) ((amend) 'vc-amend-preview)
                                                    (else 'vc-reword-preview)) repo)
                                      (cadddr draft))
                      (message! s "Nothing staged"))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-commit s n)
  "Visit a Git commit message buffer with a staged-only preview."
  (git-draft! s 'commit))

(define-command (vc-git-amend s n)
  "Edit the HEAD message and amend it with the reviewed Git index."
  (git-draft! s 'amend))

(define-command (vc-git-reword s n)
  "Edit the HEAD message without incorporating staged changes."
  (git-draft! s 'reword))

(define-command (vc-commit-cancel s n)
  "Return to the repository view and keep the unsaved draft."
  (show-buffer! s (caddr (buffer-state (session-buffer s))))
  (message! s "Message draft kept; no write made"))

(define (close-vc-draft! s b st)
  (when (eq? b (session-buffer s)) (show-buffer! s (caddr st)))
  (drop-buffer! s b)
  ;; The preview pane goes with the message, as Magit's diff does.
  (let* ((preview (buffer-named (string-append "*vc-message-preview " (cadr (car st)) "*")))
         (i (and preview (list-index (lambda (v) (document=? (view-document v) (buffer-document preview))) (session-panes s)))))
    (when (and i (> (length (session-panes s)) 1))
      (let ((focus (session-focus s)))
        (delete-pane! s i)
        (sset! s 'focus (min (if (> focus i) (- focus 1) focus) (- (length (session-panes s)) 1)))))))

(define-command (vc-commit-finish s n)
  "Save this message in the reviewed Git or Jujutsu change."
  (let* ((b (session-buffer s)) (st (buffer-state b)) (repo (car st)) (plan (cadr st))
         (kind (cadddr st)) (d (doc s)) (message (document-substring d 0 (document-length d))))
    (case kind
      ((describe) (jj-write! s repo (car plan) (list "describe" "-r" (cdr plan) "--stdin")
                             #:input message #:after (lambda () (close-vc-draft! s b st))))
      ((jj-commit) (jj-write! s repo (car plan) (list "commit" "-m" (chomp message))
                              #:after (lambda () (close-vc-draft! s b st))))
      (else (git-finish! s b st repo plan kind message)))))

(define (git-finish! s b st repo plan kind message)
  (let ((slot (or (hash-table-ref/default %git-writes repo #f) (make-request-slot)))
        (args (append (case kind ((commit) '("commit" "-F" "-")) ((amend) '("commit" "--amend" "-F" "-"))
                                  (else '("commit" "--amend" "--only" "-F" "-")))
                      (if (> (length plan) 2) (caddr plan) '()))))
    (when (string=? (string-trim message) "") (error "Enter a commit message first"))
    (when (request-pending? slot) (error "Already writing this Git index"))
    (hash-table-set! %git-writes repo slot)
    (request! slot
              (lambda ()
                (unless (equal? (car plan) (git-index-token repo))
                  (error "HEAD or staged tree changed; reopen the commit draft to review it"))
                (run-write! repo args (case kind ((commit) "Commit staged Git changes") ((amend) "Amend Git commit")
                                           (else "Reword Git commit")) #:input message)
                (string=? (cadr plan) (chomp (run-in repo '("rev-parse" "HEAD^{tree}")))))
              (lambda (reviewed?)
                (close-vc-draft! s b st)
                (message! s (if reviewed? (case kind ((commit) "Committed staged changes") ((amend) "Amended HEAD")
                                                     (else "Reworded HEAD; index unchanged"))
                                "Commit created, but its tree differs from the preview; inspect HEAD"))
                (refresh-repository! repo))
              #:fail (lambda (e) (message! s (error-text e))))))

(define %jj-writes (make-hash-table))

(define (jj-operation-id repo)
  (chomp (jj-read repo '("op" "log" "-n" "1" "--no-graph" "-T" "id ++ \"\\n\""))))

(define (jj-write! s repo before args #:after [done #f] #:input [input #f] #:destination [destination '()] #:check-files [check-files #f] #:check [check #f])
  (let ((slot (or (hash-table-ref/default %jj-writes repo #f) (make-request-slot))))
    (when (request-pending? slot) (error "Already writing this Jujutsu repository"))
    (hash-table-set! %jj-writes repo slot)
    (request! slot
              (lambda ()
                (unless (string=? before (jj-operation-id repo))
                  (error "Jujutsu operation changed; refresh before trying again"))
                (when check-files
                  (run-write! repo '("status") "Check Jujutsu working copy before rewriting a selected file")
                  (unless (string=? before (jj-operation-id repo))
                    (error "Working copy changed; review the newly recorded patch before trying again")))
                (when check (check))
                (run-write! repo args (if (null? destination) (string-append "jj " (car args)) "Push Jujutsu bookmark")
                            #:input input #:destination destination)
                (jj-operation-id repo))
              (lambda (after)
                (message! s (if (string=? before after) "No Jujutsu operation recorded"
                                (string-append "Recorded Jujutsu operation " (substring after 0 12))))
                (refresh-repository! repo)
                (when done (done)))
              #:fail (lambda (e) (message! s (error-text e)) (refresh-repository! repo)))))

(define (current-jj-repo s)
  (let* ((b (session-buffer s)) (st (and b (buffer-state b)))
         (repo (and (hash-table? st) (hash-table-ref/default st 'repo #f))))
    (unless repo (error "Open a Jujutsu workspace first"))
    (if (eq? (car repo) 'jj) repo (error "This is not a Jujutsu workspace"))))

;; Writes resolve a stable change ID after jj's implicit snapshot. First
;; check the exact reviewed commit and reject a change with visible forks.
(define (jj-write-rev repo commit)
  (let* ((visible "::visible_heads() & ")
         (change (chomp (jj-read repo (list "log" "--no-graph" "-r" (string-append visible commit)
                                          "-T" "change_id ++ \"\\n\""))))
         (rev (string-append visible change)))
    (unless (and (not (string=? change ""))
                 (string=? commit (chomp (jj-read repo (list "log" "--no-graph" "-r" rev "-T" "commit_id ++ \"\\n\"")))))
      (error "Change is stale or divergent; refresh the graph"))
    rev))

;; The recorded commit of the change at point: a change's row, or the
;; change of the file, hunk or conflict at point.
(define (jj-change-at s)
  (let ((t (target-at (doc s) (point s))))
    (and t (case (target-kind t)
             ((vc-change vc-jj-conflict) (cadr (target-value t)))
             ((vc-jj-file) (cadr (target-value t)))
             ((vc-hunk) (let ((f (jj-file-target t))) (and f (cadr (target-value f)))))
             (else #f)))))

(define (jj-marks s)
  (let ((b (session-buffer s)))
    (if (status-buffer? b) (or (hash-table-ref/default (buffer-state b) 'marks #f) '()) '())))

(define (clear-marks! s)
  (let ((b (session-buffer s)))
    (when (status-buffer? b) (hash-table-set! (buffer-state b) 'marks '()))))

(define (with-jj-change s f)
  (let* ((repo (current-jj-repo s))
         (commit (or (jj-change-at s) (error "Move to a Jujutsu change first"))))
    (request! (make-request-slot)
              (lambda ()
                (cons (jj-operation-id repo) (jj-write-rev repo commit)))
              (lambda (resolved) (f repo commit (car resolved) (cdr resolved)))
              #:fail (lambda (e) (message! s (error-text e))))))

;; Resolve COMMITS (the marked changes, else the change at point) to
;; revisions, then call (F repo before revs).
(define (with-jj-changes s f #:marks [use-marks #t])
  (let* ((repo (current-jj-repo s))
         (marks (if use-marks (jj-marks s) '()))
         (commits (if (pair? marks) marks (list (or (jj-change-at s) (error "Move to a Jujutsu change first"))))))
    (request! (make-request-slot)
              (lambda () (cons (jj-operation-id repo) (map (lambda (c) (jj-write-rev repo c)) commits)))
              (lambda (resolved) (f repo (car resolved) (cdr resolved) commits))
              #:fail (lambda (e) (message! s (error-text e))))))

;; The changes the repository's graph shows, from whatever view of it.
(define (graph-of s)
  (let ((b (status-buffer (current-jj-repo s))))
    (filter jj-entry? (or (hash-table-ref/default (buffer-state b) 'summary #f) '()))))

;; Move the edits of PATH (all of SOURCE without one) into a change read.
(define (jj-pick-squash! s repo source before source-rev path files #:options [options '()] #:check [check #f])
  (read-change s (string-append "Squash " (if path (label path) (substring source 0 8)) " into: ") (list source)
    (lambda (dest)
      (request! (make-request-slot)
                (lambda () (cons (jj-write-rev repo dest) (chomp (jj-read repo (list "log" "--no-graph" "-r" dest "-T" "description")))))
                (lambda (r)
                  (jj-write! s repo before
                             (append (list "squash" "--from" source-rev "--into" (car r))
                                     (if (string=? (cdr r) "") '() '("--use-destination-message"))
                                     options (if path (cons "--" files) '()))
                             #:check-files (and path #t) #:check check))
                #:fail (lambda (e) (message! s (error-text e)))))))

(define (graph-description graph commit)
  (let ((f (find (lambda (f) (string=? (jj-entry-commit f) commit)) graph)))
    (if f (jj-entry-description f) "")))

;; Read a change of the graph other than EXCLUDED; (F commit).
(define (read-change s prompt excluded f)
  (let* ((repo (current-jj-repo s))
         (choices (filter (lambda (r) (and (not (member (jj-entry-commit r) excluded))
                                           (not (string-prefix? "000000000000" (jj-entry-commit r)))))
                          (graph-of s))))
    (if (null? choices) (message! s "No other change in this graph")
        (completing-read s prompt
          (map (lambda (r)
                 (candidate (string-append (jj-entry-change r) "  " (let ((d (jj-entry-description r))) (if (string=? d "") "(no description)" (label d))))
                            #:annotation (substring (jj-entry-commit r) 0 8) #:target (target 'vc-change (list repo (jj-entry-commit r)))))
               choices)
          #:accept (lambda (s c) (f (cadr (target-value (candidate-target c)))))))))

(define-command (vc-jj-filter s n)
  "Show the changes of a revset in the Jujutsu graph."
  (let ((b (current-status s)))
    (current-jj-repo s)
    (completing-read s "Jujutsu revset: " '() #:require-match #f #:history 'vc-jj-revset
      #:initial (hash-table-ref/default (buffer-state b) 'revset %default-jj-revset)
      #:accept (lambda (_ c)
                 (let ((text (string-trim (candidate-text c))))
                   (refresh! b #:revset (if (string=? text "") %default-jj-revset text)))))))

(define (set-revset! s revset)
  (refresh! (current-status s) #:revset revset))

(define-command (vc-jj-log-default s n) "Show the working copy's ancestors and every head." (set-revset! s %default-jj-revset))
(define-command (vc-jj-log-all s n) "Show every visible change." (set-revset! s "all()"))
(define-command (vc-jj-log-mine s n) "Show the changes you authored." (set-revset! s "mine() | @"))
(define-command (vc-jj-log-trunk s n) "Show the changes from trunk to the working copy and its heads." (set-revset! s "trunk()::@ | trunk()::visible_heads() | trunk()"))
(define-command (vc-jj-log-bookmarks s n) "Show the bookmarked changes and the working copy." (set-revset! s "bookmarks() | remote_bookmarks() | @"))

(define-command (vc-jj-more s n)
  "Show twice as many changes in the graph."
  (let* ((b (current-status s)) (st (buffer-state b)))
    (hash-table-set! st 'limit (* 2 (hash-table-ref/default st 'limit %jj-limit)))
    (refresh! b)))

(define-command (vc-jj-log-menu s n)
  "Show the menu of revsets the graph shows."
  (vc-transient! s
    '("Jujutsu log"
      ("Revset" (suffix "l" "Working copy ancestors and heads" vc-jj-log-default) (suffix "a" "All changes" vc-jj-log-all)
                (suffix "m" "Mine" vc-jj-log-mine) (suffix "t" "From trunk" vc-jj-log-trunk) (suffix "b" "Bookmarks" vc-jj-log-bookmarks))
      ("Other" (suffix "/" "A revset" vc-jj-filter) (suffix "+" "More changes" vc-jj-more)))))

(define-command (vc-jj-describe s n)
  "Edit the description of the change at point in a message buffer."
  (let ((origin (session-buffer s)))
    (with-jj-change s
      (lambda (repo commit before rev)
        (request! (make-request-slot)
                  (lambda () (cons (jj-read repo (list "log" "--no-graph" "-r" commit "-T" "description"))
                                   (target-diff (target 'vc-change (list repo commit)))))
                  (lambda (data)
                    (guard (e (#t (message! s (error-text e))))
                      (open-vc-draft! s repo origin 'describe rev (car data) (cons before rev)
                                      (target 'vc-change (list repo commit)) (cdr data))))
                  #:fail (lambda (e) (message! s (error-text e))))))))

(define-command (vc-jj-commit s n)
  "Describe the working-copy change and start a new one on it."
  (let* ((repo (current-jj-repo s)) (origin (session-buffer s)))
    (request! (make-request-slot)
              (lambda ()
                (let* ((commit (chomp (jj-read repo '("log" "--no-graph" "-r" "@" "-T" "commit_id"))))
                       (rev (jj-write-rev repo commit)))
                  (list (jj-operation-id repo) rev commit
                        (jj-read repo (list "log" "--no-graph" "-r" commit "-T" "description"))
                        (target-diff (target 'vc-change (list repo commit))))))
              (lambda (data)
                (guard (e (#t (message! s (error-text e))))
                  (open-vc-draft! s repo origin 'jj-commit (cadr data) (cadddr data) (cons (car data) (cadr data))
                                  (target 'vc-change (list repo (caddr data))) (list-ref data 4))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define (jj-new! s args-for)
  (with-jj-changes s
    (lambda (repo before revs commits)
      (clear-marks! s)
      (jj-write! s repo before (args-for revs)))))

(define (new-args s)
  (let ((message (vc-transient-value s "--message=")))
    (append (if (vc-transient-arg? s "--no-edit") '("--no-edit") '())
            (if message (list "-m" message) '()))))

(define-command (vc-jj-new s n)
  "Start a new change on the change at point, or on the marked ones.
On several marked changes, it merges them."
  (let ((args (new-args s)))
    (jj-new! s (lambda (revs) (append '("new") args revs)))))

(define-command (vc-jj-new-after s n)
  "Insert a new change after the change at point, before its children."
  (let ((args (new-args s)))
    (jj-new! s (lambda (revs) (append '("new") args (append-map (lambda (r) (list "--insert-after" r)) revs))))))

(define-command (vc-jj-new-before s n)
  "Insert a new change before the change at point, after its parents."
  (let ((args (new-args s)))
    (jj-new! s (lambda (revs) (append '("new") args (append-map (lambda (r) (list "--insert-before" r)) revs))))))

(define-command (vc-jj-new-merge s n)
  "Start a new change merging the marked changes and the change at point."
  (let ((args (new-args s)) (marks (jj-marks s)) (at (jj-change-at s)))
    (when (null? marks) (error "Mark the changes to merge with m first"))
    (let ((repo (current-jj-repo s)) (commits (if (and at (not (member at marks))) (append marks (list at)) marks)))
      (request! (make-request-slot)
                (lambda () (cons (jj-operation-id repo) (map (lambda (c) (jj-write-rev repo c)) commits)))
                (lambda (r) (clear-marks! s) (jj-write! s repo (car r) (append '("new") args (cdr r))))
                #:fail (lambda (e) (message! s (error-text e)))))))

(define-command (vc-jj-new-menu s n)
  "Show the menu for new changes."
  (vc-transient! s
    '("New change"
      ("Arguments" (switch "-n" "Don't edit the new change" "--no-edit")
                   (option "-m" "Description" "--message="))
      ("New change" (suffix "o" "On the change at point, or the marked" vc-jj-new)
                    (suffix "a" "Inserted after it" vc-jj-new-after)
                    (suffix "b" "Inserted before it" vc-jj-new-before)
                    (suffix "m" "Merging the marked and point" vc-jj-new-merge)))))

(define-command (vc-jj-edit s n)
  "Make the change at point the working-copy change; u undoes it."
  (with-jj-change s
    (lambda (repo commit before rev)
      (jj-write! s repo before (list "edit" rev)))))

(define-command (vc-jj-abandon s n)
  "Abandon the change at point, or the marked ones, after asking."
  (with-jj-changes s
    (lambda (repo before revs commits)
      (vc-y-or-n s (string-append "Abandon " (if (= 1 (length commits)) (substring (car commits) 0 8)
                                                 (string-append (number->string (length commits)) " changes"))
                                  " and rebase their descendants?")
        (lambda () (jj-write! s repo before (cons "abandon" revs)
                              #:check-files #t #:after (lambda () (clear-marks! s))))))))

(define-command (vc-jj-duplicate s n)
  "Copy the change at point, or the marked ones, onto the same parents."
  (with-jj-changes s
    (lambda (repo before revs commits)
      (clear-marks! s)
      (jj-write! s repo before (cons "duplicate" revs)))))

(define-command (vc-jj-absorb s n)
  "Move each edit of the change at point into an ancestor, as jj absorb.
That is the ancestor that last changed its lines."
  (with-jj-change s
    (lambda (repo commit before rev)
      (jj-write! s repo before (list "absorb" "--from" rev) #:check-files #t))))

(define-command (vc-jj-undo s n)
  "Undo the last Jujutsu operation; U redoes it."
  (let ((repo (current-jj-repo s)))
    (request! (make-request-slot) (lambda () (jj-operation-id repo))
              (lambda (before) (jj-write! s repo before '("undo") #:after (lambda () (message! s "Undone · U redoes"))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-jj-redo s n)
  "Redo the Jujutsu operation last undone."
  (let ((repo (current-jj-repo s)))
    (request! (make-request-slot) (lambda () (jj-operation-id repo))
              (lambda (before) (jj-write! s repo before '("redo") #:after (lambda () (message! s "Redone"))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-jj-mark s n)
  "Mark the change at point, or unmark it, for the next command."
  (let* ((b (current-status s)) (st (buffer-state b)) (t (target-at (doc s) (point s))))
    (unless (and t (eq? (target-kind t) 'vc-change)) (error "Move to a change in the graph"))
    (let* ((commit (cadr (target-value t))) (marks (or (hash-table-ref/default st 'marks #f) '())))
      (hash-table-set! st 'marks (if (member commit marks) (remove (lambda (c) (string=? c commit)) marks) (append marks (list commit))))
      (render-status! b)
      (step-item! s 1)
      (message! s (string-append (number->string (length (hash-table-ref st 'marks))) " marked · M unmarks all")))))

(define-command (vc-jj-unmark-all s n)
  "Unmark every change."
  (let ((b (current-status s)))
    (hash-table-set! (buffer-state b) 'marks '())
    (render-status! b)
    (message! s "Nothing marked")))

;; Run (F path files options check) for the file, hunk or selected lines
;; at point: OPTIONS select the hunk for a diff editor, CHECK refuses a
;; patch that changed since it was shown. For a whole file, OPTIONS are
;; empty; for a change, F gets #f.
(define (jj-selection-from s base f) (jj-selection s f #:base base))

(define (jj-selection s f #:base [base 'left])
  (let* ((b (session-buffer s)) (selected (selected-patch-lines s b))
         (t (if selected (first-hunk selected) (target-at (doc s) (point s))))
         (file (jj-file-target t)))
    (cond ((not file) (f #f #f '() #f))
          ((not (list-ref (target-value file) 4)) (error "Renames and copies are rewritten with their whole change"))
          ((and t (eq? (target-kind t) 'vc-hunk))
           (let* ((data (target-value t)) (shown (cadr data))
                  ;; A split or squash keeps the selection on the parent's
                  ;; side; a restore takes it out of the change's: jj shows
                  ;; restore -i the change as its left side.
                  (patch (if selected (partial-index-patch t selected (eq? base 'left)) (caddr data)))
                  (header (string-split (caddr data) "\n")))
             (unless (and (pair? (cdr header)) (or (string-suffix? " 100644" (cadr header)) (string-suffix? " 100755" (cadr header))))
               (error "Hunk selection needs a regular text file without a mode change"))
             (f (caddr (target-value file)) (cadddr (target-value file))
                (jj-hunk-options (caddr (target-value file)) patch #:base 'left #:reverse (and (eq? base 'right) (not selected)))
                (lambda () (unless (string=? shown (target-diff file)) (error "The patch changed; look at it again"))))))
          (else (f (caddr (target-value file)) (cadddr (target-value file)) '() #f)))))

(define (jj-squash! s into-rev-of)
  (let ((marks (jj-marks s)) (repo (current-jj-repo s)) (keep (vc-transient-arg? s "--keep-emptied")))
    (jj-selection s
      (lambda (path files options check)
        (let* ((source (or (jj-change-at s) (error "Move to a Jujutsu change first")))
               (sources (if (and (pair? marks) (not path)) marks (list source))))
          (into-rev-of source
            (lambda (dest)
              (if (member dest sources) (message! s "A change cannot be squashed into itself")
              (request! (make-request-slot)
                        (lambda ()
                          (list (jj-operation-id repo) (map (lambda (c) (jj-write-rev repo c)) sources) (jj-write-rev repo dest)
                                (chomp (jj-read repo (list "log" "--no-graph" "-r" dest "-T" "description")))))
                        (lambda (r)
                          (clear-marks! s)
                          (jj-write! s repo (car r)
                                     (append '("squash") (append-map (lambda (rev) (list "--from" rev)) (cadr r))
                                             (list "--into" (caddr r))
                                             (if keep '("--keep-emptied") '())
                                             (if (string=? (cadddr r) "") '() '("--use-destination-message"))
                                             options (if path (cons "--" files) '()))
                                     ;; Both described: keep the destination's
                                     ;; message rather than wait for an editor.
                                     #:check-files (and path #t) #:check check))
                        #:fail (lambda (e) (message! s (error-text e))))))))))))

(define-command (vc-jj-squash s n)
  "Squash the change, file, hunk or lines at point into the parent.
Marked changes are squashed into the change at point instead."
  (let ((marks (jj-marks s)))
    (if (pair? marks)
        (let ((dest (jj-change-at s)))
          (jj-squash! s (lambda (source go) (go dest))))
        (let ((repo (current-jj-repo s)))
          (jj-squash! s (lambda (source go)
                          (request! (make-request-slot)
                                    (lambda () (nonempty-lines (jj-read repo (list "log" "--no-graph" "-r" (string-append "parents(" source ")")
                                                                                 "-T" "commit_id ++ \"\\n\""))))
                                    (lambda (parents)
                                      (if (= (length parents) 1) (go (car parents))
                                          (message! s "A merge has several parents; squash into one with s i")))
                                    #:fail (lambda (e) (message! s (error-text e))))))))))

(define-command (vc-jj-squash-into s n)
  "Squash the change, file, hunk or lines at point into a change read."
  (jj-squash! s (lambda (source go) (read-change s "Squash into: " (list source) go))))

(define-command (vc-jj-squash-menu s n)
  "Show the squash menu."
  (vc-transient! s
    '("Squash"
      ("Arguments" (switch "-k" "Keep the emptied change" "--keep-emptied"))
      ("Squash what is at point" (suffix "s" "Into its parent; marked into point" vc-jj-squash)
                                 (suffix "i" "Into a change read" vc-jj-squash-into)))))

(define-command (vc-jj-split s n)
  "Split the file, hunk or lines at point out of their change.
They go into a change before it, with a description read. On a change,
the files to split out are read, one after another."
  (let ((repo (current-jj-repo s)))
    (jj-selection s
      (lambda (path files options check)
        (let* ((commit (or (jj-change-at s) (error "Move to a change, file or hunk first")))
               (split (lambda (files options check)
                        (read-name s (string-append "Description of the part split out of " (substring commit 0 8) ": ")
                          (lambda (message)
                            (request! (make-request-slot) (lambda () (cons (jj-operation-id repo) (jj-write-rev repo commit)))
                                      (lambda (r)
                                        (jj-write! s repo (car r) (append (list "split" "-r" (cdr r) "-m" message) options (cons "--" files))
                                                   #:check-files #t #:check check))
                                      #:fail (lambda (e) (message! s (error-text e)))))))))
          (if path (split files options check)
              (request! (make-request-slot) (lambda () (jj-file-entries repo commit))
                        (lambda (entries) (read-split-files s entries '() (lambda (chosen) (split chosen '() #f))))
                        #:fail (lambda (e) (message! s (error-text e))))))))))

;; Read files among ENTRIES until "Done"; (F files), exact paths.
(define %split-done "· Done: split these out")

(define (read-split-files s entries chosen f)
  (let ((left (filter (lambda (e) (not (member (exact-jj-file (dict-ref e "path")) chosen))) entries)))
    (if (null? left) (f chosen)
        (completing-read s (if (null? chosen) "Split out file: "
                               (string-append "Also split out (" (number->string (length chosen)) " chosen): "))
          (append (if (null? chosen) '() (list %split-done)) (map (lambda (e) (dict-ref e "path")) left))
          #:accept (lambda (s c)
                     (let ((path (candidate-text c)))
                       (cond ((string=? path %split-done) (f chosen))
                             ((find (lambda (e) (string=? (dict-ref e "path") path)) left)
                              (read-split-files s entries (append chosen (list (exact-jj-file path))) f))
                             (else (message! s (string-append "Not a file of this change: " (label path)))))))))))

(define-command (vc-jj-restore s n)
  "Discard the file, hunk or lines at point from their change, if sure.
On a change, discard all its edits."
  (let ((repo (current-jj-repo s)))
    (jj-selection-from s 'right
      (lambda (path files options check)
        (let ((commit (or (jj-change-at s) (error "Move to a change, file or hunk first"))))
          (vc-y-or-n s (string-append "Discard " (cond ((not path) "every edit of")
                                                       ((null? options) (string-append (label path) " in"))
                                                       (else (string-append "the selection of " (label path) " in")))
                                      " " (substring commit 0 8) "?")
            (lambda ()
              (request! (make-request-slot) (lambda () (cons (jj-operation-id repo) (jj-write-rev repo commit)))
                        (lambda (r)
                          (jj-write! s repo (car r) (append (list "restore" "--changes-in" (cdr r)) options (if path (cons "--" files) '()))
                                     #:check-files #t #:check check))
                        #:fail (lambda (e) (message! s (error-text e)))))))))))

(define (rebase-modes s)
  (let ((args (vc-transient-args s)))
    (cons (cond ((member "-r" args) "-r") ((member "-b" args) "-b") (else "-s"))
          (cond ((member "--insert-after" args) "--insert-after") ((member "--insert-before" args) "--insert-before")
                (else "--destination")))))

(define-command (vc-jj-rebase s n)
  "Rebase the change at point, or the marked ones, onto a change read."
  (let ((modes (rebase-modes s)))
    (with-jj-changes s
      (lambda (repo before revs commits)
        (read-change s (string-append "Rebase " (if (= 1 (length commits)) (substring (car commits) 0 8) "the marked changes")
                                      (case (string->symbol (cdr modes)) ((--insert-after) " after: ") ((--insert-before) " before: ") (else " onto: ")))
                     commits
          (lambda (dest)
            (request! (make-request-slot) (lambda () (jj-write-rev repo dest))
                      (lambda (dest-rev)
                        (clear-marks! s)
                        (jj-write! s repo before (append '("rebase") (append-map (lambda (r) (list (car modes) r)) revs)
                                                         (list (cdr modes) dest-rev))))
                      #:fail (lambda (e) (message! s (error-text e))))))))))

(define-command (vc-jj-rebase-marked-here s n)
  "Rebase the marked changes onto the change at point."
  (let ((modes (rebase-modes s)) (marks (jj-marks s)) (dest (jj-change-at s)) (repo (current-jj-repo s)))
    (when (null? marks) (error "Mark the changes to move with m first"))
    (unless dest (error "Move to the destination change"))
    (request! (make-request-slot)
              (lambda () (list (jj-operation-id repo) (map (lambda (c) (jj-write-rev repo c)) marks) (jj-write-rev repo dest)))
              (lambda (r)
                (clear-marks! s)
                (jj-write! s repo (car r) (append '("rebase") (append-map (lambda (rev) (list (car modes) rev)) (cadr r))
                                                  (list (cdr modes) (caddr r)))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-jj-rebase-menu s n)
  "Show the rebase menu."
  (vc-transient! s
    '("Rebase"
      ("What" (choice "-w" "Move" ("-s" "-r" "-b") 0))
      ("Where" (choice "-p" "Place" ("--destination" "--insert-after" "--insert-before") 0))
      ("Do" (suffix "r" "Point or marked, onto a change read" vc-jj-rebase)
            (suffix "m" "The marked, onto point" vc-jj-rebase-marked-here)))))

(define (bookmarks-at repo rev)
  (nonempty-lines (jj-read repo (list "bookmark" "list" "-r" rev "-T" "if(remote, \"\", name ++ \"\\n\")"))))

(define-command (vc-jj-bookmark s n)
  "Set a bookmark, new or existing, to the change at point."
  (let ((backwards (vc-transient-arg? s "--allow-backwards")))
    (with-jj-change s
      (lambda (repo commit before rev)
        (request! (make-request-slot)
                  (lambda () (nonempty-lines (jj-read repo '("bookmark" "list" "-T" "if(remote, \"\", name ++ \"\\n\")"))))
                  (lambda (names)
                    (completing-read s (string-append "Bookmark at " (substring commit 0 8) ": ") names #:require-match #f
                      #:accept (lambda (s c)
                                 (let ((name (string-trim (candidate-text c))))
                                   (when (string=? name "") (error "Enter a bookmark name"))
                                   (jj-write! s repo before (append (list "bookmark" "set" "-r" rev)
                                                                    (if backwards '("--allow-backwards") '())
                                                                    (list "--" name)))))))
                  #:fail (lambda (e) (message! s (error-text e))))))))

(define (read-bookmark s prompt f #:remote [remote #f])
  (let ((repo (current-jj-repo s)) (at (jj-change-at s)))
    (request! (make-request-slot)
              (lambda ()
                (let ((all (nonempty-lines (jj-read repo (append '("bookmark" "list") (if remote '("--all-remotes") '())
                                                                 (list "-T" (if remote "if(remote && remote != \"git\", name ++ \"@\" ++ remote ++ \"\\n\", \"\")"
                                                                                "if(remote, \"\", name ++ \"\\n\")")))))))
                  (if at (let ((here (bookmarks-at repo at))) (append (filter (lambda (b) (member b all)) here) (remove (lambda (b) (member b here)) all)))
                      all)))
              (lambda (names)
                (if (null? names) (message! s "No bookmarks")
                    (completing-read s prompt names #:accept (lambda (s c) (f repo (candidate-text c))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define (read-bookmark-from s prompt remote f) (read-bookmark s prompt f #:remote remote))

(define (bookmark-command! s args-of prompt #:remote [remote #f])
  (read-bookmark-from s prompt remote
    (lambda (repo name)
      (request! (make-request-slot) (lambda () (jj-operation-id repo))
                (lambda (before) (jj-write! s repo before (args-of name)))
                #:fail (lambda (e) (message! s (error-text e)))))))

(define-command (vc-jj-bookmark-delete s n)
  "Delete a bookmark, here and, on the next push, on its remotes."
  (bookmark-command! s (lambda (name) (list "bookmark" "delete" "--" name)) "Delete bookmark: "))

(define-command (vc-jj-bookmark-forget s n)
  "Forget a bookmark here, leaving its remotes as they are."
  (bookmark-command! s (lambda (name) (list "bookmark" "forget" "--" name)) "Forget bookmark: "))

(define-command (vc-jj-bookmark-rename s n)
  "Rename a bookmark."
  (read-bookmark s "Rename bookmark: "
    (lambda (repo old)
      (read-name-initial s (string-append "Rename " (label old) " to: ") old
        (lambda (new)
          (request! (make-request-slot) (lambda () (jj-operation-id repo))
                    (lambda (before) (jj-write! s repo before (list "bookmark" "rename" "--" old new)))
                    #:fail (lambda (e) (message! s (error-text e)))))))))

(define-command (vc-jj-bookmark-track s n)
  "Track a remote bookmark, making a local one that follows it."
  (bookmark-command! s (lambda (name) (list "bookmark" "track" "--" name)) "Track remote bookmark: " #:remote #t))

(define-command (vc-jj-bookmark-untrack s n)
  "Stop tracking a remote bookmark."
  (bookmark-command! s (lambda (name) (list "bookmark" "untrack" "--" name)) "Untrack remote bookmark: " #:remote #t))

(define-command (vc-jj-bookmark-menu s n)
  "Show the bookmark menu."
  (vc-transient! s
    '("Bookmark"
      ("Arguments" (switch "-B" "Allow moving backwards or sideways" "--allow-backwards"))
      ("At point" (suffix "b" "Set, new or existing" vc-jj-bookmark) (suffix "s" "Set, new or existing" vc-jj-bookmark))
      ("Do" (suffix "d" "Delete" vc-jj-bookmark-delete) (suffix "f" "Forget" vc-jj-bookmark-forget)
            (suffix "r" "Rename" vc-jj-bookmark-rename))
      ("Remote" (suffix "t" "Track" vc-jj-bookmark-track) (suffix "u" "Untrack" vc-jj-bookmark-untrack)))))

(define (exact-jj-file path)
  (string-append "root-file:" (json->string path)))

(define (jj-file-entries repo commit)
  (map string->json
       (nonempty-lines
        (jj-read repo (list "diff" "-r" commit "-T"
                            "json({\"path\" => path, \"source\" => source.path(), \"status\" => status_char}) ++ \"\\n\"")))))

(define-mode vc-jj-files-mode
  "Browse the changed files of an exact recorded Jujutsu change."
  #:parent 'special-mode
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("n" vc-next-item) ("p" vc-previous-item) ("RET" vc-jj-file-diff) ("=" vc-jj-file-diff)
           ("s" vc-jj-squash-file) ("S" vc-jj-split-file) ("k" vc-jj-restore-file) ("h" vc-jj-hunks)
           ("?" vc-jj-files-dispatch) ("q" vc-jj-files-return))
  #:normal '(("n" vc-next-item) ("p" vc-previous-item) ("RET" vc-jj-file-diff) ("=" vc-jj-file-diff)
             ("s" vc-jj-squash-file) ("S" vc-jj-split-file) ("k" vc-jj-restore-file) ("h" vc-jj-hunks)
             ("?" vc-jj-files-dispatch) ("q" vc-jj-files-return)))

(define-command (vc-jj-files-dispatch s n)
  "Choose an action in the Jujutsu changed-file list."
  (let* ((t (target-at (doc s) (point s)))
         (commands (append (if t '(("RET  Inspect recorded patch" . vc-jj-file-diff)) '())
                           (if (and t (list-ref (target-value t) 4))
                               '(("s  Squash file into another change" . vc-jj-squash-file)
                                 ("S  Split file into its own change" . vc-jj-split-file)
                                 ("h  Select an individual text hunk" . vc-jj-hunks)
                                 ("k  Restore file from parents" . vc-jj-restore-file)) '())
                           '(("n  Next file" . vc-next-item) ("p  Previous file" . vc-previous-item)
                             ("q  Back to graph" . vc-jj-files-return)))))
    (vc-choose-command s "Jujutsu files: " commands)))

(define-command (vc-jj-files s n)
  "List changed files for the recorded Jujutsu change at point."
  (let* ((t (target-at (doc s) (point s))) (origin (session-buffer s)))
    (unless (and t (eq? (target-kind t) 'vc-change)) (error "Move to a Jujutsu change first"))
    (let* ((v (target-value t)) (repo (car v)) (commit (cadr v)))
      (request! (make-request-slot) (lambda () (jj-file-entries repo commit))
                (lambda (files)
                  (let* ((name (string-append "*jj files " commit " " (cadr repo) "*"))
                         (b (or (buffer-named name)
                                (make-generated-buffer! name (make-presentation) 'vc-jj-files-mode #:state (make-hash-table))))
                         (st (buffer-state b)))
                    (hash-table-set! st 'origin origin)
                    (hash-table-set! st 'repo repo)
                    (hash-table-set! st 'rows
                      (present! (buffer-document b)
                        (append (list (row (list (list (string-append "Files of recorded change " (substring commit 0 8)) 'heading)) #:key 'head)
                                      (row (list (list "RET patch · h hunks · s squash file · S split file · k restore · ? commands · q graph" 'shadow)) #:key 'keys))
                                (if (null? files) (list (row "No changed files" #:key 'empty))
                                    (map (lambda (entry)
                                           (let* ((path (dict-ref entry "path")) (source (dict-ref entry "source"))
                                                  (status (dict-ref entry "status"))
                                                  (paired? (member status '("R" "C")))
                                                  (paths (if (and paired? (not (string=? path source))) (list source path) (list path))))
                                             (row (list (list status 'keyword) "  " (if paired? (string-append (label source) " → ") "") (label path)
                                                        (if paired? (list "  (inspect only)" 'shadow) "")) #:key path
                                                  #:target (target 'vc-jj-file (list repo commit path (map exact-jj-file paths) (not paired?)))))) files)))))
                    (when (eq? origin (session-buffer s)) (show-buffer! s b))))
                #:fail (lambda (e) (message! s (error-text e)))))))

(define-mode vc-jj-hunks-mode
  "Select a hunk of a recorded Jujutsu file, not an index entry."
  #:parent 'special-mode
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("n" vc-next-item) ("p" vc-previous-item) ("S" vc-jj-split-hunk) ("s" vc-jj-squash-hunk) ("q" vc-jj-hunks-return) ("?" vc-jj-hunks-help))
  #:normal '(("n" vc-next-item) ("p" vc-previous-item) ("S" vc-jj-split-hunk) ("s" vc-jj-squash-hunk) ("q" vc-jj-hunks-return) ("?" vc-jj-hunks-help)))

(define-command (vc-jj-hunks s n)
  "Select text hunks from the exact recorded file at point."
  (let* ((t (jj-file-at s)) (origin (session-buffer s)) (v (target-value t)))
    (unless (list-ref v 4) (error "Renames and copies cannot be selected by hunk"))
    (request! %diff-slot
      (lambda ()
        (let* ((text (target-diff t)) (patch (split-index-patch text)))
          (unless (and patch (pair? (cdr patch))
                       (or (string-suffix? " 100644" (cadar patch)) (string-suffix? " 100755" (cadar patch))))
            (error "Hunk selection needs a regular text file without a mode change"))
          (cons text patch)))
      (lambda (data)
        (guard (e (#t (message! s (error-text e))))
          (when (eq? origin (session-buffer s))
            (let* ((text (car data)) (patch (cdr data)) (header (car patch)))
            (let* ((name (string-append "*jj hunks " (caddr v) " · " (cadr (car v)) "*"))
                   (b (or (buffer-named name)
                          (make-generated-buffer! name (make-presentation) 'vc-jj-hunks-mode #:state (make-hash-table))))
                   (st (buffer-state b)))
              (hash-table-set! st 'origin origin)
              (hash-table-set! st 'repo (car v))
              (hash-table-set! st 'file t)
              (hash-table-set! st 'patch text)
              (hash-table-set! st 'rows
                (present! (buffer-document b)
                  (append (list (row (list (list (comparison t) 'heading)) #:key 'comparison)
                                (row (list (list "n/p hunk · S split selected hunk · s squash selected hunk · q files" 'shadow)) #:key 'keys))
                          (apply append (map (lambda (part i)
                                        (let ((fragment (string-append (string-join (append header part) "\n") "\n")))
                                          (cons (row (list (list (car part) 'diff.hunk)) #:key (list 'hunk i)
                                                     #:target (target 'vc-jj-hunk fragment))
                                                (patch-display (cdr part) #:part i))))
                                      (cdr patch) (iota (length (cdr patch))))))))
              (show-buffer! s b)
              (move! s (lambda (_) 0)))))))
      #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-jj-hunks-help s n)
  "Choose a command in the recorded hunk selector."
  (vc-choose-command s "Jujutsu hunks: "
    '(("n  Next hunk" . vc-next-item) ("p  Previous hunk" . vc-previous-item)
      ("S  Split the selected hunk" . vc-jj-split-hunk) ("s  Squash the selected hunk" . vc-jj-squash-hunk)
      ("q  Back to files" . vc-jj-hunks-return))))

(define-command (vc-jj-hunks-return s n)
  "Return to the recorded file list."
  (show-buffer! s (hash-table-ref (buffer-state (session-buffer s)) 'origin)))

(define (with-reviewed-jj-hunk s act)
  (let* ((b (session-buffer s)) (st (buffer-state b)) (h (target-at (doc s) (point s))))
    (unless (and h (eq? (target-kind h) 'vc-jj-hunk)) (error "Move to a hunk heading with n or p"))
    (let* ((t (hash-table-ref st 'file)) (v (target-value t)) (repo (car v)) (id (cadr v))
           (shown (hash-table-ref st 'patch)) (path (caddr v)) (options (jj-hunk-options path (target-value h)))
           (check (lambda () (unless (string=? shown (target-diff t)) (error "Hunks changed; reopen the patch")))))
      (request! (make-request-slot) (lambda () (cons (jj-operation-id repo) (jj-write-rev repo id)))
        (lambda (plan)
          (when (eq? b (session-buffer s)) (act repo id path (cadddr v) options check (car plan) (cdr plan))))
        #:fail (lambda (e) (message! s (error-text e)))))))

(define-command (vc-jj-split-hunk s n)
  "Split only the text hunk at point into a new Jujutsu change."
  (with-reviewed-jj-hunk s
    (lambda (repo id path files options check before rev)
      (completing-read s (string-append "Description for selected hunk of " (label path) ": ") '() #:require-match #f
        #:accept (lambda (s c)
                   (let ((message (string-trim (candidate-text c))))
                     (when (string=? message "") (error "Enter a description for the split"))
                     (jj-write! s repo before (append (list "split" "-r" rev "-m" message) options (cons "--" files))
                                #:check-files #t #:check check)))))))

(define-command (vc-jj-squash-hunk s n)
  "Move only the text hunk at point into a chosen Jujutsu change."
  (let* ((files (hash-table-ref (buffer-state (session-buffer s)) 'origin))
         (graph (hash-table-ref (buffer-state files) 'origin)))
    (with-reviewed-jj-hunk s
      (lambda (repo id path files options check before rev)
        (jj-pick-squash! s repo id before rev path files #:options options #:check check)))))

(define-mode vc-jj-conflict-mode
  "Inspect an exact recorded Jujutsu conflict before resolving it."
  #:parent 'special-mode
  #:keys '(("1" vc-jj-resolve-first) ("2" vc-jj-resolve-second) ("?" vc-jj-conflict-help) ("q" vc-jj-conflict-return))
  #:normal '(("1" vc-jj-resolve-first) ("2" vc-jj-resolve-second) ("?" vc-jj-conflict-help) ("q" vc-jj-conflict-return)))

(define (jj-conflict-help-actions b)
  (append (if (hash-table-ref/default (buffer-state b) 'can-resolve #f)
              '(("1  Take side 1 for this file" . vc-jj-resolve-first) ("2  Take side 2 for this file" . vc-jj-resolve-second)) '())
          '(("q  Back to graph" . vc-jj-conflict-return))))

(define-command (vc-jj-conflict-help s n)
  "Choose a command for the displayed Jujutsu conflict."
  (vc-choose-command s "Jujutsu conflict: " (jj-conflict-help-actions (session-buffer s))))

(define-command (vc-jj-conflict-return s n)
  "Return to the Jujutsu graph from the conflict inspector."
  (show-buffer! s (hash-table-ref (buffer-state (session-buffer s)) 'origin)))

(define (jj-conflict-preview! s origin repo commit path count)
  (request! %diff-slot
    (lambda () (jj-read repo (list "--config" "ui.conflict-marker-style=\"git\"" "file" "show" "-r" commit "--" (exact-jj-file path))))
    (lambda (text)
      (guard (e (#t (message! s (error-text e))))
        (when (eq? origin (session-buffer s))
          (let* ((name (string-append "*jj conflict " path " · " (cadr repo) "*"))
                 (b (or (buffer-named name)
                        (make-generated-buffer! name (make-presentation) 'vc-jj-conflict-mode #:state (make-hash-table))))
                 (st (buffer-state b))
                 (can-resolve (and (= count 2) (any (lambda (line) (string-prefix? "<<<<<<< " line)) (string-split text "\n")))))
            (hash-table-set! st 'origin origin)
            (hash-table-set! st 'plan (list repo commit path))
            (hash-table-set! st 'can-resolve can-resolve)
            (present! (buffer-document b)
              (append (list (row (list (list (string-append "Recorded jj conflict · " (label path) " @ " (substring commit 0 8)) 'heading)) #:key 'heading)
                            (row (list (list (if can-resolve "1 take top side · 2 take bottom side · ? commands · q graph"
                                                "Inspect only · edit the conflicted change to resolve manually · q graph") 'shadow)) #:key 'keys)
                            (row (list (list "A side choice replaces the entire conflicted file, not one marker block." 'shadow)) #:key 'scope))
                      (patch-display (string-split (chomp text) "\n"))))
            (show-buffer! s b)
            (move! s (lambda (_) 0))))))
    #:fail (lambda (e) (message! s (error-text e)))))

(define-command (vc-jj-conflicts s n)
  "Choose a conflicted file from the recorded Jujutsu change at point."
  (let* ((origin (session-buffer s)) (repo (current-jj-repo s))
         (commit (or (jj-change-at s) (error "Move to a recorded Jujutsu change first"))))
    (begin
      (request! (make-request-slot)
        (lambda () (jj-conflicted-files repo commit))
        (lambda (files)
          (when (eq? origin (session-buffer s))
            (if (null? files) (message! s "No recorded conflicts on this change")
                (completing-read s "Recorded conflict: "
                  (map (lambda (f) (candidate (label (dict-ref f "path")) #:target f)) files)
                  #:accept (lambda (s c)
                    (let ((f (candidate-target c)))
                      (jj-conflict-preview! s origin repo commit (dict-ref f "path") (dict-ref f "sides"))))))))
        #:fail (lambda (e) (message! s (error-text e)))))))

(define (jj-conflicted-files repo commit)
  (map string->json (nonempty-lines
    (jj-read repo (list "log" "--no-graph" "-r" commit "-T"
      "conflicted_files.map(|f| json({\"path\" => f.path(), \"sides\" => f.conflict_side_count()}) ++ \"\\n\").join(\"\")")))))

(define (jj-resolve-side! s number)
  (let* ((b (session-buffer s)) (st (buffer-state b)) (plan (hash-table-ref st 'plan)))
    (unless (hash-table-ref st 'can-resolve) (error "This conflict must be resolved manually"))
    (let ((repo (car plan)) (id (cadr plan)) (path (caddr plan)))
      (request! (make-request-slot) (lambda () (cons (jj-operation-id repo) (jj-write-rev repo id)))
        (lambda (basis)
          (when (eq? b (session-buffer s))
            (completing-read s (string-append "Replace all of " (label path) " with side " number " in " (substring id 0 8) "? ") '("no" "yes")
              #:accept (lambda (s c)
                (when (string=? (candidate-text c) "yes")
                  (jj-write! s repo (car basis)
                    (list "resolve" "-r" (cdr basis) "--tool" (if (string=? number "1") ":ours" ":theirs") "--" (exact-jj-file path))
                    #:check-files #t
                    #:after (lambda () (when (eq? b (session-buffer s)) (vc-jj-conflict-return s 1)))))))))
        #:fail (lambda (e) (message! s (error-text e)))))))

(define-command (vc-jj-resolve-first s n)
  "Resolve the whole file using Jujutsu's first side."
  (jj-resolve-side! s "1"))

(define-command (vc-jj-resolve-second s n)
  "Resolve the whole file using Jujutsu's second side."
  (jj-resolve-side! s "2"))

(define-command (vc-conflicts-at-point s n)
  "Inspect Git conflict blocks or a recorded Jujutsu change's conflicts."
  (let ((t (target-at (doc s) (point s))))
    (if (and t (eq? (target-kind t) 'vc-change)) (vc-jj-conflicts s n) (vc-git-conflicts s n))))

(define-command (vc-jj-files-return s n)
  "Return to the Jujutsu graph from its file list."
  (show-buffer! s (hash-table-ref (buffer-state (session-buffer s)) 'origin)))

(define (jj-file-at s)
  (let ((t (target-at (doc s) (point s))))
    (if (and t (eq? (target-kind t) 'vc-jj-file)) t (error "Move to a changed file first"))))

(define-command (vc-jj-file-diff s n)
  "Show the selected file's recorded patch."
  (show-target-diff! s (jj-file-at s)))

(define (with-reviewed-jj-file s f)
  (let* ((v (target-value (jj-file-at s))) (repo (car v)) (commit (cadr v)))
    (unless (list-ref v 4) (error "Open the whole change to rewrite a rename or copy"))
    (request! (make-request-slot)
              (lambda () (cons (jj-operation-id repo) (jj-write-rev repo commit)))
              (lambda (plan) (f repo commit (caddr v) (cadddr v) (car plan) (cdr plan)))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-jj-squash-file s n)
  "Move a selected file's edits into a different Jujutsu change."
  (let ((origin (hash-table-ref (buffer-state (session-buffer s)) 'origin)))
    (with-reviewed-jj-file s
      (lambda (repo commit path files before rev)
        (jj-pick-squash! s repo commit before rev path files)))))

(define-command (vc-jj-split-file s n)
  "Split the selected file out into its own Jujutsu change."
  (with-reviewed-jj-file s
    (lambda (repo commit path files before rev)
      (completing-read s (string-append "Description of split for " (label path) ": ") '() #:require-match #f
        #:accept (lambda (s c)
                   (let ((message (string-trim (candidate-text c))))
                     (when (string=? message "") (error "Enter a description for the new change"))
                     (jj-write! s repo before (append (list "split" "-r" rev "-m" message "--") files) #:check-files #t)))))))

(define-command (vc-jj-restore-file s n)
  "Discard the selected file's edits, restoring it from the parents."
  (with-reviewed-jj-file s
    (lambda (repo commit path files before rev)
      (completing-read s (string-append "Discard edits to " (label path) " in " (substring commit 0 8) "? ") '("no" "yes")
        #:accept (lambda (s c)
                   (when (string=? (candidate-text c) "yes")
                     (jj-write! s repo before (append (list "restore" "--changes-in" rev "--") files) #:check-files #t)))))))


(define-mode vc-operations-mode
  "Browse recorded Jujutsu operations."
  #:parent 'special-mode
  #:directory (lambda (b) (normal-directory (cadr (hash-table-ref (buffer-state b) 'repo))))
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("RET" vc-jj-operation-diff) ("=" vc-jj-operation-diff) ("g" vc-jj-operations)
           ("U" vc-jj-revert-operation) ("R" vc-jj-restore-operation) ("u" vc-jj-undo)
           ("?" vc-jj-operation-help) ("n" vc-next-item) ("p" vc-previous-item) ("q" vc-jj-operations-return))
  #:normal '(("RET" vc-jj-operation-diff) ("=" vc-jj-operation-diff) ("g r" vc-jj-operations)
             ("U" vc-jj-revert-operation) ("?" vc-jj-operation-help) ("n" vc-next-item) ("p" vc-previous-item) ("q" vc-jj-operations-return)))

(define-command (vc-jj-operations-return s n)
  "Return to the Jujutsu graph from the operation log."
  (show-buffer! s (status-buffer (current-jj-repo s))))

(define (operation-at-point s)
  (let ((t (target-at (doc s) (point s))))
    (if (and t (eq? (target-kind t) 'vc-jj-operation)) t (error "Move to a Jujutsu operation first"))))

(define-command (vc-jj-operations s n)
  "Show the repository's recent Jujutsu operations."
  (let ((repo (current-jj-repo s)))
    (request! (make-request-slot)
              (lambda ()
                (jj-read repo '("op" "log" "--no-graph" "-n" "40" "-T" "id ++ \"\\t\" ++ description.first_line() ++ \"\\n\"")))
              (lambda (out)
                (guard (e (#t (message! s (error-text e))))
                  (let* ((name (string-append "*vc-operations " (cadr repo) "*"))
                         (b (or (buffer-named name) (make-generated-buffer! name (make-presentation) 'vc-operations-mode #:state (make-hash-table))))
                         (st (buffer-state b)))
                    (hash-table-set! st 'repo repo)
                    (hash-table-set! st 'rows
                      (present! (buffer-document b)
                        (cons (row (list (list "Jujutsu operations   ·   RET diff   ·   U revert   ·   g refresh" 'heading)) #:key 'head)
                              (filter-map (lambda (line)
                                            (let* ((fields (string-split line "\t")) (id (car fields)))
                                              (and (>= (string-length id) 12) (pair? (cdr fields))
                                                   (row (list (list (substring id 0 12) 'keyword) "  " (label (cadr fields)))
                                                        #:key id #:target (target 'vc-jj-operation (list repo id))))))
                                          (nonempty-lines out)))))
                    (show-buffer! s b))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-jj-operation-diff s n)
  "Show the selected Jujutsu operation's diff."
  (show-target-diff! s (operation-at-point s)))

(define-command (vc-jj-revert-operation s n)
  "Revert the selected Jujutsu operation after confirming its ID."
  (let* ((v (target-value (operation-at-point s))) (repo (car v)) (id (cadr v)))
    (request! (make-request-slot)
              (lambda () (jj-operation-id repo))
              (lambda (before)
                (completing-read s (string-append "Revert local repo changes from " (substring id 0 12) "? ") '("yes" "no")
                  #:accept (lambda (s c)
                             (when (string=? (candidate-text c) "yes")
                               (jj-write! s repo before (list "op" "revert" "--what" "repo" id)
                                          #:after (lambda ()
                                                    (let ((b (session-buffer s)))
                                                      (when (and b (eq? (buffer-mode b) 'vc-operations-mode))
                                                        (vc-jj-operations s 1)))))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-jj-restore-operation s n)
  "Restore the repository to how the operation at point left it, if sure."
  (let* ((v (target-value (operation-at-point s))) (repo (car v)) (id (cadr v)))
    (request! (make-request-slot)
              (lambda () (jj-operation-id repo))
              (lambda (before)
                (vc-y-or-n s (string-append "Restore the repository to operation " (substring id 0 12) "?")
                  (lambda ()
                    (jj-write! s repo before (list "op" "restore" id)
                               #:after (lambda ()
                                         (let ((b (session-buffer s)))
                                           (when (and b (eq? (buffer-mode b) 'vc-operations-mode))
                                             (vc-jj-operations s 1))))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-jj-operation-help s n)
  "Show the menu for inspecting, reverting and restoring operations."
  (vc-choose-command s "Jujutsu operations"
    '(("RET  Show the operation's changes" . vc-jj-operation-diff) ("U  Revert the operation" . vc-jj-revert-operation)
      ("R  Restore the repository to it" . vc-jj-restore-operation) ("u  Undo the last operation" . vc-jj-undo)
      ("g  Refresh" . vc-jj-operations) ("q  Back to the workspace" . vc-jj-operations-return))))

(define (current-git-repo s)
  (let* ((b (session-buffer s)) (st (and b (buffer-state b)))
         (repo (and (hash-table? st) (hash-table-ref/default st 'repo #f))))
    (unless (and repo (eq? (car repo) 'git)) (error "Open Git status or history first"))
    repo))


(define-mode vc-history-mode
  "Browse recorded Git commits and their patches."
  #:parent 'special-mode
  #:directory (lambda (b) (normal-directory (cadr (hash-table-ref (buffer-state b) 'repo))))
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("RET" vc-history-diff) ("=" vc-history-diff) ("g" vc-git-history) ("n" vc-next-item)
           ("p" vc-previous-item) ("q" vc-history-return) ("?" vc-history-help))
  #:normal '(("RET" vc-history-diff) ("=" vc-history-diff) ("g r" vc-git-history) ("n" vc-next-item)
             ("p" vc-previous-item) ("q" vc-history-return) ("?" vc-history-help)))

(define (optional-git-ref repo query)
  (guard (e ((not (cancelled-condition? e)) #f))
    (let ((ref (chomp (run-in repo query)))) (and (not (string=? ref "")) ref))))

(define (git-history-log repo revision)
  (nonempty-lines (run-in repo (list "log" "-40" "--format=%H%x09%h%x09%D%x09%s" revision "--"))))

(define (git-history-rows repo section lines)
  (cons (row (list (list (string-append section (if (= (length lines) 40) " (up to 40 shown)" "")) 'heading)) #:key (list 'history-section section))
        (if (null? lines) (list (row (list (list "  None" 'shadow)) #:key (list section 'empty)))
            (map (lambda (line)
                   (let* ((fields (string-split line "\t")) (sha (car fields))
                          (short (cadr fields)) (refs (caddr fields)) (subject (string-join (drop fields 3) "\t")))
                     (row (list (list short 'keyword) "  " (label subject)
                                (if (string=? refs "") "" (list (string-append "  " (label refs)) 'shadow)))
                          #:key (list section sha) #:target (target 'vc-git-commit (list repo sha))))) lines))))

(define-command (vc-git-history s n)
  "Inspect incoming, outgoing and recent Git commits."
  (let ((repo (current-git-repo s)))
    (request! (make-request-slot)
              (lambda ()
                (let* ((branch (car (git-summary repo)))
                       (head (optional-git-ref repo '("rev-parse" "--verify" "HEAD")))
                       (upstream (optional-git-ref repo '("rev-parse" "--verify" "@{upstream}^{commit}"))))
                  (cons branch (append (if (and head upstream)
                                          (append (git-history-rows repo "Incoming from upstream" (git-history-log repo (string-append head ".." upstream)))
                                                  (git-history-rows repo "Outgoing to upstream" (git-history-log repo (string-append upstream ".." head))))
                                          (list (row (list (list "No recorded upstream comparison" 'shadow)) #:key 'no-upstream)))
                                      (git-history-rows repo "Recent commits" (if head (git-history-log repo head) '()))))))
              (lambda (data)
                (let* ((name (string-append "*vc-history " (cadr repo) "*"))
                       (b (or (buffer-named name) (make-generated-buffer! name (make-presentation) 'vc-history-mode #:state (make-hash-table))))
                       (st (buffer-state b)))
                  (hash-table-set! st 'repo repo)
                  (hash-table-set! st 'rows
                    (present! (buffer-document b)
                              (append (list (row (list (list "Git history" 'heading)) #:key 'head)
                                            (row (list (list (string-append "Branch: " (label (car data))) 'shadow)) #:key 'branch)
                                            (row "Recorded refs · RET patch · q return" #:key 'keys))
                                      (cdr data))))
                  (show-buffer! s b)))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-history-diff s n)
  "Show the exact Git commit at point."
  (let ((t (target-at (doc s) (point s))))
    (if (and t (eq? (target-kind t) 'vc-git-commit)) (show-target-diff! s t)
        (error "Move to a commit first"))))

(define-command (vc-history-return s n)
  "Return from the history to this repository's status."
  (let ((b (status-buffer (current-git-repo s))))
    (show-buffer! s b)
    (refresh! b)))

(define-command (vc-history-help s n)
  "Show the keys for browsing Git history."
  (message! s "n/p move · RET or = inspect exact commit · g refresh · q quit"))

;; Git must never wait for an editor here: messages and plans are given.
(define %git-env '("GIT_EDITOR=true" "GIT_SEQUENCE_EDITOR=true" "GIT_TERMINAL_PROMPT=0"))

(define (git-command! s repo args title #:destination [destination '()] #:after [after #f] #:check [check #f]
                      #:input [input #f] #:env [env %git-env])
  (message! s (string-append title "…"))
  (request! (make-request-slot)
            (lambda ()
              (when check (check))
              (run-write! repo args title #:destination destination #:input input #:env env))
            (lambda (_)
              (message! s (string-append title " finished"))
              (refresh-repository! repo)
              (when after (after)))
            #:fail (lambda (e) (message! s (error-text e)) (refresh-repository! repo))))

(define-command (vc-git-integrate s n)
  "Confirm a fast-forward-only merge of the recorded upstream tip."
  (let ((repo (current-git-repo s)))
    (request! (make-request-slot)
              (lambda () (git-integration-plan repo))
              (lambda (plan)
                (let ((tip (caddr plan)))
                  (completing-read s (git-integration-prompt "Fast-forward" " to upstream " plan) '("no" "yes")
                    #:accept (lambda (s c)
                               (when (string=? (candidate-text c) "yes")
                                 (git-command! s repo (list "merge" "--ff-only" "--" tip) "Fast-forward"
                                               #:check (lambda () (check-git-integration repo plan))))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define (git-integration-basis repo)
  (cons (chomp (run-in repo '("symbolic-ref" "--quiet" "HEAD")))
        (chomp (run-in repo '("rev-parse" "--verify" "HEAD")))))

(define (git-integration-plan repo)
  (let* ((basis (git-integration-basis repo))
         (upstream (chomp (run-in repo '("rev-parse" "--symbolic-full-name" "@{upstream}"))))
         (tip (chomp (run-in repo (list "rev-parse" "--verify" (string-append upstream "^{commit}"))))))
    (list basis upstream tip)))

(define (git-integration-prompt verb direction plan)
  (let* ((branch (caar plan)) (upstream (cadr plan))
         (short (lambda (ref)
                  (cond ((string-prefix? "refs/heads/" ref) (substring ref 11))
                        ((string-prefix? "refs/remotes/" ref) (substring ref 13))
                        (else ref)))))
    (string-append verb " " (label (short branch)) direction (label (short upstream))
                   " @ " (substring (caddr plan) 0 8) "? ")))

(define (check-git-integration repo plan)
  ;; The upstream may move; keep using the reviewed commit, not the new tip.
  (unless (equal? (car plan) (git-integration-basis repo))
    (error "Branch or HEAD changed; review the integration again")))

(define-command (vc-git-rebase s n)
  "Confirm rebasing the current Git branch onto its recorded upstream."
  (let ((repo (current-git-repo s)))
    (request! (make-request-slot)
              (lambda () (git-integration-plan repo))
              (lambda (plan)
                (completing-read s (git-integration-prompt "Rebase" " onto upstream " plan) '("no" "yes")
                  #:accept (lambda (s c)
                             (when (string=? (candidate-text c) "yes")
                               (git-command! s repo (list "rebase" "--no-autostash" "--no-update-refs" "--no-rebase-merges" "--" (caddr plan))
                                             "Rebase" #:check (lambda () (check-git-integration repo plan)))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-git-rebase-continue s n)
  "Continue an in-progress Git rebase using the existing message."
  (git-command! s (current-git-repo s) '("-c" "core.editor=true" "rebase" "--continue") "Continue rebase"))

(define-command (vc-git-rebase-abort s n)
  "Confirm discarding resolutions and aborting an in-progress Git rebase."
  (let ((repo (current-git-repo s)))
    (completing-read s "Abort Git rebase and discard resolutions? " '("no" "yes")
      #:accept (lambda (s c)
                 (when (string=? (candidate-text c) "yes")
                   (git-command! s repo '("rebase" "--abort") "Abort rebase"))))))

(define-command (vc-git-abort-merge s n)
  "Ask Git to abort its in-progress merge after confirmation."
  (let ((repo (current-git-repo s)))
    (completing-read s "Discard merge resolution and abort Git merge? " '("no" "yes")
      #:accept (lambda (s c)
                 (when (string=? (candidate-text c) "yes")
                   (git-command! s repo '("merge" "--abort") "Abort merge"))))))



(define-mode vc-conflicts-mode
  "Review alternatives for each text conflict in a visiting Git file."
  #:parent 'special-mode
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("RET" vc-conflict-visit) ("n" vc-next-item) ("p" vc-previous-item) ("g" vc-conflict-refresh)
           ("1" vc-conflict-use-top) ("2" vc-conflict-use-bottom) ("3" vc-conflict-use-both) ("?" vc-conflict-help) ("q" vc-conflict-return))
  #:normal '(("RET" vc-conflict-visit) ("n" vc-next-item) ("p" vc-previous-item) ("g r" vc-conflict-refresh)
             ("1" vc-conflict-use-top) ("2" vc-conflict-use-bottom) ("3" vc-conflict-use-both) ("?" vc-conflict-help) ("q" vc-conflict-return)))

(define-command (vc-conflict-help s n)
  "Choose an action in the Git conflict guide."
  (vc-choose-command s "Git conflicts: "
    '(("n  Next block" . vc-next-item) ("p  Previous block" . vc-previous-item)
      ("RET  Edit this block manually" . vc-conflict-visit)
      ("1  Keep top" . vc-conflict-use-top) ("2  Keep bottom" . vc-conflict-use-bottom)
      ("3  Keep top then bottom" . vc-conflict-use-both)
      ("g  Refresh from buffer" . vc-conflict-refresh) ("q  Back to file" . vc-conflict-return))))

(define (conflict-preview heading text face)
  (let ((lines (string-split (chomp text) "\n")))
    (cons (row (list (list heading 'shadow)))
          (append (if (string=? text "") (list (row (list (list "  (empty)" 'shadow))))
                      (map (lambda (line) (row (list "  " (list line face)))) (take lines (min 10 (length lines)))))
                  (if (> (length lines) 10) (list (row "  … RET to see the full block")) '())))))

(define (render-conflicts! b)
  (let* ((st (buffer-state b)) (file (hash-table-ref st 'file)) (d (buffer-document file))
         (blocks (git-conflict-blocks d)))
    (hash-table-set! st 'revision (document-revision d))
    (hash-table-set! st 'rows
      (present! (buffer-document b)
        (append (list (row (list (list (string-append "Resolve text conflicts · " (label (hash-table-ref st 'path))) 'heading)) #:key 'heading)
                      (row (list (list "n/p block · 1 top · 2 bottom · 3 both in order · RET edit manually · g refresh" 'shadow)) #:key 'keys)
                      (row (list (list "Edits stay in the file buffer. q return, save, then m s in status to stage." 'shadow)) #:key 'hint))
                (if (null? blocks) (list (row "No complete text conflict blocks remain" #:key 'empty))
                    (append-map (lambda (block)
                                  (let ((top (list-ref block 5)) (base (list-ref block 6)) (bottom (list-ref block 7)))
                                    (append (list (row (list (list (string-append "Conflict at line " (number->string (caddr block))) 'warning))
                                                       #:key (list 'conflict (car block)) #:target (target 'vc-conflict block)))
                                            (conflict-preview (string-append "1  Top · " (cadddr block)) top 'diff.added)
                                            (if base (conflict-preview "   Common ancestor" base 'default) '())
                                            (conflict-preview (string-append "2  Bottom · " (list-ref block 4)) bottom 'diff.removed)))) blocks)))))))

(define (open-git-conflicts! s file path)
  (let ((d (buffer-document file)))
    (unless (document-dirty? d) (reload-document! d)))
  (let* ((name (string-append "*Resolve " (document-path (buffer-document file)) "*"))
         (b (or (buffer-named name)
                (make-generated-buffer! name (make-presentation) 'vc-conflicts-mode #:state (make-hash-table))))
         (st (buffer-state b)))
    (hash-table-set! st 'file file)
    (hash-table-set! st 'path path)
    (render-conflicts! b)
    (pop-to-buffer! s b)))

(define (git-conflict-target? t)
  (and t (eq? (target-kind t) 'vc-file) (eq? (cadddr (target-value t)) 'conflict)))

(define-command (vc-git-conflicts s n)
  "Inspect and resolve each Git conflict block, without saving or staging."
  (let ((t (target-at (doc s) (point s))))
    (if (git-conflict-target? t)
        (let* ((v (target-value t)) (d (file-document (file-path v))))
          (open-git-conflicts! s (or (document-buffer d) (add-buffer! d)) (cadr v)))
        (with-vc-file s
          (lambda (repo path)
            (unless (and (eq? (car repo) 'git)
                         (any (lambda (t) (and (git-conflict-target? t) (string=? path (cadr (target-value t))))) (changes repo)))
              (error "Visit a conflicted Git file or its status row")))
          (lambda (file repo path _) (open-git-conflicts! s file path))))))

(define-command (vc-conflict-refresh s n)
  "Refresh conflict blocks from the visiting file buffer."
  (render-conflicts! (session-buffer s)))

(define (conflict-file! s file)
  (let ((shown (list-index (lambda (p) (document=? (view-document p) (buffer-document file))) (session-panes s))))
    (if shown (sset! s 'focus shown) (pop-to-buffer! s file))))

(define-command (vc-conflict-return s n)
  "Return to the visiting file buffer."
  (conflict-file! s (hash-table-ref (buffer-state (session-buffer s)) 'file)))

(define (with-conflict-block s action)
  (let* ((b (session-buffer s)) (st (buffer-state b)) (file (hash-table-ref st 'file))
         (t (target-at (doc s) (point s))))
    (unless (and t (eq? (target-kind t) 'vc-conflict)) (error "Move to a conflict heading with n or p"))
    (unless (= (document-revision (buffer-document file)) (hash-table-ref st 'revision))
      (error "File buffer changed; press g to refresh conflict blocks"))
    (action b file (target-value t))))

(define-command (vc-conflict-visit s n)
  "Visit this block in the file for manual editing."
  (with-conflict-block s (lambda (_ file block) (conflict-file! s file) (move! s (lambda (_) (car block))))))

(define (use-conflict-side! s choose)
  (with-conflict-block s
    (lambda (b file block)
      (conflict-file! s file)
      (edit! s (list (list (car block) (cadr block) (choose (list-ref block 5) (list-ref block 7)))) "new")
      (render-conflicts! b)
      (pop-to-buffer! s b)
      (message! s "Resolved in buffer; not saved or staged"))))

(define-command (vc-conflict-use-top s n)
  "Keep the top alternative in this block."
  (use-conflict-side! s (lambda (top bottom) top)))

(define-command (vc-conflict-use-bottom s n)
  "Keep the bottom alternative in this block."
  (use-conflict-side! s (lambda (top bottom) bottom)))

(define-command (vc-conflict-use-both s n)
  "Keep top then bottom, without the ancestor or conflict markers."
  (use-conflict-side! s string-append))

(define (check-git-resolution! repo path name)
  (when (path-exists? name)
    ;; The empty base makes Git check every saved line, not just lines added
    ;; since the conflict. No-index exit bits: 1 differs, 2 check failed.
    (let ((status #f))
      (run (cadr repo) "git"
           (list "--no-lazy-fetch" "-c" "core.whitespace=-blank-at-eol,-blank-at-eof,-space-before-tab,-indent-with-non-tab,-tab-in-indent"
                 "diff" "--no-index" "--no-ext-diff" "--no-textconv" "--check" "--" "/dev/null" path)
           #:accept-exit '(0 1 2 3) #:exit (lambda (code) (set! status code)))
      (when (memv status '(2 3))
        (error "Saved file failed Git's conflict marker check; inspect it before staging")))))

(define-command (vc-git-resolve s n)
  "Stage the manually resolved contents of the conflicted Git file."
  (let* ((t (target-at (doc s) (point s))) (repo (current-git-repo s)))
    (unless (and t (eq? (target-kind t) 'vc-file) (eq? (cadddr (target-value t)) 'conflict))
      (error "Visit and resolve a conflicted file first"))
    (let* ((v (target-value t)) (path (cadr v)) (name (absolute-path (file-path v))))
      (completing-read s (string-append "Mark " (label path) " resolved with its on-disk contents? ") '("no" "yes")
        #:accept (lambda (s c)
                   (when (string=? (candidate-text c) "yes")
                     (git-command! s repo (list "--literal-pathspecs" "add" "--" path) "Stage resolution"
                       #:check (lambda ()
                                 (unless (any (lambda (item) (and (git-conflict-target? item)
                                                                  (string=? (cadr (target-value item)) path))) (changes repo))
                                   (error "File is no longer conflicted; refresh status"))
                                 (when (any (lambda (b)
                                              (let ((d (buffer-document b)))
                                                (and (equal? (document-path d) name) (document-dirty? d))))
                                            (buffer-list))
                                   (error "Save the resolved file buffer before staging"))
                                  (check-git-resolution! repo path name)))))))))

(define-command (vc-git-branch s n)
  "Switch to an existing local Git branch without forcing it."
  (let ((repo (current-git-repo s)))
    (request! (make-request-slot)
              (lambda () (nonempty-lines (run-in repo '("for-each-ref" "--format=%(refname:lstrip=2)" "refs/heads/"))))
              (lambda (branches)
                (completing-read s "Switch branch: " branches
                  #:accept (lambda (s c)
                             (let ((name (candidate-text c)))
                               (git-command! s repo (list "switch" "--no-guess" "--" name) (string-append "Switch to " (label name)))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-git-create-branch s n)
  "Create and switch to a new Git branch starting at the recorded HEAD."
  (let ((repo (current-git-repo s)))
    (request! (make-request-slot)
              (lambda () (chomp (run-in repo '("rev-parse" "--verify" "HEAD"))))
              (lambda (tip)
                (completing-read s (string-append "New branch at " (substring tip 0 8) ": ") '() #:require-match #f
                  #:accept (lambda (s c)
                             (let ((name (string-trim (candidate-text c))))
                               (when (or (string=? name "") (string-prefix? "-" name)) (error "Enter a branch name"))
                               (git-command! s repo (list "switch" (string-append "--create=" name) "--" tip)
                                             (string-append "Create and switch to " (label name)))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-git-fetch s n)
  "Fetch Git refs from a configured remote after selecting it."
  (let ((repo (current-git-repo s)))
    (request! (make-request-slot)
              (lambda () (nonempty-lines (run-in repo '("remote"))))
              (lambda (remotes)
                (completing-read s "Fetch from: " remotes
                  #:accept (lambda (s c)
                             (let ((name (candidate-text c)))
                               (git-command! s repo (list "fetch" "--" name) (string-append "Fetch from " (label name)))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define (git-upstream repo branch)
  (let ((fields (string-split (chomp (run-in repo (list "for-each-ref"
           "--format=%(upstream:remotename)%09%(upstream:remoteref)" branch))) "\t")))
    (and (= (length fields) 2) (not (member (car fields) '("" ".")))
         (string-prefix? "refs/heads/" (cadr fields)) fields)))

(define (track-published-branch! s repo branch remote)
  (let ((name (substring branch 11 (string-length branch))))
    (request! (make-request-slot)
              (lambda ()
                (if (git-upstream repo branch) #f
                    (begin
                      (run-write! repo (list "branch" (string-append "--set-upstream-to=" remote "/" name) "--" name)
                                  "Set Git upstream after publishing")
                      #t)))
              (lambda (set?)
                (message! s (if set? "Published branch and set upstream" "Published branch; existing upstream kept"))
                (refresh-repository! repo))
              #:fail (lambda (_) (message! s "Push succeeded; could not set upstream. Check the local tracking ref.")))))

(define (confirm-git-push! s repo tip remote ref branch-to-track #:lease [lease #f])
  (request! (make-request-slot)
            (lambda () (list tip remote ref (push-url-id (single-push-url repo remote))))
            (lambda (plan)
              (completing-read s (string-append (cond (lease "Replace published tip with ") (branch-to-track "Publish ") (else "Push "))
                                                (substring tip 0 8) " to " (label remote) ":" (label ref)
                                                (cond (lease (string-append " only if it still points to " (substring lease 0 8) "? "))
                                                      (branch-to-track " and set upstream? ") (else "? "))) '("no" "yes")
                #:accept (lambda (s c)
                           (when (string=? (candidate-text c) "yes")
                             (git-command! s repo (append '("push" "--no-follow-tags")
                                                                         (if lease (list (string-append "--force-with-lease=" ref ":" lease)) '())
                                                                         (list "--" remote (string-append tip ":" ref)))
                                            (if lease "Push with lease" "Push branch") #:destination plan
                                           #:after (and branch-to-track
                                                        (lambda () (track-published-branch! s repo branch-to-track remote))))))))
            #:fail (lambda (e) (message! s (error-text e)))))

(define-command (vc-git-push s n)
  "Confirm a Git push to the upstream or publish the branch to a remote."
  (let ((repo (current-git-repo s)))
    (request! (make-request-slot)
              (lambda ()
                (let* ((branch (optional-git-ref repo '("symbolic-ref" "--quiet" "HEAD")))
                       (tip (and branch (optional-git-ref repo (list "rev-parse" "--verify" branch)))))
                  (unless (and tip (string-prefix? "refs/heads/" branch)) (error "Push requires a local branch with a commit"))
                  (let ((upstream (git-upstream repo branch)))
                    (list branch tip upstream (if upstream '() (nonempty-lines (run-in repo '("remote"))))))))
              (lambda (data)
                (let ((branch (car data)) (tip (cadr data)) (upstream (caddr data)) (remotes (cadddr data)))
                  (cond (upstream (confirm-git-push! s repo tip (car upstream) (cadr upstream) #f))
                        ((null? remotes) (message! s "Add a Git remote before publishing this branch"))
                        (else (completing-read s "Publish branch to remote: " remotes
                                #:accept (lambda (s c) (confirm-git-push! s repo tip (candidate-text c) branch branch)))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-git-push-with-lease s n)
  "Replace the Git upstream only if its tip matches the recorded ref."
  (let ((repo (current-git-repo s)))
    (request! (make-request-slot)
              (lambda ()
                (let* ((branch (optional-git-ref repo '("symbolic-ref" "--quiet" "HEAD")))
                       (tip (and branch (optional-git-ref repo (list "rev-parse" "--verify" branch))))
                       (upstream (and branch (git-upstream repo branch)))
                       (lease (and upstream (optional-git-ref repo '("rev-parse" "--verify" "@{upstream}^{commit}")))))
                  (unless (and tip lease) (error "Fetch and review a configured upstream before pushing with a lease"))
                  (list tip (car upstream) (cadr upstream) lease)))
              (lambda (plan) (confirm-git-push! s repo (car plan) (cadr plan) (caddr plan) #f #:lease (cadddr plan)))
              #:fail (lambda (e) (message! s (error-text e))))))

(define (pick-jj-remote s repo prompt accept)
  (request! (make-request-slot)
            (lambda () (map (lambda (line) (car (string-split line " ")))
                            (nonempty-lines (jj-read repo '("git" "remote" "list")))))
            (lambda (remotes)
              (if (null? remotes) (message! s "No Jujutsu remotes configured")
                  (completing-read s prompt remotes #:accept (lambda (s c) (accept s (candidate-text c))))))
            #:fail (lambda (e) (message! s (error-text e)))))

(define-command (vc-jj-fetch s n)
  "Fetch from a configured Git remote of the Jujutsu workspace."
  (let ((repo (current-jj-repo s)))
    (request! (make-request-slot) (lambda () (jj-operation-id repo))
              (lambda (before)
                (pick-jj-remote s repo "Fetch jj refs from: "
                  (lambda (s remote)
                    (jj-write! s repo before (list "--ignore-working-copy" "git" "fetch" (string-append "--remote=" remote))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define (confirm-jj-publish s repo before commit bookmark remote dry?)
  (request! (make-request-slot)
            (lambda ()
              (list commit remote (string-append "refs/heads/" bookmark) (push-url-id (single-push-url repo remote))))
            (lambda (plan)
              (completing-read s (string-append (if dry? "Preview push of " "Push ") (label bookmark) " @ "
                                                (substring commit 0 8) " to " (label remote) "? ") '("no" "yes")
                #:accept (lambda (s c)
                           (when (string=? (candidate-text c) "yes")
                             (jj-write! s repo before
                                        (append (list "--ignore-working-copy" "git" "push" (string-append "--remote=" remote)
                                                      (string-append "--bookmark=exact:" bookmark))
                                                (if dry? '("--dry-run") '())) #:destination plan)))))
            #:fail (lambda (e) (message! s (error-text e)))))

(define-command (vc-jj-publish s n)
  "Confirm a push of one local bookmark on the selected recorded change."
  (let ((dry? (vc-transient-arg? s "--dry-run")))
    (with-jj-change s
      (lambda (repo commit before rev)
        (request! (make-request-slot)
                  (lambda () (nonempty-lines (jj-read repo (list "bookmark" "list" "-r" rev
                                                               "-T" "if(remote, \"\", name ++ \"\\n\")"))))
                  (lambda (bookmarks)
                    (if (null? bookmarks) (message! s "No local bookmark at point; use B to set one")
                        (completing-read s "Publish bookmark: " bookmarks
                          #:accept (lambda (s c)
                                     (let ((bookmark (candidate-text c)))
                                       (pick-jj-remote s repo "Push to: "
                                         (lambda (s remote) (confirm-jj-publish s repo before commit bookmark remote dry?))))))))
                  #:fail (lambda (e) (message! s (error-text e))))))))

;;; Forge reads are local, even when the checkout is on another machine.
;;; A provider supplies its endpoint and normalizes its own wire format.

(define (forge-segment? s)
  (and (> (string-length s) 0) (not (member s '("." "..")))
       (every (lambda (c) (or (char<=? #\a c #\z) (char<=? #\A c #\Z) (char<=? #\0 c #\9)
                             (memv c '(#\- #\_ #\.)))) (string->list s))))

(define (github-repository remote)
  (let ((prefix (find (lambda (p) (string-prefix? p remote))
                      '("https://github.com/" "git@github.com:" "ssh://git@github.com/"))))
    (and prefix
         (let* ((name (substring remote (string-length prefix) (string-length remote)))
                (plain (if (string-suffix? ".git" name) (substring name 0 (- (string-length name) 4)) name))
                (parts (string-split plain "/")))
           (and (= (length parts) 2) (every forge-segment? parts) (string-downcase plain))))))

(define (github-get path #:query [query #f])
  ;; Fixed host; never use an API response or repository config as an API URL.
  ;; Disable curlrc, permit only HTTPS, and don't follow redirects.
  (string->json (run (or (get-environment-variable "HOME") "/") "curl"
                    (append '("-q" "--proto" "=https" "--silent" "--show-error" "--fail"
                              "--connect-timeout" "5" "--max-time" "15" "--max-filesize" "2097152"
                              "--header" "Accept: application/vnd.github+json")
                            (if query (list "--get" "--data-urlencode" query) '())
                            (list (string-append "https://api.github.com/repos/" path))))))

(define (github-topic obj)
  (let ((head (dict-ref obj "head" (dict))) (base (dict-ref obj "base" (dict)))
        (user (dict-ref obj "user" #f)))
    (list (dict-ref obj "number") (dict-ref obj "title" "") (dict-ref obj "state" "")
          (dict-ref obj "draft" #f) (dict-ref head "sha" "") (dict-ref base "sha" "")
          (if (dict? user) (forge-field user "login" "unknown author") "unknown author")
          (forge-field base "ref" "unknown base") (forge-field head "label" "unknown source"))))

(define (github-open-pulls name page source)
  (let* ((head-label (and source (string-append (car (string-split (car source) "/")) ":" (cdr source))))
         (objects (vector->list (github-get (string-append name "/pulls?state=open&per_page=30&page=" (number->string page))
                                            #:query (and head-label (string-append "head=" head-label))))))
    (map github-topic
         (if (not source) objects
             (filter (lambda (obj)
                       (let* ((head (dict-ref obj "head" (dict))) (repo (dict-ref head "repo" #f))
                              (repo-name (and (dict? repo) (dict-ref repo "full_name" #f))))
                         (and (string? repo-name) (string-ci=? repo-name (car source))
                              (equal? (dict-ref head "ref" #f) (cdr source))))) objects)))))

(define (github-published-files name base head)
  ;; A compare of object IDs cannot silently follow a force-pushed PR.
  (let ((comparison (github-get (string-append name "/compare/" base "..." head "?per_page=1"))))
    (map (lambda (file) (cons (dict-ref file "filename" "") (dict-ref file "patch" #f)))
         (vector->list (dict-ref comparison "files" #())))))

(define (github-discussion name number page)
  (let* ((suffix (string-append (number->string number) "/comments?per_page=30&page=" (number->string page)))
         (task (spawn (lambda () (vector->list (github-get (string-append name "/issues/" suffix))))))
         (line (vector->list (github-get (string-append name "/pulls/" suffix))))
         (general (task-join task)))
    (cons (or (= 30 (length general)) (= 30 (length line))) (append general line))))

(define (github-checks name head page)
  (let ((response (github-get (string-append name "/commits/" head "/check-runs?per_page=30&page=" (number->string page)))))
    (cons (dict-ref response "total_count" 0) (vector->list (dict-ref response "check_runs" #())))))

(define-record-type vc-forge-provider
  (make-forge-provider title parse list-open published-files discussion checks)
  forge-provider?
  (title forge-provider-title)
  (parse forge-provider-parse)
  (list-open forge-provider-list-open)
  (published-files forge-provider-published-files)
  (discussion forge-provider-discussion)
  (checks forge-provider-checks))

(define %github (make-forge-provider "GitHub" github-repository github-open-pulls github-published-files github-discussion github-checks))

(define (forge-remote repo remote)
  (let ((parse (forge-provider-parse %github)))
    (or (and remote (or (parse (string-append "https://github.com/" remote))
                      (error "Expected a GitHub owner/repo override")))
        (parse
         (chomp (if (eq? (car repo) 'git) (run-in repo '("remote" "get-url" "origin"))
                    (let ((origin (find (lambda (line) (string-prefix? "origin " line))
                                        (nonempty-lines (jj-read repo '("git" "remote" "list"))))))
                      (if origin (substring origin 7 (string-length origin)) "")))))
      (error "No supported GitHub origin; set vc-forge-repository to owner/repo"))))

(define-option vc-forge-repository #f
  "Override the GitHub repository for this buffer, as owner/repo."
  #:type '(or (one-of #f) string))

(define %forge-slot (make-request-slot))

(define-mode vc-forge-mode
  "Browse read-only forge topics and their published revision IDs."
  #:parent 'special-mode
  #:directory (lambda (b) (normal-directory (cadr (hash-table-ref (buffer-state b) 'repo))))
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("RET" vc-forge-inspect) ("j" vc-forge-jump) ("d" vc-forge-discussion) ("C" vc-forge-checks) ("g" vc-forge) ("?" vc-forge-help)
           ("n" vc-next-item) ("p" vc-previous-item) ("]" vc-forge-next-page) ("[" vc-forge-previous-page) ("q" vc-forge-return))
  #:normal '(("RET" vc-forge-inspect) ("j" vc-forge-jump) ("d" vc-forge-discussion) ("C" vc-forge-checks) ("g r" vc-forge) ("?" vc-forge-help)
             ("n" vc-next-item) ("p" vc-previous-item) ("]" vc-forge-next-page) ("[" vc-forge-previous-page) ("q" vc-forge-return)))

(define (forge-context s)
  (let ((b (session-buffer s)))
    (unless (or (status-buffer? b) (and b (memq (buffer-mode b) '(vc-forge-mode vc-log-mode vc-refs-mode))))
      (error "Open version control status first"))
    (buffer-state b)))

(define (forge-buffer repo name source)
  (let ((buffer-name (string-append "*forge " name (if source (string-append " ← " (car source) ":" (cdr source)) "") " · " (cadr repo) "*")))
    (or (buffer-named buffer-name)
        (let ((st (make-hash-table)))
          (hash-table-set! st 'repo repo)
          (hash-table-set! st 'forge name)
          (hash-table-set! st 'source source)
          (make-generated-buffer! buffer-name (make-presentation) 'vc-forge-mode #:state st)))))

(define (render-forge! b topics failure)
  (let* ((st (buffer-state b)) (name (hash-table-ref st 'forge)))
    (hash-table-set! st 'topics topics)
    (hash-table-set! st 'rows
      (present! (buffer-document b)
        (append (list (row (list (list (string-append name " · " (forge-provider-title %github) " pull requests") 'heading)) #:key 'head)
                      (row (list (list (string-append "Read only · open PRs · page " (number->string (hash-table-ref/default st 'page 1))
                                                      " · [/] page · RET files · d discussion · C checks") 'shadow)) #:key 'keys))
                (if failure (list (row (list (list (string-append "Stale: " (label failure)) 'warning)) #:key 'stale)) '())
                (let ((source (hash-table-ref/default st 'source #f)))
                  (if source (list (row (list "Published source: " (list (label (string-append (car source) ":" (cdr source))) 'link)) #:key 'source)) '()))
                (if (and (null? topics) (not failure)) (list (row "No matching open pull requests" #:key 'empty)) '())
                (map (lambda (p)
                       (let ((number (car p)))
                         (row (list (list (string-append "#" (number->string number)) 'keyword) "  " (label (cadr p))
                                    (if (list-ref p 3) (list "  · draft" 'shadow) ""))
                              #:key number #:target (target 'vc-forge-topic (list (hash-table-ref st 'repo) name p))))) topics))))))

(define (read-forge-page! s page #:ref [ref #f])
  (let* ((st (forge-context s)) (repo (hash-table-ref st 'repo)) (name (hash-table-ref/default st 'forge #f))
         (origin (session-buffer s)) (config (option origin 'vc-forge-repository))
         (source (hash-table-ref/default st 'source #f)))
    (message! s "Reading GitHub pull requests…")
    (request! %forge-slot
              (lambda ()
                (let* ((name (or name (forge-remote repo (or config #f))))
                       (source (if ref (cons (forge-remote repo #f) ref) source)))
                  (list name source ((forge-provider-list-open %github) name page source))))
              (lambda (result)
                (guard (e (#t (message! s (error-text e))))
                  (let* ((b (forge-buffer repo (car result) (cadr result))) (contents (buffer-state b)) (topics (caddr result)))
                    (if (and (> page 1) (null? topics)) (message! s "No further open pull requests")
                        (begin
                          (hash-table-set! contents 'page page)
                          (render-forge! b topics #f)
                          (when (eq? origin (session-buffer s)) (show-buffer! s b))
                          (message! s #f))))))
              #:fail (lambda (e)
                       (when name
                         (let ((b (forge-buffer repo name source)))
                           (render-forge! b (hash-table-ref/default (buffer-state b) 'topics '()) (error-text e))))
                       (message! s (error-text e))))))

(define-command (vc-forge s n)
  "List or refresh open pull requests from the GitHub origin."
  (read-forge-page! s (hash-table-ref/default (forge-context s) 'page 1)))

(define-command (vc-forge-local-topic s n)
  "Find public PRs from this branch, or the selected bookmark, at origin."
  (let* ((origin (session-buffer s)) (repo (hash-table-ref (forge-context s) 'repo))
         (t (target-at (doc s) (point s)))
         (rev (if (and t (eq? (target-kind t) 'vc-change)) (cadr (target-value t)) "@")))
    (unless (status-buffer? origin) (error "Open status to select a branch or bookmark"))
    (request! (make-request-slot)
              (lambda ()
                (if (eq? (car repo) 'git)
                    (list (chomp (run-in repo '("symbolic-ref" "--quiet" "--short" "HEAD"))))
                    (nonempty-lines (jj-read repo (list "bookmark" "list" "-r" rev
                                                       "-T" "if(remote, \"\", name ++ \"\\n\")")))))
              (lambda (refs)
                (when (eq? origin (session-buffer s))
                  (cond ((null? refs) (message! s "No local bookmark on this recorded change"))
                        ((null? (cdr refs)) (read-forge-page! s 1 #:ref (car refs)))
                        (else (completing-read s "Find PRs from bookmark at origin: " refs
                                #:accept (lambda (s c) (read-forge-page! s 1 #:ref (candidate-text c))))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-forge-next-page s n)
  "Read the next page of open pull requests."
  (read-forge-page! s (+ 1 (hash-table-ref/default (forge-context s) 'page 1))))

(define-command (vc-forge-previous-page s n)
  "Read the preceding page of open pull requests."
  (let ((page (hash-table-ref/default (forge-context s) 'page 1)))
    (if (> page 1) (read-forge-page! s (- page 1)) (message! s "Already on the first page"))))

(define-command (vc-forge-return s n)
  "Return from the pull-request list to the repository status."
  (show-buffer! s (status-buffer (hash-table-ref (forge-context s) 'repo))))

(define (forge-at-point s)
  (let ((t (target-at (doc s) (point s))))
    (if (and t (eq? (target-kind t) 'vc-forge-topic)) (target-value t)
        (error "Move to a pull request first"))))

(define (review-sha? s)
  (and (string? s) (= (string-length s) 40)
       (every (lambda (c) (or (char<=? #\0 c #\9) (char<=? #\a c #\f))) (string->list s))))

(define-mode vc-review-mode
  "Inspect a read-only comparison pinned to published Git objects."
  #:parent 'special-mode
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("n" vc-next-item) ("p" vc-previous-item) ("j" vc-forge-jump) ("d" vc-forge-discussion)
           ("C" vc-forge-checks) ("[" vc-review-previous-page) ("]" vc-review-next-page) ("q" vc-review-return) ("?" vc-review-help))
  #:normal '(("n" vc-next-item) ("p" vc-previous-item) ("j" vc-forge-jump) ("d" vc-forge-discussion)
             ("C" vc-forge-checks) ("[" vc-review-previous-page) ("]" vc-review-next-page) ("q" vc-review-return) ("?" vc-review-help)))

(define-command (vc-review-return s n)
  "Return to the buffer that opened this pull request view."
  (let* ((origin (hash-table-ref (buffer-state (session-buffer s)) 'origin))
         (document (buffer-document origin)) (here (session-view s)) (popped (sget s 'popped)))
    (when (and popped (document=? (view-document popped) (view-document here)) (> (length (session-panes s)) 1))
      (run-command s 'quit-window 1))
    (let ((shown (list-index (lambda (pane) (document=? document (view-document pane))) (session-panes s))))
      (if shown (sset! s 'focus shown) (show-buffer! s origin)))))

(define-command (vc-review-help s n)
  "Show the ways to navigate a pinned comparison."
  (message! s "n/p item · j reviewed commit · d discussion · C checks · q back"))

(define (show-review-rows! s origin topic view rows #:pagination [pagination #f])
  (let* ((name (cadr topic)) (p (caddr topic)) (head (list-ref p 4)) (base (list-ref p 5))
         (name (string-append "*PR " name " #" (number->string (car p)) " " view " @ " (substring head 0 8)
                              " · " (hex-encode (sha-256 (call-with-output-string
                                                         (lambda (p) (write (list (car topic) base head) p))))) "*"))
         (b (or (buffer-named name) (make-generated-buffer! name (make-presentation) 'vc-review-mode #:state (make-hash-table))))
         (st (buffer-state b)))
    ;; Keep the opening path when revisiting a subview, so jumping back
    ;; and forth between Discussion and Checks doesn't make a return loop.
    (unless (hash-table-ref/default st 'origin #f) (hash-table-set! st 'origin origin))
    (hash-table-set! st 'topic topic)
    (hash-table-set! st 'pagination pagination)
    (hash-table-set! st 'rows
      (present! (buffer-document b)
        (append (list (row (list (list (label (cadr p)) 'heading)) #:key 'head)
                      (row (list (list (string-append "#" (number->string (car p)) " · " (list-ref p 6) " · "
                                                      (list-ref p 8) " → " (list-ref p 7)) 'shadow)) #:key 'topic)
                      (row (list (list (string-append "Published " base " → " head) 'shadow)) #:key 'pin)
                      (row (list (list (string-append "Read only · " view " · n/p item · d discussion · C checks · q back") 'shadow)) #:key 'keys))
                rows)))
    (when (eq? origin (session-buffer s)) (pop-to-buffer! s b))))

(define (show-published-files! s origin topic files)
  (show-review-rows! s origin topic "Files"
    (cons (row (string-append (number->string (length files)) " files returned · GitHub limit 300") #:key 'limit)
          (apply append
            (map (lambda (f i)
                   (let ((patch (cdr f)))
                     (cons (row (list (list (label (car f)) 'link)) #:key (list 'file (car f))
                                #:target (target 'vc-review-file (car f)))
                           (if (string? patch) (patch-display (nonempty-lines patch) #:key i)
                               (list (row "Patch unavailable" #:key (list 'unavailable i)))))))
                 files (iota (length files)))))))

(define (current-review-topic s)
  (let ((b (session-buffer s)))
    (if (and b (eq? (buffer-mode b) 'vc-review-mode))
        (hash-table-ref (buffer-state b) 'topic) (forge-at-point s))))

(define (forge-field obj key fallback)
  (let ((value (dict-ref obj key #f))) (if (string? value) (label value) fallback)))

(define (discussion-rows messages head page)
  (cons (row (string-append "Current topic discussion · page " (number->string page) " · [/] page") #:key 'limit)
        (if (null? messages) (list (row "No comments returned" #:key 'empty))
            (apply append
              (map (lambda (obj i)
                     (let* ((user (dict-ref obj "user" #f)) (path (forge-field obj "path" #f))
                            (commit (forge-field obj "commit_id" #f))
                            (lineno (or (dict-ref obj "line" #f) (dict-ref obj "original_line" #f)))
                            (author (if (dict? user) (forge-field user "login" "Unknown") "Unknown"))
                            (content (dict-ref obj "body" ""))
                            (lines (if (string? content) (string-split (substring content 0 (min (string-length content) 2000)) "\n") '())))
                       (cons (row (list (list author 'keyword)
                                        (if path (string-append " · " path (if (integer? lineno) (string-append ":" (number->string lineno)) ""))
                                            " · general comment")
                                        (if path (list (cond ((not commit) " · revision unavailable")
                                                             ((string=? commit head) " · on displayed head")
                                                             (else (string-append " · other revision " commit)))
                                                       'shadow) ""))
                                  #:key (list 'comment i) #:target (target 'vc-review-entry i))
                             (append (map (lambda (line j) (row (string-append "  " (label line)) #:key (list 'comment i j)))
                                          lines (iota (length lines)))
                                     (if (and (string? content) (> (string-length content) 2000))
                                         (list (row "  Comment truncated" #:key (list 'truncated i))) '())))))
                   messages (iota (length messages)))))))

(define (read-discussion-page! s page)
  (let* ((origin (session-buffer s)) (topic (current-review-topic s)) (name (cadr topic)) (p (caddr topic)))
    (request! %forge-slot
              (lambda () ((forge-provider-discussion %github) name (car p) page))
              (lambda (result)
                (guard (e (#t (message! s (error-text e))))
                  (if (and (> page 1) (null? (cdr result)))
                      (begin (hash-table-set! (buffer-state origin) 'pagination (list 'discussion (- page 1) #f))
                             (message! s "No further comments"))
                      (show-review-rows! s origin topic "Discussion" (discussion-rows (cdr result) (list-ref p 4) page)
                                         #:pagination (list 'discussion page (car result))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-forge-discussion s n)
  "Read general and line comments for the selected GitHub pull request."
  (read-discussion-page! s 1))

(define (read-checks-page! s page)
  (let* ((origin (session-buffer s)) (topic (current-review-topic s)) (name (cadr topic)) (head (list-ref (caddr topic) 4)))
    (unless (review-sha? head) (error "Pull request has no reviewed commit ID"))
    (request! %forge-slot
              (lambda () ((forge-provider-checks %github) name head page))
              (lambda (result)
                (guard (e (#t (message! s (error-text e))))
                  (let* ((checks (cdr result)) (offset (* (- page 1) 30)) (total (car result)))
                    (if (and (> page 1) (null? checks)) (message! s "No further checks")
                        (show-review-rows! s origin topic "Checks"
                          (cons (row (string-append "Check runs for displayed head · page " (number->string page) " · "
                                                    (number->string (length checks)) " on this page of " (number->string total) " runs · [/] page") #:key 'limit)
                                (map (lambda (obj i)
                                       (row (list (list (forge-field obj "name" "Unnamed check") 'keyword) "  "
                                                  (forge-field obj "status" "unknown")
                                                  (let ((done (forge-field obj "conclusion" #f))) (if done (string-append " · " done) "")))
                                            #:key (list 'check (+ offset i)) #:target (target 'vc-review-entry i))) checks (iota (length checks))))
                          #:pagination (list 'checks page (> total (+ offset (length checks)))))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-forge-inspect s n)
  "Inspect the published files for the pull request at point."
  (let* ((origin (session-buffer s)) (v (forge-at-point s)) (name (cadr v)) (p (caddr v)) (head (list-ref p 4)) (base (list-ref p 5)))
    (unless (and (review-sha? head) (review-sha? base)) (error "Pull request has no revision pin"))
    (request! %forge-slot
              (lambda () ((forge-provider-published-files %github) name base head))
              (lambda (files)
                (guard (e (#t (message! s (error-text e))))
                  (show-published-files! s origin v files)))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-forge-checks s n)
  "Read check runs for the exact displayed pull request head commit."
  (read-checks-page! s 1))

(define (step-review-page! s delta)
  (let* ((b (session-buffer s))
         (state (and (eq? (buffer-mode b) 'vc-review-mode) (hash-table-ref/default (buffer-state b) 'pagination #f))))
    (if (not state) (message! s "Open Discussion or Checks to browse pages")
        (let ((page (+ (cadr state) delta)) (discussion? (eq? (car state) 'discussion)))
          (cond ((and (> delta 0) (not (caddr state))) (message! s (if discussion? "No further comments" "No further check runs")))
                ((< page 1) (message! s "Already on the first page"))
                (discussion? (read-discussion-page! s page))
                (else (read-checks-page! s page)))))))

(define-command (vc-review-next-page s n)
  "Read the next page of comments or check runs."
  (step-review-page! s 1))

(define-command (vc-review-previous-page s n)
  "Read the preceding page of comments or check runs."
  (step-review-page! s -1))

(define-command (vc-forge-jump s n)
  "Inspect the exact reviewed commit if it is in this checkout."
  (let* ((v (current-review-topic s)) (repo (car v)) (sha (list-ref (caddr v) 4)))
    (unless (review-sha? sha) (error "Pull request has no reviewed commit ID"))
    (show-target-diff! s (target 'vc-reviewed (list repo sha)))))

(define-command (vc-forge-help s n)
  "Show the keys for browsing GitHub pull requests."
  (message! s "n/p topic · [/] page · RET files · d discussion · C checks · j reviewed commit · q status"))

(define-mode vc-evidence-mode
  "Review the observed state after a missing write reply."
  #:parent 'special-mode
  #:keys '(("a" vc-acknowledge-write) ("g" vc-remote-outcome) ("j" vc-evidence-jump) ("q" vc-evidence-return))
  #:normal '(("a" vc-acknowledge-write) ("g r" vc-remote-outcome) ("j" vc-evidence-jump) ("q" vc-evidence-return)))

(define (show-write-evidence! s status attempt seen pending?)
  (let* ((repo (hash-table-ref (buffer-state status) 'repo))
         (name (string-append "*VC evidence " (cadr repo) "*"))
         (b (or (buffer-named name)
                (make-generated-buffer! name (make-presentation) 'vc-evidence-mode #:state (make-hash-table))))
         (st (buffer-state b))
         (summary (vc-evidence-summary (cadr attempt) seen)))
    (hash-table-set! st 'status status)
    (hash-table-set! st 'attempt (and pending? attempt))
    (hash-table-set! st 'seen seen)
    (present! (buffer-document b)
        (append
          (list (row (list (list (if pending? "Write outcome unknown · writes blocked" "Acknowledged · outcome still unknown") 'warning)))
                (row (list (list (string-append (cond ((not pending?) "q status")
                                                     (seen "g refresh · a acknowledge · q status")
                                                     (else "g retry observation · q status"))
                                               (if (vc-evidence-observed-id seen) " · j inspect observed commit" "")) 'shadow)))
                (row (list (list "No write was replayed. These observations do not prove the outcome." 'shadow)))
                (row "")
                (row (list (list "Attempt: " 'heading) (label (car attempt)))))
          (if (null? summary) '() (list (row (list (list (string-join summary " · ") 'keyword)))))
          (append-map (lambda (group)
                        (cons (row (list (list (car group) 'heading)))
                              (map (lambda (line) (row (string-append "  " (label line))))
                                   (string-split (cdr group) "; "))))
                      (list (cons "Before dispatch" (cadr attempt))
                            (cons "Observed after reconnect" (or seen "Not available; writes remain blocked."))))))
    (pop-to-buffer! s b)))

(define-command (vc-evidence-jump s n)
  "Inspect the exact observed commit if it is already in this repository."
  (let* ((st (buffer-state (session-buffer s)))
         (repo (hash-table-ref (buffer-state (hash-table-ref st 'status)) 'repo))
         (id (vc-evidence-observed-id (hash-table-ref st 'seen))))
    (unless id (error "No observed commit ID is available"))
    (show-target-diff! s (target 'vc-reviewed (list repo id)))))

(define-command (vc-evidence-return s n)
  "Return to the status for this write."
  (let* ((status (hash-table-ref (buffer-state (session-buffer s)) 'status))
         (shown (list-index (lambda (p) (document=? (view-document p) (buffer-document status))) (session-panes s))))
    (if shown (sset! s 'focus shown) (show-buffer! s status))))

(define-command (vc-acknowledge-write s n)
  "Acknowledge an unknown outcome and permit new writes, replaying nothing."
  (let* ((st (buffer-state (session-buffer s)))
         (status (hash-table-ref st 'status)) (attempt (hash-table-ref st 'attempt)) (seen (hash-table-ref st 'seen))
         (repo (hash-table-ref (buffer-state status) 'repo)) (recovery (vc-recovery-state repo)))
    (unless (and attempt seen (eq? attempt (hash-table-ref/default recovery 'unresolved #f)))
      (error "There is no current observation to acknowledge; refresh first"))
    (completing-read s "Acknowledge uncertainty and permit new writes? " '("no" "yes")
      #:accept (lambda (s c)
                 (when (and (string=? (candidate-text c) "yes") (eq? attempt (hash-table-ref/default recovery 'unresolved #f)))
                   (vc-recovery-acknowledge! repo attempt seen)
                   (render-status! status)
                   (show-write-evidence! s status attempt seen #f)
                   (message! s "Outcome still unknown; no write replayed"))))))

(define-command (vc-remote-outcome s n)
  "Inspect an unresolved remote write without replaying it."
  (let* ((b (session-buffer s))
         (st (and b (case (if (status-buffer? b) 'vc-status-mode (buffer-mode b))
                      ((vc-status-mode) (buffer-state b))
                      ((vc-evidence-mode) (buffer-state (hash-table-ref (buffer-state b) 'status)))
                      ((vc-commit-mode) (buffer-state (caddr (buffer-state b))))
                      ((vc-operations-mode vc-forge-mode)
                       (buffer-state (status-buffer (hash-table-ref (buffer-state b) 'repo))))
                      (else #f))))
         (recovery (and st (vc-recovery-state (hash-table-ref st 'repo))))
         (attempt (and recovery (hash-table-ref/default recovery 'unresolved #f))))
    (unless st (error "Open version control status first"))
    (when (hash-table-ref/default recovery 'write-running #f) (error "Remote write awaiting reply"))
    (let ((repo (hash-table-ref st 'repo)) (status (status-buffer (hash-table-ref st 'repo))))
      (if (not attempt)
          (let ((last (hash-table-ref/default recovery 'last-unresolved #f)))
            (unless last (error "No unresolved write in this repository's status buffer"))
            (show-write-evidence! s status last (caddr last) #f))
          (begin
            (message! s "Observing repository without replaying the write…")
            (request! (make-request-slot)
                      (lambda () (write-observation repo (drop attempt 3)))
                      (lambda (seen)
                        (when (and (eq? b (session-buffer s)) (eq? attempt (hash-table-ref/default recovery 'unresolved #f)))
                          (show-write-evidence! s status attempt seen #t)
                          (message! s #f)))
                      #:fail (lambda (e)
                               (when (and (eq? b (session-buffer s)) (eq? attempt (hash-table-ref/default recovery 'unresolved #f)))
                                 (show-write-evidence! s status attempt #f #t)
                                 (message! s (error-text e))))))))))

;; A menu of COMMANDS, ("KEY  Description" . command), titled PROMPT.
(define (vc-choose-command s prompt commands)
  (vc-transient! s
    (list (let ((title (string-trim prompt)))
            (if (string-suffix? ":" title) (substring title 0 (- (string-length title) 1)) title))
          (cons "Commands"
                (map (lambda (c)
                       (let* ((text (car c)) (i (or (string-contains text "  ") (string-length text))))
                         (list 'suffix (substring text 0 i) (string-trim (substring text i (string-length text))) (cdr c))))
                     commands)))))

;;; Commands of the menus. A command acts on what point is on: the commit,
;;; branch or stash at point; else it asks.

(define (status-repo s)
  (let* ((b (session-buffer s)) (st (and b (buffer-state b))))
    (or (and (hash-table? st) (hash-table-ref/default st 'repo #f))
        (error "Open version control status first"))))

(define (git-repo? s) (eq? (car (status-repo s)) 'git))

;; The commit at point in a status, log or refs view, or #f.
(define (commit-at s)
  (let ((t (target-at (doc s) (point s))))
    (and t (case (target-kind t)
             ((vc-git-commit) (cadr (target-value t)))
             ((vc-git-ref) (cadr (target-value t)))
             (else #f)))))

;; Call (F commit) with the commit at point, or one read from the recent
;; history.
(define (with-commit s prompt f #:range [range "HEAD"])
  (let ((at (commit-at s)))
    (if at (f at)
        (let ((repo (current-git-repo s)))
          (request! (make-request-slot) (lambda () (git-commits repo range 200))
                    (lambda (commits)
                      (completing-read s prompt
                        (map (lambda (c) (candidate (string-append (cadr c) "  " (label (cadddr c)))
                                                    #:target (target 'vc-git-commit (list repo (car c)))))
                             commits)
                        #:require-match #f
                        #:accept (lambda (s c)
                                   (let ((t (candidate-target c)))
                                     (f (if t (cadr (target-value t)) (string-trim (candidate-text c))))))))
                    #:fail (lambda (e) (message! s (error-text e))))))))

(define (git-refs repo patterns)
  (nonempty-lines (run-in repo (append '("for-each-ref" "--format=%(refname:short)") patterns))))

;; Read a branch or revision; the branch at point is offered first.
(define (read-ref s prompt f #:patterns [patterns '("refs/heads/" "refs/remotes/")] #:require-match [require-match #f])
  (let ((repo (current-git-repo s)) (t (target-at (doc s) (point s))))
    (request! (make-request-slot) (lambda () (git-refs repo patterns))
              (lambda (refs)
                (let* ((at (and t (eq? (target-kind t) 'vc-git-ref) (caddr (target-value t))))
                       (refs (if (and at (member at refs)) (cons at (remove (lambda (r) (string=? r at)) refs)) refs)))
                  (completing-read s prompt refs #:require-match require-match #:history 'vc-git-ref
                                   #:accept (lambda (s c) (f (string-trim (candidate-text c)))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define (read-name s prompt f #:initial [initial ""])
  (completing-read s prompt '() #:require-match #f #:initial initial
                   #:accept (lambda (s c)
                              (let ((name (string-trim (candidate-text c))))
                                (when (or (string=? name "") (string-prefix? "-" name)) (error "Enter a name"))
                                (f name)))))

;; Keyword arguments go after the positional ones: these take them
;; positionally, before the procedure, as the calls read best.
(define (read-name-initial s prompt initial f) (read-name s prompt f #:initial initial))
(define (read-ref-from s prompt patterns require-match f)
  (read-ref s prompt f #:patterns patterns #:require-match require-match))

(define (short-sha sha) (substring sha 0 (min 8 (string-length sha))))

(define (capitalize word)
  (if (string=? word "") word
      (string-append (string (char-upcase (string-ref word 0))) (substring word 1 (string-length word)))))



;;; Commit

(define-command (vc-git-extend s n)
  "Amend HEAD with the staged changes, keeping its message."
  (let ((repo (current-git-repo s)))
    (git-command! s repo (append '("commit" "--amend" "--no-edit") (filter (lambda (a) (member a '("--no-verify" "--all"))) (vc-transient-args s)))
                  "Extend HEAD")))

(define-command (vc-git-fixup s n)
  "Commit the staged changes as a fixup of the commit at point.
It is squashed into it later by \\[vc-git-autosquash], or at once by
\\[vc-git-instant-fixup]."
  (let ((args (vc-transient-args s)))
    (with-commit s "Fixup commit: "
      (lambda (sha)
        (git-command! s (current-git-repo s)
                      (append (list "commit" (string-append "--fixup=" sha)) (filter (lambda (a) (member a '("--no-verify" "--all"))) args))
                      (string-append "Fixup " (short-sha sha)))))))

(define-command (vc-git-squash-into s n)
  "Commit the staged changes as a squash! commit for the commit at point."
  (let ((args (vc-transient-args s)))
    (with-commit s "Squash into commit: "
      (lambda (sha)
        (git-command! s (current-git-repo s)
                      (append (list "-c" "core.editor=true" "commit" (string-append "--squash=" sha))
                              (filter (lambda (a) (member a '("--no-verify" "--all"))) args))
                      (string-append "Squash into " (short-sha sha)))))))

(define-command (vc-git-instant-fixup s n)
  "Fold the staged changes into the commit at point now.
That is a fixup commit, then an autosquash rebase from it."
  (let ((args (vc-transient-args s)))
    (with-commit s "Fold staged changes into: "
      (lambda (sha)
        (let ((repo (current-git-repo s)))
          (git-command! s repo (append (list "commit" (string-append "--fixup=" sha))
                                       (filter (lambda (a) (member a '("--all" "--no-verify"))) args))
                        (string-append "Fixup " (short-sha sha))
                        #:after (lambda () (git-autosquash! s repo sha))))))))

(define (git-autosquash! s repo sha)
  (request! (make-request-slot) (lambda () (git-rebase-base repo sha))
            (lambda (base)
              (git-command! s repo (append '("-c" "sequence.editor=true" "rebase" "-i" "--autosquash" "--autostash")
                                           (if base (list base) '("--root")))
                            "Autosquash"))
            #:fail (lambda (e) (message! s (error-text e)))))

;; The base a rebase from SHA (itself included) starts at: its parent, or
;; #f for a root commit.
(define (git-rebase-base repo sha)
  (optional-git-ref repo (list "rev-parse" "--verify" "--quiet" (string-append sha "^"))))

(define-command (vc-commit-menu s n)
  "Show the commit menu: commit, amend, extend, reword, fixup."
  (vc-transient! s
    '("Commit"
      ("Arguments" (switch "-a" "Stage all modified and deleted" "--all")
                   (switch "-e" "Allow empty commit" "--allow-empty")
                   (switch "-n" "Disable hooks" "--no-verify")
                   (switch "-s" "Add Signed-off-by line" "--signoff"))
      ("Create" (suffix "c" "Commit" vc-commit))
      ("Edit HEAD" (suffix "e" "Extend" vc-git-extend) (suffix "a" "Amend" vc-git-amend) (suffix "w" "Reword" vc-git-reword))
      ("Edit commit at point" (suffix "f" "Fixup" vc-git-fixup) (suffix "s" "Squash" vc-git-squash-into)
                              (suffix "F" "Instant fixup" vc-git-instant-fixup)))))

;;; Branch

(define-command (vc-git-checkout s n)
  "Check out a branch or revision read in the minibuffer.
A remote branch makes a local one tracking it."
  (let ((repo (current-git-repo s)))
    (read-ref s "Checkout: "
      (lambda (ref)
        (request! (make-request-slot)
                  (lambda ()
                    (let ((local (optional-git-ref repo (list "rev-parse" "--verify" "--quiet" (string-append "refs/heads/" ref))))
                          (remote (optional-git-ref repo (list "rev-parse" "--verify" "--quiet" (string-append "refs/remotes/" ref)))))
                      (cond (local (list "switch" "--no-guess" "--" ref))
                            ((and remote (string-contains ref "/"))
                             (let ((name (substring ref (+ 1 (string-index ref #\/)) (string-length ref))))
                               (if (optional-git-ref repo (list "rev-parse" "--verify" "--quiet" (string-append "refs/heads/" name)))
                                   (list "switch" "--no-guess" "--" name)
                                   (list "switch" "--track" "-c" name (string-append "refs/remotes/" ref)))))
                            (else (list "switch" "--detach" ref)))))
                  (lambda (args) (git-command! s repo args (string-append "Checkout " (label ref))))
                  #:fail (lambda (e) (message! s (error-text e))))))))

;; Where a new branch starts: the commit at point, else HEAD; with a
;; prefix argument, a revision read.
(define (branch-start s f)
  (let ((at (commit-at s)))
    (cond (at (f at))
          ((current-prefix s) (read-ref s "Start the branch at: " f))
          (else (f "HEAD")))))

(define-command (vc-git-branch-create-checkout s n)
  "Create a branch at the commit at point, or one read, and check it out."
  (branch-start s
    (lambda (start)
      (read-name s (string-append "New branch at " (label (short-sha start)) ": ")
        (lambda (name)
          (git-command! s (current-git-repo s) (list "switch" "-c" name start) (string-append "Create and switch to " (label name))))))))

(define-command (vc-git-branch-create s n)
  "Create a branch at the commit at point, without checking it out."
  (branch-start s
    (lambda (start)
      (read-name s (string-append "New branch at " (label (short-sha start)) ": ")
        (lambda (name)
          (git-command! s (current-git-repo s) (list "branch" "--" name start) (string-append "Create " (label name))))))))

(define-command (vc-git-branch-rename s n)
  "Rename a local branch."
  (read-ref-from s "Rename branch: " '("refs/heads/") #t
    (lambda (old)
      (read-name-initial s (string-append "Rename " (label old) " to: ") old
        (lambda (new)
          (git-command! s (current-git-repo s) (list "branch" "-m" "--" old new) (string-append "Rename " (label old))))))))

(define-command (vc-git-branch-delete s n)
  "Delete a local branch after asking; it must be merged unless forced."
  (let ((force (vc-transient-arg? s "--force")))
    (read-ref-from s "Delete branch: " '("refs/heads/") #t
      (lambda (name)
        (vc-y-or-n s (string-append (if force "Force-delete" "Delete") " branch " (label name) "?")
          (lambda ()
            (git-command! s (current-git-repo s) (list "branch" (if force "-D" "-d") "--" name) (string-append "Delete " (label name)))))))))

(define-command (vc-git-branch-upstream s n)
  "Set the current branch's upstream."
  (read-ref-from s "Upstream: " '("refs/remotes/" "refs/heads/") #t
    (lambda (upstream)
      (git-command! s (current-git-repo s) (list "branch" (string-append "--set-upstream-to=" upstream))
                    (string-append "Track " (label upstream))))))

(define-command (vc-branch-menu s n)
  "Show the branch menu: checkout, create, rename, delete, upstream."
  (vc-transient! s
    '("Branch"
      ("Arguments" (switch "-f" "Force deletion of unmerged branches" "--force"))
      ("Checkout" (suffix "b" "Branch or revision" vc-git-checkout)
                  (suffix "c" "New branch" vc-git-branch-create-checkout))
      ("Create" (suffix "n" "New branch" vc-git-branch-create))
      ("Do" (suffix "m" "Rename" vc-git-branch-rename) (suffix "x" "Delete" vc-git-branch-delete)
            (suffix "u" "Set upstream" vc-git-branch-upstream)))))

;;; Fetch, pull, push

(define (fetch-args s) (filter (lambda (a) (member a '("--prune" "--tags"))) (vc-transient-args s)))

(define-command (vc-git-fetch-upstream s n)
  "Fetch from the current branch's upstream remote, or from origin."
  (let ((repo (current-git-repo s)) (args (fetch-args s)))
    (request! (make-request-slot)
              (lambda () (or (optional-git-ref repo '("config" "--get" "branch.@{0}.remote"))
                             (let* ((branch (optional-git-ref repo '("symbolic-ref" "--quiet" "--short" "HEAD")))
                                    (remote (and branch (optional-git-ref repo (list "config" "--get" (string-append "branch." branch ".remote"))))))
                               (or remote (let ((remotes (nonempty-lines (run-in repo '("remote")))))
                                            (cond ((member "origin" remotes) "origin")
                                                  ((pair? remotes) (car remotes))
                                                  (else (error "No Git remote configured"))))))))
              (lambda (remote)
                (git-command! s repo (append '("fetch") args (list "--" remote)) (string-append "Fetch from " (label remote))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-git-fetch-all s n)
  "Fetch from every remote."
  (git-command! s (current-git-repo s) (append '("fetch" "--all") (fetch-args s)) "Fetch from all remotes"))

(define-command (vc-git-fetch-elsewhere s n)
  "Fetch from a remote read in the minibuffer."
  (let ((repo (current-git-repo s)) (args (fetch-args s)))
    (request! (make-request-slot)
              (lambda () (nonempty-lines (run-in repo '("remote"))))
              (lambda (remotes)
                (completing-read s "Fetch from: " remotes
                  #:accept (lambda (s c)
                             (let ((name (candidate-text c)))
                               (git-command! s repo (append '("fetch") args (list "--" name)) (string-append "Fetch from " (label name)))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-fetch-menu s n)
  "Show the fetch menu."
  (if (git-repo? s)
      (vc-transient! s
        '("Fetch"
          ("Arguments" (switch "-p" "Prune deleted branches" "--prune") (switch "-t" "Fetch all tags" "--tags"))
          ("Fetch from" (suffix "f" "Upstream remote" vc-git-fetch-upstream) (suffix "u" "Upstream remote" vc-git-fetch-upstream)
                        (suffix "a" "All remotes" vc-git-fetch-all) (suffix "e" "Elsewhere" vc-git-fetch-elsewhere))))
      (vc-jj-fetch-menu s n)))

(define-command (vc-git-pull s n)
  "Pull from the upstream: fetch, then merge or rebase as the menu says."
  (let ((repo (current-git-repo s))
        (args (filter (lambda (a) (member a '("--rebase" "--ff-only" "--autostash" "--no-rebase"))) (vc-transient-args s))))
    (git-command! s repo (append '("pull" "--no-edit") args) "Pull")))

(define-command (vc-pull-menu s n)
  "Show the pull menu."
  (vc-transient! s
    '("Pull"
      ("Arguments" (choice "-r" "Integrate by" ("--rebase" "--no-rebase" "--ff-only"))
                   (switch "-A" "Autostash" "--autostash"))
      ("Pull into the current branch" (suffix "F" "From upstream" vc-git-pull) (suffix "u" "From upstream" vc-git-pull))
      ("Without fetching" (suffix "f" "Fast-forward to fetched upstream" vc-git-integrate)))))

(define-command (vc-git-push-elsewhere s n)
  "Push the current branch to a branch of a remote, both read."
  (let ((repo (current-git-repo s)))
    (request! (make-request-slot)
              (lambda ()
                (let* ((branch (optional-git-ref repo '("symbolic-ref" "--quiet" "HEAD")))
                       (tip (and branch (optional-git-ref repo (list "rev-parse" "--verify" branch)))))
                  (unless (and tip (string-prefix? "refs/heads/" branch)) (error "Push requires a local branch with a commit"))
                  (list branch tip (nonempty-lines (run-in repo '("remote"))))))
              (lambda (data)
                (completing-read s "Push to remote: " (caddr data)
                  #:accept (lambda (s c)
                             (let ((remote (candidate-text c)) (name (substring (car data) 11 (string-length (car data)))))
                               (read-name-initial s (string-append "Push " name " to branch of " (label remote) ": ") name
                                 (lambda (target-branch)
                                   (confirm-git-push! s repo (cadr data) remote (string-append "refs/heads/" target-branch) #f)))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-git-push-tag s n)
  "Push one tag to a remote, after asking."
  (let ((repo (current-git-repo s)))
    (read-ref-from s "Push tag: " '("refs/tags/") #t
      (lambda (tag)
        (request! (make-request-slot) (lambda () (nonempty-lines (run-in repo '("remote"))))
                  (lambda (remotes)
                    (completing-read s (string-append "Push " (label tag) " to: ") remotes
                      #:accept (lambda (s c)
                                 (let ((remote (candidate-text c)))
                                   (completing-read s (string-append "Push tag " (label tag) " to " (label remote) "? ") '("no" "yes")
                                     #:accept (lambda (s c)
                                                (when (string=? (candidate-text c) "yes")
                                                  (git-command! s repo (list "push" "--" remote (string-append "refs/tags/" tag))
                                                                (string-append "Push " (label tag))))))))))
                  #:fail (lambda (e) (message! s (error-text e))))))))

(define-command (vc-push-menu s n)
  "Show the push menu."
  (if (git-repo? s)
      (vc-transient! s
        '("Push"
          ("Push the current branch" (suffix "p" "To its upstream, or publish it" vc-git-push)
                                     (suffix "u" "To its upstream, or publish it" vc-git-push)
                                     (suffix "e" "Elsewhere" vc-git-push-elsewhere)
                                     (suffix "f" "Replace upstream, with a lease" vc-git-push-with-lease))
          ("Push" (suffix "t" "A tag" vc-git-push-tag))))
      (vc-jj-push-menu s n)))

;;; Merge, rebase, cherry-pick, revert, reset

(define (sequence-kind s)
  (let* ((b (session-buffer s)) (st (and (status-buffer? b) (buffer-state b)))
         (op (and st (hash-table-ref/default st 'operation '()))))
    (and (pair? op) (car op))))

(define-command (vc-git-merge s n)
  "Merge a branch or the commit at point into the current branch."
  (let ((args (filter (lambda (a) (member a '("--no-ff" "--ff-only" "--squash" "--no-commit"))) (vc-transient-args s)))
        (at (commit-at s)))
    (let ((go (lambda (ref) (git-command! s (current-git-repo s) (append '("merge" "--no-edit") args (list ref))
                                          (string-append "Merge " (label ref))))))
      (if at (go at) (read-ref s "Merge: " go)))))

(define-command (vc-git-continue s n)
  "Continue the rebase, cherry-pick or revert, or commit the merge."
  (case (sequence-kind s)
    ((rebase) (vc-git-rebase-continue s n))
    ((cherry-pick) (git-command! s (current-git-repo s) '("-c" "core.editor=true" "cherry-pick" "--continue") "Continue cherry-pick"))
    ((revert) (git-command! s (current-git-repo s) '("-c" "core.editor=true" "revert" "--continue") "Continue revert"))
    ((merge) (vc-commit s n))
    (else (message! s "Nothing in progress"))))

(define-command (vc-git-skip s n)
  "Skip the commit a rebase, cherry-pick or revert stopped at."
  (let ((kind (sequence-kind s)))
    (if (memq kind '(rebase cherry-pick revert))
        (git-command! s (current-git-repo s) (list (symbol->string kind) "--skip") (string-append "Skip"))
        (message! s "Nothing to skip"))))

(define-command (vc-git-abort s n)
  "Abort the merge, rebase, cherry-pick or revert in progress, if sure."
  (let ((kind (sequence-kind s)))
    (if kind
        (vc-y-or-n s (string-append "Abort the " (symbol->string kind) " and lose its resolutions?")
          (lambda () (git-command! s (current-git-repo s) (list (symbol->string kind) "--abort")
                                   (string-append "Abort " (symbol->string kind)))))
        (message! s "Nothing in progress"))))

(define-command (vc-merge-menu s n)
  "Show the merge menu."
  (vc-transient! s
    `("Merge"
      ("Arguments" (switch "-n" "No fast-forward" "--no-ff") (switch "-f" "Fast-forward only" "--ff-only")
                   (switch "-s" "Squash" "--squash") (switch "-c" "Don't commit" "--no-commit"))
      ("Merge" (suffix "m" "Merge" vc-git-merge))
      ("In progress" (suffix "c" "Commit the merge" vc-commit ,(lambda (s) (eq? (sequence-kind s) 'merge)))
                     (suffix "a" "Abort" vc-git-abort ,(lambda (s) (eq? (sequence-kind s) 'merge)))))))

(define-command (vc-git-rebase-elsewhere s n)
  "Rebase the current branch onto a branch or the commit at point."
  (let ((args (filter (lambda (a) (member a '("--autostash" "--autosquash" "--rebase-merges"))) (vc-transient-args s)))
        (at (commit-at s)))
    (let ((go (lambda (onto) (git-command! s (current-git-repo s)
                                           (append '("-c" "sequence.editor=true" "rebase") (if (member "--autosquash" args) '("-i") '())
                                                   args (list onto))
                                           (string-append "Rebase onto " (label onto))))))
      (if at (go at) (read-ref s "Rebase onto: " go)))))

(define-command (vc-git-autosquash s n)
  "Squash fixup! and squash! commits into their targets.
The rebase starts at the commit at point."
  (with-commit s "Autosquash from commit: " (lambda (sha) (git-autosquash! s (current-git-repo s) sha))))

(define (todo-action! s action)
  (with-commit s (string-append (capitalize action) " commit: ")
    (lambda (sha)
      (let ((repo (current-git-repo s)))
        (request! (make-request-slot) (lambda () (cons (git-full repo sha) (git-rebase-plan repo sha)))
                  (lambda (found)
                    (let* ((full (car found)) (plan (cdr found))
                           (entries (map (lambda (e) (if (string=? (cadr e) full) (cons action (cdr e)) e))
                                         (cadr plan))))
                      (if (string=? action "reword")
                          (let ((e (find (lambda (e) (string=? (car e) "reword")) entries)))
                            (read-name-initial s "New message: " (cadddr e)
                              (lambda (message)
                                (run-rebase-todo! s repo (car plan)
                                                  (map (lambda (x) (if (eq? x e) (append (take x 4) (list message)) x)) entries)))))
                          (run-rebase-todo! s repo (car plan) entries))))
                  #:fail (lambda (e) (message! s (error-text e))))))))

(define (git-full repo sha) (or (optional-git-ref repo (list "rev-parse" "--verify" (string-append sha "^{commit}"))) sha))

(define-command (vc-git-rebase-edit-commit s n)
  "Stop a rebase at the commit at point, to change it."
  (todo-action! s "edit"))

(define-command (vc-git-rebase-reword-commit s n)
  "Change the message of the commit at point, rebasing what follows it."
  (todo-action! s "reword"))

(define-command (vc-git-rebase-remove-commit s n)
  "Remove the commit at point from history, after asking."
  (vc-y-or-n s "Remove the commit at point and rebase what follows it?" (lambda () (todo-action! s "drop"))))

(define-command (vc-rebase-menu s n)
  "Show the rebase menu."
  (if (git-repo? s)
      (vc-transient! s
        `("Rebase"
          ("Arguments" (switch "-A" "Autostash" "--autostash") (switch "-a" "Autosquash" "--autosquash")
                       (switch "-r" "Rebase merges" "--rebase-merges"))
          ("Rebase the current branch" (suffix "u" "Onto its upstream" vc-git-rebase)
                                       (suffix "e" "Onto elsewhere" vc-git-rebase-elsewhere))
          ("Rewrite from commit at point" (suffix "i" "Interactively" vc-git-rebase-interactive)
                                          (suffix "f" "Autosquash fixups" vc-git-autosquash)
                                          (suffix "m" "Edit a commit" vc-git-rebase-edit-commit)
                                          (suffix "w" "Reword a commit" vc-git-rebase-reword-commit)
                                          (suffix "k" "Remove a commit" vc-git-rebase-remove-commit))
          ("In progress" (suffix "r" "Continue" vc-git-continue ,(lambda (s) (eq? (sequence-kind s) 'rebase)))
                         (suffix "s" "Skip" vc-git-skip ,(lambda (s) (eq? (sequence-kind s) 'rebase)))
                         (suffix "a" "Abort" vc-git-abort ,(lambda (s) (eq? (sequence-kind s) 'rebase))))))
      (vc-jj-rebase-menu s n)))

(define (pick-command! s verb args)
  (with-commit s (string-append (capitalize verb) " commit: ")
    (lambda (sha)
      (git-command! s (current-git-repo s) (append (list "-c" "core.editor=true" verb) args (list sha))
                    (string-append (capitalize verb) " " (short-sha sha))))))

(define-command (vc-git-cherry-pick s n)
  "Apply the commit at point to the current branch as a new commit."
  (pick-command! s "cherry-pick" (filter (lambda (a) (member a '("-x" "--ff"))) (vc-transient-args s))))

(define-command (vc-git-cherry-apply s n)
  "Apply the changes of the commit at point without committing them."
  (pick-command! s "cherry-pick" '("--no-commit")))

(define-command (vc-cherry-pick-menu s n)
  "Show the cherry-pick menu."
  (vc-transient! s
    `("Cherry-pick"
      ("Arguments" (switch "-x" "Reference the original commit" "-x") (switch "-F" "Fast-forward when possible" "--ff"))
      ("Apply commit at point" (suffix "A" "Pick" vc-git-cherry-pick) (suffix "a" "Apply, without committing" vc-git-cherry-apply))
      ("In progress" (suffix "r" "Continue" vc-git-continue ,(lambda (s) (eq? (sequence-kind s) 'cherry-pick)))
                     (suffix "s" "Skip" vc-git-skip ,(lambda (s) (eq? (sequence-kind s) 'cherry-pick)))
                     (suffix "x" "Abort" vc-git-abort ,(lambda (s) (eq? (sequence-kind s) 'cherry-pick)))))))

(define-command (vc-git-revert-commit s n)
  "Revert the commit at point with a new commit."
  (pick-command! s "revert" '("--no-edit")))

(define-command (vc-git-revert-no-commit s n)
  "Revert the changes of the commit at point without committing."
  (pick-command! s "revert" '("--no-commit")))

(define-command (vc-revert-menu s n)
  "Show the revert menu."
  (vc-transient! s
    `("Revert"
      ("Revert commit at point" (suffix "V" "Revert" vc-git-revert-commit) (suffix "v" "Revert, without committing" vc-git-revert-no-commit))
      ("In progress" (suffix "r" "Continue" vc-git-continue ,(lambda (s) (eq? (sequence-kind s) 'revert)))
                     (suffix "s" "Skip" vc-git-skip ,(lambda (s) (eq? (sequence-kind s) 'revert)))
                     (suffix "a" "Abort" vc-git-abort ,(lambda (s) (eq? (sequence-kind s) 'revert)))))))

(define (reset! s mode)
  (with-commit s (string-append "Reset (" mode ") to: ")
    (lambda (sha)
      (let ((go (lambda () (git-command! s (current-git-repo s) (list "reset" (string-append "--" mode) sha "--")
                                         (string-append "Reset " mode " to " (short-sha sha))))))
        (if (string=? mode "hard")
            (vc-y-or-n s (string-append "Discard every uncommitted change and reset to " (short-sha sha) "?") go)
            (go))))))

(define-command (vc-git-reset-mixed s n) "Reset HEAD and the index to the commit at point." (reset! s "mixed"))
(define-command (vc-git-reset-soft s n) "Reset HEAD to the commit at point, keeping the index." (reset! s "soft"))
(define-command (vc-git-reset-hard s n) "Reset HEAD, the index and the working tree, after asking." (reset! s "hard"))
(define-command (vc-git-reset-keep s n) "Reset HEAD and the index, keeping uncommitted changes." (reset! s "keep"))

(define-command (vc-reset-menu s n)
  "Show the reset menu."
  (vc-transient! s
    '("Reset to the commit at point"
      ("Reset" (suffix "m" "Mixed: HEAD and index" vc-git-reset-mixed) (suffix "s" "Soft: HEAD only" vc-git-reset-soft)
               (suffix "h" "Hard: HEAD, index and files" vc-git-reset-hard) (suffix "k" "Keep: HEAD and index, keep edits" vc-git-reset-keep)))))

;;; Stash

(define (stash-at s)
  (let ((t (target-at (doc s) (point s))))
    (and t (eq? (target-kind t) 'vc-stash) (cadr (target-value t)))))

(define (with-stash s prompt f)
  (let ((at (stash-at s)) (repo (current-git-repo s)))
    (if at (f at)
        (request! (make-request-slot) (lambda () (nonempty-lines (run-in repo '("stash" "list" "--format=%gd%x09%s"))))
                  (lambda (stashes)
                    (if (null? stashes) (message! s "No stashes")
                        (completing-read s prompt stashes
                          #:accept (lambda (s c) (f (car (string-split (candidate-text c) "\t")))))))
                  #:fail (lambda (e) (message! s (error-text e)))))))

(define (stash-save! s what)
  (let ((args (filter (lambda (a) (member a '("--include-untracked" "--all"))) (vc-transient-args s))))
    (read-name-initial s "Stash message: " "WIP"
      (lambda (message)
        (git-command! s (current-git-repo s)
                      (append '("stash" "push") (case what ((index) '("--staged")) ((worktree) '("--keep-index")) (else '()))
                              args (list "-m" message))
                      "Stash")))))

(define-command (vc-stash-both s n) "Stash the staged and unstaged changes." (stash-save! s 'both))
(define-command (vc-stash-index s n) "Stash the staged changes only." (stash-save! s 'index))
(define-command (vc-stash-worktree s n) "Stash the unstaged changes, keeping the index." (stash-save! s 'worktree))

(define-command (vc-stash-apply s n)
  "Apply the stash at point, keeping it."
  (with-stash s "Apply stash: " (lambda (name) (git-command! s (current-git-repo s) (list "stash" "apply" "--index" name) (string-append "Apply " name)))))

(define-command (vc-stash-pop s n)
  "Apply the stash at point and drop it."
  (with-stash s "Pop stash: " (lambda (name) (git-command! s (current-git-repo s) (list "stash" "pop" "--index" name) (string-append "Pop " name)))))

(define-command (vc-stash-drop s n)
  "Drop the stash at point, after asking."
  (with-stash s "Drop stash: "
    (lambda (name)
      (vc-y-or-n s (string-append "Drop " name "?")
        (lambda () (git-command! s (current-git-repo s) (list "stash" "drop" name) (string-append "Drop " name)))))))

(define-command (vc-stash-show s n)
  "Show the stash at point's patch."
  (with-stash s "Show stash: " (lambda (name) (show-target-diff! s (target 'vc-stash (list (current-git-repo s) name))))))

(define-command (vc-stash-branch s n)
  "Make a branch from the stash at point and apply it there."
  (with-stash s "Branch from stash: "
    (lambda (name)
      (read-name s "Branch name: "
        (lambda (branch) (git-command! s (current-git-repo s) (list "stash" "branch" branch name) (string-append "Branch " (label branch))))))))

(define-command (vc-stash-menu s n)
  "Show the stash menu."
  (vc-transient! s
    '("Stash"
      ("Arguments" (switch "-u" "Also save untracked files" "--include-untracked") (switch "-a" "Also save untracked and ignored files" "--all"))
      ("Stash" (suffix "z" "Both" vc-stash-both) (suffix "i" "Index" vc-stash-index) (suffix "w" "Worktree" vc-stash-worktree))
      ("Use" (suffix "a" "Apply" vc-stash-apply) (suffix "p" "Pop" vc-stash-pop) (suffix "k" "Drop" vc-stash-drop))
      ("Inspect" (suffix "v" "Show" vc-stash-show) (suffix "b" "Branch" vc-stash-branch)))))

;;; Tags

(define-command (vc-git-tag-create s n)
  "Tag the commit at point, or HEAD."
  (let ((at (or (commit-at s) "HEAD")) (annotate (vc-transient-arg? s "--annotate")) (force (vc-transient-arg? s "--force")))
    (read-name s (string-append "Tag " (short-sha at) " as: ")
      (lambda (name)
        (let ((go (lambda (message)
                    (git-command! s (current-git-repo s)
                                  (append '("tag") (if force '("--force") '()) (if message (list "-a" "-m" message) '()) (list name at))
                                  (string-append "Tag " (label name))))))
          (if annotate (read-name s "Tag message: " go) (go #f)))))))

(define-command (vc-git-tag-delete s n)
  "Delete a tag, after asking."
  (read-ref-from s "Delete tag: " '("refs/tags/") #t
    (lambda (name)
      (vc-y-or-n s (string-append "Delete tag " (label name) "?")
        (lambda () (git-command! s (current-git-repo s) (list "tag" "-d" name) (string-append "Delete " (label name))))))))

(define-command (vc-tag-menu s n)
  "Show the tag menu."
  (vc-transient! s
    '("Tag"
      ("Arguments" (switch "-a" "Annotate" "--annotate") (switch "-f" "Force" "--force"))
      ("Create" (suffix "t" "Tag commit at point" vc-git-tag-create))
      ("Do" (suffix "k" "Delete" vc-git-tag-delete) (suffix "p" "Push a tag" vc-git-push-tag)))))

;;; Run

(define (split-arguments text)
  (filter (lambda (w) (not (string=? w ""))) (string-split (string-trim text) " ")))

(define-command (vc-run-tool s n)
  "Run a subcommand of the repository's tool, git or jj, in its root.
What it says is in the process log ($)."
  (let ((repo (status-repo s)))
    (completing-read s (string-append "Run " (symbol->string (car repo)) " ") '() #:require-match #f #:history 'vc-run
      #:accept (lambda (s c)
                 (let ((args (split-arguments (candidate-text c))))
                   (when (null? args) (error "Enter a subcommand"))
                   (message! s (string-append "Running " (symbol->string (car repo)) " " (string-join args " ") "…"))
                   (request! (make-request-slot) (lambda () (run-write! repo args (string-append (symbol->string (car repo)) " " (car args))))
                             (lambda (out)
                               (let ((lines (nonempty-lines (string-append out %last-stderr))))
                                 (message! s (if (null? lines) "Done; $ shows the process log" (string-append (last lines) "  ($ log)"))))
                               (refresh-repository! repo))
                             #:fail (lambda (e) (message! s (error-text e)) (refresh-repository! repo))))))))

(define-command (vc-run-shell s n)
  "Run a shell command in the repository's root.
What it says is in the process log, \\[vc-process-log]."
  (let ((repo (status-repo s)))
    (completing-read s "Shell command in repository: " '() #:require-match #f #:history 'vc-shell
      #:accept (lambda (s c)
                 (let ((command (string-trim (candidate-text c))))
                   (request! (make-request-slot)
                             (lambda ()
                               (guard (e ((not (cancelled-condition? e)) (log-process! repo command (list "sh" "-c" command) 'fail (error-text e)) (raise e)))
                                 (let ((out (run (cadr repo) "sh" (list "-c" command))))
                                   (log-process! repo command (list "sh" "-c" command) 'ok out) out)))
                             (lambda (out)
                               (let ((lines (nonempty-lines out)))
                                 (message! s (if (null? lines) "Done" (string-append (last lines) "  ($ log)"))))
                               (refresh-repository! repo))
                             #:fail (lambda (e) (message! s (error-text e)))))))))

(define-command (vc-run-menu s n)
  "Show the run menu: a subcommand of git or jj, or a shell command."
  (vc-transient! s
    '("Run"
      ("Run in the repository's root" (suffix "!" "A git or jj subcommand" vc-run-tool) (suffix "s" "A shell command" vc-run-shell)
                                      (suffix "$" "Show the process log" vc-process-log)))))

;;; Dispatch: every menu, as magit-dispatch shows them.

(define-command (vc-dispatch s n)
  "Show every version control command that applies here, by key."
  (let* ((b (session-buffer s)) (repo (status-repo s)) (t (target-at (doc s) (point s))))
    (if (eq? (car repo) 'git)
        (vc-transient! s
          `("Git"
            ("Menus" (suffix "A" "Cherry-pick" vc-cherry-pick-menu) (suffix "b" "Branch" vc-branch-menu)
                     (suffix "c" "Commit" vc-commit-menu) (suffix "d" "Diff" vc-diff-menu) (suffix "f" "Fetch" vc-fetch-menu)
                     (suffix "F" "Pull" vc-pull-menu) (suffix "l" "Log" vc-log-menu) (suffix "m" "Merge" vc-merge-menu))
            ("" (suffix "P" "Push" vc-push-menu) (suffix "r" "Rebase" vc-rebase-menu) (suffix "t" "Tag" vc-tag-menu)
                (suffix "V" "Revert" vc-revert-menu) (suffix "X" "Reset" vc-reset-menu) (suffix "z" "Stash" vc-stash-menu)
                (suffix "!" "Run" vc-run-menu) (suffix "N" "Forge" vc-forge-menu))
            ("Changes at point" (suffix "s" "Stage" vc-stage) (suffix "u" "Unstage" vc-unstage) (suffix "k" "Discard" vc-discard-at-point)
                                (suffix "S" "Stage all" vc-stage-all) (suffix "U" "Unstage all" vc-unstage-all)
                                (suffix "e" "Resolve conflict" vc-conflicts-at-point
                                        ,(lambda (s) (let ((t (target-at (doc s) (point s)))) (git-conflict-target? (file-target t))))))
            ("View" (suffix "g" "Refresh" vc-status-refresh) (suffix "TAB" "Fold or show" vc-toggle-section)
                    (suffix "RET" "Visit" vc-visit-at-point) (suffix "=" "Patch at point" vc-diff-at-point)
                    (suffix "y" "Branches and tags" vc-show-refs) (suffix "$" "Process log" vc-process-log)
                    (suffix "q" "Quit" quit-window))))
        (vc-jj-dispatch s n))))

;;; Log: the history graph of the refs chosen, each commit a target.

(define-mode vc-log-mode
  "Browse a history graph of commits.
\\[vc-visit-at-point] shows the commit at point; the
menus act on it: \\[vc-cherry-pick-menu] picks it, \\[vc-reset-menu]
resets to it, \\[vc-rebase-menu] rewrites from it."
  #:parent 'special-mode
  #:directory (lambda (b) (normal-directory (cadr (hash-table-ref (buffer-state b) 'repo))))
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("RET" vc-visit-at-point) ("=" vc-diff-at-point) ("SPC" vc-diff-at-point) ("n" vc-next-item) ("p" vc-previous-item)
           ("g" vc-log-refresh) ("+" vc-log-more) ("q" vc-return-to-status) ("?" vc-dispatch) ("l" vc-log-menu)
           ("A" vc-cherry-pick-menu) ("V" vc-revert-menu) ("X" vc-reset-menu) ("r" vc-rebase-menu) ("c" vc-commit-menu)
           ("b" vc-branch-menu) ("t" vc-tag-menu) ("m" vc-merge-menu) ("d" vc-diff-menu) ("$" vc-process-log))
  #:normal '(("RET" vc-visit-at-point) ("=" vc-diff-at-point) ("n" vc-next-item) ("p" vc-previous-item)
             ("g r" vc-log-refresh) ("+" vc-log-more) ("q" vc-return-to-status) ("?" vc-dispatch)
             ("A" vc-cherry-pick-menu) ("V" vc-revert-menu) ("X" vc-reset-menu) ("r" vc-rebase-menu) ("c" vc-commit-menu)
             ("b" vc-branch-menu) ("t" vc-tag-menu) ("d" vc-diff-menu) ("$" vc-process-log)))

(define (git-graph-lines repo revs args limit)
  (map (lambda (line) (string-split line (string (integer->char 31))))
       (nonempty-lines
        (run-in repo (append (list "--no-optional-locks" "log" "--graph" "--decorate=full" "--color=never"
                                   (string-append "--max-count=" (number->string limit))
                                   "--format=%x1f%H%x1f%h%x1f%D%x1f%s%x1f%an%x1f%ar")
                             args revs '("--"))))))

(define (render-log! b)
  (let* ((st (buffer-state b)) (repo (hash-table-ref st 'repo)) (lines (hash-table-ref st 'lines))
         (limit (hash-table-ref st 'limit)))
    (hash-table-set! st 'rows
      (present! (buffer-document b)
        (append (list (row (list (list (string-append "Commits in " (string-join (hash-table-ref st 'revs) " ")) 'heading)
                                 (let ((args (hash-table-ref st 'args)))
                                   (if (null? args) "" (list (string-append "  " (string-join args " ")) 'shadow))))
                           #:key 'head))
                (hint-rows "RET show · A pick · V revert · X reset · r rebase from here · c f fixup · b c branch here · + more · q quit" 'hints)
                (map (lambda (fields i)
                       (if (< (length fields) 7)
                           (row (list (list (car fields) 'vc.graph)) #:key (list 'graph i))
                           (commit-row repo (cdr fields) (list 'commit (cadr fields)) #:graph (car fields) #:author #t)))
                     lines (iota (length lines)))
                (if (>= (count (lambda (f) (>= (length f) 7)) lines) limit)
                    (hint-rows (string-append (number->string limit) " commits shown · + shows more") 'more)
                    '()))))))

(define (show-log! s repo revs args #:limit [limit 256] #:show [show #t])
  (let ((origin (session-buffer s)))
    (when show (message! s "Reading history…"))
    (request! (make-request-slot) (lambda () (git-graph-lines repo revs args limit))
              (lambda (lines)
                (let* ((name (string-append "*vc-log " (cadr repo) "*"))
                       (b (or (buffer-named name)
                              (make-generated-buffer! name (make-presentation) 'vc-log-mode #:state (make-hash-table))))
                       (st (buffer-state b)))
                  (hash-table-set! st 'repo repo)
                  (hash-table-set! st 'revs revs)
                  (hash-table-set! st 'args args)
                  (hash-table-set! st 'limit limit)
                  (hash-table-set! st 'lines lines)
                  (hash-table-set! st 'refresh (lambda () (show-log! s repo revs args #:limit limit #:show #f)))
                  (render-log! b)
                  (when show (message! s #f))
                  (when (and show (not (eq? (session-buffer s) b)) (eq? origin (session-buffer s)))
                    (show-buffer! s b))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define (log-args s)
  (let ((args (vc-transient-args s)))
    (filter (lambda (a) (or (member a '("--first-parent" "--no-merges" "--reverse")) (string-prefix? "--author=" a)
                            (string-prefix? "--grep=" a) (string-prefix? "-G" a)))
            args)))

(define (log-limit s)
  (let ((v (vc-transient-value s "--max-count="))) (or (and v (string->number v)) 256)))

(define-command (vc-log-current s n)
  "Show the history of the current branch."
  (show-log! s (current-git-repo s) '("HEAD") (log-args s) #:limit (log-limit s)))

(define-command (vc-log-other s n)
  "Show the history of a branch or revision read in the minibuffer."
  (let ((args (log-args s)) (limit (log-limit s)))
    (read-ref s "Log of: " (lambda (ref) (show-log! s (current-git-repo s) (list ref) args #:limit limit)))))

(define-command (vc-log-branches s n)
  "Show the history of every local branch."
  (show-log! s (current-git-repo s) '("--branches") (log-args s) #:limit (log-limit s)))

(define-command (vc-log-all-branches s n)
  "Show the history of every local and remote branch."
  (show-log! s (current-git-repo s) '("--branches" "--remotes") (log-args s) #:limit (log-limit s)))

(define-command (vc-log-all s n)
  "Show the history of every ref."
  (show-log! s (current-git-repo s) '("--all") (log-args s) #:limit (log-limit s)))

(define-command (vc-return-to-status s n)
  "Leave this view for the repository's status.
The patch shown beside it is closed."
  (let* ((b (session-buffer s)) (repo (hash-table-ref (buffer-state b) 'repo)) (status (status-buffer repo)))
    (for-each (lambda (name)
                (let* ((other (buffer-named name))
                       (i (and other (list-index (lambda (v) (document=? (view-document v) (buffer-document other))) (session-panes s)))))
                  (when (and i (not (= i (session-focus s))) (> (length (session-panes s)) 1))
                    (let ((focus (session-focus s)))
                      (delete-pane! s i)
                      (sset! s 'focus (if (> focus i) (- focus 1) focus))))))
              (list (string-append "*vc-diff " (cadr repo) "*")))
    (show-buffer! s status)
    (refresh! status)))

(define-command (vc-log-refresh s n)
  "Read this history again."
  (let ((refresh (hash-table-ref/default (buffer-state (session-buffer s)) 'refresh #f)))
    (when refresh (refresh))))

(define-command (vc-log-more s n)
  "Show twice as many commits."
  (let* ((st (buffer-state (session-buffer s))))
    (show-log! s (hash-table-ref st 'repo) (hash-table-ref st 'revs) (hash-table-ref st 'args)
               #:limit (* 2 (hash-table-ref st 'limit)))))

(define-command (vc-log-menu s n)
  "Show the log menu."
  (if (git-repo? s)
      (vc-transient! s
        '("Log"
          ("Arguments" (option "-n" "Limit number of commits" "--max-count=")
                       (option "-A" "Only commits by author" "--author=")
                       (option "-F" "Only commits whose message matches" "--grep=")
                       (switch "-f" "First parent only" "--first-parent")
                       (switch "-m" "Omit merges" "--no-merges"))
          ("Log" (suffix "l" "Current branch" vc-log-current) (suffix "o" "Other" vc-log-other)
                 (suffix "h" "HEAD" vc-log-current))
          ("" (suffix "L" "Local branches" vc-log-branches) (suffix "b" "All branches" vc-log-all-branches)
              (suffix "a" "All references" vc-log-all))))
      (vc-jj-log-menu s n)))

;;; Diff menu

(define (show-range! s args title)
  (let ((flags (filter (lambda (a) (member a '("--ignore-all-space" "--ignore-blank-lines" "--function-context" "--find-renames")))
                       (vc-transient-args s))))
    (show-target-diff! s (target 'vc-git-range (list (current-git-repo s) (append flags args)
                                                     (if (null? flags) title (string-append title " · " (string-join flags " "))))))))

(define-command (vc-diff-unstaged s n) "Show every unstaged change." (show-range! s '() "Unstaged changes · index → working tree"))
(define-command (vc-diff-staged s n) "Show every staged change." (show-range! s '("--cached") "Staged changes · HEAD → index"))
(define-command (vc-diff-worktree s n) "Show every change since HEAD." (show-range! s '("HEAD") "Uncommitted changes · HEAD → working tree"))

(define-command (vc-diff-range s n)
  "Show the changes between two revisions, written A..B or A...B."
  (read-name s "Diff range (A..B): "
    (lambda (range) (show-range! s (list range) (string-append "Range " range)))))

(define-command (vc-diff-commit s n)
  "Show a commit, the one at point or one read."
  (with-commit s "Show commit: " (lambda (sha) (show-target-diff! s (target 'vc-git-commit (list (current-git-repo s) sha))))))

(define-command (vc-diff-menu s n)
  "Show the diff menu."
  (vc-transient! s
    '("Diff"
      ("Arguments" (switch "-w" "Ignore whitespace" "--ignore-all-space") (switch "-b" "Ignore blank lines" "--ignore-blank-lines")
                   (switch "-f" "Show whole functions" "--function-context") (switch "-M" "Detect renames" "--find-renames"))
      ("Show" (suffix "d" "What is at point" vc-diff-at-point) (suffix "u" "Unstaged" vc-diff-unstaged)
              (suffix "s" "Staged" vc-diff-staged) (suffix "w" "Working tree against HEAD" vc-diff-worktree))
      ("Commits" (suffix "c" "A commit" vc-diff-commit) (suffix "r" "A range" vc-diff-range)))))

;;; Refs: branches, remote branches and tags.

(define-mode vc-refs-mode
  "The repository's branches and tags, with their upstreams.
\\[vc-visit-at-point] shows a ref's history; the branch menu acts on the
branch at point."
  #:parent 'special-mode
  #:directory (lambda (b) (normal-directory (cadr (hash-table-ref (buffer-state b) 'repo))))
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("RET" vc-refs-log) ("=" vc-diff-at-point) ("n" vc-next-item) ("p" vc-previous-item) ("g" vc-show-refs)
           ("b" vc-branch-menu) ("k" vc-refs-delete) ("x" vc-refs-delete) ("q" quit-window) ("?" vc-dispatch)
           ("m" vc-merge-menu) ("r" vc-rebase-menu) ("l" vc-log-menu))
  #:normal '(("RET" vc-refs-log) ("=" vc-diff-at-point) ("n" vc-next-item) ("p" vc-previous-item) ("g r" vc-show-refs)
             ("b" vc-branch-menu) ("x" vc-refs-delete) ("q" quit-window) ("?" vc-dispatch)))

(define-command (vc-show-refs s n)
  "Show the repository's branches, remote branches and tags."
  (show-refs! s (current-git-repo s) #t))

(define (show-refs! s repo show)
  (begin
    (request! (make-request-slot)
              (lambda ()
                (map (lambda (line) (string-split line "\t"))
                     (nonempty-lines
                      (run-in repo '("for-each-ref" "--sort=-committerdate"
                                     "--format=%(refname)%09%(refname:short)%09%(objectname)%09%(objectname:short)%09%(upstream:short)%09%(upstream:track)%09%(HEAD)%09%(contents:subject)"
                                     "refs/heads/" "refs/remotes/" "refs/tags/")))))
              (lambda (refs)
                (let* ((name (string-append "*vc-refs " (cadr repo) "*"))
                       (b (or (buffer-named name) (make-generated-buffer! name (make-presentation) 'vc-refs-mode #:state (make-hash-table))))
                       (st (buffer-state b))
                       (of (lambda (prefix) (filter (lambda (r) (and (>= (length r) 8) (string-prefix? prefix (car r))
                                                                     (not (string-suffix? "/HEAD" (car r))))) refs)))
                       (ref-row (lambda (r face)
                                  (row (append (list (if (string=? (list-ref r 6) "*") (list "@ " 'vc.working-copy) "  ")
                                                     (list (pad (label (cadr r)) 28) face) (list (list-ref r 3) 'vc.hash) " ")
                                               (if (string=? (list-ref r 4) "") '()
                                                   (list (list (label (list-ref r 4)) 'vc.remote)
                                                         (list (if (string=? (list-ref r 5) "") " " (string-append " " (list-ref r 5) " ")) 'warning)))
                                               (list (label (string-join (drop r 7) "\t"))))
                                       #:key (car r) #:target (target 'vc-git-ref (list repo (list-ref r 2) (cadr r) (car r)))))))
                  (hash-table-set! st 'repo repo)
                  (hash-table-set! st 'refresh (lambda () (show-refs! s repo #f)))
                  (hash-table-set! st 'rows
                    (present! (buffer-document b)
                      (append (list (row (list (list "Branches and tags" 'heading)) #:key 'head))
                              (hint-rows "RET log · b branch menu · k delete · m merge · r rebase onto · q quit" 'hints)
                              (section 'local "Local branches" (of "refs/heads/") '() (lambda (rs) (map (lambda (r) (ref-row r 'vc.branch)) rs)))
                              (section 'remote "Remote branches" (of "refs/remotes/") '() (lambda (rs) (map (lambda (r) (ref-row r 'vc.remote)) rs)))
                              (section 'tags "Tags" (of "refs/tags/") '() (lambda (rs) (map (lambda (r) (ref-row r 'vc.tag)) rs))))))
                  (when (and show (not (eq? (session-buffer s) b))) (pop-to-buffer! s b))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define (ref-at s)
  (let ((t (target-at (doc s) (point s))))
    (if (and t (eq? (target-kind t) 'vc-git-ref)) (target-value t) (error "Move to a branch or tag first"))))

(define-command (vc-refs-log s n)
  "Show the history of the branch or tag at point."
  (let ((r (ref-at s))) (show-log! s (car r) (list (cadddr r)) '())))

(define-command (vc-refs-delete s n)
  "Delete the local branch or tag at point, after asking."
  (let* ((r (ref-at s)) (full (cadddr r)) (short (caddr r)))
    (cond ((string-prefix? "refs/heads/" full)
           (vc-y-or-n s (string-append "Delete branch " (label short) "?")
             (lambda () (git-command! s (car r) (list "branch" "-d" "--" short) (string-append "Delete " (label short))))))
          ((string-prefix? "refs/tags/" full)
           (vc-y-or-n s (string-append "Delete tag " (label short) "?")
             (lambda () (git-command! s (car r) (list "tag" "-d" short) (string-append "Delete " (label short))))))
          (else (message! s "Remote branches are deleted by pushing; use ! push --delete")))))

;;; Interactive rebase: the plan in a buffer, edited with keys, then run.

(define-mode vc-rebase-todo-mode
  "Plan an interactive rebase, oldest commit first.
Each line's action is set with a key: \\[vc-todo-pick] pick, \\[vc-todo-reword]
reword, \\[vc-todo-edit] edit, \\[vc-todo-squash] squash,
\\[vc-todo-fixup] fixup, \\[vc-todo-drop] drop; \\[vc-todo-up] and
\\[vc-todo-down] move it. \\[vc-todo-finish] runs the rebase,
\\[vc-todo-cancel] leaves it."
  #:parent 'special-mode
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("p" vc-todo-pick) ("r" vc-todo-reword) ("e" vc-todo-edit) ("s" vc-todo-squash) ("f" vc-todo-fixup)
           ("d" vc-todo-drop) ("k" vc-todo-drop) ("M-p" vc-todo-up) ("M-n" vc-todo-down) ("M-<up>" vc-todo-up) ("M-<down>" vc-todo-down)
           ("n" vc-next-item) ("RET" vc-diff-at-point) ("C-c C-c" vc-todo-finish) ("C-c C-k" vc-todo-cancel) ("q" vc-todo-cancel)
           ("?" vc-todo-help))
  #:normal '(("p" vc-todo-pick) ("r" vc-todo-reword) ("e" vc-todo-edit) ("s" vc-todo-squash) ("f" vc-todo-fixup)
             ("d" vc-todo-drop) ("M-p" vc-todo-up) ("M-n" vc-todo-down) ("RET" vc-diff-at-point)
             ("C-c C-c" vc-todo-finish) ("C-c C-k" vc-todo-cancel) ("q" vc-todo-cancel)))

(define (git-rebase-basis repo)
  (cons (optional-git-ref repo '("symbolic-ref" "--quiet" "HEAD"))
        (chomp (run-in repo '("rev-parse" "--verify" "HEAD")))))

;; The plan for SHA through the observed HEAD: (base entries basis).
(define (git-rebase-plan repo sha)
  (let* ((basis (git-rebase-basis repo))
         (head (cdr basis))
         (base (git-rebase-base repo sha))
         (range (if base (string-append base ".." head) head)))
    (unless (null? (nonempty-lines (run-in repo (list "rev-list" "--merges" range "--"))))
      (error "This history has merges; rebase it with ! rebase --rebase-merges"))
    (let ((entries (map (lambda (line) (let ((f (string-split line (string (integer->char 31)))))
                                        (list "pick" (car f) (cadr f) (string-join (cddr f) " "))))
                        (nonempty-lines (run-in repo (list "log" "--reverse" "--format=%H%x1f%h%x1f%s" range "--"))))))
      (unless (and (pair? entries) (string=? sha (cadr (car entries))))
        (error "Select a commit on the current branch to rebase it"))
      (list base entries basis))))

(define (octal-escape text)
  (call-with-output-string
    (lambda (p)
      (let ((bytes (string->utf8 text)))
        (do ((i 0 (+ i 1))) ((= i (bytevector-length bytes)))
          (let ((digits (number->string (bytevector-u8-ref bytes i) 8)))
            (display "\\0" p)
            (display (make-string (- 3 (string-length digits)) #\0) p)
            (display digits p)))))))

(define (todo-text entries)
  (apply string-append
         (map (lambda (e)
                (let ((action (car e)) (sha (cadr e)))
                  (if (and (string=? action "reword") (> (length e) 4))
                      ;; The new message, given without an editor: octal escapes
                      ;; keep it out of the shell's grammar.
                      (string-append "pick " sha "\nexec printf '%b' '" (octal-escape (list-ref e 4))
                                     "' | git commit --amend --only --allow-empty --no-verify -q -F -\n")
                      (string-append (if (string=? action "reword") "pick" action) " " sha "\n"))))
              entries)))

(define (run-rebase-todo! s repo base entries basis done)
  (when (and (pair? entries) (member (car (car entries)) '("squash" "fixup")))
    (error "The oldest commit has nothing to squash into"))
  (git-command! s repo (append '("rebase" "-i" "--autostash") (if base (list base) '("--root")))
                "Interactive rebase" #:input (todo-text entries) #:after done
                #:check (lambda ()
                          (unless (equal? basis (git-rebase-basis repo))
                            (error "Branch or HEAD changed; make a new rebase plan")))
                ;; Check again when Git starts the sequence editor, after any
                ;; asynchronous evidence reads and before replacing its todo.
                #:env (list "GIT_EDITOR=true" "GIT_TERMINAL_PROMPT=0"
                            (string-append "TECHNE_REBASE_HEAD=" (cdr basis))
                            (string-append "TECHNE_REBASE_BRANCH=" (or (car basis) ""))
                            (string-append "GIT_SEQUENCE_EDITOR=test \"$(git rev-parse --verify HEAD)\" = \"$TECHNE_REBASE_HEAD\""
                                           " && test \"$(git symbolic-ref --quiet HEAD)\" = \"$TECHNE_REBASE_BRANCH\" && cat >"))))

(define (render-todo! b)
  (let* ((st (buffer-state b)) (entries (hash-table-ref st 'entries)) (repo (hash-table-ref st 'repo)))
    (hash-table-set! st 'rows
      (present! (buffer-document b)
        (append (list (row (list (list (string-append "Rebase " (number->string (length entries)) " commits, oldest first") 'heading)) #:key 'head))
                (hint-rows "p pick · r reword · e edit · s squash · f fixup · d drop · M-p/M-n move · RET show · C-c C-c run · C-c C-k cancel" 'hints)
                (map (lambda (e i)
                       (let ((face (cond ((string=? (car e) "drop") 'error) ((member (car e) '("squash" "fixup")) 'warning)
                                         ((string=? (car e) "pick") 'keyword) (else 'success))))
                         (row (list (list (pad (car e) 8) face) (list (caddr e) 'vc.hash) " "
                                    (if (string=? (car e) "drop") (list (label (cadddr e)) 'shadow) (label (if (> (length e) 4) (list-ref e 4) (cadddr e)))))
                              #:key (list 'todo (cadr e)) #:target (target 'vc-git-commit (list repo (cadr e))))))
                     entries (iota (length entries))))))))

(define-command (vc-git-rebase-interactive s n)
  "Rewrite the commit at point and the commits after it, as planned.
The plan is edited in a buffer of its own, then \\[vc-todo-finish] runs it."
  (with-commit s "Rebase interactively from commit: "
    (lambda (sha)
      (let ((repo (current-git-repo s)) (origin (session-buffer s)))
        (request! (make-request-slot) (lambda () (git-rebase-plan repo sha))
                  (lambda (plan)
                    (let* ((name (string-append "*vc-rebase " (cadr repo) "*"))
                           (b (make-generated-buffer! name (make-presentation) 'vc-rebase-todo-mode #:state (make-hash-table)))
                           (st (buffer-state b)))
                      (hash-table-set! st 'repo repo)
                      (hash-table-set! st 'base (car plan))
                      (hash-table-set! st 'entries (cadr plan))
                      (hash-table-set! st 'basis (caddr plan))
                      (hash-table-set! st 'origin origin)
                      (render-todo! b)
                      (show-buffer! s b)
                      (listing-goto! s b (row-key-string (list 'todo (cadr (car (cadr plan))))))
                      (message! s "Set each commit's action, then C-c C-c runs the rebase")))
                  #:fail (lambda (e) (message! s (error-text e))))))))

(define (todo-index s)
  (let* ((b (session-buffer s)) (t (target-at (doc s) (point s))) (entries (hash-table-ref (buffer-state b) 'entries)))
    (or (and t (list-index (lambda (e) (string=? (cadr e) (cadr (target-value t)))) entries))
        (error "Move to a commit of the plan"))))

(define (set-todo! s f)
  (let* ((b (session-buffer s)) (st (buffer-state b)) (i (todo-index s)) (entries (hash-table-ref st 'entries)))
    (hash-table-set! st 'entries (f entries i))
    (render-todo! b)))

(define (set-action! s action)
  (set-todo! s (lambda (entries i)
                 (map (lambda (e j) (if (= i j) (cons action (take (cdr e) 3)) e)) entries (iota (length entries)))))
  (step-item! s 1))

(define-command (vc-todo-pick s n) "Keep the commit as it is." (set-action! s "pick"))
(define-command (vc-todo-edit s n) "Stop at the commit to change it." (set-action! s "edit"))
(define-command (vc-todo-squash s n) "Meld the commit into the one before, keeping both messages." (set-action! s "squash"))
(define-command (vc-todo-fixup s n) "Meld the commit into the one before, keeping its message only." (set-action! s "fixup"))
(define-command (vc-todo-drop s n) "Leave the commit out." (set-action! s "drop"))

(define-command (vc-todo-reword s n)
  "Give the commit a new message."
  (let* ((b (session-buffer s)) (i (todo-index s)) (e (list-ref (hash-table-ref (buffer-state b) 'entries) i)))
    (read-name-initial s "New message: " (if (> (length e) 4) (list-ref e 4) (cadddr e))
      (lambda (message)
        (let ((st (buffer-state b)))
          (hash-table-set! st 'entries (map (lambda (x j) (if (= i j) (list "reword" (cadr x) (caddr x) (cadddr x) message) x))
                                            (hash-table-ref st 'entries) (iota (length (hash-table-ref st 'entries)))))
          (render-todo! b))))))

(define (move-todo! s delta)
  (let ((key #f))
    (set-todo! s (lambda (entries i)
                   (let ((j (+ i delta)))
                     (if (or (< j 0) (>= j (length entries))) entries
                         (let ((v (list->vector entries)))
                           (let ((x (vector-ref v i))) (vector-set! v i (vector-ref v j)) (vector-set! v j x)
                             (set! key (cadr x)))
                           (vector->list v))))))
    (when key (listing-goto! s (session-buffer s) (row-key-string (list 'todo key))))))

(define-command (vc-todo-up s n) "Move the commit earlier." (move-todo! s -1))
(define-command (vc-todo-down s n) "Move the commit later." (move-todo! s 1))

(define-command (vc-todo-finish s n)
  "Run the rebase as planned."
  (let* ((b (session-buffer s)) (st (buffer-state b)) (origin (hash-table-ref st 'origin)))
    (run-rebase-todo! s (hash-table-ref st 'repo) (hash-table-ref st 'base) (hash-table-ref st 'entries)
                      (hash-table-ref st 'basis)
                      (lambda ()
                        (when (eq? (session-buffer s) b) (show-buffer! s origin))
                        (drop-buffer! s b)))))

(define-command (vc-todo-cancel s n)
  "Leave the plan without rebasing."
  (let* ((b (session-buffer s)) (origin (hash-table-ref (buffer-state b) 'origin)))
    (show-buffer! s origin)
    (drop-buffer! s b)
    (message! s "Rebase cancelled; nothing changed")))

(define-command (vc-todo-help s n)
  "Show the keys of the rebase plan."
  (message! s "p pick · r reword · e edit · s squash · f fixup · d drop · M-p/M-n move · C-c C-c run · C-c C-k cancel"))

;;; Forge menu

(define-command (vc-forge-menu s n)
  "Show the forge menu: pull requests on GitHub, read only."
  (vc-transient! s
    '("Forge · GitHub, read only"
      ("Pull requests" (suffix "l" "List open pull requests" vc-forge) (suffix "N" "List open pull requests" vc-forge)
                       (suffix "b" "Of this branch or bookmark" vc-forge-local-topic)))))

;;; Jujutsu's remotes, operations and evolution.

(define-command (vc-jj-fetch-default s n)
  "Fetch from the default remotes of the Jujutsu workspace."
  (let ((repo (current-jj-repo s)))
    (request! (make-request-slot) (lambda () (jj-operation-id repo))
              (lambda (before) (jj-write! s repo before '("--ignore-working-copy" "git" "fetch")))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-jj-fetch-all s n)
  "Fetch from every remote of the Jujutsu workspace."
  (let ((repo (current-jj-repo s)))
    (request! (make-request-slot) (lambda () (jj-operation-id repo))
              (lambda (before) (jj-write! s repo before '("--ignore-working-copy" "git" "fetch" "--all-remotes")))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-jj-fetch-menu s n)
  "Show the Jujutsu fetch menu."
  (vc-transient! s
    '("Fetch"
      ("Fetch from" (suffix "f" "Default remotes" vc-jj-fetch-default) (suffix "a" "All remotes" vc-jj-fetch-all)
                    (suffix "e" "A remote read" vc-jj-fetch)))))

(define (confirm-jj-push! s question args)
  (let ((repo (current-jj-repo s)))
    (request! (make-request-slot) (lambda () (jj-operation-id repo))
              (lambda (before)
                (completing-read s question '("no" "yes")
                  #:accept (lambda (s c)
                             (when (string=? (candidate-text c) "yes")
                               (jj-write! s repo before (append '("--ignore-working-copy" "git" "push") args))))))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-jj-push-change s n)
  "Push the change at point under a bookmark made from its change ID."
  (let ((dry (vc-transient-arg? s "--dry-run")))
    (with-jj-change s
      (lambda (repo commit before rev)
        (confirm-jj-push! s (string-append "Push change " (substring commit 0 8) " under a new bookmark? ")
                          (append (list "--change" rev) (if dry '("--dry-run") '())))))))

(define-command (vc-jj-push-tracked s n)
  "Push every tracked bookmark that moved."
  (confirm-jj-push! s "Push every tracked bookmark that moved? " (append '("--tracked") (if (vc-transient-arg? s "--dry-run") '("--dry-run") '()))))

(define-command (vc-jj-push-all s n)
  "Push every bookmark, new ones too."
  (confirm-jj-push! s "Push every bookmark, new ones too? " (append '("--all") (if (vc-transient-arg? s "--dry-run") '("--dry-run") '()))))

(define-command (vc-jj-push-menu s n)
  "Show the Jujutsu push menu."
  (vc-transient! s
    '("Push"
      ("Arguments" (switch "-n" "Only say what would be pushed" "--dry-run"))
      ("Push" (suffix "p" "A bookmark of the change at point" vc-jj-publish)
              (suffix "c" "The change at point, as a new bookmark" vc-jj-push-change)
              (suffix "t" "Tracked bookmarks" vc-jj-push-tracked)
              (suffix "a" "All bookmarks" vc-jj-push-all)))))

(define (jj-move! s verb)
  (let ((repo (current-jj-repo s)))
    (request! (make-request-slot) (lambda () (jj-operation-id repo))
              (lambda (before) (jj-write! s repo before (list verb)))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-jj-prev s n)
  "Move the working copy to the parent change, as jj prev."
  (jj-move! s "prev"))

(define-command (vc-jj-next s n)
  "Move the working copy to the child change, as jj next."
  (jj-move! s "next"))

(define-command (vc-jj-operations-menu s n)
  "Show the operations menu: the operation log, undo and redo."
  (vc-transient! s
    '("Operations"
      ("Operations" (suffix "l" "Operation log" vc-jj-operations) (suffix "u" "Undo the last" vc-jj-undo)
                    (suffix "r" "Redo" vc-jj-redo)))))

(define-command (vc-jj-evolog s n)
  "Show how the change at point evolved.
That is each commit it was, and the operation that made it."
  (let* ((repo (current-jj-repo s)) (commit (or (jj-change-at s) (error "Move to a change first"))) (origin (session-buffer s)))
    (request! (make-request-slot)
              (lambda ()
                (map (lambda (line) (string-split line "\t"))
                     (nonempty-lines
                      (jj-read repo (list "evolog" "--no-graph" "-r" commit "-T"
                                          (string-append "commit.commit_id() ++ \"\\t\" ++ commit.change_id().shortest(8) ++ \"\\t\""
                                                         " ++ commit.committer().timestamp().ago() ++ \"\\t\""
                                                         " ++ if(operation, operation.description().first_line(), \"\") ++ \"\\t\""
                                                         " ++ commit.description().first_line() ++ \"\\n\""))))))
              (lambda (entries)
                (show-file-records! s origin repo (string-append "change " (substring commit 0 8)) "Evolution"
                  (map (lambda (e)
                         (row (list (list (substring (car e) 0 12) 'vc.commit-id) "  " (list (pad (list-ref e 2) 16) 'vc.author)
                                    (list (pad (label (list-ref e 3)) 36) 'shadow)
                                    (if (string=? (string-join (drop e 4) "\t") "") (list "(no description)" 'shadow)
                                        (label (string-join (drop e 4) "\t"))))
                              #:key (car e) #:target (target 'vc-reviewed (list repo (car e)))))
                       entries)
))
              #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-jj-dispatch s n)
  "Show every Jujutsu command, by key."
  (vc-transient! s
    '("Jujutsu"
      ("Change at point" (suffix "d" "Describe" vc-jj-describe) (suffix "c" "Commit the working copy" vc-jj-commit)
                         (suffix "e" "Edit" vc-jj-edit) (suffix "o" "New…" vc-jj-new-menu) (suffix "O" "New on point" vc-jj-new)
                         (suffix "x" "Abandon" vc-jj-abandon) (suffix "y" "Duplicate" vc-jj-duplicate))
      ("Rewrite" (suffix "s" "Squash…" vc-jj-squash-menu) (suffix "S" "Split out" vc-jj-split)
                 (suffix "R" "Restore (discard)" vc-jj-restore) (suffix "r" "Rebase…" vc-jj-rebase-menu)
                 (suffix "A" "Absorb into ancestors" vc-jj-absorb) (suffix "C" "Resolve a conflict" vc-jj-conflicts))
      ("Repository" (suffix "b" "Bookmarks…" vc-jj-bookmark-menu) (suffix "f" "Fetch…" vc-jj-fetch-menu)
                    (suffix "P" "Push…" vc-jj-push-menu) (suffix "u" "Undo" vc-jj-undo) (suffix "U" "Redo" vc-jj-redo)
                    (suffix "X" "Operations…" vc-jj-operations-menu) (suffix "N" "Forge…" vc-forge-menu))
      ("View" (suffix "g" "Refresh" vc-status-refresh) (suffix "l" "Revsets…" vc-jj-log-menu)
              (suffix "/" "Revset" vc-jj-filter) (suffix "v" "Evolution" vc-jj-evolog)
              (suffix "TAB" "Fold or show" vc-toggle-section) (suffix "=" "Patch at point" vc-diff-at-point))
      ("Marks and tools" (suffix "m" "Mark or unmark" vc-jj-mark) (suffix "M" "Unmark all" vc-jj-unmark-all)
                         (suffix "$" "Process log" vc-process-log) (suffix "!" "Run…" vc-run-menu)
                         (suffix "q" "Quit" quit-window)))))

(define-command (vc-find-change s n)
  "Choose a changed file of this buffer's repository.
\\[minibuffer-accept] opens it, \\[minibuffer-act] offers what can be
done with it."
  (let ((dir (default-directory s)))
    (request! %open-slot
              (lambda () (changes (repository dir)))
              (lambda (files)
                (completing-read s "Changed file: "
                                 (map (lambda (t)
                                        (let* ((v (target-value t)) (side (cadddr v))
                                               (section (assq side %git-sections)))
                                          (candidate (label (cadr v)) #:annotation (if section (cdr section) "jj recorded snapshot") #:target t)))
                                      files)))
              #:fail (lambda (e) (message! s (error-text e))))))

;; Refresh the status buffers by themselves, while this generation lives.
;; A buffer still waiting for its last refresh is left to finish it: a
;; tool slower than the interval would else be cancelled every time.
(spawn (lambda ()
         (let loop ()
           (let ((ms (option #f 'vc-refresh-interval)))
             (sleep (or ms 1000))
             (when ms
               (for-each refresh!
                         (filter (lambda (b)
                                   (let ((st (buffer-state b)))
                                     (and (not (remote-path? (cadr (hash-table-ref st 'repo))))
                                          (not (request-pending? (hash-table-ref st 'slot))))))
                                 (status-buffers))))
             (loop)))))

;; A new generation takes over the status buffers shown: their requests in
;; flight were the previous generation's, cancelled with it.
(for-each (lambda (b) (hash-table-set! (buffer-state b) 'slot (make-request-slot)) (refresh! b)) (status-buffers))

(register-mode-line-segment! 'vc-keys
  (lambda (b)
    (case (buffer-mode b)
      ((vc-git-status-mode) "? menu  ·  TAB fold  ·  s stage  ·  c commit")
      ((vc-jj-status-mode) (let ((marks (or (hash-table-ref/default (buffer-state b) 'marks #f) '())))
                             (if (pair? marks)
                                 (string-append (number->string (length marks)) " marked  ·  M unmark")
                                 "? menu  ·  TAB fold  ·  u undo")))
      ((vc-log-mode) "RET show  ·  ? menu  ·  + more")
      ((vc-refs-mode) "RET log  ·  b branch  ·  k delete")
      ((vc-rebase-todo-mode) "C-c C-c run  ·  C-c C-k cancel")
      ((vc-process-mode) "q quit")
      ((vc-history-mode) "RET patch  ·  q status")
      ((vc-file-history-mode) "RET file patch  ·  n/p revision  ·  q file")
      ((vc-forge-mode) "RET files  ·  [/] page  ·  q status")
      ((vc-review-mode) "n/p item  ·  d discuss  ·  C checks  ·  q back")
      ((vc-operations-mode) "? keys  ·  RET diff")
      ((vc-jj-files-mode) "? commands  ·  n/p file  ·  q graph")
      ((vc-commit-mode) "C-c C-c save message  ·  C-c C-k keep draft")
      (else #f))))

;; Resolve the path against the checkout on the same endpoint as the file.
(define (with-vc-file s compute deliver)
  (let* ((origin (session-buffer s)) (name (document-path (doc s))) (dir (default-directory s)))
    (unless name (error "Visit a version-controlled file first"))
    (request! %open-slot
      (lambda ()
        (let* ((repo (repository dir)) (root (normal-directory (location-path (cadr repo))))
               (file (location-path name)))
          (unless (and (string-prefix? root file) (> (string-length file) (string-length root)))
            (error "The file is outside this repository root"))
          (let ((path (substring file (string-length root))))
            (list repo path (compute repo path)))))
      (lambda (result)
        (when (eq? origin (session-buffer s))
          (deliver origin (car result) (cadr result) (caddr result))))
      #:fail (lambda (e) (message! s (error-text e))))))

(define-command (vc-file-diff s n)
  "Inspect this file's staged or unstaged Git patch, or its jj patch."
  (with-vc-file s
    (lambda (repo path)
      (if (eq? (car repo) 'jj)
          (let ((id (chomp (jj-read repo '("log" "--no-graph" "-r" "@" "-T" "commit_id")))))
            (list (target 'vc-jj-file (list repo id path (list (exact-jj-file path)) #f))))
          (filter (lambda (t) (let ((v (target-value t))) (or (equal? path (cadr v)) (equal? path (list-ref v 4)))))
                  (changes repo))))
    (lambda (origin repo path targets)
      (cond ((null? targets) (message! s "No on-disk changes to this file"))
            ((null? (cdr targets)) (show-target-diff! s (car targets)))
            (else (completing-read s (string-append "Compare " (label path) ": ")
                    (map (lambda (t) (candidate (comparison t) #:target t)) targets)
                    #:accept (lambda (s c) (show-target-diff! s (candidate-target c)))))))))

(define-mode vc-file-history-mode
  "Browse history or line annotations for a version-controlled file."
  #:parent 'special-mode
  #:target-at (lambda (b pos)
                (let ((r (row-at (buffer-document b) (hash-table-ref (buffer-state b) 'rows) pos)))
                  (and r (row-target r))))
  #:keys '(("RET" vc-file-revision) ("=" vc-file-revision) ("n" vc-next-item) ("p" vc-previous-item) ("q" vc-file-history-return))
  #:normal '(("RET" vc-file-revision) ("=" vc-file-revision) ("n" vc-next-item) ("p" vc-previous-item) ("q" vc-file-history-return)))

(define-command (vc-file-history-return s n)
  "Return to the original file buffer."
  (show-buffer! s (hash-table-ref (buffer-state (session-buffer s)) 'origin)))

(define-command (vc-file-revision s n)
  "Show this file's patch in the revision at point."
  (let ((t (target-at (doc s) (point s))))
    (if (and t (memq (target-kind t) '(vc-file-commit vc-reviewed))) (show-target-diff! s t)
        (error "No committed revision at point"))))

(define (show-file-records! s origin repo path title rows)
  (let* ((name (string-append "*" title " " (location-at (cadr repo) (string-append (normal-directory (location-path (cadr repo))) path)) "*"))
         (b (or (buffer-named name)
                (make-generated-buffer! name (make-presentation) 'vc-file-history-mode #:state (make-hash-table))))
         (st (buffer-state b)))
    (hash-table-set! st 'origin origin)
    (hash-table-set! st 'rows
      (present! (buffer-document b)
        (append (list (row (list (list (string-append title " · " (label path)) 'heading)) #:key 'heading)
                      (row (list (list "Recorded revisions · n/p move · RET file patch · q back" 'shadow)) #:key 'keys)) rows)))
    (show-buffer! s b)))

(define-command (vc-file-history s n)
  "List the recent recorded history of this path in Git or Jujutsu."
  (with-vc-file s
    (lambda (repo path)
      (nonempty-lines
        (if (eq? (car repo) 'jj)
            (jj-read repo (list "log" "--no-graph" "-n" "41" "-r"
                                (string-append "ancestors(@) & files(" (exact-jj-file path) ")")
                                "-T" "commit_id ++ \"\\t\" ++ description.first_line() ++ \"\\n\""))
            (run-in repo (list "--no-lazy-fetch" "--literal-pathspecs" "log" "-41" "--format=%H%x09%s" "--" path)))))
    (lambda (origin repo path lines)
      (show-file-records! s origin repo path "File history"
        (append (map (lambda (line)
                       (let* ((fields (string-split line "\t")) (sha (car fields)))
                         (row (list (list (substring sha 0 12) 'keyword) "  " (label (string-join (cdr fields) "\t")))
                              #:key sha #:target (target 'vc-file-commit (list repo sha path)))))
                     (take lines (min 40 (length lines))))
                (cond ((null? lines) (list (row "No recorded commits for this path" #:key 'empty)))
                      ((> (length lines) 40) (list (row "Showing the latest 40 commits for this path" #:key 'limit)))
                      (else '())))))))

(define-command (vc-file-annotate s n)
  "Annotate the Git file's saved lines with their commits."
  (with-vc-file s
    (lambda (repo path)
      (unless (eq? (car repo) 'git) (error "Line annotation is currently available for Git files"))
      (nonempty-lines (run-in repo (list "--no-lazy-fetch" "--literal-pathspecs" "blame" "--root" "-l" "--date=short" "--" path))))
    (lambda (origin repo path lines)
      (show-file-records! s origin repo path "Git annotate"
        (map (lambda (line i)
               (let* ((end (string-index line #\space)) (sha (and end (substring line 0 end)))
                      (committed? (and sha (review-sha? sha) (not (string-prefix? "000000000000" sha)))))
                 (row (list (list (if committed? (substring sha 0 12) "uncommitted ") 'keyword) " "
                            (if end (label (substring line (+ 1 end))) (label line)))
                      #:key i #:target (and committed? (target 'vc-file-commit (list repo sha path))))))
             lines (iota (length lines)))))))

(define-option vc-refresh-on-save #t
  "Whether saving a file refreshes its repository's status where shown.
A Jujutsu working copy is recorded first, as Magit and Majutsu do."
  #:type 'boolean)

(add-hook! 'document-saved 'vc
  (lambda (s d)
    (let ((path (document-path d)))
      (when (and path (option #f 'vc-refresh-on-save))
        (for-each (lambda (b)
                    (let* ((st (buffer-state b)) (repo (hash-table-ref st 'repo)))
                      (when (and (string-prefix? (normal-directory (cadr repo)) path)
                                 (any (lambda (s) (any (lambda (v) (document=? (view-document v) (buffer-document b))) (session-panes s)))
                                      (attached-sessions)))
                        (refresh! b #:snapshot (and (eq? (car repo) 'jj)
                                                    (not (hash-table-ref/default (vc-recovery-state repo) 'unresolved #f)))))))
                  (status-buffers))))))

(define (file-index! s stage?)
  (with-vc-file s
    (lambda (repo path)
      (unless (eq? (car repo) 'git) (error "Jujutsu records the file itself; g in its status"))
      (run-write! repo (if stage? (list "--literal-pathspecs" "add" "--" path) (list "--literal-pathspecs" "reset" "-q" "--" path))
                  (if stage? "Stage file" "Unstage file")))
    (lambda (origin repo path _)
      (message! s (string-append (if stage? "Staged " "Unstaged ") (label path)))
      (refresh-repository! repo))))

(define-command (vc-file-stage s n)
  "Stage this buffer's file in its Git index."
  (file-index! s #t))

(define-command (vc-file-unstage s n)
  "Unstage this buffer's file from its Git index."
  (file-index! s #f))

(define-key! emacs-map "C-x g" 'vc-status)
(define-key! emacs-map "C-x v s" 'vc-file-stage)
(define-key! emacs-map "C-x v u" 'vc-file-unstage)
(define-key! emacs-map "C-x v d" 'vc-status)
(define-key! emacs-map "C-x v =" 'vc-file-diff)
(define-key! emacs-map "C-x v l" 'vc-file-history)
(define-key! emacs-map "C-x v g" 'vc-file-annotate)
(define-key! emacs-map "C-x v m" 'vc-git-conflicts)
(define-key! emacs-map "C-x v f" 'vc-find-change)
(define-key! emacs-map "C-x v !" 'vc-remote-outcome)
(name-prefix! emacs-map "C-x v" "vc")
