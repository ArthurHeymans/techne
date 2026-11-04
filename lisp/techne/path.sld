;;; File paths as text, POSIX's: their directory, file name and
;;; extension, and joining them. Nothing here looks at the file system;
;;; `file-truename` resolves links and `..`.

(define-library (techne path)
  (export path-directory path-file-name path-extension path-without-extension
          path-join path-absolute?)
  (import (techne))
  (begin

    ;; The end of PATH without the slashes after its last name, keeping a
    ;; root's.
    (define (%trimmed-end path)
      (let loop ((end (string-length path)))
        (if (and (> end 1) (char=? (string-ref path (- end 1)) #\/))
            (loop (- end 1))
            end)))

    ;; Where the last slash before END is, or #f.
    (define (%last-slash path end)
      (let loop ((i (- end 1)))
        (cond ((< i 0) #f)
              ((char=? (string-ref path i) #\/) i)
              (else (loop (- i 1))))))

    (define (path-directory path)
      "Return the directory PATH names its last entry in, as dirname does.
Slashes after the last name do not count: \"a/b/\" is in \"a\". A
name alone is in \".\", and the root's directory is the root."
      (let* ((end (%trimmed-end path))
             (slash (%last-slash path end)))
        (cond ((string=? path "") ".")
              ((not slash) (if (and (= end 1) (char=? (string-ref path 0) #\/)) "/" "."))
              (else
               ;; Without the slashes before the name, but not the root's.
               (let loop ((i slash))
                 (cond ((zero? i) "/")
                       ((char=? (string-ref path (- i 1)) #\/) (loop (- i 1)))
                       (else (substring path 0 i))))))))

    (define (path-file-name path)
      "Return the last entry PATH names, as basename does.
Slashes after it do not count: \"a/b/\" names \"b\". The root's is
\"/\", and an empty PATH's is \"\"."
      (let* ((end (%trimmed-end path))
             (slash (%last-slash path end)))
        (cond ((string=? path "") "")
              ((and (= end 1) (char=? (string-ref path 0) #\/)) "/")
              (else (substring path (if slash (+ slash 1) 0) end)))))

    ;; Where the extension's dot is in NAME, a file name, or #f: after its
    ;; leading dots and something else (".bashrc" and "..x" have none),
    ;; and not at its end.
    (define (%extension-dot name)
      (let* ((n (string-length name))
             (first (let skip ((i 0)) (if (and (< i n) (char=? (string-ref name i) #\.)) (skip (+ i 1)) i))))
        (let loop ((i (- n 1)))
          (cond ((<= i first) #f)
                ((char=? (string-ref name i) #\.)
                 (and (< i (- n 1)) i))
                (else (loop (- i 1)))))))

    (define (path-extension path)
      "Return the extension of PATH's file name, without its dot, or #f.
\"a/b.tar.gz\" has \"gz\"; \".bashrc\", \"..x\" and \"b.\" have none: the dots a
name starts with are not an extension's."
      (let* ((name (path-file-name path))
             (dot (%extension-dot name)))
        (and dot (substring name (+ dot 1) (string-length name)))))

    (define (path-without-extension path)
      "Return PATH without its file name's extension and the dot before it.
Slashes after the file name go with it. A PATH whose file name has no
extension is returned as it is."
      (let* ((name (path-file-name path))
             (dot (%extension-dot name)))
        (if dot
            (let ((end (%trimmed-end path)))
              (substring path 0 (- end (- (string-length name) dot))))
            path)))

    (define (path-absolute? path)
      "Return #t if PATH starts at the root, with a slash."
      (and (positive? (string-length path)) (char=? (string-ref path 0) #\/)))

    (define (path-join base . parts)
      "Return BASE with PARTS after it, one slash between each two.
An absolute part starts the path again, discarding what came before; an
empty part adds nothing."
      (fold (lambda (part path)
              (cond ((string=? part "") path)
                    ((path-absolute? part) part)
                    ((or (string=? path "") (char=? (string-ref path (- (string-length path) 1)) #\/))
                     (string-append path part))
                    (else (string-append path "/" part))))
            base parts))))
