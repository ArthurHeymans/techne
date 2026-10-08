;;; Regular expressions (SRFI 115) beyond its own tests: what the engine
;;; leaves out, and long strings.

(import (srfi 115))

(test-begin "regexp")

;; Look-around and backreferences are errors, which valid-sre? reports.
(test #f (valid-sre? '(: "a" (look-ahead "b"))))
(test #f (valid-sre? '(: ($ "a") (backref 1))))
(test-error (regexp '(neg-look-behind "x")))
(test #t (valid-sre? '(: bow (+ alpha) eow)))

;; A compiled regexp keeps its SRE, and is taken as it is.
(define number (regexp '(+ num)))
(test '(+ num) (regexp->sre number))
(test #t (regexp? number))
(test '("12" "345") (regexp-extract number "a12b345"))

;; Folding over the matches of a long non-ASCII string is linear: indexes
;; are counted from one match to the next.
(define long (string-append (make-string 100000 #\é) "x" (make-string 100000 #\é) "x"))
(test '(100000 200001)
      (reverse (regexp-fold "x" (lambda (i m s acc) (cons (regexp-match-submatch-start m 0) acc)) '() long)))
(test 3 (length (regexp-split "x" long)))

;; Submatch names are symbols.
(test-error (regexp '(-> "name" "a")))

;; An empty match counts once, and one at the end ends the matches.
(test '(1) (reverse (regexp-fold 'bow (lambda (i m s acc) (cons (regexp-match-submatch-start m 0) acc)) '() " a")))
(test " Xa" (regexp-replace-all 'bow " a" "X"))
(test '("" "a" "b") (regexp-partition '(or "a" eos) "ab"))
(test "a" (regexp-replace 'eos "a" "X" 0 #f 3))
(test "aX" (regexp-replace 'eos "a" "X"))

;; An empty list of substitution pieces deletes the match.
(test "ct" (regexp-replace "a" "cat" '()))
(test "bnn" (regexp-replace-all "a" "banana" '()))

;; Ignoring case takes every case of a character (K, k and the Kelvin
;; sign), but in an ASCII context only ASCII letters' other cases.
(test #t (regexp-matches? '(w/nocase #\k) "\x212A;"))
(test #t (regexp-matches? '(w/nocase (/ "AZ")) "\x212A;"))
(test #f (regexp-matches? '(w/nocase (~ ("Aab"))) "B"))
(test #t (regexp-matches? '(w/ascii (w/nocase "kk")) "Kk"))
(test #f (regexp-matches? '(w/ascii (w/nocase "kk")) "\x212A;k"))
(test #f (regexp-matches? '(w/ascii (w/nocase #\é)) "É"))

;; Empty character sets match nothing.
(test #t (valid-sre? '("")))
(test #f (regexp-search '(/) "abc"))

;; Searching on from a match still sees the character before it.
(test '("a") (regexp-extract '(or (: bow "b") "a") "ab"))

(test-end)
