;; Summarise an Org file: TODO headings per level and the most used tags.
;; Usage: techne-vm org-summary.scm FILE.org

(define-record-type heading
  (make-heading level keyword title tags)
  heading?
  (level heading-level)
  (keyword heading-keyword)
  (title heading-title)
  (tags heading-tags))

(define-syntax inc!
  (syntax-rules ()
    ((_ table key) (hash-table-update!/default table key (lambda (n) (+ n 1)) 0))))

(define (count-stars line)
  (let loop ((i 0))
    (if (and (< i (string-length line)) (char=? (string-ref line i) #\*)) (loop (+ i 1)) i)))

(define (parse-heading line)
  (let ((level (count-stars line)))
    (and (> level 0)
         (< level (string-length line))
         (char=? (string-ref line level) #\space)
         (let* ((words (string-split (substring line (+ level 1))))
                (keyword (and (pair? words) (member (car words) '("TODO" "DONE")) (car words)))
                (words (if keyword (cdr words) words))
                (tagged (and (pair? words) (string-prefix? ":" (last words))))
                (tags (if tagged (filter (lambda (t) (not (string-null? t))) (string-split (last words) ":")) '()))
                (title (string-join (if tagged (reverse (cdr (reverse words))) words) " ")))
           (make-heading level keyword title tags)))))

(define (summarise path)
  (let ((headings (filter-map parse-heading (file->lines path)))
        (tags (make-hash-table))
        (todo-by-level (make-hash-table)))
    (for-each (lambda (h)
                (for-each (lambda (t) (inc! tags t)) (heading-tags h))
                (when (equal? (heading-keyword h) "TODO") (inc! todo-by-level (heading-level h))))
              headings)
    (with-output-to-string
      (lambda ()
        (display `(headings ,(length headings)))
        (newline)
        (for-each (lambda (level) (display `(todo level ,level ,(hash-table-ref/default todo-by-level level 0))) (newline))
                  '(1 2 3))
        (for-each (lambda (kv) (display `(tag ,(car kv) ,(cdr kv))) (newline))
                  (take (sort (hash-table->alist tags) (lambda (a b) (> (cdr a) (cdr b)))) 3))))))

(define args (command-line))
(display
 (guard (e ((error-object? e) (string-append "failed: " (error-object-message e) "\n")))
   (summarise (if (> (length args) 1) (cadr args) "missing.org"))))
