;;; Census guest heap allocations in unchanged steady-state source.
;;; techne run runtime/bench/allocations.scm VM [SOURCE.scm [OUTPUT.json]]
;;; PROBES='((round 1) (round 2) (round 3) (run))' selects HOF phases.
;;; WARMUP defaults to three run calls; CPU defaults to 4. Counts, not times:
;;; the opt-in census walks completed nursery allocations at GC boundaries.
(import (techne) (srfi 115))

(define args (cdr (command-line)))
(unless (pair? args) (error "allocations.scm needs VM [SOURCE.scm [OUTPUT.json]]"))
(define binary (file-truename (car args)))
(define here
  (regexp-match-submatch
   (regexp-search '(: bos ($ (* any)) "/" (* (~ #\/)) eos) (file-truename (car (command-line)))) 1))
(define (at path) (string-append here "/" path))
(define source (file-truename (if (> (length args) 1) (cadr args) (at "steady/hof.scm"))))
(define probes (read (open-input-string (or (get-environment-variable "PROBES") "((run))"))))
(define warmup (string->number (or (get-environment-variable "WARMUP") "3")))
(define cpu (or (get-environment-variable "CPU") "4"))
(unless (and (list? probes) (pair? probes) (every pair? probes) (integer? warmup) (>= warmup 0))
  (error "PROBES must be a nonempty list of forms; WARMUP a nonnegative integer"))
(define (written x)
  (let ((p (open-output-string))) (write x p) (get-output-string p)))
(make-directory (at "build/allocations"))
(define file (at "build/allocations/probe.scm"))
(call-with-output-file file
  (lambda (p)
    (display (file->string (at "prelude/techne.scm")) p) (newline p)
    (display (file->string source) p) (newline p)
    (write `(define (allocation-probe)
              (do ((i 0 (+ i 1))) ((= i ,warmup)) (run))
              (gc-stats)
              ,@(map (lambda (form) `(begin (write ,form) (newline) (gc-stats))) probes)) p)
    (newline p) (write '(allocation-probe) p) (newline p)))
(define output
  (call-with-process "taskset" (list "-c" cpu binary file)
    (lambda (p)
      (let* ((err (spawn (lambda () (process-read-all p 'stderr))))
             (out (process-read-all p 'stdout))
             (err (task-join err)) (status (process-wait p)))
        (unless (eqv? status 0) (error "allocation probe failed" status err))
        (list out err)))
    #:directory here
    #:environment '(("TECHNE_ALLOC_CENSUS" . "1") ("TECHNE_JIT" . "1000") ("TECHNE_JIT_SYNC" . "1"))))
(define fields '(nursery_objects nursery_words pairs pair_words bigints bigint_words direct_old_objects direct_old_words))
(define (snapshot line)
  (unless (regexp-search "truncated: false" line) (error "incomplete nursery census" line))
  (apply dict
    (append-map
     (lambda (field)
       (let* ((key (symbol->string field))
              (m (regexp-search `(: ,key ": " ($ (+ digit))) line)))
         (unless m (error "missing census field" field line))
         (list key (string->number (regexp-match-submatch m 1))))) fields)))
(define snapshots
  (map snapshot (filter (lambda (s) (string-prefix? "allocation-census: " s))
                        (string-split (cadr output) #\newline))))
;; One initial snapshot, one after each form, and the CLI's final report.
(unless (= (length snapshots) (+ (length probes) 2))
  (error "VM needs allocation-census feature and TECHNE_ALLOC_CENSUS=1" (length snapshots) (cadr output)))
(define results (filter (lambda (s) (not (string=? s ""))) (string-split (car output) #\newline)))
(unless (= (length results) (length probes)) (error "probes must print only their returned values" (car output)))
(define records
  (map
   (lambda (form result before after)
     (let* ((delta (apply dict
                         (append-map (lambda (field)
                                       (let ((key (symbol->string field)))
                                         (list key (- (dict-ref after key) (dict-ref before key))))) fields)))
            (words (+ (dict-ref delta "nursery_words") (dict-ref delta "direct_old_words")))
            (pair-share (if (zero? words) 0.0 (/ (inexact (dict-ref delta "pair_words")) words)))
            (integer-share (if (zero? words) 0.0 (/ (inexact (dict-ref delta "bigint_words")) words))))
       (write (list form 'result result 'pairs (dict-ref delta "pairs")
                    'pair-bytes (* 8 (dict-ref delta "pair_words"))
                    'bigints (dict-ref delta "bigints") 'bigint-bytes (* 8 (dict-ref delta "bigint_words"))
                    'direct-old-bytes (* 8 (dict-ref delta "direct_old_words")))) (newline)
       (when (positive? (dict-ref delta "direct_old_words"))
         (display "warning: direct old allocations have no kind classification\n" (current-error-port)))
       (dict "probe" (written form) "result" result "counts" delta
             "pair_share" pair-share "bigint_share" integer-share
             "all_allocations_in_nursery" (zero? (dict-ref delta "direct_old_words")))))
   probes results (take snapshots (length probes)) (take (cdr snapshots) (length probes))))
(when (> (length args) 2)
  (call-with-output-file (caddr args)
    (lambda (p)
      (display (json->string (dict "binary" binary "source" source "cpu" cpu "warmup_runs" warmup
                                  "records" (list->vector records) "log" (cadr output))) p)
      (newline p))))
