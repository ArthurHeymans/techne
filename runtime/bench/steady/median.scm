;;; From driver.scm's line of iteration times (microseconds) on stdin,
;;; the median of the second half and the first, in ms:
;;;
;;;   ... | techne run median.scm      =>  "12.3 45.6"

(import (techne) (techne format) (srfi 132))

(define times
  (map string->number
       (filter (lambda (s) (not (string=? s ""))) (string-split (read-line) #\space))))

;; In ms, rounded as Python's statistics.median and its divisions round,
;; so that the figures are those run.sh printed before: the median of
;; an odd count exact, of an even one the mean of the middle two as a
;; float, divided by 1000 after.
(define (median-ms xs)
  (let* ((v (list->vector (list-sort < xs))) (n (vector-length v)) (half (quotient n 2)))
    (if (odd? n)
        (inexact (/ (vector-ref v half) 1000))
        (/ (inexact (/ (+ (vector-ref v (- half 1)) (vector-ref v half)) 2)) 1000.))))

(display (string-append (number->decimal (median-ms (list-tail times (quotient (length times) 2))) 1)
                        " " (number->decimal (inexact (/ (car times) 1000)) 1)))
(newline)
