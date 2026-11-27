;;; Rich target actions for files, directories, buffers, URLs, and regions.
;;; Loaded lazily on first `actions-for` lookup to keep editor startup lean.
(require "session.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "files.scm")
(require "dispatch.scm")
(require "operations.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "views.scm")
(require "lens.scm")

(define (confirm-yes-no! s prompt on-yes)
  ;; Default to "no" first so accidental RET never performs a destructive action.
  (completing-read s prompt '("no" "yes")
                   #:require-match #t
                   #:accept (lambda (s c)
                              (if (string=? (candidate-text c) "yes")
                                  (on-yes s)
                                  (message! s "Cancelled")))))

(define (read-file-text path)
  (call-with-input-file path
    (lambda (p)
      (let loop ((acc '()))
        (let ((chunk (read-string 65536 p)))
          (if (eof-object? chunk)
              (apply string-append (reverse acc))
              (loop (cons chunk acc))))))))

(define (relative-to-dir dir path)
  (let* ((d (strip-trailing-slashes (absolute-path dir)))
         (base (if (string=? d "/") d (string-append d "/")))
         (abs-path (absolute-path path)))
    (if (string-prefix? base abs-path)
        (substring abs-path (string-length base) (string-length abs-path))
        abs-path)))

(define (directory-name? path) (string-suffix? "/" path))

(define (strip-trailing-slashes path)
  (let loop ((s path))
    (if (and (> (string-length s) 1) (char=? (string-ref s (- (string-length s) 1)) #\/))
        (loop (substring s 0 (- (string-length s) 1)))
        s)))

(define-action file (delete-file-target s path)
  #:key "d" #:interactive #t
  "Delete the file after confirmation."
  (confirm-yes-no! s (string-append "Delete " (abbreviate-path path) "? ")
                   (lambda (s)
                     (delete-path-and-buffers! s path #:recursive #f)
                     (message! s (string-append "Deleted " (abbreviate-path path))))))
(define-action file (rename-file-target s path)
  #:key "r" #:interactive #t
  "Rename or move the file to a new path."
  (let ((abs (absolute-path path)))
    (read-file-name s (string-append "Rename " (file-name abs) " to: ")
                    (abbreviate-path (directory-of abs))
                    (lambda (s dest)
                      (let ((target-path (if (directory-name? dest) (string-append dest (file-name abs)) dest)))
                        (move-path! abs target-path)
                        (message! s (string-append "Renamed to " (abbreviate-path target-path))))))))
(define-action file (copy-file-target s path)
  #:key "c" #:interactive #t
  "Copy the file to a new path."
  (let ((abs (absolute-path path)))
    (read-file-name s (string-append "Copy " (file-name abs) " to: ")
                    (abbreviate-path (directory-of abs))
                    (lambda (s dest)
                      (let ((target-path (if (directory-name? dest) (string-append dest (file-name abs)) dest)))
                        (copy-file abs target-path)
                        (message! s (string-append "Copied to " (abbreviate-path target-path))))))))
(define-action file (visit-file-other-pane s path) #:key "o" "Open the file in a new pane below." (file-visit! s path #:other-pane #t))
(define-action file (copy-file-name s path) #:key "w" "Save the file's path as a kill." (kill-save! s (absolute-path path) #f #f))

(define-action buffer (show-buffer-other-pane s b) #:key "o" "Show the buffer in a new pane below." (show-in-other-pane! s (buffer-document b)))
(define-action buffer (kill-buffer-target s b) #:key "k" "Take the buffer off the buffer list." (drop-buffer! s b))
(define-action buffer (copy-buffer-name s b) #:key "w" "Save the buffer's name as a kill." (kill-save! s (buffer-name b) #f #f))

(define-action file (copy-relative-file-name s path)
  #:key "W"
  "Save the file's path relative to the current directory as a kill."
  (kill-save! s (relative-to-dir (default-directory s) path) #f #f))
(define-action file (insert-file-target s path)
  #:key "i"
  "Insert the file's contents at point."
  (let* ((abs (absolute-path path))
         (text (read-file-text abs)))
    (insert-text! s text "new")
    (message! s (string-append "Inserted " (file-name abs)))))
(define-action file (insert-file-name s path)
  #:key "I"
  "Insert the file's path at point."
  (insert-text! s (abbreviate-path (absolute-path path)) "new"))
(define-action file (make-directory-target s path)
  #:key "+" #:interactive #t
  "Create a new directory alongside the file."
  (completing-read s "Create directory: " '()
                   #:initial (abbreviate-path (directory-of (absolute-path path)))
                   #:require-match #f
                   #:accept (lambda (s c)
                              (let ((dir (absolute-path (candidate-text c))))
                                (make-directory dir)
                                (message! s (string-append "Created " (abbreviate-path dir)))))))
(define-action file (symlink-file-target s path)
  #:key "s" #:interactive #t
  "Create a symbolic link pointing to the file."
  (let ((abs (absolute-path path)))
    (read-file-name s (string-append "Symlink " (file-name abs) " at: ")
                    (abbreviate-path (directory-of abs))
                    (lambda (s dest)
                      (let* ((link-path (if (directory-name? dest) (string-append dest (file-name abs)) dest))
                             (status (call-with-process "ln" (list "-s" abs link-path)
                                                        (lambda (p) (process-wait p)))))
                        (if (= status 0)
                            (message! s (string-append "Linked " (abbreviate-path link-path) " -> " (file-name abs)))
                            (error "Failed to create symbolic link" link-path status)))))))
(define-action file (open-file-externally s path)
  #:key "x"
  "Open the file with the system's default application."
  (let ((abs (absolute-path path)))
    (call-with-process "xdg-open" (list abs) (lambda (_p) #t) #:persist #t)
    (message! s (string-append "Opened externally: " (file-name abs)))))
(define-action file (shell-command-on-file s path)
  #:key "!" #:interactive #t
  "Run a shell command with the file's path as `$1`."
  (let ((abs (absolute-path path)))
    (completing-read s (string-append "Shell command on " (file-name abs) ": ") '()
                     #:require-match #f
                     #:accept (lambda (s c)
                                (let ((cmd (candidate-text c)))
                                  (call-with-process "sh" (list "-c" cmd "--" abs)
                                                     (lambda (p)
                                                       (let* ((out (process-read-all p 'stdout))
                                                              (status (process-wait p))
                                                              (trimmed (string-trim out)))
                                                         (if (= status 0)
                                                             (message! s (if (string=? trimmed "") "Done" trimmed))
                                                             (error "Shell command failed" status trimmed))))
                                                     #:directory (directory-of abs)
                                                     #:stdout 'pipe))))))

(define-action directory (insert-directory-name s path)
  #:key "I"
  "Insert the directory's path at point."
  (insert-text! s (abbreviate-path path) "new"))
(define-action directory (rename-directory-target s path)
  #:key "r" #:interactive #t
  "Rename or move the directory."
  (let* ((clean (strip-trailing-slashes (absolute-path path)))
         (parent (directory-of clean)))
    (read-file-name s (string-append "Rename " (file-name clean) " to: ")
                    (abbreviate-path parent)
                    (lambda (s dest)
                      (let ((target-path (if (directory-name? dest) (string-append dest (file-name clean)) dest)))
                        (move-path! clean target-path)
                        (message! s (string-append "Renamed to " (abbreviate-path target-path))))))))
(define-action directory (delete-directory-target s path)
  #:key "D" #:interactive #t
  "Delete the directory and its contents after confirmation."
  (confirm-yes-no! s (string-append "Delete directory " (abbreviate-path path) "? ")
                   (lambda (s)
                     (delete-path-and-buffers! s path #:recursive #t)
                     (message! s (string-append "Deleted " (abbreviate-path path))))))
(define-action directory (make-directory-in s path)
  #:key "+" #:interactive #t
  "Create a subdirectory inside this directory."
  (completing-read s "Create directory: " '()
                   #:initial (abbreviate-path (normal-directory path))
                   #:require-match #f
                   #:accept (lambda (s c)
                              (let ((dir (absolute-path (candidate-text c))))
                                (make-directory dir)
                                (message! s (string-append "Created " (abbreviate-path dir)))))))

(define-action buffer (save-buffer-document s b)
  #:key "s"
  "Write the buffer to its file."
  (save-document-in-session! s (buffer-document b) 'save-buffer-document))
(define-action buffer (rename-buffer-target s b)
  #:key "r" #:interactive #t
  "Rename the buffer."
  (completing-read s (string-append "Rename buffer " (buffer-name b) " to: ") '()
                   #:initial (buffer-name b)
                   #:require-match #f
                   #:accept (lambda (s c)
                              (let ((n (string-trim (candidate-text c))))
                                (unless (string=? n "")
                                  (set-buffer-name! b n)
                                  (message! s (string-append "Renamed buffer to " n)))))))
(define-action buffer (insert-buffer-target s b)
  #:key "i"
  "Insert the buffer's text at point."
  (insert-text! s (document-string (buffer-document b)) "new")
  (message! s (string-append "Inserted " (buffer-name b))))

(define-action url (open-url-externally s url)
  #:key "b"
  "Open the URL with the system's default browser."
  (call-with-process "xdg-open" (list url) (lambda (_p) #t) #:persist #t)
  (message! s (string-append "Opening " url)))
(define-action url (copy-url s url)
  #:key "w"
  "Save the URL as a kill."
  (kill-save! s url #f #f)
  (message! s (string-append "Copied " url)))

(define (resolve-region-span s r)
  ;; r = (doc rev from to [captured-text]); validates document identity, maps past outside edits,
  ;; and rejects regions whose captured text changed.
  (let* ((d (list-ref r 0))
         (rev (list-ref r 1))
         (from0 (list-ref r 2))
         (to0 (list-ref r 3))
         (captured (and (> (length r) 4) (list-ref r 4))))
    (unless (document=? (doc s) d)
      (error "Region belongs to another buffer"))
    (let ((from (document-map-position d from0 rev))
          (to (document-map-position d to0 rev)))
      (unless (and from to (<= from to))
        (error "Region is no longer valid"))
      (when (and captured (not (string=? (document-substring d from to) captured)))
        (error "Region text changed since capture"))
      (values d from to))))

(define-action region (copy-region-target s r)
  #:key "w"
  "Save the region text as a kill."
  (let-values (((d from to) (resolve-region-span s r)))
    (kill-save! s (document-substring d from to) #f #f)
    (message! s "Copied region")))
(define-action region (upcase-region-target s r)
  #:key "u"
  "Convert the region text to uppercase."
  (let-values (((d from to) (resolve-region-span s r)))
    (edit! s (list (list from to (string-upcase (document-substring d from to)))) "new")))
(define-action region (downcase-region-target s r)
  #:key "l"
  "Convert the region text to lowercase."
  (let-values (((d from to) (resolve-region-span s r)))
    (edit! s (list (list from to (string-downcase (document-substring d from to)))) "new")))
(define-action region (kill-region-target s r)
  #:key "k"
  "Kill the region text."
  (let-values (((d from to) (resolve-region-span s r)))
    (let ((txt (document-substring d from to)))
      (kill-save! s txt #f #f)
      (edit! s (list (list from to "")) "new"))))
(define-action region (count-region-target s r)
  #:key "="
  "Count lines, words, and characters in the region."
  (let-values (((d from to) (resolve-region-span s r)))
    (let* ((txt (document-substring d from to))
           (lines (length (string-split txt "\n")))
           (words (let loop ((i 0) (in-word? #f) (count 0))
                    (if (>= i (string-length txt))
                        count
                        (let ((ws? (char-whitespace? (string-ref txt i))))
                          (loop (+ i 1) (not ws?) (if (and (not ws?) (not in-word?)) (+ count 1) count))))))
           (chars (string-length txt)))
      (message! s (string-append (number->string lines) " lines, "
                                 (number->string words) " words, "
                                 (number->string chars) " chars")))))

(define-command (target-collect-quit s n)
  "Close the `*Targets*` collect buffer."
  (let ((b (current-buffer s)))
    (when b (drop-buffer! s b))))

(define-mode target-collect-mode
  "Interactive list of collected targets.
Press \\[act-default-at-point] to run the default action or
\\[act-at-point] to pick an action on the target at point."
  #:parent 'rows-mode
  #:keys '(("RET" act-default-at-point)
           ("C-;" act-at-point)
           ("C-." act-at-point)
           ("C-c a" act-at-point)
           ("n" next-line)
           ("p" previous-line)
           ("q" target-collect-quit))
  #:normal '(("RET" act-default-at-point)
             ("a" act-at-point)
             ("j" next-line)
             ("k" previous-line)
             ("q" target-collect-quit)))

(define (collect-targets-view! s title cands total)
  (let* ((shown (length cands))
         (base-rows (let loop ((cs cands) (i 0) (acc '()))
                      (if (null? cs)
                          (reverse acc)
                          (let* ((c (car cs))
                                 (ann (candidate-annotation c))
                                 (t (candidate-target c))
                                 (runs (if (string=? ann "")
                                           (list (candidate-text c))
                                           (list (string-append (candidate-text c) "  ")
                                                 (list ann 'comment)))))
                            (loop (cdr cs) (+ i 1)
                                  (cons (row #:key (number->string i) #:target t runs) acc))))))
         (rows (if (> total shown)
                   (append base-rows
                           (list (row #:key "more"
                                      (list (list (string-append "… and " (number->string (- total shown))
                                                                 " more (narrow first to collect all)")
                                                  'comment)))))
                   base-rows)))
    (show-view! s title (lambda (_s) rows) #:mode 'target-collect-mode)
    (message! s (if (> total shown)
                    (string-append "Collected " (number->string shown) " of " (number->string total) " targets")
                    (string-append "Collected " (number->string shown) " targets")))))

(define %collect-limit 200)

(define (run-action-in-operation! s action targets)
  (let ((body (lambda ()
                (guard (e (#t (failed! s (action-name action) e)))
                  (for-each (lambda (t) (run-action! s action t)) targets)))))
    (if (current-operation)
        (body)
        (call-in-operation (start-operation! (action-name action) s) body))))

(define (release-action-op! op hold cancel?)
  (when op
    (when cancel? (operation-cancelled! op))
    (when hold (operation-release! op hold))))

(define (run-with-action-op! op hold thunk)
  (if op
      (dynamic-wind
       (lambda () #f)
       (lambda () (call-in-operation op thunk))
       (lambda () (when hold (operation-release! op hold))))
      (thunk)))

(define (act-on-completing-read! s targets name actions)
  (completing-read s (string-append "Act on " name ": ")
                   (map (lambda (a)
                          (let ((k (action-key a))
                                (doc (or (action-doc a) "")))
                            (candidate (symbol->string (action-name a))
                                       #:annotation (if k (string-append "[" k "] " doc) doc)
                                       #:target a)))
                        actions)
                   #:accept (lambda (s c)
                              (run-action-in-operation! s (candidate-target c) targets))))

(define (cut-action-hint text)
  (if (> (string-length text) 27)
      (string-append (substring text 0 26) "…")
      text))

(define (action-prompt-hints actions mb-ctx)
  (let* ((reserved (append (list "RET" "?" "C-h" "C-g" "ESC")
                           (if mb-ctx (list "E" "S" "A") '())))
         (default-entry (if (pair? actions)
                            (list (list "RET" (cut-action-hint (symbol->string (action-name (car actions)))) #f))
                            '()))
         (keyed
          (let loop ((as actions) (seen reserved) (acc '()))
            (if (null? as)
                (sort acc (lambda (a b) (string<? (string-downcase (car a)) (string-downcase (car b)))))
                (let* ((a (car as))
                       (k (action-key a)))
                  (if (and k (not (member k seen)))
                      (loop (cdr as) (cons k seen)
                            (cons (list k (cut-action-hint (symbol->string (action-name a))) #f) acc))
                      (loop (cdr as) seen acc))))))
         (meta (if mb-ctx
                   (list (list "E" "export" #f)
                         (list "S" "collect" #f)
                         (list "A" "act-all" #f))
                   '())))
    (append default-entry keyed meta (list (list "?" "search actions" #f) (list "C-g" "cancel" #f)))))

(define (clear-action-prompt! s)
  (sset! s 'action-prompt #f)
  (sset! s 'action-prompt-hints #f)
  (when (eq? (sget s 'transient) action-prompt-key)
    (sset! s 'transient #f))
  (when (eq? (sget s 'which-key) 'action-prompt)
    (sset! s 'which-key #f)))

(define (open-action-prompt*! s targets name actions mb-ctx msg #:op [existing-op #f] #:hold [existing-hold #f])
  ;; Invalidate any pending prefix which-key timer before showing action hints.
  (sset! s 'which-key-token (+ 1 (or (sget s 'which-key-token) 0)))
  (let* ((op (or existing-op (current-operation)))
         (ap #f)
         (hold (or existing-hold
                   (and op (operation-hold! op #:cancel (lambda ()
                                                          (when (eq? (sget s 'action-prompt) ap)
                                                            (clear-action-prompt! s))))))))
    (set! ap (list targets name actions mb-ctx op hold))
    (sset! s 'action-prompt ap)
    (sset! s 'action-prompt-hints (lambda () (action-prompt-hints actions mb-ctx)))
    (sset! s 'transient action-prompt-key)
    (sset! s 'which-key 'action-prompt)
    (message! s msg)))

(define (batch-targets-and-actions* cands)
  (let ((ts (filter target? (map candidate-target cands))))
    (cond
      ((null? ts) "No targets to act on")
      (else
       (let ((kind (target-kind (car ts))))
         (if (not (every (lambda (t) (eq? (target-kind t) kind)) ts))
             "Batch act-all requires targets of the same kind"
             (let ((acts (filter (lambda (a) (not (action-interactive? a))) (actions-for kind))))
               (if (null? acts)
                   "No non-interactive batch actions for this target kind"
                   (cons ts acts)))))))))

(define (export-targets-view! s prompt-title cands input total)
  (let ((targets (map candidate-target cands)))
    (cond
      ((location-targets? targets)
       (export-location-lens! s input targets #:total total))
      ((and (= (length targets) 1) (target? (car targets)) (eq? (target-kind (car targets)) 'directory))
       (act-default! s (car targets)))
      (else
       (collect-targets-view! s (string-append "*Export: " prompt-title "*") cands total)))))

(define (action-prompt-key s key)
  (let ((ap (sget s 'action-prompt)))
    (clear-action-prompt! s)
    (when ap
      (let ((targets (list-ref ap 0))
            (name (list-ref ap 1))
            (actions (list-ref ap 2))
            (mb-ctx (list-ref ap 3))
            (op (list-ref ap 4))
            (hold (list-ref ap 5)))
        (cond
          ((or (string=? key "C-g") (string=? key "ESC"))
           (release-action-op! op hold #t)
           (message! s "Quit"))
          ((or (string=? key "?") (string=? key "C-h"))
           (run-with-action-op! op hold
                                (lambda () (act-on-completing-read! s targets name actions))))
          ((and mb-ctx (string=? key "S"))
           (run-with-action-op!
            op hold
            (lambda ()
              (let* ((bounded ((cadr mb-ctx) %collect-limit))
                     (go (lambda () (collect-candidates-buffer! s (string-append "*Targets: " (car mb-ctx) "*") (cdr bounded) #:total (car bounded)))))
                (if (current-operation) (go) (call-in-operation (start-operation! 'minibuffer-collect s) go))))))
          ((and mb-ctx (string=? key "E"))
           (run-with-action-op!
            op hold
            (lambda ()
              (let* ((bounded ((cadr mb-ctx) %collect-limit))
                     (go (lambda () (export-targets-view! s (car mb-ctx) (cdr bounded) (list-ref mb-ctx 3) (car bounded)))))
                (if (current-operation) (go) (call-in-operation (start-operation! 'minibuffer-export s) go))))))
          ((and mb-ctx (string=? key "A"))
           (let* ((marked (caddr mb-ctx))
                  (src (if (pair? marked) marked (cdr ((cadr mb-ctx) %collect-limit))))
                  (res (batch-targets-and-actions* src)))
             (if (string? res)
                 (begin
                   (release-action-op! op hold #t)
                   (message! s res))
                 (let ((ts (car res)) (acts (cdr res)))
                   (open-action-prompt*! s ts (string-append (number->string (length ts)) " targets") acts #f
                                         (string-append "Act on ALL (" (number->string (length ts)) " targets): press key or ?")
                                         #:op op #:hold hold)))))
          ((string=? key "RET")
           (if (pair? actions)
               (run-with-action-op! op hold (lambda () (run-action-in-operation! s (car actions) targets)))
               (release-action-op! op hold #f)))
          ((find (lambda (a) (and (action-key a) (string=? (action-key a) key))) actions)
           => (lambda (a)
                (run-with-action-op! op hold (lambda () (run-action-in-operation! s a targets)))))
          (else
           (open-action-prompt*! s targets name actions mb-ctx
                                 (string-append "No action for " key " (? to search, C-g to cancel)")
                                 #:op op #:hold hold)))))))

(define (token-at-pos d pos)
  (let* ((ls (line-start d pos))
         (le (line-end d pos))
         (line (document-substring d ls le))
         (len (string-length line))
         (rel (min len (string-length (document-substring d ls pos))))
         (delim? (lambda (c) (or (char-whitespace? c) (memv c '(#\< #\> #\( #\) #\[ #\] #\" #\' #\, #\;))))))
    (if (= len 0)
        #f
        (let* ((idx (if (and (< rel len) (not (delim? (string-ref line rel))))
                        rel
                        (and (> rel 0) (not (delim? (string-ref line (- rel 1)))) (- rel 1)))))
          (and idx
               (let ((start (let loop ((i idx))
                              (if (and (> i 0) (not (delim? (string-ref line (- i 1))))) (loop (- i 1)) i)))
                     (end (let loop ((i idx))
                            (if (and (< (+ i 1) len) (not (delim? (string-ref line (+ i 1))))) (loop (+ i 1)) (+ i 1)))))
                 (substring line start end)))))))

(define (fallback-target-at d pos)
  (let ((tok (token-at-pos d pos)))
    (cond
      ((not tok) #f)
      ((or (string-prefix? "https://" tok) (string-prefix? "http://" tok))
       (target 'url tok))
      ((and (or (string-prefix? "/" tok) (string-prefix? "~/" tok))
            (guard (e (#t #f)) (path-exists? (absolute-path tok))))
       (let* ((abs (absolute-path tok))
              (attrs (guard (e (#t #f)) (file-attributes abs)))
              (dir? (and attrs (eq? (car attrs) 'directory))))
         (target (if dir? 'directory 'file) abs)))
      (else #f))))

(define (candidate-mark-key c)
  (let ((t (candidate-target c)))
    (if (target? t)
        (list (target-kind t) (target-value t))
        (list 'text (candidate-text c)))))

(define (toggle-mark*! s mb cur set-marked! next-fn)
  (let ((c (minibuffer-selected s)))
    (if (not c)
        (message! s "No candidate to mark")
        (let* ((k (candidate-mark-key c))
               (already? (find (lambda (x) (equal? (candidate-mark-key x) k)) cur))
               (next (if already?
                         (filter (lambda (x) (not (equal? (candidate-mark-key x) k))) cur)
                         (append cur (list c)))))
          (set-marked! mb next)
          (next-fn s 1)
          (message! s (string-append (number->string (length next)) " marked"))))))

(define (act-all*! s src)
  (let ((res (batch-targets-and-actions* src)))
    (if (string? res)
        (message! s res)
        (let ((ts (car res)) (acts (cdr res)))
          (abort-minibuffer! s)
          (call-in-operation
           (start-operation! 'minibuffer-act-all s)
           (lambda ()
             (open-action-prompt*! s ts (string-append (number->string (length ts)) " targets") acts #f
                                   (string-append "Act on ALL (" (number->string (length ts)) " targets): press key or ?"))))))))

(set-fallback-target-at! fallback-target-at)
(define (minibuffer-act*! s mb prompt input-target marked bounded-fn)
  (let* ((c (minibuffer-selected s))
         (inp (minibuffer-input s))
         (t (or (and c (candidate-target c))
                (and input-target (guard (e (#t #f)) (input-target inp)))))
         (label (if c (candidate-text c) inp))
         (mb-ctx (list (string-trim prompt) bounded-fn marked inp)))
    (if (target? t)
        (begin
          (abort-minibuffer! s)
          (call-in-operation
           (start-operation! 'minibuffer-act s)
           (lambda () (act-on! s t label #:minibuffer-context mb-ctx))))
        (message! s "No target to act on"))))

(define (region-target-at-point s)
  (let* ((d (doc s))
         (v (session-view s))
         (rs (ranges s))
         (pi (let ((i (view-primary v))) (if (and (exact-integer? i) (>= i 0) (< i (length rs))) i 0)))
         (r (and (pair? rs) (list-ref rs pi))))
    (and r (not (= (car r) (cadr r)))
         (let ((from (min (car r) (cadr r)))
               (to (max (car r) (cadr r))))
           (target 'region (list d (document-revision d) from to (document-substring d from to)))))))

(define (location-targets? targets)
  (and (pair? targets)
       (every (lambda (t) (and (target? t) (eq? (target-kind t) 'location))) targets)
       (begin
         (check-lens-size (length targets))
         (every (lambda (t) (not (presentation? (location-document (target-value t))))) targets))))

(define (export-location-lens! s input targets #:total [total #f])
  (check-lens-size (or total (length targets)))
  (show-lens! s (string-append "*lens " input "*")
              (line-rows (map (lambda (t) (let ((l (target-value t))) (cons (location-document l) (location-position l))))
                              targets))))

(define (strip-trailing-slashes path)
  (let loop ((p path))
    (if (and (> (string-length p) 1) (string-suffix? "/" p))
        (loop (substring p 0 (- (string-length p) 1)))
        p)))

(define (validated-entry-path path)
  (let* ((clean (strip-trailing-slashes (absolute-path path)))
         (leaf (file-name clean)))
    (when (or (string=? clean "") (string=? clean "/") (string=? leaf ".") (string=? leaf ".."))
      (error "Refusing to delete special path" path))
    clean))

(define (delete-path-and-buffers*! s path recursive? entry-truename documents-under)
  (let* ((clean (validated-entry-path path))
         (attrs (guard (e (#t #f)) (file-attributes clean)))
         (is-dir? (and attrs (eq? (car attrs) 'directory))))
    (when (and is-dir? (not recursive?))
      (error "Is a directory; use directory delete instead" clean))
    (let* ((deleted (documents-under (entry-truename clean)))
           (dirty (filter document-dirty? deleted)))
      (unless (null? dirty)
        (error "Unsaved edits: save or undo them first" (string-join (map document-path dirty) ", ")))
      (delete-path clean recursive?)
      (for-each (lambda (d)
                  (reload-document! d)
                  (let ((b (document-buffer d)))
                    (when (and b (> (length (buffer-list)) 1)) (drop-buffer! s b))))
                deleted))))

(define (collect-cmd*! s prompt bounded)
  (let ((title (string-append "*Targets: " (string-trim prompt) "*")))
    (abort-minibuffer! s)
    (call-in-operation (start-operation! 'minibuffer-collect s)
                       (lambda () (collect-targets-view! s title (cdr bounded) (car bounded))))))

(set-extra-actions!
 (lambda (op . args)
   (case op
     ((collect) (apply collect-targets-view! args))
     ((collect-cmd) (apply collect-cmd*! args))
     ((prompt) (apply open-action-prompt*! args))
     ((toggle-mark) (apply toggle-mark*! args))
     ((act-all) (apply act-all*! args))
     ((act) (apply minibuffer-act*! args))
     ((region-target) (apply region-target-at-point args))
     ((delete-path) (apply delete-path-and-buffers*! args)))))
