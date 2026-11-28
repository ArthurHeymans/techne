;;; Flexible quota-over-window habit engine for Org mode.
;;;
;;; A habit is a goal over a calendar or rolling window (e.g. "2/week",
;;; "3/7d gap 1d", "1/day on mon wed fri", "90min/week", "<=2/week"),
;;; evaluated purely from timestamped events in `:LOGBOOK:`. Urgency is
;;; feasibility slack (remaining eligible slots minus remaining target).
;;; Classic `STYLE: habit` + `SCHEDULED: <.+N/M>` headings are imported
;;; into the same evaluator with zero migration.

(import (techne time))
(require "date.scm")
(require "syntax.scm")
(require "edit.scm")

(provide org-habit-spec? org-habit-spec-kind org-habit-spec-target org-habit-spec-unit
         org-habit-spec-window-kind org-habit-spec-window-days
         org-habit-spec-weekdays org-habit-spec-gap-minutes org-habit-spec-max-per-day
         org-parse-habit-spec org-section-habit-spec org-heading-habit-spec
         make-org-habit-event org-habit-event? org-habit-event-day org-habit-event-minutes
         org-habit-event-actual-day org-habit-event-amount org-habit-event-unit org-habit-event-kind
         org-section-habit-events org-habit-events
         org-habit-eval? org-habit-eval-spec org-habit-eval-status
         org-habit-eval-progress org-habit-eval-target org-habit-eval-remaining
         org-habit-eval-opportunities org-habit-eval-slack
         org-habit-eval-window-start org-habit-eval-window-end
         org-habit-eval-expires-on org-habit-eval-streak org-habit-eval-attainment
         org-habit-eval-credited org-habit-eval-excluded
         org-habit-eval-strip org-habit-eval-summary
         org-habit-evaluate org-edit-log-habit)

;; ----- Records -----

