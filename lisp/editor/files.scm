;;; Files: paths, and the documents of files. A file is opened once, with
;;; its journal: a file visited again is the same document.

(provide file-document absolute-path directory-of file-name working-directory)

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

(define %documents (make-hash-table))

;; The document of the file at PATH, opened with its journal the first
;; time; a document already open (D) is registered as its file's.
(define (file-document path #:document [d #f])
  (let ((path (absolute-path path)))
    (or (hash-table-ref/default %documents path #f)
        (let ((d (or d (open-file path))))
          (hash-table-set! %documents path d)
          d))))
