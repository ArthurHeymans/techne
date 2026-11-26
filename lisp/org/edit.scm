;;; Pure byte-splice editing operations for Org documents.
;;;
;;; Every operation takes the authoritative source TEXT, an `org-outline`,
;;; and target parameters, and returns a list of (from to replacement)
;;; UTF-8 byte splices. Untouched text (title whitespace, unrelated
;;; timestamps, drawers, blocks) is never re-serialized.

(require "date.scm")
(require "syntax.scm")

(provide org-apply-edits org-format-state-log
         org-edit-set-todo org-edit-cycle-todo
         org-edit-set-priority org-edit-set-tags
         org-edit-set-planning org-edit-set-property
         org-edit-promote-heading org-edit-demote-heading
         org-edit-promote-subtree org-edit-demote-subtree
         org-edit-move-subtree-up org-edit-move-subtree-down
         org-edit-insert-heading org-edit-toggle-checkbox
         org-edit-shift-timestamp-at org-edit-insert-list-item
         org-edit-align-table org-edit-relevel-subtree)

;; Sort edits by start byte ascending (preserving original order for equal
;; start offsets), validate non-overlapping ranges, and coalesce adjacent
;; zero-width insertions at the exact same byte offset.
(define (normalize-edits edits)
  (let* ((indexed (let loop ((es (filter (lambda (e) e) edits)) (i 0) (acc '()))
                    (if (null? es)
                        (reverse acc)
                        (loop (cdr es) (+ i 1) (cons (cons i (car es)) acc)))))
         (sorted (sort indexed (lambda (a b)
                                 (let ((ea (cdr a)) (eb (cdr b)))
                                   (cond ((< (car ea) (car eb)) #t)
                                         ((> (car ea) (car eb)) #f)
                                         (else (< (car a) (car b))))))))
         (clean (map cdr sorted)))
    ;; Validate no overlapping spans and coalesce identical zero-width insertions.
    (let loop ((es clean) (acc '()))
      (if (null? es)
          (reverse acc)
          (let ((cur (car es)))
            (when (> (car cur) (cadr cur))
              (error "org-edit: invalid inverted splice range" cur))
            (if (null? acc)
                (loop (cdr es) (list cur))
                (let ((prev (car acc)))
                  (cond
                    ((and (= (car prev) (cadr prev)) (= (car cur) (cadr cur)) (= (car prev) (car cur)))
                     (loop (cdr es) (cons (list (car prev) (cadr prev) (string-append (caddr prev) (caddr cur))) (cdr acc))))
                    ((< (car cur) (cadr prev))
                     (error "org-edit: overlapping splice ranges" prev cur))
                    (else
                     (loop (cdr es) (cons cur acc)))))))))))

(define (org-apply-edits text edits)
  "Apply a list of (from to replacement) UTF-8 byte splices EDITS to TEXT."
  (let ((sorted (normalize-edits edits)))
    (if (null? sorted)
        text
        (let ((len (string-length text)))
          ;; Single forward pass converting byte offsets to char indices incrementally.
          (define (char-len c)
            (let ((cp (char->integer c)))
              (cond ((< cp #x80) 1) ((< cp #x800) 2) ((< cp #x10000) 3) (else 4))))
          (define (advance-to ci b target-b)
            (let loop ((i ci) (byte b))
              (if (or (>= byte target-b) (>= i len))
                  (cons i byte)
                  (loop (+ i 1) (+ byte (char-len (string-ref text i)))))))
          (let loop ((es sorted) (ci 0) (b 0) (parts '()))
            (if (null? es)
                (apply string-append (reverse (cons (substring text ci len) parts)))
                (let* ((e (car es))
                       (p1 (advance-to ci b (car e)))
                       (ci1 (car p1))
                       (b1 (cdr p1))
                       (p2 (advance-to ci1 b1 (cadr e)))
                       (ci2 (car p2))
                       (b2 (cdr p2)))
                  (loop (cdr es) ci2 b2 (cons (caddr e) (cons (substring text ci ci1) parts))))))))))

(define (now->inactive-ts now)
  (let* ((pair (cond ((pair? now) now)
                     ((exact-integer? now) (cons now #f))
                     (else (local-now)))))
    (format-org-timestamp (make-org-timestamp #f (car pair) #:time (cdr pair)))))

(define (pad-right s width)
  (if (>= (string-length s) width)
      s
      (string-append s (make-string (- width (string-length s)) #\space))))

(define (org-format-state-log to-state from-state now)
  "Format a LOGBOOK state-change line at NOW.
Records transition to TO-STATE from FROM-STATE."
  (let ((to-q (string-append "\"" (or to-state "") "\""))
        (from-q (string-append "\"" (or from-state "") "\""))
        (ts (now->inactive-ts now)))
    (string-append "- State " (pad-right to-q 12) " from " (pad-right from-q 12) " " ts)))

;; ----- Headline field edits -----

(define (keyword-splice text h new-kw)
  (let ((kr (org-heading-keyword-range h))
        (sr (org-heading-stars-range h))
        (le (org-heading-line-end h)))
    (cond
      ((and kr new-kw)
       (if (string=? (org-heading-keyword h) new-kw)
           #f
           (list (car kr) (cdr kr) new-kw)))
      ((and kr (not new-kw))
       ;; Remove keyword and one trailing space if present.
       (let ((after (if (and (< (cdr kr) le) (string=? (org-slice-bytes text (cdr kr) (+ (cdr kr) 1)) " "))
                        (+ (cdr kr) 1)
                        (cdr kr))))
         (list (car kr) after "")))
      ((and (not kr) new-kw)
       ;; Insert after stars + space.
       (let ((ins (if (< (cdr sr) le) (+ (cdr sr) 1) (cdr sr))))
         (if (= ins (cdr sr))
             (list ins ins (string-append " " new-kw))
             (list ins ins (string-append new-kw " ")))))
      (else #f))))

(define (org-edit-set-priority text outline h prio)
  "Return edits setting H's priority to PRIO in TEXT and OUTLINE.
Pass #f for PRIO to clear the priority."
  (let ((pr (org-heading-priority-range h))
        (kr (org-heading-keyword-range h))
        (sr (org-heading-stars-range h))
        (le (org-heading-line-end h)))
    (cond
      ((and pr prio)
       (if (eqv? (org-heading-priority h) prio)
           '()
           (list (list (car pr) (cdr pr) (string-append "[#" (string prio) "]")))))
      ((and pr (not prio))
       (let ((after (if (and (< (cdr pr) le) (string=? (org-slice-bytes text (cdr pr) (+ (cdr pr) 1)) " "))
                        (+ (cdr pr) 1)
                        (cdr pr))))
         (list (list (car pr) after ""))))
      ((and (not pr) prio)
       (let* ((base (if kr (cdr kr) (cdr sr)))
              (ins (if (< base le) (+ base 1) base)))
         (if (= ins base)
             (list (list ins ins (string-append " [#" (string prio) "]")))
             (list (list ins ins (string-append "[#" (string prio) "] "))))))
      (else '()))))

(define (org-edit-set-tags text outline h tags)
  "Return edits setting H's own tags to TAGS in TEXT and OUTLINE."
  (let* ((clean (filter (lambda (t) (and (string? t) (not (string-null? t)))) tags))
         (tr (org-heading-tags-range h))
         (le (org-heading-line-end h))
         (tag-str (if (null? clean) "" (string-append ":" (string-join clean ":") ":"))))
    (cond
      ((and tr (null? clean))
       ;; Also eat spaces immediately before the tag block.
       (let* ((title-end (cdr (org-heading-title-range h)))
              (from (min title-end (car tr))))
         (list (list from (cdr tr) ""))))
      ((and tr (pair? clean))
       (list (list (car tr) (cdr tr) tag-str)))
      ((and (not tr) (pair? clean))
       (list (list le le (string-append " " tag-str))))
      (else '()))))

;; ----- Planning line edits -----

;; Compute the byte position right after the heading's line break (inserting \n if EOF without \n).
(define (heading-after-line text h)
  (let ((le (org-heading-line-end h))
        (bs (org-heading-body-start h)))
    (if (> bs le) (cons bs "") (cons le "\n"))))

(define (planning-kind-label kind)
  (case kind
    ((closed) "CLOSED:")
    ((deadline) "DEADLINE:")
    ((scheduled) "SCHEDULED:")
    (else (error "Unknown planning kind" kind))))

;; Return a single edit updating or inserting KIND -> TS-STR (or #f to remove) on H's planning line.
;; Preserves CLOSED first and leaves untouched entries byte-identical.
(define (planning-splice text h elems kind ts-str)
  (let ((plan-el (org-section-planning elems)))
    (if (not plan-el)
        (if (not ts-str)
            #f
            (let* ((pos+pre (heading-after-line text h))
                   (pos (car pos+pre))
                   (pre (cdr pos+pre)))
              (list pos pos (string-append pre (planning-kind-label kind) " " ts-str "\n"))))
        (let* ((entries (car (org-element-data plan-el)))
               (indent-bytes (cdr (org-element-data plan-el)))
               (existing (find (lambda (e) (eq? (car e) kind)) entries))
               (pstart (org-element-start plan-el))
               (pend (org-element-end plan-el)))
          (cond
            ;; Update existing timestamp in place!
            ((and existing ts-str)
             (list (list-ref existing 3) (list-ref existing 4) ts-str))
            ;; Remove existing entry
            ((and existing (not ts-str))
             (if (= (length entries) 1)
                 ;; Remove whole planning line
                 (list pstart pend "")
                 ;; Remove just this entry + adjacent single space
                 (let* ((ks (list-ref existing 1))
                        (te (list-ref existing 4))
                        (is-first (eq? existing (car entries))))
                   (if is-first
                       (let ((next-ks (list-ref (cadr entries) 1)))
                         (list ks next-ks ""))
                       (let ((prev-te (let loop ((es entries))
                                        (if (eq? (cadr es) existing)
                                            (list-ref (car es) 4)
                                            (loop (cdr es))))))
                         (list prev-te te ""))))))
            ;; Insert new entry into existing planning line
            ((and (not existing) ts-str)
             (let ((chunk (string-append (planning-kind-label kind) " " ts-str)))
               (if (eq? kind 'closed)
                   ;; CLOSED goes first on the planning line (matching Emacs).
                   (let ((first-ks (list-ref (car entries) 1)))
                     (list first-ks first-ks (string-append chunk " ")))
                   ;; Append after last entry on the planning line.
                   (let ((last-te (list-ref (last entries) 4)))
                     (list last-te last-te (string-append " " chunk))))))
            (else #f))))))

(define (org-edit-set-planning text outline h kind ts)
  "Return edits setting planning KIND to TS on H in TEXT and OUTLINE.
KIND is `scheduled`, `deadline` or `closed`; TS is a timestamp, string
or #f to remove."
  (let* ((elems (org-parse-section text (org-heading-body-start h) (org-heading-section-end outline h)))
         (ts-str (cond ((not ts) #f)
                       ((string? ts) ts)
                       (else (format-org-timestamp ts))))
         (e (planning-splice text h elems kind ts-str)))
    (if e (list e) '())))

;; ----- Property & LOGBOOK drawer edits -----

;; Ensure inserting at POS starts on a fresh line (handles EOF without trailing newline).
(define (ensure-line-start text pos)
  (if (or (= pos 0) (string=? (org-slice-bytes text (- pos 1) pos) "\n"))
      (cons pos "")
      (cons pos "\n")))

;; Where a new drawer (PROPERTIES or LOGBOOK) should be inserted on H.
(define (drawer-insert-pos text h elems after-properties?)
  (let* ((plan (org-section-planning elems))
         (props (and after-properties? (org-section-properties elems))))
    (cond
      (props (ensure-line-start text (org-element-end props)))
      (plan (ensure-line-start text (org-element-end plan)))
      (else (heading-after-line text h)))))

(define (property-splice text h elems key val)
  (let ((drawer (org-section-properties elems)))
    (if (not drawer)
        (if (not val)
            #f
            (let* ((pos+pre (drawer-insert-pos text h elems #f))
                   (pos (car pos+pre))
                   (pre (cdr pos+pre)))
              (list pos pos (string-append pre ":PROPERTIES:\n:" key ": " val "\n:END:\n"))))
        (let* ((data (org-element-data drawer))
               (footer-start (caddr data))
               (entries (cadddr data))
               (existing (find (lambda (p) (string-ci=? (car p) key)) entries)))
          (cond
            ((and existing val)
             (let* ((key-end (list-ref existing 2))
                    (vs (list-ref existing 3))
                    (ve (list-ref existing 4))
                    (needs-sep? (and (= vs key-end) (not (string=? val "")))))
               (list vs ve (if needs-sep? (string-append " " val) val))))
            ((and existing (not val))
             ;; Remove the property line
             (let* ((ks (list-ref existing 1))
                    (ve (list-ref existing 4))
                    ;; Find start of line and end including \n
                    (ci-ks (org-char-index-at-byte text ks))
                    (lstart (let back ((j ci-ks) (b ks))
                              (if (or (<= j 0) (char=? (string-ref text (- j 1)) #\newline))
                                  b
                                  (back (- j 1) (- b (org-utf8-length (substring text (- j 1) j)))))))
                    (ci-ve (org-char-index-at-byte text ve))
                    (len (string-length text))
                    (lend (let fwd ((j ci-ve) (b ve))
                            (cond ((>= j len) b)
                                  ((char=? (string-ref text j) #\newline) (+ b 1))
                                  (else (fwd (+ j 1) (+ b (org-utf8-length (substring text j (+ j 1))))))))))
               (list lstart lend "")))
            ((and (not existing) val)
             (list footer-start footer-start (string-append ":" key ": " val "\n")))
            (else #f))))))

(define (org-edit-set-property text outline h key val)
  "Return edits setting property KEY to VAL on H in TEXT and OUTLINE.
Pass #f for VAL to remove the property."
  (let* ((elems (org-parse-section text (org-heading-body-start h) (org-heading-section-end outline h)))
         (e (property-splice text h elems key val)))
    (if e (list e) '())))

(define (logbook-splice text h elems line-str drawer-name)
  (if (not drawer-name)
      (let* ((pos+pre (drawer-insert-pos text h elems #t)))
        (list (car pos+pre) (car pos+pre) (string-append (cdr pos+pre) line-str "\n")))
      (let ((drawer (find (lambda (e) (and (eq? (org-element-kind e) 'drawer)
                                           (string-ci=? (car (org-element-data e)) drawer-name)))
                          elems)))
        (if drawer
            ;; Insert right after :LOGBOOK:\n (newest entry first, matching Emacs).
            (let* ((header-end (cadr (org-element-data drawer)))
                   (ins (+ header-end 1)))
              (list ins ins (string-append line-str "\n")))
            (let* ((pos+pre (drawer-insert-pos text h elems #t)))
              (list (car pos+pre) (car pos+pre)
                    (string-append (cdr pos+pre) ":" drawer-name ":\n" line-str "\n:END:\n")))))))

;; ----- TODO state changes (with repeaters, CLOSED, LOGBOOK) -----

(define (find-todo-spec outline kw)
  (and kw (assoc kw (org-outline-todo-specs outline))))

(define (sequence-for-keyword outline kw)
  (let ((seqs (org-outline-todo-sequences outline)))
    (or (and kw (find (lambda (seq) (or (member kw (car seq)) (member kw (cdr seq)))) seqs))
        (and (pair? seqs) (car seqs))
        (cons (org-outline-todo-keywords outline) (org-outline-done-keywords outline)))))

;; Combine two same-position zero-width insertions without duplicating a leading "\n".
(define (combine-insertions a b)
  (cond
    ((and a b (= (car a) (cadr a)) (= (car b) (cadr b)) (= (car a) (car b)))
     (let* ((sa (caddr a))
            (sb (caddr b))
            (sb-clean (if (and (> (string-length sa) 0)
                               (char=? (string-ref sa (- (string-length sa) 1)) #\newline)
                               (string-prefix? "\n" sb))
                          (substring sb 1)
                          sb)))
       (list (list (car a) (cadr a) (string-append sa sb-clean)))))
    (else (filter (lambda (x) x) (list a b)))))

(define (org-edit-set-todo text outline h new-kw #:now [now #f]
                           #:log-done [log-done 'time]
                           #:log-into-drawer [log-into-drawer "LOGBOOK"])
  "Return edits changing H's TODO keyword to NEW-KW in TEXT and OUTLINE.
Uses NOW for timestamps, LOG-DONE for `CLOSED:` logging, and
LOG-INTO-DRAWER for state-change logging. Handles repeaters in place."
  (let* ((old-kw (org-heading-keyword h))
         (seq (sequence-for-keyword outline (or old-kw new-kw)))
         (todo-list (car seq))
         (done-list (org-outline-done-keywords outline))
         (entering-done? (and new-kw (member new-kw done-list) #t))
         (was-done? (and old-kw (member old-kw done-list) #t))
         (elems (org-parse-section text (org-heading-body-start h) (org-heading-section-end outline h)))
         (plan-el (org-section-planning elems))
         (plan-entries (if plan-el (car (org-element-data plan-el)) '()))
         (repeating (if entering-done?
                        (filter (lambda (e) (and (memq (car e) '(scheduled deadline))
                                                 (org-timestamp-repeater (list-ref e 5))))
                                plan-entries)
                        '()))
         (now-val (or now (local-now))))
    (if (pair? repeating)
        ;; Repeating task marked DONE: shift repeating timestamps in place,
        ;; reset keyword to REPEAT_TO_STATE or first TODO keyword in its sequence,
        ;; set LAST_REPEAT, and log the DONE transition into LOGBOOK.
        (let* ((repeat-to (or (org-heading-property text outline h "REPEAT_TO_STATE")
                              (and (pair? todo-list) (car todo-list))
                              "TODO"))
               (kw-edit (keyword-splice text h repeat-to))
               (ts-edits (map (lambda (e)
                                (let* ((ts (list-ref e 5))
                                       (shifted (shift-org-timestamp ts #:now now-val)))
                                  (list (list-ref e 3) (list-ref e 4) (format-org-timestamp shifted))))
                              repeating))
               (last-rep-edit (property-splice text h elems "LAST_REPEAT" (now->inactive-ts now-val)))
               (log-edit (logbook-splice text h elems
                                         (org-format-state-log new-kw old-kw now-val)
                                         log-into-drawer))
               (drawer-edits (combine-insertions last-rep-edit log-edit)))
          (normalize-edits (append (if kw-edit (list kw-edit) '()) ts-edits drawer-edits)))
        ;; Non-repeating state change
        (let* ((kw-edit (keyword-splice text h new-kw))
               (closed-edit
                (cond
                  ((and entering-done? log-done)
                   (planning-splice text h elems 'closed (now->inactive-ts now-val)))
                  ((and was-done? (not entering-done?))
                   (planning-splice text h elems 'closed #f))
                  (else #f)))
               (new-spec (find-todo-spec outline new-kw))
               (old-spec (find-todo-spec outline old-kw))
               (should-log? (and (not (equal? old-kw new-kw))
                                 (or (and new-spec (caddr new-spec))
                                     (and old-spec (cadddr old-spec)))))
               (log-edit (and should-log?
                              (logbook-splice text h elems
                                              (org-format-state-log new-kw old-kw now-val)
                                              log-into-drawer)))
               (tail-edits (combine-insertions closed-edit log-edit)))
          (normalize-edits (append (if kw-edit (list kw-edit) '()) tail-edits))))))

(define (org-edit-cycle-todo text outline h #:now [now #f]
                             #:log-done [log-done 'time]
                             #:log-into-drawer [log-into-drawer "LOGBOOK"])
  "Return edits cycling H to the next TODO keyword in TEXT and OUTLINE.
Cycles within H's own TODO sequence. Passes NOW, LOG-DONE and
LOG-INTO-DRAWER to `org-edit-set-todo`."
  (let* ((cur (org-heading-keyword h))
         (seq (sequence-for-keyword outline cur))
         (all (append (car seq) (cdr seq)))
         (next (cond ((not cur) (and (pair? all) (car all)))
                     ((member cur all) => (lambda (tail) (if (pair? (cdr tail)) (cadr tail) #f)))
                     (else (and (pair? all) (car all))))))
    (org-edit-set-todo text outline h next #:now now #:log-done log-done #:log-into-drawer log-into-drawer)))

;; ----- Structural edits: promote, demote, move subtree, insert heading -----

(define (org-edit-promote-heading text outline h)
  "Return edits promoting H by one level in TEXT and OUTLINE."
  (if (<= (org-heading-level h) 1)
      '()
      (list (list (org-heading-start h) (+ (org-heading-start h) 1) ""))))

(define (org-edit-demote-heading text outline h)
  "Return edits demoting H by one level in TEXT and OUTLINE."
  (list (list (org-heading-start h) (org-heading-start h) "*")))

(define (subtree-headings outline h)
  (let* ((hs (org-outline-headings outline))
         (n (vector-length hs))
         (end (org-heading-subtree-end h)))
    (let loop ((i (org-heading-index h)) (acc '()))
      (if (or (>= i n) (>= (org-heading-start (vector-ref hs i)) end))
          (reverse acc)
          (loop (+ i 1) (cons (vector-ref hs i) acc))))))

(define (org-edit-promote-subtree text outline h)
  "Return edits promoting H's subtree in TEXT and OUTLINE."
  (if (<= (org-heading-level h) 1)
      '()
      (map (lambda (x) (list (org-heading-start x) (+ (org-heading-start x) 1) ""))
           (subtree-headings outline h))))

(define (org-edit-demote-subtree text outline h)
  "Return edits demoting H's subtree in TEXT and OUTLINE."
  (map (lambda (x) (list (org-heading-start x) (org-heading-start x) "*"))
       (subtree-headings outline h)))

(define (sibling-headings outline h)
  (let* ((hs (org-outline-headings outline))
         (n (vector-length hs))
         (p (org-heading-parent h))
         (lvl (org-heading-level h)))
    (let loop ((i 0) (acc '()))
      (if (>= i n)
          (reverse acc)
          (let ((x (vector-ref hs i)))
            (if (and (eqv? (org-heading-parent x) p) (= (org-heading-level x) lvl))
                (loop (+ i 1) (cons x acc))
                (loop (+ i 1) acc)))))))

(define (swap-adjacent-subtrees text a b)
  ;; A immediately precedes B. Handle case where B reaches EOF without trailing newline.
  (let* ((a-start (org-heading-start a))
         (a-end (org-heading-subtree-end a))
         (b-start (org-heading-start b))
         (b-end (org-heading-subtree-end b))
         (a-text (org-slice-bytes text a-start a-end))
         (b-text (org-slice-bytes text b-start b-end))
         (b-ends-nl? (and (> (string-length b-text) 0)
                          (char=? (string-ref b-text (- (string-length b-text) 1)) #\newline))))
    (if b-ends-nl?
        (list (list a-start b-end (string-append b-text a-text)))
        ;; B had no trailing newline; move the trailing newline from A to B.
        (let ((a-trim (if (and (> (string-length a-text) 0)
                               (char=? (string-ref a-text (- (string-length a-text) 1)) #\newline))
                          (substring a-text 0 (- (string-length a-text) 1))
                          a-text)))
          (list (list a-start b-end (string-append b-text "\n" a-trim)))))))

(define (org-edit-move-subtree-up text outline h)
  "Return edits swapping H's subtree up in TEXT and OUTLINE."
  (let* ((sibs (sibling-headings outline h))
         (pos (list-index (lambda (x) (= (org-heading-index x) (org-heading-index h))) sibs)))
    (if (and pos (> pos 0))
        (swap-adjacent-subtrees text (list-ref sibs (- pos 1)) h)
        '())))

(define (org-edit-move-subtree-down text outline h)
  "Return edits swapping H's subtree down in TEXT and OUTLINE."
  (let* ((sibs (sibling-headings outline h))
         (pos (list-index (lambda (x) (= (org-heading-index x) (org-heading-index h))) sibs)))
    (if (and pos (< (+ pos 1) (length sibs)))
        (swap-adjacent-subtrees text h (list-ref sibs (+ pos 1)))
        '())))

(define (org-edit-insert-heading text outline byte-pos #:respect-content? [respect-content? #f]
                                 #:todo [todo #f])
  "Return (edits . new-point) inserting a heading at BYTE-POS.
Uses TEXT and OUTLINE; with RESPECT-CONTENT?, inserts after the
enclosing subtree. Optional TODO sets the new heading's keyword."
  (let* ((h (org-heading-at outline byte-pos))
         (lvl (if h (org-heading-level h) 1))
         (prefix (string-append (make-string lvl #\*) " " (if todo (string-append todo " ") ""))))
    (if (and h respect-content?)
        (let* ((end (org-heading-subtree-end h))
               (needs-nl? (and (> end 0) (not (string=? (org-slice-bytes text (- end 1) end) "\n"))))
               (ins (string-append (if needs-nl? "\n" "") prefix "\n"))
               (pt (+ end (if needs-nl? 1 0) (org-utf8-length prefix))))
          (cons (list (list end end ins)) pt))
        ;; Insert at line end of byte-pos (or split line if inside title)
        (let* ((len (org-utf8-length text))
               (pos (min byte-pos len))
               (at-bol? (or (= pos 0) (string=? (org-slice-bytes text (- pos 1) pos) "\n")))
               (ins (if at-bol? (string-append prefix "\n") (string-append "\n" prefix)))
               (pt (+ pos (if at-bol? (org-utf8-length prefix) (+ 1 (org-utf8-length prefix))))))
          (cons (list (list pos pos ins)) pt)))))

;; ----- Checkboxes and statistics cookies -----

(define (format-cookie kind done total)
  (if (eq? kind 'ratio)
      (string-append "[" (number->string done) "/" (number->string total) "]")
      (string-append "[" (number->string (if (= total 0) 0 (quotient (* 100 done) total))) "%]")))

(define (org-edit-toggle-checkbox text outline h byte-pos)
  "Return edits toggling the checkbox at BYTE-POS in H.
Uses TEXT and OUTLINE, and updates statistics cookies on H's headline."
  (let* ((elems (org-parse-section text (org-heading-body-start h) (org-heading-section-end outline h)))
         (target (find (lambda (e) (and (eq? (org-element-kind e) 'list-item)
                                        (<= (org-element-start e) byte-pos)
                                        (< byte-pos (org-element-end e))))
                       elems)))
    (if (not target)
        '()
        (let* ((item (org-element-data target))
               (cb-start (list-ref item 4))
               (cb-end (list-ref item 5))
               (cb-state (list-ref item 6)))
          (if (not cb-start)
              '()
              (let* ((new-state (if (eq? cb-state 'on) 'off 'on))
                     (new-cb (if (eq? new-state 'on) "[X]" "[ ]"))
                     (cb-edit (list cb-start cb-end new-cb))
                     ;; Count top-level checkboxes in this section (post-toggle) to update headline cookies.
                     (cb-items (filter (lambda (e) (and (eq? (org-element-kind e) 'list-item)
                                                        (list-ref (org-element-data e) 4)))
                                       elems))
                     (min-indent (if (null? cb-items) 0
                                     (apply min (map (lambda (e) (car (org-element-data e))) cb-items))))
                     (top-cbs (filter (lambda (e) (= (car (org-element-data e)) min-indent)) cb-items))
                     (total (length top-cbs))
                     (done-count (length (filter (lambda (e)
                                                   (if (eq? e target)
                                                       (eq? new-state 'on)
                                                       (eq? (list-ref (org-element-data e) 6) 'on)))
                                                 top-cbs)))
                     (headline-inlines (org-parse-inline text (org-heading-start h) (org-heading-line-end h)))
                     (cookie-edits
                      (filter-map (lambda (in)
                                    (and (eq? (org-inline-kind in) 'cookie)
                                         (list (org-inline-start in) (org-inline-end in)
                                               (format-cookie (car (org-inline-data in)) done-count total))))
                                  headline-inlines)))
                (normalize-edits (cons cb-edit cookie-edits))))))))

;; ----- Context-sensitive timestamp shift, list continuation, table alignment, relevel -----

(define (org-edit-shift-timestamp-at text byte-pos delta-days)
  "Shift the Org timestamp at BYTE-POS in TEXT by DELTA-DAYS."
  (let* ((total (org-utf8-length text))
         (bol (let loop ((p (min byte-pos total)))
                (cond ((<= p 0) 0)
                      ((string=? (org-slice-bytes text (- p 1) p) "\n") p)
                      (else (loop (- p 1))))))
         (eol (let loop ((p (min byte-pos total)))
                (cond ((>= p total) total)
                      ((string=? (org-slice-bytes text p (+ p 1)) "\n") p)
                      (else (loop (+ p 1))))))
         (inlines (org-parse-inline text bol eol))
         (hit (find (lambda (in)
                      (and (memq (org-inline-kind in) '(timestamp-active timestamp-inactive))
                           (<= (org-inline-start in) byte-pos)
                           (<= byte-pos (org-inline-end in))))
                    inlines)))
    (if (not hit)
        '()
        (let ((shifted (org-shift-timestamp-days (org-inline-data hit) delta-days)))
          (list (list (org-inline-start hit) (org-inline-end hit) (format-org-timestamp shifted)))))))

(define (org-edit-insert-list-item text outline byte-pos #:checkbox? [checkbox? #f])
  "Insert a sibling list item at BYTE-POS, or return #f.
Returns `(edits . point)` using TEXT and OUTLINE; CHECKBOX? adds `[ ] `."
  (let* ((h (org-heading-at outline byte-pos))
         (from (if h (org-heading-body-start h) 0))
         (to (if h (org-heading-section-end outline h)
                 (let ((hs (org-outline-headings outline)))
                   (if (> (vector-length hs) 0)
                       (org-heading-start (vector-ref hs 0))
                       (org-utf8-length text)))))
         (elems (if (< from to) (org-parse-section text from to) '()))
         (target (find (lambda (e)
                         (and (eq? (org-element-kind e) 'list-item)
                              (<= (org-element-start e) byte-pos)
                              (<= byte-pos (org-element-end e))))
                       elems)))
    (if (not target)
        #f
        (let* ((data (org-element-data target))
               (indent (car data))
               (bullet (org-slice-bytes text (cadr data) (caddr data)))
               (has-cb? (and (list-ref data 4) #t))
               (next-bullet
                (let ((blen (string-length bullet)))
                  (if (and (>= blen 2)
                           (or (char=? (string-ref bullet (- blen 1)) #\.)
                               (char=? (string-ref bullet (- blen 1)) #\))))
                      (let ((n (string->number (substring bullet 0 (- blen 1)))))
                        (if (exact-integer? n)
                            (string-append (number->string (+ n 1)) (string (string-ref bullet (- blen 1))))
                            bullet))
                      bullet)))
               (prefix (string-append (make-string indent #\space)
                                      next-bullet " "
                                      (if (or checkbox? has-cb?) "[ ] " "")))
               (end (let ((rest (cdr (memq target elems))))
                      (let loop ((es rest) (cur-end (org-element-end target)))
                        (if (null? es)
                            cur-end
                            (let ((e (car es)))
                              (if (and (eq? (org-element-kind e) 'list-item)
                                       (> (car (org-element-data e)) indent))
                                  (loop (cdr es) (org-element-end e))
                                  cur-end))))))
               (needs-nl? (and (> end 0) (not (string=? (org-slice-bytes text (- end 1) end) "\n"))))
               (ins (string-append (if needs-nl? "\n" "") prefix "\n"))
               (pt (+ end (if needs-nl? 1 0) (org-utf8-length prefix))))
          (cons (list (list end end ins)) pt)))))

(define (parse-table-cells line)
  (let* ((t (string-trim line))
         (inner (if (string-prefix? "|" t) (substring t 1) t))
         (inner (if (string-suffix? "|" inner) (substring inner 0 (- (string-length inner) 1)) inner)))
    (map string-trim (string-split inner #\|))))

(define (table-rule-line? line)
  (let ((t (string-trim line)))
    (and (string-prefix? "|-" t) #t)))

(define (org-edit-align-table text outline byte-pos)
  "Align the Org table at BYTE-POS in TEXT / OUTLINE, returning edits."
  (let* ((h (org-heading-at outline byte-pos))
         (from (if h (org-heading-body-start h) 0))
         (to (if h (org-heading-section-end outline h)
                 (let ((hs (org-outline-headings outline)))
                   (if (> (vector-length hs) 0)
                       (org-heading-start (vector-ref hs 0))
                       (org-utf8-length text)))))
         (elems (if (< from to) (org-parse-section text from to) '()))
         (tbl (find (lambda (e)
                      (and (eq? (org-element-kind e) 'table)
                           (<= (org-element-start e) byte-pos)
                           (<= byte-pos (org-element-end e))))
                    elems)))
    (if (not tbl)
        '()
        (let* ((tstart (org-element-start tbl))
               (tend (org-element-end tbl))
               (raw (org-slice-bytes text tstart tend))
               (trailing-nl? (string-suffix? "\n" raw))
               (lines (let ((ls (string-split raw #\newline)))
                        (if (and trailing-nl? (pair? ls) (string=? (car (reverse ls)) ""))
                            (reverse (cdr (reverse ls)))
                            ls)))
               (indent (let ((first (car lines)))
                         (let loop ((i 0))
                           (if (and (< i (string-length first)) (char=? (string-ref first i) #\space))
                               (loop (+ i 1))
                               (make-string i #\space)))))
               (parsed (map (lambda (l) (if (table-rule-line? l) 'rule (parse-table-cells l))) lines))
               (ncols (fold (lambda (r acc) (if (eq? r 'rule) acc (max acc (length r)))) 0 parsed)))
          (if (= ncols 0)
              '()
              (let ((widths (make-vector ncols 1)))
                (for-each
                 (lambda (r)
                   (unless (eq? r 'rule)
                     (let loop ((cs r) (c 0))
                       (when (pair? cs)
                         (vector-set! widths c (max (vector-ref widths c) (string-length (car cs))))
                         (loop (cdr cs) (+ c 1))))))
                 parsed)
                (let* ((rule-str
                        (string-append indent "|"
                                       (let loop ((c 0) (acc '()))
                                         (if (>= c ncols)
                                             (string-join (reverse acc) "+")
                                             (loop (+ c 1) (cons (make-string (+ (vector-ref widths c) 2) #\-) acc))))
                                       "|"))
                       (out-lines
                        (map (lambda (r)
                               (if (eq? r 'rule)
                                   rule-str
                                   (string-append indent "| "
                                                  (let loop ((c 0) (cs r) (acc '()))
                                                    (if (>= c ncols)
                                                        (string-join (reverse acc) " | ")
                                                        (let* ((cell (if (pair? cs) (car cs) ""))
                                                               (w (vector-ref widths c))
                                                               (padded (string-append cell (make-string (max 0 (- w (string-length cell))) #\space))))
                                                          (loop (+ c 1) (if (pair? cs) (cdr cs) '()) (cons padded acc)))))
                                                  " |")))
                             parsed))
                       (aligned (string-append (string-join out-lines "\n") (if trailing-nl? "\n" ""))))
                  (if (string=? aligned raw)
                      '()
                      (list (list tstart tend aligned))))))))))

(define (org-edit-relevel-subtree subtree-text delta)
  "Adjust all heading star counts in SUBTREE-TEXT by DELTA (clamped >= 1)."
  (if (= delta 0)
      subtree-text
      (let* ((trailing-nl? (string-suffix? "\n" subtree-text))
             (lines (string-split subtree-text #\newline))
             (lines (if (and trailing-nl? (pair? lines) (string=? (car (reverse lines)) ""))
                        (reverse (cdr (reverse lines)))
                        lines))
             (out (map (lambda (l)
                         (let ((n (string-length l)))
                           (if (and (> n 1) (char=? (string-ref l 0) #\*))
                               (let loop ((i 0))
                                 (cond
                                   ((and (< i n) (char=? (string-ref l i) #\*)) (loop (+ i 1)))
                                   ((and (< i n) (char=? (string-ref l i) #\space))
                                    (string-append (make-string (max 1 (+ i delta)) #\*) (substring l i)))
                                   (else l)))
                               l)))
                       lines)))
        (string-append (string-join out "\n") (if trailing-nl? "\n" "")))))
