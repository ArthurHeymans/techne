;;; One or more results directories of run.sh's hyperfine JSON files as a
;;; Markdown table, each program's mean wall time per implementation in
;;; ms, with its deviation where that is over a tenth of the mean:
;;;
;;;   techne run summarize.scm results/DATE...
;;;
;;; A later directory's result for a program and implementation replaces
;;; an earlier one's.

(import (techne) (techne format))

(define programs '("startup" "fib" "tak" "nqueens" "bintrees" "hof" "qsort" "mandel" "hash" "orgparse" "ffi"))
(define implementations
  '("chez" "guile" "luajit" "lua5.4" "python3" "emacs-elc" "emacs-native2" "emacs-native3" "techne"
    "steel-stock" "steel-fork" "fork-engine" "fork-engine-ir" "fork-resumable" "fork-native"))
(define directories (cdr (command-line)))

(define (results-file directory program)
  (string-append directory "/" program ".json"))

;; PROGRAM's results as (command . result) pairs, the one that counts for
;; a command first: the last of the latest directory's.
(define (results program)
  (fold (lambda (directory found)
          (let ((file (results-file directory program)))
            (if (file-exists? file)
                (append (reverse (map (lambda (r) (cons (dict-ref r "command") r))
                                      (vector->list (dict-ref (string->json (file->string file)) "results"))))
                        found)
                found)))
        '() directories))

;; A program has a line when any directory has its file, results or none.
(define (measured? program)
  (any (lambda (directory) (file-exists? (results-file directory program))) directories))

(define (ms seconds) (number->decimal (* seconds 1000) 0))

(define (cell found implementation)
  (let ((entry (assoc implementation found)))
    (if (not entry)
        "–"
        (let ((mean (dict-ref (cdr entry) "mean"))
              (deviation (dict-ref (cdr entry) "stddev")))
          (if (and (number? deviation) (not (zero? deviation)) (> deviation (* 0.1 mean)))
              (string-append (ms mean) " ±" (ms deviation))
              (ms mean))))))

(define (row cells)
  (string-append "| " (string-join cells " | ") " |\n"))

(display (row (cons "program" implementations)))
(display (string-append (apply string-append (map (lambda (_) "|---") (cons "program" implementations))) "|\n"))
(for-each (lambda (program)
            (when (measured? program)
              (let ((found (results program)))
                (display (row (cons program (map (lambda (i) (cell found i)) implementations)))))))
          programs)
