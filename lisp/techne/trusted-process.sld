;;; Trusted local helper execution for credential tools (`gpg`, `sops`,
;;; `pass`, `gh`) and clean host baseline environment resolution.
;;;
;;; Helpers always run in a user-trusted working directory (never a
;;; project directory), resolve executables against a clean host PATH
;;; (rejecting relative helper paths containing `/`), unload any
;;; inherited startup `DIRENV_*` overlay, use null stdin, bounded
;;; stdout, a child task draining stderr to EOF, optional timeout, and
;;; full cancellation propagation.

(define-library (techne trusted-process)
  (export executable-file?
          trusted-baseline-environment trusted-baseline-clean? trusted-baseline-path
          trusted-environment-variable
          resolve-trusted-program set-trusted-direnv-program-hook!
          trusted-home expand-trusted-home trusted-cwd
          run-bounded-process run-trusted-tool run-trusted-tool-bytes)
  (import (techne) (techne path) (techne task) (techne trusted-process params))
  (begin
    (define %default-max-stdout 1048576)
    (define %max-stderr-kept 4096)
    (define %baseline-timeout-ms 5000)
    (define %default-bytes-timeout-ms 30000)
    (define %stdin-chunk-size 16384)

    (define (strip-trailing-slash path)
      (if (and (> (string-length path) 1) (string-suffix? "/" path))
          (strip-trailing-slash (substring path 0 (- (string-length path) 1)))
          path))

    (define (path-under-dir? path root)
      (let ((p (strip-trailing-slash path))
            (r (strip-trailing-slash root)))
        (or (string=? p r)
            (string=? r "/")
            (string-prefix? (string-append r "/") p))))

    (define (mode-executable? mode)
      (and (integer? mode)
           (or (= (remainder (quotient mode 64) 2) 1)
               (= (remainder (quotient mode 8) 2) 1)
               (= (remainder mode 2) 1))))

    (define (executable-file? path)
      "Return #t if PATH resolves to an existing executable regular file."
      (and (string? path)
           (> (string-length path) 0)
           (let* ((real (guard (e (#t #f)) (file-truename path)))
                  (attrs (and real (guard (e (#t #f)) (file-attributes real)))))
             (and (pair? attrs)
                  (eq? (car attrs) 'file)
                  (>= (length attrs) 4)
                  (mode-executable? (list-ref attrs 3))))))

    (define (filter-safe-path path-str dirty-root)
      (string-join
       (filter (lambda (d)
                 (and (> (string-length d) 0)
                      (string-prefix? "/" d)
                      (or (not dirty-root)
                          (not (path-under-dir? (guard (e (#t d)) (file-truename d)) dirty-root)))))
               (string-split (or path-str "") ":"))
       ":"))

    (define (resolve-safe-executable path dirty-root)
      (and (string? path)
           (> (string-length path) 0)
           (string-prefix? "/" path)
           (or (not dirty-root)
               (not (path-under-dir? path dirty-root)))
           (let* ((real (guard (e (#t #f)) (file-truename path)))
                  (attrs (and real (guard (e (#t #f)) (file-attributes real)))))
             (and real
                  (or (not dirty-root)
                      (not (path-under-dir? real dirty-root)))
                  (pair? attrs)
                  (eq? (car attrs) 'file)
                  (>= (length attrs) 4)
                  (mode-executable? (list-ref attrs 3))
                  ;; Execute the canonical path directly when its basename matches
                  ;; (closing symlink indirection), while preserving the validated
                  ;; path for multicall binaries (e.g. pwd -> coreutils, sh -> bash)
                  ;; whose behavior depends on argv[0].
                  (if (string=? (path-file-name real) (path-file-name path))
                      real
                      path)))))

    (define (find-executable-in-path name path-str dirty-root)
      (let loop ((dirs (string-split (or path-str "") ":")))
        (if (null? dirs)
            #f
            (let* ((d (car dirs))
                   (cand (and (> (string-length d) 0)
                              (string-prefix? "/" d)
                              (path-join d name)))
                   (real (and cand (resolve-safe-executable cand dirty-root))))
              (or real (loop (cdr dirs)))))))

    (define (safe-username? s)
      (and (string? s)
           (> (string-length s) 0)
           (not (string=? s "."))
           (not (string=? s ".."))
           (every (lambda (ch)
                    (or (char-alphabetic? ch)
                        (char-numeric? ch)
                        (char=? ch #\_)
                        (char=? ch #\-)
                        (char=? ch #\.)))
                  (string->list s))))

    ;; System-owned binary directories searched for bare cleanup binaries when
    ;; the inherited PATH is tainted by an active startup DIRENV_DIR overlay.
    (define (trusted-system-bin-dirs)
      (let ((user (get-environment-variable "USER")))
        (append
         (list "/run/current-system/sw/bin")
         (if (safe-username? user)
             (list (string-append "/etc/profiles/per-user/" user "/bin"))
             '())
         (list "/nix/var/nix/profiles/default/bin"
               "/usr/local/bin"
               "/usr/bin"
               "/bin"
               "/opt/homebrew/bin"))))

    (define (resolve-cleanup-executable prog dirty-root)
      (cond ((not (and (string? prog) (> (string-length prog) 0))) #f)
            ((string-prefix? "/" prog)
             (resolve-safe-executable prog dirty-root))
            ((string-contains prog "/") #f)
            (else
             (find-executable-in-path prog
                                      (string-join (trusted-system-bin-dirs) ":")
                                      dirty-root))))

    (define (drain-stream-keeping p stream keep)
      (let loop ((chunks '()) (kept 0) (truncated? #f))
        (let ((chunk (process-read p stream)))
          (if (eof-object? chunk)
              (cons (apply string-append (reverse chunks)) truncated?)
              (let ((n (string-length chunk)))
                (if (< kept keep)
                    (let* ((room (- keep kept))
                           (over? (> n room))
                           (piece (if over? (substring chunk 0 room) chunk)))
                      (loop (cons piece chunks) (+ kept (string-length piece)) (or truncated? over?)))
                    (loop chunks kept (or truncated? (> n 0)))))))))

    (define (read-stream-bounded-tagged p stream limit)
      (let loop ((chunks '()) (total 0))
        (let ((chunk (process-read p stream)))
          (cond ((eof-object? chunk) (cons 'ok (apply string-append (reverse chunks))))
                ((> (+ total (string-length chunk)) limit) '(output-limit))
                (else (loop (cons chunk chunks) (+ total (string-length chunk))))))))

    (define (read-bytes-bounded p stream limit label)
      (let loop ((chunks '()) (total 0))
        (let ((chunk (process-read-bytes p stream)))
          (cond ((eof-object? chunk)
                 (if (null? chunks)
                     #u8()
                     (apply bytevector-append (reverse chunks))))
                ((> (+ total (bytevector-length chunk)) limit)
                 (error (string-append label ": output exceeded size limit")))
                (else (loop (cons chunk chunks) (+ total (bytevector-length chunk))))))))

    (define (with-tool-timeout timeout-ms label thunk)
      (if (not timeout-ms)
          (thunk)
          (let* ((ch (make-channel 1))
                 (done #f)
                 (worker (spawn
                          (lambda ()
                            (let ((res (guard (e ((task-cancelled-condition? e)
                                                  (guard (e2 (#t #f)) (channel-send ch (cons 'cancelled e)))
                                                  (raise e))
                                                 (#t (cons 'err e)))
                                         (cons 'ok (thunk)))))
                              (guard (e (#t #f)) (channel-send ch res))))
                          #:child #t)))
            (dynamic-wind
              (lambda () #f)
              (lambda ()
                (select
                  (recv ch (res)
                    (set! done #t)
                    (cond ((eq? (car res) 'ok) (cdr res))
                          ((eq? (car res) 'cancelled) (raise (cdr res)))
                          (else (raise (cdr res)))))
                  (timeout timeout-ms
                    (set! done #t)
                    (task-cancel worker)
                    (error (string-append label ": timed out")))))
              (lambda ()
                (unless done
                  (guard (e (#t #f)) (task-cancel worker))))))))

    (define (run-bounded-process program args
                                 #:directory [dir "/"]
                                 #:environment [env '()]
                                 #:max-stdout [max-stdout %default-max-stdout]
                                 #:max-stderr [max-stderr %max-stderr-kept]
                                 #:timeout [timeout-ms #f]
                                 #:detach-tty [detach-tty? #t])
      "Run local executable PROGRAM with ARGS, returning a structured outcome.
Runs in DIR with environment delta ENV, bounding stdout to MAX-STDOUT
bytes, keeping up to MAX-STDERR bytes of stderr, applying TIMEOUT-MS
milliseconds (or `#f` for none), and detaching the TTY when DETACH-TTY?
is true. Returns `(ok STATUS STDOUT STDERR-HEAD)`, `(timeout)`,
`(output-limit)`, or `(error MESSAGE)` while propagating cancellation."
      (define (run-once)
        (guard (e ((task-cancelled-condition? e) (raise e))
                  (#t (list 'error (if (error-object? e)
                                       (error-object-message e)
                                       "process execution failed"))))
          (call-with-process program args
            (lambda (p)
              (let ((err-task (spawn
                               (lambda ()
                                 (guard (e ((not (task-cancelled-condition? e)) (cons "" #f)))
                                   (drain-stream-keeping p 'stderr max-stderr)))
                               #:child #t))
                    (done #f))
                (dynamic-wind
                  (lambda () #f)
                  (lambda ()
                    (let ((out-res (read-stream-bounded-tagged p 'stdout max-stdout)))
                      (if (eq? (car out-res) 'output-limit)
                          '(output-limit)
                          (let* ((err-pair (task-join err-task))
                                 (status (process-wait p)))
                            (set! done #t)
                            (list 'ok status (cdr out-res) (car err-pair))))))
                  (lambda ()
                    (unless done (guard (e (#t #f)) (task-cancel err-task)))))))
            #:directory dir
            #:environment env
            #:stdin 'null
            #:detach-tty detach-tty?)))
      (if (not timeout-ms)
          (run-once)
          (let* ((ch (make-channel 1))
                 (done #f)
                 (worker (spawn
                          (lambda ()
                            (let ((res (guard (e ((task-cancelled-condition? e)
                                                  (guard (e2 (#t #f)) (channel-send ch (cons 'cancelled e)))
                                                  (raise e))
                                                 (#t (list 'error (if (error-object? e)
                                                                      (error-object-message e)
                                                                      "process execution failed"))))
                                         (run-once))))
                              (guard (e (#t #f)) (channel-send ch res))))
                          #:child #t)))
            (dynamic-wind
              (lambda () #f)
              (lambda ()
                (select
                  (recv ch (res)
                    (set! done #t)
                    (if (and (pair? res) (eq? (car res) 'cancelled))
                        (raise (cdr res))
                        res))
                  (timeout timeout-ms
                    (set! done #t)
                    (task-cancel worker)
                    '(timeout))))
              (lambda ()
                (unless done
                  (guard (e (#t #f)) (task-cancel worker))))))))

    (define (run-raw-process program args dir env max-stdout label timeout-ms)
      (let ((res (run-bounded-process program args
                                      #:directory dir
                                      #:environment env
                                      #:max-stdout max-stdout
                                      #:timeout timeout-ms
                                      #:detach-tty #t)))
        (case (car res)
          ((ok) (cdr res))
          ((timeout) (error (string-append label ": timed out")))
          ((output-limit) (error (string-append label ": output exceeded size limit")))
          (else (error (string-append label ": " (cadr res)))))))

    (define (write-bytes-chunked p bytes)
      (let ((len (bytevector-length bytes)))
        (let loop ((offset 0))
          (when (< offset len)
            (let ((end (min len (+ offset %stdin-chunk-size))))
              (process-write p (if (and (= offset 0) (= end len))
                                   bytes
                                   (bytevector-copy bytes offset end)))
              (loop end))))))

    (define (run-raw-process-bytes program args dir env stdin-data max-stdout max-stderr strict-stderr? detach-tty? label timeout-ms)
      (with-tool-timeout
       timeout-ms label
       (lambda ()
         (call-with-process program args
           (lambda (p)
             (let ((write-task
                    (and stdin-data
                         (spawn
                          (lambda ()
                            (dynamic-wind
                              (lambda () #f)
                              (lambda ()
                                (guard (e ((task-cancelled-condition? e) (raise e))
                                          (#t (cons 'err e)))
                                  (let ((bytes (if (bytevector? stdin-data)
                                                   stdin-data
                                                   (string->utf8 stdin-data))))
                                    (when (> (bytevector-length bytes) 0)
                                      (write-bytes-chunked p bytes)))
                                  'ok))
                              (lambda ()
                                (guard (e (#t #f)) (process-close-input p)))))
                          #:child #t)))
                   (err-task (spawn
                              (lambda ()
                                (guard (e ((not (task-cancelled-condition? e)) (cons "" #f)))
                                  (drain-stream-keeping p 'stderr max-stderr)))
                              #:child #t))
                   (done #f))
               (dynamic-wind
                 (lambda () #f)
                 (lambda ()
                   (let* ((out (read-bytes-bounded p 'stdout max-stdout label))
                          (write-res (if write-task
                                         (guard (e ((task-cancelled-condition? e) (raise e))
                                                   (#t (cons 'err e)))
                                           (task-join write-task))
                                         'ok))
                          (err-pair (task-join err-task))
                          (err (car err-pair))
                          (err-trunc? (cdr err-pair))
                          (status (process-wait p)))
                     (set! done #t)
                     (when (and (eqv? status 0)
                                (pair? write-res)
                                (eq? (car write-res) 'err))
                       (let ((why (if (error-object? (cdr write-res))
                                      (error-object-message (cdr write-res))
                                      "stdin pipe write failed")))
                         (error (string-append label ": stdin delivery failed before helper exited 0: " why))))
                     (when (and strict-stderr? err-trunc?)
                       (error (string-append label ": stderr/status output exceeded size limit")))
                     (list status out err)))
                 (lambda ()
                   (unless done
                     (when write-task (guard (e (#t #f)) (task-cancel write-task)))
                     (guard (e (#t #f)) (task-cancel err-task)))))))
           #:directory dir
           #:environment env
           #:stdin (if stdin-data #f 'null)
           #:detach-tty detach-tty?))))

    ;;; Clean host baseline (unloading any inherited startup DIRENV_* overlay)

    (define %baseline-cache (make-hash-table))

    ;; Loader and shell-hook variables scrubbed before invoking the cleanup
    ;; executable (and prepended as unsets before the unload delta so any
    ;; value added by the startup overlay is cleared unless direnv restores a
    ;; pre-overlay value).
    (define %cleanup-scrub-unsets
      '(("LD_PRELOAD" . #f)
        ("LD_LIBRARY_PATH" . #f)
        ("DYLD_LIBRARY_PATH" . #f)
        ("DYLD_INSERT_LIBRARIES" . #f)
        ("DIRENV_CONFIG" . #f)
        ("BASH_ENV" . #f)
        ("ENV" . #f)
        ("ZDOTDIR" . #f)
        ("SHELLOPTS" . #f)
        ("BASHOPTS" . #f)
        ("CDPATH" . #f)
        ("IFS" . #f)))

    (define %cleanup-baseline-prefix
      (append %cleanup-scrub-unsets
              '(("PATH" . "/usr/bin:/bin"))))

    (define %cleanup-launch-env
      (append %cleanup-baseline-prefix
              '(("DIRENV_CONFIG" . "/dev/null"))))

    (define (startup-dirty-root)
      (let ((dd (get-environment-variable "DIRENV_DIR")))
        (and (string? dd)
             (> (string-length dd) 0)
             (let ((raw (if (string-prefix? "-" dd) (substring dd 1) dd)))
               (and (string-prefix? "/" raw)
                    (strip-trailing-slash (guard (e (#t raw)) (file-truename raw))))))))

    (define (parse-unload-json text)
      (let ((trimmed (string-trim (or text ""))))
        (if (string=? trimmed "")
            '()
            (let ((parsed (string->json trimmed)))
              (and (dict? parsed)
                   (filter-map
                    (lambda (k)
                      (let ((v (dict-ref parsed k 'missing)))
                        (cond ((string? v) (cons k v))
                              ((eq? v 'null) (cons k #f))
                              (else #f))))
                    (dict-keys parsed)))))))

    (define (valid-unload-delta? delta)
      (and (list? delta)
           (pair? delta)
           (let ((dd (assoc "DIRENV_DIR" delta)))
             (and dd (eq? (cdr dd) #f)))))

    (define (fallback-sanitized-baseline _dirty-root)
      (list (cons "DIRENV_DIR" #f)
            (cons "DIRENV_DIFF" #f)
            (cons "DIRENV_FILE" #f)
            (cons "DIRENV_WATCHES" #f)
            (cons "LD_PRELOAD" #f)
            (cons "LD_LIBRARY_PATH" #f)
            (cons "DYLD_LIBRARY_PATH" #f)
            (cons "DYLD_INSERT_LIBRARIES" #f)
            (cons "GNUPGHOME" #f)
            (cons "GPG_AGENT_INFO" #f)
            (cons "GPG_TTY" #f)
            (cons "XDG_CONFIG_HOME" #f)
            (cons "XDG_DATA_HOME" #f)
            (cons "XDG_CACHE_HOME" #f)
            (cons "XDG_STATE_HOME" #f)
            (cons "AGE_IDENTITIES" #f)
            (cons "SOPS_AGE_KEY" #f)
            (cons "SOPS_AGE_KEY_FILE" #f)
            (cons "SOPS_GPG_EXEC" #f)
            (cons "PATH" "/usr/bin:/bin")))

    (define (env-lookup-last env name default)
      (let loop ((xs (reverse env)))
        (cond ((null? xs) default)
              ((string=? (caar xs) name) (cdar xs))
              (else (loop (cdr xs))))))

    (define (compute-clean-baseline-entry direnv-prog)
      (let ((dd (get-environment-variable "DIRENV_DIR")))
        (if (not (and (string? dd) (> (string-length dd) 0)))
            (cons #t '())
            (let* ((dirty-root (startup-dirty-root))
                   (resolved-direnv (resolve-cleanup-executable direnv-prog dirty-root))
                   (unloaded
                    (and resolved-direnv
                         (let ((res (run-bounded-process resolved-direnv '("export" "json")
                                                         #:directory "/"
                                                         #:environment %cleanup-launch-env
                                                         #:max-stdout %default-max-stdout
                                                         #:timeout %baseline-timeout-ms
                                                         #:detach-tty #t)))
                           (and (eq? (car res) 'ok)
                                (eqv? (cadr res) 0)
                                (guard (e (#t #f))
                                  (parse-unload-json (caddr res))))))))
              (if (valid-unload-delta? unloaded)
                  ;; `direnv export json` computes its delta relative to
                  ;; `%cleanup-launch-env` (which sets PATH=/usr/bin:/bin and
                  ;; unsets loader/shell hook variables). Start from
                  ;; `%cleanup-baseline-prefix` so an omitted PATH never falls
                  ;; back to the inherited dirty PATH, and sanitize any PATH
                  ;; restored by `unloaded`.
                  (let* ((merged (append %cleanup-baseline-prefix unloaded))
                         (raw-path (env-lookup-last merged "PATH" "/usr/bin:/bin"))
                         (clean-path (if (string? raw-path)
                                         (filter-safe-path raw-path dirty-root)
                                         #f)))
                    (cons #t (append merged (list (cons "PATH" clean-path)))))
                  (cons #f (fallback-sanitized-baseline dirty-root)))))))

    (define (baseline-entry direnv-prog)
      (let* ((dd (or (get-environment-variable "DIRENV_DIR") ""))
             (prog (or direnv-prog (trusted-direnv-program-hook) "direnv"))
             (key (string-append dd "\x0;" prog)))
        (if (string=? dd "")
            (cons #t '())
            (or (hash-table-ref/default %baseline-cache key #f)
                (let ((entry (compute-clean-baseline-entry prog)))
                  (hash-table-set! %baseline-cache key entry)
                  entry)))))

    (define (trusted-baseline-clean? #:direnv-program [direnv-prog #f])
      "Return #t when a clean host baseline is established.
Returns #t when Techne did not start inside an active `direnv` shell or
when `direnv export json` via DIRENV-PROG (resolved from trusted system
directories or an explicit user-configured absolute path, launched with a
scrubbed environment) cleanly unloaded the startup overlay; returns #f
when the fallback sanitizer had to be used."
      (car (baseline-entry direnv-prog)))

    (define (clone-env-delta delta)
      (map (lambda (entry)
             (cons (string-copy (car entry))
                   (and (string? (cdr entry)) (string-copy (cdr entry)))))
           delta))

    (define (trusted-baseline-environment #:direnv-program [direnv-prog #f])
      "Return the `(NAME . VALUE-OR-#f)` delta restoring a clean host env.
Returns `'()` when Techne did not start inside an active `direnv` shell;
otherwise computes the unload delta once using DIRENV-PROG (or the
registered direnv program hook), falling back to stripping `DIRENV_*`,
loader variables, sensitive config vars, and project `PATH` entries."
      (clone-env-delta (cdr (baseline-entry direnv-prog))))

    (define (trusted-environment-variable name #:direnv-program [direnv-prog #f])
      "Return environment variable NAME from a clean host baseline, or #f.
Uses DIRENV-PROG (when given) to establish the clean baseline, returning
#f if NAME is unset or if a startup `direnv` overlay could not be cleanly
unloaded."
      (let ((entry (baseline-entry direnv-prog)))
        (and (car entry)
             (let ((v (env-lookup-last (cdr entry) name (get-environment-variable name))))
               (and (string? v) (string-copy v))))))

    (define (trusted-baseline-path #:direnv-program [direnv-prog #f])
      "Return the sanitized host `PATH` string with project entries removed.
Consults `trusted-baseline-environment` with DIRENV-PROG and strips
empty, relative, or startup-project `PATH` components."
      (let* ((delta (trusted-baseline-environment #:direnv-program direnv-prog))
             (raw (env-lookup-last delta "PATH" (or (get-environment-variable "PATH") ""))))
        (filter-safe-path (if (string? raw) raw "") (startup-dirty-root))))

    (define (resolve-trusted-program program #:direnv-program [direnv-prog #f]
                                     #:strict-baseline [strict-baseline? #t])
      "Resolve trusted executable PROGRAM against the clean host `PATH`.
Rejects relative paths containing `/` and executables whose canonical target
lies under an inherited startup `DIRENV_DIR` project root (including symlinks
on `PATH`). When STRICT-BASELINE? is true (the default), returns #f if an
inherited startup `DIRENV_DIR` overlay could not be cleanly unloaded.
Returns the validated executable path or #f if bare PROGRAM is not found on
`trusted-baseline-path` (honoring DIRENV-PROG for baseline cleanup)."
      (unless (and (string? program) (> (string-length program) 0))
        (error "trusted-process: program must be a non-empty string" program))
      (let ((dirty-root (startup-dirty-root))
            (entry (baseline-entry direnv-prog)))
        (cond
          ((and strict-baseline? (not (car entry))) #f)
          ((string-prefix? "/" program)
           (resolve-safe-executable program dirty-root))
          ((string-contains program "/")
           (error "trusted-process: relative executable path is not allowed" program))
          (else
           (find-executable-in-path program (trusted-baseline-path #:direnv-program direnv-prog) dirty-root)))))

    (define (trusted-home)
      "Return the user's trusted absolute HOME directory, or #f.
Reads from a verified clean host baseline (`trusted-baseline-clean?`),
returns #f if an inherited startup `DIRENV_DIR` overlay could not be
cleanly unloaded, and never falls back to `.`."
      (let ((h (trusted-environment-variable "HOME")))
        (and (string? h)
             (> (string-length h) 0)
             (string-prefix? "/" h)
             (file-exists? h)
             (let ((real (guard (e (#t #f)) (file-truename h))))
               (and real h)))))

    (define (expand-trusted-home path)
      "Expand a leading `~` or `~/` in PATH using `trusted-home`.
Raise an error if PATH uses `~` while HOME is unset, invalid, or
unavailable because a startup `direnv` overlay could not be unloaded."
      (cond ((string=? path "~")
             (or (trusted-home) (error "trusted-process: trusted HOME is unavailable")))
            ((string-prefix? "~/" path)
             (let ((h (or (trusted-home) (error "trusted-process: trusted HOME is unavailable"))))
               (string-append h (substring path 1))))
            (else path)))

    (define (trusted-cwd)
      "Return a trusted working directory for local credential helpers.
Uses `trusted-home` when available, otherwise `/`."
      (or (trusted-home) "/"))

    (define (run-trusted-tool program args #:environment [environment '()]
                              #:max-stdout [max-stdout %default-max-stdout]
                              #:timeout [timeout #f]
                              #:strict-baseline [strict-baseline? #t]
                              #:label [label #f])
      "Run local helper PROGRAM with ARGS in a trusted directory.
Returns `(STATUS STDOUT STDERR-HEAD)`. Resolves PROGRAM against the
clean host `PATH` (rejecting relative paths containing `/`) and applies
`trusted-baseline-environment` before ENVIRONMENT overrides. When
STRICT-BASELINE? is true (the default), fails closed if Techne started
inside an active `direnv` shell whose baseline could not be cleanly
unloaded. MAX-STDOUT bounds stdout (default 1 MiB), TIMEOUT bounds
runtime in milliseconds, and LABEL names the helper in errors."
      (let* ((tag (or label program))
             (entry (baseline-entry #f)))
        (when (and strict-baseline? (not (car entry)))
          (error (string-append tag ": refusing to run with uncleaned startup direnv environment")))
        (let* ((resolved (or (resolve-trusted-program program #:strict-baseline strict-baseline?)
                             (error (string-append tag ": executable not found") program)))
               (env (append (cdr entry) (list (cons "GPG_TTY" #f)) environment)))
          (run-raw-process resolved args (trusted-cwd) env max-stdout tag timeout))))

    (define (run-trusted-tool-bytes program args #:stdin [stdin #f]
                                    #:environment [environment '()]
                                    #:max-stdout [max-stdout %default-max-stdout]
                                    #:max-stderr [max-stderr %max-stderr-kept]
                                    #:strict-stderr [strict-stderr? #f]
                                    #:strict-baseline [strict-baseline? #t]
                                    #:detach-tty [detach-tty? #t]
                                    #:timeout [timeout %default-bytes-timeout-ms]
                                    #:label [label #f])
      "Run local helper PROGRAM with ARGS using binary stdin/stdout.
Returns `(STATUS STDOUT-BYTEVECTOR STDERR-HEAD)`. Optional STDIN may be
`#f` (null stdin), a bytevector, or a UTF-8 string; it is fed concurrently
with reading stdout and stderr to avoid pipe deadlocks on large payloads,
and pipe-delivery failures (such as `EPIPE` on a closed stdin) are surfaced
even if the helper exits 0. Callers/codecs remain responsible for validating
output framing, checksums, and exit status. Resolves PROGRAM against the
clean host `PATH` (rejecting relative paths containing `/`) and applies
`trusted-baseline-environment` before ENVIRONMENT overrides. When
STRICT-BASELINE? is true (the default), fails closed if startup `DIRENV_DIR`
could not be cleanly unloaded. When STRICT-STDERR? is true, raises if stderr
exceeds MAX-STDERR. DETACH-TTY? (default #t) runs in a detached session
(`setsid`) without a controlling terminal and unsets `GPG_TTY`. MAX-STDOUT
bounds stdout (default 1 MiB), MAX-STDERR bounds captured stderr (default
4 KiB), TIMEOUT bounds runtime in milliseconds (default 30000; pass `#f`
to disable), and LABEL names the helper in errors."
      (unless (or (not stdin) (bytevector? stdin) (string? stdin))
        (error "run-trusted-tool-bytes: stdin must be #f, a bytevector, or a string"))
      (unless (and (integer? max-stdout) (>= max-stdout 0))
        (error "run-trusted-tool-bytes: max-stdout must be a non-negative integer" max-stdout))
      (unless (and (integer? max-stderr) (>= max-stderr 0))
        (error "run-trusted-tool-bytes: max-stderr must be a non-negative integer" max-stderr))
      (let* ((tag (or label program))
             (entry (baseline-entry #f)))
        (when (and strict-baseline? (not (car entry)))
          (error (string-append tag ": refusing to run with uncleaned startup direnv environment")))
        (let* ((resolved (or (resolve-trusted-program program #:strict-baseline strict-baseline?)
                             (error (string-append tag ": executable not found") program)))
               (env (append (cdr entry)
                            (if detach-tty? (list (cons "GPG_TTY" #f)) '())
                            environment)))
          (run-raw-process-bytes resolved args (trusted-cwd) env stdin
                                 max-stdout max-stderr strict-stderr? detach-tty? tag timeout))))))
