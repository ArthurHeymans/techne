;;; Headless EditorConfig parser, glob compiler, and ancestor resolver.
;;; Supports the standard EditorConfig glob syntax (*, **, ?, [...], [!...]
;;; {a,b}, {n1..n2}, backslash escapes), ancestor traversal stopping only at
;;; `root = true` or filesystem root, `unset`, and bounded cached reads.

(define-library (techne editorconfig)
  (export default-editorconfig-max-bytes
          editorconfig-glob-compile editorconfig-glob-match?
          editorconfig-file? editorconfig-file-path editorconfig-file-directory
          editorconfig-file-root? editorconfig-file-sections
          editorconfig-section? editorconfig-section-pattern
          editorconfig-section-properties
          parse-editorconfig-string parse-editorconfig-file
          editorconfig-discover editorconfig-resolve editorconfig-resolve-with-sources
          editorconfig-properties->options editorconfig-sourced-options
          editorconfig-invalidate! editorconfig-invalidate-ancestors!)
  (import (techne) (techne path) (techne location))
  (begin

    (define default-editorconfig-max-bytes
      (make-parameter
       262144
       (lambda (v)
         (unless (and (integer? v) (exact? v) (> v 0))
           (error "default-editorconfig-max-bytes: expected positive exact integer" v))
         v))
      "Maximum size in bytes accepted when reading an `.editorconfig` file.")

    ;;; String and path helpers

    (define (trim-hws s)
      (let* ((len (string-length s))
             (start (let loop ((i 0))
                      (if (and (< i len)
                               (let ((c (string-ref s i)))
                                 (or (char=? c #\space) (char=? c #\tab))))
                          (loop (+ i 1))
                          i)))
             (end (let loop ((i len))
                    (if (and (> i start)
                             (let ((c (string-ref s (- i 1))))
                               (or (char=? c #\space) (char=? c #\tab))))
                        (loop (- i 1))
                        i))))
        (substring s start end)))

    (define (strip-bom s)
      (if (and (> (string-length s) 0)
               (= (char->integer (string-ref s 0)) #xFEFF))
          (substring s 1 (string-length s))
          s))

    (define (split-lines text)
      (let ((len (string-length text)))
        (let loop ((i 0) (start 0) (acc '()))
          (cond
            ((>= i len)
             (reverse (cons (substring text start len) acc)))
            ((char=? (string-ref text i) #\return)
             (let ((line (substring text start i))
                   (next (if (and (< (+ i 1) len) (char=? (string-ref text (+ i 1)) #\newline))
                             (+ i 2)
                             (+ i 1))))
               (loop next next (cons line acc))))
            ((char=? (string-ref text i) #\newline)
             (loop (+ i 1) (+ i 1) (cons (substring text start i) acc)))
            (else (loop (+ i 1) start acc))))))

    (define (has-parent-component? p)
      (and (string? p)
           (any (lambda (seg) (string=? seg "..")) (string-split p "/"))))

    (define (local-absolute-path? p)
      (and (string? p)
           (string-prefix? "/" p)
           (not (remote-path? p))
           (not (has-parent-component? p))))

    (define (normalize-lexical-path p)
      (if (not (string? p))
          "/"
          (let* ((abs? (string-prefix? "/" p))
                 (parts (string-split p "/"))
                 (stack (let loop ((xs parts) (acc '()))
                          (if (null? xs)
                              (reverse acc)
                              (let ((seg (car xs)))
                                (cond ((or (string=? seg "") (string=? seg "."))
                                       (loop (cdr xs) acc))
                                      ((string=? seg "..")
                                       (loop (cdr xs) (if (pair? acc) (cdr acc) acc)))
                                      (else (loop (cdr xs) (cons seg acc)))))))))
            (if abs?
                (if (null? stack) "/" (string-append "/" (string-join stack "/")))
                (string-join stack "/")))))

    (define (ensure-trailing-slash dir)
      (cond ((or (not (string? dir)) (string=? dir "")) "/")
            ((remote-path? dir)
             (if (string-suffix? "/" dir) dir (string-append dir "/")))
            (else
             (let ((norm (normalize-lexical-path dir)))
               (cond ((string=? norm "") "/")
                     ((string-suffix? "/" norm) norm)
                     (else (string-append norm "/")))))))

    (define (parent-dir dir)
      (let* ((d (ensure-trailing-slash dir))
             (n (string-length d)))
        (if (<= n 1)
            #f
            (let ((trimmed (substring d 0 (- n 1))))
              (let loop ((i (- (string-length trimmed) 1)))
                (cond ((< i 0) "/")
                      ((char=? (string-ref trimmed i) #\/)
                       (substring trimmed 0 (+ i 1)))
                      (else (loop (- i 1)))))))))

    (define (string-index-from s ch start)
      (let ((len (string-length s)))
        (let loop ((i start))
          (cond ((>= i len) #f)
                ((char=? (string-ref s i) ch) i)
                (else (loop (+ i 1)))))))

    (define (string-rindex s ch)
      (let loop ((i (- (string-length s) 1)))
        (cond ((< i 0) #f)
              ((char=? (string-ref s i) ch) i)
              (else (loop (- i 1))))))

    (define (parse-exact-int s)
      (let ((len (string-length s)))
        (and (> len 0)
             (let* ((neg? (char=? (string-ref s 0) #\-))
                    (start (if (or neg? (char=? (string-ref s 0) #\+)) 1 0)))
               (and (< start len)
                    (let loop ((i start) (val 0))
                      (if (>= i len)
                          (if neg? (- val) val)
                          (let ((c (string-ref s i)))
                            (and (char>=? c #\0) (char<=? c #\9)
                                 (loop (+ i 1) (+ (* val 10) (- (char->integer c) 48))))))))))))

    ;;; Compiled glob matcher (EditorConfig specification)
    ;;;
    ;;; Pattern AST nodes:
    ;;;   (lit char)
    ;;;   (any-non-slash)               ; ?
    ;;;   (star)                        ; *
    ;;;   (dstar)                       ; **
    ;;;   (class negated? specs)        ; [...] where spec is char or (lo . hi)
    ;;;   (alt branches)                ; {a,b,c} where each branch is list of nodes
    ;;;   (num-range lo hi)             ; {n1..n2}

    (define (parse-char-class pat start end)
      ;; start points right after '['. Returns (node . next-idx) or #f if unclosed.
      (if (>= start end)
          #f
          (let* ((neg? (char=? (string-ref pat start) #\!))
                 (i (if neg? (+ start 1) start)))
            (let loop ((i i) (first? #t) (specs '()))
              (cond
                ((>= i end) #f)
                ((and (not first?) (char=? (string-ref pat i) #\]))
                 (cons (list 'class neg? (reverse specs)) (+ i 1)))
                (else
                 (let* ((esc? (and (char=? (string-ref pat i) #\\) (< (+ i 1) end)))
                        (ch (if esc? (string-ref pat (+ i 1)) (string-ref pat i)))
                        (after (if esc? (+ i 2) (+ i 1))))
                   (if (and (< (+ after 1) end)
                            (char=? (string-ref pat after) #\-)
                            (not (char=? (string-ref pat (+ after 1)) #\])))
                       (let* ((esc2? (and (char=? (string-ref pat (+ after 1)) #\\) (< (+ after 2) end)))
                              (hi (if esc2? (string-ref pat (+ after 2)) (string-ref pat (+ after 1))))
                              (after2 (if esc2? (+ after 3) (+ after 2))))
                         (loop after2 #f (cons (cons ch hi) specs)))
                       (loop after #f (cons ch specs))))))))))

    (define (find-matching-brace pat start end)
      ;; start points right after '{'. Returns (close-idx top-level-commas) or #f.
      (let loop ((i start) (depth 1) (commas '()))
        (cond
          ((>= i end) #f)
          ((char=? (string-ref pat i) #\\)
           (loop (+ i 2) depth commas))
          ((char=? (string-ref pat i) #\[)
           (let ((cls (parse-char-class pat (+ i 1) end)))
             (if cls
                 (loop (cdr cls) depth commas)
                 (loop (+ i 1) depth commas))))
          ((char=? (string-ref pat i) #\{)
           (loop (+ i 1) (+ depth 1) commas))
          ((char=? (string-ref pat i) #\})
           (if (= depth 1)
               (cons i (reverse commas))
               (loop (+ i 1) (- depth 1) commas)))
          ((and (= depth 1) (char=? (string-ref pat i) #\,))
           (loop (+ i 1) depth (cons i commas)))
          (else (loop (+ i 1) depth commas)))))

    (define (parse-num-range inner)
      (let ((len (string-length inner)))
        (let loop ((i 0))
          (cond
            ((>= (+ i 1) len) #f)
            ((and (char=? (string-ref inner i) #\.)
                  (char=? (string-ref inner (+ i 1)) #\.))
             (let ((a (parse-exact-int (substring inner 0 i)))
                   (b (parse-exact-int (substring inner (+ i 2) len))))
               (and a b (list 'num-range (min a b) (max a b)))))
            (else (loop (+ i 1)))))))

    (define %max-glob-pattern-length 4096)
    (define %max-glob-brace-depth 16)
    (define %max-glob-match-steps 10000)

    (define (parse-glob-nodes pat start end depth)
      (when (> depth %max-glob-brace-depth)
        (error "editorconfig-glob-compile: brace nesting exceeds maximum depth"))
      (let loop ((i start) (acc '()))
        (if (>= i end)
            (reverse acc)
            (let ((c (string-ref pat i)))
              (cond
                ((char=? c #\\)
                 (if (< (+ i 1) end)
                     (loop (+ i 2) (cons (list 'lit (string-ref pat (+ i 1))) acc))
                     (loop (+ i 1) (cons (list 'lit #\\) acc))))
                ((char=? c #\*)
                 (if (and (< (+ i 1) end) (char=? (string-ref pat (+ i 1)) #\*))
                     ;; Collapse consecutive stars past **.
                     (let skip ((j (+ i 2)))
                       (if (and (< j end) (char=? (string-ref pat j) #\*))
                           (skip (+ j 1))
                           (loop j (cons (list 'dstar) acc))))
                     (loop (+ i 1) (cons (list 'star) acc))))
                ((char=? c #\?)
                 (loop (+ i 1) (cons (list 'any-non-slash) acc)))
                ((char=? c #\[)
                 (let ((cls (parse-char-class pat (+ i 1) end)))
                   (if cls
                       (loop (cdr cls) (cons (car cls) acc))
                       (loop (+ i 1) (cons (list 'lit #\[) acc)))))
                ((char=? c #\{)
                 (let ((br (find-matching-brace pat (+ i 1) end)))
                   (cond
                     ((not br)
                      (loop (+ i 1) (cons (list 'lit #\{) acc)))
                     ((pair? (cdr br))
                      ;; Brace alternation with commas.
                      (let* ((close (car br))
                             (commas (cdr br))
                             (bounds (append (list (+ i 1))
                                             (map (lambda (k) (+ k 1)) commas)))
                             (ends (append commas (list close)))
                             (branches (map (lambda (a b) (parse-glob-nodes pat a b (+ depth 1))) bounds ends)))
                        (loop (+ close 1) (cons (list 'alt branches) acc))))
                     (else
                      ;; No top-level comma: check {n1..n2}, else literal braces.
                      (let* ((close (car br))
                             (inner (substring pat (+ i 1) close))
                             (rng (parse-num-range inner)))
                        (if rng
                            (loop (+ close 1) (cons rng acc))
                            (loop (+ i 1) (cons (list 'lit #\{) acc))))))))
                (else
                 (loop (+ i 1) (cons (list 'lit c) acc))))))))

    (define (char-in-specs? ch specs)
      (any (lambda (s)
             (if (pair? s)
                 (and (char>=? ch (car s)) (char<=? ch (cdr s)))
                 (char=? ch s)))
           specs))

    (define (match-nodes nodes str pos len memo budget)
      (let ((rem (vector-ref budget 0)))
        (if (<= rem 0)
            #f
            (begin
              (vector-set! budget 0 (- rem 1))
              (cond
                ((null? nodes) (= pos len))
                (else
                 (let* ((node (car nodes))
                        (rest (cdr nodes))
                        (kind (car node)))
                   (case kind
                     ((lit)
                      (and (< pos len)
                           (char=? (string-ref str pos) (cadr node))
                           (match-nodes rest str (+ pos 1) len memo budget)))
                     ((any-non-slash)
                      (and (< pos len)
                           (not (char=? (string-ref str pos) #\/))
                           (match-nodes rest str (+ pos 1) len memo budget)))
                     ((class)
                      (and (< pos len)
                           (let ((ch (string-ref str pos)))
                             (and (not (char=? ch #\/))
                                  (let ((in? (char-in-specs? ch (caddr node))))
                                    (if (cadr node) (not in?) in?))))
                           (match-nodes rest str (+ pos 1) len memo budget)))
                     ((star)
                      (let ((key (cons nodes pos)))
                        (if (hash-table-contains? memo key)
                            (hash-table-ref memo key)
                            (let ((res (let loop ((p pos))
                                         (cond
                                           ((<= (vector-ref budget 0) 0) #f)
                                           ((match-nodes rest str p len memo budget) #t)
                                           ((and (< p len) (not (char=? (string-ref str p) #\/)))
                                            (loop (+ p 1)))
                                           (else #f)))))
                              (hash-table-set! memo key res)
                              res))))
                     ((dstar)
                      (let ((key (cons nodes pos)))
                        (if (hash-table-contains? memo key)
                            (hash-table-ref memo key)
                            (let ((res (let loop ((p pos))
                                         (cond
                                           ((<= (vector-ref budget 0) 0) #f)
                                           ((match-nodes rest str p len memo budget) #t)
                                           ((< p len) (loop (+ p 1)))
                                           (else #f)))))
                              (hash-table-set! memo key res)
                              res))))
                     ((alt)
                      (let ((key (cons nodes pos)))
                        (if (hash-table-contains? memo key)
                            (hash-table-ref memo key)
                            (let ((res (any (lambda (branch)
                                              (and (> (vector-ref budget 0) 0)
                                                   (match-nodes (append branch rest) str pos len memo budget)))
                                            (cadr node))))
                              (hash-table-set! memo key res)
                              res))))
                     ((num-range)
                      (let ((key (cons nodes pos)))
                        (if (hash-table-contains? memo key)
                            (hash-table-ref memo key)
                            (let* ((lo (cadr node))
                                   (hi (caddr node))
                                   (start pos)
                                   (neg? (and (< pos len) (char=? (string-ref str pos) #\-)))
                                   (dstart (if neg? (+ pos 1) pos))
                                   (res (let loop ((p dstart))
                                          (if (and (> (vector-ref budget 0) 0)
                                                   (< p len)
                                                   (let ((ch (string-ref str p)))
                                                     (and (char>=? ch #\0) (char<=? ch #\9))))
                                              (let ((next (+ p 1)))
                                                (or (let ((v (parse-exact-int (substring str start next))))
                                                      (and v (>= v lo) (<= v hi)
                                                           (match-nodes rest str next len memo budget)))
                                                    (loop next)))
                                              #f))))
                              (hash-table-set! memo key res)
                              res))))
                     (else #f)))))))))

    (define (pattern-has-unescaped-slash? pat)
      (let ((len (string-length pat)))
        (let loop ((i 0))
          (cond ((>= i len) #f)
                ((char=? (string-ref pat i) #\\) (loop (+ i 2)))
                ((char=? (string-ref pat i) #\/) #t)
                (else (loop (+ i 1)))))))

    (define (editorconfig-glob-compile pattern)
      "Compile EditorConfig glob PATTERN into a path matcher procedure.
Returns `(path -> boolean)` for paths relative to the `.editorconfig`
directory (using `/` separators). Patterns without `/` match any file
path whose basename matches; patterns containing `/` match relative to
the directory root."
      (unless (string? pattern)
        (error "editorconfig-glob-compile: pattern must be a string" pattern))
      (when (> (string-length pattern) %max-glob-pattern-length)
        (error "editorconfig-glob-compile: pattern exceeds maximum length"))
      (let* ((has-slash? (pattern-has-unescaped-slash? pattern))
             (norm-pat (if (and has-slash? (string-prefix? "/" pattern))
                           (substring pattern 1 (string-length pattern))
                           pattern))
             (nodes (parse-glob-nodes norm-pat 0 (string-length norm-pat) 0))
             (any-prefix-nodes (if has-slash?
                                   nodes
                                   (append (list (list 'dstar) (list 'lit #\/)) nodes))))
        (lambda (relpath)
          (and (string? relpath)
               (let ((clean (if (string-prefix? "/" relpath)
                                (substring relpath 1 (string-length relpath))
                                relpath)))
                 (if has-slash?
                     (match-nodes nodes clean 0 (string-length clean)
                                  (make-hash-table) (vector %max-glob-match-steps))
                     (or (match-nodes nodes clean 0 (string-length clean)
                                      (make-hash-table) (vector %max-glob-match-steps))
                         (match-nodes any-prefix-nodes clean 0 (string-length clean)
                                      (make-hash-table) (vector %max-glob-match-steps)))))))))

    (define (editorconfig-glob-match? pattern relpath)
      "Return #t if RELPATH matches EditorConfig glob PATTERN."
      ((editorconfig-glob-compile pattern) relpath))

    ;;; Parsed EditorConfig records

    (define-record-type editorconfig-section
      (%make-editorconfig-section pattern matcher props)
      editorconfig-section?
      (pattern %editorconfig-section-pattern)
      (matcher %editorconfig-section-matcher)
      (props %editorconfig-section-properties))

    (define (editorconfig-section-pattern s)
      "Return the glob pattern string of EditorConfig section S."
      (string-copy (%editorconfig-section-pattern s)))
    (define (editorconfig-section-properties s)
      "Return the `(KEY . VALUE)` property alist of EditorConfig section S."
      (map (lambda (kv) (cons (string-copy (car kv)) (string-copy (cdr kv))))
           (%editorconfig-section-properties s)))

    (define-record-type editorconfig-file
      (%make-editorconfig-file path directory root? sections)
      editorconfig-file?
      (path %editorconfig-file-path)
      (directory %editorconfig-file-directory)
      (root? editorconfig-file-root?)
      (sections %editorconfig-file-sections))

    (define (editorconfig-file-path f)
      "Return the file path of parsed EditorConfig file F, or `#f`."
      (let ((p (%editorconfig-file-path f)))
        (if (string? p) (string-copy p) p)))
    (define (editorconfig-file-directory f)
      "Return the directory path of parsed EditorConfig file F."
      (string-copy (%editorconfig-file-directory f)))
    (define (editorconfig-file-sections f)
      "Return the list of sections in parsed EditorConfig file F."
      (map (lambda (x) x) (%editorconfig-file-sections f)))

    (define %standard-lower-keys
      '("root" "indent_style" "indent_size" "tab_width"
        "end_of_line" "charset" "trim_trailing_whitespace"
        "insert_final_newline" "max_line_length"))

    (define (parse-editorconfig-string text #:path [path #f] #:directory [directory #f])
      "Parse `.editorconfig` TEXT into an `editorconfig-file` record.
Optional PATH and DIRECTORY record where the file lives."
      (let* ((clean (strip-bom text))
             (lines (split-lines clean))
             (path-copy (and (string? path) (string-copy path)))
             (dir (string-copy (or directory (and path-copy (ensure-trailing-slash (path-directory path-copy))) "/")))
             (root? #f)
             (sections '())
             (cur-pat #f)
             (cur-props '()))
        (define (flush-section!)
          (when cur-pat
            (let ((compiled (guard (e (#t #f)) (editorconfig-glob-compile cur-pat))))
              (when compiled
                (set! sections
                      (cons (%make-editorconfig-section
                             cur-pat
                             compiled
                             (reverse cur-props))
                            sections))))
            (set! cur-pat #f)
            (set! cur-props '())))
        (for-each
         (lambda (raw-line)
           (let ((line (trim-hws raw-line)))
             (cond
               ((or (string=? line "")
                    (char=? (string-ref line 0) #\#)
                    (char=? (string-ref line 0) #\;))
                #f)
               ((char=? (string-ref line 0) #\[)
                (let ((close (string-rindex line #\])))
                  (when (and close (> close 0) (<= (- close 1) 4096))
                    (flush-section!)
                    (set! cur-pat (substring line 1 close))
                    (set! cur-props '()))))
               (else
                (let ((eq-pos (string-index-from line #\= 0))
                      (colon-pos (string-index-from line #\: 0)))
                  (let ((sep (cond ((and eq-pos colon-pos) (min eq-pos colon-pos))
                                   (eq-pos eq-pos)
                                   (else colon-pos))))
                    (when (and sep (> sep 0))
                      (let* ((k (string-downcase (trim-hws (substring line 0 sep))))
                             (v-raw (trim-hws (substring line (+ sep 1) (string-length line))))
                             (v (if (member k %standard-lower-keys) (string-downcase v-raw) v-raw)))
                        (when (and (> (string-length k) 0) (<= (string-length k) 50))
                          (if (not cur-pat)
                              (when (and (string=? k "root") (string=? v "true"))
                                (set! root? #t))
                              (set! cur-props (cons (cons k v) cur-props))))))))))))
         lines)
        (flush-section!)
        (%make-editorconfig-file path-copy dir root? (reverse sections))))

    ;;; Bounded file reading and caching

    ;; Cache directory -> editorconfig-file | 'none | 'error
    (define %ec-cache (make-hash-table))

    (define (read-bounded-text* path max-bytes)
      ;; Returns string on regular file, #f on missing file/dir, or 'error on
      ;; symlinked ancestor/leaf, non-regular file, oversized file, or I/O error.
      (if (not (local-absolute-path? path))
          #f
          (let ((dir (path-directory path))
                (name (path-file-name path)))
            (if (not (and (string? dir) (> (string-length dir) 0)
                          (string? name) (> (string-length name) 0)))
                #f
                (let* ((norm-dir (ensure-trailing-slash dir))
                       (trimmed (if (and (> (string-length norm-dir) 1) (string-suffix? "/" norm-dir))
                                    (substring norm-dir 0 (- (string-length norm-dir) 1))
                                    norm-dir)))
                  (guard (e (#t 'error))
                    (read-confined-file-nofollow trimmed name max-bytes)))))))

    (define (read-bounded-text path max-bytes)
      (let ((res (read-bounded-text* path max-bytes)))
        (and (string? res) res)))

    (define (parse-editorconfig-file path)
      "Read and parse `.editorconfig` at PATH, or return #f.
Refuses files larger than `(default-editorconfig-max-bytes)`, symlinks,
or non-regular files."
      (let ((text (read-bounded-text path (default-editorconfig-max-bytes))))
        (and text
             (parse-editorconfig-string text
                                        #:path path
                                        #:directory (ensure-trailing-slash (path-directory path))))))

    (define (load-dir-editorconfig-entry dir)
      (if (not (local-absolute-path? dir))
          'none
          (let ((d (ensure-trailing-slash dir)))
            (if (hash-table-contains? %ec-cache d)
                (hash-table-ref %ec-cache d)
                (let* ((p (string-append d ".editorconfig"))
                       (raw (read-bounded-text* p (default-editorconfig-max-bytes)))
                       (entry (cond ((eq? raw 'error) 'error)
                                    ((string? raw)
                                     (parse-editorconfig-string raw #:path p #:directory d))
                                    (else 'none))))
                  (hash-table-set! %ec-cache d entry)
                  entry)))))

    (define (editorconfig-invalidate! [dir #f])
      "Invalidate cached `.editorconfig` files under DIR, or all if DIR is #f."
      (if (or (not dir) (not (local-absolute-path? dir)))
          (for-each (lambda (k) (hash-table-delete! %ec-cache k))
                    (hash-table-keys %ec-cache))
          (let ((prefix (ensure-trailing-slash dir)))
            (for-each (lambda (k)
                        (when (or (string=? k prefix) (string-prefix? prefix k))
                          (hash-table-delete! %ec-cache k)))
                      (hash-table-keys %ec-cache)))))

    (define (editorconfig-invalidate-ancestors! dir)
      "Invalidate cached `.editorconfig` files for DIR and its ancestors.
Also evicts cached entries for directories under DIR."
      (if (not (local-absolute-path? dir))
          (editorconfig-invalidate! #f)
          (let ((target (ensure-trailing-slash dir)))
            (editorconfig-invalidate! target)
            (let loop ((p (parent-dir target)))
              (when p
                (hash-table-delete! %ec-cache p)
                (loop (parent-dir p)))))))

    (define (editorconfig-discover dir)
      "Return `.editorconfig` files for DIR, ordered farthest-to-nearest.
Walks upward from DIR until a file with `root = true`, a symlink/read error,
or filesystem root. Returns the empty list for remote or relative paths."
      (if (not (local-absolute-path? dir))
          '()
          (let loop ((d (ensure-trailing-slash dir)) (acc '()))
            (let ((entry (load-dir-editorconfig-entry d)))
              (cond
                ((eq? entry 'error) acc)
                ((editorconfig-file? entry)
                 (let ((next-acc (cons entry acc)))
                   (if (editorconfig-file-root? entry)
                       next-acc
                       (let ((p (parent-dir d)))
                         (if p (loop p next-acc) next-acc)))))
                (else
                 (let ((p (parent-dir d)))
                   (if p (loop p acc) acc))))))))

    (define (relative-path-under base-dir file-path)
      (let ((b (ensure-trailing-slash base-dir)))
        (cond
          ((string-prefix? b file-path)
           (substring file-path (string-length b) (string-length file-path)))
          (else (path-file-name file-path)))))

    (define (editorconfig-resolve-with-sources file-path #:files [files #f])
      "Resolve EditorConfig properties for FILE-PATH with their source files.
Returns a list of `(KEY VALUE SOURCE-PATH SOURCE-DIR)` with `unset`
properties removed. Optional FILES overrides ancestor discovery
(ordered farthest-to-nearest)."
      (cond
        ((and (not files) (not (local-absolute-path? file-path)))
         '())
        ((and (string? file-path) (has-parent-component? file-path))
         '())
        (else
         (let* ((ecs (or files (editorconfig-discover (path-directory file-path))))
                (table (make-hash-table))
                (order '()))
           (for-each
            (lambda (ec)
              (let ((rel (relative-path-under (editorconfig-file-directory ec) file-path))
                    (ec-path (or (editorconfig-file-path ec)
                                 (string-append (editorconfig-file-directory ec) ".editorconfig")))
                    (ec-dir (editorconfig-file-directory ec)))
                (for-each
                 (lambda (sec)
                   (when ((%editorconfig-section-matcher sec) rel)
                     (for-each
                      (lambda (kv)
                        (let ((k (car kv)) (v (cdr kv)))
                          (if (string=? (string-downcase v) "unset")
                              (hash-table-delete! table k)
                              (begin
                                (unless (hash-table-contains? table k)
                                  (set! order (cons k order)))
                                (hash-table-set! table k (list v ec-path ec-dir))))))
                      (editorconfig-section-properties sec))))
                 (editorconfig-file-sections ec))))
            ecs)
           (filter-map (lambda (k)
                         (and (hash-table-contains? table k)
                              (cons k (hash-table-ref table k))))
                       (reverse order))))))

    (define (editorconfig-resolve file-path #:files [files #f])
      "Resolve effective EditorConfig properties for FILE-PATH.
Returns an alist of `(KEY . VALUE)` strings with `unset` properties
removed. Optional FILES overrides ancestor discovery (ordered
farthest-to-nearest)."
      (map (lambda (e) (cons (car e) (cadr e)))
           (editorconfig-resolve-with-sources file-path #:files files)))

    (define (editorconfig-sourced-options sourced-props)
      "Convert SOURCED-PROPS into sourced Techne option entries.
SOURCED-PROPS is a list of `(KEY VALUE SOURCE-PATH SOURCE-DIR)` as
returned by `editorconfig-resolve-with-sources`. Returns a list of
`(OPTION VALUE SOURCE-PATH SOURCE-DIR)` for standard safe formatting
options."
      (let ((out '())
            (lookup (lambda (k) (assoc k sourced-props))))
        (define (add! opt val src-entry)
          (set! out (cons (list opt val (caddr src-entry) (cadddr src-entry)) out)))
        (let* ((indent-style-e (lookup "indent_style"))
               (indent-size-e (lookup "indent_size"))
               (tab-width-e (lookup "tab_width"))
               (max-line-e (lookup "max_line_length"))
               (trim-e (lookup "trim_trailing_whitespace"))
               (final-nl-e (lookup "insert_final_newline"))
               (eol-e (lookup "end_of_line"))
               (indent-style (and indent-style-e (cadr indent-style-e)))
               (indent-size-raw (and indent-size-e (cadr indent-size-e)))
               (tab-width-raw (and tab-width-e (cadr tab-width-e)))
               (tab-w (let ((n (and tab-width-raw (parse-exact-int tab-width-raw))))
                        (and n (>= n 1) (<= n 32) n)))
               (indent-n (let ((n (and indent-size-raw (not (string=? indent-size-raw "tab"))
                                       (parse-exact-int indent-size-raw))))
                           (and n (>= n 1) (<= n 32) n))))
          (when (and indent-style (member indent-style '("tab" "space")))
            (add! 'indent-tabs (string=? indent-style "tab") indent-style-e))
          (cond
            (indent-n (add! 'indent-width indent-n indent-size-e))
            ((and indent-size-raw (string=? indent-size-raw "tab") tab-w)
             (add! 'indent-width tab-w tab-width-e))
            ((and indent-style (string=? indent-style "tab") tab-w)
             (add! 'indent-width tab-w tab-width-e)))
          (cond
            (tab-w (add! 'tab-width tab-w tab-width-e))
            (indent-n (add! 'tab-width indent-n indent-size-e)))
          (when max-line-e
            (let ((v (cadr max-line-e)))
              (cond
                ((string=? v "off") (add! 'fill-column #f max-line-e))
                ((let ((n (parse-exact-int v))) (and n (>= n 1) n))
                 => (lambda (n) (add! 'fill-column n max-line-e))))))
          (when (and trim-e (member (cadr trim-e) '("true" "false")))
            (add! 'trim-trailing-whitespace (string=? (cadr trim-e) "true") trim-e))
          (when (and final-nl-e (member (cadr final-nl-e) '("true" "false")))
            (add! 'insert-final-newline (string=? (cadr final-nl-e) "true") final-nl-e))
          (when (and eol-e (member (cadr eol-e) '("lf" "crlf" "cr")))
            (add! 'end-of-line (string->symbol (cadr eol-e)) eol-e)))
        (reverse out)))

    (define (editorconfig-properties->options props)
      "Convert resolved EditorConfig PROPS alist into Techne option pairs.
Maps standard safe formatting properties (`indent_style`, `indent_size`,
`tab_width`, `max_line_length`, `trim_trailing_whitespace`,
`insert_final_newline`, `end_of_line`) into `(OPTION . VALUE)` pairs."
      (map (lambda (e) (cons (car e) (cadr e)))
           (editorconfig-sourced-options
            (map (lambda (kv) (list (car kv) (cdr kv) ".editorconfig" "/")) props))))))

