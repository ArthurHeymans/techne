;;; Numbers written for people: a fixed number of decimal places, as
;;; printf's "%.Nf" writes them, for reports and tables.

(define-library (techne format)
  (export number->decimal)
  (import (techne))
  (begin

    (define (number->decimal x places)
      "Return the real X written with PLACES digits after the point.
X is exact or finite, PLACES an exact integer from 0 to 4096, a bound
on the text a call can ask for. X is rounded as printf's \"%.Nf\"
rounds it: its exact value, an inexact X's binary value, to the
nearest, a tie to the even. So 2.5 gives \"2\" with no places, and
1.05, a little more than 1.05 in binary, \"1.1\" with one. Without
places there is no point. A negative X keeps its sign even when it
rounds to zero, as printf's does."
      (unless (and (exact-integer? places) (<= 0 places 4096))
        (error "number->decimal: expected a count of places from 0 to 4096, got" places))
      (unless (and (real? x) (or (exact? x) (finite? x)))
        (error "number->decimal: expected a finite real, got" x))
      (let* ((r (exact x))
             (negative (or (negative? r) (eqv? x -0.0)))
             (digits (number->string (round (* (abs r) (expt 10 places)))))
             ;; At least one digit before the point.
             (digits (if (> (+ places 1) (string-length digits))
                         (string-append (make-string (- (+ places 1) (string-length digits)) #\0) digits)
                         digits))
             (point (- (string-length digits) places)))
        (string-append (if negative "-" "")
                       (substring digits 0 point)
                       (if (zero? places) "" ".")
                       (substring digits point (string-length digits)))))))
