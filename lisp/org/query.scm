;;; Unified Org query, grouping, effort-budget and proof-explanation engine.
;;;
;;; Replaces separate agenda, super-agenda, and org-ql DSLs with one
;;; inspectable s-expression pipeline:
;;;   sources -> occurrence expansion -> query (+proof) -> groups -> sort -> budget.
;;;
;;; Every predicate is plain Scheme data, so the same definition powers
;;; fast evaluation, "why is this item here?" and "why is this heading missing?".

(require "date.scm")
(require "syntax.scm")
(require "habit.scm")

(provide org-source? make-org-source org-source-path org-source-text org-source-outline
         org-entry? org-entry-source org-entry-heading org-entry-reason
         org-entry-day org-entry-time org-entry-end-time org-entry-effort
         org-entry-tags org-entry-category org-entry-habit org-entry-label org-entry-key
         org-parse-effort-minutes org-format-duration
         org-expand-entries
         org-query-eval org-query-explain
         org-group-spec? make-org-group-spec org-group-spec-name org-group-spec-pred
         org-group-spec-render org-group-spec-multi?
         org-view-spec? make-org-view-spec org-view-spec-name org-view-spec-discard
         org-view-spec-groups org-view-spec-span
         org-view-result? org-view-result-groups org-view-result-discarded
         org-view-result-budget org-group-result? org-group-result-name
         org-group-result-render org-group-result-entries org-group-result-effort
         org-group-result-unknown-effort
         org-run-view org-explain-heading org-default-day-view
         org-search-headings)

;; ----- Records -----

(define-record-type org-source
  (make-org-source path text outline)
  org-source?
  (path org-source-path)
  (text org-source-text)
  (outline org-source-outline))

(define-record-type org-entry
  (make-org-entry source heading reason day time end-time effort tags category habit label key)
  org-entry?
  (source org-entry-source)
  (heading org-entry-heading)
  (reason org-entry-reason)
  (day org-entry-day)
  (time org-entry-time)
  (end-time org-entry-end-time)
  (effort org-entry-effort)
  (tags org-entry-tags)
  (category org-entry-category)
  (habit org-entry-habit)
  (label org-entry-label)
  (key org-entry-key))

(define-record-type org-group-spec
  (%make-org-group-spec name pred render multi?)
  org-group-spec?
  (name org-group-spec-name)
  (pred org-group-spec-pred)
  (render org-group-spec-render)
  (multi? org-group-spec-multi?))

