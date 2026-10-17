;;; Calendar dates as day numbers (days since 1970-01-01), as Org writes
;;; them; the calendar is `(techne time)`'s.

(import (techne time))

(provide days-from-civil civil-from-days weekday weekday-name month-name
         date->string parse-iso-date add-months)

(define (weekday-name days)
  "Return the English name of the weekday of the day number DAYS."
  (vector-ref #("Sunday" "Monday" "Tuesday" "Wednesday" "Thursday" "Friday" "Saturday") (weekday days)))

(define (month-name m)
  "Return the English name of the month M, from 1."
  (vector-ref #("January" "February" "March" "April" "May" "June" "July" "August"
                "September" "October" "November" "December") (- m 1)))

(define (pad2 n) (if (< n 10) (string-append "0" (number->string n)) (number->string n)))

(define (date->string days)
  "Return the day number DAYS as an ISO date, such as \"2026-10-05\"."
  (match (civil-from-days days)
    [(list y m d) (string-append (number->string y) "-" (pad2 m) "-" (pad2 d))]))

(define (parse-iso-date s)
  "Return the day number of the ISO date S, such as \"2026-10-05\", or #f."
  (match (string-split s "-")
    [(list y m d)
     (let ((y (string->number y)) (m (string->number m)) (d (string->number d)))
       (and y m d (<= 1 m 12) (<= 1 d 31) (days-from-civil y m d)))]
    [_ #f]))
