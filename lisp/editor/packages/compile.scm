;;; Compiling, as Emacs's compile: M-x compile runs a command in the
;;; project's root (`project-root`), its output shown in *compilation* as
;;; it comes. Each project remembers its command, which M-x recompile, or
;;; g there, runs again; a compilation started kills the one running.
;;;
;;; The lines naming a place in a file, file:line:column: as compilers
;;; write them (rustc with --> before), are locations, once that file
;;; exists: RET goes to one in another pane, and M-g n and M-g p go to
;;; the next and the one before from anywhere, as next-error does. The
;;; current one is the selection of a view of the output no pane shows:
;;; its line, which stays put as output comes, and is left empty where
;;; dropping the output's start took the line away.

(import (techne editor))

(define compilation-output-max (* 8 1024 1024)
  "The bytes of output *compilation* keeps, about.
Past that, its start is dropped, as for *Async Shell Command*.")

;;; Diagnostics

;; A place in a file at the start of a line: file:line, file:line:column,
;; then a colon or the line's end; after spaces and rustc's --> or :::.
(define diagnostic-regexp (string->regexp "(?m)^[ \t]*(?:--> |::: )?[^ \t\n:]+:[0-9]+(?::[0-9]+)?(?::|$)"))
(define severity-regexp (string->regexp "(?m)^(?:error|warning)(?:\\[[A-Za-z0-9]+\\])?:"))

;; How many characters of TEXT come before the file's name.
(define (lead-length text)
  (let* ((spaces (let skip ((i 0))
                   (if (and (< i (string-length text)) (memv (string-ref text i) '(#\space #\tab))) (skip (+ i 1)) i)))
         (rest (substring text spaces (string-length text))))
    (if (or (string-prefix? "--> " rest) (string-prefix? "::: " rest)) (+ spaces 4) spaces)))

(define (diagnostic b pos)
  "Return the place the line at POS of the compilation buffer B names.
It is (from to location), FROM and TO where it is named, or #f when the
line names none, or a file that does not exist."
  (let* ((d (buffer-document b))
         (start (line-start d pos))
         (line (document-substring d start (line-end d pos)))
         (m (string-search-regexp line diagnostic-regexp)))
    (and m (= (car m) 0)
         (let* ((text (utf8->string (string->utf8 line) 0 (cadr m)))
                (lead (lead-length text))
                (named (let ((t (substring text lead (string-length text))))
                         (if (string-suffix? ":" t) (substring t 0 (- (string-length t) 1)) t)))
                (parts (string-split named ":"))
                (path (if (string-prefix? "/" (car parts)) (car parts) (string-append (state b 'dir) (car parts)))))
           (and (exists? b path)
                (list (+ start lead) (+ start lead (bytevector-length (string->utf8 named)))
                      (file-location path (string->number (cadr parts))
                                     #:column (if (pair? (cddr parts)) (string->number (caddr parts)) 1))))))))

;; Whether the file at PATH exists, as B found it first: the highlights
;; ask on every frame.
(define (exists? b path)
  (let* ((known (state b 'exists)) (e (hash-table-ref/default known path 'unknown)))
    (if (eq? e 'unknown)
        (let ((e (file-exists? path))) (hash-table-set! known path e) e)
        e)))

;; The first line of B's output naming a place from FROM on, or with
;; FORWARD #f the last before FROM, with the place: (start from to
;; location), or #f. Back, the candidates are found in one pass, not a
;; pass from the start for each.
(define (next-diagnostic b from forward)
  (let ((d (buffer-document b)))
    (if forward
        (let loop ((p from))
          (let ((m (and (<= p (document-length d)) (search-text-regexp d p diagnostic-regexp #t))))
            (and m (let ((found (diagnostic b (car m))))
                     (if found (cons (car m) found) (loop (+ (line-end d (car m)) 1)))))))
        (any (lambda (m) (let ((found (diagnostic b (car m)))) (and found (cons (car m) found))))
             (reverse (search-text-regexp-all d diagnostic-regexp 0 from))))))

;; The Nth line naming a place from FROM, back with N negative, or the
;; last there is on the way; #f for none.
(define (nth-diagnostic b from n)
  (let ((d (buffer-document b)))
    (let loop ((k (abs n)) (from from) (last #f))
      (let ((found (and (> k 0) (next-diagnostic b from (> n 0)))))
        (if found
            (loop (- k 1) (if (> n 0) (+ (line-end d (car found)) 1) (car found)) found)
            last)))))

;;; The buffer. Its state is a table: `dir`, `command`, `slot` (the
;;; process's request slot), `writer` (the view output is written
;;; through), `at` (the view whose selection is the current error's line)
;;; and `exists` (the files found to exist, or not).

(define (compilation-buffer) (buffer-named "*compilation*"))
(define (state b key) (hash-table-ref/default (buffer-state b) key #f))

(define-mode compilation-mode
  "A compilation's output.
\\[compile-goto-error] goes to the place named on this line, in another
pane; \\[compilation-next-error] and \\[compilation-previous-error] go to
the next line naming one and the one before. \\[recompile] runs the
command again, \\[kill-compilation] stops it."
  #:parent 'log-mode
  #:keys '(("RET" compile-goto-error) ("g" recompile) ("n" compilation-next-error) ("p" compilation-previous-error)
           ("C-c C-k" kill-compilation))
  #:normal '(("RET" compile-goto-error) ("g r" recompile) ("g j" compilation-next-error) ("g k" compilation-previous-error))
  #:directory (lambda (b) (state b 'dir))
  #:target-at (lambda (b pos)
                (let ((found (diagnostic b pos)))
                  (and found (target 'location (caddr found)))))
  #:layer (lambda (d from to)
            (let ((b (document-buffer d)))
              (append (filter-map (lambda (span)
                                    (let ((found (diagnostic b (car span))))
                                      (and found (list (car found) (cadr found) 'keyword))))
                                  (search-text-regexp-all d diagnostic-regexp from to))
                      (map (lambda (span)
                             (list (car span) (cadr span)
                                   (if (string=? (document-substring d (car span) (+ (car span) 1)) "e") 'error 'warning)))
                           (search-text-regexp-all d severity-regexp from to))))))

;;; Running

;; The command each project's root ran last.
(define %commands (make-hash-table))

(define (default-command root)
  (if (file-exists? (string-append root "Cargo.toml")) "cargo build" "make -k "))

(define (shell-quote s)
  (string-append "'" (string-join (string-split s "'") "'\\''") "'"))

(define (status-text status)
  (cond ((eqv? status 0) "finished")
        ((integer? status) (string-append "exited abnormally with code " (number->string status)))
        (else (call-with-output-string (lambda (p) (display "ended by " p) (display status p))))))

;; *compilation*, emptied, for COMMAND in DIR: made the first time, else
;; its compilation killed.
(define (fresh-buffer! dir command)
  (let* ((b (or (compilation-buffer)
                (let ((d (make-document "")))
                  (document-limit-history! d log-history)
                  (make-generated-buffer! "*compilation*" d 'compilation-mode
                                          #:state (let ((st (make-hash-table)))
                                                    (hash-table-set! st 'slot (make-request-slot))
                                                    (hash-table-set! st 'writer (make-view d "process"))
                                                    (hash-table-set! st 'at (make-view d "compile"))
                                                    st)))))
         (d (buffer-document b)))
    (cancel-request! (state b 'slot))
    (hash-table-set! (buffer-state b) 'dir dir)
    (hash-table-set! (buffer-state b) 'command command)
    (hash-table-set! (buffer-state b) 'exists (make-hash-table))
    (view-edit! (state b 'writer) (list (list 0 (document-length d) "")) "new")
    (view-set-ranges! (state b 'at) '((0 0)) 0)
    b))

(define (start-compilation! s dir command)
  (hash-table-set! %commands dir command)
  (let* ((b (fresh-buffer! dir command))
         (out (cons (buffer-document b) (state b 'writer)))
         (say! (lambda (text)
                 (append-bounded-output! out text compilation-output-max)
                 ;; The current error's view follows the edit now: a view
                 ;; behind more than the history keeps loses its place.
                 (view-ranges (state b 'at)))))
    (say! (string-append "-*- mode: compilation; default-directory: \"" (abbreviate-path dir) "\" -*-\n"
                         "Compilation started\n\n" command "\n"))
    (display-buffer! s (buffer-document b))
    (request! (state b 'slot)
              (lambda ()
                (call-with-process "sh" (list "-c" (string-append "cd " (shell-quote dir) " && { " command "\n} 2>&1"))
                  (lambda (p)
                    (process-close-input p)
                    (let loop ()
                      (let ((c (process-read p 'stdout)))
                        (unless (eof-object? c) (say! c) (loop))))
                    (process-wait p))))
              (lambda (status)
                (let ((how (status-text status)))
                  (say! (string-append "\nCompilation " how "\n"))
                  (message! s (string-append "Compilation " how))))
              #:fail (lambda (e)
                       (say! (string-append "\nCompilation failed: " (error-text e) "\n"))
                       (message! s (string-append "Compilation failed: " (error-text e)))))))

(define-command (compile s n)
  "Run a command in the project's root; show its output as it comes.
The command is the one the project ran last, to edit; it runs in a
shell, and its output and errors go to *compilation*."
  (let ((root (project-root (default-directory s))))
    (completing-read s (string-append "Compile in " (abbreviate-path root) ": ") '()
                     #:initial (hash-table-ref/default %commands root (default-command root))
                     #:require-match #f
                     #:history 'compile
                     #:accept (lambda (s c) (start-compilation! s root (candidate-text c))))))

(define-command (recompile s n)
  "Run the compilation again: *compilation*'s, else the project's.
Without one, it is read as \\[compile] reads it."
  (let* ((b (compilation-buffer))
         (here (and b (eq? (session-buffer s) b)))
         (root (if here (state b 'dir) (project-root (default-directory s))))
         (command (if here (state b 'command) (hash-table-ref/default %commands root #f))))
    (if command (start-compilation! s root command) (compile s n))))

(define-command (kill-compilation s n)
  "Stop the compilation running."
  (let ((b (compilation-buffer)))
    (if (and b (request-pending? (state b 'slot)))
        (begin (cancel-request! (state b 'slot))
               (append-bounded-output! (cons (buffer-document b) (state b 'writer)) "\nCompilation killed\n" compilation-output-max)
               (message! s "Compilation killed"))
        (message! s "No compilation is running"))))

;;; Going to errors

(define (the-compilation)
  (or (compilation-buffer) (error "No compilation")))

;; Go to the place FOUND names, (start from to location), in a pane other
;; than *compilation*'s, which shows its line at the top.
(define (visit-error! s b found)
  (let* ((d (buffer-document b)) (start (car found)) (l (cadddr found)))
    (view-set-ranges! (state b 'at) (list (list start (line-end d start))) 0)
    (when (document=? (view-document (session-view s)) d)
      (let ((n (length (session-panes s))))
        (if (> n 1) (sset! s 'focus (modulo (+ (session-focus s) 1) n)) (show-in-other-pane! s d))))
    (let* ((p (location-position l)) (v (show-document! s (location-document l))))
      (view-set-ranges! v (list (list p p)) 0))
    (display-buffer! s d)
    (for-each (lambda (v)
                (when (document=? (view-document v) d)
                  (view-set-ranges! v (list (list start start)) 0)
                  (view-set-scroll! v start)))
              (session-panes s))))

;; Go N places on from the current error, back with N negative. With
;; none, or its line gone, the next is the first from there.
(define (go-to-error! s n)
  (let* ((b (the-compilation))
         (r (car (view-ranges (state b 'at))))
         (current (< (car r) (cadr r)))
         (found (nth-diagnostic b (if (and current (> n 0)) (+ (cadr r) 1) (car r)) n)))
    (if found
        (visit-error! s b found)
        (message! s (if (> n 0) "No more errors" "No errors before")))))

(define-command (next-error s n)
  "Go to the next place *compilation* names, in another pane."
  (go-to-error! s n))

(define-command (previous-error s n)
  "Go to the place *compilation* names before the current one."
  (go-to-error! s (- n)))

(define-command (compile-goto-error s n)
  "Go to the place named on this line, in another pane."
  (let* ((b (the-compilation)) (pos (car (car (view-ranges (session-view s)))))
         (found (diagnostic b pos)))
    (if found
        (visit-error! s b (cons (line-start (buffer-document b) pos) found))
        (message! s "No place named on this line"))))

;; Move the caret of *compilation*, focused, N lines naming a place on,
;; back with N negative.
(define (move-to-error! s n)
  (let* ((b (the-compilation)) (d (buffer-document b)) (v (session-view s))
         (pos (car (car (view-ranges v))))
         (found (nth-diagnostic b (if (> n 0) (+ (line-end d pos) 1) (line-start d pos)) n)))
    (if found
        (view-set-ranges! v (list (list (car found) (car found))) 0)
        (message! s (if (> n 0) "No more errors" "No errors before")))))

(define-command (compilation-next-error s n)
  "Move to the next line naming a place."
  (move-to-error! s n))

(define-command (compilation-previous-error s n)
  "Move to the line naming a place before this one."
  (move-to-error! s (- n)))

(for-each (lambda (b) (define-key! emacs-map (car b) (cadr b)))
          '(("M-g n" next-error) ("M-g M-n" next-error) ("M-g p" previous-error) ("M-g M-p" previous-error)
            ("C-x `" next-error) ("C-c c c" compile) ("C-c c C" recompile)))
(for-each (lambda (b) (define-key! modal-map (car b) (cadr b)))
          '(("] e" next-error) ("[ e" previous-error) ("SPC c c" compile) ("SPC c C" recompile)))
(name-prefix! emacs-map "M-g" "goto")
(for-each (lambda (n) (name-prefix! modal-map (car n) (cadr n))) '(("]" "next") ("[" "previous")))
