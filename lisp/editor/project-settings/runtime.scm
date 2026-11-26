;;; Interactive project-settings commands, loaded lazily on first
;;; invocation via `register-command-package!` in
;;; `lisp/editor/project-settings.scm`.

(import (techne editor) (techne project-settings) (techne editorconfig))
(require "../project-settings-bridge.scm")
(require "../project-settings.scm")
(require "../modes.scm")
(require "../files.scm")
(require "../buffers.scm")
(require "../projects.scm")
(require "../minibuffer.scm")
(require "../views.scm")
(require "../operations.scm")
(require "../remote.scm")

(ensure-project-settings!)

(define (ensure-slash dir)
  (if (string-suffix? "/" dir) dir (string-append dir "/")))

(define (hex-byte n)
  (let ((digits "0123456789abcdef"))
    (string (string-ref digits (quotient n 16))
            (string-ref digits (remainder n 16)))))

(define (sanitize-display-text s)
  (if (not (string? s))
      ""
      (call-with-output-string
       (lambda (p)
         (for-each
          (lambda (ch)
            (let ((cp (char->integer ch)))
              (cond
                ((= cp 10) (display "\\n" p))
                ((= cp 13) (display "\\r" p))
                ((= cp 9) (display "\\t" p))
                ((or (< cp 32) (= cp 127) (and (>= cp 128) (<= cp 159)))
                 (display "\\x" p)
                 (display (hex-byte cp) p)
                 (display ";" p))
                (else (display ch p)))))
          (string->list s))))))

