;;; (techne time): the calendar, zones across daylight saving changes, and
;;; RFC 3339.

(import (techne time))

(test-begin "time")

;; The calendar, far from the epoch too.
(test 0 (days-from-civil 1970 1 1))
(test '(2024 2 29) (civil-from-days (days-from-civil 2024 2 29)))
(test '(-1 12 31) (civil-from-days (- (days-from-civil 0 1 1) 1)))
(test 4 (weekday 0))
(test '(29 28 31 30) (map (lambda (y m) (days-in-month y m)) '(2024 2100 2026 2026) '(2 2 1 4)))
;; Months clamp the day, and go back across years.
(test '(2026 2 28) (civil-from-days (add-months (days-from-civil 2026 1 31) 1)))
(test '(2025 2 28) (civil-from-days (add-months (days-from-civil 2024 2 29) 12)))
(test '(2025 11 30) (civil-from-days (add-months (days-from-civil 2026 3 31) -4)))

;; Brussels sets its clocks forward at 2026-03-29 01:00 UTC and back at
;; 2026-10-25 01:00 UTC.
(define z "Europe/Brussels")
(test 3600 (zone-offset z (civil->instant '(2026 3 29 0 59 59 999))))
(test 7200 (zone-offset z (civil->instant '(2026 3 29 1))))
(test '(2026 3 29 3 0 0 0) (instant->civil (civil->instant '(2026 3 29 1)) z))
;; 02:30 does not happen that day: it moves forward by the change.
(test "2026-03-29T03:30:00.000+02:00" (instant->rfc3339 (civil->instant '(2026 3 29 2 30) z) z))
;; 02:30 happens twice in October: the earlier.
(test "2026-10-25T02:30:00.000+02:00" (instant->rfc3339 (civil->instant '(2026 10 25 2 30) z) z))
(test "2026-10-25T02:30:00.000+01:00" (instant->rfc3339 (+ (civil->instant '(2026 10 25 2 30) z) 3600000) z))
;; A day's date depends on the zone.
(define late (civil->instant '(2026 10 16 23 30)))
(test (list (days-from-civil 2026 10 16) (days-from-civil 2026 10 17))
      (list (instant->days late) (instant->days late z)))
;; Fixed offsets.
(test '(2026 10 16 18 30 0 0) (instant->civil late (* -5 3600)))
(test-error (zone-offset "Nowhere/Atlantis" 0))
(test-error (civil->instant '(2026 2 30)))
(test-error (civil->instant '(2026 2)))

;; RFC 3339.
(test "1970-01-01T00:00:00.000Z" (instant->rfc3339 0))
(test "1969-12-31T23:59:59.999Z" (instant->rfc3339 -1))
(test "2026-10-16T19:05:00.000-03:30" (instant->rfc3339 (rfc3339->instant "2026-10-16T22:35:00Z") (* -210 60)))
(test 0 (rfc3339->instant "1970-01-01T00:00:00Z"))
(test (rfc3339->instant "2026-10-16T20:05:00.5Z") (rfc3339->instant "2026-10-16 22:05:00.500999+02:00"))
(test (rfc3339->instant "2026-10-16T22:05:00z") (rfc3339->instant "2026-10-16t22:05:00Z"))
(for-each (lambda (bad) (test-error (rfc3339->instant bad)))
          '("2026-10-16" "2026-10-16T22:05Z" "2026-10-16T22:05:00" "2026-10-16T22:05:60Z"
            "2026-02-29T00:00:00Z" "2026-13-01T00:00:00Z" "2026-10-16T22:05:00.Z" "2026-10-16T22:05:001Z"
            "2026-10-16T22:05:00+0200" "2026-10-16T22:05:00+24:00" "2026-10-16T22:05:00Zx" "+2026-10-16T22:05:00Z"
            "1970-01-01T00:00:00.123\x00b2;Z" "\x0661;970-01-01T00:00:00Z"))
(test-error (instant->rfc3339 (civil->instant '(10000 1 1))))
;; Offsets RFC 3339 cannot write: seconds, a day; and Brussels' local mean
;; time in 1880, 17 minutes 30 seconds ahead of UTC.
(test-error (instant->rfc3339 0 1))
(test-error (instant->rfc3339 0 86400))
(test-error (instant->rfc3339 (civil->instant '(1880 1 1)) z))
;; Round trips.
(for-each (lambda (t) (test t (rfc3339->instant (instant->rfc3339 t z))))
          (list 0 -1 late 1774747800000 (civil->instant '(9999 12 1))))

(test-end)
