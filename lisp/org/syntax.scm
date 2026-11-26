;;; Three-tier byte-range-indexed Org parser.
;;;
;;; The source text (a rope or string) is authoritative. This module
;;; produces derived indexes of UTF-8 byte ranges at a revision:
;;;   Tier 1: File header (file properties, keywords, TODO specs, filetags)
;;;           and a flat vector of headings with parent/subtree extents.
;;;   Tier 2: Section elements (planning, drawers, blocks, lists, tables,
;;;           keywords, comments, rules, blanks, paragraphs) that tile the
;;;           section's byte range completely.
;;;   Tier 3: Inline objects (links, timestamps, cookies, emphasis,
;;;           footnotes) parsed on demand for visible ranges.

(require "date.scm")

(provide org-utf8-length org-slice-bytes org-char-index-at-byte
         org-outline? org-outline-headings org-outline-keywords
         org-outline-file-properties org-outline-file-properties-range
         org-outline-todo-keywords org-outline-done-keywords org-outline-todo-sequences org-outline-todo-specs
         org-outline-filetags org-outline-title org-outline-category
         org-heading? org-heading-index org-heading-level org-heading-parent
         org-heading-start org-heading-line-end org-heading-body-start org-heading-subtree-end
         org-heading-stars-range org-heading-keyword-range org-heading-priority-range
         org-heading-title-range org-heading-tags-range
         org-heading-keyword org-heading-priority org-heading-tags
         org-heading-title org-heading-all-tags org-heading-at org-heading-children
         org-heading-section-end org-heading-done? org-heading-open?
         org-parse-outline org-outline-update
         org-element? org-element-kind org-element-start org-element-end org-element-data
         org-parse-section org-parse-planning-line org-section-planning org-section-properties
         org-section-logbook org-section-property org-heading-property
         org-inline? org-inline-kind org-inline-start org-inline-end org-inline-data
         org-parse-inline)

;; ----- UTF-8 byte helpers -----

