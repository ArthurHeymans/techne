(import (techne editor))

(provide git-conflict-blocks)

;; A complete conflict is two or three alternatives bracketed by a matching
;; set of Git markers. Never edit an incomplete or nested marker block.
(define (git-marker line)
  (and (not (string=? line ""))
       (let ((c (string-ref line 0)))
         (and (memv c '(#\< #\| #\= #\>))
              (let loop ((i 1))
                (cond ((and (< i (string-length line)) (char=? c (string-ref line i))) (loop (+ i 1)))
                      ((and (>= i 7) (or (= i (string-length line)) (char-whitespace? (string-ref line i)))) (cons c i))
                      (else #f)))))))

(define (git-conflict-blocks d)
  "Return the complete Git conflict blocks of the document D, in order.
Each is (start end line top-label bottom-label top base bottom); BASE is
#f without a diff3 ancestor."
  (let* ((end (document-length d)) (starts (line-starts d 0 end)))
    (let loop ((starts starts) (line 1) (open #f) (base #f) (split #f) (out '()))
      (if (null? starts) (reverse out)
          (let* ((from (car starts)) (next (if (pair? (cdr starts)) (cadr starts) end))
                 (text (document-substring d from next)) (mark (git-marker text))
                 (rest (cdr starts)) (n (+ line 1)))
            (cond ((not mark) (loop rest n open base split out))
                  ((integer? open)
                   (let ((depth (case (car mark) ((#\<) (+ open 1)) ((#\>) (- open 1)) (else open))))
                     (loop rest n (and (> depth 0) depth) #f #f out)))
                  ((char=? (car mark) #\<)
                   (loop rest n (if open 2 (list from next line (cdr mark) (string-trim (substring text (cdr mark))))) #f #f out))
                  ((not open) (loop rest n #f #f #f out))
                  ((not (= (cdr mark) (list-ref open 3))) (loop rest n 1 #f #f out))
                  ((and (char=? (car mark) #\|) (not base) (not split)) (loop rest n open (cons from next) #f out))
                  ((and (char=? (car mark) #\=) (not split)) (loop rest n open base (cons from next) out))
                  ((and (char=? (car mark) #\>) split)
                   (loop rest n #f #f #f
                         (cons (list (car open) next (caddr open) (list-ref open 4) (string-trim (substring text (cdr mark)))
                                     (document-substring d (cadr open) (car (or base split)))
                                     (and base (document-substring d (cdr base) (car split)))
                                     (document-substring d (cdr split) from)) out)))
                  (else (loop rest n #f #f #f out))))))))
