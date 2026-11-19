;;; The grid is authoritative. A row owns its projection cache, which dies
;;; with the row and is invalidated on mutation (including when in history).
;;; Cells are #f (default blank), (text . rendition), or 'tail of a wide cell.

(define-record-type row
  (make-row columns data cache rendition barrier prompt continuation) row?
  (columns terminal-row-columns)
  (data row-data row-data-set!)
  (cache terminal-row-cache terminal-row-cache-set!)
  (rendition rendition rendition-set!)
  (barrier barrier? barrier-set!)
  (prompt terminal-row-prompt? prompt-set!)
  (continuation continuation continuation-set!))

(define (terminal-row-double-width? r)
  "Return whether terminal row R has double-width columns."
  (not (eq? (rendition r) 'single)))

(define-record-type screen
  (%screen lines x y top bottom left right wrap saved) screen?
  (lines lines lines-set!) (x sx sx-set!) (y sy sy-set!)
  (top top top-set!) (bottom bottom bottom-set!)
  (left left left-set!) (right right right-set!)
  (wrap wrap? wrap-set!) (saved saved saved-set!))

(define-record-type terminal
  (%terminal rows cols main alt alternate history first next modes style protected
             g0 g1 shifted tabs title state header reply resize-request event level page-length cursor-style ansi-modes dec-modes link apc-size reflow) terminal?
  (rows terminal-rows rows-set!) (cols terminal-columns columns-set!)
  (main main main-set!) (alt alt alt-set!)
  (alternate terminal-alternate? alternate-set!)
  (history history) (first first first-set!) (next next next-set!)
  (modes modes modes-set!) (style style style-set!)
  (protected ever-protected? ever-protected-set!)
  (g0 g0 g0-set!) (g1 g1 g1-set!) (shifted shifted? shifted-set!)
  (tabs tabs tabs-set!) (title terminal-title title-set!)
  (state parser-state parser-state-set!) (header header header-set!)
  (reply reply) (resize-request resize-request) (event terminal-event)
  (level level level-set!) (page-length page-length page-length-set!)
  (cursor-style cursor-style cursor-style-set!)
  (ansi-modes ansi-modes) (dec-modes dec-modes)
  (link terminal-link link-set!) (apc-size apc-size apc-size-set!) (reflow reflow-history?))

;; An ASCII prefix with uniform attributes and a blank suffix. Complex
;; editing materializes the cells; rendering can read this form directly.
(define (fresh-row cols fill) (make-row cols (if fill (list "" '() fill) "") #f 'single #f #f #f))

(define (terminal-row-plain r)
  "Return R's unstyled ASCII prefix, or #f for other row data."
  (let ((data (row-data r))) (and (string? data) data)))

(define (terminal-row-ascii r)
  "Return compact data (text style blank-fill) for row R, or #f."
  (let ((data (row-data r)))
    (cond ((string? data) (list data '() #f)) ((pair? data) data) (else #f))))

(define ascii-characters
  (list->vector (map (lambda (i) (string (integer->char i))) (iota 95 32))))

(define (terminal-row-cells r)
  "Return the cell vector for row R, materializing it if needed."
  (let ((data (row-data r)))
    (if (vector? data) data
        (let* ((data (terminal-row-ascii r)) (text (car data)) (attrs (cadr data)) (n (string-length text))
               (cells (make-vector (terminal-row-columns r) (caddr data))))
          (do ((i 0 (+ i 1))) ((= i n))
            (vector-set! cells i (cons (vector-ref ascii-characters (- (char->integer (string-ref text i)) 32)) attrs)))
          (row-data-set! r cells)
          cells))))
(define (fresh-screen rows cols)
  (let ((v (make-vector rows)))
    (do ((i 0 (+ i 1))) ((= i rows)) (vector-set! v i (fresh-row cols #f)))
    (%screen v 0 0 0 (- rows 1) 0 (- cols 1) #f #f)))

(define (default-tabs cols)
  (let ((v (make-vector cols #f)))
    (do ((i 8 (+ i 8))) ((>= i cols) v) (vector-set! v i #t))))

(define (make-terminal rows cols #:history [limit 2000] #:reply [send (lambda (s) #f)]
                       #:resize [resize (lambda (rows cols) #f)] #:event [event (lambda (kind value) #f)] #:reflow [reflow #f])
  "Make a terminal with ROWS and COLS, and a scrollback LIMIT.
Call SEND with strings to reply to the process, and RESIZE with the new
rows and columns after a resize requested by the terminal stream.
Call EVENT with a kind and value for shell integration messages.
With REFLOW, rewrap saved lines on width changes."
  (let ((rows (max 1 rows)) (cols (max 1 cols)))
    (%terminal rows cols (fresh-screen rows cols) (fresh-screen rows cols) #f
               (make-vector (max 0 limit) #f) 0 0 '(wrap cursor) '() #f
               #f #f #f (default-tabs cols) "" 'ground "" send resize event 65 #f 2
               (make-hash-table) (make-hash-table) #f 0 reflow)))

(define (active t) (if (terminal-alternate? t) (alt t) (main t)))
(define (terminal-x t)
  "Return the zero-based cursor column of terminal T."
  (sx (active t)))
(define (terminal-y t)
  "Return the zero-based cursor row of terminal T."
  (sy (active t)))
(define (terminal-row t y)
  "Return row Y of the active screen of terminal T, counting from zero."
  (vector-ref (lines (active t)) y))
(define (terminal-mode? t mode)
  "Return whether terminal T has enabled MODE."
  (and (memq mode (modes t)) #t))
(define (mode-set! t mode on)
  (when (and on (memq mode '(dec-protect iso-protect))) (ever-protected-set! t #t))
  (unless (eq? (terminal-mode? t mode) on)
    (modes-set! t (if on (cons mode (modes t)) (remove (lambda (m) (eq? m mode)) (modes t))))))

(define (clear-history! t)
  (vector-fill! (history t) #f)
  (first-set! t (next t)))
(define (keep-row! t row)
  (let ((n (vector-length (history t))))
    (when (> n 0)
      (vector-set! (history t) (modulo (next t) n) row)
      (next-set! t (+ (next t) 1))
      (first-set! t (max (first t) (- (next t) n))))))
(define (terminal-history-start t)
  "Return the oldest visible scrollback row ID of terminal T."
  (if (terminal-alternate? t) (next t) (first t)))
(define (terminal-history-end t)
  "Return the exclusive end of the scrollback row IDs of terminal T."
  (next t))
(define (terminal-history-row t id)
  "Return scrollback row ID of terminal T, or #f if unavailable."
  (and (<= (terminal-history-start t) id) (< id (next t))
       (vector-ref (history t) (modulo id (vector-length (history t))))))

(define (terminal-history-slice t from to)
  "Return a vector of terminal T's scrollback rows from FROM up to TO."
  (unless (<= (terminal-history-start t) from to (next t)) (error "Invalid scrollback range" from to))
  (if (= from to) '#()
      (let* ((ring (history t)) (size (vector-length ring)) (i (modulo from size))
             (n (- to from)))
        (if (<= (+ i n) size) (vector-copy ring i (+ i n))
            (let* ((a (- size i)) (out (make-vector n)))
              (vector-copy! out 0 ring i size)
              (vector-copy! out a ring 0 (- n a))
              out)))))

(define (clip low n high) (max low (min n high)))
(define (move t x y lo hi)
  (let ((s (active t)))
    (sy-set! s (clip lo y hi))
    (sx-set! s (clip 0 x (line-right t (sy s))))
    (wrap-set! s #f)))
(define (address t x y)
  (let* ((origin (terminal-mode? t 'origin)) (s (active t))
         (lo (if origin (top s) 0)) (hi (if origin (bottom s) (- (terminal-rows t) 1))))
    (move t (if origin (+ x (margin-left t)) x) (+ y lo) lo hi)
    (when origin (sx-set! s (clip (min (margin-left t) (margin-right t (sy s))) (sx s) (margin-right t (sy s)))))))
(define (relative t dx dy)
  (let* ((s (active t)) (x (sx s)) (y (sy s))
         (within (or (terminal-mode? t 'origin) (<= (top s) y (bottom s))))
         (inside (inside-horizontal? t)))
    (move t (+ x dx) (+ y dy) (if within (top s) 0)
          (if within (bottom s) (- (terminal-rows t) 1)))
    (when (and (> dx 0) inside) (sx-set! s (min (sx s) (margin-right t (sy s)))))))

(define foreground-styles (list->vector (map (lambda (i) (list (cons 'fg i))) (iota 256))))
(define background-styles (list->vector (map (lambda (i) (list (cons 'bg i))) (iota 256))))

(define (put-style! t key value)
  (let ((attrs (style t)))
    (if (or (null? attrs) (and (null? (cdr attrs)) (eq? key (caar attrs))))
        (style-set! t (cond ((not value) '())
                            ((and (eq? key 'fg) (integer? value)) (vector-ref foreground-styles value))
                            ((and (eq? key 'bg) (integer? value)) (vector-ref background-styles value))
                            (else (list (cons key value)))))
        (let ((rest (filter (lambda (p) (not (eq? (car p) key))) attrs)))
          (style-set! t (if value (cons (cons key value) rest) rest))))))
(define (blank t)
  (let ((bg (assq (if (assq 'inverse (style t)) 'fg 'bg) (style t))))
    (and bg (cons " " (list (cons 'bg (cdr bg)))))))

(define (scroll! t lo hi amount save?)
  (if (terminal-mode? t 'horizontal-margins)
      (scroll-rectangle! t lo hi amount)
      (begin
        (scroll-lines! t lo hi amount save?)
        ;; A different line rendition can move under a stationary cursor.
        (let* ((s (active t)) (edge (line-right t (sy s))))
          (when (> (sx s) edge) (sx-set! s edge) (wrap-set! s #f))))))

(define (scroll-lines! t lo hi amount save?)
  (let* ((v (lines (active t))) (n (min (abs amount) (+ 1 (- hi lo))))
         (cols (terminal-columns t)) (fill (blank t)))
    (if (positive? amount)
        (begin
          (when (and save? (not (terminal-alternate? t)) (= lo 0))
            (do ((i lo (+ i 1))) ((= i (+ lo n))) (keep-row! t (vector-ref v i))))
          (vector-copy! v lo v (+ lo n) (+ hi 1))
          (do ((i (+ 1 (- hi n)) (+ i 1))) ((> i hi)) (vector-set! v i (fresh-row cols fill))))
        (begin
          (vector-copy! v (+ lo n) v lo (+ 1 (- hi n)))
          (do ((i lo (+ i 1))) ((= i (+ lo n))) (vector-set! v i (fresh-row cols fill)))))))

(define (index! t direction)
  (let* ((s (active t)) (y (sy s)))
    (wrap-set! s #f)
    (if (= y (if (> direction 0) (bottom s) (top s)))
        (when (inside-horizontal? t) (scroll! t (top s) (bottom s) direction #t))
        (sy-set! s (clip 0 (+ y direction) (- (terminal-rows t) 1))))
    (sx-set! s (min (sx s) (line-right t (sy s))))))

(define (protected? cell protection)
  (and (pair? cell)
       (or (and (memq protection '(iso both)) (assq 'iso-protect (cdr cell)))
           (and (memq protection '(dec both)) (assq 'dec-protect (cdr cell))))))

(define (screen-has-protection? t protection)
  (let ((v (lines (active t))))
    (let find-row ((y 0))
      (and (< y (vector-length v))
           (let* ((cells (terminal-row-cells (vector-ref v y))) (n (vector-length cells)))
             (let find-cell ((x 0))
               (cond ((= x n) (find-row (+ y 1)))
                     ((protected? (vector-ref cells x) protection) #t)
                     (else (find-cell (+ x 1))))))))))

(define initialized-blank (cons " " '()))
(define (erase! t y from to #:initialized [initialized #f] #:protection [protection #f])
  (let* ((r (terminal-row t y)) (v (terminal-row-cells r))
         (fill (or (blank t) (and (eq? initialized #t) initialized-blank)))
         (a (if (and (< from (vector-length v)) (eq? (vector-ref v from) 'tail)) (- from 1) from))
         (b (if (and (< to (vector-length v)) (eq? (vector-ref v to) 'tail)) (+ to 1) to)))
    (terminal-row-cache-set! r #f)
    (let loop ((i a))
      (when (< i b)
        (let ((wide (and (< (+ i 1) b) (eq? (vector-ref v (+ i 1)) 'tail))))
          (unless (protected? (vector-ref v i) protection)
            (let ((fill (or fill (and (eq? initialized 'preserve) (pair? (vector-ref v i)) initialized-blank))))
              (vector-set! v i fill)
              (when wide (vector-set! v (+ i 1) fill))))
          (loop (+ i (if wide 2 1))))))))

(define (save-cursor! t)
  (let ((s (active t)))
    (saved-set! s (list (sx s) (sy s) (wrap? s) (style t) (g0 t) (g1 t) (shifted? t)
                       (terminal-mode? t 'origin) (terminal-mode? t 'wrap)))))
(define (restore-cursor! t)
  (let* ((s (active t)) (p (saved s)))
    (if (not p) (move t 0 0 0 (- (terminal-rows t) 1))
        (begin
          (move t (list-ref p 0) (list-ref p 1) 0 (- (terminal-rows t) 1))
          (wrap-set! s (and (list-ref p 2) (= (sx s) (- (terminal-columns t) 1))))
          (style-set! t (list-ref p 3)) (g0-set! t (list-ref p 4)) (g1-set! t (list-ref p 5))
          (shifted-set! t (list-ref p 6))
          (mode-set! t 'origin (list-ref p 7)) (mode-set! t 'wrap (list-ref p 8))))))

(define (alternate! t n on)
  (unless (eq? (terminal-alternate? t) on)
    (when (and on (= n 1049)) (save-cursor! t))
    (when (or (and on (= n 1049)) (and (not on) (= n 1047)))
      (alt-set! t (fresh-screen (terminal-rows t) (terminal-columns t))))
    (alternate-set! t on)
    (when (and (not on) (= n 1049)) (restore-cursor! t))))

(define (terminal-reset! t)
  "Reset terminal T, clearing its screens, history and parser state."
  (alternate-set! t #f)
  (when (terminal-mode? t 'wide-columns) (child-resize! t (terminal-rows t) 80))
  (main-set! t (fresh-screen (terminal-rows t) (terminal-columns t)))
  (alt-set! t (fresh-screen (terminal-rows t) (terminal-columns t)))
  (clear-history! t)
  ((terminal-event t) 'reset-graphics #f)
  (link-set! t #f)
  (ever-protected-set! t #f)
  (modes-set! t '()) (soft-reset! t)
  (level-set! t 65) (page-length-set! t #f) (cursor-style-set! t 2)
  (tabs-set! t (default-tabs (terminal-columns t)))
  (title-set! t "") (parser-state-set! t 'ground) (header-set! t ""))

;; Preserve the cursor and as many rows around it as fit. Rows pushed off
;; the top of the main screen become history.
(define (terminal-resize! t rows cols)
  "Resize terminal T to ROWS and COLS, optionally rewrapping saved lines."
  (let ((rows (max 1 rows)) (cols (max 1 cols)))
    (unless (and (= rows (terminal-rows t)) (= cols (terminal-columns t)))
      (let ((old-tabs (tabs t)) (new-tabs (default-tabs cols)))
        (do ((i 0 (+ i 1))) ((= i (min cols (vector-length old-tabs))))
          (vector-set! new-tabs i (vector-ref old-tabs i)))
        (tabs-set! t new-tabs))
      (for-each
       (lambda (s)
         (let* ((old (lines s)) (v (make-vector rows)) (skip (max 0 (- (+ (sy s) 1) rows))))
           (when (eq? s (main t))
             (do ((i 0 (+ i 1))) ((= i skip)) (keep-row! t (vector-ref old i))))
           (do ((i 0 (+ i 1))) ((= i rows))
             (let ((j (+ i skip)))
               (vector-set! v i
                 (if (>= j (vector-length old)) (fresh-row cols #f)
                     (let* ((r (vector-ref old j)) (cells (terminal-row-cells r)) (n (vector-length cells)))
                       (if (= cols n) r
                           (let* ((copy (make-vector cols #f))
                                   (limit (if (terminal-row-double-width? r) (max 1 (quotient cols 2)) cols)))
                             (vector-copy! copy 0 cells 0 (min limit n))
                             (when (and (< limit n) (eq? (vector-ref cells limit) 'tail))
                               (vector-set! copy (- limit 1) #f))
                             (make-row cols copy #f (rendition r) (barrier? r) (terminal-row-prompt? r) #f))))))))
           (lines-set! s v) (sy-set! s (- (sy s) skip))
            (let* ((r (vector-ref v (sy s))) (limit (if (terminal-row-double-width? r) (max 1 (quotient cols 2)) cols)))
              (sx-set! s (min (sx s) (- limit 1))))
           (when (saved s)
             (let ((p (saved s)))
               (saved-set! s (cons (min (car p) (- cols 1))
                                   (cons (clip 0 (- (cadr p) skip) (- rows 1)) (cddr p))))))
           (top-set! s 0) (bottom-set! s (- rows 1))
            (left-set! s 0) (right-set! s (- cols 1)) (wrap-set! s #f)))
       (list (main t) (alt t)))
      (when (and (reflow-history? t) (not (= cols (terminal-columns t))))
         (reflow-scrollback! t cols))
       (rows-set! t rows) (columns-set! t cols))))

;; Common zero-width and wide ranges (ambiguous characters are narrow).
;; Limiting the cluster length also bounds a stream of combining marks.
(define (cell-width c)
  (let ((n (char->integer c)))
    (cond ((< n #x300) 1)
          ((or (<= #x300 n #x36f) (<= #x483 n #x489) (<= #x591 n #x5bd) (= n #x5bf)
               (<= #x5c1 n #x5c2) (<= #x610 n #x61a) (<= #x64b n #x65f) (= n #x670)
               (<= #x6d6 n #x6ed) (<= #x1ab0 n #x1aff) (<= #x1dc0 n #x1dff)
               (<= #x200b n #x200f) (<= #x202a n #x202e) (<= #x2060 n #x206f)
               (<= #x20d0 n #x20ff) (<= #xfe00 n #xfe0f) (<= #xfe20 n #xfe2f)
               (= n #xfeff) (<= #x1f3fb n #x1f3ff) (<= #xe0100 n #xe01ef)) 0)
          ((or (<= #x1100 n #x115f) (= n #x2329) (= n #x232a)
               (<= #x2e80 n #xa4cf) (<= #xac00 n #xd7a3) (<= #xf900 n #xfaff)
               (<= #xfe10 n #xfe19) (<= #xfe30 n #xfe6f) (<= #xff01 n #xff60)
               (<= #xffe0 n #xffe6) (<= #x1f300 n #x1faff) (<= #x20000 n #x3fffd)) 2)
          (else 1))))

(define (shift-cells! t amount)
  (let* ((x (terminal-x t)) (r (terminal-row t (terminal-y t))) (v (terminal-row-cells r))
         (end (+ 1 (margin-right t (terminal-y t)))) (n (min (abs amount) (- end x))) (fill (blank t)))
    (when (inside-horizontal? t)
      (sever-wide! v x) (sever-wide! v end)
      (if (> amount 0)
          (begin (sever-wide! v (- end n)) (vector-copy! v (+ x n) v x (- end n))
                 (vector-fill! v (or fill initialized-blank) x (+ x n)))
          (begin (sever-wide! v (+ x n)) (vector-copy! v x v (+ x n) end)
                 (vector-fill! v fill (- end n) end)))
      (terminal-row-cache-set! r #f))))

(define (graphic! t c)
  (let* ((s (active t)) (w (cell-width c)))
    (if (= w 0)
        (let* ((r (terminal-row t (sy s))) (v (terminal-row-cells r))
               (x (if (wrap? s) (sx s) (- (sx s) 1)))
               (i (if (and (>= x 0) (eq? (vector-ref v x) 'tail)) (- x 1) x))
               (old (and (>= i 0) (vector-ref v i))))
          (when (and (pair? old) (< (string-length (car old)) 16))
            (vector-set! v i (cons (string-append (car old) (string c)) (cdr old)))
            (terminal-row-cache-set! r #f)))
        (let* ((end (+ 1 (print-right t))) (wrapping (terminal-mode? t 'wrap)))
          (when (and wrapping (or (wrap? s) (and (> (+ (sx s) w) end) (> (sx s) (margin-left t)))))
            (when (and (reflow-history? t) (not (terminal-mode? t 'horizontal-margins)))
               (continuation-set! (terminal-row t (sy s)) (if (wrap? s) end (sx s))))
             (index! t 1) (carriage-return! t))
          (let* ((end (+ 1 (print-right t))) (x (sx s)) (w (if (> (+ x w) end) 1 w))
                 (y (sy s)) (v (terminal-row-cells (terminal-row t y))))
            (when (terminal-mode? t 'insert) (shift-cells! t w))
            (erase! t y x (+ x w))
            (vector-set! v x (graphic-cell t (if (< w (cell-width c)) #\xfffd c)))
            (when (= w 2) (vector-set! v (+ x 1) 'tail))
            (sx-set! s (min (- end 1) (+ x w)))
            (wrap-set! s (and wrapping (= (+ x w) end))))))))
