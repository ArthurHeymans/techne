;;; Buffers (EDITOR.md, section 1): what can be switched to. A buffer is a
;;; document with a name; the list keeps them most recently shown first.
;;; Each remembers the view it was last shown in, so going back to it finds
;;; its caret and scroll where they were.
;;;
;;; Files are opened once: a file visited again is the same document.

(require "session.scm")
(require "commands.scm")
(require "minibuffer.scm")

(provide file-document absolute-path directory-of buffer-list buffer-name add-buffer! show-document! visit!
         read-file-name find-file switch-to-buffer)

;;; File names

(define (last-slash path)
  (let loop ((i (- (string-length path) 1)))
    (cond ((< i 0) #f)
          ((char=? (string-ref path i) #\/) i)
          (else (loop (- i 1))))))

;; The directory part of a path, with its slash: "" when there is none.
(define (directory-of path)
  (let ((i (last-slash path))) (if i (substring path 0 (+ i 1)) "")))

(define (file-name path) (substring path (string-length (directory-of path)) (string-length path)))

(define (home) (or (get-environment-variable "HOME") "/"))
(define (working-directory) (or (get-environment-variable "PWD") (home)))

(define (absolute-path path)
  (cond ((string-prefix? "/" path) path)
        ((string-prefix? "~/" path) (string-append (home) (substring path 1 (string-length path))))
        (else (string-append (working-directory) "/" path))))

;;; Documents of files

(define %documents (make-hash-table))

;; The document of the file at PATH, opened with its journal the first time.
(define (file-document path)
  (let ((path (absolute-path path)))
    (or (hash-table-ref/default %documents path #f)
        (let ((d (open-file path)))
          (hash-table-set! %documents path d)
          d))))

;;; The buffer list

(define %buffers '())

(define (buffer-list) %buffers)

(define (buffer-name d) (doc-prop d 'name))

;; Put D first in the list, naming it if it is new: a file by its name,
;; with its directory's name after it when that name is taken.
(define (add-buffer! d)
  (unless (buffer-name d)
    (let* ((path (document-path d))
           (base (if path (file-name path) "*scratch*"))
           (taken? (lambda (n) (any (lambda (b) (equal? (buffer-name b) n)) %buffers))))
      (when path (hash-table-set! %documents (absolute-path path) d))
      (set-doc-prop! d 'name
                     (if (and path (taken? base))
                         (let ((dir (directory-of (absolute-path path))))
                           (string-append base "<" (file-name (substring dir 0 (- (string-length dir) 1))) ">"))
                         base))))
  (set! %buffers (cons d (remove (lambda (b) (document=? b d)) %buffers))))

;; Show D in the focused pane, in the view it was last shown in unless
;; another pane shows that one. With REMEMBER false (a preview) the list's
;; order stays.
(define (show-document! s d #:remember [remember #t])
  (let ((leaving (pane-view s)))
    (if (document=? (view-document leaving) d)
        leaving
        (let* ((last (doc-prop d 'view))
               (v (cond ((not last) (make-view d "user"))
                        ((any (lambda (p) (view=? p last)) (session-panes s)) (view-split last))
                        (else last))))
          (set-doc-prop! (view-document leaving) 'view leaving)
          (set-doc-prop! d 'view v)
          (if remember (add-buffer! d) (unless (buffer-name d) (add-buffer! d)))
          (set-pane-view! s v)
          v))))

;; Show line LINE (1-based), column COLUMN, of the file PATH in the focused
;; pane, remembering where it was (M-, goes back).
(define (visit! s path line column)
  (sset! s 'visited (cons (pane-view s) (or (sget s 'visited) '())))
  (let* ((d (file-document path))
         (v (show-document! s d))
         (p (line-down d 0 (- line 1) (- column 1))))
    (view-set-ranges! v (list (list p p)) 0)
    (view-set-scroll! v (line-start d p))))

;;; Commands

(define (directory-name? path) (string-suffix? "/" path))

;; Read a file name, completing it a directory at a time: the candidates
;; are the entries of the input's directory, matched against what follows
;; it. Taking a directory goes into it; (ACCEPT session path) gets a file's
;; absolute path, also one that does not exist yet.
(define (read-file-name s prompt initial accept)
  (completing-read s prompt
                   (lambda (input)
                     (let ((dir (directory-of input)))
                       (map (lambda (name) (candidate name #:target (string-append dir name)))
                            (guard (e (#t '()))
                              (directory-list (if (string=? dir "") (working-directory) (absolute-path dir)))))))
                   #:initial initial
                   #:pattern file-name
                   #:require-match #f
                   #:accept (lambda (s c)
                              (let ((path (or (candidate-target c) (candidate-text c))))
                                (if (directory-name? path)
                                    (read-file-name s prompt path accept)
                                    (accept s (absolute-path path)))))))

(define (default-directory s)
  (let ((path (document-path (view-document (pane-view s)))))
    (directory-of (absolute-path (or path (string-append (working-directory) "/"))))))

(define-command (find-file s n)
  "Open a file in the focused pane, completing its name."
  (read-file-name s "Find file: " (default-directory s)
                  (lambda (s path) (show-document! s (file-document path)))))

(define (buffer-annotation d)
  (string-append (if (document-dirty? d) "modified  " "") (or (document-path d) "")))

(define-command (switch-to-buffer s n)
  "Show another buffer in the focused pane, previewing it while choosing."
  (let ((current (view-document (pane-view s))))
    (completing-read s "Switch to buffer: "
                     (map (lambda (d) (candidate (buffer-name d) #:annotation (buffer-annotation d) #:target d))
                          (append (remove (lambda (d) (document=? d current)) (buffer-list)) (list current)))
                     #:preview (lambda (s c) (show-document! s (candidate-target c) #:remember #f))
                     #:accept (lambda (s c) (show-document! s (candidate-target c))))))
