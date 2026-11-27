;;; Orderless completion matching: one query contract shared by the
;;; minibuffer, the in-buffer completion popup, and project search.
;;;
;;; A pattern splits into components at unescaped spaces (`\ ` keeps a
;;; literal space inside a component). Every component must match a
;;; candidate, in any order:
;;;   - `=TEXT` matches TEXT literally (never as a regexp);
;;;   - `~TEXT` matches TEXT as a flex subsequence (characters in order);
;;;   - anything else matches literally or as a regexp (invalid regexps
;;;     fall back to literal matching so typing an incomplete regexp
;;;     never errors).
;;; Smart-case applies to every style: a component matches without case
;;; unless it contains an upper-case letter.

(provide orderless-split orderless-parse orderless-simple-pattern?
         orderless-part? orderless-part-kind orderless-part-text orderless-part-fold? orderless-part-regexp
         orderless-filter-specs orderless-matches? orderless-match-spans)

(define-record-type orderless-part
  (%orderless-part kind text fold? regexp)
  orderless-part?
  (kind orderless-part-kind)
  (text orderless-part-text)
  (fold? orderless-part-fold?)
  (regexp orderless-part-regexp))

(define (orderless-split pattern)
  "Split PATTERN into Orderless component strings at unescaped spaces.
Matches upstream `orderless-escapable-split-on-space`: an odd run of
backslashes before a space escapes the space (dropping one backslash and
keeping the space in the component); an even run of backslashes before a
space preserves all backslashes in the component and splits on the space.
Backslashes not before a space stay as typed."
  (if (not (string-contains pattern "\\"))
      (filter (lambda (p) (not (string=? p ""))) (string-split pattern " "))
      (let ((n (string-length pattern)))
        (let loop ((i 0) (cur '()) (parts '()))
          (cond
            ((>= i n)
             (reverse (if (null? cur) parts (cons (list->string (reverse cur)) parts))))
            ((char=? (string-ref pattern i) #\space)
             (loop (+ i 1) '() (if (null? cur) parts (cons (list->string (reverse cur)) parts))))
            ((char=? (string-ref pattern i) #\\)
             ;; Count the full run of consecutive backslashes.
             (let count ((j i))
               (if (and (< j n) (char=? (string-ref pattern j) #\\))
                   (count (+ j 1))
                   (let ((k (- j i)))
                     (if (and (< j n) (char=? (string-ref pattern j) #\space) (odd? k))
                         ;; Odd run before space: keep k-1 backslashes plus the literal space.
                         (let push ((m (- k 1)) (c cur))
                           (if (= m 0)
                               (loop (+ j 1) (cons #\space c) parts)
                               (push (- m 1) (cons #\\ c))))
                         ;; Even run before space, or not before space: keep all k backslashes.
                         (let push ((m k) (c cur))
                           (if (= m 0)
                               (loop j c parts)
                               (push (- m 1) (cons #\\ c)))))))))
            (else
             (loop (+ i 1) (cons (string-ref pattern i) cur) parts)))))))

(define (fold-case? s)
  (let ((n (string-length s)))
    (let loop ((i 0))
      (cond ((>= i n) #t)
            ((char-upper-case? (string-ref s i)) #f)
            (else (loop (+ i 1)))))))

(define (regexp-metachar? c)
  (and (memv c '(#\^ #\$ #\. #\* #\+ #\? #\( #\) #\[ #\] #\{ #\} #\| #\\)) #t))

(define (has-regexp-syntax? s)
  (let ((n (string-length s)))
    (let loop ((i 0))
      (cond ((>= i n) #f)
            ((regexp-metachar? (string-ref s i)) #t)
            (else (loop (+ i 1)))))))

(define (orderless-simple-pattern? pattern)
  "Return #t if PATTERN uses only plain space-separated literal substrings.
Such patterns can go straight to `matcher-filter!` without Scheme parsing."
  (let ((n (string-length pattern)))
    (let loop ((i 0))
      (if (>= i n)
          #t
          (let ((c (string-ref pattern i)))
            (if (or (char=? c #\=) (char=? c #\~) (regexp-metachar? c))
                #f
                (loop (+ i 1))))))))

(define (compile-orderless-regexp pattern fold?)
  (guard (e (#t #f))
    (string->regexp (if fold? (string-append "(?i)" pattern) pattern))))

(define (parse-orderless-part raw)
  (let ((n (string-length raw)))
    (cond
      ((and (> n 0) (char=? (string-ref raw 0) #\=))
       (let ((text (substring raw 1 n)))
         (%orderless-part 'literal text (fold-case? text) #f)))
      ((and (> n 0) (char=? (string-ref raw 0) #\~))
       (let ((text (substring raw 1 n)))
         (%orderless-part 'flex text (fold-case? text) #f)))
      (else
       (let ((fold? (fold-case? raw)))
         (if (has-regexp-syntax? raw)
             (let ((re (compile-orderless-regexp raw fold?)))
               (if re
                   (%orderless-part 'literal-or-regexp raw fold? re)
                   (%orderless-part 'literal raw fold? #f)))
             (%orderless-part 'literal raw fold? #f)))))))

(define (orderless-parse pattern)
  "Parse PATTERN into a list of `orderless-part` records."
  (map parse-orderless-part (orderless-split pattern)))

(define (orderless-filter-specs parts)
  "Return the native matcher/search specs for parsed PARTS.
Each non-empty part becomes (kind text fold?) with KIND a string:
\"literal\", \"flex\", \"literal-or-regexp\", or \"regexp\"."
  (filter-map (lambda (p)
                (let ((text (orderless-part-text p)))
                  (and (> (string-length text) 0)
                       (list (symbol->string (orderless-part-kind p))
                             text
                             (orderless-part-fold? p)))))
              parts))

;; Convert UTF-8 byte offset TARGET in STR to a character index.
(define (byte-offset->char-index str target)
  (let ((n (string-length str)))
    (if (= (bytevector-length (string->utf8 str)) n)
        (min target n)
        (let loop ((ci 0) (bi 0))
          (if (or (>= ci n) (>= bi target))
              ci
              (let* ((cp (char->integer (string-ref str ci)))
                     (w (cond ((< cp #x80) 1)
                              ((< cp #x800) 2)
                              ((< cp #x10000) 3)
                              (else 4))))
                (loop (+ ci 1) (+ bi w))))))))

;; Map each character index of FOLDED back to its original character index
;; in TEXT, only when downcasing changed the string length (e.g. `İ` -> `i\u0307`).
(define (folded-index-map text folded)
  (if (= (string-length text) (string-length folded))
      #f
      (let* ((tlen (string-length text))
             (flen (string-length folded))
             (v (make-vector flen 0)))
        (let loop ((ti 0) (fi 0))
          (when (and (< ti tlen) (< fi flen))
            (let ((step (string-length (string-downcase (string (string-ref text ti))))))
              (let fill ((k 0))
                (when (and (< k step) (< (+ fi k) flen))
                  (vector-set! v (+ fi k) ti)
                  (fill (+ k 1))))
              (loop (+ ti 1) (+ fi step)))))
        v)))

(define (map-span from to imap tlen)
  (if (not imap)
      (let ((a (max 0 (min from tlen))) (b (max 0 (min to tlen))))
        (if (< a b) (list (list a b)) '()))
      (let* ((flen (vector-length imap))
             (a (if (< from flen) (vector-ref imap from) tlen))
             (b (if (and (> to 0) (<= to flen)) (+ (vector-ref imap (- to 1)) 1) tlen)))
        (if (< a b) (list (list a b)) '()))))

;; Match NEEDLE as a subsequence of HAYSTACK, returning char spans or #f.
(define (flex-spans haystack needle imap text-len)
  (let ((hlen (string-length haystack))
        (nlen (string-length needle)))
    (let loop ((hi 0) (ni 0) (spans '()))
      (cond
        ((>= ni nlen) (reverse spans))
        ((>= hi hlen) #f)
        ((char=? (string-ref haystack hi) (string-ref needle ni))
         (let ((mapped (map-span hi (+ hi 1) imap text-len)))
           (loop (+ hi 1) (+ ni 1) (if (pair? mapped) (cons (car mapped) spans) spans))))
        (else
         (loop (+ hi 1) ni spans))))))

(define (literal-spans text folded needle fold? imap)
  (let* ((hay (if fold? folded text))
         (pat (if fold? (string-downcase needle) needle))
         (i (string-contains hay pat)))
    (and i (map-span i (+ i (string-length pat)) (and fold? imap) (string-length text)))))

(define (regexp-spans text re)
  (let ((m (string-search-regexp text re)))
    (and m
         (let ((from (byte-offset->char-index text (car m)))
               (to (byte-offset->char-index text (cadr m))))
           (if (< from to) (list (list from to)) '())))))

;; Spans of a single parsed PART in TEXT (with optional pre-folded FOLDED),
;; or #f if it does not match.
(define (part-spans text folded part imap)
  (let ((needle (orderless-part-text part))
        (kind (orderless-part-kind part))
        (fold? (orderless-part-fold? part)))
    (cond
      ((string=? needle "") '())
      ((eq? kind 'literal)
       (literal-spans text folded needle fold? imap))
      ((eq? kind 'flex)
       (let ((hay (if fold? folded text))
             (pat (if fold? (string-downcase needle) needle)))
         (flex-spans hay pat (and fold? imap) (string-length text))))
      ((eq? kind 'literal-or-regexp)
       (or (literal-spans text folded needle fold? imap)
           (regexp-spans text (orderless-part-regexp part))))
      ((eq? kind 'regexp)
       (regexp-spans text (orderless-part-regexp part)))
      (else #f))))

(define (merge-spans spans)
  (if (or (null? spans) (null? (cdr spans)))
      spans
      (let ((sorted (sort spans (lambda (a b)
                                  (if (= (car a) (car b))
                                      (< (cadr a) (cadr b))
                                      (< (car a) (car b)))))))
        (let loop ((rest (cdr sorted)) (cur (car sorted)) (acc '()))
          (if (null? rest)
              (reverse (cons cur acc))
              (let ((next (car rest)))
                (if (<= (car next) (cadr cur))
                    (loop (cdr rest) (list (car cur) (max (cadr cur) (cadr next))) acc)
                    (loop (cdr rest) next (cons cur acc)))))))))

(define (ensure-part p)
  (if (orderless-part? p) p (parse-orderless-part p)))

(define (orderless-match-spans text parts #:folded [folded #f])
  "Return merged highlight spans for PARTS matching TEXT, or #f.
PARTS is a list of `orderless-part` records (or raw component strings).
FOLDED, when given, is TEXT already folded to lower case."
  (let* ((folded (or folded (string-downcase text)))
         (imap (folded-index-map text folded)))
    (let loop ((ps parts) (acc '()))
      (if (null? ps)
          (merge-spans acc)
          (let ((s (part-spans text folded (ensure-part (car ps)) imap)))
            (and s (loop (cdr ps) (append s acc))))))))

(define (orderless-matches? text parts #:folded [folded #f])
  "Return #t if every one of PARTS matches TEXT.
FOLDED, when given, is TEXT already folded to lower case."
  (and (orderless-match-spans text parts #:folded folded) #t))