(define (char-utf8-len c)
  (let ((cp (char->integer c)))
    (cond ((< cp #x80) 1)
          ((< cp #x800) 2)
          ((< cp #x10000) 3)
          (else 4))))

(define %utf8-len-cache-str #f)
(define %utf8-len-cache-val 0)
(define %byte-cursor-str #f)
(define %byte-cursor-byte 0)
(define %byte-cursor-char 0)

(define (org-utf8-length s)
  "Return the length of string S in UTF-8 bytes."
  (if (and (> (string-length s) 64) (eq? s %utf8-len-cache-str))
      %utf8-len-cache-val
      (let ((n (bytevector-length (string->utf8 s))))
        (when (> (string-length s) 64)
          (set! %utf8-len-cache-str s)
          (set! %utf8-len-cache-val n))
        n)))

;; Internal: return (values char-index actual-byte) for TARGET-BYTE.
;; Uses a single-slot monotonic cursor cache for non-ASCII strings > 64 chars.
(define (org-char+byte-at-byte s target-byte)
  (let ((len (string-length s)))
    (cond
      ((<= target-byte 0) (values 0 0))
      ((= len (org-utf8-length s))
       (let ((c (min target-byte len))) (values c c)))
      (else
       (let* ((cacheable? (> len 64))
              (resume? (and cacheable? (eq? s %byte-cursor-str) (>= target-byte %byte-cursor-byte)))
              (start-i (if resume? %byte-cursor-char 0))
              (start-b (if resume? %byte-cursor-byte 0)))
         (let loop ((i start-i) (b start-b))
           (if (or (>= b target-byte) (>= i len))
               (begin
                 (when cacheable?
                   (set! %byte-cursor-str s)
                   (set! %byte-cursor-byte b)
                   (set! %byte-cursor-char i))
                 (values i b))
               (loop (+ i 1) (+ b (char-utf8-len (string-ref s i)))))))))))

(define (org-char-index-at-byte s target-byte)
  "Return the character index in S at UTF-8 byte TARGET-BYTE."
  (let-values (((ci _b) (org-char+byte-at-byte s target-byte)))
    ci))

(define (org-slice-bytes s from to)
  "Return the substring of S spanning UTF-8 byte offsets FROM to TO."
  (let ((len (string-length s)))
    (cond
      ((>= from to) "")
      ((= len (org-utf8-length s))
       (substring s (max 0 (min from len)) (max 0 (min to len))))
      (else
       (let-values (((ci actual-b) (org-char+byte-at-byte s from)))
         ;; Use ACTUAL-B (never FROM) so non-boundary FROM never leaks chars past TO.
         (let loop ((j ci) (b actual-b))
           (if (or (>= b to) (>= j len))
               (substring s ci j)
               (let ((cb (char-utf8-len (string-ref s j))))
                 (if (> (+ b cb) to)
                     (substring s ci j)
                     (loop (+ j 1) (+ b cb)))))))))))

;; Compute UTF-8 byte offset of char index TARGET starting from char index CI at byte B.
(define (advance-bytes s ci b target)
  (let loop ((i ci) (byte b))
    (if (>= i target)
        byte
        (loop (+ i 1) (+ byte (char-utf8-len (string-ref s i)))))))

(define (blank-char? c) (or (char=? c #\space) (char=? c #\tab)))

(define (skip-blanks-str s i end)
  (let loop ((j i))
    (if (and (< j end) (blank-char? (string-ref s j))) (loop (+ j 1)) j)))

(define (rskip-blanks-str s start end)
  (let loop ((j end))
    (if (and (> j start) (blank-char? (string-ref s (- j 1)))) (loop (- j 1)) j)))

(define (find-line-end s i len)
  (let loop ((j i))
    (if (or (>= j len) (char=? (string-ref s j) #\newline)) j (loop (+ j 1)))))

(define (count-stars-at s i end)
  (let loop ((j i))
    (if (and (< j end) (char=? (string-ref s j) #\*)) (loop (+ j 1)) (- j i))))

;; ----- Records -----

(define-record-type org-outline
  (make-org-outline headings keywords file-properties props-range
                    todo-keywords done-keywords todo-sequences todo-specs filetags)
  org-outline?
  (headings org-outline-headings)
  (keywords org-outline-keywords)
  (file-properties org-outline-file-properties)
  (props-range org-outline-file-properties-range)
  (todo-keywords org-outline-todo-keywords)
  (done-keywords org-outline-done-keywords)
  (todo-sequences org-outline-todo-sequences)
  (todo-specs org-outline-todo-specs)
  (filetags org-outline-filetags))

(define-record-type org-heading
  (make-org-heading index level parent start line-end body-start subtree-end
                    stars-range keyword-range priority-range title-range tags-range
                    keyword priority tags)
  org-heading?
  (index org-heading-index set-org-heading-index!)
  (level org-heading-level)
  (parent org-heading-parent set-org-heading-parent!)
  (start org-heading-start set-org-heading-start!)
  (line-end org-heading-line-end set-org-heading-line-end!)
  (body-start org-heading-body-start set-org-heading-body-start!)
  (subtree-end org-heading-subtree-end set-org-heading-subtree-end!)
  (stars-range org-heading-stars-range set-org-heading-stars-range!)
  (keyword-range org-heading-keyword-range set-org-heading-keyword-range!)
  (priority-range org-heading-priority-range set-org-heading-priority-range!)
  (title-range org-heading-title-range set-org-heading-title-range!)
  (tags-range org-heading-tags-range set-org-heading-tags-range!)
  (keyword org-heading-keyword)
  (priority org-heading-priority)
  (tags org-heading-tags))

(define-record-type org-element
  (make-org-element kind start end data)
  org-element?
  (kind org-element-kind)
  (start org-element-start)
  (end org-element-end)
  (data org-element-data))

(define-record-type org-inline
  (make-org-inline kind start end data)
  org-inline?
  (kind org-inline-kind)
  (start org-inline-start)
  (end org-inline-end)
  (data org-inline-data))

;; ----- TODO keyword spec parsing -----

;; Parse a token like "WAIT(w@/!)" -> (keyword shortcut enter-log leave-log)
;; where enter-log/leave-log is 'note, 'time, or #f.
(define (parse-todo-token tok)
  (let ((lp (string-index tok #\()))
    (if (not lp)
        (list tok #f #f #f)
        (let* ((kw (substring tok 0 lp))
               (rp (string-index tok #\)))
               (spec (if (and rp (> rp (+ lp 1))) (substring tok (+ lp 1) rp) ""))
               (slen (string-length spec))
               (shortcut (and (> slen 0)
                              (not (memv (string-ref spec 0) '(#\! #\@ #\/)))
                              (string-ref spec 0)))
               (rest (if shortcut (substring spec 1) spec))
               (slash (string-index rest #\/))
               (parse-log (lambda (s)
                            (cond ((string-index s #\@) 'note)
                                  ((string-index s #\!) 'time)
                                  (else #f))))
               (enter-log (parse-log (if slash (substring rest 0 slash) rest)))
               (leave-log (if slash (parse-log (substring rest (+ slash 1))) #f)))
          (list kw shortcut enter-log leave-log)))))

(define (parse-todo-directive val)
  ;; "TODO(t) NEXT(n!) | DONE(d!) CANCELLED(c@)"
  ;; -> (values todo-list done-list specs)
  (let* ((words (string-split (string-trim val)))
         (bar (list-index (lambda (w) (string=? w "|")) words))
         (raw-todo (if bar (take words bar)
                       (if (null? words) '() (reverse (cdr (reverse words))))))
         (raw-done (if bar (drop words (+ bar 1))
                       (if (null? words) '() (list (last words)))))
         (todo-specs (map parse-todo-token raw-todo))
         (done-specs (map parse-todo-token raw-done)))
    (values (map car todo-specs)
            (map car done-specs)
            (append todo-specs done-specs))))

(define (parse-filetags-value val)
  (let ((t (string-trim val)))
    (if (string-null? t)
        '()
        (append-map (lambda (tok)
                      (if (and (> (string-length tok) 1)
                               (char=? (string-ref tok 0) #\:)
                               (char=? (string-ref tok (- (string-length tok) 1)) #\:))
                          (filter (lambda (x) (not (string-null? x))) (string-split tok ":"))
                          (list tok)))
                    (string-split t)))))

;; ----- Tier 1: Outline parsing -----

(define (tag-char? c)
  (or (char-alphabetic? c) (char-numeric? c) (memv c '(#\_ #\@ #\# #\%))))

;; Parse trailing ":tag1:tag2:" at end of headline (char indices).
;; Returns (values tags-char-start tags-char-end tags-list) or (values #f #f '()).
(define (parse-headline-tags s start end)
  (let ((e (rskip-blanks-str s start end)))
    (if (and (> e (+ start 1)) (char=? (string-ref s (- e 1)) #\:))
        ;; Scan backwards for whitespace or start.
        (let loop ((j (- e 1)))
          (cond
            ((<= j start)
             (if (and (= j start) (char=? (string-ref s start) #\:))
                 (let ((tok (substring s start e)))
                   (let ((tags (filter (lambda (x) (not (string-null? x))) (string-split tok ":"))))
                     (if (pair? tags) (values start e tags) (values #f #f '()))))
                 (values #f #f '())))
            ((blank-char? (string-ref s j))
             (let ((tstart (+ j 1)))
               (if (and (< tstart (- e 1)) (char=? (string-ref s tstart) #\:))
                   (let* ((tok (substring s tstart e))
                          (tags (filter (lambda (x) (not (string-null? x))) (string-split tok ":"))))
                     (if (pair? tags) (values tstart e tags) (values #f #f '())))
                   (values #f #f '()))))
            ((or (char=? (string-ref s j) #\:) (tag-char? (string-ref s j)))
             (loop (- j 1)))
            (else (values #f #f '()))))
        (values #f #f '()))))

(define (parse-headline-at s line-ci line-byte raw-line-end-ci raw-line-end-byte body-start-byte todo done)
  (let* ((crlf? (and (> raw-line-end-ci line-ci) (char=? (string-ref s (- raw-line-end-ci 1)) #\return)))
         (line-end-ci (if crlf? (- raw-line-end-ci 1) raw-line-end-ci))
         (line-end-byte (if crlf? (- raw-line-end-byte 1) raw-line-end-byte))
         (stars (count-stars-at s line-ci line-end-ci))
         (stars-end-byte (+ line-byte stars))
         (after-stars-ci (skip-blanks-str s (+ line-ci stars) line-end-ci))
         (after-stars-byte (advance-bytes s (+ line-ci stars) stars-end-byte after-stars-ci)))
    ;; Parse tags from the right first so title-end excludes tags.
    (let-values (((tags-ci tags-end-ci tags) (parse-headline-tags s after-stars-ci line-end-ci)))
      (let* ((content-end-ci (if tags-ci (rskip-blanks-str s after-stars-ci tags-ci)
                                 (rskip-blanks-str s after-stars-ci line-end-ci)))
             ;; Check TODO keyword at after-stars-ci
             (w1-end-ci (let loop ((j after-stars-ci))
                          (if (and (< j content-end-ci) (not (blank-char? (string-ref s j))))
                              (loop (+ j 1)) j)))
             (w1 (if (> w1-end-ci after-stars-ci) (substring s after-stars-ci w1-end-ci) #f))
             (is-kw (and w1 (or (member w1 todo) (member w1 done)) #t))
             (kw (if is-kw w1 #f))
             (kw-end-byte (if is-kw (advance-bytes s after-stars-ci after-stars-byte w1-end-ci) after-stars-byte))
             (kw-range (if is-kw (cons after-stars-byte kw-end-byte) #f))
             (p1-ci (if is-kw (skip-blanks-str s w1-end-ci content-end-ci) after-stars-ci))
             (p1-byte (if is-kw (advance-bytes s w1-end-ci kw-end-byte p1-ci) after-stars-byte))
             ;; Check priority [#A] (must be ASCII letter A-Z / a-z so byte width is 4)
             (has-prio (and (>= (- content-end-ci p1-ci) 4)
                            (char=? (string-ref s p1-ci) #\[)
                            (char=? (string-ref s (+ p1-ci 1)) #\#)
                            (let ((pc (string-ref s (+ p1-ci 2))))
                              (or (char<=? #\A pc #\Z) (char<=? #\a pc #\z)))
                            (char=? (string-ref s (+ p1-ci 3)) #\])
                            (or (= (+ p1-ci 4) content-end-ci)
                                (blank-char? (string-ref s (+ p1-ci 4))))))
             (prio (if has-prio (string-ref s (+ p1-ci 2)) #f))
             (prio-end-byte (if has-prio (+ p1-byte 4) p1-byte))
             (prio-range (if has-prio (cons p1-byte prio-end-byte) #f))
             (title-ci (if has-prio (skip-blanks-str s (+ p1-ci 4) content-end-ci) p1-ci))
             (title-byte (if has-prio (advance-bytes s (+ p1-ci 4) prio-end-byte title-ci) p1-byte))
             (title-end-byte (advance-bytes s title-ci title-byte content-end-ci))
             (title-range (cons title-byte title-end-byte))
             (tags-range (if tags-ci
                             (let ((tb (advance-bytes s content-end-ci title-end-byte tags-ci)))
                               (cons tb (advance-bytes s tags-ci tb tags-end-ci)))
                             #f)))
        (make-org-heading 0 stars #f line-byte line-end-byte body-start-byte body-start-byte
                          (cons line-byte stars-end-byte)
                          kw-range prio-range title-range tags-range
                          kw prio tags)))))

;; Link parent indices and subtree-end byte offsets on a vector of headings.
(define (link-heading-tree! vec total-bytes)
  (let ((n (vector-length vec)))
    ;; Parent stack of indices
    (let loop ((i 0) (stack '()))
      (when (< i n)
        (let* ((h (vector-ref vec i))
               (lvl (org-heading-level h)))
          (set-org-heading-index! h i)
          ;; Pop headings with level >= lvl; their subtree ends at h's start.
          (let pop ((st stack))
            (if (and (pair? st) (>= (org-heading-level (vector-ref vec (car st))) lvl))
                (begin
                  (set-org-heading-subtree-end! (vector-ref vec (car st)) (org-heading-start h))
                  (pop (cdr st)))
                (begin
                  (set-org-heading-parent! h (if (pair? st) (car st) #f))
                  (loop (+ i 1) (cons i st))))))))
    ;; Any headings still on the right edge end at EOF.
    ;; Close remaining subtrees by scanning backward or using stack:
    (let loop ((i 0) (stack '()))
      (if (< i n)
          (let* ((h (vector-ref vec i))
                 (lvl (org-heading-level h)))
            (let pop ((st stack))
              (if (and (pair? st) (>= (org-heading-level (vector-ref vec (car st))) lvl))
                  (pop (cdr st))
                  (loop (+ i 1) (cons i st)))))
          (for-each (lambda (idx) (set-org-heading-subtree-end! (vector-ref vec idx) total-bytes)) stack)))
    vec))

(define (parse-property-line line-str line-byte)
  ;; ":KEY: value" -> (key key-start key-end val-start val-end value) or #f
  (let* ((len (string-length line-str))
         (ci (skip-blanks-str line-str 0 len)))
    (and (< (+ ci 2) len)
         (char=? (string-ref line-str ci) #\:)
         ;; Find the closing colon of the property key token (supports :header-args:scheme:).
         (let ((close (let loop ((j (+ ci 1)) (last-colon #f))
                        (cond ((>= j len) last-colon)
                              ((blank-char? (string-ref line-str j)) last-colon)
                              ((char=? (string-ref line-str j) #\:) (loop (+ j 1) j))
                              (else (loop (+ j 1) last-colon))))))
           (and close (> close (+ ci 1))
                (let* ((key (substring line-str (+ ci 1) close))
                       (key-start (+ line-byte (advance-bytes line-str 0 0 (+ ci 1))))
                       (key-end (+ line-byte (advance-bytes line-str 0 0 close)))
                       (vstart-ci (skip-blanks-str line-str (+ close 1) len))
                       (vend-ci (rskip-blanks-str line-str vstart-ci len))
                       (val-start (+ line-byte (advance-bytes line-str 0 0 vstart-ci)))
                       (val-end (+ line-byte (advance-bytes line-str 0 0 vend-ci)))
                       (val (substring line-str vstart-ci vend-ci)))
                  (list key key-start key-end val-start val-end val)))))))

(define (heading-starts-at? text i end)
  (let ((n (count-stars-at text i end)))
    (and (> n 0) (or (= (+ i n) end) (char=? (string-ref text (+ i n)) #\space)))))

(define (org-parse-outline text #:todo-keywords [default-todo '("TODO")]
                           #:done-keywords [default-done '("DONE")])
  "Parse Org TEXT into an `org-outline` of UTF-8 byte ranges.
DEFAULT-TODO and DEFAULT-DONE provide fallback TODO/DONE keyword lists
when the document contains no `#+TODO:` directives."
  (let* ((len (string-length text))
         (total-bytes (org-utf8-length text)))
    ;; Pass 1: scan preamble (before first heading) for file-level :PROPERTIES: and #+KEYWORDS.
    (let preamble-loop ((i 0) (b 0) (in-props #f) (props-start #f) (props-acc '())
                        (file-props '()) (file-props-range #f) (seen-non-comment #f)
                        (keywords '()) (seqs-acc '()) (spec-acc '()) (filetags '())
                        (in-block #f))
      (if (>= i len)
          (let* ((seqs (if (null? seqs-acc) (list (cons default-todo default-done)) (reverse seqs-acc)))
                 (todo (delete-duplicates (append-map car seqs)))
                 (done (delete-duplicates (append-map cdr seqs))))
            (make-org-outline (vector) (reverse keywords) file-props file-props-range
                              todo done seqs (reverse spec-acc) (delete-duplicates (reverse filetags))))
          (let* ((le (find-line-end text i len))
                 (le-byte (advance-bytes text i b le))
                 (next-i (if (< le len) (+ le 1) le))
                 (next-b (if (< le len) (+ le-byte 1) le-byte)))
            (if (heading-starts-at? text i le)
                ;; Reached first heading: finalize preamble and scan all headings.
                (let* ((seqs (if (null? seqs-acc) (list (cons default-todo default-done)) (reverse seqs-acc)))
                       (todo (delete-duplicates (append-map car seqs)))
                       (done (delete-duplicates (append-map cdr seqs)))
                       (headings-rev
                        (let hloop ((hi i) (hb b) (acc '()))
                          (if (>= hi len)
                              acc
                              (let* ((hle (find-line-end text hi len))
                                     (hle-byte (advance-bytes text hi hb hle))
                                     (hnext-i (if (< hle len) (+ hle 1) hle))
                                     (hnext-b (if (< hle len) (+ hle-byte 1) hle-byte)))
                                (if (heading-starts-at? text hi hle)
                                    (hloop hnext-i hnext-b
                                           (cons (parse-headline-at text hi hb hle hle-byte hnext-b todo done) acc))
                                    (hloop hnext-i hnext-b acc))))))
                       (vec (link-heading-tree! (list->vector (reverse headings-rev)) total-bytes)))
                  (make-org-outline vec (reverse keywords) file-props file-props-range
                                    todo done seqs (reverse spec-acc) (delete-duplicates (reverse filetags))))
                ;; Still in preamble
                (let* ((line (substring text i le))
                       (trimmed (string-trim line))
                       (trimmed-lc (string-downcase trimmed)))
                  (cond
                    (in-block
                     (preamble-loop next-i next-b #f #f '()
                                    file-props file-props-range #t
                                    keywords seqs-acc spec-acc filetags
                                    (if (string=? trimmed-lc in-block) #f in-block)))
                    (in-props
                     (if (string-ci=? trimmed ":END:")
                         (preamble-loop next-i next-b #f #f '()
                                        (reverse props-acc) (cons props-start next-b) #t
                                        keywords seqs-acc spec-acc filetags #f)
                         (let ((p (parse-property-line line b)))
                           (preamble-loop next-i next-b #t props-start (if p (cons p props-acc) props-acc)
                                          file-props file-props-range #t
                                          keywords seqs-acc spec-acc filetags #f))))
                    ((and (not seen-non-comment) (not file-props-range) (string-ci=? trimmed ":PROPERTIES:"))
                     (preamble-loop next-i next-b #t b '()
                                    file-props file-props-range #t
                                    keywords seqs-acc spec-acc filetags #f))
                    ((string-prefix? "#+begin_" trimmed-lc)
                     (let* ((rest (substring trimmed-lc 8))
                            (sp (let s ((j 0))
                                  (if (or (>= j (string-length rest)) (char-whitespace? (string-ref rest j))) j (s (+ j 1)))))
                            (btype (substring rest 0 sp)))
                       (preamble-loop next-i next-b #f #f '()
                                      file-props file-props-range #t
                                      keywords seqs-acc spec-acc filetags
                                      (if (string-null? btype) #f (string-append "#+end_" btype)))))
                    ((and (string-prefix? "#+" line) (string-index line #\:))
                     => (lambda (colon)
                          (let* ((k (string-upcase (substring line 2 colon)))
                                 (v (string-trim (substring line (+ colon 1)))))
                            (cond
                              ((or (string=? k "TODO") (string=? k "SEQ_TODO") (string=? k "TYP_TODO"))
                               (let-values (((td dn sp) (parse-todo-directive v)))
                                 (preamble-loop next-i next-b #f #f '()
                                                file-props file-props-range #t
                                                (cons (cons k v) keywords)
                                                (cons (cons td dn) seqs-acc)
                                                (append (reverse sp) spec-acc)
                                                filetags #f)))
                              ((string=? k "FILETAGS")
                               (preamble-loop next-i next-b #f #f '()
                                              file-props file-props-range #t
                                              (cons (cons k v) keywords)
                                              seqs-acc spec-acc
                                              (append (reverse (parse-filetags-value v)) filetags) #f))
                              (else
                               (preamble-loop next-i next-b #f #f '()
                                              file-props file-props-range #t
                                              (cons (cons k v) keywords)
                                              seqs-acc spec-acc filetags #f))))))
                    ((and (not (string-null? trimmed)) (char=? (string-ref trimmed 0) #\#))
                     (preamble-loop next-i next-b #f #f '()
                                    file-props file-props-range seen-non-comment
                                    keywords seqs-acc spec-acc filetags #f))
                    (else
                     (preamble-loop next-i next-b #f #f '()
                                    file-props file-props-range #t
                                    keywords seqs-acc spec-acc filetags #f))))))))))

(define (shift-range r delta)
  (and r (cons (+ (car r) delta) (+ (cdr r) delta))))

(define (shift-heading h delta)
  (make-org-heading (org-heading-index h)
                    (org-heading-level h)
                    (org-heading-parent h)
                    (+ (org-heading-start h) delta)
                    (+ (org-heading-line-end h) delta)
                    (+ (org-heading-body-start h) delta)
                    (+ (org-heading-subtree-end h) delta)
                    (shift-range (org-heading-stars-range h) delta)
                    (shift-range (org-heading-keyword-range h) delta)
                    (shift-range (org-heading-priority-range h) delta)
                    (shift-range (org-heading-title-range h) delta)
                    (shift-range (org-heading-tags-range h) delta)
                    (org-heading-keyword h)
                    (org-heading-priority h)
                    (org-heading-tags h)))

(define (org-outline-update outline new-text edit-from edit-to replacement
                            #:todo-keywords [default-todo '("TODO")]
                            #:done-keywords [default-done '("DONE")])
  "Incrementally update OUTLINE for NEW-TEXT after an edit.
REPLACEMENT replaces [EDIT-FROM, EDIT-TO). DEFAULT-TODO and
DEFAULT-DONE are fallback keyword lists if the preamble is reparsed."
  (let* ((headings (org-outline-headings outline))
         (n (vector-length headings))
         (first-body (if (> n 0) (org-heading-body-start (vector-ref headings 0)) (org-utf8-length new-text))))
    ;; If edit touches the preamble or first heading line (which could remove the first heading), reparse full document.
    (if (<= edit-from first-body)
        (org-parse-outline new-text #:todo-keywords default-todo #:done-keywords default-done)
        (let* ((delta (- (org-utf8-length replacement) (- edit-to edit-from)))
               (todo (org-outline-todo-keywords outline))
               (done (org-outline-done-keywords outline))
               (total-bytes (org-utf8-length new-text))
               ;; Keep headings whose entire headline line ends strictly before edit-from.
               ;; Find first heading whose body-start > edit-from (or start >= edit-from).
               (keep-before
                (let loop ((lo 0) (hi n))
                  (if (< lo hi)
                      (let* ((mid (quotient (+ lo hi) 2))
                             (h (vector-ref headings mid)))
                        (if (< (org-heading-body-start h) edit-from)
                            (loop (+ mid 1) hi)
                            (loop lo mid)))
                      lo)))
               ;; Keep headings starting strictly after edit-to (and whose line wasn't merged with edit-to).
               (keep-after
                (let loop ((lo keep-before) (hi n))
                  (if (< lo hi)
                      (let* ((mid (quotient (+ lo hi) 2))
                             (h (vector-ref headings mid)))
                        (if (<= (org-heading-start h) edit-to)
                            (loop (+ mid 1) hi)
                            (loop lo mid)))
                      lo)))
               ;; Byte range in new-text to rescan:
               ;; start at start of line containing (if keep-before < n, heading keep-before's start else edit-from),
               ;; backed up to line start.
               (scan-start-byte
                (let* ((base (if (< keep-before n)
                                 (min edit-from (org-heading-start (vector-ref headings keep-before)))
                                 edit-from))
                       (ci (org-char-index-at-byte new-text base)))
                  (let back ((j ci) (b base))
                    (if (or (<= j 0) (char=? (string-ref new-text (- j 1)) #\newline))
                        b
                        (back (- j 1) (- b (char-utf8-len (string-ref new-text (- j 1)))))))))
               (scan-end-byte
                (if (< keep-after n)
                    (+ (org-heading-start (vector-ref headings keep-after)) delta)
                    total-bytes))
               ;; Make sure we didn't keep a heading in keep-before that starts >= scan-start-byte
               (keep-before
                (let loop ((k keep-before))
                  (if (and (> k 0) (>= (org-heading-start (vector-ref headings (- k 1))) scan-start-byte))
                      (loop (- k 1))
                      k)))
               ;; Rescan [scan-start-byte, scan-end-byte) in new-text
               (len (string-length new-text))
               (start-ci (org-char-index-at-byte new-text scan-start-byte))
               (rescanned
                (let hloop ((hi start-ci) (hb scan-start-byte) (acc '()))
                  (if (or (>= hi len) (>= hb scan-end-byte))
                      (reverse acc)
                      (let* ((hle (find-line-end new-text hi len))
                             (hle-byte (advance-bytes new-text hi hb hle))
                             (hnext-i (if (< hle len) (+ hle 1) hle))
                             (hnext-b (if (< hle len) (+ hle-byte 1) hle-byte)))
                        (if (heading-starts-at? new-text hi hle)
                            (hloop hnext-i hnext-b
                                   (cons (parse-headline-at new-text hi hb hle hle-byte hnext-b todo done) acc))
                            (hloop hnext-i hnext-b acc))))))
               (new-len (+ keep-before (length rescanned) (- n keep-after)))
               (vec (make-vector new-len)))
          (let copy-before ((i 0))
            (when (< i keep-before)
              (vector-set! vec i (shift-heading (vector-ref headings i) 0))
              (copy-before (+ i 1))))
          (let copy-mid ((i keep-before) (rs rescanned))
            (when (pair? rs)
              (vector-set! vec i (car rs))
              (copy-mid (+ i 1) (cdr rs))))
          (let copy-after ((dst (+ keep-before (length rescanned))) (src keep-after))
            (when (< src n)
              (vector-set! vec dst (shift-heading (vector-ref headings src) delta))
              (copy-after (+ dst 1) (+ src 1))))
          (link-heading-tree! vec total-bytes)
          (make-org-outline vec
                            (org-outline-keywords outline)
                            (org-outline-file-properties outline)
                            (org-outline-file-properties-range outline)
                            todo done
                            (org-outline-todo-sequences outline)
                            (org-outline-todo-specs outline)
                            (org-outline-filetags outline))))))

;; ----- Outline accessors -----

(define (org-outline-title outline)
  "Return the TITLE keyword of OUTLINE, or #f."
  (let ((e (assoc "TITLE" (org-outline-keywords outline)))) (and e (cdr e))))

(define (org-outline-category outline #:path [path #f])
  "Return the category of OUTLINE.
Uses CATEGORY keyword, else TITLE, else PATH basename."
  (let ((cat (assoc "CATEGORY" (org-outline-keywords outline))))
    (cond (cat (cdr cat))
          ((org-outline-title outline) => (lambda (t) t))
          (path
           (let* ((name (last (string-split path "/")))
                  (dot (string-index name #\.)))
             (if dot (substring name 0 dot) name)))
          (else "org"))))

(define (org-heading-title text h)
  "Return the title text of heading H sliced from TEXT."
  (let ((r (org-heading-title-range h)))
    (org-slice-bytes text (car r) (cdr r))))

(define (org-heading-at outline byte-pos)
  "Return the `org-heading` at BYTE-POS in OUTLINE, or #f."
  (let* ((hs (org-outline-headings outline))
         (n (vector-length hs)))
    (if (= n 0)
        #f
        (let loop ((lo 0) (hi n))
          (if (< lo hi)
              (let ((mid (quotient (+ lo hi) 2)))
                (if (<= (org-heading-start (vector-ref hs mid)) byte-pos)
                    (loop (+ mid 1) hi)
                    (loop lo mid)))
              (if (> lo 0) (vector-ref hs (- lo 1)) #f))))))

(define (org-heading-all-tags outline h)
  "Return all tags of heading H in OUTLINE.
Includes filetags and inherited ancestor tags."
  (let ((hs (org-outline-headings outline)))
    (let loop ((cur h) (acc '()))
      (if (not cur)
          (delete-duplicates (append (org-outline-filetags outline) acc))
          (loop (and (org-heading-parent cur) (vector-ref hs (org-heading-parent cur)))
                (append (org-heading-tags cur) acc))))))

(define (org-heading-children outline h)
  "Return the list of direct child `org-heading` records of H in OUTLINE."
  (let* ((hs (org-outline-headings outline))
         (n (vector-length hs))
         (idx (org-heading-index h))
         (end (org-heading-subtree-end h)))
    (let loop ((i (+ idx 1)) (acc '()))
      (if (or (>= i n) (>= (org-heading-start (vector-ref hs i)) end))
          (reverse acc)
          (let ((c (vector-ref hs i)))
            (if (eqv? (org-heading-parent c) idx)
                (loop (+ i 1) (cons c acc))
                (loop (+ i 1) acc)))))))

(define (org-heading-section-end outline h)
  "Return the exclusive byte end of H's own section in OUTLINE.
This is before H's first child or its subtree end."
  (let* ((hs (org-outline-headings outline))
         (next (+ (org-heading-index h) 1)))
    (if (and (< next (vector-length hs))
             (< (org-heading-start (vector-ref hs next)) (org-heading-subtree-end h)))
        (org-heading-start (vector-ref hs next))
        (org-heading-subtree-end h))))

(define (org-heading-done? outline h)
  "Return #t if heading H has a DONE keyword in OUTLINE."
  (and (org-heading-keyword h) (member (org-heading-keyword h) (org-outline-done-keywords outline)) #t))

(define (org-heading-open? outline h)
  "Return #t if heading H has an open TODO keyword in OUTLINE."
  (and (org-heading-keyword h) (member (org-heading-keyword h) (org-outline-todo-keywords outline)) #t))

;; ----- Tier 2: Section element parsing (tiling invariant) -----

(define planning-keywords '(("CLOSED:" . closed) ("DEADLINE:" . deadline) ("SCHEDULED:" . scheduled)))

(define (org-parse-planning-line line line-byte)
  "Parse an Org planning LINE starting at UTF-8 byte LINE-BYTE.
Return (entries . indent-bytes) where each entry is
(kind key-start key-end ts-start ts-end org-timestamp), or #f."
  (let* ((len (string-length line))
         (start-ci (skip-blanks-str line 0 len)))
    (and (< start-ci len)
         (let loop ((i start-ci) (acc '()))
           (let ((i (skip-blanks-str line i len)))
             (if (>= i len)
                 (and (pair? acc)
                      (cons (reverse acc) (advance-bytes line 0 0 start-ci)))
                 (let ((kw-end (let wloop ((j i))
                                 (if (and (< j len) (not (blank-char? (string-ref line j))))
                                     (wloop (+ j 1)) j))))
                   (let ((kw (assoc (substring line i kw-end) planning-keywords)))
                     (if (not kw)
                         #f
                         (let* ((ts-ci (skip-blanks-str line kw-end len))
                                (open (and (< ts-ci len) (string-ref line ts-ci)))
                                (close (cond ((eqv? open #\<) #\>)
                                             ((eqv? open #\[) #\])
                                             (else #f)))
                                (ts-end-ci (and close
                                                (let find-close ((j (+ ts-ci 1)))
                                                  (cond ((>= j len) #f)
                                                        ((char=? (string-ref line j) close) (+ j 1))
                                                        (else (find-close (+ j 1))))))))
                           ;; Check for date range --<...>
                           (let* ((ts-end-ci
                                   (if (and ts-end-ci
                                            (< (+ ts-end-ci 3) len)
                                            (string=? (substring line ts-end-ci (+ ts-end-ci 2)) "--")
                                            (eqv? (string-ref line (+ ts-end-ci 2)) open))
                                       (let find-close2 ((j (+ ts-end-ci 3)))
                                         (cond ((>= j len) ts-end-ci)
                                               ((char=? (string-ref line j) close) (+ j 1))
                                               (else (find-close2 (+ j 1)))))
                                       ts-end-ci))
                                  (ts (and ts-end-ci (parse-org-timestamp (substring line ts-ci ts-end-ci)))))
                             (if (not ts)
                                 #f
                                 (let* ((ks (+ line-byte (advance-bytes line 0 0 i)))
                                        (ke (+ line-byte (advance-bytes line 0 0 kw-end)))
                                        (ts-s (+ line-byte (advance-bytes line 0 0 ts-ci)))
                                        (ts-e (+ line-byte (advance-bytes line 0 0 ts-end-ci))))
                                   (loop ts-end-ci (cons (list (cdr kw) ks ke ts-s ts-e ts) acc)))))))))))))))

(define (drawer-start-name line)
  ;; ":NAME:" -> "NAME" or #f (not ":END:")
  (let* ((t (string-trim line))
         (len (string-length t)))
    (and (>= len 3)
         (char=? (string-ref t 0) #\:)
         (char=? (string-ref t (- len 1)) #\:)
         (not (string-ci=? t ":END:"))
         (let ((inner (substring t 1 (- len 1))))
           (and (let loop ((i 0))
                  (or (>= i (string-length inner))
                      (let ((c (string-ref inner i)))
                        (and (or (char-alphabetic? c) (char-numeric? c) (char=? c #\_) (char=? c #\-))
                             (loop (+ i 1))))))
                inner)))))

(define (block-begin-info line)
  ;; "#+begin_src scheme :tangle foo" -> (type-downcase rest-string) or #f
  (let* ((t (string-trim line))
         (len (string-length t)))
    (and (>= len 9)
         (string-ci=? (substring t 0 8) "#+begin_")
         (let* ((sp (let loop ((j 8))
                      (if (and (< j len) (not (blank-char? (string-ref t j)))) (loop (+ j 1)) j)))
                (type (string-downcase (substring t 8 sp)))
                (rest (string-trim (substring t sp len))))
           (and (not (string-null? type)) (cons type rest))))))

(define (block-end-matches? line type)
  (let* ((t (string-trim line))
         (expected (string-append "#+end_" type)))
    (and (= (string-length t) (string-length expected))
         (string-ci=? t expected))))

(define (parse-src-header rest)
  (if (string-null? rest)
      (cons "" "")
      (let* ((len (string-length rest))
             (sp (let loop ((j 0)) (if (and (< j len) (not (blank-char? (string-ref rest j)))) (loop (+ j 1)) j))))
        (cons (substring rest 0 sp) (string-trim (substring rest sp len))))))

(define (parse-list-item-line line line-byte)
  ;; Returns (indent bullet-start bullet-end ordered? cb-start cb-end cb-state content-start) or #f
  (let* ((len (string-length line))
         (i (skip-blanks-str line 0 len)))
    (and (< i len)
         (let* ((c (string-ref line i))
                (bullet
                 (cond
                   ;; Unordered: -, +, or * (only if indented, since unindented * is a heading)
                   ((or (char=? c #\-) (char=? c #\+) (and (> i 0) (char=? c #\*)))
                    (and (or (= (+ i 1) len) (blank-char? (string-ref line (+ i 1))))
                         (list (+ i 1) #f)))
                   ;; Ordered: digits followed by . or )
                   ((char-numeric? c)
                    (let loop ((j i))
                      (if (and (< j len) (char-numeric? (string-ref line j)))
                          (loop (+ j 1))
                          (and (< j len)
                               (memv (string-ref line j) '(#\. #\)))
                               (or (= (+ j 1) len) (blank-char? (string-ref line (+ j 1))))
                               (list (+ j 1) #t)))))
                   (else #f))))
           (and bullet
                (let* ((b-end-ci (car bullet))
                       (ordered? (cadr bullet))
                       (after-b (skip-blanks-str line b-end-ci len))
                       (has-cb (and (>= (- len after-b) 3)
                                    (char=? (string-ref line after-b) #\[)
                                    (memv (string-ref line (+ after-b 1)) '(#\space #\- #\X #\x))
                                    (char=? (string-ref line (+ after-b 2)) #\])
                                    (or (= (+ after-b 3) len) (blank-char? (string-ref line (+ after-b 3))))))
                       (cb-state (and has-cb
                                      (case (string-ref line (+ after-b 1))
                                        ((#\space) 'off)
                                        ((#\-) 'trans)
                                        (else 'on))))
                       (content-ci (if has-cb (skip-blanks-str line (+ after-b 3) len) after-b))
                       (b-start (+ line-byte (advance-bytes line 0 0 i)))
                       (b-end (+ line-byte (advance-bytes line 0 0 b-end-ci)))
                       (cb-start (and has-cb (+ line-byte (advance-bytes line 0 0 after-b))))
                       (cb-end (and has-cb (+ cb-start 3)))
                       (c-start (+ line-byte (advance-bytes line 0 0 content-ci))))
                  (list i b-start b-end ordered? cb-start cb-end cb-state c-start)))))))

(define (table-line? line)
  (let* ((len (string-length line))
         (i (skip-blanks-str line 0 len)))
    (and (< i len) (char=? (string-ref line i) #\|))))

(define (table-rule-line? line)
  (let* ((len (string-length line))
         (i (skip-blanks-str line 0 len)))
    (and (< (+ i 1) len)
         (char=? (string-ref line i) #\|)
         (char=? (string-ref line (+ i 1)) #\-))))

(define (hr-line? line)
  (let* ((t (string-trim line))
         (len (string-length t)))
    (and (>= len 5)
         (let loop ((i 0))
           (or (>= i len) (and (char=? (string-ref t i) #\-) (loop (+ i 1))))))))

(define (keyword-line-info line)
  (let* ((len (string-length line))
         (i (skip-blanks-str line 0 len)))
    (and (< (+ i 2) len)
         (char=? (string-ref line i) #\#)
         (char=? (string-ref line (+ i 1)) #\+)
         (let ((colon (let loop ((j (+ i 2)))
                        (cond ((>= j len) #f)
                              ((char=? (string-ref line j) #\:) j)
                              ((blank-char? (string-ref line j)) #f)
                              (else (loop (+ j 1)))))))
           (and colon (> colon (+ i 2))
                (cons (string-upcase (substring line (+ i 2) colon))
                      (string-trim (substring line (+ colon 1) len))))))))

(define (comment-line? line)
  (let* ((len (string-length line))
         (i (skip-blanks-str line 0 len)))
    (and (< i len)
         (char=? (string-ref line i) #\#)
         (or (= (+ i 1) len) (blank-char? (string-ref line (+ i 1)))))))

(define (blank-line? line)
  (= (skip-blanks-str line 0 (string-length line)) (string-length line)))

(define (string-index-from s ch start)
  (let ((idx (string-index (substring s start) ch)))
    (and idx (+ start idx))))

(define (string-contains-from s sub start)
  (let ((idx (string-contains (substring s start) sub)))
    (and idx (+ start idx))))

;; Parse a LOGBOOK state-change line:
;; - State "DONE"       from "TODO"       [2026-11-26 Thu 20:45]
(define (parse-logbook-line line line-byte)
  (let ((t (string-trim line)))
    (cond
      ((string-prefix? "- State \"" t)
       (let* ((q1 (string-index-from t #\" 9))
              (to-state (and q1 (substring t 9 q1)))
              (from-pos (and q1 (string-contains-from t "from \"" q1)))
              (q2 (and from-pos (string-index-from t #\" (+ from-pos 6))))
              (from-state (and q2 (substring t (+ from-pos 6) q2)))
              (lb (string-index-from t #\[ (or q2 q1 0)))
              (rb (and lb (string-index-from t #\] lb)))
              (ts (and rb (parse-org-timestamp (substring t lb (+ rb 1))))))
         (and to-state ts (list 'state to-state from-state ts line-byte))))
      ((string-prefix? "CLOCK: " t)
       (let* ((rest (string-trim (substring t 7)))
              (arrow (string-contains rest " => "))
              (ts-str (if arrow (string-trim (substring rest 0 arrow)) rest))
              (ts (parse-org-timestamp ts-str))
              (dur (and arrow (string-trim (substring rest (+ arrow 4))))))
         (and ts (list 'clock ts dur line-byte))))
      (else #f))))

(define (org-parse-section text from to)
  "Parse the Org section elements in TEXT spanning UTF-8 bytes [FROM, TO).
Return a list of `org-element` records whose byte ranges tile [FROM, TO)
without gaps or overlaps."
  (if (>= from to)
      '()
      (let* ((len (string-length text))
             (start-ci (org-char-index-at-byte text from)))
        ;; Helper: collect lines in [from, to) as (line-str start-byte end-byte next-byte)
        (define lines
          (let loop ((ci start-ci) (b from) (acc '()))
            (if (or (>= ci len) (>= b to))
                (reverse acc)
                (let* ((le (find-line-end text ci len))
                       (le-byte (advance-bytes text ci b le))
                       (has-nl (and (< le len) (< le-byte to)))
                       (actual-end-byte (min le-byte to))
                       (actual-le (if (< actual-end-byte le-byte)
                                      (org-char-index-at-byte text actual-end-byte)
                                      le))
                       (next-b (if (and has-nl (< (+ le-byte 1) to))
                                   (+ le-byte 1)
                                   (if (and has-nl (= (+ le-byte 1) to)) to actual-end-byte)))
                       (next-ci (if has-nl (+ le 1) actual-le)))
                  (loop next-ci next-b
                        (cons (vector (substring text ci actual-le) b actual-end-byte next-b) acc))))))
        (let loop ((ls lines) (first? #t) (acc '()))
          (if (null? ls)
              (reverse acc)
              (let* ((cur (car ls))
                     (line (vector-ref cur 0))
                     (lb (vector-ref cur 1))
                     (le (vector-ref cur 2))
                     (lnext (vector-ref cur 3)))
                (cond
                  ;; Planning line (only allowed as first line of section)
                  ((and first? (org-parse-planning-line line lb))
                   => (lambda (plan)
                        (loop (cdr ls) #f (cons (make-org-element 'planning lb lnext plan) acc))))
                  ;; Blank line
                  ((blank-line? line)
                   (loop (cdr ls) #f (cons (make-org-element 'blank lb lnext #f) acc)))
                  ;; Block: #+begin_<type> ... #+end_<type>
                  ((block-begin-info line)
                   => (lambda (binfo)
                        (let ((btype (car binfo)) (brest (cdr binfo)))
                          (let find-end ((rest (cdr ls)))
                            (cond
                              ((null? rest)
                               ;; Unclosed block: treat header as keyword/paragraph line so we never drop bytes.
                               (loop (cdr ls) #f (cons (make-org-element 'paragraph lb lnext #f) acc)))
                              ((block-end-matches? (vector-ref (car rest) 0) btype)
                               (let* ((end-row (car rest))
                                      (end-next (vector-ref end-row 3))
                                      (body-end (vector-ref end-row 1)))
                                 (if (string=? btype "src")
                                     (let ((sh (parse-src-header brest)))
                                       (loop (cdr rest) #f
                                             (cons (make-org-element 'src-block lb end-next
                                                                     (list (car sh) (cdr sh) le lnext body-end))
                                                   acc)))
                                     (loop (cdr rest) #f
                                           (cons (make-org-element 'block lb end-next
                                                                   (list btype brest le lnext body-end))
                                                 acc)))))
                              (else (find-end (cdr rest))))))))
                  ;; Drawer: :NAME: ... :END:
                  ((drawer-start-name line)
                   => (lambda (dname)
                        (let find-end ((rest (cdr ls)) (entries '()))
                          (cond
                            ((null? rest)
                             ;; Unclosed drawer: fallback to paragraph line.
                             (loop (cdr ls) #f (cons (make-org-element 'paragraph lb lnext #f) acc)))
                            ((string-ci=? (string-trim (vector-ref (car rest) 0)) ":END:")
                             (let* ((end-row (car rest))
                                    (end-next (vector-ref end-row 3))
                                    (footer-start (vector-ref end-row 1)))
                               (loop (cdr rest) #f
                                     (cons (make-org-element 'drawer lb end-next
                                                             (list (string-upcase dname) le footer-start (reverse entries)))
                                           acc))))
                            (else
                             (let* ((r (car rest))
                                    (rl (vector-ref r 0))
                                    (rb (vector-ref r 1))
                                    (entry (cond ((string-ci=? dname "PROPERTIES") (parse-property-line rl rb))
                                                 ((string-ci=? dname "LOGBOOK") (parse-logbook-line rl rb))
                                                 (else #f))))
                               (find-end (cdr rest) (if entry (cons entry entries) entries))))))))
                  ;; Table: contiguous lines starting with |
                  ((table-line? line)
                   (let collect ((rest (cdr ls))
                                 (rows (list (list lb le (table-rule-line? line))))
                                 (last-next lnext))
                     (if (and (pair? rest) (table-line? (vector-ref (car rest) 0)))
                         (let ((r (car rest)))
                           (collect (cdr rest)
                                    (cons (list (vector-ref r 1) (vector-ref r 2) (table-rule-line? (vector-ref r 0))) rows)
                                    (vector-ref r 3)))
                         (loop rest #f (cons (make-org-element 'table lb last-next (reverse rows)) acc)))))
                  ;; List item
                  ((parse-list-item-line line lb)
                   => (lambda (item)
                        (loop (cdr ls) #f (cons (make-org-element 'list-item lb lnext item) acc))))
                  ;; Keyword line (#+NAME:, #+TBLFM:, etc.)
                  ((keyword-line-info line)
                   => (lambda (kv)
                        (loop (cdr ls) #f (cons (make-org-element 'keyword lb lnext kv) acc))))
                  ;; Comment line (# ...)
                  ((comment-line? line)
                   (loop (cdr ls) #f (cons (make-org-element 'comment lb lnext #f) acc)))
                  ;; Horizontal rule (-----)
                  ((hr-line? line)
                   (loop (cdr ls) #f (cons (make-org-element 'rule lb lnext #f) acc)))
                  ;; Paragraph (single line or merged contiguous plain lines)
                  (else
                   (let collect ((rest (cdr ls)) (last-next lnext))
                     (if (and (pair? rest)
                              (let ((nl (vector-ref (car rest) 0)))
                                (and (not (blank-line? nl))
                                     (not (block-begin-info nl))
                                     (not (drawer-start-name nl))
                                     (not (table-line? nl))
                                     (not (parse-list-item-line nl (vector-ref (car rest) 1)))
                                     (not (keyword-line-info nl))
                                     (not (comment-line? nl))
                                     (not (hr-line? nl)))))
                         (collect (cdr rest) (vector-ref (car rest) 3))
                         (loop rest #f (cons (make-org-element 'paragraph lb last-next #f) acc))))))))))))

;; ----- Section query helpers -----

(define (org-section-planning elements)
  "Return the planning `org-element` from section ELEMENTS, or #f."
  (find (lambda (e) (eq? (org-element-kind e) 'planning)) elements))

(define (org-section-properties elements)
  "Return the PROPERTIES drawer `org-element` from section ELEMENTS, or #f."
  (find (lambda (e) (and (eq? (org-element-kind e) 'drawer)
                         (string=? (car (org-element-data e)) "PROPERTIES")))
        elements))

(define (org-section-logbook elements)
  "Return the LOGBOOK drawer `org-element` from section ELEMENTS, or #f."
  (find (lambda (e) (and (eq? (org-element-kind e) 'drawer)
                         (string=? (car (org-element-data e)) "LOGBOOK")))
        elements))

(define (org-section-property elems key)
  "Return property KEY (case-insensitive) from parsed section ELEMS, or #f."
  (let ((drawer (org-section-properties elems)))
    (and drawer
         (let ((entry (find (lambda (p) (string-ci=? (car p) key)) (cadddr (org-element-data drawer)))))
           (and entry (list-ref entry 5))))))

(define (org-heading-property text outline h key)
  "Return property KEY on heading H in TEXT and OUTLINE, or #f.
Matching on KEY is case-insensitive."
  (org-section-property (org-parse-section text (org-heading-body-start h) (org-heading-section-end outline h)) key))

;; ----- Tier 3: Inline object parsing -----

(define (emphasis-pre-ok? s i)
  (or (= i 0)
      (let ((c (string-ref s (- i 1))))
        (or (char-whitespace? c) (memv c '(#\- #\( #\{ #\' #\"))))))

(define (emphasis-post-ok? s j len)
  (or (>= j len)
      (let ((c (string-ref s j)))
        (or (char-whitespace? c) (memv c '(#\- #\. #\, #\; #\: #\! #\? #\' #\" #\) #\} #\[ #\\))))))

(define (emphasis-kind c)
  (case c
    ((#\*) 'bold)
    ((#\/) 'italic)
    ((#\_) 'underline)
    ((#\+) 'strike)
    ((#\~) 'code)
    ((#\=) 'verbatim)
    (else #f)))

(define (parse-cookie-at s i len)
  ;; "[1/3]", "[33%]", "[/]", "[%]" -> (end-ci kind done total) or #f
  (and (< (+ i 2) len)
       (char=? (string-ref s i) #\[)
       (let ((rb (let loop ((j (+ i 1)))
                   (cond ((>= j len) #f)
                         ((char=? (string-ref s j) #\]) j)
                         ((or (char-numeric? (string-ref s j))
                              (memv (string-ref s j) '(#\/ #\%)))
                          (loop (+ j 1)))
                         (else #f)))))
         (and rb
              (let ((inner (substring s (+ i 1) rb)))
                (cond
                  ((string=? inner "/") (list (+ rb 1) 'ratio #f #f))
                  ((string=? inner "%") (list (+ rb 1) 'percent #f #f))
                  ((string-index inner #\/)
                   => (lambda (sl)
                        (let ((a (string->number (substring inner 0 sl)))
                              (b (string->number (substring inner (+ sl 1)))))
                          (and a b (exact-integer? a) (exact-integer? b)
                               (list (+ rb 1) 'ratio a b)))))
                  ((and (> (string-length inner) 1)
                        (char=? (string-ref inner (- (string-length inner) 1)) #\%))
                   (let ((p (string->number (substring inner 0 (- (string-length inner) 1)))))
                     (and p (exact-integer? p) (list (+ rb 1) 'percent p #f))))
                  (else #f)))))))

(define (url-char? c)
  (and (not (char-whitespace? c))
       (not (memv c '(#\< #\> #\[ #\] #\" #\' #\) #\})))))

(define (org-parse-inline text from to)
  "Parse inline Org objects in TEXT within UTF-8 byte range [FROM, TO).
Return a list of `org-inline` records in order."
  (if (>= from to)
      '()
      (let* ((sub (org-slice-bytes text from to))
             (len (string-length sub))
             (link-dead-until -1))
        (let loop ((i 0) (b from) (acc '()))
          (if (>= i len)
              (reverse acc)
              (let ((c (string-ref sub i)))
                (cond
                  ;; Bracket link [[target]] or [[target][desc]]
                  ((and (>= i link-dead-until) (char=? c #\[) (< (+ i 1) len) (char=? (string-ref sub (+ i 1)) #\[))
                   (let find-mid ((j (+ i 2)))
                     (cond
                       ((or (>= j len) (char=? (string-ref sub j) #\newline))
                        (set! link-dead-until j)
                        (loop (+ i 1) (+ b 1) acc))
                       ((and (char=? (string-ref sub j) #\])
                             (< (+ j 1) len)
                             (char=? (string-ref sub (+ j 1)) #\]))
                        ;; [[target]]
                        (let* ((target (substring sub (+ i 2) j))
                               (end-ci (+ j 2))
                               (end-b (advance-bytes sub i b end-ci))
                               (t-start (+ b 2))
                               (t-end (- end-b 2)))
                          (loop end-ci end-b
                                (cons (make-org-inline 'link b end-b (list target #f #f t-start t-end)) acc))))
                       ((and (char=? (string-ref sub j) #\])
                             (< (+ j 1) len)
                             (char=? (string-ref sub (+ j 1)) #\[))
                        ;; [[target][desc]]
                        (let find-end ((k (+ j 2)))
                          (cond
                            ((or (>= k len) (char=? (string-ref sub k) #\newline))
                             (set! link-dead-until k)
                             (loop (+ i 1) (+ b 1) acc))
                            ((and (char=? (string-ref sub k) #\])
                                  (< (+ k 1) len)
                                  (char=? (string-ref sub (+ k 1)) #\]))
                             (let* ((target (substring sub (+ i 2) j))
                                    (end-ci (+ k 2))
                                    (t-start (+ b 2))
                                    (t-end (advance-bytes sub i b j))
                                    (d-start (+ t-end 2))
                                    (d-end (advance-bytes sub j t-end k))
                                    (end-b (+ d-end 2)))
                               (loop end-ci end-b
                                     (cons (make-org-inline 'link b end-b (list target d-start d-end t-start t-end)) acc))))
                            (else (find-end (+ k 1))))))
                       (else (find-mid (+ j 1))))))
                  ;; Footnote [fn:label]
                  ((and (char=? c #\[) (<= (+ i 5) len) (string=? (substring sub i (+ i 4)) "[fn:"))
                   (let find-rb ((j (+ i 4)))
                     (cond
                       ((>= j len) (loop (+ i 1) (+ b 1) acc))
                       ((char=? (string-ref sub j) #\])
                        (let* ((end-ci (+ j 1))
                               (end-b (advance-bytes sub i b end-ci)))
                          (loop end-ci end-b
                                (cons (make-org-inline 'footnote b end-b (substring sub (+ i 4) j)) acc))))
                       ((or (char-whitespace? (string-ref sub j)) (char=? (string-ref sub j) #\[))
                        (loop (+ i 1) (+ b 1) acc))
                       (else (find-rb (+ j 1))))))
                  ;; Statistics cookie [1/3] or [33%]
                  ((and (char=? c #\[) (parse-cookie-at sub i len))
                   => (lambda (ck)
                        (let* ((end-ci (car ck))
                               (end-b (advance-bytes sub i b end-ci)))
                          (loop end-ci end-b
                                (cons (make-org-inline 'cookie b end-b (cdr ck)) acc)))))
                  ;; Active <...> or inactive [...] timestamp (requires YYYY-MM-DD prefix)
                  ((and (or (char=? c #\<) (char=? c #\[))
                        (<= (+ i 12) len)
                        (char-numeric? (string-ref sub (+ i 1)))
                        (char-numeric? (string-ref sub (+ i 2)))
                        (char-numeric? (string-ref sub (+ i 3)))
                        (char-numeric? (string-ref sub (+ i 4)))
                        (char=? (string-ref sub (+ i 5)) #\-)
                        (char-numeric? (string-ref sub (+ i 6)))
                        (char-numeric? (string-ref sub (+ i 7)))
                        (char=? (string-ref sub (+ i 8)) #\-)
                        (char-numeric? (string-ref sub (+ i 9)))
                        (char-numeric? (string-ref sub (+ i 10))))
                   (let* ((close (if (char=? c #\<) #\> #\]))
                          (scan-lim (min len (+ i 64)))
                          (end-ci (let find-c ((j (+ i 11)))
                                    (cond ((>= j scan-lim) #f)
                                          ((char=? (string-ref sub j) #\newline) #f)
                                          ((char=? (string-ref sub j) close) (+ j 1))
                                          (else (find-c (+ j 1)))))))
                     ;; Check optional date range --<...>
                     (let* ((end-ci (if (and end-ci (< (+ end-ci 3) len)
                                             (string=? (substring sub end-ci (+ end-ci 2)) "--")
                                             (char=? (string-ref sub (+ end-ci 2)) c))
                                        (let ((scan-lim2 (min len (+ end-ci 66))))
                                          (let find-c2 ((j (+ end-ci 3)))
                                            (cond ((>= j scan-lim2) end-ci)
                                                  ((char=? (string-ref sub j) #\newline) end-ci)
                                                  ((char=? (string-ref sub j) close) (+ j 1))
                                                  (else (find-c2 (+ j 1))))))
                                        end-ci))
                            (ts (and end-ci (parse-org-timestamp (substring sub i end-ci)))))
                       (if ts
                           (let ((end-b (advance-bytes sub i b end-ci)))
                             (loop end-ci end-b
                                   (cons (make-org-inline (if (org-timestamp-active? ts) 'timestamp-active 'timestamp-inactive)
                                                          b end-b ts)
                                         acc)))
                           (loop (+ i 1) (+ b 1) acc)))))
                  ;; Bare http:// or https:// URL
                  ((and (char=? c #\h)
                        (or (and (<= (+ i 8) len) (string=? (substring sub i (+ i 8)) "https://"))
                            (and (<= (+ i 7) len) (string=? (substring sub i (+ i 7)) "http://"))))
                   (let find-u ((j (+ i 7)))
                     (if (and (< j len) (url-char? (string-ref sub j)))
                         (find-u (+ j 1))
                         ;; Trim trailing punctuation .,;:
                         (let trim ((e j))
                           (if (and (> e (+ i 8)) (memv (string-ref sub (- e 1)) '(#\. #\, #\; #\:)))
                               (trim (- e 1))
                               (let* ((url (substring sub i e))
                                      (end-b (advance-bytes sub i b e)))
                                 (loop e end-b
                                       (cons (make-org-inline 'link b end-b (list url #f #f b end-b)) acc))))))))
                  ;; Inline emphasis: *bold* /italic/ _underline_ +strike+ ~code~ =verbatim=
                  ((and (emphasis-kind c) (emphasis-pre-ok? sub i) (< (+ i 2) len)
                        (not (char-whitespace? (string-ref sub (+ i 1)))))
                   (let ((kind (emphasis-kind c)))
                     (let find-close ((j (+ i 1)))
                       (cond
                         ((>= j len) (loop (+ i 1) (+ b 1) acc))
                         ((char=? (string-ref sub j) #\newline) (loop (+ i 1) (+ b 1) acc))
                         ((and (char=? (string-ref sub j) c)
                               (> j (+ i 1))
                               (not (char-whitespace? (string-ref sub (- j 1))))
                               (emphasis-post-ok? sub (+ j 1) len))
                          (let* ((end-ci (+ j 1))
                                 (end-b (advance-bytes sub i b end-ci))
                                 (outer (make-org-inline kind b end-b (cons (+ b 1) (- end-b 1))))
                                 ;; Keep ~code~ and =verbatim= opaque; expose child objects (links, etc.) in ordinary emphasis.
                                 (children (if (memq kind '(code verbatim))
                                               '()
                                               (org-parse-inline text (+ b 1) (- end-b 1)))))
                            (loop end-ci end-b
                                  (append (reverse children) (cons outer acc)))))
                         (else (find-close (+ j 1)))))))
                  (else
                   (loop (+ i 1) (+ b (char-utf8-len c)) acc)))))))))
