;;; Time: instants, civil dates and times, time zones and RFC 3339.
;;;
;;; An instant is an exact integer, milliseconds since the Unix epoch, as
;;; `current-milliseconds` returns; a duration is a difference of them. A
;;; date is a day number, days since 1970-01-01 (Howard Hinnant's
;;; civil-calendar algorithms). A civil time is a list (year month day
;;; hour minute second millisecond), months and days from 1. A zone is an
;;; IANA name, such as "Europe/Brussels" or "UTC", or a fixed offset from
;;; UTC in seconds. What zones do comes from the IANA database bundled
;;; into Techne (crates/techne-vm/src/time.rs), the same on every machine;
;;; the rest is here.

(define-library (techne time)
  (export days-from-civil civil-from-days weekday days-in-month add-months
          instant->civil civil->instant instant->days zone-offset local-zone
          instant->rfc3339 rfc3339->instant)
  (import (techne))
  (begin

    (define (days-from-civil y m d)
      "Return the day number, days since 1970-01-01, of the date Y M D.
Y is the year, M the month and D the day, from 1."
      (let* ((y (if (<= m 2) (- y 1) y))
             (era (quotient (if (>= y 0) y (- y 399)) 400))
             (yoe (- y (* era 400)))
             (doy (+ (quotient (+ (* 153 (+ m (if (> m 2) -3 9))) 2) 5) (- d 1)))
             (doe (+ (* yoe 365) (quotient yoe 4) (- (quotient yoe 100)) doy)))
        (+ (* era 146097) doe -719468)))

    (define (civil-from-days z)
      "Return the date of the day number Z as (year month day)."
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

    (define (weekday days)
      "Return the weekday of the day number DAYS, 0 for Sunday."
      (modulo (+ days 4) 7))

    (define (days-in-month y m)
      "Return the number of days of month M of year Y."
      (- (if (= m 12) (days-from-civil (+ y 1) 1 1) (days-from-civil y (+ m 1) 1))
         (days-from-civil y m 1)))

    (define (add-months days n)
      "Return the day number N months after DAYS, the same day of the month.
The day is clamped to the month's length."
      (let* ((date (civil-from-days days))
             (index (+ (* (car date) 12) (- (cadr date) 1) n))
             (y (floor-quotient index 12))
             (m (+ (floor-remainder index 12) 1)))
        (days-from-civil y m (min (caddr date) (days-in-month y m)))))

    (define %ms-per-day 86400000)

    (define (zone-offset zone instant)
      "Return the offset of ZONE from UTC at INSTANT, in seconds east."
      (%zone-offset zone instant))

    (define (local-zone)
      "Return the IANA name of the system's time zone.
The world must be granted the environment."
      (%local-zone))

    (define (instant->civil instant . zone)
      "Return the civil time of INSTANT in ZONE, UTC if not given.
It is a list (year month day hour minute second millisecond)."
      (let* ((local (+ instant (* 1000 (%zone-offset (if (pair? zone) (car zone) "UTC") instant))))
             (ms (floor-remainder local %ms-per-day)))
        (append (civil-from-days (floor-quotient local %ms-per-day))
                (list (quotient ms 3600000) (quotient (remainder ms 3600000) 60000)
                      (quotient (remainder ms 60000) 1000) (remainder ms 1000)))))

    (define (instant->days instant . zone)
      "Return the day number of the date of INSTANT in ZONE.
ZONE is UTC if not given."
      (let ((c (apply instant->civil instant zone)))
        (days-from-civil (car c) (cadr c) (caddr c))))

    (define (civil->instant civil . zone)
      "Return the instant the civil time CIVIL is in ZONE, UTC if not given.
CIVIL is a list (year month day hour minute second millisecond); missing
fields at its end are 0. A time that happens twice, as clocks are set
back, is the earlier; one that does not happen, as they are set forward,
is moved forward by the change."
      (let ((fields (append civil (list-tail '(0 0 0 0) (max 0 (- (length civil) 3))))))
        (unless (= (length fields) 7)
          (error "civil->instant: expected (year month day [hour minute second millisecond]):" civil))
        (apply %zone-instant (if (pair? zone) (car zone) "UTC") fields)))

    (define (%digit? c) (and (char<=? #\0 c) (char<=? c #\9)))

    (define (%pad n width)
      (let ((s (number->string n)))
        (string-append (make-string (max 0 (- width (string-length s))) #\0) s)))

    (define (instant->rfc3339 instant . zone)
      "Return INSTANT as RFC 3339 text in ZONE, UTC if not given.
Milliseconds are always written; UTC is written `Z`, other zones as their
offset then, such as `2026-10-16T22:05:00.000+02:00`. An offset RFC 3339
cannot write, not whole minutes (local mean time before 1900) or a day
or more, is an error."
      (let* ((zone (if (pair? zone) (car zone) "UTC"))
             (seconds (%zone-offset zone instant))
             (offset (quotient seconds 60))
             (c (instant->civil instant zone)))
        (unless (and (zero? (remainder seconds 60)) (< (abs seconds) 86400))
          (error "instant->rfc3339: an offset RFC 3339 cannot write, in seconds:" seconds))
        (unless (<= 0 (car c) 9999)
          (error "instant->rfc3339: the year is not one RFC 3339 writes:" (car c)))
        (string-append (%pad (list-ref c 0) 4) "-" (%pad (list-ref c 1) 2) "-" (%pad (list-ref c 2) 2)
                       "T" (%pad (list-ref c 3) 2) ":" (%pad (list-ref c 4) 2) ":" (%pad (list-ref c 5) 2)
                       "." (%pad (list-ref c 6) 3)
                       (if (and (equal? zone "UTC") (= offset 0))
                           "Z"
                           (string-append (if (< offset 0) "-" "+") (%pad (quotient (abs offset) 60) 2)
                                          ":" (%pad (remainder (abs offset) 60) 2))))))

    (define (rfc3339->instant text)
      "Return the instant the RFC 3339 date and time TEXT writes.
TEXT is like `2026-10-16T22:05:00Z` or `2026-10-16 22:05:00.5+02:00`:
fractions of a second past milliseconds are dropped. Anything else, a
leap second included, is an error."
      (define n (string-length text))
      (define (bad) (error "rfc3339->instant: not an RFC 3339 date and time:" text))
      (define (digits at count)
        (if (and (<= (+ at count) n)
                 (let loop ((i at)) (or (= i (+ at count)) (and (%digit? (string-ref text i)) (loop (+ i 1))))))
            (string->number (substring text at (+ at count)))
            (bad)))
      (define (expect at chars)
        (unless (and (< at n) (memv (string-ref text at) chars)) (bad)))
      (expect 4 '(#\-)) (expect 7 '(#\-)) (expect 10 '(#\T #\t #\space)) (expect 13 '(#\:)) (expect 16 '(#\:))
      (let* ((y (digits 0 4)) (mo (digits 5 2)) (d (digits 8 2)) (h (digits 11 2)) (mi (digits 14 2)) (s (digits 17 2))
             ;; The fraction's digits run to the zone.
             (zone-at (let loop ((i 19))
                        (if (or (= i n)
                                    (if (= i 19) (char=? (string-ref text i) #\.) (%digit? (string-ref text i))))
                            (if (= i n) (bad) (loop (+ i 1)))
                            i)))
             (fraction (substring text (min 20 zone-at) zone-at))
             (ms (if (= zone-at 19)
                     0
                     (if (string=? fraction "") (bad) (string->number (substring (string-append fraction "00") 0 3)))))
             (offset (if (memv (string-ref text zone-at) '(#\Z #\z))
                         (if (= (+ zone-at 1) n) 0 (bad))
                         (begin (expect zone-at '(#\+ #\-))
                                (unless (= (+ zone-at 6) n) (bad))
                                (expect (+ zone-at 3) '(#\:))
                                (let ((oh (digits (+ zone-at 1) 2)) (om (digits (+ zone-at 4) 2)))
                                  (unless (and (< oh 24) (< om 60)) (bad))
                                  (* (if (char=? (string-ref text zone-at) #\-) -1 1) (+ (* 60 oh) om)))))))
        (unless (and (<= 1 mo 12) (<= 1 d (days-in-month y mo)) (< h 24) (< mi 60) (< s 60)) (bad))
        (- (+ (* (days-from-civil y mo d) %ms-per-day) (* 1000 (+ (* 3600 h) (* 60 mi) s)) ms)
           (* 60000 offset))))))
