;;; SRFI 130: cursor-based string library.
;;;
;;; The cursor procedures are Techne's own (crates/techne-vm/src/cursors.rs);
;;; the rest is here, walking strings with cursors so that every procedure
;;; is linear whatever the characters. Where the root module has procedures
;;; of the same names (string-index, string-trim, string-split...), these
;;; are SRFI 130's: importing (srfi 130) replaces them. A character
;;; predicate may also be a character, which matches itself.

(define-library (srfi 130)
  (export string-cursor? string-cursor-start string-cursor-end
          string-cursor-next string-cursor-prev string-cursor-forward string-cursor-back
          string-cursor=? string-cursor<? string-cursor>? string-cursor<=? string-cursor>=?
          string-cursor-diff string-cursor->index string-index->cursor
          string-null? string-every string-any
          string-tabulate string-unfold string-unfold-right
          string->list/cursors string->vector/cursors reverse-list->string string-join
          string-ref/cursor substring/cursors string-copy/cursors
          string-take string-take-right string-drop string-drop-right
          string-pad string-pad-right string-trim string-trim-right string-trim-both
          string-prefix-length string-suffix-length string-prefix? string-suffix?
          string-index string-index-right string-skip string-skip-right
          string-contains string-contains-right
          string-reverse string-concatenate string-concatenate-reverse
          string-fold string-fold-right string-for-each-cursor
          string-replicate string-count string-replace string-split
          string-filter string-remove)
  (import (except (techne)
                  string-join string-trim string-trim-right string-prefix? string-suffix?
                  string-index string-contains string-replace string-split))
  (begin

    ;; Cursors of the optional START and END, cursors or indexes.
    (define (%start s start) (if start (string-index->cursor s start) (string-cursor-start s)))
    (define (%end s end) (if end (string-index->cursor s end) (string-cursor-end s)))

    (define (%pred p)
      (if (char? p) (lambda (c) (char=? c p)) p))

    (define (string-every pred s [start #f] [end #f])
      "Return #f if a character of S from START to END fails PRED.
Else return what PRED returned for the last character, or #t if there
are none."
      (let ((pred (%pred pred)) (end (%end s end)))
        (let loop ((i (%start s start)) (last #t))
          (if (string-cursor<? i end)
              (let ((r (pred (string-ref/cursor s i))))
                (and r (loop (string-cursor-next s i) r)))
              last))))

    (define (string-any pred s [start #f] [end #f])
      "Return the first true value PRED gives for a character of S.
Only the characters from START to END are tried; #f if none passes."
      (let ((pred (%pred pred)) (end (%end s end)))
        (let loop ((i (%start s start)))
          (and (string-cursor<? i end)
               (or (pred (string-ref/cursor s i)) (loop (string-cursor-next s i)))))))

    (define (string-tabulate proc len)
      "Return a string of LEN characters, character I being (PROC I)."
      (let loop ((i (- len 1)) (acc '()))
        (if (< i 0) (list->string acc) (loop (- i 1) (cons (proc i) acc)))))

    ;; The pieces MAPPER gives (characters or strings) from SEED until STOP?,
    ;; first first, and the final seed.
    (define (%unfold stop? mapper successor seed k)
      (let loop ((seed seed) (acc '()))
        (if (stop? seed)
            (k (reverse acc) seed)
            (loop (successor seed) (cons (let ((x (mapper seed))) (if (char? x) (string x) x)) acc)))))

    (define (string-unfold stop? mapper successor seed [base ""] [make-final (lambda (x) "")])
      "Return BASE, then (MAPPER seed) for each seed, then (MAKE-FINAL seed).
The seeds are SEED, (SUCCESSOR SEED) and so on, up to the first for
which STOP? is true, which goes to MAKE-FINAL."
      (%unfold stop? mapper successor seed
               (lambda (pieces seed) (string-concatenate (cons base (append pieces (list (make-final seed))))))))

    (define (string-unfold-right stop? mapper successor seed [base ""] [make-final (lambda (x) "")])
      "Return what `string-unfold` does, with the pieces right to left.
BASE ends the string and (MAKE-FINAL seed) starts it."
      (%unfold stop? mapper successor seed
               (lambda (pieces seed) (string-concatenate (cons (make-final seed) (reverse (cons base pieces)))))))

    (define (string->list/cursors s [start #f] [end #f])
      "Return a list of the characters of S from START to END."
      (let ((start (%start s start)))
        (let loop ((i (%end s end)) (acc '()))
          (if (string-cursor>? i start)
              (let ((i (string-cursor-prev s i))) (loop i (cons (string-ref/cursor s i) acc)))
              acc))))

    (define (string->vector/cursors s [start #f] [end #f])
      "Return a vector of the characters of S from START to END."
      (list->vector (string->list/cursors s start end)))

    (define (reverse-list->string chars)
      "Return a string of CHARS in reverse order."
      (list->string (reverse chars)))

    (define (string-join strings [delimiter " "] [grammar 'infix])
      "Return STRINGS joined with DELIMITER.
GRAMMAR says where DELIMITER goes: infix (between, the default),
strict-infix (between, and STRINGS must not be empty), suffix (after
each) or prefix (before each)."
      (case grammar
        ((infix strict-infix)
         (cond ((pair? strings)
                (string-concatenate (cons (car strings) (append-map (lambda (s) (list delimiter s)) (cdr strings)))))
               ((eq? grammar 'strict-infix) (error "string-join: no strings to join with strict-infix"))
               (else "")))
        ((suffix) (string-concatenate (append-map (lambda (s) (list s delimiter)) strings)))
        ((prefix) (string-concatenate (append-map (lambda (s) (list delimiter s)) strings)))
        (else (error "string-join: unknown grammar" grammar))))

    (define (string-take s n)
      "Return the first N characters of S."
      (substring/cursors s (string-cursor-start s) (string-cursor-forward s (string-cursor-start s) n)))
    (define (string-drop s n)
      "Return S without its first N characters."
      (substring/cursors s (string-cursor-forward s (string-cursor-start s) n) (string-cursor-end s)))
    (define (string-take-right s n)
      "Return the last N characters of S."
      (substring/cursors s (string-cursor-back s (string-cursor-end s) n) (string-cursor-end s)))
    (define (string-drop-right s n)
      "Return S without its last N characters."
      (substring/cursors s (string-cursor-start s) (string-cursor-back s (string-cursor-end s) n)))

    (define (string-pad s len [char #\space] [start #f] [end #f])
      "Return the characters of S from START to END, LEN long.
They are padded with CHAR on the left, or lose characters on the left."
      (let* ((start (%start s start)) (end (%end s end)) (n (string-cursor-diff s start end)))
        (if (>= n len)
            (substring/cursors s (string-cursor-back s end len) end)
            (string-append (make-string (- len n) char) (substring/cursors s start end)))))

    (define (string-pad-right s len [char #\space] [start #f] [end #f])
      "Return the characters of S from START to END, LEN long.
They are padded with CHAR on the right, or lose characters on the right."
      (let* ((start (%start s start)) (end (%end s end)) (n (string-cursor-diff s start end)))
        (if (>= n len)
            (substring/cursors s start (string-cursor-forward s start len))
            (string-append (substring/cursors s start end) (make-string (- len n) char)))))

    (define (string-trim s [pred char-whitespace?] [start #f] [end #f])
      "Return S from START to END without the characters on its left passing PRED."
      (substring/cursors s (string-skip s pred start end) (%end s end)))
    (define (string-trim-right s [pred char-whitespace?] [start #f] [end #f])
      "Return S from START to END without the characters on its right passing PRED."
      (substring/cursors s (%start s start) (string-skip-right s pred start end)))
    (define (string-trim-both s [pred char-whitespace?] [start #f] [end #f])
      "Return S from START to END without the characters on both sides passing PRED."
      (let ((left (string-skip s pred start end)))
        (substring/cursors s left (string-skip-right s pred left end))))

    (define (string-prefix-length s1 s2 [start1 #f] [end1 #f] [start2 #f] [end2 #f])
      "Return how many characters S1 and S2 have in common at their start.
Only S1 from START1 to END1 and S2 from START2 to END2 count."
      (let ((end1 (%end s1 end1)) (end2 (%end s2 end2)))
        (let loop ((i (%start s1 start1)) (j (%start s2 start2)) (n 0))
          (if (and (string-cursor<? i end1) (string-cursor<? j end2)
                   (char=? (string-ref/cursor s1 i) (string-ref/cursor s2 j)))
              (loop (string-cursor-next s1 i) (string-cursor-next s2 j) (+ n 1))
              n))))

    (define (string-suffix-length s1 s2 [start1 #f] [end1 #f] [start2 #f] [end2 #f])
      "Return how many characters S1 and S2 have in common at their end.
Only S1 from START1 to END1 and S2 from START2 to END2 count."
      (let ((start1 (%start s1 start1)) (start2 (%start s2 start2)))
        (let loop ((i (%end s1 end1)) (j (%end s2 end2)) (n 0))
          (if (and (string-cursor>? i start1) (string-cursor>? j start2)
                   (char=? (string-ref/cursor s1 (string-cursor-prev s1 i)) (string-ref/cursor s2 (string-cursor-prev s2 j))))
              (loop (string-cursor-prev s1 i) (string-cursor-prev s2 j) (+ n 1))
              n))))

    (define (string-prefix? s1 s2 [start1 #f] [end1 #f] [start2 #f] [end2 #f])
      "Return #t if S1 from START1 to END1 starts S2 from START2 to END2."
      (= (string-prefix-length s1 s2 start1 end1 start2 end2)
         (string-cursor-diff s1 (%start s1 start1) (%end s1 end1))))

    (define (string-suffix? s1 s2 [start1 #f] [end1 #f] [start2 #f] [end2 #f])
      "Return #t if S1 from START1 to END1 ends S2 from START2 to END2."
      (= (string-suffix-length s1 s2 start1 end1 start2 end2)
         (string-cursor-diff s1 (%start s1 start1) (%end s1 end1))))

    ;; The cursor of the first character from START to END for which (PRED
    ;; c) is WANT, else END.
    (define (%find s pred want start end)
      (let ((pred (%pred pred)) (end (%end s end)))
        (let loop ((i (%start s start)))
          (if (and (string-cursor<? i end) (not (eq? want (not (pred (string-ref/cursor s i))))))
              (loop (string-cursor-next s i))
              i))))

    ;; The cursor after the last character from START to END for which
    ;; (PRED c) is WANT, else START.
    (define (%find-right s pred want start end)
      (let ((pred (%pred pred)) (start (%start s start)))
        (let loop ((i (%end s end)))
          (if (and (string-cursor>? i start) (not (eq? want (not (pred (string-ref/cursor s (string-cursor-prev s i)))))))
              (loop (string-cursor-prev s i))
              i))))

    (define (string-index s pred [start #f] [end #f])
      "Return the cursor of the first character of S passing PRED.
The search goes from START to END, and gives END if none passes."
      (%find s pred #f start end))
    (define (string-index-right s pred [start #f] [end #f])
      "Return the cursor after the last character of S passing PRED.
The search goes from END back to START, and gives START if none passes."
      (%find-right s pred #f start end))
    (define (string-skip s pred [start #f] [end #f])
      "Return the cursor of the first character of S failing PRED.
The search goes from START to END, and gives END if none fails."
      (%find s pred #t start end))
    (define (string-skip-right s pred [start #f] [end #f])
      "Return the cursor after the last character of S failing PRED.
The search goes from END back to START, and gives START if none fails."
      (%find-right s pred #t start end))

    (define (string-contains s1 s2 [start1 #f] [end1 #f] [start2 #f] [end2 #f])
      "Return the cursor of the first S2 in S1, or #f.
Only S1 from START1 to END1 and S2 from START2 to END2 count."
      (%string-contains/cursor s1 s2 (%start s1 start1) (%end s1 end1) (%start s2 start2) (%end s2 end2)))
    (define (string-contains-right s1 s2 [start1 #f] [end1 #f] [start2 #f] [end2 #f])
      "Return the cursor of the last S2 in S1, or #f.
Only S1 from START1 to END1 and S2 from START2 to END2 count."
      (%string-contains-right/cursor s1 s2 (%start s1 start1) (%end s1 end1) (%start s2 start2) (%end s2 end2)))

    (define (string-reverse s [start #f] [end #f])
      "Return the characters of S from START to END in reverse order."
      (reverse-list->string (string->list/cursors s start end)))

    (define (string-concatenate strings)
      "Return a new string of the STRINGS one after another."
      (apply string-append strings))

    (define (string-concatenate-reverse strings [final ""] [end #f])
      "Return the STRINGS in reverse order, one after another, then FINAL.
Only FINAL's characters before END count."
      (string-concatenate (reverse (cons (substring/cursors final (string-cursor-start final) (%end final end)) strings))))

    (define (string-fold kons knil s [start #f] [end #f])
      "Fold KONS over the characters of S from START to END, left to right.
KONS takes a character and the result so far, starting from KNIL."
      (let ((end (%end s end)))
        (let loop ((i (%start s start)) (acc knil))
          (if (string-cursor<? i end)
              (loop (string-cursor-next s i) (kons (string-ref/cursor s i) acc))
              acc))))

    (define (string-fold-right kons knil s [start #f] [end #f])
      "Fold KONS over the characters of S from END back to START.
KONS takes a character and the result so far, starting from KNIL."
      (let ((start (%start s start)))
        (let loop ((i (%end s end)) (acc knil))
          (if (string-cursor>? i start)
              (let ((i (string-cursor-prev s i))) (loop i (kons (string-ref/cursor s i) acc)))
              acc))))

    (define (string-for-each-cursor proc s [start #f] [end #f])
      "Call PROC with the cursor of each character of S from START to END."
      (let ((end (%end s end)))
        (let loop ((i (%start s start)))
          (when (string-cursor<? i end)
            (proc i)
            (loop (string-cursor-next s i))))))

    (define (string-replicate s from to [start #f] [end #f])
      "Return characters FROM to TO of S from START to END repeated without end.
Index 0 is START's character, and the repetition goes both ways."
      (let* ((v (string->vector/cursors s start end)) (n (vector-length v)))
        (cond ((> from to) (error "string-replicate: from is after to" from to))
              ((= from to) "")
              ((= n 0) (error "string-replicate: nothing to replicate"))
              (else (string-tabulate (lambda (i) (vector-ref v (modulo (+ from i) n))) (- to from))))))

    (define (string-count s pred [start #f] [end #f])
      "Return how many characters of S from START to END pass PRED."
      (let ((pred (%pred pred)))
        (string-fold (lambda (c n) (if (pred c) (+ n 1) n)) 0 s start end)))

    (define (string-replace s1 s2 start1 end1 [start2 #f] [end2 #f])
      "Return S1 with its characters from START1 to END1 replaced.
They are replaced by S2's from START2 to END2."
      (string-append (substring/cursors s1 (string-cursor-start s1) start1)
                     (substring/cursors s2 (%start s2 start2) (%end s2 end2))
                     (substring/cursors s1 end1 (string-cursor-end s1))))

    (define (string-split s delimiter [grammar 'infix] [limit #f] [start #f] [end #f])
      "Return the parts of S from START to END between DELIMITERs.
GRAMMAR is as `string-join` takes it: prefix drops an empty first part,
suffix an empty last one. With LIMIT, at most LIMIT splits are made. An
empty DELIMITER splits into characters."
      (let ((start (%start s start)) (end (%end s end)))
        (define (split)
          (let loop ((i start) (limit limit) (acc '()))
            (let ((found (cond ((eqv? limit 0) #f)
                               ((string-null? delimiter)
                                (and (string-cursor<? (string-cursor-next s i) end) (string-cursor-next s i)))
                               (else (string-contains s delimiter i end)))))
              (if found
                  (loop (string-cursor-forward s found (string-length delimiter))
                        (and limit (- limit 1))
                        (cons (substring/cursors s i found) acc))
                  (reverse (cons (substring/cursors s i end) acc))))))
        (cond ((string-cursor<? start end)
               (let ((parts (split)))
                 (cond ((and (eq? grammar 'prefix) (equal? (car parts) "")) (cdr parts))
                       ((and (eq? grammar 'suffix) (equal? (last parts) "")) (reverse (cdr (reverse parts))))
                       (else parts))))
              ((eq? grammar 'strict-infix) (error "string-split: nothing to split with strict-infix"))
              (else '()))))

    (define (string-filter pred s [start #f] [end #f])
      "Return the characters of S from START to END that pass PRED."
      (let ((pred (%pred pred)))
        (reverse-list->string (string-fold (lambda (c acc) (if (pred c) (cons c acc) acc)) '() s start end))))

    (define (string-remove pred s [start #f] [end #f])
      "Return the characters of S from START to END that fail PRED."
      (let ((pred (%pred pred)))
        (reverse-list->string (string-fold (lambda (c acc) (if (pred c) acc (cons c acc))) '() s start end))))))
