;;; Differences between two texts, line by line, as `diff -u` shows them:
;;; Myers's algorithm ("An O(ND) Difference Algorithm and Its
;;; Variations", 1986), after the lines the two begin and end with alike
;;; are set aside. `diff-mode` colours what it writes, and what git and
;;; jj write.

(require "session.scm")
(require "modes.scm")
(require "faces.scm")

(provide line-diff unified-diff text-lines)

(define (text-lines text)
  "Return the lines of TEXT, a vector, without their newlines.
A newline ending TEXT ends its last line, and starts none. A last line
without one keeps a newline after it instead, which no other line has:
it differs from the same line with one, as `diff` says it does."
  (let ((lines (string-split text "\n")))
    (cond ((null? lines) (vector))
          ((string=? (last lines) "") (list->vector (reverse (cdr (reverse lines)))))
          (else (list->vector (append (reverse (cdr (reverse lines))) (list (string-append (last lines) "\n"))))))))

(define (line-diff a b #:max-differences [max-differences 2000])
  "Return how to make the lines A into the lines B, vectors of strings.
A list of (= . line) kept, (- . line) deleted and (+ . line) added, in
order, as few deleted and added as can be. Past MAX-DIFFERENCES of them,
what differs is said as all deleted, then all added."
  (let* ((n (vector-length a)) (m (vector-length b))
         (pre (let loop ((i 0))
                (if (and (< i n) (< i m) (string=? (vector-ref a i) (vector-ref b i))) (loop (+ i 1)) i)))
         (suf (let loop ((i 0))
                (if (and (< i (- n pre)) (< i (- m pre))
                         (string=? (vector-ref a (- n i 1)) (vector-ref b (- m i 1))))
                    (loop (+ i 1))
                    i))))
    (append (lines '= a 0 pre)
            (myers a pre (- n suf) b pre (- m suf) max-differences)
            (lines '= a (- n suf) n))))

;; (OP . line) for the lines FROM to TO of V.
(define (lines op v from to)
  (let loop ((i (- to 1)) (acc '()))
    (if (< i from) acc (loop (- i 1) (cons (cons op (vector-ref v i)) acc)))))

;; The script from A's lines A0 to A1 to B's B0 to B1. V holds, by
;; diagonal k (x - y), how far along A the furthest path with d
;; differences reaches; each d's values are kept, those of the diagonals
;; it can reach from, for following the path back.
(define (myers a a0 a1 b b0 b1 max-d)
  (let* ((n (- a1 a0)) (m (- b1 b0)) (mx (+ n m)) (off (+ mx 1))
         (v (make-vector (+ (* 2 mx) 3) 0))
         (vref (lambda (k) (vector-ref v (+ k off))))
         (same? (lambda (x y) (string=? (vector-ref a (+ a0 x)) (vector-ref b (+ b0 y)))))
         ;; Diagonals -(d+1) to d+1 of V, as they are before step d.
         (save (lambda (d)
                 (let ((s (make-vector (+ (* 2 d) 3) 0)))
                   (do ((k (- (+ d 1)) (+ k 1))) ((> k (+ d 1)) (cons d s))
                     (when (<= (abs k) (+ mx 1)) (vector-set! s (+ k d 1) (vref k)))))))
         (down? (lambda (ref k d) (or (= k (- d)) (and (not (= k d)) (< (ref (- k 1)) (ref (+ k 1))))))))
    (let outer ((d 0) (trace '()))
      (if (> d (min mx max-d))
          (append (lines '- a a0 a1) (lines '+ b b0 b1))
          (let ((trace (cons (save d) trace)))
            (let inner ((k (- d)))
              (if (> k d)
                  (outer (+ d 1) trace)
                  (let* ((x (let snake ((x (if (down? vref k d) (vref (+ k 1)) (+ (vref (- k 1)) 1))))
                              (if (and (< x n) (< (- x k) m) (same? x (- x k))) (snake (+ x 1)) x))))
                    (vector-set! v (+ k off) x)
                    (if (and (>= x n) (>= (- x k) m))
                        (back a a0 b b0 trace n m down?)
                        (inner (+ k 2)))))))))))

;; Follow the path ending at (N, M) back through TRACE, newest step first.
(define (back a a0 b b0 trace n m down?)
  (let loop ((trace trace) (x n) (y m) (acc '()))
    (if (null? trace)
        acc
        (let* ((d (car (car trace))) (saved (cdr (car trace)))
               (ref (lambda (k) (vector-ref saved (+ k d 1))))
               (k (- x y))
               (pk (if (down? ref k d) (+ k 1) (- k 1)))
               (px (ref pk)) (py (- px pk)))
          (let snake ((x x) (y y) (acc acc))
            (if (and (> x px) (> y py) (> x 0) (> y 0))
                (snake (- x 1) (- y 1) (cons (cons '= (vector-ref a (+ a0 x -1))) acc))
                (if (= d 0)
                    acc
                    (loop (cdr trace) px py
                          (cons (if (= x px) (cons '+ (vector-ref b (+ b0 y -1))) (cons '- (vector-ref a (+ a0 x -1))))
                                acc)))))))))

(define (unified-diff a b #:from [from "a"] #:to [to "b"] #:context [context 3])
  "Return the differences of the texts A and B as `diff -u` writes them.
FROM and TO name them in its header; CONTEXT lines alike are shown
around each change. \"\" when their lines are the same."
  (let* ((script (list->vector (line-diff (text-lines a) (text-lines b))))
         (count (vector-length script))
         (changed (filter (lambda (i) (not (eq? (car (vector-ref script i)) '=))) (iota count))))
    (if (null? changed)
        ""
        ;; Hunks: runs of the script around changes, CONTEXT lines either
        ;; side, joined where they would meet.
        (let* ((hunks (reverse
                       (fold (lambda (i hunks)
                               (let ((start (max 0 (- i context))) (end (min count (+ i context 1))))
                                 (if (and (pair? hunks) (<= start (cdr (car hunks))))
                                     (cons (cons (car (car hunks)) end) (cdr hunks))
                                     (cons (cons start end) hunks))))
                             '() changed)))
               ;; The line of A and of B each entry of the script is at.
               (at (let ((v (make-vector (+ count 1))))
                     (let loop ((i 0) (ai 0) (bi 0))
                       (vector-set! v i (cons ai bi))
                       (when (< i count)
                         (let ((op (car (vector-ref script i))))
                           (loop (+ i 1) (if (eq? op '+) ai (+ ai 1)) (if (eq? op '-) bi (+ bi 1))))))
                     v)))
          (call-with-output-string
           (lambda (p)
             (display (string-append "--- " from "\n+++ " to "\n") p)
             (for-each
              (lambda (h)
                (let* ((s (vector-ref at (car h))) (e (vector-ref at (cdr h)))
                       (al (- (car e) (car s))) (bl (- (cdr e) (cdr s)))
                       ;; diff's: an empty side starts at the line before.
                       (range (lambda (start len)
                                (string-append (number->string (if (= len 0) start (+ start 1)))
                                               (if (= len 1) "" (string-append "," (number->string len)))))))
                  (display (string-append "@@ -" (range (car s) al) " +" (range (cdr s) bl) " @@\n") p)
                  (do ((i (car h) (+ i 1))) ((= i (cdr h)))
                    (let* ((entry (vector-ref script i)) (line (cdr entry)))
                      (display (case (car entry) ((=) " ") ((-) "-") (else "+")) p)
                      (display line p)
                      ;; A last line without a newline (`text-lines`).
                      (if (string-suffix? "\n" line)
                          (display "\\ No newline at end of file\n" p)
                          (newline p))))))
              hunks)))))))

;;; Showing differences

(define-face diff.added "A line a difference adds." #:fg 'green)
(define-face diff.removed "A line a difference deletes." #:fg 'red)
(define-face diff.hunk "Where a hunk of differences is: its @@ line." #:fg 'mauve)
(define-face diff.header "What a difference is between: its file lines." #:weight 'bold)

;; A line's face by how it starts.
(define %diff-lines
  (map (lambda (k) (cons (string->regexp (car k)) (cdr k)))
       '(("(?m)^(?:---|\\+\\+\\+|diff |index ).*$" . diff.header)
         ("(?m)^@@.*$" . diff.hunk)
         ("(?m)^\\+.*$" . diff.added)
         ("(?m)^-.*$" . diff.removed))))

(define-mode diff-mode
  "Differences between texts, as `diff -u` writes them.
What a hunk deletes and adds is coloured, as are its headers."
  #:parent 'log-mode
  #:layer (lambda (d from to)
            ;; Headers first: a --- line is not one deleted.
            (let loop ((kinds %diff-lines) (spans '()))
              (if (null? kinds)
                  (sort spans (lambda (a b) (< (car a) (car b))))
                  (loop (cdr kinds)
                        (fold (lambda (span acc)
                                (if (any (lambda (s) (= (car s) (car span))) acc)
                                    acc
                                    (cons (list (car span) (cadr span) (cdr (car kinds))) acc)))
                              spans
                              (search-text-regexp-all d (car (car kinds)) from to)))))))
