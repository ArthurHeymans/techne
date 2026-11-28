;;; Tests for lisp/org/date.scm, lisp/org/syntax.scm, and lisp/org/edit.scm.
(import (techne time) (techne test))
(test-file #:script #t)
(require "../org/date.scm")
(require "../org/syntax.scm")
(require "../org/edit.scm")

;; ----- 1. Timestamps, repeaters, warnings, time zones -----

(let ((ts (parse-org-timestamp "<2026-11-26 Thu 09:30-10:30 .+2d/4d -1d>")))
  (check "ts active?" #t (org-timestamp-active? ts))
  (check "ts day" (days-from-civil 2026 11 26) (org-timestamp-day ts))
  (check "ts time" 570 (org-timestamp-time ts))
  (check "ts end-time" 630 (org-timestamp-end-time ts))
  (check "ts habit repeater" '(".+" 2 #\d 4) (org-timestamp-repeater ts))
  (check "ts warning" '("-" 1 #\d) (org-timestamp-warning ts))
  (check "ts round-trip" "<2026-11-26 Thu 09:30-10:30 .+2d/4d -1d>" (format-org-timestamp ts))
  (let ((shifted (shift-org-timestamp ts #:now (cons (days-from-civil 2026 11 26) 600))))
    (check "habit shift preserves /4d, -1d, duration"
           "<2026-11-28 Sat 09:30-10:30 .+2d/4d -1d>"
           (format-org-timestamp shifted))))

;; ++ catch-up past now, + single step, hour unit, --1d warning, date range
(let ((ts-pp (parse-org-timestamp "<2026-11-01 Sun ++1w --2d>")))
  (check "++ catches up past today"
         "<2026-11-29 Sun ++1w --2d>"
         (format-org-timestamp (shift-org-timestamp ts-pp #:now (days-from-civil 2026 11 26)))))

(let ((ts-h (parse-org-timestamp "<2026-11-26 Thu 22:00 +4h>")))
  (check "+4h crosses midnight"
         "<2026-11-27 Fri 02:00 +4h>"
         (format-org-timestamp (shift-org-timestamp ts-h #:now (cons (days-from-civil 2026 11 26) 1320)))))

(let ((ts-range (parse-org-timestamp "<2026-11-26 Thu>--<2026-11-28 Sat>")))
  (check "date range round-trip" "<2026-11-26 Thu>--<2026-11-28 Sat>" (format-org-timestamp ts-range))
  (check "date range end day" (days-from-civil 2026 11 28) (org-timestamp-day (org-timestamp-end ts-range))))

;; local-now with DST zone (Europe/Brussels winter +01:00 vs summer +02:00)
(check "local-now winter" (cons (days-from-civil 2026 11 26) (+ (* 20 60) 45))
       (local-now #:instant (civil->instant '(2026 11 26 19 45)) #:zone "Europe/Brussels"))
(check "local-now summer" (cons (days-from-civil 2026 7 15) (+ (* 21 60) 45))
       (local-now #:instant (civil->instant '(2026 7 15 19 45)) #:zone "Europe/Brussels"))

;; ----- 2. Tier 1: Outline, file properties, multiple #+TODO, filetags, UTF-8 -----

(define sample-doc
  (string-append
   ":PROPERTIES:\n"
   ":ID:       abc-123\n"
   ":END:\n"
   "#+title: My Notes\n"
   "#+filetags: :todo:work:\n"
   "#+TODO: TODO(t) NEXT(n!) WAIT(w@/!) | DONE(d!) CANCELLED(c@)\n"
   "#+SEQ_TODO: REVIEW(r) | ARCHIVED(a)\n"
   "\n"
   "* TODO [#A] Café   task :urgent:\n"
   "  SCHEDULED: <2026-11-26 Thu 10:00>\n"
   "** NEXT Child task :deep:\n"
   "Body text.\n"
   "* REVIEW Second top\n"
   "Body 2.\n"))

(let ((ol (org-parse-outline sample-doc)))
  (check "file ID property" "abc-123" (list-ref (car (org-outline-file-properties ol)) 5))
  (check "title" "My Notes" (org-outline-title ol))
  (check "filetags" '("todo" "work") (org-outline-filetags ol))
  (check "merged todo keywords" '("TODO" "NEXT" "WAIT" "REVIEW") (org-outline-todo-keywords ol))
  (check "merged done keywords" '("DONE" "CANCELLED" "ARCHIVED") (org-outline-done-keywords ol))
  (check "WAIT logging spec" '("WAIT" #\w note time) (assoc "WAIT" (org-outline-todo-specs ol)))
  (let ((hs (org-outline-headings ol)))
    (check "heading count" 3 (vector-length hs))
    (let ((h0 (vector-ref hs 0))
          (h1 (vector-ref hs 1))
          (h2 (vector-ref hs 2)))
      (check "h0 title preserves whitespace and UTF-8" "Café   task" (org-heading-title sample-doc h0))
      (check "h0 priority" #\A (org-heading-priority h0))
      (check "h0 tags" '("urgent") (org-heading-tags h0))
      (check "h1 parent" 0 (org-heading-parent h1))
      (check "h1 inherited all-tags" '("todo" "work" "urgent" "deep") (org-heading-all-tags ol h1))
      (check "h0 subtree-end equals h2 start" (org-heading-start h2) (org-heading-subtree-end h0))
      (check "h2 keyword REVIEW" "REVIEW" (org-heading-keyword h2))
      (check "binary search heading-at" 1 (org-heading-index (org-heading-at ol (org-heading-body-start h1)))))))

;; Config-provided TODO keywords when document has no #+TODO
(let ((ol (org-parse-outline "* REVIEW Check PR\n* HOLD Wait\n"
                             #:todo-keywords '("TODO" "REVIEW" "HOLD")
                             #:done-keywords '("DONE" "KILL"))))
  (check "config REVIEW" "REVIEW" (org-heading-keyword (vector-ref (org-outline-headings ol) 0)))
  (check "config HOLD" "HOLD" (org-heading-keyword (vector-ref (org-outline-headings ol) 1))))

;; ----- 3. Incremental outline update == full reparse -----

(define (outline-snapshot text ol)
  (map (lambda (h)
         (list (org-heading-index h) (org-heading-level h) (org-heading-parent h)
               (org-heading-start h) (org-heading-line-end h) (org-heading-body-start h)
               (org-heading-subtree-end h) (org-heading-keyword h) (org-heading-priority h)
               (org-heading-title text h) (org-heading-tags h)))
       (vector->list (org-outline-headings ol))))

(let* ((ol0 (org-parse-outline sample-doc))
       (h1 (vector-ref (org-outline-headings ol0) 1))
       ;; Edit 1: insert a new child heading inside h1's body
       (pos (org-heading-body-start h1))
       (ins "*** TODO Inserted grand-child\n")
       (doc1 (org-apply-edits sample-doc (list (list pos pos ins))))
       (ol1-inc (org-outline-update ol0 doc1 pos pos ins))
       (ol1-full (org-parse-outline doc1)))
  (check "incremental insert heading == full" (outline-snapshot doc1 ol1-full) (outline-snapshot doc1 ol1-inc))
  ;; Edit 2: delete that heading
  (let* ((h2 (vector-ref (org-outline-headings ol1-inc) 2))
         (from (org-heading-start h2))
         (to (org-heading-subtree-end h2))
         (doc2 (org-apply-edits doc1 (list (list from to ""))))
         (ol2-inc (org-outline-update ol1-inc doc2 from to ""))
         (ol2-full (org-parse-outline doc2)))
    (check "incremental delete heading == full" (outline-snapshot doc2 ol2-full) (outline-snapshot doc2 ol2-inc))))

;; ----- 4. Tier 2: Section elements & tiling invariant -----

(define rich-section
  (string-append
   "  SCHEDULED: <2026-11-26 Thu> DEADLINE: <2026-12-01 Tue>\n"
   ":PROPERTIES:\n"
   ":OWNER: arthur\n"
   ":EFFORT: 1:30\n"
   ":END:\n"
   ":LOGBOOK:\n"
   "- State \"DONE\"       from \"TODO\"       [2026-11-25 Wed 09:00]\n"
   ":END:\n"
   "\n"
   "#+name: demo\n"
   "#+begin_src scheme :tangle yes\n"
   "(+ 1 2)\n"
   "#+end_src\n"
   "| a | b |\n"
   "|---+---|\n"
   "| 1 | 2 |\n"
   "- [ ] first\n"
   "- [X] second\n"
   "# a comment\n"
   "-----\n"
   "A plain paragraph\n"
   "spanning two lines.\n"))

(define (check-tiling label text from to elems)
  (let loop ((es elems) (pos from))
    (if (null? es)
        (check (string-append label " reaches end") to pos)
        (let ((e (car es)))
          (check (string-append label " contiguous at " (number->string pos)) pos (org-element-start e))
          (check-true (string-append label " non-empty") (< (org-element-start e) (org-element-end e)))
          (loop (cdr es) (org-element-end e))))))

(let* ((total (org-utf8-length rich-section))
       (elems (org-parse-section rich-section 0 total)))
  (check-tiling "rich-section" rich-section 0 total elems)
  (check "element kinds"
         '(planning drawer drawer blank keyword src-block table list-item list-item comment rule paragraph)
         (map org-element-kind elems))
  (check "src lang" "scheme" (car (org-element-data (list-ref elems 5))))
  (check "logbook state entry" "DONE" (cadr (car (cadddr (org-element-data (list-ref elems 2)))))))

;; ----- 5. Tier 3: Inline objects -----

(let* ((line "See [[id:123][My Node]] and *bold* /ital/ ~code~ =verb= +del+ _und_ foo_bar_baz [1/3] <2026-11-26 Thu> https://example.com/a.")
       (inlines (org-parse-inline line 0 (org-utf8-length line))))
  (check "inline kinds"
         '(link bold italic code verbatim strike underline cookie timestamp-active link)
         (map org-inline-kind inlines))
  (check "bracket link target" "id:123" (car (org-inline-data (car inlines))))
  (check "bare url trims trailing dot" "https://example.com/a" (car (org-inline-data (last inlines)))))

;; ----- 6. Edit confinement & Emacs-compatible mutations -----

;; Fable 5.2's exact habit case!
(let* ((now (cons (days-from-civil 2026 11 26) (+ (* 20 60) 45)))
       (doc (string-append
             "* TODO Pray   twice :PRAYER:\n"
             "  SCHEDULED: <2026-11-26 Thu .+2d/4d -1d> DEADLINE: <2026-12-01 Tue>\n"))
       (ol (org-parse-outline doc))
       (h (vector-ref (org-outline-headings ol) 0))
       (edits (org-edit-set-todo doc ol h "DONE" #:now now))
       (out (org-apply-edits doc edits))
       (expected
        (string-append
         "* TODO Pray   twice :PRAYER:\n"
         "  SCHEDULED: <2026-11-28 Sat .+2d/4d -1d> DEADLINE: <2026-12-01 Tue>\n"
         ":PROPERTIES:\n"
         ":LAST_REPEAT: [2026-11-26 Thu 20:45]\n"
         ":END:\n"
         ":LOGBOOK:\n"
         "- State \"DONE\"       from \"TODO\"       [2026-11-26 Thu 20:45]\n"
         ":END:\n")))
  (check "habit DONE preserves whitespace, /4d, -1d, DEADLINE, logs to LOGBOOK" expected out))

;; Non-repeating task: CLOSED goes FIRST on planning line, removed on reopen.
(let* ((now (cons (days-from-civil 2026 11 26) (+ (* 20 60) 45)))
       (doc "* TODO Write   docs :work:\n  SCHEDULED: <2026-11-26 Thu>\n")
       (ol (org-parse-outline doc))
       (h (vector-ref (org-outline-headings ol) 0))
       (done-doc (org-apply-edits doc (org-edit-set-todo doc ol h "DONE" #:now now))))
  (check "non-repeating DONE puts CLOSED first"
         "* DONE Write   docs :work:\n  CLOSED: [2026-11-26 Thu 20:45] SCHEDULED: <2026-11-26 Thu>\n"
         done-doc)
  (let* ((ol2 (org-parse-outline done-doc))
         (h2 (vector-ref (org-outline-headings ol2) 0))
         (reopened (org-apply-edits done-doc (org-edit-set-todo done-doc ol2 h2 "TODO" #:now now))))
    (check "reopening removes CLOSED cleanly" doc reopened)))

;; Priority, tags, promote/demote, move subtree
(let* ((doc "* TODO Alpha\n** Sub\n* TODO Beta")
       (ol (org-parse-outline doc))
       (h0 (vector-ref (org-outline-headings ol) 0))
       (h2 (vector-ref (org-outline-headings ol) 2)))
  (check "set priority" "* TODO [#A] Alpha\n** Sub\n* TODO Beta"
         (org-apply-edits doc (org-edit-set-priority doc ol h0 #\A)))
  (check "set tags" "* TODO Alpha :a:b:\n** Sub\n* TODO Beta"
         (org-apply-edits doc (org-edit-set-tags doc ol h0 '("a" "b"))))
  (check "demote subtree" "** TODO Alpha\n*** Sub\n* TODO Beta"
         (org-apply-edits doc (org-edit-demote-subtree doc ol h0)))
  (check "move subtree down (EOF no newline)" "* TODO Beta\n* TODO Alpha\n** Sub"
         (org-apply-edits doc (org-edit-move-subtree-down doc ol h0)))
  (check "move subtree up (EOF no newline)" "* TODO Beta\n* TODO Alpha\n** Sub"
         (org-apply-edits doc (org-edit-move-subtree-up doc ol h2))))

;; Toggle checkbox updates headline statistics cookies [n/m] and [p%]
(let* ((doc "* Tasks [1/2] [50%]\n- [ ] one\n- [X] two\n")
       (ol (org-parse-outline doc))
       (h (vector-ref (org-outline-headings ol) 0))
       (pos (string-contains doc "- [ ] one"))
       (out (org-apply-edits doc (org-edit-toggle-checkbox doc ol h pos))))
  (check "checkbox toggle updates cookies"
         "* Tasks [2/2] [100%]\n- [X] one\n- [X] two\n"
         out))

;; ----- 7. Regression tests from GPT-6.2 Sol review -----

;; Date-only hourly ++ repeater must not hang and must advance cleanly.
(let ((ts (parse-org-timestamp "<2026-01-01 Thu ++1h>")))
  (check "date-only ++1h advances without hanging"
         "<2026-01-01 Thu 02:00 ++1h>"
         (format-org-timestamp (shift-org-timestamp ts #:now (cons (days-from-civil 2026 1 1) 90)))))

;; Timed ++1d that lands later today must NOT skip today.
(let ((ts (parse-org-timestamp "<2026-01-01 Thu 18:00 ++1d>")))
  (check "timed ++1d lands later today"
         "<2026-01-02 Fri 18:00 ++1d>"
         (format-org-timestamp (shift-org-timestamp ts #:now (cons (days-from-civil 2026 1 2) 600)))))

;; Warning --2d inside date range <... --2d>--<...> and shifting both endpoints.
(let ((ts (parse-org-timestamp "<2026-01-01 Thu +1w --2d>--<2026-01-03 Sat>")))
  (check "warning + date range round-trip"
         "<2026-01-01 Thu +1w --2d>--<2026-01-03 Sat>"
         (format-org-timestamp ts))
  (check "shifting date range shifts both endpoints"
         "<2026-01-08 Thu +1w --2d>--<2026-01-10 Sat>"
         (format-org-timestamp (shift-org-timestamp ts #:now (days-from-civil 2026 1 1)))))

;; EOF without trailing newline: repeating completion must not glue :PROPERTIES: or :LOGBOOK: onto previous line.
(let* ((now (cons (days-from-civil 2026 1 10) 600))
       (doc1 "* TODO task\nSCHEDULED: <2026-01-10 Sat +1d>")
       (ol1 (org-parse-outline doc1))
       (out1 (org-apply-edits doc1 (org-edit-set-todo doc1 ol1 (vector-ref (org-outline-headings ol1) 0) "DONE" #:now now))))
  (check "EOF no-newline repeater inserts newline before :PROPERTIES:"
         (string-append
          "* TODO task\n"
          "SCHEDULED: <2026-01-11 Sun +1d>\n"
          ":PROPERTIES:\n:LAST_REPEAT: [2026-01-10 Sat 10:00]\n:END:\n"
          ":LOGBOOK:\n- State \"DONE\"       from \"TODO\"       [2026-01-10 Sat 10:00]\n:END:\n")
         out1))

(let* ((now (cons (days-from-civil 2026 1 10) 600))
       (doc2 "* TODO task\nSCHEDULED: <2026-01-10 Sat +1d>\n:PROPERTIES:\n:ID: 1\n:END:")
       (ol2 (org-parse-outline doc2))
       (out2 (org-apply-edits doc2 (org-edit-set-todo doc2 ol2 (vector-ref (org-outline-headings ol2) 0) "DONE" #:now now))))
  (check "EOF no-newline existing :PROPERTIES: inserts newline before :LOGBOOK:"
         (string-append
          "* TODO task\n"
          "SCHEDULED: <2026-01-11 Sun +1d>\n"
          ":PROPERTIES:\n:ID: 1\n:LAST_REPEAT: [2026-01-10 Sat 10:00]\n:END:\n"
          ":LOGBOOK:\n- State \"DONE\"       from \"TODO\"       [2026-01-10 Sat 10:00]\n:END:\n")
         out2))

;; Non-ASCII priority [#é] must be rejected and kept as title text.
(let* ((doc "* TODO [#é] Café\n")
       (ol (org-parse-outline doc))
       (h (vector-ref (org-outline-headings ol) 0)))
  (check "non-ASCII priority rejected" #f (org-heading-priority h))
  (check "invalid priority preserved in title" "[#é] Café" (org-heading-title doc h)))

;; Separate #+TODO sequences cycle within their own sequence.
(let* ((doc "#+TODO: TODO NEXT | DONE\n#+SEQ_TODO: REVIEW | ARCHIVED\n* REVIEW Check\n")
       (ol (org-parse-outline doc))
       (h (vector-ref (org-outline-headings ol) 0)))
  (check "REVIEW cycles to ARCHIVED within its sequence"
         "#+TODO: TODO NEXT | DONE\n#+SEQ_TODO: REVIEW | ARCHIVED\n* ARCHIVED Check\n"
         (org-apply-edits doc (org-edit-cycle-todo doc ol h #:log-done #f))))

;; Nested link inside *bold [[https://example.com][link]]* is discoverable; ~code~ stays opaque.
(let* ((line "*bold [[https://example.com][link]]* and ~[[https://ignored.example][no]]~")
       (inlines (org-parse-inline line 0 (org-utf8-length line))))
  (check "nested link inside bold exposed, code opaque"
         '(bold link code)
         (map org-inline-kind inlines)))


