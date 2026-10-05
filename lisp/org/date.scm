;;; Calendar dates as day numbers (days since 1970-01-01), using Howard
;;; Hinnant's civil-calendar algorithms.

(provide days-from-civil civil-from-days weekday weekday-name month-name
         date->string parse-iso-date add-months)

(define (days-from-civil y m d)
  (let* ((y (if (<= m 2) (- y 1) y))
         (era (quotient (if (>= y 0) y (- y 399)) 400))
         (yoe (- y (* era 400)))
         (doy (+ (quotient (+ (* 153 (+ m (if (> m 2) -3 9))) 2) 5) (- d 1)))
         (doe (+ (* yoe 365) (quotient yoe 4) (- (quotient yoe 100)) doy)))
    (+ (* era 146097) doe -719468)))

;; Returns (year month day).
(define (civil-from-days z)
  (let* ((z (+ z 719468))
         (era (quotient (if (>= z 0) z (- z 146096)) 146097))
         (doe (- z (* era 146097)))
         (yoe (quotient (- doe (quotient doe 1460) (- (quotient doe 36524)) (quotient doe 146096)) 365))
         (y (+ yoe (* era 400)))
         (doy (- doe (+ (* 365 yoe) (quotient yoe 4) (- (quotient yoe 100)))))
         (mp (quotient (+ (* 5 doy) 2) 153))
         (d (+ (- doy (quotient (+ (* 153 mp) 2) 5)) 1))
         (m (if (< mp 10) (+ mp 3) (- mp 9))))
    (list (if (<= m 2) (+ y 1) y) m d)))

;; 0 = Sunday.
(define (weekday days) (modulo (+ days 4) 7))

(define (weekday-name days)
  (vector-ref #("Sunday" "Monday" "Tuesday" "Wednesday" "Thursday" "Friday" "Saturday") (weekday days)))

(define (month-name m)
  (vector-ref #("January" "February" "March" "April" "May" "June" "July" "August"
                "September" "October" "November" "December") (- m 1)))

(define (pad2 n) (if (< n 10) (string-append "0" (number->string n)) (number->string n)))

;; "2026-10-05"
(define (date->string days)
  (match (civil-from-days days)
    [(list y m d) (string-append (number->string y) "-" (pad2 m) "-" (pad2 d))]))

;; "2026-10-05" -> day number, or #f.
(define (parse-iso-date s)
  (match (string-split s "-")
    [(list y m d)
     (let ((y (string->number y)) (m (string->number m)) (d (string->number d)))
       (and y m d (<= 1 m 12) (<= 1 d 31) (days-from-civil y m d)))]
    [_ #f]))

;; Same day of month, clamped to the month's length.
(define (add-months days n)
  (match (civil-from-days days)
    [(list y m d)
     (let* ((index (+ (* y 12) (- m 1) n))
            (y2 (quotient index 12))
            (m2 (+ (modulo index 12) 1))
            (last (- (if (= m2 12) (days-from-civil (+ y2 1) 1 1) (days-from-civil y2 (+ m2 1) 1))
                     (days-from-civil y2 m2 1))))
       (days-from-civil y2 m2 (min d last)))]))
