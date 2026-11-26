;;; Calendar dates as day numbers (days since 1970-01-01), as Org writes
;;; them; the calendar is `(techne time)`'s.

(import (techne time))

(provide days-from-civil civil-from-days weekday weekday-name weekday-short-name month-name
         date->string parse-iso-date add-months
         format-time parse-time local-now
         make-org-timestamp org-timestamp?
         org-timestamp-active? org-timestamp-day org-timestamp-time org-timestamp-end-time
         org-timestamp-repeater org-timestamp-warning org-timestamp-end
         parse-org-timestamp format-org-timestamp shift-org-timestamp
         org-shift-timestamp-days org-parse-relative-date)

(define (weekday-name days)
  "Return the English name of the weekday of the day number DAYS."
  (vector-ref #("Sunday" "Monday" "Tuesday" "Wednesday" "Thursday" "Friday" "Saturday") (weekday days)))

(define (weekday-short-name days)
  "Return the three-letter English weekday name of the day number DAYS."
  (vector-ref #("Sun" "Mon" "Tue" "Wed" "Thu" "Fri" "Sat") (weekday days)))

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
       (and y m d (exact-integer? y) (exact-integer? m) (exact-integer? d)
            (<= 1 m 12) (<= 1 d (days-in-month y m)) (days-from-civil y m d)))]
    [_ #f]))

(define (format-time minutes)
  "Format MINUTES since midnight as \"HH:MM\"."
  (string-append (pad2 (quotient minutes 60)) ":" (pad2 (modulo minutes 60))))

(define (parse-time s)
  "Parse \"H:MM\" or \"HH:MM\" in S as minutes since midnight, or #f."
  (match (string-split s ":")
    [(list h m)
     (let ((h (string->number h)) (m (string->number m)))
       (and h m (exact-integer? h) (exact-integer? m) (<= 0 h 23) (<= 0 m 59) (+ (* h 60) m)))]
    [_ #f]))

(define (local-now #:instant [instant #f] #:zone [zone #f])
  "Return local time as (day . minutes) for INSTANT in ZONE.
INSTANT defaults to `current-milliseconds` and ZONE to `local-zone`,
falling back to \"UTC\"."
  (let* ((ms (or instant (current-milliseconds)))
         (z (or zone (guard (e (#t "UTC")) (local-zone))))
         (civ (instant->civil ms z)))
    (cons (days-from-civil (list-ref civ 0) (list-ref civ 1) (list-ref civ 2))
          (+ (* (list-ref civ 3) 60) (list-ref civ 4)))))

;; repeater: #f or (kind min-n unit max-n)
;;   kind: "+" | "++" | ".+"
;;   unit: #\h | #\d | #\w | #\m | #\y
;;   max-n: #f or exact positive integer (for habit ranges like .+2d/4d)
;; warning: #f or (kind n unit)
;;   kind: "-" | "--"
;;   unit: #\h | #\d | #\w | #\m | #\y
(define-record-type org-timestamp
  (%make-org-timestamp active? day time end-time repeater warning end)
  org-timestamp?
  (active? org-timestamp-active?)
  (day org-timestamp-day)
  (time org-timestamp-time)
  (end-time org-timestamp-end-time)
  (repeater org-timestamp-repeater)
  (warning org-timestamp-warning)
  (end org-timestamp-end))

(define (make-org-timestamp active? day #:time [time #f] #:end-time [end-time #f]
                            #:repeater [repeater #f] #:warning [warning #f] #:end [end #f])
  "Construct an Org timestamp record.
ACTIVE? is true for <...> and #f for [...]. DAY is days since epoch.
TIME and END-TIME are minutes since midnight or #f. REPEATER is
(kind min-n unit max-n) or #f. WARNING is (kind n unit) or #f.
END is a second org-timestamp for date ranges or #f."
  (%make-org-timestamp active? day time end-time repeater warning end))

(define (parse-positive-int s)
  (let ((n (string->number s)))
    (and n (exact-integer? n) (> n 0) n)))

(define (parse-unit-spec s)
  ;; "2d" -> (2 #\d) or #f
  (let ((len (string-length s)))
    (and (>= len 2)
         (let ((u (string-ref s (- len 1)))
               (n (parse-positive-int (substring s 0 (- len 1)))))
           (and n (memv u '(#\h #\d #\w #\m #\y)) (list n u))))))

(define (parse-org-repeater tok)
  (let ((kind (cond ((string-prefix? ".+" tok) ".+")
                    ((string-prefix? "++" tok) "++")
                    ((string-prefix? "+" tok) "+")
                    (else #f))))
    (and kind
         (let* ((rest (substring tok (string-length kind)))
                (slash (string-index rest #\/)))
           (if slash
               (let ((min-spec (parse-unit-spec (substring rest 0 slash)))
                     (max-spec (parse-unit-spec (substring rest (+ slash 1)))))
                 (and min-spec max-spec (char=? (cadr min-spec) (cadr max-spec))
                      (list kind (car min-spec) (cadr min-spec) (car max-spec))))
               (let ((spec (parse-unit-spec rest)))
                 (and spec (list kind (car spec) (cadr spec) #f))))))))

(define (parse-org-warning tok)
  (let ((kind (cond ((string-prefix? "--" tok) "--")
                    ((string-prefix? "-" tok) "-")
                    (else #f))))
    (and kind
         (let ((spec (parse-unit-spec (substring tok (string-length kind)))))
           (and spec (list kind (car spec) (cadr spec)))))))

(define (parse-single-timestamp s)
  (let ((len (string-length s)))
    (and (>= len 12)
         (let* ((open (string-ref s 0))
                (close (string-ref s (- len 1)))
                (active? (cond ((and (char=? open #\<) (char=? close #\>)) #t)
                               ((and (char=? open #\[) (char=? close #\])) #f)
                               (else 'bad))))
           (and (not (eq? active? 'bad))
                (let* ((tokens (string-split (substring s 1 (- len 1))))
                       (day (and (pair? tokens) (parse-iso-date (car tokens)))))
                  (and day
                       (let loop ((ts (cdr tokens)) (time #f) (end-time #f) (rep #f) (warn #f))
                         (if (null? ts)
                             (%make-org-timestamp active? day time end-time rep warn #f)
                             (let ((tok (car ts)))
                               (cond
                                 ((and (not rep) (parse-org-repeater tok))
                                  => (lambda (r) (loop (cdr ts) time end-time r warn)))
                                 ((and (not warn) (parse-org-warning tok))
                                  => (lambda (w) (loop (cdr ts) time end-time rep w)))
                                 ((and (not time) (string-index tok #\:))
                                  (let ((dash (string-index tok #\-)))
                                    (if dash
                                        (let ((t1 (parse-time (substring tok 0 dash)))
                                              (t2 (parse-time (substring tok (+ dash 1)))))
                                          (if (and t1 t2)
                                              (loop (cdr ts) t1 t2 rep warn)
                                              #f))
                                        (let ((t1 (parse-time tok)))
                                          (if t1 (loop (cdr ts) t1 #f rep warn) #f)))))
                                 (else (loop (cdr ts) time end-time rep warn)))))))))))))

(define (parse-org-timestamp s)
  "Parse Org timestamp string S into an `org-timestamp`, or #f."
  (let ((len (string-length s)))
    (and (>= len 12)
         (let* ((open (string-ref s 0))
                (close (cond ((char=? open #\<) #\>)
                             ((char=? open #\[) #\])
                             (else #f)))
                (first-close (and close (string-index s close))))
           (and first-close
                (let ((after (+ first-close 1)))
                  (if (and (< (+ after 2) len)
                           (string=? (substring s after (+ after 2)) "--")
                           (memv (string-ref s (+ after 2)) '(#\< #\[)))
                      (let ((start-ts (parse-single-timestamp (substring s 0 after)))
                            (end-ts (parse-single-timestamp (substring s (+ after 2)))))
                        (and start-ts end-ts
                             (%make-org-timestamp (org-timestamp-active? start-ts)
                                                  (org-timestamp-day start-ts)
                                                  (org-timestamp-time start-ts)
                                                  (org-timestamp-end-time start-ts)
                                                  (org-timestamp-repeater start-ts)
                                                  (org-timestamp-warning start-ts)
                                                  end-ts)))
                      (parse-single-timestamp s))))))))

(define (format-single-timestamp ts)
  (let* ((active? (org-timestamp-active? ts))
         (day (org-timestamp-day ts))
         (time (org-timestamp-time ts))
         (end-time (org-timestamp-end-time ts))
         (rep (org-timestamp-repeater ts))
         (warn (org-timestamp-warning ts)))
    (string-append
     (if active? "<" "[")
     (date->string day) " " (weekday-short-name day)
     (if time
         (string-append " " (format-time time)
                        (if end-time (string-append "-" (format-time end-time)) ""))
         "")
     (if rep
         (string-append " " (car rep) (number->string (cadr rep)) (string (caddr rep))
                        (if (cadddr rep)
                            (string-append "/" (number->string (cadddr rep)) (string (caddr rep)))
                            ""))
         "")
     (if warn
         (string-append " " (car warn) (number->string (cadr warn)) (string (caddr warn)))
         "")
     (if active? ">" "]"))))

(define (format-org-timestamp ts)
  "Format the `org-timestamp` TS back to Org syntax."
  (if (org-timestamp-end ts)
      (string-append (format-single-timestamp ts) "--" (format-single-timestamp (org-timestamp-end ts)))
      (format-single-timestamp ts)))

(define (add-days-unit day n unit)
  (case unit
    ((#\d) (+ day n))
    ((#\w) (+ day (* 7 n)))
    ((#\m) (add-months day n))
    ((#\y) (add-months day (* 12 n)))
    (else day)))

(define (shift-day-and-time day time n unit)
  (if (char=? unit #\h)
      ;; Hourly repeaters on a date-only timestamp acquire 00:00 so shifting
      ;; always makes forward progress and never loops forever.
      (let* ((base-min (or time 0))
             (total (+ base-min (* n 60)))
             (extra-days (floor-quotient total 1440))
             (rem-min (floor-remainder total 1440)))
        (cons (+ day extra-days) rem-min))
      (cons (add-days-unit day n unit) time)))

(define (shift-end-timestamp end-ts day-delta min-delta)
  (and end-ts
       (let* ((d (+ (org-timestamp-day end-ts) day-delta))
              (t (org-timestamp-time end-ts))
              (et (org-timestamp-end-time end-ts)))
         (if (and t (not (= min-delta 0)))
             (let* ((tot (+ t min-delta))
                    (ed (+ d (floor-quotient tot 1440)))
                    (nt (floor-remainder tot 1440))
                    (net (and et (floor-remainder (+ et min-delta) 1440))))
               (%make-org-timestamp (org-timestamp-active? end-ts) ed nt net
                                    (org-timestamp-repeater end-ts)
                                    (org-timestamp-warning end-ts)
                                    (org-timestamp-end end-ts)))
             (%make-org-timestamp (org-timestamp-active? end-ts) d t et
                                  (org-timestamp-repeater end-ts)
                                  (org-timestamp-warning end-ts)
                                  (org-timestamp-end end-ts))))))

(define (shift-org-timestamp ts #:now [now #f])
  "Shift repeating `org-timestamp` TS past NOW.
Preserves repeater, warning and duration. NOW may be a day number,
a (day . minutes) pair, or #f for `local-now`."
  (let ((rep (org-timestamp-repeater ts)))
    (if (not rep)
        ts
        (let* ((now-has-time? (or (pair? now) (not now)))
               (now-pair (cond ((pair? now) now)
                               ((exact-integer? now) (cons now 0))
                               (else (local-now))))
               (now-day (car now-pair))
               (now-min (cdr now-pair))
               (kind (car rep))
               (n (cadr rep))
               (unit (caddr rep))
               (orig-day (org-timestamp-day ts))
               (orig-time (org-timestamp-time ts))
               (orig-end (org-timestamp-end-time ts))
               (duration (and orig-time orig-end (- orig-end orig-time)))
               (after-now? (lambda (d t)
                             (if (and t now-has-time?)
                                 (or (> d now-day) (and (= d now-day) (> t now-min)))
                                 (> d now-day))))
               (shifted
                (cond
                  ((string=? kind "+")
                   (shift-day-and-time orig-day orig-time n unit))
                  ((string=? kind ".+")
                   (shift-day-and-time now-day (if (char=? unit #\h) now-min orig-time) n unit))
                  (else ; "++"
                   (let loop ((d orig-day) (t orig-time))
                     (let ((next (shift-day-and-time d t n unit)))
                       (cond
                         ((after-now? (car next) (cdr next)) next)
                         ;; Safety guard: guarantee strict forward progress
                         ((and (= (car next) d) (equal? (cdr next) t)) next)
                         (else (loop (car next) (cdr next)))))))))
               (new-day (car shifted))
               (new-time (cdr shifted))
               (new-end (and duration new-time (floor-remainder (+ new-time duration) 1440)))
               (day-delta (- new-day orig-day))
               (min-delta (if (and orig-time new-time) (- new-time orig-time) 0)))
          (%make-org-timestamp (org-timestamp-active? ts)
                               new-day new-time new-end rep
                               (org-timestamp-warning ts)
                               (shift-end-timestamp (org-timestamp-end ts) day-delta min-delta))))))

(define (org-shift-timestamp-days ts delta-days)
  "Return a new `org-timestamp` shifting TS by DELTA-DAYS.
Preserves active/inactive brackets, time, repeater, warning, and range."
  (let* ((new-day (+ (org-timestamp-day ts) delta-days))
         (end-ts (org-timestamp-end ts))
         (new-end (and end-ts (org-shift-timestamp-days end-ts delta-days))))
    (%make-org-timestamp (org-timestamp-active? ts)
                         new-day
                         (org-timestamp-time ts)
                         (org-timestamp-end-time ts)
                         (org-timestamp-repeater ts)
                         (org-timestamp-warning ts)
                         new-end)))

(define (parse-hhmm-token tok)
  (let ((colon (string-index tok #\:)))
    (and colon (> colon 0) (< (+ colon 1) (string-length tok))
         (let ((hh (string->number (substring tok 0 colon)))
               (mm (string->number (substring tok (+ colon 1)))))
           (and (exact-integer? hh) (exact-integer? mm)
                (<= 0 hh 23) (<= 0 mm 59)
                (+ (* hh 60) mm))))))

(define (parse-time-range-token tok)
  (let ((dash (string-index tok #\-)))
    (if dash
        (let ((t1 (parse-hhmm-token (substring tok 0 dash)))
              (t2 (parse-hhmm-token (substring tok (+ dash 1)))))
          (and t1 t2 (cons t1 t2)))
        (let ((t1 (parse-hhmm-token tok)))
          (and t1 (cons t1 #f))))))

(define (weekday-token->num tok)
  (let ((s (string-downcase tok)))
    (cond
      ((or (string=? s "sun") (string=? s "sunday")) 0)
      ((or (string=? s "mon") (string=? s "monday")) 1)
      ((or (string=? s "tue") (string=? s "tuesday")) 2)
      ((or (string=? s "wed") (string=? s "wednesday")) 3)
      ((or (string=? s "thu") (string=? s "thursday")) 4)
      ((or (string=? s "fri") (string=? s "friday")) 5)
      ((or (string=? s "sat") (string=? s "saturday")) 6)
      (else #f))))

(define (org-parse-relative-date input #:today [today #f] #:base-ts [base-ts #f])
  "Parse INPUT as an Org timestamp (ISO, `+2d`, `fri`, `tomorrow`).
Supports optional `HH:MM`, civil day TODAY (defaulting to local today),
and optional BASE-TS to preserve time, repeater, and warning."
  (let* ((trimmed (string-trim input))
         (ref-day (or today (car (local-now)))))
    (if (string=? trimmed "")
        #f
        ;; If user typed a full Org timestamp <...> or [...], parse directly.
        (if (or (string-prefix? "<" trimmed) (string-prefix? "[" trimmed))
            (parse-org-timestamp trimmed)
            (let* ((toks (filter (lambda (s) (> (string-length s) 0)) (string-split trimmed #\space)))
                   (d-tok (car toks))
                   (rest-toks (cdr toks))
                   (tr (and (pair? rest-toks) (parse-time-range-token (car rest-toks))))
                   (low (string-downcase d-tok))
                   (parsed-day
                    (cond
                      ((parse-iso-date d-tok) => (lambda (d) d))
                      ((string=? low "today") ref-day)
                      ((string=? low "tomorrow") (+ ref-day 1))
                      ((string=? low "yesterday") (- ref-day 1))
                      ((weekday-token->num low)
                       => (lambda (target-wd)
                            (let* ((cur-wd (weekday ref-day))
                                   (diff (modulo (- target-wd cur-wd) 7)))
                              (+ ref-day (if (= diff 0) 7 diff)))))
                      ((and (>= (string-length low) 3)
                            (or (char=? (string-ref low 0) #\+) (char=? (string-ref low 0) #\-)))
                       (let* ((sign (if (char=? (string-ref low 0) #\-) -1 1))
                              (unit (string-ref low (- (string-length low) 1)))
                              (num (string->number (substring low 1 (- (string-length low) 1)))))
                         (and (exact-integer? num)
                              (case unit
                                ((#\d) (+ ref-day (* sign num)))
                                ((#\w) (+ ref-day (* sign num 7)))
                                ((#\m) (add-months ref-day (* sign num)))
                                ((#\y) (add-months ref-day (* sign num 12)))
                                (else #f)))))
                      (else #f))))
              (and parsed-day
                   (%make-org-timestamp
                    (if base-ts (org-timestamp-active? base-ts) #t)
                    parsed-day
                    (if tr (car tr) (and base-ts (org-timestamp-time base-ts)))
                    (if tr (cdr tr) (and base-ts (org-timestamp-end-time base-ts)))
                    (and base-ts (org-timestamp-repeater base-ts))
                    (and base-ts (org-timestamp-warning base-ts))
                    #f)))))))
