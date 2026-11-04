;;; (techne path): directories, file names, extensions and joining, as
;;; dirname and basename say them.

(import (techne path))

(test-begin "path")

(test "a/b" (path-directory "a/b/c"))
(test "a" (path-directory "a/b/"))
(test "a" (path-directory "a//b"))
(test "." (path-directory "a"))
(test "." (path-directory "a/"))
(test "." (path-directory ""))
(test "/" (path-directory "/"))
(test "/" (path-directory "/a"))
(test "/" (path-directory "//a"))
(test "/x" (path-directory "/x/y.scm"))

(test "c" (path-file-name "a/b/c"))
(test "b" (path-file-name "a/b/"))
(test "a" (path-file-name "a"))
(test "/" (path-file-name "/"))
(test "/" (path-file-name "//"))
(test "" (path-file-name ""))

(test "scm" (path-extension "a/b.scm"))
(test "gz" (path-extension "b.tar.gz"))
(test #f (path-extension ".bashrc"))
(test #f (path-extension "..bashrc"))
(test #f (path-extension "..."))
(test "b" (path-extension ".a.b"))
(test #f (path-extension "b."))
(test #f (path-extension "a.d/b"))
(test "scm" (path-extension "a/b.scm/"))

(test "a/b" (path-without-extension "a/b.scm"))
(test "b.tar" (path-without-extension "b.tar.gz"))
(test ".bashrc" (path-without-extension ".bashrc"))
(test "..bashrc" (path-without-extension "..bashrc"))
(test ".a" (path-without-extension ".a.b"))
(test "a/b" (path-without-extension "a/b.scm/"))
(test "a/b/" (path-without-extension "a/b/"))
(test "a.d/b" (path-without-extension "a.d/b"))

(test #t (path-absolute? "/a"))
(test #f (path-absolute? "a"))
(test #f (path-absolute? ""))

(test "a/b/c" (path-join "a" "b" "c"))
(test "a/b" (path-join "a/" "b"))
(test "/c" (path-join "a" "b" "/c"))
(test "a" (path-join "a" ""))
(test "b" (path-join "" "b"))
(test "a" (path-join "a"))

(test-end)
