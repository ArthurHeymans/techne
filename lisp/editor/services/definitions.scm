;;; A service finding definitions (`examples/sibling.rs` measures it): the
;;; lines starting a top-level `(define`, with the name each defines. Its
;;; world has no editor; it reads the texts it is given as snapshots.

(define (definitions-in text)
  "Return (name position) of each line of TEXT starting with (define.
Positions are byte offsets in TEXT's UTF-8, as the editor's are."
  (let* ((bytes (string->utf8 text)) (n (bytevector-length bytes)))
    (define (byte i) (bytevector-u8-ref bytes i))
    ;; Whether BYTES has PREFIX, ASCII, at I.
    (define (at? prefix i)
      (let ((m (string-length prefix)))
        (and (<= (+ i m) n)
             (let same ((k 0)) (or (= k m) (and (= (char->integer (string-ref prefix k)) (byte (+ i k))) (same (+ k 1))))))))
    (define (name-end i)
      (if (or (= i n) (memv (byte i) '(32 41 10))) i (name-end (+ i 1))))
    (define (next-line i)
      (cond ((= i n) n) ((= (byte i) 10) (+ i 1)) (else (next-line (+ i 1)))))
    (let loop ((i 0) (found '()))
      (if (>= i n)
          (reverse found)
          (loop (next-line i)
                (if (at? "(define " i)
                    (let* ((start (+ i 8)) (start (if (at? "(" start) (+ start 1) start)))
                      (cons (list (utf8->string bytes start (name-end start)) i) found))
                    found))))))

(define (definitions argument snapshots)
  "Return (id revision definitions) of each snapshot of SNAPSHOTS."
  (map (lambda (s) (list (snapshot-id s) (snapshot-revision s) (definitions-in (snapshot-string s)))) snapshots))

(define (echo argument snapshots)
  "Return ARGUMENT: a request's cost, without its work."
  argument)

;; The definitions of each text indexed, by its id: (revision . definitions).
(define %index (make-hash-table))

(define (index argument snapshots)
  "Index the definitions of each snapshot of SNAPSHOTS, kept here.
Return (id revision count) of each: the index stays in this world, and
`lookup` answers from it, so the asking world copies little."
  (map (lambda (s)
         (let ((defs (definitions-in (snapshot-string s))))
           (hash-table-set! %index (snapshot-id s) (cons (snapshot-revision s) defs))
           (list (snapshot-id s) (snapshot-revision s) (length defs))))
       snapshots))

(define (lookup name snapshots)
  "Return (id revision position) of each definition of NAME indexed."
  (append-map (lambda (entry)
                (let ((id (car entry)) (revision (cadr entry)))
                  (filter-map (lambda (d) (and (string=? (car d) name) (list id revision (cadr d)))) (cddr entry))))
              (hash-table->alist %index)))