(define (format-settings-value val [max-len #f])
  (define (emit v p)
    (cond
      ((json-unset? v) (display "(unset)" p))
      ((dict? v)
       (display "(dict" p)
       (for-each (lambda (k)
                   (display " " p)
                   (write k p)
                   (display " " p)
                   (emit (dict-ref v k) p))
                 (sort (dict-keys v) string<?))
       (display ")" p))
      ((vector? v)
       (display "#(" p)
       (let ((n (vector-length v)))
         (let loop ((i 0))
           (when (< i n)
             (when (> i 0) (display " " p))
             (emit (vector-ref v i) p)
             (loop (+ i 1)))))
       (display ")" p))
      ((pair? v)
       (display "(" p)
       (let loop ((xs v) (first? #t))
         (cond
           ((null? xs) (display ")" p))
           ((pair? xs)
            (unless first? (display " " p))
            (emit (car xs) p)
            (loop (cdr xs) #f))
           (else
            (display " . " p)
            (emit xs p)
            (display ")" p)))))
      (else (write v p))))
  (let ((s (sanitize-display-text (call-with-output-string (lambda (p) (emit val p))))))
    (if (and max-len (> (string-length s) max-len))
        (string-append (substring s 0 (max 0 (- max-len 3))) "...")
        s)))

(define (outermost-dir-for-reload dir prev-snap new-snap)
  (let ((d (ensure-slash dir))
        (best (ensure-slash dir)))
    (define (consider cand)
      (when (and (string? cand) (not (remote-path? cand)))
        (let ((c (ensure-slash cand)))
          (when (and (or (string=? c d) (string-prefix? c d))
                     (< (string-length c) (string-length best)))
            (set! best c)))))
    (for-each
     (lambda (snap)
       (when snap
         (for-each (lambda (tf) (consider (project-settings-file-directory tf)))
                   (project-settings-snapshot-techne-files snap))
         (for-each (lambda (ec) (consider (editorconfig-file-directory ec)))
                   (project-settings-snapshot-editorconfig-files snap))))
     (list prev-snap new-snap))
    (guard (e (#t #f)) (consider (project-root d)))
    best))

(define (current-settings-target s)
  (let* ((b (current-buffer s))
         (bdir (local-dir-of-buffer b))
         (dir (or bdir (let ((d (default-directory s))) (and (string? d) (not (remote-path? d)) (ensure-slash d)))))
         (d (and b (buffer-document b)))
         (raw-path (and d (document-path d)))
         (path (and d (document-logical-path d)))
         (modes (if b (map mode-name (mode-chain (buffer-mode b))) '())))
    (values dir raw-path path modes)))

(define (collect-pending-for-target snap raw-path path modes)
  (if (not snap)
      '()
      (if (techne-settings-file-path? raw-path)
          (project-settings-collect-all-pending snap editor-option-meta)
          (project-settings-collect-pending snap path modes editor-option-meta))))

(define (status-rows dir raw-path path modes snap)
  (let* ((tfs (if snap (project-settings-snapshot-techne-files snap) '()))
         (ecs (if snap (project-settings-snapshot-editorconfig-files snap) '()))
         (diags (if snap (project-settings-snapshot-diagnostics snap) '()))
         (pending (collect-pending-for-target snap raw-path path modes)))
    (append
     (list (row "directory" (sanitize-display-text (abbreviate-path dir))))
     (if (null? tfs)
         (list (row ".techne.scm" "(none)"))
         (map (lambda (tf)
                (row ".techne.scm"
                     (string-append (sanitize-display-text (abbreviate-path (project-settings-file-path tf)))
                                    (if (project-settings-file-root? tf) "  [root]" ""))))
              tfs))
     (if (null? ecs)
         (list (row ".editorconfig" "(none)"))
         (map (lambda (ec)
                (row ".editorconfig"
                     (string-append (sanitize-display-text (abbreviate-path (editorconfig-file-path ec)))
                                    (if (editorconfig-file-root? ec) "  [root]" ""))))
              ecs))
     (if (null? pending)
         (list (row "trust" "no untrusted settings pending"))
         (map (lambda (p)
                (row (string-append "pending (" (symbol->string (list-ref p 4)) ")")
                     (string-append (symbol->string (list-ref p 1))
                                    " = " (format-settings-value (list-ref p 2) #f)
                                    "  [" (sanitize-display-text (abbreviate-path (list-ref p 0))) "]")))
              pending))
     (map (lambda (d)
            (row (symbol->string (settings-diagnostic-severity d))
                 (string-append (sanitize-display-text (abbreviate-path (settings-diagnostic-path d))) ": "
                                (sanitize-display-text (settings-diagnostic-message d)))))
          diags))))

(define-command (project-settings-status s n)
  "Show discovered `.techne.scm` and `.editorconfig` files and trust state.
Displays active project-setting files, any pending untrusted values, and
parse or validation diagnostics for the current directory."
  (let-values (((dir raw-path path modes) (current-settings-target s)))
    (unless dir (error "Project settings are not supported for remote directories"))
    (project-settings-load! dir)
    (project-settings-refresh-display!)
    (show-view! s "*project-settings*"
                (lambda (s)
                  (let ((snap (project-settings-load! dir)))
                    (status-rows dir raw-path path modes snap))))))

(define (confirm-pending-entry! s entry)
  (let* ((scope (list-ref entry 0))
         (opt (list-ref entry 1))
         (val (list-ref entry 2))
         (src (list-ref entry 3))
         (verdict (list-ref entry 4))
         (scope-label (sanitize-display-text (abbreviate-path scope)))
         (full-val (format-settings-value val #f))
         (prompt (string-append "Trust " (symbol->string opt) " [" (symbol->string verdict) "] = "
                                full-val
                                " in " scope-label "? ")))
    (completing-read
     s prompt '("cancel" "approve" "deny" "visit")
     #:accept
     (scope-procedure
      (lambda (s c)
        (let ((choice (candidate-text c)))
          (cond
            ((string=? choice "approve")
             (note-effect! 'settings)
             (%interactive-approve! scope opt val)
             (message! s (string-append "Trusted " (symbol->string opt) " in " scope-label)))
            ((string=? choice "deny")
             (note-effect! 'settings)
             (%interactive-deny! scope opt val)
             (message! s (string-append "Denied " (symbol->string opt) " in " scope-label)))
            ((string=? choice "visit")
             (file-visit! s src))
            (else
             (message! s "Project settings trust cancelled")))))))))

(define-command (project-settings-trust s n)
  "Review and approve or deny pending untrusted project settings.
Prompts with the exact option name, scope directory, and full escaped value
before recording a value-based trust verdict."
  (let-values (((dir raw-path path modes) (current-settings-target s)))
    (unless dir (error "Project settings are not supported for remote directories"))
    (let* ((snap (project-settings-load! dir))
           (pending (collect-pending-for-target snap raw-path path modes)))
      (cond
        ((null? pending)
         (message! s "No untrusted project settings pending here"))
        ((= (length pending) 1)
         (confirm-pending-entry! s (car pending)))
        (else
         (let ((cands (map (lambda (p)
                             (candidate
                              (string-append (symbol->string (list-ref p 1))
                                             " (" (symbol->string (list-ref p 4)) ") = "
                                             (format-settings-value (list-ref p 2) #f)
                                             " [" (sanitize-display-text (abbreviate-path (list-ref p 0))) "]")
                              #:target p))
                           pending)))
           (completing-read
            s "Review project setting: "
            cands
            #:accept
            (scope-procedure
             (lambda (s c)
               (let ((entry (candidate-target c)))
                 (when entry (confirm-pending-entry! s entry))))))))))))

(define-command (project-settings-revoke s n)
  "Revoke stored trust verdicts for project settings.
Revokes trust for the selected `(scope, option)` entry and immediately
refreshes display caches and live language server configuration."
  (let* ((entries (project-trust-entries)))
    (if (null? entries)
        (message! s "No stored project-setting trust verdicts")
        (let ((cands (map (lambda (e)
                            (let* ((digest (caddr e))
                                   (short (substring digest 0 (min 8 (string-length digest)))))
                              (candidate
                               (string-append (symbol->string (cadr e))
                                              " (" (symbol->string (cadddr e)) " " short ") in "
                                              (sanitize-display-text (abbreviate-path (car e))))
                               #:target e)))
                          entries)))
          (completing-read
           s "Revoke project setting trust: "
           cands
           #:accept
           (scope-procedure
            (lambda (s c)
              (let ((e (candidate-target c)))
                (when e
                  (note-effect! 'settings)
                  (%interactive-revoke! (car e) (cadr e))
                  (message! s (string-append "Revoked trust for " (symbol->string (cadr e))
                                             " in " (sanitize-display-text (abbreviate-path (car e))))))))))))))

(define-command (project-settings-reload s n)
  "Reload `.techne.scm` and `.editorconfig` for the current directory.
Invalidates and reloads cached snapshots (including ancestors), buffer
display options, and live language server configuration."
  (let-values (((dir raw-path path modes) (current-settings-target s)))
    (unless dir (error "Project settings are not supported for remote directories"))
    (let ((prev-snap (project-settings-cached-snapshot dir)))
      (project-settings-invalidate-ancestors! dir)
      (let* ((snap (project-settings-load! dir))
             (reload-root (outermost-dir-for-reload dir prev-snap snap)))
        (reload-open-buffers-under! reload-root)
        (project-settings-refresh-display!)
        (notify-project-settings-changed! reload-root)
        (let ((d-slash (ensure-slash dir)))
          (unless (string=? d-slash reload-root)
            (notify-project-settings-changed! d-slash)))
        (let ((n-tf (length (project-settings-snapshot-techne-files snap)))
              (n-ec (length (project-settings-snapshot-editorconfig-files snap)))
              (n-err (length (project-settings-snapshot-diagnostics snap))))
          (message! s (string-append "Reloaded project settings: "
                                     (number->string n-tf) " .techne.scm, "
                                     (number->string n-ec) " .editorconfig"
                                     (if (> n-err 0)
                                         (string-append " (" (number->string n-err) " errors)")
                                         ""))))))))
