;;; Rich text (EDITOR.md, sections 2 and 3): documents of headings,
;;; paragraphs, lists, quotes, code and tables, shown as a presentation.
;;; Formats (markdown.scm) parse into the tree described here; rendering,
;;; faces, links and code colouring are written once, for all of them.
;;;
;;; The tree is data. A block is one of
;;;
;;;   (heading LEVEL inline ...)     LEVEL from 1
;;;   (paragraph inline ...)
;;;   (code-block LANGUAGE line ...) LANGUAGE a name a fence gives, or #f;
;;;                                  each line a span or a string
;;;   (quote block ...)
;;;   (list item ...)                each (item MARKER CHECK block ...),
;;;                                  MARKER #f for a bullet or the text
;;;                                  numbering it, CHECK #f, open or done
;;;   (table ALIGNS HEADER ROW ...)  ALIGNS left, center or right per
;;;                                  column; HEADER and each ROW a list
;;;                                  of cells, each a list of inlines
;;;   (rule)
;;;
;;; and an inline one of
;;;
;;;   "text"                         generated text
;;;   (span DOCUMENT FROM TO)        text of a document, on one line:
;;;                                  editing the rendering edits it there
;;;   (break)                        a line break inside a paragraph,
;;;                                  shown as a space: the text flows on
;;;   (hard-break)                   a line break kept: what follows
;;;                                  starts a row of its own
;;;   (strong inline ...)  (emph inline ...)  (strike inline ...)
;;;   (code inline ...)
;;;   (link TARGET inline ...)       TARGET what RET acts on (`target`)
;;;   (image TARGET inline ...)      the inlines its description
;;;
;;; `rich-present!' shows a tree in a buffer in `rich-mode' (or one
;;; extending it): RET follows the link at point, elsewhere goes to the
;;; source of the text there. How it looks is the faces': the `rich.'
;;; ones, headings in sizes of their own and prose in a variable-pitch
;;; font, code and tables in the fixed one, for a frontend that has
;;; them; a terminal draws the weights and colours.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "faces.scm")
(require "commands.scm")
(require "targets.scm")
(require "buffers.scm")
(require "views.scm")
(require "syntax.scm")
(require "remote.scm")

(provide rich-present! make-rich-buffer! rich-code-language)

;;; Faces

