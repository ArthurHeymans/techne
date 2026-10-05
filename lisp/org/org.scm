;;; Org documents: parsing, lossless writing and TODO state changes.
;;;
;;; A document keeps every source line. Each heading owns the raw lines of its
;;; section (heading line, planning line, property drawer, body) up to the next
;;; heading, so writing an unmodified document reproduces it byte for byte.
;;; Edits regenerate only the lines they change.

(require "date.scm")

(provide read-org-file parse-org org->string write-org-file
         org-file? org-file-path org-file-title org-file-headings org-file-todo-states org-file-done-states
         heading? heading-level heading-keyword heading-priority heading-title heading-tags
         heading-planning heading-properties heading-children heading-line heading-lines
         heading-scheduled heading-deadline heading-closed heading-property
         timestamp? timestamp-active? timestamp-day timestamp-time timestamp-end-time timestamp-repeater timestamp->string
         all-headings heading-done? heading-open? set-todo! file-category)

(define-record-type org-file
  (make-org-file path keywords todo-states done-states preamble headings)
  org-file?
  (path org-file-path)
  (keywords org-file-keywords)
  (todo-states org-file-todo-states)
  (done-states org-file-done-states)
  (preamble org-file-preamble)
  (headings org-file-headings set-org-file-headings!))

(define-record-type heading
  (make-heading level keyword priority title tags planning properties lines children)
  heading?
  (level heading-level)
  (keyword heading-keyword set-heading-keyword!)
  (priority heading-priority)
  (title heading-title)
  (tags heading-tags)
  (planning heading-planning set-heading-planning!)
  (properties heading-properties)
  ;; Raw lines of this heading's own section, starting with the heading line.
  (lines heading-lines set-heading-lines!)
  (children heading-children set-heading-children!))

;; repeater: #f or (kind n unit), kind "+" "++" ".+", unit one of h d w m y.
(define-record-type timestamp
  (make-timestamp active? day time end-time repeater)
  timestamp?
  (active? timestamp-active?)
  (day timestamp-day)
  (time timestamp-time)
  (end-time timestamp-end-time)
  (repeater timestamp-repeater))

