;;; One line of run.sh's results.tsv from a wrk --latency log:
;;;
;;;   techne run wrk.scm IMPL THREADS PATH CONNS IDLE RSS_MB LOG
;;;
;;; The requests per second, the 50th and 99th percentile latencies in ms
;;; (empty where wrk did not report them), and the socket errors and
;;; non-2xx responses it counted, after the run's own columns.

(import (techne) (techne format) (srfi 115))

(define args (cdr (command-line)))
(define text (file->string (list-ref args 6)))
(define lines (string-split text #\newline))

;; The first submatches of RE's first match in a line of the log.
(define (search re)
  (let loop ((lines lines))
    (cond ((null? lines) #f)
          ((regexp-search re (car lines)) => regexp-match->list)
          (else (loop (cdr lines))))))

(define (latency percentile)
  (let ((m (search `(: bos (+ space) ,percentile "%" (+ space)
                       ($ (+ (or digit "."))) ($ (or "us" "ms" "s")) eow))))
    (if (not m)
        ""
        (number->decimal (* (string->number (cadr m))
                            (cdr (assoc (caddr m) '(("us" . 1e-3) ("ms" . 1) ("s" . 1e3)))))
                         3))))

(define rps (cadr (search '(: "Requests/sec:" (+ space) ($ (+ (or digit ".")))))))

(define errors
  (string-trim
   (apply string-append
          (regexp-extract '(or (: "Socket errors:" (* nonl)) (: "Non-2xx" (* nonl))) text))))

(display (string-join (append (take args 5)
                              (list (number->decimal (inexact (string->number rps)) 0)
                                    (latency "50") (latency "99")
                                    (list-ref args 5) errors))
                      "\t"))
(newline)