(define-face rich "Rich text.")
(define-face rich.text "Prose in rich text." #:family 'variable)
(define-face rich.heading "A heading in rich text." #:inherit 'markup.heading #:family 'variable)
(define-face rich.heading.1 "A first-level heading." #:height 1.6 #:fg 'mauve)
(define-face rich.heading.2 "A second-level heading." #:height 1.35 #:fg 'blue)
(define-face rich.heading.3 "A third-level heading." #:height 1.15 #:fg 'sky)
(define-face rich.heading.4 "A heading of the fourth level or below." #:fg 'teal)
(define-face rich.heading-rule "The line under a first- or second-level heading, across the pane." #:underline '(line surface) #:extend #t)
(define-face rich.strong "Strong text." #:inherit 'rich.text #:weight 'bold)
(define-face rich.emphasis "Emphasised text." #:inherit 'rich.text #:slant 'italic)
(define-face rich.strong-emphasis "Strong and emphasised text." #:inherit 'rich.text #:weight 'bold #:slant 'italic)
(define-face rich.strike "Struck-through text." #:inherit 'rich.text #:strike #t #:fg 'overlay)
(define-face rich.code "Code in a line of text." #:family 'fixed #:fg 'peach #:bg 'surface0)
(define-face rich.code-block "A code block's background." #:bg 'mantle #:extend #t)
(define-face rich.link "A link." #:inherit 'rich.text #:fg 'blue #:underline 'line)
(define-face rich.image "An image, as its description." #:inherit 'rich.text #:fg 'teal)
(define-face rich.bullet "A list item's bullet or number." #:fg 'peach)
(define-face rich.task-done "A done task's box." #:fg 'green)
(define-face rich.quote "Quoted text." #:inherit 'rich.text #:fg 'subtext #:slant 'italic)
(define-face rich.quote-bar "The bar beside quoted text." #:fg 'surface)
(define-face rich.rule "A thematic break: a line across the pane." #:underline '(line surface) #:extend #t)
(define-face rich.table "A table's lines." #:fg 'surface)
(define-face rich.table.cell "A table's cell: fixed-pitch, so the columns line up." #:inherit #f #:family 'fixed)
(define-face rich.table.header "A table's header cells." #:inherit #f #:family 'fixed #:weight 'bold)

;;; Rendering: blocks into rows of runs. A run is #(text face document
;;; from to target picture): TEXT generated when DOCUMENT is #f, else an
;;; excerpt FROM to TO of it; TARGET what a link's runs stand for, or #f;
;;; PICTURE the file an image's runs show, or #f. A row is (kind
;;; prefix-runs . runs): KIND text, or the face the whole row has after
;;; its prefix (a code block's background, a heading's rule), which the
;;; mode's layer gives it.

(define (gen text face) (vector text face #f 0 0 #f #f))
(define (run-text r) (vector-ref r 0))
(define (run-bytes r) (if (vector-ref r 2) (- (vector-ref r 4) (vector-ref r 3)) (utf8-length (run-text r))))
(define (run-width r)
  (if (vector-ref r 2)
      (string-length (document-substring (vector-ref r 2) (vector-ref r 3) (vector-ref r 4)))
      (string-length (run-text r))))
(define (runs-width runs) (fold (lambda (r n) (+ n (run-width r))) 0 runs))
(define (utf8-length s) (bytevector-length (string->utf8 s)))
(define (with-target r t)
  (vector (vector-ref r 0) (vector-ref r 1) (vector-ref r 2) (vector-ref r 3) (vector-ref r 4) t (vector-ref r 6)))
(define (with-picture r p)
  (vector (vector-ref r 0) (vector-ref r 1) (vector-ref r 2) (vector-ref r 3) (vector-ref r 4) (vector-ref r 5) p))

;; The picture an image's target shows: a file of a format decoded.
(define picture-suffixes '(".png" ".jpg" ".jpeg" ".gif" ".webp" ".bmp" ".svg"))
(define (picture-of t)
  (and t (eq? (target-kind t) 'file)
       (let ((path (target-value t)))
         (and (any (lambda (s) (string-suffix? s (string-downcase path))) picture-suffixes) path))))
(define (spaces n) (make-string n #\space))
(define (cons* x . rest) (if (null? rest) x (cons x (apply cons* rest))))

(define (heading-face level)
  (case level ((1) 'rich.heading.1) ((2) 'rich.heading.2) ((3) 'rich.heading.3) (else 'rich.heading.4)))

;; Faces whose text keeps them whatever is inside: a heading's, a cell's.
(define (whole? face) (and face (memq face '(rich.heading.1 rich.heading.2 rich.heading.3 rich.heading.4 rich.table.cell rich.table.header))))

;; FACE with STYLE over it: a run has one face.
(define (styled face style)
  (cond ((eq? style 'code) 'rich.code)
        ((whole? face) face)
        ((eq? style 'strong) (if (memq face '(rich.emphasis rich.strong-emphasis)) 'rich.strong-emphasis 'rich.strong))
        ((eq? style 'emph) (if (memq face '(rich.strong rich.strong-emphasis)) 'rich.strong-emphasis 'rich.emphasis))
        ((eq? style 'strike) 'rich.strike)
        ((eq? style 'link) 'rich.link)
        (else face)))

(define (inline-runs xs face)
  (append-map
   (lambda (x)
     (cond ((string? x) (list (gen x face)))
           ;; An empty span too: what is typed there goes into it.
           ((eq? (car x) 'span) (list (vector "" face (cadr x) (caddr x) (cadddr x) #f #f)))
           ((eq? (car x) 'break) (list (gen " " face)))
           ;; Split on by the paragraph it is in (`rows-of`).
           ((eq? (car x) 'hard-break) (list 'hard-break))
           ((memq (car x) '(strong emph strike code)) (inline-runs (cdr x) (styled face (car x))))
           ((eq? (car x) 'link)
            (map (lambda (r) (with-target r (cadr x))) (inline-runs (cddr x) (styled face 'link))))
           ((eq? (car x) 'image)
            (map (lambda (r) (with-picture (with-target r (cadr x)) (picture-of (cadr x))))
                 (cons (gen "▣ " 'rich.image) (inline-runs (cddr x) (if (whole? face) face 'rich.image)))))
           (else '())))
   xs))

;; RUNS cut at their hard breaks: a list of rows' runs.
(define (rows-of runs)
  (let loop ((rs runs) (cur '()) (acc '()))
    (cond ((null? rs) (reverse (cons (reverse cur) acc)))
          ((eq? (car rs) 'hard-break) (loop (cdr rs) '() (cons (reverse cur) acc)))
          (else (loop (cdr rs) (cons (car rs) cur) acc)))))

;; RUNS on one row: a hard break a space.
(define (one-row runs face)
  (map (lambda (r) (if (eq? r 'hard-break) (gen " " face) r)) runs))

(define bullets '#("•" "◦" "▪"))


;; The rows of BLOCKS: the first row after FIRST, the others after REST
;; (prefix runs), in FACE. GAP puts an empty row between blocks.
(define (render-blocks blocks first rest face depth gap)
  (let loop ((bs blocks) (prefix first) (acc '()))
    (if (null? bs)
        (reverse acc)
        (let ((acc (append (reverse (render-block (car bs) prefix rest face depth)) acc)))
          (loop (cdr bs) rest (if (and gap (pair? (cdr bs))) (cons (list 'text rest) acc) acc))))))

(define (render-block b first rest face depth)
  (case (car b)
    ((paragraph)
     (let ((rows (rows-of (inline-runs (cdr b) face))))
       (cons (cons* 'text first (car rows)) (map (lambda (r) (cons* 'text rest r)) (cdr rows)))))
    ((heading)
     (let* ((level (cadr b)) (hf (heading-face level)) (runs (one-row (inline-runs (cddr b) hf) hf)))
       (list (cons* (if (<= level 2) 'rich.heading-rule 'text) first runs))))
    ((rule) (list (cons* 'rich.rule first '())))
    ((code-block) (code-rows (cadr b) (cddr b) first rest))
    ((quote)
     (let ((bar (list (gen "┃ " 'rich.quote-bar))))
       (render-blocks (cdr b) (append first bar) (append rest bar) 'rich.quote depth #t)))
    ((list)
     (let loop ((items (cdr b)) (prefix first) (acc '()))
       (if (null? items)
           (reverse acc)
           (let* ((it (car items))
                  (mark (case (caddr it)
                          ((open) (gen "☐ " 'rich.bullet))
                          ((done) (gen "☑ " 'rich.task-done))
                          (else (gen (string-append (or (cadr it) (vector-ref bullets (modulo depth 3))) " ") 'rich.bullet))))
                  (indent (list (gen (spaces (string-length (run-text mark))) #f)))
                  (blocks (if (null? (cdddr it)) (list (list 'paragraph)) (cdddr it)))
                  (rows (render-blocks blocks (append prefix (list mark)) (append rest indent) face (+ depth 1) #f)))
             (loop (cdr items) rest (append (reverse rows) acc))))))
    ((table) (table-rows (cadr b) (caddr b) (cdddr b) first rest))
    (else '())))

;;; Code blocks, coloured in their language.

(define language-aliases
  '(("sh" . bash) ("shell" . bash) ("console" . bash) ("zsh" . bash) ("js" . javascript) ("ts" . typescript)
    ("py" . python) ("rs" . rust) ("scm" . scheme) ("lisp" . scheme) ("elisp" . scheme) ("emacs-lisp" . scheme)
    ("racket" . scheme) ("yml" . yaml) ("md" . markdown)))

(define (rich-code-language name)
  "Return the syntax language a code block naming NAME is in, or #f.
NAME is what the block says, a fence's info string: its first word, a
language's name, an alias such as sh, or a file suffix."
  (let* ((name (if (symbol? name) (symbol->string name) (or name "")))
         (end (let loop ((i 0)) (if (and (< i (string-length name)) (not (memv (string-ref name i) '(#\space #\tab #\{)))) (loop (+ i 1)) i)))
         (word (string-downcase (substring name 0 end)))
         (alias (assoc word language-aliases)))
    (cond ((string=? word "") #f)
          (alias (cdr alias))
          ((find-language (string->symbol word)) (string->symbol word))
          (else (path-language (string-append "x." word))))))

(define (line-text l) (if (string? l) l (document-substring (cadr l) (caddr l) (cadddr l))))

;; The faces of the code: (face start end) in bytes of its lines joined.
;; Kept by language and text, so rendering again colours only the code
;; that changed, until a language changes; the table is emptied when it
;; grows past a few hundred.
(define %code-faces (make-hash-table))
(define (code-faces language lines)
  (let ((lang (rich-code-language language)))
    (if (not lang)
        '()
        (let* ((text (string-join (map line-text lines) "\n"))
               (key (string-append (number->string (language-generation)) "\x0;" (symbol->string lang) "\x0;" text)))
          (or (hash-table-ref/default %code-faces key #f)
              (let ((faces (guard (e (#t '())) (syntax-highlights (make-document text) lang))))
                (when (> (hash-table-size %code-faces) 500) (set! %code-faces (make-hash-table)))
                (hash-table-set! %code-faces key faces)
                faces))))))

;; Line L's runs, its text starting at OFFSET of the code, coloured by FACES.
(define (painted-runs l offset faces)
  (let* ((text (line-text l)) (len (utf8-length text)) (paint (make-vector len #f))
         ;; A span is edited through; a string is generated, in one run.
         (piece (lambda (from to face)
                  (if (string? l)
                      (vector (utf8-substring text from to) face #f 0 0 #f #f)
                      (vector "" face (cadr l) (+ (caddr l) from) (+ (caddr l) to) #f #f)))))
    (for-each (lambda (f)
                (let ((a (max (cadr f) offset)) (b (min (caddr f) (+ offset len))))
                  (do ((i a (+ i 1))) ((>= i b)) (vector-set! paint (- i offset) (car f)))))
              faces)
    (let loop ((i 0) (from 0) (acc '()))
      (cond ((and (= len 0) (not (string? l))) (list (piece 0 0 #f)))
            ((= i len) (reverse (if (< from i) (cons (piece from i (vector-ref paint from)) acc) acc)))
            ((eq? (vector-ref paint i) (vector-ref paint from)) (loop (+ i 1) from acc))
            (else (loop (+ i 1) i (cons (piece from i (vector-ref paint from)) acc)))))))

(define (utf8-substring s from to)
  (utf8->string (bytevector-copy (string->utf8 s) from to)))

(define (code-rows language lines first rest)
  (let ((faces (code-faces language lines))
        (pad (list (gen " " #f)))
        (lines (if (null? lines) (list "") lines)))
    (let loop ((ls lines) (offset 0) (prefix first) (acc '()))
      (if (null? ls)
          (reverse acc)
          (loop (cdr ls) (+ offset (utf8-length (line-text (car ls))) 1) rest
                (cons (cons* 'rich.code-block prefix (append pad (painted-runs (car ls) offset faces))) acc))))))

;;; Tables, drawn with lines, in the fixed-pitch font so they line up.

(define (table-rows aligns header body first rest)
  (let* ((cells (lambda (row f) (map (lambda (c) (one-row (inline-runs c f) f)) row)))
         (head (cells header 'rich.table.header))
         (rows (map (lambda (r) (cells r 'rich.table.cell)) body))
         (widths (fold (lambda (r ws) (map (lambda (c w) (max w (runs-width c))) r ws)) (map (lambda (_) 1) aligns) (cons head rows)))
         (line-of (lambda (l m r) (gen (string-append l (string-join (map (lambda (w) (make-string (+ w 2) #\─)) widths) m) r) 'rich.table)))
         (bar (lambda (s) (gen s 'rich.table)))
         (cells-row (lambda (r)
                      (cons (bar "│")
                            (apply append
                                   (map (lambda (c w a)
                                          (let* ((room (- w (runs-width c)))
                                                 (left (case a ((right) room) ((center) (quotient room 2)) (else 0))))
                                            (append (list (gen (spaces (+ 1 left)) 'rich.table.cell)) c
                                                    (list (gen (spaces (+ 1 (- room left))) 'rich.table.cell) (bar "│")))))
                                        r widths aligns))))))
    (append (list (cons* 'text first (list (line-of "┌" "┬" "┐")))
                  (cons* 'text rest (cells-row head))
                  (cons* 'text rest (list (line-of "├" "┼" "┤"))))
            (map (lambda (r) (cons* 'text rest (cells-row r))) rows)
            (list (cons* 'text rest (list (line-of "└" "┴" "┘")))))))

;;; Buffers of rich text. Their state is a table, plain data, keyed by
;;; row: 'links, each row's (from to target) in bytes of the row;
;;; 'row-faces, (bytes-of-prefix face) of a row with a face of its own;
;;; 'hangs, the bytes of the prefix a row hangs under; 'pictures, the
;;; files a row's images show; 'next-key, the next key never given. What
;;; the buffer's maker keeps beside is its own.

(define (run->spec r)
  (if (vector-ref r 2)
      (list (vector-ref r 2) (vector-ref r 3) (vector-ref r 4) (vector-ref r 1))
      (list (run-text r) (vector-ref r 1))))

;; The keys of ROWS, rendered again for presentation P: a row showing a
;; place of a document an old row shows now (its first excerpt where the
;; old one's is, followed through edits) takes its key, so a caret on it
;; stays on it; rows of generated text are keyed after the row before
;; them; any other row has a key never given before.
(define (row-keys! p st rows)
  (let ((old (make-hash-table)) (used (make-hash-table)))
    (for-each (lambda (x) (let ((place (string-append (number->string (cadr x)) ":" (number->string (caddr x)))))
                            (unless (hash-table-ref/default old place #f) (hash-table-set! old place (car x)))))
              (presentation-row-sources p))
    (let loop ((rs rows) (before "^") (n 0) (acc '()))
      (if (null? rs)
          (reverse acc)
          (let* ((source (find (lambda (r) (vector-ref r 2)) (append (cadr (car rs)) (cddr (car rs)))))
                 (place (and source (string-append (number->string (document-id (vector-ref source 2))) ":"
                                                   (number->string (vector-ref source 3)))))
                 (kept (and place (hash-table-ref/default old place #f)))
                 (key (cond ((and kept (not (hash-table-ref/default used kept #f))) kept)
                            (place (let ((k (hash-table-ref/default st 'next-key 0)))
                                     (hash-table-set! st 'next-key (+ k 1))
                                     (string-append "k" (number->string k))))
                            (else (string-append before "+" (number->string (+ n 1)))))))
            (hash-table-set! used key #t)
            (loop (cdr rs) (if place key before) (if place 0 (+ n 1)) (cons key acc)))))))

(define (rich-present! b blocks)
  "Show the rich text BLOCKS in buffer B, a buffer of `make-rich-buffer!'.
Only rows that differ change: a row showing the same place of its
document as before keeps its key, so a caret on it stays. Pictures load
in the background when their rows are shown, and show when they have."
  (let* ((st (buffer-state b)) (p (buffer-document b))
         ;; An empty row last: every row of text has a line break after
         ;; it, which an extending face covers.
         (rows (append (render-blocks blocks '() '() 'rich.text 0 #t) (list (list 'text '()))))
         (keys (row-keys! p st rows))
         (links (make-hash-table)) (faces (make-hash-table)) (hangs (make-hash-table)) (pictures (make-hash-table)))
    (for-each
     (lambda (r key)
       (let ((runs (append (cadr r) (cddr r)))
             (prefix (fold (lambda (r n) (+ n (run-bytes r))) 0 (cadr r)))
             (shown (delete-duplicates (filter-map (lambda (r) (vector-ref r 6)) (cddr r)))))
         ;; Where each link's runs are in the row.
         (let walk ((rs runs) (at 0) (acc '()))
           (if (null? rs)
               (unless (null? acc) (hash-table-set! links key (reverse acc)))
               (let* ((x (car rs)) (end (+ at (run-bytes x))) (t (vector-ref x 5)))
                 (walk (cdr rs) end
                       (cond ((not t) acc)
                             ((and (pair? acc) (eq? (caddr (car acc)) t) (= (cadr (car acc)) at))
                              (cons (list (car (car acc)) end t) (cdr acc)))
                             (else (cons (list at end t) acc)))))))
         (unless (eq? (car r) 'text) (hash-table-set! faces key (list prefix (car r))))
         (when (and (> prefix 0) (eq? (car r) 'text)) (hash-table-set! hangs key prefix))
         (unless (null? shown) (hash-table-set! pictures key shown))))
     rows keys)
    (present! p (map (lambda (r key) (row (map run->spec (append (cadr r) (cddr r))) #:key key)) rows keys))
    (hash-table-set! st 'links links)
    (hash-table-set! st 'row-faces faces)
    (hash-table-set! st 'hangs hangs)
    (hash-table-set! st 'pictures pictures)
    (set! %failed (make-hash-table))))

;; Pictures loading, and those that failed to (not tried again until
;; something is rendered anew): by path.
(define %loading (make-hash-table))
(define %failed (make-hash-table))

;; The picture at PATH if it is loaded; else #f, and it starts loading in
;; a task of its own, once: then the projections show it. One evicted
;; from the images kept loads again when it is shown again.
(define (picture path)
  (or (image-loaded path)
      (begin
        (unless (or (hash-table-ref/default %loading path #f) (hash-table-ref/default %failed path #f))
          (hash-table-set! %loading path #t)
          (spawn (lambda ()
                   (guard (e (#t (hash-table-set! %failed path #t))) (image-load path))
                   (hash-table-delete! %loading path))))
        #f)))

(define (make-rich-buffer! name #:mode [mode 'rich-mode] #:state [state (make-hash-table)])
  "Return a new buffer NAME for rich text, in MODE.
MODE is `rich-mode' or one extending it. STATE is a table the buffer
keeps; `rich-present!' adds its own entries."
  (make-generated-buffer! name (make-presentation) mode #:state state))

;; The state of the buffer of presentation P, and the rows from FROM to
;; TO there (key start end): what is shown, not every row.
(define (shown-rows p from to)
  (let* ((b (document-buffer p)) (st (and b (buffer-state b))))
    (if (hash-table? st) (values st (presentation-rows-between p from to)) (values #f '()))))

;; The rows with a face of their own have it after their prefix and
;; through their line break, so a background or an underline reaches the
;; pane's edge.
(define (row-faces p from to)
  (call-with-values (lambda () (shown-rows p from to))
    (lambda (st rows)
      (let ((len (document-length p)))
        (filter-map (lambda (r)
                      (let ((f (hash-table-ref/default (hash-table-ref/default st 'row-faces (make-hash-table)) (car r) #f)))
                        (and f (list (+ (cadr r) (car f)) (min len (+ (caddr r) 1)) (cadr f)))))
                    rows)))))

;; Rows under bullets and bars hang under their text; pictures show
;; above their rows once loaded, and start loading when shown.
(define (rich-projections p from to)
  (call-with-values (lambda () (shown-rows p from to))
    (lambda (st rows)
      (let ((hangs (and st (hash-table-ref/default st 'hangs #f))) (pictures (and st (hash-table-ref/default st 'pictures #f))))
        (append-map (lambda (r)
                      (let ((hang (and hangs (hash-table-ref/default hangs (car r) #f)))
                            (paths (if pictures (hash-table-ref/default pictures (car r) '()) '())))
                        (append (if hang (list (list 'hang (cadr r) (+ (cadr r) hang))) '())
                                (filter-map (lambda (path) (let ((image (picture path))) (and image (list 'block (cadr r) image))))
                                            paths))))
                    rows)))))

;; The link at POS, else the source of the text there.
(define (rich-target b pos)
  (let* ((st (buffer-state b)) (p (buffer-document b)) (row (and (hash-table? st) (find (lambda (r) (<= (cadr r) pos (caddr r))) (presentation-rows-between p pos pos))))
         (key (and row (car row))))
    (and (hash-table? st)
         (or (and key
                  (let* ((start (cadr row)) (rel (- pos start))
                         (link (find (lambda (l) (and (<= (car l) rel) (< rel (cadr l)))) (hash-table-ref/default (hash-table-ref st 'links) key '()))))
                    (and link (caddr link))))
             (let ((at (presentation-source-at p pos)))
               (and at (target 'location (location (car at) (cadr at)))))))))

(define-mode rich-mode
  "Rich text: headings, lists, code and tables, without markup.
Where the text is a document's, editing it edits the document.
\\[act-default-at-point] follows the link at point, else goes to the
source of the text there."
  #:keys '(("RET" act-default-at-point) ("C-c C-o" act-default-at-point))
  #:normal '(("RET" act-default-at-point))
  #:layer row-faces
  #:projections rich-projections
  #:target-at rich-target)

;;; Addresses

(define-action url (browse-url s u)
  "Open the address in the web browser."
  (spawn (lambda () (call-with-process-at (default-directory s) "xdg-open" (list u) (lambda (p) (process-wait p))))))

(define-action url (copy-url s u) "Save the address as a kill." (kill-save! s u #f #f))
