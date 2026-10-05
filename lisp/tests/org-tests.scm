(require "../test.scm")
(require "../org/date.scm")
(require "../org/org.scm")
(require "../org/agenda.scm")

(define fixture "tests/fixtures/agenda.org")
(define text (file->string fixture))
(define org (read-org-file fixture))
(define today (days-from-civil 2026 10 5))
(define (find-heading title) (find (lambda (h) (string=? (heading-title h) title)) (all-headings org)))

;; Dates
(check "epoch" 0 (days-from-civil 1970 1 1))
(check "civil round trip" '(2024 2 29) (civil-from-days (days-from-civil 2024 2 29)))
(check "weekday" "Monday" (weekday-name today))
(check "add-months clamps" "2025-02-28" (date->string (add-months (days-from-civil 2025 1 31) 1)))

;; Parsing
(check "title" "Agenda fixture" (org-file-title org))
(check "todo states" '("TODO" "NEXT" "WAIT") (org-file-todo-states org))
(check "done states" '("DONE" "CANCELLED") (org-file-done-states org))
(check "top-level count" 9 (length (org-file-headings org)))
(check "all headings" 12 (length (all-headings org)))
(let ((h (find-heading "Write report")))
  (check "keyword" "TODO" (heading-keyword h))
  (check "priority" #\A (heading-priority h))
  (check "tags" '("work") (heading-tags h))
  (check "scheduled time" 600 (timestamp-time (heading-scheduled h)))
  (check "heading line" "* TODO [#A] Write report :work:" (heading-line h)))
(let ((h (find-heading "Weekly review")))
  (check "indented planning repeater" '("+" 1 #\w) (timestamp-repeater (heading-scheduled h))))
(let ((p (find-heading "Project")))
  (check "no keyword" #f (heading-keyword p))
  (check "properties" '(("OWNER" . "arthur") ("EFFORT" . "2:00")) (heading-properties p))
  (check "children" '("Subtask one" "Subtask two") (map heading-title (heading-children p)))
  (check "grandchild" '("Deep task") (map heading-title (heading-children (cadr (heading-children p))))))
(check "time range" '(570 . 630)
       (let ((ts (heading-scheduled (find-heading "Subtask one")))) (cons (timestamp-time ts) (timestamp-end-time ts))))
(check "timestamp->string" "<2026-10-06 Tue 09:30-10:30>" (timestamp->string (heading-scheduled (find-heading "Subtask one"))))

;; Lossless writing
(check "round trip" text (org->string org))

;; Agenda for the week of 2026-10-05
;; Repeating tasks whose date has passed are overdue today (they move only when
;; marked done); future repeats appear on their days.
(define expected-agenda
  (string-append
   "Monday     5 October 2026\n"
   "  agenda:     10:00...... Scheduled:  TODO [#A] Write report   :work:\n"
   "  agenda:                 2 d. ago:   WAIT Reply to email   :work:mail:\n"
   "  agenda:                 In 4 d.:    TODO Pay rent\n"
   "  agenda:                 Sched.3x:   NEXT Weekly review   :home:\n"
   "  agenda:                 Sched.5x:   TODO Water plants\n"
   "  agenda:                 Sched.28x:  TODO Monthly invoice\n"
   "  agenda:                 Scheduled:  Plain heading without keyword\n"
   "Tuesday    6 October 2026\n"
   "  agenda:     9:30......  Scheduled:  TODO [#C] Subtask one   :work:\n"
   "  agenda:                 Scheduled:  TODO Water plants\n"
   "Wednesday  7 October 2026\n"
   "  agenda:                 Scheduled:  TODO Deep task\n"
   "  agenda:                 Scheduled:  TODO Monthly invoice\n"
   "Thursday   8 October 2026\n"
   "Friday     9 October 2026\n"
   "  agenda:                 Deadline:   TODO Pay rent\n"
   "  agenda:                 Scheduled:  NEXT Weekly review   :home:\n"
   "  agenda:                 Scheduled:  TODO Water plants\n"))
(check "agenda" expected-agenda (agenda->string (agenda (list org) #:start today #:span 5)))

;; Queries
(check "todo list" '("Write report" "Weekly review" "Reply to email" "Pay rent" "Water plants" "Subtask one" "Deep task" "Monthly invoice")
       (map (lambda (e) (heading-title (cdr e))) (todo-list (list org))))
(check "tags +work-mail" '("Write report" "Subtask one")
       (map (lambda (e) (heading-title (cdr e))) (tags-match (list org) "+work-mail")))

;; Editing
(let ((h (find-heading "Write report")))
  (set-todo! org h "DONE" #:today today)
  (check "done keyword" "DONE" (heading-keyword h))
  (check "closed added" "SCHEDULED: <2026-10-05 Mon 10:00> CLOSED: [2026-10-05 Mon]" (cadr (heading-lines h)))
  (set-todo! org h "TODO" #:today today)
  (check "reopen removes closed" "SCHEDULED: <2026-10-05 Mon 10:00>" (cadr (heading-lines h))))
(let ((h (find-heading "Weekly review")))
  (set-todo! org h "DONE" #:today today)
  (check "repeat keeps open" "NEXT" (heading-keyword h))
  (check "repeat shifts +1w, keeps indent" "  SCHEDULED: <2026-10-09 Fri +1w>" (cadr (heading-lines h))))
(let ((h (find-heading "Water plants")))
  (set-todo! org h "DONE" #:today today)
  (check "repeat .+3d from today" "SCHEDULED: <2026-10-08 Thu .+3d>" (cadr (heading-lines h))))
(let ((h (find-heading "Monthly invoice")))
  (set-todo! org h "DONE" #:today today)
  (check "repeat +1m once" "SCHEDULED: <2026-10-07 Wed +1m>" (cadr (heading-lines h))))
(let ((h (find-heading "Pay rent")))
  (set-todo! org h "CANCELLED" #:today today)
  (check "cancelled leaves agenda" #f
         (any (lambda (item) (eq? (agenda-item-heading item) h))
              (append-map cdr (agenda (list org) #:start today #:span 7)))))
(check "edited file reparses" 12 (length (all-headings (parse-org (string-split (org->string org) "\n")))))

(test-summary)
