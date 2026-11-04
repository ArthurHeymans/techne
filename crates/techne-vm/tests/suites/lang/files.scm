;;; Directories and their entries: making, listing, describing, moving,
;;; copying and deleting them.

(test-begin "files")

(define tmp (or (get-environment-variable "TMPDIR") "/tmp"))

;; A directory of this run's own: create-directory refuses one that is
;; there, so the first name it makes is nobody else's, whichever modes
;; run beside this one.
(define root
  (let claim ((n 0))
    (let ((path (string-append tmp "/techne-files-suite-" (number->string n))))
      (if (guard (e (#t #f)) (create-directory path #o700) #t)
          path
          (claim (+ n 1))))))
(define (at . parts) (apply string-append root (map (lambda (p) (string-append "/" p)) parts)))

(test-error (create-directory root #o700))
(test-error (create-directory (at "missing" "child") #o700))
(test-error (create-directory (at "x") #o10000))
(test '(directory 448) (let ((a (file-attributes root))) (list (car a) (cadddr a))))

;; make-directory makes what is missing above, and leaves one there.
(make-directory (at "a" "b"))
(make-directory (at "a" "b"))
(call-with-output-file (at "a" "f") (lambda (p) (display "text" p)))
(test '("b/" "f") (directory-list (at "a")))
(test '(file 4) (let ((a (file-attributes (at "a" "f")))) (list (car a) (cadr a))))
(test #f (file-attributes (at "nothing")))
(test (at "a" "b") (file-truename (at "a" "b" ".." "b")))
(test (at "a" "new") (file-truename (at "a" "new")))

(set-file-mode! (at "a" "f") #o600)
(test #o600 (cadddr (file-attributes (at "a" "f"))))

;; Nothing is replaced: copying or moving onto an entry is refused.
(copy-file (at "a") (at "c"))
(test "text" (file->string (at "c" "f")))
(test-error (copy-file (at "a") (at "c")))
(test-error (rename-file (at "a") (at "c")))
(rename-file (at "c") (at "d"))
(test '("a/" "d/") (directory-list root))

(replace-file (at "d" "f") "new text")
(test "new text" (file->string (at "d" "f")))
(test "new" (file-head (at "d" "f") 3))

;; A directory goes only when empty, unless recursively.
(test-error (delete-path (at "d") #f))
(delete-path (at "d") #t)
(delete-path root #t)
(test #f (file-exists? root))

(test-end)
