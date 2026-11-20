;;; Column layout for history rows. Keep refs styled within the subject.
(import (techne editor))
(provide vc-log-column-runs)

(define (cell-width c)
  (let ((n (char->integer c)))
    (cond ((or (<= #x0300 n #x036f) (<= #xfe00 n #xfe0f) (= n #x200d)) 0)
          ((or (<= #x1100 n #x115f) (<= #x2e80 n #xa4cf) (<= #xac00 n #xd7a3)
               (<= #xf900 n #xfaff) (<= #xff00 n #xff60) (<= #x1f300 n #x1faff)
               (<= #x20000 n #x3fffd)) 2)
          (else 1))))
(define (text-width text) (let loop ((i 0) (n 0))
    (if (= i (string-length text)) n (loop (+ i 1) (+ n (cell-width (string-ref text i)))))))
(define (run-text run) (if (string? run) run (car run)))
(define (runs-width runs) (fold (lambda (r n) (+ n (text-width (run-text r)))) 0 runs))
(define (spaces n) (make-string (max 0 n) #\space))

(define (fit runs width)
  (cond ((<= width 0) '())
        ((<= (runs-width runs) width) runs)
        (else
         (let loop ((rest runs) (room (- width 1)) (out '()))
           (let* ((r (car rest)) (text (run-text r)) (n (text-width text)))
             (if (<= n room) (loop (cdr rest) (- room n) (cons r out))
                 (let* ((end (let cut ((i 0) (left room))
                               (let ((w (cell-width (string-ref text i))))
                                 (if (<= w left) (cut (+ i 1) (- left w)) i))))
                        (part (substring text 0 end)))
                   (reverse (cons (list "…" 'shadow)
                                  (if (zero? end) out (cons (if (string? r) part (list part (cadr r))) out)))))))))))

(define (vc-log-column-runs columns prefix graph subject author age)
  "Lay out a Git log row in COLUMNS.
Put GRAPH and SUBJECT runs after PREFIX. Keep AUTHOR and AGE in the
right columns when there is enough room."
  (let* ((left (append prefix (list (list graph 'vc.graph))))
         (room (- columns (runs-width left)))
         (age-width (if (and (>= columns 52) (>= room 38)) 13 0))
         (author-width (if (and (>= columns 78) (>= room 62)) 18 0))
         (main-width (max 1 (- room (if (> age-width 0) (+ age-width 2) 0)
                                    (if (> author-width 0) (+ author-width 2) 0))))
         (subject (fit subject main-width))
         (author (fit (list (list author 'vc.author)) author-width))
         (age-text (if (string-suffix? " ago" age) (substring age 0 (- (string-length age) 4)) age))
         (age (fit (list (list age-text 'shadow)) age-width)))
    (append left subject
            (if (zero? age-width) '() (list (spaces (+ 2 (- main-width (runs-width subject))))))
            (if (zero? author-width) '() (append author (list (spaces (+ 2 (- author-width (runs-width author)))))))
            (if (zero? age-width) '() (cons (spaces (- age-width (runs-width age))) age)))))
