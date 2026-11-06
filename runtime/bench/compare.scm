;;; Compare two frozen VM binaries on unchanged steady-state programs.
;;;   techne run runtime/bench/compare.scm BEFORE AFTER [OUTPUT.json]
;;; Run inside nix develop for valgrind. PROGS selects workloads, RUNS
;;; process samples (default 3), ITERS calls per sample (default 10), CPU
;;; the pinned core (default 4). COUNTS=1 uses Cachegrind instructions
;;; instead of wall time (including boot and compilation, not hot-only).
;;; JIT compilation is synchronous at threshold 1000 in both binaries.
;;; Logs retain GC work, native compilation time, and IR size. Results
;;; must agree, even when the sampled targets or collection paths differ.
;;; BEFORE_ENV / AFTER_ENV add space-separated NAME=VALUE overrides, e.g.
;;; BEFORE_ENV=TECHNE_JIT_FEEDBACK=0 to ablate feedback in the first binary.
(import (techne) (srfi 115) (srfi 132))

(define args (cdr (command-line)))
(unless (>= (length args) 2) (error "compare.scm needs BEFORE AFTER [OUTPUT.json]"))
(define binaries (map file-truename (take args 2)))
(define here
  (regexp-match-submatch
   (regexp-search '(: bos ($ (* any)) "/" (* (~ #\/)) eos) (file-truename (car (command-line)))) 1))
(define (at path) (string-append here "/" path))
(define (env name default) (or (get-environment-variable name) default))
(define (words text) (regexp-extract '(+ (~ space)) text))
(define programs (words (env "PROGS" "indirect direct fib tak nqueens bintrees hof qsort mandel hash orgparse")))
(define runs (string->number (env "RUNS" "3")))
(define iters (env "ITERS" "10"))
(define counts? (string=? (env "COUNTS" "0") "1"))
(define cpu (env "CPU" "4"))
(unless (and runs (positive? runs) (string->number iters) (>= (string->number iters) 2))
  (error "RUNS must be positive and ITERS at least 2"))

(define (run program arguments environment)
  (call-with-process program arguments
    (lambda (p)
      (let* ((err (spawn (lambda () (process-read-all p 'stderr))))
             (out (process-read-all p 'stdout))
             (err (task-join err))
             (status (process-wait p)))
        (unless (eqv? status 0) (error "comparison child failed" program status err))
        (list out err)))
    #:directory here #:environment environment))

(define (median values)
  (let* ((v (list->vector (list-sort < values))) (n (vector-length v)) (half (quotient n 2)))
    (if (odd? n) (vector-ref v half) (/ (+ (vector-ref v (- half 1)) (vector-ref v half)) 2))))

(make-directory (at "build/compare"))
(unless (file-exists? (at "org-sample.org")) (run "python3" '("gen-org.py") '()))
(define prelude (file->string (at "prelude/techne.scm")))
(define driver (file->string (at "steady/driver.scm")))
(define records '())
(define (save!)
  (when (>= (length args) 3)
    (call-with-output-file (list-ref args 2)
      (lambda (p) (display (json->string (list->vector (reverse records))) p) (newline p)))))
(define (overrides index)
  (map (lambda (setting)
         (let ((m (regexp-search '(: bos ($ (+ (~ #\=))) "=" ($ (* any)) eos) setting)))
           (unless m (error "expected NAME=VALUE" setting))
           (cons (regexp-match-submatch m 1) (regexp-match-submatch m 2))))
       (words (env (if (= index 0) "BEFORE_ENV" "AFTER_ENV") ""))))
(for-each
 (lambda (program)
   (let ((file (at (string-append "build/compare/" program ".scm")))
         (expected #f) (before '()) (after '()))
     (call-with-output-file file
       (lambda (p) (display prelude p) (display (file->string (at (string-append "steady/" program ".scm"))) p)
         (newline p) (display driver p)))
     (do ((sample 0 (+ sample 1))) ((= sample runs))
       ;; Alternate order to avoid giving either binary all the early runs.
       (for-each
        (lambda (index)
          (let* ((binary (list-ref binaries index))
                 (command (if counts? "valgrind" "taskset"))
                 (arguments (if counts?
                                 (list "--tool=cachegrind" "--cache-sim=no" "--cachegrind-out-file=/dev/null" binary file)
                                 (list "-c" cpu binary file)))
                 (result (run command arguments
                              (append (list (cons "ITERS" iters) '("TECHNE_JIT" . "1000") '("TECHNE_JIT_SYNC" . "1")
                                            '("TECHNE_JIT_LOG" . "1") '("TECHNE_JIT_IR" . "1") '("TECHNE_GC_STATS" . "1"))
                                      (overrides index))))
                 (lines (filter (lambda (s) (not (string=? s ""))) (string-split (car result) #\newline)))
                 (answer (car lines))
                 (times (map string->number (words (last lines))))
                 (match (and counts? (regexp-search '(: "I" (* space) "refs:" (* space) ($ (+ (or digit ",")))) (cadr result))))
                 (value (if counts?
                            (and match (string->number (regexp-replace-all "," (regexp-match-submatch match 1) "")))
                            (/ (inexact (median (list-tail times (quotient (length times) 2)))) 1000.))))
            (unless (and value (every number? times) (= (length times) (string->number iters)))
              (error "malformed benchmark output" program result))
            (if expected (unless (string=? answer expected) (error "results differ" program expected answer)) (set! expected answer))
            (if (= index 0) (set! before (cons value before)) (set! after (cons value after)))
            (set! records (cons (dict "program" program "binary" binary "side" (if (= index 0) "before" "after") "sample" sample "value" value
                                     "unit" (if counts? "whole-process instructions" "hot ms") "result" answer
                                     "iterations_us" (list->vector times) "log" (cadr result)) records))
            (save!)))
        (if (even? sample) '(0 1) '(1 0))))
     (display (list program 'before (median before) 'after (median after)
                    'ratio (/ (inexact (median after)) (median before)))) (newline)))
 programs)
(save!)