(define (make-org-group-spec name pred #:render [render 'list] #:multi? [multi? #f])
  "Construct an `org-group-spec` named NAME matching PRED.
RENDER is `list`, `time-grid`, or `habit-strip`. With MULTI?, matched
entries may also appear in later groups."
  (%make-org-group-spec name pred render multi?))

(define-record-type org-view-spec
  (%make-org-view-spec name discard groups span)
  org-view-spec?
  (name org-view-spec-name)
  (discard org-view-spec-discard)
  (groups org-view-spec-groups)
  (span org-view-spec-span))

(define (make-org-view-spec name groups #:discard [discard #f] #:span [span 1])
  "Construct an `org-view-spec` named NAME with ordered GROUPS.
DISCARD is an optional global exclusion predicate evaluated before
group routing. SPAN is the number of days to expand."
  (%make-org-view-spec name discard groups span))

(define-record-type org-group-result
  (make-org-group-result name render entries effort unknown)
  org-group-result?
  (name org-group-result-name)
  (render org-group-result-render)
  (entries org-group-result-entries)
  (effort org-group-result-effort)
  (unknown org-group-result-unknown-effort))

;; budget: alist ((workday . M) (booked . M) (free . M) (planned . M) (unknown . N))
(define-record-type org-view-result
  (make-org-view-result groups discarded budget)
  org-view-result?
  (groups org-view-result-groups)
  (discarded org-view-result-discarded)
  (budget org-view-result-budget))

;; ----- Effort & duration helpers -----

(define (org-parse-effort-minutes s)
  "Parse an Org EFFORT string S such as \"1:30\" or \"45\" into minutes, or #f."
  (and (string? s)
       (let ((t (string-trim s)))
         (cond
           ((string-index t #\:)
            => (lambda (colon)
                 (let ((h (string->number (substring t 0 colon)))
                       (m (string->number (substring t (+ colon 1)))))
                   (and h m (exact-integer? h) (exact-integer? m) (>= h 0) (<= 0 m 59) (+ (* h 60) m)))))
           (else
            (let ((m (string->number t)))
              (and m (exact-integer? m) (>= m 0) m)))))))

(define (org-format-duration minutes)
  "Format MINUTES as \"H:MM\"."
  (let ((h (quotient minutes 60))
        (m (modulo minutes 60)))
    (string-append (number->string h) ":" (if (< m 10) "0" "") (number->string m))))

;; ----- Occurrence expansion -----

(define (warning-days-for ts default-warning)
  (let ((w (org-timestamp-warning ts)))
    (if (not w)
        default-warning
        (let ((n (cadr w)) (u (caddr w)))
          (case u ((#\d) n) ((#\w) (* n 7)) ((#\m) (* n 30)) ((#\y) (* n 365)) (else default-warning))))))

(define (heading-stable-key src h id reason day ord)
  (string-append (or id (string-append (or (org-source-path src) "<buf>") ":" (number->string (org-heading-index h))))
                 "|" (symbol->string reason) "|" (number->string day) "|" (number->string ord)))

(define (advance-day-by-unit day n unit)
  (case unit
    ((#\d) (+ day n))
    ((#\w) (+ day (* n 7)))
    ((#\m) (add-months day n))
    ((#\y) (add-months day (* n 12)))
    (else #f)))

(define (project-repeater-occurrences ts from-day to-day)
  ;; Project a timestamp TS (date range or repeater, including hourly) into [FROM-DAY, TO-DAY].
  ;; Returns a sorted list of (day . time) pairs.
  (let ((rep (org-timestamp-repeater ts))
        (base (org-timestamp-day ts))
        (t (org-timestamp-time ts))
        (end-ts (org-timestamp-end ts)))
    (cond
      ((not rep)
       (if end-ts
           (let* ((end-d (org-timestamp-day end-ts))
                  (lo (max from-day base))
                  (hi (min to-day end-d)))
             (let loop ((d lo) (acc '()))
               (if (> d hi)
                   (reverse acc)
                   (loop (+ d 1) (cons (cons d (if (= d base) t #f)) acc)))))
           (if (<= from-day base to-day) (list (cons base t)) '())))
      (else
       (let ((n (cadr rep))
             (u (caddr rep)))
         (cond
           ((<= n 0)
            (if (<= from-day base to-day) (list (cons base t)) '()))
           ((char=? u #\h)
            (let* ((step (* n 60))
                   (base-m (+ (* base 1440) (or t 0)))
                   (from-m (* from-day 1440))
                   (to-m (+ (* to-day 1440) 1439))
                   (start-m (if (< base-m from-m)
                                (+ base-m (* (quotient (- from-m base-m) step) step))
                                base-m)))
              (let loop ((m start-m) (acc '()))
                (cond
                  ((> m to-m) (reverse acc))
                  ((>= m from-m)
                   (loop (+ m step) (cons (cons (quotient m 1440) (modulo m 1440)) acc)))
                  (else (loop (+ m step) acc))))))
           ((memv u '(#\d #\w #\m #\y))
            (let* ((start
                    (if (< base from-day)
                        (case u
                          ((#\d) (+ base (* (quotient (- from-day base) n) n)))
                          ((#\w) (let ((step (* n 7)))
                                   (+ base (* (quotient (- from-day base) step) step))))
                          ((#\m #\y)
                           (let* ((bc (civil-from-days base))
                                  (fc (civil-from-days from-day))
                                  (step-m (if (char=? u #\y) (* n 12) n))
                                  (diff-m (+ (* (- (car fc) (car bc)) 12) (- (cadr fc) (cadr bc))))
                                  (steps (max 0 (quotient (- diff-m 1) step-m))))
                             (if (> steps 0)
                                 (add-months base (* steps step-m))
                                 base)))
                          (else base))
                        base)))
              (let loop ((cur start) (acc '()))
                (cond
                  ((> cur to-day) (reverse acc))
                  ((>= cur from-day)
                   (let ((nxt (advance-day-by-unit cur n u)))
                     (if (or (not nxt) (<= nxt cur))
                         (reverse (cons (cons cur t) acc))
                         (loop nxt (cons (cons cur t) acc)))))
                  (else
                   (let ((nxt (advance-day-by-unit cur n u)))
                     (if (or (not nxt) (<= nxt cur))
                         (reverse acc)
                         (loop nxt acc))))))))
           (else
            (if (<= from-day base to-day) (list (cons base t)) '()))))))))

(define (expand-heading-occurrences src h today span warning-days day-start-hour now-min week-start
                                    #:skip-scheduled-if-deadline-shown [skip-sched? #t])
  (let* ((text (org-source-text src))
         (ol (org-source-outline src))
         (done? (org-heading-done? ol h)))
    (if done?
        '()
        ;; Parse the heading's section ONCE and reuse its properties, planning, and logbook!
        (let* ((elems (org-parse-section text (org-heading-body-start h) (org-heading-section-end ol h)))
               (id (org-section-property elems "ID"))
               (tags (org-heading-all-tags ol h))
               (cat (or (org-section-property elems "CATEGORY")
                        (org-outline-category ol #:path (org-source-path src))))
               (effort (org-parse-effort-minutes (org-section-property elems "EFFORT")))
               (plan (org-section-planning elems))
               (plan-entries (if plan (car (org-element-data plan)) '()))
               (sched-e (find (lambda (e) (eq? (car e) 'scheduled)) plan-entries))
               (dead-e (find (lambda (e) (eq? (car e) 'deadline)) plan-entries))
               (sched (and sched-e (list-ref sched-e 5)))
               (dead (and dead-e (list-ref dead-e 5)))
               (hspec (org-section-habit-spec elems))
               (end-day (+ today (- span 1)))
               (ord 0)
               (make-e (lambda (reason day time end-time habit label)
                         (let ((k (heading-stable-key src h id reason day ord)))
                           (set! ord (+ ord 1))
                           (make-org-entry src h reason day time end-time effort tags cat habit label k)))))
          (if hspec
              ;; Flexible or classic habit: one entry carrying evaluated slack & strip.
              (let* ((events (org-section-habit-events text ol elems #:day-start-hour day-start-hour))
                     (since-str (org-section-property elems "HABIT_SINCE"))
                     (since (and since-str (parse-iso-date since-str)))
                     (logical-today (if (and (> day-start-hour 0) (< now-min (* day-start-hour 60)))
                                        (- today 1)
                                        today))
                     (hev (org-habit-evaluate hspec events logical-today
                                              #:now-minutes now-min #:week-start week-start #:since since
                                              #:day-start-hour day-start-hour)))
                (list (make-e 'habit logical-today (and sched (org-timestamp-time sched))
                              (and sched (org-timestamp-end-time sched))
                              hev (org-habit-eval-summary hev))))
              (let ((acc '())
                    (dead-shown? #f))
                ;; Deadline occurrences (including projected repeater dates in [today, end-day])
                (when dead
                  (let* ((dday (org-timestamp-day dead))
                         (wday (warning-days-for dead warning-days))
                         (t (org-timestamp-time dead))
                         (et (org-timestamp-end-time dead))
                         (proj (if (or (org-timestamp-repeater dead) (org-timestamp-end dead))
                                   (project-repeater-occurrences dead today end-day)
                                   '())))
                    (cond
                      ((< dday today)
                       (set! dead-shown? #t)
                       (set! acc (cons (make-e 'deadline-past today t et #f
                                               (string-append (number->string (- today dday)) " d. ago"))
                                       acc)))
                      ((<= today dday end-day)
                       (set! dead-shown? #t)
                       (set! acc (cons (make-e 'deadline dday t et #f
                                               (if (= dday today) "Due today" (string-append "In " (number->string (- dday today)) " d.")))
                                       acc)))
                      ((<= (- dday today) wday)
                       (set! dead-shown? #t)
                       (set! acc (cons (make-e 'deadline-soon today t et #f
                                               (string-append "In " (number->string (- dday today)) " d."))
                                       acc))))
                    (for-each (lambda (occ)
                                (let* ((pd (car occ))
                                       (pt (cdr occ))
                                       (pet (and et pt (if t (+ pt (- et t)) et))))
                                  (unless (and (= pd dday) (equal? pt t))
                                    (set! dead-shown? #t)
                                    (set! acc (cons (make-e 'deadline pd pt pet #f
                                                            (if (= pd today) "Due today" (string-append "In " (number->string (- pd today)) " d.")))
                                                    acc)))))
                              proj)))
                ;; Scheduled occurrences (skipping untimed scheduled when deadline is already shown)
                (when sched
                  (let* ((sday (org-timestamp-day sched))
                         (t (org-timestamp-time sched))
                         (et (org-timestamp-end-time sched))
                         (suppress? (and skip-sched? dead-shown? (not t)))
                         (proj (if (or (org-timestamp-repeater sched) (org-timestamp-end sched))
                                   (project-repeater-occurrences sched today end-day)
                                   '())))
                    (unless suppress?
                      (cond
                        ((< sday today)
                         (set! acc (cons (make-e 'scheduled-past today t et #f
                                                 (string-append "Sched." (number->string (- today sday)) "x"))
                                         acc)))
                        ((<= today sday end-day)
                         (set! acc (cons (make-e 'scheduled sday t et #f
                                                 (if t (format-time t) "Scheduled"))
                                         acc))))
                      (for-each (lambda (occ)
                                  (let* ((pd (car occ))
                                         (pt (cdr occ))
                                         (pet (and et pt (if t (+ pt (- et t)) et))))
                                    (unless (and (= pd sday) (equal? pt t))
                                      (set! acc (cons (make-e 'scheduled pd pt pet #f
                                                              (if pt (format-time pt) "Scheduled"))
                                                      acc)))))
                                proj))))
                ;; Active inline timestamps in heading body (skipping planning line & exact duplicates)
                (let ((body-from (if plan (org-element-end plan) (org-heading-body-start h)))
                      (body-to (org-heading-section-end ol h)))
                  (when (< body-from body-to)
                    (let ((inlines (org-parse-inline text body-from body-to)))
                      (for-each
                       (lambda (in)
                         (when (eq? (org-inline-kind in) 'timestamp-active)
                           (let* ((ts (org-inline-data in))
                                  (t (org-timestamp-time ts))
                                  (et (org-timestamp-end-time ts))
                                  (occs (project-repeater-occurrences ts today end-day)))
                             (for-each
                              (lambda (occ)
                                (let* ((d (car occ))
                                       (pt (cdr occ))
                                       (pet (and et pt (if t (+ pt (- et t)) et)))
                                       (dup-sched? (and sched (= d (org-timestamp-day sched)) (equal? pt (org-timestamp-time sched))))
                                       (dup-dead? (and dead (= d (org-timestamp-day dead)) (equal? pt (org-timestamp-time dead)))))
                                  (unless (or dup-sched? dup-dead?)
                                    (set! acc (cons (make-e 'inline-ts d pt pet #f
                                                            (if pt (format-time pt) "Event"))
                                                    acc)))))
                              occs))))
                       inlines))))
                (reverse acc)))))))

(define (org-expand-entries sources #:today today #:span [span 1]
                            #:warning-days [warning-days 14]
                            #:day-start-hour [day-start-hour 0]
                            #:now-minutes [now-min 720]
                            #:week-start [week-start 1])
  "Expand `org-source` list SOURCES into `org-entry` occurrences.
Uses civil day TODAY, SPAN days, WARNING-DAYS, DAY-START-HOUR,
NOW-MIN, and WEEK-START."
  (append-map
   (lambda (src)
     (let* ((ol (org-source-outline src))
            (hs (org-outline-headings ol))
            (n (vector-length hs)))
       (let loop ((i 0) (acc '()))
         (if (>= i n)
             (reverse acc)
             (let ((es (expand-heading-occurrences src (vector-ref hs i) today span
                                                   warning-days day-start-hour now-min week-start)))
               (loop (+ i 1) (append (reverse es) acc)))))))
   sources))

;; ----- Inspectable s-exp query evaluator & proof tracer -----

(define (explain-pred pred entry today)
  (cond
    ((eq? pred #t) (list #t #t "always true" '()))
    ((eq? pred #f) (list #f #f "always false" '()))
    ((not (pair? pred)) (list pred #f "invalid predicate" '()))
    (else
     (let ((op (car pred))
           (args (cdr pred))
           (src (org-entry-source entry))
           (h (org-entry-heading entry)))
       (case op
         ((and)
          (let ((children (map (lambda (p) (explain-pred p entry today)) args)))
            (list pred (every cadr children)
                  (if (every cadr children) "all clauses matched" "a clause failed")
                  children)))
         ((or)
          (let ((children (map (lambda (p) (explain-pred p entry today)) args)))
            (list pred (any cadr children)
                  (if (any cadr children) "a clause matched" "no clause matched")
                  children)))
         ((not)
          (let ((c (explain-pred (car args) entry today)))
            (list pred (not (cadr c))
                  (if (not (cadr c)) "negated clause failed" "negated clause matched")
                  (list c))))
         ((open?)
          (let ((ok (org-heading-open? (org-source-outline src) h)))
            (list pred ok (if ok (string-append "keyword is " (org-heading-keyword h)) "not an open TODO") '())))
         ((done?)
          (let ((ok (org-heading-done? (org-source-outline src) h)))
            (list pred ok (if ok (string-append "keyword is " (org-heading-keyword h)) "not DONE") '())))
         ((todo)
          (let* ((kw (org-heading-keyword h))
                 (ok (and kw (member kw args) #t)))
            (list pred ok (if ok (string-append "keyword " kw " matched")
                              (string-append "keyword " (or kw "<none>") " not in list")) '())))
         ((tag)
          (let* ((tags (org-entry-tags entry))
                 (ok (any (lambda (t) (and (member t tags) #t)) args)))
            (list pred ok (if ok "tag matched" (string-append "tags " (string-join tags ":") " did not match")) '())))
         ((priority)
          (let* ((p (org-heading-priority h))
                 (ok (and p (memv p args) #t)))
            (list pred ok (if ok (string-append "priority #" (string p)) "priority did not match") '())))
         ((category)
          (let* ((c (org-entry-category entry))
                 (ok (and (member c args) #t)))
            (list pred ok (string-append "category is " c) '())))
         ((timed)
          (let ((ok (and (org-entry-time entry)
                         (memq (org-entry-reason entry) '(scheduled deadline inline-ts))
                         #t)))
            (list pred ok (if ok (string-append "timed at " (format-time (org-entry-time entry))) "untimed") '())))
         ((habit)
          (let ((hev (org-entry-habit entry)))
            (cond
              ((not hev) (list pred #f "not a habit" '()))
              ((null? args) (list pred #t (org-habit-eval-summary hev) '()))
              (else
               (let* ((st (org-habit-eval-status hev))
                      (ok (and (memq st args) #t)))
                 (list pred ok (string-append "habit status is " (symbol->string st)) '()))))))
         ((reason)
          (let* ((r (org-entry-reason entry))
                 (ok (and (memq r args) #t)))
            (list pred ok (string-append "occurrence reason is " (symbol->string r)) '())))
         ((scheduled)
          (let* ((r (org-entry-reason entry))
                 (mode (if (null? args) 'any (car args)))
                 (ok (case mode
                       ((today) (and (memq r '(scheduled inline-ts)) (= (org-entry-day entry) today)))
                       ((past) (eq? r 'scheduled-past))
                       ((future) (and (eq? r 'scheduled) (> (org-entry-day entry) today)))
                       (else (memq r '(scheduled scheduled-past inline-ts))))))
            (list pred ok (string-append "reason=" (symbol->string r)) '())))
         ((deadline)
          (let* ((r (org-entry-reason entry))
                 (mode (if (null? args) 'any (car args)))
                 (ok (case mode
                       ((today) (and (eq? r 'deadline) (= (org-entry-day entry) today)))
                       ((past) (eq? r 'deadline-past))
                       ((soon) (eq? r 'deadline-soon))
                       ((future) (or (eq? r 'deadline-soon)
                                     (and (eq? r 'deadline) (> (org-entry-day entry) today))))
                       (else (memq r '(deadline deadline-past deadline-soon))))))
            (list pred ok (string-append "reason=" (symbol->string r)) '())))
         ((heading)
          (let* ((title (org-heading-title (org-source-text src) h))
                 (needle (string-downcase (car args)))
                 (ok (and (string-contains (string-downcase title) needle) #t)))
            (list pred ok (if ok "title matched" "title did not match") '())))
         ((property)
          (let* ((val (org-heading-property (org-source-text src) (org-source-outline src) h (car args)))
                 (ok (if (null? (cdr args))
                         (and val #t)
                         (and val (string-ci=? val (cadr args))))))
            (list pred ok (string-append (car args) "=" (or val "<none>")) '())))
         (else
          (list pred #f (string-append "unknown predicate: " (symbol->string op)) '())))))))

;; Fast short-circuiting boolean evaluator (allocates no proof trees).
(define (eval-pred-fast pred entry today)
  (cond
    ((eq? pred #t) #t)
    ((eq? pred #f) #f)
    ((not (pair? pred)) #f)
    (else
     (let ((op (car pred))
           (args (cdr pred))
           (src (org-entry-source entry))
           (h (org-entry-heading entry)))
       (case op
         ((and) (every (lambda (p) (eval-pred-fast p entry today)) args))
         ((or) (any (lambda (p) (eval-pred-fast p entry today)) args))
         ((not) (not (eval-pred-fast (car args) entry today)))
         ((open?) (org-heading-open? (org-source-outline src) h))
         ((done?) (org-heading-done? (org-source-outline src) h))
         ((todo) (let ((kw (org-heading-keyword h))) (and kw (member kw args) #t)))
         ((tag) (let ((tags (org-entry-tags entry))) (any (lambda (t) (and (member t tags) #t)) args)))
         ((priority) (let ((p (org-heading-priority h))) (and p (memv p args) #t)))
         ((category) (and (member (org-entry-category entry) args) #t))
         ((timed) (and (org-entry-time entry) (memq (org-entry-reason entry) '(scheduled deadline inline-ts)) #t))
         ((habit)
          (let ((hev (org-entry-habit entry)))
            (and hev (or (null? args) (and (memq (org-habit-eval-status hev) args) #t)))))
         ((reason) (and (memq (org-entry-reason entry) args) #t))
         ((scheduled)
          (let ((r (org-entry-reason entry))
                (mode (if (null? args) 'any (car args))))
            (case mode
              ((today) (and (memq r '(scheduled inline-ts)) (= (org-entry-day entry) today)))
              ((past) (eq? r 'scheduled-past))
              ((future) (and (eq? r 'scheduled) (> (org-entry-day entry) today)))
              (else (memq r '(scheduled scheduled-past inline-ts))))))
         ((deadline)
          (let ((r (org-entry-reason entry))
                (mode (if (null? args) 'any (car args))))
            (case mode
              ((today) (and (eq? r 'deadline) (= (org-entry-day entry) today)))
              ((past) (eq? r 'deadline-past))
              ((soon) (eq? r 'deadline-soon))
              ((future) (or (eq? r 'deadline-soon)
                            (and (eq? r 'deadline) (> (org-entry-day entry) today))))
              (else (memq r '(deadline deadline-past deadline-soon))))))
         ((heading)
          (and (string-contains (string-downcase (org-heading-title (org-source-text src) h))
                                (string-downcase (car args)))
               #t))
         ((property)
          (let ((val (org-heading-property (org-source-text src) (org-source-outline src) h (car args))))
            (if (null? (cdr args)) (and val #t) (and val (string-ci=? val (cadr args))))))
         (else #f))))))

(define (org-query-eval pred entry #:today today)
  "Return #t if `org-entry` ENTRY matches s-exp predicate PRED on TODAY."
  (eval-pred-fast pred entry today))

(define (org-query-explain pred entry #:today today)
  "Return a proof tree for PRED on ENTRY and TODAY.
Each node is (pred matched? detail children)."
  (explain-pred pred entry today))

;; ----- Sorting & effort budget -----

(define (habit-urgency-rank hev)
  (if (not hev)
      99
      (case (org-habit-eval-status hev)
        ((over-limit) 0)
        ((missed) 1)
        ((must-today) 2)
        ((at-risk) 3)
        ((on-track) 4)
        ((cooldown) 5)
        ((satisfied) 6)
        (else 7))))

(define (priority-rank h)
  (let ((p (org-heading-priority h)))
    (if p (- (char->integer (char-upcase p)) (char->integer #\A)) 2)))

(define (entry<? a b)
  (let ((ta (org-entry-time a)) (tb (org-entry-time b))
        (ha (org-entry-habit a)) (hb (org-entry-habit b)))
    (cond
      ((and ta tb (not (= ta tb))) (< ta tb))
      ((and ta (not tb)) #t)
      ((and tb (not ta)) #f)
      ((and ha hb (not (= (habit-urgency-rank ha) (habit-urgency-rank hb))))
       (< (habit-urgency-rank ha) (habit-urgency-rank hb)))
      ((not (= (priority-rank (org-entry-heading a)) (priority-rank (org-entry-heading b))))
       (< (priority-rank (org-entry-heading a)) (priority-rank (org-entry-heading b))))
      (else (< (org-entry-day a) (org-entry-day b))))))

(define (entry-booked-minutes e)
  (let ((t (org-entry-time e))
        (et (org-entry-end-time e)))
    (cond
      ((and t et (> et t)) (- et t))
      ;; A point DEADLINE (no end time) is a cutoff, not a time-blocked appointment.
      ((eq? (org-entry-reason e) 'deadline) 0)
      ((and t (org-entry-effort e)) (org-entry-effort e))
      (t 30)
      (else 0))))

(define (sum-effort-and-unknown entries)
  (let loop ((es entries) (mins 0) (unk 0))
    (if (null? es)
        (cons mins unk)
        (let ((ef (org-entry-effort (car es))))
          (if ef
              (loop (cdr es) (+ mins ef) unk)
              (loop (cdr es) mins (+ unk 1)))))))

;; ----- View execution & explanation -----

(define (org-run-view spec sources #:today today #:now-minutes [now-min 720]
                      #:workday-minutes [workday-minutes 480]
                      #:warning-days [warning-days 14]
                      #:day-start-hour [day-start-hour 0]
                      #:week-start [week-start 1])
  "Evaluate `org-view-spec` SPEC over SOURCES for TODAY.
Uses NOW-MIN, WORKDAY-MINUTES, WARNING-DAYS, DAY-START-HOUR, and
WEEK-START. Returns an `org-view-result` with grouped entries and
effort/capacity budgets."
  (let* ((all (org-expand-entries sources #:today today #:span (org-view-spec-span spec)
                                  #:warning-days warning-days
                                  #:day-start-hour day-start-hour
                                  #:now-minutes now-min
                                  #:week-start week-start))
         (discard-pred (org-view-spec-discard spec))
         (discarded '())
         (kept '()))
    ;; Global discard runs BEFORE group routing so HOLD never leaks into Schedule.
    (for-each (lambda (e)
                (if (and discard-pred (org-query-eval discard-pred e #:today today))
                    (set! discarded (cons e discarded))
                    (set! kept (cons e kept))))
              all)
    (let* ((kept (reverse kept))
           (g-specs (org-view-spec-groups spec))
           (buckets (map (lambda (gs) (cons gs '())) g-specs)))
      ;; Route each entry through ordered groups.
      (for-each
       (lambda (e)
         (let loop ((bs buckets) (claimed? #f))
           (when (and (pair? bs) (not claimed?))
             (let* ((pair (car bs))
                    (gs (car pair)))
               (if (org-query-eval (org-group-spec-pred gs) e #:today today)
                   (begin
                     (set-cdr! pair (cons e (cdr pair)))
                     (loop (cdr bs) (not (org-group-spec-multi? gs))))
                   (loop (cdr bs) claimed?))))))
       kept)
      (let* ((group-results
              (map (lambda (pair)
                     (let* ((gs (car pair))
                            (es (sort (reverse (cdr pair)) entry<?))
                            (ef+u (sum-effort-and-unknown es)))
                       (make-org-group-result (org-group-spec-name gs)
                                              (org-group-spec-render gs)
                                              es (car ef+u) (cdr ef+u))))
                   buckets))
             ;; Count booked capacity only from actual timed occurrences on TODAY.
             (booked (fold (lambda (e acc)
                             (if (and (org-entry-time e)
                                      (= (org-entry-day e) today)
                                      (memq (org-entry-reason e) '(scheduled deadline inline-ts)))
                                 (+ acc (entry-booked-minutes e))
                                 acc))
                           0 kept))
             ;; Deduplicate untimed tasks by heading identity so a heading with both
             ;; schedule and deadline counts its EFFORT once.
             (seen-h (make-hash-table))
             (untimed-tasks
              (filter (lambda (e)
                        (and (not (org-entry-time e))
                             (not (org-entry-habit e))
                             (let ((hk (cons (org-entry-source e) (org-heading-index (org-entry-heading e)))))
                               (if (hash-table-ref/default seen-h hk #f)
                                   #f
                                   (begin (hash-table-set! seen-h hk #t) #t)))))
                      kept))
             (ef+u (sum-effort-and-unknown untimed-tasks))
             (free (max 0 (- workday-minutes booked)))
             (budget `((workday . ,workday-minutes)
                       (booked . ,booked)
                       (free . ,free)
                       (planned . ,(car ef+u))
                       (unknown . ,(cdr ef+u)))))
        (make-org-view-result group-results (reverse discarded) budget)))))

(define (org-explain-heading spec src h #:today today #:now-minutes [now-min 720]
                             #:warning-days [warning-days 14]
                             #:day-start-hour [day-start-hour 0]
                             #:week-start [week-start 1])
  "Explain whether heading H in SRC matches SPEC on TODAY.
Uses NOW-MIN, WARNING-DAYS, DAY-START-HOUR, and WEEK-START.
Returns an alist describing occurrences, discard proof, and routing."
  (let* ((entries (expand-heading-occurrences src h today (org-view-spec-span spec)
                                              warning-days day-start-hour now-min week-start))
         (discard-pred (org-view-spec-discard spec))
         (g-specs (org-view-spec-groups spec)))
    (if (null? entries)
        `((status . excluded)
          (reason . ,(if (org-heading-done? (org-source-outline src) h)
                         "Heading is in a DONE state"
                         "Heading generated no occurrences for this date span (no SCHEDULED, DEADLINE, active timestamp, or HABIT)"))
          (occurrences . ()))
        (let ((traces
               (map (lambda (e)
                      (let ((dproof (and discard-pred (org-query-explain discard-pred e #:today today))))
                        (if (and dproof (cadr dproof))
                            `((entry . ,e) (outcome . discarded) (discard-proof . ,dproof))
                            (let loop ((gs g-specs) (tried '()))
                              (if (null? gs)
                                  `((entry . ,e) (outcome . unmatched) (tried . ,(reverse tried)))
                                  (let* ((g (car gs))
                                         (p (org-query-explain (org-group-spec-pred g) e #:today today)))
                                    (if (cadr p)
                                        `((entry . ,e)
                                          (outcome . matched)
                                          (group . ,(org-group-spec-name g))
                                          (proof . ,p))
                                        (loop (cdr gs) (cons (cons (org-group-spec-name g) p) tried)))))))))
                    entries)))
          `((status . ,(if (any (lambda (t) (eq? (cdr (assq 'outcome t)) 'matched)) traces) 'included 'excluded))
            (occurrences . ,traces))))))

(define (org-search-headings pred sources #:today [today #f] #:include-done [include-done? #t])
  "Search ALL headings across SOURCES matching query predicate PRED.
Unlike `org-run-view`, includes undated TODOs and (when INCLUDE-DONE? is
true) DONE headings. Optional TODAY sets the reference civil day."
  (let ((ref-day (or today (car (local-now)))))
    (append-map
     (lambda (src)
       (let* ((text (org-source-text src))
              (ol (org-source-outline src))
              (hs (org-outline-headings ol))
              (n (vector-length hs)))
         (let loop ((i 0) (acc '()))
           (if (>= i n)
               (reverse acc)
               (let* ((h (vector-ref hs i))
                      (done? (org-heading-done? ol h)))
                 (if (and done? (not include-done?))
                     (loop (+ i 1) acc)
                     (let* ((elems (org-parse-section text (org-heading-body-start h) (org-heading-section-end ol h)))
                            (id (org-section-property elems "ID"))
                            (tags (org-heading-all-tags ol h))
                            (cat (or (org-section-property elems "CATEGORY")
                                     (org-outline-category ol #:path (org-source-path src))))
                            (effort (org-parse-effort-minutes (org-section-property elems "EFFORT")))
                            (key (heading-stable-key src h id 'heading ref-day 0))
                            (entry (make-org-entry src h 'heading ref-day #f #f effort tags cat #f "" key)))
                       (if (org-query-eval pred entry #:today ref-day)
                           (loop (+ i 1) (cons entry acc))
                           (loop (+ i 1) acc)))))))))
     sources)))

(define (org-default-day-view)
  "Return the default daily `org-view-spec` for Org agenda."
  (make-org-view-spec
   "Today"
   (list (make-org-group-spec "Schedule" '(timed) #:render 'time-grid)
         (make-org-group-spec "Habit: ora" '(and (habit) (tag "PRAYER")) #:render 'habit-strip)
         (make-org-group-spec "Habit: corpus" '(and (habit) (tag "LICHAAM")) #:render 'habit-strip)
         (make-org-group-spec "Habits" '(habit) #:render 'habit-strip)
         (make-org-group-spec "Today: labora" '(and (scheduled today) (tag "WORK")))
         (make-org-group-spec "Today: other" '(scheduled today))
         (make-org-group-spec "Due today" '(deadline today))
         (make-org-group-spec "Overdue" '(deadline past))
         (make-org-group-spec "Scheduled earlier" '(scheduled past))
         (make-org-group-spec "Due soon" '(deadline soon)))
   #:discard '(todo "HOLD")
   #:span 1))
