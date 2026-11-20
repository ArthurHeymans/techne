;;; How a patch's lines are drawn: each with its face, and, where a run
;;; of deleted lines is followed by as many added ones, the words that
;;; differ between each pair marked, as Magit's refined hunks are. The
;;; words are compared with the editor's own Myers diff (diff.scm).

(import (techne editor))
(require "patch-syntax.scm")

(provide patch-line-runs refine-patch-lines)

;; Git's stat rows end in ` | NUM ++--`. Work backwards so a pipe or
;; plus/minus in the filename stays plain. Binary and rename-only stats
;; have no bar to colour.
(define (diffstat-runs line)
  (and (string-prefix? " " line)
       (let* ((end (string-length line))
              (minus (scan-back line end (lambda (c) (char=? c #\-))))
              (plus (scan-back line minus (lambda (c) (char=? c #\+))))
              (num-end (scan-back line plus char-whitespace?))
              (num-start (scan-back line num-end char-numeric?))
              (pipe (scan-back line num-start char-whitespace?)))
         (and (< plus end) (< num-end plus) (< num-start num-end) (< pipe num-start)
              (> pipe 1) (char=? (string-ref line (- pipe 1)) #\|)
              (append (list (substring line 0 plus))
                      (if (< plus minus) (list (list (substring line plus minus) 'diff.added)) '())
                      (if (< minus end) (list (list (substring line minus end) 'diff.removed)) '()))))))

(define (scan-back text at pred)
  (if (and (> at 0) (pred (string-ref text (- at 1)))) (scan-back text (- at 1) pred) at))

(define (refine-patch-lines lines #:path [path #f] #:stat [stat? #f])
  "Return the presentation runs for each patch line in LINES.
Mark differing words, colour source syntax using file headers or PATH,
and colour Git's stat bars before the patch when STAT? is true."
  (let ((marks (patch-syntax lines path)))
    (let loop ((lines lines) (runs (refine-lines lines)) (i 0) (stat? stat?) (out '()))
      (if (null? lines) (reverse out)
          (let* ((line (car lines))
                 (stat? (and stat? (not (or (string-prefix? "diff " line) (string-prefix? "@@" line)))))
                 (faces (hash-table-ref/default marks i '()))
                 (row (or (and stat? (diffstat-runs line))
                          (if (null? faces) (car runs) (paint-patch-line (car runs) faces)))))
            (loop (cdr lines) (cdr runs) (+ i 1) stat? (cons row out)))))))

(define-face diff.refine-removed "Words a changed line no longer has." #:fg 'red #:bg '("#5a2b38" 1) #:weight 'bold)
(define-face diff.refine-added "Words a changed line gained." #:fg 'green #:bg '("#2b4d36" 2) #:weight 'bold)
(define-face diff.context "A line a patch keeps, around what it changes.")

;; The words, spaces and single punctuation characters of TEXT, a vector.
(define (tokens text)
  (let ((n (string-length text)))
    (let loop ((i 0) (out '()))
      (if (>= i n) (list->vector (reverse out))
          (let* ((c (string-ref text i))
                 (kind (cond ((or (char-alphabetic? c) (char-numeric? c) (char=? c #\_)) 'word)
                             ((char-whitespace? c) 'space)
                             (else 'mark)))
                 (end (if (eq? kind 'mark) (+ i 1)
                          (let scan ((j (+ i 1)))
                            (if (and (< j n)
                                     (let ((d (string-ref text j)))
                                       (if (eq? kind 'word)
                                           (or (char-alphabetic? d) (char-numeric? d) (char=? d #\_))
                                           (char-whitespace? d))))
                                (scan (+ j 1)) j)))))
            (loop end (cons (substring text i end) out)))))))

(define (line-face line)
  (cond ((string-prefix? "@@" line) 'diff.hunk)
        ((or (string-prefix? "diff " line) (string-prefix? "index " line) (string=? line "---")
             (string-prefix? "commit " line)
             (string-prefix? "--- " line) (string-prefix? "+++ " line)
             (string-prefix? "new file" line) (string-prefix? "deleted file" line)
             (string-prefix? "rename " line) (string-prefix? "similarity " line)
             (string-prefix? "old mode" line) (string-prefix? "new mode" line)) 'diff.header)
        ((string-prefix? "+" line) 'diff.added)
        ((string-prefix? "-" line) 'diff.removed)
        (else 'diff.context)))

(define (patch-line-runs line)
  "Return the runs drawing the patch LINE in its face."
  (list (list line (line-face line))))

;; Runs for a deleted and an added line, the words only one has marked.
(define (refine-pair old new)
  (let* ((a (tokens (substring old 1 (string-length old))))
         (b (tokens (substring new 1 (string-length new))))
         (script (line-diff a b #:max-differences 200))
         (kept (count (lambda (e) (eq? (car e) '=)) script)))
    ;; Lines with nothing in common are said as wholly replaced.
    (if (or (= kept 0) (> (- (length script) kept) (* 2 (+ kept 1))))
        #f
        (let ((side (lambda (keep mark plain face)
                      (cons (list (substring (if (eq? mark '-) old new) 0 1) plain)
                            (filter-map (lambda (e)
                                          (cond ((eq? (car e) '=) (list (cdr e) plain))
                                                ((eq? (car e) mark) (list (cdr e) face))
                                                (else #f)))
                                        script)))))
          (cons (side '= '- 'diff.removed 'diff.refine-removed)
                (side '= '+ 'diff.added 'diff.refine-added))))))

(define (refine-lines lines)
  (let loop ((rest lines) (out '()))
    (cond ((null? rest) (reverse out))
          ((and (string-prefix? "-" (car rest)) (not (string-prefix? "---" (car rest))) (not (string=? (car rest) "---")))
           (let* ((removed (let take-run ((r rest) (acc '()))
                             (if (and (pair? r) (string-prefix? "-" (car r)) (not (string-prefix? "---" (car r))))
                                 (take-run (cdr r) (cons (car r) acc)) (reverse acc))))
                  (after (drop rest (length removed)))
                  (added (let take-run ((r after) (acc '()))
                           (if (and (pair? r) (string-prefix? "+" (car r)) (not (string-prefix? "+++" (car r))))
                               (take-run (cdr r) (cons (car r) acc)) (reverse acc))))
                  (pairs (and (= (length removed) (length added)) (<= (length removed) 40)
                              (every (lambda (l) (< (string-length l) 400)) (append removed added))
                              (map refine-pair removed added))))
             (if (and pairs (every (lambda (p) p) pairs))
                 (loop (drop after (length added))
                       (append (reverse (map cdr pairs)) (reverse (map car pairs)) out))
                 (loop (drop after (length added))
                       (append (reverse (map patch-line-runs added)) (reverse (map patch-line-runs removed)) out)))))
          (else (loop (cdr rest) (cons (patch-line-runs (car rest)) out))))))
