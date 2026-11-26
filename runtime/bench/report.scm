;;; Changes worth showing in CI, separate from the complete history and
;;; regression gate maintained by github-action-benchmark.
;;;   techne run runtime/bench/report.scm HISTORY.js CURRENT.json
;;;   techne run runtime/bench/report.scm --test
(import (techne))

;; Instruction counts have a little noise even though they are much more
;; stable than wall time. Do not show a row whose ratio rounds to 1.00.
(define (ratio-text current previous)
  (if (zero? previous)
      (if (zero? current) "1.00" "new (was zero)")
      (let* ((hundredths (exact (floor (+ 1/2 (* 100 (/ current previous))))))
             (fraction (number->string (modulo hundredths 100))))
        (string-append (number->string (quotient hundredths 100)) "."
                       (if (< (string-length fraction) 2) "0" "") fraction))))

(define (report current baseline)
  (let* ((previous (if baseline (vector->list (dict-ref baseline "benches")) '()))
         (commit (and baseline (dict-ref baseline "commit" #f)))
         (revision (and commit (dict-ref commit "id" #f)))
         (rows
          (filter-map
           (lambda (entry)
             (let* ((name (dict-ref entry "name"))
                    (old (find (lambda (e) (string=? name (dict-ref e "name"))) previous))
                    (value (dict-ref entry "value"))
                    (ratio (if old (ratio-text value (dict-ref old "value")) "new")))
               (and (not (string=? ratio "1.00"))
                    (string-append "| `" name "` | "
                                   (if old (number->string (dict-ref old "value")) "—")
                                   " | " (number->string value) " | "
                                   (dict-ref entry "unit") " | " ratio " |"))))
           current)))
    (if (null? rows) ""
        (string-append "## Benchmark changes\n\n"
                       (if revision (string-append "Published baseline: `" revision "`.\n\n") "")
                       "Ratios are current / previous; lower is better. Rows rounding to 1.00 are omitted.\n\n"
                       "| Benchmark | Previous | Current | Unit | Ratio |\n"
                       "|---|---:|---:|---|---:|\n"
                       (string-join rows "\n") "\n"))))

(define (previous-run script)
  ;; data.js is JSON with a fixed assignment prefix, not executable input.
  (let ((prefix "window.BENCHMARK_DATA = "))
    (unless (string-prefix? prefix script) (error "unexpected benchmark history format"))
    (let* ((data (string->json (substring script (string-length prefix) (string-length script))))
           (entries (dict-ref (dict-ref data "entries") "techne-vm" #()))
           (count (vector-length entries)))
      (and (positive? count) (vector-ref entries (- count 1))))))

(define (tests)
  (define (check expected actual)
    (unless (equal? expected actual) (error "benchmark report test failed" expected actual)))
  (define (entry name value) (dict "name" name "unit" "instructions" "value" value))
  (define old (list (entry "same" 1000) (entry "faster" 1000) (entry "slower" 1000)))
  (define baseline (dict "commit" (dict "id" "baseline-sha") "benches" (list->vector old)))
  (check "1.00" (ratio-text 1000 1000))
  (check "1.00" (ratio-text 1004 1000))
  (check "1.00" (ratio-text 996 1000))
  (check "1.01" (ratio-text 1005 1000))
  (check "0.99" (ratio-text 994 1000))
  (check "1.00" (ratio-text 0 0))
  (check "0.00" (ratio-text 0 1000))
  (check "new (was zero)" (ratio-text 1 0))
  (check "" (report old baseline))
  (check "" (report (list (entry "same" 1004)) baseline))
  (let ((text (report (list (entry "same" 1000) (entry "faster" 900)
                            (entry "slower" 1100) (entry "added" 1)) baseline)))
    (check #f (string-contains text "`same`"))
    (for-each (lambda (s) (unless (string-contains text s) (error "missing report text" s)))
              '("`baseline-sha`" "| `faster` | 1000 | 900 | instructions | 0.90 |"
                "| `slower` | 1000 | 1100 | instructions | 1.10 |"
                "| `added` | — | 1 | instructions | new |")))
  (check #f (previous-run "window.BENCHMARK_DATA = {\"entries\":{}}"))
  (let* ((runs (vector (dict "benches" #()) baseline))
         (history (dict "entries" (dict "techne-vm" runs)))
         (latest (previous-run (string-append "window.BENCHMARK_DATA = " (json->string history)))))
    (check old (vector->list (dict-ref latest "benches"))))
  (unless (string-contains (report (list (entry "first" 1)) #f) "new")
    (error "first run must report new benchmarks"))
  (display "benchmark report tests passed\n"))

(define args (cdr (command-line)))
(cond ((equal? args '("--test")) (tests))
      ((= (length args) 2)
       (display (report (vector->list (string->json (file->string (cadr args))))
                        (previous-run (file->string (car args))))))
      (else (error "usage: report.scm HISTORY.js CURRENT.json (or --test)")))
