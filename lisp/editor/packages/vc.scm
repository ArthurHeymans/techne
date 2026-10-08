;;; Version control status, as Magit's status and vc-dir show it: the files
;;; changed in a repository (Jujutsu's working-copy change, or Git's work
;;; tree), each a row standing for a vc-file target. M-x vc-status shows
;;; the repository of the focused buffer's file.
;;;
;;; It is a package against (techne editor) like any extension, and the
;;; vertical slice of the architecture review (EDITOR.md, section 1):
;;;
;;; - its rows come from a process, in the background, a newer refresh
;;;   cancelling an older one (request slots); the view updates in place
;;;   and the caret stays on its file's row, keyed by path;
;;; - it refreshes by itself every `vc-refresh-interval` milliseconds, in a
;;;   task the package owns, so reloading or unloading the package stops it
;;;   and cancels what it has in flight; a new generation takes over the
;;;   status buffers already shown, whose state is plain data;
;;; - a vc-file target carries all its actions need (repository, tool,
;;;   path), so they work wherever the target comes from: the view, or the
;;;   minibuffer (M-x vc-find-change, then C-;), the view not even shown.

(import (techne editor))

(define-option vc-refresh-interval 2000
  "How often status buffers refresh by themselves, in milliseconds.
With #f, they never do."
  #:type '(or (one-of #f) natural))

;;; Repositories, through their tools: (tool root), TOOL `jj` or `git`.

;; Run PROGRAM with ARGS; its output, or an error with what it said.
(define (run program args)
  (call-with-process program args
                     (lambda (p)
                       (let* ((out (process-read-all p 'stdout)) (err (process-read-all p 'stderr)) (status (process-wait p)))
                         (if (eqv? status 0) out (error (string-append program ": " (string-trim err))))))))

(define (chomp s) (if (string-suffix? "\n" s) (substring s 0 (- (string-length s) 1)) s))

;; The repository DIR is in: Jujutsu's when there is one, else Git's.
(define (repository dir)
  (or (guard (e (#t #f)) (list 'jj (chomp (run "jj" (list "-R" dir "root" "--ignore-working-copy")))))
      (guard (e (#t #f)) (list 'git (chomp (run "git" (list "-C" dir "rev-parse" "--show-toplevel")))))
      (error "Not in a repository" dir)))

;; The files changed: a list of (status . path), sorted by path.
(define (changes repo)
  (let* ((tool (car repo)) (root (cadr repo))
         (lines (filter (lambda (l) (not (string=? l "")))
                        (string-split (if (eq? tool 'jj)
                                          (run "jj" (list "-R" root "diff" "--summary" "--color" "never"))
                                          (run "git" (list "-C" root "status" "--porcelain=v1" "--untracked-files=all")))
                                      "\n"))))
    (sort (map (lambda (l)
                 (if (eq? tool 'jj)
                     (cons (substring l 0 1) (substring l 2 (string-length l)))
                     (cons (string-trim (substring l 0 2)) (substring l 3 (string-length l)))))
               lines)
          (lambda (a b) (string<? (cdr a) (cdr b))))))

(define (diff repo path)
  (if (eq? (car repo) 'jj)
      (run "jj" (list "-R" (cadr repo) "diff" "--git" "--color" "never" "--" path))
      (run "git" (list "-C" (cadr repo) "diff" "HEAD" "--" path))))

(define (revert repo path status)
  (cond ((eq? (car repo) 'jj) (run "jj" (list "-R" (cadr repo) "restore" "--" path)))
        ((string=? status "??") (error "Untracked, nothing to revert to" path))
        (else (run "git" (list "-C" (cadr repo) "checkout" "HEAD" "--" path)))))

;;; Targets: a changed file, as (repo path status).

(define (vc-file repo path status) (target 'vc-file (list repo path status)))
(define (file-path f) (string-append (cadr (car f)) "/" (cadr f)))

(define-action vc-file (vc-visit s f) "Open the file." (show-document! s (file-document (file-path f))))

(define-action vc-file (vc-diff s f)
  "Show what changed in the file."
  (show-diff! s f))

(define-action vc-file (vc-revert s f)
  "Put back the file as the repository has it, after asking."
  (completing-read s (string-append "Revert " (cadr f) "? ") '("yes" "no")
                   #:accept (lambda (s c)
                              (when (string=? (candidate-text c) "yes")
                                (request! (make-request-slot)
                                          (lambda () (revert (car f) (cadr f) (caddr f)))
                                          (lambda (_)
                                            (message! s (string-append "Reverted " (cadr f)))
                                            (refresh-repository! (car f)))
                                          #:fail (lambda (e) (message! s (error-text e))))))))

(define-action vc-file (vc-copy-path s f) "Save the file's path as a kill." (kill-save! s (file-path f) #f #f))

;; The diff of F in *vc-diff*, shown beside the focused pane.
(define %diff-slot (make-request-slot))
(define (show-diff! s f)
  (request! %diff-slot
            (lambda () (diff (car f) (cadr f)))
            (lambda (text)
              (let* ((b (or (buffer-named "*vc-diff*") (make-generated-buffer! "*vc-diff*" (make-document "") 'log-mode)))
                     (d (buffer-document b)))
                (view-edit! (make-view d "vc") (list (list 0 (document-length d) text)) "new")
                (display-buffer! s d)))
            #:fail (lambda (e) (message! s (error-text e)))))

;;; Status buffers. Their state is plain data, a table, so the package's
;;; next generation reads it as this one does.

(define-mode vc-status-mode
  "The files changed in a repository.
\\[act-default-at-point] opens one, \\[vc-diff-at-point] shows its
diff, \\[act-at-point] offers every action; \\[vc-status-refresh]
refreshes."
  #:parent 'special-mode
  #:keys '(("RET" act-default-at-point) ("=" vc-diff-at-point) ("g" vc-status-refresh) ("C-c C-r" vc-status-refresh))
  #:normal '(("RET" act-default-at-point) ("=" vc-diff-at-point) ("g r" vc-status-refresh))
  #:target-at (lambda (b pos)
                (let ((st (buffer-state b)))
                  (and (hash-table? st)
                       (let ((r (row-at (buffer-document b) (hash-table-ref st 'rows) pos)))
                         (and r (row-target r)))))))

(define (status-buffers) (filter (lambda (b) (eq? (buffer-mode b) 'vc-status-mode)) (buffer-list)))

;; The rows for REPO's CHANGES: where it is, then a file a row, keyed by
;; its path.
(define (status-rows repo changes)
  (cons (row (list (list (string-append (symbol->string (car repo)) " ") 'keyword) (list (cadr repo) 'comment)) #:key 'head)
        (if (null? changes)
            (list (row (list (list "No changes" 'comment)) #:key 'none))
            (map (lambda (c)
                   (row (list (list (car c) 'warning)) (cdr c) #:key (cdr c) #:target (vc-file repo (cdr c) (car c))))
                 changes))))

;; Show CHANGES in status buffer B: only rows that differ change.
(define (show-changes! b changes)
  (let ((st (buffer-state b)))
    (hash-table-set! st 'rows (present! (buffer-document b) (status-rows (hash-table-ref st 'repo) changes)))))

;; Ask for B's changes again; its rows change when they come.
(define (refresh! b)
  (let ((st (buffer-state b)))
    (request! (hash-table-ref st 'slot)
              (lambda () (changes (hash-table-ref st 'repo)))
              (lambda (changes) (show-changes! b changes))
              #:fail (lambda (e)
                       (present! (buffer-document b) (list (row (list (list (error-text e) 'error)) #:key 'error)))))))

(define (refresh-repository! repo)
  (for-each refresh! (filter (lambda (b) (equal? (hash-table-ref (buffer-state b) 'repo) repo)) (status-buffers))))

(define %open-slot (make-request-slot))

(define-command (vc-status s n)
  "Show the files changed in the repository of this buffer's file.
Its rows come in the background: the editor never waits for the tool."
  (let ((dir (default-directory s)))
    (message! s "Looking for the repository…")
    (request! %open-slot
              (lambda () (let ((repo (repository dir))) (cons repo (changes repo))))
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
          (hash-table-set! st 'rows (present! p (status-rows repo '())))
          (make-generated-buffer! name p 'vc-status-mode #:state st)))))

(define-command (vc-status-refresh s n)
  "Ask for this repository's changes again."
  (let ((b (session-buffer s)))
    (if (and b (eq? (buffer-mode b) 'vc-status-mode)) (refresh! b) (error "Not a status buffer"))))

(define-command (vc-diff-at-point s n)
  "Show what changed in the file at point."
  (let ((t (target-at (doc s) (point s))))
    (if (and t (eq? (target-kind t) 'vc-file)) (show-diff! s (target-value t)) (error "No file here"))))

(define-command (vc-find-change s n)
  "Choose a changed file of this buffer's repository.
\\[minibuffer-accept] opens it, \\[minibuffer-act] offers what can be
done with it."
  (let ((dir (default-directory s)))
    (request! %open-slot
              (lambda () (let ((repo (repository dir))) (cons repo (changes repo))))
              (lambda (found)
                (completing-read s "Changed file: "
                                 (map (lambda (c) (candidate (cdr c) #:annotation (car c) #:target (vc-file (car found) (cdr c) (car c))))
                                      (cdr found))))
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
                         (remove (lambda (b) (request-pending? (hash-table-ref (buffer-state b) 'slot)))
                                 (status-buffers))))
             (loop)))))

;; A new generation takes over the status buffers shown: their requests in
;; flight were the previous generation's, cancelled with it.
(for-each (lambda (b) (hash-table-set! (buffer-state b) 'slot (make-request-slot)) (refresh! b)) (status-buffers))

(define-key! emacs-map "C-x v d" 'vc-status)
(define-key! emacs-map "C-x v f" 'vc-find-change)
(name-prefix! emacs-map "C-x v" "vc")