(define default-todo '("TODO"))
(define default-done '("DONE"))

;; ----- small string helpers -----

(define (blank? c) (or (char=? c #\space) (char=? c #\tab)))

(define (skip-blanks s i)
  (if (and (< i (string-length s)) (blank? (string-ref s i))) (skip-blanks s (+ i 1)) i))

(define (word-end s i)
  (if (and (< i (string-length s)) (not (blank? (string-ref s i)))) (word-end s (+ i 1)) i))

(define (count-stars line)
  (let loop ((i 0))
    (if (and (< i (string-length line)) (char=? (string-ref line i) #\*)) (loop (+ i 1)) i)))

(define (heading-line? line)
  (let ((n (count-stars line)))
    (and (> n 0) (or (= n (string-length line)) (char=? (string-ref line n) #\space)))))

(define (tag-token? s)
  (let ((n (string-length s)))
    (and (> n 1) (char=? (string-ref s 0) #\:) (char=? (string-ref s (- n 1)) #\:))))

;; ----- timestamps -----

(define (parse-time s)
  (match (string-split s ":")
    [(list h m) (let ((h (string->number h)) (m (string->number m))) (and h m (+ (* h 60) m)))]
    [_ #f]))

(define (parse-repeater s)
  (let* ((kind (cond ((string-prefix? ".+" s) ".+") ((string-prefix? "++" s) "++") ((string-prefix? "+" s) "+") (else #f)))
         (rest (and kind (substring s (string-length kind))))
         (n (and rest (> (string-length rest) 1) (string->number (substring rest 0 (- (string-length rest) 1)))))
         (unit (and n (string-ref rest (- (string-length rest) 1)))))
    (and n (memv unit '(#\h #\d #\w #\m #\y)) (list kind n unit))))

;; "<2026-10-05 Mon 10:00-11:00 +1w>" or "[...]" -> timestamp or #f.
(define (parse-timestamp s)
  (let ((n (string-length s)))
    (and (> n 2)
         (let ((active (char=? (string-ref s 0) #\<)))
           (match (string-split (substring s 1 (- n 1)))
             [(cons date parts)
              (let ((day (parse-iso-date date)))
                (and day
                     (let loop ((parts parts) (time #f) (end #f) (repeater #f))
                       (match parts
                         ['() (make-timestamp active day time end repeater)]
                         [(cons p rest)
                          (cond ((and (string-contains p ":") (not time))
                                 (match (string-split p "-")
                                   [(list a b) (loop rest (parse-time a) (parse-time b) repeater)]
                                   [_ (loop rest (parse-time p) end repeater)]))
                                ((parse-repeater p) => (lambda (r) (loop rest time end r)))
                                (else (loop rest time end repeater)))]))))]
             [_ #f])))))

(define (format-time minutes)
  (let ((h (quotient minutes 60)) (m (modulo minutes 60)))
    (string-append (if (< h 10) "0" "") (number->string h) ":" (if (< m 10) "0" "") (number->string m))))

(define (timestamp->string ts)
  (let ((r (timestamp-repeater ts)))
    (string-append
     (if (timestamp-active? ts) "<" "[")
     (date->string (timestamp-day ts)) " " (substring (weekday-name (timestamp-day ts)) 0 3)
     (if (timestamp-time ts)
         (string-append " " (format-time (timestamp-time ts))
                        (if (timestamp-end-time ts) (string-append "-" (format-time (timestamp-end-time ts))) ""))
         "")
     (if r (string-append " " (car r) (number->string (cadr r)) (string (caddr r))) "")
     (if (timestamp-active? ts) ">" "]"))))

;; ----- planning lines and property drawers -----

(define planning-keywords '(("SCHEDULED:" . scheduled) ("DEADLINE:" . deadline) ("CLOSED:" . closed)))

(define (planning-line? line)
  (let ((t (string-trim line)))
    (any (lambda (k) (string-prefix? (car k) t)) planning-keywords)))

;; "SCHEDULED: <...> DEADLINE: <...>" -> alist ((scheduled . ts) ...)
(define (parse-planning line)
  (let loop ((i (skip-blanks line 0)) (acc '()))
    (if (>= i (string-length line))
        (reverse acc)
        (let* ((j (word-end line i))
               (key (assoc (substring line i j) planning-keywords))
               (k (skip-blanks line j)))
          (if (and key (< k (string-length line)) (memv (string-ref line k) '(#\< #\[)))
              (let* ((close (if (char=? (string-ref line k) #\<) #\> #\]))
                     (end (let find ((e k)) (cond ((>= e (string-length line)) #f)
                                                  ((char=? (string-ref line e) close) (+ e 1))
                                                  (else (find (+ e 1))))))
                     (ts (and end (parse-timestamp (substring line k end)))))
                (loop (skip-blanks line (or end (string-length line)))
                      (if ts (cons (cons (cdr key) ts) acc) acc)))
              (loop (skip-blanks line j) acc))))))

(define (planning->string planning indent)
  (string-append
   indent
   (string-join (filter-map (lambda (k)
                              (let ((entry (assq (cdr k) planning)))
                                (and entry (string-append (car k) " " (timestamp->string (cdr entry))))))
                            planning-keywords)
                " ")))

(define (parse-properties lines)
  (filter-map (lambda (line)
                (let ((t (string-trim line)))
                  (and (> (string-length t) 1) (char=? (string-ref t 0) #\:)
                       (let ((close (string-index (substring t 1) #\:)))
                         (and close
                              (cons (substring t 1 (+ close 1))
                                    (string-trim (substring t (+ close 2)))))))))
              lines))

;; ----- headings -----

(define (parse-heading-line line level todo done)
  ;; After the stars: [KEYWORD] [[#P]] title [:tags:]
  (let* ((rest (string-trim (substring line level)))
         (words (string-split rest))
         (keyword (and (pair? words) (or (member (car words) todo) (member (car words) done)) (car words)))
         (words (if keyword (cdr words) words))
         (priority (and (pair? words)
                        (let ((w (car words)))
                          (and (= (string-length w) 4) (string-prefix? "[#" w) (char=? (string-ref w 3) #\])
                               (string-ref w 2)))))
         (words (if priority (cdr words) words))
         (tags (if (and (pair? words) (tag-token? (last words)))
                   (filter (lambda (t) (not (string-null? t))) (string-split (last words) ":"))
                   '()))
         (title-words (if (null? tags) words (reverse (cdr (reverse words))))))
    (list keyword priority (string-join title-words " ") tags)))

(define (heading-line h)
  (string-append (make-string (heading-level h) #\*)
                 (if (heading-keyword h) (string-append " " (heading-keyword h)) "")
                 (if (heading-priority h) (string-append " [#" (string (heading-priority h)) "]") "")
                 (if (string-null? (heading-title h)) "" (string-append " " (heading-title h)))
                 (if (null? (heading-tags h)) "" (string-append " :" (string-join (heading-tags h) ":") ":"))))

;; A heading from its raw section lines.
(define (section->heading lines todo done)
  (let* ((first-line (car lines))
         (level (count-stars first-line))
         (parts (parse-heading-line first-line level todo done))
         (after (cdr lines))
         (planning (if (and (pair? after) (planning-line? (car after))) (parse-planning (car after)) '()))
         (after (if (null? planning) after (cdr after)))
         (properties
          (if (and (pair? after) (string=? (string-trim (car after)) ":PROPERTIES:"))
              (let loop ((ls (cdr after)) (acc '()))
                (cond ((null? ls) '())
                      ((string=? (string-trim (car ls)) ":END:") (parse-properties (reverse acc)))
                      (else (loop (cdr ls) (cons (car ls) acc)))))
              '())))
    (match parts
      [(list keyword priority title tags)
       (make-heading level keyword priority title tags planning properties lines '())])))

;; ----- documents -----

;; "#+TODO: TODO NEXT | DONE CANCELLED" -> (todo-states . done-states)
(define (todo-keywords lines)
  (let ((spec (find (lambda (l) (string-prefix? "#+TODO:" (string-upcase l))) lines)))
    (if (not spec)
        (cons default-todo default-done)
        (let* ((words (string-split (substring spec 7)))
               (words (map (lambda (w) (let ((p (string-index w #\())) (if p (substring w 0 p) w))) words))
               (bar (list-index (lambda (w) (string=? w "|")) words)))
          (cond (bar (cons (take words bar) (drop words (+ bar 1))))
                ((null? words) (cons default-todo default-done))
                (else (cons (reverse (cdr (reverse words))) (list (last words)))))))))

(define (file-keywords lines)
  (filter-map (lambda (l)
                (and (string-prefix? "#+" l)
                     (let ((colon (string-index l #\:)))
                       (and colon (cons (string-upcase (substring l 2 colon)) (string-trim (substring l (+ colon 1))))))))
              lines))

;; Split lines into the preamble and per-heading sections.
(define (sections lines)
  (let loop ((lines lines) (current '()) (acc '()) (preamble #f))
    (cond ((null? lines)
           (let ((acc (if (and preamble (pair? current)) (cons (reverse current) acc) acc)))
             (values (if preamble preamble (reverse current)) (reverse acc))))
          ((heading-line? (car lines))
           (if preamble
               (loop (cdr lines) (list (car lines)) (cons (reverse current) acc) preamble)
               (loop (cdr lines) (list (car lines)) acc (reverse current))))
          (else (loop (cdr lines) (cons (car lines) current) acc preamble)))))

;; Build the tree: each heading takes following deeper headings as children.
(define (build-tree headings)
  (let loop ((hs headings) (acc '()))
    (if (null? hs)
        (reverse acc)
        (let* ((h (car hs))
               (deeper (let take-deeper ((rest (cdr hs)) (kids '()))
                         (if (and (pair? rest) (> (heading-level (car rest)) (heading-level h)))
                             (take-deeper (cdr rest) (cons (car rest) kids))
                             (cons (reverse kids) rest)))))
          (set-heading-children! h (build-tree (car deeper)))
          (loop (cdr deeper) (cons h acc))))))

(define (parse-org lines #:path [path #f])
  (let* ((states (todo-keywords lines))
         (todo (car states))
         (done (cdr states)))
    (receive (preamble secs) (sections lines)
      (make-org-file path (file-keywords preamble) todo done preamble
                     (build-tree (map (lambda (s) (section->heading s todo done)) secs))))))

;; `file->lines` drops a trailing newline; remember whether the file had one.
(define (read-org-file path)
  (parse-org (file->lines path) #:path path))

(define (all-headings file)
  (let walk ((hs (org-file-headings file)))
    (append-map (lambda (h) (cons h (walk (heading-children h)))) hs)))

(define (org->string file)
  (let ((lines (append (org-file-preamble file) (append-map heading-lines (all-headings file)))))
    (if (null? lines) "" (string-append (string-join lines "\n") "\n"))))

(define (write-org-file file path)
  (let ((port (open-output-file path)))
    (write-string (org->string file) port)
    (close-port port)))

(define (org-file-title file)
  (let ((entry (assoc "TITLE" (org-file-keywords file)))) (and entry (cdr entry))))

;; Agenda category: the CATEGORY keyword, else the file name without extension.
(define (file-category file)
  (let ((entry (assoc "CATEGORY" (org-file-keywords file))))
    (cond (entry (cdr entry))
          ((org-file-path file)
           (let* ((name (last (string-split (org-file-path file) "/")))
                  (dot (string-index name #\.)))
             (if dot (substring name 0 dot) name)))
          (else "org"))))

(define (heading-scheduled h) (let ((e (assq 'scheduled (heading-planning h)))) (and e (cdr e))))
(define (heading-deadline h) (let ((e (assq 'deadline (heading-planning h)))) (and e (cdr e))))
(define (heading-closed h) (let ((e (assq 'closed (heading-planning h)))) (and e (cdr e))))
(define (heading-property h key) (let ((e (assoc key (heading-properties h)))) (and e (cdr e))))

(define (heading-done? file h)
  (and (heading-keyword h) (member (heading-keyword h) (org-file-done-states file)) #t))
(define (heading-open? file h)
  (and (heading-keyword h) (member (heading-keyword h) (org-file-todo-states file)) #t))

;; ----- editing -----

(define (replace-planning! h planning)
  ;; Keep the existing planning line's indentation; otherwise insert one.
  (let* ((lines (heading-lines h))
         (old (and (pair? (cdr lines)) (planning-line? (cadr lines)) (cadr lines)))
         (indent (if old (substring old 0 (skip-blanks old 0)) ""))
         (rest (if old (cddr lines) (cdr lines))))
    (set-heading-planning! h planning)
    (set-heading-lines! h (append (list (car lines))
                                  (if (null? planning) '() (list (planning->string planning indent)))
                                  rest))))

(define (shift-day day repeater today)
  (match repeater
    [(list kind n unit)
     (let ((step (lambda (d)
                   (case unit
                     ((#\d) (+ d n))
                     ((#\w) (+ d (* 7 n)))
                     ((#\m) (add-months d n))
                     ((#\y) (add-months d (* 12 n)))
                     (else d)))))
       (cond ((string=? kind ".+") (step today))
             ((string=? kind "++") (let loop ((d (step day))) (if (> d today) d (loop (step d)))))
             (else (step day))))]))

(define (shift-timestamp ts today)
  (if (timestamp-repeater ts)
      (make-timestamp (timestamp-active? ts) (shift-day (timestamp-day ts) (timestamp-repeater ts) today)
                      (timestamp-time ts) (timestamp-end-time ts) (timestamp-repeater ts))
      ts))

;; Set the TODO keyword like org-todo: completing a repeating task moves its
;; dates forward and keeps it open; completing others records CLOSED.
(define (set-todo! file h keyword #:today today)
  (let* ((done (and keyword (member keyword (org-file-done-states file)) #t))
         (repeating (any (lambda (e) (and (memq (car e) '(scheduled deadline)) (timestamp-repeater (cdr e))))
                         (heading-planning h))))
    (cond ((and done repeating)
           (replace-planning! h (map (lambda (e) (if (eq? (car e) 'closed) e (cons (car e) (shift-timestamp (cdr e) today))))
                                     (heading-planning h))))
          (else
           (set-heading-keyword! h keyword)
           (let ((planning (filter (lambda (e) (not (eq? (car e) 'closed))) (heading-planning h))))
             (replace-planning! h (if done
                                      (append planning (list (cons 'closed (make-timestamp #f today #f #f #f))))
                                      planning)))))
    (set-heading-lines! h (cons (heading-line h) (cdr (heading-lines h))))))
