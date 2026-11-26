;;; Org Babel: source-block parsing, header-arg inheritance, :noweb
;;; expansion, :tangle, synchronous/asynchronous execution, and lossless
;;; #+RESULTS: byte-splice updates. Pure Scheme.

(require "syntax.scm")
(require "edit.scm")
(require "mem.scm")

;; ----- Records -----

(define-record-type <org-src-block>
  (make-org-src-block name lang params body start end body-start body-end heading-index)
  org-src-block?
  (name org-src-block-name)
  (lang org-src-block-lang)
  (params org-src-block-params)
  (body org-src-block-body)
  (start org-src-block-start)
  (end org-src-block-end)
  (body-start org-src-block-body-start)
  (body-end org-src-block-body-end)
  (heading-index org-src-block-heading-index))

;; ----- Header argument parsing & merging -----

(define (parse-header-arg-string raw)
  "Parse an Org Babel header argument string RAW into an alist of `(key . val)`."
  (if (or (not (string? raw)) (string=? (string-trim raw) ""))
      '()
      (let* ((s (string-trim raw))
             (n (string-length s)))
        ;; Tokenize respecting double quotes
        (define (tokenize i acc)
          (cond
            ((>= i n) (reverse acc))
            ((char-whitespace? (string-ref s i)) (tokenize (+ i 1) acc))
            ((char=? (string-ref s i) #\")
             (let loop ((j (+ i 1)))
               (cond
                 ((>= j n) (reverse (cons (substring s (+ i 1) j) acc)))
                 ((char=? (string-ref s j) #\") (tokenize (+ j 1) (cons (substring s (+ i 1) j) acc)))
                 (else (loop (+ j 1))))))
            (else
             (let loop ((j i))
               (if (or (>= j n) (char-whitespace? (string-ref s j)))
                   (tokenize j (cons (substring s i j) acc))
                   (loop (+ j 1)))))))
        (let ((toks (tokenize 0 '())))
          (let loop ((ts toks) (cur-key #f) (cur-vals '()) (acc '()))
            (cond
              ((null? ts)
               (reverse (if cur-key
                            (cons (cons cur-key (string-join (reverse cur-vals) " ")) acc)
                            acc)))
              ((and (> (string-length (car ts)) 1)
                    (char=? (string-ref (car ts) 0) #\:))
               (let ((acc2 (if cur-key
                               (cons (cons cur-key (string-join (reverse cur-vals) " ")) acc)
                               acc)))
                 (loop (cdr ts) (string-downcase (car ts)) '() acc2)))
              (cur-key
               (loop (cdr ts) cur-key (cons (car ts) cur-vals) acc))
              (else
               (loop (cdr ts) cur-key cur-vals acc))))))))

(define (merge-header-args base extra)
  ;; Merge EXTRA over BASE; `:var` accumulates, other keys override.
  (let loop ((es extra) (acc base))
    (if (null? es)
        acc
        (let* ((pair (car es))
               (k (car pair)))
          (if (string=? k ":var")
              (loop (cdr es) (append acc (list pair)))
              (loop (cdr es)
                    (append (filter (lambda (p) (not (string=? (car p) k))) acc)
                            (list pair))))))))

(define (header-arg-ref params key default)
  (let ((p (assoc key params)))
    (if p (cdr p) default)))

(define (header-arg-vars params)
  (filter-map
   (lambda (p)
     (and (string=? (car p) ":var")
          (let* ((v (string-trim (cdr p)))
                 (eq-i (string-index v #\=)))
            (and eq-i (> eq-i 0)
                 (cons (string-trim (substring v 0 eq-i))
                       (string-trim (substring v (+ eq-i 1))))))))
   params))

(define (file-header-args ol lang)
  ;; Extract #+PROPERTY: header-args and #+PROPERTY: header-args:<lang>
  (let* ((lkey (string-append "header-args:" (string-downcase lang)))
         (kws (org-outline-keywords ol)))
    (let loop ((ks kws) (gen '()) (spec '()))
      (if (null? ks)
          (merge-header-args gen spec)
          (let ((k (string-upcase (car (car ks))))
                (v (string-trim (cdr (car ks)))))
            (if (string=? k "PROPERTY")
                (let ((sp (string-index v #\space)))
                  (if (not sp)
                      (loop (cdr ks) gen spec)
                      (let ((pk (string-downcase (substring v 0 sp)))
                            (pv (string-trim (substring v (+ sp 1)))))
                        (cond
                          ((string=? pk "header-args")
                           (loop (cdr ks) (merge-header-args gen (parse-header-arg-string pv)) spec))
                          ((string=? pk lkey)
                           (loop (cdr ks) gen (merge-header-args spec (parse-header-arg-string pv))))
                          (else (loop (cdr ks) gen spec))))))
                (loop (cdr ks) gen spec)))))))

(define (heading-chain-header-args text ol h lang)
  ;; Inherit :header-args: and :header-args:<lang>: from ancestors down to H.
  (if (not h)
      '()
      (let* ((hs (org-outline-headings ol))
             (lkey (string-append "header-args:" (string-downcase lang)))
             (chain (let loop ((cur h) (acc '()))
                      (if (not cur)
                          acc
                          (let ((pi (org-heading-parent cur)))
                            (loop (and pi (vector-ref hs pi)) (cons cur acc)))))))
        (let loop ((cs chain) (acc '()))
          (if (null? cs)
              acc
              (let* ((ch (car cs))
                     (g (org-heading-property text ol ch "header-args"))
                     (s (org-heading-property text ol ch lkey))
                     (acc1 (if g (merge-header-args acc (parse-header-arg-string g)) acc))
                     (acc2 (if s (merge-header-args acc1 (parse-header-arg-string s)) acc1)))
                (loop (cdr cs) acc2)))))))

;; ----- Block extraction -----

(define (extract-blocks-in-section text ol elems h)
  (let ((hidx (and h (org-heading-index h))))
    (let loop ((es elems) (pend-name #f) (pend-hdrs '()) (acc '()))
      (if (null? es)
          (reverse acc)
          (let* ((el (car es))
                 (k (org-element-kind el)))
            (cond
              ((eq? k 'keyword)
               (let ((kw (string-upcase (car (org-element-data el))))
                     (val (string-trim (cdr (org-element-data el)))))
                 (cond
                   ((string=? kw "NAME")
                    (loop (cdr es) val pend-hdrs acc))
                   ((or (string=? kw "HEADER") (string=? kw "HEADERS"))
                    (loop (cdr es) pend-name (merge-header-args pend-hdrs (parse-header-arg-string val)) acc))
                   (else
                    (loop (cdr es) #f '() acc)))))
              ((eq? k 'src-block)
               (let* ((data (org-element-data el))
                      (lang (car data))
                      (inline-hdr (let ((raw (cadr data)))
                                    (if (string? raw) (parse-header-arg-string raw) raw)))
                      (bstart (list-ref data 3))
                      (bend (list-ref data 4))
                      (body (org-slice-bytes text bstart bend))
                      (params (merge-header-args
                               (merge-header-args
                                (merge-header-args (file-header-args ol lang)
                                                   (heading-chain-header-args text ol h lang))
                                pend-hdrs)
                               inline-hdr))
                      (blk (make-org-src-block pend-name lang params body
                                               (org-element-start el) (org-element-end el)
                                               bstart bend hidx)))
                 (loop (cdr es) #f '() (cons blk acc))))
              (else
               (loop (cdr es) #f '() acc))))))))

(define (org-babel-blocks text ol)
  "Return all `org-src-block` records in TEXT / OL in document order."
  (let* ((hs (org-outline-headings ol))
         (n (vector-length hs))
         (first-h (if (> n 0) (org-heading-start (vector-ref hs 0)) (org-utf8-length text)))
         (pre (extract-blocks-in-section text ol (org-parse-section text 0 first-h) #f)))
    (let loop ((i 0) (acc (reverse pre)))
      (if (>= i n)
          (reverse acc)
          (let* ((h (vector-ref hs i))
                 (bs (org-heading-body-start h))
                 (se (org-heading-section-end ol h))
                 (blks (if (< bs se)
                           (extract-blocks-in-section text ol (org-parse-section text bs se) h)
                           '())))
            (loop (+ i 1) (append (reverse blks) acc)))))))

(define (org-babel-block-at text ol byte-pos)
  "Return the `org-src-block` containing BYTE-POS in TEXT / OL, or #f."
  (let* ((h (org-heading-at ol byte-pos))
         (hs (org-outline-headings ol))
         (n (vector-length hs))
         (from (if h (org-heading-body-start h) 0))
         (to (if h
                 (org-heading-section-end ol h)
                 (if (> n 0) (org-heading-start (vector-ref hs 0)) (org-utf8-length text)))))
    (if (<= to from)
        #f
        (let ((blks (extract-blocks-in-section text ol (org-parse-section text from to) h)))
          (find (lambda (b)
                  (and (>= byte-pos (org-src-block-start b))
                       (< byte-pos (org-src-block-end b))))
                blks)))))

;; ----- Noweb expansion (<<name>>) -----

(define (babel-contains-from s pat i)
  (let ((hit (string-contains (substring s i) pat)))
    (and hit (+ i hit))))

(define (expand-noweb body by-name seen)
  (let ((n (string-length body)))
    (let loop ((i 0) (acc '()))
      (if (>= i n)
          (apply string-append (reverse acc))
          (let ((open (babel-contains-from body "<<" i)))
            (if (not open)
                (apply string-append (reverse (cons (substring body i n) acc)))
                (let ((close (babel-contains-from body ">>" (+ open 2))))
                  (if (not close)
                      (apply string-append (reverse (cons (substring body i n) acc)))
                      (let* ((ref (string-trim (substring body (+ open 2) close)))
                             (target (and (not (member ref seen))
                                          (hash-table-ref/default by-name ref #f))))
                        (if (not target)
                            (loop (+ close 2) (cons (substring body i (+ close 2)) acc))
                            (let ((rep (string-trim-right (expand-noweb target by-name (cons ref seen)))))
                              (loop (+ close 2)
                                    (cons rep (cons (substring body i open) acc))))))))))))))

(define (string-trim-right s)
  (let loop ((n (string-length s)))
    (cond ((<= n 0) "")
          ((char=? (string-ref s (- n 1)) #\newline) (loop (- n 1)))
          (else (substring s 0 n)))))

;; ----- Tangling (:tangle) -----

(define (lang-extension lang)
  (let ((l (string-downcase lang)))
    (cond
      ((or (string=? l "elisp") (string=? l "emacs-lisp")) "el")
      ((or (string=? l "scheme") (string=? l "techne")) "scm")
      ((or (string=? l "sh") (string=? l "bash") (string=? l "shell")) "sh")
      ((or (string=? l "python") (string=? l "python3")) "py")
      ((string=? l "rust") "rs")
      ((string=? l "nix") "nix")
      ((string=? l "c") "c")
      ((string=? l "javascript") "js")
      ((string=? l "typescript") "ts")
      ((string=? l "yaml") "yml")
      ((string=? l "json") "json")
      (else l))))

(define (path-dir-of p)
  (let loop ((i (- (string-length p) 1)))
    (cond ((< i 0) "")
          ((char=? (string-ref p i) #\/) (substring p 0 (+ i 1)))
          (else (loop (- i 1))))))

(define (path-stem-of p)
  (let ((len (string-length p)))
    (if (string-suffix? ".org" p)
        (substring p 0 (- len 4))
        p)))

(define (babel-expand-tilde p)
  (cond
    ((string=? p "~") (or (guard (e (#t #f)) (get-environment-variable "HOME")) p))
    ((string-prefix? "~/" p)
     (let ((h (guard (e (#t #f)) (get-environment-variable "HOME"))))
       (if h (string-append h "/" (substring p 2)) p)))
    (else p)))

(define (normalize-path-lexical p)
  ;; Lexically collapse `.`, `..`, and duplicate `/` so `out.sh` and `./out.sh` match.
  (let* ((abs? (string-prefix? "/" p))
         (parts (string-split p #\/)))
    (let loop ((ps parts) (stack '()))
      (if (null? ps)
          (let ((joined (string-join (reverse stack) "/")))
            (if abs? (string-append "/" joined) (if (string=? joined "") "." joined)))
          (let ((seg (car ps)))
            (cond
              ((or (string=? seg "") (string=? seg "."))
               (loop (cdr ps) stack))
              ((string=? seg "..")
               (if (and (pair? stack) (not (string=? (car stack) "..")))
                   (loop (cdr ps) (cdr stack))
                   (if abs? (loop (cdr ps) stack) (loop (cdr ps) (cons ".." stack)))))
              (else
               (loop (cdr ps) (cons seg stack)))))))))

(define (resolve-tangle-target tangle-val lang source-path)
  (let ((v (string-trim tangle-val)))
    (cond
      ((or (string=? v "") (string-ci=? v "no")) #f)
      ((string-ci=? v "yes")
       (normalize-path-lexical
        (string-append (if source-path (path-stem-of (babel-expand-tilde source-path)) "tangled")
                       "." (lang-extension lang))))
      ((or (string-prefix? "/" v) (string-prefix? "~" v))
       (normalize-path-lexical (babel-expand-tilde v)))
      ((and source-path (> (string-length (path-dir-of (babel-expand-tilde source-path))) 0))
       (normalize-path-lexical (string-append (path-dir-of (babel-expand-tilde source-path)) v)))
      (else (normalize-path-lexical v)))))

(define (org-babel-expand-body text ol blk)
  "Return the body of `org-src-block` BLK with `:noweb yes` references expanded."
  (let ((noweb? (string-ci=? (header-arg-ref (org-src-block-params blk) ":noweb" "no") "yes")))
    (if (not noweb?)
        (org-src-block-body blk)
        (let ((by-name (make-hash-table)))
          (for-each (lambda (b)
                      (when (org-src-block-name b)
                        (hash-table-set! by-name (org-src-block-name b) (org-src-block-body b))))
                    (org-babel-blocks text ol))
          (expand-noweb (org-src-block-body blk) by-name '())))))

(define (org-babel-tangle-plan text ol #:source-path [source-path #f])
  "Compute tangle output files for TEXT and OL at optional SOURCE-PATH.
Returns an alist of `(target-path . content-string)` in block order.
Refuses targets that resolve to SOURCE-PATH."
  (let* ((blks (org-babel-blocks text ol))
         (norm-src (and source-path (normalize-path-lexical (babel-expand-tilde source-path))))
         (by-name (make-hash-table))
         (order '())
         (groups (make-hash-table))
         (shebangs (make-hash-table)))
    (for-each (lambda (b)
                (when (org-src-block-name b)
                  (hash-table-set! by-name (org-src-block-name b) (org-src-block-body b))))
              blks)
    (for-each
     (lambda (b)
       (let* ((ps (org-src-block-params b))
              (tval (header-arg-ref ps ":tangle" "no"))
              (target (resolve-tangle-target tval (org-src-block-lang b) source-path)))
         (when target
           (when (and norm-src (string=? target norm-src))
             (error (string-append "Refusing to tangle over source Org file: " target)))
           (unless (hash-table-ref/default groups target #f)
             (set! order (cons target order))
             (hash-table-set! groups target '()))
           (let* ((noweb? (string-ci=? (header-arg-ref ps ":noweb" "no") "yes"))
                  (pad? (not (string-ci=? (header-arg-ref ps ":padline" "yes") "no")))
                  (sb (header-arg-ref ps ":shebang" #f))
                  (raw-body (org-src-block-body b))
                  (body (if noweb? (expand-noweb raw-body by-name '()) raw-body)))
             (when (and sb (not (hash-table-ref/default shebangs target #f)))
               (hash-table-set! shebangs target sb))
             (hash-table-set! groups target
                              (cons (cons body pad?) (hash-table-ref groups target)))))))
     blks)
    (map (lambda (target)
           (let* ((chunks (reverse (hash-table-ref groups target)))
                  (sb (hash-table-ref/default shebangs target #f))
                  (parts
                   (let loop ((cs chunks) (first? #t) (acc (if sb (list (string-append sb "\n")) '())))
                     (if (null? cs)
                         (reverse acc)
                         (let* ((body (car (car cs)))
                                (pad? (cdr (car cs)))
                                (sep (if (and (not first?) pad?) "\n" ""))
                                (chunk (if (string-suffix? "\n" body) body (string-append body "\n"))))
                           (loop (cdr cs) #f (cons chunk (cons sep acc))))))))
             (cons target (apply string-append parts))))
         (reverse order))))

(define (org-babel-tangle-file! text ol #:source-path [source-path #f])
  "Tangle all `:tangle` blocks in TEXT / OL (at SOURCE-PATH) to disk.
Creates missing parent directories, writes files atomically, sets mode
0755 for `:shebang` outputs, and refuses to overwrite SOURCE-PATH.
Returns the list of written file paths."
  (let ((plan (org-babel-tangle-plan text ol #:source-path source-path))
        (src-true (and source-path (guard (e (#t #f)) (file-truename (babel-expand-tilde source-path))))))
    ;; Validate canonical truename against source before writing anything.
    (for-each
     (lambda (pair)
       (let ((t-true (guard (e (#t #f)) (file-truename (car pair)))))
         (when (and src-true t-true (string=? src-true t-true))
           (error (string-append "Refusing to tangle over source Org file: " (car pair))))))
     plan)
    (for-each
     (lambda (pair)
       (let* ((target (car pair))
              (content (cdr pair))
              (dir (path-dir-of target)))
         (when (> (string-length dir) 0)
           (make-directory dir))
         (replace-file target content)
         (when (string-prefix? "#!" content)
           (guard (e (#t #f)) (set-file-mode! target #o755)))))
     plan)
    (map car plan)))

;; ----- Results formatting & lossless #+RESULTS: byte-splice -----

(define (parse-results-tokens params)
  (let ((raw (header-arg-ref params ":results" "replace")))
    (filter (lambda (s) (> (string-length s) 0))
            (map string-downcase (string-split (string-trim raw) #\space)))))

(define (format-babel-result output lang params)
  "Format execution OUTPUT for LANG according to `:results` in PARAMS."
  (let* ((toks (parse-results-tokens params))
         (trimmed (string-trim-right output)))
    (cond
      ((member "silent" toks) #f)
      ((member "raw" toks) (string-append trimmed "\n"))
      ((member "code" toks)
       (string-append "#+begin_src " lang "\n" trimmed "\n#+end_src\n"))
      ((member "drawer" toks)
       (string-append ":RESULTS:\n" trimmed "\n:END:\n"))
      ((string=? trimmed "") ": \n")
      ((not (string-index trimmed #\newline))
       (string-append ": " trimmed "\n"))
      (else
       (let ((lines (string-split trimmed #\newline)))
         (if (and (<= (length lines) 10) (not (member "verbatim" toks)))
             (string-append (string-join (map (lambda (l) (string-append ": " l)) lines) "\n") "\n")
             (string-append "#+begin_example\n" trimmed "\n#+end_example\n")))))))

(define (line-is-heading? trimmed)
  (let ((n (string-length trimmed)))
    (and (> n 1)
         (char=? (string-ref trimmed 0) #\*)
         (let loop ((i 1))
           (cond ((>= i n) #f)
                 ((char=? (string-ref trimmed i) #\*) (loop (+ i 1)))
                 ((or (char=? (string-ref trimmed i) #\space) (char=? (string-ref trimmed i) #\tab)) #t)
                 (else #f))))))

(define (line-is-fixed-width? trimmed)
  (or (string=? trimmed ":")
      (string-prefix? ": " trimmed)
      (string-prefix? ":\t" trimmed)))

(define (slice-lines-with-bytes text from to)
  ;; Return list of (line-str byte-start byte-end) for [from, to).
  (let* ((slice (org-slice-bytes text from to))
         (len (string-length slice)))
    (let loop ((i 0) (b from) (acc '()))
      (if (>= i len)
          (reverse acc)
          (let* ((nl (let scan ((j i))
                       (cond ((>= j len) len)
                             ((char=? (string-ref slice j) #\newline) j)
                             (else (scan (+ j 1))))))
                 (next-i (if (< nl len) (+ nl 1) len))
                 (line (substring slice i nl))
                 (b-len (bytevector-length (string->utf8 (substring slice i next-i)))))
            (loop next-i (+ b b-len) (cons (list line b (+ b b-len)) acc)))))))

(define (find-results-range text blk #:outline [ol #f])
  ;; Locate an existing #+RESULTS: keyword + ONLY identifiable result lines/blocks
  ;; within BLK's section. Never crosses a heading or consumes unrelated prose.
  (let* ((total (org-utf8-length text))
         (start-after (org-src-block-end blk))
         (sec-limit
          (if ol
              (let ((hidx (org-src-block-heading-index blk))
                    (hs (org-outline-headings ol)))
                (if (and hidx (< hidx (vector-length hs)))
                    (org-heading-section-end ol (vector-ref hs hidx))
                    (if (> (vector-length hs) 0)
                        (org-heading-start (vector-ref hs 0))
                        total)))
              total))
         (lines (if (< start-after sec-limit) (slice-lines-with-bytes text start-after sec-limit) '())))
    (let loop ((ls lines))
      (if (null? ls)
          #f
          (let* ((entry (car ls))
                 (trimmed (string-trim (car entry))))
            (cond
              ((string=? trimmed "") (loop (cdr ls)))
              ((line-is-heading? trimmed) #f)
              ((string-prefix? "#+RESULTS" (string-upcase trimmed))
               (let* ((rstart (cadr entry))
                      (hdr-end (caddr entry))
                      (rest (cdr ls)))
                 (if (null? rest)
                     (cons rstart hdr-end)
                     (let* ((first-t (string-trim (car (car rest))))
                            (first-up (string-upcase first-t)))
                       (cond
                         ((line-is-heading? first-t) (cons rstart hdr-end))
                         ;; Fixed-width lines (: ...)
                         ((line-is-fixed-width? first-t)
                          (let fl ((rs rest) (end hdr-end))
                            (if (and (pair? rs) (line-is-fixed-width? (string-trim (car (car rs)))))
                                (fl (cdr rs) (caddr (car rs)))
                                (cons rstart end))))
                         ;; Table lines (|...)
                         ((string-prefix? "|" first-t)
                          (let tl ((rs rest) (end hdr-end))
                            (if (and (pair? rs) (string-prefix? "|" (string-trim (car (car rs)))))
                                (tl (cdr rs) (caddr (car rs)))
                                (cons rstart end))))
                         ;; :RESULTS: drawer
                         ((string=? first-up ":RESULTS:")
                          (let dl ((rs (cdr rest)))
                            (cond
                              ((null? rs) (cons rstart hdr-end))
                              ((string=? (string-upcase (string-trim (car (car rs)))) ":END:")
                               (cons rstart (caddr (car rs))))
                              (else (dl (cdr rs))))))
                         ;; #+begin_... block (example, src, quote, etc.)
                         ((string-prefix? "#+BEGIN_" first-up)
                          (let* ((sp (string-index first-up #\space))
                                 (btype (substring first-up 8 (or sp (string-length first-up))))
                                 (end-marker (string-append "#+END_" btype)))
                            (let bl ((rs (cdr rest)))
                              (cond
                                ((null? rs) (cons rstart hdr-end))
                                ((string=? (string-upcase (string-trim (car (car rs)))) end-marker)
                                 (cons rstart (caddr (car rs))))
                                (else (bl (cdr rs)))))))
                         ;; Ambiguous/raw prose: do not consume unrelated prose!
                         (else (cons rstart hdr-end)))))))
              (else #f)))))))

(define (org-babel-results-edits text blk output #:outline [ol #f])
  "Return byte-splice edits inserting or updating `#+RESULTS:` for BLK with OUTPUT.
Optional OL bounds the search strictly to BLK's section."
  (let* ((params (org-src-block-params blk))
         (toks (parse-results-tokens params))
         (formatted (format-babel-result output (org-src-block-lang blk) params)))
    (if (not formatted)
        '()
        (let* ((existing (find-results-range text blk #:outline ol))
               (block-str (string-append "#+RESULTS:\n" formatted)))
          (cond
            ((and existing (member "append" toks))
             (list (list (cdr existing) (cdr existing) formatted)))
            ((and existing (member "prepend" toks))
             (let* ((slice (org-slice-bytes text (car existing) (cdr existing)))
                    (nl (string-index slice #\newline))
                    (ins (if nl (+ (car existing) nl 1) (cdr existing))))
               (list (list ins ins formatted))))
            (existing
             (list (list (car existing) (cdr existing) block-str)))
            (else
             (let* ((ins (org-src-block-end blk))
                    (needs-nl? (and (> ins 0)
                                    (not (string-suffix? "\n" (org-slice-bytes text (max 0 (- ins 1)) ins))))))
               (list (list ins ins (string-append (if needs-nl? "\n" "") "\n" block-str))))))))))

(define (org-babel-remove-result-edits text blk #:outline [ol #f])
  "Return byte-splice edits removing the `#+RESULTS:` block after BLK, or `'()`.
Optional OL bounds the search strictly to BLK's section."
  (let ((existing (find-results-range text blk #:outline ol)))
    (if existing
        (list (list (car existing) (cdr existing) ""))
        '())))

;; ----- Execution helpers (Scheme + external shell/python/etc.) -----

(define (shell-single-quote s)
  (string-append "'" (string-join (string-split s "'") "'\\''") "'"))

(define (valid-c-ident? s)
  (let ((n (string-length s)))
    (and (> n 0)
         (let ((c0 (string-ref s 0)))
           (or (char-alphabetic? c0) (char=? c0 #\_)))
         (let loop ((i 1))
           (if (>= i n)
               #t
               (let ((c (string-ref s i)))
                 (and (or (char-alphabetic? c) (char-numeric? c) (char=? c #\_))
                      (loop (+ i 1)))))))))

(define (valid-scheme-ident? s)
  (let ((n (string-length s)))
    (and (> n 0)
         (let loop ((i 0))
           (if (>= i n)
               #t
               (let ((c (string-ref s i)))
                 (and (not (char-whitespace? c))
                      (not (memv c '(#\( #\) #\[ #\] #\" #\' #\` #\, #\; #\\ #\# #\|)))
                      (loop (+ i 1)))))))))

(define (org-babel-eval-scheme body params)
  "Evaluate Scheme source BODY in a fresh lexical scope according to PARAMS.
Validates `:var` names and returns the output or formatted value string."
  (let* ((toks (parse-results-tokens params))
         (want-output? (member "output" toks))
         (vars (header-arg-vars params))
         (_ (for-each (lambda (v)
                        (unless (valid-scheme-ident? (car v))
                          (error (string-append "Invalid Scheme :var name: " (car v)))))
                      vars))
         (bindings (map (lambda (v) (list (string->symbol (car v)) (cdr v))) vars))
         (forms (let ((port (open-input-string body)))
                  (let loop ((acc '()))
                    (let ((x (read port)))
                      (if (eof-object? x) (reverse acc) (loop (cons x acc)))))))
         ;; Fresh lexical scope via (let ... ((lambda () ...))) so definitions never leak globally!
         (expr (list 'let bindings (cons (list 'lambda '() (if (null? forms) #f (cons 'begin forms))) '()))))
    (if want-output?
        (call-with-output-string
         (lambda (p)
           (parameterize ((current-output-port p))
             (eval expr))))
        (let ((val (eval expr)))
          (if (string? val)
              val
              (call-with-output-string (lambda (p) (write val p))))))))

(define (org-babel-command-for blk #:body [override-body #f])
  "Return `(program args stdin-string)` for external execution of BLK, or #f.
Optional OVERRIDE-BODY supplies a `:noweb`-expanded body. Validates `:var` names."
  (let* ((lang (string-downcase (org-src-block-lang blk)))
         (params (org-src-block-params blk))
         (vars (header-arg-vars params))
         (body (or override-body (org-src-block-body blk))))
    (cond
      ((member lang '("sh" "bash" "shell" "zsh" "fish"))
       (for-each (lambda (v)
                   (unless (valid-c-ident? (car v))
                     (error (string-append "Invalid shell :var name: " (car v)))))
                 vars)
       (let* ((prog (if (string=? lang "shell") "sh" lang))
              (var-lines (apply string-append
                                (map (lambda (v)
                                       (string-append (car v) "=" (shell-single-quote (cdr v)) "\n"))
                                     vars))))
         (list prog (list "-c" (string-append var-lines body)) #f)))
      ((member lang '("python" "python3"))
       (for-each (lambda (v)
                   (unless (valid-c-ident? (car v))
                     (error (string-append "Invalid Python :var name: " (car v)))))
                 vars)
       (let ((var-lines (apply string-append
                               (map (lambda (v)
                                      (string-append (car v) " = "
                                                     (call-with-output-string (lambda (p) (write (cdr v) p)))
                                                     "\n"))
                                    vars))))
         (list "python3" (list "-c" (string-append var-lines body)) #f)))
      ((string=? lang "jq")
       (list "jq" (list "-n" body) #f))
      (else #f))))

(define (org-babel-async? blk)
  "Return #t if `org-src-block` BLK requests asynchronous execution (`:async`)."
  (let ((p (assoc ":async" (org-src-block-params blk))))
    (and p (not (string-ci=? (string-trim (cdr p)) "no")))))

(define (org-babel-eval-disabled? blk)
  "Return #t if `org-src-block` BLK disables evaluation (`:eval no` or `never`)."
  (let ((v (string-downcase (string-trim (header-arg-ref (org-src-block-params blk) ":eval" "yes")))))
    (or (string=? v "no") (string=? v "never"))))
