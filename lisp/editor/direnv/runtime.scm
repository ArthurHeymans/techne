;;; Interactive direnv commands, loaded lazily on first invocation via
;;; `register-command-package!` in `lisp/editor/direnv.scm`.

(import (techne editor) (techne execution-site))
(require "../remote.scm")

(ensure-execution-site!)

(define %direnv-slot (make-request-slot))
(define %direnv-deny-slot (make-request-slot))

(define (internal-direnv-var? name)
  (string-prefix? "DIRENV_" name))

;; Summarize only variable names (+VAR / -VAR), never values (which may be secrets).
(define (format-delta-names delta)
  (let* ((visible (filter (lambda (e) (not (internal-direnv-var? (car e)))) delta))
         (names (sort (map (lambda (e) (string-append (if (cdr e) "+" "-") (car e))) visible)
                      string<?))
         (count (length names)))
    (cond ((= count 0) "no variable changes")
          ((<= count 8) (string-join names " "))
          (else (string-append (string-join (take names 8) " ")
                               " (+" (number->string (- count 8)) " more)")))))

(define (format-snapshot-summary dir snap)
  (let ((st (env-snapshot-status snap))
        (rc (env-snapshot-rc-path snap))
        (msg (env-snapshot-message snap)))
    (case st
      ((active)
       (string-append "direnv active (" (abbreviate-path (or rc dir)) "): "
                      (format-delta-names (env-snapshot-delta snap))))
      ((blocked)
       (string-append "direnv blocked (" (abbreviate-path (or rc dir)) "): run M-x direnv-allow"))
      ((denied)
       (string-append "direnv denied (" (abbreviate-path (or rc dir)) ")"))
      ((error)
       (string-append "direnv error (" (abbreviate-path (or rc dir)) "): " (or msg "failed")))
      ((unavailable)
       (or msg "direnv unavailable (executable not found)"))
      ((unsupported)
       (or msg "direnv not supported for this location"))
      (else
       (if (pair? (env-snapshot-delta snap))
           (string-append "direnv unloaded: " (format-delta-names (env-snapshot-delta snap)))
           (string-append "direnv: none in " (abbreviate-path dir)))))))

(define (run-direnv-async! s pending-msg thunk deliver #:slot [slot %direnv-slot])
  (when pending-msg (message! s pending-msg))
  (request! slot
            thunk
            deliver
            #:fail (lambda (e) (message! s (string-append "direnv: " (error-text e))))))

(define-command (direnv-status s n)
  "Show the direnv execution-site status for the current directory.
Lists changed variable names only; variable values are never displayed.
Evaluates asynchronously when the cache is cold or stale."
  (let* ((dir (default-directory s))
         (cached (execution-site-cached-snapshot dir)))
    (if (and cached (not (execution-site-needs-eval? dir)))
        (message! s (format-snapshot-summary dir cached))
        (begin
          (when (execution-site-has-env-config? dir)
            (note-effect! 'external))
          (run-direnv-async! s
                             "Checking direnv..."
                             (lambda ()
                               (execution-site-env-snapshot (execution-site-for dir #:policy 'project)))
                             (lambda (snap)
                               (message! s (format-snapshot-summary dir snap))))))))

(define (confirm-and-allow! s dir target)
  (let* ((canon-target (guard (e (#t target)) (file-truename target)))
         (digest (execution-site-direnv-file-digest canon-target)))
    (completing-read s (string-append "Allow " (abbreviate-path canon-target) "? ")
                     '("no" "yes" "visit")
                     #:accept (lambda (s c)
                                (let ((choice (candidate-text c)))
                                  (cond ((string=? choice "yes")
                                         (note-effect! 'external)
                                         (run-direnv-async!
                                          s
                                          "Allowing direnv..."
                                          (lambda ()
                                            (execution-site-direnv-allow!
                                             dir
                                             #:expected-target canon-target
                                             #:expected-digest (or digest "")))
                                          (lambda (snap)
                                            (message! s (format-snapshot-summary dir snap)))))
                                        ((string=? choice "visit")
                                         (show-document! s (file-document canon-target)))
                                        (else
                                         (message! s "direnv allow cancelled"))))))))

(define-command (direnv-allow s n)
  "Approve the `.envrc` for the current directory after confirmation.
Shows the resolved `.envrc` path and offers `yes`, `no`, or `visit`
before running `direnv allow` asynchronously under the clean host
environment."
  (let* ((dir (default-directory s))
         (cached (and (not (execution-site-needs-eval? dir))
                      (execution-site-cached-snapshot dir)))
         (cached-rc (and cached (env-snapshot-rc-path cached))))
    (when (remote-path? dir)
      (error "direnv: remote direnv allow is not supported yet"))
    (if (string? cached-rc)
        (confirm-and-allow! s dir cached-rc)
        (begin
          (note-effect! 'external)
          (run-direnv-async! s
                             "Finding .envrc..."
                             (lambda () (execution-site-direnv-target dir))
                             (lambda (target) (confirm-and-allow! s dir target)))))))

(define-command (direnv-deny s n)
  "Revoke `.envrc` approval for the current directory.
Clears the cached environment, runs `direnv deny` asynchronously under
the clean host environment, and reports the new status. Because denial
strictly reduces trust, it runs immediately without an interactive
confirmation prompt or digest pinning."
  (let ((dir (default-directory s)))
    (note-effect! 'external)
    (run-direnv-async! s
                       "Denying direnv..."
                       (lambda () (execution-site-direnv-deny! dir))
                       (lambda (snap)
                         (message! s (format-snapshot-summary dir snap)))
                       #:slot %direnv-deny-slot)))

(define-command (direnv-reload s n)
  "Reload the cached direnv environment for the current directory.
With a prefix argument, clears cached environments for all directories
before reloading the current one asynchronously."
  (let ((dir (default-directory s))
        (all? (and (current-prefix s) #t)))
    (note-effect! 'external)
    (execution-site-invalidate! (if all? #f dir))
    (run-direnv-async! s
                       "Reloading direnv..."
                       (lambda ()
                         (execution-site-env-snapshot (execution-site-for dir #:policy 'project) #:refresh? #t))
                       (lambda (snap)
                         (message! s (format-snapshot-summary dir snap))))))
