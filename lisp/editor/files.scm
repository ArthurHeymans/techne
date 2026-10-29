;;; Files: paths, and the documents of files. A file is opened once, with
;;; its journal: a file visited again is the same document.

(require "remote.scm")
(require "session.scm")
(require "dispatch.scm")

(provide file-document absolute-path directory-of file-name working-directory normal-directory abbreviate-path state-file
         open-file-documents file-document-moved! save-document! reload-document! directory-entries
         path-exists?)

(define (last-slash path)
  (let loop ((i (- (string-length path) 1)))
    (cond ((< i 0) #f)
          ((char=? (string-ref path i) #\/) i)
          (else (loop (- i 1))))))

(define (directory-of path)
  "Return the directory part of PATH, with its slash, or \"\"."
  (let ((i (last-slash path))) (if i (substring path 0 (+ i 1)) "")))

(define (file-name path)
  "Return the part of PATH after its directory."
  (substring path (string-length (directory-of path)) (string-length path)))

(define (home) (or (get-environment-variable "HOME") "/"))

(define (state-file name)
  "Return the file NAME of the editor's state, kept across runs.
It is in techne in $XDG_STATE_HOME, else in ~/.local/state, as XDG
says, a relative $XDG_STATE_HOME being left out; that directory is made
if missing, and only its user can read it."
  (let* ((xdg (get-environment-variable "XDG_STATE_HOME"))
         (dir (string-append (if (and xdg (string-prefix? "/" xdg)) xdg (string-append (home) "/.local/state")) "/techne/")))
    (make-directory dir)
    (set-file-mode! dir #o700)
    (string-append dir name)))
(define (working-directory)
  "Return the directory the editor was started in."
  (or (get-environment-variable "PWD") (home)))

(define (absolute-path path)
  "Return PATH as an absolute path, with ~ for the home directory.
PATH may be a location on another machine (doc/remote.md): its path
there is made absolute, ~ standing for the home directory there."
  (cond ((remote-path? path) (remote-absolute-path path))
        ((string-prefix? "/" path) path)
        ((string-prefix? "~/" path) (string-append (home) (substring path 1 (string-length path))))
        (else (string-append (working-directory) "/" path))))

(define (normal-directory path)
  "Return the directory PATH, absolute, without its . and .. parts.
They are taken away as written, a symbolic link not followed. The
result ends with a slash. On another machine, it is its path there that
is made normal."
  (let* ((path (absolute-path path))
         (parts (fold (lambda (part acc)
                        (cond ((or (string=? part "") (string=? part ".")) acc)
                              ((string=? part "..") (if (null? acc) acc (cdr acc)))
                              (else (cons part acc))))
                      '()
                      (string-split (location-path path) "/"))))
    (location-at path (string-append "/" (apply string-append (map (lambda (p) (string-append p "/")) (reverse parts)))))))

(define (abbreviate-path path)
  "Return PATH with ~ for the home directory when it is inside it.
A home directory that is the root is left, as Emacs leaves it."
  (let ((h (let ((h (if (remote-path? path) "/" (home))))
             (if (and (> (string-length h) 1) (string-suffix? "/" h)) (substring h 0 (- (string-length h) 1)) h))))
    (cond ((string=? h "/") path)
          ((string=? path h) "~")
          ((and (string-prefix? h path) (> (string-length path) (string-length h))
                (char=? (string-ref path (string-length h)) #\/))
           (string-append "~" (substring path (string-length h) (string-length path))))
          (else path))))

(define %documents (make-hash-table))

(define (file-document path #:document [d #f])
  "Return the document of the file at PATH.
It is opened with its journal the first time; D, a document already
open, is registered as the file's."
  (let ((path (absolute-path path)))
    (or (hash-table-ref/default %documents path #f)
        (let ((d (or d (if (remote-path? path)
                           (%open-remote-file (location-endpoint path) (location-path path) path)
                           (open-file path)))))
          (hash-table-set! %documents path d)
          d))))

(define (save-document! d #:overwrite [overwrite #f])
  "Write the document D to its file, refusing an external change.
With OVERWRITE, an external change is replaced. A file on another
machine is written through its endpoint, only if it still has the text
D was read or last saved with. Then `document-saved` runs."
  (let ((path (document-path d)))
    (cond ((and path (remote-path? path))
           (%document-save-remote! d (location-endpoint path) (location-path path) overwrite))
          (overwrite (document-save-overwriting! d))
          (else (document-save! d)))
    (when (current-session) (run-hook! (current-session) 'document-saved d))))

(define-hook document-saved
  "After a document is written to its file: (session document).
The session is the current one then.")

(define (reload-document! d)
  "Read the file of the document D again if it changed since it was read.
It is refused while D has unsaved edits. Return #t if the text changed."
  (let ((path (document-path d)))
    (if (and path (remote-path? path))
        (%document-reload-remote! d (location-endpoint path) (location-path path))
        (document-reload! d))))

(define (directory-entries dir)
  "Return the entries of the directory DIR, sorted.
A directory's has a slash after its name. DIR may be a location on
another machine."
  (if (remote-path? dir)
      (node-directory-list (location-endpoint dir) (location-path dir))
      (directory-list dir)))

(define (path-exists? path)
  "Return #t if something is at PATH, here or on another machine."
  (if (remote-path? path)
      (and (node-file-stat (location-endpoint path) (location-path path)) #t)
      (file-exists? path)))

(define (open-file-documents)
  "Return the documents of the files open, each once."
  (fold (lambda (d acc) (if (any (lambda (o) (same-document? o d)) acc) acc (cons d acc)))
        '() (hash-table-values %documents)))

(define (same-document? a b) (= (document-id a) (document-id b)))

(define (forget-file-document! d)
  (for-each (lambda (k) (when (same-document? (hash-table-ref %documents k) d) (hash-table-delete! %documents k)))
            (hash-table-keys %documents)))

(define-hook document-moved
  "After a document's file moves with the document: (session document).
The session is the current one then.")

(define (file-document-moved! d)
  "Register the document D under the path it has now, its file moved.
Then `document-moved` runs."
  (forget-file-document! d)
  (hash-table-set! %documents (document-path d) d)
  (when (current-session) (run-hook! (current-session) 'document-moved d)))
