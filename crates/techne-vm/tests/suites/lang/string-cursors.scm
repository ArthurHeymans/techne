;;; String cursors (SRFI 130).

(import (srfi 130))

(test-begin "string cursors")

;; Cursors step over characters of any size, and convert to indexes.
(define s "aé€😀b")
(let* ((c1 (string-cursor-next s (string-cursor-start s)))
       (c3 (string-cursor-forward s c1 2)))
  (test #\é (string-ref/cursor s c1))
  (test #\😀 (string-ref/cursor s c3))
  (test 3 (string-cursor->index s c3))
  (test #t (string-cursor=? c3 (string-index->cursor s 3)))
  (test c1 (string-cursor-back s c3 2))
  (test "é€" (substring/cursors s c1 c3))
  (test "é" (substring/cursors s c1 (string-cursor-prev s c3)))
  (test 2 (string-cursor-diff s c1 c3))
  (test 'string-cursor (type-of c1))
  (test #f (string-cursor? 1)))

;; Indexes work where cursors do, and give indexes back.
(test 2 (string-cursor-next s 1))
(test "€😀" (substring/cursors s 2 4))

;; A cursor must be at a character of its string; cursors and indexes
;; do not compare.
(test-error (string-ref/cursor s (string-cursor-end s)))
(test-error (string-ref/cursor "€" (string-cursor-next "ab" (string-cursor-start "ab"))))
(test-error (string-cursor-next s (string-cursor-end s)))
(test-error (string-cursor<? 0 (string-cursor-start s)))

;; Walking a long non-ASCII string with cursors takes linear time: by
;; index this would take minutes.
(define long (make-string 200000 #\é))
(test 200000 (string-fold (lambda (c n) (+ n 1)) 0 long))
(test (string-cursor-end long) (string-index long #\x))

;; SRFI 130's procedures replace the root module's of the same names only
;; where (srfi 130) is imported.
(test (string-cursor-end "abc") (string-index "abc" #\x))
(test #f (eval '(string-index "abc" #\x) (environment '(techne))))
(test "x  " (string-trim "  x  "))
(test "x" (eval '(string-trim "  x  ") (environment '(techne))))

(test-end)
