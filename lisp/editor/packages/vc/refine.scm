;;; How a patch's lines are drawn: each with its face, and, where a run
;;; of deleted lines is followed by as many added ones, the words that
;;; differ between each pair marked, as Magit's refined hunks are. The
;;; words are compared with the editor's own Myers diff (diff.scm).

(import (techne editor))

(provide patch-line-runs refine-patch-lines)

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

(define (refine-patch-lines lines)
  "Return the runs of each of LINES, a patch's lines, in order.
Deleted lines followed by as many added ones have their differing words
marked."
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
