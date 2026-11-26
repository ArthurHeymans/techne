;;; In-memory Org knowledge-graph index (org-mem / org-node) and dynamic
;;; #+filetags: todo discovery. Pure Scheme, no SQLite, Syncthing-safe.

(require "date.scm")
(require "syntax.scm")
(require "query.scm")

;; ----- UUID & filename helpers -----

(define %org-id-seq 0)

(define (hex-digit n)
  (string-ref "0123456789abcdef" (modulo n 16)))

(define (hex-n val digits)
  (let loop ((i 0) (v val) (cs '()))
    (if (>= i digits)
        (list->string cs)
        (loop (+ i 1) (quotient v 16) (cons (hex-digit v) cs)))))

(define (lcg-next s)
  (modulo (+ (* s 1103515245) 12345) 2147483648))

(define (urandom-16)
  (guard (e (#t #f))
    (let* ((p (open-binary-input-file "/dev/urandom"))
           (bv (read-bytevector 16 p)))
      (close-port p)
      (and (bytevector? bv) (= (bytevector-length bv) 16) bv))))

(define (bv-hex-range bv from to)
  (let loop ((i from) (acc '()))
    (if (>= i to)
        (apply string-append (reverse acc))
        (let ((b (bytevector-u8-ref bv i)))
          (loop (+ i 1)
                (cons (string (hex-digit (quotient b 16)) (hex-digit (modulo b 16))) acc))))))

(define (org-new-id)
  "Generate a new UUID v4-formatted Org `:ID:` string."
  (let ((bv (urandom-16)))
    (if bv
        (begin
          ;; RFC 4122 v4 version & variant bits
          (bytevector-u8-set! bv 6 (+ #x40 (modulo (bytevector-u8-ref bv 6) 16)))
          (bytevector-u8-set! bv 8 (+ #x80 (modulo (bytevector-u8-ref bv 8) 64)))
          (string-append (bv-hex-range bv 0 4) "-"
                         (bv-hex-range bv 4 6) "-"
                         (bv-hex-range bv 6 8) "-"
                         (bv-hex-range bv 8 10) "-"
                         (bv-hex-range bv 10 16)))
        (begin
          (set! %org-id-seq (+ %org-id-seq 1))
          (let* ((s0 (modulo (+ (* (current-milliseconds) 2654435761)
                                (* (current-jiffy) 1664525)
                                (* %org-id-seq 40503))
                             2147483648))
                 (s1 (lcg-next s0))
                 (s2 (lcg-next s1))
                 (s3 (lcg-next s2))
                 (s4 (lcg-next s3))
                 (s5 (lcg-next s4))
                 (p1 (hex-n (modulo (+ s0 s1) 4294967296) 8))
                 (p2 (hex-n (modulo s2 65536) 4))
                 (p3 (string-append "4" (hex-n (modulo s3 4096) 3)))
                 (p4 (string-append (string (string-ref "89ab" (modulo s4 4)))
                                    (hex-n (modulo (quotient s4 4) 4096) 3)))
                 (p5 (string-append (hex-n (modulo s5 16777216) 6)
                                    (hex-n (modulo (lcg-next s5) 16777216) 6))))
            (string-append p1 "-" p2 "-" p3 "-" p4 "-" p5))))))

(define (org-slugify title)
  "Convert TITLE to a lowercase filesystem-safe slug."
  (let* ((s (string-downcase title))
         (n (string-length s)))
    (let loop ((i 0) (prev-sep? #t) (acc '()))
      (if (>= i n)
          (let ((res (list->string (reverse acc))))
            ;; Trim trailing underscore
            (let trim ((len (string-length res)))
              (cond ((<= len 0) "untitled")
                    ((char=? (string-ref res (- len 1)) #\_) (trim (- len 1)))
                    (else (substring res 0 len)))))
          (let ((c (string-ref s i)))
            (cond
              ((or (and (char>=? c #\a) (char<=? c #\z))
                   (and (char>=? c #\0) (char<=? c #\9)))
               (loop (+ i 1) #f (cons c acc)))
              ((not prev-sep?)
               (loop (+ i 1) #t (cons #\_ acc)))
              (else
               (loop (+ i 1) #t acc))))))))

(define (pad2 n)
  (if (< n 10)
      (string-append "0" (number->string n))
      (number->string n)))

(define (path-join-clean dir leaf)
  (if (or (string=? dir "") (string-suffix? "/" dir))
      (string-append dir leaf)
      (string-append dir "/" leaf)))

(define (org-node-filename dir title #:now [now #f])
  "Return a timestamped node file path in DIR for TITLE (`YYYYMMDDHHMMSS-slug.org`).
NOW is an optional `(day . minutes)` pair; defaults to `(local-now)`."
  (let* ((np (or now (local-now)))
         (ymd (civil-from-days (car np)))
         (mins (cdr np))
         (hh (quotient mins 60))
         (mm (modulo mins 60))
         (ss (modulo (quotient (current-milliseconds) 1000) 60))
         (ts (string-append (number->string (car ymd))
                            (pad2 (cadr ymd))
                            (pad2 (caddr ymd))
                            (pad2 hh) (pad2 mm) (pad2 ss)))
         (slug (org-slugify title)))
    (path-join-clean dir (string-append ts "-" slug ".org"))))

(define (org-new-node-content title #:id [id #f])
  "Return initial Org file contents for a new ID-node with TITLE and optional ID."
  (let ((nid (or id (org-new-id))))
    (string-append ":PROPERTIES:\n:ID:       " nid "\n:END:\n#+title: " title "\n\n")))

(define (org-parse-roam-aliases raw)
  "Parse an Org `:ROAM_ALIASES:` property string RAW into a list of alias strings."
  (if (not (string? raw))
      '()
      (let ((n (string-length raw)))
        (let loop ((i 0) (acc '()))
          (cond
            ((>= i n) (reverse acc))
            ((char-whitespace? (string-ref raw i)) (loop (+ i 1) acc))
            ((char=? (string-ref raw i) #\")
             (let qloop ((j (+ i 1)))
               (cond
                 ((>= j n) (reverse (if (> j (+ i 1)) (cons (substring raw (+ i 1) j) acc) acc)))
                 ((char=? (string-ref raw j) #\")
                  (loop (+ j 1) (cons (substring raw (+ i 1) j) acc)))
                 (else (qloop (+ j 1))))))
            (else
             (let wloop ((j i))
               (if (or (>= j n) (char-whitespace? (string-ref raw j)))
                   (loop j (cons (substring raw i j) acc))
                   (wloop (+ j 1))))))))))

;; ----- Auto #+filetags: todo maintenance -----

(define (org-file-has-todo? text ol)
  "Return #t if outline OL of TEXT contains any open (non-DONE) TODO heading."
  (let* ((hs (org-outline-headings ol))
         (done (org-outline-done-keywords ol))
         (n (vector-length hs)))
    (let loop ((i 0))
      (cond
        ((>= i n) #f)
        ((let ((kw (org-heading-keyword (vector-ref hs i))))
           (and kw (not (member kw done))))
         #t)
        (else (loop (+ i 1)))))))

(define (string-ci-member? needle lst)
  (let ((low (string-downcase needle)))
    (let loop ((xs lst))
      (cond ((null? xs) #f)
            ((string=? (string-downcase (car xs)) low) #t)
            (else (loop (cdr xs)))))))

(define (parse-filetag-tokens raw)
  (let* ((trimmed (string-trim raw))
         (colon? (and (> (string-length trimmed) 0) (char=? (string-ref trimmed 0) #\:)))
         (parts (if colon?
                    (string-split trimmed #\:)
                    (string-split trimmed #\space))))
    (filter (lambda (s) (> (string-length s) 0)) (map string-trim parts))))

(define (org-edit-update-todo-filetag text ol)
  "Return byte-splice edits to keep `todo` in `#+filetags:` synced with open TODOs.
Inspects TEXT and OL; returns `'()` when already up to date."
  (let* ((has-todo? (org-file-has-todo? text ol))
         (ftags (org-outline-filetags ol))
         (has-tag? (string-ci-member? "todo" ftags)))
    (if (eq? has-todo? has-tag?)
        '()
        (let* ((hs (org-outline-headings ol))
               (first-h (if (> (vector-length hs) 0)
                            (org-heading-start (vector-ref hs 0))
                            (org-utf8-length text)))
               (elems (org-parse-section text 0 first-h))
               (ft-els (filter (lambda (el)
                                 (and (eq? (org-element-kind el) 'keyword)
                                      (string=? (string-upcase (car (org-element-data el))) "FILETAGS")))
                               elems)))
          (if has-todo?
              ;; Adding `todo`: update first FILETAGS directive or insert after top keywords/drawer.
              (if (pair? ft-els)
                  (let* ((el (car ft-els))
                         (cur (parse-filetag-tokens (cdr (org-element-data el))))
                         (new-line (string-append "#+filetags: :" (string-join (cons "todo" cur) ":") ":\n")))
                    (list (list (org-element-start el) (org-element-end el) new-line)))
                  (let ((ins-pos (let loop ((es elems) (pos 0))
                                   (if (null? es)
                                       pos
                                       (let ((k (org-element-kind (car es))))
                                         (if (memq k '(drawer keyword))
                                             (loop (cdr es) (org-element-end (car es)))
                                             pos))))))
                    (list (list ins-pos ins-pos "#+filetags: :todo:\n"))))
              ;; Removing `todo`: strip from EVERY FILETAGS directive so multi-line directives stay clean.
              (filter-map
               (lambda (el)
                 (let ((cur (parse-filetag-tokens (cdr (org-element-data el)))))
                   (and (string-ci-member? "todo" cur)
                        (let* ((kept (filter (lambda (t) (not (string=? (string-downcase t) "todo"))) cur))
                               (new-line (if (null? kept)
                                             ""
                                             (string-append "#+filetags: :" (string-join kept ":") ":\n"))))
                          (list (org-element-start el) (org-element-end el) new-line)))))
               ft-els))))))

;; ----- Records -----

(define-record-type <org-mem-node>
  (make-org-mem-node id title aliases path heading-index pos level tags outline-path)
  org-mem-node?
  (id org-mem-node-id)
  (title org-mem-node-title)
  (aliases org-mem-node-aliases)
  (path org-mem-node-path)
  (heading-index org-mem-node-heading-index)
  (pos org-mem-node-pos)
  (level org-mem-node-level)
  (tags org-mem-node-tags)
  (outline-path org-mem-node-outline-path))

(define-record-type <org-mem-link>
  (make-org-mem-link source-path source-id source-title target-id pos context)
  org-mem-link?
  (source-path org-mem-link-source-path)
  (source-id org-mem-link-source-id)
  (source-title org-mem-link-source-title)
  (target-id org-mem-link-target-id)
  (pos org-mem-link-pos)
  (context org-mem-link-context))

(define-record-type <org-mem-file>
  (make-org-mem-file path mtime size text outline title filetags has-todo? nodes links live?)
  org-mem-file?
  (path org-mem-file-path)
  (mtime org-mem-file-mtime)
  (size org-mem-file-size)
  (text org-mem-file-text)
  (outline org-mem-file-outline)
  (title org-mem-file-title)
  (filetags org-mem-file-filetags)
  (has-todo? org-mem-file-has-todo?)
  (nodes org-mem-file-nodes)
  (links org-mem-file-links)
  (live? org-mem-file-live?))

(define-record-type <org-mem-index>
  (%make-org-mem-index roots files nodes backlinks todo-keywords done-keywords)
  org-mem-index?
  (roots org-mem-index-roots set-org-mem-index-roots!)
  (files org-mem-index-files)
  (nodes org-mem-index-nodes)
  (backlinks org-mem-index-backlinks)
  (todo-keywords org-mem-index-todo-keywords)
  (done-keywords org-mem-index-done-keywords))

(define (make-org-mem roots #:todo-keywords [todo '("TODO")] #:done-keywords [done '("DONE")])
  "Create an empty in-memory Org index watching directory ROOTS.
Uses optional TODO-KEYWORDS and DONE-KEYWORDS defaults."
  (%make-org-mem-index roots (make-hash-table) (make-hash-table) (make-hash-table) todo done))

;; ----- Fast single-file extraction -----

(define (path-basename-no-ext path)
  (let* ((len (string-length path))
         (slash (let loop ((i (- len 1)))
                  (cond ((< i 0) -1)
                        ((char=? (string-ref path i) #\/) i)
                        (else (loop (- i 1))))))
         (base (substring path (+ slash 1) len)))
    (if (string-suffix? ".org" base)
        (substring base 0 (- (string-length base) 4))
        base)))

(define (extract-id-links-in-slice text from to path src-id src-title)
  ;; Fast pre-check: skip inline parsing completely if slice doesn't contain "id:".
  (if (<= to from)
      '()
      (let ((slice (org-slice-bytes text from to)))
        (if (not (string-contains slice "id:"))
            '()
            (let ((inlines (org-parse-inline text from to)))
              (filter-map
               (lambda (in)
                 (and (eq? (org-inline-kind in) 'link)
                      (let ((raw (car (org-inline-data in))))
                        (and (string-prefix? "id:" raw)
                             (let* ((tid (string-trim (substring raw 3)))
                                    (lpos (org-inline-start in))
                                    (ctx (string-trim
                                          (org-slice-bytes text
                                                           (max from (- lpos 30))
                                                           (min to (+ (org-inline-end in) 40))))))
                               (and (> (string-length tid) 0)
                                    (make-org-mem-link path src-id src-title tid lpos ctx)))))))
               inlines))))))

(define (heading-outline-path text ol h)
  (let loop ((cur (org-heading-parent h)) (acc '()))
    (if (not cur)
        acc
        (let ((ph (vector-ref (org-outline-headings ol) cur)))
          (loop (org-heading-parent ph) (cons (org-heading-title text ph) acc))))))

(define (org-file-prop ol key)
  (let ((ukey (string-upcase key)))
    (let loop ((ps (org-outline-file-properties ol)))
      (cond
        ((null? ps) #f)
        ((string=? (string-upcase (car (car ps))) ukey) (list-ref (car ps) 5))
        (else (loop (cdr ps)))))))

(define (extract-id-links-from-elements text elems path src-id src-title)
  ;; Only scan non-literal elements so [[id:...]] in src blocks or comments are ignored.
  (let loop ((es elems) (acc '()))
    (if (null? es)
        (reverse acc)
        (let* ((el (car es))
               (k (org-element-kind el)))
          (if (memq k '(paragraph list-item table table-row))
              (let ((ls (extract-id-links-in-slice text (org-element-start el) (org-element-end el)
                                                   path src-id src-title)))
                (loop (cdr es) (append (reverse ls) acc)))
              (loop (cdr es) acc))))))

(define (org-mem-scan-file path text ol mtime size live?)
  "Index parsed outline OL of TEXT at PATH into an `org-mem-file`.
Records MTIME, SIZE, LIVE?, all `:ID:` nodes, and all `id:` links."
  (let* ((hs (org-outline-headings ol))
         (n (vector-length hs))
         (first-h (if (> n 0) (org-heading-start (vector-ref hs 0)) (org-utf8-length text)))
         (pre-elems (org-parse-section text 0 first-h))
         (file-id (org-file-prop ol "ID"))
         (file-aliases-raw (org-file-prop ol "ROAM_ALIASES"))
         (file-aliases (if file-aliases-raw (org-parse-roam-aliases file-aliases-raw) '()))
         (title-el (find (lambda (el)
                           (and (eq? (org-element-kind el) 'keyword)
                                (string=? (string-upcase (car (org-element-data el))) "TITLE")))
                         pre-elems))
         (file-title (if title-el
                         (string-trim (cdr (org-element-data title-el)))
                         (path-basename-no-ext path)))
         (ftags (org-outline-filetags ol))
         (has-todo? (org-file-has-todo? text ol))
         (nodes (if (and file-id (> (string-length file-id) 0))
                    (list (make-org-mem-node file-id file-title file-aliases path #f 0 0 ftags '()))
                    '()))
         (links (extract-id-links-from-elements text pre-elems path file-id file-title)))
    ;; Keep traversal strictly monotonic in byte offsets (headline title -> body elements).
    (let loop ((i 0) (nacc nodes) (lacc (reverse links)))
      (if (>= i n)
          (make-org-mem-file path mtime size text ol file-title ftags has-todo?
                             (reverse nacc) (reverse lacc) live?)
          (let* ((h (vector-ref hs i))
                 (hstart (org-heading-start h))
                 (tr (org-heading-title-range h))
                 (h-title (org-heading-title text h))
                 (bstart (org-heading-body-start h))
                 (sec-end (org-heading-section-end ol h))
                 (elems (if (< bstart sec-end) (org-parse-section text bstart sec-end) '()))
                 (hid (org-section-property elems "ID"))
                 (src-id (or hid file-id))
                 (title-links (extract-id-links-in-slice text (car tr) (cdr tr) path src-id h-title))
                 (body-links (extract-id-links-from-elements text elems path src-id h-title))
                 (new-nacc
                  (if (and hid (> (string-length hid) 0))
                      (let ((h-aliases (org-parse-roam-aliases (org-section-property elems "ROAM_ALIASES")))
                            (htags (org-heading-all-tags ol h))
                            (opath (heading-outline-path text ol h)))
                        (cons (make-org-mem-node hid h-title h-aliases path i hstart
                                                 (org-heading-level h) htags opath)
                              nacc))
                      nacc)))
            (loop (+ i 1) new-nacc (append (reverse body-links) (reverse title-links) lacc)))))))

;; ----- Directory walking & incremental sync -----

(define (walk-org-files roots)
  (let ((acc '()))
    (define (visit dir)
      (let ((entries (guard (e (#t '())) (directory-list dir))))
        (for-each
         (lambda (name)
           (cond
             ((string-prefix? "." name) #f)
             ((string-suffix? "/" name)
              ;; Check file-attributes (lstat, does not follow symlinks) before descending.
              (let* ((child (path-join-clean dir (substring name 0 (- (string-length name) 1))))
                     (attrs (guard (e (#t #f)) (file-attributes child))))
                (when (and attrs (eq? (car attrs) 'directory))
                  (visit child))))
             ((string-suffix? ".org" name)
              (let* ((child (path-join-clean dir name))
                     (attrs (guard (e (#t #f)) (file-attributes child))))
                (when (and attrs (eq? (car attrs) 'file))
                  (set! acc (cons child acc)))))))
         entries)))
    (for-each (lambda (r)
                (let ((attrs (guard (e (#t #f)) (file-attributes r))))
                  (when (and attrs (eq? (car attrs) 'directory))
                    (visit r))))
              roots)
    (reverse acc)))

(define (rebuild-mem-tables! idx)
  (let ((nodes-tbl (org-mem-index-nodes idx))
        (bl-tbl (org-mem-index-backlinks idx)))
    (for-each (lambda (k) (hash-table-delete! nodes-tbl k)) (hash-table-keys nodes-tbl))
    (for-each (lambda (k) (hash-table-delete! bl-tbl k)) (hash-table-keys bl-tbl))
    (for-each
     (lambda (path)
       (let ((f (hash-table-ref (org-mem-index-files idx) path)))
         (for-each (lambda (nd)
                     (hash-table-set! nodes-tbl (org-mem-node-id nd) nd))
                   (org-mem-file-nodes f))
         (for-each (lambda (lk)
                     (let* ((tid (org-mem-link-target-id lk))
                            (prev (hash-table-ref/default bl-tbl tid '())))
                       (hash-table-set! bl-tbl tid (cons lk prev))))
                   (org-mem-file-links f))))
     (hash-table-keys (org-mem-index-files idx)))))

(define (org-mem-sync! idx #:live-buffers [live-buffers '()])
  "Incrementally sync `org-mem-index` IDX from disk and LIVE-BUFFERS.
LIVE-BUFFERS is an alist of `(path text . outline)`. Unchanged disk files
with matching `(mtime, size)` are skipped without re-reading or parsing."
  (let* ((files-tbl (org-mem-index-files idx))
         (todo (org-mem-index-todo-keywords idx))
         (done (org-mem-index-done-keywords idx))
         (now-sec (inexact->exact (floor (current-second))))
         (live-map (make-hash-table))
         (seen (make-hash-table))
         (dirty? #f))
    (for-each (lambda (entry) (hash-table-set! live-map (car entry) (cdr entry))) live-buffers)
    ;; 1. Walk disk files under roots
    (for-each
     (lambda (path)
       (hash-table-set! seen path #t)
       (let ((live (hash-table-ref/default live-map path #f)))
         (if live
             (let ((old (hash-table-ref/default files-tbl path #f)))
               (when (or (not old) (not (org-mem-file-live? old))
                         (not (eq? (org-mem-file-outline old) (cdr live))))
                 (hash-table-set! files-tbl path
                                  (org-mem-scan-file path (car live) (cdr live) #f #f #t))
                 (set! dirty? #t)))
             (let ((attrs (guard (e (#t #f)) (file-attributes path))))
               (when (and attrs (eq? (car attrs) 'file))
                 (let* ((sz (cadr attrs))
                        (mt (caddr attrs))
                        (old (hash-table-ref/default files-tbl path #f))
                        ;; Racy-git rule: files modified in the current second cannot trust 1s mtime.
                        (racy? (and (number? mt) (>= mt (- now-sec 1)))))
                   (when (or (not old)
                             (org-mem-file-live? old)
                             racy?
                             (not (equal? (org-mem-file-mtime old) mt))
                             (not (equal? (org-mem-file-size old) sz)))
                     (guard (e (#t #f))
                       (let* ((txt (file->string path))
                              (attrs2 (guard (e (#t #f)) (file-attributes path)))
                              (stable? (and attrs2 (eq? (car attrs2) 'file)
                                            (equal? (cadr attrs2) sz)
                                            (equal? (caddr attrs2) mt)
                                            (not racy?)))
                              (ol (org-parse-outline txt #:todo-keywords todo #:done-keywords done)))
                         ;; Store #f mtime when racy/changing so the next sync re-verifies.
                         (hash-table-set! files-tbl path
                                          (org-mem-scan-file path txt ol (and stable? mt) sz #f))
                         (set! dirty? #t))))))))))
     (walk-org-files (org-mem-index-roots idx)))
    ;; 2. Include any live buffers outside roots
    (for-each
     (lambda (entry)
       (let ((path (car entry))
             (txt (cadr entry))
             (ol (cddr entry)))
         (unless (hash-table-ref/default seen path #f)
           (hash-table-set! seen path #t)
           (let ((old (hash-table-ref/default files-tbl path #f)))
             (when (or (not old) (not (eq? (org-mem-file-outline old) ol)))
               (hash-table-set! files-tbl path (org-mem-scan-file path txt ol #f #f #t))
               (set! dirty? #t))))))
     live-buffers)
    ;; 3. Drop deleted files
    (for-each
     (lambda (path)
       (unless (hash-table-ref/default seen path #f)
         (hash-table-delete! files-tbl path)
         (set! dirty? #t)))
     (hash-table-keys files-tbl))
    (when dirty?
      (rebuild-mem-tables! idx))
    idx))

(define (org-mem-update-file! idx path text ol #:live [live? #f])
  "Update a single file PATH with TEXT and OL in `org-mem-index` IDX.
Sets LIVE? to indicate an unsaved buffer overlay, then rebuilds tables."
  (let* ((attrs (and (not live?) (guard (e (#t #f)) (file-attributes path))))
         (sz (and attrs (cadr attrs)))
         (mt (and attrs (caddr attrs))))
    (hash-table-set! (org-mem-index-files idx) path
                     (org-mem-scan-file path text ol mt sz live?))
    (rebuild-mem-tables! idx)
    idx))

;; ----- Query APIs -----

(define (org-mem-find-node idx id)
  "Look up an `org-mem-node` by `:ID:` string ID in IDX, or #f."
  (hash-table-ref/default (org-mem-index-nodes idx) id #f))

(define (org-mem-all-nodes idx)
  "Return a list of all indexed `org-mem-node` records in IDX."
  (let ((tbl (org-mem-index-nodes idx)))
    (map (lambda (k) (hash-table-ref tbl k)) (hash-table-keys tbl))))

(define (org-mem-backlinks idx id)
  "Return a list of `org-mem-link` records targeting ID in IDX."
  (hash-table-ref/default (org-mem-index-backlinks idx) id '()))

(define (org-mem-todo-files idx)
  "Return paths of indexed files in IDX whose `#+filetags:` contain `todo`."
  (let ((tbl (org-mem-index-files idx)))
    (filter (lambda (path)
              (string-ci-member? "todo" (org-mem-file-filetags (hash-table-ref tbl path))))
            (hash-table-keys tbl))))

(define (org-mem-file-source idx path)
  "Return a cached `org-source` for PATH from IDX, or #f if not indexed."
  (let ((f (hash-table-ref/default (org-mem-index-files idx) path #f)))
    (and f (make-org-source path (org-mem-file-text f) (org-mem-file-outline f)))))

(define (org-mem-stats idx)
  "Return `(files . nodes)` count pair for `org-mem-index` IDX."
  (cons (hash-table-size (org-mem-index-files idx))
        (hash-table-size (org-mem-index-nodes idx))))
