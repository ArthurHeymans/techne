;;; Pure-Scheme tests for Slice 3: flexible habits (lisp/org/habit.scm)
;;; and unified query/grouping/proof engine (lisp/org/query.scm).
(import (techne test))
(test-file #:script #t)
(require "../org/date.scm")
(require "../org/syntax.scm")
(require "../org/edit.scm")
(require "../org/habit.scm")
(require "../org/query.scm")

;; ----- 1. Flexible :HABIT: spec parsing -----

(let ((s (org-parse-habit-spec "2/week")))
  (check-true "2/week parsed" (org-habit-spec? s))
  (check "2/week kind" 'at-least (org-habit-spec-kind s))
  (check "2/week target" 2 (org-habit-spec-target s))
  (check "2/week unit" 'times (org-habit-spec-unit s))
  (check "2/week window" 'week (org-habit-spec-window-kind s)))

(let ((s (org-parse-habit-spec "3/7d gap 20h max 1/day")))
  (check "3/7d rolling" 'rolling (org-habit-spec-window-kind s))
  (check "3/7d days" 7 (org-habit-spec-window-days s))
  (check "3/7d gap 20h" 1200 (org-habit-spec-gap-minutes s))
  (check "3/7d max 1/day" 1 (org-habit-spec-max-per-day s)))

(let ((s (org-parse-habit-spec "1/day on mon wed fri")))
  (check "weekday set" '(1 3 5) (org-habit-spec-weekdays s)))

(let ((s (org-parse-habit-spec "90min/week")))
  (check "90min target" 90 (org-habit-spec-target s))
  (check "90min unit" 'minutes (org-habit-spec-unit s)))

(let ((s (org-parse-habit-spec "<=2/week")))
  (check "ceiling kind" 'at-most (org-habit-spec-kind s))
  (check "ceiling target" 2 (org-habit-spec-target s)))

;; ----- 2. Calendar-week flexibility ("2 times a week regardless of when") -----
;; Week of Mon 2026-11-23 .. Sun 2026-11-29.

(define mon (days-from-civil 2026 11 23))
(define wed (days-from-civil 2026 11 25))
(define sat (days-from-civil 2026 11 28))
(define sun (days-from-civil 2026 11 29))

(let* ((doc (string-append
             "* Run :LICHAAM:\n"
             ":PROPERTIES:\n"
             ":HABIT: 2/week\n"
             ":END:\n"
             ":LOGBOOK:\n"
             "- Habit: 1 [2026-11-23 Mon 07:30]\n"
             ":END:\n"))
       (ol (org-parse-outline doc))
       (h (vector-ref (org-outline-headings ol) 0))
       (spec (org-heading-habit-spec doc ol h))
       (events (org-habit-events doc ol h))
       ;; On Wednesday: 1/2 done, 5 days left (Wed..Sun) -> slack = 4 -> on-track!
       (ev-wed (org-habit-evaluate spec events wed))
       ;; On Sunday: 1/2 done, only Sun left -> 1 slot, 1 needed -> slack = 0 -> must-today!
       (ev-sun (org-habit-evaluate spec events sun)))
  (check "2/week Wed progress" 1 (org-habit-eval-progress ev-wed))
  (check "2/week Wed status on-track" 'on-track (org-habit-eval-status ev-wed))
  (check "2/week Wed summary" "1/2 · 1 left by Sun" (org-habit-eval-summary ev-wed))
  (check "2/week Sun status must-today" 'must-today (org-habit-eval-status ev-sun))
  (check "2/week Sun summary" "1/2 · last chance today" (org-habit-eval-summary ev-sun)))

;; Weekday restriction: 2/week on mon..sat (Sun ineligible) -> Sat is must-today!
(let* ((spec (org-parse-habit-spec "2/week on mon tue wed thu fri sat"))
       (events (list (make-org-habit-event mon 480 1 'times 'habit)))
       (ev-sat (org-habit-evaluate spec events sat)))
  (check "Sat with Sun ineligible is must-today" 'must-today (org-habit-eval-status ev-sat))
  (check "Sat slack is 0" 0 (org-habit-eval-slack ev-sat)))

;; ----- 3. Rolling 7-day window & expiry + gap exclusion proof -----

(let* ((spec (org-parse-habit-spec "3/7d gap 20h"))
       ;; Events: Mon 08:00, Mon 19:00 (only 11h gap -> excluded!), Wed 08:00, Fri 08:00
       (events (list (make-org-habit-event mon 480 1 'times 'habit)
                     (make-org-habit-event mon 1140 1 'times 'habit)
                     (make-org-habit-event wed 480 1 'times 'habit)
                     (make-org-habit-event (+ wed 2) 480 1 'times 'habit)))
       (ev-sat (org-habit-evaluate spec events sat)))
  (check "gap 20h excludes same-evening second log" 1 (length (org-habit-eval-excluded ev-sat)))
  (check "rolling 3/7d satisfied with 3 credited" 'satisfied (org-habit-eval-status ev-sat))
  ;; Oldest credited event is Mon 2026-11-23; expires Mon + 7 = Mon 2026-11-30!
  (check "rolling 3/7d expires on next Mon" (+ mon 7) (org-habit-eval-expires-on ev-sat))
  (check "rolling 3/7d summary shows expiry" "3/3 · drops Mon" (org-habit-eval-summary ev-sat)))

;; ----- 4. Ceiling habit (<=2/week) & late-night day-start-hour -----

(let* ((spec (org-parse-habit-spec "<=2/week"))
       (ev-ok (org-habit-evaluate spec (list (make-org-habit-event mon 600 1 'times 'habit)) wed))
       (ev-over (org-habit-evaluate spec (list (make-org-habit-event mon 600 1 'times 'habit)
                                               (make-org-habit-event (+ mon 1) 600 1 'times 'habit)
                                               (make-org-habit-event wed 600 1 'times 'habit))
                                    wed)))
  (check "ceiling <=2/week under limit is satisfied" 'satisfied (org-habit-eval-status ev-ok))
  (check "ceiling <=2/week exceeded is over-limit" 'over-limit (org-habit-eval-status ev-over)))

;; Late-night 00:30 completion with day-start-hour=4 counts toward previous logical day!
(let* ((doc "* Pray :PRAYER:\n:PROPERTIES:\n:HABIT: 1/day\n:END:\n:LOGBOOK:\n- Habit: 1 [2026-11-24 Tue 00:30]\n:END:\n")
       (ol (org-parse-outline doc))
       (h (vector-ref (org-outline-headings ol) 0))
       (evs (org-habit-events doc ol h #:day-start-hour 4)))
  (check "00:30 with day-start-hour=4 counts as Mon" mon (org-habit-event-day (car evs))))

;; Classic STYLE: habit import & one-keystroke org-edit-log-habit splice
(let* ((doc "* TODO Pray twice :PRAYER:\n  SCHEDULED: <2026-11-26 Thu .+2d/4d>\n:PROPERTIES:\n:STYLE: habit\n:END:\n")
       (ol (org-parse-outline doc))
       (h (vector-ref (org-outline-headings ol) 0))
       (spec (org-heading-habit-spec doc ol h))
       (logged (org-apply-edits doc (org-edit-log-habit doc ol h #:now (cons sat 495)))))
  (check "classic habit imports as rolling 4d" 4 (org-habit-spec-window-days spec))
  (check "org-edit-log-habit appends LOGBOOK without closing heading"
         (string-append doc ":LOGBOOK:\n- Habit: 1 [2026-11-28 Sat 08:15]\n:END:\n")
         logged))

;; ----- 5. Unified query, grouping, effort budget & proof explanation -----

(define agenda-doc
  (string-append
   "#+title: Daily\n"
   "* TODO Standup :WORK:\n"
   "  SCHEDULED: <2026-11-28 Sat 10:00-10:30>\n"
   "* Pray :PRAYER:\n"
   ":PROPERTIES:\n"
   ":HABIT: 2/week\n"
   ":END:\n"
   ":LOGBOOK:\n"
   "- Habit: 1 [2026-11-24 Tue 08:00]\n"
   ":END:\n"
   "* TODO Ship Slice 3 :WORK:\n"
   "  SCHEDULED: <2026-11-28 Sat>\n"
   ":PROPERTIES:\n"
   ":EFFORT: 2:00\n"
   ":END:\n"
   "* HOLD Blocked meeting :WORK:\n"
   "  SCHEDULED: <2026-11-28 Sat 14:00-15:00>\n"
   "* TODO Untimed no-effort task\n"
   "  SCHEDULED: <2026-11-28 Sat>\n"
   "* TODO Unscheduled backlog item\n"))

(let* ((ol (org-parse-outline agenda-doc #:todo-keywords '("TODO" "HOLD") #:done-keywords '("DONE")))
       (src (make-org-source "daily.org" agenda-doc ol))
       (view (org-default-day-view))
       (res (org-run-view view (list src) #:today sat #:workday-minutes 480))
       (group-of (lambda (name) (find (lambda (g) (string=? (org-group-result-name g) name)) (org-view-result-groups res))))
       (budget (org-view-result-budget res)))
  ;; HOLD timed meeting must be discarded BEFORE Schedule!
  (check "HOLD discarded globally" 1 (length (org-view-result-discarded res)))
  (check "Schedule has only Standup (not HOLD)" 1 (length (org-group-result-entries (group-of "Schedule"))))
  (check "Habit: ora has Pray" 1 (length (org-group-result-entries (group-of "Habit: ora"))))
  (check "Today: labora has Ship Slice 3" 1 (length (org-group-result-entries (group-of "Today: labora"))))
  (check "Today: other has untimed task" 1 (length (org-group-result-entries (group-of "Today: other"))))
  ;; Effort & capacity budget: 30m booked (10:00-10:30), 450m free, 120m planned, 1 unknown effort!
  (check "budget booked 30m" 30 (cdr (assq 'booked budget)))
  (check "budget free 450m" 450 (cdr (assq 'free budget)))
  (check "budget planned 120m" 120 (cdr (assq 'planned budget)))
  (check "budget unknown effort count" 1 (cdr (assq 'unknown budget)))
  ;; Explain "Why here?" for Ship Slice 3
  (let* ((h-ship (vector-ref (org-outline-headings ol) 2))
         (exp (org-explain-heading view src h-ship #:today sat)))
    (check "explain included heading status" 'included (cdr (assq 'status exp)))
    (check "explain included heading group" "Today: labora"
           (cdr (assq 'group (car (cdr (assq 'occurrences exp)))))))
  ;; Explain "Why missing?" for HOLD meeting
  (let* ((h-hold (vector-ref (org-outline-headings ol) 3))
         (exp (org-explain-heading view src h-hold #:today sat)))
    (check "explain HOLD status" 'excluded (cdr (assq 'status exp)))
    (check "explain HOLD outcome" 'discarded
           (cdr (assq 'outcome (car (cdr (assq 'occurrences exp)))))))
  ;; Explain "Why missing?" for unscheduled backlog item
  (let* ((h-back (vector-ref (org-outline-headings ol) 5))
         (exp (org-explain-heading view src h-back #:today sat)))
    (check "explain unscheduled status" 'excluded (cdr (assq 'status exp)))))

;; ----- 6. Regressions from review -----

;; Timed deadline + timed schedule on the same heading have unique keys & distinct reasons
(let* ((doc (string-append
             "* TODO Release :WORK:\n"
             "  SCHEDULED: <2026-11-28 Sat 10:00-11:00> DEADLINE: <2026-11-28 Sat 16:00>\n"
             ":PROPERTIES:\n:EFFORT: 1:00\n:END:\n"))
       (ol (org-parse-outline doc))
       (src (make-org-source "rel.org" doc ol))
       (res (org-run-view (org-default-day-view) (list src) #:today sat #:workday-minutes 480))
       (sched-g (find (lambda (g) (string=? (org-group-result-name g) "Schedule")) (org-view-result-groups res)))
       (es (org-group-result-entries sched-g))
       (keys (map org-entry-key es))
       (reasons (map org-entry-reason es))
       (budget (org-view-result-budget res)))
  (check "both timed occurrences in Schedule" 2 (length es))
  (check "timed schedule & deadline have distinct keys" #f (string=? (car keys) (cadr keys)))
  (check-true "timed deadline keeps deadline reason" (and (memq 'deadline reasons) #t))
  (check-true "timed deadline matches (deadline today)"
              (any (lambda (e) (org-query-eval '(deadline today) e #:today sat)) es))
  ;; Booked counts 60m (10:00-11:00) + 0m (point deadline)
  (check "booked minutes for timed schedule" 60 (cdr (assq 'booked budget))))

;; 2/day max 2/day gap 20h: second slot cannot fit in the same day -> missed (slots = 1)
(let* ((spec (org-parse-habit-spec "2/day max 2/day gap 20h"))
       (ev (org-habit-evaluate spec '() sat)))
  (check "2/day with 20h gap has only 1 opportunity" 1 (org-habit-eval-opportunities ev))
  (check "2/day with 20h gap is missed" 'missed (org-habit-eval-status ev)))

;; on mon wed fri: off-day completion (Tue) is excluded, not credited
(let* ((spec (org-parse-habit-spec "3/week on mon wed fri"))
       (tue (days-from-civil 2026 11 24))
       (ev (org-habit-evaluate spec (list (make-org-habit-event tue tue 480 1 'times)) wed)))
  (check "off-day event excluded" 0 (org-habit-eval-progress ev))
  (check "off-day event recorded in excluded" 1 (length (org-habit-eval-excluded ev))))

;; Rolling 2/7d with 3 events: expires-on uses the event whose drop puts progress < 2
(let* ((spec (org-parse-habit-spec "2/7d"))
       (d1 (days-from-civil 2026 11 22))
       (d2 (days-from-civil 2026 11 24))
       (d3 (days-from-civil 2026 11 26))
       (ev (org-habit-evaluate spec
                               (list (make-org-habit-event d1 d1 480 1 'times)
                                     (make-org-habit-event d2 d2 480 1 'times)
                                     (make-org-habit-event d3 d3 480 1 'times))
                               sat)))
  (check "rolling 2/7d with 3 events expires when 2nd oldest drops (d2+7)"
         (+ d2 7) (org-habit-eval-expires-on ev)))


