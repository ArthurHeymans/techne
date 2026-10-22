;;; A service searching a project's files (`search-project`): the lines
;;; that have every part of a pattern, as the minibuffer matches them. Its
;;; world has the files capability and no editor: open documents come as
;;; snapshots, searched instead of their files, unsaved edits and all.
;;; The files of a project are listed when a search starts (`fresh`) and
;;; kept for the requests after, a key each.

;; The files under each root searched, by root: (canonical-root name ...).
(define %files (make-hash-table))

(define (files-of root fresh)
  (or (and (not fresh) (hash-table-ref/default %files root #f))
      (let ((files (project-files root)))
        (hash-table-set! %files root files)
        files)))

(define (search argument snapshots)
  "Search the files under a root for the lines with every part of a pattern.
ARGUMENT is (root parts limit paths fresh): PATHS are the files of
SNAPSHOTS, in order, canonical, which are searched instead of those
files; FRESH lists the root's files again. Return LIMIT matches at
most, each (path name line position text id revision): PATH is
canonical, NAME is it under the root, ID and REVISION are the
snapshot's when it was one, else #f, and POSITION is a byte offset."
  (let* ((root (list-ref argument 0)) (parts (list-ref argument 1)) (limit (list-ref argument 2))
         (paths (list-ref argument 3)) (fresh (list-ref argument 4))
         (listed (files-of root fresh)) (base (car listed))
         (open (make-hash-table))
         (open? (lambda (name) (hash-table-contains? open (string-append base name)))))
    (for-each (lambda (p s) (hash-table-set! open p s)) paths snapshots)
    ;; Open documents first, then the other files, until LIMIT.
    (let loop ((todo (append (filter open? (cdr listed)) (remove open? (cdr listed)))) (found '()) (left limit))
      (if (or (null? todo) (<= left 0))
          (reverse found)
          (let* ((name (car todo)) (path (string-append base name)) (s (hash-table-ref/default open path #f))
                 (lines (if s (search-snapshot s parts left) (search-file path parts left)))
                 (id (and s (snapshot-id s))) (revision (and s (snapshot-revision s))))
            (loop (cdr todo)
                  (fold (lambda (l found) (cons (list path name (car l) (cadr l) (caddr l) id revision) found)) found lines)
                  (- left (length lines))))))))
