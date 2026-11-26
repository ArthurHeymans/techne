;;; Compare two Lisp trees with the same release editor probe binary.
;;; techne run runtime/bench/editor-syntax-startup.scm PROBE BEFORE-LISP AFTER-LISP [SAMPLES]
(import (techne) (srfi 132))

(define args (cdr (command-line)))
(unless (<= 3 (length args) 4) (error "expected PROBE BEFORE-LISP AFTER-LISP [SAMPLES]"))
(define bin (file-truename (car args)))
(define lisp (map file-truename (list (cadr args) (caddr args))))
;; Pin both runs to the same core if CPU is set. Otherwise no taskset is needed.
(define cpu (get-environment-variable "CPU"))
(define runs (string->number (or (get-environment-variable "RUNS") "7")))
(unless (and (exact-integer? runs) (positive? runs)) (error "expected RUNS > 0"))

(define (run side)
  (call-with-process (if cpu "taskset" bin) (if cpu (list "-c" cpu bin) '())
    (lambda (p)
      (let* ((t (spawn (lambda () (process-read-all p 'stderr))))
             (o (process-read-all p 'stdout)) (e (task-join t)))
        (unless (eqv? (process-wait p) 0) (error "profile_syntax failed" e))
        (let ((row (read (open-input-string o))))
          (unless (and (list? row) (= (length row) 5) (eq? (car row) 'syntax)
                       (every (lambda (x) (and (exact-integer? x) (>= x 0))) (cdr row)))
            (error "bad phase result" row))
          (cons side (append (cdr row) (list (+ (cadr row) (caddr row))))))))
    #:environment (list (cons "TECHNE_LISP_DIR" (list-ref lisp side)))))

(define (median-ms xs)
  (let* ((v (list->vector (list-sort < xs))) (n (vector-length v)) (h (quotient n 2)))
    (/ (if (odd? n) (vector-ref v h) (/ (+ (vector-ref v (- h 1)) (vector-ref v h)) 2.0)) 1000.0)))
(define all
  (apply append (map (lambda (i) (map run (if (even? i) '(0 1) '(1 0)))) (iota runs))))
(for-each (lambda (side)
            (let ((rows (filter-map (lambda (r) (and (= side (car r)) (cdr r))) all)))
              (write (list (if (= side 0) 'before 'after) 'open/snapshot/edit/jump/ready-ms (apply map (lambda us (median-ms us)) rows)))
              (newline))) '(0 1))
(when (= (length args) 4)
  (call-with-output-file (cadddr args) (lambda (p) (write all p) (newline p))))
