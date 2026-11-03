;;; Markdown (CommonMark, with GitHub's tables, strikethrough and task
;;; lists, less what a renderer of text cannot show: raw HTML is kept as
;;; text, reference links are not resolved), parsed into rich text
;;; (rich.scm). Each inline's text is a span of the document it was
;;; parsed from, so a rendering of it edits the markdown, the markup it
;;; hides staying as it was.

(require "modes.scm")
(require "targets.scm")
(require "rich.scm")

(provide markdown->rich markdown-render! markdown-escape)

;;; Lines of the markdown: where each starts in the document, in bytes, and
;;; its text without the line break. Containers take their markers off the
;;; front of their lines, moving where they start.

(define (line start text) (cons start text))
(define line-start car)
(define line-text cdr)

(define (utf8-length s) (bytevector-length (string->utf8 s)))

(define (line-sub l from to)
  (let ((t (line-text l)))
    (line (+ (line-start l) (utf8-length (substring t 0 from))) (substring t from to))))

(define (line-drop l n)
  (line-sub l (min n (string-length (line-text l))) (string-length (line-text l))))

(define (space? c) (or (char=? c #\space) (char=? c #\tab)))

;; Where T's first character that is not a space is.
(define (first-nonspace t)
  (let loop ((i 0)) (if (and (< i (string-length t)) (space? (string-ref t i))) (loop (+ i 1)) i)))

(define (blank? t) (= (first-nonspace t) (string-length t)))

;; T's indentation in columns, a tab to the next multiple of four.
(define (leading t)
  (let loop ((i 0) (col 0))
    (if (and (< i (string-length t)) (space? (string-ref t i)))
        (loop (+ i 1) (if (char=? (string-ref t i) #\tab) (* 4 (+ 1 (quotient col 4))) (+ col 1)))
        col)))

;; L without up to COLS columns of its indentation.
(define (line-dedent l cols)
  (let ((t (line-text l)))
    (let loop ((i 0) (col 0))
      (if (and (< i (string-length t)) (< col cols) (space? (string-ref t i)))
          (loop (+ i 1) (if (char=? (string-ref t i) #\tab) (* 4 (+ 1 (quotient col 4))) (+ col 1)))
          (line-drop l i)))))

(define (line-trim-left l) (line-drop l (first-nonspace (line-text l))))

(define (line-trim l)
  (let* ((l (line-trim-left l)) (t (line-text l)))
    (let loop ((e (string-length t)))
      (if (and (> e 0) (space? (string-ref t (- e 1)))) (loop (- e 1)) (line (line-start l) (substring t 0 e))))))

(define (run-of t i c)
  (let loop ((j i)) (if (and (< j (string-length t)) (char=? (string-ref t j) c)) (loop (+ j 1)) (- j i))))

(define (has-char? t c)
  (let loop ((i 0)) (and (< i (string-length t)) (or (char=? (string-ref t i) c) (loop (+ i 1))))))

;;; What starts a block

(define (small-indent? t) (< (leading t) 4))

;; An ATX heading's level, or #f.
(define (atx t)
  (and (small-indent? t)
       (let* ((i (first-nonspace t)) (n (run-of t i #\#)) (j (+ i n)))
         (and (<= 1 n 6) (or (= j (string-length t)) (space? (string-ref t j))) n))))

;; A code fence: (char length info indent), or #f.
(define (fence t)
  (and (small-indent? t)
       (let ((i (first-nonspace t)))
         (and (< i (string-length t))
              (memv (string-ref t i) '(#\` #\~))
              (let* ((c (string-ref t i)) (n (run-of t i c)))
                (and (>= n 3)
                     (let ((info (string-trim (substring t (+ i n) (string-length t)))))
                       (and (not (and (char=? c #\`) (has-char? info #\`)))
                            (list c n info (leading t))))))))))

(define (closes-fence? t f)
  (and (small-indent? t)
       (let* ((i (first-nonspace t)) (n (run-of t i (car f))))
         (and (>= n (cadr f)) (blank? (substring t (+ i n) (string-length t)))))))

(define (rule? t)
  (and (small-indent? t)
       (let ((cs (filter (lambda (c) (not (space? c))) (string->list t))))
         (and (>= (length cs) 3)
              (memv (car cs) '(#\- #\* #\_))
              (every (lambda (c) (char=? c (car cs))) cs)))))

(define (setext-level t)
  (and (small-indent? t)
       (let* ((i (first-nonspace t)) (c (and (< i (string-length t)) (string-ref t i))))
         (and (memv c '(#\= #\-))
              (blank? (substring t (+ i (run-of t i c)) (string-length t)))
              (if (char=? c #\=) 1 2)))))

;; Where a quoted line's text starts after its >, or #f.
(define (quote-start t)
  (and (small-indent? t)
       (let ((i (first-nonspace t)))
         (and (< i (string-length t)) (char=? (string-ref t i) #\>)
              (if (and (< (+ i 1) (string-length t)) (space? (string-ref t (+ i 1)))) (+ i 2) (+ i 1))))))

;; A list item's marker: (kind marker content-index content-column), KIND
;; the bullet character, or . or ) for a numbered one; or #f.
(define (list-marker t)
  (and (small-indent? t)
       (let* ((i (first-nonspace t)) (len (string-length t)))
         (define (after j kind)
           (cond ((= j len) (list kind (substring t i j) j (+ (leading t) (- j i) 1)))
                 ((space? (string-ref t j))
                  (let* ((sp (run-of t j (string-ref t j))) (sp (if (> sp 4) 1 sp)))
                    (list kind (substring t i j) (+ j sp) (+ (leading t) (- j i) sp))))
                 (else #f)))
         (cond ((and (< i len) (memv (string-ref t i) '(#\- #\* #\+))) (after (+ i 1) (string-ref t i)))
               (else
                (let ((j (let loop ((j i)) (if (and (< j len) (char-numeric? (string-ref t j))) (loop (+ j 1)) j))))
                  (and (> j i) (<= (- j i) 9) (< j len) (memv (string-ref t j) '(#\. #\)))
                       (after (+ j 1) (string-ref t j)))))))))

(define (ordered-kind? kind) (memv kind '(#\. #\))))

;; Whether T ends a paragraph before it.
(define (interrupts? t)
  (or (blank? t) (atx t) (fence t) (rule? t) (quote-start t)
      (let ((m (list-marker t))) (and m (< (caddr m) (string-length t))))))

;;; Tables, as GitHub has them

;; T's cells as (from . to), its outer pipes left out.
(define (cell-ranges t)
  (let* ((i (first-nonspace t))
         (end (let loop ((e (string-length t))) (if (and (> e 0) (space? (string-ref t (- e 1)))) (loop (- e 1)) e)))
         (i (if (and (< i end) (char=? (string-ref t i) #\|)) (+ i 1) i))
         (end (if (and (> end i) (char=? (string-ref t (- end 1)) #\|)
                       (not (and (> (- end 1) i) (char=? (string-ref t (- end 2)) #\\))))
                  (- end 1)
                  end)))
    (let loop ((j i) (from i) (acc '()))
      (cond ((>= j end) (reverse (cons (cons from end) acc)))
            ((char=? (string-ref t j) #\\) (loop (+ j 2) from acc))
            ((char=? (string-ref t j) #\|) (loop (+ j 1) (+ j 1) (cons (cons from j) acc)))
            (else (loop (+ j 1) from acc))))))

;; A delimiter row's alignments, or #f.
(define (delimiter-row t)
  (and (has-char? t #\|)
       (let ((aligns (map (lambda (r)
                            (let* ((c (string-trim (substring t (car r) (cdr r)))) (n (string-length c)))
                              (and (> n 0)
                                   (let* ((left (char=? (string-ref c 0) #\:))
                                          (right (char=? (string-ref c (- n 1)) #\:))
                                          (dashes (substring c (if left 1 0) (if (and right (> n 1)) (- n 1) n))))
                                     (and (> (string-length dashes) 0)
                                          (every (lambda (ch) (char=? ch #\-)) (string->list dashes))
                                          (cond ((and left right) 'center) (right 'right) (else 'left)))))))
                          (cell-ranges t))))
         (and (every values aligns) aligns))))

(define (row-cells l count)
  (let ((cells (map (lambda (r) (line-trim (line-sub l (car r) (cdr r)))) (cell-ranges (line-text l)))))
    (let loop ((cells cells) (n 0) (acc '()))
      (cond ((= n count) (reverse acc))
            ((null? cells) (loop '() (+ n 1) (cons (line (line-start l) "") acc)))
            (else (loop (cdr cells) (+ n 1) (cons (car cells) acc)))))))

;;; Blocks: (heading level lines), (paragraph lines), (code info lines),
;;; (rule), (quote blocks), (list ordered? items) with each item
;;; (marker task blocks), TASK #f, open or done; (table aligns header rows).

(define (parse-blocks lines #:extents [extents #f] #:units [units #f])
  (let* ((v (if (vector? lines) lines (list->vector lines))) (n (vector-length v)))
    (define (text i) (line-text (vector-ref v i)))
    (define (next-nonblank i) (if (and (< i n) (blank? (text i))) (next-nonblank (+ i 1)) i))
    ;; Lines from I while (keep? text) holds: (lines . next).
    (define (take-while i keep?)
      (let loop ((j i) (acc '()))
        (if (and (< j n) (keep? (text j))) (loop (+ j 1) (cons (vector-ref v j) acc)) (cons (reverse acc) j))))

    (define (indented-code i)
      (let* ((got (take-while i (lambda (t) (or (blank? t) (>= (leading t) 4)))))
             (lines (let trim ((ls (reverse (car got)))) (if (and (pair? ls) (blank? (line-text (car ls)))) (trim (cdr ls)) (reverse ls)))))
        (values (list 'code "" (map (lambda (l) (line-dedent l 4)) lines)) (cdr got))))

    (define (fenced i f)
      (let loop ((j (+ i 1)) (acc '()))
        (cond ((>= j n) (values (list 'code (caddr f) (reverse acc)) j))
              ((closes-fence? (text j) f) (values (list 'code (caddr f) (reverse acc)) (+ j 1)))
              (else (loop (+ j 1) (cons (line-dedent (vector-ref v j) (cadddr f)) acc))))))

    (define (heading i level)
      (let* ((l (line-trim (line-drop (vector-ref v i) (+ (first-nonspace (text i)) level))))
             (t (line-text l))
             ;; A closing run of #, after a space or alone.
             (e (let loop ((e (string-length t))) (if (and (> e 0) (char=? (string-ref t (- e 1)) #\#)) (loop (- e 1)) e)))
             (l (if (and (< e (string-length t)) (or (= e 0) (space? (string-ref t (- e 1)))))
                    (line-trim (line-sub l 0 e))
                    l)))
        (values (list 'heading level (list l)) (+ i 1))))

    (define (blockquote i)
      (let loop ((j i) (acc '()))
        (if (>= j n)
            (values (list 'quote (parse-blocks (reverse acc))) j)
            (let* ((t (text j)) (q (quote-start t)))
              (cond (q (loop (+ j 1) (cons (line-drop (vector-ref v j) q) acc)))
                    ;; A paragraph's lazy continuation.
                    ((and (not (blank? t)) (pair? acc) (not (blank? (line-text (car acc)))) (not (interrupts? t)))
                     (loop (+ j 1) (cons (line-trim-left (vector-ref v j)) acc)))
                    (else (values (list 'quote (parse-blocks (reverse acc))) j)))))))

    (define (item i m)
      (let* ((col (cadddr m))
             (first (line-drop (vector-ref v i) (caddr m)))
             (ft (line-text first))
             (task (cond ((or (string-prefix? "[ ] " ft) (string=? ft "[ ]")) 'open)
                         ((or (string-prefix? "[x] " ft) (string-prefix? "[X] " ft) (string=? ft "[x]") (string=? ft "[X]")) 'done)
                         (else #f)))
             (first (if task (line-drop first 4) first)))
        (let loop ((j (+ i 1)) (acc (list first)))
          (define (done) (values (list (cadr m) task (parse-blocks (reverse acc))) j))
          (if (>= j n)
              (done)
              (let ((t (text j)))
                (cond ((blank? t)
                       (let ((k (next-nonblank j)))
                         (if (and (< k n) (>= (leading (text k)) col))
                             (loop k (cons (line (line-start (vector-ref v j)) "") acc))
                             (done))))
                      ((>= (leading t) col) (loop (+ j 1) (cons (line-dedent (vector-ref v j) col) acc)))
                      ((and (not (blank? (line-text (car acc)))) (not (interrupts? t)))
                       (loop (+ j 1) (cons (line-trim-left (vector-ref v j)) acc)))
                      (else (done))))))))

    (define (lst i m)
      (let loop ((j i) (items '()))
        (let* ((k (next-nonblank j))
               (mk (and (< k n) (list-marker (text k)))))
          (if (and mk (eqv? (car mk) (car m)))
              (call-with-values (lambda () (item k mk)) (lambda (it next) (loop next (cons it items))))
              (values (list 'list (ordered-kind? (car m)) (reverse items)) j)))))

    (define (table-start? i)
      (and (< (+ i 1) n) (has-char? (text i) #\|)
           (let ((aligns (delimiter-row (text (+ i 1)))))
             (and aligns (= (length aligns) (length (cell-ranges (text i)))) aligns))))

    (define (table i aligns)
      (let* ((count (length aligns))
             (got (take-while (+ i 2) (lambda (t) (and (not (blank? t)) (has-char? t #\|) (not (interrupts? t)))))))
        (values (list 'table aligns (row-cells (vector-ref v i) count) (map (lambda (l) (row-cells l count)) (car got)))
                (cdr got))))

    ;; Its lines keep their trailing spaces but the last's: two or more
    ;; before a line break make it a hard one.
    (define (paragraph i)
      (let* ((done (lambda (acc) (reverse (cons (line-trim (car acc)) (cdr acc)))))
             (heading (lambda (level acc) (list 'heading level (map line-trim (reverse acc))))))
        (let loop ((j (+ i 1)) (acc (list (line-trim-left (vector-ref v i)))))
          (cond ((>= j n) (values (list 'paragraph (done acc)) j))
                ((setext-level (text j)) => (lambda (level) (values (heading level acc) (+ j 1))))
                ((interrupts? (text j)) (values (list 'paragraph (done acc)) j))
                (else (loop (+ j 1) (cons (line-trim-left (vector-ref v j)) acc)))))))

    ;; With UNITS, (from state complete stop?): the units from line FROM
    ;; (`markdown-units'), STATE the list kind it continues or #f.
    (define (in-units from state complete stop?)
      ;; A unit whose lines or look ahead reach the lines given, not the
      ;; text's end, is not known yet: more are needed.
      (define (check j) (unless (or complete (< (next-nonblank j) n)) (raise 'markdown-need-more)))
      (let loop ((i from) (state state) (acc '()) (count 0))
        (let ((i (next-nonblank i)))
          (cond ((>= i n) (if complete (values (reverse acc) i #f #t) (raise 'markdown-need-more)))
                ((stop? i state count) (values (reverse acc) i state #f))
                (state
                 (let ((mk (list-marker (text i))))
                   (if (and mk (eqv? (car mk) state))
                       (call-with-values (lambda () (item i mk))
                         (lambda (it j) (check j) (loop j state (cons (vector 'item it i j state #f (ordered-kind? state)) acc) (+ count 1))))
                       (loop i #f acc count))))
                (else
                 (let* ((t (text i))
                        (mk (and (< (leading t) 4) (not (fence t)) (not (atx t)) (not (rule? t)) (not (quote-start t)) (list-marker t))))
                   (if mk
                       (call-with-values (lambda () (item i mk))
                         (lambda (it j) (check j) (loop j (car mk) (cons (vector 'item it i j #f #t (ordered-kind? (car mk))) acc) (+ count 1))))
                       (call-with-values (lambda () (block-at i))
                         (lambda (b j) (check j) (loop j #f (cons (vector 'block b i j #f #f #f) acc) (+ count 1)))))))))))

    ;; The top-level block at I, not a list's: (values block next).
    (define (block-at i)
      (let ((t (text i)))
        (cond ((>= (leading t) 4) (indented-code i))
              ((fence t) => (lambda (f) (fenced i f)))
              ((atx t) => (lambda (level) (heading i level)))
              ((rule? t) (values (list 'rule) (+ i 1)))
              ((quote-start t) (blockquote i))
              ((table-start? i) => (lambda (aligns) (table i aligns)))
              (else (paragraph i)))))

    ;; Each block, or with EXTENTS (block from to), the lines it is of.
    (if units (apply in-units units)
    (let loop ((i 0) (acc '()))
      (if (>= i n)
          (reverse acc)
          (let ((t (text i)))
            (define (next block j) (loop j (cons (if extents (list block i j) block) acc)))
            (cond ((blank? t) (loop (+ i 1) acc))
                  ((>= (leading t) 4) (call-with-values (lambda () (indented-code i)) next))
                  ((fence t) => (lambda (f) (call-with-values (lambda () (fenced i f)) next)))
                  ((atx t) => (lambda (level) (call-with-values (lambda () (heading i level)) next)))
                  ((rule? t) (next (list 'rule) (+ i 1)))
                  ((quote-start t) (call-with-values (lambda () (blockquote i)) next))
                  ((list-marker t) => (lambda (m) (call-with-values (lambda () (lst i m)) next)))
                  ((table-start? i) => (lambda (aligns) (call-with-values (lambda () (table i aligns)) next)))
                  (else (call-with-values (lambda () (paragraph i)) next)))))))))

;;; Inlines, over a flow: a block's lines as one string, a line break
;;; between each, knowing where each character is in the document.

(define-record-type flow
  (make-flow text starts lines)
  flow?
  (text flow-text)
  ;; Where each line starts in TEXT.
  (starts flow-starts)
  (lines flow-lines))

(define (lines->flow lines)
  (let loop ((ls lines) (at 0) (starts '()))
    (if (null? ls)
        (make-flow (string-join (map line-text lines) "\n") (list->vector (reverse starts)) (list->vector lines))
        (loop (cdr ls) (+ at (string-length (line-text (car ls))) 1) (cons at starts)))))

;; Where the character I of the flow F is in the document.
(define (flow-byte f i)
  (let* ((starts (flow-starts f))
         (k (let loop ((k (- (vector-length starts) 1))) (if (and (> k 0) (> (vector-ref starts k) i)) (loop (- k 1)) k)))
         (l (vector-ref (flow-lines f) k))
         (o (min (- i (vector-ref starts k)) (string-length (line-text l)))))
    (+ (line-start l) (utf8-length (substring (line-text l) 0 o)))))

(define punctuation "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~")
(define (punct? c) (has-char? punctuation c))
(define (word? c) (or (char-alphabetic? c) (char-numeric? c)))

;; Inline nodes: (text from to), (code from to), (strong nodes),
;; (emph nodes), (strong-emph nodes), (strike nodes), (link nodes url
;; from to) and (image from to url), over the flow's characters.
(define (parse-inlines s from to)
  (define (at i) (and (< i to) (string-ref s i)))
  (define (flush start i acc) (if (< start i) (cons (list 'text start i) acc) acc))
  (let loop ((i from) (start from) (acc '()))
    (if (>= i to)
        (reverse (flush start i acc))
        (let ((c (string-ref s i)))
          (define (took node end) (loop end end (cons node (flush start i acc))))
          (cond ((and (char=? c #\\) (at (+ i 1)) (punct? (at (+ i 1))))
                 (took (list 'text (+ i 1) (+ i 2)) (+ i 2)))
                ((char=? c #\`)
                 (let* ((n (run-of s i #\`)) (close (closing-backticks s (+ i n) to n)))
                   (if close
                       (let* ((a (+ i n)) (b close)
                              ;; One space off each end, unless it is all spaces.
                              (pad (and (> (- b a) 1) (char=? (string-ref s a) #\space) (char=? (string-ref s (- b 1)) #\space)
                                        (let any ((k a)) (and (< k b) (or (not (char=? (string-ref s k) #\space)) (any (+ k 1))))))))
                         (took (list 'code (if pad (+ a 1) a) (if pad (- b 1) b)) (+ close n)))
                       (loop (+ i n) start acc))))
                ((and (char=? c #\!) (eqv? (at (+ i 1)) #\[) (link-at s (+ i 1) to))
                 => (lambda (l) (took (list 'image (cadr l) (caddr l) (cadddr l)) (car l))))
                ((and (char=? c #\[) (link-at s i to))
                 => (lambda (l) (took (list 'link (parse-inlines s (cadr l) (caddr l)) (cadddr l) (cadr l) (caddr l)) (car l))))
                ((and (char=? c #\<) (autolink-at s i to))
                 => (lambda (end) (took (list 'link (list (list 'text (+ i 1) (- end 1))) (substring s (+ i 1) (- end 1)) (+ i 1) (- end 1)) end)))
                ((and (memv c '(#\h #\w)) (or (= i from) (not (word? (string-ref s (- i 1))))) (bare-url-at s i to))
                 => (lambda (end) (took (list 'link (list (list 'text i end)) (let ((u (substring s i end))) (if (string-prefix? "www." u) (string-append "https://" u) u)) i end) end)))
                ((and (memv c '(#\* #\_ #\~)) (emphasis-at s i from to))
                 => (lambda (e) (took (cdr e) (car e))))
                ((memv c '(#\* #\_ #\~)) (loop (+ i (run-of s i c)) start acc))
                (else (loop (+ i 1) start acc)))))))

(define (closing-backticks s i to n)
  (let loop ((j i))
    (cond ((>= j to) #f)
          ((char=? (string-ref s j) #\`) (let ((m (run-of s j #\`))) (if (= m n) j (loop (+ j m)))))
          (else (loop (+ j 1))))))

;; A link or image's text and destination from the [ at I:
;; (end text-from text-to url), or #f.
(define (link-at s i to)
  (let close ((j (+ i 1)) (depth 0))
    (cond ((>= j to) #f)
          ((char=? (string-ref s j) #\\) (close (+ j 2) depth))
          ((char=? (string-ref s j) #\`)
           (let* ((n (run-of s j #\`)) (c (closing-backticks s (+ j n) to n))) (close (if c (+ c n) (+ j n)) depth)))
          ((char=? (string-ref s j) #\[) (close (+ j 1) (+ depth 1)))
          ((and (char=? (string-ref s j) #\]) (> depth 0)) (close (+ j 1) (- depth 1)))
          ((char=? (string-ref s j) #\])
           (and (< (+ j 1) to) (char=? (string-ref s (+ j 1)) #\()
                (let* ((k (let skip ((k (+ j 2))) (if (and (< k to) (space? (string-ref s k))) (skip (+ k 1)) k)))
                       (dest (destination s k to)))
                  (and dest
                       (let* ((e (let skip ((e (cdr dest))) (if (and (< e to) (memv (string-ref s e) '(#\space #\tab #\newline))) (skip (+ e 1)) e)))
                              ;; A title, which is not shown.
                              (e (if (and (< e to) (memv (string-ref s e) '(#\" #\')))
                                     (let ((q (string-ref s e)))
                                       (let find ((f (+ e 1))) (cond ((>= f to) #f) ((char=? (string-ref s f) q) (+ f 1)) (else (find (+ f 1))))))
                                     e))
                              (e (and e (let skip ((e e)) (if (and (< e to) (space? (string-ref s e))) (skip (+ e 1)) e)))))
                         (and e (< e to) (char=? (string-ref s e) #\))
                              (list (+ e 1) (+ i 1) j (car dest))))))))
          (else (close (+ j 1) depth)))))

;; A link destination from K: (url . end), or #f.
(define (destination s k to)
  (if (and (< k to) (char=? (string-ref s k) #\<))
      (let loop ((e (+ k 1)))
        (cond ((>= e to) #f)
              ((char=? (string-ref s e) #\>) (cons (substring s (+ k 1) e) (+ e 1)))
              ((char=? (string-ref s e) #\newline) #f)
              (else (loop (+ e 1)))))
      (let loop ((e k) (depth 0))
        (if (or (>= e to) (memv (string-ref s e) '(#\space #\tab #\newline)) (and (char=? (string-ref s e) #\)) (= depth 0)))
            (cons (substring s k e) e)
            (loop (+ e 1) (case (string-ref s e) ((#\() (+ depth 1)) ((#\)) (- depth 1)) (else depth)))))))

(define (autolink-at s i to)
  (let loop ((e (+ i 1)))
    (cond ((>= e to) #f)
          ((memv (string-ref s e) '(#\space #\newline #\<)) #f)
          ((char=? (string-ref s e) #\>)
           (let ((u (substring s (+ i 1) e))) (and (or (has-char? u #\:) (has-char? u #\@)) (+ e 1))))
          (else (loop (+ e 1))))))

(define (bare-url-at s i to)
  (let ((rest (substring s i (min to (+ i 8)))))
    (and (or (string-prefix? "https://" rest) (string-prefix? "http://" rest) (string-prefix? "www." rest))
         (let* ((e (let loop ((e i)) (if (and (< e to) (not (memv (string-ref s e) '(#\space #\tab #\newline #\<)))) (loop (+ e 1)) e)))
                (e (let back ((e e)) (if (and (> e i) (memv (string-ref s (- e 1)) '(#\. #\, #\; #\: #\! #\? #\) #\* #\_ #\~ #\' #\"))) (back (- e 1)) e))))
           (and (> e (+ i 4)) e)))))

;; Emphasis from the run of * _ or ~ at I: (end . node), or #f.
(define (emphasis-at s i from to)
  (let* ((c (string-ref s i)) (n (run-of s i c)))
    (define (opens? d)
      (let ((after (and (< (+ i d) to) (string-ref s (+ i d)))))
        (and after (not (char-whitespace? after))
             (or (not (char=? c #\_)) (= i from) (not (word? (string-ref s (- i 1))))))))
    (define (closer d)
      (let loop ((j (+ i d)))
        (cond ((>= j to) #f)
              ((char=? (string-ref s j) #\`)
               (let* ((m (run-of s j #\`)) (e (closing-backticks s (+ j m) to m))) (loop (if e (+ e m) (+ j m)))))
              ((char=? (string-ref s j) #\\) (loop (+ j 2)))
              ((char=? (string-ref s j) c)
               ;; A run that can close and is long enough closes from its
               ;; first characters; what is left of it closes outer
               ;; emphasis. A run that can only open starts emphasis of
               ;; its own, skipped whole.
               (let ((m (run-of s j c)))
                 (cond ((and (>= m d) (> j (+ i d)) (not (char-whitespace? (string-ref s (- j 1))))
                             (or (not (char=? c #\_)) (>= (+ j m) to) (not (word? (string-ref s (+ j m))))))
                        j)
                       ((and (< (+ j m) to) (not (char-whitespace? (string-ref s (+ j m)))) (emphasis-at s j from to))
                        => (lambda (inner) (loop (car inner))))
                       (else (loop (+ j m))))))
              (else (loop (+ j 1))))))
    (if (char=? c #\~)
        (and (= n 2) (opens? 2)
             (let ((j (closer 2))) (and j (cons (+ j 2) (list 'strike (parse-inlines s (+ i 2) j))))))
        (let try ((d (min n 3)))
          (and (> d 0)
               (or (and (= d n) (opens? d)
                        (let ((j (closer d)))
                          (and j (cons (+ j d) (list (case d ((1) 'emph) ((2) 'strong) (else 'strong-emph))
                                                     (parse-inlines s (+ i d) j))))))
                   (try (- d 1))))))))

;;; Into rich text

;; The rich inlines of NODES, over the flow F of document D: text as
;; spans of D, a span a line, a line break between lines.
(define (rich-inlines d f nodes link)
  (append-map
   (lambda (n)
     (case (car n)
       ((text) (flow-spans d f (cadr n) (caddr n) #t))
       ((code) (list (cons 'code (flow-spans d f (cadr n) (caddr n) #f))))
       ((strong emph strike) (list (cons (car n) (rich-inlines d f (cadr n) link))))
       ((strong-emph) (list (list 'strong (cons 'emph (rich-inlines d f (cadr n) link)))))
       ((link) (list (cons* 'link (link (caddr n)) (rich-inlines d f (cadr n) link))))
       ((image) (list (cons* 'image (link (cadddr n)) (flow-spans d f (cadr n) (caddr n) #f))))
       (else '())))
   nodes))

(define (cons* x . rest) (if (null? rest) x (cons x (apply cons* rest))))

;; The flow's characters FROM to TO as spans, (break) at each line break.
;; With BREAKS, a line break after two spaces or more, or after a
;; backslash, is (hard-break), the spaces or backslash not shown.
(define (flow-spans d f from to breaks)
  (let ((s (flow-text f)))
    (let loop ((a from) (b from) (acc '()))
      (define (piece end) (if (< a end) (cons (list 'span d (flow-byte f a) (flow-byte f end)) acc) acc))
      (cond ((>= b to) (reverse (piece b)))
            ((char=? (string-ref s b) #\newline)
             (let* ((spaces (let back ((k b)) (if (and (> k a) (char=? (string-ref s (- k 1)) #\space)) (back (- k 1)) (- b k))))
                    (slash (and breaks (= spaces 0) (> b a) (char=? (string-ref s (- b 1)) #\\)))
                    (hard (and breaks (or slash (>= spaces 2)))))
               (loop (+ b 1) (+ b 1) (cons (if hard '(hard-break) '(break)) (piece (- b (if slash 1 spaces)))))))
            (else (loop a (+ b 1) acc))))))

(define (text-inlines d lines link)
  (let ((f (lines->flow lines)))
    (rich-inlines d f (parse-inlines (flow-text f) 0 (string-length (flow-text f))) link)))

(define (line-span d l) (list 'span d (line-start l) (+ (line-start l) (utf8-length (line-text l)))))

(define (rich-blocks d blocks link)
  (map (lambda (b)
         (case (car b)
           ((paragraph) (cons 'paragraph (text-inlines d (cadr b) link)))
           ((heading) (cons* 'heading (cadr b) (text-inlines d (caddr b) link)))
           ((code) (cons* 'code-block (and (not (string=? (cadr b) "")) (cadr b)) (map (lambda (l) (line-span d l)) (caddr b))))
           ((rule) '(rule))
           ((quote) (cons 'quote (rich-blocks d (cadr b) link)))
           ((list) (cons 'list (map (lambda (it)
                                      (cons* 'item (and (cadr b) (car it)) (cadr it) (rich-blocks d (caddr it) link)))
                                    (caddr b))))
           ((table) (cons* 'table (cadr b)
                           (map (lambda (c) (text-inlines d (list c) link)) (caddr b))
                           (map (lambda (r) (map (lambda (c) (text-inlines d (list c) link)) r)) (cadddr b))))
           (else '(paragraph))))
       blocks))

;; What a link's URL stands for: an address, else a file in DIRECTORY.
(define (url-target url directory)
  (cond ((or (string-prefix? "http://" url) (string-prefix? "https://" url) (string-prefix? "mailto:" url)) (target 'url url))
        ((or (string-prefix? "#" url) (not directory)) #f)
        (else
         (let ((path (let loop ((i 0)) (cond ((= i (string-length url)) url) ((char=? (string-ref url i) #\#) (substring url 0 i)) (else (loop (+ i 1)))))))
           (target 'file (if (string-prefix? "/" path) path (string-append directory path)))))))

(define (markdown->rich d #:directory [directory #f])
  "Return the markdown of document D as rich text (rich.scm).
Its text is spans of D. A link's target is its address, or a file:
one relative to DIRECTORY, a directory ending in /, when given."
  (rich-blocks d (parse-blocks (map (lambda (l) (line (car l) (cadr l))) (document-lines d)))
               (lambda (url) (url-target url directory))))

;;; Rendering again only what changed. A document is parsed into units,
;;; as rich text renders them (`rich-units'): its top-level blocks, and
;;; each item of a top-level list. A buffer keeps where each unit starts
;;; ('md-starts, at the revision rendered) and the parser's state there
;;; ('md-states: #f at the top level, the kind of the list an item goes
;;; on). After edits, parsing starts a unit before the first one they
;;; touch (whose extent or look ahead may reach them) and goes on until,
;;; past the last edit, it starts a unit where an old one started, in the
;;; same state: from there it would parse what it parsed before, so the
;;; old units are kept, moved. Only the units between are rendered again.

;; The units from LINES (a vector of lines, from the line at index FROM)
;; in parser STATE: (values units next state end?), each unit #(kind
;; block-or-item start-line end-line state first-of-list ordered?).
;; COMPLETE says the lines reach the text's end; STOP? (line state
;; count) stops before a unit. Raises markdown-need-more when the lines
;; given do not reach far enough.
(define (markdown-units lines from state complete stop?)
  (parse-blocks lines #:units (list from state complete stop?)))

;; Rich units of the parsed UNITS of document D, the first FIRST in the
;; document when FIRST.
(define (rich-of-units d units first link)
  (let loop ((us units) (first first) (acc '()))
    (if (null? us)
        (reverse acc)
        (let* ((u (car us)) (gap (not first)))
          (loop (cdr us) #f
                (cons (if (eq? (vector-ref u 0) 'block)
                          (list 'block gap (car (rich-blocks d (list (vector-ref u 1)) link)))
                          (list 'item (and gap (vector-ref u 5))
                                (cadr (car (rich-blocks d (list (list 'list (vector-ref u 6) (list (vector-ref u 1)))) link)))))
                      acc))))))

;; Where the edits since REVISION of D are, (from . to) now, '() for
;; none, or #f when the history no longer says.
(define (damage-since d revision)
  (let loop ((r (+ revision 1)) (from #f) (to #f))
    (if (> r (document-revision d))
        (if from (cons from to) '())
        (let ((spans (document-change-spans d r)))
          (and spans
               (loop (+ r 1)
                     (fold (lambda (s m) (if m (min m (car s)) (car s))) from spans)
                     (fold (lambda (s m) (if m (max m (cadr s)) (cadr s))) to spans)))))))

;; The index of the last element of the sorted vector V not above X, or -1.
(define (last-not-above v x)
  (let loop ((lo 0) (hi (vector-length v)))
    (if (>= lo hi) (- lo 1) (let ((mid (quotient (+ lo hi) 2))) (if (<= (vector-ref v mid) x) (loop (+ mid 1) hi) (loop lo mid))))))

(define (markdown-render! b d #:directory [directory #f])
  "Show the markdown of document D in buffer B, a rich buffer, again.
Only the units of D the edits since it was last shown touch are parsed
and rendered again; the first time, or when D's history no longer says
what changed, all of it is. Links are to files in DIRECTORY, as
`markdown->rich' takes it."
  (let* ((st (buffer-state b)) (link (lambda (url) (url-target url directory)))
         (rev (document-revision d)) (len (document-length d))
         ;; What was rendered, if of this document and directory: links
         ;; and pictures are resolved against it.
         (shown (and (eq? (hash-table-ref/default st 'md-document #f) d)
                     (equal? (hash-table-ref/default st 'md-directory #f) directory)
                     (hash-table-ref/default st 'md-revision #f)))
         (damage (and shown (damage-since d shown))))
    (define (lines-from pos count)
      (let ((got (document-lines-from d pos count)))
        (values (list->vector (map (lambda (l) (line (car l) (cadr l))) got)) (< (length got) count))))
    (define (remember! starts states)
      (hash-table-set! st 'md-document d)
      (hash-table-set! st 'md-directory directory)
      (hash-table-set! st 'md-revision rev)
      (hash-table-set! st 'md-length len)
      (hash-table-set! st 'md-starts starts)
      (hash-table-set! st 'md-states states))
    (cond
     ((and damage (null? damage)) (remember! (hash-table-ref st 'md-starts) (hash-table-ref st 'md-states)))
     ((not damage)
      (let ((v (list->vector (map (lambda (l) (line (car l) (cadr l))) (document-lines d)))))
        (call-with-values (lambda () (markdown-units v 0 #f #t (lambda (i s c) #f)))
          (lambda (units next state end)
            (rich-splice! b 0 (vector-length (hash-table-ref/default st 'units (vector))) (rich-of-units d units #t link))
            (remember! (list->vector (map (lambda (u) (line-start (vector-ref v (vector-ref u 2)))) units))
                       (list->vector (map (lambda (u) (vector-ref u 4)) units)))))))
     (else
      (let* ((starts (hash-table-ref st 'md-starts)) (states (hash-table-ref st 'md-states))
             (n (vector-length starts)) (delta (- len (hash-table-ref st 'md-length)))
             (a (car damage)) (z (cdr damage))
             ;; A unit before the first the edits touch: its look ahead
             ;; may reach them, a paragraph go on into them.
             (k (max 0 (- (last-not-above starts a) 1)))
             ;; From the text's start when that is the first unit: edits
             ;; may be in the blank lines before it.
             (from (if (and (> k 0) (< k n)) (vector-ref starts k) 0))
             (state (and (> k 0) (< k n) (vector-ref states k))))
        (let retry ((count 256))
          (call-with-values (lambda () (lines-from from count))
            (lambda (v complete)
              (let ((result
                     (guard (e ((eq? e 'markdown-need-more) #f))
                       (call-with-values
                           (lambda ()
                             (markdown-units v 0 state complete
                                             (lambda (i s c)
                                               (let* ((p (line-start (vector-ref v i))) (old (- p delta)) (r (last-not-above starts old)))
                                                 (and (>= p z) (>= r 0) (< r n) (> r k) (= (vector-ref starts r) old)
                                                      (eqv? (vector-ref states r) s)
                                                      ;; Not first in the text now, as it was not.
                                                      (not (and (= k 0) (= c 0))))))))
                         (lambda (units next s end)
                           (list units
                                 ;; The old units kept from here: past
                                 ;; the edits, where the parse stopped.
                                 (if end n (last-not-above starts (- (line-start (vector-ref v next)) delta)))))))))
                (if (not result)
                    (retry (* count 2))
                    (let* ((units (car result)) (r (cadr result)) (first (= k 0)))
                      (rich-splice! b k (- r k) (rich-of-units d units first link))
                      (remember! (vector-append (vector-copy starts 0 k)
                                                (list->vector (map (lambda (u) (line-start (vector-ref v (vector-ref u 2)))) units))
                                                (vector-map (lambda (p) (+ p delta)) (vector-copy starts r)))
                                 (vector-append (vector-copy states 0 k)
                                                (list->vector (map (lambda (u) (vector-ref u 4)) units))
                                                (vector-copy states r))))))))))))))

(define (markdown-escape text)
  "Return markdown that reads as TEXT, plain text.
Punctuation is escaped, a line break kept as a hard break, a blank line
as one, and the spaces a line starts with kept as no-break spaces."
  (let loop ((ls (string-split text "\n")) (acc '()))
    (if (null? ls)
        (apply string-append (reverse acc))
        (let* ((l (car ls)) (i (first-nonspace l)) (more (and (pair? (cdr ls)) (not (blank? (cadr ls))))))
          (loop (cdr ls)
                (cons (if (blank? l)
                          "\n"
                          (string-append (make-string i #\xA0)
                                         (apply string-append (map (lambda (c) (if (punct? c) (string #\\ c) (string c)))
                                                                   (string->list (substring l i (string-length l)))))
                                         (cond (more "\\\n") ((pair? (cdr ls)) "\n") (else ""))))
                      acc))))))
