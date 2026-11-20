;;; Unified patch parser for Forge Code Review.
;;; Maps every patch line to its exact old/new line numbers and diff side
;;; ("LEFT" or "RIGHT") so inline comments and GitHub suggestion blocks attach
;;; accurately to lines in the pull request diff.
(define-library (forge diff)
  (export forge-parse-hunk-header forge-parse-patch-lines forge-suggestion-block)
  (import (techne))
  (begin
    ;; Parse "@@ -old[,count] +new[,count] @@..." into (old-start . new-start), or #f.
    (define (forge-parse-hunk-header line)
      "Parse unified diff hunk LINE into (OLD-START . NEW-START), or #f."
      (and (string? line) (string-prefix? "@@ -" line)
           (let* ((rest (substring line 4 (string-length line)))
                  (sp (string-index rest #\space)))
             (and sp
                  (let* ((old-spec (substring rest 0 sp))
                         (old-comma (string-index old-spec #\,))
                         (old-start (string->number (if old-comma (substring old-spec 0 old-comma) old-spec)))
                         (after-sp (string-trim (substring rest (+ sp 1) (string-length rest)))))
                    (and (integer? old-start) (string-prefix? "+" after-sp)
                         (let* ((after-plus (substring after-sp 1 (string-length after-sp)))
                                (sp2 (or (string-index after-plus #\space) (string-length after-plus)))
                                (new-spec (substring after-plus 0 sp2))
                                (new-comma (string-index new-spec #\,))
                                (new-start (string->number (if new-comma (substring new-spec 0 new-comma) new-spec))))
                           (and (integer? new-start) (cons old-start new-start)))))))))

    (define (forge-parse-patch-lines patch)
      "Parse unified PATCH string into a list of line alists.
Each entry has keys: kind ('hunk 'add 'del 'ctx 'meta), text, code, old-line,
new-line, side (\"LEFT\", \"RIGHT\", or #f), and line (the anchor line for GitHub)."
      (if (not (and (string? patch) (> (string-length patch) 0)))
          '()
          (let loop ((lines (string-split patch "\n")) (old-ln 0) (new-ln 0) (in-hunk? #f) (out '()))
            (if (null? lines)
                (reverse out)
                (let ((raw (car lines)))
                  (cond
                    ((string-prefix? "@@ -" raw)
                     (let ((hdr (forge-parse-hunk-header raw)))
                       (if hdr
                           (loop (cdr lines) (car hdr) (cdr hdr) #t
                                 (cons `((kind . hunk) (text . ,raw) (code . "")
                                         (old-line . #f) (new-line . #f) (side . #f) (line . #f))
                                       out))
                           (loop (cdr lines) old-ln new-ln #f
                                 (cons `((kind . meta) (text . ,raw) (code . "")
                                         (old-line . #f) (new-line . #f) (side . #f) (line . #f))
                                       out)))))
                    ((not in-hunk?)
                     (loop (cdr lines) old-ln new-ln #f
                           (if (> (string-length raw) 0)
                               (cons `((kind . meta) (text . ,raw) (code . "")
                                       (old-line . #f) (new-line . #f) (side . #f) (line . #f))
                                     out)
                               out)))
                    ((and (> (string-length raw) 0) (char=? (string-ref raw 0) #\+))
                     (let ((code (substring raw 1 (string-length raw))))
                       (loop (cdr lines) old-ln (+ new-ln 1) #t
                             (cons `((kind . add) (text . ,raw) (code . ,code)
                                     (old-line . #f) (new-line . ,new-ln) (side . "RIGHT") (line . ,new-ln))
                                   out))))
                    ((and (> (string-length raw) 0) (char=? (string-ref raw 0) #\-))
                     (let ((code (substring raw 1 (string-length raw))))
                       (loop (cdr lines) (+ old-ln 1) new-ln #t
                             (cons `((kind . del) (text . ,raw) (code . ,code)
                                     (old-line . ,old-ln) (new-line . #f) (side . "LEFT") (line . ,old-ln))
                                   out))))
                    ((and (> (string-length raw) 0) (char=? (string-ref raw 0) #\space))
                     (let ((code (substring raw 1 (string-length raw))))
                       (loop (cdr lines) (+ old-ln 1) (+ new-ln 1) #t
                             (cons `((kind . ctx) (text . ,raw) (code . ,code)
                                     (old-line . ,old-ln) (new-line . ,new-ln) (side . "RIGHT") (line . ,new-ln))
                                   out))))
                    ((and (> (string-length raw) 0) (char=? (string-ref raw 0) #\\))
                     (loop (cdr lines) old-ln new-ln #t
                           (cons `((kind . meta) (text . ,raw) (code . "")
                                   (old-line . #f) (new-line . #f) (side . #f) (line . #f))
                                 out)))
                    (else
                     (loop (cdr lines) old-ln new-ln #f out))))))))

    (define (forge-suggestion-block code)
      "Wrap CODE in a GitHub pull-request suggestion Markdown fence."
      (string-append "```suggestion\n" code "\n```\n"))))
