;;; Org agenda views over parsed files.
;;;
;;; For each day: scheduled and deadline entries (repeaters expanded); on
;;; today, also overdue scheduled items ("Sched. 3x:"), missed deadlines
;;; ("2 d. ago:") and upcoming deadlines within the warning period
;;; ("In 4 d.:"). Done headings are left out.

(require "date.scm")
(require "org.scm")

(provide agenda agenda->string todo-list tags-match agenda-item? agenda-item-heading agenda-item-label)

;; kind: deadline or scheduled (org collects deadlines first).
(define-record-type agenda-item
  (make-agenda-item kind category label time heading file)
  agenda-item?
  (kind agenda-item-kind)
  (category agenda-item-category)
  (label agenda-item-label)
  (time agenda-item-time)
  (heading agenda-item-heading)
  (file agenda-item-file))

;; Does a timestamp (with its repeater) fall on `day`?
(define (occurs-on? ts day)
  (let ((base (timestamp-day ts)) (r (timestamp-repeater ts)))
    (cond ((= base day) #t)
          ((or (not r) (< day base)) #f)
          (else
           (match r
             [(list _ n #\d) (= 0 (modulo (- day base) n))]
             [(list _ n #\w) (= 0 (modulo (- day base) (* 7 n)))]
             [(list _ n (and unit (or #\m #\y)))
              (let ((months (if (char=? unit #\m) n (* 12 n))))
                (let loop ((k 1))
                  (let ((d (add-months base (* k months))))
                    (cond ((= d day) #t) ((> d day) #f) (else (loop (+ k 1)))))))]
             [_ #f])))))

(define (items-for-day file h day today warning-days)
  (let ((scheduled (heading-scheduled h))
        (deadline (heading-deadline h))
        (item (lambda (kind label ts) (make-agenda-item kind (file-category file) label (and ts (timestamp-time ts)) h file))))
    (append
     (cond ((not deadline) '())
           ((occurs-on? deadline day) (list (item 'deadline "Deadline:" deadline)))
           ((and (= day today) (< (timestamp-day deadline) today))
            (list (item 'deadline (string-append (number->string (- today (timestamp-day deadline))) " d. ago:") #f)))
           ((and (= day today) (<= (- (timestamp-day deadline) today) warning-days))
            (list (item 'deadline (string-append "In " (number->string (- (timestamp-day deadline) today)) " d.:") #f)))
           (else '()))
     (cond ((not scheduled) '())
           ((occurs-on? scheduled day) (list (item 'scheduled "Scheduled:" scheduled)))
           ((and (= day today) (< (timestamp-day scheduled) today))
            (list (item 'scheduled (string-append "Sched." (number->string (- today (timestamp-day scheduled))) "x:") #f)))
           (else '())))))

(define (priority-rank h)
  (let ((p (heading-priority h)))
    (if p (- (char->integer p) (char->integer #\A)) 1)))

;; Timed items first by time; then deadlines before scheduled items (org's
;; collection order), then priority; file order otherwise (the sort is stable).
(define (item<? a b)
  (let ((ta (agenda-item-time a)) (tb (agenda-item-time b))
        (ka (if (eq? (agenda-item-kind a) 'deadline) 0 1)) (kb (if (eq? (agenda-item-kind b) 'deadline) 0 1)))
    (cond ((and ta tb) (< ta tb))
          (ta #t)
          (tb #f)
          ((not (= ka kb)) (< ka kb))
          (else (< (priority-rank (agenda-item-heading a)) (priority-rank (agenda-item-heading b)))))))

(define (agenda files #:start start #:span [span 7] #:today [today start] #:warning-days [warning-days 14])
  "Return the agenda of FILES: ((day item ...) ...).
It has SPAN days from START, TODAY marking overdue items, with
deadlines shown WARNING-DAYS ahead."
  (let ((entries (append-map (lambda (f)
                               (map (lambda (h) (cons f h))
                                    (filter (lambda (h) (not (heading-done? f h))) (all-headings f))))
                             files)))
    (map (lambda (day)
           (cons day (sort (append-map (lambda (e) (items-for-day (car e) (cdr e) day today warning-days)) entries)
                           item<?)))
         (iota span start))))

(define (pad s width)
  (if (>= (string-length s) width) s (string-append s (make-string (- width (string-length s)) #\space))))

(define (format-item item)
  (let* ((h (agenda-item-heading item))
         (time (agenda-item-time item))
         (minutes->hhmm (lambda (m) (string-append (number->string (quotient m 60)) ":"
                                                   (let ((mm (modulo m 60))) (string-append (if (< mm 10) "0" "") (number->string mm))))))
         (head (string-append (if (heading-keyword h) (string-append (heading-keyword h) " ") "")
                              (if (heading-priority h) (string-append "[#" (string (heading-priority h)) "] ") "")
                              (heading-title h))))
    (string-append "  " (pad (string-append (agenda-item-category item) ":") 12)
                   (pad (if time (string-append (minutes->hhmm time) "......") "") 12)
                   (pad (agenda-item-label item) 12)
                   head
                   (if (null? (heading-tags h)) "" (string-append "   :" (string-join (heading-tags h) ":") ":")))))

(define (day-header day)
  (match (civil-from-days day)
    [(list y m d) (string-append (pad (weekday-name day) 10) " " (number->string d) " " (month-name m) " " (number->string y))]))

(define (agenda->string days)
  "Return the agenda DAYS, as `agenda` gives it, written as text."
  (with-output-to-string
    (lambda ()
      (for-each (lambda (entry)
                  (displayln (day-header (car entry)))
                  (for-each (lambda (item) (displayln (format-item item))) (cdr entry)))
                days))))

(define (todo-list files)
  "Return every open TODO heading of FILES, as (file . heading)."
  (append-map (lambda (f) (map (lambda (h) (cons f h)) (filter (lambda (h) (heading-open? f h)) (all-headings f))))
              files))

(define (tags-match files query)
  "Return the headings of FILES matching the Org tag QUERY.
QUERY is like \"+work-home\" or \"work\": tags required and excluded."
  (let* ((terms (let loop ((i 0) (acc '()))
                  (if (>= i (string-length query))
                      (reverse acc)
                      (let* ((sign (string-ref query i))
                             (start (if (memv sign '(#\+ #\-)) (+ i 1) i))
                             (end (let find ((j start))
                                    (if (and (< j (string-length query)) (not (memv (string-ref query j) '(#\+ #\-))))
                                        (find (+ j 1)) j))))
                        (loop end (cons (cons (not (char=? sign #\-)) (substring query start end)) acc))))))
         (matches? (lambda (h)
                     (every (lambda (t) (eq? (car t) (and (member (cdr t) (heading-tags h)) #t))) terms))))
    (append-map (lambda (f) (map (lambda (h) (cons f h)) (filter matches? (all-headings f)))) files)))