;; kind: 'at-least | 'at-most
;; unit: 'times | 'minutes | string (e.g. "km")
;; window-kind: 'day | 'week | 'month | 'rolling
;; window-days: integer (for 'rolling, else #f)
;; weekdays: #f (any day) or list of ints 0..6 (0=Sun..6=Sat)
;; gap-minutes: #f or positive int
;; max-per-day: #f or positive int
(define-record-type org-habit-spec
  (make-org-habit-spec kind target unit window-kind window-days weekdays gap-minutes max-per-day)
  org-habit-spec?
  (kind org-habit-spec-kind)
  (target org-habit-spec-target)
  (unit org-habit-spec-unit)
  (window-kind org-habit-spec-window-kind)
  (window-days org-habit-spec-window-days)
  (weekdays org-habit-spec-weekdays)
  (gap-minutes org-habit-spec-gap-minutes)
  (max-per-day org-habit-spec-max-per-day))

;; day: logical day (adjusted by day-start-hour, used for window/quota membership)
;; actual-day & minutes: civil timestamp (used for chronological sorting and gap checks)
(define-record-type org-habit-event
  (%make-org-habit-event day actual-day minutes amount unit kind)
  org-habit-event?
  (day org-habit-event-day)
  (actual-day org-habit-event-actual-day)
  (minutes org-habit-event-minutes)
  (amount org-habit-event-amount)
  (unit org-habit-event-unit)
  (kind org-habit-event-kind))

(define (make-org-habit-event day minutes amount unit kind #:actual-day [actual-day #f])
  "Construct an `org-habit-event` on logical DAY at MINUTES.
AMOUNT, UNIT, and KIND describe the logged completion; ACTUAL-DAY
defaults to DAY for civil elapsed-time ordering and gap checks."
  (%make-org-habit-event day (or actual-day day) minutes amount unit kind))

;; status: 'satisfied | 'on-track | 'at-risk | 'must-today | 'missed | 'over-limit | 'cooldown
;; excluded: list of (event . reason-string)
(define-record-type org-habit-eval
  (make-org-habit-eval spec status progress target remaining opportunities slack
                       win-start win-end expires-on streak attainment
                       credited excluded strip summary)
  org-habit-eval?
  (spec org-habit-eval-spec)
  (status org-habit-eval-status)
  (progress org-habit-eval-progress)
  (target org-habit-eval-target)
  (remaining org-habit-eval-remaining)
  (opportunities org-habit-eval-opportunities)
  (slack org-habit-eval-slack)
  (win-start org-habit-eval-window-start)
  (win-end org-habit-eval-window-end)
  (expires-on org-habit-eval-expires-on)
  (streak org-habit-eval-streak)
  (attainment org-habit-eval-attainment)
  (credited org-habit-eval-credited)
  (excluded org-habit-eval-excluded)
  (strip org-habit-eval-strip)
  (summary org-habit-eval-summary))

;; ----- Parsing :HABIT: specs -----

(define (parse-weekday-name s)
  (let ((d (string-downcase s)))
    (cond ((or (string=? d "sun") (string=? d "sunday")) 0)
          ((or (string=? d "mon") (string=? d "monday")) 1)
          ((or (string=? d "tue") (string=? d "tuesday")) 2)
          ((or (string=? d "wed") (string=? d "wednesday")) 3)
          ((or (string=? d "thu") (string=? d "thursday")) 4)
          ((or (string=? d "fri") (string=? d "friday")) 5)
          ((or (string=? d "sat") (string=? d "saturday")) 6)
          (else #f))))

(define (parse-duration-minutes s)
  ;; "1d" -> 1440, "20h" -> 1200, "30m" / "30min" -> 30
  (let* ((d (string-downcase (string-trim s)))
         (len (string-length d)))
    (cond
      ((and (> len 3) (string-suffix? "min" d))
       (let ((n (string->number (substring d 0 (- len 3)))))
         (and n (exact-integer? n) (> n 0) n)))
      ((and (> len 1) (char=? (string-ref d (- len 1)) #\m))
       (let ((n (string->number (substring d 0 (- len 1)))))
         (and n (exact-integer? n) (> n 0) n)))
      ((and (> len 1) (char=? (string-ref d (- len 1)) #\h))
       (let ((n (string->number (substring d 0 (- len 1)))))
         (and n (exact-integer? n) (> n 0) (* n 60))))
      ((and (> len 1) (char=? (string-ref d (- len 1)) #\d))
       (let ((n (string->number (substring d 0 (- len 1)))))
         (and n (exact-integer? n) (> n 0) (* n 1440))))
      (else #f))))

;; Parse target token like "2", "90min", "2h", "30km" -> (amount . unit) or #f.
(define (parse-target-token tok)
  (let ((len (string-length tok)))
    (let loop ((i 0))
      (if (and (< i len) (char-numeric? (string-ref tok i)))
          (loop (+ i 1))
          (and (> i 0)
               (let ((n (string->number (substring tok 0 i)))
                     (u (string-downcase (substring tok i len))))
                 (and n (> n 0)
                      (cond ((or (string-null? u) (string=? u "x") (string=? u "times")) (cons n 'times))
                            ((or (string=? u "min") (string=? u "m")) (cons n 'minutes))
                            ((or (string=? u "h") (string=? u "hr") (string=? u "hours")) (cons (* n 60) 'minutes))
                            (else (cons n u))))))))))

;; Parse window token: "day", "week", "calendar-week", "month", "calendar-month", "7d", "rolling-7d"
;; -> (window-kind . window-days) or #f.
(define (parse-window-token tok)
  (let ((w (string-downcase tok)))
    (cond
      ((or (string=? w "day") (string=? w "d") (string=? w "calendar-day")) (cons 'day #f))
      ((or (string=? w "week") (string=? w "w") (string=? w "wk") (string=? w "calendar-week")) (cons 'week #f))
      ((or (string=? w "month") (string=? w "calendar-month")) (cons 'month #f))
      ((string-prefix? "rolling-" w)
       (parse-window-token (substring w 8)))
      ((and (> (string-length w) 1) (char=? (string-ref w (- (string-length w) 1)) #\d))
       (let ((n (string->number (substring w 0 (- (string-length w) 1)))))
         (and n (exact-integer? n) (> n 0) (cons 'rolling n))))
      (else #f))))

;; Split spec on whitespace and semicolons/commas for clauses.
(define (tokenize-spec str)
  (filter (lambda (t) (not (string-null? t)))
          (append-map (lambda (w) (string-split w ";")) (string-split (string-trim str)))))

(define (org-parse-habit-spec str)
  "Parse a `:HABIT:` property string STR into an `org-habit-spec`, or #f.
Supports forms such as \"2/week\", \"3/7d gap 1d\",
\"1/day on mon wed fri\", \"90min/week\", and \"<=2/week\"."
  (let ((toks (tokenize-spec str)))
    (and (pair? toks)
         (let* ((first (car toks))
                (kind (if (string-prefix? "<=" first) 'at-most 'at-least))
                (first-clean (if (eq? kind 'at-most) (substring first 2) first))
                (slash (string-index first-clean #\/)))
           (let-values (((target-tok win-tok rest)
                         (cond
                           (slash
                            (values (substring first-clean 0 slash)
                                    (substring first-clean (+ slash 1))
                                    (cdr toks)))
                           ((and (pair? (cdr toks))
                                 (string-ci=? (cadr toks) "per")
                                 (pair? (cddr toks)))
                            (values first-clean (caddr toks) (cdddr toks)))
                           (else (values #f #f '())))))
             (let ((t (and target-tok (parse-target-token target-tok)))
                   (w (and win-tok (parse-window-token win-tok))))
               (and t w
                    (let loop ((rs rest) (weekdays #f) (gap #f) (max-day #f))
                      (if (null? rs)
                          (make-org-habit-spec kind (car t) (cdr t) (car w) (cdr w) weekdays gap max-day)
                          (let ((tok (string-downcase (car rs))))
                            (cond
                              ((string=? tok "gap")
                               (let ((g (and (pair? (cdr rs)) (parse-duration-minutes (cadr rs)))))
                                 (if g (loop (cddr rs) weekdays g max-day) (loop (cdr rs) weekdays gap max-day))))
                              ((string-prefix? "min-gap=" tok)
                               (loop (cdr rs) weekdays (or (parse-duration-minutes (substring tok 8)) gap) max-day))
                              ((string-prefix? "gap=" tok)
                               (loop (cdr rs) weekdays (or (parse-duration-minutes (substring tok 4)) gap) max-day))
                              ((string=? tok "max")
                               (if (pair? (cdr rs))
                                   (let* ((nxt (cadr rs))
                                          (sl (string-index nxt #\/))
                                          (n (string->number (if sl (substring nxt 0 sl) nxt))))
                                     (loop (cddr rs) weekdays gap (if (and n (> n 0)) n max-day)))
                                   (loop (cdr rs) weekdays gap max-day)))
                              ((string-prefix? "max/day=" tok)
                               (let ((n (string->number (substring tok 8))))
                                 (loop (cdr rs) weekdays gap (if (and n (> n 0)) n max-day))))
                              ((or (string=? tok "on") (string-prefix? "on=" tok))
                               (let* ((inline-days (if (string-prefix? "on=" tok)
                                                       (string-split (substring tok 3) ",")
                                                       '()))
                                      (more (if (string=? tok "on") (cdr rs) (cdr rs))))
                                 (if (pair? inline-days)
                                     (let ((ds (filter-map parse-weekday-name inline-days)))
                                       (loop (cdr rs) (if (pair? ds) ds weekdays) gap max-day))
                                     (let collect ((ms more) (ds '()))
                                       (if (pair? ms)
                                           (let* ((parts (string-split (car ms) ","))
                                                  (parsed (filter-map parse-weekday-name parts)))
                                             (if (= (length parsed) (length parts))
                                                 (collect (cdr ms) (append ds parsed))
                                                 (loop ms (if (pair? ds) ds weekdays) gap max-day)))
                                           (loop ms (if (pair? ds) ds weekdays) gap max-day))))))
                              (else (loop (cdr rs) weekdays gap max-day)))))))))))))

(define (org-section-habit-spec elems)
  "Return the `org-habit-spec` from parsed section ELEMS, or #f.
Prefers an explicit `:HABIT:` property; falls back to classic
`:STYLE: habit` with a repeating `SCHEDULED:` timestamp."
  (let ((prop (org-section-property elems "HABIT")))
    (cond
      (prop (org-parse-habit-spec prop))
      ((let ((st (org-section-property elems "STYLE")))
         (and st (string-ci=? st "habit")))
       (let* ((plan (org-section-planning elems))
              (sched (and plan (find (lambda (e) (eq? (car e) 'scheduled)) (car (org-element-data plan)))))
              (rep (and sched (org-timestamp-repeater (list-ref sched 5)))))
         (if (not rep)
             (make-org-habit-spec 'at-least 1 'times 'day #f #f #f #f)
             (let* ((min-n (cadr rep))
                    (unit (caddr rep))
                    (max-n (cadddr rep))
                    (to-days (lambda (n)
                               (case unit ((#\d) n) ((#\w) (* n 7)) ((#\m) (* n 30)) ((#\y) (* n 365)) (else 1))))
                    (min-d (to-days min-n))
                    (win-d (to-days (or max-n min-n)))
                    (gap-m (and max-n (> min-d 1) (* min-d 1440))))
               (if (= win-d 1)
                   (make-org-habit-spec 'at-least 1 'times 'day #f #f #f #f)
                   (make-org-habit-spec 'at-least 1 'times 'rolling win-d #f gap-m #f))))))
      (else #f))))

(define (org-heading-habit-spec text outline h)
  "Return the `org-habit-spec` for heading H in TEXT and OUTLINE, or #f.
Prefers an explicit `:HABIT:` property; falls back to classic
`:STYLE: habit` with a repeating `SCHEDULED:` timestamp."
  (org-section-habit-spec (org-parse-section text (org-heading-body-start h) (org-heading-section-end outline h))))

;; ----- Event extraction from :LOGBOOK: -----

(define (parse-clock-duration s)
  ;; "1:30" -> 90
  (and s
       (match (string-split (string-trim s) ":")
         [(list h m)
          (let ((h (string->number h)) (m (string->number m)))
            (and h m (exact-integer? h) (exact-integer? m) (+ (* h 60) m)))]
         [_ #f])))

(define (parse-habit-log-line line)
  ;; "- Habit: 1 [2026-11-26 Thu 08:15]" or "- Habit: 30min [2026-11-26 Thu 08:15]" or "- Habit: [ts]"
  (let ((t (string-trim line)))
    (and (string-prefix? "- Habit:" t)
         (let* ((lb (string-index t #\[))
                (rb (and lb (string-index (substring t lb) #\])))
                (ts (and rb (parse-org-timestamp (substring t lb (+ lb rb 1)))))
                (mid (and lb (string-trim (substring t 8 lb)))))
           (and ts
                (let* ((first-tok (and (not (string-null? mid)) (car (string-split mid))))
                       (parsed (and first-tok (parse-target-token first-tok)))
                       (amt (if parsed (car parsed) 1))
                       (unit (if parsed (cdr parsed) 'times)))
                  (list ts amt unit)))))))

(define (adjust-logical-day day minutes day-start-hour)
  (if (and minutes (> day-start-hour 0) (< minutes (* day-start-hour 60)))
      (- day 1)
      day))

(define (org-section-habit-events text outline elems #:day-start-hour [day-start-hour 0])
  "Extract chronological `org-habit-event` records from ELEMS in TEXT.
Uses OUTLINE for DONE keywords. Events before DAY-START-HOUR count
toward the previous logical day while preserving civil timestamps."
  (let* ((lb (org-section-logbook elems))
         (done-states (org-outline-done-keywords outline)))
    (if (not lb)
        '()
        (let* ((data (org-element-data lb))
               (hdr-end (cadr data))
               (ftr-start (caddr data))
               (entries (cadddr data))
               (raw-body (org-slice-bytes text (min (+ hdr-end 1) ftr-start) ftr-start))
               (raw-lines (string-split raw-body "\n"))
               (from-entries
                (filter-map
                 (lambda (e)
                   (cond
                     ((and (eq? (car e) 'state) (member (cadr e) done-states))
                      (let* ((ts (list-ref e 3))
                             (aday (org-timestamp-day ts))
                             (mins (org-timestamp-time ts))
                             (lday (adjust-logical-day aday mins day-start-hour)))
                        (make-org-habit-event lday (or mins 720) 1 'times 'done #:actual-day aday)))
                     ((eq? (car e) 'clock)
                      (let* ((ts (cadr e))
                             (aday (org-timestamp-day ts))
                             (dur (parse-clock-duration (caddr e)))
                             (mins (org-timestamp-time ts))
                             (lday (adjust-logical-day aday mins day-start-hour)))
                        (and dur (> dur 0)
                             (make-org-habit-event lday (or mins 720) dur 'minutes 'clock #:actual-day aday))))
                     (else #f)))
                 entries))
               (from-habit-lines
                (filter-map
                 (lambda (line)
                   (let ((p (parse-habit-log-line line)))
                     (and p
                          (let* ((ts (car p))
                                 (aday (org-timestamp-day ts))
                                 (mins (org-timestamp-time ts))
                                 (lday (adjust-logical-day aday mins day-start-hour)))
                            (make-org-habit-event lday (or mins 720) (cadr p) (caddr p) 'habit #:actual-day aday)))))
                 raw-lines))
               (all (append from-entries from-habit-lines)))
          ;; Sort by civil timestamp (actual-day + minutes), NOT logical day!
          (sort all (lambda (a b)
                      (or (< (org-habit-event-actual-day a) (org-habit-event-actual-day b))
                          (and (= (org-habit-event-actual-day a) (org-habit-event-actual-day b))
                               (< (org-habit-event-minutes a) (org-habit-event-minutes b))))))))))

(define (org-habit-events text outline h #:day-start-hour [day-start-hour 0])
  "Extract chronological `org-habit-event` records for heading H in TEXT.
Reads standard DONE state-change lines, `- Habit:` lines, and `CLOCK:`
lines from `:LOGBOOK:` in OUTLINE. Events before DAY-START-HOUR count
toward the previous logical day."
  (org-section-habit-events text outline
                            (org-parse-section text (org-heading-body-start h) (org-heading-section-end outline h))
                            #:day-start-hour day-start-hour))

;; ----- Window & slack computation -----

(define (week-start-of day week-start)
  (let* ((wd (weekday day))
         (diff (modulo (- wd week-start) 7)))
    (- day diff)))

(define (window-bounds spec day week-start)
  (case (org-habit-spec-window-kind spec)
    ((day) (cons day day))
    ((week)
     (let ((s (week-start-of day week-start)))
       (cons s (+ s 6))))
    ((month)
     (match (civil-from-days day)
       [(list y m d)
        (let ((s (days-from-civil y m 1)))
          (cons s (+ s (- (days-in-month y m) 1))))]))
    ((rolling)
     (let ((n (org-habit-spec-window-days spec)))
       (cons (- day (- n 1)) day)))))

(define (prev-window-anchor spec win-start)
  (case (org-habit-spec-window-kind spec)
    ((day) (- win-start 1))
    ((week) (- win-start 7))
    ((month) (- win-start 1))
    ((rolling) (- win-start 1))))

(define (event-total-minutes ev)
  (+ (* (org-habit-event-actual-day ev) 1440) (org-habit-event-minutes ev)))

(define (day-eligible? spec day)
  (let ((wds (org-habit-spec-weekdays spec)))
    (or (not wds) (and (memv (weekday day) wds) #t))))

;; Filter events into (credited . excluded) applying unit, weekday eligibility, gap, and max-per-day.
(define (credit-events spec events)
  (let ((want-unit (org-habit-spec-unit spec))
        (gap (org-habit-spec-gap-minutes spec))
        (max-day (org-habit-spec-max-per-day spec))
        (day-counts (make-hash-table)))
    (let loop ((es events) (last-credited-min #f) (cred '()) (excl '()))
      (if (null? es)
          (cons (reverse cred) (reverse excl))
          (let* ((ev (car es))
                 (ev-min (event-total-minutes ev))
                 (ev-day (org-habit-event-day ev))
                 (dcount (hash-table-ref/default day-counts ev-day 0))
                 (unit-ok? (or (equal? want-unit 'times)
                               (equal? want-unit (org-habit-event-unit ev)))))
            (cond
              ((not unit-ok?)
               (loop (cdr es) last-credited-min cred
                     (cons (cons ev "event unit does not match habit target") excl)))
              ((not (day-eligible? spec ev-day))
               (loop (cdr es) last-credited-min cred
                     (cons (cons ev (string-append (weekday-short-name ev-day) " is not an eligible weekday")) excl)))
              ((and max-day (>= dcount max-day))
               (loop (cdr es) last-credited-min cred
                     (cons (cons ev (string-append "exceeded max " (number->string max-day) "/day cap")) excl)))
              ((and gap last-credited-min (< (- ev-min last-credited-min) gap))
               (loop (cdr es) last-credited-min cred
                     (cons (cons ev (string-append "within " (number->string (quotient gap 60)) "h gap of previous event")) excl)))
              (else
               (hash-table-set! day-counts ev-day (+ dcount 1))
               (loop (cdr es) ev-min (cons ev cred) excl))))))))

;; Count remaining eligible slots in [today, win-end], placing each slot individually
;; against weekday eligibility, gap, and max-per-day.
(define (count-remaining-opportunities spec today now-min win-end credited-events
                                       #:remaining-need [remaining-need 1]
                                       #:day-start-hour [day-start-hour 0])
  (let* ((gap (org-habit-spec-gap-minutes spec))
         (max-day (or (org-habit-spec-max-per-day spec)
                      (if (eq? (org-habit-spec-unit spec) 'times)
                          (max 1 remaining-need)
                          9999)))
         (cutoff (* day-start-hour 60))
         (now-total (if (< now-min cutoff)
                        (+ (* (+ today 1) 1440) now-min)
                        (+ (* today 1440) now-min)))
         (last-ev (and (pair? credited-events) (last credited-events)))
         (last-min (and last-ev (event-total-minutes last-ev)))
         (today-count (length (filter (lambda (e) (= (org-habit-event-day e) today)) credited-events))))
    (let loop ((d today) (prev-min last-min) (slots 0))
      (if (> d win-end)
          slots
          (if (not (day-eligible? spec d))
              (loop (+ d 1) prev-min slots)
              (let* ((used (if (= d today) today-count 0))
                     (avail (max 0 (- max-day used)))
                     (logical-start (+ (* d 1440) cutoff))
                     (day-start-m (if (= d today) (max logical-start now-total) logical-start))
                     (day-end-m (+ (* (+ d 1) 1440) cutoff -1)))
                ;; Place individual slots inside day d respecting gap.
                (let place ((rem avail) (pm prev-min) (added 0))
                  (if (<= rem 0)
                      (loop (+ d 1) pm (+ slots added))
                      (let ((earliest (if (and gap pm) (max day-start-m (+ pm gap)) day-start-m)))
                        (if (<= earliest day-end-m)
                            (place (- rem 1) earliest (+ added 1))
                            (loop (+ d 1) pm (+ slots added))))))))))))

(define (sum-events spec es)
  (if (eq? (org-habit-spec-unit spec) 'times)
      (length es)
      (fold (lambda (e acc) (+ acc (org-habit-event-amount e))) 0 es)))

(define (window-satisfied? spec progress)
  (if (eq? (org-habit-spec-kind spec) 'at-most)
      (<= progress (org-habit-spec-target spec))
      (>= progress (org-habit-spec-target spec))))

;; Compute period streak & attainment over the last N completed windows (plus current if met).
(define (compute-period-stats spec credited today week-start since history-periods)
  (let* ((cur-bounds (window-bounds spec today week-start))
         (cur-prog (sum-events spec (filter (lambda (e) (<= (car cur-bounds) (org-habit-event-day e) (cdr cur-bounds))) credited)))
         (cur-met? (window-satisfied? spec cur-prog))
         (earliest-day (or since (and (pair? credited) (org-habit-event-day (car credited))) today)))
    ;; Walk backwards up to history-periods completed windows before cur-bounds
    (let loop ((k 0) (anchor (prev-window-anchor spec (car cur-bounds)))
               (streak (if cur-met? 1 0)) (streak-open? #t)
               (met-count (if cur-met? 1 0)) (eval-count (if cur-met? 1 0)))
      (if (>= k history-periods)
          (cons streak (cons met-count eval-count))
          (let ((b (window-bounds spec anchor week-start)))
            (if (< (cdr b) earliest-day)
                (cons streak (cons met-count eval-count))
                (let* ((prog (sum-events spec (filter (lambda (e) (<= (car b) (org-habit-event-day e) (cdr b))) credited)))
                       (met? (window-satisfied? spec prog)))
                  (loop (+ k 1) (prev-window-anchor spec (car b))
                        (if (and streak-open? met?) (+ streak 1) streak)
                        (and streak-open? met?)
                        (+ met-count (if met? 1 0))
                        (+ eval-count 1)))))))))

(define (format-period-strip spec credited today week-start)
  (let* ((cur (window-bounds spec today week-start))
         (prev (window-bounds spec (prev-window-anchor spec (car cur)) week-start))
         (render-span
          (lambda (s e)
            (let* ((n (min 14 (+ (- e s) 1)))
                   (start-d (- e (- n 1)))
                   (cells (let loop ((d start-d) (acc '()))
                            (if (> d e)
                                (apply string-append (reverse acc))
                                (let ((done? (any (lambda (ev) (= (org-habit-event-day ev) d)) credited)))
                                  (loop (+ d 1)
                                        (cons (cond (done? "▪")
                                                    ((> d today) "·")
                                                    ((day-eligible? spec d) "·")
                                                    (else " "))
                                              acc))))))
                   (prog (sum-events spec (filter (lambda (ev) (<= s (org-habit-event-day ev) e)) credited)))
                   (tgt (org-habit-spec-target spec))
                   (mark (if (window-satisfied? spec prog) "✓" "")))
              (string-append cells " " (number->string prog) "/" (number->string tgt)
                             (if (string-null? mark) "" (string-append " " mark)))))))
    (if (eq? (org-habit-spec-window-kind spec) 'rolling)
        (render-span (car cur) (cdr cur))
        (string-append (render-span (car prev) (cdr prev)) " │ " (render-span (car cur) (cdr cur))))))

(define (format-unit-amount amt unit)
  (cond ((eq? unit 'times) (number->string amt))
        ((eq? unit 'minutes) (string-append (number->string amt) "m"))
        (else (string-append (number->string amt) unit))))

(define (org-habit-evaluate spec events today
                            #:now-minutes [now-min 720]
                            #:week-start [week-start 1]
                            #:since [since #f]
                            #:history-periods [history-periods 12]
                            #:day-start-hour [day-start-hour 0])
  "Evaluate habit SPEC against EVENTS on civil day TODAY.
NOW-MIN is minutes since midnight, WEEK-START is 0..6 (default 1 for
Monday), SINCE is an optional earliest day number, HISTORY-PERIODS
controls attainment lookback, and DAY-START-HOUR sets the logical day cutoff."
  (let* ((eligible-events (filter (lambda (e) (<= (org-habit-event-day e) today)) events))
         (cred+excl (credit-events spec eligible-events))
         (credited (car cred+excl))
         (excluded (cdr cred+excl))
         (bounds (window-bounds spec today week-start))
         (wstart (car bounds))
         (wend (cdr bounds))
         (win-events (filter (lambda (e) (<= wstart (org-habit-event-day e) (min wend today))) credited))
         (progress (sum-events spec win-events))
         (target (org-habit-spec-target spec))
         (unit (org-habit-spec-unit spec))
         (ceiling? (eq? (org-habit-spec-kind spec) 'at-most))
         (remaining (max 0 (- target progress)))
         (slots (if ceiling?
                    0
                    (count-remaining-opportunities spec today now-min wend credited
                                                   #:remaining-need remaining
                                                   #:day-start-hour day-start-hour)))
         (slack (cond (ceiling? (- target progress))
                      ((not (eq? unit 'times)) (- slots (if (> remaining 0) 1 0)))
                      (else (- slots remaining))))
         ;; For rolling windows, find the first event whose expiry drops progress below target.
         (expires-on
          (and (eq? (org-habit-spec-window-kind spec) 'rolling)
               (not ceiling?)
               (>= progress target)
               (pair? win-events)
               (let ((wlen (org-habit-spec-window-days spec)))
                 (let drop-loop ((es win-events) (rem-prog progress))
                   (if (null? es)
                       #f
                       (let* ((ev (car es))
                              (amt (if (eq? unit 'times) 1 (org-habit-event-amount ev)))
                              (after (- rem-prog amt)))
                         (if (< after target)
                             (+ (org-habit-event-day ev) wlen)
                             (drop-loop (cdr es) after))))))))
         (today-blocked?
          (and (not ceiling?) (> remaining 0)
               (= (count-remaining-opportunities spec today now-min today credited
                                                 #:remaining-need remaining
                                                 #:day-start-hour day-start-hour)
                  0)
               (> slots 0)))
         (must-today?
          (and (not ceiling?) (> remaining 0) (>= slack 0) (not today-blocked?)
               (let ((after-today (count-remaining-opportunities spec (+ today 1) 0 wend credited
                                                                 #:remaining-need remaining
                                                                 #:day-start-hour day-start-hour)))
                 (if (eq? unit 'times)
                     (< after-today remaining)
                     (= after-today 0)))))
         (status
          (cond
            (ceiling? (if (<= progress target) 'satisfied 'over-limit))
            ((>= progress target) 'satisfied)
            ((< slack 0) 'missed)
            ((or (= slack 0) must-today?) 'must-today)
            (today-blocked? 'cooldown)
            ((= slack 1) 'at-risk)
            (else 'on-track)))
         (stats (compute-period-stats spec credited today week-start since history-periods))
         (streak (car stats))
         (attainment (cdr stats))
         (strip (format-period-strip spec credited today week-start))
         (prog-str (string-append (format-unit-amount progress unit) "/" (format-unit-amount target unit)))
         (summary
          (cond
            ((eq? status 'over-limit)
             (string-append prog-str " · over limit"))
            ((and (eq? status 'satisfied) expires-on)
             (string-append prog-str " · drops " (weekday-short-name expires-on)))
            ((eq? status 'satisfied)
             (string-append prog-str " ✓"))
            ((eq? status 'missed)
             (string-append prog-str " · missed"))
            ((eq? status 'must-today)
             (string-append prog-str " · last chance today"))
            ((eq? status 'cooldown)
             (string-append prog-str " · cooldown"))
            (else
             (string-append prog-str " · " (format-unit-amount remaining unit) " left by " (weekday-short-name wend))))))
    (make-org-habit-eval spec status progress target remaining slots slack
                         wstart wend expires-on streak attainment
                         win-events excluded strip summary)))

;; ----- One-keystroke habit logging splice -----

(define (org-edit-log-habit text outline h #:now [now #f] #:amount [amount #f] #:unit [unit #f] #:style [style 'habit])
  "Return edits logging a habit completion on H in TEXT and OUTLINE.
Uses NOW for timestamp, optional AMOUNT and UNIT, and STYLE (`habit` for
`- Habit:` lines or `state` for Emacs `- State \"DONE\"` lines)."
  (let* ((now-val (or now (local-now)))
         (ts-str (format-org-timestamp (make-org-timestamp #f (car now-val) #:time (cdr now-val))))
         (line (if (eq? style 'state)
                   (org-format-state-log "DONE" (or (org-heading-keyword h) "TODO") now-val)
                   (string-append "- Habit: "
                                  (if amount (format-unit-amount amount (or unit 'times)) "1")
                                  " " ts-str)))
         (elems (org-parse-section text (org-heading-body-start h) (org-heading-section-end outline h)))
         (lb (org-section-logbook elems)))
    (if lb
        (let ((ins (+ (cadr (org-element-data lb)) 1)))
          (list (list ins ins (string-append line "\n"))))
        (let* ((props (org-section-properties elems))
               (plan (org-section-planning elems))
               (pos (cond (props (org-element-end props))
                          (plan (org-element-end plan))
                          (else (org-heading-body-start h))))
               (pre (if (or (= pos 0) (string=? (org-slice-bytes text (- pos 1) pos) "\n")) "" "\n")))
          (list (list pos pos (string-append pre ":LOGBOOK:\n" line "\n:END:\n")))))))
