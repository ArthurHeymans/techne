;;; Org mode: outline editing, task state transitions, highlighting,
;;; visual indentation, checkboxes and link navigation, built on
;;; lisp/org/{date,syntax,edit}.scm.

(import (techne editor))
(require "../../../org/date.scm")
(require "../../../org/syntax.scm")
(require "../../../org/edit.scm")
(require "../../../org/habit.scm")
(require "../../../org/query.scm")
(require "../../../org/mem.scm")
(require "../../../org/capture.scm")
(require "../../../org/babel.scm")
(require "../../../org/export.scm")

;; ----- Options (loaded on demand with Org runtime) -----

(define-option org-directory "~/org/"
  "Root directory for Org files."
  #:type 'string)

(define-option org-agenda-files '()
  "List of Org file paths included in the agenda.
When empty, open `org-mode` buffers are queried."
  #:type '(list-of string))

(define-option org-mem-watch-dirs '()
  "Directories watched by `org-mem` (defaults to `org-directory`)."
  #:type '(list-of string))

(define-option org-daily-directory "~/org/roam/daily/"
  "Directory for daily and meeting notes created by `org-capture`."
  #:type 'string)

(define-option org-default-agenda-files
  '("9e_work.org" "cal.org" "aheymans_cal.org" "gedeelde_cal.org")
  "Org filenames under `org-directory` always included in the agenda."
  #:type '(list-of string))

(define-option org-todo-keywords '("TODO" "NEXT" "WAIT" "REVIEW" "HOLD")
  "Default open TODO keywords when a file has no #+TODO directive."
  #:type '(list-of string))

(define-option org-done-keywords '("DONE" "KILL" "CANCELLED")
  "Default DONE keywords when a file has no #+TODO directive."
  #:type '(list-of string))

(define-option org-log-done 'time
  "What to record when a task enters a DONE state: `time`, `note` or #f."
  #:type '(one-of time note #f))

(define-option org-log-into-drawer "LOGBOOK"
  "Drawer name for state-change logs, or #f to log directly in the body."
  #:type '(or (one-of #f) string))

(define-option org-indent-indentation-per-level 2
  "Visual indentation spaces per heading level in `org-indent-mode`."
  #:type 'natural)

(define-option org-hide-leading-stars #t
  "Whether leading stars of Org headings blend into the background."
  #:type 'boolean)

(define-option org-effort-values
  '("0:05" "0:10" "0:15" "0:20" "0:30" "0:45" "1:00" "1:30" "2:00" "3:00" "4:00" "5:00" "6:00" "7:00")
  "Suggested values when setting a heading's EFFORT property."
  #:type '(list-of string))

(define-option org-auto-revert-interval 2000
  "Polling interval in milliseconds for `org-auto-revert-mode`."
  #:type 'natural)

(define-option org-day-start-hour 0
  "Hour of day (0-23) when a logical habit day starts.
Set to e.g. 4 so late-night completions count toward the evening."
  #:type 'natural)

(define-option org-workday-minutes 480
  "Daily capacity in minutes used for agenda effort budgeting."
  #:type 'natural)

(define-option org-auto-todo-filetag #t
  "Auto-maintain `todo` in `#+filetags:` on save under `org-directory`."
  #:type 'boolean)

(define-option org-agenda-hide-tags '("todo")
  "Tag names hidden when rendering rows in `*Org Agenda*`."
  #:type '(list-of string))

(define-option org-meeting-default-title "microsoft_daily"
  "Default meeting name suggested by `org-capture` meeting notes."
  #:type 'string)

(define-option org-meeting-default-participants "Arthur Heymans"
  "Default participants value for `org-capture` meeting notes."
  #:type 'string)

(define-option org-hide-link-brackets #t
  "Conceal `[[url][desc]]` link markup in Org buffers."
  #:type 'boolean)

(define-option org-hide-emphasis-markers #t
  "Conceal Org emphasis markers (`*bold*`, `/italic/`, `~code~`)."
  #:type 'boolean)

(define-option org-capture-use-draft #t
  "Open an editable `*Org Capture*` draft buffer before committing."
  #:type 'boolean)

(define-option org-cycle-global-at-bob #f
  "Cycle global visibility when `org-cycle` runs before the first heading."
  #:type 'boolean)

;; ----- Faces -----

(define-face org "Org mode text.")
(define-face org.heading "An Org heading." #:weight 'bold)
(define-face org.heading.1 "A level-1 Org heading." #:inherit 'org.heading #:fg 'mauve #:height 1.25)
(define-face org.heading.2 "A level-2 Org heading." #:inherit 'org.heading #:fg 'blue #:height 1.15)
(define-face org.heading.3 "A level-3 Org heading." #:inherit 'org.heading #:fg 'sky #:height 1.05)
(define-face org.heading.4 "A level-4 Org heading." #:inherit 'org.heading #:fg 'teal)
(define-face org.heading.5 "A level-5 Org heading." #:inherit 'org.heading #:fg 'green)
(define-face org.heading.6 "A level-6 or deeper Org heading." #:inherit 'org.heading #:fg 'yellow)
(define-face org.stars.leading "Leading stars hidden on an Org heading." #:fg 'base)
(define-face org.todo "An open Org TODO keyword." #:fg 'red #:weight 'bold)
(define-face org.todo.review "An Org REVIEW keyword." #:fg 'sky #:weight 'bold)
(define-face org.done "A completed Org DONE keyword." #:fg 'green #:weight 'bold #:strike #t)
(define-face org.priority "An Org priority cookie such as [#A]." #:fg 'peach #:weight 'bold)
(define-face org.tag "Org heading tags." #:fg 'overlay2)
(define-face org.planning "An Org planning keyword (SCHEDULED, DEADLINE, CLOSED)." #:fg 'overlay2 #:weight 'bold)
(define-face org.timestamp.active "An active Org timestamp <...>." #:fg 'blue)
(define-face org.timestamp.inactive "An inactive Org timestamp [...]." #:fg 'overlay)
(define-face org.drawer "An Org drawer delimiter (:PROPERTIES:, :LOGBOOK:, :END:)." #:fg 'overlay #:weight 'bold)
(define-face org.property.key "An Org property name." #:fg 'teal)
(define-face org.property.value "An Org property value." #:fg 'subtext)
(define-face org.keyword "An Org keyword line (#+TITLE:, #+NAME:, etc.)." #:fg 'overlay2)
(define-face org.block.delimiter "An Org block delimiter (#+begin_... / #+end_...)." #:fg 'overlay #:bg 'mantle #:extend #t)
(define-face org.block.body "The body of an Org source or example block." #:bg 'mantle #:extend #t)
(define-face org.code "Inline Org ~code~." #:fg 'peach #:bg 'surface0)
(define-face org.verbatim "Inline Org =verbatim=." #:fg 'yellow #:bg 'surface0)
(define-face org.bold "Bold Org *text*." #:weight 'bold)
(define-face org.italic "Italic Org /text/." #:slant 'italic)
(define-face org.underline "Underlined Org _text_." #:underline 'line)
(define-face org.strike "Struck-through Org +text+." #:strike #t #:fg 'overlay)
(define-face org.markup "Faint Org inline markup delimiters." #:fg 'surface)
(define-face org.link "An Org link." #:fg 'blue #:underline 'line)
(define-face org.list.bullet "An Org plain list bullet." #:fg 'peach #:weight 'bold)
(define-face org.checkbox "An unchecked Org checkbox [ ] or [-]." #:fg 'yellow #:weight 'bold)
(define-face org.checkbox.done "A checked Org checkbox [X]." #:fg 'green #:weight 'bold)
(define-face org.cookie "An Org statistics cookie [1/3] or [33%]." #:fg 'teal #:weight 'bold)
(define-face org.table "An Org table." #:fg 'sky)
(define-face org.table.rule "An Org table horizontal rule line." #:fg 'overlay)
(define-face org.comment "An Org comment line." #:fg 'overlay #:slant 'italic)
(define-face org.rule "An Org horizontal rule." #:fg 'surface)
(define-face org.indent "Visual indentation prefix in `org-indent-mode`." #:fg 'surface)
(define-face org.agenda.header "Day title in `*Org Agenda*`." #:fg 'mauve #:weight 'bold #:height 1.2)
(define-face org.agenda.group "Group heading in `*Org Agenda*`." #:fg 'peach #:weight 'bold #:height 1.1)
(define-face org.agenda.budget "Effort and capacity summary in `*Org Agenda*`." #:fg 'sky)
(define-face org.agenda.budget.over "Over-capacity warning in `*Org Agenda*`." #:fg 'red #:weight 'bold)
(define-face org.agenda.habit.ok "Satisfied or on-track habit status." #:fg 'green)
(define-face org.agenda.habit.risk "At-risk habit status." #:fg 'yellow #:weight 'bold)
(define-face org.agenda.habit.must "Must-today habit status." #:fg 'peach #:weight 'bold)
(define-face org.agenda.habit.missed "Missed or over-limit habit status." #:fg 'red #:weight 'bold)
(define-face org.agenda.habit.strip "Period strip in `*Org Agenda*`." #:fg 'overlay2)
(define-face org.agenda.rule "Subtle divider rule in `*Org Agenda*`." #:fg 'surface2)
(define-face org.agenda.grid "Time-grid hour row in `*Org Agenda*`." #:fg 'overlay)
(define-face org.agenda.now "Current-time marker in `*Org Agenda*`." #:fg 'yellow #:weight 'bold)
(define-face org.agenda.cat "Category column in `*Org Agenda*`." #:fg 'green)
(define-face org.agenda.cat.past "Overdue category column in `*Org Agenda*`." #:fg 'peach)
(define-face org.agenda.effort "Effort column in `*Org Agenda*`." #:fg 'teal)
(define-face org.agenda.todo.badge "TODO pill badge in `*Org Agenda*`." #:fg 'base #:bg 'red #:weight 'bold)
(define-face org.agenda.review.badge "REVIEW pill badge in `*Org Agenda*`." #:fg 'base #:bg 'sky #:weight 'bold)
(define-face org.agenda.done.badge "DONE pill badge in `*Org Agenda*`." #:fg 'base #:bg 'green #:weight 'bold)
(define-face org.agenda.prio.a "Priority #A badge in `*Org Agenda*`." #:fg 'base #:bg 'mauve #:weight 'bold)
(define-face org.agenda.prio.b "Priority #B badge in `*Org Agenda*`." #:fg 'base #:bg 'peach #:weight 'bold)
(define-face org.agenda.prio.c "Priority #C badge in `*Org Agenda*`." #:fg 'text #:bg 'surface2 #:weight 'bold)
(define-face org.agenda.tag.badge "Right-aligned tag pill in `*Org Agenda*`." #:fg 'base #:bg 'blue #:weight 'bold)
(define-face org.agenda.habit.bar.ok "On-track habit bar in `*Org Agenda*`." #:fg 'base #:bg 'green #:weight 'bold)
(define-face org.agenda.habit.bar.risk "At-risk habit bar in `*Org Agenda*`." #:fg 'base #:bg 'yellow #:weight 'bold)
(define-face org.agenda.habit.bar.must "Must-today habit bar in `*Org Agenda*`." #:fg 'base #:bg 'peach #:weight 'bold)
(define-face org.agenda.habit.bar.miss "Missed/overdue habit bar in `*Org Agenda*`." #:fg 'base #:bg 'red #:weight 'bold)

;; ----- Per-document outline cache (keyed by document revision) -----

(define %org-cache (make-hash-table))

(define (doc-org-options d)
  (let ((b (document-buffer d)))
    (if b
        (values (option b 'org-todo-keywords) (option b 'org-done-keywords))
        (values (option #f 'org-todo-keywords) (option #f 'org-done-keywords)))))

(define (org-doc-outline d)
  (let-values (((todo done) (doc-org-options d)))
    (let* ((rev (document-revision d))
           (ckey (list rev todo done))
           (cached (hash-table-ref/default %org-cache d #f)))
      (if (and cached (equal? (car cached) ckey))
          (cdr cached)
          (let* ((text (document-string d))
                 (ol (org-parse-outline text #:todo-keywords todo #:done-keywords done)))
            (hash-table-set! %org-cache d (cons ckey (cons text ol)))
            (cons text ol))))))

;; Per-document revision-keyed caches so cursor/arrow-key navigation does not
;; re-parse sections or recompute highlights/conceals/inlays on every frame.
(define %org-section-cache (make-hash-table))
(define %org-layer-cache (make-hash-table))
(define %org-conceal-cache (make-hash-table))
(define %org-indent-inlay-cache (make-hash-table))
(define %org-indent-proj-cache (make-hash-table))

(define (org-cached-section d text bstart sec-end)
  (let* ((rev (document-revision d))
         (entry (hash-table-ref/default %org-section-cache d #f))
         (tbl (if (and entry (equal? (car entry) rev))
                  (cdr entry)
                  (let ((fresh (make-hash-table)))
                    (hash-table-set! %org-section-cache d (cons rev fresh))
                    fresh)))
         (key (cons bstart sec-end))
         (hit (hash-table-ref/default tbl key #f)))
    (if hit
        hit
        (let ((elems (org-parse-section text bstart sec-end)))
          (hash-table-set! tbl key elems)
          elems))))

(add-hook! 'buffer-killed 'org-cache-cleanup
           (lambda (s b)
             (when (buffer? b)
               (let ((d (buffer-document b)))
                 (hash-table-delete! %org-cache d)
                 (hash-table-delete! %org-section-cache d)
                 (hash-table-delete! %org-layer-cache d)
                 (hash-table-delete! %org-conceal-cache d)
                 (hash-table-delete! %org-indent-inlay-cache d)
                 (hash-table-delete! %org-indent-proj-cache d)))))

;; ----- Per-view folding (headings, drawers, blocks) -----
;;
;; Fold state is keyed per view and stores stable heading identities
;; (level, title, occurrence) rather than raw byte offsets, so folds
;; survive edits and external reloads cleanly.

(define %org-view-folds (make-hash-table))

(define (view-fold-state v)
  (let* ((id (view-id v))
         (st (hash-table-ref/default %org-view-folds id #f)))
    (if st
        st
        (let ((fresh (vector (make-hash-table) (make-hash-table) 'show-all #f)))
          ;; #(heading-states element-folds global-state cached-folds)
          (hash-table-set! %org-view-folds id fresh)
          fresh))))

(define (heading-state-table v) (vector-ref (view-fold-state v) 0))
(define (element-fold-table v) (vector-ref (view-fold-state v) 1))
(define (view-global-visibility v) (vector-ref (view-fold-state v) 2))
(define (set-view-global-visibility! v st) (vector-set! (view-fold-state v) 2 st))
(define (invalidate-view-folds! v) (vector-set! (view-fold-state v) 3 #f))

;; Compute stable identity keys for every heading in OL.
(define (heading-identity-keys text ol)
  (let* ((hs (org-outline-headings ol))
         (n (vector-length hs))
         (keys (make-vector n))
         (seen (make-hash-table)))
    (let loop ((i 0))
      (when (< i n)
        (let* ((h (vector-ref hs i))
               (base (cons (org-heading-level h) (org-heading-title text h)))
               (occ (hash-table-ref/default seen base 0)))
          (hash-table-set! seen base (+ occ 1))
          (vector-set! keys i (list (car base) (cdr base) occ))
          (loop (+ i 1)))))
    keys))

;; Merge sorted (from to) fold ranges.
(define (merge-fold-ranges ranges)
  (let ((sorted (sort (filter (lambda (r) (< (car r) (cadr r))) ranges)
                      (lambda (a b) (or (< (car a) (car b))
                                        (and (= (car a) (car b)) (< (cadr a) (cadr b))))))))
    (let loop ((rs sorted) (acc '()))
      (if (null? rs)
          (reverse acc)
          (let ((cur (car rs)))
            (if (and (pair? acc) (<= (car cur) (cadr (car acc))))
                (loop (cdr rs) (cons (list (caar acc) (max (cadr (car acc)) (cadr cur))) (cdr acc)))
                (loop (cdr rs) (cons cur acc))))))))

(define (org-view-folds v)
  (let* ((d (view-document v))
         (st (hash-table-ref/default %org-view-folds (view-id v) #f)))
    (if (not st)
        '()
        (let* ((h-tbl (vector-ref st 0))
               (e-tbl (vector-ref st 1)))
          (if (and (= (hash-table-size h-tbl) 0) (= (hash-table-size e-tbl) 0))
              '()
              (let* ((rev (document-revision d))
                     (cached (vector-ref st 3)))
                (if (and cached (equal? (car cached) rev))
                    (cdr cached)
                    (let ((res (let* ((pair (org-doc-outline d))
                     (text (car pair))
                     (ol (cdr pair))
                     (hs (org-outline-headings ol))
                     (n (vector-length hs))
                     (keys (heading-identity-keys text ol))
                     (acc '()))
                (let loop ((i 0))
                  (when (< i n)
                    (let* ((h (vector-ref hs i))
                           (k (vector-ref keys i))
                           (mode (hash-table-ref/default h-tbl k #f))
                           (bstart (org-heading-body-start h))
                           (sec-end (org-heading-section-end ol h))
                           (sub-end (org-heading-subtree-end h)))
                      (cond
                        ((eq? mode 'folded)
                         (when (< bstart sub-end)
                           (set! acc (cons (list bstart sub-end) acc))))
                        ((eq? mode 'body-only)
                         (when (< bstart sec-end)
                           (set! acc (cons (list bstart sec-end) acc))))
                        ((eq? mode 'children)
                         ;; Direct children carry their own 'folded state so cycling a child works independently.
                         (when (< bstart sec-end)
                           (set! acc (cons (list bstart sec-end) acc))))))
                    ;; Per-section drawer/block folds only when drawer/block folds exist.
                    (when (and (> (hash-table-size e-tbl) 0)
                               (not (eq? (hash-table-ref/default h-tbl (vector-ref keys i) #f) 'folded)))
                      (let* ((h (vector-ref hs i))
                             (k (vector-ref keys i))
                             (bstart (org-heading-body-start h))
                             (sec-end (org-heading-section-end ol h)))
                        (when (< bstart sec-end)
                          (let ((elems (org-cached-section d text bstart sec-end)))
                            (let eloop ((es elems) (ord 0))
                              (when (pair? es)
                                (let* ((el (car es))
                                       (kind (org-element-kind el)))
                                  (if (memq kind '(drawer src-block block))
                                      (begin
                                        (when (hash-table-ref/default e-tbl (list k kind ord) #f)
                                          (let* ((hdr-end (if (eq? kind 'drawer)
                                                              (cadr (org-element-data el))
                                                              (list-ref (org-element-data el) 2)))
                                                 (fold-from (min (+ hdr-end 1) (org-element-end el))))
                                            (when (< fold-from (org-element-end el))
                                              (set! acc (cons (list fold-from (org-element-end el)) acc)))))
                                        (eloop (cdr es) (+ ord 1)))
                                      (eloop (cdr es) ord)))))))))
                    (loop (+ i 1))))
                (merge-fold-ranges acc))))
                      (vector-set! st 3 (cons rev res))
                      res))))))))

(define (pos-hidden-in? folds pos)
  (any (lambda (f) (and (<= (car f) pos) (< pos (cadr f)))) folds))

(define (normalize-point-visible! s)
  (let* ((v (session-view s))
         (folds (org-view-folds v))
         (p (point s))
         (f (find (lambda (r) (and (<= (car r) p) (< p (cadr r)))) folds)))
    (when f
      (let ((hdr (if (> (car f) 0) (line-start (doc s) (- (car f) 1)) 0)))
        (move! s (lambda (_) hdr))))))

;; ----- Highlighting layer -----

(define (heading-level-face level)
  (case level
    ((1) 'org.heading.1)
    ((2) 'org.heading.2)
    ((3) 'org.heading.3)
    ((4) 'org.heading.4)
    ((5) 'org.heading.5)
    (else 'org.heading.6)))

(define (todo-keyword-face ol kw)
  (cond
    ((member kw (org-outline-done-keywords ol)) 'org.done)
    ((string=? kw "REVIEW") 'org.todo.review)
    (else 'org.todo)))

(define (inline-highlights inlines)
  (append-map
   (lambda (in)
     (let ((k (org-inline-kind in))
           (s (org-inline-start in))
           (e (org-inline-end in))
           (d (org-inline-data in)))
       (case k
         ((link)
          (let ((desc-s (list-ref d 1))
                (desc-e (list-ref d 2)))
            (if desc-s
                ;; [[target][desc]] -> dim target/brackets, highlight desc as link
                (list (list s desc-s 'org.markup)
                      (list desc-s desc-e 'org.link)
                      (list desc-e e 'org.markup))
                (list (list s e 'org.link)))))
         ((timestamp-active) (list (list s e 'org.timestamp.active)))
         ((timestamp-inactive) (list (list s e 'org.timestamp.inactive)))
         ((cookie) (list (list s e 'org.cookie)))
         ((footnote) (list (list s e 'org.link)))
         ((bold italic underline strike code verbatim)
          (let ((face (case k
                        ((bold) 'org.bold)
                        ((italic) 'org.italic)
                        ((underline) 'org.underline)
                        ((strike) 'org.strike)
                        ((code) 'org.code)
                        ((verbatim) 'org.verbatim))))
            (list (list s (+ s 1) 'org.markup)
                  (list (+ s 1) (- e 1) face)
                  (list (- e 1) e 'org.markup))))
         (else '()))))
   inlines))

(define (element-highlights text el from to)
  (let ((k (org-element-kind el))
        (s (org-element-start el))
        (e (org-element-end el))
        (d (org-element-data el)))
    (if (or (<= e from) (>= s to))
        '()
        (case k
          ((planning)
           (append-map
            (lambda (entry)
              (let ((ks (list-ref entry 1))
                    (ke (list-ref entry 2))
                    (ts-s (list-ref entry 3))
                    (ts-e (list-ref entry 4))
                    (ts (list-ref entry 5)))
                (list (list ks ke 'org.planning)
                      (list ts-s ts-e (if (org-timestamp-active? ts) 'org.timestamp.active 'org.timestamp.inactive)))))
            (car d)))
          ((drawer)
           (let* ((dname (car d))
                  (hdr-end (cadr d))
                  (ftr-start (caddr d))
                  (entries (cadddr d))
                  (base (list (list s hdr-end 'org.drawer)
                              (list ftr-start e 'org.drawer))))
             (if (string=? dname "PROPERTIES")
                 (append base
                         (append-map (lambda (p)
                                       (list (list (- (list-ref p 1) 1) (+ (list-ref p 2) 1) 'org.property.key)
                                             (list (list-ref p 3) (list-ref p 4) 'org.property.value)))
                                     entries))
                 (append base (inline-highlights (org-parse-inline text (max (+ hdr-end 1) from) (min ftr-start to)))))))
          ((src-block block)
           (let ((body-start (list-ref d 3))
                 (body-end (list-ref d 4)))
             (list (list s body-start 'org.block.delimiter)
                   (list body-start body-end 'org.block.body)
                   (list body-end e 'org.block.delimiter))))
          ((table)
           (list (list (max s from) (min e to) 'org.table)))
          ((list-item)
           (let* ((b-start (list-ref d 1))
                  (b-end (list-ref d 2))
                  (cb-start (list-ref d 4))
                  (cb-end (list-ref d 5))
                  (cb-state (list-ref d 6))
                  (c-start (list-ref d 7))
                  (base (list (list b-start b-end 'org.list.bullet)))
                  (cb-hl (if cb-start
                             (list (list cb-start cb-end (if (eq? cb-state 'on) 'org.checkbox.done 'org.checkbox)))
                             '()))
                  (inlines (inline-highlights (org-parse-inline text (max c-start from) (min e to)))))
             (append base cb-hl inlines)))
          ((keyword) (list (list s e 'org.keyword)))
          ((comment) (list (list s e 'org.comment)))
          ((rule) (list (list s e 'org.rule)))
          ((paragraph) (inline-highlights (org-parse-inline text (max s from) (min e to))))
          (else '())))))

(define (org-highlight-layer d from to)
  (let* ((rev (document-revision d))
         (b (document-buffer d))
         (todo (if b (option b 'org-todo-keywords) (option #f 'org-todo-keywords)))
         (done (if b (option b 'org-done-keywords) (option #f 'org-done-keywords)))
         (hide-stars? (if b (option b 'org-hide-leading-stars) #t))
         (ckey (list rev from to hide-stars? todo done))
         (cached (hash-table-ref/default %org-layer-cache d #f)))
    (if (and cached (equal? (car cached) ckey))
        (cdr cached)
        (let* ((pair (org-doc-outline d))
               (text (car pair))
               (ol (cdr pair))
               (hs (org-outline-headings ol))
               (n (vector-length hs))
               (first-h-start (if (> n 0) (org-heading-start (vector-ref hs 0)) (org-utf8-length text)))
               (acc '())
               (push! (lambda (x) (set! acc (cons x acc))))
               (push-all! (lambda (xs) (for-each (lambda (x) (set! acc (cons x acc))) xs))))
          ;; Preamble elements if visible
          (when (< from first-h-start)
            (for-each (lambda (el) (push-all! (element-highlights text el from to)))
                      (org-cached-section d text 0 first-h-start)))
          ;; Headings intersecting [from, to)
          (let* ((start-h (org-heading-at ol from))
                 (start-idx (if start-h (org-heading-index start-h) 0)))
            (let loop ((i start-idx))
              (when (< i n)
                (let* ((h (vector-ref hs i))
                       (hstart (org-heading-start h))
                       (hlen-end (org-heading-line-end h))
                       (bstart (org-heading-body-start h))
                       (sec-end (org-heading-section-end ol h)))
                  (when (< hstart to)
                    ;; Headline highlights
                    (when (< from hlen-end)
                      (let* ((lvl (org-heading-level h))
                             (sr (org-heading-stars-range h))
                             (kr (org-heading-keyword-range h))
                             (pr (org-heading-priority-range h))
                             (tr (org-heading-title-range h))
                             (tgr (org-heading-tags-range h)))
                        (push! (list hstart hlen-end (heading-level-face lvl)))
                        (when (and hide-stars? (> lvl 1))
                          (push! (list (car sr) (- (cdr sr) 1) 'org.stars.leading)))
                        (when kr
                          (push! (list (car kr) (cdr kr) (todo-keyword-face ol (org-heading-keyword h)))))
                        (when pr
                          (push! (list (car pr) (cdr pr) 'org.priority)))
                        (when tgr
                          (push! (list (car tgr) (cdr tgr) 'org.tag)))
                        ;; Inline objects in heading title (links, cookies, timestamps)
                        (when (< (car tr) (cdr tr))
                          (push-all! (inline-highlights (org-parse-inline text (car tr) (cdr tr)))))))
                    ;; Section body elements if visible
                    (when (and (< bstart sec-end) (< from sec-end) (> to bstart))
                      (for-each (lambda (el) (push-all! (element-highlights text el from to)))
                                (org-cached-section d text bstart sec-end)))
                    (loop (+ i 1)))))))
          (let ((res (reverse acc)))
            (hash-table-set! %org-layer-cache d (cons ckey res))
            res)))))

;; ----- org-indent-mode: visual indentation via inlays + hangs -----

(define (org-indent-level-spaces b level)
  (let ((per (if b (option b 'org-indent-indentation-per-level) 2)))
    (max 0 (* (- level 1) per))))

(define (src-block-rail-at elems ls)
  ;; Return "┌ ", "│ ", "└ ", or #f if LS is a line of a src-block in ELEMS.
  (let loop ((es elems))
    (if (null? es)
        #f
        (let ((el (car es)))
          (if (and (eq? (org-element-kind el) 'src-block)
                   (>= ls (org-element-start el))
                   (< ls (org-element-end el)))
              (let* ((data (org-element-data el))
                     (hdr-end (list-ref data 2))
                     (body-end (list-ref data 4)))
                (cond
                  ((<= ls hdr-end) "┌ ")
                  ((>= ls body-end) "└ ")
                  (else "│ ")))
              (loop (cdr es)))))))

(define (org-indent-inlays d from to)
  (let* ((rev (document-revision d))
         (b (document-buffer d))
         (per (if b (option b 'org-indent-indentation-per-level) 2))
         (ckey (list rev from to per))
         (cached (hash-table-ref/default %org-indent-inlay-cache d #f)))
    (if (and cached (equal? (car cached) ckey))
        (cdr cached)
        (let* ((pair (org-doc-outline d))
               (text (car pair))
               (ol (cdr pair))
               (hs (org-outline-headings ol))
               (n (vector-length hs))
               (start-h (org-heading-at ol from))
               (start-idx (if start-h (org-heading-index start-h) 0))
               (res
                (let loop ((i start-idx) (acc '()))
                  (if (>= i n)
                      (reverse acc)
                      (let* ((h (vector-ref hs i))
                             (hstart (org-heading-start h))
                             (bstart (org-heading-body-start h))
                             (sec-end (org-heading-section-end ol h))
                             (lvl (org-heading-level h)))
                        (if (>= hstart to)
                            (reverse acc)
                            (let* ((h-pad (org-indent-level-spaces b lvl))
                                   (body-pad (+ h-pad lvl 1))
                                   (acc (if (and (> h-pad 0) (>= hstart from) (< hstart to))
                                            (cons (list hstart (make-string h-pad #\space) 'org.indent) acc)
                                            acc))
                                   ;; Walk visible body lines; add org-modern-indent style rails on src blocks.
                                   (acc
                                    (if (and (> body-pad 0) (< bstart sec-end) (< from sec-end) (> to bstart))
                                        (let* ((pad-str (make-string body-pad #\space))
                                               (elems (org-cached-section d text bstart sec-end))
                                               (vis-end (min sec-end to))
                                               (first-p (max bstart from))
                                               (ls0 (line-start d first-p))
                                               (start-ls (if (< ls0 first-p)
                                                             (let ((le (line-end d ls0)))
                                                               (if (< le vis-end) (+ le 1) vis-end))
                                                             ls0)))
                                          (let lloop ((ls start-ls) (a acc))
                                            (if (>= ls vis-end)
                                                a
                                                (let* ((le (line-end d ls))
                                                       (rail (src-block-rail-at elems ls))
                                                       (a1 (cons (list ls pad-str 'org.indent) a))
                                                       (a2 (if rail (cons (list ls rail 'org.block.delimiter) a1) a1)))
                                                  (lloop (if (< le vis-end) (+ le 1) vis-end) a2)))))
                                        acc)))
                              (loop (+ i 1) acc))))))))
          (hash-table-set! %org-indent-inlay-cache d (cons ckey res))
          res))))

(define (org-indent-projections d from to)
  ;; Hanging indent for wrapped lines under heading titles and list items.
  (let* ((rev (document-revision d))
         (ckey (list rev from to))
         (cached (hash-table-ref/default %org-indent-proj-cache d #f)))
    (if (and cached (equal? (car cached) ckey))
        (cdr cached)
        (let* ((pair (org-doc-outline d))
               (text (car pair))
               (ol (cdr pair))
               (hs (org-outline-headings ol))
               (n (vector-length hs))
               (start-h (org-heading-at ol from))
               (start-idx (if start-h (org-heading-index start-h) 0))
               (res
                (let loop ((i start-idx) (acc '()))
                  (if (>= i n)
                      (reverse acc)
                      (let* ((h (vector-ref hs i))
                             (hstart (org-heading-start h))
                             (bstart (org-heading-body-start h))
                             (sec-end (org-heading-section-end ol h)))
                        (if (>= hstart to)
                            (reverse acc)
                            (let* ((tr (org-heading-title-range h))
                                   (acc1 (if (and (>= hstart from) (< (car tr) (cdr tr)))
                                             (cons (list 'hang hstart (car tr)) acc)
                                             acc))
                                   ;; Also emit hangs for list items so wrapped bullets align under text.
                                   (acc2
                                    (if (and (< bstart sec-end) (< from sec-end) (> to bstart))
                                        (let ((elems (org-cached-section d text bstart sec-end)))
                                          (let eloop ((es elems) (a acc1))
                                            (if (null? es)
                                                a
                                                (let ((el (car es)))
                                                  (if (and (eq? (org-element-kind el) 'list-item)
                                                           (>= (org-element-start el) from)
                                                           (< (org-element-start el) to))
                                                      (let* ((ld (org-element-data el))
                                                             (hang-at (min (list-ref ld 7) (org-element-end el))))
                                                        (eloop (cdr es)
                                                               (if (> hang-at (org-element-start el))
                                                                   (cons (list 'hang (org-element-start el) hang-at) a)
                                                                   a)))
                                                      (eloop (cdr es) a))))))
                                        acc1)))
                              (loop (+ i 1) acc2))))))))
          (hash-table-set! %org-indent-proj-cache d (cons ckey res))
          res))))



(define (inline-conceals-in text from to hide-links? hide-emph? acc)
  (let loop ((ins (org-parse-inline text from to)) (a acc))
    (if (null? ins)
        a
        (let* ((in (car ins))
               (k (org-inline-kind in))
               (s (org-inline-start in))
               (e (org-inline-end in))
               (d (org-inline-data in)))
          (cond
            ((and hide-links? (eq? k 'link))
             (let ((ds (list-ref d 1))
                   (de (list-ref d 2))
                   (ts (list-ref d 3))
                   (te (list-ref d 4)))
               (cond
                 ((and ds de (> ds s) (< de e))
                  (loop (cdr ins) (cons (list 'conceal de e) (cons (list 'conceal s ds) a))))
                 ((and ts te (= (- ts s) 2) (= (- e te) 2))
                  (loop (cdr ins) (cons (list 'conceal te e) (cons (list 'conceal s ts) a))))
                 (else (loop (cdr ins) a)))))
            ((and hide-emph? (memq k '(bold italic underline strike code verbatim)) (>= (- e s) 3))
             (loop (cdr ins) (cons (list 'conceal (- e 1) e) (cons (list 'conceal s (+ s 1)) a))))
            (else (loop (cdr ins) a)))))))

(define (org-conceal-projections d from to)
  ;; Inline concealment for Org links and emphasis markers in [from, to).
  (let* ((buf (document-buffer d))
         (hide-links? (option buf 'org-hide-link-brackets))
         (hide-emph? (option buf 'org-hide-emphasis-markers)))
    (if (and (not hide-links?) (not hide-emph?))
        '()
        (let* ((rev (document-revision d))
               (ckey (list rev from to hide-links? hide-emph?))
               (cached (hash-table-ref/default %org-conceal-cache d #f)))
          (if (and cached (equal? (car cached) ckey))
              (cdr cached)
              (let* ((pair (org-doc-outline d))
                     (text (car pair))
                     (ol (cdr pair))
                     (hs (org-outline-headings ol))
                     (n (vector-length hs))
                     (acc '()))
                ;; Preamble before first heading
                (let ((pre-end (if (> n 0) (org-heading-start (vector-ref hs 0)) (org-utf8-length text))))
                  (when (and (< from pre-end) (> to 0))
                    (for-each
                     (lambda (el)
                       (when (and (memq (org-element-kind el) '(paragraph list-item))
                                  (< (org-element-start el) to)
                                  (> (org-element-end el) from))
                         (set! acc (inline-conceals-in text (org-element-start el) (org-element-end el) hide-links? hide-emph? acc))))
                     (org-cached-section d text 0 pre-end))))
                ;; Headings and sections overlapping [from, to)
                (let* ((start-h (org-heading-at ol from))
                       (start-idx (if start-h (org-heading-index start-h) 0)))
                  (let loop ((i start-idx))
                    (when (< i n)
                      (let* ((h (vector-ref hs i))
                             (hstart (org-heading-start h)))
                        (when (< hstart to)
                          (let ((tr (org-heading-title-range h))
                                (bstart (org-heading-body-start h))
                                (sec-end (org-heading-section-end ol h)))
                            (when (and (< (car tr) (cdr tr)) (>= (cdr tr) from) (<= (car tr) to))
                              (set! acc (inline-conceals-in text (car tr) (cdr tr) hide-links? hide-emph? acc)))
                            (when (and (< bstart sec-end) (< from sec-end) (> to bstart))
                              (for-each
                               (lambda (el)
                                 (when (and (memq (org-element-kind el) '(paragraph list-item))
                                            (< (org-element-start el) to)
                                            (> (org-element-end el) from))
                                   (set! acc (inline-conceals-in text (org-element-start el) (org-element-end el) hide-links? hide-emph? acc))))
                               (org-cached-section d text bstart sec-end))))
                          (loop (+ i 1)))))))
                (let ((res (reverse acc)))
                  (hash-table-set! %org-conceal-cache d (cons ckey res))
                  res)))))))

(define-minor-mode org-indent-mode
  "Visually indent Org headings and body text according to outline level."
  #:inlays org-indent-inlays
  #:projections org-indent-projections)

;; ----- Target resolution (links and headings) -----

(define (org-link-at-point d pos)
  (let* ((ls (line-start d pos))
         (le (line-end d pos))
         (text (document-string d))
         (inlines (org-parse-inline text ls le)))
    (find (lambda (in) (and (eq? (org-inline-kind in) 'link)
                            (<= (org-inline-start in) pos)
                            (< pos (org-inline-end in))))
          inlines)))

(define (org-target-at b pos)
  (let* ((d (buffer-document b))
         (link (org-link-at-point d pos)))
    (and link
         (let ((raw (car (org-inline-data link))))
           (cond
             ((string-prefix? "file:" raw)
              (target 'file (substring raw 5)))
             ((or (string-prefix? "https://" raw) (string-prefix? "http://" raw))
              (target 'url raw))
             (else (target 'org-link raw)))))))

;; ----- Editing command helpers -----

(define (org-current-heading s)
  (let* ((d (doc s))
         (pair (org-doc-outline d))
         (h (org-heading-at (cdr pair) (point s))))
    (unless h (error "Not on an Org heading"))
    (values (car pair) (cdr pair) h)))

(define (org-apply-session-edits! s edits)
  (when (pair? edits)
    (edit! s edits 'new)))

;; Prompt in the minibuffer and apply edits against the originating view & revision.
(define (org-prompt-edit! s prompt initial candidates make-edits)
  (let* ((v (session-view s))
         (d (view-document v))
         (base-rev (document-revision d)))
    (completing-read s prompt candidates
                     #:initial initial
                     #:require-match #f
                     #:accept
                     (lambda (s cand)
                       (let* ((input (if (string? cand) cand (candidate-text cand)))
                              (edits (make-edits input)))
                         (when (pair? edits)
                           (check-editable! v)
                           (view-edit-at! v base-rev edits "new")))))))

;; ----- Navigation commands -----

(define-command (org-next-visible-heading s n)
  "Move point to the next visible Org heading."
  (let* ((pair (org-doc-outline (doc s)))
         (hs (org-outline-headings (cdr pair)))
         (len (vector-length hs))
         (folds (org-view-folds (session-view s)))
         (p (point s))
         (cur (org-heading-at (cdr pair) p))
         (start-idx (if (not cur) 0 (+ (org-heading-index cur) 1))))
    (let loop ((i start-idx) (rem (max 1 n)) (found #f))
      (cond
        ((or (>= i len) (= rem 0))
         (if found
             (move! s (lambda (_) (org-heading-start found)))
             (message! s "No next heading")))
        (else
         (let ((h (vector-ref hs i)))
           (if (and (> (org-heading-start h) p) (not (pos-hidden-in? folds (org-heading-start h))))
               (loop (+ i 1) (- rem 1) h)
               (loop (+ i 1) rem found))))))))

(define-command (org-previous-visible-heading s n)
  "Move point to the previous visible Org heading."
  (let* ((pair (org-doc-outline (doc s)))
         (hs (org-outline-headings (cdr pair)))
         (folds (org-view-folds (session-view s)))
         (p (point s))
         (cur (org-heading-at (cdr pair) p)))
    (cond
      ((not cur) (message! s "No previous heading"))
      ((and (> p (org-heading-start cur)) (not (pos-hidden-in? folds (org-heading-start cur))))
       (move! s (lambda (_) (org-heading-start cur))))
      (else
       (let loop ((i (- (org-heading-index cur) 1)) (rem (max 1 n)) (found #f))
         (cond
           ((or (< i 0) (= rem 0))
            (if found
                (move! s (lambda (_) (org-heading-start found)))
                (message! s "No previous heading")))
           (else
            (let ((h (vector-ref hs i)))
              (if (not (pos-hidden-in? folds (org-heading-start h)))
                  (loop (- i 1) (- rem 1) h)
                  (loop (- i 1) rem found))))))))))

;; ----- Visibility cycling & reveal commands -----

(define-command (org-cycle s n)
  "Cycle visibility of the Org headline, drawer, or block at point.
Matches Emacs Org: cycles a subtree only on its headline, toggles
drawers/blocks on their lines, aligns tables, and leaves plain body
lines unfolded. Use `org-shifttab` (`S-TAB`) for global cycling."
  (let* ((v (session-view s))
         (d (doc s))
         (pair (org-doc-outline d))
         (text (car pair))
         (ol (cdr pair))
         (p (point s))
         (h (org-heading-at ol p)))
    (cond
      ((let ((edits (org-edit-align-table text ol p)))
         (and (pair? edits) edits))
       => (lambda (edits)
            (org-apply-session-edits! s edits)
            (message! s "Aligned table")))
      ((not h)
       (if (and (option (current-buffer s) 'org-cycle-global-at-bob)
                (> (vector-length (org-outline-headings ol)) 0))
           (org-shifttab s n)
           (message! s "Use S-TAB for global visibility cycling")))
      (else
       (let* ((keys (heading-identity-keys text ol))
              (hk (vector-ref keys (org-heading-index h)))
              (hstart (org-heading-start h))
              (hlen-end (org-heading-line-end h))
              (bstart (org-heading-body-start h))
              (sec-end (org-heading-section-end ol h))
              (sub-end (org-heading-subtree-end h))
              (on-headline? (and (>= p hstart) (<= p hlen-end)))
              (e-tbl (element-fold-table v))
              (h-tbl (heading-state-table v)))
         (let ((on-elem
                (and (not on-headline?) (>= p bstart) (< p sec-end)
                     (let loop ((es (org-cached-section d text bstart sec-end)) (ord 0))
                       (if (null? es)
                           #f
                           (let* ((el (car es))
                                  (kind (org-element-kind el)))
                             (if (memq kind '(drawer src-block block))
                                 (if (and (<= (org-element-start el) p) (< p (org-element-end el)))
                                     (cons (list hk kind ord) el)
                                     (loop (cdr es) (+ ord 1)))
                                 (loop (cdr es) ord))))))))
           (cond
             (on-elem
              (let* ((ek (car on-elem))
                     (folded? (hash-table-ref/default e-tbl ek #f)))
                (if folded?
                    (begin (hash-table-delete! e-tbl ek) (invalidate-view-folds! v) (message! s "Unfolded"))
                    (begin (hash-table-set! e-tbl ek #t)
                           (invalidate-view-folds! v)
                           (normalize-point-visible! s)
                           (message! s "Folded")))))
             ((not on-headline?)
              ;; Plain body line: match Emacs Org default (do not fold the enclosing heading).
              (indent-for-tab-command s n))
             ((>= bstart sub-end)
              (message! s "EMPTY ENTRY"))
             (else
              (let* ((cur (hash-table-ref/default h-tbl hk #f))
                     (children (org-heading-children ol h))
                     (has-children? (pair? children))
                     (next (cond
                             ((not cur) 'folded)
                             ((and (eq? cur 'folded) has-children?) 'children)
                             (else #f))))
                (if next
                    (hash-table-set! h-tbl hk next)
                    (hash-table-delete! h-tbl hk))
                (when (eq? next 'children)
                  ;; Mark each direct child folded so cycling a child works independently.
                  (for-each (lambda (c)
                              (hash-table-set! h-tbl (vector-ref keys (org-heading-index c)) 'folded))
                            children))
                (when (not next)
                  (let* ((hs (org-outline-headings ol))
                         (nlen (vector-length hs)))
                    (let clear ((i (+ (org-heading-index h) 1)))
                      (when (and (< i nlen) (< (org-heading-start (vector-ref hs i)) sub-end))
                        (hash-table-delete! h-tbl (vector-ref keys i))
                        (clear (+ i 1))))))
                (invalidate-view-folds! v)
                (normalize-point-visible! s)
                (message! s (case next
                              ((folded) "FOLDED")
                              ((children) "CHILDREN")
                              (else "SUBTREE"))))))))))))

(define (buffer-in-overview? hs keys h-tbl)
  (let ((len (vector-length hs)))
    (let loop ((i 0) (any-top? #f))
      (if (>= i len)
          any-top?
          (let ((h (vector-ref hs i)))
            (if (not (org-heading-parent h))
                (if (eq? (hash-table-ref/default h-tbl (vector-ref keys i) #f) 'folded)
                    (loop (+ i 1) #t)
                    #f)
                (loop (+ i 1) any-top?)))))))

(define (buffer-in-contents? hs keys h-tbl)
  (let ((len (vector-length hs)))
    (and (> len 0)
         (let loop ((i 0))
           (cond
             ((>= i len) #t)
             ((eq? (hash-table-ref/default h-tbl (vector-ref keys i) #f) 'body-only)
              (loop (+ i 1)))
             (else #f))))))

(define-command (org-shifttab s n)
  "Cycle global visibility of all headings: OVERVIEW -> CONTENTS -> SHOW ALL."
  (let* ((v (session-view s))
         (pair (org-doc-outline (doc s)))
         (text (car pair))
         (ol (cdr pair))
         (hs (org-outline-headings ol))
         (len (vector-length hs))
         (keys (heading-identity-keys text ol))
         (h-tbl (heading-state-table v))
         (next (cond
                 ((buffer-in-overview? hs keys h-tbl) 'contents)
                 ((buffer-in-contents? hs keys h-tbl) 'show-all)
                 (else 'overview))))
    (for-each (lambda (k) (hash-table-delete! h-tbl k)) (hash-table-keys h-tbl))
    (case next
      ((overview)
       (let loop ((i 0))
         (when (< i len)
           (let ((h (vector-ref hs i)))
             (when (not (org-heading-parent h))
               (hash-table-set! h-tbl (vector-ref keys i) 'folded)))
           (loop (+ i 1)))))
      ((contents)
       (let loop ((i 0))
         (when (< i len)
           (hash-table-set! h-tbl (vector-ref keys i) 'body-only)
           (loop (+ i 1)))))
      ((show-all)
       (let ((e-tbl (element-fold-table v)))
         (for-each (lambda (k) (hash-table-delete! e-tbl k)) (hash-table-keys e-tbl)))))
    (set-view-global-visibility! v next)
    (invalidate-view-folds! v)
    (normalize-point-visible! s)
    (message! s (case next
                  ((overview) "OVERVIEW")
                  ((contents) "CONTENTS")
                  (else "SHOW ALL")))))

(define-command (org-reveal s n)
  "Unfold any Org folds hiding point in the current view."
  (let* ((v (session-view s))
         (pair (org-doc-outline (doc s)))
         (text (car pair))
         (ol (cdr pair))
         (keys (heading-identity-keys text ol))
         (h-tbl (heading-state-table v))
         (e-tbl (element-fold-table v))
         (h (org-heading-at ol (point s))))
    (when h
      (let loop ((cur h))
        (when cur
          (hash-table-delete! h-tbl (vector-ref keys (org-heading-index cur)))
          (loop (and (org-heading-parent cur) (vector-ref (org-outline-headings ol) (org-heading-parent cur))))))
      (invalidate-view-folds! v)
      (let ((hk (vector-ref keys (org-heading-index h))))
        (for-each (lambda (ek)
                    (when (and (pair? ek) (equal? (car ek) hk))
                      (hash-table-delete! e-tbl ek)))
                  (hash-table-keys e-tbl))))
    (message! s "Revealed")))

;; ----- External-change handling & dirty-safe auto-revert -----

(define (view-pos v)
  (let ((rs (view-ranges v)))
    (if (pair? rs) (cadr (car rs)) 0)))

(define (org-reload-buffer! s b)
  (let* ((d (buffer-document b))
         (path (document-path d)))
    (cond
      ((not path) #f)
      ((document-dirty? d)
       (message! s (string-append (abbreviate-path path) " has unsaved edits; not auto-reloading"))
       #f)
      (else
       (let* ((pair (org-doc-outline d))
              (text (car pair))
              (ol (cdr pair))
              (keys (heading-identity-keys text ol))
              (views (filter (lambda (v) (document=? (view-document v) d)) (session-panes s)))
              (saved (map (lambda (v)
                            (let* ((p (view-pos v))
                                   (h (org-heading-at ol p)))
                              (cons v (and h (cons (vector-ref keys (org-heading-index h))
                                                   (- p (org-heading-start h)))))))
                          views)))
         (let ((changed? (reload-document! d)))
           (when changed?
             (let* ((pair2 (org-doc-outline d))
                    (text2 (car pair2))
                    (ol2 (cdr pair2))
                    (hs2 (org-outline-headings ol2))
                    (keys2 (heading-identity-keys text2 ol2))
                    (len2 (document-length d)))
               (for-each
                (lambda (entry)
                  (let ((v (car entry))
                        (info (cdr entry)))
                    (when info
                      (let* ((hk (car info))
                             (rel (cdr info))
                             (found-idx
                              (let loop ((i 0))
                                (cond ((>= i (vector-length keys2)) #f)
                                      ((equal? (vector-ref keys2 i) hk) i)
                                      (else (loop (+ i 1)))))))
                        (when found-idx
                          (let* ((h2 (vector-ref hs2 found-idx))
                                 (target (min (+ (org-heading-start h2) rel) (org-heading-subtree-end h2) len2)))
                            (view-set-ranges! v (list (list target target)) 0)))))
                    ;; Keep point outside any active fold after reload.
                    (let* ((p (view-pos v))
                           (folds (org-view-folds v)))
                      (for-each (lambda (r)
                                  (when (and (> p (car r)) (< p (cadr r)))
                                    (view-set-ranges! v (list (list (car r) (car r))) 0)))
                                folds))))
                saved)))
           changed?))))))

(define-command (org-reload s n)
  "Reload the current Org buffer from disk if clean, preserving folds."
  (let ((b (current-buffer s)))
    (guard (e (#t (message! s (string-append "Reload failed: " (error-object-message e)))))
      (if (org-reload-buffer! s b)
          (message! s "Reloaded Org buffer from disk")
          (unless (document-dirty? (buffer-document b))
            (message! s "Org buffer is up to date"))))))

(define-minor-mode org-auto-revert-mode
  "Check clean Org buffers for external disk changes on focus.")

(add-hook! 'frontend-focus 'org-auto-revert
           (lambda (s focused?)
             (when focused?
               (for-each (lambda (b)
                           (when (and (eq? (buffer-mode b) 'org-mode)
                                      (option b 'org-auto-revert-mode)
                                      (not (document-dirty? (buffer-document b))))
                             (guard (e (#t #f)) (org-reload-buffer! s b))))
                         (buffer-list)))))

(define-command (org-forward-heading-same-level s n)
  "Move point to the next sibling Org heading at the same level."
  (let-values (((text ol h) (org-current-heading s)))
    (let* ((hs (org-outline-headings ol))
           (len (vector-length hs))
           (lvl (org-heading-level h))
           (parent (org-heading-parent h)))
      (let loop ((i (+ (org-heading-index h) 1)) (rem (max 1 n)) (found #f))
        (cond
          ((or (>= i len) (= rem 0))
           (if found
               (move! s (lambda (_) (org-heading-start found)))
               (message! s "No next sibling heading")))
          (else
           (let ((x (vector-ref hs i)))
             (cond
               ((< (org-heading-level x) lvl)
                (if found
                    (move! s (lambda (_) (org-heading-start found)))
                    (message! s "No next sibling heading")))
               ((and (= (org-heading-level x) lvl) (eqv? (org-heading-parent x) parent))
                (loop (+ i 1) (- rem 1) x))
               (else (loop (+ i 1) rem found))))))))))

(define-command (org-backward-heading-same-level s n)
  "Move point to the previous sibling Org heading at the same level."
  (let-values (((text ol h) (org-current-heading s)))
    (let* ((hs (org-outline-headings ol))
           (lvl (org-heading-level h))
           (parent (org-heading-parent h)))
      (let loop ((i (- (org-heading-index h) 1)) (rem (max 1 n)) (found #f))
        (cond
          ((or (< i 0) (= rem 0))
           (if found
               (move! s (lambda (_) (org-heading-start found)))
               (message! s "No previous sibling heading")))
          (else
           (let ((x (vector-ref hs i)))
             (cond
               ((< (org-heading-level x) lvl)
                (if found
                    (move! s (lambda (_) (org-heading-start found)))
                    (message! s "No previous sibling heading")))
               ((and (= (org-heading-level x) lvl) (eqv? (org-heading-parent x) parent))
                (loop (- i 1) (- rem 1) x))
               (else (loop (- i 1) rem found))))))))))

(define-command (org-up-heading s n)
  "Move point to the parent Org heading."
  (let-values (((text ol h) (org-current-heading s)))
    (let ((p (org-heading-parent h)))
      (if p
          (move! s (lambda (_) (org-heading-start (vector-ref (org-outline-headings ol) p))))
          (message! s "Already at top-level heading")))))

;; ----- Task & metadata commands -----

(define-command (org-todo s n)
  "Cycle the current Org heading's TODO keyword."
  (let-values (((text ol h) (org-current-heading s)))
    (let* ((b (current-buffer s))
           (log-done (option b 'org-log-done))
           (drawer (option b 'org-log-into-drawer))
           (edits (org-edit-cycle-todo text ol h #:log-done log-done #:log-into-drawer drawer)))
      (org-apply-session-edits! s edits))))

(define-command (org-priority s n)
  "Cycle the current Org heading's priority (#A -> #B -> #C -> none)."
  (let-values (((text ol h) (org-current-heading s)))
    (let* ((cur (org-heading-priority h))
           (next (cond ((not cur) #\A)
                       ((char=? cur #\A) #\B)
                       ((char=? cur #\B) #\C)
                       (else #f)))
           (edits (org-edit-set-priority text ol h next)))
      (org-apply-session-edits! s edits))))

(define (heading-existing-planning-ts text ol h kind)
  (let* ((elems (org-parse-section text (org-heading-body-start h) (org-heading-section-end ol h)))
         (plan (org-section-planning elems))
         (entries (if plan (car (org-element-data plan)) '()))
         (hit (find (lambda (e) (eq? (car e) kind)) entries)))
    (and hit (list-ref hit 5))))

(define-command (org-schedule s n)
  "Set the SCHEDULED timestamp on the current Org heading.
Accepts ISO dates or relative specs (`+2d`, `fri`, `tomorrow`, `HH:MM`),
preserving existing time/repeater/warning; empty clears SCHEDULED."
  (let-values (((text ol h) (org-current-heading s)))
    (let* ((existing (heading-existing-planning-ts text ol h 'scheduled))
           (default (if existing (date->string (org-timestamp-day existing)) (date->string (car (local-now))))))
      (org-prompt-edit! s "Scheduled (YYYY-MM-DD / +2d / fri, empty clears): " default '()
                        (lambda (input)
                          (let ((trimmed (string-trim input)))
                            (if (string-null? trimmed)
                                (org-edit-set-planning text ol h 'scheduled #f)
                                (let ((ts (org-parse-relative-date trimmed #:base-ts existing)))
                                  (if ts
                                      (org-edit-set-planning text ol h 'scheduled ts)
                                      (begin (message! s (string-append "Invalid date: " trimmed)) '()))))))))))

(define-command (org-deadline s n)
  "Set the DEADLINE timestamp on the current Org heading.
Accepts ISO dates or relative specs (`+2d`, `fri`, `tomorrow`, `HH:MM`),
preserving existing time/repeater/warning; empty clears DEADLINE."
  (let-values (((text ol h) (org-current-heading s)))
    (let* ((existing (heading-existing-planning-ts text ol h 'deadline))
           (default (if existing (date->string (org-timestamp-day existing)) (date->string (car (local-now))))))
      (org-prompt-edit! s "Deadline (YYYY-MM-DD / +2d / fri, empty clears): " default '()
                        (lambda (input)
                          (let ((trimmed (string-trim input)))
                            (if (string-null? trimmed)
                                (org-edit-set-planning text ol h 'deadline #f)
                                (let ((ts (org-parse-relative-date trimmed #:base-ts existing)))
                                  (if ts
                                      (org-edit-set-planning text ol h 'deadline ts)
                                      (begin (message! s (string-append "Invalid date: " trimmed)) '()))))))))))

(define-command (org-set-tags s n)
  "Set the tags on the current Org heading (colon- or space-separated)."
  (let-values (((text ol h) (org-current-heading s)))
    (let ((initial (string-join (org-heading-tags h) ":")))
      (org-prompt-edit! s "Tags: " initial '()
                        (lambda (input)
                          (let ((tags (filter (lambda (t) (not (string-null? t)))
                                              (append-map (lambda (w) (string-split w ":"))
                                                          (string-split (string-trim input))))))
                            (org-edit-set-tags text ol h tags)))))))

(define-command (org-set-property s n)
  "Set a property on the current Org heading (prompts for NAME=VALUE)."
  (let-values (((text ol h) (org-current-heading s)))
    (org-prompt-edit! s "Property (NAME=VALUE): " "" '()
                      (lambda (input)
                        (let ((eq (string-index input #\=)))
                          (if (not eq)
                              (begin (message! s "Use NAME=VALUE (empty value deletes)") '())
                              (let ((k (string-trim (substring input 0 eq)))
                                    (v (string-trim (substring input (+ eq 1)))))
                                (org-edit-set-property text ol h k (if (string-null? v) #f v)))))))))

(define-command (org-set-effort s n)
  "Set the EFFORT property on the current Org heading."
  (let-values (((text ol h) (org-current-heading s)))
    (let ((vals (option (current-buffer s) 'org-effort-values)))
      (org-prompt-edit! s "Effort: " "" vals
                        (lambda (input)
                          (let ((v (string-trim input)))
                            (org-edit-set-property text ol h "EFFORT" (if (string-null? v) #f v))))))))

;; ----- Structural editing commands -----

(define-command (org-do-promote s n)
  "Promote the current Org heading by one level."
  (let-values (((text ol h) (org-current-heading s)))
    (let ((edits (org-edit-promote-heading text ol h)))
      (if (null? edits)
          (message! s "Cannot promote a level-1 heading")
          (org-apply-session-edits! s edits)))))

(define-command (org-do-demote s n)
  "Demote the current Org heading by one level."
  (let-values (((text ol h) (org-current-heading s)))
    (org-apply-session-edits! s (org-edit-demote-heading text ol h))))

(define-command (org-promote-subtree s n)
  "Promote the current Org subtree by one level."
  (let-values (((text ol h) (org-current-heading s)))
    (let ((edits (org-edit-promote-subtree text ol h)))
      (if (null? edits)
          (message! s "Cannot promote a level-1 subtree")
          (org-apply-session-edits! s edits)))))

(define-command (org-demote-subtree s n)
  "Demote the current Org subtree by one level."
  (let-values (((text ol h) (org-current-heading s)))
    (org-apply-session-edits! s (org-edit-demote-subtree text ol h))))

(define-command (org-move-subtree-up s n)
  "Swap the current Org subtree with its previous sibling."
  (let-values (((text ol h) (org-current-heading s)))
    (let ((edits (org-edit-move-subtree-up text ol h)))
      (if (null? edits)
          (message! s "Cannot move subtree past first sibling")
          (begin
            (org-apply-session-edits! s edits)
            (move! s (lambda (_) (car (car edits)))))))))

(define-command (org-move-subtree-down s n)
  "Swap the current Org subtree with its next sibling."
  (let-values (((text ol h) (org-current-heading s)))
    (let ((edits (org-edit-move-subtree-down text ol h)))
      (if (null? edits)
          (message! s "Cannot move subtree past last sibling")
          (let* ((a-start (org-heading-start h))
                 (a-len (- (org-heading-subtree-end h) a-start))
                 (e (car edits))
                 (total-len (- (cadr e) (car e)))
                 (new-pos (+ a-start (- total-len a-len))))
            (org-apply-session-edits! s edits)
            (move! s (lambda (_) (min new-pos (document-length (doc s))))))))))

(define-command (org-meta-return s n)
  "Insert a new list item when in a list, or a heading at current level."
  (let* ((pair (org-doc-outline (doc s)))
         (list-res (org-edit-insert-list-item (car pair) (cdr pair) (point s) #:checkbox? #f))
         (res (or list-res (org-edit-insert-heading (car pair) (cdr pair) (point s) #:respect-content? #f))))
    (org-apply-session-edits! s (car res))
    (move! s (lambda (_) (min (cdr res) (document-length (doc s)))))))

(define-command (org-insert-todo-heading s n)
  "Insert a new checkbox item when in a list, or a TODO heading."
  (let* ((pair (org-doc-outline (doc s)))
         (list-res (org-edit-insert-list-item (car pair) (cdr pair) (point s) #:checkbox? #t))
         (res (or list-res (org-edit-insert-heading (car pair) (cdr pair) (point s) #:todo "TODO"))))
    (org-apply-session-edits! s (car res))
    (move! s (lambda (_) (min (cdr res) (document-length (doc s)))))))

(define-command (org-shift-right s n)
  "Shift timestamp at point +1 day, or cycle heading TODO forward."
  (let* ((d (doc s))
         (pair (org-doc-outline d))
         (ts-edits (org-edit-shift-timestamp-at (car pair) (point s) (max 1 n))))
    (if (pair? ts-edits)
        (org-apply-session-edits! s ts-edits)
        (org-todo s n))))

(define-command (org-shift-left s n)
  "Shift timestamp at point -1 day, or cycle heading TODO backward."
  (let* ((d (doc s))
         (pair (org-doc-outline d))
         (ts-edits (org-edit-shift-timestamp-at (car pair) (point s) (- (max 1 n)))))
    (if (pair? ts-edits)
        (org-apply-session-edits! s ts-edits)
        (let-values (((text ol h) (org-current-heading s)))
          (let* ((all (append (org-outline-todo-keywords ol) (org-outline-done-keywords ol)))
                 (cur (org-heading-keyword h))
                 (prev (if (not cur)
                           (and (pair? all) (car (reverse all)))
                           (let loop ((xs all) (last #f))
                             (cond ((null? xs) #f)
                                   ((string=? (car xs) cur) last)
                                   (else (loop (cdr xs) (car xs))))))))
            (org-apply-session-edits! s (org-edit-set-todo text ol h prev #:log-done #f)))))))

(define-command (org-shift-up s n)
  "Shift timestamp at point +1 day, or raise heading priority."
  (let* ((d (doc s))
         (pair (org-doc-outline d))
         (ts-edits (org-edit-shift-timestamp-at (car pair) (point s) (max 1 n))))
    (if (pair? ts-edits)
        (org-apply-session-edits! s ts-edits)
        (let-values (((text ol h) (org-current-heading s)))
          (let* ((cur (org-heading-priority h))
                 (nxt (case cur ((#\C) #\B) ((#\B) #\A) ((#\A) #f) (else #\C))))
            (org-apply-session-edits! s (org-edit-set-priority text ol h nxt)))))))

(define-command (org-shift-down s n)
  "Shift timestamp at point -1 day, or lower heading priority."
  (let* ((d (doc s))
         (pair (org-doc-outline d))
         (ts-edits (org-edit-shift-timestamp-at (car pair) (point s) (- (max 1 n)))))
    (if (pair? ts-edits)
        (org-apply-session-edits! s ts-edits)
        (let-values (((text ol h) (org-current-heading s)))
          (let* ((cur (org-heading-priority h))
                 (nxt (case cur ((#\A) #\B) ((#\B) #\C) ((#\C) #f) (else #\A))))
            (org-apply-session-edits! s (org-edit-set-priority text ol h nxt)))))))

(define-command (org-id-get-create s n)
  "Ensure the current Org heading has a UUID `:ID:` property."
  (let-values (((text ol h) (org-current-heading s)))
    (let ((existing (org-heading-property text ol h "ID")))
      (if existing
          (message! s (string-append "Existing :ID: " existing))
          (let ((nid (org-new-id)))
            (org-apply-session-edits! s (org-edit-set-property text ol h "ID" nid))
            (message! s (string-append "Created :ID: " nid)))))))

(define-command (org-refile s n)
  "Move the current Org subtree into another Org node (file or heading)."
  (let-values (((text ol h) (org-current-heading s)))
    (let* ((orig-v (session-view s))
           (orig-d (view-document orig-v))
           (orig-rev (document-revision orig-d))
           (h-start (org-heading-start h))
           (h-end (org-heading-subtree-end h))
           (h-lvl (org-heading-level h))
           (sub-text (org-slice-bytes text h-start h-end))
           (idx (org-mem-current-index))
           (cands (node-choice-candidates idx)))
      (completing-read
       s "Refile to node: " (map car cands)
       #:require-match #t
       #:accept
       (lambda (s cand)
         (let ((hit (assoc (cand->string cand) cands)))
           (when hit
             (let* ((nd (cdr hit))
                    (t-path (absolute-path (org-mem-node-path nd)))
                    (same-doc? (and (document-path orig-d)
                                    (string=? (absolute-path (document-path orig-d)) t-path)))
                    (t-lvl (+ (org-mem-node-level nd) 1))
                    (releveled (org-edit-relevel-subtree sub-text (- t-lvl h-lvl))))
               (if same-doc?
                   (message! s "Cross-file refile supported; use M-up/M-down within same file")
                   (let* ((td (file-document t-path))
                          (tpair (org-doc-outline td))
                          (nid (org-mem-node-id nd))
                          (th (and (org-mem-node-heading-index nd)
                                   (find-unique-heading-by-id (car tpair) (cdr tpair) nid))))
                     (if (and (org-mem-node-heading-index nd) (not th))
                         (message! s "Refile target heading is missing or ambiguous")
                         (if (not (= (document-revision orig-d) orig-rev))
                             (message! s "Source buffer changed; refile cancelled")
                             (let* ((tv (make-view td "user"))
                                    (old-t-str (document-string td))
                                    (add-edits (org-capture-append-edits (car tpair) releveled #:heading th)))
                               (check-editable! tv)
                               (check-editable! orig-v)
                               (view-edit-at! tv (document-revision td) add-edits "new")
                               (guard (e (#t
                                          (guard (_ (#t #f))
                                            (view-edit-at! tv (document-revision td)
                                                           (list (list 0 (document-length td) old-t-str)) "new"))
                                          (raise e)))
                                 (view-edit-at! orig-v orig-rev (list (list h-start h-end "")) "new"))
                               (message! s (string-append "Refiled to " (org-mem-node-title nd))))))))))))))))

(define-command (org-archive-subtree s n)
  "Archive the current Org subtree to `<file>_archive`."
  (let-values (((text ol h) (org-current-heading s)))
    (let* ((orig-v (session-view s))
           (orig-d (view-document orig-v))
           (orig-rev (document-revision orig-d))
           (p (document-path orig-d)))
      (if (not p)
          (message! s "Save buffer before archiving")
          (let* ((arch-path (string-append (absolute-path p) "_archive"))
                 (h-start (org-heading-start h))
                 (h-end (org-heading-subtree-end h))
                 (sub-text (org-edit-relevel-subtree (org-slice-bytes text h-start h-end) (- 1 (org-heading-level h))))
                 (ad (file-document arch-path))
                 (av (make-view ad "user"))
                 (add-edits (org-capture-append-edits (document-string ad) sub-text)))
            (check-editable! av)
            (check-editable! orig-v)
            (view-edit-at! av (document-revision ad) add-edits "new")
            (save-document! ad)
            (view-edit-at! orig-v orig-rev (list (list h-start h-end "")) "new")
            (message! s (string-append "Archived to " (abbreviate-path arch-path))))))))

(define-command (org-babel-execute-buffer s n)
  "Execute all executable Org source blocks in the current buffer."
  (let* ((d (doc s))
         (pair (org-doc-outline d))
         (blks (org-babel-blocks (car pair) (cdr pair)))
         (ran 0))
    ;; Execute from bottom to top so earlier byte offsets stay valid!
    (for-each
     (lambda (blk)
       (unless (or (org-babel-eval-disabled? blk) (org-babel-async? blk))
         (move! s (lambda (_) (org-src-block-body-start blk)))
         (org-babel-execute-src-block s 1)
         (set! ran (+ ran 1))))
     (reverse blks))
    (message! s (string-append "Executed " (number->string ran) " source block(s)"))))

(define-command (org-edit-src-exit s n)
  "Write the `*Org Src*` buffer back into its originating Org source block."
  (let* ((b (current-buffer s))
         (st (and b (buffer-state b))))
    (if (not (and (hash-table? st) (hash-table-ref/default st 'src-orig-doc #f)))
        (message! s "Not in an Org source edit buffer")
        (let* ((orig-d (hash-table-ref st 'src-orig-doc))
               (orig-rev (hash-table-ref st 'src-orig-rev))
               (bstart (hash-table-ref st 'src-body-start))
               (bend (hash-table-ref st 'src-body-end))
               (new-body (document-string (doc s)))
               (new-body (if (or (string=? new-body "") (string-suffix? "\n" new-body))
                             new-body
                             (string-append new-body "\n"))))
          (if (not (equal? orig-rev (document-revision orig-d)))
              (message! s "Originating Org buffer changed; refusing stale writeback")
              (let ((v (make-view orig-d "user")))
                (check-editable! v)
                (view-edit-at! v orig-rev (list (list bstart bend new-body)) "new")
                (show-document! s orig-d)
                (drop-buffer! s b)
                (message! s "Updated Org source block")))))))

(define-command (org-edit-src-abort s n)
  "Discard edits in `*Org Src*` and return to the Org buffer."
  (let* ((b (current-buffer s))
         (st (and b (buffer-state b)))
         (orig-d (and (hash-table? st) (hash-table-ref/default st 'src-orig-doc #f))))
    (when orig-d (show-document! s orig-d))
    (when b (drop-buffer! s b))
    (message! s "Aborted Org source edit")))

(define-minor-mode org-src-edit-mode
  "Minor mode for editing an Org source block body in its native major mode."
  #:keys '(("C-c C-c" org-edit-src-exit)
           ("C-c '" org-edit-src-exit)
           ("C-c C-k" org-edit-src-abort))
  #:normal '(("SPC m c" org-edit-src-exit)
             ("SPC m k" org-edit-src-abort)))

(define-command (org-edit-special s n)
  "Edit the Org source block at point in a dedicated native major-mode buffer."
  (let* ((d (doc s))
         (rev (document-revision d))
         (pair (org-doc-outline d))
         (blk (org-babel-block-at (car pair) (cdr pair) (point s))))
    (if (not blk)
        (message! s "No Org source block at point")
        (let* ((lang (org-src-block-lang blk))
               (fake-file (string-append "src." (lang-extension lang)))
               (mode (mode-for-file fake-file))
               (st (make-hash-table))
               (nd (make-document (org-src-block-body blk)))
               (nb (make-generated-buffer! (string-append "*Org Src " lang "*") nd mode #:state st)))
          (hash-table-set! st 'src-orig-doc d)
          (hash-table-set! st 'src-orig-rev rev)
          (hash-table-set! st 'src-body-start (org-src-block-body-start blk))
          (hash-table-set! st 'src-body-end (org-src-block-body-end blk))
          (show-buffer! s nb)
          (toggle-mode! s 'org-src-edit-mode)
          (message! s "Edit block: C-c C-c to commit, C-c C-k to abort")))))

(define-command (org-export s n)
  "Export the current Org buffer to Markdown (`md`), HTML (`html`), or Hugo (`hugo`)."
  (let* ((d (doc s))
         (pair (org-doc-outline d)))
    (completing-read
     s "Export format: " '("md — Markdown" "html — HTML" "hugo — Hugo Markdown")
     #:require-match #f
     #:accept
     (lambda (s cand)
       (let* ((c (string-downcase (string-trim (cand->string cand))))
              (out+mode
               (cond
                 ((string-prefix? "html" c) (cons (org->html (car pair) (cdr pair)) 'text-mode))
                 ((string-prefix? "hugo" c) (cons (org->hugo (car pair) (cdr pair)) 'text-mode))
                 (else (cons (org->markdown (car pair) (cdr pair)) 'text-mode))))
              (buf (make-generated-buffer! "*Org Export*" (make-document (car out+mode)) (cdr out+mode))))
         (pop-to-buffer! s buf)
         (message! s (string-append "Exported Org to " c)))))))

(define-command (org-insert-heading-respect-content s n)
  "Insert a new Org heading after the current subtree's content."
  (let* ((pair (org-doc-outline (doc s)))
         (res (org-edit-insert-heading (car pair) (cdr pair) (point s) #:respect-content? #t)))
    (org-apply-session-edits! s (car res))
    (move! s (lambda (_) (min (cdr res) (document-length (doc s)))))))

(define (read-process-stream p stream)
  (let loop ((chunks '()))
    (let ((c (process-read p stream)))
      (if (eof-object? c)
          (apply string-append (reverse chunks))
          (loop (cons c chunks))))))

(define (run-external-babel-sync dir prog args stdin-text)
  (call-with-process-at dir prog args
    (lambda (p)
      (when stdin-text (process-write p stdin-text))
      (process-close-input p)
      (let* ((err-task (spawn (lambda () (read-process-stream p 'stderr))))
             (out (read-process-stream p 'stdout))
             (err (task-join err-task))
             (status (process-wait p)))
        (values status (if (and (string? err) (> (string-length err) 0))
                           (string-append out (if (and (> (string-length out) 0)
                                                       (not (string-suffix? "\n" out)))
                                                  "\n" "")
                                          err)
                           out))))))

(define (resolve-babel-dir s blk)
  (let* ((dval (header-arg-ref (org-src-block-params blk) ":dir" #f))
         (base (default-directory s)))
    (cond
      ((not dval) base)
      ((or (string-prefix? "/" dval) (string-prefix? "~" dval))
       (absolute-path (expand-tilde dval)))
      (else (absolute-path (path-join-clean base dval))))))

(define (replace-async-placeholder! d placeholder formatted)
  ;; Locate the exact unmodified placeholder line `: <token>\n` in live document D
  ;; and replace only that exact line via revision-checked view-edit-at!.
  (let* ((rev (document-revision d))
         (txt (document-string d))
         (needle (string-append ": " placeholder "\n"))
         (ci (string-contains txt needle)))
    (when (and ci (or (= ci 0) (char=? (string-ref txt (- ci 1)) #\newline)))
      (let* ((b-from (bytevector-length (string->utf8 (substring txt 0 ci))))
             (b-to (+ b-from (bytevector-length (string->utf8 needle))))
             (v (make-view d "user")))
        (view-edit-at! v rev (list (list b-from b-to formatted)) "new")))))

(define-command (org-babel-execute-src-block s n)
  "Execute the Org source block at point and insert or update `#+RESULTS:`."
  (let* ((orig-v (session-view s))
         (d (view-document orig-v))
         (orig-rev (document-revision d))
         (pair (org-doc-outline d))
         (text (car pair))
         (ol (cdr pair))
         (blk (org-babel-block-at text ol (point s))))
    (cond
      ((not blk)
       (message! s "No Org source block at point"))
      ((org-babel-eval-disabled? blk)
       (message! s "Evaluation disabled by :eval no"))
      (else
       (let* ((lang (string-downcase (org-src-block-lang blk)))
              (params (org-src-block-params blk))
              (body (org-babel-expand-body text ol blk)))
         (cond
           ((or (string=? lang "scheme") (string=? lang "techne"))
            (guard (e (#t (message! s (string-append "Babel error: " (error-object-message e)))))
              (let* ((out (org-babel-eval-scheme body params))
                     (edits (org-babel-results-edits text blk out #:outline ol)))
                (if (pair? edits)
                    (begin (check-editable! orig-v)
                           (view-edit-at! orig-v orig-rev edits "new")
                           (message! s "Babel: updated #+RESULTS:"))
                    (message! s (string-append "Babel => " (string-trim out)))))))
           (else
            (guard (e (#t (message! s (string-append "Babel error: " (error-object-message e)))))
              (let ((cmd (org-babel-command-for blk #:body body)))
                (if (not cmd)
                    (message! s (string-append "Unsupported Babel language: " lang))
                    (let ((dir (resolve-babel-dir s blk))
                          (prog (car cmd))
                          (args (cadr cmd))
                          (stdin-text (caddr cmd)))
                      (if (org-babel-async? blk)
                          ;; Asynchronous execution: insert plain fixed-width placeholder line,
                          ;; then replace only that exact line on completion (never nesting wrappers).
                          (let* ((token (string-append "<async:org-babel-" (org-new-id) ">"))
                                 (plain-blk (make-org-src-block (org-src-block-name blk)
                                                                (org-src-block-lang blk)
                                                                '((":results" . "replace"))
                                                                body
                                                                (org-src-block-start blk)
                                                                (org-src-block-end blk)
                                                                (org-src-block-body-start blk)
                                                                (org-src-block-body-end blk)
                                                                (org-src-block-heading-index blk)))
                                 (edits (org-babel-results-edits text plain-blk token #:outline ol)))
                            (when (pair? edits)
                              (check-editable! orig-v)
                              (view-edit-at! orig-v orig-rev edits "new"))
                            (message! s (string-append "Babel: running " lang " asynchronously..."))
                            (spawn
                             (lambda ()
                               (guard (e (#t (replace-async-placeholder!
                                              d token
                                              (string-append ": ERROR: " (error-object-message e) "\n"))
                                             (message! s (string-append "Babel async failed: " (error-object-message e)))))
                                 (let-values (((_status out) (run-external-babel-sync dir prog args stdin-text)))
                                   (let ((formatted (or (format-babel-result out lang params) "")))
                                     (replace-async-placeholder! d token formatted)
                                     (message! s (string-append "Babel async finished (" lang ")"))))))))
                          ;; Synchronous execution: apply edits against originating view & revision.
                          (let-values (((_status out) (run-external-babel-sync dir prog args stdin-text)))
                            (let ((edits (org-babel-results-edits text blk out #:outline ol)))
                              (if (pair? edits)
                                  (begin (check-editable! orig-v)
                                         (view-edit-at! orig-v orig-rev edits "new")
                                         (message! s "Babel: updated #+RESULTS:"))
                                  (message! s (string-append "Babel => " (string-trim out))))))))))))))))))

(define-command (org-babel-tangle s n)
  "Tangle all `:tangle` source blocks in the current Org buffer to disk."
  (let* ((d (doc s))
         (pair (org-doc-outline d))
         (src-path (document-path d)))
    (guard (e (#t (message! s (string-append "Tangle failed: " (error-object-message e)))))
      (let ((files (org-babel-tangle-file! (car pair) (cdr pair) #:source-path src-path)))
        (if (null? files)
            (message! s "No :tangle blocks found")
            (message! s (string-append "Tangled " (number->string (length files))
                                       " file(s): " (string-join files ", "))))))))

(define-command (org-babel-remove-result s n)
  "Remove the `#+RESULTS:` block for the Org source block at point."
  (let* ((d (doc s))
         (pair (org-doc-outline d))
         (blk (org-babel-block-at (car pair) (cdr pair) (point s))))
    (if (not blk)
        (message! s "No Org source block at point")
        (let ((edits (org-babel-remove-result-edits (car pair) blk #:outline (cdr pair))))
          (if (null? edits)
              (message! s "No #+RESULTS: block to remove")
              (begin
                (org-apply-session-edits! s edits)
                (message! s "Removed #+RESULTS:")))))))

(define-command (org-ctrl-c-ctrl-c s n)
  "Perform a context-sensitive Org action at point.
Executes a source block, aligns a table, or toggles checkboxes and
updates headline statistics cookies."
  (let* ((d (doc s))
         (pair (org-doc-outline d))
         (text (car pair))
         (ol (cdr pair))
         (p (point s))
         (blk (org-babel-block-at text ol p)))
    (cond
      (blk (org-babel-execute-src-block s n))
      ((let ((edits (org-edit-align-table text ol p)))
         (and (pair? edits) edits))
       => (lambda (edits)
            (org-apply-session-edits! s edits)
            (message! s "Aligned table")))
      (else
       (let-values (((text ol h) (org-current-heading s)))
         (let ((edits (org-edit-toggle-checkbox text ol h p)))
           (if (pair? edits)
               (org-apply-session-edits! s edits)
               (message! s "Nothing to do here"))))))))

;; ----- Link following & meeting link opener -----

(define (meeting-url? url)
  (or (string-prefix? "https://meet.google.com/" url)
      (string-prefix? "https://teams.microsoft.com/meet/" url)
      (string-prefix? "https://teams.microsoft.com/l/meetup-join/" url)))

(define (clean-url url)
  (let loop ((e (string-length url)))
    (if (and (> e 0) (memv (string-ref url (- e 1)) '(#\] #\) #\. #\, #\; #\:)))
        (loop (- e 1))
        (substring url 0 e))))

(define (org-meeting-links-in text)
  (let ((inlines (org-parse-inline text 0 (org-utf8-length text))))
    (delete-duplicates
     (filter-map (lambda (in)
                   (and (eq? (org-inline-kind in) 'link)
                        (let ((u (clean-url (car (org-inline-data in)))))
                          (and (meeting-url? u) u))))
                 inlines))))

(define (open-external-url! s url)
  (note-effect! 'external)
  (spawn (lambda ()
           (guard (e (#t (message! s (string-append "Could not open " url))))
             (call-with-process "xdg-open" (list url) process-wait
                                #:stdin 'null #:stdout 'null #:stderr 'null))))
  (message! s (string-append "Opening " url)))

(define (jump-to-heading-title! s ol text title)
  (let* ((hs (org-outline-headings ol))
         (n (vector-length hs)))
    (let loop ((i 0))
      (cond
        ((>= i n) (message! s (string-append "Heading not found: " title)))
        ((string-ci=? (org-heading-title text (vector-ref hs i)) title)
         (move! s (lambda (_) (org-heading-start (vector-ref hs i)))))
        (else (loop (+ i 1)))))))

(define-command (org-open-at-point s n)
  "Open the Org link at point (file:, http(s):, heading target *Title)."
  (let* ((d (doc s))
         (link (org-link-at-point d (point s))))
    (if (not link)
        (message! s "No Org link at point")
        (let ((raw (car (org-inline-data link))))
          (cond
            ((or (string-prefix? "https://" raw) (string-prefix? "http://" raw))
             (open-external-url! s raw))
            ((string-prefix? "id:" raw)
             (let* ((nid (string-trim (substring raw 3)))
                    (idx (org-mem-current-index))
                    (nd (org-mem-find-node idx nid)))
               (if (not nd)
                   (message! s (string-append "Org node not found: " nid))
                   (org-visit-node! s nd))))
            ((string-prefix? "file:" raw)
             (let* ((path (substring raw 5))
                    (base (default-directory s))
                    (full (if (or (string-prefix? "/" path) (string-prefix? "~" path) (not base))
                              path
                              (path-join-clean base path))))
               (file-visit! s full)))
            ((string-prefix? "*" raw)
             (let ((pair (org-doc-outline d)))
               (jump-to-heading-title! s (cdr pair) (car pair) (substring raw 1))))
            (else
             (message! s (string-append "Unsupported link: " raw))))))))

(define-command (org-open-meeting-link s n)
  "Open a Google Meet or Microsoft Teams link in the current Org entry."
  (if (not (eq? (buffer-mode (session-buffer s)) 'org-mode))
      (message! s "Not in an Org buffer")
      (let* ((pair (org-doc-outline (doc s)))
             (text (car pair))
             (ol (cdr pair))
             (h (org-heading-at ol (point s)))
             (slice (if h (org-slice-bytes text (org-heading-start h) (org-heading-subtree-end h)) text))
             (links (org-meeting-links-in slice)))
        (if (null? links)
            (message! s "No Google Meet or Microsoft Teams link found here")
            (open-external-url! s (car links))))))

;; ----- Habit logging & interactive Agenda lens -----

(define-command (org-habit-log s n)
  "Log one habit completion into `:LOGBOOK:` for the current Org heading."
  (let-values (((text ol h) (org-current-heading s)))
    (let* ((hspec (org-heading-habit-spec text ol h))
           (unit (and hspec (org-habit-spec-unit hspec))))
      (if (and unit (not (eq? unit 'times)))
          (let* ((orig-v (session-view s))
                 (orig-d (view-document orig-v))
                 (orig-rev (document-revision orig-d))
                 (title (org-heading-title text h))
                 (u-str (symbol->string unit)))
            (completing-read
             s (string-append "Log " title " (" u-str "): ") '()
             #:require-match #f
             #:accept
             (lambda (s cand)
               (let ((amt (string->number (string-trim (cand->string cand)))))
                 (cond
                   ((or (not amt) (<= amt 0))
                    (message! s (string-append "Expected a positive number of " u-str)))
                   ((not (= (document-revision orig-d) orig-rev))
                    (message! s "Buffer changed; habit log cancelled"))
                   (else
                    (let ((edits (org-edit-log-habit text ol h #:now (local-now) #:amount amt #:unit unit)))
                      (check-editable! orig-v)
                      (view-edit-at! orig-v orig-rev edits "new")
                      (message! s (string-append "Logged habit: " title)))))))))
          (let ((edits (org-edit-log-habit text ol h #:now (local-now))))
            (org-apply-session-edits! s edits)
            (message! s (string-append "Logged habit: " (org-heading-title text h))))))))

(define (expand-tilde path)
  (cond
    ((string=? path "~") (or (get-environment-variable "HOME") path))
    ((string-prefix? "~/" path)
     (let ((home (get-environment-variable "HOME")))
       (if home (string-append home "/" (substring path 2)) path)))
    (else path)))

(define (org-resolved-directory)
  (absolute-path (expand-tilde (option #f 'org-directory))))

(define (org-resolved-daily-directory)
  (absolute-path (expand-tilde (option #f 'org-daily-directory))))

(define (org-resolved-watch-dirs)
  (let ((dirs (option #f 'org-mem-watch-dirs)))
    (map (lambda (p) (absolute-path (expand-tilde p)))
         (if (pair? dirs) dirs (list (option #f 'org-directory))))))

(define (path-in-dir? path dir)
  (let* ((ap (absolute-path (expand-tilde path)))
         (ad (absolute-path (expand-tilde dir)))
         (prefix (if (string-suffix? "/" ad) ad (string-append ad "/"))))
    (string-prefix? prefix ap)))

(define %org-mem #f)

(define (live-org-buffer-entries)
  (filter-map
   (lambda (b)
     (and (eq? (buffer-mode b) 'org-mode)
          (let* ((d (buffer-document b))
                 (p (and (document-path d) (absolute-path (document-path d))))
                 (pair (org-doc-outline d)))
            (and p (cons p pair)))))
   (buffer-list)))

(define (org-mem-current-index)
  "Return the synced global `org-mem-index` covering watched dirs and open buffers."
  (let ((roots (org-resolved-watch-dirs)))
    (when (not %org-mem)
      (set! %org-mem (make-org-mem roots
                                   #:todo-keywords (option #f 'org-todo-keywords)
                                   #:done-keywords (option #f 'org-done-keywords))))
    (unless (equal? (org-mem-index-roots %org-mem) roots)
      (set-org-mem-index-roots! %org-mem roots))
    (org-mem-sync! %org-mem #:live-buffers (live-org-buffer-entries))))

(define (agenda-collect-sources)
  ;; Collect open org-mode buffers plus dynamic #+filetags: todo files from org-mem,
  ;; default agenda files under org-directory, and explicit org-agenda-files.
  ;; Each item is (source buffer snapshot-revision-or-#f).
  (let* ((open-org (filter (lambda (b) (eq? (buffer-mode b) 'org-mode)) (buffer-list)))
         (seen-paths (make-hash-table))
         (from-buffers
          (map (lambda (b)
                 (let* ((d (buffer-document b))
                        (p (or (document-path d) (buffer-name b)))
                        (pair (org-doc-outline d)))
                   (when (document-path d) (hash-table-set! seen-paths (absolute-path (document-path d)) #t))
                   (list (make-org-source p (car pair) (cdr pair)) b (document-revision d))))
               open-org))
         (idx (org-mem-current-index))
         (org-dir (org-resolved-directory))
         (todo-files (org-mem-todo-files idx))
         (def-files (map (lambda (f)
                           (if (or (string-prefix? "/" f) (string-prefix? "~" f))
                               (absolute-path (expand-tilde f))
                               (path-join-clean org-dir f)))
                         (option #f 'org-default-agenda-files)))
         (cfg-files (map (lambda (f) (absolute-path (expand-tilde f))) (option #f 'org-agenda-files)))
         (disk-candidates (append todo-files def-files cfg-files))
         (todo (option #f 'org-todo-keywords))
         (done (option #f 'org-done-keywords))
         (from-disk
          (filter-map
           (lambda (abs)
             (and (not (hash-table-ref/default seen-paths abs #f))
                  (begin
                    (hash-table-set! seen-paths abs #t)
                    (or (let ((cached (org-mem-file-source idx abs)))
                          (and cached (list cached #f #f)))
                        (and (path-exists? abs)
                             (guard (e (#t #f))
                               (let* ((txt (file->string abs))
                                      (ol (org-parse-outline txt #:todo-keywords todo #:done-keywords done)))
                                 (list (make-org-source abs txt ol) #f #f))))))))
           disk-candidates)))
    (append from-buffers from-disk)))

(define (habit-status-face hev)
  (case (org-habit-eval-status hev)
    ((satisfied on-track cooldown) 'org.agenda.habit.ok)
    ((at-risk) 'org.agenda.habit.risk)
    ((must-today) 'org.agenda.habit.must)
    (else 'org.agenda.habit.missed)))

(define (pad-col s width)
  (let ((len (string-length s)))
    (if (>= len width) s (string-append s (make-string (- width len) #\space)))))

(define (trunc-pad-col s width)
  ;; Fixed-width column: truncates with "…" when longer than WIDTH, pads with spaces when shorter.
  (let ((len (string-length s)))
    (cond
      ((<= len width) (string-append s (make-string (- width len) #\space)))
      ((<= width 1) (substring s 0 width))
      (else (string-append (substring s 0 (- width 1)) "…")))))

(define (todo-badge-face ol kw)
  (cond
    ((not kw) 'org.keyword)
    ((member kw (org-outline-done-keywords ol)) 'org.agenda.done.badge)
    ((string-ci=? kw "REVIEW") 'org.agenda.review.badge)
    (else 'org.agenda.todo.badge)))

(define (prio-badge-face prio)
  (case prio
    ((#\A) 'org.agenda.prio.a)
    ((#\B) 'org.agenda.prio.b)
    (else 'org.agenda.prio.c)))

(define (habit-bar-face hev)
  (case (org-habit-eval-status hev)
    ((satisfied on-track cooldown) 'org.agenda.habit.bar.ok)
    ((at-risk) 'org.agenda.habit.bar.risk)
    ((must-today) 'org.agenda.habit.bar.must)
    (else 'org.agenda.habit.bar.miss)))

(define (group-hidden-tags group-name)
  ;; Hide tags already implied by the group header so rows stay crisp.
  (cond
    ((string=? group-name "Habit: ora") '("prayer" "religie"))
    ((string=? group-name "Habit: corpus") '("lichaam"))
    (else '())))

(define (format-habit-right-bar hev)
  ;; Compact fixed-width visual bar (22 chars) with progress + status glyph.
  (let* ((prog (org-habit-eval-progress hev))
         (targ (max 1 (org-habit-eval-target hev)))
         (status (org-habit-eval-status hev))
         (filled (min 14 (quotient (* prog 14) targ)))
         (bar (string-append (make-string filled #\█)
                             (make-string (- 14 filled) #\░)))
         (glyph (case status
                  ((satisfied) " ✓ ")
                  ((on-track cooldown) " · ")
                  ((at-risk) " ! ")
                  ((must-today) " ! ")
                  (else " ✕ ")))
         (lbl (string-append " " (number->string prog) "/" (number->string targ) " " bar glyph)))
    (list (list lbl (habit-bar-face hev)))))

(define (format-agenda-entry-row group-name entry meta-map)
  (let* ((src (org-entry-source entry))
         (h (org-entry-heading entry))
         (ol (org-source-outline src))
         (text (org-source-text src))
         (title (org-heading-title text h))
         (hev (org-entry-habit entry))
         (reason (org-entry-reason entry))
         (past? (memq reason '(scheduled-past deadline-past)))
         (t (org-entry-time entry))
         (et (org-entry-end-time entry))
         (ef (org-entry-effort entry))
         (kw (org-heading-keyword h))
         (prio (org-heading-priority h))
         (hidden-tags (append (map string-downcase (option #f 'org-agenda-hide-tags))
                              (group-hidden-tags group-name)))
         (tags (filter (lambda (tg) (not (member (string-downcase tg) hidden-tags)))
                       (org-entry-tags entry)))
         (meta (hash-table-ref/default meta-map src '(#f #f)))
         (buf (car meta))
         (snap-rev (cadr meta))
         (cat-str (string-append "  " (trunc-pad-col (org-entry-category entry) 16) "  "))
         (cat-face (if past? 'org.agenda.cat.past 'org.agenda.cat))
         (time-str (cond
                     (hev (make-string 13 #\space))
                     ((and t et) (pad-col (string-append (format-time t) "-" (format-time et)) 13))
                     (t (pad-col (format-time t) 13))
                     (else (trunc-pad-col (org-entry-label entry) 13))))
         (time-face (if t 'org.timestamp.active 'org.agenda.cat.past))
         (eff-str (if ef (pad-col (org-format-duration ef) 6) (make-string 6 #\space)))
         (kw-runs (if kw
                      (list (list (string-append " " kw " ") (todo-badge-face ol kw))
                            (make-string (max 1 (- 6 (string-length kw))) #\space))
                      (list (make-string 8 #\space))))
         (prio-runs (if prio
                        (list (list (string-append " #" (string prio) " ") (prio-badge-face prio)) " ")
                        (list (make-string 5 #\space))))
         (title-col 40)
         (title-disp (trunc-pad-col title title-col))
         (title-face (if past? 'org.agenda.cat.past 'default))
         (right-runs
          (cond
            (hev (cons "  " (format-habit-right-bar hev)))
            ((pair? tags)
             (cons "  "
                   (append-map (lambda (tg)
                                 (list (list (string-append " " tg " ") 'org.agenda.tag.badge) " "))
                               tags)))
            (else '())))
         (runs
          (if (string=? group-name "Schedule")
              ;; Time-grid schedule rows: Category | Time | Title | Tags
              (append
               (list (list cat-str 'org.agenda.cat)
                     (list (string-append time-str "  ") 'org.timestamp.active)
                     (list title-disp 'default))
               right-runs)
              ;; Aligned task & habit rows: Category | Time/Sched | Effort | TODO | #P | Title | Right bar/tags
              (append
               (list (list cat-str cat-face)
                     (list (string-append time-str " ") time-face)
                     (list (string-append eff-str " ") 'org.agenda.effort))
               kw-runs
               prio-runs
               (list (list title-disp title-face))
               right-runs))))
    (row #:key (string-append group-name "|" (org-entry-key entry))
         #:target (target 'org-entry (list entry buf snap-rev))
         runs)))

(define (build-time-grid-rows group-name entries meta-map is-today? now-min)
  ;; Interleave 08:00..20:00 time-grid rows and a `now` line with timed schedule events.
  (let* ((ev-times (map (lambda (e) (or (org-entry-time e) -1)) entries))
         (grid-hours (filter (lambda (m) (not (member m ev-times))) '(480 600 720 840 960 1080 1200)))
         (grid-items (map (lambda (m) (list m 'grid #f)) grid-hours))
         (now-item (if is-today? (list (list now-min 'now #f)) '()))
         (ev-items (map (lambda (e) (list (or (org-entry-time e) 0) 'event e)) entries))
         (merged (sort (append grid-items now-item ev-items)
                       (lambda (a b)
                         (if (= (car a) (car b))
                             (let ((rank (lambda (k) (case k ((event) 0) ((now) 1) (else 2)))))
                               (< (rank (cadr a)) (rank (cadr b))))
                             (< (car a) (car b)))))))
    (map (lambda (item)
           (let ((m (car item))
                 (kind (cadr item))
                 (e (caddr item)))
             (case kind
               ((event) (format-agenda-entry-row group-name e meta-map))
               ((now)
                (row #:key (string-append group-name "|grid:now")
                     (list (list (string-append (make-string 20 #\space)
                                                (format-time m) "......  now ────────────────────────────────────────")
                                 'org.agenda.now))))
               (else
                (row #:key (string-append group-name "|grid:" (number->string m))
                     (list (list (string-append (make-string 20 #\space)
                                                (format-time m) "......  ────────────────────")
                                 'org.agenda.grid)))))))
         merged)))

(define (build-agenda-rows view-spec src+meta day now-min workday day-start-hour actual-today)
  (let* ((meta-map (make-hash-table))
         (sources (map (lambda (m) (hash-table-set! meta-map (car m) (cdr m)) (car m)) src+meta))
         (res (org-run-view view-spec sources
                            #:today day #:now-minutes now-min
                            #:workday-minutes workday #:day-start-hour day-start-hour))
         (budget (org-view-result-budget res))
         (booked (cdr (assq 'booked budget)))
         (free (cdr (assq 'free budget)))
         (planned (cdr (assq 'planned budget)))
         (unk (cdr (assq 'unknown budget)))
         (over? (> planned free))
         (span (org-view-spec-span view-spec))
         (is-today? (= day actual-today))
         (budget-text
          (string-append "  Booked " (org-format-duration booked)
                         " · Free " (org-format-duration free)
                         " · Planned " (org-format-duration planned)
                         (if (> unk 0) (string-append " (+" (number->string unk) " unestimated)") "")
                         (if over? (string-append " · OVER by " (org-format-duration (- planned free))) "")))
         (day-str (if (> span 1)
                      (string-append (date->string day) " .. " (date->string (+ day (- span 1)))
                                     " (" (number->string span) " days)")
                      (string-append (weekday-name day) " " (date->string day))))
         (rows (list (row #:key "pad:top" "")
                     (row #:key "header:day" (list (list (string-append "  " (org-view-spec-name view-spec) " — " day-str) 'org.agenda.header)))
                     (row #:key "header:budget" (list (list budget-text (if over? 'org.agenda.budget.over 'org.agenda.budget))))
                     (row #:key "sep:top" ""))))
    (for-each
     (lambda (g)
       (let ((es (org-group-result-entries g))
             (g-name (org-group-result-name g))
             (render (org-group-result-render g)))
         (when (or (pair? es) (and (eq? render 'time-grid) is-today?))
           (let* ((ef (org-group-result-effort g))
                  (u (org-group-result-unknown-effort g))
                  (g-hdr (string-append "  " g-name
                                        (if (> ef 0) (string-append "  [" (org-format-duration ef) "]") "")
                                        (if (and (> ef 0) (> u 0)) (string-append " (+" (number->string u) "?)") "")))
                  (item-rows (if (eq? render 'time-grid)
                                 (build-time-grid-rows g-name es meta-map is-today? now-min)
                                 (map (lambda (e) (format-agenda-entry-row g-name e meta-map)) es))))
             (set! rows (append rows
                                (list (row #:key (string-append "rule:" g-name)
                                           (list (list "  ────────────────────────────────────────────────────────────────────────────────────────" 'org.agenda.rule)))
                                      (row #:key (string-append "group:" g-name)
                                           (list (list g-hdr 'org.agenda.group))))
                                item-rows
                                (list (row #:key (string-append "sep:" g-name) ""))))))))
     (org-view-result-groups res))
    (values rows res)))

(define (build-search-rows pred raw src+meta)
  (let* ((meta-map (make-hash-table))
         (sources (map (lambda (m) (hash-table-set! meta-map (car m) (cdr m)) (car m)) src+meta))
         (hits (org-search-headings pred sources #:include-done #t))
         (rows (append
                (list (row #:key "pad:s" "")
                      (row #:key "header:s"
                           (list (list (string-append "  Org Search: " raw " (" (number->string (length hits)) " headings)")
                                       'org.agenda.header)))
                      (row #:key "sep:s" ""))
                (if (null? hits)
                    (list (row #:key "empty:s" (list (list "  (no matching headings)" 'org.keyword))))
                    (map (lambda (e) (format-agenda-entry-row "Search" e meta-map)) hits)))))
    (values rows hits)))

(define (agenda-refresh-buffer! b #:today [today #f] #:now-minutes [now-min #f])
  (let* ((st (buffer-state b))
         (search-pred (hash-table-ref/default st 'search-pred #f))
         (now-pair (local-now))
         (actual-today (car now-pair))
         (day (or today (hash-table-ref/default st 'today #f) actual-today))
         (nmin (or now-min (hash-table-ref/default st 'now-min #f) (cdr now-pair)))
         (src+bufs (agenda-collect-sources)))
    (if search-pred
        (let ((raw (hash-table-ref/default st 'search-raw "")))
          (let-values (((rows hits) (build-search-rows search-pred raw src+bufs)))
            (let ((table (present! (buffer-document b) rows)))
              (hash-table-set! st 'table table)
              (hash-table-set! st 'last-day day)
              (hash-table-set! st 'last-now-min nmin)
              hits)))
        (let* ((view-spec (hash-table-ref st 'view-spec))
               (workday (option b 'org-workday-minutes))
               (dsh (option b 'org-day-start-hour)))
          (let-values (((rows res) (build-agenda-rows view-spec src+bufs day nmin workday dsh actual-today)))
            (let ((table (present! (buffer-document b) rows)))
              (hash-table-set! st 'table table)
              (hash-table-set! st 'result res)
              (hash-table-set! st 'last-day day)
              (hash-table-set! st 'last-now-min nmin)
              res))))))

(define (agenda-entry-at s)
  (let* ((b (current-buffer s))
         (st (and b (buffer-state b)))
         (table (and (hash-table? st) (hash-table-ref/default st 'table #f)))
         (r (and table (row-at (buffer-document b) table (point s))))
         (t (and r (row-target r))))
    (and t (eq? (target-kind t) 'org-entry) (target-value t))))

(define (find-open-buffer-by-path path)
  (and (string? path)
       (let ((abs (absolute-path path)))
         (find (lambda (b)
                 (let ((p (document-path (buffer-document b))))
                   (and p (string=? (absolute-path p) abs))))
               (buffer-list)))))

(define (find-unique-heading-by-id text ol eid)
  ;; Return the unique heading with :ID: = EID in OL, or #f if missing or duplicated.
  (let* ((hs (org-outline-headings ol))
         (n (vector-length hs)))
    (let loop ((i 0) (found #f))
      (if (>= i n)
          found
          (let* ((cand (vector-ref hs i))
                 (cid (org-heading-property text ol cand "ID")))
            (if (and cid (string=? cid eid))
                (if found #f (loop (+ i 1) cand))
                (loop (+ i 1) found)))))))

(define (resolve-heading-in-outline entry text ol same-snapshot?)
  ;; Resolve ENTRY's heading in OL safely:
  ;; 1) When snapshot is unchanged, exact index is guaranteed (even if IDs are duplicated).
  ;; 2) Otherwise require :ID: to be UNIQUE in BOTH the original snapshot and the live outline.
  ;; 3) Otherwise return #f so callers refuse stale/ambiguous edits.
  (let* ((h (org-entry-heading entry))
         (src (org-entry-source entry))
         (hs (org-outline-headings ol))
         (n (vector-length hs)))
    (if same-snapshot?
        (let ((idx (org-heading-index h)))
          (and (< idx n) (vector-ref hs idx)))
        (let* ((src-txt (org-source-text src))
               (src-ol (org-source-outline src))
               (eid (org-heading-property src-txt src-ol h "ID")))
          (and eid (string? eid) (> (string-length eid) 0)
               (find-unique-heading-by-id src-txt src-ol eid)
               (find-unique-heading-by-id text ol eid))))))

(define (resolve-agenda-target entry-info)
  ;; Returns (values doc opened-now? text ol heading-or-#f).
  (let* ((entry (car entry-info))
         (orig-buf (cadr entry-info))
         (snap-rev (caddr entry-info))
         (src (org-entry-source entry))
         (live-buf (or orig-buf (find-open-buffer-by-path (org-source-path src)))))
    (if live-buf
        (let* ((d (buffer-document live-buf))
               (pair (org-doc-outline d))
               (same? (if orig-buf
                          (equal? snap-rev (document-revision d))
                          (string=? (car pair) (org-source-text src))))
               (h2 (resolve-heading-in-outline entry (car pair) (cdr pair) same?)))
          (values d #f (car pair) (cdr pair) h2))
        (let* ((d (file-document (absolute-path (org-source-path src))))
               (pair (org-doc-outline d))
               (same? (string=? (car pair) (org-source-text src)))
               (h2 (resolve-heading-in-outline entry (car pair) (cdr pair) same?)))
          (values d #t (car pair) (cdr pair) h2)))))

(define (mutate-entry-source! s entry-info make-edits)
  ;; Safely mutate ENTRY-INFO's source heading; returns #t on success, #f if stale or refused.
  (let-values (((d opened-now? text ol h2) (resolve-agenda-target entry-info)))
    (if (not h2)
        (begin
          (message! s "Agenda item is stale; press g to refresh")
          #f)
        (let* ((v (make-view d "user"))
               (base-rev (document-revision d))
               (edits (make-edits text ol h2)))
          (when (pair? edits)
            (check-editable! v)
            (view-edit-at! v base-rev edits "new")
            (when opened-now?
              ;; Keep #+filetags: todo synced on detached agenda saves too.
              (when (and (option #f 'org-auto-todo-filetag)
                         (document-path d)
                         (path-in-dir? (document-path d) (org-resolved-directory)))
                (let* ((p2 (org-doc-outline d))
                       (ft-edits (org-edit-update-todo-filetag (car p2) (cdr p2))))
                  (when (pair? ft-edits)
                    (view-edit-at! v (document-revision d) ft-edits "new"))))
              ;; Checked save refuses external (e.g. Syncthing) disk changes instead of clobbering.
              (save-document! d)))
          #t))))

(define (format-proof-lines proof indent)
  (let* ((pred (car proof))
         (ok? (cadr proof))
         (detail (caddr proof))
         (children (cadddr proof))
         (line (string-append indent (if ok? "[OK] " "[NO] ")
                              (call-with-output-string (lambda (p) (write pred p)))
                              "  -- " detail)))
    (cons line (append-map (lambda (c) (format-proof-lines c (string-append indent "  "))) children))))

(define (show-explanation-buffer! s title exp)
  (let* ((lines (list (string-append "Explanation: " title)
                      (string-append "Status: " (symbol->string (cdr (assq 'status exp))))
                      ""))
         (reason (assq 'reason exp))
         (occs (cdr (assq 'occurrences exp)))
         (lines (if reason (append lines (list (cdr reason))) lines))
         (lines
          (append lines
                  (append-map
                   (lambda (o)
                     (let* ((e (cdr (assq 'entry o)))
                            (outcome (cdr (assq 'outcome o)))
                            (hdr (string-append "Occurrence (" (symbol->string (org-entry-reason e))
                                                ", day " (date->string (org-entry-day e)) "): "
                                                (symbol->string outcome))))
                       (append
                        (list hdr)
                        (cond
                          ((eq? outcome 'matched)
                           (append (list (string-append "  Claimed by group: " (cdr (assq 'group o))))
                                   (format-proof-lines (cdr (assq 'proof o)) "  ")))
                          ((eq? outcome 'discarded)
                           (append (list "  Discarded before grouping:")
                                   (format-proof-lines (cdr (assq 'discard-proof o)) "  ")))
                          (else
                           (append-map (lambda (tr)
                                         (cons (string-append "  Group " (car tr) " did not match:")
                                               (format-proof-lines (cdr tr) "    ")))
                                       (cdr (assq 'tried o)))))
                        (let ((hev (org-entry-habit e)))
                          (if (not hev)
                              '()
                              (append
                               (list ""
                                     (string-append "  Habit: " (org-habit-eval-summary hev)
                                                    " (streak " (number->string (org-habit-eval-streak hev))
                                                    ", attainment " (number->string (car (org-habit-eval-attainment hev)))
                                                    "/" (number->string (cdr (org-habit-eval-attainment hev))) ")")
                                     (string-append "  Credited events in window: " (number->string (length (org-habit-eval-credited hev)))))
                               (map (lambda (ex)
                                      (string-append "  Excluded event on " (date->string (org-habit-event-day (car ex)))
                                                     ": " (cdr ex)))
                                    (org-habit-eval-excluded hev)))))
                        (list ""))))
                   occs)))
         (doc (make-document (string-append (string-join lines "\n") "\n")))
         (buf (make-generated-buffer! "*Org Explain*" doc 'special-mode)))
    (pop-to-buffer! s buf)))

(define-command (org-agenda-refresh s n)
  "Refresh the `*Org Agenda*` lens from open buffers and disk."
  (let ((b (current-buffer s)))
    (unless (eq? (buffer-mode b) 'org-agenda-mode) (error "Not in Org Agenda"))
    (agenda-refresh-buffer! b)
    (message! s "Agenda refreshed")))

(define-command (org-agenda-goto s n)
  "Jump to the source Org heading of the agenda row at point."
  (let ((ep (agenda-entry-at s)))
    (if (not ep)
        (message! s "No agenda item on this line")
        (let-values (((d _opened text ol h2) (resolve-agenda-target ep)))
          (let* ((entry (car ep))
                 (pos (if h2 (org-heading-start h2) (org-heading-start (org-entry-heading entry)))))
            (show-document! s d)
            (move! s (lambda (_) (min pos (document-length (doc s)))))
            (unless h2
              (message! s "Heading moved since agenda refresh; jumped to last offset")))))))

(define-command (org-agenda-log-habit s n)
  "Log one habit completion for the agenda row at point and refresh."
  (let ((ep (agenda-entry-at s))
        (b (current-buffer s)))
    (cond
      ((not ep) (message! s "No agenda item on this line"))
      ((not (org-entry-habit (car ep)))
       (message! s "Not a habit; use t to cycle TODO state"))
      (else
       (when (mutate-entry-source! s ep (lambda (txt ol h) (org-edit-log-habit txt ol h #:now (local-now))))
         (agenda-refresh-buffer! b)
         (message! s "Logged habit completion"))))))

(define-command (org-agenda-later s n)
  "Move `*Org Agenda*` forward by one view span (day or week)."
  (let* ((b (current-buffer s))
         (st (buffer-state b))
         (vs (hash-table-ref st 'view-spec))
         (cur (hash-table-ref/default st 'last-day (car (local-now))))
         (next (+ cur (* (max 1 n) (org-view-spec-span vs)))))
    (hash-table-set! st 'today next)
    (agenda-refresh-buffer! b #:today next)
    (message! s (string-append "Agenda: " (date->string next)))))

(define-command (org-agenda-earlier s n)
  "Move `*Org Agenda*` backward by one view span (day or week)."
  (let* ((b (current-buffer s))
         (st (buffer-state b))
         (vs (hash-table-ref st 'view-spec))
         (cur (hash-table-ref/default st 'last-day (car (local-now))))
         (prev (- cur (* (max 1 n) (org-view-spec-span vs)))))
    (hash-table-set! st 'today prev)
    (agenda-refresh-buffer! b #:today prev)
    (message! s (string-append "Agenda: " (date->string prev)))))

(define-command (org-agenda-goto-today s n)
  "Reset `*Org Agenda*` to today."
  (let* ((b (current-buffer s))
         (st (buffer-state b))
         (now-day (car (local-now))))
    (hash-table-set! st 'today now-day)
    (agenda-refresh-buffer! b #:today now-day)
    (message! s "Agenda: today")))

(define-command (org-agenda-day-view s n)
  "Switch `*Org Agenda*` to a 1-day span."
  (let* ((b (current-buffer s))
         (st (buffer-state b)))
    (hash-table-set! st 'view-spec (org-default-day-view))
    (agenda-refresh-buffer! b)
    (message! s "Agenda: day view")))

(define-command (org-agenda-week-view s n)
  "Switch `*Org Agenda*` to a 7-day week span."
  (let* ((b (current-buffer s))
         (st (buffer-state b))
         (base (org-default-day-view))
         (week-groups
          (append (org-view-spec-groups base)
                  (list (make-org-group-spec "Upcoming scheduled" '(scheduled future))
                        (make-org-group-spec "Upcoming deadlines" '(deadline future)))))
         (week-spec (make-org-view-spec "Week"
                                        week-groups
                                        #:discard (org-view-spec-discard base)
                                        #:span 7)))
    (hash-table-set! st 'view-spec week-spec)
    (agenda-refresh-buffer! b)
    (message! s "Agenda: week view")))

(define-command (org-agenda-priority s n)
  "Cycle priority on the source heading of the agenda row at point."
  (let ((ep (agenda-entry-at s))
        (b (current-buffer s)))
    (if (not ep)
        (message! s "No agenda item on this line")
        (when (mutate-entry-source! s ep
                                    (lambda (txt ol h)
                                      (let* ((cur (org-heading-priority h))
                                             (nxt (case cur ((#\A) #\B) ((#\B) #\C) ((#\C) #f) (else #\A))))
                                        (org-edit-set-priority txt ol h nxt))))
          (agenda-refresh-buffer! b)
          (message! s "Updated priority")))))

(define-command (org-agenda-schedule s n)
  "Schedule the source heading of the agenda row at point."
  (let ((ep (agenda-entry-at s))
        (b (current-buffer s)))
    (if (not ep)
        (message! s "No agenda item on this line")
        (let* ((entry (car ep))
               (today (car (local-now)))
               (init (date->string (or (org-entry-day entry) today))))
          (completing-read s "Schedule (YYYY-MM-DD, +2d, fri): " (list init "+1d" "+1w")
                           #:initial init
                           #:require-match #f
                           #:accept
                           (lambda (s cand)
                             (let ((ts (org-parse-relative-date (cand->string cand) #:today today)))
                               (if (not ts)
                                   (message! s "Invalid date")
                                   (when (mutate-entry-source! s ep
                                                               (lambda (txt ol h)
                                                                 (org-edit-set-planning txt ol h 'scheduled ts)))
                                     (agenda-refresh-buffer! b)
                                     (message! s "Updated SCHEDULED"))))))))))

(define-command (org-agenda-deadline s n)
  "Set DEADLINE on the source heading of the agenda row at point."
  (let ((ep (agenda-entry-at s))
        (b (current-buffer s)))
    (if (not ep)
        (message! s "No agenda item on this line")
        (let* ((entry (car ep))
               (today (car (local-now)))
               (init (date->string (or (org-entry-day entry) today))))
          (completing-read s "Deadline (YYYY-MM-DD, +2d, fri): " (list init "+1d" "+1w")
                           #:initial init
                           #:require-match #f
                           #:accept
                           (lambda (s cand)
                             (let ((ts (org-parse-relative-date (cand->string cand) #:today today)))
                               (if (not ts)
                                   (message! s "Invalid date")
                                   (when (mutate-entry-source! s ep
                                                               (lambda (txt ol h)
                                                                 (org-edit-set-planning txt ol h 'deadline ts)))
                                     (agenda-refresh-buffer! b)
                                     (message! s "Updated DEADLINE"))))))))))

(define-command (org-agenda-habit-history s n)
  "Show detailed habit completion history and statistics for the row at point."
  (let ((ep (agenda-entry-at s)))
    (cond
      ((not ep) (message! s "No agenda item on this line"))
      ((not (org-entry-habit (car ep))) (org-agenda-explain s n))
      (else
       (let* ((entry (car ep))
              (hev (org-entry-habit entry))
              (src (org-entry-source entry))
              (title (org-heading-title (org-source-text src) (org-entry-heading entry)))
              (att (org-habit-eval-attainment hev))
              (lines
               (append
                (list (string-append "Habit History: " title)
                      (string-append "Status: " (org-habit-eval-summary hev))
                      (string-append "Window: " (date->string (org-habit-eval-window-start hev))
                                     " .. " (date->string (org-habit-eval-window-end hev)))
                      (string-append "Streak: " (number->string (org-habit-eval-streak hev))
                                     "  |  Attainment: " (number->string (car att)) "/" (number->string (cdr att)))
                      (string-append "Strip:  " (org-habit-eval-strip hev))
                      ""
                      "Credited completions in current window:")
                (if (null? (org-habit-eval-credited hev))
                    (list "  (none yet)")
                    (map (lambda (ev)
                           (string-append "  ✓ " (date->string (org-habit-event-day ev))
                                          " (" (number->string (org-habit-event-amount ev)) ")"))
                         (org-habit-eval-credited hev)))
                (if (null? (org-habit-eval-excluded hev))
                    '()
                    (cons ""
                          (cons "Excluded events:"
                                (map (lambda (ex)
                                       (string-append "  ✕ " (date->string (org-habit-event-day (car ex)))
                                                      " — " (cdr ex)))
                                     (org-habit-eval-excluded hev)))))))
              (doc (make-document (string-append (string-join lines "\n") "\n")))
              (buf (make-generated-buffer! "*Org Habit History*" doc 'special-mode)))
         (pop-to-buffer! s buf))))))

(define-command (org-search s n)
  "Search all Org headings across watched files using a query or keyword."
  (completing-read
   s "Org query (e.g. (tag \"WORK\") or text): " '("(todo)" "(done?)" "(tag \"WORK\")")
   #:require-match #f
   #:accept
   (lambda (s cand)
     (let ((raw (string-trim (cand->string cand))))
       (unless (string=? raw "")
         (let* ((pred (if (string-prefix? "(" raw)
                          (guard (e (#t `(tag ,raw))) (read (open-input-string raw)))
                          `(tag ,raw)))
                (src+meta (agenda-collect-sources)))
           (let-values (((rows _hits) (build-search-rows pred raw src+meta)))
             (let* ((st (make-hash-table))
                    (d (make-presentation))
                    (table (present! d rows))
                    (buf (make-generated-buffer! "*Org Search*" d 'org-agenda-mode #:state st)))
               (hash-table-set! st 'table table)
               (hash-table-set! st 'search-pred pred)
               (hash-table-set! st 'search-raw raw)
               (hash-table-set! st 'last-day (car (local-now)))
               (hash-table-set! st 'last-now-min (cdr (local-now)))
               (pop-to-buffer! s buf)))))))))

(define-command (org-agenda-todo s n)
  "Cycle TODO state on the source heading of the agenda row at point."
  (let ((ep (agenda-entry-at s))
        (b (current-buffer s)))
    (if (not ep)
        (message! s "No agenda item on this line")
        (let ((log-done (option #f 'org-log-done))
              (drawer (option #f 'org-log-into-drawer)))
          (when (mutate-entry-source! s ep (lambda (txt ol h)
                                             (org-edit-cycle-todo txt ol h #:now (local-now)
                                                                  #:log-done log-done #:log-into-drawer drawer)))
            (agenda-refresh-buffer! b)
            (message! s "Updated TODO state"))))))

(define-command (org-agenda-open-meeting-link s n)
  "Open a Google Meet or Microsoft Teams link in the agenda row's entry."
  (let ((ep (agenda-entry-at s)))
    (if (not ep)
        (message! s "No agenda item on this line")
        (let* ((entry (car ep))
               (src (org-entry-source entry))
               (h (org-entry-heading entry))
               (slice (org-slice-bytes (org-source-text src) (org-heading-start h) (org-heading-subtree-end h)))
               (links (org-meeting-links-in slice)))
          (if (null? links)
              (message! s "No Google Meet or Microsoft Teams link found here")
              (open-external-url! s (car links)))))))

(define-command (org-agenda-explain s n)
  "Explain why the agenda item at point matched and which group claimed it."
  (let* ((ep (agenda-entry-at s))
         (b (current-buffer s))
         (st (and b (buffer-state b))))
    (if (not ep)
        (message! s "No agenda item on this line")
        (let* ((entry (car ep))
               (src (org-entry-source entry))
               (h (org-entry-heading entry))
               (view-spec (hash-table-ref st 'view-spec))
               (day (hash-table-ref st 'last-day))
               (nmin (hash-table-ref st 'last-now-min))
               (exp (org-explain-heading view-spec src h #:today day #:now-minutes nmin
                                         #:day-start-hour (option b 'org-day-start-hour))))
          (show-explanation-buffer! s (org-heading-title (org-source-text src) h) exp)))))

(define-command (org-explain-in-agenda s n)
  "Explain whether the current Org heading matches the agenda."
  (let-values (((text ol h) (org-current-heading s)))
    (let* ((d (doc s))
           (src (make-org-source (or (document-path d) (buffer-name (current-buffer s))) text ol))
           (now-pair (local-now))
           (exp (org-explain-heading (org-default-day-view) src h
                                     #:today (car now-pair) #:now-minutes (cdr now-pair)
                                     #:day-start-hour (option (current-buffer s) 'org-day-start-hour))))
      (show-explanation-buffer! s (org-heading-title text h) exp))))

(define-mode org-agenda-mode
  "Interactive Org agenda lens with budgets, habits and explanations.
\\[org-agenda-goto] jumps to the source heading, \\[org-agenda-log-habit]
logs a habit completion, \\[org-agenda-todo] cycles TODO state,
\\[org-agenda-day-view] / \\[org-agenda-week-view] switch spans,
\\[org-agenda-later] / \\[org-agenda-earlier] step dates,
\\[org-agenda-habit-history] shows habit history, \\[org-agenda-explain]
explains why an item is shown, and \\[org-agenda-refresh] refreshes."
  #:parent 'special-mode
  #:keys '(("RET" org-agenda-goto)
           ("l" org-agenda-log-habit)
           ("t" org-agenda-todo)
           ("s" org-agenda-schedule)
           ("C-c C-d" org-agenda-deadline)
           ("," org-agenda-priority)
           ("d" org-agenda-day-view)
           ("w" org-agenda-week-view)
           ("f" org-agenda-later)
           ("b" org-agenda-earlier)
           ("." org-agenda-goto-today)
           ("j" org-agenda-open-meeting-link)
           ("C-c j" org-agenda-open-meeting-link)
           ("?" org-agenda-explain)
           ("H" org-agenda-habit-history)
           ("g" org-agenda-refresh))
  #:normal '(("RET" org-agenda-goto)
             ("l" org-agenda-log-habit)
             ("t" org-agenda-todo)
             ("s" org-agenda-schedule)
             ("C-c C-d" org-agenda-deadline)
             ("," org-agenda-priority)
             ("d" org-agenda-day-view)
             ("w" org-agenda-week-view)
             ("f" org-agenda-later)
             ("b" org-agenda-earlier)
             ("." org-agenda-goto-today)
             ("j" org-agenda-open-meeting-link)
             ("?" org-agenda-explain)
             ("H" org-agenda-habit-history)
             ("g" org-agenda-refresh)))

(define-command (org-agenda s n)
  "Open the interactive `*Org Agenda*` lens for today."
  (let* ((existing (buffer-named "*Org Agenda*"))
         (b (if (and existing (eq? (buffer-mode existing) 'org-agenda-mode))
                existing
                (let ((st (make-hash-table)))
                  (hash-table-set! st 'view-spec (org-default-day-view))
                  (make-generated-buffer! "*Org Agenda*" (make-presentation) 'org-agenda-mode #:state st)))))
    (agenda-refresh-buffer! b)
    (pop-to-buffer! s b)))

;; ----- Knowledge graph (org-mem / org-node), auto todo-filetag & capture -----

(define (org-visit-node! s nd)
  (let* ((path (org-mem-node-path nd))
         (pos (org-mem-node-pos nd))
         (d (file-document (absolute-path path))))
    (show-document! s d)
    (move! s (lambda (_) (min pos (document-length (doc s)))))))

(define (unique-node-path dir title)
  ;; Allocate a non-existent filename so a same-second collision never drops the new ID.
  (let ((base (org-node-filename dir title)))
    (if (not (path-exists? base))
        base
        (let ((stem (if (string-suffix? ".org" base)
                        (substring base 0 (- (string-length base) 4))
                        base)))
          (let loop ((n 2))
            (let ((cand (string-append stem "-" (number->string n) ".org")))
              (if (not (path-exists? cand)) cand (loop (+ n 1)))))))))

(define (org-create-node! title)
  (let* ((dir (org-resolved-directory))
         (_ (make-directory dir))
         (path (unique-node-path dir title))
         (nid (org-new-id))
         (content (org-new-node-content title #:id nid))
         (d (file-document (absolute-path path)))
         (v (make-view d "user")))
    (view-edit-at! v (document-revision d) (list (list 0 0 content)) "new")
    (save-document! d)
    (let ((pair (org-doc-outline d)))
      (when %org-mem
        (org-mem-update-file! %org-mem (absolute-path path) (car pair) (cdr pair) #:live #f)))
    (values d nid path)))

(define (node-choice-candidates idx)
  ;; Build disambiguated (display-label . org-mem-node) alist including aliases.
  (let* ((raw '())
         (counts (make-hash-table))
         (add! (lambda (lbl nd)
                 (hash-table-set! counts lbl (+ 1 (hash-table-ref/default counts lbl 0)))
                 (set! raw (cons (cons lbl nd) raw)))))
    (for-each
     (lambda (nd)
       (let* ((title (org-mem-node-title nd))
              (opath (org-mem-node-outline-path nd))
              (suffix (if (pair? opath)
                          (string-append "  (" (string-join opath " / ") ")")
                          ""))
              (lbl (string-append title suffix)))
         (add! lbl nd)
         (for-each (lambda (al) (add! (string-append al "  → " title) nd))
                   (org-mem-node-aliases nd))))
     (org-mem-all-nodes idx))
    (let* ((pass1
            (map (lambda (p)
                   (let ((lbl (car p))
                         (nd (cdr p)))
                     (if (> (hash-table-ref/default counts lbl 0) 1)
                         (cons (string-append lbl "  [" (abbreviate-path (org-mem-node-path nd)) "]") nd)
                         p)))
                 (reverse raw)))
           (counts2 (make-hash-table)))
      (for-each (lambda (p)
                  (hash-table-set! counts2 (car p) (+ 1 (hash-table-ref/default counts2 (car p) 0))))
                pass1)
      (map (lambda (p)
             (let ((lbl (car p))
                   (nd (cdr p)))
               (if (> (hash-table-ref/default counts2 lbl 0) 1)
                   (cons (string-append lbl " @" (number->string (org-mem-node-pos nd))) nd)
                   p)))
           pass1))))

(define-command (org-mem-refresh s n)
  "Resync the in-memory `org-mem` index across watched directories."
  (set! %org-mem #f)
  (let* ((idx (org-mem-current-index))
         (st (org-mem-stats idx)))
    (message! s (string-append "org-mem indexed " (number->string (car st))
                               " files, " (number->string (cdr st)) " nodes"))))

(define-command (org-update-todo-tag s n)
  "Sync `todo` in `#+filetags:` with open TODOs in this buffer."
  (let* ((d (doc s))
         (pair (org-doc-outline d))
         (edits (org-edit-update-todo-filetag (car pair) (cdr pair))))
    (if (null? edits)
        (message! s "Filetags already up to date")
        (begin
          (org-apply-session-edits! s edits)
          (message! s "Updated #+filetags: todo")))))

(add-hook! 'before-save 'org-auto-todo-filetag
           (lambda (s d)
             (let ((b (document-buffer d))
                   (p (document-path d)))
               (when (and b p
                          (eq? (buffer-mode b) 'org-mode)
                          (option b 'org-auto-todo-filetag)
                          (path-in-dir? p (org-resolved-directory)))
                 (let* ((pair (org-doc-outline d))
                        (edits (org-edit-update-todo-filetag (car pair) (cdr pair))))
                   (when (pair? edits)
                     (let ((v (make-view d "user")))
                       (view-edit-at! v (document-revision d) edits "new"))))))))

(add-hook! 'document-saved 'org-mem-on-save
           (lambda (s d)
             (let ((b (document-buffer d))
                   (p (document-path d)))
               (when (and b p %org-mem (eq? (buffer-mode b) 'org-mode))
                 (let ((pair (org-doc-outline d)))
                   (org-mem-update-file! %org-mem (absolute-path p) (car pair) (cdr pair) #:live #f))))))

(define (cand->string cand)
  (if (string? cand) cand (candidate-text cand)))

(define-command (org-node-find s n)
  "Find an Org ID-node by title or alias, or create a new timestamped node."
  (let* ((idx (org-mem-current-index))
         (cands (node-choice-candidates idx))
         (labels (map car cands)))
    (completing-read s "Node: " labels
                     #:require-match #f
                     #:accept
                     (lambda (s cand)
                       (let* ((choice (cand->string cand))
                              (trimmed (string-trim choice)))
                         (unless (string=? trimmed "")
                           (let ((hit (assoc choice cands)))
                             (if hit
                                 (org-visit-node! s (cdr hit))
                                 (let-values (((d _nid _path) (org-create-node! trimmed)))
                                   (show-document! s d)
                                   (move! s (lambda (_) (document-length (doc s))))
                                   (message! s (string-append "Created node: " trimmed)))))))))))

(define-command (org-node-insert-link s n)
  "Insert an `[[id:...][...]]` link to an Org node at point."
  (let* ((orig-v (session-view s))
         (orig-d (view-document orig-v))
         (orig-rev (document-revision orig-d))
         (orig-pos (view-pos orig-v))
         (idx (org-mem-current-index))
         (cands (node-choice-candidates idx))
         (labels (map car cands)))
    (completing-read s "Link to node: " labels
                     #:require-match #f
                     #:accept
                     (lambda (s cand)
                       (let* ((choice (cand->string cand))
                              (trimmed (string-trim choice)))
                         (unless (string=? trimmed "")
                           (check-editable! orig-v)
                           (if (not (= (document-revision orig-d) orig-rev))
                               (message! s "Buffer changed; link insertion cancelled")
                               (let* ((hit (assoc choice cands))
                                      (created-path #f)
                                      (id+title
                                       (if hit
                                           (cons (org-mem-node-id (cdr hit))
                                                 (org-mem-node-title (cdr hit)))
                                           (let-values (((_d nid npath) (org-create-node! trimmed)))
                                             (set! created-path npath)
                                             (cons nid trimmed))))
                                      (link-text (string-append "[[id:" (car id+title) "][" (cdr id+title) "]]")))
                                 (guard (e (#t
                                            (when created-path
                                              (guard (_ (#t #f)) (delete-file created-path)))
                                            (raise e)))
                                   (view-edit-at! orig-v orig-rev (list (list orig-pos orig-pos link-text)) "new"))))))))))

(define-command (org-node-visit-random s n)
  "Jump to a random Org ID-node in the `org-mem` index."
  (let* ((idx (org-mem-current-index))
         (all (org-mem-all-nodes idx))
         (len (length all)))
    (if (= len 0)
        (message! s "No Org nodes indexed")
        (let ((nd (list-ref all (modulo (current-milliseconds) len))))
          (org-visit-node! s nd)
          (message! s (string-append "Random node: " (org-mem-node-title nd)))))))

(define (current-node-id-at s)
  (let* ((d (doc s))
         (pair (org-doc-outline d))
         (text (car pair))
         (ol (cdr pair))
         (h (org-heading-at ol (point s)))
         (hid (and h (org-heading-property text ol h "ID"))))
    (or hid (org-file-prop ol "ID"))))

(define (backlink-at s)
  (let* ((b (current-buffer s))
         (st (and b (buffer-state b)))
         (table (and (hash-table? st) (hash-table-ref/default st 'table #f)))
         (r (and table (row-at (buffer-document b) table (point s))))
         (t (and r (row-target r))))
    (and t (eq? (target-kind t) 'org-backlink) (target-value t))))

(define-command (org-backlinks-goto s n)
  "Jump to the source location of the Org backlink at point."
  (let ((lk (backlink-at s)))
    (if (not lk)
        (message! s "No backlink on this line")
        (let ((d (file-document (absolute-path (org-mem-link-source-path lk)))))
          (show-document! s d)
          (move! s (lambda (_) (min (org-mem-link-pos lk) (document-length (doc s)))))))))

(define-mode org-backlinks-mode
  "Interactive lens of backlinks targeting an Org ID-node.
\\[org-backlinks-goto] jumps to the citing node."
  #:parent 'special-mode
  #:keys '(("RET" org-backlinks-goto))
  #:normal '(("RET" org-backlinks-goto)))

(define-command (org-node-backlinks s n)
  "Show all Org backlinks pointing to the current heading or file `:ID:`."
  (if (not (eq? (buffer-mode (session-buffer s)) 'org-mode))
      (message! s "Not in an Org buffer")
      (let ((nid (current-node-id-at s)))
    (if (not nid)
        (message! s "Current heading/file has no :ID: property")
        (let* ((idx (org-mem-current-index))
               (nd (org-mem-find-node idx nid))
               (title (if nd (org-mem-node-title nd) nid))
               (links (org-mem-backlinks idx nid))
               (rows
                (append
                 (list (row #:key "header:bl"
                            (list (list (string-append "Backlinks to " title " (" (number->string (length links)) ")")
                                        'org.agenda.header)))
                       (row #:key "sep:bl" ""))
                 (if (null? links)
                     (list (row #:key "empty:bl" (list (list "  (no backlinks)" 'org.keyword))))
                     (let loop ((ls links) (i 0) (acc '()))
                       (if (null? ls)
                           (reverse acc)
                           (let ((lk (car ls)))
                             (loop (cdr ls) (+ i 1)
                                   (cons (row #:key (string-append "bl:" (number->string i))
                                              #:target (target 'org-backlink lk)
                                              (list (list (string-append "  " (org-mem-link-source-title lk) "  ") 'org.heading.2)
                                                    (list (org-mem-link-context lk) 'org.keyword)))
                                         acc))))))))
               (st (make-hash-table))
               (d (make-presentation))
               (table (present! d rows))
               (buf (make-generated-buffer! "*Org Backlinks*" d 'org-backlinks-mode #:state st)))
          (hash-table-set! st 'table table)
          (pop-to-buffer! s buf))))))

(define (org-mem-grep-hits idx query)
  ;; Return list of org-mem-link records for every line matching QUERY (case-insensitive).
  (let* ((qlow (string-downcase query))
         (files-tbl (org-mem-index-files idx))
         (hits '()))
    (for-each
     (lambda (path)
       (let* ((f (hash-table-ref files-tbl path))
              (txt (org-mem-file-text f))
              (title (org-mem-file-title f))
              (len (string-length txt)))
         (let loop ((pos 0) (byte-pos 0))
           (when (< pos len)
             (let* ((nl (let scan ((i pos))
                          (cond ((>= i len) len)
                                ((char=? (string-ref txt i) #\newline) i)
                                (else (scan (+ i 1))))))
                    (line (substring txt pos nl))
                    (next-pos (if (< nl len) (+ nl 1) len))
                    (line-bytes (bytevector-length (string->utf8 (substring txt pos next-pos)))))
               (when (string-contains (string-downcase line) qlow)
                 (set! hits (cons (make-org-mem-link path #f title query byte-pos (string-trim line))
                                  hits)))
               (loop next-pos (+ byte-pos line-bytes)))))))
     (sort (hash-table-keys files-tbl) string<?))
    (reverse hits)))

(define-command (org-node-grep s n)
  "Search text across all Org files indexed by `org-mem`."
  (completing-read
   s "Grep Org nodes: " '()
   #:require-match #f
   #:accept
   (lambda (s cand)
     (let ((q (string-trim (cand->string cand))))
       (unless (string=? q "")
         (let* ((idx (org-mem-current-index))
                (hits (org-mem-grep-hits idx q))
                (rows
                 (append
                  (list (row #:key "header:grep"
                             (list (list (string-append "Org Grep: \"" q "\" (" (number->string (length hits)) " matches)")
                                         'org.agenda.header)))
                        (row #:key "sep:grep" ""))
                  (if (null? hits)
                      (list (row #:key "empty:grep" (list (list "  (no matches)" 'org.keyword))))
                      (let loop ((hs hits) (i 0) (acc '()))
                        (if (null? hs)
                            (reverse acc)
                            (let ((lk (car hs)))
                              (loop (cdr hs) (+ i 1)
                                    (cons (row #:key (string-append "grep:" (number->string i))
                                               #:target (target 'org-backlink lk)
                                               (list (list (string-append "  " (org-mem-link-source-title lk) "  ") 'org.heading.2)
                                                     (list (org-mem-link-context lk) 'org.keyword)))
                                          acc))))))))
                (st (make-hash-table))
                (d (make-presentation))
                (table (present! d rows))
                (buf (make-generated-buffer! "*Org Grep*" d 'org-backlinks-mode #:state st)))
           (hash-table-set! st 'table table)
           (pop-to-buffer! s buf)))))))

(define %capture-draft-prefix "# Org Capture (C-c C-c to commit, C-c C-k to cancel)\n")

(define (strip-capture-header txt)
  (if (string-prefix? "# Org Capture " txt)
      (let ((nl (string-index txt #\newline)))
        (if nl (substring txt (+ nl 1)) ""))
      txt))

(define-command (org-capture-finalize s n)
  "Commit the active `*Org Capture*` draft to its target and return."
  (let* ((b (current-buffer s))
         (st (and b (buffer-state b)))
         (commit (and (hash-table? st) (hash-table-ref/default st 'capture-commit #f)))
         (ret (and (hash-table? st) (hash-table-ref/default st 'capture-return #f))))
    (if (not commit)
        (message! s "Not in an Org capture draft buffer")
        (let ((body (strip-capture-header (document-string (doc s)))))
          (unless (eq? (commit s body) #f)
            (when (and ret (buffer-document ret))
              (show-buffer! s ret))
            (when b (drop-buffer! s b)))))))

(define-command (org-capture-kill s n)
  "Discard the active `*Org Capture*` draft without modifying the target."
  (let* ((b (current-buffer s))
         (st (and b (buffer-state b)))
         (ret (and (hash-table? st) (hash-table-ref/default st 'capture-return #f))))
    (when (and ret (buffer-document ret))
      (show-buffer! s ret))
    (when b (drop-buffer! s b))
    (message! s "Capture cancelled")))

(define-minor-mode org-capture-mode
  "Minor mode for editing an `*Org Capture*` draft buffer."
  #:keys '(("C-c C-c" org-capture-finalize)
           ("C-c C-k" org-capture-kill))
  #:normal '(("SPC m c" org-capture-finalize)
             ("SPC m k" org-capture-kill)))

(define (open-capture-draft! s label initial-body commit-fn)
  (let* ((ret (current-buffer s))
         (hdr (string-append "# Org Capture: " label " — C-c C-c to commit, C-c C-k to cancel\n"))
         (d (make-document (string-append hdr initial-body)))
         (st (make-hash-table))
         (buf (make-generated-buffer! "*Org Capture*" d 'org-mode #:state st)))
    (hash-table-set! st 'capture-commit commit-fn)
    (hash-table-set! st 'capture-return ret)
    (show-buffer! s buf)
    (unless (option buf 'org-capture-mode)
      (toggle-mode! s 'org-capture-mode))
    (move! s (lambda (_) (document-length (doc s))))
    (message! s "Edit capture draft: C-c C-c to commit, C-c C-k to cancel")))

(define (capture-into-file! s path header-thunk body-text #:heading [h #f] #:save [save? #f])
  (let* ((abs (absolute-path path))
         (parent (let loop ((i (- (string-length abs) 1)))
                   (cond ((< i 0) #f)
                         ((char=? (string-ref abs i) #\/) (substring abs 0 i))
                         (else (loop (- i 1)))))))
    (when (and parent (> (string-length parent) 0))
      (make-directory parent))
    (let* ((d (file-document abs))
           (v (make-view d "user")))
      (when (= (document-length d) 0)
        (let ((hdr (header-thunk)))
          (when (and hdr (> (string-length hdr) 0))
            (view-edit-at! v (document-revision d) (list (list 0 0 hdr)) "new"))))
      (let* ((txt (document-string d))
             (edits (org-capture-append-edits txt body-text #:heading h))
             (insert-pos (car (car edits))))
        (view-edit-at! v (document-revision d) edits "new")
        (when save? (save-document! d))
        (let ((pair (org-doc-outline d)))
          (when %org-mem
            (org-mem-update-file! %org-mem abs (car pair) (cdr pair) #:live (not save?))))
        (show-document! s d)
        (move! s (lambda (_) (min (+ insert-pos (org-utf8-length body-text)) (document-length (doc s)))))))))

(define-command (org-capture s n)
  "Capture a daily note, meeting note, new ID-node, node entry, or TODO."
  (let ((templates '("d — Daily note"
                     "m — Meeting note"
                     "n — New node"
                     "r — Capture into node"
                     "t — Quick TODO (daily)"))
        (use-draft? (option (current-buffer s) 'org-capture-use-draft)))
    (completing-read
     s "Capture template: " templates
     #:require-match #f
     #:accept
     (lambda (s cand)
       (let ((choice (cand->string cand)))
         (cond
           ((or (string=? choice "d") (string-prefix? "d " choice))
            (completing-read
             s "Daily heading: " '()
             #:require-match #f
             #:accept
             (lambda (s tcand)
               (let* ((title (cand->string tcand))
                      (day (car (local-now)))
                      (path (org-capture-daily-path (org-resolved-daily-directory) day))
                      (body (string-append "* " (string-trim title) "\n")))
                 (if use-draft?
                     (open-capture-draft! s "Daily note" body
                                          (lambda (s final-body)
                                            (capture-into-file! s path
                                                                (lambda () (org-capture-daily-header day))
                                                                final-body)
                                            (message! s "Captured daily note")))
                     (begin
                       (capture-into-file! s path (lambda () (org-capture-daily-header day)) body)
                       (message! s "Captured daily note")))))))
           ((or (string=? choice "t") (string-prefix? "t " choice))
            (completing-read
             s "TODO: " '()
             #:require-match #f
             #:accept
             (lambda (s tcand)
               (let* ((title (cand->string tcand))
                      (day (car (local-now)))
                      (path (org-capture-daily-path (org-resolved-daily-directory) day))
                      (body (string-append "* TODO " (string-trim title) "\n")))
                 (if use-draft?
                     (open-capture-draft! s "Quick TODO" body
                                          (lambda (s final-body)
                                            (capture-into-file! s path
                                                                (lambda () (org-capture-daily-header day))
                                                                final-body)
                                            (message! s "Captured TODO")))
                     (begin
                       (capture-into-file! s path (lambda () (org-capture-daily-header day)) body)
                       (message! s "Captured TODO")))))))
           ((or (string=? choice "m") (string-prefix? "m " choice))
            (let ((def-title (option #f 'org-meeting-default-title))
                  (def-part (option #f 'org-meeting-default-participants)))
              (completing-read
               s (string-append "Meeting (" def-title "): ") (list def-title)
               #:require-match #f
               #:accept
               (lambda (s mcand)
                 (let* ((mtitle (cand->string mcand))
                        (t (if (string=? (string-trim mtitle) "") def-title (string-trim mtitle))))
                   (completing-read
                    s (string-append "Participants (" def-part "): ") (list def-part)
                    #:require-match #f
                    #:accept
                    (lambda (s pcand)
                      (let* ((part (cand->string pcand))
                             (p (if (string=? (string-trim part) "") def-part (string-trim part)))
                             (np (local-now))
                             (day (car np))
                             (path (org-capture-meeting-path (org-resolved-daily-directory) day t))
                             (body (org-capture-meeting-entry t p np)))
                        (if use-draft?
                            (open-capture-draft! s (string-append "Meeting: " t) body
                                                 (lambda (s final-body)
                                                   (capture-into-file! s path
                                                                       (lambda () (org-capture-daily-header day))
                                                                       final-body)
                                                   (message! s (string-append "Captured meeting: " t))))
                            (begin
                              (capture-into-file! s path (lambda () (org-capture-daily-header day)) body)
                              (message! s (string-append "Captured meeting: " t))))))))))))
           ((or (string=? choice "n") (string-prefix? "n " choice))
            (completing-read
             s "New node title: " '()
             #:require-match #f
             #:accept
             (lambda (s tcand)
               (let ((t (string-trim (cand->string tcand))))
                 (unless (string=? t "")
                   (let-values (((d _nid _path) (org-create-node! t)))
                     (show-document! s d)
                     (move! s (lambda (_) (document-length (doc s))))
                     (message! s (string-append "Created node: " t))))))))
           ((or (string=? choice "r") (string-prefix? "r " choice))
            (let* ((idx (org-mem-current-index))
                   (cands (node-choice-candidates idx)))
              (completing-read
               s "Target node: " (map car cands)
               #:require-match #f
               #:accept
               (lambda (s ncand)
                 (let* ((nchoice (cand->string ncand))
                        (hit (assoc nchoice cands)))
                   (if (not hit)
                       (message! s "Node not found")
                       (let ((nd (cdr hit)))
                         (completing-read
                          s "Entry heading: " '()
                          #:require-match #f
                          #:accept
                          (lambda (s tcand)
                            (let* ((title (cand->string tcand))
                                   (target-path (org-mem-node-path nd))
                                   (nid (org-mem-node-id nd))
                                   (file-node? (not (org-mem-node-heading-index nd)))
                                   (resolve-target
                                    (lambda ()
                                      (let* ((d (file-document (absolute-path target-path)))
                                             (pair (org-doc-outline d)))
                                        (if file-node?
                                            (and (equal? (org-file-prop (cdr pair) "ID") nid) 'file)
                                            (find-unique-heading-by-id (car pair) (cdr pair) nid)))))
                                   (initial-target (resolve-target)))
                              (if (not initial-target)
                                  (message! s (if file-node?
                                                  "Capture target file node is stale or missing"
                                                  "Capture target heading is missing or ambiguous"))
                                  (let* ((stars (if file-node? "*" (make-string (+ (org-heading-level initial-target) 1) #\*)))
                                         (body (string-append stars " " (string-trim title) "\n"))
                                         (commit!
                                          (lambda (s final-body)
                                            (let ((t (resolve-target)))
                                              (if (not t)
                                                  (begin
                                                    (message! s (if file-node?
                                                                    "Capture target file node is stale or missing"
                                                                    "Capture target heading is missing or ambiguous"))
                                                    #f)
                                                  (begin
                                                    (if file-node?
                                                        (capture-into-file! s target-path (lambda () "") final-body)
                                                        (capture-into-file! s target-path (lambda () "") final-body #:heading t))
                                                    (message! s (string-append "Captured into " (org-mem-node-title nd)))
                                                    #t))))))
                                    (if use-draft?
                                        (open-capture-draft! s (string-append "Node: " (org-mem-node-title nd)) body commit!)
                                        (commit! s body))))))))))))))
           (else (message! s (string-append "Unknown capture template: " choice)))))))))

;; ----- Major mode definition -----

(define-mode org-mode
  "Outline-based notes management and organizer.
\\[org-cycle] folds or unfolds headings and drawers, \\[org-shifttab]
cycles global visibility, \\[org-todo] cycles TODO state,
\\[org-ctrl-c-ctrl-c] toggles checkboxes, \\[org-meta-return] inserts a
heading, and \\[org-open-at-point] follows links."
  #:parent 'text-mode
  #:files '(".org")
  #:layer org-highlight-layer
  #:projections org-conceal-projections
  #:folds org-view-folds
  #:target-at org-target-at
  #:keys '(("TAB" org-cycle)
           ("S-TAB" org-shifttab)
           ("<backtab>" org-shifttab)
           ("C-c C-r" org-reveal)
           ("C-c C-n" org-next-visible-heading)
           ("C-c C-p" org-previous-visible-heading)
           ("C-c C-f" org-forward-heading-same-level)
           ("C-c C-b" org-backward-heading-same-level)
           ("C-c C-u" org-up-heading)
           ("C-c C-t" org-todo)
           ("C-c ," org-priority)
           ("C-c C-s" org-schedule)
           ("C-c C-d" org-deadline)
           ("C-c C-q" org-set-tags)
           ("C-c C-x" "org-extra")
           ("C-c C-x p" org-set-property)
           ("C-c C-x e" org-set-effort)
           ("C-c C-x i" org-id-get-create)
           ("C-c C-x C-a" org-archive-subtree)
           ("C-c C-w" org-refile)
           ("C-c C-e" org-export)
           ("C-c '" org-edit-special)
           ("C-c C-v" "org-babel")
           ("C-c C-v e" org-babel-execute-src-block)
           ("C-c C-v b" org-babel-execute-buffer)
           ("C-c C-v t" org-babel-tangle)
           ("C-c C-v k" org-babel-remove-result)
           ("C-c C-c" org-ctrl-c-ctrl-c)
           ("C-c C-o" org-open-at-point)
           ("C-c j" org-open-meeting-link)
           ("C-c h" org-habit-log)
           ("C-c a" org-agenda)
           ("C-c c" org-capture)
           ("C-c ?" org-explain-in-agenda)
           ("C-c n" "org-node")
           ("C-c n f" org-node-find)
           ("C-c n i" org-node-insert-link)
           ("C-c n b" org-node-backlinks)
           ("C-c n r" org-node-visit-random)
           ("C-c n g" org-node-grep)
           ("C-c n s" org-search)
           ("M-RET" org-meta-return)
           ("M-S-RET" org-insert-todo-heading)
           ("C-RET" org-insert-heading-respect-content)
           ("S-<left>" org-shift-left)
           ("S-<right>" org-shift-right)
           ("S-<up>" org-shift-up)
           ("S-<down>" org-shift-down)
           ("M-<left>" org-do-promote)
           ("M-<right>" org-do-demote)
           ("M-S-<left>" org-promote-subtree)
           ("M-S-<right>" org-demote-subtree)
           ("M-<up>" org-move-subtree-up)
           ("M-<down>" org-move-subtree-down))
  #:normal '(("TAB" org-cycle)
             ("S-TAB" org-shifttab)
             ("<backtab>" org-shifttab)
             ("z a" org-cycle)
             ("z R" org-reveal)
             ("g j" org-forward-heading-same-level)
             ("g k" org-backward-heading-same-level)
             ("g h" org-up-heading)
             ("SPC m" "org")
             ("SPC m t" org-todo)
             ("SPC m p" org-priority)
             ("SPC m s" org-schedule)
             ("SPC m d" org-deadline)
             ("SPC m q" org-set-tags)
             ("SPC m o" org-open-at-point)
             ("SPC m j" org-open-meeting-link)
             ("SPC m h" org-habit-log)
             ("SPC m a" org-agenda)
             ("SPC m c" org-capture)
             ("SPC m ?" org-explain-in-agenda)
             ("SPC m x" org-ctrl-c-ctrl-c)
             ("SPC m b" "babel")
             ("SPC m b e" org-babel-execute-src-block)
             ("SPC m b b" org-babel-execute-buffer)
             ("SPC m b t" org-babel-tangle)
             ("SPC m b k" org-babel-remove-result)
             ("SPC m '" org-edit-special)
             ("SPC m r" org-refile)
             ("SPC m A" org-archive-subtree)
             ("SPC m e" org-export)
             ("SPC N" "org-node")
             ("SPC N f" org-node-find)
             ("SPC N i" org-node-insert-link)
             ("SPC N b" org-node-backlinks)
             ("SPC N r" org-node-visit-random)
             ("SPC N g" org-node-grep)
             ("SPC X" org-capture)
             ("M-RET" org-meta-return)
             ("C-RET" org-insert-heading-respect-content)
             ("M-<left>" org-do-promote)
             ("M-<right>" org-do-demote)
             ("M-S-<left>" org-promote-subtree)
             ("M-S-<right>" org-demote-subtree)
             ("M-<up>" org-move-subtree-up)
             ("M-<down>" org-move-subtree-down)))


