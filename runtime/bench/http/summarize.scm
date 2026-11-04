;;; Markdown tables from a run.sh results.tsv:
;;;
;;;   techne run summarize.scm results/DATE/results.tsv

(import (techne) (techne format))

;; The file is run.sh's own: tab-separated, nothing quoted.
(define lines
  (filter (lambda (line) (not (string=? line ""))) (file->lines (cadr (command-line)))))
(define columns (if (null? lines) '() (string-split (car lines) #\tab)))

;; Each row as an alist from column name to its text.
(define rows
  (if (null? lines) '() (map (lambda (line) (map cons columns (string-split line #\tab))) (cdr lines))))

(define (field row column) (cdr (assoc column row)))

(define implementations (delete-duplicates (map (lambda (r) (field r "impl")) rows)))

;; A table of the rows KEEP selects: one line per implementation, a cell
;; per value of COLUMN in VALUES, CELL rendering a row.
(define (table title keep column values cell)
  (let ((kept (filter keep rows)))
    (display (string-append "### " title "\n\n"))
    (display (string-append "| impl | " (string-join values " | ") " |\n"))
    (display (string-append (apply string-append (map (lambda (_) "|---") (cons "impl" values))) "|\n"))
    (for-each
     (lambda (impl)
       (let ((mine (filter (lambda (r) (string=? (field r "impl") impl)) kept)))
         (unless (null? mine)
           (display
            (string-append
             "| " impl " | "
             (string-join
              (map (lambda (value)
                     ;; The last row for a value, as results.tsv appends.
                     (let ((found (filter (lambda (r) (string=? (field r column) value)) mine)))
                       (if (null? found) "" (cell (last found)))))
                   values)
              " | ")
             " |\n")))))
     implementations)
    (newline)))

(define (thousands row)
  (string-append (number->decimal (inexact (/ (string->number (field row "rps")) 1000)) 0) "k"))

(define (plain? row #:threads [threads #f] #:conns [conns #f])
  (and (string=? (field row "path") "/")
       (string=? (field row "idle") "0")
       (or (not threads) (string=? (field row "threads") threads))
       (or (not conns) (string=? (field row "conns") conns))))

(table "Plaintext, 256 connections, by server threads (req/s)"
       (lambda (r) (plain? r #:conns "256")) "threads" '("1" "2" "4" "8") thousands)
(table "/work, 256 connections, by server threads (req/s)"
       (lambda (r) (string=? (field r "path") "/work")) "threads" '("1" "4" "8") thousands)
(table "Plaintext, 1 thread, by connections (req/s / p99 ms)"
       (lambda (r) (plain? r #:threads "1")) "conns" '("16" "256" "1024" "4096")
       (lambda (r) (string-append (thousands r) " / "
                                  (number->decimal (inexact (string->number (field r "p99_ms"))) 1))))
(table "Plaintext, 1 thread, 16 busy connections, by idle connections (req/s / RSS)"
       (lambda (r) (and (string=? (field r "path") "/") (string=? (field r "threads") "1")
                        (string=? (field r "conns") "16")))
       "idle" '("0" "1000" "10000")
       (lambda (r) (string-append (thousands r) " / " (field r "rss_mb") " MB")))
